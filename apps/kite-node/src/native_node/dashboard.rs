//! Terminal trade ledger and strategy-state monitor for Trend Ribbon.
//!
//! Historical mode is fully read-only. Live monitor mode consumes the existing
//! paper actor and Nautilus Sandbox fills; it never enables Kite broker execution.
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

use super::trend_ribbon_backtest::{Event as StrategyEvent, MonitorBar};

const CONTRACT_MULTIPLIER: f64 = 100.0;

#[cfg(test)]
#[derive(serde::Deserialize)]
struct Fixture {
    instrument: Option<String>,
    candles: Vec<Candle>,
}

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
    fast_hold_seconds: u64,
    fast_body_atr_min: f64,
    fast_range_atr_min: f64,
    pre_close_enabled: bool,
    pre_close_seconds: u64,
    squeeze_exit_enabled: bool,
    squeeze_bb_length: usize,
    squeeze_bb_mult: f64,
    squeeze_kc_length: usize,
    squeeze_kc_mult: f64,
    squeeze_use_true_range: bool,
    squeeze_weak_bars_required: usize,
    squeeze_transition_pct: f64,
    deviation_multiplier: f64,
    minimum_slope: f64,
}

impl MonitorParams {
    fn from_selection(selection: &super::production::Selection) -> Self {
        let realtime = &selection.trend_ribbon.realtime;
        Self {
            fast_hold_seconds: realtime.fast_hold_seconds,
            fast_body_atr_min: realtime.fast_body_atr_min,
            fast_range_atr_min: realtime.fast_range_atr_min,
            pre_close_enabled: realtime.pre_close_enabled,
            pre_close_seconds: realtime.pre_close_seconds,
            squeeze_exit_enabled: realtime.squeeze_exit_enabled,
            squeeze_bb_length: realtime.squeeze_bb_length,
            squeeze_bb_mult: realtime.squeeze_bb_mult,
            squeeze_kc_length: realtime.squeeze_kc_length,
            squeeze_kc_mult: realtime.squeeze_kc_mult,
            squeeze_use_true_range: realtime.squeeze_use_true_range,
            squeeze_weak_bars_required: realtime.squeeze_weak_bars_required,
            squeeze_transition_pct: realtime.squeeze_transition_pct,
            deviation_multiplier: selection.trend_ribbon.deviation_multiplier,
            minimum_slope: selection.trend_ribbon.minimum_slope,
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
        state: &super::trend_ribbon_actor::State,
        control: &super::live_control::Control,
    ) -> Result<()> {
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

#[derive(Clone)]
struct DailySummary {
    date: NaiveDate,
    trades: usize,
    wins: usize,
    losses: usize,
    breakeven: usize,
    points: f64,
}

struct SummaryApp {
    instrument: String,
    from: NaiveDate,
    to: NaiveDate,
    days: Vec<DailySummary>,
    selected: usize,
}

impl SummaryApp {
    fn selected_date(&self) -> NaiveDate {
        self.days[self.selected].date
    }

    fn select_previous(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    fn select_next(&mut self) {
        if self.selected + 1 < self.days.len() {
            self.selected += 1;
        }
    }
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

    fn speed_up(&mut self) {
        self.delay =
            Duration::from_millis(self.delay.as_millis().saturating_sub(20).max(20) as u64);
    }

    fn slow_down(&mut self) {
        self.delay = Duration::from_millis((self.delay.as_millis() as u64 + 20).min(1000));
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

pub fn run_summary(config: &str, from: &str, to: &str, snapshot: bool) -> Result<()> {
    let from = NaiveDate::parse_from_str(from, "%Y-%m-%d")
        .context("summary FROM date must be YYYY-MM-DD")?;
    let to =
        NaiveDate::parse_from_str(to, "%Y-%m-%d").context("summary TO date must be YYYY-MM-DD")?;
    ensure!(from <= to, "summary FROM date must not be after TO date");
    ensure!(
        (to - from).num_days() <= 366,
        "summary range is limited to 366 calendar days"
    );

    let selection = super::production::Selection::load(config)?;
    let runtime = tokio::runtime::Runtime::new()?;
    let candles = runtime.block_on(fetch_range(&selection, from, to))?;
    let mut app = build_summary(&selection, &candles, from, to)?;
    if snapshot {
        render_summary_snapshot(&mut app)
    } else {
        run_summary_terminal(&selection, &candles, &mut app)
    }
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
    candles.sort_by_key(|candle| candle.time().ok());
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
) -> Result<SummaryApp> {
    ensure!(!candles.is_empty(), "summary replay has no candles");
    let report = super::trend_ribbon_backtest::simulate(
        selection.trend_ribbon.clone(),
        selection.session_calendar.clone(),
        selection.bar_ns(),
        &selection.instrument,
        selection.interval_name(),
        candles,
    )?;

    let mut trading_dates = BTreeSet::new();
    for candle in candles {
        let date = candle.time()?.date_naive();
        if date >= from && date <= to {
            trading_dates.insert(date);
        }
    }
    ensure!(
        !trading_dates.is_empty(),
        "summary range has no trading-day candles"
    );

    let mut daily: BTreeMap<NaiveDate, DailySummary> = trading_dates
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
        let Some(date_text) = event.timestamp.get(..10) else {
            continue;
        };
        let date = NaiveDate::parse_from_str(date_text, "%Y-%m-%d")?;
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

    Ok(SummaryApp {
        instrument: selection.instrument.clone(),
        from,
        to,
        days: daily.into_values().collect(),
        selected: 0,
    })
}

#[cfg(test)]
fn run_fixture(config: &str, fixture: &str, date: NaiveDate) -> Result<ReplayApp> {
    let selection = super::production::Selection::load(config)?;
    let fixture: Fixture = serde_json::from_str(&std::fs::read_to_string(fixture)?)?;
    let instrument = fixture
        .instrument
        .as_deref()
        .unwrap_or(&selection.instrument);
    ensure!(
        instrument == selection.instrument,
        "dashboard fixture instrument mismatch"
    );
    build_app(&selection, &fixture.candles, date)
}

fn build_app(
    selection: &super::production::Selection,
    candles: &[Candle],
    date: NaiveDate,
) -> Result<ReplayApp> {
    ensure!(!candles.is_empty(), "dashboard replay has no candles");
    let (report, monitor_bars) = super::trend_ribbon_backtest::simulate_with_monitor(
        selection.trend_ribbon.clone(),
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
            .context("dashboard monitor state missing for candle")?;
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

fn trade_rows(events: &[EventView], current_price: f64) -> Vec<TradeRow> {
    struct OpenTrade {
        side: &'static str,
        entry_time: String,
        entry_price: f64,
        direction: i8,
    }

    let mut open: Option<OpenTrade> = None;
    let mut rows = Vec::new();

    for event in events {
        match event.action.as_str() {
            "BUY" if event.position_after > 0 => {
                open = Some(OpenTrade {
                    side: "LONG",
                    entry_time: short_time(&event.timestamp),
                    entry_price: event.price,
                    direction: 1,
                });
            }
            "SHORT" if event.position_after < 0 => {
                open = Some(OpenTrade {
                    side: "SHORT",
                    entry_time: short_time(&event.timestamp),
                    entry_price: event.price,
                    direction: -1,
                });
            }
            "SELL" | "COVER" if event.position_after == 0 => {
                if let Some(entry) = open.take() {
                    let points = event.points.unwrap_or({
                        if entry.direction > 0 {
                            event.price - entry.entry_price
                        } else {
                            entry.entry_price - event.price
                        }
                    });
                    rows.push(TradeRow {
                        side: entry.side,
                        entry_time: entry.entry_time,
                        entry_price: entry.entry_price,
                        exit_time: Some(short_time(&event.timestamp)),
                        exit_price: Some(event.price),
                        exit_reason: Some(reason_label(&event.reason)),
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
            current_price - entry.entry_price
        } else {
            entry.entry_price - current_price
        };
        rows.push(TradeRow {
            side: entry.side,
            entry_time: entry.entry_time,
            entry_price: entry.entry_price,
            exit_time: None,
            exit_price: None,
            exit_reason: None,
            points,
            closed: false,
        });
    }
    rows
}

fn short_time(timestamp: &str) -> String {
    timestamp.get(11..16).unwrap_or("--:--").to_owned()
}

fn reason_label(reason: &str) -> String {
    match reason {
        "trend_reversal" => "REVERSAL",
        "squeeze_long_exit" => "QLX",
        "squeeze_short_exit" => "QSX",
        "squeeze_re_buy" => "RB",
        "squeeze_re_short" => "RS",
        "session_end" => "SQ OFF",
        "shutdown" => "SHUTDOWN",
        "trend_ribbon" => "REVERSAL",
        "fast_buy" | "fast_short" => "FAST",
        "preclose_buy" | "preclose_short" => "PRE-CLOSE",
        "close_sync" => "CLOSE SYNC",
        other => other,
    }
    .to_owned()
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
    let mut last_advance = Instant::now();
    loop {
        terminal.draw(|frame| render(frame, app))?;
        let timeout = if app.playing {
            app.delay.saturating_sub(last_advance.elapsed())
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
                KeyCode::Char('+') | KeyCode::Char('=') => app.speed_up(),
                KeyCode::Char('-') => app.slow_down(),
                _ => {}
            }
        }

        if app.playing && last_advance.elapsed() >= app.delay {
            app.advance();
            last_advance = Instant::now();
        }
    }
    Ok(())
}

fn render_snapshot(app: &mut ReplayApp) -> Result<()> {
    app.index = app.frames.len() - 1;
    app.playing = false;
    let backend = TestBackend::new(160, 42);
    let mut terminal = Terminal::new(backend)?;
    terminal.draw(|frame| render(frame, app))?;
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

    let rows = trade_rows(&app.current().events_so_far, app.current().close);
    let realized: f64 = rows
        .iter()
        .filter(|row| row.closed)
        .map(|row| row.points)
        .sum();
    println!(
        "\nDASHBOARD_REPLAY_COMPLETE date={} trades={} gross_points={:.1} gross_inr={:.0}",
        app.date,
        rows.iter().filter(|row| row.closed).count(),
        realized,
        realized * CONTRACT_MULTIPLIER
    );
    Ok(())
}

fn render(frame: &mut ratatui::Frame<'_>, app: &ReplayApp) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Min(22),
            Constraint::Length(4),
            Constraint::Length(5),
            Constraint::Length(2),
        ])
        .split(frame.area());

    render_header(frame, app, root[0]);
    render_main(frame, app, root[1]);
    render_current_trade(frame, app, root[2]);
    render_totals(frame, app, root[3]);
    render_footer(frame, app, root[4]);
}

fn render_main(frame: &mut ratatui::Frame<'_>, app: &ReplayApp, area: Rect) {
    if area.width < 120 {
        render_trade_table(frame, app, area);
        return;
    }
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(66), Constraint::Percentage(34)])
        .split(area);
    render_trade_table(frame, app, columns[0]);
    render_monitor_panel(frame, app, columns[1]);
}

fn render_header(frame: &mut ratatui::Frame<'_>, app: &ReplayApp, area: Rect) {
    let current = app.current();
    let mode = if app.playing { "PLAY" } else { "PAUSED" };
    let monitor = &current.monitor;
    let last_reason = current
        .events_so_far
        .last()
        .map_or("--".to_owned(), |event| reason_label(&event.reason));
    let position = position_label(monitor.position);
    let entry = monitor
        .entry_price
        .map_or_else(|| "--".into(), |value| format!("{value:.0}"));
    let open_points = monitor.open_points.unwrap_or(0.0);
    let title = vec![
        Line::from(vec![
            Span::styled(
                " TREND RIBBON v2.22 - STRATEGY MONITOR ",
                Style::default().fg(Color::Yellow),
            ),
            Span::raw("   READ-ONLY"),
        ]),
        Line::from(format!(
            " {} | {} | {} | 5m | Bar {}/{} | {} | LTP {:.0}",
            app.instrument,
            app.date,
            short_time(&current.timestamp),
            app.index + 1,
            app.frames.len(),
            mode,
            current.close
        )),
        Line::from(format!(
            " Position: {position} @ {entry} | Open P&L: {open_points:+.0} pt / {:+.0} INR | Last event: {last_reason}",
            open_points * CONTRACT_MULTIPLIER
        )),
    ];
    frame.render_widget(
        Paragraph::new(title).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn render_trade_table(frame: &mut ratatui::Frame<'_>, app: &ReplayApp, area: Rect) {
    let current = app.current();
    let trades = trade_rows(&current.events_so_far, current.close);

    let rows = trades.iter().enumerate().map(|(index, trade)| {
        let pnl = trade.points * CONTRACT_MULTIPLIER;
        let style = if trade.points > 0.0 {
            Style::default().fg(Color::Green)
        } else if trade.points < 0.0 {
            Style::default().fg(Color::Red)
        } else {
            Style::default()
        };
        Row::new(vec![
            Cell::from(format!("{}", index + 1)),
            Cell::from(trade.side),
            Cell::from(trade.entry_time.clone()),
            Cell::from(format!("{:.0}", trade.entry_price)),
            Cell::from(trade.exit_time.clone().unwrap_or_else(|| "--".into())),
            Cell::from(
                trade
                    .exit_price
                    .map_or_else(|| "--".into(), |price| format!("{price:.0}")),
            ),
            Cell::from(trade.exit_reason.clone().unwrap_or_else(|| "OPEN".into())),
            Cell::from(format!("{:+.0}", trade.points)),
            Cell::from(format!("{:+.0}", pnl)),
            Cell::from(if trade.closed { "CLOSED" } else { "OPEN" }),
        ])
        .style(style)
    });

    let header = Row::new(vec![
        "#",
        "SIDE",
        "ENTRY",
        "ENTRY PX",
        "EXIT",
        "EXIT PX",
        "EXIT REASON",
        "POINTS",
        "P&L INR",
        "STATUS",
    ])
    .style(Style::default().fg(Color::Cyan));

    let widths = [
        Constraint::Length(3),
        Constraint::Length(7),
        Constraint::Length(7),
        Constraint::Length(10),
        Constraint::Length(7),
        Constraint::Length(9),
        Constraint::Length(13),
        Constraint::Length(8),
        Constraint::Length(11),
        Constraint::Length(8),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .column_spacing(1)
        .block(Block::default().borders(Borders::ALL).title(" TRADES "));
    frame.render_widget(table, area);
}

fn render_monitor_panel(frame: &mut ratatui::Frame<'_>, app: &ReplayApp, area: Rect) {
    let current = app.current();
    let m = &current.monitor;
    let atr = m.atr.unwrap_or(0.0);
    let bull_body_atr = if atr > 0.0 {
        (m.close - m.open).max(0.0) / atr
    } else {
        0.0
    };
    let bear_body_atr = if atr > 0.0 {
        (m.open - m.close).max(0.0) / atr
    } else {
        0.0
    };
    let range_atr = if atr > 0.0 {
        (m.high - m.low) / atr
    } else {
        0.0
    };
    let setup = if m.bull_setup {
        "BULL"
    } else if m.bear_setup {
        "BEAR"
    } else {
        "NONE"
    };
    let squeeze_state = if m.squeeze_on {
        "SQUEEZE ON"
    } else if m.squeeze_off {
        "SQUEEZE OFF"
    } else if m.squeeze_no {
        "NO SQUEEZE"
    } else {
        "WARMUP"
    };
    let squeeze_direction = if !m.squeeze_ready {
        "WARMUP"
    } else if m.squeeze_value > 0.0 {
        if m.squeeze_strengthening_long {
            "POS STRONG"
        } else {
            "POS WEAK"
        }
    } else if m.squeeze_value < 0.0 {
        if m.squeeze_strengthening_short {
            "NEG STRONG"
        } else {
            "NEG WEAK"
        }
    } else {
        "ZERO"
    };
    let extreme = if m.position > 0 {
        m.squeeze_peak
    } else if m.position < 0 {
        m.squeeze_trough
    } else {
        None
    };
    let squeeze_watch = if m.exited_trend == 1 {
        if m.squeeze_reentry_ready {
            "RE-BUY READY"
        } else {
            "FLAT / WAIT RB"
        }
    } else if m.exited_trend == -1 {
        if m.squeeze_reentry_ready {
            "RE-SHORT READY"
        } else {
            "FLAT / WAIT RS"
        }
    } else if m.squeeze_exit_ready {
        "EXIT READY"
    } else if m.squeeze_exit_used_in_trend {
        "EXIT USED"
    } else if m.position > 0 {
        if m.squeeze_armed {
            "LONG TRANS WATCH"
        } else {
            "LONG WATCH"
        }
    } else if m.position < 0 {
        if m.squeeze_armed {
            "SHORT TRANS WATCH"
        } else {
            "SHORT WATCH"
        }
    } else {
        "IDLE"
    };
    let last_reason = current
        .events_so_far
        .last()
        .map_or("--".to_owned(), |event| reason_label(&event.reason));
    let signal = if m.signal > 0 {
        "BUY"
    } else if m.signal < 0 {
        "SHORT"
    } else {
        "--"
    };

    let squeeze_value_text = if m.squeeze_ready {
        format!("{:.1}", m.squeeze_value)
    } else {
        "--".into()
    };
    let lines = vec![
        section_line("TREND / POSITION"),
        kv_pair_line(
            "Trend",
            direction_label(m.direction),
            "Position",
            position_label(m.position),
        ),
        kv_pair_line("Setup", setup, "Entry", &fmt_opt(m.entry_price, 0)),
        kv_pair_line("Signal", signal, "Reason", &last_reason),
        section_line("RIBBON"),
        kv_line("ALMA", &fmt_opt(m.alma, 1)),
        kv_line(
            "Upper / Lower",
            &format!(
                "{} / {}",
                fmt_opt(m.upper_confirm, 1),
                fmt_opt(m.lower_confirm, 1)
            ),
        ),
        kv_pair_line("ATR", &fmt_opt(m.atr, 1), "Dev", &fmt_opt(m.deviation, 1)),
        kv_pair_line(
            "Slope",
            &fmt_opt(m.slope_score, 3),
            "Session",
            if m.in_session { "YES" } else { "NO" },
        ),
        section_line("FAST / PRE-CLOSE"),
        kv_line(
            "Body / Range ATR",
            &format!("B {bull_body_atr:.2} S {bear_body_atr:.2} | R {range_atr:.2}"),
        ),
        kv_line(
            "Threshold",
            &format!(
                "B {:.2} / R {:.2}",
                app.params.fast_body_atr_min, app.params.fast_range_atr_min
            ),
        ),
        kv_line(
            "Hold / Pre-close",
            &format!(
                "N/A replay | {}s / {}",
                app.params.fast_hold_seconds,
                if app.params.pre_close_enabled {
                    format!("{}s", app.params.pre_close_seconds)
                } else {
                    "OFF".into()
                }
            ),
        ),
        section_line("SQUEEZE MOMENTUM"),
        kv_pair_line(
            "Engine",
            if app.params.squeeze_exit_enabled {
                "ON"
            } else {
                "OFF"
            },
            "Ready",
            if m.squeeze_ready { "YES" } else { "NO" },
        ),
        kv_pair_line("Value", &squeeze_value_text, "Dir", squeeze_direction),
        kv_line("State", squeeze_state),
        kv_pair_line(
            "Armed",
            if m.squeeze_armed { "YES" } else { "NO" },
            "Extreme",
            &fmt_opt(extreme, 1),
        ),
        kv_pair_line(
            "Weak bars",
            &format!(
                "{}/{}",
                m.squeeze_weak_bars, app.params.squeeze_weak_bars_required
            ),
            "Transition",
            &format!(
                "{:.0}%/{:.0}%",
                m.squeeze_decay_pct, app.params.squeeze_transition_pct
            ),
        ),
        kv_line("SQZ exit/reentry", squeeze_watch),
        kv_line(
            "Inputs BB / KC",
            &format!(
                "{}({:.1}) / {}({:.1})",
                app.params.squeeze_bb_length,
                app.params.squeeze_bb_mult,
                app.params.squeeze_kc_length,
                app.params.squeeze_kc_mult
            ),
        ),
        kv_line(
            "KC range",
            if app.params.squeeze_use_true_range {
                "TRUE RANGE"
            } else {
                "HIGH-LOW"
            },
        ),
    ];

    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" CURRENT STRATEGY STATE "),
        ),
        area,
    );
}

fn render_current_trade(frame: &mut ratatui::Frame<'_>, app: &ReplayApp, area: Rect) {
    let m = &app.current().monitor;
    let lines = if m.position == 0 {
        vec![
            Line::from(" FLAT - no open trade"),
            Line::from(" MFE / MAE / retained profit will appear when a position is open."),
        ]
    } else {
        let mfe = m.mfe_points.unwrap_or(0.0).max(0.0);
        let mae = m.mae_points.unwrap_or(0.0);
        let open = m.open_points.unwrap_or(0.0);
        let retained = if mfe > 0.0 {
            (open.max(0.0) / mfe * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        };
        let giveback = (mfe - open).max(0.0);
        let duration = trade_duration(m.opened_at.as_deref(), &m.timestamp);
        vec![
            Line::from(format!(
                " {} @ {}   Best: {}   Worst: {}   MFE: {mfe:+.0} pt   MAE: {mae:+.0} pt   Open: {open:+.0} pt   Retained: {retained:.1}%",
                position_label(m.position),
                fmt_opt(m.entry_price, 0),
                fmt_opt(m.best_price, 0),
                fmt_opt(m.worst_price, 0),
            )),
            Line::from(format!(
                " Duration: {duration}   Best P&L: {:+.0} INR   Current P&L: {:+.0} INR   Giveback: {giveback:.0} pt",
                mfe * CONTRACT_MULTIPLIER,
                open * CONTRACT_MULTIPLIER
            )),
        ]
    };
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" CURRENT TRADE "),
        ),
        area,
    );
}

fn section_line(label: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!(" {label}"),
        Style::default().fg(Color::Cyan),
    ))
}

fn kv_line(label: &str, value: &str) -> Line<'static> {
    Line::from(format!(" {label:<18} {value}"))
}

fn kv_pair_line(
    left_label: &str,
    left_value: &str,
    right_label: &str,
    right_value: &str,
) -> Line<'static> {
    Line::from(format!(
        " {left_label:<8} {left_value:<11} {right_label:<8} {right_value}"
    ))
}

fn fmt_opt(value: Option<f64>, decimals: usize) -> String {
    value.map_or_else(|| "--".into(), |value| format!("{value:.decimals$}"))
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

fn direction_label(direction: i8) -> &'static str {
    if direction > 0 {
        "BULLISH"
    } else if direction < 0 {
        "BEARISH"
    } else {
        "FLAT"
    }
}

fn trade_duration(opened_at: Option<&str>, current: &str) -> String {
    let Some(opened_at) = opened_at else {
        return "--".into();
    };
    let Ok(opened) = chrono::DateTime::parse_from_rfc3339(opened_at) else {
        return "--".into();
    };
    let Ok(now) = chrono::DateTime::parse_from_rfc3339(current) else {
        return "--".into();
    };
    let seconds = (now - opened).num_seconds().max(0);
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60
    )
}

