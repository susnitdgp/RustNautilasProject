//! Nautilus strategy for one `sats` portfolio slot.
//!
//! One instance per slot (strategy ID `SATS-<slot>`), so several slots can run
//! side by side with isolated positions. SATS is bar-close only: every model
//! event (BUY / SELL entries, TP1–3, SL, flip, timeout) is confirmed when a bar
//! closes and is turned into a market order sized by the slot's `execution`
//! settings — the same thing a once-per-bar-close alert to AlgoMojo does.
//!
//! Live safety (when constructed `with_live`):
//! * bars that closed at or before `start_ns` only warm the engine — never trade;
//! * the bar closing at the daily square-off time (IST) flattens the position and
//!   blocks new entries for the rest of that day;
//! * a 1-second guard flattens on a data-feed fault or a stop request and reports
//!   flatness to the runner through `Control::flat`;
//! * any order rejection/denial or position mismatch halts new orders.
use super::live_control::Control;
use super::sats_dashboard::{self, Board, Shared};
use super::sats_config::{Execution, SatsConfig};
use anyhow::Result;
use chrono::{DateTime, FixedOffset, NaiveDate, Timelike};
use nautilus_common::{actor::DataActor, timer::TimeEvent};
use nautilus_core::DurationNanos;
use nautilus_model::{
    data::{Bar, BarType, QuoteTick},
    enums::{OrderSide, TimeInForce},
    events::{OrderDenied, OrderFilled, OrderRejected},
    identifiers::ClientId,
    types::Quantity,
};
use nautilus_trading::{
    nautilus_strategy,
    strategy::{Strategy, StrategyConfig, StrategyCore},
};
use sats::{BarInput, Engine, Event, Side};
use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

const GUARD_TIMER: &str = "sats_guard";

#[derive(Debug)]
struct Live {
    start_ns: i64,
    square_off_minute: u32,
    control: Control,
    market_price: Arc<AtomicI64>,
}

#[derive(Debug)]
pub struct SatsStrategy {
    core: StrategyCore,
    instance_id: String,
    bar_type: BarType,
    data_client: Option<ClientId>,
    bar_ns: i64,
    lots: u32,
    execution: Execution,
    engine: Engine,
    orders_enabled: bool,
    live: Option<Live>,
    /// Lots this run believes are open for the current model trade.
    open_lots: u32,
    /// Entry bar of the model trade this run actually opened; exits of other
    /// (warm-up or post-square-off) model trades are ignored.
    live_entry_bar: Option<i64>,
    /// IST date on which the square-off already happened (entries blocked).
    squared_off_on: Option<NaiveDate>,
    /// A flattening order is in flight.
    exiting: bool,
    last_close_ns: i64,
    halted: Option<String>,
    board: Option<Shared>,
}

fn ist() -> FixedOffset {
    FixedOffset::east_opt(19_800).expect("IST")
}

impl SatsStrategy {
    pub fn new(instance_id: &str, bar_type: BarType, config: &SatsConfig, engine: Engine, orders_enabled: bool) -> Self {
        Self {
            core: StrategyCore::new(StrategyConfig {
                strategy_id: Some(format!("SATS-{instance_id}").as_str().into()),
                log_events: false,
                log_commands: false,
                ..Default::default()
            }),
            instance_id: instance_id.to_owned(),
            bar_type,
            data_client: None,
            bar_ns: config.bar_ns(),
            lots: config.lots,
            execution: config.execution,
            engine,
            orders_enabled,
            live: None,
            open_lots: 0,
            live_entry_bar: None,
            squared_off_on: None,
            exiting: false,
            last_close_ns: 0,
            halted: None,
            board: None,
        }
    }

    pub fn with_data_client(mut self, client: ClientId) -> Self {
        self.data_client = Some(client);
        self
    }

    /// Enables the live safety behaviour described in the module docs.
    pub fn with_live(mut self, start_ns: i64, square_off_minute: u32, control: Control, market_price: Arc<AtomicI64>) -> Self {
        self.live = Some(Live { start_ns, square_off_minute, control, market_price });
        self
    }

    /// Live terminal dashboard state, updated on every bar, quote, order and fill.
    pub fn with_dashboard(mut self, board: Shared) -> Self {
        self.board = Some(board);
        self
    }

