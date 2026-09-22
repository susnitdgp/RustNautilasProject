use super::*;
fn settings() -> Settings {
    serde_json::from_value(
        serde_json::json!({"pivot_period":2,"atr_period":10,"atr_factor":3.,
        "session":{"start":"09:00:00","end":"23:15:00","days":"23456","reset_daily":true}}),
    )
    .unwrap()
}
fn ts(text: &str) -> u64 {
    chrono::DateTime::parse_from_rfc3339(text)
        .unwrap()
        .timestamp_nanos_opt()
        .unwrap() as u64
}
fn series() -> Vec<(f64, f64, f64, u64)> {
    let start = ts("2026-09-22T09:05:00+05:30");
    (0..174)
        .map(|i| {
            let x = i % 40;
            let close = 6000.
                + if x < 20 {
                    x as f64 * 10.
                } else {
                    (40 - x) as f64 * 10.
                };
            (close + 5., close - 5., close, start + i as u64 * BAR_NS)
        })
        .collect()
}
#[test]
fn pine_atr_uses_sma_seed_wilder_updates_and_gap_true_range() {
    let mut a = Atr::new(3);
    assert_eq!(a.update(101., 99., 100.), None);
    assert_eq!(a.update(102., 98., 100.), None);
    assert_eq!(a.update(104., 96., 100.), Some(14. / 3.));
    assert!((a.update(101., 99., 100.).unwrap() - 34. / 9.).abs() < 1e-12);
    let previous = a.value.unwrap();
    assert!((a.update(121., 119., 120.).unwrap() - (2. * previous + 21.) / 3.).abs() < 1e-12);
}
#[test]
fn pivots_wait_for_right_bars_and_center_uses_two_to_one_weighting() {
    let mut cfg = settings();
    cfg.atr_period = 1;
    let mut p = PivotPoint::new(cfg, super::super::session_calendar::fixture()).unwrap();
    let start = ts("2026-09-22T09:05:00+05:30");
    let rows = [
        (100., 98.),
        (103., 101.),
        (110., 108.),
        (105., 103.),
        (104., 102.),
        (103., 100.),
        (105., 102.),
        (108., 105.),
    ];
    for (i, (h, l)) in rows.into_iter().enumerate() {
        let o = p
            .update(h, l, (h + l) / 2., start + i as u64 * BAR_NS)
            .unwrap();
        if i < 4 {
            assert!(o.pivot_high.is_none());
            assert!(o.center.is_none());
        }
        if i == 4 {
            assert_eq!(o.pivot_high, Some(110.));
            assert_eq!(o.center, Some(110.));
        }
        if i == 7 {
            assert_eq!(o.pivot_low, Some(100.));
            assert_eq!(o.center, Some(320. / 3.));
        }
    }
}
#[test]
fn tied_pivots_select_rightmost_extreme_and_high_wins_simultaneous_confirmation() {
    let mut cfg = settings();
    cfg.atr_period = 1;
    cfg.pivot_period = 1;
    let mut p = PivotPoint::new(cfg.clone(), super::super::session_calendar::fixture()).unwrap();
    let start = ts("2026-09-22T09:05:00+05:30");
    for (i, (h, l)) in [(101., 99.), (110., 90.), (110., 91.), (105., 95.)]
        .into_iter()
        .enumerate()
    {
        let o = p.update(h, l, 100., start + i as u64 * BAR_NS).unwrap();
        if i == 2 {
            assert!(o.pivot_high.is_none());
        }
        if i == 3 {
            assert_eq!(o.pivot_high, Some(110.));
        }
    }
    let mut p = PivotPoint::new(cfg, super::super::session_calendar::fixture()).unwrap();
    p.update(101., 99., 100., start).unwrap();
    p.update(110., 90., 100., start + BAR_NS).unwrap();
    let o = p.update(101., 99., 100., start + 2 * BAR_NS).unwrap();
    assert_eq!(
        (o.pivot_high, o.pivot_low, o.center),
        (Some(110.), Some(90.), Some(110.))
    );
}
#[test]
fn daily_reset_handles_missing_overnight_bars_and_clears_prior_session_bands() {
    let mut p = PivotPoint::new(settings(), super::super::session_calendar::fixture()).unwrap();
    for (h, l, c, t) in series() {
        p.update(h, l, c, t).unwrap();
    }
    assert!(p.center.is_some());
    let o = p
        .update(6301., 6299., 6300., ts("2026-09-23T09:05:00+05:30"))
        .unwrap();
    assert!(o.new_session);
    assert_eq!(o.center, None);
    assert_eq!(o.direction, 0);
    assert_eq!(o.signal, 0);
    assert!(o.atr.is_some());
    assert!(!o.initialized);
    assert_eq!(p.window.len(), 1);
    let mut cfg = settings();
    cfg.session.reset_daily = false;
    let mut p = PivotPoint::new(cfg, super::super::session_calendar::fixture()).unwrap();
    for (h, l, c, t) in series() {
        p.update(h, l, c, t).unwrap();
    }
    let o = p
        .update(6301., 6299., 6300., ts("2026-09-23T09:05:00+05:30"))
        .unwrap();
    assert!(o.new_session && o.center.is_some() && o.initialized);
}
#[test]
fn flips_are_causal_and_rebuild_reproduces_the_same_observations() {
    let cfg = settings();
    let calendar = super::super::session_calendar::fixture();
    let mut p = PivotPoint::new(cfg.clone(), calendar.clone()).unwrap();
    let mut observations = vec![];
    for (h, l, c, t) in series() {
        observations.push(p.update(h, l, c, t).unwrap());
    }
    assert!(observations.iter().any(|o| o.signal == 1));
    assert!(observations.iter().any(|o| o.signal == -1));
    let mut rebuilt = p.rebuild_empty().unwrap();
    for ((h, l, c, t), expected) in series().into_iter().zip(&observations) {
        assert_eq!(
            serde_json::to_value(rebuilt.update(h, l, c, t).unwrap()).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
    }
    for n in [5, 20, 60, 120] {
        let mut prefix = PivotPoint::new(cfg.clone(), calendar.clone()).unwrap();
        for ((h, l, c, t), expected) in series().into_iter().take(n).zip(&observations) {
            assert_eq!(
                serde_json::to_value(prefix.update(h, l, c, t).unwrap()).unwrap(),
                serde_json::to_value(expected).unwrap()
            );
        }
    }
}
#[test]
fn square_off_cutoff_weekdays_holidays_and_late_sessions_are_explicit() {
    let s = settings();
    let c = super::super::session_calendar::fixture();
    assert!(
        s.session
            .contains(ts("2026-09-22T09:00:00+05:30"), &c)
            .unwrap()
    );
    assert!(
        !s.session
            .contains(ts("2026-09-22T08:59:59+05:30"), &c)
            .unwrap()
    );
    assert!(
        !s.session
            .contains(ts("2026-09-22T23:15:00+05:30"), &c)
            .unwrap()
    );
    assert!(
        !s.session
            .contains(ts("2026-09-26T12:00:00+05:30"), &c)
            .unwrap()
    );
    assert!(
        !s.session
            .contains(ts("2026-10-02T12:00:00+05:30"), &c)
            .unwrap()
    );
    assert!(
        !s.session
            .contains(ts("2026-09-14T10:00:00+05:30"), &c)
            .unwrap()
    );
    assert!(
        s.session
            .contains(ts("2026-09-14T17:00:00+05:30"), &c)
            .unwrap()
    );
    let mut p = PivotPoint::new(s, c).unwrap();
    for (h, l, c, t) in series() {
        let o = p.update(h, l, c, t).unwrap();
        if t >= ts("2026-09-22T23:15:00+05:30") {
            assert!(!o.in_session);
            assert_eq!(o.signal, 0);
        }
    }
}
#[test]
fn invalid_settings_duplicate_bars_and_bad_prices_fail_before_state_changes() {
    for period in [0, 51] {
        let mut s = settings();
        s.pivot_period = period;
        assert!(s.validate().is_err());
    }
    let mut s = settings();
    s.atr_factor = f64::NAN;
    assert!(s.validate().is_err());
    let mut s = settings();
    s.session.days = "22".into();
    assert!(s.validate().is_err());
    let mut s = settings();
    s.session.end = "08:00:00".parse().unwrap();
    assert!(s.validate().is_err());
    let mut p = PivotPoint::new(settings(), super::super::session_calendar::fixture()).unwrap();
    let t = ts("2026-09-22T09:05:00+05:30");
    p.update(101., 99., 100., t).unwrap();
    assert!(p.update(101., 99., 100., t).is_err());
    assert!(p.update(f64::NAN, 99., 100., t + BAR_NS).is_err());
    assert_eq!(p.last_bar, t);
}
