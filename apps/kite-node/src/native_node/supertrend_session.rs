//! Run only the configured contract's current session; stop before market close.
use anyhow::{Result, ensure};
pub fn duration(now: u64) -> Result<u64> {
    let date = chrono::DateTime::from_timestamp_nanos(now as i64)
        .with_timezone(&chrono::FixedOffset::east_opt(19800).unwrap())
        .date_naive();
    let (start, end) = super::vwap_input::bounds(date)?;
    let stop = end - 60_000_000_000;
    ensure!(
        now >= start && now + 5_000_000_000 < stop,
        "Session mode starts during trading hours and before the final minute"
    );
    Ok((stop - now) / 1_000_000_000)
}
pub fn run(config: &str) -> Result<()> {
    super::supertrend_live_runner::run(config, duration(super::data::now())?, false)
}
#[test]
fn session_deadline_is_before_market_close() {
    let date = chrono::NaiveDate::from_ymd_opt(2026, 9, 16).unwrap();
    let (start, end) = super::vwap_input::bounds(date).unwrap();
    assert_eq!(duration(start).unwrap(), (end - start) / 1_000_000_000 - 60);
    assert!(duration(start - 1).is_err());
    assert!(duration(end).is_err());
}