    fn board(&self, f: impl FnOnce(&mut Board)) {
        if let Some(b) = &self.board
            && let Ok(mut b) = b.lock()
        {
            f(&mut b);
        }
    }

    fn bar_input(&self, bar: &Bar) -> BarInput {
        let close = bar.ts_event.as_u64() as i64;
        BarInput {
            open_time_ns: close - self.bar_ns,
            close_time_ns: close,
            open: bar.open.as_f64(),
            high: bar.high.as_f64(),
            low: bar.low.as_f64(),
            close: bar.close.as_f64(),
            volume: Some(bar.volume.as_f64()),
        }
    }

    fn position(&self) -> f64 {
        self.cache()
            .positions_open(None, Some(&self.bar_type.instrument_id()), self.strategy_id().as_ref(), None, None)
            .iter()
            .map(|p| p.signed_qty)
            .sum()
    }

    fn stopping(&self) -> bool {
        self.live.as_ref().is_some_and(|l| {
            l.control.stopping.load(Ordering::Acquire) || l.control.fault.lock().map(|f| f.is_some()).unwrap_or(true)
        })
    }

    fn set_flat(&self, flat: bool) {
        if let Some(l) = &self.live {
            l.control.flat.store(flat, Ordering::Release);
        }
    }

    fn halt(&mut self, reason: &str) {
        self.log_json(serde_json::json!({"event":"sats_halted","instance":self.instance_id,"reason":reason}));
        self.board(|b| {
            b.halted = Some(reason.to_owned());
            b.status = "HALTED".into();
            b.event(format!("HALTED: {reason}"));
        });
        self.halted = Some(reason.to_owned());
    }

    fn log_json(&self, value: serde_json::Value) {
        sats_dashboard::emit(value);
    }

    fn log(&self, ev: &Event, lots: u32, note: &str) {
        self.log_json(serde_json::json!({
            "event": "sats_signal",
            "instance": self.instance_id,
            "kind": ev.kind,
            "bar_close_ist": DateTime::from_timestamp_nanos(ev.time_ns).with_timezone(&ist()).format("%Y-%m-%d %H:%M").to_string(),
            "side": if ev.trade.side == Side::Long { "long" } else { "short" },
            "level": ev.level,
            "model_fill": ev.fill,
            "lots": lots,
            "entry": ev.trade.entry, "sl": ev.trade.sl,
            "tp": [ev.trade.tp1, ev.trade.tp2, ev.trade.tp3],
            "score": ev.trade.entry_score, "tqi": ev.trade.entry_tqi,
            "orders_enabled": self.orders_enabled,
            "note": note,
        }));
    }

    /// Market order closing the whole broker position for this strategy.
    fn flatten(&mut self, why: &str) -> Result<()> {
        let position = self.position();
        self.live_entry_bar = None;
        self.open_lots = 0;
        if position == 0.0 {
            self.exiting = false;
            self.set_flat(true);
            return Ok(());
        }
        if self.exiting || !self.orders_enabled {
            return Ok(());
        }
        let side = if position > 0.0 { OrderSide::Sell } else { OrderSide::Buy };
        let order = self.order().market(
            self.bar_type.instrument_id(),
            side,
            Quantity::new(position.abs(), 0),
            Some(TimeInForce::Day),
            Some(true),
            Some(false),
            None,
            None,
            None,
            None,
        );
        self.exiting = true;
        self.set_flat(false);
        self.board(|b| b.event(format!("FLATTEN {side:?} {:.0} lot ({why})", position.abs())));
        self.log_json(serde_json::json!({"event":"sats_flatten","instance":self.instance_id,"reason":why,"qty":position.abs(),"side":format!("{side:?}")}));
        self.submit_order(order, None, None, None)?;
        Ok(())
    }

