//! USER STRATEGY: edit signal decisions here. Runtime, orders and persistence live elsewhere.
use anyhow::Result;
use kite_strategy::config::Config;
use nautilus_indicators::{average::sma::SimpleMovingAverage, indicator::Indicator};
use nautilus_model::{
    data::QuoteTick,
    enums::{OrderSide, PriceType},
};

#[derive(Debug)]
pub struct UserStrategy {
    fast: SimpleMovingAverage,
    slow: SimpleMovingAverage,
    previous: Option<bool>,
}
impl UserStrategy {
    pub fn new(config: &Config) -> Self {
        Self {
            fast: SimpleMovingAverage::new(config.fast, Some(PriceType::Mid)),
            slow: SimpleMovingAverage::new(config.slow, Some(PriceType::Mid)),
            previous: None,
        }
    }
    /// Return a desired order side, or None to wait. Position is native net contracts.
    /// The runtime separately enforces one long contract and one pending order.
    pub fn on_quote(&mut self, quote: &QuoteTick, position: f64) -> Result<Option<OrderSide>> {
        self.fast.handle_quote(quote)?;
        self.slow.handle_quote(quote)?;
        if !self.fast.initialized() || !self.slow.initialized() {
            return Ok(None);
        }
        let above = self.fast.value > self.slow.value;
        let prior = self.previous.replace(above);
        Ok(match prior {
            Some(false) if above && position == 0.0 => Some(OrderSide::Buy),
            Some(true) if !above && position == 1.0 => Some(OrderSide::Sell),
            _ => None,
        })
    }
    /// Full live/synthetic packet entry point. Edit here for depth, OI, volume or OHLC signals.
    /// The default policy shares its quote logic with quote-only BacktestNode replay.
    pub fn on_full_tick(
        &mut self,
        tick: &kite_adapter::data::full_tick::KiteFullTick,
        position: f64,
    ) -> Result<Option<OrderSide>> {
        self.on_quote(&tick.quote, position)
    }

    pub fn reset(&mut self) {
        self.fast.reset();
        self.slow.reset();
        self.previous = None;
    }
}
