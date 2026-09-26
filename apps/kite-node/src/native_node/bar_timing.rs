//! Boundary-aligned completed-bar polling; never an intrabar trading signal.
use serde::Serialize;

pub const COMPLETION_GRACE_NS: u64 = 2_000_000_000;
pub const AUDIT_INTERVAL_NS: u64 = 10_000_000_000;
pub const PUBLICATION_RETRY_NS: u64 = 500_000_000;
const PUBLICATION_WAIT_NS: u64 = 15_000_000_000;

pub fn eligible_close(now: u64, step: u64) -> u64 {
    now.saturating_sub(COMPLETION_GRACE_NS) / step * step
}

/// Keep full-history correction audits, but give the next close priority.
/// Avoid starting a routine request immediately before the boundary request.
pub fn next_poll(now: u64, latest: u64, step: u64) -> u64 {
    let eligible = eligible_close(now, step);
    if latest < eligible {
        return now.saturating_add(PUBLICATION_RETRY_NS);
    }
    let boundary = eligible
        .saturating_add(step)
        .saturating_add(COMPLETION_GRACE_NS);
    let audit = now.saturating_add(AUDIT_INTERVAL_NS);
    if audit.saturating_add(COMPLETION_GRACE_NS) >= boundary {
        boundary
    } else {
        audit
    }
}

/// A newly closed candle can take time to appear in a successful API response.
/// Wait briefly without falsely treating a single unpublished tail as a gap.
/// The strategy's current_bar guard still blocks entries while it is missing.
pub fn publication_pending(latest: u64, requested_at: u64, step: u64) -> bool {
    let expected = eligible_close(requested_at, step);
    latest.checked_add(step) == Some(expected)
        && requested_at.saturating_sub(expected.saturating_add(COMPLETION_GRACE_NS))
            < PUBLICATION_WAIT_NS
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Stats {
    pub fetches: u64,
    pub failures: u64,
    pub publication_waits: u64,
    pub rebuild_requests: u64,
    pub price_revisions: u64,
    pub volume_only_revisions: u64,
    pub ignored_volume_revisions: u64,
    pub last_fetch_ms: u64,
    pub last_candle_open_ns: u64,
    pub last_candle_close_ns: u64,
    pub last_candle_received_ns: u64,
    pub last_close_to_receive_ms: u64,
    pub last_rebuild_reason: String,
    pub last_revision_samples: Vec<String>,
}
impl Stats {
    pub fn received(&mut self, close: u64, step: u64, received: u64) {
        // Routine re-audits must not overwrite original arrival latency.
        if close > self.last_candle_close_ns {
            self.last_candle_open_ns = close.saturating_sub(step);
            self.last_candle_close_ns = close;
            self.last_candle_received_ns = received;
            self.last_close_to_receive_ms = received.saturating_sub(close) / 1_000_000;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const SECOND: u64 = 1_000_000_000;

    #[test]
    fn both_intervals_poll_at_close_plus_grace_not_ten_seconds_later() {
        for step in [180 * SECOND, 300 * SECOND] {
            let close = 100 * step;
            for seconds_before in 0..=9 {
                let now = close - seconds_before * SECOND;
                assert_eq!(
                    next_poll(now, close - step, step),
                    close + COMPLETION_GRACE_NS
                );
            }
            assert_eq!(
                eligible_close(close + COMPLETION_GRACE_NS - 1, step),
                close - step
            );
            assert_eq!(eligible_close(close + COMPLETION_GRACE_NS, step), close);
        }
    }

    #[test]
    fn audits_continue_mid_candle_and_unpublished_candles_retry_without_burst() {
        let step = 180 * SECOND;
        let close = 100 * step;
        assert_eq!(
            next_poll(close + 30 * SECOND, close, step),
            close + 40 * SECOND
        );
        assert_eq!(
            next_poll(close + 2 * SECOND, close - step, step),
            close + 2 * SECOND + PUBLICATION_RETRY_NS
        );
        assert!(publication_pending(close - step, close + 2 * SECOND, step));
        assert!(publication_pending(close - step, close + 16 * SECOND, step));
        assert!(!publication_pending(
            close - step,
            close + 17 * SECOND,
            step
        ));
        assert!(!publication_pending(
            close - 2 * step,
            close + 2 * SECOND,
            step
        ));
        assert!(!publication_pending(close, close + 2 * SECOND, step));
        assert!(!publication_pending(close - step, close + SECOND, step));
    }

    #[test]
    fn arrival_latency_is_not_inflated_by_subsequent_audits() {
        let mut stats = Stats::default();
        let close = 180 * SECOND;
        stats.received(close, close, close + 2 * SECOND + 100_000_000);
        stats.received(close, close, close + 15 * SECOND);
        assert_eq!(stats.last_close_to_receive_ms, 2100);
        assert_eq!(stats.last_candle_open_ns, 0);
        stats.received(2 * close, close, 2 * close + 3 * SECOND);
        assert_eq!(stats.last_close_to_receive_ms, 3000);
        assert_eq!(stats.last_candle_open_ns, close);
    }
}
