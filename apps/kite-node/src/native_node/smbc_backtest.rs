//! Deterministic bar replay for SMBC.
use super::smbc_strategy::{Engine, Observation, Settings};
use anyhow::{Context, Result, ensure};
use chrono::Datelike;
use kite_adapter::http::historical::Candle;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Event {
    pub timestamp: String,
    pub action: &'static str,
    pub reason: String,
    pub price: f64,
    pub position_before: i8,
    pub position_after: i8,
    pub trade_points: Option<f64>,
}
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub instrument: String,
    pub interval: String,
    pub bars: usize,
    pub events: Vec<Event>,
    pub closed_trades: usize,
    pub gross_points: f64,
    pub open_position: i8,
    pub open_entry_price: Option<f64>,
}

#[derive(Clone, Copy)]
struct Protection {
    entry: f64,
    sl: f64,
    tp: Option<f64>,
    risk: f64,
    be_done: bool,
    trail_on: bool,
    entry_idx: usize,
}
struct Position {
    side: i8,
    entry: Option<f64>,
    protection: Option<Protection>,
}
impl Position {
    fn new() -> Self {
        Self {
            side: 0,
            entry: None,
            protection: None,
        }
    }
}

fn protection(
    settings: &Settings,
    o: Observation,
    side: i8,
    entry: f64,
    idx: usize,
) -> Option<Protection> {
    let atr = o.atr?;
    let source = if side > 0 {
        match settings.stop_mode.as_str() {
            "ATR" => entry - atr * settings.sl_atr_mult,
            "Candle Wick" => o.wick_low.unwrap_or(entry - atr * settings.sl_atr_mult),
            _ => o.wick_low.unwrap_or(entry) - atr * settings.wick_atr_buffer,
        }
    } else {
        match settings.stop_mode.as_str() {
            "ATR" => entry + atr * settings.sl_atr_mult,
            "Candle Wick" => o.wick_high.unwrap_or(entry + atr * settings.sl_atr_mult),
            _ => o.wick_high.unwrap_or(entry) + atr * settings.wick_atr_buffer,
        }
    };
    let risk = settings.clamp_stop_distance(
        if side > 0 {
            entry - source
        } else {
            source - entry
        }
        .max(0.0),
    );
    if risk <= 0.0 {
        return None;
    }
    let sl = if side > 0 { entry - risk } else { entry + risk };
    let tp = match settings.target_mode.as_str() {
        "R:R" => Some(if side > 0 {
            entry + settings.cap_target_distance(risk * settings.reward_risk)
        } else {
            entry - settings.cap_target_distance(risk * settings.reward_risk)
        }),
        "ATR" => Some(if side > 0 {
            entry + settings.cap_target_distance(atr * settings.tp_atr_mult)
        } else {
            entry - settings.cap_target_distance(atr * settings.tp_atr_mult)
        }),
        _ => None,
    };
    Some(Protection {
        entry,
        sl,
        tp,
        risk,
        be_done: false,
        trail_on: false,
        entry_idx: idx,
    })
}
fn close(
    events: &mut Vec<Event>,
    p: &mut Position,
    ts: &str,
    price: f64,
    action: &'static str,
    reason: &str,
) -> f64 {
    let before = p.side;
    let entry = p.entry.unwrap();
    let pts = if before > 0 {
        price - entry
    } else {
        entry - price
    };
    events.push(Event {
        timestamp: ts.into(),
        action,
        reason: reason.into(),
        price,
        position_before: before,
        position_after: 0,
        trade_points: Some(pts),
    });
    p.side = 0;
    p.entry = None;
    p.protection = None;
    pts
}
#[allow(clippy::too_many_arguments)]
fn open(
    events: &mut Vec<Event>,
    p: &mut Position,
    settings: &Settings,
    o: Observation,
    idx: usize,
    ts: &str,
    side: i8,
    price: f64,
) {
    events.push(Event {
        timestamp: ts.into(),
        action: if side > 0 { "BUY" } else { "SHORT" },
        reason: "BREAKOUT".into(),
        price,
        position_before: 0,
        position_after: side,
        trade_points: None,
    });
    p.side = side;
    p.entry = Some(price);
    p.protection = protection(settings, o, side, price, idx);
}
fn simulate_with_cooldown(
    settings: Settings,
    calendar: super::session_calendar::Calendar,
    bar_ns: u64,
    instrument: &str,
    interval: &str,
    candles: &[Candle],
    cooldown_bars: usize,
) -> Result<Report> {
    ensure!(!candles.is_empty(), "historical replay has no candles");
    let mut engine = Engine::new(settings.clone(), calendar)?;
    let mut p = Position::new();
    let mut events = Vec::new();
    let mut gross = 0.0;
    let mut closed = 0;
    let mut next_entry_idx = 0usize;
    for (idx, c) in candles.iter().enumerate() {
        let open_time = c.time()?;
        let open_ns = u64::try_from(
            open_time
                .timestamp_nanos_opt()
                .context("timestamp overflow")?,
        )?;
        let close_ns = open_ns + bar_ns;
        let o = engine.update_confirmed(c.open, c.high, c.low, c.close, close_ns, bar_ns)?;
        let ts = open_time.to_rfc3339();
        let mut exited = false;
        if p.side != 0
            && let Some(pr) = p.protection
        {
            let stop = if p.side > 0 {
                c.low <= pr.sl
            } else {
                c.high >= pr.sl
            };
            let tp = if p.side > 0 {
                pr.tp.is_some_and(|v| c.high >= v)
            } else {
                pr.tp.is_some_and(|v| c.low <= v)
            };
            if stop || tp {
                let px = if stop { pr.sl } else { pr.tp.unwrap() };
                let reason = if stop {
                    if pr.be_done && (pr.sl - pr.entry).abs() < 1e-9 {
                        "BE"
                    } else if pr.trail_on {
                        "TSL"
                    } else {
                        "SL"
                    }
                } else {
                    "TP"
                };
                let action = if p.side > 0 { "SELL" } else { "COVER" };
                gross += close(&mut events, &mut p, &ts, px, action, reason);
                closed += 1;
                exited = true;
            }
        }
        let mut reverse_to = 0;
        if p.side != 0 && !exited {
            if o.eod_hit {
                let a = if p.side > 0 { "SELL" } else { "COVER" };
                gross += close(&mut events, &mut p, &ts, c.close, a, "EOD");
                closed += 1;
                next_entry_idx = idx.saturating_add(cooldown_bars);
            } else if p.side > 0 && o.bearish_breakout && settings.opposite_breakout != "Ignore" {
                reverse_to = if settings.opposite_breakout == "Reverse" && settings.enable_shorts {
                    -1
                } else {
                    0
                };
                gross += close(&mut events, &mut p, &ts, c.close, "SELL", "REV");
                closed += 1;
                next_entry_idx = idx.saturating_add(cooldown_bars);
            } else if p.side < 0 && o.bullish_breakout && settings.opposite_breakout != "Ignore" {
                reverse_to = if settings.opposite_breakout == "Reverse" && settings.enable_longs {
                    1
                } else {
                    0
                };
                gross += close(&mut events, &mut p, &ts, c.close, "COVER", "REV");
                closed += 1;
                next_entry_idx = idx.saturating_add(cooldown_bars);
            }
        }
        if p.side == 0 && o.can_enter && idx >= next_entry_idx {
            let side = if reverse_to != 0 {
                reverse_to
            } else if o.bullish_breakout && settings.enable_longs {
                1
            } else if o.bearish_breakout && settings.enable_shorts {
                -1
            } else {
                0
            };
            if side != 0 {
                open(&mut events, &mut p, &settings, o, idx, &ts, side, c.close);
            }
        }
        if p.side != 0
            && let Some(mut pr) = p.protection
        {
            if idx > pr.entry_idx && pr.risk > 0.0 {
                let mr = if p.side > 0 {
                    (c.close - pr.entry) / pr.risk
                } else {
                    (pr.entry - c.close) / pr.risk
                };
                if settings.breakeven_r > 0.0 && !pr.be_done && mr >= settings.breakeven_r {
                    pr.sl = if p.side > 0 {
                        pr.sl.max(pr.entry)
                    } else {
                        pr.sl.min(pr.entry)
                    };
                    pr.be_done = true;
                }
                if settings.trail_mode != "Off"
                    && mr >= settings.trail_activate_r
                    && let Some(atr) = o.atr
                {
                    let nt = if p.side > 0 {
                        if settings.trail_mode == "ATR Trail" {
                            c.close - atr * settings.trail_atr_mult
                        } else {
                            o.trail_low.unwrap_or(c.close) - atr * settings.wick_atr_buffer
                        }
                    } else if settings.trail_mode == "ATR Trail" {
                        c.close + atr * settings.trail_atr_mult
                    } else {
                        o.trail_high.unwrap_or(c.close) + atr * settings.wick_atr_buffer
                    };
                    if (p.side > 0 && nt > pr.sl) || (p.side < 0 && nt < pr.sl) {
                        pr.trail_on = true;
                    }
                    pr.sl = if p.side > 0 {
                        pr.sl.max(nt)
                    } else {
                        pr.sl.min(nt)
                    };
                }
            }
            p.protection = Some(pr);
        }
    }
    Ok(Report {
        instrument: instrument.into(),
        interval: interval.into(),
        bars: candles.len(),
        events,
        closed_trades: closed,
        gross_points: gross,
        open_position: p.side,
        open_entry_price: p.entry,
    })
}
pub fn simulate(
    settings: Settings,
    calendar: super::session_calendar::Calendar,
    bar_ns: u64,
    instrument: &str,
    interval: &str,
    candles: &[Candle],
) -> Result<Report> {
    let cooldown_bars = settings.entry_cooldown_bars;
    simulate_with_cooldown(
        settings,
        calendar,
        bar_ns,
        instrument,
        interval,
        candles,
        cooldown_bars,
    )
}