fn render_totals(frame: &mut ratatui::Frame<'_>, app: &ReplayApp, area: Rect) {
    let current = app.current();
    let trades = trade_rows(&current.events_so_far, current.close);
    let realized: f64 = trades
        .iter()
        .filter(|trade| trade.closed)
        .map(|trade| trade.points)
        .sum();
    let unrealized: f64 = trades
        .iter()
        .filter(|trade| !trade.closed)
        .map(|trade| trade.points)
        .sum();
    let realized = clean_zero(realized);
    let unrealized = clean_zero(unrealized);
    let total = clean_zero(realized + unrealized);
    let closed = trades.iter().filter(|trade| trade.closed).count();
    let wins = trades
        .iter()
        .filter(|trade| trade.closed && trade.points > 0.0)
        .count();
    let losses = trades
        .iter()
        .filter(|trade| trade.closed && trade.points < 0.0)
        .count();

    let lines = vec![
        Line::from(format!(
            " Closed trades: {closed}   Winners: {wins}   Losers: {losses}"
        )),
        Line::from(vec![
            Span::raw(format!(
                " Realized: {realized:+.0} pt / {:+.0} INR    ",
                realized * CONTRACT_MULTIPLIER
            )),
            Span::raw(format!(
                "Open: {unrealized:+.0} pt / {:+.0} INR    ",
                unrealized * CONTRACT_MULTIPLIER
            )),
            Span::styled(
                format!(
                    "TOTAL: {total:+.0} pt / {:+.0} INR",
                    total * CONTRACT_MULTIPLIER
                ),
                pnl_style(total),
            ),
        ]),
    ];

    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" P&L SUMMARY "),
        ),
        area,
    );
}

