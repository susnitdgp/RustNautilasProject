//! Offline execution simulation only. Redis journal only; no broker HTTP client or live order implementation.
pub mod coordinator;
pub mod management;
pub mod mock;
pub mod rate_limit;
pub mod reports;
pub mod translation;
pub mod verification;
