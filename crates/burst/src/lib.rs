//! CRUDE-BURST v1 — a 1-minute momentum-burst scalper for MCX crude oil.
//!
//! * [`engine::Engine`] turns closed 1m bars into entry [`engine::Signal`]s: a strong,
//!   high-volume bar that closes out of a tight base, on the right side of VWAP,
//!   with room before the next opposing level.
//! * [`trade::Trade`] manages one position: stop, breakeven, swing trail, target and
//!   time stop — on live prices (`on_price`) or, for backtests, on bar OHLC with
//!   pessimistic intrabar ordering (`on_bar`).
//! * [`trade::DayRisk`] enforces the daily limits (trades, losing streak, loss).
//!
//! Price, volume and time only, so it can be backtested on Kite 1m history.
pub mod engine;
pub mod indicators;
pub mod params;
pub mod trade;

pub use engine::{Bar, Engine, Signal};
pub use params::Params;
pub use trade::{DayRisk, ExitReason, Trade};

/// Long or short.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Side {
    Long,
    Short,
}
impl Side {
    /// +1 for long, -1 for short.
    pub fn dir(self) -> f64 {
        match self {
            Side::Long => 1.0,
            Side::Short => -1.0,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Side::Long => "LONG",
            Side::Short => "SHORT",
        }
    }
}

#[cfg(test)]
mod tests;
