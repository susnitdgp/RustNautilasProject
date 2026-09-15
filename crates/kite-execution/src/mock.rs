use crate::translation::LimitRequest;
#[derive(Clone)]
pub enum Outcome {
    RateLimited { retry_after_ms: u64 },
    Accepted(String),
    AmbiguousTimeout,
    Rejected,
}
pub struct MockBroker {
    pub calls: usize,
    outcome: Outcome,
}
impl MockBroker {
    pub fn new(outcome: Outcome) -> Self {
        Self { calls: 0, outcome }
    }
    pub(crate) fn submit(&mut self, _request: &LimitRequest) -> Outcome {
        self.calls += 1;
        self.outcome.clone()
    }
}
