use super::{data, live_control::Control, smbc_actor::State};
use std::{sync::atomic::Ordering, time::Instant};
pub struct Display {
    started: Instant,
    seconds: u64,
}
impl Display {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        seconds: u64,
        _warmup: usize,
        sim: bool,
        id: &str,
        real: bool,
        mock_: bool,
        s: &super::production::Selection,
    ) -> Self {
        eprintln!(
            "{} | {} | {}m | run {} | Feed {} | Execution {} | REAL ORDERS {}",
            s.strategy,
            s.symbol,
            s.interval_minutes(),
            id,
            if sim { "SYNTHETIC" } else { "KITE LIVE" },
            if real {
                "KITE PRODUCTION"
            } else if mock_ {
                "KITE MOCK"
            } else {
                "NAUTILUS SANDBOX"
            },
            if real { "ENABLED" } else { "OFF" }
        );
        Self {
            started: Instant::now(),
            seconds,
        }
    }
    pub fn with_strategy_start_ns(self, _: u64) -> Self {
        self
    }
    pub fn render(&self, state: &State, control: &Control) {
        let elapsed = self.started.elapsed().as_secs();
        let pos = state
            .cache
            .as_ref()
            .map(|c| {
                c.borrow()
                    .positions_open(None, None, None, None, None)
                    .iter()
                    .map(|p| p.signed_qty)
                    .sum::<f64>()
            })
            .unwrap_or(0.0);
        let o = state.latest_strategy;
        let phase = if control.fault.lock().expect("fault").is_some() {
            "REVIEW"
        } else if control.paused.load(Ordering::Acquire) {
            "PAUSED"
        } else {
            "MONITORING"
        };
        eprintln!(
            "[{}] {} | {}s/{}s | pos {} | channels {} | bull {} bear {} | signals {} fills {}",
            fmt(data::now()),
            phase,
            elapsed,
            self.seconds,
            pos,
            o.map_or(0, |x| x.channel_count),
            o.is_some_and(|x| x.bullish_breakout),
            o.is_some_and(|x| x.bearish_breakout),
            state.signals.len(),
            state.fills.len()
        );
    }
}
pub fn step(m: &str) {
    eprintln!("[STARTUP] {m}");
}
pub fn finish(clean: bool, folder: &std::path::Path, real: bool) {
    eprintln!(
        "{} | REAL ORDERS {} | Reports {}",
        if clean {
            "STOPPED CLEANLY"
        } else {
            "REVIEW REQUIRED"
        },
        if real { "ENABLED" } else { "OFF" },
        folder.display()
    );
}
fn fmt(ns: u64) -> String {
    chrono::DateTime::from_timestamp_nanos(ns as i64)
        .with_timezone(&chrono::FixedOffset::east_opt(19_800).expect("IST"))
        .format("%d-%m %H:%M:%S IST")
        .to_string()
}
