//! Institutional Liquidity Reversal & Continuation (ILRC) v1 research engine.
//! Backtest-only prototype. No live order path is exposed from this module.
use anyhow::{Result, ensure};
use kite_adapter::http::historical::{Candle, Interval};
use serde::Serialize;
use std::collections::{BTreeMap, VecDeque};

#[derive(Debug, Clone, Copy, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Params {
    pub atr_len: usize,
    pub avg_body_len: usize,
    pub swing_len: usize,
    pub internal_len: usize,
    pub displacement_body_mult: f64,
    pub displacement_atr_mult: f64,
    pub sweep_buffer_atr: f64,
    pub min_stop_atr: f64,
    pub max_stop_atr: f64,
    pub min_rr: f64,
    pub retrace_bars: usize,
}
impl Params {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.atr_len > 0
                && self.avg_body_len > 0
                && self.swing_len > 0
                && self.internal_len > 0
                && self.retrace_bars > 0,
            "ILRC lookbacks must be positive"
        );
        ensure!(
            self.displacement_body_mult > 0.0
                && self.displacement_atr_mult > 0.0
                && self.sweep_buffer_atr >= 0.0
                && self.min_stop_atr > 0.0
                && self.max_stop_atr >= self.min_stop_atr
                && self.min_rr > 0.0,
            "ILRC thresholds are invalid"
        );
        Ok(())
    }
}

impl Default for Params {
    fn default() -> Self {
        Self {
            atr_len: 14,
            avg_body_len: 20,
            swing_len: 20,
            internal_len: 5,
            displacement_body_mult: 1.3,
            displacement_atr_mult: 0.8,
            sweep_buffer_atr: 0.15,
            min_stop_atr: 0.5,
            max_stop_atr: 1.5,
            min_rr: 1.5,
            retrace_bars: 5,
        }
    }
}

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
pub struct Summary {
    pub instrument: String,
    pub timeframe_minutes: u64,
    pub bars: usize,
    pub trades: usize,
    pub wins: usize,
    pub losses: usize,
    pub gross_points: f64,
    pub average_points: f64,
    pub profit_factor: f64,
    pub gross_inr: f64,
    pub estimated_charges_inr: f64,
    pub net_inr: f64,
    pub max_drawdown_inr: f64,
    pub positive_days: usize,
    pub negative_days: usize,
}

#[derive(Debug, Clone)]
struct Sweep {
    side: i8,
    extreme: f64,
    created_idx: usize,
    opposing_liquidity: f64,
}

#[derive(Debug, Clone)]
struct Pending {
    side: i8,
    sweep_extreme: f64,
    created_idx: usize,
    zone_low: f64,
    zone_high: f64,
    target_liquidity: f64,
}

fn aggregate(candles: &[Candle], minutes: u64, session_open: u32) -> Result<Vec<Candle>> {
    use chrono::Timelike;
    if minutes == 1 {
        return Ok(candles.to_vec());
    }
    let mut out = Vec::new();
    let mut bucket: Vec<&Candle> = Vec::new();
    let mut key = None;
    let flush = |bucket: &mut Vec<&Candle>, out: &mut Vec<Candle>| {
        if bucket.is_empty() {
            return;
        }
        let f = bucket[0];
        let l = bucket[bucket.len() - 1];
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
        let moday = t.hour() * 60 + t.minute();
        if moday < session_open {
            continue;
        }
        let k = (t.date_naive(), (moday - session_open) / minutes as u32);
        if key.is_some_and(|x| x != k) {
            flush(&mut bucket, &mut out)
        }
        key = Some(k);
        bucket.push(c);
    }
    flush(&mut bucket, &mut out);
    Ok(out)
}