pub fn run_date(config: &str, date: &str) -> Result<()> {
    let s = super::production::Selection::load(config)?;
    let target = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")?;
    let fetch_to = target
        .checked_add_signed(chrono::Duration::days(1))
        .ok_or_else(|| anyhow::anyhow!("date overflow"))?;
    let candles = tokio::runtime::Runtime::new()?.block_on(
        kite_adapter::http::historical::fetch_window_for(
            s.instrument_token,
            fetch_to,
            7,
            s.interval,
        ),
    )?;
    let target_candles: Vec<&Candle> = candles
        .iter()
        .filter(|candle| candle.time().is_ok_and(|ts| ts.date_naive() == target))
        .collect();
    ensure!(
        !target_candles.is_empty(),
        "Kite returned no candles for {target}"
    );
    let first = target_candles.first().expect("non-empty").timestamp.clone();
    let last = target_candles.last().expect("non-empty").timestamp.clone();
    let report = simulate(
        s.smbc.clone(),
        s.session_calendar.clone(),
        s.bar_ns(),
        &s.instrument,
        s.interval_name(),
        &candles,
    )?;
    let prefix = target.format("%Y-%m-%d").to_string();
    let events: Vec<_> = report
        .events
        .iter()
        .filter(|event| event.timestamp.starts_with(&prefix))
        .cloned()
        .collect();
    let closed: Vec<_> = events
        .iter()
        .filter_map(|event| event.trade_points)
        .collect();
    let wins = closed.iter().filter(|points| **points > 0.0).count();
    let losses = closed.iter().filter(|points| **points < 0.0).count();
    let gross_points: f64 = closed.iter().sum();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "date": target,
            "warmup_first_candle": candles.first().map(|c| &c.timestamp),
            "target_first_candle": first,
            "target_last_candle": last,
            "warmup_and_test_candles": candles.len(),
            "target_candles": target_candles.len(),
            "events": events,
            "closed_trades": closed.len(),
            "winning_trades": wins,
            "losing_trades": losses,
            "gross_points": gross_points,
            "average_points_per_trade": if closed.is_empty() { 0.0 } else { gross_points / closed.len() as f64 }
        }))?
    );
    Ok(())
}

