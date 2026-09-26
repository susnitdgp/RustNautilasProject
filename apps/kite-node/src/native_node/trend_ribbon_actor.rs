//! Trend Ribbon live strategy actor.
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
    pub rebuilds: Vec<serde_json::Value>,
    pub indicators: Vec<serde_json::Value>,
    pub signals: Vec<serde_json::Value>,
    pub fills: Vec<serde_json::Value>,
    pub errors: Vec<String>,
    pub started: bool,
    pub stopped: bool,
}

fn ribbon_close_sync_target(
    current_target: i8,
    confirmed_direction: i8,
    sync_active: bool,
    blocked_this_bar: bool,
    same_trend_locked: bool,
) -> Option<i8> {
    (sync_active
        && !blocked_this_bar
        && confirmed_direction != 0
        && current_target != confirmed_direction
        && !same_trend_locked)
        .then_some(confirmed_direction)
}

#[derive(Debug)]
pub struct BarStrategy {
    core: StrategyCore,
    ribbon: super::trend_ribbon::TrendRibbon,
    ribbon_live: super::trend_ribbon_realtime::RealtimeRibbon,
    bar_type: BarType,
    start: u64,
    end: u64,
    target: i8,
    live: Option<super::live_control::Control>,
    last_bar: u64,
    pending: bool,
    ribbon_sync_active: bool,
    target_reason: Option<&'static str>,
    state: Rc<RefCell<State>>,
}

impl BarStrategy {
    pub fn new(
        bar_type: BarType,
        start: u64,
        end: u64,
        state: Rc<RefCell<State>>,
        settings: super::trend_ribbon::Settings,
        calendar: super::session_calendar::Calendar,
        bar_ns: u64,
    ) -> Result<Self> {
        let ribbon_live = super::trend_ribbon_realtime::RealtimeRibbon::new(&settings, bar_ns)?;
        let ribbon =
            super::trend_ribbon::TrendRibbon::new_for_interval(settings, calendar, bar_ns)?;
        Ok(Self {
            core: StrategyCore::new(StrategyConfig {
                strategy_id: Some("TREND-RIBBON-001".into()),
                log_events: false,
                log_commands: false,
                ..Default::default()
            }),
            ribbon,
            ribbon_live,
            bar_type,
            start,
            end,
            target: 0,
            live: None,
            last_bar: 0,
            pending: false,
            ribbon_sync_active: false,
            target_reason: None,
            state,
        })
    }

