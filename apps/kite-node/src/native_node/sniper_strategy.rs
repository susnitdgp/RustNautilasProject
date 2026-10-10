//! Nautilus strategy for one `sniper` portfolio slot (Precision Sniper v2.1.0).
//!
//! The `sniper` engine is the single source of trade decisions; this module turns its
//! events into a TARGET POSITION and moves the broker position toward it with one
//! market order at a time:
//! * Entry → target = ±lots (signal bar close, entries window / blackouts from JSON);
//! * Partial (TP1 / TP2) → target shrinks by `tp1_lots` / `tp2_lots`;
//! * Exit (SL, step stop, TP3, reversal, square-off) → target 0;
//! * a reversal is ONE flip order: it closes the lots still open (3, 2 or 1 after TP1 /
//!   TP2) and opens the opposite side, e.g. +2 → −3 is one SELL 5 (one order's brokerage,
//!   no wait for a separate exit fill). A partly filled flip that leaves a smaller position
//!   on the new side is never topped up: the strategy halts and flattens.
//!
//! Between bar closes every quote is checked like the script's intrabar model:
//! the executable price (bid for a long, ask for a short) against the current stop
//! (stop first) and TP1 / TP2 / TP3; a touch is applied to the engine
//! (`force_close` / `mark_target`) so the model and the executed position agree.
//! Step-stop moves take effect at the bar close, exactly as in the script.
//!
//! Live safety: warm-up bars never trade (a model trade still open at the live start is
//! closed in the model); square-off flattens and blocks entries for the day; a feed
//! fault / stop request flattens; any rejection, denial, unresolved order (30 s) or
//! position disagreement halts entries and flattens.
use super::live_control::Control;
use super::{
    dash_writer::Feed,
    sats_dashboard::{self, Board},
};
use super::sniper_config::SniperConfig;
use anyhow::Result;
use chrono::{DateTime, FixedOffset, NaiveDate};
use nautilus_common::{actor::DataActor, timer::TimeEvent};
use nautilus_core::DurationNanos;
use nautilus_model::{
    data::{Bar, BarType, QuoteTick},
    enums::{OrderSide, TimeInForce},
    events::{OrderCanceled, OrderDenied, OrderExpired, OrderFilled, OrderRejected},
    identifiers::{ClientId, ClientOrderId},
    orders::Order,
    types::Quantity,
};
use nautilus_trading::{
    nautilus_strategy,
    strategy::{Strategy, StrategyConfig, StrategyCore},
};
use sniper::{Engine, Event};
use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

const GUARD_TIMER: &str = "sniper_guard";
/// An order not resolved by the broker within this time halts the strategy.
const IN_FLIGHT_NS: i64 = 30_000_000_000;
/// A bar this long after its close came from the startup backfill, not the stream.
const BACKFILLED_NS: i64 = 10_000_000_000;
/// No new entries on a bar delivered later than this after its close.
const MAX_ENTRY_LATENESS_NS: i64 = 90_000_000_000;
/// After a halt the first flatten attempts go out on every guard tick, then one every
/// `FLATTEN_RETRY_NS` until flat. It never gives up: until 2.22.0 it stopped for the rest
/// of the day after 3 attempts, which an unresolved earlier order (each attempt denied at
/// once) could use up in ~3 s.
const FLATTEN_FAST_ATTEMPTS: u32 = 3;
const FLATTEN_RETRY_NS: i64 = 10_000_000_000;
/// Consecutive broker-cancelled exit orders before the strategy halts (it keeps flattening).
const MAX_EXIT_CANCELS: u32 = 5;

/// Whether a halted strategy may send its next flatten order now.
pub fn flatten_due(attempts: u32, last_ns: i64, now_ns: i64) -> bool {
    attempts < FLATTEN_FAST_ATTEMPTS || now_ns.saturating_sub(last_ns) >= FLATTEN_RETRY_NS
}