pub fn run_month(config: &str, month: &str) -> Result<()> {
    use std::collections::BTreeMap;
    let s = super::production::Selection::load(config)?;
    let first = chrono::NaiveDate::parse_from_str(&format!("{month}-01"), "%Y-%m-%d")?;
    let next_month = if first.month() == 12 {
        chrono::NaiveDate::from_ymd_opt(first.year() + 1, 1, 1).unwrap()
    } else {
        chrono::NaiveDate::from_ymd_opt(first.year(), first.month() + 1, 1).unwrap()
    };
    let split = first
        .checked_add_signed(chrono::Duration::days(15))
        .ok_or_else(|| anyhow::anyhow!("date overflow"))?;
    let warm_start = first
        .checked_sub_signed(chrono::Duration::days(7))
        .ok_or_else(|| anyhow::anyhow!("date overflow"))?;
    let days_a = (split - warm_start).num_days();
    let days_b = (next_month - split).num_days();
    let rt = tokio::runtime::Runtime::new()?;
    let mut candles = rt.block_on(kite_adapter::http::historical::fetch_window_for(
        s.instrument_token,
        split,
        days_a,
        s.interval,
    ))?;
    let second = rt.block_on(kite_adapter::http::historical::fetch_window_for(
        s.instrument_token,
        next_month,
        days_b,
        s.interval,
    ))?;
    candles.extend(second);
    candles.sort_by_key(|c| c.timestamp.clone());
    candles.dedup_by(|a, b| a.timestamp == b.timestamp);

    let report = simulate(
        s.smbc.clone(),
        s.session_calendar.clone(),
        s.bar_ns(),
        &s.instrument,
        s.interval_name(),
        &candles,
    )?;
    let prefix = format!("{month}-");
    let events: Vec<_> = report
        .events
        .iter()
        .filter(|e| e.timestamp.starts_with(&prefix))
        .cloned()
        .collect();

    let mut daily: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for event in &events {
        if let Some(points) = event.trade_points {
            daily
                .entry(event.timestamp[..10].to_string())
                .or_default()
                .push(points);
        }
    }
    let mut daily_rows = Vec::new();
    let mut total_points = 0.0;
    let mut total_closed = 0usize;
    let mut total_wins = 0usize;
    let mut total_losses = 0usize;
    let mut total_be = 0usize;
    for (date, pts) in daily {
        let gross: f64 = pts.iter().sum();
        let wins = pts.iter().filter(|p| **p > 0.0).count();
        let losses = pts.iter().filter(|p| **p < 0.0).count();
        let be = pts.iter().filter(|p| p.abs() < 1e-12).count();
        total_points += gross;
        total_closed += pts.len();
        total_wins += wins;
        total_losses += losses;
        total_be += be;
        daily_rows.push(serde_json::json!({
            "date": date, "trades": pts.len(), "wins": wins, "losses": losses,
            "breakeven": be, "gross_points": gross
        }));
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "month": month,
            "warmup_first_candle": candles.first().map(|c| &c.timestamp),
            "last_candle": candles.last().map(|c| &c.timestamp),
            "candles_loaded": candles.len(),
            "closed_trades": total_closed,
            "winning_trades": total_wins,
            "losing_trades": total_losses,
            "breakeven_trades": total_be,
            "gross_points": total_points,
            "average_points_per_trade": if total_closed == 0 { 0.0 } else { total_points / total_closed as f64 },
            "daily": daily_rows
        }))?
    );
    Ok(())
}

