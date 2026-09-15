use anyhow::{Result, ensure};
#[derive(Clone, Debug)]
pub enum Command {
    Place {
        symbol: String,
        side: String,
        product: String,
        quantity: u32,
        price_rupees: i64,
        tag: String,
    },
    Modify {
        order_id: String,
        quantity: u32,
        price_rupees: i64,
    },
    Cancel {
        order_id: String,
    },
}
impl Command {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Place {
                symbol,
                side,
                product,
                quantity,
                price_rupees,
                tag,
            } => {
                ensure!(symbol == "CRUDEOIL26SEPFUT", "Unsupported order symbol");
                ensure!(
                    matches!(side.as_str(), "BUY" | "SELL")
                        && matches!(product.as_str(), "MIS" | "NRML"),
                    "Unsupported order side or product"
                );
                ensure!(
                    !tag.is_empty()
                        && tag.len() <= 20
                        && tag.bytes().all(|b| b.is_ascii_alphanumeric()),
                    "Invalid order tag"
                );
                terms(*quantity, *price_rupees)?;
            }
            Self::Modify {
                order_id,
                quantity,
                price_rupees,
            } => {
                broker_id(order_id)?;
                terms(*quantity, *price_rupees)?;
            }
            Self::Cancel { order_id } => broker_id(order_id)?,
        }
        Ok(())
    }
    pub(crate) fn wire(&self) -> (reqwest::Method, String, Vec<(&'static str, String)>) {
        match self {
            Self::Place {
                symbol,
                side,
                product,
                quantity,
                price_rupees,
                tag,
            } => (
                reqwest::Method::POST,
                "/orders/regular".into(),
                vec![
                    ("exchange", "MCX".into()),
                    ("tradingsymbol", symbol.clone()),
                    ("transaction_type", side.clone()),
                    ("product", product.clone()),
                    ("quantity", quantity.to_string()),
                    ("price", price_rupees.to_string()),
                    ("tag", tag.clone()),
                    ("order_type", "LIMIT".into()),
                    ("validity", "DAY".into()),
                ],
            ),
            Self::Modify {
                order_id,
                quantity,
                price_rupees,
            } => (
                reqwest::Method::PUT,
                format!("/orders/regular/{order_id}"),
                vec![
                    ("quantity", quantity.to_string()),
                    ("price", price_rupees.to_string()),
                    ("order_type", "LIMIT".into()),
                    ("validity", "DAY".into()),
                ],
            ),
            Self::Cancel { order_id } => (
                reqwest::Method::DELETE,
                format!("/orders/regular/{order_id}"),
                vec![],
            ),
        }
    }
    pub(crate) fn expected_id(&self) -> Option<&str> {
        match self {
            Self::Modify { order_id, .. } | Self::Cancel { order_id } => Some(order_id),
            _ => None,
        }
    }
}
fn terms(quantity: u32, price: i64) -> Result<()> {
    ensure!(
        (1..=100).contains(&quantity) && price > 0,
        "Invalid order quantity or price"
    );
    Ok(())
}
pub(crate) fn broker_id(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty() && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_digit()),
        "Invalid broker order ID"
    );
    Ok(())
}