fn render_footer(frame: &mut ratatui::Frame<'_>, app: &ReplayApp, area: Rect) {
    let text = format!(
        " q quit | space play/pause | ←/→ step | Home/End | +/- speed   delay={}ms   STRATEGY MONITOR   NO EXECUTION ",
        app.delay.as_millis()
    );
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

fn render_live(
    frame: &mut ratatui::Frame<'_>,
    state: &super::trend_ribbon_actor::State,
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
            Constraint::Min(22),
            Constraint::Length(4),
            Constraint::Length(5),
            Constraint::Length(2),
        ])
        .split(frame.area());

    let current_price = live_price(state);
    let trades = live_trade_rows(state, current_price);
    render_live_header(
        frame,
        state,
        control,
        instrument,
        current_price,
        (elapsed, remaining),
        root[0],
    );

    if root[1].width >= 120 {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(66), Constraint::Percentage(34)])
            .split(root[1]);
        render_live_trade_table(frame, &trades, columns[0]);
        render_live_monitor_panel(frame, state, params, columns[1]);
    } else {
        render_live_trade_table(frame, &trades, root[1]);
    }
    render_live_current_trade(frame, state, current_price, root[2]);
    render_live_totals(frame, &trades, root[3]);
    frame.render_widget(
        Paragraph::new(
            " Ctrl-C graceful stop | Kite live data | Nautilus Sandbox fills | NO BROKER ORDERS ",
        )
        .style(Style::default().fg(Color::DarkGray)),
        root[4],
    );
}

