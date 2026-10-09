use crate::indicators::{History, Rma, Rsi, pivot_high, pivot_low};
use crate::params::{Params, Preset};
use crate::trade::{self, BarCtx, EntryInputs, EventKind, ExitRules, Side, Ticks, TradeSnapshot};
use crate::*;

const MIN: i64 = 60_000_000_000;

fn hist(vals_oldest_first: &[f64]) -> History<f64> {
    let mut h = History::new(64);
    for v in vals_oldest_first {
        h.push(*v);
    }
    h
}

#[test]
fn rma_seeds_with_sma_then_wilder_step() {
    let mut r = Rma::new(3);
    assert_eq!(r.update(Some(3.0)), None);
    assert_eq!(r.update(None), None, "na input leaves state untouched");
    assert_eq!(r.update(Some(6.0)), None);
    assert_eq!(r.update(Some(9.0)), Some(6.0));
    assert_eq!(r.update(Some(12.0)), Some((6.0 * 2.0 + 12.0) / 3.0));
}

#[test]
fn rsi_is_na_on_first_bar_and_saturates() {
    let mut r = Rsi::new(2);
    assert_eq!(r.update(10.0), None);
    assert_eq!(r.update(11.0), None);
    assert_eq!(r.update(12.0), Some(100.0), "no down moves -> 100");
    let mut f = Rsi::new(2);
    for x in [10.0, 9.0, 8.0] {
        f.update(x);
    }
    assert_eq!(f.update(7.0), Some(0.0), "no up moves -> 0");
}

#[test]
fn pivots_keep_the_newest_of_equal_extremes() {
    // oldest -> newest; centre is index 3 (n = 2 bars back from the newest is index 4 ... use window of 5)
    // window [1, 5, 5, 3, 2]: the newer 5 (2 bars back) is the pivot, the older one is not.
    assert_eq!(pivot_high(&hist(&[1.0, 5.0, 5.0, 3.0, 2.0]), 2), Some(5.0));
    // window [1, 3, 5, 5, 2]: a newer bar ties the centre -> not a pivot.
    assert_eq!(pivot_high(&hist(&[1.0, 3.0, 5.0, 5.0, 2.0]), 2), None);
    assert_eq!(pivot_low(&hist(&[9.0, 2.0, 2.0, 4.0, 6.0]), 2), Some(2.0));
    assert_eq!(pivot_low(&hist(&[9.0, 4.0, 2.0, 2.0, 6.0]), 2), None);
    assert_eq!(pivot_high(&hist(&[1.0, 5.0, 3.0]), 2), None, "needs 2n+1 bars");
}

#[test]
fn auto_preset_and_validation_follow_the_script() {
    let p = Params::default();
    let r = p.resolve(5.0);
    assert_eq!(r.preset, Preset::Scalping);
    assert_eq!((r.atr_len, r.er_len, r.rsi_len), (10, 14, 9));
    assert_eq!((r.base_mult, r.sl_mult), (1.5, 1.0));
    assert_eq!(r.warmup_bars, 108, "max(50, 10+100-2, 9+20-1, 14, 10, 20)");
    assert_eq!(p.resolve(60.0).preset, Preset::Default);
    assert_eq!(p.resolve(1440.0).preset, Preset::Swing);
    let swapped = Params { tp1_r: 3.0, tp3_r: 1.0, ..Params::default() }.resolve(5.0);
    assert_eq!((swapped.fixed_tp1_r, swapped.fixed_tp2_r, swapped.fixed_tp3_r), (1.0, 2.0, 3.0));
    assert!(Params { char_flip_low_tqi: 0.5, char_flip_high_tqi: 0.4, ..Params::default() }.validate().is_err());
    assert!(Params { atr_length: 2, ..Params::default() }.validate().is_err());
    assert!(Engine::new(Params::default(), SymbolSpec { tick_size: 0.0, bar_minutes: 5.0 }).is_err());
}

#[test]
fn tick_rounding_matches_pine_helpers() {
    let t = Ticks(0.25);
    assert_eq!(t.round(100.1), 100.0);
    assert_eq!(t.round(100.125), 100.25, "ties round up");
    assert_eq!(t.down(100.24), 100.0);
    assert_eq!(t.up(100.01), 100.25);
    assert_eq!(Ticks(1.0).down(94.9999999999), 95.0, "1e-9 tolerance absorbs float dust");
}

