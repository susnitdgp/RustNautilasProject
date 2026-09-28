//! Compact Pure Squeeze Momentum runtime status output.
use super::{data, live_control::Control, squeeze_momentum_actor::State};
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
    #[allow(clippy::too_many_arguments)]
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
        let latest = state.latest_strategy;
        let bar = latest.map_or(0, |v| v.bar_close_ns);
        let sqz = latest.map_or("WARMUP", |v| v.momentum_state.label());
        let bar_status = latest.map_or("WARMUP", |v| {
            if v.confirmed {
                "CONFIRMED"
            } else {
                "LIVE/FORMING"
            }
        });
        let position = state
            .cache
            .as_ref()
            .map(|cache| {
                let qty: f64 = cache
                    .borrow()
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
        let price = state
            .last_accepted_quote
            .as_ref()
            .map(|q| format!("bid {} / ask {}", q.bid_price, q.ask_price))
            .unwrap_or_else(|| "waiting for quote".into());
        let signal = state
            .signals
            .last()
            .and_then(|v| v.get("intent"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("none");
        let reason = state
            .signals
            .last()
            .and_then(|v| v.get("reason"))
            .and_then(serde_json::Value::as_str)
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
            "[{}] {} | {}s elapsed / {}s remaining{}\n  {} | Position: {} | Open/inflight: {}\n  Bar: {} | {} | SQZ: {} | Signals: {} [{} / {}] | Fills: {} | Recoveries: {}",
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
            bar_status,
            sqz,
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

fn format_ist(ns: u64) -> String {
    chrono::DateTime::from_timestamp_nanos(ns as i64)
        .with_timezone(&chrono::FixedOffset::east_opt(19_800).expect("IST"))
        .format("%d-%m %H:%M:%S IST")
        .to_string()
}
