use nautilus_model::{data::QuoteTick, instruments::FuturesContract};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Record {
    Header {
        schema_version: u32,
        instrument: Box<FuturesContract>,
        instrument_token: u32,
    },
    Connected {
        generation: u32,
    },
    Gap {
        generation: u32,
    },
    Quote {
        quote: QuoteTick,
        generation: u32,
    },
    End {
        quotes: u64,
    },
}
impl Record {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Header { .. } => "header",
            Self::Connected { .. } => "connected",
            Self::Gap { .. } => "gap",
            Self::Quote { .. } => "quote",
            Self::End { .. } => "end",
        }
    }
}
