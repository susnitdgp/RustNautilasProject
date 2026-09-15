use anyhow::{Context, Result, ensure};
use chrono::{Datelike, NaiveDate};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub mode: String,
    pub exchange: String,
    pub segment: String,
    pub underlying: String,
    pub instrument_type: String,
    pub expiry_year: i32,
    pub expiry_month: u32,
    pub expected_symbol: String,
    pub expected_expiry: NaiveDate,
}

impl Config {
    pub fn parse(input: &str) -> Result<Self> {
        let config: Self = toml::from_str(input).context("Invalid target configuration")?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.mode == "data_only",
            "Only data_only mode is implemented"
        );
        ensure!(
            self.exchange == "MCX" && self.segment == "MCX-FUT",
            "Only MCX futures supported"
        );
        ensure!(
            self.underlying == "CRUDEOIL" && self.instrument_type == "FUT",
            "Only standard CRUDEOIL futures supported"
        );
        ensure!(
            (1..=12).contains(&self.expiry_month),
            "Invalid expiry month"
        );
        ensure!(
            self.expiry_year == self.expected_expiry.year()
                && self.expiry_month == self.expected_expiry.month(),
            "Expiry and target month disagree"
        );
        ensure!(
            !self.expected_symbol.is_empty()
                && self
                    .expected_symbol
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()),
            "Invalid expected symbol"
        );
        Ok(())
    }
}