pub fn run_scan(config: &str) -> Result<()> {
    #[derive(Debug, Clone, Serialize)]
    struct Row {
        name: String,
        strong_closes_only: bool,
        box_detection_length: usize,
        min_stop_points: f64,
        max_stop_points: f64,
        cooldown_bars: usize,
        august_points: f64,
        august_trades: usize,
        september_points: f64,
        september_trades: usize,
        october_points: f64,
        october_trades: usize,
        combined_points: f64,
    }

    fn score(report: &Report, prefixes: &[&str]) -> (usize, f64) {
        let pts: Vec<f64> = report
            .events
            .iter()
            .filter(|e| prefixes.iter().any(|p| e.timestamp.starts_with(p)))
            .filter_map(|e| e.trade_points)
            .collect();
        (pts.len(), pts.iter().sum())
    }

    let selection = super::production::Selection::load(config)?;
    let rt = tokio::runtime::Runtime::new()?;
    let mut scan_calendar = selection.session_calendar.clone();
    scan_calendar.valid_from = chrono::NaiveDate::from_ymd_opt(2026, 8, 10).unwrap();

    let aug_end = chrono::NaiveDate::from_ymd_opt(2026, 9, 1).unwrap();
    let mut august = rt.block_on(kite_adapter::http::historical::fetch_window_for(
        selection.instrument_token,
        aug_end,
        22,
        selection.interval,
    ))?;
    august.sort_by_key(|c| c.timestamp.clone());
    august.dedup_by(|a, b| a.timestamp == b.timestamp);

    let sep_split = chrono::NaiveDate::from_ymd_opt(2026, 9, 16).unwrap();
    let sep_warm = chrono::NaiveDate::from_ymd_opt(2026, 8, 25).unwrap();
    let oct1 = chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
    let mut september = rt.block_on(kite_adapter::http::historical::fetch_window_for(
        selection.instrument_token,
        sep_split,
        (sep_split - sep_warm).num_days(),
        selection.interval,
    ))?;
    september.extend(
        rt.block_on(kite_adapter::http::historical::fetch_window_for(
            selection.instrument_token,
            oct1,
            (oct1 - sep_split).num_days(),
            selection.interval,
        ))?,
    );
    september.sort_by_key(|c| c.timestamp.clone());
    september.dedup_by(|a, b| a.timestamp == b.timestamp);

    let oct_end = chrono::NaiveDate::from_ymd_opt(2026, 10, 8).unwrap();
    let mut october = rt.block_on(kite_adapter::http::historical::fetch_window_for(
        selection.instrument_token,
        oct_end,
        8,
        selection.interval,
    ))?;
    october.sort_by_key(|c| c.timestamp.clone());
    october.dedup_by(|a, b| a.timestamp == b.timestamp);

    let mut rows = Vec::new();
    for strong in [true, false] {
        for box_len in [10usize, 14, 18, 22] {
            for (min_sl, max_sl) in [(4.0, 12.0), (4.0, 15.0), (6.0, 15.0), (8.0, 18.0)] {
                for cooldown in [0usize, 5, 15] {
                    let mut settings = selection.smbc.clone();
                    settings.strong_closes_only = strong;
                    settings.box_detection_length = box_len;
                    settings.min_stop_points = min_sl;
                    settings.max_stop_points = max_sl;
                    settings.entry_cooldown_bars = cooldown;
                    settings.validate()?;
                    let sep = simulate_with_cooldown(
                        settings.clone(),
                        scan_calendar.clone(),
                        selection.bar_ns(),
                        &selection.instrument,
                        selection.interval_name(),
                        &september,
                        cooldown,
                    )?;
                    let aug = simulate_with_cooldown(
                        settings.clone(),
                        scan_calendar.clone(),
                        selection.bar_ns(),
                        &selection.instrument,
                        selection.interval_name(),
                        &august,
                        cooldown,
                    )?;
                    let oct = simulate_with_cooldown(
                        settings,
                        scan_calendar.clone(),
                        selection.bar_ns(),
                        &selection.instrument,
                        selection.interval_name(),
                        &october,
                        cooldown,
                    )?;
                    let (aug_trades, aug_points) = score(
                        &aug,
                        &[
                            "2026-08-17",
                            "2026-08-18",
                            "2026-08-19",
                            "2026-08-20",
                            "2026-08-21",
                            "2026-08-24",
                            "2026-08-25",
                            "2026-08-26",
                            "2026-08-27",
                            "2026-08-28",
                            "2026-08-31",
                        ],
                    );
                    let (sep_trades, sep_points) = score(&sep, &["2026-09-"]);
                    let (oct_trades, oct_points) =
                        score(&oct, &["2026-10-05", "2026-10-06", "2026-10-07"]);
                    rows.push(Row {
                        name: format!(
                            "strong={strong},box={box_len},sl={min_sl:.0}-{max_sl:.0},cooldown={cooldown}"
                        ),
                        strong_closes_only: strong,
                        box_detection_length: box_len,
                        min_stop_points: min_sl,
                        max_stop_points: max_sl,
                        cooldown_bars: cooldown,
                        august_points: aug_points,
                        august_trades: aug_trades,
                        september_points: sep_points,
                        september_trades: sep_trades,
                        october_points: oct_points,
                        october_trades: oct_trades,
                        combined_points: aug_points + sep_points + oct_points,
                    });
                }
            }
        }
    }
    rows.sort_by(|a, b| b.combined_points.total_cmp(&a.combined_points));
    let baseline = rows
        .iter()
        .find(|r| {
            r.strong_closes_only
                && r.box_detection_length == 14
                && r.min_stop_points == 4.0
                && r.max_stop_points == 15.0
                && r.cooldown_bars == 0
        })
        .cloned();
    let top: Vec<_> = rows.iter().take(15).cloned().collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "baseline": baseline,
            "top_15_by_combined_points": top,
            "candidates_tested": rows.len(),
            "selection_rule": "ranked by Aug 17-31 + September + Oct 5-7 gross points; August and October are holdouts"
        }))?
    );
    Ok(())
}

