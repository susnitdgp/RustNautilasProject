//! Backtest-only strategy with native simulated stop-market protection.
use super::{
    supertrend_actor::State,
    vwap_signal::{Policy, stop_price},
};
use anyhow::{Result, ensure};
use nautilus_common::actor::DataActor;
use nautilus_model::{
    data::{Bar, BarType, QuoteTick},
    enums::{OrderSide, TimeInForce, TriggerType},
    events::{OrderCanceled, OrderDenied, OrderFilled, OrderRejected},
    identifiers::{ClientOrderId, InstrumentId},
    orders::Order,
    types::Price,
};
use nautilus_trading::{
    nautilus_strategy,
    strategy::{Strategy, StrategyConfig, StrategyCore},
};
use std::{cell::RefCell, collections::HashMap, rc::Rc};
#[derive(Debug)]
pub struct VwapStrategy {
    core: StrategyCore,
    policy: Policy,
    bar_type: BarType,
    start: u64,
    end: u64,
    state: Rc<RefCell<State>>,
    queued: Option<(i8, f64, u64)>,
    pending: bool,
    entry_atr: f64,
    stop: Option<ClientOrderId>,
    reasons: HashMap<ClientOrderId, String>,
}
impl VwapStrategy {
    pub fn new(bar_type: BarType, start: u64, end: u64, state: Rc<RefCell<State>>) -> Self {
        Self {
            core: StrategyCore::new(StrategyConfig {
                strategy_id: Some("VWAP-EMA-MACD-001".into()),
                log_events: false,
                log_commands: false,
                ..Default::default()
            }),
            policy: Policy::new(),
            bar_type,
            start,
            end,
            state,
            queued: None,
            pending: false,
            entry_atr: 0.,
            stop: None,
            reasons: HashMap::new(),
        }
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
    fn cancel_stop(&mut self) -> Result<()> {
        if let Some(id) = self.stop.take() {
            self.cancel_order(id, None, None)?;
        }
        Ok(())
    }
    fn protect(&mut self, event: &OrderFilled) -> Result<()> {
        let long = event.order_side == OrderSide::Buy;
        let trigger = stop_price(event.last_px.as_f64(), self.entry_atr, long);
        ensure!(self.entry_atr > 0. && trigger > 0., "Invalid ATR stop");
        let order = self.order().stop_market(
            self.instrument(),
            if long {
                OrderSide::Sell
            } else {
                OrderSide::Buy
            },
            1.into(),
            Price::new(trigger, 0),
            Some(TriggerType::Default),
            Some(TimeInForce::Gtc),
            None,
            Some(true),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        let id = order.client_order_id();
        self.stop = Some(id);
        self.reasons.insert(id, "stop_loss".into());
        self.state.borrow_mut().signals.push(serde_json::json!({"timestamp_ns":event.ts_event.as_u64(),"intent":"PLACE_STOP","reason":"stop_loss","stop_price":trigger,"atr_at_signal":self.entry_atr,"entry_price":event.last_px.as_f64(),"client_order_id":id.to_string()}));
        self.submit_order(order, None, None, None)
    }
    fn on_fill(&mut self, event: &OrderFilled) -> Result<()> {
        let reason = self
            .reasons
            .remove(&event.client_order_id)
            .unwrap_or_else(|| "unknown".into());
        let is_entry = reason == "entry";
        if self.stop == Some(event.client_order_id) {
            self.stop = None;
        } else {
            self.pending = false;
        }
        let intent = if is_entry {
            if event.order_side == OrderSide::Buy {
                "BUY"
            } else {
                "SELL"
            }
        } else if event.order_side == OrderSide::Sell {
            "BUY_EXIT"
        } else {
            "SELL_EXIT"
        };
        self.state.borrow_mut().fills.push(serde_json::json!({"timestamp_ns":event.ts_event.as_u64(),"client_order_id":event.client_order_id.to_string(),"side":event.order_side.to_string(),"intent":intent,"reason":reason,"quantity":event.last_qty.to_string(),"price":event.last_px.to_string(),"commission":event.commission.map(|c|c.to_string())}));
        if is_entry {
            self.protect(event)?;
        }
        Ok(())
    }
}
impl DataActor for VwapStrategy {
    fn on_start(&mut self) -> Result<()> {
        self.subscribe_bars(self.bar_type, None, None);
        self.subscribe_quotes(self.instrument(), None, None);
        self.state.borrow_mut().started = true;
        Ok(())
    }
    fn on_stop(&mut self) -> Result<()> {
        self.cancel_stop()?;
        self.state.borrow_mut().stopped = true;
        Ok(())
    }
    fn on_bar(&mut self, b: &Bar) -> Result<()> {
        let ts = b.ts_event.as_u64();
        let reading = self.policy.update(
            b.high.as_f64(),
            b.low.as_f64(),
            b.close.as_f64(),
            b.volume.as_f64(),
            ts,
        );
        self.queued = if ts > self.start && ts < self.end && reading.entry != 0 {
            Some((reading.entry, reading.atr, ts))
        } else {
            None
        };
        self.state
            .borrow_mut()
            .indicators
            .push(serde_json::json!({"bar_close_ns":ts,"values":reading}));
        Ok(())
    }
    fn on_quote(&mut self, q: &QuoteTick) -> Result<()> {
        let ts = q.ts_event.as_u64();
        if ts < self.start || self.pending {
            return Ok(());
        }
        let position = self.position();
        let (target, atr, signal_ts) = if ts >= self.end {
            (0, 0., self.end)
        } else if let Some(v) = self.queued {
            v
        } else {
            return Ok(());
        };
        if position == f64::from(target) {
            self.queued = None;
            return Ok(());
        }
        let exit = position != 0.;
        let side = if (exit && position > 0.) || (!exit && target < 0) {
            OrderSide::Sell
        } else {
            OrderSide::Buy
        };
        if exit {
            self.cancel_stop()?;
        } else {
            self.entry_atr = atr;
            self.queued = None;
        }
        let reason = if ts >= self.end {
            "end_of_day"
        } else if exit {
            "opposite_setup"
        } else {
            "entry"
        };
        let order = self.order().market(
            self.instrument(),
            side,
            1.into(),
            Some(TimeInForce::Gtc),
            Some(exit),
            None,
            None,
            None,
            None,
            None,
        );
        self.reasons.insert(order.client_order_id(), reason.into());
        self.state.borrow_mut().signals.push(serde_json::json!({"timestamp_ns":ts,"signal_bar_close_ns":signal_ts,"intent":if exit {if side==OrderSide::Sell {"BUY_EXIT"} else {"SELL_EXIT"}} else if side==OrderSide::Buy {"BUY"} else {"SELL"},"reason":reason,"atr_at_signal":atr}));
        self.pending = true;
        self.submit_order(order, None, None, None)?;
        Ok(())
    }
}
nautilus_strategy!(VwapStrategy, {
    fn on_order_filled(&mut self, event: &OrderFilled) {
        if let Err(error) = self.on_fill(event) {
            self.state.borrow_mut().errors.push(error.to_string());
        }
    }
    fn on_order_canceled(&mut self, event: &OrderCanceled) {
        self.reasons.remove(&event.client_order_id);
        if self.stop == Some(event.client_order_id) {
            self.stop = None;
        }
    }
    fn on_order_denied(&mut self, event: OrderDenied) {
        self.state
            .borrow_mut()
            .errors
            .push(format!("Order denied: {}", event.reason));
    }
    fn on_order_rejected(&mut self, event: OrderRejected) {
        self.state
            .borrow_mut()
            .errors
            .push(format!("Order rejected: {}", event.reason));
    }
});