fn render_live_header(
    frame: &mut ratatui::Frame<'_>,
    state: &super::trend_ribbon_actor::State,
    control: &super::live_control::Control,
    instrument: &str,
    current_price: f64,
    runtime: (u64, u64),
    area: Rect,
) {
    let (elapsed, remaining) = runtime;
    let phase = if control.fault.lock().expect("fault lock").is_some() {
        "REVIEW REQUIRED"
    } else if control.stopping.load(Ordering::Acquire) {
        "STOPPING"
    } else if control.paused.load(Ordering::Acquire) {
        "PAUSED / REBUILD"
    } else {
        "MONITORING"
    };
    let trade = &state.trade_monitor;
    let position = position_label(trade.side);
    let entry = fmt_opt(trade.entry, 0);
    let open_points = trade.open_points(current_price).unwrap_or(0.0);
    let last_reason = state
        .signals
        .last()
        .and_then(|value| value.get("reason"))
        .and_then(serde_json::Value::as_str)
        .map(reason_label)
        .unwrap_or_else(|| "--".into());
    let now = super::data::now();
    let lines = vec![
        Line::from(vec![
            Span::styled(
                " TREND RIBBON v2.22 - LIVE MONITOR ",
                Style::default().fg(Color::Yellow),
            ),
            Span::raw("   PAPER / NO BROKER ORDERS"),
        ]),
        Line::from(format!(
            " {instrument} | {} | 5m | {phase} | LTP {:.0} | elapsed {elapsed}s / remaining {remaining}s",
            format_ns_ist(now),
            current_price
        )),
        Line::from(format!(
            " Position: {position} @ {entry} | Open P&L: {open_points:+.0} pt / {:+.0} INR | Last event: {last_reason}",
            open_points * CONTRACT_MULTIPLIER
        )),
    ];
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn render_live_monitor_panel(
    frame: &mut ratatui::Frame<'_>,
    state: &super::trend_ribbon_actor::State,
    params: &MonitorParams,
    area: Rect,
) {
    let Some(m) = state.latest_realtime else {
        frame.render_widget(
            Paragraph::new(vec![
                section_line("CURRENT STRATEGY STATE"),
                Line::from(" Waiting for trusted realtime candle..."),
                Line::from(" Completed-bar warmup remains active."),
            ])
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" CURRENT STRATEGY STATE "),
            ),
            area,
        );
        return;
    };
    let snapshot = m.snapshot;
    let direction = latest_confirmed_direction(state);
    let setup = if snapshot.bull_setup {
        "BULL"
    } else if snapshot.bear_setup {
        "BEAR"
    } else {
        "NONE"
    };
    let squeeze_state = if snapshot.squeeze_on {
        "SQUEEZE ON"
    } else if snapshot.squeeze_off {
        "SQUEEZE OFF"
    } else if snapshot.squeeze_no {
        "NO SQUEEZE"
    } else {
        "WARMUP"
    };
    let squeeze_direction = if !snapshot.squeeze_ready {
        "WARMUP"
    } else if snapshot.squeeze_value > 0.0 {
        if snapshot.squeeze_strengthening_long {
            "POS STRONG"
        } else {
            "POS WEAK"
        }
    } else if snapshot.squeeze_value < 0.0 {
        if snapshot.squeeze_strengthening_short {
            "NEG STRONG"
        } else {
            "NEG WEAK"
        }
    } else {
        "ZERO"
    };
    let extreme = if m.position > 0 {
        m.squeeze_peak
    } else if m.position < 0 {
        m.squeeze_trough
    } else {
        None
    };
    let squeeze_watch = if m.exited_trend == 1 {
        if m.squeeze_reentry_ready {
            "RE-BUY READY"
        } else {
            "FLAT / WAIT RB"
        }
    } else if m.exited_trend == -1 {
        if m.squeeze_reentry_ready {
            "RE-SHORT READY"
        } else {
            "FLAT / WAIT RS"
        }
    } else if m.squeeze_exit_ready {
        "EXIT READY"
    } else if m.squeeze_exit_used_in_trend {
        "EXIT USED"
    } else if m.position > 0 {
        if m.squeeze_armed {
            "LONG TRANS WATCH"
        } else {
            "LONG WATCH"
        }
    } else if m.position < 0 {
        if m.squeeze_armed {
            "SHORT TRANS WATCH"
        } else {
            "SHORT WATCH"
        }
    } else {
        "IDLE"
    };
    let signal = state
        .signals
        .last()
        .and_then(|value| value.get("intent"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("--");
    let reason = state
        .signals
        .last()
        .and_then(|value| value.get("reason"))
        .and_then(serde_json::Value::as_str)
        .map(reason_label)
        .unwrap_or_else(|| "--".into());
    let lock = if !snapshot.trusted {
        "UNTRUSTED"
    } else if m.event_locked {
        "EVENT"
    } else if m.same_trend_lock {
        "SAME-TREND"
    } else {
        "OPEN"
    };
    let upper = snapshot.alma + snapshot.deviation * params.deviation_multiplier;
    let lower = snapshot.alma - snapshot.deviation * params.deviation_multiplier;

    let squeeze_value_text = if snapshot.squeeze_ready {
        format!("{:.1}", snapshot.squeeze_value)
    } else {
        "--".into()
    };
    let lines = vec![
        section_line("TREND / POSITION"),
        kv_pair_line(
            "Trend",
            direction_label(direction),
            "Position",
            position_label(m.position),
        ),
        kv_pair_line("Setup", setup, "Lock", lock),
        kv_pair_line("Signal", signal, "Reason", &reason),
        section_line("RIBBON"),
        kv_line("ALMA", &format!("{:.1}", snapshot.alma)),
        kv_line("Upper / Lower", &format!("{upper:.1} / {lower:.1}")),
        kv_pair_line(
            "ATR",
            &format!("{:.1}", snapshot.atr),
            "Dev",
            &format!("{:.1}", snapshot.deviation),
        ),
        kv_pair_line(
            "Slope",
            &format!("{:.3}", snapshot.slope_score),
            "Min",
            &format!("±{:.3}", params.minimum_slope),
        ),
        section_line("FAST / PRE-CLOSE"),
        kv_line(
            "Body / Range ATR",
            &format!(
                "B {:.2} S {:.2} | R {:.2}",
                m.bullish_body_atr, m.bearish_body_atr, m.range_atr
            ),
        ),
        kv_line(
            "Hold / Close",
            &format!(
                "{:.1}/{:.0}s | {:.1}s",
                m.opposite_hold_seconds, params.fast_hold_seconds as f64, m.remaining_seconds
            ),
        ),
        kv_pair_line(
            "FAST",
            if m.fast_ready { "READY" } else { "WAIT" },
            "PRE-CLOSE",
            if m.preclose_ready { "READY" } else { "WAIT" },
        ),
        section_line("SQUEEZE MOMENTUM"),
        kv_pair_line(
            "Engine",
            if params.squeeze_exit_enabled {
                "ON"
            } else {
                "OFF"
            },
            "Ready",
            if snapshot.squeeze_ready { "YES" } else { "NO" },
        ),
        kv_pair_line("Value", &squeeze_value_text, "Dir", squeeze_direction),
        kv_line("State", squeeze_state),
        kv_pair_line(
            "Armed",
            if m.squeeze_armed { "YES" } else { "NO" },
            "Extreme",
            &fmt_opt(extreme, 1),
        ),
        kv_pair_line(
            "Weak bars",
            &format!(
                "{}/{}",
                m.squeeze_weak_bars, params.squeeze_weak_bars_required
            ),
            "Transition",
            &format!(
                "{:.0}%/{:.0}%",
                m.squeeze_decay_pct, params.squeeze_transition_pct
            ),
        ),
        kv_line("SQZ exit/reentry", squeeze_watch),
        kv_line(
            "Inputs BB / KC",
            &format!(
                "{}({:.1}) / {}({:.1})",
                params.squeeze_bb_length,
                params.squeeze_bb_mult,
                params.squeeze_kc_length,
                params.squeeze_kc_mult
            ),
        ),
        kv_line(
            "KC range",
            if params.squeeze_use_true_range {
                "TRUE RANGE"
            } else {
                "HIGH-LOW"
            },
        ),
    ];
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" CURRENT STRATEGY STATE "),
        ),
        area,
    );
}

