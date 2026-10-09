//! Precision Sniper v2.1.0 (WillyAlgoTrader) — Rust port of the signal and trade model.
//!
//! EMA fast/slow crossover with mandatory price momentum, scored by optional evidence
//! (trend EMA, RSI band, MACD histogram and slope, ADX/DMI, volume spike, session VWAP),
//! a high-volatility skip, an ATR/structure stop and TP1/TP2/TP3 with a step stop.
//! Entries at the signal bar close; exits stop-first on bar OHLC; reversal on an
//! accepted opposite signal. Dashboard, journal, alerts and HTF bias are not ported.
pub mod engine;
pub mod params;
pub mod ta;

pub use engine::{Bar, Engine, Event, Status, Trade};

/// Pine source version this crate ports.
pub const PORT_VERSION: &str = "2.1.0";
pub use params::Params;

#[cfg(test)]
mod tests;
