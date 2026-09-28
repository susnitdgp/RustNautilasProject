//! Pure Squeeze Momentum historical/live dashboard.
//! Historical decisions are confirmed-bar only. Live SQZ diagnostics are previewed
//! tick-by-tick without mutating trading state.
use anyhow::{Context, Result, ensure};
use chrono::{Duration as ChronoDuration, NaiveDate};
use crossterm::{
    cursor,
    event::{self, Event as TermEvent, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use kite_adapter::http::historical::Candle;
use ratatui::{
    Terminal,
    backend::{CrosstermBackend, TestBackend},
    layout::{Constraint, Direction, Layout, Rect},
    prelude::{Color, Line, Span, Style},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, stdout},
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

use super::{
    squeeze_momentum_backtest::{Event as StrategyEvent, MonitorBar},
    squeeze_momentum_strategy::MomentumState,
};

const CONTRACT_MULTIPLIER: f64 = 100.0;

#[derive(Clone)]
struct EventView {
    timestamp: String,
    action: String,
    reason: String,
    price: f64,
    points: Option<f64>,
    position_after: i8,
}

#[derive(Clone)]
struct ReplayFrame {
    timestamp: String,
    close: f64,
    events_so_far: Vec<EventView>,
    monitor: MonitorBar,
}

#[derive(Clone)]
struct TradeRow {
    side: &'static str,
    entry_time: String,
    entry_price: f64,
    exit_time: Option<String>,
    exit_price: Option<f64>,
    exit_reason: Option<String>,
    points: f64,
    closed: bool,
}

#[derive(Clone)]
struct MonitorParams {
    sqz_length: usize,
    sqz_length_kc: usize,
    sqz_mult_kc: f64,
    use_true_range: bool,
    entry_strength_bars: usize,
    entry_deadband: f64,
    dynamic_deadband_ema_length: usize,
    dynamic_deadband_pct: f64,
    same_wave_reentry_limit: usize,
    weak_bars: usize,
    transition_pct: f64,
    allow_entries_only_in_session: bool,
    force_flat_at_session_end: bool,
    auto_sq_off_hour: u32,
    auto_sq_off_minute: u32,
    show_dashboard: bool,
}

impl MonitorParams {
    fn from_selection(selection: &super::production::Selection) -> Self {
        let s = &selection.squeeze_momentum;
        Self {
            sqz_length: s.sqz_length,
            sqz_length_kc: s.sqz_length_kc,
            sqz_mult_kc: s.sqz_mult_kc,
            use_true_range: s.sqz_use_true_range,
            entry_strength_bars: s.entry_strength_bars,
            entry_deadband: s.sqz_entry_deadband,
            dynamic_deadband_ema_length: s.sqz_dynamic_deadband_ema_length,
            dynamic_deadband_pct: s.sqz_dynamic_deadband_pct,
            same_wave_reentry_limit: s.same_wave_reentry_limit,
            weak_bars: s.sqz_weak_bars_req,
            transition_pct: s.sqz_transition_pct,
            allow_entries_only_in_session: s.allow_entries_only_in_session,
            force_flat_at_session_end: s.force_flat_at_session_end,
            auto_sq_off_hour: s.auto_sq_off_hour,
            auto_sq_off_minute: s.auto_sq_off_minute,
            show_dashboard: s.display.show_dashboard,
        }
    }
}

struct ReplayApp {
    instrument: String,
    date: NaiveDate,
    frames: Vec<ReplayFrame>,
    index: usize,
    playing: bool,
    delay: Duration,
    params: MonitorParams,
}

impl ReplayApp {
    fn current(&self) -> &ReplayFrame {
        &self.frames[self.index]
    }
    fn advance(&mut self) {
        if self.index + 1 < self.frames.len() {
            self.index += 1;
        } else {
            self.playing = false;
        }
    }
    fn retreat(&mut self) {
        self.index = self.index.saturating_sub(1);
    }
}

#[derive(Clone)]
struct DailySummary {
    date: NaiveDate,
    trades: usize,
    wins: usize,
    losses: usize,
    breakeven: usize,
    points: f64,
}

pub struct LiveDashboard {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    instrument: String,
    params: MonitorParams,
    started: Instant,
    seconds: u64,
}

impl LiveDashboard {
    pub fn new(selection: &super::production::Selection, seconds: u64) -> Result<Self> {
        let mut out = stdout();
        execute!(out, EnterAlternateScreen, cursor::Hide)?;
        let backend = CrosstermBackend::new(out);
        let mut terminal = Terminal::new(backend)?;
        terminal.clear()?;
        Ok(Self {
            terminal,
            instrument: selection.instrument.clone(),
            params: MonitorParams::from_selection(selection),
            started: Instant::now(),
            seconds,
        })
    }

    pub fn render(
        &mut self,
        state: &super::squeeze_momentum_actor::State,
        control: &super::live_control::Control,
    ) -> Result<()> {
        if !self.params.show_dashboard {
            return Ok(());
        }
        let elapsed = self.started.elapsed().as_secs();
        let remaining = self.seconds.saturating_sub(elapsed);
        let instrument = self.instrument.clone();
        let params = self.params.clone();
        self.terminal.draw(|frame| {
            render_live(
                frame,
                state,
                control,
                &instrument,
                &params,
                elapsed,
                remaining,
            )
        })?;
        Ok(())
    }
}

impl Drop for LiveDashboard {
    fn drop(&mut self) {
        let _ = execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            cursor::Show
        );
        let _ = self.terminal.show_cursor();
    }
}

pub fn run_history(config: &str, date: &str, snapshot: bool) -> Result<()> {
    let date =
        NaiveDate::parse_from_str(date, "%Y-%m-%d").context("dashboard DATE must be YYYY-MM-DD")?;
    let selection = super::production::Selection::load(config)?;
    let runtime = tokio::runtime::Runtime::new()?;
    let candles = runtime.block_on(kite_adapter::http::historical::fetch_window_for(
        selection.instrument_token,
        date,
        7,
        selection.interval,
    ))?;
    let mut app = build_app(&selection, &candles, date)?;
    if snapshot {
        render_snapshot(&mut app)
    } else {
        run_terminal(&mut app)
    }
}

pub fn run_summary(config: &str, from: &str, to: &str, _snapshot: bool) -> Result<()> {
    let from = NaiveDate::parse_from_str(from, "%Y-%m-%d")
        .context("summary FROM date must be YYYY-MM-DD")?;
    let to =
        NaiveDate::parse_from_str(to, "%Y-%m-%d").context("summary TO date must be YYYY-MM-DD")?;
    ensure!(from <= to, "summary FROM date must not be after TO date");
    ensure!(
        (to - from).num_days() <= 366,
        "summary range limited to 366 days"
    );
    let selection = super::production::Selection::load(config)?;
    let runtime = tokio::runtime::Runtime::new()?;
    let candles = runtime.block_on(fetch_range(&selection, from, to))?;
    let days = build_summary(&selection, &candles, from, to)?;
    render_summary_snapshot(&selection.instrument, from, to, &days);
    Ok(())
}

async fn fetch_range(
    selection: &super::production::Selection,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<Vec<Candle>> {
    let warmup_start = from
        .checked_sub_signed(ChronoDuration::days(7))
        .context("summary warmup date overflow")?;
    let mut cursor = to;
    let mut reader = kite_adapter::http::historical::Reader::default();
    let mut by_timestamp = BTreeMap::new();
    while cursor >= warmup_start {
        let remaining = (cursor - warmup_start).num_days();
        let days = remaining.clamp(1, 30);
        let batch = reader
            .fetch_window_for(selection.instrument_token, cursor, days, selection.interval)
            .await?;
        for candle in batch {
            let date = candle.time()?.date_naive();
            if date >= warmup_start && date <= to {
                by_timestamp.insert(candle.timestamp.clone(), candle);
            }
        }
        cursor = cursor
            .checked_sub_signed(ChronoDuration::days(days + 1))
            .context("summary historical cursor overflow")?;
    }
    let mut candles: Vec<_> = by_timestamp.into_values().collect();
    candles.sort_by_key(|c| c.time().ok());
    ensure!(
        !candles.is_empty(),
        "summary historical range has no candles"
    );
    Ok(candles)
}

fn build_summary(
    selection: &super::production::Selection,
    candles: &[Candle],
    from: NaiveDate,
    to: NaiveDate,
) -> Result<Vec<DailySummary>> {
    let report = super::squeeze_momentum_backtest::simulate(
        selection.squeeze_momentum.clone(),
        selection.session_calendar.clone(),
        selection.bar_ns(),
        &selection.instrument,
        selection.interval_name(),
        candles,
    )?;
    let mut dates = BTreeSet::new();
    for candle in candles {
        let d = candle.time()?.date_naive();
        if d >= from && d <= to {
            dates.insert(d);
        }
    }
    let mut daily: BTreeMap<NaiveDate, DailySummary> = dates
        .into_iter()
        .map(|date| {
            (
                date,
                DailySummary {
                    date,
                    trades: 0,
                    wins: 0,
                    losses: 0,
                    breakeven: 0,
                    points: 0.0,
                },
            )
        })
        .collect();
    for event in &report.events {
        let Some(points) = event.trade_points else {
            continue;
        };
        let date = NaiveDate::parse_from_str(&event.timestamp[..10], "%Y-%m-%d")?;
        let Some(day) = daily.get_mut(&date) else {
            continue;
        };
        day.trades += 1;
        day.points += points;
        if points > 0.0 {
            day.wins += 1;
        } else if points < 0.0 {
            day.losses += 1;
        } else {
            day.breakeven += 1;
        }
    }
    Ok(daily.into_values().collect())
}

fn build_app(
    selection: &super::production::Selection,
    candles: &[Candle],
    date: NaiveDate,
) -> Result<ReplayApp> {
    let (report, monitor_bars) = super::squeeze_momentum_backtest::simulate_with_monitor(
        selection.squeeze_momentum.clone(),
        selection.session_calendar.clone(),
        selection.bar_ns(),
        &selection.instrument,
        selection.interval_name(),
        candles,
    )?;
    let mut by_time: BTreeMap<String, Vec<EventView>> = BTreeMap::new();
    for event in &report.events {
        by_time
            .entry(event.timestamp.clone())
            .or_default()
            .push(event_view(event));
    }
    let monitor_by_time: BTreeMap<String, MonitorBar> = monitor_bars
        .into_iter()
        .map(|bar| (bar.timestamp.clone(), bar))
        .collect();
    let mut events_so_far = Vec::new();
    let mut frames = Vec::new();
    for candle in candles {
        let open = candle.time()?;
        if open.date_naive() != date {
            continue;
        }
        let timestamp = open.to_rfc3339();
        if let Some(events) = by_time.get(&timestamp) {
            events_so_far.extend(events.iter().cloned());
        }
        let monitor = monitor_by_time
            .get(&timestamp)
            .cloned()
            .context("SQZ dashboard monitor state missing")?;
        frames.push(ReplayFrame {
            timestamp,
            close: candle.close,
            events_so_far: events_so_far.clone(),
            monitor,
        });
    }
    ensure!(!frames.is_empty(), "dashboard date has no candles");
    Ok(ReplayApp {
        instrument: selection.instrument.clone(),
        date,
        frames,
        index: 0,
        playing: true,
        delay: Duration::from_millis(80),
        params: MonitorParams::from_selection(selection),
    })
}

fn event_view(event: &StrategyEvent) -> EventView {
    EventView {
        timestamp: event.timestamp.clone(),
        action: event.action.into(),
        reason: event.reason.into(),
        price: event.price,
        points: event.trade_points,
        position_after: event.position_after,
    }
}

fn trade_rows(events: &[EventView], current_price: f64, transition_pct: f64) -> Vec<TradeRow> {
    struct OpenTrade {
        side: &'static str,
        time: String,
        price: f64,
        direction: i8,
    }
    let mut open: Option<OpenTrade> = None;
    let mut rows = Vec::new();
    for event in events {
        match event.action.as_str() {
            "BUY" if event.position_after == 1 => {
                open = Some(OpenTrade {
                    side: "LONG",
                    time: short_time(&event.timestamp),
                    price: event.price,
                    direction: 1,
                });
            }
            "SHORT" if event.position_after == -1 => {
                open = Some(OpenTrade {
                    side: "SHORT",
                    time: short_time(&event.timestamp),
                    price: event.price,
                    direction: -1,
                });
            }
            "SELL" | "COVER" if event.position_after == 0 => {
                if let Some(entry) = open.take() {
                    let points = event.points.unwrap_or(if entry.direction > 0 {
                        event.price - entry.price
                    } else {
                        entry.price - event.price
                    });
                    rows.push(TradeRow {
                        side: entry.side,
                        entry_time: entry.time,
                        entry_price: entry.price,
                        exit_time: Some(short_time(&event.timestamp)),
                        exit_price: Some(event.price),
                        exit_reason: Some(exit_label(&event.action, &event.reason, transition_pct)),
                        points,
                        closed: true,
                    });
                }
            }
            _ => {}
        }
    }
    if let Some(entry) = open {
        let points = if entry.direction > 0 {
            current_price - entry.price
        } else {
            entry.price - current_price
        };
        rows.push(TradeRow {
            side: entry.side,
            entry_time: entry.time,
            entry_price: entry.price,
            exit_time: None,
            exit_price: None,
            exit_reason: None,
            points,
            closed: false,
        });
    }
    rows
}

fn exit_label(action: &str, reason: &str, transition_pct: f64) -> String {
    if reason == "session_force_flat" {
        return "SQOFF".into();
    }
    let marker = if action == "SELL" { "QLX" } else { "QSX" };
    if reason == "zero_cross" {
        format!("{marker}/ZERO")
    } else {
        format!("{marker}/{transition_pct:.0}%")
    }
}

fn short_time(timestamp: &str) -> String {
    timestamp.get(11..16).unwrap_or("--:--").to_owned()
}

fn run_terminal(app: &mut ReplayApp) -> Result<()> {
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, cursor::Hide)?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    let result = interactive_loop(&mut terminal, app);
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, cursor::Show)?;
    terminal.show_cursor()?;
    result
}