/// What to do when the broker cancels or expires one of our orders without filling it.
#[derive(Debug, PartialEq, Eq)]
pub enum BrokerClosed {
    /// An exit (reduce-only): leave the target; the next tick sends it again.
    RetryExit,
    /// An entry or flip (or an order the cache does not know): give up this trade
    /// (target 0, flatten whatever filled) without re-sending; later signals still trade.
    AbandonTrade,
    /// Too many cancelled exits in a row: halt (it keeps flattening, with backoff).
    Halt,
}
pub fn on_broker_closed(reduce_only: Option<bool>, exit_cancels: u32) -> BrokerClosed {
    match reduce_only {
        Some(true) if exit_cancels < MAX_EXIT_CANCELS => BrokerClosed::RetryExit,
        Some(true) => BrokerClosed::Halt,
        _ => BrokerClosed::AbandonTrade,
    }
}

fn ist() -> FixedOffset {
    FixedOffset::east_opt(19_800).expect("IST")
}

/// The single next order moving `pos` toward `target` (signed lots), as
/// `(side, lots, reduce_only)`:
/// * from flat → entry of `|target|`;
/// * same side, smaller, or to flat → reduce-only exit of the difference;
/// * opposite side → ONE flip order of `|pos| + |target|` (not reduce-only);
/// * same side, larger → `None` (never adds).
pub fn next_order(pos: i64, target: i64) -> Option<(OrderSide, i64, bool)> {
    if pos == target {
        return None;
    }
    let toward = |signed: i64| if signed > 0 { OrderSide::Buy } else { OrderSide::Sell };
    if pos == 0 {
        Some((toward(target), target.abs(), false))
    } else if target == 0 || (target.signum() == pos.signum() && target.abs() < pos.abs()) {
        Some((toward(-pos), (pos - target).abs(), true))
    } else if target.signum() == -pos.signum() {
        Some((toward(-pos), pos.abs() + target.abs(), false))
    } else {
        None
    }
}

#[derive(Debug)]
struct Live {
    start_ns: i64,
    control: Control,
    market_price: Arc<AtomicI64>,
}

#[derive(Debug)]
pub struct SniperStrategy {
    core: StrategyCore,
    instance_id: String,
    bar_type: BarType,
    data_client: Option<ClientId>,
    bar_ns: i64,
    config: SniperConfig,
    engine: Engine,
    orders_enabled: bool,
    live: Option<Live>,
    /// Signed lots the model wants held now.
    target: i64,
    /// This run opened the engine's current trade (warm-up trades never trade).
    ours: bool,
    /// The one order allowed in flight, and when it was sent (ns).
    in_flight: Option<(ClientOrderId, i64)>,
    squared_off_on: Option<NaiveDate>,
    last_close_ns: i64,
    halted: Option<String>,
    flatten_attempts: u32,
    last_flatten_ns: i64,
    /// Orders that passed `IN_FLIGHT_NS` unresolved: they may still be live at Kite, so the
    /// run is never reported flat while any of them is open.
    timed_out: Vec<ClientOrderId>,
    /// Exit orders the broker cancelled in a row (reset by any fill).
    exit_cancels: u32,
    board: Option<Feed>,
    live_started: bool,
}

impl SniperStrategy {
    pub fn new(instance_id: &str, bar_type: BarType, config: &SniperConfig, orders_enabled: bool) -> Self {
        Self {
            core: StrategyCore::new(StrategyConfig {
                strategy_id: Some(format!("SNIPER-{instance_id}").as_str().into()),
                log_events: false,
                log_commands: false,
                ..Default::default()
            }),
            instance_id: instance_id.to_owned(),
            bar_type,
            data_client: None,
            bar_ns: i64::from(config.bar_minutes) * 60_000_000_000,
            engine: Engine::new(config.engine_params()),
            config: config.clone(),
            orders_enabled,
            live: None,
            target: 0,
            ours: false,
            in_flight: None,
            squared_off_on: None,
            last_close_ns: 0,
            halted: None,
            flatten_attempts: 0,
            last_flatten_ns: 0,
            timed_out: Vec::new(),
            exit_cancels: 0,
            board: None,
            live_started: false,
        }
    }

    pub fn with_data_client(mut self, client: ClientId) -> Self {
        self.data_client = Some(client);
        self
    }

    pub fn with_live(mut self, start_ns: i64, control: Control, market_price: Arc<AtomicI64>) -> Self {
        self.live = Some(Live { start_ns, control, market_price });
        self
    }

