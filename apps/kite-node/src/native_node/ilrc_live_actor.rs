//! Incremental ILRC Nautilus strategy actor. Real orders require explicit runtime policy.
//! Entry orders are submitted only on an admitted completed bar; positions use a
//! broker-observed stop before any target or break-even management.
use super::{
    ilrc_backtest::{self, EntryEvent},
    ilrc_config::Selection,
    ilrc_continuation_backtest, ilrc_live_dashboard,
    live_control::Control,
};
use anyhow::{Result, ensure};
use chrono::Timelike;
use kite_adapter::http::historical::Candle;
use nautilus_common::{actor::DataActor, timer::TimeEvent};
use nautilus_core::DurationNanos;
use nautilus_model::{
    data::{Bar, BarType, QuoteTick},
    enums::{OrderSide, TimeInForce},
    events::{OrderAccepted, OrderCanceled, OrderDenied, OrderFilled, OrderRejected, OrderUpdated},
    identifiers::ClientOrderId,
    orders::Order,
    types::{Price, Quantity},
};
use nautilus_trading::{
    nautilus_strategy,
    strategy::{Strategy, StrategyConfig, StrategyCore},
};
use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    },
};

#[derive(Clone, Debug)]
struct Protection {
    side: i8,
    entry: f64,
    stop: f64,
    target: f64,
    risk: f64,
    be_sent: bool,
}
#[derive(Debug)]
pub struct IlrcActor {
    core: StrategyCore,
    bar_type: BarType,
    selection: Selection,
    candles: Vec<Candle>,
    seen: BTreeSet<String>,
    start_ns: u64,
    last_close_ns: u64,
    pending_entry: Option<ClientOrderId>,
    pending_stop: Option<ClientOrderId>,
    accepted_stop: Option<ClientOrderId>,
    entry_signal: Option<EntryEvent>,
    protection: Option<Protection>,
    waiting_cancel_for_exit: bool,
    awaiting_exit: bool,
    waiting_modify: bool,
    market_price: Arc<AtomicI64>,
    control: Option<Control>,
    dashboard: Option<ilrc_live_dashboard::Shared>,
}
impl IlrcActor {
    pub fn new(
        bar_type: BarType,
        selection: Selection,
        start_ns: u64,
        market_price: Arc<AtomicI64>,
    ) -> Self {
        Self {
            core: StrategyCore::new(StrategyConfig {
                strategy_id: Some("ILRC-001".into()),
                log_events: false,
                log_commands: false,
                ..Default::default()
            }),
            bar_type,
            selection,
            candles: Vec::new(),
            seen: BTreeSet::new(),
            start_ns,
            last_close_ns: 0,
            pending_entry: None,
            pending_stop: None,
            accepted_stop: None,
            entry_signal: None,
            protection: None,
            waiting_cancel_for_exit: false,
            awaiting_exit: false,
            waiting_modify: false,
            market_price,
            control: None,
            dashboard: None,
        }
    }
    pub fn with_control(mut self, control: Control) -> Self {
        self.control = Some(control);
        self
    }
    pub fn with_dashboard(mut self, dashboard: ilrc_live_dashboard::Shared) -> Self {
        self.dashboard = Some(dashboard);
        self
    }
    fn dashboard_update(&self, event: &str) {
        if let Some(shared) = &self.dashboard
            && let Ok(mut d) = shared.lock()
        {
            d.position = self.position();
            d.stop = self
                .accepted_stop
                .and(self.protection.as_ref().map(|p| p.stop));
            d.target = self.protection.as_ref().map(|p| p.target);
            d.event = event.to_owned();
        }
    }
    fn position(&self) -> f64 {
        self.cache()
            .positions_open(
                None,
                Some(&self.bar_type.instrument_id()),
                self.strategy_id().as_ref(),
                None,
                None,
            )
            .iter()
            .map(|p| p.signed_qty)
            .sum()
    }
    fn fail_closed(&mut self, reason: &str) {
        eprintln!("ILRC EXECUTION HALTED: {reason}");
        self.awaiting_exit = true; // prevent further entries; broker exposure must be reconciled manually.
        if let Some(shared) = &self.dashboard
            && let Ok(mut d) = shared.lock()
        {
            d.fault = Some(reason.to_owned());
            d.event = "HALTED".into();
        }
    }
    fn signal(&self) -> Result<Option<EntryEvent>> {
        let selected = &self.selection;
        let mut candidates = ilrc_backtest::entry_events_config_candles(selected, &self.candles)?;
        candidates.extend(ilrc_continuation_backtest::entry_events_candles(
            &self.candles,
            selected.continuation.target_r,
        )?);
        let ts = self.last_close_ns as i64;
        let mut relevant = Vec::new();
        for e in candidates {
            let observed = chrono::DateTime::parse_from_rfc3339(&e.observed_at)?
                .timestamp_nanos_opt()
                .ok_or_else(|| anyhow::anyhow!("Timestamp overflow"))?;
            if observed == ts {
                relevant.push(e);
            }
        }
        relevant.sort_by_key(|x| x.setup); // A precedes B
        Ok(relevant.into_iter().next())
    }
    fn submit_exit(&mut self) -> Result<()> {
        let p = self.position();
        if p == 0.0 {
            self.waiting_cancel_for_exit = false;
            self.awaiting_exit = false;
            self.protection = None;
            self.dashboard_update("Exit filled");
            return Ok(());
        }
        ensure!(p.abs() == 1.0, "ILRC position exceeds one contract");
        let side = if p > 0.0 {
            OrderSide::Sell
        } else {
            OrderSide::Buy
        };
        let order = self.order().market(
            self.bar_type.instrument_id(),
            side,
            Quantity::from(1),
            Some(TimeInForce::Day),
            Some(true),
            Some(false),
            None,
            None,
            None,
            None,
        );
        self.awaiting_exit = true;
        self.submit_order(order, None, None, None)?;
        Ok(())
    }
    fn maybe_manage_position(&mut self, bar: &Bar) -> Result<()> {
        let Some(p) = self.protection.clone() else {
            return Ok(());
        };
        if self.accepted_stop.is_none()
            || self.waiting_cancel_for_exit
            || self.awaiting_exit
            || self.waiting_modify
        {
            return Ok(());
        }
        let now = chrono::DateTime::from_timestamp_nanos(self.last_close_ns as i64)
            .with_timezone(&chrono::FixedOffset::east_opt(19800).expect("IST"));
        let session_end = now.hour() * 60 + now.minute() >= self.selection.entry_cutoff_minute;
        let target = if p.side > 0 {
            bar.high.as_f64() >= p.target
        } else {
            bar.low.as_f64() <= p.target
        };
        if target || session_end {
            self.waiting_cancel_for_exit = true;
            self.cancel_order(self.accepted_stop.expect("stop order exists"), None, None)?;
            return Ok(());
        }
        if !p.be_sent {
            let one_r = if p.side > 0 {
                bar.high.as_f64() >= p.entry + p.risk
            } else {
                bar.low.as_f64() <= p.entry - p.risk
            };
            if one_r {
                ensure!(
                    p.entry.fract() == 0.0,
                    "Break-even trigger not aligned to verified contract tick"
                );
                self.waiting_modify = true;
                self.modify_order(
                    self.accepted_stop.expect("stop order exists"),
                    None,
                    None,
                    Some(Price::new(p.entry, 0)),
                    None,
                    None,
                )?;
            }
        }
        Ok(())
    }
    fn open_entry(&mut self, signal: EntryEvent) -> Result<()> {
        ensure!(
            self.position() == 0.0
                && self.pending_entry.is_none()
                && self.pending_stop.is_none()
                && self.accepted_stop.is_none()
                && !self.awaiting_exit
                && !self.waiting_cancel_for_exit,
            "ILRC cannot open overlapping positions"
        );
        let entry_side = if signal.side == "LONG" {
            OrderSide::Buy
        } else {
            OrderSide::Sell
        };
        ensure!(
            signal.stop.is_finite() && signal.stop > 0.0,
            "Invalid protective stop"
        );
        let order = self.order().market(
            self.bar_type.instrument_id(),
            entry_side,
            Quantity::from(1),
            Some(TimeInForce::Day),
            Some(false),
            Some(false),
            None,
            None,
            None,
            None,
        );
        let id = order.client_order_id();
        println!(
            "{}",
            serde_json::json!({"event":"ilrc_native_entry_intent","setup":signal.setup,"side":signal.side,"entry_signal_time":signal.entry_time,"protective_stop":signal.stop,"target":signal.target})
        );
        if let Some(control) = &self.control {
            control.flat.store(false, Ordering::Release);
        }
        if let Some(shared) = &self.dashboard
            && let Ok(mut d) = shared.lock()
        {
            d.setup = format!("{} {}", signal.setup, signal.side);
            d.event = "Entry order submitted".into();
        }
        self.entry_signal = Some(signal);
        self.pending_entry = Some(id);
        self.submit_order(order, None, None, None)?;
        Ok(())
    }
}
impl DataActor for IlrcActor {
    fn on_start(&mut self) -> Result<()> {
        self.subscribe_bars(
            self.bar_type,
            Some(
                if self.control.as_ref().is_some_and(|c| c.real) {
                    "KITE"
                } else {
                    "STBARS"
                }
                .into(),
            ),
            None,
        );
        self.subscribe_quotes(self.bar_type.instrument_id(), Some("KITE".into()), None);
        if self.control.is_some() {
            self.clock().set_timer_ns(
                "ilrc_exit_guard",
                DurationNanos::from_millis(250),
                None,
                None,
                None,
                None,
                None,
            )?;
        }
        Ok(())
    }
    fn on_time_event(&mut self, e: &TimeEvent) -> Result<()> {
        if e.name.as_str() != "ilrc_exit_guard" {
            return Ok(());
        }
        let Some(control) = &self.control else {
            return Ok(());
        };
        let stopping = control.stopping.load(Ordering::Acquire);
        let today =
            chrono::Utc::now().with_timezone(&chrono::FixedOffset::east_opt(19800).expect("IST"));
        let cutoff = !control.sim
            && today.hour() * 60 + today.minute() >= self.selection.entry_cutoff_minute;
        if !stopping && !cutoff {
            return Ok(());
        }
        if self.position() == 0.0 {
            control.flat.store(true, Ordering::Release);
            return Ok(());
        }
        control.flat.store(false, Ordering::Release);
        if self.waiting_cancel_for_exit || self.awaiting_exit {
            return Ok(());
        }
        if self.waiting_modify || self.accepted_stop.is_none() {
            return Err(anyhow::anyhow!(
                "ILRC exit guard cannot reconcile an unconfirmed stop or modification"
            ));
        }
        self.waiting_cancel_for_exit = true;
        self.cancel_order(self.accepted_stop.expect("stop"), None, None)?;
        Ok(())
    }
    fn on_quote(&mut self, quote: &QuoteTick) -> Result<()> {
        if let Some(shared) = &self.dashboard
            && let Ok(mut d) = shared.lock()
        {
            d.last_price = (quote.bid_price.as_f64() + quote.ask_price.as_f64()) / 2.0;
            if d.tick_count == 0 {
                eprintln!("[PASS] Kite market-data connection — first valid quote received");
            }
            d.tick_count += 1;
            d.last_tick_epoch = Some(chrono::Utc::now().timestamp());
            d.updates.notify_one();
        }
        Ok(())
    }
    fn on_bar(&mut self, bar: &Bar) -> Result<()> {
        let close = bar.ts_event.as_u64();
        ensure!(
            close > self.last_close_ns,
            "Repeated or out-of-order ILRC candle"
        );
        self.last_close_ns = close;
        let start = close
            .checked_sub(180_000_000_000)
            .ok_or_else(|| anyhow::anyhow!("Invalid ILRC bar timestamp"))?;
        let dt = chrono::DateTime::from_timestamp_nanos(start as i64)
            .with_timezone(&chrono::FixedOffset::east_opt(19800).expect("IST"));
        self.market_price
            .store(bar.close.as_f64().round() as i64, Ordering::Release);
        if let Some(shared) = &self.dashboard
            && let Ok(mut d) = shared.lock()
        {
            d.bars += 1;
            d.last_price = bar.close.as_f64();
            d.last_bar = dt.format("%d-%m %H:%M").to_string();
            d.last_bar_epoch = Some((close / 1_000_000_000) as i64);
            let p = self.selection.ilrc;
            let prior = self
                .candles
                .iter()
                .rev()
                .take(p.swing_len)
                .collect::<Vec<_>>();
            if prior.len() == p.swing_len {
                let lo = prior.iter().map(|c| c.low).fold(f64::INFINITY, f64::min);
                let hi = prior
                    .iter()
                    .map(|c| c.high)
                    .fold(f64::NEG_INFINITY, f64::max);
                let c = bar.close.as_f64();
                let long = bar.low.as_f64() < lo && c > lo;
                let short = bar.high.as_f64() > hi && c < hi;
                d.trigger_a = format!(
                    "{} | lo {:.0} hi {:.0} sweep:{}",
                    p.swing_len,
                    lo,
                    hi,
                    if long {
                        "L"
                    } else if short {
                        "S"
                    } else {
                        "NO"
                    }
                );
                let bprior = self.candles.iter().rev().take(20).collect::<Vec<_>>();
                let bhi = bprior
                    .iter()
                    .map(|v| v.high)
                    .fold(f64::NEG_INFINITY, f64::max);
                let blo = bprior.iter().map(|v| v.low).fold(f64::INFINITY, f64::min);
                let bodies = self
                    .candles
                    .iter()
                    .rev()
                    .take(20)
                    .map(|v| (v.close - v.open).abs())
                    .collect::<Vec<_>>();
                let avg = bodies.iter().sum::<f64>() / bodies.len() as f64;
                let body = (c - bar.open.as_f64()).abs();
                let bos = if c > bhi {
                    "LONG"
                } else if c < blo {
                    "SHORT"
                } else {
                    "NO"
                };
                d.trigger_b = format!("BOS:{} body {:.0}/{:.0} (1.3x)", bos, body, avg * 1.3);
                let session_day = dt.date_naive();
                let same_day = self
                    .candles
                    .iter()
                    .rev()
                    .take_while(|v| v.time().is_ok_and(|t| t.date_naive() == session_day));
                let (mut pv, mut vol) = (0.0, 0.0);
                for c in same_day {
                    let v = c.volume as f64;
                    pv += ((c.high + c.low + c.close) / 3.0) * v;
                    vol += v;
                }
                let current_vol = bar.volume.as_f64();
                pv += ((bar.high.as_f64() + bar.low.as_f64() + c) / 3.0) * current_vol;
                vol += current_vol;
                let vwap = if vol > 0.0 { pv / vol } else { c };
                let close_vwap = if bos == "SHORT" {
                    c <= vwap
                } else if bos == "LONG" {
                    c >= vwap
                } else {
                    false
                };
                let body_ok = body >= avg * 1.3;
                let bbreak = bos != "NO";
                let raw_ranges = self
                    .candles
                    .iter()
                    .rev()
                    .take(p.atr_len)
                    .collect::<Vec<_>>();
                let atr = if raw_ranges.len() == p.atr_len {
                    raw_ranges.iter().map(|v| v.high - v.low).sum::<f64>() / raw_ranges.len() as f64
                } else {
                    0.0
                };
                let atr_body = atr > 0.0 && body >= atr * 0.8;
                let status = |ok: bool| if ok { "PASS" } else { "FAIL" };
                d.gates_a = vec![
                    format!(
                        "{} Sweep/reclaim: L={} S={}",
                        status(long || short),
                        long,
                        short
                    ),
                    format!(
                        "{} Close {:.1} vs swing {:.1}/{:.1}",
                        status(long || short),
                        c,
                        lo,
                        hi
                    ),
                    format!("INFO VWAP {:.1} | close {:.1}", vwap, c),
                    "WAIT displacement / confirmation".into(),
                    "WAIT retrace zone / stop / R:R".into(),
                ];
                d.gates_b = vec![
                    format!("{} 20-bar BOS {}", status(bbreak), bos),
                    format!("{} Body {:.1} >= {:.1}", status(body_ok), body, avg * 1.3),
                    format!(
                        "{} ATR body {:.1} >= {:.1}",
                        status(atr_body),
                        body,
                        atr * 0.8
                    ),
                    format!(
                        "{} VWAP {:.1} (BOS side)",
                        if bos == "NO" {
                            "WAIT"
                        } else {
                            status(close_vwap)
                        },
                        vwap
                    ),
                    "WAIT internal BOS / pullback / stop".into(),
                ];
            } else {
                d.trigger_a = "WARMUP · insufficient swing bars".into();
                d.trigger_b = "WARMUP · insufficient 20 bars".into();
            }
            d.updates.notify_one();
        }
        self.candles.push(Candle {
            timestamp: dt.to_rfc3339(),
            open: bar.open.as_f64(),
            high: bar.high.as_f64(),
            low: bar.low.as_f64(),
            close: bar.close.as_f64(),
            volume: bar.volume.as_f64() as u64,
            oi: 0,
        });
        if self.control.as_ref().is_some_and(|c| {
            c.real
                && (c.stopping.load(Ordering::Acquire) || c.fault.lock().is_ok_and(|f| f.is_some()))
        }) {
            return Ok(());
        }
        if close <= self.start_ns || self.candles.len() <= 100 {
            return Ok(());
        }
        if self.position() != 0.0 {
            return self.maybe_manage_position(bar);
        }
        if self.pending_entry.is_some()
            || self.pending_stop.is_some()
            || self.accepted_stop.is_some()
            || self.awaiting_exit
            || self.waiting_cancel_for_exit
        {
            return Ok(());
        }
        let Some(signal) = self.signal()? else {
            return Ok(());
        };
        let key = format!("{}|{}|{}", signal.setup, signal.entry_time, signal.side);
        if !self.seen.insert(key) {
            return Ok(());
        }
        self.open_entry(signal)
    }
    fn on_stop(&mut self) -> Result<()> {
        if self.control.is_some() {
            self.clock().cancel_timer("ilrc_exit_guard");
        }
        ensure!(
            self.position() == 0.0
                && self.pending_entry.is_none()
                && self.pending_stop.is_none()
                && self.accepted_stop.is_none()
                && !self.waiting_modify
                && !self.waiting_cancel_for_exit
                && !self.awaiting_exit,
            "ILRC shutdown has exposure or unresolved orders: manual broker reconciliation required"
        );
        Ok(())
    }
}
nautilus_strategy!(IlrcActor, {
    fn on_order_accepted(&mut self, e: OrderAccepted) {
        if self.pending_stop == Some(e.client_order_id) {
            println!(
                "{}",
                serde_json::json!({"event":"ilrc_native_stop_accepted","client_order_id":e.client_order_id.to_string()})
            );
            self.accepted_stop = self.pending_stop.take();
            self.dashboard_update("Protective stop accepted");
        }
    }
    fn on_order_filled(&mut self, e: &OrderFilled) {
        if self.pending_entry == Some(e.client_order_id) {
            if let Some(control) = &self.control {
                control.flat.store(false, Ordering::Release);
            }
            self.pending_entry = None;
            let Some(sig) = self.entry_signal.take() else {
                self.fail_closed("Entry fill without signal");
                return;
            };
            let side = if sig.side == "LONG" { 1 } else { -1 };
            let entry = e.last_px.as_f64();
            let stop = if side > 0 {
                sig.stop.floor()
            } else {
                sig.stop.ceil()
            };
            let risk = (entry - stop) * side as f64;
            if risk <= 0.0 || !risk.is_finite() {
                self.fail_closed("Invalid fill-to-stop risk");
                return;
            }
            println!(
                "{}",
                serde_json::json!({"event":"ilrc_native_entry_fill","price":entry,"stop":stop,"target":sig.target,"broker_client_order_id":e.client_order_id.to_string()})
            );
            if let Some(shared) = &self.dashboard
                && let Ok(mut dashboard) = shared.lock()
            {
                dashboard.open_trade = Some(ilrc_live_dashboard::OpenTrade {
                    time: chrono::Utc::now()
                        .with_timezone(&chrono::FixedOffset::east_opt(19800).expect("IST"))
                        .format("%d-%m %H:%M")
                        .to_string(),
                    setup: sig.setup.to_owned(),
                    side: sig.side.to_owned(),
                    entry,
                });
            }
            self.protection = Some(Protection {
                side,
                entry,
                stop,
                target: sig.target,
                risk,
                be_sent: false,
            });
            let order = self.order().stop_market(
                self.bar_type.instrument_id(),
                if side > 0 {
                    OrderSide::Sell
                } else {
                    OrderSide::Buy
                },
                Quantity::from(1),
                Price::new(stop, 0),
                None,
                Some(TimeInForce::Day),
                None,
                Some(true),
                Some(false),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            );
            let id = order.client_order_id();
            self.pending_stop = Some(id);
            self.dashboard_update("Entry filled, protective stop pending");
            if self.submit_order(order, None, None, None).is_err() {
                self.fail_closed("Protective stop submission failed");
            }
        } else if self.accepted_stop == Some(e.client_order_id)
            || self.pending_stop == Some(e.client_order_id)
        {
            self.pending_stop = None;
            self.accepted_stop = None;
            self.protection = None;
            if let Some(shared) = &self.dashboard
                && let Ok(mut dashboard) = shared.lock()
                && let Some(open) = dashboard.open_trade.take()
            {
                let exit = e.last_px.as_f64();
                let points = (exit - open.entry) * if open.side == "LONG" { 1.0 } else { -1.0 };
                dashboard.trades.push(ilrc_live_dashboard::TradeRow {
                    time: open.time,
                    setup: open.setup,
                    side: open.side,
                    entry: open.entry,
                    exit,
                    reason: "STOP-LOSS".into(),
                    points,
                });
            }
        } else if self.awaiting_exit {
            if let Some(shared) = &self.dashboard
                && let Ok(mut dashboard) = shared.lock()
                && let Some(open) = dashboard.open_trade.take()
            {
                let exit = e.last_px.as_f64();
                let points = (exit - open.entry) * if open.side == "LONG" { 1.0 } else { -1.0 };
                dashboard.trades.push(ilrc_live_dashboard::TradeRow {
                    time: open.time,
                    setup: open.setup,
                    side: open.side,
                    entry: open.entry,
                    exit,
                    reason: "TARGET / EOD / EXIT".into(),
                    points,
                });
            }
            println!(
                "{}",
                serde_json::json!({"event":"ilrc_native_exit_filled","client_order_id":e.client_order_id.to_string(),"fill_price":e.last_px.as_f64()})
            );
            self.awaiting_exit = false;
            self.protection = None;
            if let Some(control) = &self.control {
                control
                    .flat
                    .store(self.position() == 0.0, Ordering::Release);
            }
        }
    }
    fn on_order_updated(&mut self, e: OrderUpdated) {
        if self.accepted_stop == Some(e.client_order_id) && self.waiting_modify {
            println!(
                "{}",
                serde_json::json!({"event":"ilrc_native_stop_modified","client_order_id":e.client_order_id.to_string()})
            );
            self.waiting_modify = false;
            if let Some(p) = self.protection.as_mut() {
                p.be_sent = true;
                p.stop = p.entry;
            }
            self.dashboard_update("Stop moved to break-even");
        }
    }
    fn on_order_canceled(&mut self, e: &OrderCanceled) {
        if self.accepted_stop == Some(e.client_order_id) && self.waiting_cancel_for_exit {
            println!(
                "{}",
                serde_json::json!({"event":"ilrc_native_stop_canceled","client_order_id":e.client_order_id.to_string()})
            );
            self.accepted_stop = None;
            self.waiting_cancel_for_exit = false;
            self.dashboard_update("Protective stop canceled; exit pending");
            if self.submit_exit().is_err() {
                self.fail_closed("Exit after confirmed stop cancellation failed");
            }
        }
    }
    fn on_order_rejected(&mut self, e: OrderRejected) {
        self.fail_closed(&format!("Broker rejected order: {}", e.reason));
    }
    fn on_order_denied(&mut self, e: OrderDenied) {
        self.fail_closed(&format!("Risk engine denied order: {}", e.reason));
    }
});