fn interactive_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut ReplayApp,
) -> Result<()> {
    let mut last = Instant::now();
    loop {
        terminal.draw(|frame| render_history(frame, app))?;
        let timeout = if app.playing {
            app.delay.saturating_sub(last.elapsed())
        } else {
            Duration::from_millis(250)
        };
        if event::poll(timeout)?
            && let TermEvent::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Char(' ') => app.playing = !app.playing,
                KeyCode::Right => {
                    app.playing = false;
                    app.advance();
                }
                KeyCode::Left => {
                    app.playing = false;
                    app.retreat();
                }
                KeyCode::Home => {
                    app.playing = false;
                    app.index = 0;
                }
                KeyCode::End => {
                    app.playing = false;
                    app.index = app.frames.len() - 1;
                }
                _ => {}
            }
        }
        if app.playing && last.elapsed() >= app.delay {
            app.advance();
            last = Instant::now();
        }
    }
    Ok(())
}

fn render_snapshot(app: &mut ReplayApp) -> Result<()> {
    app.index = app.frames.len() - 1;
    app.playing = false;
    let backend = TestBackend::new(150, 38);
    let mut terminal = Terminal::new(backend)?;
    terminal.draw(|frame| render_history(frame, app))?;
    let buffer = terminal.backend().buffer();
    for y in buffer.area.top()..buffer.area.bottom() {
        let mut line = String::new();
        for x in buffer.area.left()..buffer.area.right() {
            if let Some(cell) = buffer.cell((x, y)) {
                line.push_str(cell.symbol());
            }
        }
        println!("{}", line.trim_end());
    }
    let rows = trade_rows(
        &app.current().events_so_far,
        app.current().close,
        app.params.transition_pct,
    );
    let realized: f64 = rows.iter().filter(|r| r.closed).map(|r| r.points).sum();
    println!(
        "\nSQZ_DASHBOARD_REPLAY_COMPLETE date={} trades={} gross_points={:.1} gross_inr={:.0}",
        app.date,
        rows.iter().filter(|r| r.closed).count(),
        realized,
        realized * CONTRACT_MULTIPLIER
    );
    Ok(())
}

