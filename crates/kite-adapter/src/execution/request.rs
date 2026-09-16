use anyhow::{Result, ensure};
#[derive(Clone, Debug)]
pub enum Command {
    ProtectedMarket {
        symbol: String,
        side: String,
        product: String,
        quantity: u32,
        tag: String,
        market_protection: i32,
    },
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
            Self::ProtectedMarket {
                symbol,
                side,
                product,
                quantity,
                tag,
                market_protection,
            } => {
                ensure!(
                    *market_protection == -1 || (1..=100).contains(market_protection),
                    "Market protection must be auto (-1) or 1..100 percent; unprotected orders are forbidden"
                );
                Self::Place {
                    symbol: symbol.clone(),
                    side: side.clone(),
                    product: product.clone(),
                    quantity: *quantity,
                    price_rupees: 1,
                    tag: tag.clone(),
                }
                .validate()?;
            }
            Self::Place {
                symbol,
                side,
                product,
                quantity,
                price_rupees,
                tag,
            } => {
                crate::instruments::contract::validate_symbol(symbol)?;
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
            Self::ProtectedMarket {
                symbol,
                side,
                product,
                quantity,
                tag,
                market_protection,
            } => (
                reqwest::Method::POST,
                "/orders/regular".into(),
                vec![
                    ("exchange", "MCX".into()),
                    ("tradingsymbol", symbol.clone()),
                    ("transaction_type", side.clone()),
                    ("product", product.clone()),
                    ("quantity", quantity.to_string()),
                    ("tag", tag.clone()),
                    ("order_type", "MARKET".into()),
                    ("validity", "DAY".into()),
                    ("market_protection", market_protection.to_string()),
                ],
            ),
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

#[cfg(test)]
mod protected_market_tests {
    use super::*;
    #[test]
    fn market_protection_is_transmitted_without_a_limit_price() {
        let make = |market_protection| Command::ProtectedMarket {
            symbol: "CRUDEOIL26SEPFUT".into(),
            side: "BUY".into(),
            product: "NRML".into(),
            quantity: 1,
            tag: "TestMarket1".into(),
            market_protection,
        };
        let order = make(-1);
        order.validate().unwrap();
        let (method, path, fields) = order.wire();
        assert_eq!(method, reqwest::Method::POST);
        assert_eq!(path, "/orders/regular");
        assert!(fields.contains(&("order_type", "MARKET".into())));
        assert!(fields.contains(&("market_protection", "-1".into())));
        assert!(!fields.iter().any(|(key, _)| *key == "price"));
        for value in [0, -2, 101] {
            assert!(make(value).validate().is_err());
        }
    }
}
