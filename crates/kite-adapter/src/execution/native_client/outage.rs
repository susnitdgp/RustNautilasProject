//! Bounded retry for classified read failures only. Mutations never retry.
use super::broker::{Broker, Snapshot};
use anyhow::{Result, anyhow};
use std::{future::Future, time::Duration};
#[derive(Debug)]
pub(crate) enum ReadFailure {
    Transient,
    SessionExpired,
    RateLimited(u64),
}
impl std::fmt::Display for ReadFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Transient => "Kite read temporarily unavailable",
            Self::SessionExpired => "Kite session rejected; renew the token in Redis",
            Self::RateLimited(_) => "Kite read rate limited; review cooldown",
        })
    }
}
impl std::error::Error for ReadFailure {}

/// Full account snapshot (start-up, reports), with the bounded retry.
pub(crate) async fn snapshot(broker: &dyn Broker) -> Result<Snapshot> {
    read(|| broker.snapshot()).await
}

/// Up to 3 attempts, 12 s each; only `ReadFailure::Transient` (and a timeout) is retried,
/// after 250 ms and then 500 ms. Session, rate-limit and integrity errors return at once.
pub(crate) async fn read<T, F, Fut>(mut attempt_read: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    for attempt in 0..3 {
        let result = tokio::time::timeout(Duration::from_secs(12), attempt_read())
            .await
            .unwrap_or_else(|_| Err(anyhow!(ReadFailure::Transient)));
        match result {
            Ok(value) => return Ok(value),
            Err(e)
                if matches!(e.downcast_ref::<ReadFailure>(), Some(ReadFailure::Transient))
                    && attempt < 2 =>
            {
                tokio::time::sleep(Duration::from_millis(250 * (attempt + 1))).await
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}
