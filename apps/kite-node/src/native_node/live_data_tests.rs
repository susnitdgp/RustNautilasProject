use super::super::history_revision::History;
use super::*;

fn fixture(interval: Interval) -> (Vec<Candle>, History, chrono::NaiveDate, u64) {
    let date = chrono::NaiveDate::from_ymd_opt(2026, 9, 23).unwrap();
    let calendar = super::super::session_calendar::fixture();
    let (start, _) = calendar.bounds(date).unwrap();
    let rows: Vec<_> = (0..105)
        .map(|i| Candle {
            timestamp: chrono::DateTime::from_timestamp_nanos(
                (start + i * interval.nanoseconds()) as i64,
            )
            .with_timezone(&chrono::FixedOffset::east_opt(19800).unwrap())
            .to_rfc3339(),
            open: 6000.,
            high: 6010.,
            low: 5990.,
            close: 6001.,
            volume: 100,
            oi: 100,
        })
        .collect();
    let history = History::new_for(&rows[..100], calendar, interval)
        .unwrap()
        .with_volume_sensitive(false);
    (rows, history, date, start)
}

#[test]
fn unpublished_tail_waits_without_replaying_or_committing_then_emits_once() {
    for interval in [Interval::ThreeMinute, Interval::FiveMinute] {
        let (rows, mut history, date, start) = fixture(interval);
        let step = interval.nanoseconds();
        let requested = start + 101 * step + timing::COMPLETION_GRACE_NS;
        assert!(
            prepare_update(
                &mut history,
                rows[..100].to_vec(),
                requested,
                date,
                interval
            )
            .unwrap()
            .is_none()
        );
        assert_eq!(history.latest_close(), start + 100 * step);
        let update = prepare_update(
            &mut history,
            rows[..101].to_vec(),
            requested + 500_000_000,
            date,
            interval,
        )
        .unwrap()
        .unwrap();
        assert_eq!(update.new.len(), 1);
        assert_eq!(update.revised, 0);
        let again = prepare_update(
            &mut history,
            rows[..101].to_vec(),
            requested + 1_000_000_000,
            date,
            interval,
        )
        .unwrap()
        .unwrap();
        assert!(again.new.is_empty());
    }
}

#[test]
fn stale_tail_exhaustion_and_new_internal_gaps_fail_without_history_mutation() {
    for interval in [Interval::ThreeMinute, Interval::FiveMinute] {
        let (rows, mut history, date, start) = fixture(interval);
        let step = interval.nanoseconds();
        let requested = start + 101 * step + timing::COMPLETION_GRACE_NS;
        assert!(
            prepare_update(
                &mut history,
                rows[..100].to_vec(),
                requested + 15_000_000_000,
                date,
                interval
            )
            .is_err()
        );
        let mut gap = rows[..103].to_vec();
        gap.remove(101);
        assert!(
            prepare_update(
                &mut history,
                gap,
                start + 103 * step + timing::COMPLETION_GRACE_NS,
                date,
                interval
            )
            .is_err()
        );
        assert_eq!(history.latest_close(), start + 100 * step);
    }
}

#[test]
fn crossing_boundary_during_a_request_cannot_admit_the_unfinished_next_bar() {
    for interval in [Interval::ThreeMinute, Interval::FiveMinute] {
        let (rows, mut history, date, start) = fixture(interval);
        let step = interval.nanoseconds();
        let requested = start + 102 * step + timing::COMPLETION_GRACE_NS - 1;
        let update = prepare_update(
            &mut history,
            rows[..102].to_vec(),
            requested,
            date,
            interval,
        )
        .unwrap()
        .unwrap();
        assert_eq!(update.new.len(), 1);
        assert_eq!(history.latest_close(), start + 101 * step);
    }
}
