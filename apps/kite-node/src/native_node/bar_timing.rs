//! How long after a candle's close Kite's historical API is treated as final.

/// Clock lag before a candle counts as closed (test helpers for warm-up checks).
#[cfg(test)]
pub const COMPLETION_GRACE_NS: u64 = 2_000_000_000;
/// Lag before a closed candle is treated as broker-finalized (warm-up, gap backfill).
pub const FINALIZATION_DELAY_NS: u64 = 45_000_000_000;
