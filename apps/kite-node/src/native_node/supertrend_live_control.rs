//! Paper-only LiveNode control and freshness checks.
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
type Rebuild = Arc<Mutex<Option<(u64, Vec<nautilus_model::data::Bar>)>>>;
#[derive(Debug, Clone)]
pub struct Control {
    pub paused: Arc<AtomicBool>,
    pub online: Arc<AtomicBool>,
    pub epoch: Arc<AtomicU64>,
    pub recoveries: Arc<AtomicU64>,
    pub rebuild: Rebuild,
    pub order_deadline: Arc<AtomicU64>,
    pub bar_ns: u64,
    pub sim: bool,
    pub real: bool,
    pub recovery_fixture: bool,
    pub done: Arc<AtomicBool>,
    pub stopping: Arc<AtomicBool>,
    pub flat: Arc<AtomicBool>,
    pub fault: Arc<Mutex<Option<String>>>,
}
impl Control {
    pub fn new(sim: bool) -> Self {
        Self {
            paused: Arc::new(AtomicBool::new(false)),
            online: Arc::new(AtomicBool::new(sim)),
            epoch: Arc::new(AtomicU64::new(0)),
            recoveries: Arc::new(AtomicU64::new(0)),
            rebuild: Arc::new(Mutex::new(None)),
            order_deadline: Arc::new(AtomicU64::new(0)),
            bar_ns: 300_000_000_000,
            sim,
            real: false,
            recovery_fixture: false,
            done: Arc::new(AtomicBool::new(false)),
            stopping: Arc::new(AtomicBool::new(false)),
            flat: Arc::new(AtomicBool::new(true)),
            fault: Arc::new(Mutex::new(None)),
        }
    }
    pub fn with_bar_ns(mut self, bar_ns: u64) -> Self {
        assert!(bar_ns > 0, "bar interval must be positive");
        self.bar_ns = bar_ns;
        self
    }
    pub fn pause(&self) -> u64 {
        self.paused.store(true, Ordering::Release);
        self.epoch.fetch_add(1, Ordering::AcqRel) + 1
    }
    pub fn fail(&self, reason: &str) {
        *self.fault.lock().expect("fault lock") = Some(reason.into());
        self.stopping.store(true, Ordering::Release);
        self.done.store(true, Ordering::Release);
    }
    pub fn stop(&self) {
        self.stopping.store(true, Ordering::Release);
        self.done.store(true, Ordering::Release);
    }
    pub fn current_bar(&self, bar: u64, now: u64) -> bool {
        self.sim
            || (bar > 0
                && bar <= now
                && bar == now.saturating_sub(2_000_000_000) / self.bar_ns * self.bar_ns)
    }
    pub fn fresh_quote(&self, event: u64, received: u64, now: u64) -> bool {
        self.sim
            || (event <= now
                && received <= now
                && now - event <= 5_000_000_000
                && now - received <= 5_000_000_000)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_stale_future_quotes_and_old_completed_bars() {
        let c = Control::new(false);
        let now = 1_800_000_010_000_000_000;
        assert!(c.fresh_quote(now - 1, now, now));
        assert!(!c.fresh_quote(now - 6_000_000_000, now, now));
        assert!(!c.fresh_quote(now + 1, now, now));
        let last = (now - 2_000_000_000) / 300_000_000_000 * 300_000_000_000;
        assert!(c.current_bar(last, now));
        assert!(!c.current_bar(last - 300_000_000_000, now));
        let three = Control::new(false).with_bar_ns(180_000_000_000);
        let last_three = (now - 2_000_000_000) / three.bar_ns * three.bar_ns;
        assert!(three.current_bar(last_three, now));
        assert!(!three.current_bar(last_three - three.bar_ns, now));
        c.fail("gap");
        assert!(c.stopping.load(Ordering::Acquire));
        assert!(c.done.load(Ordering::Acquire));
    }
}
