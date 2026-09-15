use crate::{
    http::authenticated::{Endpoint, ReadClient},
    orders::Order,
    positions::Positions,
    trades::Trade,
};
use anyhow::Result;
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Snapshot {
    pub orders: Vec<Order>,
    pub trades: Vec<Trade>,
    pub positions: Positions,
}
impl Snapshot {
    pub async fn read(client: &ReadClient, symbol: &str, token: u32) -> Result<Self> {
        let mut orders: Vec<Order> = client.get(Endpoint::Orders).await?;
        let mut trades: Vec<Trade> = client.get(Endpoint::Trades).await?;
        let mut positions: Positions = client.get(Endpoint::Positions).await?;
        // Include token OR symbol matches so an identity mismatch cannot disappear silently.
        orders.retain(|x| {
            x.instrument_token == token || (x.exchange == "MCX" && x.tradingsymbol == symbol)
        });
        trades.retain(|x| {
            x.instrument_token == token || (x.exchange == "MCX" && x.tradingsymbol == symbol)
        });
        for rows in [&mut positions.net, &mut positions.day] {
            rows.retain(|x| {
                x.instrument_token == token || (x.exchange == "MCX" && x.tradingsymbol == symbol)
            });
            rows.sort();
        }
        orders.sort();
        trades.sort();
        Ok(Self {
            orders,
            trades,
            positions,
        })
    }
}
