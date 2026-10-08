//! JSON-only portfolio manifest for isolated strategy instances.
//! Deliberately read-only: does not authorize broker execution.
use anyhow::{Result, ensure};
use serde::Deserialize;
use std::{collections::HashSet, fs};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Portfolio {
    pub version: u32,
    pub broker_config: String,
    pub redis_prefix: String,
    pub instances: Vec<Instance>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Instance {
    pub id: String,
    pub enabled: bool,
    pub strategy: String,
    pub instrument: String,
    pub instrument_token: u32,
    pub strategy_config: String,
    pub live_orders_enabled: bool,
}
fn segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 48
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}
impl Portfolio {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "Unsupported portfolio version");
        ensure!(segment(&self.redis_prefix), "Unsafe Redis prefix");
        ensure!(
            !self.broker_config.trim().is_empty(),
            "Missing broker configuration path"
        );
        ensure!(
            (1..=4).contains(&self.instances.len()),
            "Portfolio must contain 1-4 instances"
        );
        let mut ids = HashSet::new();
        let mut bindings = HashSet::new();
        let mut token_instruments = std::collections::HashMap::new();
        for instance in &self.instances {
            ensure!(segment(&instance.id), "Unsafe instance ID");
            ensure!(ids.insert(&instance.id), "Duplicate instance ID");
            ensure!(
                !instance.strategy.trim().is_empty() && !instance.instrument.trim().is_empty(),
                "Missing strategy or instrument"
            );
            ensure!(
                instance.instrument.ends_with(".MCX") || instance.instrument.ends_with(".NFO"),
                "Portfolio requires an NFO or MCX instrument"
            );
            if instance.enabled {
                ensure!(
                    instance.instrument_token != 0,
                    "Enabled instance needs a verified instrument token"
                );
                if let Some(previous) =
                    token_instruments.insert(instance.instrument_token, &instance.instrument)
                {
                    ensure!(
                        previous == &instance.instrument,
                        "Instrument token assigned to different instruments"
                    );
                }
            }
            ensure!(
                !instance.strategy_config.trim().is_empty(),
                "Missing strategy config path"
            );
            ensure!(
                !instance.live_orders_enabled,
                "Portfolio execution is not yet enabled; use read-only instances"
            );
            ensure!(
                bindings.insert((&instance.instrument, &instance.strategy)),
                "Duplicate instrument/strategy binding"
            );
        }
        Ok(())
    }
    pub fn tokens(&self) -> Vec<u32> {
        let mut tokens: Vec<_> = self
            .instances
            .iter()
            .filter(|v| v.enabled)
            .map(|v| v.instrument_token)
            .collect();
        tokens.sort_unstable();
        tokens.dedup();
        tokens
    }
    pub fn key(&self, id: &str, kind: &str) -> Result<String> {
        ensure!(
            segment(id) && segment(kind) && self.instances.iter().any(|v| v.id == id),
            "Unknown/unsafe key identity"
        );
        Ok(format!("{}:v1:{{{}}}:{}", self.redis_prefix, id, kind))
    }
}
pub fn inspect(path: &str) -> Result<()> {
    let portfolio: Portfolio = serde_json::from_str(&fs::read_to_string(path)?)?;
    portfolio.validate()?;
    println!(
        "{}",
        serde_json::json!({
            "event": "portfolio_validation",
            "mode": "read_only",
            "broker_config": portfolio.broker_config,
            "tokens": portfolio.tokens(),
            "instances": portfolio.instances.iter().map(|v| serde_json::json!({
                "id": v.id, "enabled": v.enabled, "strategy": v.strategy, "instrument": v.instrument,
                "token": v.instrument_token, "strategy_config": v.strategy_config,
                "redis_journal": portfolio.key(&v.id, "journal").expect("validated"),
                "redis_owner": portfolio.key(&v.id, "owner").expect("validated")
            })).collect::<Vec<_>>()
        })
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> Portfolio {
        serde_json::from_str(r#"{
            "version":1,"broker_config":"config/kite.json","redis_prefix":"kite-dev",
            "instances":[
                {"id":"crudeoil26oct-ilrc","enabled":true,"strategy":"ilrc","instrument":"CRUDEOIL26OCTFUT.MCX","instrument_token":123,"strategy_config":"config/crude.json","live_orders_enabled":false},
                {"id":"gold-trend","enabled":true,"strategy":"trend","instrument":"GOLD26DEC.MCX","instrument_token":456,"strategy_config":"config/gold.json","live_orders_enabled":false}
            ]}"#).unwrap()
    }
    #[test]
    fn validates_isolated_keys_and_tokens() {
        let p = sample();
        p.validate().unwrap();
        assert_eq!(p.tokens(), vec![123, 456]);
        assert_ne!(
            p.key("crudeoil26oct-ilrc", "journal").unwrap(),
            p.key("gold-trend", "journal").unwrap()
        );
    }
    #[test]
    fn fails_closed_on_duplicates_and_live_orders() {
        let mut p = sample();
        p.instances[1].id = "crudeoil26oct-ilrc".into();
        assert!(p.validate().is_err());
        p.instances[1].id = "gold-trend".into();
        p.instances[1].live_orders_enabled = true;
        assert!(p.validate().is_err());
        p.instances[1].live_orders_enabled = false;
        p.instances[1].instrument_token = 123;
        p.instances[1].instrument = p.instances[0].instrument.clone();
        p.validate().unwrap(); // Distinct strategies may share the same instrument.
        assert_eq!(p.tokens(), vec![123]);
    }
    #[test]
    fn disabled_placeholder_is_excluded_and_can_be_enabled_with_token() {
        let mut p = sample();
        p.instances[1].enabled = false;
        p.instances[1].instrument_token = 0;
        p.validate().unwrap();
        assert_eq!(p.tokens(), vec![123]);
        p.instances[1].enabled = true;
        assert!(p.validate().is_err());
        p.instances[1].instrument_token = 456;
        p.validate().unwrap();
        assert_eq!(p.tokens(), vec![123, 456]);
    }
    #[test]
    fn token_collision_across_instruments_is_rejected() {
        let mut p = sample();
        p.instances[1].instrument_token = 123;
        assert!(p.validate().is_err());
    }
    #[test]
    fn rejects_key_injection() {
        let mut p = sample();
        p.redis_prefix = "bad:key".into();
        assert!(p.validate().is_err());
    }
}
