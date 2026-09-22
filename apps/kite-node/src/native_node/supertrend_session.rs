//! Run only the configured contract's current session; stop before market close.
use anyhow::{Result, ensure};
pub const EXIT_BUFFER_SECONDS: u64 = 30 * 60;
pub fn duration(now: u64, calendar: &super::session_calendar::Calendar) -> Result<u64> {
    let date = chrono::DateTime::from_timestamp_nanos(now as i64)
        .with_timezone(&chrono::FixedOffset::east_opt(19800).unwrap())
        .date_naive();
    let (start, end) = calendar.bounds(date)?;
    let stop = end - EXIT_BUFFER_SECONDS * 1_000_000_000;
    ensure!(
        now >= start && now + 5_000_000_000 < stop,
        "Session mode starts during trading hours and before the MIS application exit window"
    );
    Ok((stop - now) / 1_000_000_000)
}
pub fn run(config: &str) -> Result<()> {
    let selection = super::production::Selection::load(config)?;
    super::supertrend_live_runner::run(
        config,
        duration(super::data::now(), &selection.session_calendar)?,
        false,
    )
}
#[test]
fn session_deadline_is_before_market_close() {
    let date = chrono::NaiveDate::from_ymd_opt(2026, 9, 16).unwrap();
    let calendar = super::session_calendar::fixture();
    let (start, end) = calendar.bounds(date).unwrap();
    assert_eq!(
        duration(start, &calendar).unwrap(),
        (end - start) / 1_000_000_000 - EXIT_BUFFER_SECONDS
    );
    assert!(duration(start - 1, &calendar).is_err());
    assert!(duration(end, &calendar).is_err());
}

#[test]
fn json_calendar_allows_october_contract_sessions_and_rejects_holidays() {
    let calendar = super::session_calendar::fixture();
    for date in ["2026-09-22", "2026-10-19"] {
        let date = date.parse().unwrap();
        let (start, end) = calendar.bounds(date).unwrap();
        assert_eq!(
            duration(start, &calendar).unwrap(),
            (end - start) / 1_000_000_000 - EXIT_BUFFER_SECONDS
        );
        assert!(duration(end - EXIT_BUFFER_SECONDS * 1_000_000_000, &calendar).is_err());
    }
    for date in ["2026-10-02", "2026-10-03", "2026-10-20"] {
        let time = chrono::DateTime::parse_from_rfc3339(&format!("{date}T09:00:00+05:30")).unwrap();
        assert!(duration(time.timestamp_nanos_opt().unwrap() as u64, &calendar).is_err());
    }
}