fn render_history(frame: &mut ratatui::Frame<'_>, app: &ReplayApp) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Min(17),
            Constraint::Length(1),
        ])
        .split(frame.area());
    let current = app.current();
    let rows = trade_rows(
        &current.events_so_far,
        current.close,
        app.params.transition_pct,
    );
    let realized: f64 = rows.iter().filter(|r| r.closed).map(|r| r.points).sum();
    let header = vec![
        Line::from(vec![
            Span::styled(
                " MCX CRUDE PURE SQZ v2.28.3 ",
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" CONFIRMED-BAR STRATEGY"),
        ]),
        Line::from(format!(
            " {} | {} | {} | 5m | Bar {}/{} | LTP {:.0}",
            app.instrument,
            app.date,
            short_time(&current.timestamp),
            app.index + 1,
            app.frames.len(),
            current.close
        )),
        Line::from(format!(
            " Position: {} | Realized: {realized:+.0} pt / {:+.0} INR | Actions: confirmed close only",
            position_label(current.monitor.position),
            realized * CONTRACT_MULTIPLIER
        )),
    ];
    frame.render_widget(
        Paragraph::new(header).block(Block::default().borders(Borders::ALL)),
        root[0],
    );

    let main = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(root[1]);
    render_trade_table(frame, &rows, main[0]);
    render_historical_monitor(
        frame,
        &current.monitor,
        &app.params,
        &current.events_so_far,
        main[1],
    );
    frame.render_widget(
        Paragraph::new(format!(
            " Closed {} | Net {realized:+.0} pt / {:+.0} INR | q quit | space play/pause | ←/→ step",
            rows.iter().filter(|r| r.closed).count(),
            realized * CONTRACT_MULTIPLIER
        )),
        root[2],
    );
}

