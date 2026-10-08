//! Research-only port of Self-Aware Trend System v1.13.1 defaults on 3-minute bars.
//! Source supplied by the user. No execution path.
use anyhow::{Result, ensure};
use kite_adapter::http::historical::{Candle, Interval};
use serde::Serialize;
use std::collections::VecDeque;

#[derive(Clone, Copy)]
struct Pos {
    side: i8,
    entry: f64,
    sl: f64,
    tp1: f64,
    tp2: f64,
    tp3: f64,
    hit1: bool,
    hit2: bool,
    remaining: f64,
    taken_points: f64,
    entry_idx: usize,
    entry_time: usize,
}

#[derive(Debug, Clone, Serialize)]
struct Trade {
    entry_time: String,
    exit_time: String,
    side: &'static str,
    entry: f64,
    exit: f64,
    sl: f64,
    tp1: f64,
    tp2: f64,
    tp3: f64,
    reason: &'static str,
    points: f64,
}

#[derive(Debug, Serialize)]
struct Summary {
    strategy: &'static str,
    period: &'static str,
    instrument: String,
    timeframe_minutes: u64,
    trades: usize,
    wins: usize,
    losses: usize,
    breakeven: usize,
    gross_points: f64,
    gross_inr: f64,
    estimated_charges_inr: f64,
    net_inr: f64,
    profit_factor: f64,
}

#[derive(Default)]
struct Wilder {
    len: usize,
    count: usize,
    seed: f64,
    value: Option<f64>,
}
impl Wilder {
    fn new(len: usize) -> Self {
        Self {
            len,
            ..Self::default()
        }
    }
    fn update(&mut self, x: f64) -> Option<f64> {
        if let Some(v) = self.value {
            let n = self.len as f64;
            let next = (v * (n - 1.0) + x) / n;
            self.value = Some(next);
        } else {
            self.seed += x;
            self.count += 1;
            if self.count == self.len {
                self.value = Some(self.seed / self.len as f64);
            }
        }
        self.value
    }
}

fn aggregate(candles: &[Candle], minutes: u64, session_open: u32) -> Result<Vec<Candle>> {
    use chrono::Timelike;
    let mut out = Vec::new();
    let mut bucket: Vec<&Candle> = Vec::new();
    let mut key = None;
    let flush = |bucket: &mut Vec<&Candle>, out: &mut Vec<Candle>| {
        if bucket.is_empty() {
            return;
        }
        let f = bucket[0];
        let l = *bucket.last().expect("bucket");
        out.push(Candle {
            timestamp: f.timestamp.clone(),
            open: f.open,
            high: bucket
                .iter()
                .map(|x| x.high)
                .fold(f64::NEG_INFINITY, f64::max),
            low: bucket.iter().map(|x| x.low).fold(f64::INFINITY, f64::min),
            close: l.close,
            volume: bucket.iter().map(|x| x.volume).sum(),
            oi: l.oi,
        });
        bucket.clear();
    };
    for c in candles {
        let t = c.time()?;
        let m = t.hour() * 60 + t.minute();
        if m < session_open {
            continue;
        }
        let k = (t.date_naive(), (m - session_open) / minutes as u32);
        if key.is_some_and(|x| x != k) {
            flush(&mut bucket, &mut out)
        }
        key = Some(k);
        bucket.push(c);
    }
    flush(&mut bucket, &mut out);
    Ok(out)
}

fn crude_leg_charges(price: f64, is_buy: bool, fraction: f64) -> f64 {
    let notional = price * 100.0 * fraction;
    let brokerage = (notional * 0.0003).min(20.0 * fraction.max(1.0 / 3.0));
    let transaction = notional * 0.000021;
    let sebi = notional * 0.000001;
    let ctt = if is_buy { 0.0 } else { notional * 0.0001 };
    let stamp = if is_buy { notional * 0.00002 } else { 0.0 };
    let gst = (brokerage + transaction + sebi) * 0.18;
    brokerage + transaction + sebi + ctt + stamp + gst
}

