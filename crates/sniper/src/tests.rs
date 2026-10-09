use crate::{
    Bar, Engine, Event, Params,
    params::Preset,
    ta::{Rsi, Smoothed},
};

#[test]
fn ema_and_rma_seed_with_sma_like_pine() {
    let mut e = Smoothed::ema(3);
    assert_eq!(e.update(1.0), None);
    assert_eq!(e.update(2.0), None);
    assert_eq!(e.update(3.0), Some(2.0), "SMA seed of the first 3");
    assert_eq!(e.update(4.0), Some(3.0), "0.5 * 4 + 0.5 * 2");
    let mut r = Smoothed::rma(2);
    r.update(2.0);
    assert_eq!(r.update(4.0), Some(3.0));
    assert_eq!(r.update(5.0), Some(4.0));
    let mut rsi = Rsi::new(3);
    let vals: Vec<_> = [1.0, 2.0, 3.0, 4.0, 5.0].iter().map(|x| rsi.update(*x)).collect();
    assert_eq!(vals[4], Some(100.0), "only gains");
}

#[test]
fn auto_preset_resolves_by_timeframe() {
    let mut p = Params::default();
    assert_eq!(p.resolve().preset, Preset::Scalping);
    assert_eq!((p.resolve().fast, p.resolve().slow, p.resolve().trend, p.resolve().sl_mult), (5, 13, 34, 0.8));
    p.timeframe_minutes = 15;
    assert_eq!(p.resolve().preset, Preset::Default);
    p.timeframe_minutes = 240;
    assert_eq!(p.resolve().preset, Preset::Swing);
    assert!((Params::default().required_ratio() - 0.5).abs() < 1e-12, "hide C lifts Scalping 0.4 to 0.5");
    let back: Params = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
    assert_eq!(back, p);
}

/// A deterministic wavy market with trends and pullbacks; checks model invariants.
#[test]
fn trades_alternate_and_respect_their_geometry() {
    let mut e = Engine::new(Params::default());
    let mut open: Option<(i32, f64, f64, f64)> = None;
    let (mut entries, mut exits) = (0, 0);
    let mut px = 8000.0_f64;
    for i in 0..3000_i64 {
        let drift = (i as f64 / 37.0).sin() * 6.0 + (i as f64 / 11.0).cos() * 3.0;
        let o = px;
        let c = (px + drift).round();
        let h = o.max(c) + 2.0 + ((i % 5) as f64);
        let l = o.min(c) - 2.0 - ((i % 3) as f64);
        px = c;
        let bar = Bar { start: 1_790_000_000 + i * 300, open: o, high: h, low: l, close: c, volume: 100.0 + ((i * 37) % 90) as f64 };
        for ev in e.on_bar(bar, true) {
            match ev {
                Event::Entry { dir, price, stop, tp1, tp2, tp3, .. } => {
                    assert!(open.is_none(), "entry while a trade is open");
                    let d = dir as f64;
                    assert!(d * (price - stop) > 0.0 && d * (tp1 - price) > 0.0 && d * (tp2 - tp1) > 0.0 && d * (tp3 - tp2) > 0.0);
                    assert_eq!(price, c, "entry at the signal bar close");
                    open = Some((dir, price, stop, tp3));
                    entries += 1;
                }
                Event::Partial { .. } => panic!("no partials with default fractions"),
                Event::Exit { dir, price, reason, gross_r, .. } => {
                    let (od, entry, stop, tp3) = open.take().expect("exit without entry");
                    assert_eq!(dir, od);
                    let r = (entry - stop).abs();
                    assert!((gross_r - dir as f64 * (price - entry) / r).abs() < 1e-9);
                    match reason {
                        "TP3" => assert_eq!(price, tp3),
                        "SL" => assert!(dir as f64 * (price - stop) <= 0.0),
                        "Reversal" => assert_eq!(price, c),
                        _ => {}
                    }
                    exits += 1;
                }
            }
        }
    }
    assert!(entries > 5, "the synthetic market should produce trades, got {entries}");
    assert!(exits + 1 >= entries);
}

