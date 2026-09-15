use anyhow::Result;
use kite_journal::model::{Intent, Product, Side};
#[derive(Debug, PartialEq, Eq)]
pub struct LimitRequest {
    pub exchange: &'static str,
    pub tradingsymbol: String,
    pub transaction_type: &'static str,
    pub product: &'static str,
    pub order_type: &'static str,
    pub validity: &'static str,
    pub variety: &'static str,
    pub quantity: u32,
    pub price_rupees: i64,
    pub tag: String,
}
pub fn translate(intent: &Intent) -> Result<LimitRequest> {
    intent.validate()?;
    Ok(LimitRequest {
        exchange: "MCX",
        tradingsymbol: intent.symbol.clone(),
        transaction_type: match intent.side {
            Side::Buy => "BUY",
            Side::Sell => "SELL",
        },
        product: match intent.product {
            Product::Mis => "MIS",
            Product::Nrml => "NRML",
        },
        order_type: "LIMIT",
        validity: "DAY",
        variety: "regular",
        quantity: intent.quantity,
        price_rupees: intent.limit_price_paise / 100,
        tag: intent.id.clone(),
    })
}
