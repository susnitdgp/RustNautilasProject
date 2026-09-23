//! Shared confirmed bar strategy: BacktestNode or paper-only LiveNode.
use super::supertrend::Supertrend;
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
#[derive(Default, Debug)]
pub struct State {
    pub cache: Option<Rc<RefCell<Cache>>>,
    pub live_quotes: u64,
    pub last_accepted_quote: Option<QuoteTick>,
    pub rejected_quotes: u64,
    pub blocked_direction: i8,
    pub rebuilds: Vec<serde_json::Value>,
    pub indicators: Vec<serde_json::Value>,
    pub signals: Vec<serde_json::Value>,
    pub fills: Vec<serde_json::Value>,
    pub errors: Vec<String>,
    pub started: bool,
    pub stopped: bool,
}
#[derive(Debug)]
pub struct BarStrategy {
    core: StrategyCore,
    indicator: Supertrend,
    pivot: Option<super::pivot_point::PivotPoint>,
    ribbon: Option<super::trend_ribbon::TrendRibbon>,
    bar_type: BarType,
    start: u64,
    end: u64,
    target: i8,
    live: Option<super::supertrend_live_control::Control>,
    confirmation: super::supertrend_confirmation::Confirmation,
    filtered: bool,
    allowed: i8,
    last_bar: u64,
    pending: bool,
    state: Rc<RefCell<State>>,
}
impl BarStrategy {
    pub fn new(bar_type: BarType, start: u64, end: u64, state: Rc<RefCell<State>>) -> Self {
        Self {
            core: StrategyCore::new(StrategyConfig {
                strategy_id: Some("SUPERTREND-001".into()),
                log_events: false,
                log_commands: false,
                ..Default::default()
            }),
            indicator: Supertrend::new(),
            pivot: None,
            ribbon: None,
            bar_type,
            start,
            end,
            target: 0,
            live: None,
            confirmation: super::supertrend_confirmation::Confirmation::new(),
            filtered: false,
            allowed: 0,
            last_bar: 0,
            pending: false,
            state,
        }
    }
    pub fn with_pivot(
        mut self,
        settings: super::pivot_point::Settings,
        calendar: super::session_calendar::Calendar,
    ) -> Result<Self> {
        self.pivot = Some(super::pivot_point::PivotPoint::new(settings, calendar)?);
        self.filtered = false;
        Ok(self)
    }
    pub fn with_ribbon_interval(
        mut self,
        settings: super::trend_ribbon::Settings,
        calendar: super::session_calendar::Calendar,
        bar_ns: u64,
    ) -> Result<Self> {
        self.ribbon = Some(super::trend_ribbon::TrendRibbon::new_for_interval(
            settings, calendar, bar_ns,
        )?);
        self.filtered = false;
        Ok(self)
    }
    pub fn with_confirmation(mut self, filtered: bool) -> Self {
        self.filtered = filtered;
        self
    }
    pub fn with_live(mut self, control: super::supertrend_live_control::Control) -> Self {
        self.live = Some(control);
        self
    }
    fn trade_target(&mut self, target: i8, ts: u64, reason: &str) -> Result<()> {
        if self.pending {
            return Ok(());
        }
        let position = self.position();
        if let Some(control) = &self.live {
            control.flat.store(position == 0., Ordering::Release);
        }
        if position == f64::from(target) {
            return Ok(());
        }
        let exit = position != 0.;
        // Confirm only entries: loss of confirmation never blocks a reversal exit.
        // No prior-day VWAP may authorize an entry at the new session open.
        if !exit && self.filtered && (self.allowed != target || self.last_bar <= self.start) {
            return Ok(());
        }
        let side = if (exit && position > 0.) || (!exit && target < 0) {
            OrderSide::Sell
        } else {
            OrderSide::Buy
        };
        let intent = match (exit, side) {
            (false, OrderSide::Buy) => "BUY",
            (false, _) => "SELL",
            (true, OrderSide::Sell) => "BUY_EXIT",
            (true, _) => "SELL_EXIT",
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
        self.state.borrow_mut().signals.push(serde_json::json!({"timestamp_ns":ts,"intent":intent,"target":target,"position_before":position,"entry_filtered":self.filtered,"reason":reason,"bar_close_ns":self.last_bar,"bar_to_signal_ms":ts.saturating_sub(self.last_bar)/1_000_000}));
        if let Some(control) = &self.live {
            control.flat.store(false, Ordering::Release);
        }
        if let Some(c) = &self.live {
            c.order_deadline
                .store(super::data::now() + 10_000_000_000, Ordering::Release);
        }
        self.pending = true;
        self.submit_order(order, None, None, None)?;
        Ok(())
    }
    fn instrument(&self) -> InstrumentId {
        self.bar_type.instrument_id()
    }
    fn position(&self) -> f64 {
        self.cache()
            .positions_open(
                None,
                Some(&self.instrument()),
                self.strategy_id().as_ref(),
                None,
                None,
            )
            .iter()
            .map(|p| p.signed_qty)
            .sum()
    }
}
impl DataActor for BarStrategy {
    fn on_start(&mut self) -> Result<()> {
        self.state.borrow_mut().cache = Some(DataActorNative::cache_rc(self));
        let client = self.live.as_ref().map(|_| "STBARS".into());
        let quotes = self.live.as_ref().map(|c| {
            if c.sim {
                "STBARS".into()
            } else {
                "KITE".into()
            }
        });
        self.subscribe_bars(self.bar_type, client, None);
        self.subscribe_quotes(self.instrument(), quotes, None);
        if self.live.as_ref().is_some_and(|c| !c.sim) {
            self.subscribe_data(
                DataType::new("KiteFeedStatus", None, None),
                Some("KITE".into()),
                None,
            );
        }
        if self.pivot.is_some() && self.live.is_some() {
            self.clock().set_timer_ns(
                "pivot_square_off",
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
        if self.pivot.is_some() {
            self.clock().cancel_timer("pivot_square_off");
        }
        if self.live.is_some() {
            self.cancel_all_orders(self.instrument(), None, None, true, None)?;
        }
        self.state.borrow_mut().stopped = true;
        Ok(())
    }
    fn on_time_event(&mut self, event: &TimeEvent) -> Result<()> {
        if event.name.as_str() != "pivot_square_off" {
            return Ok(());
        }
        let now = self.clock().timestamp_ns().as_u64();
        let stopping = self
            .live
            .as_ref()
            .is_some_and(|c| c.stopping.load(Ordering::Acquire));
        if now >= self.end || stopping {
            self.target = 0;
            self.trade_target(
                0,
                now,
                if now >= self.end {
                    "session_end"
                } else {
                    "shutdown"
                },
            )?;
        }
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
                "connected" => {
                    control.online.store(true, Ordering::Release);
                }
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
        let summary = serde_json::json!({"last_bar":self.last_bar,"direction":self.target,"allowed":self.allowed,"position":self.position(),"automatic_resume_enabled":false,"paper_only":self.live.is_some(),"live_orders_enabled":self.live.as_ref().is_some_and(|c|c.real)});
        Ok(indexmap::IndexMap::from([(
            "supertrend_state".into(),
            serde_json::to_vec(&summary)?,
        )]))
    }
    fn on_bar(&mut self, b: &Bar) -> Result<()> {
        if let Some(control) = &self.live
            && (b.ts_event.as_u64() <= self.last_bar
                || (!control.sim && b.ts_event.as_u64() > self.clock().timestamp_ns().as_u64()))
        {
            control.fail("Out-of-order or future bar");
            return Ok(());
        }
        if let Some(ribbon) = &mut self.ribbon {
            let observation = ribbon.update(
                b.high.as_f64(),
                b.low.as_f64(),
                b.close.as_f64(),
                b.ts_event.as_u64(),
            )?;
            if !observation.in_session {
                self.target = 0;
            }
            if b.ts_event.as_u64() > self.start && observation.signal != 0 {
                self.target = observation.signal;
            }
            self.last_bar = b.ts_event.as_u64();
            self.state
                .borrow_mut()
                .indicators
                .push(serde_json::to_value(observation)?);
            return Ok(());
        }
        if let Some(pivot) = &mut self.pivot {
            let observation = pivot.update(
                b.high.as_f64(),
                b.low.as_f64(),
                b.close.as_f64(),
                b.ts_event.as_u64(),
            )?;
            // Historical warmup never becomes an entry instruction. A fresh +/-
            // transition after startup is required, as in the supplied Pine alerts.
            if !observation.in_session || observation.new_session {
                self.target = 0;
            }
            if b.ts_event.as_u64() > self.start && observation.signal != 0 {
                self.target = observation.signal;
            }
            self.last_bar = b.ts_event.as_u64();
            self.state
                .borrow_mut()
                .indicators
                .push(serde_json::to_value(observation)?);
            return Ok(());
        }
        let result = self
            .indicator
            .update(b.high.as_f64(), b.low.as_f64(), b.close.as_f64());
        if let Some((direction, _)) = result {
            self.target = direction;
        }
        let (allowed, confirmation) = self.confirmation.update(
            b.high.as_f64(),
            b.low.as_f64(),
            b.close.as_f64(),
            b.volume.as_f64(),
            b.ts_event.as_u64(),
        );
        self.allowed = allowed;
        self.last_bar = b.ts_event.as_u64();
        self.state.borrow_mut().indicators.push(serde_json::json!({
            "bar_close_ns":b.ts_event.as_u64(),"close":b.close.as_f64(),"atr":self.indicator.atr.value,
            "confirmation":confirmation,"initialized":self.indicator.atr.initialized,"direction":result.map(|x|x.0),"supertrend":result.map(|x|x.1)
        }));
        Ok(())
    }
    fn on_quote(&mut self, q: &QuoteTick) -> Result<()> {
        let ts = q.ts_event.as_u64();
        if let Some(control) = &self.live {
            self.state.borrow_mut().live_quotes += 1;
            let now = self.clock().timestamp_ns().as_u64();
            if !control.fresh_quote(ts, q.ts_init.as_u64(), now)
                || q.bid_price.as_f64() <= 0.
                || q.ask_price < q.bid_price
            {
                self.state.borrow_mut().rejected_quotes += 1;
                return Ok(());
            }
        }
        if self.live.is_some() {
            self.state.borrow_mut().last_accepted_quote = Some(*q);
        }
        if let Some(control) = self.live.clone() {
            let rebuild = control.rebuild.lock().expect("rebuild lock").take();
            if let Some((epoch, bars)) = rebuild
                && epoch == control.epoch.load(Ordering::Acquire)
                && control.online.load(Ordering::Acquire)
            {
                let previous = self.last_bar;
                self.indicator = Supertrend::new();
                if let Some(pivot) = &self.pivot {
                    self.pivot = Some(pivot.rebuild_empty()?);
                }
                if let Some(ribbon) = &self.ribbon {
                    self.ribbon = Some(ribbon.rebuild_empty()?);
                }
                self.confirmation = super::supertrend_confirmation::Confirmation::new();
                self.last_bar = 0;
                self.target = 0;
                self.allowed = 0;
                self.state.borrow_mut().indicators.clear();
                for bar in bars {
                    self.on_bar(&bar)?;
                }
                if self.pivot.is_some() || self.ribbon.is_some() {
                    // A corrected history can require an exit, but cannot replay an
                    // old entry. Keep only an existing position that still agrees.
                    let position = self.position();
                    let latest = self.state.borrow().indicators.last().cloned();
                    let direction = latest
                        .as_ref()
                        .and_then(|v| v["direction"].as_i64())
                        .unwrap_or(0);
                    let inside = latest.as_ref().is_some_and(|v| v["in_session"] == true);
                    // An empty position sum is -0.0, whose signum is -1.0.
                    // Neither signed zero is an open position to preserve.
                    self.target =
                        if inside && position != 0. && position.signum() == direction as f64 {
                            direction as i8
                        } else {
                            0
                        };
                }
                self.state.borrow_mut().rebuilds.push(serde_json::json!({"epoch":epoch,"previous_bar":previous,"rebuilt_bar":self.last_bar,"received_ns":q.ts_init.as_u64(),"past_orders_replayed":false,"reason":control.bar_feed.lock().expect("bar feed stats").last_rebuild_reason.clone()}));
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
        let stopping = self
            .live
            .as_ref()
            .is_some_and(|c| c.stopping.load(Ordering::Acquire));
        let target = if ts >= self.end || stopping {
            0
        } else {
            self.target
        };
        if let Some(control) = &self.live
            && !stopping
            && ts < self.end
            && (!control.current_bar(self.last_bar, self.clock().timestamp_ns().as_u64())
                || ts <= self.last_bar)
        {
            return Ok(());
        }
        let session_clock = if self.live.as_ref().is_some_and(|c| c.sim) {
            self.last_bar
        } else {
            ts
        };
        let session_closed = if let Some(pivot) = &self.pivot {
            !pivot.in_session(session_clock)?
        } else if let Some(ribbon) = &self.ribbon {
            !ribbon.in_session(session_clock)?
        } else {
            false
        };
        let target = if session_closed { 0 } else { target };
        let reason = if (self.pivot.is_some() || self.ribbon.is_some())
            && (session_closed || ts >= self.end)
        {
            "session_end"
        } else if stopping {
            "shutdown"
        } else if ts >= self.end {
            "end_of_day"
        } else if self.pivot.is_some() {
            "pivot_supertrend"
        } else if self.ribbon.is_some() {
            "trend_ribbon"
        } else {
            "supertrend"
        };
        self.trade_target(target, ts, reason)?;
        Ok(())
    }
}
nautilus_strategy!(BarStrategy, {
    fn on_order_filled(&mut self, event: &OrderFilled) {
        self.pending = false;
        if let Some(c) = &self.live {
            c.order_deadline.store(0, Ordering::Release);
        }
        if let Some(control) = &self.live {
            control.flat.store(self.position() == 0., Ordering::Release);
        }
        self.state.borrow_mut().fills.push(serde_json::json!({"instrument_id":event.instrument_id.to_string(),"timestamp_ns":event.ts_event.as_u64(),"client_order_id":event.client_order_id.to_string(),"side":event.order_side.to_string(),"quantity":event.last_qty.to_string(),"price":event.last_px.to_string(),"commission":event.commission.map(|v|v.to_string())}));
    }
    fn on_order_canceled(&mut self, _: &OrderCanceled) {
        self.pending = false;
        if let Some(c) = &self.live {
            c.order_deadline.store(0, Ordering::Release);
        }
    }
    fn on_order_denied(&mut self, _: OrderDenied) {
        self.pending = false;
        if let Some(c) = &self.live {
            c.order_deadline.store(0, Ordering::Release);
        }
        self.state.borrow_mut().errors.push("Order denied".into());
        if let Some(control) = &self.live {
            control.fail("Order denied");
        }
    }
    fn on_order_rejected(&mut self, _: OrderRejected) {
        self.pending = false;
        if let Some(c) = &self.live {
            c.order_deadline.store(0, Ordering::Release);
        }
        self.state.borrow_mut().errors.push("Order rejected".into());
        if let Some(control) = &self.live {
            control.fail("Order rejected");
        }
    }
});
