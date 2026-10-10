//! Shared run control for live and paper runs: stop/fault flags read by the strategy,
//! the data feed and the runner's watcher, plus freshness checks.
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
/// Fault recorded when the Kite execution client stopped itself.
pub const ORDER_CLIENT_STOPPED: &str =
    "Kite order client stopped (order stream, session or reconciliation failure): \
     no more orders can be sent; check positions and open orders in Kite now";
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
    pub bar_feed: Arc<Mutex<super::bar_timing::Stats>>,
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
            bar_feed: Arc::new(Mutex::new(super::bar_timing::Stats::default())),
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
    /// Faults the run: the strategy flattens and the runner's watcher stops the node.
    /// The first reason is kept (later failures are usually consequences of it).
    pub fn fail(&self, reason: &str) {
        let mut fault = self.fault.lock().unwrap_or_else(|p| p.into_inner());
        if fault.is_none() {
            *fault = Some(reason.into());
        }
        drop(fault);
        self.stopping.store(true, Ordering::Release);
        self.done.store(true, Ordering::Release);
    }
    /// For the runner's watcher: why the run must stop now, if it must.
    /// * a recorded fault (feed gap, invalid packet, feed ended, …);
    /// * `done` set by someone other than [`Control::fail`]: the Kite execution client
    ///   gets `done` as its stop signal and sets it when it faults (order stream lost,
    ///   session expired, failed reconciliation). That is recorded as a fault here, so the
    ///   strategies (which watch `fault`) stop trading into a dead client.
    pub fn run_fault(&self) -> Option<String> {
        if let Some(reason) = self.fault.lock().unwrap_or_else(|p| p.into_inner()).clone() {
            return Some(reason);
        }
        if self.done.load(Ordering::Acquire) && !self.stopping.load(Ordering::Acquire) {
            self.fail(ORDER_CLIENT_STOPPED);
            return Some(ORDER_CLIENT_STOPPED.into());
        }
        None
    }
    pub fn stop(&self) {
        self.stopping.store(true, Ordering::Release);
        self.done.store(true, Ordering::Release);
    }
    /// Resolves with the reason once [`Control::run_fault`] reports one (polled every 250 ms).
    pub async fn wait_fault(&self) -> String {
        loop {
            if let Some(reason) = self.run_fault() {
                return reason;
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    }
    #[allow(dead_code)]
    pub fn current_bar(&self, bar: u64, now: u64) -> bool {
        self.sim
            || (bar > 0 && bar <= now && bar == super::bar_timing::eligible_close(now, self.bar_ns))
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
    #[test]
    fn first_fault_wins_and_run_fault_reports_it() {
        let c = Control::new(false);
        assert_eq!(c.run_fault(), None);
        c.fail("WebSocket feed gap");
        c.fail("Kite market-data feed stopped");
        assert_eq!(c.run_fault().as_deref(), Some("WebSocket feed gap"));
    }
    #[test]
    fn execution_client_stop_signal_becomes_a_fault() {
        let c = Control::new(false);
        // what the Kite execution client does when it faults: only `done`
        c.done.store(true, Ordering::Release);
        assert_eq!(c.run_fault().as_deref(), Some(ORDER_CLIENT_STOPPED));
        assert!(c.stopping.load(Ordering::Acquire), "strategies now see the stop");
        assert_eq!(
            c.fault.lock().unwrap().as_deref(),
            Some(ORDER_CLIENT_STOPPED),
            "strategies read `fault`"
        );
    }
    #[test]
    fn a_plain_stop_is_not_a_fault() {
        let c = Control::new(false);
        c.stop();
        assert_eq!(c.run_fault(), None);
    }
}
