//! Precision Sniper v2.1.0 model, bar for bar on confirmed bars:
//! indicators → crossover candidate → gate → (old trade: stop first, targets, reversal,
//! step stop for the NEXT bar) → new entry at the bar close.
use crate::{
    params::{Params, Resolved, StructurePolicy, VolMode},
    ta::{Atr, Dmi, MacdHist, Rsi, Sma, Smoothed, Vwap},
};
use serde::Serialize;
use std::collections::VecDeque;

const ADX_TREND: f64 = 20.0;
const ATR_MEAN_LEN: usize = 42;
const VOLUME_MEAN_LEN: usize = 20;
const VOLUME_SPIKE: f64 = 1.2;
const STRUCTURE_BUFFER: f64 = 0.2;
const STRUCTURE_CAP: f64 = 1.5;
const RSI_UPPER: f64 = 75.0;
const RSI_LOWER: f64 = 25.0;
const IST_OFFSET: i64 = 19_800;

/// One confirmed bar; `start` is epoch seconds (UTC) of its open.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bar {
    pub start: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum Event {
    Entry { dir: i32, price: f64, stop: f64, tp1: f64, tp2: f64, tp3: f64, score: f64, score_max: f64, grade: &'static str, origin: &'static str },
    Exit { dir: i32, price: f64, reason: &'static str, gross_r: f64, ambiguous: bool },
}

#[derive(Clone, Debug)]
pub struct Trade {
    pub dir: i32,
    pub entry_bar: u64,
    pub entry: f64,
    pub initial_stop: f64,
    pub stop: f64,
    pub risk: f64,
    pub tp1: f64,
    pub tp2: f64,
    pub tp3: f64,
    pub hit1: bool,
    pub hit2: bool,
    pub hit3: bool,
    pub ambiguous: bool,
}

struct Plan {
    entry: f64,
    stop: f64,
    risk: f64,
    tp: [f64; 3],
    capped: bool,
    valid: bool,
    origin: &'static str,
}

/// Why the last candidate was not taken (diagnostics; script's reject codes).
pub fn reject_reason(code: u8) -> &'static str {
    match code {
        1 => "Data not ready / warmup",
        2 => "Outside entry window",
        4 => "High volatility blocks entry",
        5 => "Price momentum lost",
        6 => "RSI extreme",
        8 => "Insufficient score",
        9 => "Invalid tick/risk/target geometry",
        10 => "Structure exceeds cap",
        11 => "Too far from fast EMA",
        12 => "Same direction already open",
        _ => "Accepted",
    }
}

pub struct Engine {
    p: Params,
    r: Resolved,
    bar_index: u64,
    ema_fast: Smoothed,
    ema_slow: Smoothed,
    ema_trend: Smoothed,
    atr: Atr,
    atr_mean: Sma,
    rsi: Rsi,
    macd: MacdHist,
    dmi: Dmi,
    vol_mean: Sma,
    vwap: Vwap,
    has_volume: bool,
    prev_fast_slow: Option<(f64, f64)>,
    prev_hist: Option<f64>,
    window: VecDeque<(f64, f64)>,
    pending_dir: i32,
    pending_bar: u64,
    pub trade: Option<Trade>,
    /// Reject-code counts for terminal candidates.
    pub rejects: [u32; 13],
    pub candidates: u32,
}

fn round_tick(x: f64, t: f64) -> f64 {
    (x / t).round() * t
}

impl Engine {
    pub fn new(p: Params) -> Self {
        let r = p.resolve();
        Self {
            bar_index: 0,
            ema_fast: Smoothed::ema(r.fast),
            ema_slow: Smoothed::ema(r.slow),
            ema_trend: Smoothed::ema(r.trend),
            atr: Atr::new(r.atr),
            atr_mean: Sma::new(ATR_MEAN_LEN),
            rsi: Rsi::new(r.rsi),
            macd: MacdHist::new(12, 26, 9),
            dmi: Dmi::new(14, 14),
            vol_mean: Sma::new(VOLUME_MEAN_LEN),
            vwap: Vwap::default(),
            has_volume: false,
            prev_fast_slow: None,
            prev_hist: None,
            window: VecDeque::new(),
            pending_dir: 0,
            pending_bar: 0,
            trade: None,
            rejects: [0; 13],
            candidates: 0,
            p,
            r,
        }
    }
    pub fn resolved(&self) -> Resolved {
        self.r
    }

