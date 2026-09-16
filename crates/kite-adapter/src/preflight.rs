use crate::{
    config::Config,
    instruments::{master, resolver},
    mapping::identity,
};
use anyhow::{Context, Result, ensure};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::Serialize;
use std::{io::Read, str::FromStr};

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
    report(row, as_of)
}

/// Resolves one exact live contract from the JSON-selected symbol and Kite token.
pub fn run_selected(symbol: &str, token: u32, csv: impl Read, as_of: NaiveDate) -> Result<Report> {
    crate::instruments::contract::validate_symbol(symbol)?;
    ensure!(token > 0, "Zero configured instrument token");
    let rows = master::parse(csv)?;
    let matches: Vec<_> = rows
        .iter()
        .filter(|row| row.tradingsymbol == symbol || row.instrument_token == token)
        .collect();
    ensure!(
        matches.len() == 1,
        "Configured symbol/token must identify exactly one instrument-master row"
    );
    let row = matches[0];
    ensure!(
        row.tradingsymbol == symbol && row.instrument_token == token,
        "Configured symbol and instrument token identify different contracts"
    );
    ensure!(
        row.exchange == "MCX"
            && row.segment == "MCX-FUT"
            && row.name == "CRUDEOIL"
            && row.instrument_type == "FUT",
        "Configured contract is not a standard MCX CRUDEOIL future"
    );
    let expiry = NaiveDate::parse_from_str(&row.expiry, "%Y-%m-%d")
        .context("Invalid futures expiry in instrument master")?;
    ensure!(
        expiry >= as_of,
        "Configured contract has expired; explicit roll required"
    );
    ensure!(row.lot_size > 0, "Zero broker lot size");
    ensure!(
        Decimal::from_str(&row.tick_size).context("Invalid tick size")? > Decimal::ZERO,
        "Tick size must be positive"
    );
    report(row, as_of)
}

fn report(row: &master::InstrumentRow, as_of: NaiveDate) -> Result<Report> {
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

#[cfg(test)]
mod tests {
    use super::*;

    const MASTER: &str = "instrument_token,tradingsymbol,name,expiry,tick_size,lot_size,instrument_type,segment,exchange\n\
144870151,CRUDEOIL26SEPFUT,CRUDEOIL,2026-09-21,1,100,FUT,MCX-FUT,MCX\n\
155000001,CRUDEOIL26OCTFUT,CRUDEOIL,2026-10-19,1,100,FUT,MCX-FUT,MCX\n";

    #[test]
    fn selected_contract_supports_rollover_and_rejects_split_identity() {
        let as_of = NaiveDate::from_ymd_opt(2026, 9, 16).unwrap();
        let report =
            run_selected("CRUDEOIL26OCTFUT", 155_000_001, MASTER.as_bytes(), as_of).unwrap();
        assert_eq!(report.instrument_id, "CRUDEOIL26OCTFUT.MCX");
        assert_eq!(report.instrument_token, 155_000_001);

        assert!(run_selected("CRUDEOIL26OCTFUT", 144_870_151, MASTER.as_bytes(), as_of,).is_err());
        assert!(
            run_selected(
                "CRUDEOIL26SEPFUT",
                144_870_151,
                MASTER.as_bytes(),
                NaiveDate::from_ymd_opt(2026, 9, 22).unwrap(),
            )
            .is_err()
        );
    }
}
