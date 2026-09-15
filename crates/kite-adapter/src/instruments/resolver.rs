use crate::{config::Config, instruments::master::InstrumentRow};
use anyhow::{Context, Result, ensure};
use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;
use std::str::FromStr;

pub fn resolve<'a>(
    rows: &'a [InstrumentRow],
    config: &Config,
    as_of: NaiveDate,
) -> Result<&'a InstrumentRow> {
    config.validate()?;
    let mut matches = Vec::new();
    for row in rows {
        if row.exchange != config.exchange
            || row.segment != config.segment
            || row.name != config.underlying
            || row.instrument_type != config.instrument_type
        {
            continue;
        }
        let expiry = NaiveDate::parse_from_str(&row.expiry, "%Y-%m-%d")
            .context("Invalid futures expiry in instrument master")?;
        if expiry.year() == config.expiry_year && expiry.month() == config.expiry_month {
            matches.push((row, expiry));
        }
    }
    ensure!(
        matches.len() == 1,
        "Expected exactly one contract; found {}",
        matches.len()
    );
    let (row, expiry) = matches[0];
    ensure!(
        row.tradingsymbol == config.expected_symbol,
        "Resolved symbol differs from configured symbol"
    );
    ensure!(
        expiry == config.expected_expiry,
        "Resolved expiry differs from configured expiry"
    );
    ensure!(
        expiry >= as_of,
        "Target contract has expired; explicit roll configuration required"
    );
    ensure!(row.instrument_token != 0, "Zero instrument token");
    ensure!(
        rows.iter()
            .filter(|r| r.instrument_token == row.instrument_token)
            .count()
            == 1,
        "Instrument token is not unique in this master"
    );
    ensure!(row.lot_size > 0, "Zero broker lot size");
    let tick = Decimal::from_str(&row.tick_size).context("Invalid tick size")?;
    ensure!(tick > Decimal::ZERO, "Tick size must be positive");
    Ok(row)
}
