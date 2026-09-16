//! Human-readable status on stderr; stdout remains machine-readable JSON.
use super::{data, supertrend_actor::State, supertrend_live_control::Control};
use std::{
    io::{self, Write},
    sync::atomic::Ordering,
    time::Instant,
};
pub struct Display {
    started: Instant,
    seconds: u64,
    warmup: usize,
}
impl Display {
    pub fn new(seconds: u64, warmup: usize, sim: bool, id: &str, real: bool, mock: bool) -> Self {
        let execution = if real {
            "KITE PRODUCTION"
        } else if mock {
            "KITE MOCK"
        } else {
            "SANDBOX"
        };
        let orders = if real { "ENABLED" } else { "OFF" };
        line(&format!(
            "SUPERTREND + MACD + VWAP | CRUDEOIL | 5m | 1 lot\nFeed: {} | Execution: {execution} | REAL ORDERS: {orders}\nRun: {id} | Limit: {seconds}s | Ctrl-C: graceful stop",
            if sim { "SYNTHETIC" } else { "KITE LIVE" }
        ));
        Self {
            started: Instant::now(),
            seconds,
            warmup,
        }
    }
    pub fn render(&self, s: &State, c: &Control) {
        let now = data::now();
        let bar = s
            .indicators
            .last()
            .and_then(|v| v["bar_close_ns"].as_u64())
            .unwrap_or(0);
        let fault = c.fault.lock().expect("fault lock").clone();
        let quote = s.last_accepted_quote.as_ref();
        let fresh =
            quote.is_some_and(|q| c.fresh_quote(q.ts_event.as_u64(), q.ts_init.as_u64(), now));
        let status = phase(
            s.started,
            s.stopped,
            c.stopping.load(Ordering::Acquire),
            fault.is_some(),
            s.indicators.len() >= self.warmup,
            fresh,
            c.current_bar(bar, now),
        );
        let status = if c.paused.load(Ordering::Acquire) && !c.stopping.load(Ordering::Acquire) {
            "PAUSED / REBUILDING HISTORY"
        } else {
            status
        };
        let elapsed = self.started.elapsed().as_secs();
        let (position, orders) = s
            .cache
            .as_ref()
            .map(|v| {
                let v = v.borrow();
                (
                    v.positions_open(None, None, None, None, None)
                        .iter()
                        .map(|p| p.signed_qty)
                        .sum::<f64>(),
                    v.orders_open(None, None, None, None, None).len()
                        + v.orders_inflight(None, None, None, None, None).len(),
                )
            })
            .unwrap_or((0., 0));
        let position = if s.cache.is_none() {
            "WAITING FOR CACHE".into()
        } else if position == 0. {
            "FLAT".into()
        } else if position > 0. {
            format!("LONG {position}")
        } else {
            format!("SHORT {}", position.abs())
        };
        let price = quote
            .map(|q| {
                format!(
                    "bid {} / ask {} | quote age {:.1}s",
                    q.bid_price,
                    q.ask_price,
                    now.saturating_sub(q.ts_event.as_u64()) as f64 / 1e9
                )
            })
            .unwrap_or_else(|| "waiting for valid quote".into());
        let intent = s
            .signals
            .last()
            .and_then(|v| v["intent"].as_str())
            .unwrap_or("none");
        line(&format!(
            "[{} IST] {status} | {elapsed}s elapsed / {}s remaining\n  {price} | Position: {position} | Open/inflight: {orders}\n  Bar close: {} IST | Bars: {} (+{} new) | Quotes: {} ({} rejected) | Signals: {} [{intent}] | Fills: {}",
            ist(now),
            self.seconds.saturating_sub(elapsed),
            ist(bar),
            s.indicators.len(),
            s.indicators.len().saturating_sub(self.warmup),
            s.live_quotes,
            s.rejected_quotes,
            s.signals.len(),
            s.fills.len()
        ));
        if let Some(reason) = fault {
            line(&format!("  STOP REASON: {}", clean(&reason)));
        }
    }
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
}