fn leg_charges(price: f64, is_buy: bool, lot: u32) -> f64 {
    let notional = price * lot as f64;
    let brokerage = (notional * 0.0003).min(20.0);
    let transaction = notional * 0.0000173; // NFO futures current exchange charge approximation
    let sebi = notional * 0.000001;
    let stt = if is_buy { 0.0 } else { notional * 0.0002 };
    let stamp = if is_buy { notional * 0.00002 } else { 0.0 };
    let gst = (brokerage + transaction + sebi) * 0.18;
    brokerage + transaction + sebi + stt + stamp + gst
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

#[allow(clippy::too_many_arguments)]
fn simulate(
    name: &str,
    bars: &[Candle],
    minutes: u64,
    session_open: u32,
    session_close: u32,
    lot: u32,
    is_crude: bool,
    p: Params,
) -> Result<(Summary, Vec<Trade>)> {
    ensure!(bars.len() > 100, "insufficient bars");
    use chrono::Timelike;
    let mut trs = VecDeque::new();
    let mut bodies = VecDeque::new();
    let mut closes = VecDeque::new();
    let mut highs = VecDeque::new();
    let mut lows = VecDeque::new();
    let mut prev_close: Option<f64> = None;
    let mut atr = None;
    let mut atr_seed = 0.0;
    let mut atr_count = 0usize;
    let mut cur_date = None;
    let mut prev_day_hi = None;
    let mut prev_day_lo = None;
    let mut day_hi = f64::NEG_INFINITY;
    let mut day_lo = f64::INFINITY;
    let mut pv = 0.0;
    let mut vol = 0.0;
    let mut sweep: Option<Sweep> = None;
    let mut pending: Option<Pending> = None;
    let mut position: Option<(i8, f64, f64, f64, String)> = None;
    let mut trades = Vec::new();
    for (idx, c) in bars.iter().enumerate() {
        let t = c.time()?;
        let date = t.date_naive();
        let minute = t.hour() * 60 + t.minute();
        if cur_date != Some(date) {
            if cur_date.is_some() {
                prev_day_hi = Some(day_hi);
                prev_day_lo = Some(day_lo)
            }
            cur_date = Some(date);
            day_hi = f64::NEG_INFINITY;
            day_lo = f64::INFINITY;
            pv = 0.0;
            vol = 0.0;
            sweep = None;
            pending = None;
        }
        let tr = if let Some(pc) = prev_close {
            (c.high - c.low)
                .max((c.high - pc).abs())
                .max((c.low - pc).abs())
        } else {
            c.high - c.low
        };
        if atr_count < p.atr_len {
            atr_seed += tr;
            atr_count += 1;
            if atr_count == p.atr_len {
                atr = Some(atr_seed / p.atr_len as f64)
            }
        } else if let Some(a) = atr {
            atr = Some((a * (p.atr_len as f64 - 1.0) + tr) / p.atr_len as f64)
        }
        prev_close = Some(c.close);
        trs.push_back(tr);
        if trs.len() > p.atr_len {
            trs.pop_front();
        }
        let body = (c.close - c.open).abs();
        bodies.push_back(body);
        if bodies.len() > p.avg_body_len {
            bodies.pop_front();
        }
        pv += ((c.high + c.low + c.close) / 3.0) * c.volume as f64;
        vol += c.volume as f64;
        let vwap = if vol > 0.0 { pv / vol } else { c.close };
        let avg_body = if bodies.is_empty() {
            0.0
        } else {
            bodies.iter().sum::<f64>() / bodies.len() as f64
        };
        let prior_hi = highs
            .iter()
            .rev()
            .take(p.swing_len)
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        let prior_lo = lows
            .iter()
            .rev()
            .take(p.swing_len)
            .copied()
            .fold(f64::INFINITY, f64::min);
        let internal_hi = highs
            .iter()
            .rev()
            .take(p.internal_len)
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        let internal_lo = lows
            .iter()
            .rev()
            .take(p.internal_len)
            .copied()
            .fold(f64::INFINITY, f64::min);
        let a = atr.unwrap_or(0.0);

        if let Some((side, entry, sl, tp, entry_time)) = position.clone() {
            let stop = if side > 0 { c.low <= sl } else { c.high >= sl };
            let target = if side > 0 { c.high >= tp } else { c.low <= tp };
            let eod = minute + minutes as u32 >= session_close;
            if stop || target || eod {
                let px = if stop {
                    sl
                } else if target {
                    tp
                } else {
                    c.close
                };
                let pts = if side > 0 { px - entry } else { entry - px };
                trades.push(Trade {
                    entry_time,
                    exit_time: t.to_rfc3339(),
                    side: if side > 0 { "LONG" } else { "SHORT" },
                    entry,
                    exit: px,
                    stop: sl,
                    target: tp,
                    reason: if stop {
                        "SL"
                    } else if target {
                        "TP"
                    } else {
                        "EOD"
                    },
                    points: pts,
                });
                position = None;
                sweep = None;
                pending = None;
            }
        }

        if position.is_none()
            && minute >= session_open
            && minute < session_close
            && a > 0.0
            && idx > p.swing_len.max(p.avg_body_len)
        {
            if let Some(q) = pending.clone() {
                if idx > q.created_idx + p.retrace_bars {
                    pending = None
                } else {
                    let touches = c.low <= q.zone_high && c.high >= q.zone_low;
                    if touches {
                        let entry = (q.zone_low + q.zone_high) / 2.0;
                        let raw_sl = if q.side > 0 {
                            q.sweep_extreme - a * p.sweep_buffer_atr
                        } else {
                            q.sweep_extreme + a * p.sweep_buffer_atr
                        };
                        let risk = if q.side > 0 {
                            entry - raw_sl
                        } else {
                            raw_sl - entry
                        };
                        let reward = if q.side > 0 {
                            q.target_liquidity - entry
                        } else {
                            entry - q.target_liquidity
                        };
                        if risk >= a * p.min_stop_atr
                            && risk <= a * p.max_stop_atr
                            && reward >= risk * p.min_rr
                        {
                            position =
                                Some((q.side, entry, raw_sl, q.target_liquidity, t.to_rfc3339()));
                            pending = None;
                        }
                    }
                }
            }
            if pending.is_none() && position.is_none() {
                if let Some(sw) = sweep.clone() {
                    if idx > sw.created_idx + 3 {
                        sweep = None;
                    } else {
                        let bull_disp = sw.side > 0
                            && c.close > c.open
                            && body >= avg_body * p.displacement_body_mult
                            && body >= a * p.displacement_atr_mult
                            && c.close > internal_hi
                            && c.close >= vwap;
                        let bear_disp = sw.side < 0
                            && c.close < c.open
                            && body >= avg_body * p.displacement_body_mult
                            && body >= a * p.displacement_atr_mult
                            && c.close < internal_lo
                            && c.close <= vwap;
                        if bull_disp || bear_disp {
                            let mid = (c.open + c.close) / 2.0;
                            let (zone_low, zone_high) = if sw.side > 0 {
                                (c.open.min(vwap).min(mid), c.open.min(vwap).max(mid))
                            } else {
                                (mid.min(c.open.max(vwap)), mid.max(c.open.max(vwap)))
                            };
                            pending = Some(Pending {
                                side: sw.side,
                                sweep_extreme: sw.extreme,
                                created_idx: idx,
                                zone_low,
                                zone_high,
                                target_liquidity: sw.opposing_liquidity,
                            });
                            sweep = None;
                        }
                    }
                }
                if sweep.is_none() && pending.is_none() {
                    let swept_pdl =
                        prev_day_lo.is_some_and(|level| c.low < level && c.close > level);
                    let swept_swing_low =
                        prior_lo.is_finite() && c.low < prior_lo && c.close > prior_lo;
                    let swept_pdh =
                        prev_day_hi.is_some_and(|level| c.high > level && c.close < level);
                    let swept_swing_high =
                        prior_hi.is_finite() && c.high > prior_hi && c.close < prior_hi;

                    let long_target = [prev_day_hi, prior_hi.is_finite().then_some(prior_hi)]
                        .into_iter()
                        .flatten()
                        .filter(|level| *level > c.close)
                        .min_by(|a, b| a.total_cmp(b));
                    let short_target = [prev_day_lo, prior_lo.is_finite().then_some(prior_lo)]
                        .into_iter()
                        .flatten()
                        .filter(|level| *level < c.close)
                        .max_by(|a, b| a.total_cmp(b));

                    if swept_pdl || swept_swing_low {
                        if let Some(target) = long_target {
                            sweep = Some(Sweep {
                                side: 1,
                                extreme: c.low,
                                created_idx: idx,
                                opposing_liquidity: target,
                            });
                        }
                    } else if (swept_pdh || swept_swing_high)
                        && let Some(target) = short_target
                    {
                        sweep = Some(Sweep {
                            side: -1,
                            extreme: c.high,
                            created_idx: idx,
                            opposing_liquidity: target,
                        });
                    }
                }
            }
        }
        day_hi = day_hi.max(c.high);
        day_lo = day_lo.min(c.low);
        highs.push_back(c.high);
        lows.push_back(c.low);
        closes.push_back(c.close);
        if highs.len() > 100 {
            highs.pop_front();
            lows.pop_front();
            closes.pop_front();
        }
    }
    let scored: Vec<_> = trades
        .iter()
        .filter(|x| {
            x.exit_time.as_str() >= "2026-08-17" && x.exit_time.as_str() <= "2026-10-07T23:59:59"
        })
        .cloned()
        .collect();
    let wins = scored.iter().filter(|x| x.points > 0.0).count();
    let losses = scored.iter().filter(|x| x.points < 0.0).count();
    let gross = scored.iter().map(|x| x.points).sum::<f64>();
    let gp = scored
        .iter()
        .filter(|x| x.points > 0.0)
        .map(|x| x.points)
        .sum::<f64>();
    let gl = -scored
        .iter()
        .filter(|x| x.points < 0.0)
        .map(|x| x.points)
        .sum::<f64>();
    let mut daily: BTreeMap<String, f64> = BTreeMap::new();
    let mut charges = 0.0;
    for tr in &scored {
        let entry_buy = tr.side == "LONG";
        let exit_buy = tr.side == "SHORT";
        charges += if is_crude {
            crude_leg_charges(tr.entry, entry_buy) + crude_leg_charges(tr.exit, exit_buy)
        } else {
            leg_charges(tr.entry, entry_buy, lot) + leg_charges(tr.exit, exit_buy, lot)
        };
        *daily.entry(tr.exit_time[..10].to_string()).or_default() += tr.points * lot as f64;
    }
    let mut cum: f64 = 0.0;
    let mut peak: f64 = 0.0;
    let mut maxdd: f64 = 0.0;
    let mut posdays = 0;
    let mut negdays = 0;
    // allocate total charges pro-rata by trade count per day approximately through average trade charge
    let avg_charge = if scored.is_empty() {
        0.0
    } else {
        charges / scored.len() as f64
    };
    for (d, v) in &daily {
        let n = scored.iter().filter(|x| x.exit_time.starts_with(d)).count();
        let net = *v - avg_charge * n as f64;
        cum += net;
        peak = peak.max(cum);
        maxdd = maxdd.max(peak - cum);
        if net > 0.0 {
            posdays += 1
        } else if net < 0.0 {
            negdays += 1
        }
    }
    let gross_inr = gross * lot as f64;
    let net = gross_inr - charges;
    Ok((
        Summary {
            instrument: name.into(),
            timeframe_minutes: minutes,
            bars: bars.len(),
            trades: scored.len(),
            wins,
            losses,
            gross_points: gross,
            average_points: if scored.is_empty() {
                0.0
            } else {
                gross / scored.len() as f64
            },
            profit_factor: if gl > 0.0 { gp / gl } else { 0.0 },
            gross_inr,
            estimated_charges_inr: charges,
            net_inr: net,
            max_drawdown_inr: maxdd,
            positive_days: posdays,
            negative_days: negdays,
        },
        scored,
    ))
}

#[allow(clippy::too_many_arguments)]
pub fn run_date(
    name: &str,
    token: u32,
    lot: u32,
    is_crude: bool,
    session_open: u32,
    session_close: u32,
    date: &str,
) -> Result<()> {
    run_date_with_params(
        name,
        token,
        lot,
        is_crude,
        session_open,
        session_close,
        date,
        Params::default(),
    )
}

pub fn evaluate_config_candles(
    selection: &super::ilrc_config::Selection,
    candles: &[Candle],
    date: &str,
) -> Result<Vec<Trade>> {
    let (_summary, trades) = simulate(
        &selection.symbol,
        candles,
        3,
        selection.session_open_minute,
        selection.entry_cutoff_minute,
        100,
        true,
        selection.ilrc,
    )?;
    Ok(trades
        .into_iter()
        .filter(|t| t.exit_time.starts_with(date))
        .collect())
}

pub fn run_config_date(config: &str, date: &str) -> Result<()> {
    let selection = super::ilrc_config::Selection::load(config)?;
    run_date_with_params(
        &selection.symbol,
        selection.instrument_token,
        100,
        true,
        selection.session_open_minute,
        selection.entry_cutoff_minute,
        date,
        selection.ilrc,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_date_with_params(
    name: &str,
    token: u32,
    lot: u32,
    is_crude: bool,
    session_open: u32,
    session_close: u32,
    date: &str,
    params: Params,
) -> Result<()> {
    let target = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")?;
    let end = target
        .succ_opt()
        .ok_or_else(|| anyhow::anyhow!("date overflow"))?;
    let rt = tokio::runtime::Runtime::new()?;
    let mut raw = rt.block_on(kite_adapter::http::historical::fetch_window_for(
        token,
        end,
        8,
        Interval::OneMinute,
    ))?;
    raw.sort_by_key(|c| c.timestamp.clone());
    raw.dedup_by(|a, b| a.timestamp == b.timestamp);
    let bars = aggregate(&raw, 3, session_open)?;
    let (summary, trades) = simulate(
        name,
        &bars,
        3,
        session_open,
        session_close,
        lot,
        is_crude,
        params,
    )?;
    let prefix = date.to_string();
    let selected: Vec<_> = trades
        .into_iter()
        .filter(|t| t.exit_time.starts_with(&prefix))
        .collect();

    let mut charges = 0.0;
    let mut gross_points = 0.0;
    for tr in &selected {
        gross_points += tr.points;
        let entry_buy = tr.side == "LONG";
        let exit_buy = tr.side == "SHORT";
        charges += if is_crude {
            crude_leg_charges(tr.entry, entry_buy) + crude_leg_charges(tr.exit, exit_buy)
        } else {
            leg_charges(tr.entry, entry_buy, lot) + leg_charges(tr.exit, exit_buy, lot)
        };
    }
    let gross_inr = gross_points * lot as f64;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "strategy":"ILRC_v1",
            "date":date,
            "instrument":name,
            "timeframe_minutes":3,
            "trades":selected,
            "closed_trades":selected.len(),
            "wins":selected.iter().filter(|t|t.points>0.0).count(),
            "losses":selected.iter().filter(|t|t.points<0.0).count(),
            "gross_points":gross_points,
            "gross_inr":gross_inr,
            "estimated_charges_inr":charges,
            "net_inr":gross_inr-charges,
            "research_summary_context":summary
        }))?
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn run_research(
    name: &str,
    token: u32,
    lot: u32,
    is_crude: bool,
    session_open: u32,
    session_close: u32,
) -> Result<()> {
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
    let mut results = Vec::new();
    for m in [1_u64, 3, 5] {
        let bars = aggregate(&raw, m, session_open)?;
        let (summary, _) = simulate(
            name,
            &bars,
            m,
            session_open,
            session_close,
            lot,
            is_crude,
            Params::default(),
        )?;
        results.push(summary);
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "strategy":"ILRC_v1",
            "period":"2026-08-17 through 2026-10-07",
            "results":results
        }))?
    );
    Ok(())
}
