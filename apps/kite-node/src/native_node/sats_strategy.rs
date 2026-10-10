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
//! * any order rejection/denial or position mismatch halts new orders;
//! * with `exchange_stop_loss`, a reduce-only SL-M rests at the broker at SATS's SL
//!   once the entry fills; every other exit cancels it (confirmed) and only then
//!   sends the market exit, so the two can never both fill.
use super::live_control::Control;
use super::{
    dash_writer::Feed,
    sats_dashboard::{self, Board},
};
use super::sats_config::{Execution, ExitMode, SatsConfig};
use super::sats_trail::{Bracket, BracketExit, Trail};
use anyhow::Result;
use chrono::{DateTime, FixedOffset, NaiveDate, Timelike};
use nautilus_common::{actor::DataActor, timer::TimeEvent};
use nautilus_core::DurationNanos;
use nautilus_model::{
    data::{Bar, BarType, QuoteTick},
    enums::{OrderSide, TimeInForce},
    events::{OrderCancelRejected, OrderCanceled, OrderDenied, OrderFilled, OrderRejected},
    identifiers::{ClientId, ClientOrderId},
    orders::Order,
    types::{Price, Quantity},
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
    /// The slot's strategy file (entry window etc.).
    config: SatsConfig,
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
    board: Option<Feed>,
    /// The "warm-up finished" dashboard event was shown.
    warm_reported: bool,
    /// Same-bar entry waiting for this run's closing order to fill (event, deadline ns).
    pending_entry: Option<(Event, i64)>,
    /// Live trailing stop of the open position (`exit_mode: "trail"`).
    trail: Option<Trail>,
    /// Live tick-checked SL / target (`exit_mode: "single"`, `intrabar_exits`).
    bracket: Option<Bracket>,
    /// Resting reduce-only SL-M at the broker (`exchange_stop_loss`).
    stop_order: Option<ClientOrderId>,
    /// Side and SL of the position this run opened (the SL-M trigger).
    entry_sl: Option<(Side, f64)>,
    /// An exit waits for the SL-M cancel to be confirmed: (reason, deadline ns).
    exit_after_cancel: Option<(String, i64)>,
    /// The broker refused an SL-M; the tick / bar-close SL protects for this run.
    stop_unavailable: Option<String>,
}

/// How long an exit waits for the broker to confirm the SL-M cancel before halting.
const CANCEL_EXIT_NS: i64 = 15_000_000_000;