    pub fn with_dashboard(mut self, board: Feed) -> Self {
        self.board = Some(board);
        self
    }

    /// Queues a dashboard update; never blocks (see `dash_writer`).
    fn board(&self, f: impl FnOnce(&mut Board) + Send + 'static) {
        if let Some(feed) = &self.board {
            feed.push(f);
        }
    }

    fn log_json(&self, mut value: serde_json::Value) {
        value["instance"] = serde_json::Value::String(self.instance_id.clone());
        sats_dashboard::emit(value);
    }

    fn now_ns() -> i64 {
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    }

    fn position(&self) -> i64 {
        self.cache()
            .positions_open(None, Some(&self.bar_type.instrument_id()), self.strategy_id().as_ref(), None, None)
            .iter()
            .map(|p| p.signed_qty)
            .sum::<f64>()
            .round() as i64
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
        if self.halted.is_none() {
            self.log_json(serde_json::json!({"event":"sniper_halted","reason":reason}));
            let why = reason.to_owned();
            self.board(move |b| {
                b.event(format!("HALTED: {why}"));
                b.halted = Some(why);
                b.status = "HALTED".into();
            });
            self.halted = Some(reason.to_owned());
        }
        self.target = 0;
        self.ours = false;
    }

    /// Model stop / targets onto the dashboard.
    fn show_levels(&self) {
        let levels = self.engine.trade.as_ref().filter(|_| self.ours).map(|t| (t.stop, [t.tp1, t.tp2, t.tp3]));
        self.board(move |b| {
            b.sl = levels.map(|l| l.0);
            b.tps = levels.map(|l| l.1);
        });
    }

    fn show_model(&self, close_ns: i64, close: f64, live_bar: bool) {
        let s = self.engine.status().clone();
        let r = self.engine.resolved();
        let label = DateTime::from_timestamp_nanos(close_ns).with_timezone(&ist()).format("%d %b %H:%M").to_string();
        self.board(move |b| {
            b.last_bar = Some((label, close));
            if live_bar {
                b.live_bars += 1;
            } else {
                b.history_bars += 1;
            }
            b.warmed = s.ready;
            b.trend = s.trend_dir as i8;
            b.model_rows = vec![
                (format!("EMA {}/{}", r.fast, r.slow), format!("{:.0} / {:.0}  (trend {:.0})", s.ema_fast, s.ema_slow, s.ema_trend)),
                ("Score".into(), format!("bull {:.0} · bear {:.0} of {:.0}", s.bull_score, s.bear_score, s.score_max)),
                ("ADX / RSI".into(), format!("{:.0} / {:.0}  {}{}", s.adx, s.rsi, s.regime, if s.high_vol { " · high vol" } else { "" })),
            ];
        });
    }

    /// Engine events → target position (only for the trade this run opened).
    fn apply(&mut self, events: Vec<Event>, entries_ok: bool) {
        for ev in events {
            match ev {
                Event::Entry { dir, price, stop, tp1, tp2, tp3, score, score_max, grade, origin } => {
                    let note = if !entries_ok {
                        "entry blocked (halted / stopping / squared off / late bar)"
                    } else {
                        self.target = i64::from(dir) * i64::from(self.config.lots);
                        self.ours = true;
                        "target set"
                    };
                    self.log_json(serde_json::json!({"event":"sniper_signal","kind":"entry","dir":dir,"price":price,"stop":stop,
                        "tp":[tp1,tp2,tp3],"score":score,"score_max":score_max,"grade":grade,"origin":origin,"lots":self.config.lots,"note":note}));
                    let side = if dir == 1 { "LONG" } else { "SHORT" };
                    let lots = self.config.lots;
                    self.board(move |b| b.event(format!("{side} {grade} {lots} lot @ {price:.0}  SL {stop:.0}  TP {tp1:.0}/{tp2:.0}/{tp3:.0}  ({note})")));
                }
                Event::Partial { level, price, .. } => {
                    if !self.ours {
                        continue;
                    }
                    let lots = i64::from(if level == 1 { self.config.tp1_lots } else { self.config.tp2_lots });
                    let open = self.target.abs();
                    self.target = self.target.signum() * (open - lots).max(0);
                    self.log_json(serde_json::json!({"event":"sniper_signal","kind":"partial","level":level,"price":price,"lots":lots}));
                    self.board(move |b| b.event(format!("TP{level} {price:.0}: close {lots} lot")));
                }
                Event::Exit { price, reason, gross_r, .. } => {
                    if !self.ours {
                        self.log_json(serde_json::json!({"event":"sniper_signal","kind":"exit","reason":reason,"note":"model trade this run did not open"}));
                        continue;
                    }
                    self.target = 0;
                    self.ours = false;
                    self.log_json(serde_json::json!({"event":"sniper_signal","kind":"exit","reason":reason,"price":price,"model_r":gross_r}));
                    self.board(move |b| b.event(format!("EXIT {reason} @ {price:.0}  (model {gross_r:+.2} R)")));
                }
            }
        }
        self.show_levels();
    }

