//! Bounded retry for classified read failures only. Mutations never retry.
use super::broker::{Broker, Snapshot};
use anyhow::{Result, anyhow};
use std::time::Duration;
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
pub(crate) async fn snapshot(broker: &dyn Broker) -> Result<Snapshot> {
    snapshot_checked(broker, |_| Ok(()))
        .await
        .map(|(snapshot, ())| snapshot)
}
/// Transport failures and lagging broker views share one retry budget. The
/// validator must be side-effect free: only the successful result is published.
pub(crate) async fn snapshot_checked<T>(
    broker: &dyn Broker,
    mut validate: impl FnMut(&Snapshot) -> Result<T>,
) -> Result<(Snapshot, T)> {
    for attempt in 0..3 {
        let result = tokio::time::timeout(Duration::from_secs(12), broker.snapshot())
            .await
            .unwrap_or_else(|_| Err(anyhow!(ReadFailure::Transient)))
            .and_then(|snapshot| validate(&snapshot).map(|value| (snapshot, value)));
        match result {
            Ok(s) => return Ok(s),
            Err(e)
                if (matches!(
                    e.downcast_ref::<ReadFailure>(),
                    Some(ReadFailure::Transient)
                ) || e.is::<super::super::broker_events::ObservationLag>())
                    && attempt < 2 =>
            {
                tokio::time::sleep(Duration::from_millis(250 * (attempt + 1))).await
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}