    pub fn with_live(mut self, control: super::live_control::Control) -> Self {
        self.live = Some(control);
        self
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
        self.state.borrow_mut().signals.push(serde_json::json!({
            "timestamp_ns":ts,
            "intent":intent,
            "target":target,
            "position_before":position,
            "entry_filtered":false,
            "reason":reason,
            "bar_close_ns":self.last_bar,
            "bar_to_signal_ms":ts.saturating_sub(self.last_bar)/1_000_000
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
            .map(|position| position.signed_qty)
            .sum()
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
            let client = if self.live.as_ref().is_some_and(|control| control.sim) {
                "STBARS".into()
            } else {
                "KITE".into()
            };
            self.subscribe_data(
                DataType::new("KiteFullTick", None, None),
                Some(client),
                None,
            );
            self.clock().set_timer_ns(
                "strategy_square_off",
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
            self.clock().cancel_timer("strategy_square_off");
            self.cancel_all_orders(self.instrument(), None, None, true, None)?;
        }
        self.state.borrow_mut().stopped = true;
        Ok(())
    }

    fn on_time_event(&mut self, event: &TimeEvent) -> Result<()> {
        if event.name.as_str() != "strategy_square_off" {
            return Ok(());
        }
        let now = self.clock().timestamp_ns().as_u64();
        let stopping = self
            .live
            .as_ref()
            .is_some_and(|control| control.stopping.load(Ordering::Acquire));
        if now >= self.end || stopping {
            self.target = 0;
            self.ribbon_live.on_session_end();
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
        if let Some(full) = data
            .data
            .as_any()
            .downcast_ref::<kite_adapter::data::full_tick::KiteFullTick>()
        {
            let Some(control) = self.live.clone() else {
                return Ok(());
            };
            if control.paused.load(Ordering::Acquire) || control.stopping.load(Ordering::Acquire) {
                return Ok(());
            }
            let packet_ts = full.quote.ts_event.as_u64();
            let now = if control.sim {
                packet_ts
            } else {
                self.clock().timestamp_ns().as_u64()
            };
            if !control.fresh_quote(packet_ts, full.quote.ts_init.as_u64(), now) {
                return Ok(());
            }
            let Some(raw) = full.snapshot.raw.as_ref() else {
                return Ok(());
            };
            let Some(trade_seconds) = raw.full.as_ref().and_then(|f| f.last_trade_timestamp) else {
                return Ok(());
            };
            let trade_ts = u64::from(trade_seconds) * 1_000_000_000;
            if trade_ts < self.start || trade_ts > now {
                return Ok(());
            }
            let price = f64::from(raw.ltp_paise) / 100.0;
            let in_session = self.ribbon.in_session(trade_ts)?;
            let position_value = self.position();
            let position = if position_value > 0.0 {
                1
            } else if position_value < 0.0 {
                -1
            } else {
                0
            };
            let (snapshot, event) = self.ribbon_live.on_tick(
                price,
                trade_ts,
                now,
                position,
                in_session,
                !self.pending,
            )?;
            if let Some(event) = event {
                self.target = event.target;
                self.ribbon_sync_active = true;
                self.target_reason = Some(event.reason());
                self.state.borrow_mut().indicators.push(serde_json::json!({
                    "realtime_event":event.kind,
                    "snapshot":snapshot,
                    "received_ns":now
                }));
                self.trade_target(event.target, now, event.reason())?;
            }
            return Ok(());
        }

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
            "direction":self.target,
            "position":self.position(),
            "automatic_resume_enabled":false,
            "paper_only":self.live.is_some(),
            "live_orders_enabled":self.live.as_ref().is_some_and(|control|control.real)
        });
        Ok(indexmap::IndexMap::from([(
            "trend_ribbon_state".into(),
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

        let high = bar.high.as_f64();
        let low = bar.low.as_f64();
        let close = bar.close.as_f64();
        let bar_close_ns = bar.ts_event.as_u64();
        self.ribbon_live
            .on_confirmed_bar(high, low, close, bar_close_ns)?;
        let observation = self.ribbon.update(high, low, close, bar_close_ns)?;

        if !observation.in_session {
            self.target = 0;
            self.target_reason = Some("session_end");
            self.ribbon_live.on_session_end();
        }

        let blocked_this_bar = self.ribbon_live.confirmed_event_blocked(bar_close_ns);
        if bar_close_ns > self.start {
            if observation.signal != 0 && !blocked_this_bar {
                self.target = observation.signal;
                self.ribbon_sync_active = true;
                self.target_reason = Some("trend_ribbon");
            } else if let Some(sync_target) = ribbon_close_sync_target(
                self.target,
                observation.direction,
                self.ribbon_sync_active,
                blocked_this_bar,
                self.ribbon_live
                    .blocks_confirmed_sync(observation.direction),
            ) {
                self.target = sync_target;
                self.target_reason = Some("close_sync");
            }
        }
        self.ribbon_live
            .on_confirmed_direction(observation.direction);
        self.last_bar = bar_close_ns;
        self.state
            .borrow_mut()
            .indicators
            .push(serde_json::to_value(observation)?);
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

        if let Some(control) = self.live.clone() {
            let rebuild = control.rebuild.lock().expect("rebuild lock").take();
            if let Some((epoch, bars)) = rebuild
                && epoch == control.epoch.load(Ordering::Acquire)
                && control.online.load(Ordering::Acquire)
            {
                let previous = self.last_bar;
                self.ribbon = self.ribbon.rebuild_empty()?;
                self.ribbon_live = super::trend_ribbon_realtime::RealtimeRibbon::new(
                    &self.ribbon.settings,
                    control.bar_ns,
                )?;
                self.last_bar = 0;
                self.target = 0;
                self.state.borrow_mut().indicators.clear();
                for bar in bars {
                    self.on_bar(&bar)?;
                }

                let position = self.position();
                let latest = self.state.borrow().indicators.last().cloned();
                let direction = latest
                    .as_ref()
                    .and_then(|value| value["direction"].as_i64())
                    .unwrap_or(0);
                let inside = latest
                    .as_ref()
                    .is_some_and(|value| value["in_session"] == true);
                self.target = if inside && position != 0.0 && position.signum() == direction as f64
                {
                    self.ribbon_sync_active = true;
                    direction as i8
                } else {
                    0
                };

                self.state.borrow_mut().rebuilds.push(serde_json::json!({
                    "epoch":epoch,
                    "previous_bar":previous,
                    "rebuilt_bar":self.last_bar,
                    "received_ns":quote.ts_init.as_u64(),
                    "past_orders_replayed":false,
                    "reason":control.bar_feed.lock().expect("bar feed stats").last_rebuild_reason.clone()
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
        let stopping = self
            .live
            .as_ref()
            .is_some_and(|control| control.stopping.load(Ordering::Acquire));
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

        let session_clock = if self.live.as_ref().is_some_and(|control| control.sim) {
            self.last_bar
        } else {
            ts
        };
        let session_closed = !self.ribbon.in_session(session_clock)?;
        let target = if session_closed { 0 } else { target };
        let reason = if session_closed || ts >= self.end {
            "session_end"
        } else if stopping {
            "shutdown"
        } else {
            self.target_reason.unwrap_or("trend_ribbon")
        };
        self.trade_target(target, ts, reason)?;
        Ok(())
    }
}

#[cfg(test)]
mod ribbon_sync_tests {
    use super::ribbon_close_sync_target;

    #[test]
    fn close_sync_requires_live_state_and_respects_bar_and_wt_locks() {
        assert_eq!(ribbon_close_sync_target(1, -1, false, false, false), None);
        assert_eq!(ribbon_close_sync_target(1, -1, true, true, false), None);
        assert_eq!(ribbon_close_sync_target(0, -1, true, false, true), None);
        assert_eq!(ribbon_close_sync_target(-1, -1, true, false, false), None);
        assert_eq!(
            ribbon_close_sync_target(1, -1, true, false, false),
            Some(-1)
        );
        assert_eq!(ribbon_close_sync_target(-1, 1, true, false, false), Some(1));
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
        self.state.borrow_mut().fills.push(serde_json::json!({
            "instrument_id":event.instrument_id.to_string(),
            "timestamp_ns":event.ts_event.as_u64(),
            "client_order_id":event.client_order_id.to_string(),
            "side":event.order_side.to_string(),
            "quantity":event.last_qty.to_string(),
            "price":event.last_px.to_string(),
            "commission":event.commission.map(|value|value.to_string())
        }));
    }

    fn on_order_canceled(&mut self, _: &OrderCanceled) {
        self.pending = false;
        if let Some(control) = &self.live {
            control.order_deadline.store(0, Ordering::Release);
        }
    }

    fn on_order_denied(&mut self, _: OrderDenied) {
        self.pending = false;
        if let Some(control) = &self.live {
            control.order_deadline.store(0, Ordering::Release);
            control.fail("Order denied");
        }
        self.state.borrow_mut().errors.push("Order denied".into());
    }

    fn on_order_rejected(&mut self, _: OrderRejected) {
        self.pending = false;
        if let Some(control) = &self.live {
            control.order_deadline.store(0, Ordering::Release);
            control.fail("Order rejected");
        }
        self.state.borrow_mut().errors.push("Order rejected".into());
    }
});
