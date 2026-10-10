//! Shared run control for live and paper runs: stop/fault flags read by the strategy,
//! the data feed and the runner's watcher.
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
/// Fault recorded when the Kite execution client stopped itself.
pub const ORDER_CLIENT_STOPPED: &str =
    "Kite order client stopped (order stream, session or reconciliation failure): \
     no more orders can be sent; check positions and open orders in Kite now";
#[derive(Debug, Clone)]
pub struct Control {
    pub done: Arc<AtomicBool>,
    pub stopping: Arc<AtomicBool>,
    pub flat: Arc<AtomicBool>,
    pub fault: Arc<Mutex<Option<String>>>,
}
impl Control {
    pub fn new() -> Self {
        Self {
            done: Arc::new(AtomicBool::new(false)),
            stopping: Arc::new(AtomicBool::new(false)),
            flat: Arc::new(AtomicBool::new(true)),
            fault: Arc::new(Mutex::new(None)),
        }
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
    /// Resolves with the reason once [`Control::run_fault`] reports one (polled every 250 ms).
    pub async fn wait_fault(&self) -> String {
        loop {
            if let Some(reason) = self.run_fault() {
                return reason;
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_fault_stops_the_run() {
        let c = Control::new();
        c.fail("gap");
        assert!(c.stopping.load(Ordering::Acquire));
        assert!(c.done.load(Ordering::Acquire));
    }
    #[test]
    fn first_fault_wins_and_run_fault_reports_it() {
        let c = Control::new();
        assert_eq!(c.run_fault(), None);
        c.fail("WebSocket feed gap");
        c.fail("Kite market-data feed stopped");
        assert_eq!(c.run_fault().as_deref(), Some("WebSocket feed gap"));
    }
    #[test]
    fn execution_client_stop_signal_becomes_a_fault() {
        let c = Control::new();
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
}
