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
    ProtectiveStopMarket {
        symbol: String,
        side: String,
        product: String,
        quantity: u32,
        trigger_price_rupees: i64,
        tag: String,
        market_protection: i32,
    },
    ModifyProtectiveStop {
        order_id: String,
        quantity: u32,
        trigger_price_rupees: i64,
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
            Self::ProtectiveStopMarket {
                symbol,
                side,
                product,
                quantity,
                trigger_price_rupees,
                tag,
                market_protection,
            } => {
                ensure!(
                    *market_protection == -1 || (1..=100).contains(market_protection),
                    "Invalid stop market protection"
                );
                Self::Place {
                    symbol: symbol.clone(),
                    side: side.clone(),
                    product: product.clone(),
                    quantity: *quantity,
                    price_rupees: *trigger_price_rupees,
                    tag: tag.clone(),
                }
                .validate()?;
            }
            Self::ModifyProtectiveStop {
                order_id,
                quantity,
                trigger_price_rupees,
                market_protection,
            } => {
                broker_id(order_id)?;
                ensure!(
                    *market_protection == -1 || (1..=100).contains(market_protection),
                    "Invalid stop market protection"
                );
                terms(*quantity, *trigger_price_rupees)?;
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
            Self::ProtectiveStopMarket {
                symbol,
                side,
                product,
                quantity,
                trigger_price_rupees,
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
                    ("trigger_price", trigger_price_rupees.to_string()),
                    ("tag", tag.clone()),
                    ("order_type", "SL-M".into()),
                    ("validity", "DAY".into()),
                    ("market_protection", market_protection.to_string()),
                ],
            ),
            Self::ModifyProtectiveStop {
                order_id,
                quantity,
                trigger_price_rupees,
                market_protection,
            } => (
                reqwest::Method::PUT,
                format!("/orders/regular/{order_id}"),
                vec![
                    ("quantity", quantity.to_string()),
                    ("trigger_price", trigger_price_rupees.to_string()),
                    ("order_type", "SL-M".into()),
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
            Self::Cancel { order_id } => (
                reqwest::Method::DELETE,
                format!("/orders/regular/{order_id}"),
                vec![],
            ),
        }
    }
    pub(crate) fn expected_id(&self) -> Option<&str> {
        match self {
            Self::ModifyProtectiveStop { order_id, .. }
            | Self::Cancel { order_id } => Some(order_id),
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

#[cfg(test)]
mod protective_stop_tests {
    use super::*;
    #[test]
    fn slm_place_and_modify_encode_trigger_without_limit_price() {
        let place = Command::ProtectiveStopMarket {
            symbol: "CRUDEOIL26OCTFUT".into(),
            side: "SELL".into(),
            product: "MIS".into(),
            quantity: 1,
            trigger_price_rupees: 8690,
            tag: "ILRCSTOP1".into(),
            market_protection: -1,
        };
        place.validate().unwrap();
        let (verb, path, fields) = place.wire();
        assert_eq!(verb, reqwest::Method::POST);
        assert_eq!(path, "/orders/regular");
        assert!(fields.contains(&("order_type", "SL-M".into())));
        assert!(fields.contains(&("trigger_price", "8690".into())));
        assert!(!fields.iter().any(|(k, _)| *k == "price"));
        let modify = Command::ModifyProtectiveStop {
            order_id: "123456789".into(),
            quantity: 1,
            trigger_price_rupees: 8700,
            market_protection: -1,
        };
        modify.validate().unwrap();
        let (verb, path, fields) = modify.wire();
        assert_eq!(verb, reqwest::Method::PUT);
        assert_eq!(path, "/orders/regular/123456789");
        assert!(fields.contains(&("trigger_price", "8700".into())));
    }
    #[test]
    fn invalid_stops_rejected() {
        let bad = Command::ProtectiveStopMarket {
            symbol: "CRUDEOIL26OCTFUT".into(),
            side: "SELL".into(),
            product: "MIS".into(),
            quantity: 0,
            trigger_price_rupees: 8690,
            tag: "ILRCSTOP1".into(),
            market_protection: -1,
        };
        assert!(bad.validate().is_err());
        let invalid = Command::ModifyProtectiveStop {
            order_id: "bad".into(),
            quantity: 1,
            trigger_price_rupees: 8700,
            market_protection: -1,
        };
        assert!(invalid.validate().is_err());
    }
}
