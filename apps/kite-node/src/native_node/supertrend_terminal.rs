//! Real-time terminal dashboard on stderr; stdout remains machine-readable JSON.
use super::{data, supertrend_actor::State, supertrend_live_control::Control};
use serde_json::Value;
use std::{
    io::{self, IsTerminal, Write},
    sync::atomic::Ordering,
    time::Instant,
};

const PARAMETERS: &str = "Supertrend ATR(7) Wilder x 2";
const ENTRY_RULE: &str = "Supertrend direction + matching MACD/VWAP confirmation";
const EXIT_RULE: &str = "opposite Supertrend; stop-loss disabled";
const DASHBOARD_INNER_WIDTH: usize = 84;
const DASHBOARD_VALUE_WIDTH: usize = 69;

pub struct Display {
    started: Instant,
    seconds: u64,
    warmup: usize,
    sim: bool,
    real: bool,
    mock: bool,
    id: String,
    symbol: String,
    pivot: Option<super::pivot_point::Settings>,
    dashboard: bool,
}

impl Display {
    pub fn new(
        seconds: u64,
        warmup: usize,
        sim: bool,
        id: &str,
        real: bool,
        mock: bool,
        selection: &super::production::Selection,
    ) -> Self {
        let dashboard = io::stderr().is_terminal()
            && std::env::var("KITE_TERMINAL_DASHBOARD").as_deref() != Ok("0");
        let display = Self {
            started: Instant::now(),
            seconds,
            warmup,
            sim,
            real,
            mock,
            id: id.into(),
            symbol: selection.symbol.clone(),
            pivot: selection.pivot_point.clone(),
            dashboard,
        };
        if !dashboard {
            line(&format!(
                "{} | {} | 5m | 1 lot\nFeed: {} | Execution: {} | REAL ORDERS: {}\nRun: {id} | Limit: {seconds}s | Ctrl-C: graceful stop",
                selection.strategy,
                display.symbol,
                display.feed(),
                display.execution(),
                display.orders()
            ));
        }
        display
    }

    pub fn render(&self, s: &State, c: &Control) {
        let snapshot = Snapshot::new(self, s, c);
        if self.dashboard {
            let mut stderr = io::stderr().lock();
            let _ = write!(stderr, "\x1b[2J\x1b[H{}", snapshot.dashboard(self));
            let _ = stderr.flush();
        } else {
            line(&snapshot.lines(self));
        }
    }

    fn feed(&self) -> &'static str {
        if self.sim { "SYNTHETIC" } else { "KITE LIVE" }
    }

    fn execution(&self) -> &'static str {
        if self.real {
            "KITE PRODUCTION"
        } else if self.mock {
            "KITE MOCK"
        } else {
            "NAUTILUS SANDBOX"
        }
    }

    fn orders(&self) -> &'static str {
        if self.real { "ENABLED" } else { "OFF" }
    }
}

struct Snapshot {
    now: u64,
    bar: u64,
    phase: &'static str,
    fault: Option<String>,
    elapsed: u64,
    position: String,
    orders: usize,
    price: String,
    bid: Option<f64>,
    ask: Option<f64>,
    quote_age: Option<f64>,
    intent: String,
    reason: String,
    close: Option<f64>,
    atr: Option<f64>,
    supertrend: Option<f64>,
    direction: Option<i64>,
    vwap: Option<f64>,
    macd: Option<f64>,
    macd_signal: Option<f64>,
    confirmation: Option<i64>,
    confirmation_ready: bool,
    deadline: u64,
    bars: usize,
    quotes: u64,
    rejected: u64,
    signals: usize,
    fills: usize,
    online: bool,
    paused: bool,
    recoveries: u64,
}