fn render_live_current_trade(
    frame: &mut ratatui::Frame<'_>,
    state: &super::trend_ribbon_actor::State,
    current_price: f64,
    area: Rect,
) {
    let trade = &state.trade_monitor;
    let lines = if trade.side == 0 {
        vec![
            Line::from(" FLAT - no open trade"),
            Line::from(" MFE / MAE / retained profit will appear when a position is open."),
        ]
    } else {
        let mfe = trade.mfe_points().unwrap_or(0.0).max(0.0);
        let mae = trade.mae_points().unwrap_or(0.0).min(0.0);
        let open = trade.open_points(current_price).unwrap_or(0.0);
        let retained = if mfe > 0.0 {
            (open.max(0.0) / mfe * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        };
        let giveback = (mfe - open).max(0.0);
        let duration = trade.opened_ns.map_or_else(
            || "--".into(),
            |opened| format_duration_ns(super::data::now().saturating_sub(opened)),
        );
        vec![
            Line::from(format!(
                " {} @ {}   Best: {}   Worst: {}   MFE: {mfe:+.0} pt   MAE: {mae:+.0} pt   Open: {open:+.0} pt   Retained: {retained:.1}%",
                position_label(trade.side),
                fmt_opt(trade.entry, 0),
                fmt_opt(trade.best_price, 0),
                fmt_opt(trade.worst_price, 0),
            )),
            Line::from(format!(
                " Duration: {duration}   Best P&L: {:+.0} INR   Current P&L: {:+.0} INR   Giveback: {giveback:.0} pt",
                mfe * CONTRACT_MULTIPLIER,
                open * CONTRACT_MULTIPLIER
            )),
        ]
    };
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" CURRENT TRADE "),
        ),
        area,
    );
}

