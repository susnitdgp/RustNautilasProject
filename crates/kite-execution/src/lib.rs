//! Shared Redis order-rate budget for every slot on one Kite account (5/s, 100/min,
//! 1000/day by default), reserved atomically by a Lua script. No broker transport.
pub mod rate_limit;
