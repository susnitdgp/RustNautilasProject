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
    pub last_bar: Option<&'a kite_adapter::http::historical::Candle>,
    pub bars_loaded: usize,
    pub trades: &'a [Trade],
    pub report_directory: &'a std::path::Path,
}

pub fn render(s: Snapshot<'_>) {
    let minute = s.now.hour() * 60 + s.now.minute();
    let session = if minute < s.session_open_minute {
        "PRE-MARKET"
    } else if minute >= s.entry_cutoff_minute {
        "ENTRY CLOSED"
    } else {
        "ACTIVE"
    };
    let (last_bar, freshness) = match s.last_bar {
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
                    "{} | O {:.0} H {:.0} L {:.0} C {:.0}",
                    stamp, bar.open, bar.high, bar.low, bar.close
                ),
                format!("{}s", age),
            )
        }
        None => ("none".to_string(), "n/a".to_string()),
    };
    let gross_points: f64 = s.trades.iter().map(|t| t.points).sum();
    let wins = s.trades.iter().filter(|t| t.points > 0.0).count();
    let losses = s.trades.iter().filter(|t| t.points < 0.0).count();
    let setup = s
        .trades
        .last()
        .map(|t| {
            format!(
                "last {} {} → {} ({:+.2} pts)",
                t.side, t.entry, t.exit, t.points
            )
        })
        .unwrap_or_else(|| "SCANNING — no completed ILRC setup today".to_string());

    eprintln!();
    eprintln!("┌──────────────────────────────────────────────────────────────────────┐");
    eprintln!("│ ILRC v1 · PRODUCTION SHADOW DASHBOARD                              │");
    eprintln!("├──────────────────────────────────────────────────────────────────────┤");
    eprintln!("│ Run        {:<58}│", s.namespace);
    eprintln!("│ Strategy   {:<58}│", s.strategy);
    eprintln!(
        "│ Instrument {:<58}│",
        format!("{} · {}", s.instrument, s.interval)
    );
    eprintln!(
        "│ Session    {:<58}│",
        format!("{} · {}", session, s.now.format("%d-%m-%Y %H:%M:%S IST"))
    );
    eprintln!("│ Last bar   {:<58}│", last_bar);
    eprintln!(
        "│ Feed       {:<58}│",
        format!("bars {} · age {}", s.bars_loaded, freshness)
    );
    eprintln!("│ ILRC state {:<58}│", setup);
    eprintln!(
        "│ Today      {:<58}│",
        format!(
            "{} trades · {}W/{}L · {:+.2} gross pts",
            s.trades.len(),
            wins,
            losses,
            gross_points
        )
    );
    eprintln!(
        "│ Orders     {:<58}│",
        "DISABLED · no execution client loaded"
    );
    eprintln!(
        "│ Safety     {:<58}│",
        "strategy gate OFF · broker gate OFF · EOD 23:15"
    );
    eprintln!("│ Reports    {:<58}│", s.report_directory.display());
    eprintln!("└──────────────────────────────────────────────────────────────────────┘");
}
