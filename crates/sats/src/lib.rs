//! Self-Aware Trend System (SATS) v1.13.1 by WillyAlgoTrader — Rust port, version 1.13.1.
//!
//! Mirrors the Pine v6 script section by section:
//! * preset resolution and input validation (sections 3 / 3.5)        → [`params`]
//! * Pine built-ins with TradingView's na / warm-up semantics          → [`indicators`]
//! * efficiency ratio, Trend Quality Index, dynamic TP scale, score    → [`quality`]
//! * adaptive asymmetric SuperTrend with character-flip (6.2 / 6.3)    → [`supertrend`]
//! * the single model position: SL-first settlement, ⅓ partials at
//!   TP1/TP2, final TP3, flip and timeout exits, tick rounding, fees    → [`trade`]
//! * self-learning Quality-Influence calibration (experimental)        → [`learn`]
//! * per-bar orchestration and history                                 → [`engine`]
//!
//! Everything is evaluated on confirmed (closed) bars, exactly like the
//! script's `alert.freq_once_per_bar_close` events. Drawing, dashboard and
//! alert text are presentation only and are not part of the port.

pub mod engine;
pub mod indicators;
pub mod learn;
pub mod params;
pub mod quality;
pub mod supertrend;
pub mod trade;

pub use engine::{BarInput, Engine, Status, SymbolSpec};
pub use params::{Params, Preset, Source, TpMode, TqiVolMode};
pub use trade::{Event, EventKind, Side, TradeSnapshot};

/// Version of this port; kept in step with the crate version.
pub const PORT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests;
