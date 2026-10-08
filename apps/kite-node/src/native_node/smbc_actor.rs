//! Nautilus actor for Smart Money Breakout Channels.
use super::smbc_strategy::{Engine, Observation, Settings};
use anyhow::Result;
use nautilus_common::{
    actor::{DataActor, DataActorNative},
    cache::Cache,
    timer::TimeEvent,
};
use nautilus_core::DurationNanos;
use nautilus_model::{
    data::{Bar, BarType, CustomData, DataType, QuoteTick},
    enums::{OrderSide, TimeInForce},
    events::{OrderCanceled, OrderDenied, OrderFilled, OrderRejected},
    identifiers::InstrumentId,
};
use nautilus_trading::{
    nautilus_strategy,
    strategy::{Strategy, StrategyConfig, StrategyCore},
};
use std::{cell::RefCell, rc::Rc, sync::atomic::Ordering};

#[derive(Default, Debug, Clone)]
pub struct TradeMonitor {
    pub side: i8,
    pub entry: Option<f64>,
    pub opened_ns: Option<u64>,
    pub best_price: Option<f64>,
    pub worst_price: Option<f64>,
}
impl TradeMonitor {
    fn update(&mut self, side: i8, entry: Option<f64>, price: f64, ts: u64) {
        if side != self.side {
            self.side = side;
            self.entry = if side != 0 {
                entry.or(Some(price))
            } else {
                None
            };
            self.opened_ns = (side != 0).then_some(ts);
            self.best_price = self.entry;
            self.worst_price = self.entry;
        } else if side != 0 && entry.is_some() {
            self.entry = entry;
        }
        if side > 0 {
            self.best_price = Some(self.best_price.map_or(price, |v| v.max(price)));
            self.worst_price = Some(self.worst_price.map_or(price, |v| v.min(price)));
        } else if side < 0 {
            self.best_price = Some(self.best_price.map_or(price, |v| v.min(price)));
            self.worst_price = Some(self.worst_price.map_or(price, |v| v.max(price)));
        } else {
            self.entry = None;
            self.opened_ns = None;
            self.best_price = None;
            self.worst_price = None;
        }
    }
}

#[derive(Default, Debug)]
pub struct State {
    pub cache: Option<Rc<RefCell<Cache>>>,
    pub live_quotes: u64,
    pub last_accepted_quote: Option<QuoteTick>,
    pub latest_strategy: Option<Observation>,
    pub latest_ohlc: Option<[f64; 4]>,
    pub trade_monitor: TradeMonitor,
    pub rejected_quotes: u64,
    pub rebuilds: Vec<serde_json::Value>,
    pub indicators: Vec<serde_json::Value>,
    pub signals: Vec<serde_json::Value>,
    pub fills: Vec<serde_json::Value>,
    pub errors: Vec<String>,
    pub started: bool,
    pub stopped: bool,
}

#[derive(Debug, Clone, Copy)]
struct Protection {
    side: i8,
    entry: f64,
    sl: f64,
    tp: Option<f64>,
    risk: f64,
    be_done: bool,
    trail_on: bool,
    entry_bar: u64,
}

#[derive(Debug, Clone, Copy)]
struct LiveCandle {
    open_ns: u64,
    open: f64,
    high: f64,
    low: f64,
    close: f64,
}
impl LiveCandle {
    fn new(open_ns: u64, p: f64) -> Self {
        Self {
            open_ns,
            open: p,
            high: p,
            low: p,
            close: p,
        }
    }
    fn update(&mut self, p: f64) {
        self.high = self.high.max(p);
        self.low = self.low.min(p);
        self.close = p;
    }
}

