//! Verified specification for standard MCX September 2026 crude oil only.
use crate::preflight::Report;
use anyhow::{Result, ensure};
use chrono::DateTime;
use nautilus_core::{Params, UnixNanos};
use nautilus_model::{
    enums::AssetClass,
    identifiers::{InstrumentId, Symbol},
    instruments::FuturesContract,
    types::{Currency, Price, Quantity},
};
use std::str::FromStr;

pub const SPEC_SOURCE: &str = "https://www.mcxindia.com/docs/default-source/products/contract-specification/crude-oil/crude-oil-january-2026-contract-onwards267be8c1-650a-4baa-aabd-ffcc9364c100.pdf";

pub fn build(report: &Report, now: UnixNanos) -> Result<FuturesContract> {
    ensure!(
        report.instrument_id == "CRUDEOIL26SEPFUT.MCX" && report.expiry == "2026-09-21",
        "No verified economic specification for this contract"
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
    let info: Params = serde_json::from_value(serde_json::json!({
        "source": SPEC_SOURCE, "settlement": "cash", "quantity_unit": "contracts",
        "barrels_per_contract": 100, "data_only": true,
        "margin_and_fee_fields_are_unconfigured": true,
        "expiry_time_basis": "23:30 IST during US daylight saving time",
    }))?;
    Ok(FuturesContract::builder()
        .instrument_id(InstrumentId::from_as_ref(&report.instrument_id)?)
        .raw_symbol(Symbol::new("CRUDEOIL26SEPFUT"))
        .asset_class(AssetClass::Commodity)
        .underlying("CRUDEOIL".into())
        .activation_ns(timestamp("2026-03-20T09:00:00+05:30")?)
        .expiration_ns(timestamp("2026-09-21T23:30:00+05:30")?)
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
