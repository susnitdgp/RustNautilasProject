use super::ilrc_backtest::Trade;
use chrono::{DateTime, FixedOffset, Timelike};

pub struct Snapshot<'a> {
    pub namespace: &'a str,
    pub strategy: &'a str,
    pub instrument: &'a str,
    pub interval: &'a str,
    pub now: DateTime<FixedOffset>,
    pub session_open_minute: u32,
    pub entry_cutoff_minute: u32,
    pub break_even_r: f64,
    pub continuation_target_r: f64,
    pub last_bar: Option<&'a kite_adapter::http::historical::Candle>,
    pub bars_loaded: usize,
    pub trades: &'a [Trade],
    pub report_directory: &'a std::path::Path,
}

fn clip(value: impl ToString, width: usize) -> String {
    let value = value.to_string();
    if value.chars().count() <= width {
        return value;
    }
    let mut out = value
        .chars()
        .take(width.saturating_sub(1))
        .collect::<String>();
    out.push('…');
    out
}

fn cell(label: &str, value: impl ToString, width: usize) -> String {
    let label_width = 10usize;
    let value_width = width.saturating_sub(label_width + 1);
    format!(
        "{:<label_width$} {:<value_width$}",
        label,
        clip(value, value_width),
        label_width = label_width,
        value_width = value_width
    )
}

const DASHBOARD_WIDTH: usize = 115;
const TRADE_WIDTHS: [usize; 7] = [8, 5, 10, 10, 8, 10, 42];

fn trade_row(values: [&str; 7]) -> String {
    let mut row = String::from("│");
    for (idx, (value, width)) in values.iter().zip(TRADE_WIDTHS).enumerate() {
        row.push(' ');
        row.push_str(&format!("{:<width$}", clip(value, width), width = width));
        row.push(' ');
        row.push('│');
        if idx + 1 == TRADE_WIDTHS.len() {
            break;
        }
    }
    debug_assert_eq!(row.chars().count(), DASHBOARD_WIDTH);
    row
}

fn full_row(value: impl ToString) -> String {
    let inner = DASHBOARD_WIDTH - 2;
    format!("│{:<inner$}│", clip(value, inner), inner = inner)
}