#[derive(Debug)]
pub struct BarStrategy {
    core: StrategyCore,
    engine: Engine,
    settings: Settings,
    bar_type: BarType,
    bar_ns: u64,
    start: u64,
    target: i8,
    target_reason: String,
    live: Option<super::live_control::Control>,
    last_bar: u64,
    pending: bool,
    live_candle: Option<LiveCandle>,
    last_observation: Option<Observation>,
    protection: Option<Protection>,
    cooldown_until_ns: u64,
    state: Rc<RefCell<State>>,
}
impl BarStrategy {
    pub fn new(
        bar_type: BarType,
        start: u64,
        _end: u64,
        state: Rc<RefCell<State>>,
        settings: Settings,
        calendar: super::session_calendar::Calendar,
        bar_ns: u64,
    ) -> Result<Self> {
        let engine = Engine::new(settings.clone(), calendar)?;
        Ok(Self {
            core: StrategyCore::new(StrategyConfig {
                strategy_id: Some("SMBC-001".into()),
                log_events: false,
                log_commands: false,
                ..Default::default()
            }),
            engine,
            settings,
            bar_type,
            bar_ns,
            start,
            target: 0,
            target_reason: String::new(),
            live: None,
            last_bar: 0,
            pending: false,
            live_candle: None,
            last_observation: None,
            protection: None,
            cooldown_until_ns: 0,
            state,
        })
    }
    pub fn with_live(mut self, control: super::live_control::Control) -> Self {
        self.live = Some(control);
        self
    }
    fn instrument(&self) -> InstrumentId {
        self.bar_type.instrument_id()
    }
    fn position_with_entry(&self) -> (f64, Option<f64>) {
        let positions = self.cache().positions_open(
            None,
            Some(&self.instrument()),
            self.strategy_id().as_ref(),
            None,
            None,
        );
        let signed = positions.iter().map(|p| p.signed_qty).sum::<f64>();
        let abs = positions.iter().map(|p| p.signed_qty.abs()).sum::<f64>();
        let entry = (abs > 0.0).then(|| {
            positions
                .iter()
                .map(|p| p.avg_px_open * p.signed_qty.abs())
                .sum::<f64>()
                / abs
        });
        (signed, entry)
    }
    fn position_side(&self) -> i8 {
        let p = self.position_with_entry().0;
        if p > 0.0 {
            1
        } else if p < 0.0 {
            -1
        } else {
            0
        }
    }

    fn initial_protection(&self, side: i8, entry: f64, bar: u64) -> Option<Protection> {
        let o = self.last_observation?;
        let atr = o.atr?;
        let source = if side > 0 {
            match self.settings.stop_mode.as_str() {
                "ATR" => entry - atr * self.settings.sl_atr_mult,
                "Candle Wick" => o
                    .wick_low
                    .unwrap_or(entry - atr * self.settings.sl_atr_mult),
                _ => o.wick_low.unwrap_or(entry) - atr * self.settings.wick_atr_buffer,
            }
        } else {
            match self.settings.stop_mode.as_str() {
                "ATR" => entry + atr * self.settings.sl_atr_mult,
                "Candle Wick" => o
                    .wick_high
                    .unwrap_or(entry + atr * self.settings.sl_atr_mult),
                _ => o.wick_high.unwrap_or(entry) + atr * self.settings.wick_atr_buffer,
            }
        };
        let raw = if side > 0 {
            entry - source
        } else {
            source - entry
        };
        let risk = self.settings.clamp_stop_distance(raw.max(0.0));
        if risk <= 0.0 {
            return None;
        }
        let sl = if side > 0 { entry - risk } else { entry + risk };
        let tp = match self.settings.target_mode.as_str() {
            "R:R" => Some(if side > 0 {
                entry
                    + self
                        .settings
                        .cap_target_distance(risk * self.settings.reward_risk)
            } else {
                entry
                    - self
                        .settings
                        .cap_target_distance(risk * self.settings.reward_risk)
            }),
            "ATR" => Some(if side > 0 {
                entry
                    + self
                        .settings
                        .cap_target_distance(atr * self.settings.tp_atr_mult)
            } else {
                entry
                    - self
                        .settings
                        .cap_target_distance(atr * self.settings.tp_atr_mult)
            }),
            _ => None,
        };
        Some(Protection {
            side,
            entry,
            sl,
            tp,
            risk,
            be_done: false,
            trail_on: false,
            entry_bar: bar,
        })
    }