fn sma(v: &VecDeque<f64>) -> Option<f64> {
    (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
}
fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    v.max(lo).min(hi)
}
fn map(v: f64, in_lo: f64, in_hi: f64, out_lo: f64, out_hi: f64) -> f64 {
    let t = clamp((v - in_lo) / (in_hi - in_lo), 0.0, 1.0);
    out_lo + t * (out_hi - out_lo)
}
fn er(closes: &VecDeque<f64>, len: usize) -> f64 {
    if closes.len() < len + 1 {
        return 0.0;
    }
    let n = closes.len();
    let change = (closes[n - 1] - closes[n - 1 - len]).abs();
    let mut vol = 0.0;
    for i in n - len..n {
        vol += (closes[i] - closes[i - 1]).abs();
    }
    if vol > 0.0 { change / vol } else { 0.0 }
}
fn pivot_low(b: &[Candle], i: usize, n: usize) -> Option<f64> {
    if i < n * 2 {
        return None;
    }
    let c = i - n;
    let x = b[c].low;
    (b[c - n..c].iter().all(|z| x < z.low) && b[c + 1..=c + n].iter().all(|z| x <= z.low))
        .then_some(x)
}
fn pivot_high(b: &[Candle], i: usize, n: usize) -> Option<f64> {
    if i < n * 2 {
        return None;
    }
    let c = i - n;
    let x = b[c].high;
    (b[c - n..c].iter().all(|z| x > z.high) && b[c + 1..=c + n].iter().all(|z| x >= z.high))
        .then_some(x)
}

