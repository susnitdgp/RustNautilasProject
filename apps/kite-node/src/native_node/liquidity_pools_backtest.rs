//! Research-only backtest port of Liquidity Pools Pro v1.2.0 defaults.
//! Source supplied by the user. No execution path.
use anyhow::{Result, ensure};
use kite_adapter::http::historical::{Candle, Interval};
use serde::Serialize;

#[derive(Debug, Clone)]
struct Pool {
    level: f64,
    top: f64,
    bot: f64,
    last_touch: usize,
    touches: u32,
    volume: f64,
    is_high: bool,
    state: u8, // 0 active, 1 swept, 2 mitigated
    htf_bonus: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Trade {
    pub entry_time: String,
    pub exit_time: String,
    pub side: &'static str,
    pub entry: f64,
    pub exit: f64,
    pub stop_initial: f64,
    pub tp1: f64,
    pub tp2: f64,
    pub tp3: f64,
    pub reason: &'static str,
    pub points: f64,
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

#[derive(Clone)]
struct Position {
    side: i8,
    entry: f64,
    sl: f64,
    initial_sl: f64,
    tp1: f64,
    tp2: f64,
    tp3: f64,
    be: bool,
    entry_time: String,
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

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) * 0.5
    })
}

fn strength(pool: &Pool, idx: usize, half_life: f64, vol_median: Option<f64>) -> f64 {
    let age = idx.saturating_sub(pool.last_touch) as f64;
    let decay = 0.5_f64.powf(age / half_life);
    let touch_pts = (pool.touches.saturating_sub(1) as f64 * 9.0).min(35.0);
    let recency = 35.0 * decay;
    let vol_pts = vol_median
        .filter(|m| *m > 0.0)
        .map(|m| {
            let r = (pool.volume / m).max(1.0);
            (r.ln() * 8.0).min(20.0)
        })
        .unwrap_or(0.0);
    (touch_pts + recency + vol_pts + pool.htf_bonus).clamp(0.0, 100.0)
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
        let first = bucket[0];
        let last = *bucket.last().expect("bucket");
        out.push(Candle {
            timestamp: first.timestamp.clone(),
            open: first.open,
            high: bucket
                .iter()
                .map(|x| x.high)
                .fold(f64::NEG_INFINITY, f64::max),
            low: bucket.iter().map(|x| x.low).fold(f64::INFINITY, f64::min),
            close: last.close,
            volume: bucket.iter().map(|x| x.volume).sum(),
            oi: last.oi,
        });
        bucket.clear();
    };
    for c in candles {
        let t = c.time()?;
        let minute = t.hour() * 60 + t.minute();
        if minute < session_open {
            continue;
        }
        let k = (t.date_naive(), (minute - session_open) / minutes as u32);
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

fn pivot_high(bars: &[Candle], candidate: usize, left: usize, right: usize) -> bool {
    if candidate < left || candidate + right >= bars.len() {
        return false;
    }
    let x = bars[candidate].high;
    bars[candidate - left..candidate].iter().all(|b| x > b.high)
        && bars[candidate + 1..=candidate + right]
            .iter()
            .all(|b| x >= b.high)
}
fn pivot_low(bars: &[Candle], candidate: usize, left: usize, right: usize) -> bool {
    if candidate < left || candidate + right >= bars.len() {
        return false;
    }
    let x = bars[candidate].low;
    bars[candidate - left..candidate].iter().all(|b| x < b.low)
        && bars[candidate + 1..=candidate + right]
            .iter()
            .all(|b| x <= b.low)
}

fn simulate(bars: &[Candle]) -> Result<Vec<Trade>> {
    ensure!(bars.len() > 100, "insufficient bars");
    const RIGHT: usize = 2;
    const MAX_LOOKBACK: usize = 200;
    const MAX_POOLS: usize = 40;
    const MIN_STRENGTH: f64 = 25.0;
    const HALF_LIFE: f64 = 150.0;

    let mut atr14 = Wilder::new(14);
    let mut atr50 = Wilder::new(50);
    let mut prev_close: Option<f64> = None;
    let mut pools: Vec<Pool> = Vec::new();
    let mut vol_median = None;
    let mut position: Option<Position> = None;
    let mut trades = Vec::new();

    for idx in 0..bars.len() {
        let c = &bars[idx];
        let tr = if let Some(pc) = prev_close {
            (c.high - c.low)
                .max((c.high - pc).abs())
                .max((c.low - pc).abs())
        } else {
            c.high - c.low
        };
        let fast = atr14.update(tr);
        let slow = atr50.update(tr);
        prev_close = Some(c.close);
        let (Some(atr_fast), Some(atr_slow)) = (fast, slow) else {
            continue;
        };
        let ratio = if atr_slow > 0.0 {
            atr_fast / atr_slow
        } else {
            1.0
        };
        let left = (10.0 / ratio.max(0.5)).clamp(4.0, 16.0).round() as usize;
        let tolerance = atr_fast * 0.25;
        let warmed = idx >= 50;

        // Pine position lifecycle: existing position can close on this bar.
        let mut closed_this_bar = false;
        if let Some(mut p) = position.take() {
            let stop = if p.side > 0 {
                c.low <= p.sl
            } else {
                c.high >= p.sl
            };
            let tp1 = if p.side > 0 {
                c.high >= p.tp1
            } else {
                c.low <= p.tp1
            };
            let tp3 = if p.side > 0 {
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
                    side: if p.side > 0 { "LONG" } else { "SHORT" },
                    entry: p.entry,
                    exit,
                    stop_initial: p.initial_sl,
                    tp1: p.tp1,
                    tp2: p.tp2,
                    tp3: p.tp3,
                    reason,
                    points,
                });
                closed_this_bar = true;
            } else {
                if tp1 && !p.be {
                    p.sl = p.entry;
                    p.be = true;
                }
                position = Some(p);
            }
        }

        // New confirmed pivots.
        if warmed && idx >= RIGHT {
            let cand = idx - RIGHT;
            let pvol = bars[cand].volume as f64;
            for is_high in [true, false] {
                let is_pivot = if is_high {
                    pivot_high(bars, cand, left, RIGHT)
                } else {
                    pivot_low(bars, cand, left, RIGHT)
                };
                if !is_pivot {
                    continue;
                }
                let price = if is_high {
                    bars[cand].high
                } else {
                    bars[cand].low
                };
                if let Some(pi) = pools.iter().position(|p| {
                    p.state == 0
                        && p.is_high == is_high
                        && (price - p.level).abs() <= tolerance
                        && idx.saturating_sub(p.last_touch) <= MAX_LOOKBACK
                }) {
                    let p = &mut pools[pi];
                    let n = p.touches as f64;
                    p.level = (p.level * n + price) / (n + 1.0);
                    p.top = p.level + tolerance / 2.0;
                    p.bot = p.level - tolerance / 2.0;
                    p.last_touch = cand;
                    p.touches += 1;
                    p.volume += pvol;
                } else {
                    pools.push(Pool {
                        level: price,
                        top: price + tolerance / 2.0,
                        bot: price - tolerance / 2.0,
                        last_touch: cand,
                        touches: 1,
                        volume: pvol,
                        is_high,
                        state: 0,
                        htf_bonus: 0.0,
                    });
                }
            }
        }

        // Pine refreshes median every 25 bars or first time enough pools exist.
        if pools.len() >= 3 && (vol_median.is_none() || idx % 25 == 0) {
            vol_median = median(pools.iter().map(|p| p.volume).collect());
        }

        // Volume at level accumulation.
        if warmed {
            let bar_vol = c.volume as f64;
            if bar_vol > 0.0 {
                for p in &mut pools {
                    if p.state != 0 || idx <= p.last_touch {
                        continue;
                    }
                    let touched = (c.high >= p.bot && c.high <= p.top)
                        || (c.low >= p.bot && c.low <= p.top)
                        || (c.low <= p.bot && c.high >= p.top);
                    if touched {
                        p.volume += bar_vol;
                    }
                }
            }
        }

        // Sweeps and signals.
        let mut buy = false;
        let mut sell = false;
        let mut best_strength: f64 = 0.0;
        if warmed {
            for p in &mut pools {
                if p.state != 0 || idx < p.last_touch + RIGHT + 1 {
                    continue;
                }
                let score = strength(p, idx, HALF_LIFE, vol_median);
                let wick = if p.is_high {
                    c.high > p.top
                } else {
                    c.low < p.bot
                };
                let close_back = if p.is_high {
                    c.close < p.level
                } else {
                    c.close > p.level
                };
                if wick {
                    p.state = if close_back { 2 } else { 1 };
                    if close_back && score >= MIN_STRENGTH {
                        if p.is_high {
                            sell = true
                        } else {
                            buy = true
                        }
                        best_strength = best_strength.max(score);
                    }
                }
            }
        }
        let _ = best_strength;

        // Enforce max pool count: oldest non-active first, else weakest active.
        if pools.len() > MAX_POOLS {
            if let Some(i) = pools
                .iter()
                .enumerate()
                .filter(|(_, p)| p.state != 0)
                .min_by_key(|(_, p)| p.last_touch)
                .map(|(i, _)| i)
            {
                pools.remove(i);
            } else if let Some(i) = pools
                .iter()
                .enumerate()
                .filter(|(_, p)| p.state == 0)
                .min_by(|(_, a), (_, b)| {
                    strength(a, idx, HALF_LIFE, vol_median)
                        .total_cmp(&strength(b, idx, HALF_LIFE, vol_median))
                })
                .map(|(i, _)| i)
            {
                pools.remove(i);
            }
        }

        // Pine allows a replacement trade on a bar which closes the prior trade.
        let may_enter = position.is_none();
        if may_enter && (buy || sell) {
            let side = if buy { 1 } else { -1 }; // buy wins if both
            let risk = atr_fast * 1.5; // Balanced preset, ATR stop mode
            if risk > 0.0 {
                let entry = c.close;
                let sl = entry - side as f64 * risk;
                position = Some(Position {
                    side,
                    entry,
                    sl,
                    initial_sl: sl,
                    tp1: entry + side as f64 * risk,
                    tp2: entry + side as f64 * risk * 2.0,
                    tp3: entry + side as f64 * risk * 3.0,
                    be: false,
                    entry_time: c.timestamp.clone(),
                });
            }
        }
        let _ = closed_this_bar;
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
    let start = "2026-08-17";
    let scored: Vec<_> = trades
        .into_iter()
        .filter(|t| {
            t.exit_time.as_str() >= start && t.exit_time.as_str() <= "2026-10-07T23:59:59+05:30"
        })
        .collect();
    let wins = scored.iter().filter(|t| t.points > 0.0).count();
    let losses = scored.iter().filter(|t| t.points < 0.0).count();
    let be = scored.iter().filter(|t| t.points.abs() < 1e-9).count();
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
            let entry_buy = t.side == "LONG";
            let exit_buy = t.side == "SHORT";
            crude_leg_charges(t.entry, entry_buy) + crude_leg_charges(t.exit, exit_buy)
        })
        .sum();
    let summary = Summary {
        strategy: "Liquidity Pools Pro v1.2.0 defaults",
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