    fn update_trailing(&mut self, o: Observation) {
        let Some(mut p) = self.protection else { return };
        if p.side != self.position_side() || o.bar_close_ns <= p.entry_bar || p.risk <= 0.0 {
            return;
        }
        let move_r = if p.side > 0 {
            (o.close - p.entry) / p.risk
        } else {
            (p.entry - o.close) / p.risk
        };
        if self.settings.breakeven_r > 0.0 && !p.be_done && move_r >= self.settings.breakeven_r {
            p.sl = if p.side > 0 {
                p.sl.max(p.entry)
            } else {
                p.sl.min(p.entry)
            };
            p.be_done = true;
        }
        if self.settings.trail_mode != "Off"
            && move_r >= self.settings.trail_activate_r
            && let Some(atr) = o.atr
        {
            let nt = if p.side > 0 {
                if self.settings.trail_mode == "ATR Trail" {
                    o.close - atr * self.settings.trail_atr_mult
                } else {
                    o.trail_low.unwrap_or(o.close) - atr * self.settings.wick_atr_buffer
                }
            } else if self.settings.trail_mode == "ATR Trail" {
                o.close + atr * self.settings.trail_atr_mult
            } else {
                o.trail_high.unwrap_or(o.close) + atr * self.settings.wick_atr_buffer
            };
            if (p.side > 0 && nt > p.sl) || (p.side < 0 && nt < p.sl) {
                p.trail_on = true;
            }
            p.sl = if p.side > 0 {
                p.sl.max(nt)
            } else {
                p.sl.min(nt)
            };
        }
        self.protection = Some(p);
    }

    fn price_exit_reason(&self, bid: f64, ask: f64) -> Option<&'static str> {
        let p = self.protection?;
        let (stop, tp) = if p.side > 0 {
            (bid <= p.sl, p.tp.is_some_and(|v| bid >= v))
        } else {
            (ask >= p.sl, p.tp.is_some_and(|v| ask <= v))
        };
        if stop {
            Some(if p.be_done && (p.sl - p.entry).abs() < 1e-9 {
                "BE"
            } else if p.trail_on {
                "TSL"
            } else {
                "SL"
            })
        } else if tp {
            Some("TP")
        } else {
            None
        }
    }
    fn bar_exit_reason(&self, bar: &Bar) -> Option<&'static str> {
        let p = self.protection?;
        let (stop, tp) = if p.side > 0 {
            (
                bar.low.as_f64() <= p.sl,
                p.tp.is_some_and(|v| bar.high.as_f64() >= v),
            )
        } else {
            (
                bar.high.as_f64() >= p.sl,
                p.tp.is_some_and(|v| bar.low.as_f64() <= v),
            )
        };
        if stop {
            Some(if p.be_done && (p.sl - p.entry).abs() < 1e-9 {
                "BE"
            } else if p.trail_on {
                "TSL"
            } else {
                "SL"
            })
        } else if tp {
            Some("TP")
        } else {
            None
        }
    }

    fn trade_target(&mut self, ts: u64) -> Result<()> {
        if self.pending {
            return Ok(());
        }
        let (position, _) = self.position_with_entry();
        if let Some(c) = &self.live {
            c.flat.store(position == 0.0, Ordering::Release);
        }
        let side_now = if position > 0.0 {
            1
        } else if position < 0.0 {
            -1
        } else {
            0
        };
        if side_now == self.target {
            return Ok(());
        }
        if side_now == 0 && self.target != 0 && ts < self.cooldown_until_ns {
            return Ok(());
        }
        let exit = position != 0.0;
        let side = if (exit && position > 0.0) || (!exit && self.target < 0) {
            OrderSide::Sell
        } else {
            OrderSide::Buy
        };
        let intent = match (exit, side) {
            (false, OrderSide::Buy) => "BUY",
            (false, _) => "SHORT",
            (true, OrderSide::Sell) => "SELL",
            (true, _) => "COVER",
        };
        let order = self.order().market(
            self.instrument(),
            side,
            1.into(),
            Some(TimeInForce::Day),
            Some(exit),
            None,
            None,
            None,
            None,
            None,
        );
        self.state.borrow_mut().signals.push(serde_json::json!({"timestamp_ns":ts,"intent":intent,"target":self.target,
            "position_before":position,"reason":self.target_reason,"bar_close_ns":self.last_bar,"confirmed_bar_only":self.target_reason!="SL"&&self.target_reason!="TP"&&self.target_reason!="BE"&&self.target_reason!="TSL"}));
        if let Some(c) = &self.live {
            c.flat.store(false, Ordering::Release);
            c.order_deadline
                .store(super::data::now() + 10_000_000_000, Ordering::Release);
        }
        self.pending = true;
        self.submit_order(order, None, None, None)?;
        Ok(())
    }
}

