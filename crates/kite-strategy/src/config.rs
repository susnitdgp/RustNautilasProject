use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub enable_short: bool,
    #[serde(default = "default_stop")]
    pub stop_loss_rupees: u32,
    #[serde(default = "default_target")]
    pub target_rupees: u32,
    pub fast: usize,
    pub slow: usize,
    pub max_spread_rupees: u32,
    pub max_age_seconds: u32,
    pub max_entries: u32,
}
impl Config {
    pub fn parse(input: &str) -> Result<Self> {
        let config: Self = toml::from_str(input)?;
        config.validate()?;
        Ok(config)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=10000).contains(&self.stop_loss_rupees)
                && (1..=10000).contains(&self.target_rupees)
                && self.fast > 0
                && self.fast < self.slow
                && self.slow <= 1000
                && (1..=10).contains(&self.max_spread_rupees)
                && (1..=30).contains(&self.max_age_seconds)
                && (1..=100).contains(&self.max_entries),
            "Invalid strategy configuration"
        );
        Ok(())
    }
}

fn default_stop() -> u32 {
    30
}
fn default_target() -> u32 {
    60
}
