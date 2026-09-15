//! Offline execution simulation only. Redis journal only; no broker HTTP client or live order implementation.
pub mod coordinator;
pub mod mock;
pub mod translation;
pub mod verification;
