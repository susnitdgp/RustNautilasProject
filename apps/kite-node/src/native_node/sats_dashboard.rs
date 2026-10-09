//! Live terminal dashboard for one `sats` slot run, drawn on stderr once a second.
//! JSON event logs go to stdout (a file); output errors are ignored everywhere so a
//! closed terminal or log can never crash the runner.
use std::collections::VecDeque;
use std::io::{IsTerminal, Write};
use std::sync::{Arc, Mutex};

pub type Shared = Arc<Mutex<Board>>;

/// Writes one JSON event line to stdout; never panics on a broken pipe.
pub fn emit(value: serde_json::Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{value}");
    let _ = out.flush();
}

/// Writes a human note to stderr; never panics.
pub fn note(text: &str) {
    let _ = writeln!(std::io::stderr().lock(), "{text}");
}

#[derive(Debug, Default)]
pub struct Board {
    pub mode: String,
    pub slot: String,
    pub instrument: String,
    pub square_off: String,
    pub exit_rule: String,
    pub redis_namespace: String,
    pub point_value: f64,
    pub lots: u32,
    /// Header title; empty = "SATS v…".
    pub title: String,
    /// Model panel: title and rows. Empty rows = the SATS panel.
    pub model_title: String,
    pub model_rows: Vec<(String, String)>,
    // feed
    pub last_price: Option<f64>,
    pub last_bar: Option<(String, f64)>,
    pub live_bars: u64,
    pub history_bars: u64,
    /// Bar length (ns) for the next-bar countdown; 0 hides it.
    pub bar_ns: i64,
    pub feed_fault: Option<String>,
    // SATS
    pub warmed: bool,
    pub trend: i8,
    pub supertrend: Option<f64>,
    pub tqi: f64,
    pub regime: String,
    pub next_r: [f64; 3],
    // position (broker fills)
    pub position: f64,
    pub entry_avg: Option<f64>,
    pub sl: Option<f64>,
    /// Trigger of the SL-M resting at the broker, if any.
    pub exchange_stop: Option<f64>,
    pub tps: Option<[f64; 3]>,
    pub realized_points: f64,
    pub round_trips: u32,
    pub fills: u32,
    // control
    pub status: String,
    pub halted: Option<String>,
    pub(crate) events: VecDeque<String>,
}

impl Board {
    pub fn shared(mut self) -> Shared {
        self.status = "STARTING".into();
        Arc::new(Mutex::new(self))
    }

    pub fn event(&mut self, text: String) {
        let stamp = chrono::Utc::now().with_timezone(&ist()).format("%H:%M:%S");
        self.events.push_front(format!("{stamp}  {text}"));
        self.events.truncate(8);
    }

    /// Applies one broker fill (`signed_qty` > 0 buys) to position, average entry and realised points.
    pub fn apply_fill(&mut self, signed_qty: f64, price: f64) {
        self.fills += 1;
        let before = self.position;
        let after = before + signed_qty;
        if before == 0.0 || before.signum() == signed_qty.signum() {
            // opening or adding
            let cost = self.entry_avg.unwrap_or(price) * before.abs() + price * signed_qty.abs();
            self.entry_avg = Some(cost / after.abs());
        } else {
            // reducing / closing (no reversals: SATS flattens before re-entering)
            let closed = signed_qty.abs().min(before.abs());
            if let Some(entry) = self.entry_avg {
                self.realized_points += before.signum() * (price - entry) * closed;
            }
            if after == 0.0 {
                self.entry_avg = None;
                self.round_trips += 1;
                self.sl = None;
                self.tps = None;
            }
        }
        self.position = after;
    }

    /// "13:20  (in 3:41)" — the next bar close, bars aligned to the clock (5m → :00, :05, …).
    pub fn next_bar(&self, now: chrono::DateTime<chrono::FixedOffset>) -> String {
        if self.bar_ns <= 0 {
            return "—".into();
        }
        let now_ns = now.timestamp_nanos_opt().unwrap_or(0);
        let next = (now_ns / self.bar_ns + 1) * self.bar_ns;
        let left = (next - now_ns) / 1_000_000_000;
        let at = chrono::DateTime::from_timestamp_nanos(next).with_timezone(&ist());
        format!("{}  (in {}:{:02})", at.format("%H:%M"), left / 60, left % 60)
    }