    /// Closes the open model trade at `price` (e.g. a daily square-off the script does not have).
    pub fn force_close(&mut self, price: f64, reason: &'static str) -> Option<Event> {
        let t = self.trade.take()?;
        Some(Event::Exit { dir: t.dir, price, reason, gross_r: t.dir as f64 * (price - t.entry) / t.risk, ambiguous: t.ambiguous })
    }

    /// Feed one confirmed bar. `entries_allowed` = false acts like the script's date
    /// window ending: no new entries, candidates die.
    pub fn on_bar(&mut self, b: Bar, entries_allowed: bool) -> Vec<Event> {
        let p = self.p.clone();
        let tick = p.tick_size;
        let idx = self.bar_index;
        self.bar_index += 1;

        // ── indicators (every bar) ──
        let fast = self.ema_fast.update(b.close);
        let slow = self.ema_slow.update(b.close);
        let trend = self.ema_trend.update(b.close);
        let atr = self.atr.update(b.high, b.low, b.close);
        let atr_mean = atr.and_then(|a| self.atr_mean.update(a));
        let rsi = self.rsi.update(b.close);
        let hist = self.macd.update(b.close);
        let dmi = self.dmi.update(b.high, b.low, b.close);
        let vol_mean = self.vol_mean.update(b.volume);
        let hlc3 = (b.high + b.low + b.close) / 3.0;
        let vwap = self.vwap.update((b.start + IST_OFFSET).div_euclid(86_400), hlc3, b.volume);
        if b.volume > 0.0 {
            self.has_volume = true;
        }
        self.window.push_back((b.high, b.low));
        while self.window.len() > p.swing_lookback + 1 {
            self.window.pop_front();
        }
        let prev_fs = self.prev_fast_slow;
        if let (Some(f), Some(s)) = (fast, slow) {
            self.prev_fast_slow = Some((f, s));
        }
        let prev_hist = self.prev_hist;
        self.prev_hist = hist;

        let warmup_bars = (self.r.trend * p.warmup_mult).max(self.r.atr + ATR_MEAN_LEN) as u64;
        let warmed = idx >= warmup_bars;
        let volume_ready = !self.has_volume || vol_mean.is_some_and(|m| m > 0.0);
        let vwap_on = p.vwap && self.has_volume;
        let ready = warmed
            && atr.is_some_and(|a| a > 0.0)
            && atr_mean.is_some_and(|a| a > 0.0)
            && rsi.is_some()
            && prev_hist.is_some()
            && dmi.is_some()
            && volume_ready
            && (!vwap_on || vwap.is_some())
            && fast.is_some()
            && slow.is_some()
            && trend.is_some();
        // Before the EMAs exist every comparison is false (Pine na), but an open trade
        // is still managed below.
        let (fast, slow, trend_ema) = (fast.unwrap_or(f64::NAN), slow.unwrap_or(f64::NAN), trend.unwrap_or(f64::NAN));
        let atr_v = atr.unwrap_or(0.0);
        let vol_ratio = atr_mean.filter(|m| *m > 0.0).map(|m| atr_v / m);
        let high_vol = vol_ratio.is_some_and(|v| v > p.vol_threshold);
        let stop_mult = if p.vol_mode == VolMode::Widen && high_vol { p.vol_widen } else { 1.0 };
        let bull_cross = prev_fs.is_some_and(|(pf, ps)| fast > slow && pf <= ps);
        let bear_cross = prev_fs.is_some_and(|(pf, ps)| fast < slow && pf >= ps);
        let bull_mom = b.close > fast && b.close > slow;
        let bear_mom = b.close < fast && b.close < slow;
        let (di_plus, di_minus, adx) = dmi.unwrap_or((0.0, 0.0, 0.0));
        let trending = adx >= ADX_TREND;
        let trend_dir = if fast > slow && b.close > trend_ema { 1 } else if fast < slow && b.close < trend_ema { -1 } else { 0 };
        let _regime = if !trending { 0 } else if trend_dir != 0 { 1 } else { 2 };
        let rsi_v = rsi.unwrap_or(50.0);
        let h = hist.unwrap_or(0.0);
        let ph = prev_hist.unwrap_or(0.0);
        let vol_above = self.has_volume && volume_ready && vol_mean.is_some_and(|m| b.volume > m * VOLUME_SPIKE);
        let score_max = 5.0 + if self.has_volume { 1.0 } else { 0.0 } + if vwap_on { 1.0 } else { 0.0 };
        let vw = vwap.unwrap_or(b.close);
        let b2f = |x: bool| if x { 1.0 } else { 0.0 };
        let bull_score = b2f(b.close > trend_ema) + b2f(rsi_v > 50.0 && rsi_v < RSI_UPPER) + b2f(h > 0.0) + b2f(h > ph)
            + b2f(trending && di_plus > di_minus) + b2f(vol_above) + b2f(vwap_on && b.close > vw);
        let bear_score = b2f(b.close < trend_ema) + b2f(rsi_v < 50.0 && rsi_v > RSI_LOWER) + b2f(h < 0.0) + b2f(h < ph)
            + b2f(trending && di_minus > di_plus) + b2f(vol_above) + b2f(vwap_on && b.close < vw);
        let required = p.required_ratio();
        let swing_low = self.window.iter().map(|w| w.1).fold(f64::MAX, f64::min);
        let swing_high = self.window.iter().map(|w| w.0).fold(f64::MIN, f64::max);

        let sl_mult = self.r.sl_mult;
        let plan = |d: i32| -> Plan {
            let df = d as f64;
            let e = round_tick(b.close, tick);
            let atr_dist = atr_v * sl_mult * stop_mult;
            let atr_stop = e - df * atr_dist;
            let struct_stop = if d == 1 { swing_low - atr_v * STRUCTURE_BUFFER } else { swing_high + atr_v * STRUCTURE_BUFFER };
            let raw = if p.structure {
                if d == 1 { atr_stop.min(struct_stop) } else { atr_stop.max(struct_stop) }
            } else {
                atr_stop
            };
            let capped = p.structure && (e - raw).abs() > atr_dist * STRUCTURE_CAP;
            let bounded = if capped { e - df * atr_dist * STRUCTURE_CAP } else { raw };
            let stop = if d == 1 {
                (bounded / tick + 1e-8).floor() * tick
            } else {
                (bounded / tick - 1e-8).ceil() * tick
            };
            let risk = (e - stop).abs();
            let tp = [p.tp1_r, p.tp2_r, p.tp3_r].map(|m| round_tick(e + df * risk * m, tick));
            let valid = risk >= p.min_risk_ticks as f64 * tick - tick * 1e-6
                && df * (e - stop) > 0.0
                && df * (tp[0] - e) > 0.0
                && df * (tp[1] - tp[0]) > 0.0
                && df * (tp[2] - tp[1]) > 0.0
                && (!p.positive_only || e.min(stop).min(tp[0]).min(tp[1]).min(tp[2]) > 0.0);
            let origin = if !p.structure {
                "ATR"
            } else if capped {
                "ATR capped"
            } else if raw != atr_stop {
                "Structure"
            } else {
                "ATR"
            };
            Plan { entry: e, stop, risk, tp, capped, valid, origin }
        };
        let gate = |d: i32, valid: bool, capped: bool| -> u8 {
            let evidence = if d == 1 { bull_score } else { bear_score };
            let momentum = if d == 1 { bull_mom } else { bear_mom };
            if !ready {
                1
            } else if !entries_allowed {
                2
            } else if p.vol_mode == VolMode::Skip && high_vol {
                4
            } else if !momentum {
                5
            } else if if d == 1 { rsi_v >= RSI_UPPER } else { rsi_v <= RSI_LOWER } {
                6
            } else if evidence + 1e-9 < required * score_max {
                8
            } else if !valid {
                9
            } else if p.structure_policy == StructurePolicy::Skip && capped {
                10
            } else if p.max_extension > 0.0 && (b.close - fast).abs() / atr_v > p.max_extension {
                11
            } else {
                0
            }
        };

        // ── candidate lifecycle ──
        if (bull_cross || bear_cross) && warmed {
            self.pending_dir = if bull_cross { 1 } else { -1 };
            self.pending_bar = idx;
            self.candidates += 1;
        }
        let mut signal = 0;
        if self.pending_dir != 0 {
            let d = self.pending_dir;
            let pl = plan(d);
            let mut code = gate(d, pl.valid, pl.capped);
            if !(if d == 1 { bull_mom } else { bear_mom }) {
                code = 5;
            }
            let last_chance = idx - self.pending_bar >= p.confirm_window as u64;
            let already_open = self.trade.as_ref().is_some_and(|t| t.dir == d);
            if code == 0 && !already_open {
                signal = d;
                self.pending_dir = 0;
            } else if last_chance || code == 5 || already_open || !entries_allowed {
                self.rejects[if already_open { 12 } else { code as usize }] += 1;
                self.pending_dir = 0;
            }
        }

        let mut events = Vec::new();
        // ── open trade: stop first (old stop), then targets, reversal, step stop ──
        if let Some(t) = self.trade.as_mut()
            && idx > t.entry_bar
        {
            let d = t.dir as f64;
            let old_stop = t.stop;
            let stopped = if t.dir == 1 { b.low <= old_stop } else { b.high >= old_stop };
            let touch = |lvl: f64| if t.dir == 1 { b.high >= lvl } else { b.low <= lvl };
            let (t1, t2, t3) = (!t.hit1 && touch(t.tp1), !t.hit2 && touch(t.tp2), !t.hit3 && touch(t.tp3));
            let mut exit: Option<(f64, &'static str)> = None;
            if stopped {
                let fill = if t.dir == 1 { b.open.min(old_stop) } else { b.open.max(old_stop) };
                t.ambiguous |= t1 || t2 || t3;
                exit = Some((fill, if old_stop == t.initial_stop { "SL" } else { "Step stop" }));
            } else {
                t.hit1 |= t1;
                t.hit2 |= t2;
                if t3 {
                    t.hit3 = true;
                    if p.full_tp3 {
                        exit = Some((t.tp3, "TP3"));
                    }
                }
                if exit.is_none() {
                    if signal == -t.dir {
                        exit = Some((b.close, "Reversal"));
                    } else if p.step_stop {
                        let next = if t.hit3 { t.tp2 } else if t.hit2 { t.tp1 } else if t.hit1 { t.entry } else { t.stop };
                        if d * (next - t.stop) > tick * 0.1 {
                            t.stop = next;
                        }
                    }
                }
            }
            if let Some((px, reason)) = exit {
                events.push(Event::Exit { dir: t.dir, price: px, reason, gross_r: d * (px - t.entry) / t.risk, ambiguous: t.ambiguous });
                self.trade = None;
            }
        }
        // ── new entry at the bar close ──
        if signal != 0 && self.trade.is_none() {
            let pl = plan(signal);
            let score = if signal == 1 { bull_score } else { bear_score };
            let ratio = score / score_max;
            let grade = if ratio >= 0.8 { "A+" } else if ratio >= 0.65 { "A" } else if ratio >= 0.5 { "B" } else { "C" };
            self.trade = Some(Trade {
                dir: signal,
                entry_bar: idx,
                entry: pl.entry,
                initial_stop: pl.stop,
                stop: pl.stop,
                risk: pl.risk,
                tp1: pl.tp[0],
                tp2: pl.tp[1],
                tp3: pl.tp[2],
                hit1: false,
                hit2: false,
                hit3: false,
                ambiguous: false,
            });
            events.push(Event::Entry {
                dir: signal,
                price: pl.entry,
                stop: pl.stop,
                tp1: pl.tp[0],
                tp2: pl.tp[1],
                tp3: pl.tp[2],
                score,
                score_max,
                grade,
                origin: pl.origin,
            });
        }
        events
    }
}