pub fn render(s: Snapshot<'_>) {
    use std::io::{self, Write};

    let minute = s.now.hour() * 60 + s.now.minute();
    let session = if minute < s.session_open_minute {
        "PRE-MARKET"
    } else if minute >= s.entry_cutoff_minute {
        "ENTRY CLOSED"
    } else {
        "ACTIVE"
    };

    let (last_bar, freshness, last_price) = match s.last_bar {
        Some(bar) => {
            let stamp = bar
                .time()
                .map(|x| x.format("%d-%m %H:%M IST").to_string())
                .unwrap_or_else(|_| bar.timestamp.clone());
            let age = bar
                .time()
                .ok()
                .map(|x| (s.now.timestamp() - x.timestamp()).max(0))
                .unwrap_or(0);
            (
                format!(
                    "{} O {:.0} H {:.0} L {:.0} C {:.0}",
                    stamp, bar.open, bar.high, bar.low, bar.close
                ),
                format!("{}s", age),
                format!("{:.0}", bar.close),
            )
        }
        None => ("none".into(), "n/a".into(), "n/a".into()),
    };

    let gross_points: f64 = s.trades.iter().map(|t| t.points).sum();
    let wins = s.trades.iter().filter(|t| t.points > 0.0).count();
    let losses = s.trades.iter().filter(|t| t.points < 0.0).count();
    let setup = s
        .trades
        .last()
        .map(|t| format!("{} {}→{} {:+.2}pt", t.side, t.entry, t.exit, t.points))
        .unwrap_or_else(|| "SCANNING · no completed setup".to_string());

    let left = [
        cell(
            "Instrument",
            format!("{} · {}", s.instrument, s.interval),
            55,
        ),
        cell("Last price", last_price, 55),
        cell("Last bar", last_bar, 55),
        cell("ILRC", setup, 55),
        cell(
            "Today",
            format!(
                "{} trades · {}W/{}L · {:+.2}pt",
                s.trades.len(),
                wins,
                losses,
                gross_points
            ),
            55,
        ),
    ];

    let right = [
        cell(
            "Session",
            format!("{} · {}", session, s.now.format("%H:%M:%S IST")),
            55,
        ),
        cell(
            "Feed",
            format!("{} bars · age {}", s.bars_loaded, freshness),
            55,
        ),
        cell("Orders", "DISABLED · no execution client", 55),
        cell(
            "Safety",
            format!(
                "OFF · A BE {:.1}R · B TP {:.0}R/BE {:.1}R",
                s.break_even_r, s.continuation_target_r, s.break_even_r
            ),
            55,
        ),
        cell("Reports", s.report_directory.display(), 55),
    ];

    let mut body = String::new();
    body.push_str("┌─────────────────────────────────────────────────────────────────────────────────────────────────────────────────┐\n");
    body.push_str(&full_row(" ILRC COMBINED · PRODUCTION SHADOW DASHBOARD"));
    body.push('\n');
    body.push_str("├───────────────────────────────────────────────────────┬─────────────────────────────────────────────────────────┤\n");
    body.push_str(&format!(
        "│ {:<53} │ {:<55} │\n",
        clip(format!("Run {}", s.namespace), 53),
        clip(s.strategy, 55)
    ));
    body.push_str("├───────────────────────────────────────────────────────┼─────────────────────────────────────────────────────────┤\n");
    for (l, r) in left.iter().zip(right.iter()) {
        body.push_str(&format!("│ {:<53} │ {:<55} │\n", clip(l, 53), clip(r, 55)));
    }
    body.push_str("├─────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤\n");
    body.push_str("│ TODAY'S TRADE HISTORY                                                                                           │\n");
    body.push_str("├──────────┬───────┬────────────┬────────────┬──────────┬────────────┬────────────────────────────────────────────┤\n");
    body.push_str(&trade_row([
        "Time", "Side", "Entry", "Exit", "Reason", "Points", "Result",
    ]));
    body.push('\n');
    body.push_str("├──────────┼───────┼────────────┼────────────┼──────────┼────────────┼────────────────────────────────────────────┤\n");

    if s.trades.is_empty() {
        body.push_str(&trade_row([
            "--:--",
            "--",
            "--",
            "--",
            "--",
            "--",
            "No completed ILRC trades today",
        ]));
        body.push('\n');
    } else {
        for trade in s.trades.iter().rev().take(8).rev() {
            let time = DateTime::parse_from_rfc3339(&trade.exit_time)
                .map(|t| {
                    t.with_timezone(&FixedOffset::east_opt(19_800).expect("IST"))
                        .format("%H:%M")
                        .to_string()
                })
                .unwrap_or_else(|_| "--:--".to_string());
            let entry = format!("{:.2}", trade.entry);
            let exit = format!("{:.2}", trade.exit);
            let points = format!("{:+.2}", trade.points);
            let setup = if trade.reason.starts_with("B_") {
                "B"
            } else {
                "A"
            };
            let outcome = if trade.points > 0.0 {
                "WIN"
            } else if trade.points < 0.0 {
                "LOSS"
            } else {
                "BE"
            };
            let result = format!("{setup} {outcome}");
            body.push_str(&trade_row([
                &time,
                trade.side,
                &entry,
                &exit,
                trade.reason,
                &points,
                &result,
            ]));
            body.push('\n');
        }
    }
    body.push_str("├──────────┴───────┴────────────┴────────────┴──────────┴────────────┴────────────────────────────────────────────┤\n");
    body.push_str(&full_row(format!(
        " Daily total: {} trades · {}W / {}L · {:+.2} points",
        s.trades.len(),
        wins,
        losses,
        gross_points
    )));
    body.push('\n');
    body.push_str("└─────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘");

    let mut stderr = io::stderr().lock();
    if cfg!(debug_assertions)
        && let Some((line_no, width)) = body
            .lines()
            .enumerate()
            .map(|(idx, line)| (idx + 1, line.chars().count()))
            .find(|(_, width)| *width != DASHBOARD_WIDTH)
    {
        let _ = writeln!(
            stderr,
            "[dashboard warning] row {line_no} has width {width}, expected {DASHBOARD_WIDTH}"
        );
    }
    let _ = writeln!(stderr, "\x1b[H\x1b[2J{body}");
    let _ = stderr.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trade_rows_have_fixed_dashboard_width() {
        let header = trade_row([
            "Time", "Side", "Entry", "Exit", "Reason", "Points", "Result",
        ]);
        let empty = trade_row([
            "--:--",
            "--",
            "--",
            "--",
            "--",
            "--",
            "No completed ILRC trades today",
        ]);
        assert_eq!(header.chars().count(), DASHBOARD_WIDTH);
        assert_eq!(empty.chars().count(), DASHBOARD_WIDTH);
        assert_eq!(full_row(" Daily total").chars().count(), DASHBOARD_WIDTH);
        assert_eq!(
            full_row(" ILRC v1 · PRODUCTION SHADOW DASHBOARD")
                .chars()
                .count(),
            DASHBOARD_WIDTH
        );
    }
}
