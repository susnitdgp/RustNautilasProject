use anyhow::Result;
use kite_journal::{
    actions::{Change, validate_change},
    state::Order,
};
#[derive(Debug, PartialEq, Eq)]
pub struct ModifyRequest {
    pub broker_id: String,
    pub variety: &'static str,
    pub quantity: u32,
    pub price_rupees: i64,
    pub order_type: &'static str,
    pub validity: &'static str,
}
pub fn translate(order: &Order, quantity: u32, limit_price_paise: i64) -> Result<ModifyRequest> {
    validate_change(
        order,
        &Change::Modify {
            quantity,
            limit_price_paise,
        },
    )?;
    Ok(ModifyRequest {
        broker_id: order.broker_id.clone().unwrap(),
        variety: "regular",
        quantity,
        price_rupees: limit_price_paise / 100,
        order_type: "LIMIT",
        validity: "DAY",
    })
}
