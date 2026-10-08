//! Research-only second setup for ILRC: directional continuation.
//! No execution path. Production ILRC configuration is not modified.
use anyhow::{Result, ensure};
use kite_adapter::http::historical::{Candle, Interval};
use serde::Serialize;
use std::collections::VecDeque;

#[derive(Debug, Clone, Serialize)]
pub struct Trade {
    pub entry_time: String,
    pub exit_time: String,
    pub side: &'static str,
    pub entry: f64,
    pub exit: f64,
    pub stop: f64,
    pub target: f64,
    pub reason: &'static str,
    pub points: f64,
}

#[derive(Debug, Clone, Serialize)]
struct Summary {
    setup: &'static str,
    trades: usize,
    wins: usize,
    losses: usize,
    breakeven: usize,
    gross_points: f64,
    profit_factor: f64,
    gross_inr: f64,
    estimated_charges_inr: f64,
    net_inr: f64,
    max_trades_in_day: usize,
    days_with_4plus: usize,
    avg_trades_per_active_day: f64,
}

#[derive(Clone)]
struct Pending {
    side: i8,
    created_idx: usize,
    zone_low: f64,
    zone_high: f64,
    stop_anchor: f64,
}

#[derive(Clone)]
struct Position {
    side: i8,
    entry: f64,
    stop: f64,
    initial_stop: f64,
    target: f64,
    initial_risk: f64,
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
            flush(&mut bucket, &mut out);
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

fn simulate(bars: &[Candle], target_r: f64) -> Result<Vec<Trade>> {
    simulate_with_events(bars, target_r, &mut Vec::new(), false)
}

fn simulate_with_events(
    bars: &[Candle],
    target_r: f64,
    events: &mut Vec<super::ilrc_backtest::EntryEvent>,
    incremental: bool,
) -> Result<Vec<Trade>> {
    use chrono::Timelike;
    ensure!(incremental || bars.len() > 100, "insufficient bars");

    const ATR_LEN: usize = 14;
    const BODY_LEN: usize = 20;
    const SWING_LEN: usize = 20;
    const INTERNAL_LEN: usize = 5;
    const RETRACE_BARS: usize = 5;
    const BODY_MULT: f64 = 1.3;
    const ATR_MULT: f64 = 0.8;
    const STOP_BUFFER_ATR: f64 = 0.15;
    const MIN_STOP_ATR: f64 = 0.5;
    const MAX_STOP_ATR: f64 = 1.5;
    const BE_R: f64 = 1.0;

    let mut atr = Wilder::new(ATR_LEN);
    let mut prev_close: Option<f64> = None;
    let mut bodies = VecDeque::<f64>::new();
    let mut highs = VecDeque::<f64>::new();
    let mut lows = VecDeque::<f64>::new();
    let mut cur_date = None;
    let mut pv = 0.0;
    let mut volume_sum = 0.0;
    let mut pending: Option<Pending> = None;
    let mut position: Option<Position> = None;
    let mut trades = Vec::new();

    for (idx, c) in bars.iter().enumerate() {
        let t = c.time()?;
        let minute = t.hour() * 60 + t.minute();
        let date = t.date_naive();

        if cur_date != Some(date) {
            cur_date = Some(date);
            pv = 0.0;
            volume_sum = 0.0;
            pending = None;
        }

        let tr = if let Some(pc) = prev_close {
            (c.high - c.low)
                .max((c.high - pc).abs())
                .max((c.low - pc).abs())
        } else {
            c.high - c.low
        };
        prev_close = Some(c.close);
        let a = atr.update(tr).unwrap_or(0.0);

        let body = (c.close - c.open).abs();
        bodies.push_back(body);
        if bodies.len() > BODY_LEN {
            bodies.pop_front();
        }
        let avg_body = if bodies.is_empty() {
            0.0
        } else {
            bodies.iter().sum::<f64>() / bodies.len() as f64
        };

        pv += ((c.high + c.low + c.close) / 3.0) * c.volume as f64;
        volume_sum += c.volume as f64;
        let vwap = if volume_sum > 0.0 {
            pv / volume_sum
        } else {
            c.close
        };

        let prior_hi = highs
            .iter()
            .rev()
            .take(SWING_LEN)
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        let prior_lo = lows
            .iter()
            .rev()
            .take(SWING_LEN)
            .copied()
            .fold(f64::INFINITY, f64::min);
        let internal_hi = highs
            .iter()
            .rev()
            .take(INTERNAL_LEN)
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        let internal_lo = lows
            .iter()
            .rev()
            .take(INTERNAL_LEN)
            .copied()
            .fold(f64::INFINITY, f64::min);

        // Existing position: stop first, then target, then EOD.
        if let Some(mut p) = position.take() {
            let stop_hit = if p.side > 0 {
                c.low <= p.stop
            } else {
                c.high >= p.stop
            };
            let target_hit = if p.side > 0 {
                c.high >= p.target
            } else {
                c.low <= p.target
            };
            let eod = minute + 3 >= 1395;

            if stop_hit || target_hit || eod {
                let px = if stop_hit {
                    p.stop
                } else if target_hit {
                    p.target
                } else {
                    c.close
                };
                let points = (px - p.entry) * p.side as f64;
                trades.push(Trade {
                    entry_time: p.entry_time,
                    exit_time: t.to_rfc3339(),
                    side: if p.side > 0 { "LONG" } else { "SHORT" },
                    entry: p.entry,
                    exit: px,
                    stop: p.initial_stop,
                    target: p.target,
                    reason: if stop_hit && p.be {
                        "BE"
                    } else if stop_hit {
                        "SL"
                    } else if target_hit {
                        "TP"
                    } else {
                        "EOD"
                    },
                    points,
                });
            } else {
                if !p.be {
                    let be_trigger = if p.side > 0 {
                        c.high >= p.entry + p.initial_risk * BE_R
                    } else {
                        c.low <= p.entry - p.initial_risk * BE_R
                    };
                    if be_trigger {
                        p.stop = p.entry;
                        p.be = true;
                    }
                }
                position = Some(p);
            }
        }

        if position.is_none()
            && (540..1395).contains(&minute)
            && a > 0.0
            && idx > SWING_LEN.max(BODY_LEN)
        {
            // Pullback entry after a continuation displacement.
            if let Some(q) = pending.clone() {
                if idx > q.created_idx + RETRACE_BARS {
                    pending = None;
                } else {
                    let touches = c.low <= q.zone_high && c.high >= q.zone_low;
                    if touches {
                        let entry = (q.zone_low + q.zone_high) * 0.5;
                        let raw_stop = if q.side > 0 {
                            q.stop_anchor - a * STOP_BUFFER_ATR
                        } else {
                            q.stop_anchor + a * STOP_BUFFER_ATR
                        };
                        let risk = (entry - raw_stop) * q.side as f64;
                        if risk >= a * MIN_STOP_ATR && risk <= a * MAX_STOP_ATR {
                            let target = entry + q.side as f64 * risk * target_r;
                            events.push(super::ilrc_backtest::EntryEvent {
                                setup: "B",
                                entry_time: t.to_rfc3339(),
                                observed_at: (t + chrono::Duration::minutes(3)).to_rfc3339(),
                                side: if q.side > 0 { "LONG" } else { "SHORT" },
                                entry,
                                stop: raw_stop,
                                target,
                            });
                            position = Some(Position {
                                side: q.side,
                                entry,
                                stop: raw_stop,
                                initial_stop: raw_stop,
                                target,
                                initial_risk: risk,
                                be: false,
                                entry_time: t.to_rfc3339(),
                            });
                            pending = None;
                        }
                    }
                }
            }

            // Independent continuation trigger: external swing break + displacement + VWAP.
            if pending.is_none() && position.is_none() {
                let bull = prior_hi.is_finite()
                    && c.close > prior_hi
                    && c.close > internal_hi
                    && c.close > c.open
                    && c.close >= vwap
                    && body >= avg_body * BODY_MULT
                    && body >= a * ATR_MULT;
                let bear = prior_lo.is_finite()
                    && c.close < prior_lo
                    && c.close < internal_lo
                    && c.close < c.open
                    && c.close <= vwap
                    && body >= avg_body * BODY_MULT
                    && body >= a * ATR_MULT;

                if bull || bear {
                    let side = if bull { 1 } else { -1 };
                    let midpoint = (c.open + c.close) * 0.5;
                    let (zone_low, zone_high) = if side > 0 {
                        (c.open.min(midpoint), c.open.max(midpoint))
                    } else {
                        (midpoint.min(c.open), midpoint.max(c.open))
                    };
                    pending = Some(Pending {
                        side,
                        created_idx: idx,
                        zone_low,
                        zone_high,
                        stop_anchor: if side > 0 { c.low } else { c.high },
                    });
                }
            }
        }

        highs.push_back(c.high);
        lows.push_back(c.low);
        if highs.len() > 100 {
            highs.pop_front();
            lows.pop_front();
        }
    }
    Ok(trades)
}

pub fn entry_events_candles(
    candles: &[Candle],
    target_r: f64,
) -> Result<Vec<super::ilrc_backtest::EntryEvent>> {
    let mut events = Vec::new();
    let _ = simulate_with_events(candles, target_r, &mut events, true)?;
    Ok(events)
}

pub fn evaluate_candles(candles: &[Candle], target_r: f64, date: &str) -> Result<Vec<Trade>> {
    ensure!(target_r > 0.0, "continuation target R must be positive");
    let trades = simulate(candles, target_r)?;
    Ok(trades
        .into_iter()
        .filter(|t| t.exit_time.starts_with(date))
        .collect())
}

fn summarize(setup: &'static str, trades: &[Trade]) -> Summary {
    use std::collections::BTreeMap;
    let wins = trades.iter().filter(|t| t.points > 1e-9).count();
    let losses = trades.iter().filter(|t| t.points < -1e-9).count();
    let breakeven = trades.len() - wins - losses;
    let gross_points = trades.iter().map(|t| t.points).sum::<f64>();
    let gp = trades
        .iter()
        .filter(|t| t.points > 0.0)
        .map(|t| t.points)
        .sum::<f64>();
    let gl = -trades
        .iter()
        .filter(|t| t.points < 0.0)
        .map(|t| t.points)
        .sum::<f64>();
    let charges = trades
        .iter()
        .map(|t| {
            let entry_buy = t.side == "LONG";
            crude_leg_charges(t.entry, entry_buy) + crude_leg_charges(t.exit, !entry_buy)
        })
        .sum::<f64>();
    let mut per_day = BTreeMap::<String, usize>::new();
    for t in trades {
        *per_day.entry(t.exit_time[..10].to_string()).or_default() += 1;
    }
    let active_days = per_day.len();
    Summary {
        setup,
        trades: trades.len(),
        wins,
        losses,
        breakeven,
        gross_points,
        profit_factor: if gl > 0.0 { gp / gl } else { 0.0 },
        gross_inr: gross_points * 100.0,
        estimated_charges_inr: charges,
        net_inr: gross_points * 100.0 - charges,
        max_trades_in_day: per_day.values().copied().max().unwrap_or(0),
        days_with_4plus: per_day.values().filter(|&&n| n >= 4).count(),
        avg_trades_per_active_day: if active_days > 0 {
            trades.len() as f64 / active_days as f64
        } else {
            0.0
        },
    }
}

#[derive(Clone)]
struct CandidateTrade {
    entry_time: String,
    exit_time: String,
    side: &'static str,
    entry: f64,
    exit: f64,
    stop: f64,
    target: f64,
    reason: &'static str,
    points: f64,
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
    let continuation = simulate(&bars, 2.0)?
        .into_iter()
        .filter(|t| {
            t.exit_time.as_str() >= "2026-08-17"
                && t.exit_time.as_str() <= "2026-10-07T23:59:59+05:30"
        })
        .collect::<Vec<_>>();
    let target_variants = [1.5_f64, 2.0, 2.5, 3.0]
        .into_iter()
        .map(|target_r| {
            let trades = simulate(&bars, target_r)?
                .into_iter()
                .filter(|t| {
                    t.exit_time.as_str() >= "2026-08-17"
                        && t.exit_time.as_str() <= "2026-10-07T23:59:59+05:30"
                })
                .collect::<Vec<_>>();
            Ok(serde_json::json!({
                "target_r": target_r,
                "summary": summarize("ILRC_continuation_target_variant", &trades),
                "trades": trades
            }))
        })
        .collect::<Result<Vec<_>>>()?;

