//! Four explicit strategy intents. An exit can only reduce its matching position.
use anyhow::{Result, ensure};
use kite_strategy::config::Config;
use nautilus_model::{data::QuoteTick, enums::OrderSide};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    Buy,
    BuyExit,
    Sell,
    SellExit,
}
impl Signal {
    pub fn name(self) -> &'static str {
        match self {
            Self::Buy => "BUY",
            Self::BuyExit => "BUY_EXIT",
            Self::Sell => "SELL",
            Self::SellExit => "SELL_EXIT",
        }
    }
    pub fn side(self) -> OrderSide {
        match self {
            Self::Buy | Self::SellExit => OrderSide::Buy,
            Self::Sell | Self::BuyExit => OrderSide::Sell,
        }
    }
    pub fn is_exit(self) -> bool {
        matches!(self, Self::BuyExit | Self::SellExit)
    }
    pub fn valid(self, position: f64, short: bool) -> bool {
        match self {
            Self::Buy => position == 0.0,
            Self::Sell => position == 0.0 && short,
            Self::BuyExit => position == 1.0,
            Self::SellExit => position == -1.0,
        }
    }
}
/// Local tick-triggered protection; not an exchange stop and not a fill guarantee.
pub fn protection(
    q: &QuoteTick,
    position: f64,
    entry: f64,
    config: &Config,
) -> Result<Option<Signal>> {
    if position == 0.0 {
        return Ok(None);
    }
    ensure!(
        matches!(position, 1.0 | -1.0) && entry.is_finite() && entry > 0.0,
        "Invalid protective-exit position or entry price"
    );
    let (pnl, signal) = if position > 0.0 {
        (q.bid_price.as_f64() - entry, Signal::BuyExit)
    } else {
        (entry - q.ask_price.as_f64(), Signal::SellExit)
    };
    Ok(
        (pnl <= -f64::from(config.stop_loss_rupees) || pnl >= f64::from(config.target_rupees))
            .then_some(signal),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> Config {
        Config::parse(include_str!("../../../../config/strategy-crossover.toml")).unwrap()
    }
    fn quote(bid: i64, ask: i64) -> QuoteTick {
        QuoteTick::new(
            "CRUDEOIL26SEPFUT.MCX".into(),
            nautilus_model::types::Price::new(bid as f64, 0),
            nautilus_model::types::Price::new(ask as f64, 0),
            1.into(),
            1.into(),
            1.into(),
            1.into(),
        )
    }
    #[test]
    fn long_and_short_protection_uses_executable_side_and_exact_thresholds() {
        let c = config();
        for (position, bid, ask, expected) in [
            (1.0, 5970, 5971, Some(Signal::BuyExit)),
            (1.0, 5971, 5972, None),
            (1.0, 6060, 6061, Some(Signal::BuyExit)),
            (-1.0, 6029, 6030, Some(Signal::SellExit)),
            (-1.0, 6028, 6029, None),
            (-1.0, 5939, 5940, Some(Signal::SellExit)),
        ] {
            assert_eq!(
                protection(&quote(bid, ask), position, 6000.0, &c).unwrap(),
                expected
            );
        }
        assert!(protection(&quote(6000, 6001), 1.0, 0.0, &c).is_err());
        assert_eq!(protection(&quote(5900, 5901), 0.0, 0.0, &c).unwrap(), None);
    }
    #[test]
    fn four_intents_cannot_reverse_or_exit_wrong_side() {
        assert!(Signal::Buy.valid(0.0, true));
        assert!(!Signal::Buy.valid(-1.0, true));
        assert!(Signal::Sell.valid(0.0, true));
        assert!(!Signal::Sell.valid(0.0, false));
        assert!(Signal::BuyExit.valid(1.0, false));
        assert!(!Signal::BuyExit.valid(-1.0, true));
        assert!(Signal::SellExit.valid(-1.0, false));
        assert!(!Signal::SellExit.valid(1.0, true));
        assert_eq!(Signal::BuyExit.side(), OrderSide::Sell);
        assert_eq!(Signal::SellExit.side(), OrderSide::Buy);
    }
}