fn render_trade_table(frame: &mut ratatui::Frame<'_>, rows: &[TradeRow], area: Rect) {
    let table_rows = rows.iter().enumerate().map(|(i, r)| {
        let style = if r.points > 0.0 {
            Style::default().fg(Color::Green)
        } else if r.points < 0.0 {
            Style::default().fg(Color::Red)
        } else {
            Style::default()
        };
        Row::new(vec![
            Cell::from((i + 1).to_string()),
            Cell::from(r.side),
            Cell::from(r.entry_time.clone()),
            Cell::from(format!("{:.0}", r.entry_price)),
            Cell::from(r.exit_time.clone().unwrap_or_else(|| "--".into())),
            Cell::from(
                r.exit_price
                    .map_or_else(|| "--".into(), |v| format!("{v:.0}")),
            ),
            Cell::from(r.exit_reason.clone().unwrap_or_else(|| "OPEN".into())),
            Cell::from(format!("{:+.0}", r.points)),
            Cell::from(format!("{:+.0}", r.points * CONTRACT_MULTIPLIER)),
        ])
        .style(style)
    });
    let widths = [
        Constraint::Length(2),
        Constraint::Length(6),
        Constraint::Length(6),
        Constraint::Length(7),
        Constraint::Length(6),
        Constraint::Length(7),
        Constraint::Length(10),
        Constraint::Length(6),
        Constraint::Length(8),
    ];
    let table = Table::new(table_rows, widths)
        .header(
            Row::new(vec![
                "#", "SIDE", "ENTRY", "ENT PX", "EXIT", "EXT PX", "REASON", "PTS", "P&L",
            ])
            .style(Style::default().fg(Color::Cyan)),
        )
        .column_spacing(1)
        .block(Block::default().borders(Borders::ALL).title(" TRADES "));
    frame.render_widget(table, area);
}