    /// One market order at a time toward `target`; never adds; a reversal is one flip order.
    fn reconcile(&mut self) -> Result<()> {
        if self.live.is_none() || !self.orders_enabled {
            return Ok(());
        }
        if let Some((id, sent)) = self.in_flight {
            if self.cache().order(&id).is_some_and(|o| o.is_closed()) {
                self.in_flight = None;
            } else {
                if Self::now_ns() - sent > IN_FLIGHT_NS {
                    self.in_flight = None;
                    self.timed_out.push(id);
                    self.halt("order not resolved by the broker within 30 s; check Kite");
                }
                return Ok(());
            }
        }
        self.forget_closed_timed_out();
        let pos = self.position();
        let target = if self.halted.is_some() || self.stopping() { 0 } else { self.target };
        if pos == target {
            self.set_flat(pos == 0 && self.timed_out.is_empty());
            return Ok(());
        }
        let now = Self::now_ns();
        if self.halted.is_some() && !flatten_due(self.flatten_attempts, self.last_flatten_ns, now) {
            self.set_flat(false);
            return Ok(());
        }
        let Some((side, qty, reduce)) = next_order(pos, target) else {
            self.halt(&format!("model wants {target} lots while {pos} are open (never adds)"));
            return Ok(());
        };
        if self.halted.is_some() {
            self.flatten_attempts += 1;
            self.last_flatten_ns = now;
        }
        let order = self.order().market(
            self.bar_type.instrument_id(),
            side,
            Quantity::new(qty as f64, 0),
            Some(TimeInForce::Day),
            Some(reduce),
            Some(false),
            None,
            None,
            None,
            None,
        );
        self.in_flight = Some((order.client_order_id(), Self::now_ns()));
        self.set_flat(false);
        self.log_json(serde_json::json!({"event":"sniper_order","side":format!("{side:?}"),"qty":qty,"reduce_only":reduce,"position":pos,"target":target}));
        self.board(move |b| b.event(format!("ORDER {side:?} {qty} lot{} (position {pos} → target {target})", if reduce { " reduce-only" } else { "" })));
        self.submit_order(order, None, None, None)?;
        Ok(())
    }

    /// Drops timed-out orders the cache now shows as closed.
    fn forget_closed_timed_out(&mut self) {
        if self.timed_out.is_empty() {
            return;
        }
        let still_open: Vec<ClientOrderId> = {
            let cache = self.cache();
            self.timed_out
                .iter()
                .filter(|id| !cache.order(id).is_some_and(|o| o.is_closed()))
                .copied()
                .collect()
        };
        self.timed_out = still_open;
    }