    fn handle(&mut self, ev: Event, entries_allowed: bool) -> Result<()> {
        let (side, lots, reduce_only) = if ev.kind.is_entry() {
            if !entries_allowed {
                self.log(&ev, 0, "entry blocked (square-off reached, stopping, or halted)");
                self.board(|b| b.event(format!("{} signal blocked (cut-off/stopping/halted)", ev.kind.label())));
                return Ok(());
            }
            self.live_entry_bar = Some(ev.trade.entry_bar);
            self.open_lots = self.lots;
            let side = if ev.trade.side == Side::Long { OrderSide::Buy } else { OrderSide::Sell };
            (side, self.lots, false)
        } else {
            if self.live_entry_bar != Some(ev.trade.entry_bar) {
                self.log(&ev, 0, "exit of a model trade this run did not open; ignored");
                return Ok(());
            }
            let n = self.execution.lots_to_close(ev.kind, self.lots, self.open_lots);
            self.open_lots -= n;
            if ev.closes_trade {
                self.live_entry_bar = None;
            }
            if n == 0 {
                self.log(&ev, 0, "model event; no order in this execution mode");
                return Ok(());
            }
            let side = if ev.trade.side == Side::Long { OrderSide::Sell } else { OrderSide::Buy };
            (side, n, true)
        };
        if let Some(reason) = &self.halted {
            let note = format!("halted: {reason}");
            self.log(&ev, lots, &note);
            return Ok(());
        }
        if !self.orders_enabled {
            self.log(&ev, lots, "shadow");
            return Ok(());
        }
        let position = self.position();
        let consistent = if reduce_only {
            (side == OrderSide::Sell && position >= f64::from(lots)) || (side == OrderSide::Buy && -position >= f64::from(lots))
        } else {
            position == 0.0 && !self.exiting
        };
        if !consistent {
            self.halt(&format!("{:?} for {lots} lot(s) with broker position {position}", ev.kind));
            self.log(&ev, lots, "position mismatch");
            return self.flatten("position mismatch");
        }
        let order = self.order().market(
            self.bar_type.instrument_id(),
            side,
            Quantity::new(f64::from(lots), 0),
            Some(TimeInForce::Day),
            Some(reduce_only),
            Some(false),
            None,
            None,
            None,
            None,
        );
        self.set_flat(false);
        self.log(&ev, lots, "order submitted");
        let t = ev.trade.clone();
        self.board(|b| {
            b.event(format!("{} {side:?} {lots} lot  (model {:.0}, SL {:.0}, TP1 {:.0})", ev.kind.label(), ev.fill, t.sl, t.tp1));
            if ev.kind.is_entry() {
                b.sl = Some(t.sl);
                b.tps = Some([t.tp1, t.tp2, t.tp3]);
            }
        });
        self.submit_order(order, None, None, None)?;
        Ok(())
    }
}

impl DataActor for SatsStrategy {
    fn on_start(&mut self) -> Result<()> {
        self.subscribe_bars(self.bar_type, self.data_client, None);
        if self.live.is_some() {
            self.subscribe_quotes(self.bar_type.instrument_id(), self.data_client, None);
            self.clock().set_timer_ns(GUARD_TIMER, DurationNanos::from_millis(1000), None, None, None, None, None)?;
        }
        Ok(())
    }

