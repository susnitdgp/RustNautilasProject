//! Verified specifications for JSON-selected MCX crude-oil futures
//! (standard CRUDEOIL and mini CRUDEOILM). Anything else is rejected.
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

/// One verified contract family: Kite instrument-master `name`, barrels per lot.
#[derive(Debug, PartialEq, Eq)]
pub struct CrudeSpec {
    pub underlying: &'static str,
    pub barrels_per_contract: u32,
    pub source: &'static str,
}

/// Longest prefix first: "CRUDEOIL" is a prefix of "CRUDEOILM".
pub const SPECS: [CrudeSpec; 2] = [
    CrudeSpec {
        underlying: "CRUDEOILM",
        barrels_per_contract: 10,
        source: "MCX Crude Oil Mini contract specification (10 barrels per lot, Rs/barrel quote, tick Re 1)",
    },
    CrudeSpec { underlying: "CRUDEOIL", barrels_per_contract: 100, source: SPEC_SOURCE },
];

const MONTHS: [&str; 12] = ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"];

/// The verified family for a monthly future symbol such as `CRUDEOILM26OCTFUT`.
pub fn spec_for(symbol: &str) -> Result<&'static CrudeSpec> {
    for spec in &SPECS {
        let Some(contract) = symbol.strip_prefix(spec.underlying).and_then(|v| v.strip_suffix("FUT")) else {
            continue;
        };
        if contract.len() == 5 && contract.is_ascii() {
            let (year, month) = contract.split_at(2);
            if year.bytes().all(|b| b.is_ascii_digit()) && MONTHS.contains(&month) {
                return Ok(spec);
            }
        }
    }
    anyhow::bail!("Only verified MCX CRUDEOIL / CRUDEOILM monthly futures are supported")
}

pub fn validate_symbol(symbol: &str) -> Result<()> {
    spec_for(symbol).map(|_| ())
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
    let spec = spec_for(symbol)?;
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
        "source": spec.source, "settlement": "cash", "quantity_unit": "contracts",
        "barrels_per_contract": spec.barrels_per_contract, "data_only": true,
        "margin_and_fee_fields_are_unconfigured": true,
        "expiry_time_basis": "23:30 IST during US daylight saving time",
        "activation_basis": "conservative one-year metadata window; live identity verified against Kite master",
    }))?;
    Ok(FuturesContract::builder()
        .instrument_id(InstrumentId::from_as_ref(&report.instrument_id)?)
        .raw_symbol(Symbol::new(symbol))
        .asset_class(AssetClass::Commodity)
        .underlying(spec.underlying.into())
        .activation_ns(timestamp(&format!("{activation}T00:00:00+05:30"))?)
        .expiration_ns(timestamp(&format!("{expiry}T23:30:00+05:30"))?)
        .currency(Currency::from_str("INR")?)
        .price_precision(0)
        .price_increment(Price::from_str("1").map_err(anyhow::Error::msg)?)
        .multiplier(Quantity::from(spec.barrels_per_contract))
        .lot_size(Quantity::from(1))
        .info(info)
        .ts_event(now)
        .ts_init(now)
        .build()?)
}

#[cfg(test)]
mod spec_tests {
    use super::*;

    #[test]
    fn mini_and_standard_resolve_to_their_own_specs() {
        assert_eq!(spec_for("CRUDEOILM26OCTFUT").unwrap().barrels_per_contract, 10);
        assert_eq!(spec_for("CRUDEOIL26OCTFUT").unwrap().barrels_per_contract, 100);
        assert_eq!(symbol_from_instrument_id("CRUDEOILM26OCTFUT.MCX").unwrap(), "CRUDEOILM26OCTFUT");
    }

    #[test]
    fn everything_else_is_rejected() {
        for bad in ["CRUDEOILM26OCT", "CRUDEOILX26OCTFUT", "NATURALGAS26OCTFUT", "CRUDEOIL26XYZFUT", "CRUDEOILM2026OCTFUT", "GOLD26OCTFUT"] {
            assert!(spec_for(bad).is_err(), "{bad} must be rejected");
        }
        assert!(symbol_from_instrument_id("CRUDEOILM26OCTFUT.NFO").is_err());
    }
}
