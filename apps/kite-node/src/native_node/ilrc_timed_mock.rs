//! Post-close market-at-next-open simulation; intentionally no broker or execution client.
use super::ilrc_backtest::EntryEvent;
use anyhow::{Result, ensure};
use chrono::DateTime;
use kite_adapter::http::historical::Candle;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
struct Stats {
    candidates: usize,
    filled: usize,
    blocked: usize,
    unfilled: usize,
    stopped: usize,
    targeted: usize,
    breakeven: usize,
    gross_points: f64,
    slippage_points: f64,
    rejected_by_kill_switch: usize,
    completed_orders: usize,
}
#[derive(Clone, Serialize, Deserialize)]
struct Position {
    long: bool,
    entry: f64,
    stop: f64,
    target: f64,
    risk: f64,
    be: bool,
    #[allow(dead_code)]
    id: String,
}
fn timestamp(s: &str) -> Result<i64> {
    Ok(DateTime::parse_from_rfc3339(s)?.timestamp())
}
#[derive(Clone, Serialize, Deserialize)]
struct ReplayState {
    stats: Stats,
    active: Option<Position>,
    next: usize,
    seen: BTreeSet<String>,
    last_bar: Option<i64>,
    stopped: bool,
}
impl ReplayState {
    fn new(candidates: usize) -> Self {
        Self {
            stats: Stats {
                candidates,
                ..Stats::default()
            },
            active: None,
            next: 0,
            seen: BTreeSet::new(),
            last_bar: None,
            stopped: false,
        }
    }
}
fn event_id(e: &EntryEvent) -> String {
    format!("{}:{}:{}:{}", e.setup, e.entry_time, e.side, e.entry)
}
fn replay_with_state(
    bars: &[Candle],
    events: &[EntryEvent],
    state: &mut ReplayState,
    slippage: f64,
    kill_at: Option<i64>,
) -> Result<()> {
    ensure!(
        slippage.is_finite() && (0.0..=50.0).contains(&slippage),
        "invalid slippage"
    );
    let mut sorted = events.to_vec();
    sorted.sort_by(|a, b| {
        timestamp(&a.observed_at)
            .unwrap_or(0)
            .cmp(&timestamp(&b.observed_at).unwrap_or(0))
            .then(a.setup.cmp(b.setup))
    });
    ensure!(
        state.stats.candidates == events.len(),
        "event count changed across restart"
    );
    for bar in bars {
        let now = bar.time()?.timestamp();
        if state.last_bar.is_some_and(|last| now <= last) {
            continue;
        }
        if kill_at.is_some_and(|stop| now >= stop) {
            state.stopped = true;
        }
        // Existing positions are managed before fresh entries. Stop wins OHLC ambiguity.
        if let Some(mut p) = state.active.take() {
            let stop = if p.long {
                bar.low <= p.stop
            } else {
                bar.high >= p.stop
            };
            let target = if p.long {
                bar.high >= p.target
            } else {
                bar.low <= p.target
            };
            if stop || target {
                // Stop gaps and adverse exit slippage are charged against the position.
                let stop_base = if p.long {
                    p.stop.min(bar.open)
                } else {
                    p.stop.max(bar.open)
                };
                let base = if stop { stop_base } else { p.target };
                let at = base + if p.long { -slippage } else { slippage };
                state.stats.slippage_points += slippage;
                state.stats.completed_orders += 1;
                state.stats.gross_points += if p.long { at - p.entry } else { p.entry - at };
                if stop {
                    if p.be {
                        state.stats.breakeven += 1;
                    } else {
                        state.stats.stopped += 1;
                    }
                } else {
                    state.stats.targeted += 1;
                }
            } else {
                let one_r = if p.long {
                    bar.high >= p.entry + p.risk
                } else {
                    bar.low <= p.entry - p.risk
                };
                if one_r {
                    p.stop = p.entry;
                    p.be = true;
                }
                state.active = Some(p);
            }
        }
        let mut grouped = Vec::new();
        while state.next < sorted.len() && timestamp(&sorted[state.next].observed_at)? <= now {
            grouped.push(&sorted[state.next]);
            state.next += 1;
        }
        if grouped.is_empty() {
            state.last_bar = Some(now);
            continue;
        }
        // One attempt per signal, evaluated strictly at the next available bar open.
        // No retroactive filling at the historical intrabar touched price.
        for e in grouped {
            let id = event_id(e);
            if !state.seen.insert(id.clone()) {
                state.stats.blocked += 1;
                continue;
            }
            if state.stopped {
                state.stats.rejected_by_kill_switch += 1;
                continue;
            }
            if state.active.is_some() {
                state.stats.blocked += 1;
                continue;
            }
            let long = e.side == "LONG";
            ensure!(long || e.side == "SHORT", "invalid side");
            let entry = bar.open + if long { slippage } else { -slippage };
            let risk = if long { entry - e.stop } else { e.stop - entry };
            let reward = if long {
                e.target - entry
            } else {
                entry - e.target
            };
            if !entry.is_finite() || !risk.is_finite() || risk <= 0.0 || reward <= 0.0 {
                state.stats.unfilled += 1;
                continue;
            }
            state.stats.slippage_points += slippage;
            state.active = Some(Position {
                long,
                entry,
                stop: e.stop,
                target: e.target,
                risk,
                be: false,
                id,
            });
            state.stats.filled += 1;
            // No same-bar stop/target claim without intrabar ordering data.
        }
        state.last_bar = Some(now);
    }
    Ok(())
}
pub(super) fn research_session_net(
    bars: &[Candle],
    events: &[EntryEvent],
    slippage: f64,
    round_trip_cost_points: f64,
) -> Result<serde_json::Value> {
    ensure!(
        slippage.is_finite() && slippage >= 0.0 && slippage <= 50.0,
        "Invalid slippage"
    );
    ensure!(
        round_trip_cost_points.is_finite() && round_trip_cost_points >= 0.0,
        "Invalid cost"
    );
    let mut days = std::collections::BTreeMap::<String, Vec<Candle>>::new();
    for bar in bars {
        let t = bar.time()?;
        // Exit before the live 23:15 entry cutoff, using the final 3-minute candle close.
        if t.format("%H:%M").to_string().as_str() > "23:12" {
            continue;
        }
        days.entry(t.date_naive().to_string())
            .or_default()
            .push(bar.clone());
    }
    let mut total = Stats::default();
    let (mut net, mut peak, mut drawdown) = (0.0_f64, 0.0_f64, 0.0_f64);
    let mut daily = Vec::new();
    let mut eod_exits = 0usize;
    let mut eod_details = Vec::new();
    let mut eod_net_points = 0.0_f64;
    for (date, day_bars) in &days {
        let day_events: Vec<_> = events
            .iter()
            .filter(|e| e.observed_at.starts_with(date))
            .cloned()
            .collect();
        let mut state = ReplayState::new(day_events.len());
        replay_with_state(day_bars, &day_events, &mut state, slippage, None)?;
        state.stats.unfilled += day_events.len() - state.next;
        if let Some(open) = state.active.take() {
            let last = day_bars.last().expect("date has bars");
            let exit = last.close + if open.long { -slippage } else { slippage };
            state.stats.gross_points += if open.long {
                exit - open.entry
            } else {
                open.entry - exit
            };
            state.stats.slippage_points += slippage;
            state.stats.completed_orders += 1;
            let trade_points = if open.long {
                exit - open.entry
            } else {
                open.entry - exit
            };
            eod_net_points += trade_points - round_trip_cost_points;
            eod_details.push(serde_json::json!({"date":date,"signal_id":open.id,
                "entry":open.entry,"exit":exit,"last_candle":last.timestamp,
                "side":if open.long {"LONG"} else {"SHORT"},
                "net_points":trade_points-round_trip_cost_points}));
            eod_exits += 1;
        }
        let daily_net =
            state.stats.gross_points - round_trip_cost_points * state.stats.completed_orders as f64;
        net += daily_net;
        peak = peak.max(net);
        drawdown = drawdown.max(peak - net);
        daily.push(
            serde_json::json!({"date":date,"candidates":state.stats.candidates,
            "fills":state.stats.filled,"exits":state.stats.completed_orders,
            "net_points":daily_net}),
        );
        total.candidates += state.stats.candidates;
        total.filled += state.stats.filled;
        total.blocked += state.stats.blocked;
        total.unfilled += state.stats.unfilled;
        total.stopped += state.stats.stopped;
        total.targeted += state.stats.targeted;
        total.breakeven += state.stats.breakeven;
        total.completed_orders += state.stats.completed_orders;
        total.gross_points += state.stats.gross_points;
    }
    Ok(serde_json::json!({
        "sessions":days.len(),"candidates":total.candidates,"filled":total.filled,
        "completed":total.completed_orders,"blocked":total.blocked,"unfilled":total.unfilled,
        "stopped":total.stopped,"targeted":total.targeted,"breakeven":total.breakeven,
        "eod_exits":eod_exits,"eod_details":eod_details,
        "net_points_excluding_eod_trades":net-eod_net_points,
        "eod_trade_net_points":eod_net_points,
        "gross_points_after_slippage":total.gross_points,
        "cost_points":total.completed_orders as f64*round_trip_cost_points,
        "net_points":net,"daily_close_max_drawdown_points":drawdown,
        "assumed_slippage_per_side_points":slippage,
        "round_trip_cost_points":round_trip_cost_points,"daily":daily,
        "warning":"Research-only daily-flat next-bar mock; EOD exits at final historical bar close, not verified exchange fills. Intrabar sequencing and broker execution not modeled."
    }))
}

