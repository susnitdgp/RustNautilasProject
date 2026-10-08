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
    pub gates_a: Vec<String>,
    pub gates_b: Vec<String>,
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
const COL: usize = 43;
const WIDTH: usize = COL * 2 + 7;
fn pair(out: &mut String, left: &str, right: &str) {
    out.push_str(&format!(
        "│ {:<COL$} │ {:<COL$} │\n",
        clip(left, COL),
        clip(right, COL)
    ));
}
fn whole(out: &mut String, message: &str) {
    out.push_str(&format!(
        "│ {:<w$} │\n",
        clip(message, WIDTH - 4),
        w = WIDTH - 4
    ));
}
fn border(out: &mut String, left: char, middle: char, right: char, fill: char) {
    out.push_str(&format!(
        "{left}{}{middle}{}{right}\n",
        fill.to_string().repeat(COL + 2),
        fill.to_string().repeat(COL + 2)
    ));
}
fn full_border(out: &mut String, left: char, right: char, fill: char) {
    out.push_str(&format!(
        "{left}{}{right}\n",
        fill.to_string().repeat(WIDTH - 2)
    ));
}
fn line(label: &str, value: impl std::fmt::Display) -> String {
    format!("{label:<11} {value}")
}
const TRADE_COLS: [usize; 8] = [12, 3, 5, 9, 9, 16, 8, 6];
fn trade_cells(out: &mut String, cells: [&str; 8]) {
    out.push('│');
    for (cell, width) in cells.iter().zip(TRADE_COLS) {
        out.push_str(&format!(" {:<width$} │", clip(cell, width)));
    }
    out.push('\n');
}
fn trade_separator(out: &mut String) {
    out.push('├');
    for (idx, width) in TRADE_COLS.iter().enumerate() {
        out.push_str(&"─".repeat(width + 2));
        out.push(if idx == TRADE_COLS.len() - 1 {
            '┤'
        } else {
            '┼'
        });
    }
    out.push('\n');
}
fn trade_line(out: &mut String, trade: &TradeRow) {
    let entry = format!("{:.2}", trade.entry);
    let exit = format!("{:.2}", trade.exit);
    let points = format!("{:+.2}", trade.points);
    let result = if trade.points > 0.0 {
        "WIN"
    } else if trade.points < 0.0 {
        "LOSS"
    } else {
        "FLAT"
    };
    trade_cells(
        out,
        [
            &trade.time,
            &trade.setup,
            &trade.side,
            &entry,
            &exit,
            &trade.reason,
            &points,
            result,
        ],
    );
}
pub fn render(state: &State, instrument: &str, mode: &str) {
    let mut out = String::new();
    let is_tty = io::stdout().is_terminal();
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
        Some(s) if s <= 240 => format!("OK · close {}s ago", s),
        Some(s) => format!("STALE · close {}s ago", s),
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
    } else if age.is_none_or(|s| s > 240) {
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
    } else if age.is_none_or(|s| s > 240) {
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
    } else if age.is_none_or(|s| s > 240) {
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
    border(&mut out, '┌', '┬', '┐', '─');
    pair(
        &mut out,
        "ILRC COMBINED | OPERATIONS",
        &format!("NAUTILUS / KITE | {mode}"),
    );
    border(&mut out, '├', '┼', '┤', '─');
    pair(&mut out, &line("STATUS", status), &line("FEED", &feed));
    pair(&mut out, &line("WHY", reason), &line("NEXT", next));
    pair(
        &mut out,
        &line("SETUP A", &state.trigger_a),
        &line("SETUP B", &state.trigger_b),
    );
    pair(
        &mut out,
        &line("Ticks", state.tick_count),
        &line(
            "Last tick",
            state.last_tick_epoch.map_or("waiting".into(), |ts| {
                format!("{}s ago", (now.timestamp() - ts).max(0))
            }),
        ),
    );
    border(&mut out, '├', '┼', '┤', '─');
    pair(
        &mut out,
        &line("Instrument", instrument),
        &line("Time IST", now.format("%d-%m-%Y %H:%M:%S")),
    );
    pair(
        &mut out,
        &line("Market", format!("{:.2}", state.last_price)),
        &line("Session", session),
    );
    pair(
        &mut out,
        &line("Bar start", &state.last_bar),
        &line("Data", format!("{} candles", state.bars)),
    );
    pair(
        &mut out,
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
            format!("{position} ({:.0})", state.position.abs()),
        ),
    );
    pair(
        &mut out,
        &line(
            "Target",
            state.target.map_or("n/a".into(), |v| format!("{v:.2}")),
        ),
        &line(
            "Stop-loss",
            state
                .stop
                .map_or("not confirmed".into(), |v| format!("{v:.2} tracked")),
        ),
    );
    pair(
        &mut out,
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
        &mut out,
        &line(
            "Realized",
            format!(
                "{:.2} gross points",
                if points == 0.0 { 0.0 } else { points }
            ),
        ),
        &line(
            "Today",
            format!("{} trades | {wins}W {losses}L", state.trades.len()),
        ),
    );
    full_border(&mut out, '├', '┤', '─');
    whole(
        &mut out,
        "ENTRY CONDITIONS | Latest completed 3-minute candle (diagnostic only)",
    );
    border(&mut out, '├', '┬', '┤', '─');
    pair(
        &mut out,
        "SETUP A | Liquidity reversal",
        "SETUP B | BOS continuation",
    );
    let n = state.gates_a.len().max(state.gates_b.len());
    for i in 0..n {
        pair(
            &mut out,
            state.gates_a.get(i).map_or("", String::as_str),
            state.gates_b.get(i).map_or("", String::as_str),
        );
    }
    border(&mut out, '├', '┴', '┤', '─');
    whole(
        &mut out,
        "WAIT = pending-state/confirmation not exposed; not an entry signal",
    );
    full_border(&mut out, '├', '┤', '─');
    whole(
        &mut out,
        "TRADE HISTORY | Current run (not broker account history)",
    );
    trade_separator(&mut out);
    trade_cells(
        &mut out,
        [
            "Time", "Set", "Side", "Entry", "Exit", "Reason", "Points", "Result",
        ],
    );
    trade_separator(&mut out);
    if state.trades.is_empty() {
        whole(&mut out, "No completed order-fill trades this run");
    }
    for trade in state.trades.iter().rev().take(10).rev() {
        trade_line(&mut out, trade);
    }
    if let Some(t) = &state.open_trade {
        let entry = format!("{:.2}", t.entry);
        trade_cells(
            &mut out,
            [
                &t.time,
                &t.setup,
                &t.side,
                &entry,
                "—",
                "Position open",
                "—",
                "OPEN",
            ],
        );
    }
    full_border(&mut out, '└', '┘', '─');
    out.push_str("Press Ctrl+C to quit the running bot.\n");
    let mut stdout = io::stdout().lock();
    if is_tty {
        let _ = stdout.write_all(b"\x1b[?25l\x1b[H\x1b[2J");
    }
    if is_tty {
        // Prevent a tall dashboard from scrolling its own header off a short terminal.
        // Reserve the bottom line for the Ctrl+C instruction.
        if let Ok((_cols, rows)) = crossterm::terminal::size() {
            let usable = rows.saturating_sub(1) as usize;
            let lines: Vec<&str> = out.lines().collect();
            if lines.len() > usable && usable >= 5 {
                let body = lines.iter().take(usable.saturating_sub(1));
                let clipped = body.copied().collect::<Vec<_>>().join("\n");
                let _ = stdout.write_all(clipped.as_bytes());
                let _ = stdout.write_all(b"\nPress Ctrl+C to quit the running bot.\n");
            } else {
                let _ = stdout.write_all(out.as_bytes());
            }
        } else {
            let _ = stdout.write_all(out.as_bytes());
        }
    } else {
        let _ = stdout.write_all(out.as_bytes());
    }
    let _ = stdout.flush();
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn trade_table_width_matches_dashboard() {
        let mut output = String::new();
        trade_separator(&mut output);
        trade_cells(
            &mut output,
            [
                "Time", "Set", "Side", "Entry", "Exit", "Reason", "Points", "Result",
            ],
        );
        for row in output.lines() {
            assert_eq!(row.chars().count(), WIDTH);
        }
    }
    #[test]
    fn initial_state_is_flat_and_empty() {
        let x = State::default();
        assert!(x.trades.is_empty() && x.open_trade.is_none() && x.stop.is_none());
    }
}