impl DataActor for BarStrategy {
    fn on_start(&mut self) -> Result<()> {
        self.state.borrow_mut().cache = Some(DataActorNative::cache_rc(self));
        let bar_client = self.live.as_ref().map(|_| "STBARS".into());
        let quote_client = self.live.as_ref().map(|c| {
            if c.sim {
                "STBARS".into()
            } else {
                "KITE".into()
            }
        });
        self.subscribe_bars(self.bar_type, bar_client, None);
        self.subscribe_quotes(self.instrument(), quote_client, None);
        if self.live.as_ref().is_some_and(|c| !c.sim) {
            self.subscribe_data(
                DataType::new("KiteFeedStatus", None, None),
                Some("KITE".into()),
                None,
            );
        }
        if self.live.is_some() {
            self.clock().set_timer_ns(
                "strategy_session_guard",
                DurationNanos::from_millis(250),
                None,
                None,
                None,
                None,
                None,
            )?;
        }
        self.state.borrow_mut().started = true;
        Ok(())
    }
    fn on_stop(&mut self) -> Result<()> {
        if self.live.is_some() {
            self.clock().cancel_timer("strategy_session_guard");
            self.cancel_all_orders(self.instrument(), None, None, true, None)?;
        }
        self.state.borrow_mut().stopped = true;
        Ok(())
    }
    fn on_time_event(&mut self, event: &TimeEvent) -> Result<()> {
        if event.name.as_str() != "strategy_session_guard" {
            return Ok(());
        }
        // Independent EOD guard: if the configured square-off time has passed, desire FLAT.
        if self.settings.force_flat_at_session_end && self.position_side() != 0 {
            use chrono::Timelike;
            let now = self.clock().timestamp_ns().as_u64();
            let dt = chrono::DateTime::from_timestamp_nanos(now as i64)
                .with_timezone(&chrono::FixedOffset::east_opt(19_800).expect("IST"));
            if dt.hour() * 60 + dt.minute()
                >= self.settings.auto_sq_off_hour * 60 + self.settings.auto_sq_off_minute
            {
                self.target = 0;
                self.target_reason = "EOD_TIMER".into();
                self.trade_target(now)?;
            }
        }
        Ok(())
    }
    fn on_data(&mut self, data: &CustomData) -> Result<()> {
        if let (Some(c), Some(status)) = (
            &self.live,
            data.data
                .as_any()
                .downcast_ref::<super::status::FeedStatus>(),
        ) {
            match status.kind.as_str() {
                "connected" => c.online.store(true, Ordering::Release),
                "gap" => {
                    c.online.store(false, Ordering::Release);
                    c.pause();
                }
                "failed" => c.fail("Kite quote reconnect attempts exhausted"),
                "complete" => c.stop(),
                _ => {}
            }
        }
        Ok(())
    }
    fn on_save(&self) -> Result<indexmap::IndexMap<String, Vec<u8>>> {
        let p = self.protection;
        let summary = serde_json::json!({"last_bar":self.last_bar,"target":self.target,"position":self.position_side(),
            "stop":p.map(|x|x.sl),"target_price":p.and_then(|x|x.tp),"cooldown_until_ns":self.cooldown_until_ns,"live_orders_enabled":self.live.as_ref().is_some_and(|c|c.real)});
        Ok(indexmap::IndexMap::from([(
            "smbc_state".into(),
            serde_json::to_vec(&summary)?,
        )]))
    }
    fn on_bar(&mut self, bar: &Bar) -> Result<()> {
        if let Some(c) = &self.live
            && (bar.ts_event.as_u64() <= self.last_bar
                || (!c.sim && bar.ts_event.as_u64() > self.clock().timestamp_ns().as_u64()))
        {
            c.fail("Out-of-order or future bar");
            return Ok(());
        }
        let close_ns = bar.ts_event.as_u64();
        let warming = close_ns <= self.start;
        let o = self.engine.update_confirmed(
            bar.open.as_f64(),
            bar.high.as_f64(),
            bar.low.as_f64(),
            bar.close.as_f64(),
            close_ns,
            self.bar_ns,
        )?;
        self.last_bar = close_ns;
        self.last_observation = Some(o);
        if self
            .live_candle
            .is_some_and(|c| c.open_ns + self.bar_ns <= close_ns)
        {
            self.live_candle = None;
        }
        {
            let mut s = self.state.borrow_mut();
            s.latest_strategy = Some(o);
            s.latest_ohlc = Some([
                bar.open.as_f64(),
                bar.high.as_f64(),
                bar.low.as_f64(),
                bar.close.as_f64(),
            ]);
            s.indicators.push(serde_json::to_value(o)?);
        }
        if warming {
            return Ok(());
        }
        if self.pending {
            return Ok(());
        }
        let pos = self.position_side();
        if pos != 0 {
            if let Some(reason) = self.bar_exit_reason(bar) {
                self.target = 0;
                self.target_reason = reason.into();
            } else if o.eod_hit {
                self.target = 0;
                self.target_reason = "EOD".into();
            } else if pos > 0 && o.bearish_breakout && self.settings.opposite_breakout != "Ignore" {
                self.target = if self.settings.opposite_breakout == "Reverse"
                    && self.settings.enable_shorts
                {
                    -1
                } else {
                    0
                };
                self.target_reason = "REV".into();
            } else if pos < 0 && o.bullish_breakout && self.settings.opposite_breakout != "Ignore" {
                self.target =
                    if self.settings.opposite_breakout == "Reverse" && self.settings.enable_longs {
                        1
                    } else {
                        0
                    };
                self.target_reason = "REV".into();
            } else {
                self.target = pos;
                self.update_trailing(o);
            }
        } else if o.can_enter {
            if o.bullish_breakout && self.settings.enable_longs {
                self.target = 1;
                self.target_reason = "BREAKOUT".into();
            } else if o.bearish_breakout && self.settings.enable_shorts {
                self.target = -1;
                self.target_reason = "BREAKOUT".into();
            } else {
                self.target = 0;
            }
        } else {
            self.target = 0;
        }
        Ok(())
    }
    fn on_quote(&mut self, quote: &QuoteTick) -> Result<()> {
        let ts = quote.ts_event.as_u64();
        if let Some(c) = &self.live {
            self.state.borrow_mut().live_quotes += 1;
            let now = self.clock().timestamp_ns().as_u64();
            if !c.fresh_quote(ts, quote.ts_init.as_u64(), now)
                || quote.bid_price.as_f64() <= 0.0
                || quote.ask_price < quote.bid_price
            {
                self.state.borrow_mut().rejected_quotes += 1;
                return Ok(());
            }
            self.state.borrow_mut().last_accepted_quote = Some(*quote);
        }
        let mid = (quote.bid_price.as_f64() + quote.ask_price.as_f64()) * 0.5;
        let (position, entry) = self.position_with_entry();
        let side = if position > 0.0 {
            1
        } else if position < 0.0 {
            -1
        } else {
            0
        };
        self.state
            .borrow_mut()
            .trade_monitor
            .update(side, entry, mid, ts);
        let open_ns = ts / self.bar_ns * self.bar_ns;
        match self.live_candle.as_mut() {
            Some(c) if c.open_ns == open_ns => c.update(mid),
            _ => self.live_candle = Some(LiveCandle::new(open_ns, mid)),
        }
        if let Some(candle) = self.live_candle {
            let preview = self
                .engine
                .preview_live(candle.close, candle.open_ns + self.bar_ns);
            let mut s = self.state.borrow_mut();
            s.latest_strategy = Some(preview);
            s.latest_ohlc = Some([candle.open, candle.high, candle.low, candle.close]);
        }
        if let Some(c) = self.live.clone() {
            let rebuild = c.rebuild.lock().expect("rebuild lock").take();
            if let Some((epoch, bars)) = rebuild
                && epoch == c.epoch.load(Ordering::Acquire)
                && c.online.load(Ordering::Acquire)
            {
                self.engine = self.engine.rebuild_empty()?;
                self.last_bar = 0;
                self.live_candle = None;
                self.last_observation = None;
                self.protection = None;
                self.cooldown_until_ns = 0;
                self.target = self.position_side();
                {
                    let mut s = self.state.borrow_mut();
                    s.indicators.clear();
                    s.latest_strategy = None;
                    s.latest_ohlc = None;
                }
                for b in bars {
                    self.on_bar(&b)?
                }
                self.target = self.position_side();
                self.target_reason.clear();
                self.state.borrow_mut().rebuilds.push(serde_json::json!({"epoch":epoch,"rebuilt_bar":self.last_bar,"past_orders_replayed":false}));
                c.recoveries.fetch_add(1, Ordering::AcqRel);
                if epoch == c.epoch.load(Ordering::Acquire) {
                    c.paused.store(false, Ordering::Release);
                }
            }
            if c.paused.load(Ordering::Acquire) && !c.stopping.load(Ordering::Acquire) {
                return Ok(());
            }
        }
        if self.settings.intrabar_exit
            && !self.pending
            && side != 0
            && let Some(reason) =
                self.price_exit_reason(quote.bid_price.as_f64(), quote.ask_price.as_f64())
        {
            self.target = 0;
            self.target_reason = reason.into();
        }
        if ts < self.start || self.pending {
            return Ok(());
        }
        self.trade_target(ts)
    }
}