fn long_trade() -> TradeSnapshot {
    TradeSnapshot {
        side: Side::Long,
        entry_bar: 0,
        entry_time_ns: 0,
        signal_price: 100.0,
        entry: 100.0,
        sl: 90.0,
        risk: 10.0,
        tp1: 110.0,
        tp2: 120.0,
        tp3: 130.0,
        r1: 1.0,
        r2: 2.0,
        r3: 3.0,
        hit1: false,
        hit2: false,
        hit3: false,
        taken_r: 0.0,
        cost_r: 0.0,
        remaining: 1.0,
        entry_tqi: 0.5,
        entry_score: 50.0,
        reason: String::new(),
        limitations: String::new(),
        epoch: 0,
    }
}

fn ctx(i: i64, o: f64, h: f64, l: f64, c: f64, trend: i8) -> BarCtx {
    BarCtx { bar_index: i, time_ns: i * MIN, open: o, high: h, low: l, close: c, trend }
}

const RULES: ExitRules = ExitRules { timeout_bars: 100, slip: 0.0, fee_pct: 0.0 };

#[test]
fn thirds_then_stop_with_gap_fill() {
    let mut t = long_trade();
    let (ev, closed) = trade::settle(&mut t, &ctx(1, 101.0, 125.0, 101.0, 121.0, 1), &RULES);
    assert_eq!(ev.iter().map(|e| e.kind).collect::<Vec<_>>(), [EventKind::Tp1Hit, EventKind::Tp2Hit]);
    assert!(closed.is_none());
    assert!((t.taken_r - 1.0).abs() < 1e-12 && (t.remaining - 1.0 / 3.0).abs() < 1e-12);
    // gaps below the stop: filled at the open, not the stop level
    let (ev, closed) = trade::settle(&mut t, &ctx(2, 85.0, 86.0, 80.0, 82.0, 1), &RULES);
    assert_eq!(ev[0].kind, EventKind::SlHit);
    assert_eq!(ev[0].fill, 85.0);
    assert!(ev[0].closes_trade);
    assert!((closed.unwrap() - (1.0 - 1.5 / 3.0)).abs() < 1e-12);
}

#[test]
fn stop_wins_an_ambiguous_bar_and_entry_bar_is_skipped() {
    let mut t = long_trade();
    assert!(trade::settle(&mut t, &ctx(0, 100.0, 140.0, 80.0, 100.0, 1), &RULES).0.is_empty());
    let (ev, closed) = trade::settle(&mut t, &ctx(1, 100.0, 140.0, 80.0, 100.0, 1), &RULES);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].kind, EventKind::SlHit);
    assert_eq!(closed, Some(-1.0));
}

#[test]
fn flip_and_timeout_exit_at_close_with_slippage_and_fees() {
    let mut t = long_trade();
    let rules = ExitRules { timeout_bars: 100, slip: 1.0, fee_pct: 0.1 };
    let (ev, closed) = trade::settle(&mut t, &ctx(3, 104.0, 106.0, 103.0, 105.0, -1), &rules);
    assert_eq!(ev[0].kind, EventKind::FlipExit);
    assert_eq!(ev[0].fill, 104.0);
    let expected = 0.4 - 104.0 * 0.1 / 100.0 / 10.0;
    assert!((closed.unwrap() - expected).abs() < 1e-12);

    let mut t = long_trade();
    let (ev, _) = trade::settle(&mut t, &ctx(100, 104.0, 106.0, 103.0, 105.0, 1), &RULES);
    assert_eq!(ev[0].kind, EventKind::TimeoutExit);
}

#[test]
fn plan_uses_pivot_buffer_and_cap_and_rejects_tiny_risk() {
    let base = EntryInputs {
        side: Side::Long,
        close: 100.0,
        low: 99.0,
        high: 101.0,
        atr_value: 2.0,
        sl_mult: 1.0,
        max_sl_dist: 4.0,
        last_pivot_low: Some(97.0),
        last_pivot_high: None,
        valid_low_pivot: true,
        valid_high_pivot: false,
        live_r: [1.0, 2.0, 3.0],
        dyn_floors: None,
        dyn_ceiling: 8.0,
        min_risk_ticks: 2,
        slip: 0.0,
        fee_pct: 0.0,
        ticks: Ticks(1.0),
    };
    let pl = trade::plan(&base).unwrap();
    assert_eq!((pl.sl, pl.risk), (95.0, 5.0), "pivot 97 minus 1 ATR buffer");
    assert_eq!(pl.tp, [105.0, 110.0, 115.0]);
    let far = trade::plan(&EntryInputs { last_pivot_low: Some(50.0), ..base_clone(&base) }).unwrap();
    assert_eq!(far.sl, 92.0, "capped at 4 ATR");
    assert!(trade::plan(&EntryInputs { min_risk_ticks: 100, ..base_clone(&base) }).is_err());
}