impl Snapshot {
    fn new(display: &Display, s: &State, c: &Control) -> Self {
        let now = data::now();
        let latest = s.indicators.last();
        let bar = u64_metric(latest, "bar_close_ns").unwrap_or(0);
        let fault = c.fault.lock().expect("fault lock").clone();
        let quote = s.last_accepted_quote.as_ref();
        let fresh =
            quote.is_some_and(|q| c.fresh_quote(q.ts_event.as_u64(), q.ts_init.as_u64(), now));
        let mut phase = phase(
            s.started,
            s.stopped,
            c.stopping.load(Ordering::Acquire),
            fault.is_some(),
            s.indicators.len() >= display.warmup,
            fresh,
            c.current_bar(bar, now),
        );
        let paused = c.paused.load(Ordering::Acquire);
        if paused && !c.stopping.load(Ordering::Acquire) {
            phase = "PAUSED / REBUILDING HISTORY";
        }
        let (quantity, orders) = s
            .cache
            .as_ref()
            .map(|cache| {
                let cache = cache.borrow();
                (
                    cache
                        .positions_open(None, None, None, None, None)
                        .iter()
                        .map(|p| p.signed_qty)
                        .sum::<f64>(),
                    cache.orders_open(None, None, None, None, None).len()
                        + cache.orders_inflight(None, None, None, None, None).len(),
                )
            })
            .unwrap_or((0., 0));
        let position = if s.cache.is_none() {
            "WAITING FOR CACHE".into()
        } else if quantity == 0. {
            "FLAT".into()
        } else if quantity > 0. {
            format!("LONG {quantity}")
        } else {
            format!("SHORT {}", quantity.abs())
        };
        let (price, bid, ask, quote_age) = quote
            .map(|q| {
                let bid = q.bid_price.as_f64();
                let ask = q.ask_price.as_f64();
                let age = now.saturating_sub(q.ts_event.as_u64()) as f64 / 1e9;
                (
                    format!(
                        "bid {} / ask {} | quote age {age:.1}s",
                        q.bid_price, q.ask_price
                    ),
                    Some(bid),
                    Some(ask),
                    Some(age),
                )
            })
            .unwrap_or_else(|| ("waiting for valid quote".into(), None, None, None));
        let signal = s.signals.last();
        let intent = text_metric(signal, "intent").unwrap_or("none").to_string();
        let reason = text_metric(signal, "reason").unwrap_or("--").to_string();
        Self {
            now,
            bar,
            phase,
            fault,
            elapsed: display.started.elapsed().as_secs(),
            position,
            orders,
            price,
            bid,
            ask,
            quote_age,
            intent,
            reason,
            close: f64_metric(latest, "close"),
            atr: f64_metric(latest, "atr"),
            supertrend: f64_metric(latest, "supertrend"),
            direction: i64_metric(latest, "direction"),
            vwap: nested_f64(latest, "confirmation", "vwap"),
            macd: nested_f64(latest, "confirmation", "macd"),
            macd_signal: nested_f64(latest, "confirmation", "macd_signal"),
            confirmation: nested_i64(latest, "confirmation", "confirmation_direction"),
            confirmation_ready: nested_bool(latest, "confirmation", "ready").unwrap_or(false),
            deadline: c.order_deadline.load(Ordering::Acquire),
            bars: s.indicators.len(),
            quotes: s.live_quotes,
            rejected: s.rejected_quotes,
            signals: s.signals.len(),
            fills: s.fills.len(),
            online: c.online.load(Ordering::Acquire),
            paused,
            recoveries: c.recoveries.load(Ordering::Acquire),
        }
    }