pub(super) fn research_summary(
    bars: &[Candle],
    events: &[EntryEvent],
    slippage: f64,
) -> Result<serde_json::Value> {
    let mut state = ReplayState::new(events.len());
    replay_with_state(bars, events, &mut state, slippage, None)?;
    state.stats.unfilled += events.len() - state.next;
    Ok(serde_json::json!({
        "candidates":state.stats.candidates,"filled":state.stats.filled,
        "completed":state.stats.completed_orders,"blocked":state.stats.blocked,
        "unfilled":state.stats.unfilled,"stopped":state.stats.stopped,
        "targeted":state.stats.targeted,"breakeven":state.stats.breakeven,
        "gross_points_after_slippage_before_fees":state.stats.gross_points,
        "unclosed_at_end":state.active.is_some(),"assumed_slippage_per_side_points":slippage,
        "warning":"Next-available-3m-bar-open mock, no actual broker fills, no same-bar stop, no session-end forced liquidation, no full charges."
    }))
}
fn replay(bars: &[Candle], events: &[EntryEvent]) -> Result<Stats> {
    let mut state = ReplayState::new(events.len());
    replay_with_state(bars, events, &mut state, 0.0, None)?;
    state.stats.unfilled += events.len() - state.next;
    Ok(state.stats)
}
pub fn run_scenario(
    config: &str,
    fixture: &str,
    date: &str,
    slippage_points: f64,
    kill_after_bars: usize,
) -> Result<()> {
    let selection = super::ilrc_config::Selection::load(config)?;
    let bars: Vec<Candle> = serde_json::from_slice(&std::fs::read(fixture)?)?;
    ensure!(bars.len() > 100, "too few bars");
    for w in bars.windows(2) {
        ensure!(w[0].time()? < w[1].time()?, "bars must be strictly ordered");
    }
    let mut events = super::ilrc_backtest::entry_events_config_candles(&selection, &bars)?;
    events.extend(super::ilrc_continuation_backtest::entry_events_candles(
        &bars,
        selection.continuation.target_r,
    )?);
    events.retain(|e| e.entry_time.starts_with(date));
    let kill_at = if kill_after_bars == 0 {
        None
    } else {
        Some(
            bars.get(kill_after_bars)
                .ok_or_else(|| anyhow::anyhow!("kill index outside fixture"))?
                .time()?
                .timestamp(),
        )
    };
    let mut state = ReplayState::new(events.len());
    let split = bars.len() / 2;
    replay_with_state(
        &bars[..split],
        &events,
        &mut state,
        slippage_points,
        kill_at,
    )?;
    // Simulate restart across a persisted serialized state checkpoint.
    let checkpoint = serde_json::to_vec(&state)?;
    let checkpoint_dir = std::path::Path::new("data/ilrc-test/checkpoints");
    std::fs::create_dir_all(checkpoint_dir)?;
    let checkpoint_path = checkpoint_dir.join(format!("mock-{}-{}.json", std::process::id(), date));
    let mut checkpoint_file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&checkpoint_path)?;
    use std::io::Write;
    checkpoint_file.write_all(&checkpoint)?;
    checkpoint_file.sync_all()?;
    drop(checkpoint_file);
    let mut restored: ReplayState = serde_json::from_slice(&std::fs::read(&checkpoint_path)?)?;
    std::fs::remove_file(&checkpoint_path)?;
    replay_with_state(
        &bars[split..],
        &events,
        &mut restored,
        slippage_points,
        kill_at,
    )?;
    restored.stats.unfilled += events.len() - restored.next;
    let mut uninterrupted = ReplayState::new(events.len());
    replay_with_state(&bars, &events, &mut uninterrupted, slippage_points, kill_at)?;
    uninterrupted.stats.unfilled += events.len() - uninterrupted.next;
    ensure!(
        serde_json::to_value(&restored.stats)? == serde_json::to_value(&uninterrupted.stats)?,
        "restart replay diverged"
    );
    ensure!(
        restored.active.as_ref().map(|p| p.id.as_str())
            == uninterrupted.active.as_ref().map(|p| p.id.as_str()),
        "restart active position mismatch"
    );
    println!(
        "{}",
        serde_json::json!({"event":"ilrc_timed_mock_scenario","date":date,"slippage_points_per_fill":slippage_points,"candidates":restored.stats.candidates,"fills":restored.stats.filled,"blocked":restored.stats.blocked,"unfilled":restored.stats.unfilled,"stops":restored.stats.stopped,"targets":restored.stats.targeted,"breakevens":restored.stats.breakeven,"gross_points":restored.stats.gross_points,"slippage_points_applied":restored.stats.slippage_points,"kill_switch_rejections":restored.stats.rejected_by_kill_switch,"restart_equivalence":true,"disk_checkpoint_roundtrip":true,"mock_only":true,"broker_orders_sent":false,"execution_client_loaded":false})
    );
    Ok(())
}
pub fn run(config: &str, fixture: &str, date: &str) -> Result<()> {
    let s = super::ilrc_config::Selection::load(config)?;
    let bars: Vec<Candle> = serde_json::from_slice(&std::fs::read(fixture)?)?;
    ensure!(bars.len() > 100, "too few bars");
    for w in bars.windows(2) {
        ensure!(w[0].time()? < w[1].time()?, "bars must be strictly ordered");
    }
    let mut entries = super::ilrc_backtest::entry_events_config_candles(&s, &bars)?;
    entries.extend(super::ilrc_continuation_backtest::entry_events_candles(
        &bars,
        s.continuation.target_r,
    )?);
    entries.retain(|e| e.entry_time.starts_with(date));
    let x = replay(&bars, &entries)?;
    println!(
        "{}",
        serde_json::json!({"event":"ilrc_timed_mock_replay","date":date,"candidate_entries":x.candidates,"fills_at_next_open":x.filled,"blocked":x.blocked,"unfilled":x.unfilled,"stop_exits":x.stopped,"target_exits":x.targeted,"breakeven_exits":x.breakeven,"gross_points_before_charges":x.gross_points,"execution_client_loaded":false,"broker_orders_sent":false,"real_orders_enabled":false,"limitations":"bar-open zero slippage; no same-fill-bar exits; not broker fill parity; no persisted order reconciliation"})
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn bar(t: &str, open: f64, high: f64, low: f64) -> Candle {
        Candle {
            timestamp: t.into(),
            open,
            high,
            low,
            close: open,
            volume: 100,
            oi: 0,
        }
    }
    fn e(setup: &'static str, observed: &str) -> EntryEvent {
        EntryEvent {
            setup,
            entry_time: "2026-10-07T09:00:00+05:30".into(),
            observed_at: observed.into(),
            side: "LONG",
            entry: 100.,
            stop: 95.,
            target: 115.,
        }
    }
    #[test]
    fn restart_preserves_idempotency_and_position() {
        let b = vec![
            bar("2026-10-07T09:03:00+05:30", 100., 102., 99.),
            bar("2026-10-07T09:06:00+05:30", 101., 116., 100.),
        ];
        let events = [e("A", "2026-10-07T09:03:00+05:30")];
        let mut state = ReplayState::new(1);
        replay_with_state(&b[..1], &events, &mut state, 0.5, None).unwrap();
        assert_eq!(state.stats.filled, 1);
        let checkpoint = serde_json::to_vec(&state).unwrap();
        let mut resumed: ReplayState = serde_json::from_slice(&checkpoint).unwrap();
        replay_with_state(&b, &events, &mut resumed, 0.5, None).unwrap();
        assert_eq!(resumed.stats.filled, 1);
        assert_eq!(resumed.stats.targeted, 1);
    }
    #[test]
    fn kill_switch_blocks_new_orders() {
        let b = vec![bar("2026-10-07T09:03:00+05:30", 100., 102., 99.)];
        let mut state = ReplayState::new(1);
        replay_with_state(
            &b,
            &[e("A", "2026-10-07T09:03:00+05:30")],
            &mut state,
            0.0,
            Some(b[0].time().unwrap().timestamp()),
        )
        .unwrap();
        assert_eq!(
            (state.stats.filled, state.stats.rejected_by_kill_switch),
            (0, 1)
        );
    }
    #[test]
    fn slippage_worsens_long_fill() {
        let b = vec![bar("2026-10-07T09:03:00+05:30", 100., 102., 99.)];
        let mut state = ReplayState::new(1);
        replay_with_state(
            &b,
            &[e("A", "2026-10-07T09:03:00+05:30")],
            &mut state,
            0.75,
            None,
        )
        .unwrap();
        assert!((state.active.unwrap().entry - 100.75).abs() < 1e-9);
    }
    #[test]
    fn next_bar_only_and_priority() {
        let b = vec![
            bar("2026-10-07T09:00:00+05:30", 100., 110., 90.),
            bar("2026-10-07T09:03:00+05:30", 101., 115., 100.),
            bar("2026-10-07T09:06:00+05:30", 101., 115., 99.),
        ];
        let x = replay(
            &b,
            &[
                e("B", "2026-10-07T09:03:00+05:30"),
                e("A", "2026-10-07T09:03:00+05:30"),
            ],
        )
        .unwrap();
        assert_eq!((x.filled, x.blocked, x.targeted), (1, 1, 1));
    }
    #[test]
    fn invalid_post_close_open_is_not_filled() {
        let b = vec![bar("2026-10-07T09:03:00+05:30", 94., 96., 90.)];
        let x = replay(&b, &[e("A", "2026-10-07T09:03:00+05:30")]).unwrap();
        assert_eq!((x.filled, x.unfilled), (0, 1));
    }
    #[test]
    fn stop_before_target_and_be_only_next_bar() {
        let b = vec![
            bar("2026-10-07T09:03:00+05:30", 100., 120., 90.),
            bar("2026-10-07T09:06:00+05:30", 100., 120., 94.),
        ];
        let x = replay(&b, &[e("A", "2026-10-07T09:03:00+05:30")]).unwrap();
        assert_eq!(x.stopped, 1);
        assert_eq!(x.targeted, 0);
    }
    #[test]
    fn session_research_rejects_invalid_costs_and_excludes_late_bars() {
        let bars = vec![
            bar("2026-10-07T23:12:00+05:30", 100., 101., 99.),
            bar("2026-10-07T23:15:00+05:30", 100., 101., 99.),
        ];
        assert!(research_session_net(&bars, &[], 0.5, -2.0).is_err());
        assert!(research_session_net(&bars, &[], f64::NAN, 2.0).is_err());
        let report = research_session_net(&bars, &[], 0.5, 2.0).unwrap();
        assert_eq!(report["sessions"], 1);
        assert_eq!(report["completed"], 0);
    }
}