fn render_live_totals(frame: &mut ratatui::Frame<'_>, trades: &[TradeRow], area: Rect) {
    let realized: f64 = trades
        .iter()
        .filter(|trade| trade.closed)
        .map(|trade| trade.points)
        .sum();
    let unrealized: f64 = trades
        .iter()
        .filter(|trade| !trade.closed)
        .map(|trade| trade.points)
        .sum();
    let total = clean_zero(realized + unrealized);
    let closed = trades.iter().filter(|trade| trade.closed).count();
    let wins = trades
        .iter()
        .filter(|trade| trade.closed && trade.points > 0.0)
        .count();
    let losses = trades
        .iter()
        .filter(|trade| trade.closed && trade.points < 0.0)
        .count();
    let lines = vec![
        Line::from(format!(
            " Closed trades: {closed}   Winners: {wins}   Losers: {losses}"
        )),
        Line::from(vec![
            Span::raw(format!(
                " Realized: {realized:+.0} pt / {:+.0} INR    Open: {unrealized:+.0} pt / {:+.0} INR    ",
                realized * CONTRACT_MULTIPLIER,
                unrealized * CONTRACT_MULTIPLIER
            )),
            Span::styled(
                format!(
                    "TOTAL: {total:+.0} pt / {:+.0} INR",
                    total * CONTRACT_MULTIPLIER
                ),
                pnl_style(total),
            ),
        ]),
    ];
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" P&L SUMMARY "),
        ),
        area,
    );
}

fn render_live_trade_table(frame: &mut ratatui::Frame<'_>, trades: &[TradeRow], area: Rect) {
    let rows = trades.iter().enumerate().map(|(index, trade)| {
        let pnl = trade.points * CONTRACT_MULTIPLIER;
        let style = if trade.points > 0.0 {
            Style::default().fg(Color::Green)
        } else if trade.points < 0.0 {
            Style::default().fg(Color::Red)
        } else {
            Style::default()
        };
        Row::new(vec![
            Cell::from(format!("{}", index + 1)),
            Cell::from(trade.side),
            Cell::from(trade.entry_time.clone()),
            Cell::from(format!("{:.0}", trade.entry_price)),
            Cell::from(trade.exit_time.clone().unwrap_or_else(|| "--".into())),
            Cell::from(
                trade
                    .exit_price
                    .map_or_else(|| "--".into(), |price| format!("{price:.0}")),
            ),
            Cell::from(trade.exit_reason.clone().unwrap_or_else(|| "OPEN".into())),
            Cell::from(format!("{:+.0}", trade.points)),
            Cell::from(format!("{:+.0}", pnl)),
            Cell::from(if trade.closed { "CLOSED" } else { "OPEN" }),
        ])
        .style(style)
    });
    let header = Row::new(vec![
        "#",
        "SIDE",
        "ENTRY",
        "ENTRY PX",
        "EXIT",
        "EXIT PX",
        "EXIT REASON",
        "POINTS",
        "P&L INR",
        "STATUS",
    ])
    .style(Style::default().fg(Color::Cyan));
    let widths = [
        Constraint::Length(3),
        Constraint::Length(7),
        Constraint::Length(7),
        Constraint::Length(10),
        Constraint::Length(7),
        Constraint::Length(9),
        Constraint::Length(13),
        Constraint::Length(8),
        Constraint::Length(11),
        Constraint::Length(8),
    ];
    frame.render_widget(
        Table::new(rows, widths)
            .header(header)
            .column_spacing(1)
            .block(Block::default().borders(Borders::ALL).title(" TRADES ")),
        area,
    );
}