fn simulate(bars: &[Candle]) -> Result<Vec<Trade>> {
    ensure!(bars.len() > 200, "insufficient bars");
    // Auto on 3m => Scalping.
    const ATR_LEN: usize = 10;
    const BASE_MULT: f64 = 1.5;
    const ER_LEN: usize = 14;
    const ATR_BASELINE: usize = 100;
    const TQI_STRUCT: usize = 20;
    const TQI_MOM: usize = 10;
    const WARMUP: usize = 108;
    const SL_MULT: f64 = 1.0;
    const SL_MAX: f64 = 4.0;
    const TIMEOUT: usize = 100;
    const PIVOT: usize = 3;
    const PIVOT_MAX_AGE: usize = 100;

    let mut atr = Wilder::new(ATR_LEN);
    let mut raw_atrs = VecDeque::new();
    let mut closes = VecDeque::new();
    let mut prev_close: Option<f64> = None;
    let mut active_mult_sm: Option<f64> = None;
    let mut passive_mult_sm: Option<f64> = None;
    let mut lower: Option<f64> = None;
    let mut upper: Option<f64> = None;
    let mut trend: i8 = 1;
    let mut trend_start = 0usize;
    let mut tqis = VecDeque::new();
    let mut last_pivot_low: Option<(f64, usize)> = None;
    let mut last_pivot_high: Option<(f64, usize)> = None;
    let mut pos: Option<Pos> = None;
    let mut trades = Vec::new();

    for i in 0..bars.len() {
        let c = &bars[i];
        let tr = if let Some(pc) = prev_close {
            (c.high - c.low)
                .max((c.high - pc).abs())
                .max((c.low - pc).abs())
        } else {
            c.high - c.low
        };
        prev_close = Some(c.close);
        let raw_atr = atr.update(tr);
        closes.push_back(c.close);
        if closes.len() > 250 {
            closes.pop_front();
        }
        if let Some(a) = raw_atr {
            raw_atrs.push_back(a);
            if raw_atrs.len() > ATR_BASELINE {
                raw_atrs.pop_front();
            }
        }
        if let Some(v) = pivot_low(bars, i, PIVOT) {
            last_pivot_low = Some((v, i - PIVOT));
        }
        if let Some(v) = pivot_high(bars, i, PIVOT) {
            last_pivot_high = Some((v, i - PIVOT));
        }

        let (Some(raw_atr), Some(atr_base)) = (
            raw_atr,
            if raw_atrs.len() == ATR_BASELINE {
                sma(&raw_atrs)
            } else {
                None
            },
        ) else {
            continue;
        };
        let er_v = er(&closes, ER_LEN);
        let vol_ratio = if atr_base > 0.0 {
            raw_atr / atr_base
        } else {
            1.0
        };
        let atr_value = raw_atr * (0.5 + 0.5 * er_v);

        let struct_start = i.saturating_sub(TQI_STRUCT - 1);
        let sh = bars[struct_start..=i]
            .iter()
            .map(|b| b.high)
            .fold(f64::NEG_INFINITY, f64::max);
        let sl = bars[struct_start..=i]
            .iter()
            .map(|b| b.low)
            .fold(f64::INFINITY, f64::min);
        let price_pos = if sh > sl {
            (c.close - sl) / (sh - sl)
        } else {
            0.5
        };
        let tqi_struct = clamp((price_pos - 0.5).abs() * 2.0, 0.0, 1.0);
        let tqi_mom = if i >= TQI_MOM {
            let change = c.close - bars[i - TQI_MOM].close;
            let mut up = 0;
            let mut down = 0;
            for j in i - TQI_MOM + 1..=i {
                if bars[j].close > bars[j - 1].close {
                    up += 1
                } else if bars[j].close < bars[j - 1].close {
                    down += 1
                }
            }
            if change > 0.0 {
                up as f64 / TQI_MOM as f64
            } else if change < 0.0 {
                down as f64 / TQI_MOM as f64
            } else {
                0.0
            }
        } else {
            0.0
        };
        let tqi_vol = map(vol_ratio, 0.6, 1.8, 0.0, 1.0);
        let tqi = clamp(
            er_v * 0.35 + tqi_vol * 0.20 + tqi_struct * 0.25 + tqi_mom * 0.20,
            0.0,
            1.0,
        );
        tqis.push_back(tqi);
        if tqis.len() > 50 {
            tqis.pop_front();
        }

        let legacy = 1.0 + 0.5 * (0.5 - er_v);
        let quality_dev = (1.0 - tqi).powf(1.5);
        let tqi_mult = 1.0 - 0.4 + 0.4 * (0.6 + 0.8 * quality_dev);
        let sym = BASE_MULT * legacy * tqi_mult;
        let active_raw = sym * (1.0 - 0.5 * tqi * 0.3);
        let passive_raw = sym * (1.0 + 0.5 * tqi * 0.4);
        active_mult_sm = Some(active_mult_sm.map_or(active_raw, |v| v * 0.85 + active_raw * 0.15));
        passive_mult_sm =
            Some(passive_mult_sm.map_or(passive_raw, |v| v * 0.85 + passive_raw * 0.15));
        let am = active_mult_sm.unwrap();
        let pm = passive_mult_sm.unwrap();

        let prev_trend = trend;
        let lower_mult = if prev_trend == 1 { am } else { pm };
        let upper_mult = if prev_trend == 1 { pm } else { am };
        let lower_raw = c.close - lower_mult * atr_value;
        let upper_raw = c.close + upper_mult * atr_value;
        let prev_lower = lower;
        let prev_upper = upper;
        lower = Some(if let Some(pl) = prev_lower {
            if i > 0 && bars[i - 1].close > pl {
                lower_raw.max(pl)
            } else {
                lower_raw
            }
        } else {
            lower_raw
        });
        upper = Some(if let Some(pu) = prev_upper {
            if i > 0 && bars[i - 1].close < pu {
                upper_raw.min(pu)
            } else {
                upper_raw
            }
        } else {
            upper_raw
        });

        let price_up = prev_trend == -1 && prev_upper.is_some_and(|u| c.close > u);
        let price_down = prev_trend == 1 && prev_lower.is_some_and(|l| c.close < l);
        let age = i.saturating_sub(trend_start);
        let win = 5usize;
        let tqi_hi = tqis
            .iter()
            .rev()
            .take(win)
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        let char_base = age >= 5 && tqi_hi > 0.55 && tqi < 0.25 && i >= win;
        let char_down = char_base && prev_trend == 1 && c.close < bars[i - win].close;
        let char_up = char_base && prev_trend == -1 && c.close > bars[i - win].close;
        if price_up || char_up {
            trend = 1
        } else if price_down || char_down {
            trend = -1
        }
        let flip = trend != prev_trend;
        if flip {
            trend_start = i;
        }

        // Settle old trade first, SL first, then partial TPs, then flip/timeout.
        if let Some(mut p) = pos.take() {
            if i > p.entry_idx {
                let stop_hit = if p.side == 1 {
                    c.low <= p.sl
                } else {
                    c.high >= p.sl
                };
                let opposite = p.side != trend;
                let timed = i - p.entry_idx >= TIMEOUT;
                let mut reason = None;
                if stop_hit {
                    let fill = if p.side == 1 {
                        c.open.min(p.sl)
                    } else {
                        c.open.max(p.sl)
                    };
                    p.taken_points += p.remaining * (fill - p.entry) * p.side as f64;
                    p.remaining = 0.0;
                    reason = Some(("SL", fill));
                } else {
                    if !p.hit1
                        && (if p.side == 1 {
                            c.high >= p.tp1
                        } else {
                            c.low <= p.tp1
                        })
                    {
                        p.hit1 = true;
                        p.taken_points += (1.0 / 3.0) * (p.tp1 - p.entry) * p.side as f64;
                        p.remaining -= 1.0 / 3.0;
                    }
                    if !p.hit2
                        && (if p.side == 1 {
                            c.high >= p.tp2
                        } else {
                            c.low <= p.tp2
                        })
                    {
                        p.hit2 = true;
                        p.taken_points += (1.0 / 3.0) * (p.tp2 - p.entry) * p.side as f64;
                        p.remaining -= 1.0 / 3.0;
                    }
                    let hit3 = if p.side == 1 {
                        c.high >= p.tp3
                    } else {
                        c.low <= p.tp3
                    };
                    if hit3 {
                        p.taken_points += p.remaining * (p.tp3 - p.entry) * p.side as f64;
                        p.remaining = 0.0;
                        reason = Some(("TP3", p.tp3));
                    } else if opposite || timed {
                        let fill = c.close;
                        p.taken_points += p.remaining * (fill - p.entry) * p.side as f64;
                        p.remaining = 0.0;
                        reason = Some((if opposite { "FLIP" } else { "TIMEOUT" }, fill));
                    }
                }
                if let Some((why, fill)) = reason {
                    trades.push(Trade {
                        entry_time: bars[p.entry_time].timestamp.clone(),
                        exit_time: c.timestamp.clone(),
                        side: if p.side == 1 { "LONG" } else { "SHORT" },
                        entry: p.entry,
                        exit: fill,
                        sl: p.sl,
                        tp1: p.tp1,
                        tp2: p.tp2,
                        tp3: p.tp3,
                        reason: why,
                        points: p.taken_points,
                    });
                } else {
                    pos = Some(p)
                }
            } else {
                pos = Some(p)
            }
        }

        let warmed = i >= WARMUP;
        if flip && warmed && pos.is_none() {
            let side = trend;
            let pivot = if side == 1 {
                last_pivot_low
                    .filter(|(_, j)| {
                        i.saturating_sub(*j) <= PIVOT_MAX_AGE && bars[*j].low < c.close
                    })
                    .map(|x| x.0)
            } else {
                last_pivot_high
                    .filter(|(_, j)| {
                        i.saturating_sub(*j) <= PIVOT_MAX_AGE && bars[*j].high > c.close
                    })
                    .map(|x| x.0)
            };
            let base = pivot.unwrap_or(if side == 1 { c.low } else { c.high });
            let buffer = SL_MULT * atr_value;
            let cap = SL_MAX.max(SL_MULT) * atr_value;
            let raw_stop = if side == 1 {
                (base - buffer).min(c.close - buffer).max(c.close - cap)
            } else {
                (base + buffer).max(c.close + buffer).min(c.close + cap)
            };
            let risk = (c.close - raw_stop) * side as f64;
            if risk >= 2.0 {
                // CRUDE tick size 1, minRiskTicks=2
                pos = Some(Pos {
                    side,
                    entry: c.close,
                    sl: raw_stop,
                    tp1: c.close + side as f64 * risk,
                    tp2: c.close + side as f64 * risk * 2.0,
                    tp3: c.close + side as f64 * risk * 3.0,
                    hit1: false,
                    hit2: false,
                    remaining: 1.0,
                    taken_points: 0.0,
                    entry_idx: i,
                    entry_time: i,
                });
            }
        }
    }
    Ok(trades)
}