fn base_clone(b: &EntryInputs) -> EntryInputs {
    EntryInputs { ..*b }
}

/// A long rally, a crash and a recovery: enough structure for both flips.
fn tape() -> Vec<BarInput> {
    let start = 1_790_000_000_000_000_000_i64;
    let mut price = 5000.0_f64;
    (0..600)
        .map(|i| {
            let drift = if i < 250 { 3.0 } else if i < 400 { -6.0 } else { 4.0 };
            let wiggle = ((i as f64) * 0.7).sin() * 4.0;
            let open = price;
            price += drift + wiggle;
            let close = price;
            let t = start + i as i64 * 5 * MIN;
            BarInput {
                open_time_ns: t,
                close_time_ns: t + 5 * MIN,
                open,
                high: open.max(close) + 2.0,
                low: open.min(close) - 2.0,
                close,
                volume: Some(1000.0 + (i % 7) as f64 * 50.0),
            }
        })
        .collect()
}

#[test]
fn engine_trades_flips_after_warmup_with_exits_before_entries() {
    let mut e = Engine::new(Params::default(), SymbolSpec { tick_size: 1.0, bar_minutes: 5.0 }).unwrap();
    let mut all = Vec::new();
    for (i, b) in tape().iter().enumerate() {
        let evs = e.on_bar(b);
        if let Some(first_entry) = evs.iter().position(|x| x.kind.is_entry()) {
            assert!(evs[first_entry..].iter().all(|x| x.kind.is_entry()), "exits precede the entry on a bar");
        }
        all.extend(evs.into_iter().map(|x| (i, x)));
    }
    let entries: Vec<_> = all.iter().filter(|(_, x)| x.kind.is_entry()).collect();
    assert!(entries.len() >= 2, "expected long and short entries, got {}", entries.len());
    assert!(all.iter().all(|(i, _)| *i >= 108), "no events before warm-up");
    assert!(entries.iter().any(|(_, x)| x.kind == EventKind::Buy));
    assert!(entries.iter().any(|(_, x)| x.kind == EventKind::Sell));
    // never two open trades: every entry after the first is preceded by a closing event
    let mut open = false;
    for (_, x) in &all {
        if x.kind.is_entry() {
            assert!(!open, "overlapping model trades");
            open = true;
            let t = &x.trade;
            let d = t.side.sign();
            assert!(d * (t.entry - t.sl) > 0.0 && d * (t.tp1 - t.entry) > 0.0);
            assert!(d * (t.tp2 - t.tp1) >= 0.0 && d * (t.tp3 - t.tp2) >= 0.0);
        } else if x.closes_trade {
            open = false;
        }
    }
    assert!(e.status().warmed_up);
}

#[test]
fn engine_state_round_trips_through_json() {
    let bars = tape();
    let mut a = Engine::new(Params::default(), SymbolSpec { tick_size: 1.0, bar_minutes: 5.0 }).unwrap();
    for b in &bars[..300] {
        a.on_bar(b);
    }
    let mut b: Engine = serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap();
    for bar in &bars[300..] {
        assert_eq!(a.on_bar(bar), b.on_bar(bar));
    }
}

#[test]
fn params_json_uses_pine_option_names() {
    let p: Params = serde_json::from_str(
        r#"{"preset":"Crypto 24/7","tp_mode":"Dynamic","tqi_volatility_factor":"Volume activity","source":"hlc3"}"#,
    )
    .unwrap();
    assert_eq!(p.preset, Preset::Crypto247);
    assert_eq!(p.tp_mode, TpMode::Dynamic);
    assert_eq!(p.tqi_volatility_factor, TqiVolMode::VolumeActivity);
    assert_eq!(p.source, Source::Hlc3);
    assert!(serde_json::from_str::<Params>(r#"{"atr_lenght": 10}"#).is_err(), "typos are rejected");
}
