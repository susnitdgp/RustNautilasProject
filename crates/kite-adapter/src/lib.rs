//! NautilusTrader adapter for Zerodha Kite Connect: market data, instruments, history and
//! the native execution client (real orders only with the `live-orders` feature).
pub mod auth;
pub mod credentials;
pub mod data;
pub mod execution;
pub mod http;
pub mod instruments;
pub mod mapping;
pub mod preflight;
pub mod websocket;