fn render_historical_monitor(
    frame: &mut ratatui::Frame<'_>,
    m: &MonitorBar,
    p: &MonitorParams,
    events: &[EventView],
    area: Rect,
) {
    let last = events.last();
    let event = last.map(|e| e.action.as_str()).unwrap_or("NONE");
    let reason = last.map(|e| e.reason.as_str()).unwrap_or("-");
    let squeeze = if m.squeeze_on {
        "ON"
    } else if m.squeeze_off {
        "OFF"
    } else {
        "NO SQZ"
    };
    let wave = if m.wave_side > 0 {
        if m.reentry_armed {
            "LONG RE-ENTRY"
        } else if m.wave_used {
            "LONG USED"
        } else {
            "LONG READY"
        }
    } else if m.wave_side < 0 {
        if m.reentry_armed {
            "SHORT RE-ENTRY"
        } else if m.wave_used {
            "SHORT USED"
        } else {
            "SHORT READY"
        }
    } else {
        "RESET"
    };
    let lines = monitor_lines(
        m.position,
        true,
        m.ready,
        m.squeeze_value,
        m.momentum_state,
        squeeze,
        wave,
        m.entry_ready,
        m.reentry_armed,
        m.reentries_used,
        m.strengthening_count,
        m.weakening_count,
        m.retracement_pct,
        m.extreme,
        m.in_session,
        event,
        reason,
        p,
        Some([m.open, m.high, m.low, m.close]),
        area.width,
    );
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" SQZ STATE ")),
        area,
    );
}

