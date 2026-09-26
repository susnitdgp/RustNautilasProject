//! Compact Trend Ribbon runtime status output.
use super::{data, live_control::Control, trend_ribbon_actor::State};
use serde_json::Value;
use std::{sync::atomic::Ordering, time::Instant};

pub struct Display {
    started: Instant,
    seconds: u64,
    warmup: usize,
    sim: bool,
    real: bool,
    mock: bool,
    id: String,
    symbol: String,
    interval_minutes: u64,
    strategy_start_ns: u64,
    production_cutoff: Option<String>,
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
        let production_cutoff = if real {
            selection
                .execution_bounds(super::strategy_session::date(data::now()), true)
                .ok()
                .map(|(_, end)| format_ist(end))
        } else {
            None
        };
        eprintln!(
            "{} | {} | {}m | 1 lot\nFeed: {} | Execution: {} | REAL ORDERS: {}\nRun: {} | Limit: {}s | Ctrl-C: graceful stop",
            selection.strategy,
            selection.symbol,
            selection.interval_minutes(),
            if sim { "SYNTHETIC" } else { "KITE LIVE" },
            if real {
                "KITE PRODUCTION"
            } else if mock {
                "KITE MOCK"
            } else {
                "NAUTILUS SANDBOX"
            },
            if real { "ENABLED" } else { "OFF" },
            id,
            seconds
        );
        Self {
            started: Instant::now(),
            seconds,
            warmup,
            sim,
            real,
            mock,
            id: id.into(),
            symbol: selection.symbol.clone(),
            interval_minutes: selection.interval_minutes(),
            strategy_start_ns: u64::MAX,
            production_cutoff,
        }
    }

    pub fn with_strategy_start_ns(mut self, strategy_start_ns: u64) -> Self {
        self.strategy_start_ns = strategy_start_ns;
        self
    }

    pub fn render(&self, state: &State, control: &Control) {
        let now = data::now();
        let latest = state.indicators.last();
        let bar = u64_metric(latest, "bar_close_ns").unwrap_or(0);
        let direction = i64_metric(latest, "direction")
            .map(direction_name)
            .unwrap_or("WARMING");
        let position = state
            .cache
            .as_ref()
            .map(|cache| {
                let cache = cache.borrow();
                let qty: f64 = cache
                    .positions_open(None, None, None, None, None)
                    .iter()
                    .map(|p| p.signed_qty)
                    .sum();
                if qty == 0.0 {
                    "FLAT".to_string()
                } else if qty > 0.0 {
                    format!("LONG {qty}")
                } else {
                    format!("SHORT {}", qty.abs())
                }
            })
            .unwrap_or_else(|| "WAITING FOR CACHE".into());
        let open_orders = state
            .cache
            .as_ref()
            .map(|cache| {
                let cache = cache.borrow();
                cache.orders_open(None, None, None, None, None).len()
                    + cache.orders_inflight(None, None, None, None, None).len()
            })
            .unwrap_or(0);
        let quote = state.last_accepted_quote.as_ref();
        let price = quote
            .map(|q| format!("bid {} / ask {}", q.bid_price, q.ask_price))
            .unwrap_or_else(|| "waiting for quote".into());
        let signal = state
            .signals
            .last()
            .and_then(|v| v.get("intent"))
            .and_then(Value::as_str)
            .unwrap_or("none");
        let reason = state
            .signals
            .last()
            .and_then(|v| v.get("reason"))
            .and_then(Value::as_str)
            .unwrap_or("--");
        let phase = if control.fault.lock().expect("fault lock").is_some() {
            "REVIEW REQUIRED"
        } else if control.stopping.load(Ordering::Acquire) {
            "STOPPING"
        } else if control.paused.load(Ordering::Acquire) {
            "PAUSED / REBUILDING"
        } else if state.indicators.len() < self.warmup {
            "WARMING"
        } else {
            "MONITORING"
        };
        let elapsed = self.started.elapsed().as_secs();
        let remaining = self.seconds.saturating_sub(elapsed);
        let cutoff = self
            .production_cutoff
            .as_deref()
            .map(|v| format!(" | cutoff {v}"))
            .unwrap_or_default();
        eprintln!(
            "[{}] {} | {}s elapsed / {}s remaining{}\n  {} | Position: {} | Open/inflight: {}\n  Bar: {} | Direction: {} | Signals: {} [{} / {}] | Fills: {} | Recoveries: {}",
            format_ist(now),
            phase,
            elapsed,
            remaining,
            cutoff,
            price,
            position,
            open_orders,
            if bar > 0 {
                format_ist(bar)
            } else {
                "--".into()
            },
            direction,
            state.signals.len(),
            signal,
            reason,
            state.fills.len(),
            control.recoveries.load(Ordering::Acquire)
        );
    }

    #[allow(dead_code)]
    fn mode(&self) -> (&str, &str) {
        (
            if self.sim { "SYNTHETIC" } else { "KITE LIVE" },
            if self.real {
                "KITE PRODUCTION"
            } else if self.mock {
                "KITE MOCK"
            } else {
                "NAUTILUS SANDBOX"
            },
        )
    }

    #[allow(dead_code)]
    fn identity(&self) -> (&str, &str, u64, u64) {
        (
            &self.id,
            &self.symbol,
            self.interval_minutes,
            self.strategy_start_ns,
        )
    }
}

pub fn step(message: &str) {
    eprintln!("[STARTUP] {message}");
}

pub fn finish(clean: bool, folder: &std::path::Path, real: bool) {
    eprintln!(
        "{} | REAL ORDERS: {}\nReports: {}",
        if clean {
            "STOPPED CLEANLY"
        } else {
            "STOPPED - REVIEW REQUIRED"
        },
        if real { "ENABLED" } else { "OFF" },
        folder.display()
    );
}

fn u64_metric(value: Option<&Value>, key: &str) -> Option<u64> {
    value?.get(key)?.as_u64()
}

fn i64_metric(value: Option<&Value>, key: &str) -> Option<i64> {
    value?.get(key)?.as_i64()
}

fn direction_name(value: i64) -> &'static str {
    match value {
        1 => "LONG",
        -1 => "SHORT",
        _ => "FLAT",
    }
}

fn format_ist(ns: u64) -> String {
    chrono::DateTime::from_timestamp_nanos(ns as i64)
        .with_timezone(&chrono::FixedOffset::east_opt(19_800).expect("IST"))
        .format("%d-%m %H:%M:%S IST")
        .to_string()
}
