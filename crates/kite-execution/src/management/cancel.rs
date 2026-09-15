use anyhow::Result;
use kite_journal::{
    actions::{Change, validate_change},
    state::Order,
};
#[derive(Debug, PartialEq, Eq)]
pub struct CancelRequest {
    pub broker_id: String,
    pub variety: &'static str,
}
pub fn translate(order: &Order) -> Result<CancelRequest> {
    validate_change(order, &Change::Cancel)?;
    Ok(CancelRequest {
        broker_id: order.broker_id.clone().unwrap(),
        variety: "regular",
    })
}
