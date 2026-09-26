//! Read-only terminal trade ledger for Trend Ribbon replay.
//!
//! No charts, no execution client, and no Redis trading-state writes.
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
    time::{Duration, Instant},
};

use super::trend_ribbon_backtest::Event as StrategyEvent;

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

struct ReplayApp {
    instrument: String,
    date: NaiveDate,
    frames: Vec<ReplayFrame>,
    index: usize,
    playing: bool,
    delay: Duration,
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
    let report = super::trend_ribbon_backtest::simulate(
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
        frames.push(ReplayFrame {
            timestamp,
            close: candle.close,
            events_so_far: events_so_far.clone(),
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
        "wt_long_exit" => "WT LX",
        "wt_short_exit" => "WT SX",
        "session_end" => "SQ OFF",
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
    let backend = TestBackend::new(142, 28);
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
            Constraint::Length(4),
            Constraint::Min(12),
            Constraint::Length(5),
            Constraint::Length(2),
        ])
        .split(frame.area());

    render_header(frame, app, root[0]);
    render_trade_table(frame, app, root[1]);
    render_totals(frame, app, root[2]);
    render_footer(frame, app, root[3]);
}

fn render_header(frame: &mut ratatui::Frame<'_>, app: &ReplayApp, area: Rect) {
    let current = app.current();
    let mode = if app.playing { "PLAY" } else { "PAUSED" };
    let title = vec![
        Line::from(vec![
            Span::styled(
                " TREND RIBBON v2.10 - TRADE LEDGER ",
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
        " q quit | space play/pause | ←/→ step | Home/End | +/- speed   delay={}ms   NO EXECUTION ",
        app.delay.as_millis()
    );
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(Color::DarkGray)),
        area,
    );
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
                " TREND RIBBON v2.10 - DAILY PERFORMANCE ",
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
        assert_eq!(summary.days[0].trades, 6);
        assert_eq!(summary.days[0].wins, 4);
        assert_eq!(summary.days[0].losses, 2);
        assert_eq!(summary.days[0].points, 170.0);
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

        assert_eq!(closed.len(), 6);
        assert_eq!(total, 170.0);
        assert_eq!(closed[0].side, "LONG");
        assert_eq!(closed[0].entry_price, 8925.0);
        assert_eq!(closed[0].exit_price, Some(8940.0));
        assert_eq!(
            closed.last().unwrap().exit_reason.as_deref(),
            Some("SQ OFF")
        );
    }
}
