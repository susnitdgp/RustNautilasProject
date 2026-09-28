//! Nautilus actor for the confirmed-bar pure LazyBear SQZ strategy.
use anyhow::Result;
use nautilus_common::{
    actor::{DataActor, DataActorNative},
    cache::Cache,
    timer::TimeEvent,
};
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
use std::sync::atomic::Ordering;
use std::{cell::RefCell, rc::Rc};

use super::squeeze_momentum_strategy::{Engine, Observation, Settings};

#[derive(Default, Debug, Clone)]
pub struct TradeMonitor {
    pub side: i8,
    pub entry: Option<f64>,
    pub opened_ns: Option<u64>,
    pub best_price: Option<f64>,
    pub worst_price: Option<f64>,
}

impl TradeMonitor {
    fn update(&mut self, side: i8, entry: Option<f64>, price: f64, ts_ns: u64) {
        if side != self.side {
            self.side = side;
            self.entry = (side != 0)
                .then_some(entry)
                .flatten()
                .or((side != 0).then_some(price));
            self.opened_ns = (side != 0).then_some(ts_ns);
            self.best_price = self.entry;
            self.worst_price = self.entry;
        } else if side != 0
            && let Some(entry) = entry
        {
            self.entry = Some(entry);
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

    pub fn open_points(&self, price: f64) -> Option<f64> {
        self.entry.map(|entry| {
            if self.side > 0 {
                price - entry
            } else {
                entry - price
            }
        })
    }
}

#[derive(Default, Debug)]
pub struct State {
    pub cache: Option<Rc<RefCell<Cache>>>,
    pub live_quotes: u64,
    pub last_accepted_quote: Option<QuoteTick>,
    pub latest_strategy: Option<Observation>,
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
struct LiveCandle {
    open_ns: u64,
    high: f64,
    low: f64,
    close: f64,
}

impl LiveCandle {
    fn new(open_ns: u64, price: f64) -> Self {
        Self {
            open_ns,
            high: price,
            low: price,
            close: price,
        }
    }
    fn update(&mut self, price: f64) {
        self.high = self.high.max(price);
        self.low = self.low.min(price);
        self.close = price;
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
    end: u64,
    target: i8,
    live: Option<super::live_control::Control>,
    last_bar: u64,
    pending: bool,
    target_reason: Option<&'static str>,
    history_position: i8,
    live_candle: Option<LiveCandle>,
    state: Rc<RefCell<State>>,
}

impl BarStrategy {
    pub fn new(
        bar_type: BarType,
        start: u64,
        end: u64,
        state: Rc<RefCell<State>>,
        settings: Settings,
        calendar: super::session_calendar::Calendar,
        bar_ns: u64,
    ) -> Result<Self> {
        let engine = Engine::new(settings.clone(), calendar)?;
        Ok(Self {
            core: StrategyCore::new(StrategyConfig {
                strategy_id: Some("SQZ-MOMENTUM-001".into()),
                log_events: false,
                log_commands: false,
                ..Default::default()
            }),
            engine,
            settings,
            bar_type,
            bar_ns,
            start,
            end,
            target: 0,
            live: None,
            last_bar: 0,
            pending: false,
            target_reason: None,
            history_position: 0,
            live_candle: None,
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
        let signed_qty = positions.iter().map(|p| p.signed_qty).sum::<f64>();
        let abs_qty = positions.iter().map(|p| p.signed_qty.abs()).sum::<f64>();
        let entry = (abs_qty > 0.0).then(|| {
            positions
                .iter()
                .map(|p| p.avg_px_open * p.signed_qty.abs())
                .sum::<f64>()
                / abs_qty
        });
        (signed_qty, entry)
    }

    fn position(&self) -> f64 {
        self.position_with_entry().0
    }

    fn position_side(&self) -> i8 {
        let p = self.position();
        if p > 0.0 {
            1
        } else if p < 0.0 {
            -1
        } else {
            0
        }
    }

    fn trade_target(&mut self, target: i8, ts: u64, reason: &str) -> Result<()> {
        if self.pending {
            return Ok(());
        }
        let position = self.position();
        if let Some(control) = &self.live {
            control.flat.store(position == 0.0, Ordering::Release);
        }
        if position == f64::from(target) {
            self.target_reason = None;
            return Ok(());
        }
        let exit = position != 0.0;
        let side = if (exit && position > 0.0) || (!exit && target < 0) {
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
        self.state.borrow_mut().signals.push(serde_json::json!({
            "timestamp_ns":ts,
            "intent":intent,
            "target":target,
            "position_before":position,
            "reason":reason,
            "bar_close_ns":self.last_bar,
            "confirmed_bar_only":true
        }));
        if let Some(control) = &self.live {
            control.flat.store(false, Ordering::Release);
            control
                .order_deadline
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
        let quote_client = self.live.as_ref().map(|control| {
            if control.sim {
                "STBARS".into()
            } else {
                "KITE".into()
            }
        });
        self.subscribe_bars(self.bar_type, bar_client, None);
        self.subscribe_quotes(self.instrument(), quote_client, None);
        if self.live.as_ref().is_some_and(|control| !control.sim) {
            self.subscribe_data(
                DataType::new("KiteFeedStatus", None, None),
                Some("KITE".into()),
                None,
            );
        }
        if self.live.is_some() {
            self.clock().set_timer_ns(
                "strategy_session_guard",
                250_000_000,
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
        // v2.28.3 square-off is decided only by the confirmed 23:15 candle.
        // The timer never creates a trade action.
        Ok(())
    }

    fn on_data(&mut self, data: &CustomData) -> Result<()> {
        if let (Some(control), Some(status)) = (
            &self.live,
            data.data
                .as_any()
                .downcast_ref::<super::status::FeedStatus>(),
        ) {
            match status.kind.as_str() {
                "connected" => control.online.store(true, Ordering::Release),
                "gap" => {
                    control.online.store(false, Ordering::Release);
                    control.pause();
                }
                "failed" => control.fail("Kite quote reconnect attempts exhausted"),
                "complete" => control.stop(),
                _ => {}
            }
        }
        Ok(())
    }

    fn on_save(&self) -> Result<indexmap::IndexMap<String, Vec<u8>>> {
        let summary = serde_json::json!({
            "last_bar":self.last_bar,
            "runtime_end_ns":self.end,
            "target":self.target,
            "position":self.position(),
            "force_flat_at_session_end":self.settings.force_flat_at_session_end,
            "confirmed_bar_only":true,
            "live_orders_enabled":self.live.as_ref().is_some_and(|control|control.real)
        });
        Ok(indexmap::IndexMap::from([(
            "squeeze_momentum_state".into(),
            serde_json::to_vec(&summary)?,
        )]))
    }

    fn on_bar(&mut self, bar: &Bar) -> Result<()> {
        if let Some(control) = &self.live
            && (bar.ts_event.as_u64() <= self.last_bar
                || (!control.sim && bar.ts_event.as_u64() > self.clock().timestamp_ns().as_u64()))
        {
            control.fail("Out-of-order or future bar");
            return Ok(());
        }
        let bar_close_ns = bar.ts_event.as_u64();
        let warming = bar_close_ns <= self.start;
        let position = if warming {
            self.history_position
        } else {
            self.position_side()
        };
        let observation = self.engine.update_confirmed(
            bar.high.as_f64(),
            bar.low.as_f64(),
            bar.close.as_f64(),
            bar_close_ns,
            self.bar_ns,
            position,
            warming || !self.pending,
        )?;
        self.last_bar = bar_close_ns;
        if self
            .live_candle
            .is_some_and(|c| c.open_ns + self.bar_ns <= bar_close_ns)
        {
            self.live_candle = None;
        }
        self.state.borrow_mut().latest_strategy = Some(observation);
        self.state
            .borrow_mut()
            .indicators
            .push(serde_json::to_value(observation)?);

        if warming {
            if let Some(action) = observation.action {
                self.history_position = action.target();
            }
            return Ok(());
        }

        if let Some(action) = observation.action {
            self.target = action.target();
            self.target_reason = Some(if observation.force_flat_event {
                "session_force_flat"
            } else if observation.exit_zero_cross {
                "zero_cross"
            } else if matches!(
                action,
                super::squeeze_momentum_strategy::Action::Sell
                    | super::squeeze_momentum_strategy::Action::Cover
            ) {
                "sqz_transition"
            } else {
                action.reason()
            });
        }
        Ok(())
    }

    fn on_quote(&mut self, quote: &QuoteTick) -> Result<()> {
        let ts = quote.ts_event.as_u64();
        if let Some(control) = &self.live {
            self.state.borrow_mut().live_quotes += 1;
            let now = self.clock().timestamp_ns().as_u64();
            if !control.fresh_quote(ts, quote.ts_init.as_u64(), now)
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

        // Tick-by-tick dashboard preview only. This never mutates strategy state.
        let open_ns = ts / self.bar_ns * self.bar_ns;
        match self.live_candle.as_mut() {
            Some(candle) if candle.open_ns == open_ns => candle.update(mid),
            _ => self.live_candle = Some(LiveCandle::new(open_ns, mid)),
        }
        if let Some(candle) = self.live_candle {
            let preview = self.engine.preview_live(
                candle.high,
                candle.low,
                candle.close,
                candle.open_ns + self.bar_ns,
                self.bar_ns,
                side,
            )?;
            self.state.borrow_mut().latest_strategy = Some(preview);
        }

        if let Some(control) = self.live.clone() {
            let rebuild = control.rebuild.lock().expect("rebuild lock").take();
            if let Some((epoch, bars)) = rebuild
                && epoch == control.epoch.load(Ordering::Acquire)
                && control.online.load(Ordering::Acquire)
            {
                let previous = self.last_bar;
                self.engine = self.engine.rebuild_empty()?;
                self.last_bar = 0;
                self.history_position = 0;
                self.live_candle = None;
                self.target = self.position_side();
                {
                    let mut state = self.state.borrow_mut();
                    state.indicators.clear();
                    state.latest_strategy = None;
                }
                for bar in bars {
                    self.on_bar(&bar)?;
                }
                self.state.borrow_mut().rebuilds.push(serde_json::json!({
                    "epoch":epoch,
                    "previous_bar":previous,
                    "rebuilt_bar":self.last_bar,
                    "received_ns":quote.ts_init.as_u64(),
                    "past_orders_replayed":false
                }));
                control.recoveries.fetch_add(1, Ordering::AcqRel);
                if epoch == control.epoch.load(Ordering::Acquire) {
                    control.paused.store(false, Ordering::Release);
                }
            }
            if control.paused.load(Ordering::Acquire) && !control.stopping.load(Ordering::Acquire) {
                return Ok(());
            }
        }

        if ts < self.start || self.pending {
            return Ok(());
        }
        let reason = self.target_reason.unwrap_or("sqz_hold");
        self.trade_target(self.target, ts, reason)?;
        Ok(())
    }
}

nautilus_strategy!(BarStrategy, {
    fn on_order_filled(&mut self, event: &OrderFilled) {
        self.pending = false;
        if let Some(control) = &self.live {
            control.order_deadline.store(0, Ordering::Release);
            control
                .flat
                .store(self.position() == 0.0, Ordering::Release);
        }
        let (position_after, entry_price) = self.position_with_entry();
        let position_side = if position_after > 0.0 {
            1
        } else if position_after < 0.0 {
            -1
        } else {
            0
        };
        let reason = self.target_reason.unwrap_or("sqz_hold");
        let mut state = self.state.borrow_mut();
        state.trade_monitor.update(
            position_side,
            entry_price,
            event.last_px.as_f64(),
            event.ts_event.as_u64(),
        );
        state.fills.push(serde_json::json!({
            "instrument_id":event.instrument_id.to_string(),
            "timestamp_ns":event.ts_event.as_u64(),
            "client_order_id":event.client_order_id.to_string(),
            "side":event.order_side.to_string(),
            "quantity":event.last_qty.to_string(),
            "price":event.last_px.to_string(),
            "commission":event.commission.map(|value|value.to_string()),
            "reason":reason,
            "position_after":position_after
        }));
        self.target_reason = None;
    }

    fn on_order_rejected(&mut self, event: OrderRejected) {
        self.pending = false;
        self.state
            .borrow_mut()
            .errors
            .push(format!("order rejected: {}", event.reason));
        if let Some(control) = &self.live {
            control.fail("SQZ order rejected");
        }
    }
    fn on_order_denied(&mut self, event: OrderDenied) {
        self.pending = false;
        self.state
            .borrow_mut()
            .errors
            .push(format!("order denied: {}", event.reason));
        if let Some(control) = &self.live {
            control.fail("SQZ order denied");
        }
    }
    fn on_order_canceled(&mut self, _event: &OrderCanceled) {
        self.pending = false;
    }
});
