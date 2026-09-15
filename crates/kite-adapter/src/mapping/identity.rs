use crate::instruments::master::InstrumentRow;
use anyhow::Result;
use nautilus_model::identifiers::InstrumentId;

pub fn instrument_id(row: &InstrumentRow) -> Result<InstrumentId> {
    Ok(InstrumentId::from_as_ref(format!(
        "{}.{}",
        row.tradingsymbol, row.exchange
    ))?)
}
