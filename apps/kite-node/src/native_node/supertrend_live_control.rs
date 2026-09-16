//! Paper-only LiveNode control and freshness checks.
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
#[derive(Debug, Clone)]
pub struct Control {
    pub sim: bool,
    pub done: Arc<AtomicBool>,
    pub stopping: Arc<AtomicBool>,
    pub flat: Arc<AtomicBool>,
    pub fault: Arc<Mutex<Option<String>>>,
}
impl Control {
    pub fn new(sim: bool) -> Self {
        Self {
            sim,
            done: Arc::new(AtomicBool::new(false)),
            stopping: Arc::new(AtomicBool::new(false)),
            flat: Arc::new(AtomicBool::new(true)),
            fault: Arc::new(Mutex::new(None)),
        }
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
                && bar == now.saturating_sub(2_000_000_000) / 300_000_000_000 * 300_000_000_000)
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
        c.fail("gap");
        assert!(c.stopping.load(Ordering::Acquire));
        assert!(c.done.load(Ordering::Acquire));
    }
}