fn live_trade_rows(state: &super::trend_ribbon_actor::State, current_price: f64) -> Vec<TradeRow> {
    struct OpenTrade {
        side: &'static str,
        entry_time: String,
        entry_price: f64,
        direction: i8,
    }

    let mut open: Option<OpenTrade> = None;
    let mut rows = Vec::new();
    for (signal, fill) in state.signals.iter().zip(state.fills.iter()) {
        let Some(intent) = signal.get("intent").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let Some(price) = fill
            .get("price")
            .and_then(serde_json::Value::as_str)
            .and_then(|value| value.parse::<f64>().ok())
        else {
            continue;
        };
        let ts = fill
            .get("timestamp_ns")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let time = short_time_ns(ts);
        let reason = signal
            .get("reason")
            .and_then(serde_json::Value::as_str)
            .map(reason_label)
            .unwrap_or_else(|| "--".into());

        match intent {
            "BUY" => {
                open = Some(OpenTrade {
                    side: "LONG",
                    entry_time: time,
                    entry_price: price,
                    direction: 1,
                });
            }
            "SELL" => {
                open = Some(OpenTrade {
                    side: "SHORT",
                    entry_time: time,
                    entry_price: price,
                    direction: -1,
                });
            }
            "BUY_EXIT" | "SELL_EXIT" => {
                if let Some(entry) = open.take() {
                    let points = if entry.direction > 0 {
                        price - entry.entry_price
                    } else {
                        entry.entry_price - price
                    };
                    rows.push(TradeRow {
                        side: entry.side,
                        entry_time: entry.entry_time,
                        entry_price: entry.entry_price,
                        exit_time: Some(time),
                        exit_price: Some(price),
                        exit_reason: Some(reason),
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
            current_price - entry.entry_price
        } else {
            entry.entry_price - current_price
        };
        rows.push(TradeRow {
            side: entry.side,
            entry_time: entry.entry_time,
            entry_price: entry.entry_price,
            exit_time: None,
            exit_price: None,
            exit_reason: None,
            points,
            closed: false,
        });
    }
    rows
}

fn live_price(state: &super::trend_ribbon_actor::State) -> f64 {
    if let Some(monitor) = state.latest_realtime {
        return monitor.snapshot.close;
    }
    state
        .last_accepted_quote
        .map(|quote| (quote.bid_price.as_f64() + quote.ask_price.as_f64()) / 2.0)
        .unwrap_or(0.0)
}

fn latest_confirmed_direction(state: &super::trend_ribbon_actor::State) -> i8 {
    state
        .indicators
        .iter()
        .rev()
        .find_map(|value| value.get("direction").and_then(serde_json::Value::as_i64))
        .map_or(0, |value| value as i8)
}

fn short_time_ns(ns: u64) -> String {
    if ns == 0 {
        return "--:--".into();
    }
    chrono::DateTime::from_timestamp_nanos(ns as i64)
        .with_timezone(&chrono::FixedOffset::east_opt(19_800).expect("IST"))
        .format("%H:%M")
        .to_string()
}

fn format_ns_ist(ns: u64) -> String {
    chrono::DateTime::from_timestamp_nanos(ns as i64)
        .with_timezone(&chrono::FixedOffset::east_opt(19_800).expect("IST"))
        .format("%d-%b-%Y %H:%M:%S")
        .to_string()
}

fn format_duration_ns(ns: u64) -> String {
    let seconds = ns / 1_000_000_000;
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60
    )
}

fn run_summary_terminal(
    selection: &super::production::Selection,
    candles: &[Candle],
    app: &mut SummaryApp,
) -> Result<()> {
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, cursor::Hide)?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    let result = summary_loop(&mut terminal, selection, candles, app);
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, cursor::Show)?;
    terminal.show_cursor()?;
    result
}

fn summary_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    selection: &super::production::Selection,
    candles: &[Candle],
    app: &mut SummaryApp,
) -> Result<()> {
    loop {
        terminal.draw(|frame| render_summary(frame, app))?;
        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        let TermEvent::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => break,
            KeyCode::Up => app.select_previous(),
            KeyCode::Down => app.select_next(),
            KeyCode::Home => app.selected = 0,
            KeyCode::End => app.selected = app.days.len() - 1,
            KeyCode::Enter => {
                let mut detail = build_app(selection, candles, app.selected_date())?;
                detail.index = detail.frames.len() - 1;
                detail.playing = false;
                interactive_loop(terminal, &mut detail)?;
                terminal.clear()?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn render_summary_snapshot(app: &mut SummaryApp) -> Result<()> {
    app.selected = app.days.len() - 1;
    let height = (app.days.len() + 12).clamp(20, 60) as u16;
    let backend = TestBackend::new(118, height);
    let mut terminal = Terminal::new(backend)?;
    terminal.draw(|frame| render_summary(frame, app))?;
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

    let total_trades: usize = app.days.iter().map(|day| day.trades).sum();
    let total_points: f64 = app.days.iter().map(|day| day.points).sum();
    println!(
        "\nDASHBOARD_SUMMARY_COMPLETE from={} to={} days={} trades={} gross_points={:.1} gross_inr={:.0}",
        app.from,
        app.to,
        app.days.len(),
        total_trades,
        total_points,
        total_points * CONTRACT_MULTIPLIER
    );
    Ok(())
}

fn render_summary(frame: &mut ratatui::Frame<'_>, app: &SummaryApp) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(8),
            Constraint::Length(5),
            Constraint::Length(2),
        ])
        .split(frame.area());

    render_summary_header(frame, app, root[0]);
    render_summary_table(frame, app, root[1]);
    render_summary_totals(frame, app, root[2]);
    render_summary_footer(frame, root[3]);
}

fn render_summary_header(frame: &mut ratatui::Frame<'_>, app: &SummaryApp, area: Rect) {
    let lines = vec![
        Line::from(vec![
            Span::styled(
                " TREND RIBBON v2.22 - DAILY PERFORMANCE ",
                Style::default().fg(Color::Yellow),
            ),
            Span::raw("   READ-ONLY"),
        ]),
        Line::from(format!(
            " {} | {} to {} | {} trading days | selected {}",
            app.instrument,
            app.from,
            app.to,
            app.days.len(),
            app.selected_date()
        )),
    ];
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn render_summary_table(frame: &mut ratatui::Frame<'_>, app: &SummaryApp, area: Rect) {
    let max_rows = area.height.saturating_sub(3).max(1) as usize;
    let start = app
        .selected
        .saturating_sub(max_rows.saturating_sub(1))
        .min(app.days.len().saturating_sub(max_rows));
    let end = (start + max_rows).min(app.days.len());

    let rows = app.days[start..end]
        .iter()
        .enumerate()
        .map(|(visible_index, day)| {
            let index = start + visible_index;
            let selected = index == app.selected;
            let style = if selected {
                Style::default().fg(Color::Yellow)
            } else {
                pnl_style(day.points)
            };
            Row::new(vec![
                Cell::from(if selected { ">" } else { " " }),
                Cell::from(day.date.format("%d-%b-%Y").to_string()),
                Cell::from(day.trades.to_string()),
                Cell::from(day.wins.to_string()),
                Cell::from(day.losses.to_string()),
                Cell::from(day.breakeven.to_string()),
                Cell::from(format!("{:+.0}", clean_zero(day.points))),
                Cell::from(format!(
                    "{:+.0}",
                    clean_zero(day.points * CONTRACT_MULTIPLIER)
                )),
            ])
            .style(style)
        });

    let header = Row::new(vec![
        "", "DATE", "TRADES", "WIN", "LOSS", "B/E", "POINTS", "P&L INR",
    ])
    .style(Style::default().fg(Color::Cyan));

    let widths = [
        Constraint::Length(2),
        Constraint::Length(13),
        Constraint::Length(8),
        Constraint::Length(6),
        Constraint::Length(6),
        Constraint::Length(5),
        Constraint::Length(10),
        Constraint::Length(13),
    ];

    frame.render_widget(
        Table::new(rows, widths)
            .header(header)
            .column_spacing(2)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" DAILY RESULTS "),
            ),
        area,
    );
}