nautilus_strategy!(BarStrategy, {
    fn on_order_filled(&mut self, event: &OrderFilled) {
        self.pending = false;
        if let Some(c) = &self.live {
            c.order_deadline.store(0, Ordering::Release);
            c.flat.store(self.position_side() == 0, Ordering::Release);
        }
        let (position, entry) = self.position_with_entry();
        let side = if position > 0.0 {
            1
        } else if position < 0.0 {
            -1
        } else {
            0
        };
        if side == 0 {
            self.protection = None;
            self.cooldown_until_ns = event
                .ts_event
                .as_u64()
                .saturating_add(self.settings.entry_cooldown_bars as u64 * self.bar_ns);
        } else if let Some(entry) = entry {
            self.protection = self.initial_protection(side, entry, self.last_bar);
        }
        let reason = self.target_reason.clone();
        self.state.borrow_mut().fills.push(serde_json::json!({"timestamp_ns":event.ts_event.as_u64(),
            "client_order_id":event.client_order_id.to_string(),"side":event.order_side.to_string(),"quantity":event.last_qty.to_string(),
            "price":event.last_px.to_string(),"reason":reason,"position_after":position,"stop":self.protection.map(|p|p.sl),"target_price":self.protection.and_then(|p|p.tp)}));
        self.state.borrow_mut().trade_monitor.update(
            side,
            entry,
            event.last_px.as_f64(),
            event.ts_event.as_u64(),
        );
        if side == self.target {
            self.target_reason.clear();
        }
    }
    fn on_order_rejected(&mut self, event: OrderRejected) {
        self.pending = false;
        self.state
            .borrow_mut()
            .errors
            .push(format!("order rejected: {}", event.reason));
        if let Some(c) = &self.live {
            c.fail("SMBC order rejected");
        }
    }
    fn on_order_denied(&mut self, event: OrderDenied) {
        self.pending = false;
        self.state
            .borrow_mut()
            .errors
            .push(format!("order denied: {}", event.reason));
        if let Some(c) = &self.live {
            c.fail("SMBC order denied");
        }
    }
    fn on_order_canceled(&mut self, _event: &OrderCanceled) {
        self.pending = false;
    }
});
