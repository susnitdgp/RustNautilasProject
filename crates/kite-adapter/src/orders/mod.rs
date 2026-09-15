use serde::Deserialize;
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
pub(crate) struct Order {
    pub order_id: String,
    pub exchange: String,
    pub tradingsymbol: String,
    pub instrument_token: u32,
    pub product: String,
    pub transaction_type: String,
    pub status: String,
    pub quantity: i64,
    pub filled_quantity: i64,
    pub pending_quantity: i64,
    pub cancelled_quantity: i64,
}
impl Order {
    pub fn terminal(&self) -> bool {
        matches!(self.status.as_str(), "COMPLETE" | "CANCELLED" | "REJECTED")
    }
}