    fn dashboard(&self, display: &Display) -> String {
        let spread = match (self.bid, self.ask) {
            (Some(bid), Some(ask)) => format!("{:.2}", ask - bid),
            _ => "--".into(),
        };
        let age = self
            .quote_age
            .map(|v| format!("{v:.1}s"))
            .unwrap_or_else(|| "--".into());
        let pending = if self.deadline > self.now {
            format!(
                "fill deadline {:.1}s",
                (self.deadline - self.now) as f64 / 1e9
            )
        } else if self.orders > 0 {
            "broker update pending".into()
        } else {
            "none".into()
        };
        let safety = if self.fault.is_some() {
            "REVIEW REQUIRED"
        } else if self.paused {
            "PAUSED"
        } else {
            "OK"
        };
        let fault = self
            .fault
            .as_deref()
            .map(clean)
            .unwrap_or_else(|| "none".into());
        let order_mode = if display.real {
            "MARKET / MIS / DAY | market protection -1"
        } else {
            "simulated MARKET / DAY"
        };

        let title = format!(
            "{} LIVE STRATEGY | {} IST",
            display.symbol,
            ist_title(self.now)
        );
        let mut output = heading('┌', '┐', &title);
        output.push_str(&row("Status", self.phase));
        output.push_str(&row("Run", &display.id));
        output.push_str(&section("Configuration"));
        output.push_str(&row(
            "Feed / exec",
            &format!("{} | {}", display.feed(), display.execution()),
        ));
        output.push_str(&row(
            "Orders",
            &format!("{} | {order_mode}", display.orders()),
        ));
        output.push_str(&row(
            "Instrument",
            &format!("{} | 5-minute | 1 lot", display.symbol),
        ));
        if let Some(p) = &display.pivot {
            output.push_str(&row(
                "Strategy",
                &format!(
                    "Pivot Point ST({}) | ATR({}) x {}",
                    p.pivot_period, p.atr_period, p.atr_factor
                ),
            ));
            output.push_str(&row(
                "Session",
                &format!(
                    "{}-{} IST | days {} | reset {}",
                    p.session.start, p.session.end, p.session.days, p.session.reset_daily
                ),
            ));
            output.push_str(&row(
                "Entry",
                "Confirmed Pivot Point Supertrend flip; no MACD/VWAP filter",
            ));
            output.push_str(&row("Exit", "Opposite flip or timed session square-off"));
        } else {
            output.push_str(&row("Strategy", PARAMETERS));
            output.push_str(&row("Confirmation", "MACD EMA(12,26,9) + session VWAP"));
            output.push_str(&row("Entry", ENTRY_RULE));
            output.push_str(&row("Exit", EXIT_RULE));
        }
        output.push_str(&section("Live market and indicators"));
        output.push_str(&row("Quote", &self.price));
        output.push_str(&row(
            "Market",
            &format!(
                "spread {spread} | age {age} | bar {} | next candle {}",
                ist(self.bar),
                candle_countdown(self.now)
            ),
        ));
        output.push_str(&row(
            "Price / ST",
            &format!(
                "close {} | line {} | ATR {}",
                show(self.close),
                show(self.supertrend),
                show(self.atr)
            ),
        ));
        if display.pivot.is_some() {
            output.push_str(&row(
                "Direction",
                &format!("Pivot Point Supertrend {}", direction(self.direction)),
            ));
        } else {
            output.push_str(&row(
                "Direction",
                &format!(
                    "Supertrend {} | confirmation {} | ready {}",
                    direction(self.direction),
                    direction(self.confirmation),
                    self.confirmation_ready
                ),
            ));
            output.push_str(&row(
                "Confirmation",
                &format!(
                    "VWAP {} | MACD {} | signal {}",
                    show(self.vwap),
                    show(self.macd),
                    show(self.macd_signal)
                ),
            ));
        }
        output.push_str(&section("Position and safety"));
        output.push_str(&row(
            "Position",
            &format!("{} | open/inflight {}", self.position, self.orders),
        ));
        output.push_str(&row(
            "Last signal",
            &format!(
                "{} | reason {} | pending {pending}",
                self.intent, self.reason
            ),
        ));
        output.push_str(&row(
            "Totals",
            &format!(
                "bars {} | quotes {} | rejected {} | signals {} | fills {}",
                self.bars, self.quotes, self.rejected, self.signals, self.fills
            ),
        ));
        output.push_str(&row(
            "Feed state",
            &format!(
                "online {} | paused {} | recoveries {} | safety {safety}",
                self.online, self.paused, self.recoveries
            ),
        ));
        output.push_str(&row(
            "Runtime",
            &format!(
                "{}s elapsed | {}s remaining",
                self.elapsed,
                display.seconds.saturating_sub(self.elapsed)
            ),
        ));
        output.push_str(&row("Fault", &fault));
        output.push_str(&heading(
            '└',
            '┘',
            "Ctrl-C: graceful square-off; verify FLAT in Kite",
        ));
        output
    }

    fn lines(&self, display: &Display) -> String {
        let mut output = format!(
            "[{} IST] {} | {}s elapsed / {}s remaining\n  {} | Position: {} | Open/inflight: {}\n  Bar close: {} IST | Bars: {} (+{} new) | Quotes: {} ({} rejected) | Signals: {} [{}] | Fills: {}",
            ist(self.now),
            self.phase,
            self.elapsed,
            display.seconds.saturating_sub(self.elapsed),
            self.price,
            self.position,
            self.orders,
            ist(self.bar),
            self.bars,
            self.bars.saturating_sub(display.warmup),
            self.quotes,
            self.rejected,
            self.signals,
            self.intent,
            self.fills
        );
        if let Some(reason) = &self.fault {
            output.push_str(&format!("\n  STOP REASON: {}", clean(reason)));
        }
        output
    }
}

fn heading(left: char, right: char, title: &str) -> String {
    let title = clean(title);
    let prefix = format!("─ {title} ");
    let fill = "─".repeat(DASHBOARD_INNER_WIDTH.saturating_sub(prefix.chars().count()));
    format!("{left}{prefix}{fill}{right}\n")
}

fn section(title: &str) -> String {
    heading('├', '┤', title)
}

fn row(label: &str, value: &str) -> String {
    let label = truncate(label, 12);
    let value = truncate(value, DASHBOARD_VALUE_WIDTH);
    format!("│ {label:<12} {value:<69} │\n")
}

fn phase(
    started: bool,
    stopped: bool,
    stopping: bool,
    fault: bool,
    warm: bool,
    fresh: bool,
    bar: bool,
) -> &'static str {
    if fault {
        "REVIEW REQUIRED"
    } else if stopped {
        "STOPPED"
    } else if stopping {
        "DRAINING / CLOSING POSITION"
    } else if !started {
        "CONNECTING"
    } else if !warm {
        "WARMING UP"
    } else if !fresh {
        "WAITING FOR FRESH QUOTES"
    } else if !bar {
        "WAITING FOR COMPLETED BAR"
    } else {
        "MONITORING SIGNALS"
    }
}