    fn title(&self) -> String {
        if self.title.is_empty() { format!("SATS v{}", sats::PORT_VERSION) } else { self.title.clone() }
    }

    pub fn unrealized_points(&self) -> Option<f64> {
        Some(self.position * (self.last_price? - self.entry_avg?))
    }

    pub fn render(&self, now: chrono::DateTime<chrono::FixedOffset>, square_off_ns: i64) -> String {
        let (g, r, y, b, d, x) = ("\x1b[32m", "\x1b[31m", "\x1b[33m", "\x1b[1m", "\x1b[2m", "\x1b[0m");
        let money = |pts: f64| {
            let c = if pts > 0.0 { g } else if pts < 0.0 { r } else { "" };
            format!("{c}{pts:+.1} pts  ₹{:+.0}{x}", pts * self.point_value)
        };
        let px = |v: Option<f64>| v.map_or("—".into(), |v| format!("{v:.0}"));
        let left = (square_off_ns - now.timestamp_nanos_opt().unwrap_or(0)) / 1_000_000_000;
        let countdown = if left > 0 { format!("{}h {:02}m", left / 3600, (left % 3600) / 60) } else { "reached".into() };
        let status_col = match self.status.as_str() {
            "RUNNING" => g,
            "STARTING" | "SQUARED OFF" | "STOPPING" => y,
            _ => r,
        };
        let mode_col = if self.mode.starts_with("LIVE") { r } else { y };
        let trend = match self.trend {
            1 => format!("{g}▲ BULLISH{x}"),
            -1 => format!("{r}▼ BEARISH{x}"),
            _ => "—".into(),
        };
        let pos = if self.position > 0.0 {
            format!("{g}LONG {:.0} lot{x}", self.position)
        } else if self.position < 0.0 {
            format!("{r}SHORT {:.0} lot{x}", -self.position)
        } else {
            "FLAT".into()
        };
        let line = format!("{d}{}{x}", "─".repeat(64));
        let mut s = String::new();
        let mut row = |t: String| {
            s.push_str(&t);
            s.push('\n');
        };
        row(format!("{b} {}  ·  {}  ·  {}{x}", self.title(), self.instrument, self.slot));
        row(format!(" {mode_col}{b}{}{x}   status {status_col}{b}{}{x}   {} IST", self.mode, self.status, now.format("%H:%M:%S")));
        row(line.clone());
        row(format!(" Price        {b}{}{x}     last bar {}", px(self.last_price),
            self.last_bar.as_ref().map_or("—".into(), |(t, c)| format!("{t} close {c:.0}"))));
        row(format!(" Bars         history {} · live {}   next {}", self.history_bars, self.live_bars, self.next_bar(now)));
        row(format!(" Feed         {}",
            self.feed_fault.as_ref().map_or(format!("{g}OK{x}"), |f| format!("{r}FAULT: {f}{x}"))));
        row(line.clone());
        if self.model_rows.is_empty() {
            row(format!(" Trend        {trend}    SuperTrend {}", px(self.supertrend)));
            row(format!(" TQI          {:.2} ({})    warmed {}", self.tqi, self.regime, if self.warmed { "yes" } else { "no" }));
            row(format!(" TP1 at      {:.2} R   exit: {}", self.next_r[0], self.exit_rule));
        } else {
            row(format!(" Trend        {trend}    warmed {}", if self.warmed { "yes" } else { "no" }));
            for (k, v) in &self.model_rows {
                row(format!(" {k:<12} {v}"));
            }
            row(format!(" Exit         {}", self.exit_rule));
        }
        row(line.clone());
        row(format!(" Position     {pos}    entry {}", px(self.entry_avg)));
        row(format!(" Stop / TP1   {} / {}{}", px(self.sl), px(self.tps.map(|t| t[0])),
            self.exchange_stop.map_or(String::new(), |t| format!("   (SL-M {t:.0} at Zerodha)"))));
        row(format!(" Unrealised   {}", self.unrealized_points().map_or("—".into(), money)));
        row(format!(" Realised     {}   ({} round trips, {} fills)", money(self.realized_points), self.round_trips, self.fills));
        row(line.clone());
        row(format!(" Square-off   {} IST  (in {countdown})   lots {}", self.square_off, self.lots));
        if let Some(h) = &self.halted {
            row(format!(" {r}{b}HALTED: {h}{x}"));
        }
        row(format!(" Redis run    {}", self.redis_namespace));
        row(line);
        row(format!("{b} Recent events{x}"));
        if self.events.is_empty() {
            row(format!(" {d}none yet{x}"));
        }
        for e in &self.events {
            row(format!(" {e}"));
        }
        row(format!("{d} Ctrl+C = flatten and stop · JSON log in logs/{x}"));
        s
    }
}