#[allow(clippy::too_many_arguments)]
fn monitor_lines<'a>(
    position: i8,
    confirmed: bool,
    ready: bool,
    value: f64,
    momentum: MomentumState,
    squeeze: &'a str,
    wave: &'a str,
    entry_ready: bool,
    reentry_armed: bool,
    reentries_used: usize,
    strength_count: usize,
    weak_count: usize,
    retracement: f64,
    extreme: Option<f64>,
    in_session: bool,
    event: &'a str,
    reason: &'a str,
    p: &MonitorParams,
    ohlc: Option<[f64; 4]>,
    area_width: u16,
) -> Vec<Line<'a>> {
    let deadband_label = if p.dynamic_deadband_pct > 0.0 {
        format!(
            "DYN EMA{} x {:.0}%",
            p.dynamic_deadband_ema_length, p.dynamic_deadband_pct
        )
    } else {
        format!("{:.1}", p.entry_deadband)
    };
    let bar_status = if confirmed { "CONFIRMED" } else { "FORMING" };
    let sqz_text = if ready {
        format!("{value:.1}")
    } else {
        "--".into()
    };
    let extreme_text = extreme.map_or_else(|| "-".into(), |v| format!("{v:.1}"));
    let entry_text = if entry_ready { "YES" } else { "NO" };
    let session_text = if in_session { "YES" } else { "NO" };
    let entry_restriction = if p.allow_entries_only_in_session {
        "ON"
    } else {
        "OFF"
    };
    let sqoff = if p.force_flat_at_session_end {
        "ON"
    } else {
        "OFF"
    };
    let separator = "─".repeat(usize::from(area_width.saturating_sub(4).clamp(8, 48)));

    let mut lines = vec![
        Line::from(format!(
            " State {} | Bar {bar_status}",
            position_label(position)
        )),
        Line::from(format!(" SQZ {sqz_text} | {}", momentum.label())),
        Line::from(format!(" Squeeze {squeeze} | Wave {wave}")),
        Line::from(format!(
            " Entry {entry_text} Str {strength_count}/{} | Weak {weak_count}/{}",
            p.entry_strength_bars, p.weak_bars
        )),
        Line::from(format!(
            " ReEntry {} {reentries_used}/{}",
            if reentry_armed { "ARMED" } else { "-" },
            p.same_wave_reentry_limit
        )),
        Line::from(format!(
            " Decay {retracement:.1}/{:.0}% | Extreme {extreme_text}",
            p.transition_pct
        )),
        Line::from(format!(
            " Session {session_text} Entries {entry_restriction} | SqOff {sqoff} {:02}:{:02}",
            p.auto_sq_off_hour, p.auto_sq_off_minute
        )),
        Line::from(format!(
            " Inputs B{} K{}x{:.1} TR{} | DB {}",
            p.sqz_length,
            p.sqz_length_kc,
            p.sqz_mult_kc,
            if p.use_true_range { "ON" } else { "OFF" },
            deadband_label
        )),
        Line::from(format!(" Event {event}")),
        Line::from(format!(" Reason {reason}")),
        Line::from(format!(" {separator}")),
    ];
    if let Some([open, high, low, close]) = ohlc {
        lines.extend([
            Line::from(format!(" Open       {open:>10.0}")),
            Line::from(format!(" High       {high:>10.0}")),
            Line::from(format!(" Low        {low:>10.0}")),
            Line::from(format!(" Close      {close:>10.0}")),
        ]);
    } else {
        lines.push(Line::from(" OHLC       --"));
    }
    lines
}

