use anyhow::{Result, ensure};
use kite_strategy::config::Config;
use nautilus_model::{data::QuoteTick, enums::OrderSide, events::OrderInitialized};
use rust_decimal::Decimal;
/// Additional paper feed/position controls, applied before native RiskEngine.
pub struct Controls {
    pub generation: u64,
    pub connected: bool,
    pub last_source: u64,
    pub last_received: u64,
    pub blocked: u64,
    pub gaps: u64,
    pub max_notional: Decimal,
}
impl Default for Controls {
    fn default() -> Self {
        Self {
            generation: 0,
            connected: false,
            last_source: 0,
            last_received: 0,
            blocked: 0,
            gaps: 0,
            max_notional: Decimal::from(2_000_000),
        }
    }
}
impl Controls {
    pub fn connect(&mut self, generation: u64) -> Result<()> {
        ensure!(
            generation > self.generation,
            "Feed generation did not advance"
        );
        self.generation = generation;
        self.connected = true;
        self.last_source = 0;
        self.last_received = 0;
        Ok(())
    }
    pub fn gap(&mut self) {
        self.connected = false;
        self.gaps += 1;
    }
    pub fn quote(
        &mut self,
        q: &QuoteTick,
        generation: u64,
        now: u64,
        config: &Config,
    ) -> std::result::Result<(), &'static str> {
        if generation != self.generation {
            return Err("obsolete_generation");
        }
        if !self.connected {
            return Err("disconnected");
        }
        if let Some(reason) =
            super::diagnostics::quote_reason(q, config, self.last_source, self.last_received, now)
        {
            return Err(reason);
        }
        self.last_source = q.ts_event.as_u64();
        self.last_received = q.ts_init.as_u64();
        Ok(())
    }
    pub fn order(
        &mut self,
        o: &OrderInitialized,
        position: u32,
        now: u64,
        config: &Config,
    ) -> Result<()> {
        let result = (|| {
            ensure!(
                self.connected
                    && self.last_source > 0
                    && now.saturating_sub(self.last_source)
                        <= u64::from(config.max_age_seconds) * 1_000_000_000,
                "Paper feed is not fresh"
            );
            kite_paper::validation::order(o)?;
            ensure!(
                (o.order_side == OrderSide::Buy && position == 0)
                    || (o.order_side == OrderSide::Sell && position == 1),
                "Paper position limit exceeded"
            );
            ensure!(
                o.price.unwrap().as_decimal() * Decimal::from(100) <= self.max_notional,
                "Paper notional limit exceeded"
            );
            Ok(())
        })();
        if result.is_err() {
            self.blocked += 1;
        }
        result
    }
}