pub fn run(name: &str, token: u32) -> Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    let split = chrono::NaiveDate::from_ymd_opt(2026, 9, 8).unwrap();
    let end = chrono::NaiveDate::from_ymd_opt(2026, 10, 8).unwrap();
    let mut raw = rt.block_on(kite_adapter::http::historical::fetch_window_for(
        token,
        split,
        29,
        Interval::OneMinute,
    ))?;
    raw.extend(
        rt.block_on(kite_adapter::http::historical::fetch_window_for(
            token,
            end,
            30,
            Interval::OneMinute,
        ))?,
    );
    raw.sort_by_key(|c| c.timestamp.clone());
    raw.dedup_by(|a, b| a.timestamp == b.timestamp);
    let bars = aggregate(&raw, 3, 540)?;
    let trades = simulate(&bars)?;
    let scored: Vec<_> = trades
        .into_iter()
        .filter(|t| {
            t.exit_time.as_str() >= "2026-08-17"
                && t.exit_time.as_str() <= "2026-10-07T23:59:59+05:30"
        })
        .collect();
    let wins = scored.iter().filter(|t| t.points > 1e-9).count();
    let losses = scored.iter().filter(|t| t.points < -1e-9).count();
    let be = scored.len() - wins - losses;
    let gross: f64 = scored.iter().map(|t| t.points).sum();
    let gp: f64 = scored
        .iter()
        .filter(|t| t.points > 0.0)
        .map(|t| t.points)
        .sum();
    let gl: f64 = -scored
        .iter()
        .filter(|t| t.points < 0.0)
        .map(|t| t.points)
        .sum::<f64>();
    // Approximate one entry + weighted partial exits. This is a model cost estimate;
    // actual 1-contract MCX execution cannot fractional-exit thirds.
    let mut charges = 0.0;
    for t in &scored {
        let buy = t.side == "LONG";
        charges += crude_leg_charges(t.entry, buy, 1.0);
        // Approximate three equal exit fills at TP1/TP2/final exit.
        charges += crude_leg_charges(t.tp1, !buy, 1.0 / 3.0);
        charges += crude_leg_charges(t.tp2, !buy, 1.0 / 3.0);
        charges += crude_leg_charges(t.exit, !buy, 1.0 / 3.0);
    }
    let summary = Summary {
        strategy: "Self-Aware Trend System v1.13.1 defaults (3m Auto=Scalping)",
        period: "2026-08-17 through 2026-10-07",
        instrument: name.into(),
        timeframe_minutes: 3,
        trades: scored.len(),
        wins,
        losses,
        breakeven: be,
        gross_points: gross,
        gross_inr: gross * 100.0,
        estimated_charges_inr: charges,
        net_inr: gross * 100.0 - charges,
        profit_factor: if gl > 0.0 { gp / gl } else { 0.0 },
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({"summary":summary,"trades":scored}))?
    );
    Ok(())
}