    // Existing ILRC uses its current production-style defaults, including 1R BE.
    let ilrc_params = super::ilrc_backtest::Params {
        break_even_r: 1.0,
        ..super::ilrc_backtest::Params::default()
    };
    let ilrc = super::ilrc_backtest::research_trades_for_params(
        name,
        &bars,
        3,
        540,
        1395,
        100,
        true,
        ilrc_params,
    )?;

    // Approximate combined portfolio with one active trade at a time:
    // sort candidate trades by entry time and skip overlaps.
    let mut candidates = Vec::<CandidateTrade>::new();
    for t in &ilrc {
        candidates.push(CandidateTrade {
            entry_time: t.entry_time.clone(),
            exit_time: t.exit_time.clone(),
            side: t.side,
            entry: t.entry,
            exit: t.exit,
            stop: t.stop,
            target: t.target,
            reason: t.reason,
            points: t.points,
        });
    }
    for t in &continuation {
        candidates.push(CandidateTrade {
            entry_time: t.entry_time.clone(),
            exit_time: t.exit_time.clone(),
            side: t.side,
            entry: t.entry,
            exit: t.exit,
            stop: t.stop,
            target: t.target,
            reason: t.reason,
            points: t.points,
        });
    }
    candidates.sort_by(|a, b| a.entry_time.cmp(&b.entry_time));
    let mut combined = Vec::<Trade>::new();
    let mut busy_until = String::new();
    for c in candidates {
        if !busy_until.is_empty() && c.entry_time < busy_until {
            continue;
        }
        busy_until = c.exit_time.clone();
        combined.push(Trade {
            entry_time: c.entry_time,
            exit_time: c.exit_time,
            side: c.side,
            entry: c.entry,
            exit: c.exit,
            stop: c.stop,
            target: c.target,
            reason: c.reason,
            points: c.points,
        });
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "strategy": "ILRC_second_setup_research",
            "instrument": name,
            "period": "2026-08-17 through 2026-10-07",
            "ilrc": summarize("ILRC_v1_BE1R", &ilrc.iter().map(|t| Trade {
                entry_time: t.entry_time.clone(),
                exit_time: t.exit_time.clone(),
                side: t.side,
                entry: t.entry,
                exit: t.exit,
                stop: t.stop,
                target: t.target,
                reason: t.reason,
                points: t.points,
            }).collect::<Vec<_>>()),
            "continuation": summarize("ILRC_continuation_v0", &continuation),
            "target_variants": target_variants,
            "combined": summarize("ILRC_plus_continuation_v0", &combined),
            "continuation_trades": continuation,
            "combined_trades": combined
        }))?
    );
    Ok(())
}
