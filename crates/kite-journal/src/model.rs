use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Intent {
    pub id: String,
    pub symbol: String,
    pub side: Side,
    pub product: Product,
    pub quantity: u32,
    pub limit_price_paise: i64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Buy,
    Sell,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Product {
    Mis,
    Nrml,
}
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_alphanumeric())
}
impl Intent {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            valid_id(&self.id),
            "Intent ID must be 1..20 alphanumeric characters"
        );
        ensure!(
            self.symbol == "CRUDEOIL26SEPFUT",
            "Unsupported simulation contract"
        );
        ensure!(
            (1..=100).contains(&self.quantity),
            "Simulation quantity must be 1..100 contracts"
        );
        ensure!(
            self.limit_price_paise > 0 && self.limit_price_paise % 100 == 0,
            "Limit price must be positive and on the one-rupee tick"
        );
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", deny_unknown_fields)]
pub enum Event {
    Intent {
        intent: Intent,
    },
    Dispatch {
        id: String,
    },
    Acknowledged {
        id: String,
        broker_id: String,
    },
    Unknown {
        id: String,
    },
    Rejected {
        id: String,
    },
    Fill {
        id: String,
        broker_id: String,
        trade_id: String,
        quantity: u32,
        price_paise: i64,
    },
    Cancelled {
        id: String,
        broker_id: String,
    },
}
impl Event {
    pub fn id(&self) -> &str {
        match self {
            Self::Intent { intent } => &intent.id,
            Self::Dispatch { id }
            | Self::Acknowledged { id, .. }
            | Self::Unknown { id }
            | Self::Rejected { id }
            | Self::Fill { id, .. }
            | Self::Cancelled { id, .. } => id,
        }
    }
}