#[test]
fn stop_is_checked_before_targets_and_force_close_exits() {
    let mut e = Engine::new(Params::default());
    e.trade = Some(crate::Trade {
        dir: 1,
        entry_bar: 0,
        entry: 100.0,
        initial_stop: 95.0,
        stop: 95.0,
        risk: 5.0,
        tp1: 105.0,
        tp2: 110.0,
        tp3: 115.0,
        hit1: false,
        hit2: false,
        hit3: false,
        ambiguous: false,
        remaining: 1.0,
        gross_r: 0.0,
    });
    // bar_index 0 is the entry bar; feed a dummy first bar, then a bar touching both
    let _ = e.on_bar(Bar { start: 0, open: 100.0, high: 101.0, low: 99.0, close: 100.0, volume: 10.0 }, true);
    let ev = e.on_bar(Bar { start: 300, open: 100.0, high: 116.0, low: 94.0, close: 110.0, volume: 10.0 }, true);
    assert_eq!(ev, vec![Event::Exit { dir: 1, price: 95.0, reason: "SL", fraction: 1.0, gross_r: -1.0, ambiguous: true }]);
    assert!(e.force_close(100.0, "EOD").is_none());
}

fn open_long(e: &mut Engine) {
    e.trade = Some(crate::Trade {
        dir: 1, entry_bar: 0, entry: 100.0, initial_stop: 95.0, stop: 95.0, risk: 5.0,
        tp1: 105.0, tp2: 110.0, tp3: 115.0, hit1: false, hit2: false, hit3: false,
        ambiguous: false, remaining: 1.0, gross_r: 0.0,
    });
}

#[test]
fn thirds_close_at_each_target_and_live_marks_match_the_bar_model() {
    let third = 1.0 / 3.0;
    let p = Params { tp1_close_fraction: third, tp2_close_fraction: third, ..Params::default() };
    // bar model: one bar touching TP1 and TP2 books two thirds, stop steps to TP1
    let mut e = Engine::new(p.clone());
    open_long(&mut e);
    let _ = e.on_bar(Bar { start: 0, open: 100.0, high: 101.0, low: 99.0, close: 100.0, volume: 10.0 }, true);
    let ev = e.on_bar(Bar { start: 300, open: 100.0, high: 111.0, low: 99.0, close: 109.0, volume: 10.0 }, true);
    assert_eq!(ev.len(), 2);
    assert!(matches!(ev[0], Event::Partial { level: 1, price: 105.0, .. }));
    assert!(matches!(ev[1], Event::Partial { level: 2, price: 110.0, .. }));
    assert_eq!(e.trade.as_ref().unwrap().stop, 105.0, "step stop to TP1 for the next bar");
    // stopped at the stepped stop: last third, whole trade R = (1 + 2 + 1) / 3
    let ev = e.on_bar(Bar { start: 600, open: 108.0, high: 108.0, low: 104.0, close: 104.0, volume: 10.0 }, true);
    match &ev[..] {
        [Event::Exit { reason: "Step stop", price, fraction, gross_r, .. }] => {
            assert_eq!(*price, 105.0);
            assert!((fraction - third).abs() < 1e-9);
            assert!((gross_r - 4.0 / 3.0).abs() < 1e-9);
        }
        other => panic!("{other:?}"),
    }
    // live: ticks mark TP1 then TP3 inside a bar; the bar close does not book them again
    let mut l = Engine::new(p);
    open_long(&mut l);
    let _ = l.on_bar(Bar { start: 0, open: 100.0, high: 101.0, low: 99.0, close: 100.0, volume: 10.0 }, true);
    assert!(matches!(&l.mark_target(1)[..], [Event::Partial { level: 1, .. }]));
    assert!(l.mark_target(1).is_empty(), "a target is booked once");
    let ev = l.mark_target(3);
    assert!(matches!(&ev[..], [Event::Partial { level: 2, .. }, Event::Exit { reason: "TP3", .. }]));
    assert!(l.trade.is_none());
}
