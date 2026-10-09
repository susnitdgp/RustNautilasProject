use crate::{Bar, DayRisk, Engine, ExitReason, Params, Side, Trade};
use chrono::{NaiveDate, TimeZone, Utc};

/// 2026-10-09 09:00 IST as epoch seconds.
fn session_open() -> i64 {
    Utc.with_ymd_and_hms(2026, 10, 9, 3, 30, 0).unwrap().timestamp()
}
fn bar(i: i64, o: f64, h: f64, l: f64, c: f64, v: f64) -> Bar {
    Bar { start: session_open() + i * 60, seconds: 60, open: o, high: h, low: l, close: c, volume: v }
}
/// 30 quiet bars 100-102 (ATR 2, volume 10) from 09:00.
fn quiet(e: &mut Engine) {
    for i in 0..30 {
        assert!(e.on_bar(bar(i, 101.0, 102.0, 100.0, 101.0, 10.0)).is_none());
    }
}

#[test]
fn burst_out_of_tight_base_gives_long_signal() {
    let mut e = Engine::new(Params::default());
    quiet(&mut e);
    let s = e.on_bar(bar(30, 102.0, 108.0, 101.5, 107.5, 50.0)).expect("burst signal");
    assert_eq!(s.side, Side::Long);
    assert_eq!(s.stop, 99.0, "base low 100 minus 1 point buffer");
    assert_eq!(s.risk, 8.5);
    assert!((s.volume_ratio - 5.0).abs() < 1e-9);
}

#[test]
fn burst_needs_volume_a_strong_close_and_a_capped_stop() {
    let mut weak_volume = Engine::new(Params::default());
    quiet(&mut weak_volume);
    assert!(weak_volume.on_bar(bar(30, 102.0, 108.0, 101.5, 107.5, 15.0)).is_none());

    let mut wick = Engine::new(Params::default());
    quiet(&mut wick);
    assert!(wick.on_bar(bar(30, 102.0, 108.0, 101.5, 103.0, 50.0)).is_none(), "closes low in its range");

    let mut wide = Engine::new(Params { stop_cap_points: 5.0, stop_min_points: 2.0, ..Params::default() });
    quiet(&mut wide);
    assert!(wide.on_bar(bar(30, 102.0, 108.0, 101.5, 107.5, 50.0)).is_none(), "stop 8.5 > cap 5");

    let mut short = Engine::new(Params::default());
    quiet(&mut short);
    let s = short.on_bar(bar(30, 100.0, 100.5, 94.0, 94.5, 50.0)).expect("short burst");
    assert_eq!((s.side, s.stop), (Side::Short, 103.0));
}

#[test]
fn no_entries_outside_the_window_or_into_a_close_level() {
    let early = Params { entry_from: chrono::NaiveTime::from_hms_opt(10, 0, 0).unwrap(), ..Params::default() };
    let mut e = Engine::new(early);
    quiet(&mut e);
    assert!(e.on_bar(bar(30, 102.0, 108.0, 101.5, 107.5, 50.0)).is_none(), "09:31 is before 10:00");

    // Previous day high at 110 sits 2.5 points above the close: no room for the target.
    let mut room = Engine::new(Params::default());
    let prev = session_open() - 86_400;
    for i in 0..30 {
        let b = Bar { start: prev + i * 60, ..bar(0, 101.0, if i == 5 { 110.0 } else { 102.0 }, 100.0, 101.0, 10.0) };
        room.on_bar(b);
    }
    quiet(&mut room);
    assert!(room.on_bar(bar(30, 102.0, 108.0, 101.5, 107.5, 50.0)).is_none());
}

#[test]
fn trade_exits_are_pessimistic_and_labelled_by_stop_stage() {
    let p = Params::default();
    let t = Trade::open(Side::Long, 100.0, 92.0, &p).unwrap();
    assert_eq!(t.target, 112.0, "1.5R of 8");
    assert_eq!(t.on_bar(101.0, 113.0, 91.0), Some((ExitReason::Stop, 92.0)), "both touched: stop first");
    assert_eq!(t.on_bar(90.0, 95.0, 89.0), Some((ExitReason::Stop, 90.0)), "gap below the stop fills at the open");
    assert_eq!(t.on_bar(101.0, 113.0, 99.0), Some((ExitReason::Target, 112.0)));
    assert_eq!(t.on_price(112.0), Some(ExitReason::Target));
    assert!(Trade::open(Side::Long, 100.0, 101.0, &p).is_none(), "entry already through the stop");

    let mut be = Trade::open(Side::Long, 100.0, 92.0, &p).unwrap();
    assert!(be.on_bar_close(108.5, 100.0, 104.0, Some(99.0), &p).is_none());
    assert_eq!(be.stop, 102.0, "breakeven + 2 at 1R; swing 99 is below it");
    assert_eq!(be.on_price(102.0), Some(ExitReason::Breakeven));
    assert!(be.on_bar_close(110.0, 104.0, 109.0, Some(105.0), &p).is_none());
    assert_eq!(be.stop, 105.0);
    assert_eq!(be.on_price(105.0), Some(ExitReason::Trail));

    let mut slow = Trade::open(Side::Short, 100.0, 108.0, &p).unwrap();
    for _ in 0..4 {
        assert!(slow.on_bar_close(101.0, 99.0, 100.0, Some(101.0), &p).is_none());
    }
    assert_eq!(slow.on_bar_close(101.0, 99.0, 100.0, Some(101.0), &p), Some(ExitReason::Time));
}

#[test]
fn day_risk_limits_and_reset() {
    let p = Params { max_trades_per_day: 2, max_consecutive_losses: 2, daily_loss_limit_points: 20.0, ..Params::default() };
    let d1 = NaiveDate::from_ymd_opt(2026, 10, 9).unwrap();
    let d2 = d1.succ_opt().unwrap();
    let mut r = DayRisk::default();
    assert!(r.allowed(d1, &p).is_ok());
    r.on_entry(d1);
    r.on_exit(d1, -5.0);
    r.on_entry(d1);
    r.on_exit(d1, -6.0);
    assert_eq!(r.allowed(d1, &p), Err("daily trade limit"));
    assert!(r.allowed(d2, &p).is_ok(), "new day resets");
    r.on_entry(d2);
    r.on_exit(d2, -25.0);
    assert_eq!(r.allowed(d2, &p), Err("daily loss limit"));
}

#[test]
fn params_defaults_validate_and_json_round_trips() {
    let p = Params::default();
    p.validate().unwrap();
    let back: Params = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
    assert_eq!(back, p);
    let empty: Params = serde_json::from_str("{}").unwrap();
    assert_eq!(empty, p);
    assert!(serde_json::from_str::<Params>(r#"{"nope": 1}"#).is_err());
}
