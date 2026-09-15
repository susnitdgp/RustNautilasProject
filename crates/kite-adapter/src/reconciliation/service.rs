//! Bounded read-only observation; equal samples are not an atomic broker snapshot.
use super::{Summary, checks, snapshot::Snapshot};
use crate::{
    account::profile::Profile,
    credentials::KiteCredentials,
    http::authenticated::{Endpoint, ReadClient},
};
use anyhow::{Result, anyhow};
use chrono::{FixedOffset, Utc};
use std::time::Duration;

pub async fn run(credentials: &KiteCredentials, symbol: &str, token: u32) -> Result<Summary> {
    tokio::time::timeout(Duration::from_secs(90), observe(credentials, symbol, token))
        .await
        .map_err(|_| anyhow!("Kite reconciliation timed out; verification incomplete"))?
}
async fn observe(credentials: &KiteCredentials, symbol: &str, token: u32) -> Result<Summary> {
    let date = || {
        Utc::now()
            .with_timezone(&FixedOffset::east_opt(19800).unwrap())
            .date_naive()
    };
    let started = date();
    let client = ReadClient::new(credentials)?;
    let profile: Profile = client.get(Endpoint::Profile).await?;
    profile.validate()?;
    let mut previous = Snapshot::read(&client, symbol, token).await?;
    for attempt in 0..2 {
        tokio::time::sleep(Duration::from_millis(1100)).await;
        let next = Snapshot::read(&client, symbol, token).await?;
        let stable = previous == next && date() == started;
        if stable || attempt == 1 {
            let mut summary = checks::check(&next, symbol, token, &profile.products);
            summary.trading_date_ist = started.to_string();
            summary.stable_observation = stable;
            if !stable {
                summary.issues.insert("observation_changed_or_date_rolled");
            }
            summary.consistency_checks_passed = summary.issues.is_empty();
            return Ok(summary);
        }
        previous = next;
    }
    unreachable!()
}