/// How long a deferred flip entry waits for the closing fill before the strategy halts.
const PENDING_ENTRY_NS: i64 = 30_000_000_000;
/// A bar this long after its close came from the startup backfill, not the stream.
const BACKFILLED_NS: i64 = 10_000_000_000;
/// No new entries on a bar delivered later than this after its close.
const MAX_ENTRY_LATENESS_NS: i64 = 90_000_000_000;

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
            config: config.clone(),
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
            warm_reported: false,
            pending_entry: None,
            trail: None,
            bracket: None,
            stop_order: None,
            entry_sl: None,
            exit_after_cancel: None,
            stop_unavailable: None,
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

    /// Live dashboard feed (see `dash_writer`), updated on every bar, quote, order and fill.
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

    /// Copies the engine's current state onto the dashboard (history or live bar).
    fn show_model(&self, close_ist: DateTime<FixedOffset>, close: f64, live_bar: bool) {
        let status = self.engine.status().clone();
        let label = close_ist.format("%d %b %H:%M").to_string();
        let open_lots = self.open_lots;
        self.board(move |b| {
            b.last_bar = Some((label, close));
            if live_bar {
                b.live_bars += 1;
            } else {
                b.history_bars += 1;
            }
            b.warmed = status.warmed_up;
            b.trend = status.trend;
            b.supertrend = status.supertrend;
            b.tqi = status.tqi;
            b.regime = status.regime.clone();
            b.next_r = status.next_r;
            if open_lots == 0 && b.position == 0.0 {
                b.sl = None;
                b.tps = None;
            }
        });
    }

    /// One-time "warm-up finished" event; status turns RUNNING.
    fn report_warm(&mut self) {
        if self.warm_reported {
            return;
        }
        self.warm_reported = true;
        let status = self.engine.status().clone();
        let trend = match status.trend {
            1 => "BULLISH",
            -1 => "BEARISH",
            _ => "neutral",
        };
        // The bar in progress at start-up is incomplete on the stream; it is
        // backfilled from Kite history ~45 s after it closes, before the first
        // full stream bar.
        let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let gap_close = DateTime::from_timestamp_nanos((now_ns / self.bar_ns + 1) * self.bar_ns).with_timezone(&ist());
        self.board(move |b| {
            let msg = format!(
                "Warm-up done on {} history bars · trend {trend} · warmed {} · bar closing {} is backfilled from Kite history ~45s later",
                b.history_bars,
                if status.warmed_up { "yes" } else { "NO" },
                gap_close.format("%H:%M")
            );
            b.event(msg);
            if b.status == "STARTING" {
                b.status = "RUNNING".into();
            }
        });
        self.log_json(serde_json::json!({"event":"sats_warmed","instance":self.instance_id,
            "warmed_up":status.warmed_up,"trend":status.trend,"supertrend":status.supertrend}));
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
        let why = reason.to_owned();
        self.board(move |b| {
            b.event(format!("HALTED: {why}"));
            b.halted = Some(why);
            b.status = "HALTED".into();
        });
        self.halted = Some(reason.to_owned());
        if self.pending_entry.take().is_some() {
            self.live_entry_bar = None;
            self.open_lots = 0;
        }
    }

    /// Releases a deferred same-bar entry once the closing order has filled, or halts
    /// if the close is not confirmed within `PENDING_ENTRY_NS`.
    fn try_pending(&mut self, now_ns: i64) -> Result<()> {
        let Some(deadline) = self.pending_entry.as_ref().map(|(_, d)| *d) else {
            return Ok(());
        };
        if self.position() == 0.0 {
            self.exiting = false;
            let (ev, _) = self.pending_entry.take().expect("pending entry");
            let cutoff_passed = self.live.as_ref().is_some_and(|l| {
                let t = DateTime::from_timestamp_nanos(now_ns).with_timezone(&ist());
                t.hour() * 60 + t.minute() >= l.square_off_minute || self.squared_off_on == Some(t.date_naive())
            });
            let allowed = !cutoff_passed && !self.stopping() && self.halted.is_none();
            self.live_entry_bar = None;
            self.open_lots = 0;
            return self.handle(ev, allowed);
        }
        if now_ns > deadline {
            self.halt("closing order not filled within 30 s; flip entry cancelled");
            return self.flatten("closing order not filled");
        }
        Ok(())
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

    fn trail_mode(&self) -> bool {
        self.execution.exit_mode == ExitMode::Trail && self.live.is_some()
    }

    fn bracket_mode(&self) -> bool {
        self.execution.intrabar_single() && self.live.is_some()
    }

    /// Every quote (single mode, intrabar exits): executable price against the
    /// SL and the target; exits the whole position the moment one is crossed.
    fn check_bracket(&mut self, bid: f64, ask: f64) -> Result<()> {
        if self.open_lots == 0 || self.exiting {
            return Ok(());
        }
        let Some(b) = self.bracket.clone() else { return Ok(()) };
        let px = if b.side == Side::Long { bid } else { ask };
        let target = self.execution.single_exit_at.label();
        match b.on_price(px) {
            // with a resting SL-M the exchange executes the stop itself
            Some(BracketExit::Stop) if self.stop_open() => Ok(()),
            Some(BracketExit::Stop) => self.flatten(&format!("SL {:.0} hit at {px:.0} (intrabar)", b.stop)),
            Some(BracketExit::Target) => self.flatten(&format!("{target} {:.0} reached at {px:.0} (intrabar)", b.target)),
            None => Ok(()),
        }
    }

    /// Every quote: executable price against the trailing stop.
    fn check_trail(&mut self, bid: f64, ask: f64) -> Result<()> {
        if self.open_lots == 0 || self.exiting {
            return Ok(());
        }
        let Some(mut trail) = self.trail.take() else { return Ok(()) };
        let px = if trail.side == Side::Long { bid } else { ask };
        let (moved, hit) = trail.on_price(&self.execution.trail, px);
        let stop = trail.stop;
        let stage = trail.stage_label();
        self.trail = Some(trail);
        if let Some(m) = moved {
            let text = format!("TRAIL {m}");
            self.board(move |b| {
                b.sl = Some(stop);
                b.event(text);
            });
            self.log_json(serde_json::json!({"event":"sats_trail","instance":self.instance_id,"move":m,"stop":stop,"price":px}));
        }
        let at_initial_sl = self.trail.as_ref().is_some_and(|t| !t.active);
        if hit && !(at_initial_sl && self.stop_open()) {
            // before activation the stop is the SL the resting SL-M already covers
            return self.flatten(&format!("{stage} hit at {px:.0} (stop {stop:.0})"));
        }
        Ok(())
    }

    /// Bar close: follow the SuperTrend line once the trail is active.
    fn trail_bar_close(&mut self) {
        if self.open_lots == 0 || self.exiting {
            return;
        }
        let status = self.engine.status().clone();
        let Some(trail) = self.trail.as_mut() else { return };
        if let Some(stop) = trail.on_bar_close(&self.execution.trail, status.trend, status.supertrend) {
            self.board(move |b| {
                b.sl = Some(stop);
                b.event(format!("TRAIL stop -> SuperTrend {stop:.0}"));
            });
            self.log_json(serde_json::json!({"event":"sats_trail","instance":self.instance_id,"move":"supertrend","stop":stop}));
        }
    }

    /// An exit waiting for the SL-M cancel: resolve it if the stop filled (position
    /// flat), or halt if the broker has not confirmed in time.
    fn check_cancel_deadline(&mut self, now_ns: i64) {
        let Some((_, deadline)) = self.exit_after_cancel.as_ref().map(|(w, d)| (w.clone(), *d)) else { return };
        if self.position() == 0.0 && !self.stop_open() {
            self.exit_after_cancel = None;
            self.exiting = false;
        } else if now_ns > deadline {
            self.exit_after_cancel = None;
            self.halt("SL-M cancel not confirmed by the broker within 15 s; check Kite now");
        }
    }

    fn exchange_stop_mode(&self) -> bool {
        self.execution.exchange_stop_loss && self.live.is_some() && self.orders_enabled && self.stop_unavailable.is_none()
    }

    /// The SL-M this run placed is still working at the broker.
    fn stop_open(&self) -> bool {
        self.stop_order.is_some_and(|id| self.cache().order(&id).is_some_and(|o| !o.is_closed()))
    }

    fn now_ns() -> i64 {
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    }

    /// Entry filled: rest a reduce-only SL-M for the whole position at SATS's SL.
    fn place_exchange_stop(&mut self) -> Result<()> {
        let Some((side, sl)) = self.entry_sl else { return Ok(()) };
        let position = self.position();
        if position == 0.0 || self.stop_order.is_some() || self.exiting || (side == Side::Long) != (position > 0.0) {
            return Ok(());
        }
        let order_side = if position > 0.0 { OrderSide::Sell } else { OrderSide::Buy };
        let order = self.order().stop_market(
            self.bar_type.instrument_id(),
            order_side,
            Quantity::new(position.abs(), 0),
            Price::new(sl.round(), 0),
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
        self.stop_order = Some(order.client_order_id());
        self.board(move |b| {
            b.exchange_stop = Some(sl.round());
            b.event(format!("SL-M {order_side:?} {:.0} lot @ {:.0} sent to Zerodha", position.abs(), sl.round()));
        });
        self.log_json(serde_json::json!({"event":"sats_exchange_stop","instance":self.instance_id,"action":"place","trigger":sl.round(),"side":format!("{order_side:?}")}));
        self.submit_order(order, None, None, None)?;
        Ok(())
    }

    /// The broker refused the SL-M: keep trading, protected by the program's own SL.
    fn exchange_stop_failed(&mut self, reason: &str) {
        self.stop_order = None;
        self.stop_unavailable = Some(reason.to_owned());
        let text = format!("SL-M refused by broker ({reason}); program SL protects this run");
        self.board(move |b| {
            b.exchange_stop = None;
            b.event(text);
        });
        self.log_json(serde_json::json!({"event":"sats_exchange_stop","instance":self.instance_id,"action":"refused","reason":reason}));
    }

    /// Market order closing the whole broker position for this strategy.
    fn flatten(&mut self, why: &str) -> Result<()> {
        let position = self.position();
        self.live_entry_bar = None;
        self.open_lots = 0;
        self.trail = None;
        self.bracket = None;
        if self.stop_open() {
            // Never send the market exit while the SL-M can still fill: cancel it,
            // wait for the broker's confirmation (on_order_canceled), then exit.
            if self.exit_after_cancel.is_none() {
                let id = self.stop_order.expect("open stop");
                self.exit_after_cancel = Some((why.to_owned(), Self::now_ns() + CANCEL_EXIT_NS));
                self.exiting = true;
                self.set_flat(false);
                let text = format!("Cancelling SL-M before exit ({why})");
                self.board(move |b| b.event(text));
                self.log_json(serde_json::json!({"event":"sats_exchange_stop","instance":self.instance_id,"action":"cancel","reason":why}));
                self.cancel_order(id, None, None)?;
            }
            return Ok(());
        }
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
        let text = format!("FLATTEN {side:?} {:.0} lot ({why})", position.abs());
        self.board(move |b| b.event(text));
        self.log_json(serde_json::json!({"event":"sats_flatten","instance":self.instance_id,"reason":why,"qty":position.abs(),"side":format!("{side:?}")}));
        self.submit_order(order, None, None, None)?;
        Ok(())
    }

    fn handle(&mut self, ev: Event, entries_allowed: bool) -> Result<()> {
        let (side, lots, reduce_only) = if ev.kind.is_entry() {
            if self.trail_mode() && self.open_lots > 0 {
                // Trail mode has no target, so a position can outlive SATS's own
                // trade; the new (opposite) signal closes it first. The entry below
                // then waits for that close to fill (pending entry).
                self.flatten(&format!("trend flip ({})", ev.kind.label()))?;
            }
            if !entries_allowed {
                self.log(&ev, 0, "entry blocked (square-off reached, stopping, or halted)");
                self.board(move |b| b.event(format!("{} signal blocked (cut-off/stopping/halted)", ev.kind.label())));
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
        if reduce_only && self.stop_open() {
            // the resting SL-M must be cancelled (confirmed) before any market exit
            self.log(&ev, lots, "exit: cancelling the SL-M first");
            return self.flatten(&format!("{} at bar close", ev.kind.label()));
        }
        if !reduce_only && self.exiting && self.live.is_some() {
            // Same-bar flip: our closing order is still in flight at the broker. Wait for
            // its fill (guard timer / fill handler), never stack the entry on top of it.
            let deadline = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0) + PENDING_ENTRY_NS;
            self.log(&ev, lots, "entry deferred until the closing order fills");
            self.board(move |b| b.event(format!("{} waiting for exit fill before entering", ev.kind.label())));
            self.pending_entry = Some((ev, deadline));
            return Ok(());
        }
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
        if reduce_only && self.open_lots == 0 && self.live.is_some() {
            self.exiting = true; // closing order in flight; cleared when the position is flat
        }
        self.log(&ev, lots, "order submitted");
        let t = ev.trade.clone();
        self.board(move |b| {
            b.event(format!("{} {side:?} {lots} lot  (model {:.0}, SL {:.0}, TP1 {:.0})", ev.kind.label(), ev.fill, t.sl, t.tp1));
            if ev.kind.is_entry() {
                b.sl = Some(t.sl);
                b.tps = Some([t.tp1, t.tp2, t.tp3]);
            }
        });
        if ev.kind.is_entry() {
            self.entry_sl = Some((t.side, t.sl));
        }
        if ev.kind.is_entry() && self.trail_mode() {
            // breakeven switches to the real fill price once it is known
            self.trail = Some(Trail::new(t.side, t.entry, t.sl, t.tp1));
        } else if ev.kind.is_entry() && self.bracket_mode() {
            self.bracket = Some(Bracket::new(t.side, t.sl, self.execution.single_target(&t)));
        } else if self.open_lots == 0 {
            self.trail = None;
            self.bracket = None;
        }
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
        // configured entry window (IST, by bar close); exits are never blocked
        self.engine.set_entries_enabled(self.config.entries_allowed(input.close_time_ns));
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
            // historical warm-up: model state only, never orders — but show it at once
            self.show_model(close_ist, input.close, false);
            if input.close_time_ns >= start_ns {
                self.report_warm();
            }
            return Ok(());
        }
        self.report_warm();
        if at_or_after_cutoff && self.squared_off_on != Some(close_ist.date_naive()) {
            self.squared_off_on = Some(close_ist.date_naive());
            self.log_json(serde_json::json!({"event":"sats_square_off","instance":self.instance_id,"bar_close_ist":close_ist.format("%Y-%m-%d %H:%M").to_string()}));
            self.board(move |b| {
                b.status = "SQUARED OFF".into();
                b.event("Daily square-off: flattening, no new entries today".into());
            });
            self.flatten("daily square-off")?;
        }
        // Stream bars arrive ~1 s after close; a startup-gap bar backfilled from
        // Kite history arrives ~45-60 s after. Entries on anything older are refused.
        let late_ns = Self::now_ns() - input.close_time_ns;
        let too_late = late_ns > MAX_ENTRY_LATENESS_NS;
        if late_ns > BACKFILLED_NS {
            let secs = late_ns / 1_000_000_000;
            self.board(move |b| b.event(format!(
                "Backfilled bar {} from Kite history ({secs}s after close){}",
                close_ist.format("%H:%M"),
                if too_late { " · too late for an entry, exits only" } else { "" }
            )));
            self.log_json(serde_json::json!({"event":"sats_late_bar","instance":self.instance_id,
                "bar_close_ist":close_ist.format("%H:%M").to_string(),"late_s":secs,"entries_allowed":!too_late}));
        }
        let entries_allowed = !at_or_after_cutoff && !self.stopping() && self.halted.is_none() && !too_late;
        for ev in events {
            if at_or_after_cutoff && !ev.kind.is_entry() && self.live_entry_bar.is_none() {
                continue; // already flattened at square-off
            }
            self.handle(ev, entries_allowed)?;
        }
        if self.trail_mode() {
            self.trail_bar_close();
        }
        self.show_model(close_ist, input.close, true);
        let status = self.engine.status().clone();
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
            let (bid, ask) = (quote.bid_price.as_f64(), quote.ask_price.as_f64());
            let mid = (bid + ask) / 2.0;
            live.market_price.store(mid.round() as i64, Ordering::Release);
            self.board(move |b| b.last_price = Some(mid));
            if self.trail_mode() {
                self.check_trail(bid, ask)?;
            } else if self.bracket_mode() {
                self.check_bracket(bid, ask)?;
            }
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
                self.board(move |b| {
                    b.feed_fault = shown;
                    b.status = "STOPPING".into();
                });
                self.halt(&fault.unwrap_or_else(|| "stop requested".into()));
            }
            self.flatten("stop / feed fault")?;
            self.check_cancel_deadline(e.ts_event.as_u64() as i64);
        } else {
            self.try_pending(e.ts_event.as_u64() as i64)?;
            self.check_cancel_deadline(e.ts_event.as_u64() as i64);
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
        let side = e.order_side;
        self.board(move |b| {
            b.apply_fill(signed, px);
            b.event(format!("FILL {side:?} {qty:.0} @ {px:.0}"));
        });
        if self.stop_order == Some(e.client_order_id) {
            // the SL-M executed at the exchange
            self.stop_order = None;
            self.exit_after_cancel = None;
            self.board(move |b| {
                b.exchange_stop = None;
                b.event(format!("SL-M executed at Zerodha @ {px:.0}"));
            });
        }
        // entry fill: breakeven is measured from the real fill price
        if let Some(trail) = self.trail.as_mut()
            && !trail.active
            && (e.order_side == OrderSide::Buy) == (trail.side == Side::Long)
        {
            trail.entry = px;
        }
        if self.pending_entry.is_some() {
            // a same-bar flip entry was waiting for this closing fill
            let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
            if let Err(err) = self.try_pending(now) {
                self.halt(&format!("deferred entry failed: {err:#}"));
            }
        } else if self.position() == 0.0 {
            self.exiting = false;
            self.set_flat(true);
        }
        // entry filled: protect it at the broker
        if self.exchange_stop_mode()
            && self.position() != 0.0
            && self.stop_order.is_none()
            && self.entry_sl.is_some_and(|(side, _)| (e.order_side == OrderSide::Buy) == (side == Side::Long))
            && let Err(err) = self.place_exchange_stop()
        {
            self.exchange_stop_failed(&format!("submit failed: {err:#}"));
        }
    }
    fn on_order_canceled(&mut self, e: &OrderCanceled) {
        if self.stop_order != Some(e.client_order_id) {
            return;
        }
        self.stop_order = None;
        self.board(move |b| {
            b.exchange_stop = None;
            b.event("SL-M cancelled at Zerodha".into());
        });
        if let Some((why, _)) = self.exit_after_cancel.take() {
            self.exiting = false;
            if let Err(err) = self.flatten(&why) {
                self.halt(&format!("exit after SL-M cancel failed: {err:#}"));
            }
        }
    }
    fn on_order_cancel_rejected(&mut self, e: OrderCancelRejected) {
        if self.stop_order == Some(e.client_order_id) {
            // usually the SL-M has just triggered; its fill flattens the position
            self.board(move |b| b.event(format!("SL-M cancel refused ({}); waiting for its fill", e.reason)));
            self.log_json(serde_json::json!({"event":"sats_exchange_stop","instance":self.instance_id,"action":"cancel_rejected","reason":e.reason.to_string()}));
        }
    }
    fn on_order_rejected(&mut self, e: OrderRejected) {
        if self.stop_order == Some(e.client_order_id) {
            self.exchange_stop_failed(&format!("rejected: {}", e.reason));
            return;
        }
        self.exiting = false;
        self.halt(&format!("order rejected: {}", e.reason));
    }
    fn on_order_denied(&mut self, e: OrderDenied) {
        if self.stop_order == Some(e.client_order_id) {
            self.exchange_stop_failed(&format!("denied: {}", e.reason));
            return;
        }
        self.exiting = false;
        self.halt(&format!("order denied: {}", e.reason));
    }
});
