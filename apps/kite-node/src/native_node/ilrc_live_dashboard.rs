//! Two-column read-only ILRC live terminal dashboard with broker-event trade history.
use std::{
    io::{self, IsTerminal, Write},
    sync::{Arc, Mutex},
};

#[derive(Clone, Debug)]
pub struct TradeRow {
    pub time: String,
    pub setup: String,
    pub side: String,
    pub entry: f64,
    pub exit: f64,
    pub reason: String,
    pub points: f64,
}
#[derive(Clone, Debug)]
pub struct OpenTrade {
    pub time: String,
    pub setup: String,
    pub side: String,
    pub entry: f64,
}
#[derive(Clone, Debug, Default)]
pub struct State {
    pub updates: Arc<tokio::sync::Notify>,
    pub tick_count: u64,
    pub last_tick_epoch: Option<i64>,
    pub trigger_a: String,
    pub trigger_b: String,
    pub bars: usize,
    pub last_bar_epoch: Option<i64>,
    pub last_bar: String,
    pub last_price: f64,
    pub position: f64,
    pub stop: Option<f64>,
    pub target: Option<f64>,
    pub setup: String,
    pub event: String,
    pub fault: Option<String>,
    pub open_trade: Option<OpenTrade>,
    pub trades: Vec<TradeRow>,
}
pub type Shared = Arc<Mutex<State>>;
pub fn shared() -> Shared {
    Arc::new(Mutex::new(State::default()))
}
fn clip(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        s.to_owned()
    } else {
        format!(
            "{}…",
            s.chars().take(width.saturating_sub(1)).collect::<String>()
        )
    }
}
fn pair(left: &str, right: &str) {
    let left = clip(left, 52);
    let right = clip(right, 52);
    println!("│ {left:<52} │ {right:<52} │");
}
fn line(label: &str, value: impl std::fmt::Display) -> String {
    format!("{label:<11} {value}")
}
fn trade_line(t: &TradeRow) {
    let result = if t.points > 0.0 {
        "WIN"
    } else if t.points < 0.0 {
        "LOSS"
    } else {
        "FLAT"
    };
    println!(
        "│ {:<14} │ {:<6} │ {:<6} │ {:>11.2} │ {:>11.2} │ {:<19} │ {:+10.2} │ {:<11} │",
        clip(&t.time, 14),
        clip(&t.setup, 6),
        clip(&t.side, 6),
        t.entry,
        t.exit,
        clip(&t.reason, 19),
        t.points,
        result
    );
}
pub fn render(state: &State, instrument: &str, mode: &str) {
    if io::stdout().is_terminal() {
        print!("\x1b[2J\x1b[H");
    }
    let now = chrono::Utc::now().with_timezone(&chrono::FixedOffset::east_opt(19800).expect("IST"));
    let minute = chrono::Timelike::hour(&now) * 60 + chrono::Timelike::minute(&now);
    let session = if minute < 540 {
        "PRE-OPEN · 09:00 IST"
    } else if minute >= 1395 {
        "ENTRY CLOSED · 23:15"
    } else {
        "OPEN · until 23:15 IST"
    };
    let age = state.last_bar_epoch.map(|ts| (now.timestamp() - ts).max(0));
    let feed = match age {
        Some(s) if s <= 360 => format!("OK · bar {}s old", s),
        Some(s) => format!("STALE · {}s old", s),
        None => "WAITING · no candle".into(),
    };
    let status = if state.fault.is_some() {
        "HALTED · MANUAL REVIEW"
    } else if state.open_trade.is_some() && state.stop.is_none() {
        "UNPROTECTED / STOP PENDING"
    } else if state.open_trade.is_some() {
        "IN POSITION · STOP TRACKED"
    } else if state.event == "Entry order submitted" {
        "ENTRY PENDING"
    } else if age.is_none_or(|s| s > 360) {
        "DATA NOT READY"
    } else if !(540..1395).contains(&minute) {
        "ENTRY WINDOW CLOSED"
    } else {
        "SCANNING · WAITING FOR A/B"
    };
    let reason = if state.fault.is_some() {
        "Execution halted: inspect alert and Kite"
    } else if state.open_trade.is_some() && state.stop.is_none() {
        "Fill observed: confirm protective SL-M"
    } else if state.open_trade.is_some() {
        "Position open: monitoring stop / target"
    } else if state.event == "Entry order submitted" {
        "Entry submitted: awaiting broker fill"
    } else if age.is_none_or(|s| s > 360) {
        "No fresh completed 3-minute candle"
    } else if !(540..1395).contains(&minute) {
        "New entries disabled by session clock"
    } else {
        "No qualifying Setup A or B signal yet"
    };
    let next = if state.fault.is_some() {
        "Check broker orders and positions"
    } else if state.open_trade.is_some() && state.stop.is_none() {
        "Await SL-M acceptance; verify in Kite"
    } else if state.open_trade.is_some() {
        "Watch target / BE / exit events"
    } else if state.event == "Entry order submitted" {
        "Await order accepted or fill event"
    } else if age.is_none_or(|s| s > 360) {
        "Wait for finalized candle; check feed"
    } else {
        "Evaluate next completed 3-minute bar"
    };
    let position = if state.position > 0.0 {
        "LONG"
    } else if state.position < 0.0 {
        "SHORT"
    } else {
        "FLAT"
    };
    let points: f64 = state.trades.iter().map(|t| t.points).sum();
    let wins = state.trades.iter().filter(|t| t.points > 0.0).count();
    let losses = state.trades.iter().filter(|t| t.points < 0.0).count();
    println!(
        "┌──────────────────────────────────────────────────────┬──────────────────────────────────────────────────────┐"
    );
    pair(
        "ILRC COMBINED · LIVE TRADING DASHBOARD",
        &format!("NAUTILUS / KITE · {mode}"),
    );
    println!(
        "├──────────────────────────────────────────────────────┼──────────────────────────────────────────────────────┤"
    );
    pair(&line("STATUS", status), &line("DATA FEED", &feed));
    pair(&line("WHY", reason), &line("NEXT", next));
    pair(
        &line("SETUP A", &state.trigger_a),
        &line("SETUP B", &state.trigger_b),
    );
    pair(
        &line("Ticks", state.tick_count),
        &line(
            "Last tick",
            state.last_tick_epoch.map_or("waiting".into(), |ts| {
                format!("{}s ago", (now.timestamp() - ts).max(0))
            }),
        ),
    );
    println!(
        "├──────────────────────────────────────────────────────┼──────────────────────────────────────────────────────┤"
    );
    pair(
        &line("Instrument", instrument),
        &line("Time IST", now.format("%d-%m-%Y %H:%M:%S")),
    );
    pair(
        &line(
            "Market",
            if state.bars > 0 {
                format!("{:.2}", state.last_price)
            } else {
                "waiting".into()
            },
        ),
        &line("Session", session),
    );
    pair(
        &line(
            "Last bar",
            if state.last_bar.is_empty() {
                "waiting"
            } else {
                &state.last_bar
            },
        ),
        &line("Data", format!("{} candles", state.bars)),
    );
    pair(
        &line(
            "Setup",
            if state.setup.is_empty() {
                "SCANNING"
            } else {
                &state.setup
            },
        ),
        &line(
            "Position",
            format!("{position} ({:.0} contract)", state.position.abs()),
        ),
    );
    pair(
        &line(
            "Target",
            state.target.map_or("n/a".into(), |v| format!("{v:.2}")),
        ),
        &line(
            "Stop-loss",
            state
                .stop
                .map_or("not confirmed".into(), |v| format!("{v:.2} (tracked)")),
        ),
    );
    pair(
        &line(
            "Last event",
            if state.event.is_empty() {
                "waiting"
            } else {
                &state.event
            },
        ),
        &line(
            "Risk",
            state.fault.as_deref().unwrap_or("No reported alert"),
        ),
    );
    pair(
        &line(
            "Realized",
            format!(
                "{:.2} gross points",
                if points == 0.0 { 0.0 } else { points }
            ),
        ),
        &line(
            "Today",
            format!("{} trades · {wins} W / {losses} L", state.trades.len()),
        ),
    );
    println!(
        "├──────────────────────────────────────────────────────┴──────────────────────────────────────────────────────┤"
    );
    println!(
        "│ CURRENT RUN · ORDER-FILL TRADE HISTORY (resets on restart)                                                   │"
    );
    println!(
        "├────────────────┬────────┬────────┬─────────────┬─────────────┬─────────────────────┬────────────┬─────────────┤"
    );
    println!(
        "│ Time           │ Setup  │ Side   │ Entry       │ Exit        │ Reason              │ Points     │ Result      │"
    );
    println!(
        "├────────────────┼────────┼────────┼─────────────┼─────────────┼─────────────────────┼────────────┼─────────────┤"
    );
    if state.trades.is_empty() {
        println!(
            "│ No completed broker-observed ILRC trades                                                                     │"
        );
    }
    for t in state.trades.iter().rev().take(10).rev() {
        trade_line(t);
    }
    if let Some(t) = &state.open_trade {
        println!(
            "│ OPEN {:<10} {:<6} {:<6} {:>11.2} {:>11} {:<19} {:>10} {:<11} │",
            clip(&t.time, 10),
            clip(&t.setup, 6),
            clip(&t.side, 6),
            t.entry,
            "—",
            "POSITION OPEN",
            "—",
            "OPEN"
        );
    }
    println!(
        "└─────────────────────────────────────────────────────────────────────────────────────────────────────────────┘"
    );
    println!(
        "  Gross price points exclude brokerage, taxes and slippage. Confirm stop and positions directly in Kite."
    );
    let _ = io::stdout().flush();
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn initial_state_is_flat_and_empty() {
        let x = State::default();
        assert!(x.trades.is_empty() && x.open_trade.is_none() && x.stop.is_none());
    }
}
