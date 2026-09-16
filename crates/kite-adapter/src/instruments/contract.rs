//! Verified specification for a JSON-selected standard MCX crude-oil future.
use crate::preflight::Report;
use anyhow::{Result, ensure};
use chrono::{DateTime, Duration, NaiveDate};
use nautilus_core::{Params, UnixNanos};
use nautilus_model::{
    enums::AssetClass,
    identifiers::{InstrumentId, Symbol},
    instruments::FuturesContract,
    types::{Currency, Price, Quantity},
};
use std::str::FromStr;

pub const SPEC_SOURCE: &str = "https://www.mcxindia.com/docs/default-source/products/contract-specification/crude-oil/crude-oil-january-2026-contract-onwards267be8c1-650a-4baa-aabd-ffcc9364c100.pdf";

pub fn validate_symbol(symbol: &str) -> Result<()> {
    let contract = symbol
        .strip_prefix("CRUDEOIL")
        .and_then(|value| value.strip_suffix("FUT"))
        .unwrap_or_default();
    ensure!(
        contract.len() == 5 && contract.is_ascii(),
        "Only standard MCX CRUDEOIL futures are supported"
    );
    let (year, month) = contract.split_at(2);
    ensure!(
        year.bytes().all(|b| b.is_ascii_digit())
            && matches!(
                month,
                "JAN"
                    | "FEB"
                    | "MAR"
                    | "APR"
                    | "MAY"
                    | "JUN"
                    | "JUL"
                    | "AUG"
                    | "SEP"
                    | "OCT"
                    | "NOV"
                    | "DEC"
            ),
        "Only standard MCX CRUDEOIL futures are supported"
    );
    Ok(())
}

pub fn symbol_from_instrument_id(instrument_id: &str) -> Result<&str> {
    let symbol = instrument_id
        .strip_suffix(".MCX")
        .ok_or_else(|| anyhow::anyhow!("Only MCX instruments are supported"))?;
    validate_symbol(symbol)?;
    Ok(symbol)
}

pub fn build(report: &Report, now: UnixNanos) -> Result<FuturesContract> {
    let symbol = symbol_from_instrument_id(&report.instrument_id)?;
    let expiry = NaiveDate::parse_from_str(&report.expiry, "%Y-%m-%d")?;
    ensure!(
        expiry >= report.validation_date_ist,
        "No verified economic specification for an expired contract"
    );
    ensure!(
        report.broker_lot_size == 1
            && report.tick_size.parse::<rust_decimal::Decimal>()? == rust_decimal::Decimal::ONE,
        "Broker quantity/tick metadata differs from verified specification"
    );
    let timestamp = |value: &str| -> Result<UnixNanos> {
        let date = DateTime::parse_from_rfc3339(value)?;
        Ok(u64::try_from(
            date.timestamp_nanos_opt()
                .ok_or_else(|| anyhow::anyhow!("Timestamp overflow"))?,
        )?
        .into())
    };
    let activation = expiry - Duration::days(365);
    let info: Params = serde_json::from_value(serde_json::json!({
        "source": SPEC_SOURCE, "settlement": "cash", "quantity_unit": "contracts",
        "barrels_per_contract": 100, "data_only": true,
        "margin_and_fee_fields_are_unconfigured": true,
        "expiry_time_basis": "23:30 IST during US daylight saving time",
        "activation_basis": "conservative one-year metadata window; live identity verified against Kite master",
    }))?;
    Ok(FuturesContract::builder()
        .instrument_id(InstrumentId::from_as_ref(&report.instrument_id)?)
        .raw_symbol(Symbol::new(symbol))
        .asset_class(AssetClass::Commodity)
        .underlying("CRUDEOIL".into())
        .activation_ns(timestamp(&format!("{activation}T00:00:00+05:30"))?)
        .expiration_ns(timestamp(&format!("{expiry}T23:30:00+05:30"))?)
        .currency(Currency::from_str("INR")?)
        .price_precision(0)
        .price_increment(Price::from_str("1").map_err(anyhow::Error::msg)?)
        .multiplier(Quantity::from(100))
        .lot_size(Quantity::from(1))
        .info(info)
        .ts_event(now)
        .ts_init(now)
        .build()?)
}
