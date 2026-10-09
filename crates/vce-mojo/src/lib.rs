//! VCE-Mojo v1.6 (Volatility Coil Edge, BullByte, AlgoMojo mod) signal engine — Rust port.
//!
//! Port version 1.6.0. Mirrors the Pine v6 script section by section:
//! coil detection, watched coil, bar-close triggers, SL/target first-touch exits
//! (SL wins a same-bar tie) and the IST end-of-day square-off.
//!
//! The engine is pure: no broker, no clock, no I/O. Feed it completed bars with
//! [`Engine::on_bar`] (Pine bar-close semantics) and, when running live, every
//! trade price with [`Engine::on_price`] (Pine's intrabar first-touch exit alerts).
//! It emits the same four AlgoMojo actions: BUY, SELL, SHORT, COVER.

pub mod engine;
pub mod indicators;
pub mod params;

pub use engine::{Action, BarInput, Engine, Event, ExitReason, Levels, Side, Status};
pub use params::{ExitTarget, ManualOverrides, Params, Resolved, Sensitivity, TouchMode};

/// Version of this port; kept in step with the crate version.
pub const PORT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests;
