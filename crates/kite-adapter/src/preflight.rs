use crate::{
    config::Config,
    instruments::{master, resolver},
    mapping::identity,
};
use anyhow::Result;
use chrono::NaiveDate;
use serde::Serialize;
use std::io::Read;

#[derive(Debug, Serialize)]
pub struct Report {
    pub mode: &'static str,
    pub instrument_id: String,
    pub instrument_token: u32,
    pub expiry: String,
    pub tick_size: String,
    pub broker_lot_size: u32,
    pub validation_date_ist: NaiveDate,
    pub live_orders_enabled: bool,
    pub contract_multiplier_verified: bool,
    pub engine_started: bool,
}

pub fn run(config: &Config, csv: impl Read, as_of: NaiveDate) -> Result<Report> {
    let rows = master::parse(csv)?;
    let row = resolver::resolve(&rows, config, as_of)?;
    Ok(Report {
        mode: "data_only",
        instrument_id: identity::instrument_id(row)?.to_string(),
        instrument_token: row.instrument_token,
        expiry: row.expiry.clone(),
        tick_size: row.tick_size.clone(),
        broker_lot_size: row.lot_size,
        validation_date_ist: as_of,
        live_orders_enabled: false,
        contract_multiplier_verified: false,
        engine_started: false,
    })
}