pub fn run_equity(config: &str) -> Result<()> {
    use std::collections::BTreeMap;

    #[derive(Debug, Clone, Serialize)]
    struct Day {
        date: String,
        trades: usize,
        gross_points: f64,
        gross_inr: f64,
        charges_inr: f64,
        net_inr: f64,
        net_after_one_tick_each_leg_inr: f64,
        cumulative_net_inr: f64,
        cumulative_conservative_inr: f64,
        drawdown_inr: f64,
        conservative_drawdown_inr: f64,
    }

    fn leg_charges(price: f64, is_buy: bool) -> f64 {
        let notional = price * 100.0;
        let brokerage = (notional * 0.0003).min(20.0);
        let transaction = notional * 0.000021;
        let sebi = notional * 0.000001;
        let ctt = if is_buy { 0.0 } else { notional * 0.0001 };
        let stamp = if is_buy { notional * 0.00002 } else { 0.0 };
        let gst = (brokerage + transaction + sebi) * 0.18;
        brokerage + transaction + sebi + ctt + stamp + gst
    }

    let selection = super::production::Selection::load(config)?;
    let mut calendar = selection.session_calendar.clone();
    calendar.valid_from = chrono::NaiveDate::from_ymd_opt(2026, 8, 10).unwrap();

    let rt = tokio::runtime::Runtime::new()?;
    let split = chrono::NaiveDate::from_ymd_opt(2026, 9, 8).unwrap();
    let end = chrono::NaiveDate::from_ymd_opt(2026, 10, 8).unwrap();
    let mut candles = rt.block_on(kite_adapter::http::historical::fetch_window_for(
        selection.instrument_token,
        split,
        29,
        selection.interval,
    ))?;
    candles.extend(
        rt.block_on(kite_adapter::http::historical::fetch_window_for(
            selection.instrument_token,
            end,
            30,
            selection.interval,
        ))?,
    );
    candles.sort_by_key(|c| c.timestamp.clone());
    candles.dedup_by(|a, b| a.timestamp == b.timestamp);

    let report = simulate(
        selection.smbc.clone(),
        calendar,
        selection.bar_ns(),
        &selection.instrument,
        selection.interval_name(),
        &candles,
    )?;

    let first_scored = "2026-08-17";
    let last_scored = "2026-10-07";
    let mut open: Option<&Event> = None;
    let mut daily: BTreeMap<String, (usize, f64, f64, f64)> = BTreeMap::new();
    for event in &report.events {
        if event.timestamp.as_str() < first_scored
            || event.timestamp.as_str() > "2026-10-07T23:59:59"
        {
            continue;
        }
        if event.trade_points.is_none() {
            open = Some(event);
            continue;
        }
        let Some(entry) = open.take() else { continue };
        let date = event.timestamp[..10].to_string();
        let points = event.trade_points.unwrap_or(0.0);
        let gross_inr = points * 100.0;
        let entry_buy = entry.action == "BUY";
        let exit_buy = event.action == "COVER";
        let charges = leg_charges(entry.price, entry_buy) + leg_charges(event.price, exit_buy);
        // One MCX tick is ₹1 in price, and the contract multiplier is 100.
        // One adverse tick on entry plus one on exit = ₹200 per round trip.
        let conservative = gross_inr - charges - 200.0;
        let row = daily.entry(date).or_insert((0, 0.0, 0.0, 0.0));
        row.0 += 1;
        row.1 += points;
        row.2 += charges;
        row.3 += conservative;
    }

    let mut rows = Vec::new();
    let mut cum_net: f64 = 0.0;
    let mut cum_cons: f64 = 0.0;
    let mut peak_net: f64 = 0.0;
    let mut peak_cons: f64 = 0.0;
    let mut max_dd: f64 = 0.0;
    let mut max_cons_dd: f64 = 0.0;
    let mut total_gross_points = 0.0;
    let mut total_charges = 0.0;
    let mut total_trades = 0usize;
    let mut positive_net_days = 0usize;
    let mut negative_net_days = 0usize;

    for (date, (trades, gross_points, charges, conservative_net)) in daily {
        let gross_inr = gross_points * 100.0;
        let net = gross_inr - charges;
        cum_net += net;
        cum_cons += conservative_net;
        peak_net = peak_net.max(cum_net);
        peak_cons = peak_cons.max(cum_cons);
        let dd = peak_net - cum_net;
        let cons_dd = peak_cons - cum_cons;
        max_dd = max_dd.max(dd);
        max_cons_dd = max_cons_dd.max(cons_dd);
        total_gross_points += gross_points;
        total_charges += charges;
        total_trades += trades;
        if net > 0.0 {
            positive_net_days += 1;
        }
        if net < 0.0 {
            negative_net_days += 1;
        }
        rows.push(Day {
            date,
            trades,
            gross_points,
            gross_inr,
            charges_inr: charges,
            net_inr: net,
            net_after_one_tick_each_leg_inr: conservative_net,
            cumulative_net_inr: cum_net,
            cumulative_conservative_inr: cum_cons,
            drawdown_inr: dd,
            conservative_drawdown_inr: cons_dd,
        });
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "period": format!("{first_scored} through {last_scored}"),
            "profile": {
                "strong_closes_only": selection.smbc.strong_closes_only,
                "box_detection_length": selection.smbc.box_detection_length,
                "min_stop_points": selection.smbc.min_stop_points,
                "max_stop_points": selection.smbc.max_stop_points,
                "entry_cooldown_bars": selection.smbc.entry_cooldown_bars
            },
            "contract_multiplier": 100,
            "cost_model": {
                "brokerage": "min(0.03% of turnover, INR 20) per executed order",
                "ctt": "0.01% sell side",
                "mcx_transaction": "0.0021% both sides",
                "sebi": "INR 10/crore both sides",
                "stamp": "0.002% buy side",
                "gst": "18% of brokerage + transaction + SEBI",
                "conservative_slippage": "1 price tick adverse on entry and exit = INR 200/round trip"
            },
            "trades": total_trades,
            "gross_points": total_gross_points,
            "gross_inr": total_gross_points * 100.0,
            "charges_inr": total_charges,
            "net_inr": cum_net,
            "net_points_after_charges": cum_net / 100.0,
            "net_after_one_tick_each_leg_inr": cum_cons,
            "net_points_after_charges_and_slippage": cum_cons / 100.0,
            "max_drawdown_inr": max_dd,
            "max_drawdown_points": max_dd / 100.0,
            "conservative_max_drawdown_inr": max_cons_dd,
            "conservative_max_drawdown_points": max_cons_dd / 100.0,
            "positive_net_days": positive_net_days,
            "negative_net_days": negative_net_days,
            "daily": rows
        }))?
    );
    Ok(())
}

