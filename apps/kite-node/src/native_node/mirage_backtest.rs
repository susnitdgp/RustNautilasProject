//! Research-only port of Mirage Liquidity Sweep Pro v1.3.1 defaults.
//! Source supplied by user. No execution path.
use anyhow::{Result, ensure};
use kite_adapter::http::historical::{Candle, Interval};
use serde::Serialize;
use std::collections::VecDeque;

#[derive(Clone)]
struct Swing {
    level: f64,
    bar: usize,
    used: bool,
}

#[derive(Clone)]
struct Pending {
    side: i8,
    start: usize,
    level: f64,
    wick: f64,
    score: f64,
}

#[derive(Clone)]
struct Position {
    side: i8,
    entry: f64,
    sl: f64,
    tp1: f64,
    tp2: f64,
    tp3: f64,
    be: bool,
    hit1: bool,
    hit2: bool,
    entry_idx: usize,
    entry_time: String,
}

#[derive(Debug, Clone, Serialize)]
struct Trade {
    entry_time: String,
    exit_time: String,
    side: &'static str,
    entry: f64,
    exit: f64,
    sl_initial: f64,
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

fn crude_leg_charges(price: f64, is_buy: bool) -> f64 {
    let notional = price * 100.0;
    let brokerage = (notional * 0.0003).min(20.0);
    let transaction = notional * 0.000021;
    let sebi = notional * 0.000001;
    let ctt = if is_buy { 0.0 } else { notional * 0.0001 };
    let stamp = if is_buy { notional * 0.00002 } else { 0.0 };
    let gst = (brokerage + transaction + sebi) * 0.18;
    brokerage + transaction + sebi + ctt + stamp + gst
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
fn pivot_low(b: &[Candle], i: usize, n: usize) -> Option<f64> {
    if i < n * 2 {
        return None;
    }
    let c = i - n;
    let x = b[c].low;
    (b[c - n..c].iter().all(|z| x < z.low) && b[c + 1..=c + n].iter().all(|z| x <= z.low))
        .then_some(x)
}

fn ema(prev: Option<f64>, x: f64, len: usize) -> f64 {
    let a = 2.0 / (len as f64 + 1.0);
    prev.map_or(x, |p| p * (1.0 - a) + x * a)
}

fn simulate(bars: &[Candle]) -> Result<Vec<Trade>> {
    ensure!(bars.len() > 200, "insufficient bars");
    const SWING: usize = 21;
    const LOOKBACK: usize = 80;
    const MIN_SCORE: f64 = 50.0;
    const MINOR: usize = 8;
    const CONFIRM: usize = 13;
    const VOL_LEN: usize = 21;
    const MAX_STORED: usize = 25;

    let mut atr = Wilder::new(14);
    let mut prev_close: Option<f64> = None;
    let mut vols: VecDeque<f64> = VecDeque::new();
    let mut highs: Vec<Swing> = Vec::new();
    let mut lows: Vec<Swing> = Vec::new();
    let mut last_minor_hi: Option<f64> = None;
    let mut last_minor_lo: Option<f64> = None;
    let mut pending: Option<Pending> = None;
    let mut pos: Option<Position> = None;
    let mut trades = Vec::new();

    // Approximate non-repainting 4H bias by aggregating 3m closes into 80-bar buckets
    // and using the prior completed bucket close / EMA(50).
    let mut htf_ema: Option<f64> = None;
    let mut completed_htf_close: Option<f64> = None;
    let mut bucket_last_close = None;
    let mut bucket_idx = 0usize;

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
        let Some(risk_atr) = atr.update(tr) else {
            continue;
        };

        vols.push_back(c.volume as f64);
        if vols.len() > VOL_LEN {
            vols.pop_front();
        }
        let vol_sma = if vols.len() == VOL_LEN {
            Some(vols.iter().sum::<f64>() / VOL_LEN as f64)
        } else {
            None
        };
        let vol_ratio = vol_sma.filter(|m| *m > 0.0).map(|m| c.volume as f64 / m);
        let vol_comp = vol_ratio
            .map(|r| ((r - 1.0) / 0.5).clamp(0.0, 1.0))
            .unwrap_or(0.5);

        let cur_bucket = i / 80;
        if cur_bucket != bucket_idx {
            if let Some(cl) = bucket_last_close {
                completed_htf_close = Some(cl);
                htf_ema = Some(ema(htf_ema, cl, 50));
            }
            bucket_idx = cur_bucket;
        }
        bucket_last_close = Some(c.close);
        let htf_bull = completed_htf_close
            .zip(htf_ema)
            .is_some_and(|(cl, e)| cl > e);
        let htf_bear = completed_htf_close
            .zip(htf_ema)
            .is_some_and(|(cl, e)| cl < e);
        let htf_bull_comp = if htf_bull { 1.0 } else { 0.0 };
        let htf_bear_comp = if htf_bear { 1.0 } else { 0.0 };

        if let Some(v) = pivot_high(bars, i, SWING) {
            highs.push(Swing {
                level: v,
                bar: i - SWING,
                used: false,
            });
            if highs.len() > MAX_STORED {
                highs.remove(0);
            }
        }
        if let Some(v) = pivot_low(bars, i, SWING) {
            lows.push(Swing {
                level: v,
                bar: i - SWING,
                used: false,
            });
            if lows.len() > MAX_STORED {
                lows.remove(0);
            }
        }
        if let Some(v) = pivot_high(bars, i, MINOR) {
            last_minor_hi = Some(v);
        }
        if let Some(v) = pivot_low(bars, i, MINOR) {
            last_minor_lo = Some(v);
        }

        let warmed = i >= 50;
        let mut bull: Option<(f64, f64)> = None;
        for s in lows.iter_mut().rev() {
            if s.used || i.saturating_sub(s.bar) > LOOKBACK {
                continue;
            }
            if c.close < s.level {
                s.used = true;
                continue;
            }
            if c.low < s.level && c.close > s.level {
                s.used = true;
                let rng = c.high - c.low;
                if rng > 0.0 {
                    let wick = (c.open.min(c.close) - c.low).max(0.0);
                    let reclaim = (c.close - s.level).max(0.0);
                    let cp = (c.close - c.low) / rng;
                    let score = ((wick / risk_atr).clamp(0.0, 1.0) * 0.30
                        + (reclaim / risk_atr).clamp(0.0, 1.0) * 0.25
                        + cp * 0.20
                        + vol_comp * 0.15
                        + htf_bull_comp * 0.10)
                        * 100.0;
                    if warmed && score >= MIN_SCORE {
                        bull = Some((s.level, score));
                    }
                }
                break;
            }
        }
        let mut bear: Option<(f64, f64)> = None;
        for s in highs.iter_mut().rev() {
            if s.used || i.saturating_sub(s.bar) > LOOKBACK {
                continue;
            }
            if c.close > s.level {
                s.used = true;
                continue;
            }
            if c.high > s.level && c.close < s.level {
                s.used = true;
                let rng = c.high - c.low;
                if rng > 0.0 {
                    let wick = (c.high - c.open.max(c.close)).max(0.0);
                    let reclaim = (s.level - c.close).max(0.0);
                    let cp = 1.0 - (c.close - c.low) / rng;
                    let score = ((wick / risk_atr).clamp(0.0, 1.0) * 0.30
                        + (reclaim / risk_atr).clamp(0.0, 1.0) * 0.25
                        + cp * 0.20
                        + vol_comp * 0.15
                        + htf_bear_comp * 0.10)
                        * 100.0;
                    if warmed && score >= MIN_SCORE {
                        bear = Some((s.level, score));
                    }
                }
                break;
            }
        }

        if let Some((lvl, score)) = bull {
            pending = Some(Pending {
                side: 1,
                start: i,
                level: lvl,
                wick: c.low,
                score,
            });
        }
        if let Some((lvl, score)) = bear {
            pending = Some(Pending {
                side: -1,
                start: i,
                level: lvl,
                wick: c.high,
                score,
            });
        }
        if pending
            .as_ref()
            .is_some_and(|p| i.saturating_sub(p.start) > CONFIRM)
        {
            pending = None;
        }

        let mut fire: Option<Pending> = None;
        if let Some(p) = pending.clone() {
            let ok = if p.side == 1 {
                last_minor_hi.is_some_and(|h| c.close > h)
            } else {
                last_minor_lo.is_some_and(|l| c.close < l)
            };
            if ok {
                fire = Some(p);
                pending = None;
            }
        }

        // manage existing trade first; stop wins same-bar ambiguity
        if let Some(mut p) = pos.take() {
            if i > p.entry_idx {
                let stop = if p.side == 1 {
                    c.low <= p.sl
                } else {
                    c.high >= p.sl
                };
                let tp1 = if p.side == 1 {
                    c.high >= p.tp1
                } else {
                    c.low <= p.tp1
                };
                let tp2 = if p.side == 1 {
                    c.high >= p.tp2
                } else {
                    c.low <= p.tp2
                };
                let tp3 = if p.side == 1 {
                    c.high >= p.tp3
                } else {
                    c.low <= p.tp3
                };
                if stop || tp3 {
                    let exit = if stop { p.sl } else { p.tp3 };
                    let reason = if stop && p.be {
                        "BE"
                    } else if stop {
                        "SL"
                    } else {
                        "TP3"
                    };
                    let points = (exit - p.entry) * p.side as f64;
                    trades.push(Trade {
                        entry_time: p.entry_time.clone(),
                        exit_time: c.timestamp.clone(),
                        side: if p.side == 1 { "LONG" } else { "SHORT" },
                        entry: p.entry,
                        exit,
                        sl_initial: if p.side == 1 {
                            p.entry - (p.tp1 - p.entry)
                        } else {
                            p.entry + (p.entry - p.tp1)
                        },
                        tp1: p.tp1,
                        tp2: p.tp2,
                        tp3: p.tp3,
                        reason,
                        points,
                    });
                } else {
                    if tp1 && !p.hit1 {
                        p.hit1 = true;
                        if !p.be {
                            p.sl = p.entry;
                            p.be = true;
                        }
                    }
                    if tp2 && !p.hit2 {
                        p.hit2 = true;
                    }
                    pos = Some(p);
                }
            } else {
                pos = Some(p)
            }
        }

        if pos.is_none()
            && let Some(f) = fire
        {
            let entry = c.close;
            let mut sl = if f.side == 1 {
                f.wick - risk_atr * 0.25
            } else {
                f.wick + risk_atr * 0.25
            };
            let mut dist = (entry - sl).abs();
            if dist < risk_atr * 0.5 {
                dist = risk_atr * 0.5;
                sl = if f.side == 1 {
                    entry - dist
                } else {
                    entry + dist
                };
            }
            if dist > 0.0 {
                pos = Some(Position {
                    side: f.side,
                    entry,
                    sl,
                    tp1: entry + f.side as f64 * dist,
                    tp2: entry + f.side as f64 * dist * 2.0,
                    tp3: entry + f.side as f64 * dist * 3.0,
                    be: false,
                    hit1: false,
                    hit2: false,
                    entry_idx: i,
                    entry_time: c.timestamp.clone(),
                });
            }
            let _ = (f.level, f.score);
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
    let charges: f64 = scored
        .iter()
        .map(|t| {
            let buy = t.side == "LONG";
            crude_leg_charges(t.entry, buy) + crude_leg_charges(t.exit, !buy)
        })
        .sum();
    let summary = Summary {
        strategy: "Mirage Liquidity Sweep Pro v1.3.1 defaults",
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