fn render_summary_totals(frame: &mut ratatui::Frame<'_>, app: &SummaryApp, area: Rect) {
    let trades: usize = app.days.iter().map(|day| day.trades).sum();
    let wins: usize = app.days.iter().map(|day| day.wins).sum();
    let losses: usize = app.days.iter().map(|day| day.losses).sum();
    let breakeven: usize = app.days.iter().map(|day| day.breakeven).sum();
    let points = clean_zero(app.days.iter().map(|day| day.points).sum());
    let positive_days = app.days.iter().filter(|day| day.points > 0.0).count();
    let negative_days = app.days.iter().filter(|day| day.points < 0.0).count();

    let lines = vec![
        Line::from(format!(
            " Trading days: {}   Positive: {}   Negative: {}   Trades: {}   W/L/B: {}/{}/{}",
            app.days.len(),
            positive_days,
            negative_days,
            trades,
            wins,
            losses,
            breakeven
        )),
        Line::from(vec![
            Span::raw(" Total: "),
            Span::styled(
                format!("{points:+.0} pt / {:+.0} INR", points * CONTRACT_MULTIPLIER),
                pnl_style(points),
            ),
        ]),
    ];

    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" PERIOD SUMMARY "),
        ),
        area,
    );
}

fn render_summary_footer(frame: &mut ratatui::Frame<'_>, area: Rect) {
    frame.render_widget(
        Paragraph::new(
            " ↑/↓ select day | Enter open trade ledger | Home/End | q quit   NO EXECUTION ",
        )
        .style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

fn clean_zero(value: f64) -> f64 {
    if value.abs() < 0.000_001 { 0.0 } else { value }
}

fn pnl_style(value: f64) -> Style {
    if value > 0.0 {
        Style::default().fg(Color::Green)
    } else if value < 0.0 {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::Gray)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_fill_ledger_pairs_signals_and_sandbox_fills() {
        let state = super::super::trend_ribbon_actor::State {
            signals: vec![
                serde_json::json!({"intent":"BUY","reason":"trend_ribbon"}),
                serde_json::json!({"intent":"BUY_EXIT","reason":"squeeze_long_exit"}),
            ],
            fills: vec![
                serde_json::json!({"timestamp_ns":1_800_000_000_000_000_000u64,"price":"100.0"}),
                serde_json::json!({"timestamp_ns":1_800_000_300_000_000_000u64,"price":"112.0"}),
            ],
            ..Default::default()
        };
        let rows = live_trade_rows(&state, 112.0);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].closed);
        assert_eq!(rows[0].side, "LONG");
        assert_eq!(rows[0].points, 12.0);
        assert_eq!(rows[0].exit_reason.as_deref(), Some("QLX"));
    }

    #[test]
    fn live_dashboard_layout_renders_without_broker_state() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let config = root.join("../../config/production-trend-ribbon.json");
        let selection =
            super::super::production::Selection::load(config.to_str().unwrap()).unwrap();
        let params = MonitorParams::from_selection(&selection);
        let state = super::super::trend_ribbon_actor::State::default();
        let control = super::super::live_control::Control::new(true);
        let backend = TestBackend::new(160, 42);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_live(
                    frame,
                    &state,
                    &control,
                    &selection.instrument,
                    &params,
                    1,
                    29,
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("LIVE MONITOR"));
        assert!(text.contains("CURRENT STRATEGY STATE"));
        assert!(text.contains("NO BROKER ORDERS"));
    }

    #[test]
    fn sep22_daily_summary_matches_trade_ledger() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let config = root.join("../../config/production-trend-ribbon.json");
        let fixture = root.join("tests/fixtures/trend_ribbon_sep18_21_22.json");
        let selection =
            super::super::production::Selection::load(config.to_str().unwrap()).unwrap();
        let fixture: Fixture =
            serde_json::from_str(&std::fs::read_to_string(fixture).unwrap()).unwrap();
        let date = NaiveDate::from_ymd_opt(2026, 9, 22).unwrap();
        let summary = build_summary(&selection, &fixture.candles, date, date).unwrap();

        assert_eq!(summary.days.len(), 1);
        assert_eq!(summary.days[0].trades, 7);
        assert_eq!(summary.days[0].wins, 4);
        assert_eq!(summary.days[0].losses, 3);
        assert_eq!(summary.days[0].points, 262.0);
    }

    #[test]
    fn sep22_replay_builds_trade_ledger_with_expected_pnl() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let config = root.join("../../config/production-trend-ribbon.json");
        let fixture = root.join("tests/fixtures/trend_ribbon_sep18_21_22.json");
        let app = run_fixture(
            config.to_str().unwrap(),
            fixture.to_str().unwrap(),
            NaiveDate::from_ymd_opt(2026, 9, 22).unwrap(),
        )
        .unwrap();

        let final_frame = app.frames.last().unwrap();
        let trades = trade_rows(&final_frame.events_so_far, final_frame.close);
        let closed: Vec<_> = trades.iter().filter(|trade| trade.closed).collect();
        let total: f64 = closed.iter().map(|trade| trade.points).sum();

        assert_eq!(closed.len(), 7);
        assert_eq!(total, 262.0);
        assert_eq!(closed[0].side, "LONG");
        assert_eq!(closed[0].entry_price, 8925.0);
        assert_eq!(closed[0].exit_price, Some(8962.0));
        assert_eq!(closed[0].exit_reason.as_deref(), Some("QLX"));
        assert_eq!(
            closed.last().unwrap().exit_reason.as_deref(),
            Some("SQ OFF")
        );

        let monitored = app
            .frames
            .iter()
            .find(|frame| {
                frame.monitor.position != 0 && frame.monitor.mfe_points.is_some_and(|mfe| mfe > 0.0)
            })
            .expect("open trade monitor frame");
        assert!(monitored.monitor.alma.is_some());
        assert!(monitored.monitor.atr.is_some());
        assert!(monitored.monitor.slope_score.is_some());
        assert!(monitored.monitor.squeeze_ready);
        assert!(monitored.monitor.entry_price.is_some());
        assert!(monitored.monitor.opened_at.is_some());
        assert!(monitored.monitor.mfe_points.unwrap() > 0.0);
        assert!(monitored.monitor.mae_points.unwrap() <= 0.0);
        assert!(monitored.monitor.open_points.is_some());
    }
}
