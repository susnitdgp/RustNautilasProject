use crate::indicators::{Atr, Rma, RollingMedian};
use crate::*;
use chrono::{FixedOffset, NaiveDate, TimeZone};

const MIN: i64 = 60_000_000_000;

/// Builds consecutive 1-minute IST candles starting at `day` `hh:mm`.
struct Tape {
    t: i64,
    bars: Vec<BarInput>,
}
impl Tape {
    fn at(day: u32, hh: u32, mm: u32) -> Self {
        let ist = FixedOffset::east_opt(19_800).unwrap();
        let d = NaiveDate::from_ymd_opt(2026, 9, day)
            .unwrap()
            .and_hms_opt(hh, mm, 0)
            .unwrap();
        Self {
            t: ist
                .from_local_datetime(&d)
                .unwrap()
                .timestamp_nanos_opt()
                .unwrap(),
            bars: Vec::new(),
        }
    }
    fn push(&mut self, o: f64, h: f64, l: f64, c: f64) -> BarInput {
        let b = BarInput {
            open_time_ns: self.t,
            close_time_ns: self.t + MIN,
            open: o,
            high: h,
            low: l,
            close: c,
        };
        self.t += MIN;
        self.bars.push(b);
        b
    }
}

/// Prior day of 10-point candles (warms ATRs and the 50-bar median), then a day
/// that trends up, coils tightly near the session high and breaks down.
/// Returns the engine fed up to (not including) the breakdown bar, and that bar.
fn coil_at_high(day1_start: (u32, u32)) -> (Engine, Tape, BarInput) {
    let mut e = Engine::new(Params::default()).unwrap();
    let mut prev = Tape::at(14, 9, 0);
    for _ in 0..60 {
        let b = prev.push(100.0, 105.0, 95.0, 100.0);
        assert!(e.on_bar(&b).is_empty());
    }
    let mut tape = Tape::at(15, day1_start.0, day1_start.1);
    for i in 0..30 {
        let c = 100.0 + 2.0 * i as f64;
        let b = tape.push(c - 2.0, c + 5.0, c - 5.0, c);
        assert!(e.on_bar(&b).is_empty(), "no signal while trending");
    }
    for _ in 0..8 {
        let b = tape.push(158.0, 158.5, 157.5, 158.0);
        assert!(e.on_bar(&b).is_empty(), "no signal inside the coil");
    }
    let s = e.status();
    assert!(
        s.watch_active && s.watch_is_short,
        "coil at session high is watched for a short"
    );
    let breakdown = tape.push(158.0, 158.2, 128.0, 129.0);
    (e, tape, breakdown)
}

fn entry(events: &[Event]) -> Levels {
    match events {
        [
            Event::Entry {
                action: Action::Short,
                price,
                levels,
                ..
            },
        ] => {
            assert_eq!(*price, 129.0);
            *levels
        }
        other => panic!("expected one SHORT entry, got {other:?}"),
    }
}

#[test]
fn rma_seeds_with_sma_then_smooths() {
    let mut r = Rma::new(3);
    assert_eq!(r.update(3.0), None);
    assert_eq!(r.update(6.0), None);
    assert_eq!(r.update(9.0), Some(6.0));
    assert!((r.update(12.0).unwrap() - (12.0 / 3.0 + 6.0 * 2.0 / 3.0)).abs() < 1e-12);
}

#[test]
fn atr_uses_high_low_on_first_bar_then_true_range() {
    let mut a = Atr::new(2);
    assert_eq!(a.update(10.0, 8.0, 9.0), None);
    // gap up: TR = |high - prev close| = 15 - 9 = 6
    assert_eq!(a.update(15.0, 14.0, 14.5), Some(4.0));
}

#[test]
fn median_is_na_until_window_full() {
    let mut m = RollingMedian::new(4);
    for x in [4.0, 1.0, 3.0] {
        assert_eq!(m.update(x), None);
    }
    assert_eq!(m.update(2.0), Some(2.5));
    assert_eq!(m.update(10.0), Some(2.5)); // window 1,3,2,10
}

