use super::super::history_revision::{History, Update};
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
    let history = History::new_for(&rows[..100], calendar, interval).unwrap();
    (rows, history, date, start)
}

#[test]
fn newly_closed_bar_needs_delay_and_two_stable_broker_reads() {
    for interval in [Interval::ThreeMinute, Interval::FiveMinute] {
        let (rows, mut history, date, start) = fixture(interval);
        let step = interval.nanoseconds();
        let close = start + 101 * step;
        let mut stability = TailStability::default();

        // A bar visible at close+2s is still provisional and must not be committed.
        assert!(
            prepare_update(
                &mut history,
                &mut stability,
                rows[..101].to_vec(),
                close + timing::COMPLETION_GRACE_NS,
                date,
                interval,
            )
            .unwrap()
            .is_none()
        );
        assert_eq!(history.latest_close(), start + 100 * step);

        // First read after the finalization delay only establishes the candidate.
        assert!(
            prepare_update(
                &mut history,
                &mut stability,
                rows[..101].to_vec(),
                close + timing::FINALIZATION_DELAY_NS,
                date,
                interval,
            )
            .unwrap()
            .is_none()
        );
        assert!(
            prepare_update(
                &mut history,
                &mut stability,
                rows[..101].to_vec(),
                close + timing::FINALIZATION_DELAY_NS + 1_000_000_000,
                date,
                interval,
            )
            .unwrap()
            .is_none()
        );

        // Only a second identical read separated by the stability interval admits it.
        let update = prepare_update(
            &mut history,
            &mut stability,
            rows[..101].to_vec(),
            close + timing::FINALIZATION_DELAY_NS + timing::STABILITY_CONFIRM_NS,
            date,
            interval,
        )
        .unwrap()
        .unwrap();
        assert_eq!(update.new.len(), 1);
        assert_eq!(update.price_revised, 0);
        assert_eq!(history.latest_close(), close);

        let again = prepare_update(
            &mut history,
            &mut stability,
            rows[..101].to_vec(),
            close + timing::FINALIZATION_DELAY_NS + timing::STABILITY_CONFIRM_NS + 1_000_000_000,
            date,
            interval,
        )
        .unwrap()
        .unwrap();
        assert!(again.new.is_empty());
    }
}

#[test]
fn broker_ohlc_revision_before_admission_resets_stability_clock() {
    for interval in [Interval::ThreeMinute, Interval::FiveMinute] {
        let (rows, mut history, date, start) = fixture(interval);
        let step = interval.nanoseconds();
        let close = start + 101 * step;
        let mut stability = TailStability::default();

        assert!(
            prepare_update(
                &mut history,
                &mut stability,
                rows[..101].to_vec(),
                close + timing::FINALIZATION_DELAY_NS,
                date,
                interval,
            )
            .unwrap()
            .is_none()
        );

        // Simulate exactly what happened live: Kite changes the just-published OHLC.
        let mut revised = rows[..101].to_vec();
        revised[100].high += 7.0;
        revised[100].close += 4.0;
        assert!(
            prepare_update(
                &mut history,
                &mut stability,
                revised.clone(),
                close + timing::FINALIZATION_DELAY_NS + 1_000_000_000,
                date,
                interval,
            )
            .unwrap()
            .is_none()
        );
        assert!(
            prepare_update(
                &mut history,
                &mut stability,
                revised.clone(),
                close + timing::FINALIZATION_DELAY_NS + 2_000_000_000,
                date,
                interval,
            )
            .unwrap()
            .is_none()
        );
        let update = prepare_update(
            &mut history,
            &mut stability,
            revised,
            close + timing::FINALIZATION_DELAY_NS + 3_000_000_000,
            date,
            interval,
        )
        .unwrap()
        .unwrap();
        assert_eq!(update.new.len(), 1);
        assert_eq!(update.new[0].close, 6005.0);
        assert_eq!(history.latest_close(), close);
    }
}

#[test]
fn stale_tail_exhaustion_and_multi_bar_catchup_fail_without_history_mutation() {
    for interval in [Interval::ThreeMinute, Interval::FiveMinute] {
        let (rows, mut history, date, start) = fixture(interval);
        let step = interval.nanoseconds();
        let close = start + 101 * step;
        let mut stability = TailStability::default();

        assert!(
            prepare_update(
                &mut history,
                &mut stability,
                rows[..100].to_vec(),
                close + 78_000_000_000,
                date,
                interval,
            )
            .is_err()
        );
        assert_eq!(history.latest_close(), start + 100 * step);

        // Catch-up across multiple completed bars is never converted into old orders.
        assert!(
            prepare_update(
                &mut history,
                &mut stability,
                rows[..103].to_vec(),
                start + 103 * step + timing::FINALIZATION_DELAY_NS,
                date,
                interval,
            )
            .is_err()
        );
        assert_eq!(history.latest_close(), start + 100 * step);
    }
}

#[test]
fn crossing_next_boundary_cannot_admit_the_unfinished_following_bar() {
    for interval in [Interval::ThreeMinute, Interval::FiveMinute] {
        let (rows, mut history, date, start) = fixture(interval);
        let step = interval.nanoseconds();
        let close = start + 101 * step;
        let mut stability = TailStability::default();

        assert!(
            prepare_update(
                &mut history,
                &mut stability,
                rows[..102].to_vec(),
                close + timing::FINALIZATION_DELAY_NS,
                date,
                interval,
            )
            .unwrap()
            .is_none()
        );
        let update = prepare_update(
            &mut history,
            &mut stability,
            rows[..102].to_vec(),
            close + timing::FINALIZATION_DELAY_NS + timing::STABILITY_CONFIRM_NS,
            date,
            interval,
        )
        .unwrap()
        .unwrap();
        assert_eq!(update.new.len(), 1);
        assert_eq!(history.latest_close(), close);
    }
}

fn empty_update() -> Update {
    Update {
        new: vec![],
        price_revised: 0,
        volume_only_revised: 0,
        revision_samples: vec![],
    }
}

#[test]
fn live_update_validation_is_fail_closed_for_revisions_and_catchup() {
    let mut update = empty_update();
    assert!(validate_live_update(&update, false).is_ok());

    update.price_revised = 1;
    assert!(validate_live_update(&update, false).is_err());

    let mut update = empty_update();
    update.new.push(Candle {
        timestamp: "2026-09-23T09:00:00+05:30".into(),
        open: 1.0,
        high: 1.0,
        low: 1.0,
        close: 1.0,
        volume: 1,
        oi: 1,
    });
    assert!(validate_live_update(&update, true).is_err());
}

#[test]
fn market_open_waits_for_first_real_candle_finalization() {
    for interval in [Interval::ThreeMinute, Interval::FiveMinute] {
        let date = chrono::NaiveDate::from_ymd_opt(2026, 9, 23).unwrap();
        let calendar = super::super::session_calendar::fixture();
        let (start, _) = calendar.bounds(date).unwrap();
        let step = interval.nanoseconds();
        let previous_close = start.saturating_sub(step);
        assert_eq!(
            next_live_poll(
                start + 10_000_000_000,
                previous_close,
                step,
                date,
                &calendar
            )
            .unwrap(),
            start + step + timing::FINALIZATION_DELAY_NS
        );
    }
}
