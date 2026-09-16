//! Backtest-only bar strategy. Signals are executed on the next synthetic open quote.
use super::supertrend::Supertrend;
use anyhow::Result;
use nautilus_common::actor::DataActor;
use nautilus_model::{
    data::{Bar, BarType, QuoteTick},
    enums::{OrderSide, TimeInForce},
    events::{OrderDenied, OrderFilled, OrderRejected},
    identifiers::InstrumentId,
};
use nautilus_trading::{
    nautilus_strategy,
    strategy::{Strategy, StrategyConfig, StrategyCore},
};
use std::{cell::RefCell, rc::Rc};
#[derive(Default, Debug)]
pub struct State {
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
    bar_type: BarType,
    start: u64,
    end: u64,
    target: i8,
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
            bar_type,
            start,
            end,
            target: 0,
            pending: false,
            state,
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
}
impl DataActor for BarStrategy {
    fn on_start(&mut self) -> Result<()> {
        self.subscribe_bars(self.bar_type, None, None);
        self.subscribe_quotes(self.instrument(), None, None);
        self.state.borrow_mut().started = true;
        Ok(())
    }
    fn on_stop(&mut self) -> Result<()> {
        self.state.borrow_mut().stopped = true;
        Ok(())
    }
    fn on_bar(&mut self, b: &Bar) -> Result<()> {
        let result = self
            .indicator
            .update(b.high.as_f64(), b.low.as_f64(), b.close.as_f64());
        if let Some((direction, _)) = result {
            self.target = direction;
        }
        self.state.borrow_mut().indicators.push(serde_json::json!({
            "bar_close_ns":b.ts_event.as_u64(),"close":b.close.as_f64(),"atr":self.indicator.atr.value,
            "initialized":self.indicator.atr.initialized,"direction":result.map(|x|x.0),"supertrend":result.map(|x|x.1)
        }));
        Ok(())
    }
    fn on_quote(&mut self, q: &QuoteTick) -> Result<()> {
        let ts = q.ts_event.as_u64();
        if ts < self.start || self.pending {
            return Ok(());
        }
        let target = if ts >= self.end { 0 } else { self.target };
        let position = self.position();
        if position == f64::from(target) {
            return Ok(());
        }
        let exit = position != 0.;
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
            Some(TimeInForce::Gtc),
            Some(exit),
            None,
            None,
            None,
            None,
            None,
        );
        self.state.borrow_mut().signals.push(serde_json::json!({"timestamp_ns":ts,"intent":intent,"target":target,"position_before":position,"reason":if ts>=self.end {"end_of_day"} else {"supertrend"}}));
        self.pending = true;
        self.submit_order(order, None, None, None)?;
        Ok(())
    }
}
nautilus_strategy!(BarStrategy, {
    fn on_order_filled(&mut self, event: &OrderFilled) {
        self.pending = false;
        self.state.borrow_mut().fills.push(serde_json::json!({"timestamp_ns":event.ts_event.as_u64(),"client_order_id":event.client_order_id.to_string(),"side":event.order_side.to_string(),"quantity":event.last_qty.to_string(),"price":event.last_px.to_string(),"commission":event.commission.map(|v|v.to_string())}));
    }
    fn on_order_denied(&mut self, _: OrderDenied) {
        self.pending = false;
        self.state.borrow_mut().errors.push("Order denied".into());
    }
    fn on_order_rejected(&mut self, _: OrderRejected) {
        self.pending = false;
        self.state.borrow_mut().errors.push("Order rejected".into());
    }
});