    /// The broker cancelled or expired one of our orders (no fill, or only part of it).
    /// An entry or flip is never re-sent (it would chase the price away from the model's
    /// levels): this trade is abandoned and whatever filled is flattened; later signals
    /// still trade. An exit is re-sent by the next tick, up to `MAX_EXIT_CANCELS` in a
    /// row, then the strategy halts (and keeps flattening).
    fn closed_by_broker(&mut self, id: ClientOrderId, how: &str) {
        if self.in_flight.is_some_and(|(i, _)| i == id) {
            self.in_flight = None;
        }
        let reduce_only = self.cache().order(&id).map(|o| o.is_reduce_only());
        self.log_json(serde_json::json!({"event":"sniper_order_closed_by_broker","client_order_id":id.to_string(),
            "how":how,"reduce_only":reduce_only,"exit_cancels":self.exit_cancels}));
        match on_broker_closed(reduce_only, self.exit_cancels) {
            BrokerClosed::RetryExit => {
                self.exit_cancels += 1;
                let n = self.exit_cancels;
                let how = how.to_owned();
                self.board(move |b| b.event(format!("Exit order {how} by Kite; retrying ({n}/{MAX_EXIT_CANCELS})")));
            }
            BrokerClosed::AbandonTrade => {
                self.target = 0;
                self.ours = false;
                let how = how.to_owned();
                self.board(move |b| b.event(format!("Entry/flip order {how} by Kite: trade abandoned, not re-sent")));
                self.show_levels();
            }
            BrokerClosed::Halt => {
                self.halt(&format!("exit order {how} by Kite {MAX_EXIT_CANCELS} times in a row; flattening"));
            }
        }
    }

    /// Every quote: stop first, then TP1 / TP2 / TP3 on the executable price.
    fn check_levels(&mut self, bid: f64, ask: f64) -> Result<()> {
        if !self.ours || self.halted.is_some() {
            return Ok(());
        }
        let Some(t) = self.engine.trade.as_ref() else { return Ok(()) };
        let d = f64::from(t.dir);
        let px = if t.dir == 1 { bid } else { ask };
        if (px - t.stop) * d <= 0.0 {
            let reason = if t.stop == t.initial_stop { "SL" } else { "Step stop" };
            let events: Vec<Event> = self.engine.force_close(px, reason).into_iter().collect();
            self.apply(events, false);
            return self.reconcile();
        }
        let level = [(1u8, t.tp1, t.hit1), (2, t.tp2, t.hit2), (3, t.tp3, t.hit3)]
            .iter()
            .filter(|(_, tp, hit)| !hit && (px - tp) * d >= 0.0)
            .map(|(l, _, _)| *l)
            .max();
        if let Some(level) = level {
            let events = self.engine.mark_target(level);
            self.apply(events, false);
            return self.reconcile();
        }
        Ok(())
    }
}

