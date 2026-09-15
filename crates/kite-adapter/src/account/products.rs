//! Product buckets remain distinct; profile permission is not per-instrument eligibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Product {
    Intraday,
    CarryForward,
}
impl Product {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "MIS" => Some(Self::Intraday),
            "NRML" => Some(Self::CarryForward),
            _ => None,
        }
    }
}