fn position_label(position: i8) -> &'static str {
    if position > 0 {
        "LONG"
    } else if position < 0 {
        "SHORT"
    } else {
        "FLAT"
    }
}

fn render_summary_snapshot(
    instrument: &str,
    from: NaiveDate,
    to: NaiveDate,
    days: &[DailySummary],
) {
    println!("PURE SQZ v2.28.3 DAILY SUMMARY | {instrument} | {from} -> {to}");
    println!("DATE        TRADES   W   L   BE   POINTS   P&L INR");
    let mut trades = 0;
    let mut wins = 0;
    let mut losses = 0;
    let mut be = 0;
    let mut points = 0.0;
    for d in days {
        println!(
            "{}  {:>6}  {:>2}  {:>2}  {:>3}  {:+7.0}  {:+9.0}",
            d.date,
            d.trades,
            d.wins,
            d.losses,
            d.breakeven,
            d.points,
            d.points * CONTRACT_MULTIPLIER
        );
        trades += d.trades;
        wins += d.wins;
        losses += d.losses;
        be += d.breakeven;
        points += d.points;
    }
    println!(
        "TOTAL       {:>6}  {:>2}  {:>2}  {:>3}  {:+7.0}  {:+9.0}",
        trades,
        wins,
        losses,
        be,
        points,
        points * CONTRACT_MULTIPLIER
    );
}

fn render_live(
    frame: &mut ratatui::Frame<'_>,
    state: &super::squeeze_momentum_actor::State,
    control: &super::live_control::Control,
    instrument: &str,
    params: &MonitorParams,
    elapsed: u64,
    remaining: u64,
) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Min(17),
            Constraint::Length(1),
        ])
        .split(frame.area());
    let price = live_price(state);
    let phase = if control.fault.lock().expect("fault lock").is_some() {
        "REVIEW REQUIRED"
    } else if control.stopping.load(Ordering::Acquire) {
        "STOPPING"
    } else if control.paused.load(Ordering::Acquire) {
        "PAUSED"
    } else {
        "MONITORING"
    };
    let header = vec![
        Line::from(vec![
            Span::styled(
                " MCX CRUDE PURE SQZ v2.28.3 ",
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" TICK DASHBOARD / CLOSE-ONLY ACTIONS"),
        ]),
        Line::from(format!(
            " {instrument} | {phase} | LTP {price:.0} | elapsed {elapsed}s / remaining {remaining}s"
        )),
        Line::from(format!(
            " Position: {} | Signals: {} | Fills: {} | REAL ORDERS: {}",
            position_label(state.trade_monitor.side),
            state.signals.len(),
            state.fills.len(),
            if control.real { "ENABLED" } else { "OFF" }
        )),
    ];
    frame.render_widget(
        Paragraph::new(header).block(Block::default().borders(Borders::ALL)),
        root[0],
    );
    let main = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(root[1]);
    let trades = live_trade_rows(state, price, params.transition_pct);
    render_trade_table(frame, &trades, main[0]);
    render_live_monitor(frame, state, params, main[1]);
    let open = state.trade_monitor.open_points(price).unwrap_or(0.0);
    frame.render_widget(
        Paragraph::new(format!(
            " Open P&L {open:+.0} pt / {:+.0} INR | forming candle preview; actions close-only",
            open * CONTRACT_MULTIPLIER
        )),
        root[2],
    );
}