impl DataActor for SniperStrategy {
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
        let close_ns = bar.ts_event.as_u64() as i64;
        if close_ns <= self.last_close_ns {
            return Ok(()); // repeated or out-of-order bar
        }
        self.last_close_ns = close_ns;
        let close_s = close_ns / 1_000_000_000;
        let input = sniper::Bar {
            start: close_s - self.bar_ns / 1_000_000_000,
            open: bar.open.as_f64(),
            high: bar.high.as_f64(),
            low: bar.low.as_f64(),
            close: bar.close.as_f64(),
            volume: bar.volume.as_f64(),
        };
        let live_bar = self.live.as_ref().is_some_and(|l| {
            l.market_price.store(input.close.round() as i64, Ordering::Release);
            close_ns > l.start_ns
        });
        if !live_bar {
            // warm-up (and backtest-like) bars: model only, never orders
            let _ = self.engine.on_bar(input, self.config.entries_allowed_at(close_s));
            self.show_model(close_ns, input.close, false);
            return Ok(());
        }
        if !self.live_started {
            self.live_started = true;
            if let Some(Event::Exit { reason: _, gross_r, .. }) = self.engine.force_close(input.open, "Live start") {
                self.log_json(serde_json::json!({"event":"sniper_warmup_trade_closed","model_r":gross_r}));
            }
            let s = self.engine.status().clone();
            self.board(move |b| {
                b.event(format!(
                    "Warm-up done on {} bars · trend {} · ready {} · trading from this bar",
                    b.history_bars,
                    match s.trend_dir {
                        1 => "BULLISH",
                        -1 => "BEARISH",
                        _ => "neutral",
                    },
                    if s.ready { "yes" } else { "NO" }
                ));
                if b.status == "STARTING" {
                    b.status = "RUNNING".into();
                }
            });
            self.log_json(serde_json::json!({"event":"sniper_warmed","ready":s.ready,"trend":s.trend_dir}));
        }
        let close_ist = DateTime::from_timestamp_nanos(close_ns).with_timezone(&ist());
        let late_ns = Self::now_ns() - close_ns;
        let too_late = late_ns > MAX_ENTRY_LATENESS_NS;
        if late_ns > BACKFILLED_NS {
            let secs = late_ns / 1_000_000_000;
            self.board(move |b| b.event(format!("Backfilled bar {} ({secs}s after close){}", close_ist.format("%H:%M"),
                if too_late { " · exits only" } else { "" })));
        }
        let squared = self.squared_off_on == Some(close_ist.date_naive());
        let entries_ok = !squared && !too_late && self.halted.is_none() && !self.stopping();
        let events = self.engine.on_bar(input, self.config.entries_allowed_at(close_s) && entries_ok);
        self.apply(events, entries_ok);
        if self.config.square_off_due(close_s) && !squared {
            self.squared_off_on = Some(close_ist.date_naive());
            let events: Vec<Event> = self.engine.force_close(input.close, "Square-off").into_iter().collect();
            self.apply(events, false);
            self.target = 0;
            self.board(move |b| {
                b.status = "SQUARED OFF".into();
                b.event("Daily square-off: flattening, no new entries today".into());
            });
            self.log_json(serde_json::json!({"event":"sniper_square_off","bar_close_ist":close_ist.format("%Y-%m-%d %H:%M").to_string()}));
        }
        self.reconcile()?;
        self.show_model(close_ns, input.close, true);
        self.show_levels();
        let s = self.engine.status().clone();
        let t = self.engine.trade.as_ref().map(|t| serde_json::json!({"dir":t.dir,"entry":t.entry,"stop":t.stop,"tp":[t.tp1,t.tp2,t.tp3],"hits":[t.hit1,t.hit2,t.hit3],"remaining":t.remaining}));
        self.log_json(serde_json::json!({
            "event": "sniper_bar", "bar_close_ist": close_ist.format("%H:%M").to_string(), "close": input.close,
            "trend": s.trend_dir, "bull": s.bull_score, "bear": s.bear_score, "adx": (s.adx * 10.0).round() / 10.0,
            "ready": s.ready, "target": self.target, "position": self.position(), "model_trade": t,
        }));
        Ok(())
    }

    fn on_quote(&mut self, quote: &QuoteTick) -> Result<()> {
        if let Some(live) = &self.live
            && quote.instrument_id == self.bar_type.instrument_id()
        {
            let (bid, ask) = (quote.bid_price.as_f64(), quote.ask_price.as_f64());
            let mid = (bid + ask) / 2.0;
            live.market_price.store(mid.round() as i64, Ordering::Release);
            self.board(move |b| b.last_price = Some(mid));
            self.check_levels(bid, ask)?;
        }
        Ok(())
    }

    fn on_time_event(&mut self, e: &TimeEvent) -> Result<()> {
        if e.name.as_str() != GUARD_TIMER || self.live.is_none() {
            return Ok(());
        }
        if self.stopping() && self.halted.is_none() {
            let fault = self.live.as_ref().and_then(|l| l.control.fault.lock().ok().and_then(|f| f.clone()));
            let shown = fault.clone();
            self.board(move |b| {
                b.feed_fault = shown;
                b.status = "STOPPING".into();
            });
            self.halt(&fault.unwrap_or_else(|| "stop requested".into()));
        }
        self.reconcile()?;
        if self.in_flight.is_none() {
            self.set_flat(self.position() == 0 && self.timed_out.is_empty());
        }
        Ok(())
    }
}