fn ist() -> chrono::FixedOffset {
    chrono::FixedOffset::east_opt(19_800).expect("IST")
}

/// Ratatui view of the board: header, market / SATS / position panels, session and events.
pub fn draw(frame: &mut ratatui::Frame, b: &Board, now: chrono::DateTime<chrono::FixedOffset>, square_off_ns: i64) {
    use ratatui::{
        layout::{Constraint, Layout},
        style::{Color, Modifier, Style},
        text::{Line, Span},
        widgets::{Block, BorderType, Paragraph, Wrap},
    };
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(Color::DarkGray);
    let green = Style::default().fg(Color::Green);
    let red = Style::default().fg(Color::Red);
    let yellow = Style::default().fg(Color::Yellow);
    let pnl = |pts: f64| {
        let style = if pts > 0.0 { green } else if pts < 0.0 { red } else { Style::default() };
        Span::styled(format!("{pts:+.1} pts  ₹{:+.0}", pts * b.point_value), style.add_modifier(Modifier::BOLD))
    };
    let px = |v: Option<f64>| v.map_or("—".to_string(), |v| format!("{v:.0}"));
    let kv = |k: &str, v: Vec<Span<'static>>| {
        let mut spans = vec![Span::styled(format!("{k:<12}"), dim)];
        spans.extend(v);
        Line::from(spans)
    };
    let panel = |title: &str| Block::bordered().border_type(BorderType::Rounded).title(Span::styled(format!(" {title} "), bold));

    let [header, panels, session, events] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(8),
        Constraint::Length(5),
        Constraint::Min(4),
    ])
    .areas(frame.area());
    let [market, model, position] =
        Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(33), Constraint::Percentage(37)]).areas(panels);

    let mode_style = if b.mode.starts_with("LIVE") { red } else { yellow };
    let status_style = match b.status.as_str() {
        "RUNNING" => green,
        "STARTING" | "SQUARED OFF" | "STOPPING" => yellow,
        _ => red,
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!(" {} ", b.title()), bold),
            Span::raw(format!("· {} · {}   ", b.instrument, b.slot)),
            Span::styled(b.mode.clone(), mode_style.add_modifier(Modifier::BOLD)),
            Span::raw("   "),
            Span::styled(b.status.clone(), status_style.add_modifier(Modifier::BOLD | Modifier::REVERSED)),
            Span::styled(format!("   {} IST", now.format("%H:%M:%S")), dim),
        ]))
        .block(Block::bordered().border_type(BorderType::Rounded)),
        header,
    );

    let feed = match &b.feed_fault {
        None => Span::styled("OK", green),
        Some(f) => Span::styled(format!("FAULT: {f}"), red),
    };
    frame.render_widget(
        Paragraph::new(vec![
            kv("Price", vec![Span::styled(px(b.last_price), bold)]),
            kv("Last bar", vec![Span::raw(b.last_bar.as_ref().map_or("—".into(), |(t, c)| format!("{t}  close {c:.0}")))]),
            kv("Bars", vec![Span::raw(format!("history {} · live {}", b.history_bars, b.live_bars))]),
            kv("Next bar", vec![Span::styled(b.next_bar(now), yellow)]),
            kv("Feed", vec![feed]),
        ])
        .block(panel("Market")),
        market,
    );

    let trend = match b.trend {
        1 => Span::styled("▲ BULLISH", green.add_modifier(Modifier::BOLD)),
        -1 => Span::styled("▼ BEARISH", red.add_modifier(Modifier::BOLD)),
        _ => Span::raw("—"),
    };
    let warmed = kv("Warmed", vec![if b.warmed { Span::styled("yes", green) } else { Span::styled("no", yellow) }]);
    let model_lines = if b.model_rows.is_empty() {
        vec![
            kv("Trend", vec![trend]),
            kv("SuperTrend", vec![Span::raw(px(b.supertrend))]),
            kv("TQI", vec![Span::raw(format!("{:.2}  {}", b.tqi, b.regime))]),
            kv("TP1 at", vec![Span::raw(format!("{:.2} R", b.next_r[0]))]),
            kv("Exit", vec![Span::raw(b.exit_rule.clone())]),
            warmed,
        ]
    } else {
        let mut lines = vec![kv("Trend", vec![trend])];
        lines.extend(b.model_rows.iter().map(|(k, v)| kv(k, vec![Span::raw(v.clone())])));
        lines.push(kv("Exit", vec![Span::raw(b.exit_rule.clone())]));
        lines.push(warmed);
        lines
    };
    let model_title = if b.model_title.is_empty() { "SATS" } else { b.model_title.as_str() };
    frame.render_widget(Paragraph::new(model_lines).block(panel(model_title)), model);

    let side = if b.position > 0.0 {
        Span::styled(format!("LONG {:.0} lot", b.position), green.add_modifier(Modifier::BOLD))
    } else if b.position < 0.0 {
        Span::styled(format!("SHORT {:.0} lot", -b.position), red.add_modifier(Modifier::BOLD))
    } else {
        Span::styled("FLAT", dim)
    };
    frame.render_widget(
        Paragraph::new(vec![
            kv("Position", vec![side]),
            kv("Entry", vec![Span::raw(px(b.entry_avg))]),
            kv("Stop", match b.exchange_stop {
                Some(t) => vec![Span::styled(px(b.sl), red), Span::styled(format!("  SL-M {t:.0} at Zerodha"), green)],
                None => vec![Span::styled(px(b.sl), red)],
            }),
            kv("TP1", vec![Span::styled(
                px(b.tps.map(|t| t[0])),
                green,
            )]),
            kv("Unrealised", vec![b.unrealized_points().map_or(Span::raw("—"), pnl)]),
            kv("Realised", vec![pnl(b.realized_points), Span::styled(format!("  {} trips", b.round_trips), dim)]),
        ])
        .block(panel("Position")),
        position,
    );

    let left = (square_off_ns - now.timestamp_nanos_opt().unwrap_or(0)) / 1_000_000_000;
    let countdown = if left > 0 { format!("in {}h {:02}m", left / 3600, (left % 3600) / 60) } else { "reached".into() };
    let mut session_lines = vec![
        kv("Square-off", vec![Span::raw(format!("{} IST  ({countdown})   lots {}", b.square_off, b.lots))]),
        kv("Redis run", vec![Span::styled(b.redis_namespace.clone(), dim)]),
    ];
    if let Some(h) = &b.halted {
        session_lines.push(kv("HALTED", vec![Span::styled(h.clone(), red.add_modifier(Modifier::BOLD))]));
    }
    frame.render_widget(Paragraph::new(session_lines).wrap(Wrap { trim: false }).block(panel("Session")), session);

    // Newest first; long events wrap onto the next line instead of being cut off.
    let lines: Vec<Line> = if b.events.is_empty() {
        vec![Line::from(Span::styled("none yet", dim))]
    } else {
        b.events.iter().map(|e| Line::from(e.clone())).collect()
    };
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(panel("Events").title_bottom(Span::styled(" Ctrl+C = flatten and stop · JSON log in logs/ ", dim))),
        events,
    );
}