pub fn run_timeframes(config: &str) -> Result<()> {
    #[derive(Debug, Clone, Serialize)]
    struct TfResult {
        timeframe_minutes: u64,
        bars: usize,
        trades: usize,
        gross_points: f64,
        gross_inr: f64,
        charges_inr: f64,
        net_inr: f64,
        net_points_after_charges: f64,
        conservative_net_inr: f64,
        conservative_net_points: f64,
        max_drawdown_inr: f64,
        conservative_max_drawdown_inr: f64,
        positive_net_days: usize,
        negative_net_days: usize,
    }

    fn leg_charges(price: f64, is_buy: bool) -> f64 {
        let notional = price * 100.0;
        let brokerage = (notional * 0.0003).min(20.0);
        let transaction = notional * 0.000021;
        let sebi = notional * 0.000001;
        let ctt = if is_buy { 0.0 } else { notional * 0.0001 };
        let stamp = if is_buy { notional * 0.00002 } else { 0.0 };
        let gst = (brokerage + transaction + sebi) * 0.18;
        brokerage + transaction + sebi + ctt + stamp + gst
    }

    fn aggregate(candles: &[Candle], minutes: u64) -> Result<Vec<Candle>> {
        use chrono::Timelike;
        let mut out = Vec::new();
        let mut bucket: Vec<&Candle> = Vec::new();
        let mut current_key: Option<(chrono::NaiveDate, u32)> = None;
        for c in candles {
            let t = c.time()?;
            let minute_of_day = t.hour() * 60 + t.minute();
            let key = (t.date_naive(), minute_of_day / minutes as u32);
            if current_key.is_some_and(|k| k != key) && !bucket.is_empty() {
                let first = bucket[0];
                let last = bucket[bucket.len() - 1];
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
            }
            current_key = Some(key);
            bucket.push(c);
        }
        if !bucket.is_empty() {
            let first = bucket[0];
            let last = bucket[bucket.len() - 1];
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
        }
        Ok(out)
    }

    fn evaluate(
        settings: Settings,
        calendar: super::session_calendar::Calendar,
        instrument: &str,
        candles: &[Candle],
        minutes: u64,
    ) -> Result<TfResult> {
        let bar_ns = minutes * 60_000_000_000;
        let report = simulate(
            settings,
            calendar,
            bar_ns,
            instrument,
            "aggregated",
            candles,
        )?;
        let mut open: Option<&Event> = None;
        let mut daily: std::collections::BTreeMap<String, (usize, f64, f64, f64)> =
            std::collections::BTreeMap::new();
        for event in &report.events {
            if event.timestamp.as_str() < "2026-08-17"
                || event.timestamp.as_str() > "2026-10-07T23:59:59"
            {
                continue;
            }
            if event.trade_points.is_none() {
                open = Some(event);
                continue;
            }
            let Some(entry) = open.take() else { continue };
            let points = event.trade_points.unwrap_or(0.0);
            let gross_inr = points * 100.0;
            let charges = leg_charges(entry.price, entry.action == "BUY")
                + leg_charges(event.price, event.action == "COVER");
            let conservative = gross_inr - charges - 200.0;
            let row = daily
                .entry(event.timestamp[..10].to_string())
                .or_insert((0, 0.0, 0.0, 0.0));
            row.0 += 1;
            row.1 += points;
            row.2 += charges;
            row.3 += conservative;
        }

        let mut trades = 0usize;
        let mut gross_points = 0.0;
        let mut charges = 0.0;
        let mut cum_net: f64 = 0.0;
        let mut cum_cons: f64 = 0.0;
        let mut peak_net: f64 = 0.0;
        let mut peak_cons: f64 = 0.0;
        let mut max_dd: f64 = 0.0;
        let mut max_cons_dd: f64 = 0.0;
        let mut positive = 0usize;
        let mut negative = 0usize;

        for (_, (n, pts, cost, cons)) in daily {
            trades += n;
            gross_points += pts;
            charges += cost;
            let net = pts * 100.0 - cost;
            cum_net += net;
            cum_cons += cons;
            peak_net = peak_net.max(cum_net);
            peak_cons = peak_cons.max(cum_cons);
            max_dd = max_dd.max(peak_net - cum_net);
            max_cons_dd = max_cons_dd.max(peak_cons - cum_cons);
            if net > 0.0 {
                positive += 1;
            }
            if net < 0.0 {
                negative += 1;
            }
        }
        Ok(TfResult {
            timeframe_minutes: minutes,
            bars: candles.len(),
            trades,
            gross_points,
            gross_inr: gross_points * 100.0,
            charges_inr: charges,
            net_inr: cum_net,
            net_points_after_charges: cum_net / 100.0,
            conservative_net_inr: cum_cons,
            conservative_net_points: cum_cons / 100.0,
            max_drawdown_inr: max_dd,
            conservative_max_drawdown_inr: max_cons_dd,
            positive_net_days: positive,
            negative_net_days: negative,
        })
    }

    let selection = super::production::Selection::load(config)?;
    let mut calendar = selection.session_calendar.clone();
    calendar.valid_from = chrono::NaiveDate::from_ymd_opt(2026, 8, 10).unwrap();
    let rt = tokio::runtime::Runtime::new()?;
    let split = chrono::NaiveDate::from_ymd_opt(2026, 9, 8).unwrap();
    let end = chrono::NaiveDate::from_ymd_opt(2026, 10, 8).unwrap();
    let mut one_minute = rt.block_on(kite_adapter::http::historical::fetch_window_for(
        selection.instrument_token,
        split,
        29,
        kite_adapter::http::historical::Interval::OneMinute,
    ))?;
    one_minute.extend(
        rt.block_on(kite_adapter::http::historical::fetch_window_for(
            selection.instrument_token,
            end,
            30,
            kite_adapter::http::historical::Interval::OneMinute,
        ))?,
    );
    one_minute.sort_by_key(|c| c.timestamp.clone());
    one_minute.dedup_by(|a, b| a.timestamp == b.timestamp);

    let mut results = Vec::new();
    for minutes in [2_u64, 3, 5] {
        let bars = aggregate(&one_minute, minutes)?;
        results.push(evaluate(
            selection.smbc.clone(),
            calendar.clone(),
            &selection.instrument,
            &bars,
            minutes,
        )?);
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "period": "2026-08-17 through 2026-10-07",
            "source": "Kite 1-minute candles aggregated locally",
            "profile": {
                "strong_closes_only": selection.smbc.strong_closes_only,
                "box_detection_length": selection.smbc.box_detection_length,
                "min_stop_points": selection.smbc.min_stop_points,
                "max_stop_points": selection.smbc.max_stop_points,
                "entry_cooldown_bars": selection.smbc.entry_cooldown_bars
            },
            "results": results
        }))?
    );
    Ok(())
}