    fn on_bar(&mut self, bar: &Bar) -> Result<()> {
        if bar.bar_type != self.bar_type {
            return Ok(());
        }
        let input = self.bar_input(bar);
        if input.close_time_ns <= self.last_close_ns {
            return Ok(()); // repeated or out-of-order bar; already processed
        }
        self.last_close_ns = input.close_time_ns;
        let Some((start_ns, cutoff_minute)) = self.live.as_ref().map(|l| {
            l.market_price.store(input.close.round() as i64, Ordering::Release);
            (l.start_ns, l.square_off_minute)
        }) else {
            for ev in self.engine.on_bar(&input) {
                self.handle(ev, true)?;
            }
            return Ok(());
        };
        let warming = input.close_time_ns <= start_ns;
        let close_ist = DateTime::from_timestamp_nanos(input.close_time_ns).with_timezone(&ist());
        let at_or_after_cutoff = close_ist.hour() * 60 + close_ist.minute() >= cutoff_minute;
        let events = self.engine.on_bar(&input);
        if warming {
            return Ok(()); // historical warm-up: model state only, never orders
        }
        if at_or_after_cutoff && self.squared_off_on != Some(close_ist.date_naive()) {
            self.squared_off_on = Some(close_ist.date_naive());
            self.log_json(serde_json::json!({"event":"sats_square_off","instance":self.instance_id,"bar_close_ist":close_ist.format("%Y-%m-%d %H:%M").to_string()}));
            self.board(|b| {
                b.status = "SQUARED OFF".into();
                b.event("Daily square-off: flattening, no new entries today".into());
            });
            self.flatten("daily square-off")?;
        }
        let entries_allowed = !at_or_after_cutoff && !self.stopping() && self.halted.is_none();
        for ev in events {
            if at_or_after_cutoff && !ev.kind.is_entry() && self.live_entry_bar.is_none() {
                continue; // already flattened at square-off
            }
            self.handle(ev, entries_allowed)?;
        }
        let status = self.engine.status().clone();
        let bar_label = close_ist.format("%H:%M").to_string();
        let open_lots = self.open_lots;
        self.board(|b| {
            b.last_bar = Some((bar_label, input.close));
            b.live_bars += 1;
            b.warmed = status.warmed_up;
            b.trend = status.trend;
            b.supertrend = status.supertrend;
            b.tqi = status.tqi;
            b.regime = status.regime.clone();
            b.next_r = status.next_r;
            if b.status == "STARTING" {
                b.status = "RUNNING".into();
            }
            if open_lots == 0 && b.position == 0.0 {
                b.sl = None;
                b.tps = None;
            }
        });
        self.log_json(serde_json::json!({
            "event": "sats_bar", "instance": self.instance_id,
            "bar_close_ist": close_ist.format("%H:%M").to_string(),
            "close": input.close, "trend": status.trend, "supertrend": status.supertrend,
            "tqi": (status.tqi * 100.0).round() / 100.0, "regime": status.regime,
            "warmed_up": status.warmed_up, "open_lots": self.open_lots,
        }));
        Ok(())
    }

    fn on_quote(&mut self, quote: &QuoteTick) -> Result<()> {
        if let Some(live) = &self.live
            && quote.instrument_id == self.bar_type.instrument_id()
        {
            let mid = (quote.bid_price.as_f64() + quote.ask_price.as_f64()) / 2.0;
            live.market_price.store(mid.round() as i64, Ordering::Release);
            self.board(|b| b.last_price = Some(mid));
        }
        Ok(())
    }

    fn on_time_event(&mut self, e: &TimeEvent) -> Result<()> {
        if e.name.as_str() != GUARD_TIMER || self.live.is_none() {
            return Ok(());
        }
        if self.stopping() {
            if self.halted.is_none() {
                let fault = self.live.as_ref().and_then(|l| l.control.fault.lock().ok().and_then(|f| f.clone()));
                let shown = fault.clone();
                self.board(|b| {
                    b.feed_fault = shown;
                    b.status = "STOPPING".into();
                });
                self.halt(&fault.unwrap_or_else(|| "stop requested".into()));
            }
            self.flatten("stop / feed fault")?;
        } else {
            self.set_flat(self.position() == 0.0 && !self.exiting);
        }
        Ok(())
    }
}

nautilus_strategy!(SatsStrategy, {
    fn on_order_filled(&mut self, e: &OrderFilled) {
        sats_dashboard::emit(serde_json::json!({
            "event": "sats_fill",
            "instance": self.instance_id,
            "client_order_id": e.client_order_id.to_string(),
            "side": format!("{:?}", e.order_side),
            "qty": e.last_qty.as_f64(),
            "price": e.last_px.as_f64(),
        }));
        let qty = e.last_qty.as_f64();
        let signed = if e.order_side == OrderSide::Buy { qty } else { -qty };
        let px = e.last_px.as_f64();
        self.board(|b| {
            b.apply_fill(signed, px);
            b.event(format!("FILL {:?} {qty:.0} @ {px:.0}", e.order_side));
        });
        if self.position() == 0.0 {
            self.exiting = false;
            self.set_flat(true);
        }
    }
    fn on_order_rejected(&mut self, e: OrderRejected) {
        self.exiting = false;
        self.halt(&format!("order rejected: {}", e.reason));
    }
    fn on_order_denied(&mut self, e: OrderDenied) {
        self.exiting = false;
        self.halt(&format!("order denied: {}", e.reason));
    }
});