nautilus_strategy!(SniperStrategy, {
    fn on_order_filled(&mut self, e: &OrderFilled) {
        let qty = e.last_qty.as_f64();
        let signed = if e.order_side == OrderSide::Buy { qty } else { -qty };
        let px = e.last_px.as_f64();
        sats_dashboard::emit(serde_json::json!({"event":"sniper_fill","instance":self.instance_id,
            "client_order_id":e.client_order_id.to_string(),"side":format!("{:?}", e.order_side),"qty":qty,"price":px}));
        let side = e.order_side;
        self.exit_cancels = 0;
        self.board(move |b| {
            b.apply_fill(signed, px);
            b.event(format!("FILL {side:?} {qty:.0} @ {px:.0}"));
        });
        if let Err(err) = self.reconcile() {
            self.halt(&format!("order after fill failed: {err:#}"));
        }
    }
    fn on_order_canceled(&mut self, e: &OrderCanceled) {
        self.closed_by_broker(e.client_order_id, "cancelled");
    }
    fn on_order_expired(&mut self, e: OrderExpired) {
        self.closed_by_broker(e.client_order_id, "expired");
    }
    fn on_order_rejected(&mut self, e: OrderRejected) {
        if self.in_flight.is_some_and(|(id, _)| id == e.client_order_id) {
            self.in_flight = None;
        }
        self.halt(&format!("order rejected: {}", e.reason));
    }
    fn on_order_denied(&mut self, e: OrderDenied) {
        if self.in_flight.is_some_and(|(id, _)| id == e.client_order_id) {
            self.in_flight = None;
        }
        self.halt(&format!("order denied: {}", e.reason));
    }
});

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn next_order_scales_out_flips_in_one_order_and_never_adds() {
        assert_eq!(next_order(0, 3), Some((OrderSide::Buy, 3, false)), "entry from flat");
        assert_eq!(next_order(0, -3), Some((OrderSide::Sell, 3, false)), "short entry from flat");
        assert_eq!(next_order(3, 2), Some((OrderSide::Sell, 1, true)), "TP1: one lot");
        assert_eq!(next_order(2, 1), Some((OrderSide::Sell, 1, true)), "TP2: one lot");
        assert_eq!(next_order(3, 1), Some((OrderSide::Sell, 2, true)), "TP1 + TP2 on one tick");
        assert_eq!(next_order(1, 0), Some((OrderSide::Sell, 1, true)), "TP3 / stop: last lot");
        assert_eq!(next_order(-2, 0), Some((OrderSide::Buy, 2, true)));
        // flip quantity = lots still open + new lots, whichever TPs were hit
        assert_eq!(next_order(3, -3), Some((OrderSide::Sell, 6, false)), "flip, no TP hit");
        assert_eq!(next_order(2, -3), Some((OrderSide::Sell, 5, false)), "flip after TP1");
        assert_eq!(next_order(1, -3), Some((OrderSide::Sell, 4, false)), "flip after TP2");
        assert_eq!(next_order(-3, 3), Some((OrderSide::Buy, 6, false)));
        assert_eq!(next_order(-1, 3), Some((OrderSide::Buy, 4, false)));
        // never adds: an open position below target on the same side (e.g. a partly filled flip)
        assert_eq!(next_order(1, 3), None);
        assert_eq!(next_order(-1, -3), None);
        assert_eq!(next_order(2, 2), None);
    }
    #[test]
    fn flattening_after_a_halt_never_gives_up() {
        let s = 1_000_000_000_i64;
        // the first attempts go out on every guard tick
        assert!(flatten_due(0, 0, 0));
        assert!(flatten_due(2, 100 * s, 100 * s));
        // then one every 10 s, for as long as it takes (it used to stop after 3)
        assert!(!flatten_due(3, 100 * s, 105 * s));
        assert!(flatten_due(3, 100 * s, 110 * s));
        assert!(!flatten_due(500, 100 * s, 109 * s));
        assert!(flatten_due(500, 100 * s, 3600 * s));
    }
    #[test]
    fn broker_cancels_never_re_send_entries_and_retry_exits_a_bounded_number_of_times() {
        assert_eq!(on_broker_closed(Some(false), 0), BrokerClosed::AbandonTrade, "entry/flip");
        assert_eq!(on_broker_closed(None, 0), BrokerClosed::AbandonTrade, "unknown order");
        assert_eq!(on_broker_closed(Some(true), 0), BrokerClosed::RetryExit);
        assert_eq!(on_broker_closed(Some(true), MAX_EXIT_CANCELS - 1), BrokerClosed::RetryExit);
        assert_eq!(on_broker_closed(Some(true), MAX_EXIT_CANCELS), BrokerClosed::Halt);
    }
}