fn f64_metric(v: Option<&Value>, key: &str) -> Option<f64> {
    v.and_then(|v| v[key].as_f64())
}
fn i64_metric(v: Option<&Value>, key: &str) -> Option<i64> {
    v.and_then(|v| v[key].as_i64())
}
fn u64_metric(v: Option<&Value>, key: &str) -> Option<u64> {
    v.and_then(|v| v[key].as_u64())
}
fn text_metric<'a>(v: Option<&'a Value>, key: &str) -> Option<&'a str> {
    v.and_then(|v| v[key].as_str())
}
fn nested_f64(v: Option<&Value>, parent: &str, key: &str) -> Option<f64> {
    v.and_then(|v| v[parent][key].as_f64())
}
fn nested_i64(v: Option<&Value>, parent: &str, key: &str) -> Option<i64> {
    v.and_then(|v| v[parent][key].as_i64())
}
fn nested_bool(v: Option<&Value>, parent: &str, key: &str) -> Option<bool> {
    v.and_then(|v| v[parent][key].as_bool())
}

fn show(v: Option<f64>) -> String {
    v.map(|v| format!("{v:.2}")).unwrap_or_else(|| "--".into())
}
fn direction(v: Option<i64>) -> &'static str {
    match v {
        Some(1) => "LONG",
        Some(-1) => "SHORT",
        Some(0) => "NEUTRAL",
        _ => "--",
    }
}
fn truncate(value: &str, width: usize) -> String {
    let clean = clean(value);
    clean.chars().take(width).collect()
}
fn candle_countdown(now: u64) -> String {
    const FIVE_MINUTES_NS: u64 = 300_000_000_000;
    const ONE_SECOND_NS: u64 = 1_000_000_000;
    let remaining_ns = FIVE_MINUTES_NS - now % FIVE_MINUTES_NS;
    let seconds = remaining_ns.div_ceil(ONE_SECOND_NS);
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

fn ist_title(ts: u64) -> String {
    chrono::DateTime::from_timestamp_nanos(ts as i64)
        .with_timezone(&chrono::FixedOffset::east_opt(19800).unwrap())
        .format("%d-%m-%Y %H:%M:%S")
        .to_string()
}

fn ist(ts: u64) -> String {
    if ts == 0 {
        return "--".into();
    }
    chrono::DateTime::from_timestamp_nanos(ts as i64)
        .with_timezone(&chrono::FixedOffset::east_opt(19800).unwrap())
        .format("%d-%m %H:%M:%S")
        .to_string()
}
fn clean(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}
fn line(s: &str) {
    let _ = writeln!(io::stderr().lock(), "{s}");
}
pub fn step(s: &str) {
    line(&format!("[STARTUP] {s}"));
}
pub fn finish(clean: bool, folder: &std::path::Path, real: bool) {
    let orders = if real { "ENABLED FOR THIS RUN" } else { "OFF" };
    line(&format!(
        "{} | REAL ORDERS: {orders}\nReports: {}",
        if clean {
            "STOPPED CLEANLY"
        } else {
            "STOPPED - REVIEW REQUIRED"
        },
        folder.display()
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn faults_and_shutdown_take_priority_over_ready_market_data() {
        assert_eq!(
            phase(true, false, false, false, true, true, true),
            "MONITORING SIGNALS"
        );
        assert_eq!(
            phase(true, false, false, false, true, false, true),
            "WAITING FOR FRESH QUOTES"
        );
        assert_eq!(
            phase(true, false, false, false, true, true, false),
            "WAITING FOR COMPLETED BAR"
        );
        assert_eq!(
            phase(true, false, true, false, true, true, true),
            "DRAINING / CLOSING POSITION"
        );
        assert_eq!(
            phase(true, true, true, true, true, true, true),
            "REVIEW REQUIRED"
        );
        assert_eq!(clean("bad\n\u{1b}[2J"), "bad  [2J");
    }
    #[test]
    fn dashboard_parameters_match_reviewed_strategy() {
        assert!(PARAMETERS.contains("ATR(7)"));
        assert_eq!(PARAMETERS, "Supertrend ATR(7) Wilder x 2");
        assert!(ENTRY_RULE.contains("MACD/VWAP"));
        assert!(EXIT_RULE.contains("stop-loss disabled"));
        assert_eq!(row("Status", "OK").chars().count(), 87);
        assert_eq!(direction(Some(-1)), "SHORT");
        assert_eq!(truncate("123456", 4), "1234");
        assert_eq!(candle_countdown(300_000_000_000), "05:00");
        assert_eq!(candle_countdown(60_000_000_000), "04:00");
        assert_eq!(candle_countdown(299_200_000_000), "00:01");
        assert_eq!(ist_title(0), "01-01-1970 05:30:00");
    }
}