/// Ratatui dashboard on stderr (alternate screen), redrawn every second while
/// stderr is a terminal. Raw mode is not enabled, so Ctrl+C still stops the run.
pub fn spawn(board: Shared, square_off_ns: i64) -> Option<Dashboard> {
    if !std::io::stderr().is_terminal() {
        return None;
    }
    let mut err = std::io::stderr();
    let _ = crossterm::execute!(err, crossterm::terminal::EnterAlternateScreen, crossterm::cursor::Hide);
    let mut terminal = ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(std::io::stderr())).ok()?;
    let task = tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
        loop {
            tick.tick().await;
            let now = chrono::Utc::now().with_timezone(&ist());
            if let Ok(b) = board.lock() {
                let _ = terminal.draw(|f| draw(f, &b, now, square_off_ns));
            }
        }
    });
    Some(Dashboard { task })
}

pub struct Dashboard {
    task: tokio::task::JoinHandle<()>,
}

impl Dashboard {
    /// Stops redrawing and restores the normal screen and cursor.
    pub fn close(self) {
        self.task.abort();
        let mut err = std::io::stderr();
        let _ = crossterm::execute!(err, crossterm::cursor::Show, crossterm::terminal::LeaveAlternateScreen);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_track_average_entry_and_realised_points() {
        let mut b = Board { point_value: 10.0, ..Board::default() };
        b.apply_fill(1.0, 8790.0);
        assert_eq!((b.position, b.entry_avg), (1.0, Some(8790.0)));
        b.last_price = Some(8800.0);
        assert_eq!(b.unrealized_points(), Some(10.0));
        b.apply_fill(-1.0, 8805.0);
        assert_eq!((b.position, b.entry_avg, b.round_trips), (0.0, None, 1));
        assert_eq!(b.realized_points, 15.0);
        b.apply_fill(-1.0, 8800.0); // short
        b.apply_fill(1.0, 8810.0); // covered higher: loss
        assert_eq!(b.realized_points, 5.0);
    }

    #[test]
    fn next_bar_counts_down_to_the_clock_aligned_close() {
        let b = Board { bar_ns: 300_000_000_000, ..Board::default() };
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-09T13:14:42+05:30").unwrap();
        assert_eq!(b.next_bar(now), "13:15  (in 0:18)");
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-09T13:15:00+05:30").unwrap();
        assert_eq!(b.next_bar(now), "13:20  (in 5:00)");
        assert_eq!(Board::default().next_bar(now), "—");
    }

    #[test]
    fn render_shows_the_essentials() {
        let mut b = Board {
            mode: "LIVE (real Zerodha orders)".into(),
            instrument: "CRUDEOILM26OCTFUT.MCX".into(),
            square_off: "23:15".into(),
            point_value: 10.0,
            status: "RUNNING".into(),
            ..Board::default()
        };
        b.apply_fill(1.0, 8790.0);
        b.last_price = Some(8780.0);
        b.event("BUY 1 lot".into());
        let text = b.render(chrono::Utc::now().with_timezone(&ist()), 0);
        for needle in ["CRUDEOILM26OCTFUT.MCX", "LIVE", "LONG 1 lot", "₹-100", "BUY 1 lot", "Square-off"] {
            assert!(text.contains(needle), "missing {needle}");
        }
    }

    #[test]
    fn ratatui_view_draws_every_panel() {
        use ratatui::{Terminal, backend::TestBackend};
        let mut b = Board { mode: "PAPER".into(), instrument: "CRUDEOILM26OCTFUT.MCX".into(), point_value: 10.0, ..Board::default() };
        b.apply_fill(-1.0, 8800.0);
        b.last_price = Some(8790.0);
        b.event("SELL 1 lot".into());
        let mut t = Terminal::new(TestBackend::new(110, 32)).unwrap();
        t.draw(|f| draw(f, &b, chrono::Utc::now().with_timezone(&ist()), 0)).unwrap();
        let text: String = t.backend().buffer().content().iter().map(|c| c.symbol()).collect();
        for needle in ["Market", "SATS", "Position", "Session", "Events", "SHORT 1 lot", "SELL 1 lot", "+10.0 pts"] {
            assert!(text.contains(needle), "missing {needle}");
        }
    }

    #[test]
    fn long_events_wrap_instead_of_being_cut() {
        use ratatui::{Terminal, backend::TestBackend};
        let mut b = Board { mode: "PAPER".into(), ..Board::default() };
        b.event(format!("Warm-up done on 1297 history bars {} END-OF-EVENT", "x".repeat(120)));
        let mut t = Terminal::new(TestBackend::new(100, 32)).unwrap();
        t.draw(|f| draw(f, &b, chrono::Utc::now().with_timezone(&ist()), 0)).unwrap();
        let text: String = t.backend().buffer().content().iter().map(|c| c.symbol()).collect();
        assert!(text.contains("END-OF-EVENT"), "tail of a long event must be visible");
    }
}

