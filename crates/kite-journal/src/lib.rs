//! Redis connection helpers shared by the token store and the order-rate budget
//! (URL from `KITE_REDIS_URL`, `noeviction` check, durability sync). The order journal
//! itself was removed in kite-node 2.21.0; the crate name is historical.
pub mod connection;