fn render_live_monitor(
    frame: &mut ratatui::Frame<'_>,
    state: &super::squeeze_momentum_actor::State,
    p: &MonitorParams,
    area: Rect,
) {
    let Some(m) = state.latest_strategy else {
        frame.render_widget(
            Paragraph::new(" Waiting for SQZ warmup / first tick...")
                .block(Block::default().borders(Borders::ALL).title(" SQZ STATE ")),
            area,
        );
        return;
    };
    let squeeze = if m.squeeze_on {
        "ON"
    } else if m.squeeze_off {
        "OFF"
    } else {
        "NO SQZ"
    };
    let wave = if m.wave_side > 0 {
        if m.reentry_armed {
            "LONG RE-ENTRY"
        } else if m.wave_used {
            "LONG USED"
        } else {
            "LONG READY"
        }
    } else if m.wave_side < 0 {
        if m.reentry_armed {
            "SHORT RE-ENTRY"
        } else if m.wave_used {
            "SHORT USED"
        } else {
            "SHORT READY"
        }
    } else {
        "RESET"
    };
    let last = state.signals.last();
    let event = last
        .and_then(|v| v.get("intent"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("NONE");
    let reason = last
        .and_then(|v| v.get("reason"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("-");
    let lines = monitor_lines(
        state.trade_monitor.side,
        m.confirmed,
        m.ready,
        m.value,
        m.momentum_state,
        squeeze,
        wave,
        m.entry_ready,
        m.reentry_armed,
        m.reentries_used,
        m.strengthening_count,
        m.weakening_count,
        m.retracement_pct,
        m.extreme,
        m.in_session,
        event,
        reason,
        p,
        state.latest_ohlc,
        area.width,
    );
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" SQZ STATE ")),
        area,
    );
}

fn live_trade_rows(
    state: &super::squeeze_momentum_actor::State,
    current_price: f64,
    transition_pct: f64,
) -> Vec<TradeRow> {
    struct Open {
        side: &'static str,
        time: String,
        price: f64,
        direction: i8,
    }
    let mut open: Option<Open> = None;
    let mut rows = Vec::new();
    for (signal, fill) in state.signals.iter().zip(state.fills.iter()) {
        let Some(intent) = signal.get("intent").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let Some(price) = fill
            .get("price")
            .and_then(serde_json::Value::as_str)
            .and_then(|v| v.parse::<f64>().ok())
        else {
            continue;
        };
        let ts = fill
            .get("timestamp_ns")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let time = format_ns_short(ts);
        match intent {
            "BUY" => {
                open = Some(Open {
                    side: "LONG",
                    time,
                    price,
                    direction: 1,
                })
            }
            "SHORT" => {
                open = Some(Open {
                    side: "SHORT",
                    time,
                    price,
                    direction: -1,
                })
            }
            "SELL" | "COVER" => {
                if let Some(e) = open.take() {
                    let points = if e.direction > 0 {
                        price - e.price
                    } else {
                        e.price - price
                    };
                    let reason = signal
                        .get("reason")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("-");
                    rows.push(TradeRow {
                        side: e.side,
                        entry_time: e.time,
                        entry_price: e.price,
                        exit_time: Some(time),
                        exit_price: Some(price),
                        exit_reason: Some(exit_label(intent, reason, transition_pct)),
                        points,
                        closed: true,
                    });
                }
            }
            _ => {}
        }
    }
    if let Some(e) = open {
        let points = if e.direction > 0 {
            current_price - e.price
        } else {
            e.price - current_price
        };
        rows.push(TradeRow {
            side: e.side,
            entry_time: e.time,
            entry_price: e.price,
            exit_time: None,
            exit_price: None,
            exit_reason: None,
            points,
            closed: false,
        });
    }
    rows
}

fn live_price(state: &super::squeeze_momentum_actor::State) -> f64 {
    state
        .last_accepted_quote
        .as_ref()
        .map(|q| (q.bid_price.as_f64() + q.ask_price.as_f64()) * 0.5)
        .or_else(|| state.latest_strategy.map(|m| m.close))
        .unwrap_or(0.0)
}
fn format_ns_short(ns: u64) -> String {
    chrono::DateTime::from_timestamp_nanos(ns as i64)
        .with_timezone(&chrono::FixedOffset::east_opt(19_800).expect("IST"))
        .format("%H:%M")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exit_labels_are_sqz_specific() {
        assert_eq!(exit_label("SELL", "zero_cross", 45.0), "QLX/ZERO");
        assert_eq!(exit_label("COVER", "sqz_transition", 45.0), "QSX/45%");
        assert_eq!(exit_label("SELL", "session_force_flat", 45.0), "SQOFF");
    }
}