pub fn run_index_timeframes(config: &str, name: &str, token: u32, lot_size: u32) -> Result<()> {
    #[derive(Debug, Clone, Serialize)]
    struct Row {
        timeframe_minutes: u64,
        bars: usize,
        trades: usize,
        wins: usize,
        losses: usize,
        breakeven: usize,
        gross_points: f64,
        gross_inr: f64,
        average_points_per_trade: f64,
    }

    fn aggregate(
        candles: &[Candle],
        minutes: u64,
        session_open_minute: u32,
    ) -> Result<Vec<Candle>> {
        use chrono::Timelike;
        let mut out = Vec::new();
        let mut bucket: Vec<&Candle> = Vec::new();
        let mut current_key: Option<(chrono::NaiveDate, u32)> = None;
        for c in candles {
            let t = c.time()?;
            let minute_of_day = t.hour() * 60 + t.minute();
            if minute_of_day < session_open_minute {
                continue;
            }
            let key = (
                t.date_naive(),
                (minute_of_day - session_open_minute) / minutes as u32,
            );
            if current_key.is_some_and(|k| k != key) && !bucket.is_empty() {
                let first = bucket[0];
                let last = bucket[bucket.len() - 1];
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
            }
            current_key = Some(key);
            bucket.push(c);
        }
        if !bucket.is_empty() {
            let first = bucket[0];
            let last = bucket[bucket.len() - 1];
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
        }
        Ok(out)
    }

    let selection = super::production::Selection::load(config)?;
    let rt = tokio::runtime::Runtime::new()?;
    let split = chrono::NaiveDate::from_ymd_opt(2026, 9, 8).unwrap();
    let end = chrono::NaiveDate::from_ymd_opt(2026, 10, 8).unwrap();
    let mut raw = rt.block_on(kite_adapter::http::historical::fetch_window_for(
        token,
        split,
        29,
        kite_adapter::http::historical::Interval::OneMinute,
    ))?;
    raw.extend(
        rt.block_on(kite_adapter::http::historical::fetch_window_for(
            token,
            end,
            30,
            kite_adapter::http::historical::Interval::OneMinute,
        ))?,
    );
    raw.sort_by_key(|c| c.timestamp.clone());
    raw.dedup_by(|a, b| a.timestamp == b.timestamp);

    let mut settings = selection.smbc.clone();
    settings.session.start = chrono::NaiveTime::from_hms_opt(9, 15, 0).unwrap();
    settings.session.end = chrono::NaiveTime::from_hms_opt(15, 30, 0).unwrap();
    settings.auto_sq_off_hour = 15;
    settings.auto_sq_off_minute = 30;

    let calendar: super::session_calendar::Calendar = serde_json::from_value(serde_json::json!({
        "timezone":"Asia/Kolkata",
        "valid_from":"2026-08-10",
        "valid_through":"2026-10-08",
        "regular":{"open":"09:15:00","close":"15:30:00"},
        "overrides":{}
    }))?;

    let mut results = Vec::new();
    for minutes in [1_u64, 2, 3, 4, 5] {
        let bars = if minutes == 1 {
            raw.clone()
        } else {
            aggregate(&raw, minutes, 9 * 60 + 15)?
        };
        let report = simulate(
            settings.clone(),
            calendar.clone(),
            minutes * 60_000_000_000,
            name,
            "aggregated",
            &bars,
        )?;
        let pts: Vec<f64> = report
            .events
            .iter()
            .filter(|e| {
                e.timestamp.as_str() >= "2026-08-17"
                    && e.timestamp.as_str() <= "2026-10-07T23:59:59"
            })
            .filter_map(|e| e.trade_points)
            .collect();
        let gross: f64 = pts.iter().sum();
        results.push(Row {
            timeframe_minutes: minutes,
            bars: bars.len(),
            trades: pts.len(),
            wins: pts.iter().filter(|p| **p > 0.0).count(),
            losses: pts.iter().filter(|p| **p < 0.0).count(),
            breakeven: pts.iter().filter(|p| p.abs() < 1e-12).count(),
            gross_points: gross,
            gross_inr: gross * lot_size as f64,
            average_points_per_trade: if pts.is_empty() {
                0.0
            } else {
                gross / pts.len() as f64
            },
        });
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "instrument":name,
            "instrument_token":token,
            "lot_size":lot_size,
            "period":"2026-08-17 through 2026-10-07",
            "session":"09:15-15:30 Asia/Kolkata",
            "source":"Kite 1-minute candles; 2/3/4/5m aggregated locally from 09:15",
            "profile":{
                "strong_closes_only":settings.strong_closes_only,
                "box_detection_length":settings.box_detection_length,
                "min_stop_points":settings.min_stop_points,
                "max_stop_points":settings.max_stop_points,
                "entry_cooldown_bars":settings.entry_cooldown_bars
            },
            "results":results
        }))?
    );
    Ok(())
}

pub fn run(config: &str, fixture: &str) -> Result<()> {
    #[derive(serde::Deserialize)]
    struct Fixture {
        instrument: Option<String>,
        candles: Vec<Candle>,
    }
    let s = super::production::Selection::load(config)?;
    let f: Fixture = serde_json::from_str(&std::fs::read_to_string(fixture)?)?;
    let instrument = f.instrument.as_deref().unwrap_or(&s.instrument);
    ensure!(instrument == s.instrument, "fixture instrument mismatch");
    let r = simulate(
        s.smbc.clone(),
        s.session_calendar.clone(),
        s.bar_ns(),
        &s.instrument,
        s.interval_name(),
        &f.candles,
    )?;
    println!("{}", serde_json::to_string_pretty(&r)?);
    Ok(())
}