#[test]
fn presets_and_input_limits_match_pine() {
    let p = Params::default();
    let r = p.resolve();
    assert_eq!(
        (
            r.comp_min_bars,
            r.comp_max_bars,
            r.touch_mode,
            r.watch_expiry
        ),
        (4, 14, TouchMode::Edge, 10)
    );
    let c = Params {
        sensitivity: Sensitivity::Conservative,
        ..Params::default()
    }
    .resolve();
    assert_eq!(
        (c.comp_min_bars, c.max_violations, c.touch_mode),
        (5, 1, TouchMode::Full)
    );
    let manual = Params {
        sensitivity: Sensitivity::Aggressive,
        overrides: Some(ManualOverrides {
            comp_min_bars: 6,
            ..ManualOverrides::default()
        }),
        ..Params::default()
    };
    assert_eq!(manual.resolve().touch_mode, TouchMode::Edge);
    assert!(
        Params {
            tp1_r: 4.0,
            ..Params::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        Engine::new(Params {
            atr_period: 2,
            ..Params::default()
        })
        .is_err()
    );
}

#[test]
fn short_on_coil_breakdown_then_cover_at_tp1() {
    let (mut e, mut tape, breakdown) = coil_at_high((9, 0));
    let t = entry(&e.on_bar(&breakdown));
    assert_eq!(t.side, Side::Short);
    assert!(t.sl > t.entry && t.tp1 < t.entry);
    assert!((t.entry - t.tp1 - t.risk).abs() < 1e-9, "TP1 is 1R");
    assert!((t.sl - t.entry - t.risk).abs() < 1e-9);
    assert_eq!(e.status().state, "ACTIVE SHORT");

    let next = tape.push(129.0, 130.0, 110.0, 112.0);
    match e.on_bar(&next).as_slice() {
        [
            Event::Exit {
                action: Action::Cover,
                reason: ExitReason::Target(ExitTarget::Tp1),
                price,
                intrabar: false,
                ..
            },
        ] => {
            assert_eq!(*price, t.tp1)
        }
        other => panic!("expected COVER at TP1, got {other:?}"),
    }
    assert!(e.position().is_none());
}

#[test]
fn stop_wins_when_sl_and_target_touch_on_same_bar() {
    let (mut e, mut tape, breakdown) = coil_at_high((9, 0));
    let t = entry(&e.on_bar(&breakdown));
    let wide = tape.push(129.0, t.sl + 1.0, t.tp1 - 1.0, 129.0);
    match e.on_bar(&wide).as_slice() {
        [
            Event::Exit {
                reason: ExitReason::StopLoss,
                price,
                ..
            },
        ] => assert_eq!(*price, t.sl),
        other => panic!("expected SL, got {other:?}"),
    }
}

#[test]
fn intrabar_touch_exits_once_and_gate_blocks_reentry_that_bar() {
    let (mut e, mut tape, breakdown) = coil_at_high((9, 0));
    let t = entry(&e.on_bar(&breakdown));
    let ev = e
        .on_price(t.sl + 0.5, breakdown.close_time_ns + 1)
        .expect("stop touched intrabar");
    assert!(matches!(
        ev,
        Event::Exit {
            action: Action::Cover,
            reason: ExitReason::StopLoss,
            intrabar: true,
            ..
        }
    ));
    assert!(
        e.on_price(t.sl + 9.0, breakdown.close_time_ns + 2)
            .is_none(),
        "fires once"
    );
    let closed = tape.push(129.0, t.sl + 1.0, 128.0, 140.0);
    assert!(
        e.on_bar(&closed).is_empty(),
        "bar close repeats no exit and takes no entry"
    );
}

#[test]
fn eod_square_off_and_no_entry_after_cutoff() {
    // Breakdown bar closes 23:12 IST; the 23:14–23:15 bar squares off.
    let (mut e, mut tape, breakdown) = coil_at_high((22, 33));
    assert_eq!(
        breakdown.close_time_ns - tape.bars[0].open_time_ns,
        39 * MIN
    );
    entry(&e.on_bar(&breakdown));
    for _ in 0..2 {
        let b = tape.push(129.0, 129.5, 128.5, 129.0);
        assert!(e.on_bar(&b).is_empty());
    }
    let cutoff = tape.push(129.0, 129.5, 128.5, 129.2);
    match e.on_bar(&cutoff).as_slice() {
        [
            Event::Exit {
                reason: ExitReason::EndOfDay,
                price,
                ..
            },
        ] => assert_eq!(*price, 129.2),
        other => panic!("expected EOD exit, got {other:?}"),
    }
    assert_eq!(e.status().state, "EOD · NO ENTRY");

    let (mut late, _, breakdown) = coil_at_high((22, 37)); // breakdown closes 23:16
    assert!(
        late.on_bar(&breakdown).is_empty(),
        "no entries at/after the cut-off"
    );
}

#[test]
fn new_day_resets_session_counter_and_watch() {
    let (mut e, _, _) = coil_at_high((9, 0));
    let mut next_day = Tape::at(16, 9, 0);
    let b = next_day.push(158.0, 158.5, 157.5, 158.0);
    e.on_bar(&b);
    let s = e.status();
    assert_eq!(s.session_bars, 1);
    assert!(!s.watch_active);
    assert_eq!(s.state, "WARM-UP 1/15");
}

#[test]
fn engine_state_round_trips_through_json() {
    let (e, _, breakdown) = coil_at_high((9, 0));
    let mut a = e.clone();
    let mut b: Engine = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
    assert_eq!(a.on_bar(&breakdown), b.on_bar(&breakdown));
}
