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
    #[serde(default)]
    pub rollover: Option<Rollover>,
    pub enabled: bool,
    pub strategy: String,
    pub instrument: String,
    pub instrument_token: u32,
    pub strategy_config: String,
    pub live_orders_enabled: bool,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rollover {
    pub strategy_id: String,
    pub contract_month: String,
    pub expected_expiry: chrono::NaiveDate,
    pub lot_size: u32,
    pub next_contract: Option<NextContract>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NextContract {
    pub instrument: String,
    pub instrument_token: u32,
    pub contract_month: String,
    pub expected_expiry: chrono::NaiveDate,
    pub verified: bool,
    pub approved: bool,
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
            if let Some(roll) = &instance.rollover {
                ensure!(
                    segment(&roll.strategy_id),
                    "Unsafe permanent strategy identity"
                );
                ensure!(
                    roll.contract_month.len() == 7
                        && roll.contract_month.as_bytes()[4] == b'-'
                        && roll.contract_month[..4].bytes().all(|c| c.is_ascii_digit())
                        && roll.contract_month[5..].bytes().all(|c| c.is_ascii_digit())
                        && (1..=12).contains(&roll.contract_month[5..].parse::<u32>()?),
                    "Invalid contract month"
                );
                ensure!(
                    roll.expected_expiry.format("%Y-%m").to_string() == roll.contract_month,
                    "Expiry disagrees with contract month"
                );
                ensure!(roll.lot_size > 0, "Invalid lot size");
                let symbol_month = roll
                    .expected_expiry
                    .format("%y%b")
                    .to_string()
                    .to_uppercase();
                ensure!(
                    instance.instrument.contains(&symbol_month),
                    "Contract symbol does not match expiry month"
                );
                ensure!(
                    instance.id
                        == format!(
                            "{}-{}",
                            roll.strategy_id,
                            roll.contract_month.replace('-', "")
                        ),
                    "Instance ID must bind strategy and contract month"
                );
                if let Some(next) = &roll.next_contract {
                    ensure!(
                        !next.approved && !next.verified && next.instrument_token == 0,
                        "Next contract must remain unverified and unapproved in portfolio skeleton"
                    );
                    ensure!(
                        next.contract_month > roll.contract_month
                            && next.expected_expiry.format("%Y-%m").to_string()
                                == next.contract_month,
                        "Invalid next contract month/expiry"
                    );
                    ensure!(next.instrument.ends_with(".MCX"), "Invalid next exchange");
                }
            }

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
                !instance.live_orders_enabled || cfg!(feature = "live-orders"),
                "live_orders_enabled needs a `--features live-orders` build"
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
    /// Redis key space for everything one slot's execution owns
    /// (`<prefix>:v1:{<slot>}:…`, plus the per-account order budget).
    pub fn keyspace(&self, id: &str) -> Result<kite_adapter::execution::native_client::keys::KeySpace> {
        ensure!(self.instances.iter().any(|v| v.id == id), "Unknown portfolio instance");
        kite_adapter::execution::native_client::keys::KeySpace::portfolio(&self.redis_prefix, id)
    }
    /// Nautilus trader ID for a slot; its Redis cache keys become
    /// `trader-<prefix>-<slot>:<run uuid>:…`.
    pub fn trader_id(&self, id: &str) -> Result<String> {
        ensure!(self.instances.iter().any(|v| v.id == id), "Unknown portfolio instance");
        Ok(format!("{}-{}", self.redis_prefix, id))
    }
    pub fn key(&self, id: &str, kind: &str) -> Result<String> {
        ensure!(
            segment(id) && segment(kind) && self.instances.iter().any(|v| v.id == id),
            "Unknown/unsafe key identity"
        );
        Ok(format!("{}:v1:{{{}}}:{}", self.redis_prefix, id, kind))
    }
}
/// Strategy-specific settings for the report. A `sats` slot's own config must
/// load and validate when the slot is enabled; for a disabled slot the error is reported.
fn strategy_settings(v: &Instance) -> Result<serde_json::Value> {
    if v.strategy != super::sats_config::STRATEGY {
        return Ok(serde_json::Value::Null);
    }
    match super::sats_config::SatsConfig::load(&v.strategy_config) {
        Ok(c) => Ok(serde_json::json!({
            "bar_minutes": c.bar_minutes, "lots": c.lots, "tick_size": c.tick_size,
            "point_value": c.point_value, "execution": c.execution,
            "preset": c.params.preset, "tp_mode": c.params.tp_mode,
        })),
        Err(e) if !v.enabled => Ok(serde_json::json!({ "error": format!("{e:#}") })),
        Err(e) => Err(e),
    }
}
pub fn inspect(path: &str) -> Result<()> {
    let portfolio: Portfolio = serde_json::from_str(&fs::read_to_string(path)?)?;
    portfolio.validate()?;
    let settings = portfolio.instances.iter().map(strategy_settings).collect::<Result<Vec<_>>>()?;
    println!(
        "{}",
        serde_json::json!({
            "event": "portfolio_validation",
            "mode": "read_only",
            "broker_config": portfolio.broker_config,
            "tokens": portfolio.tokens(),
            "instances": portfolio.instances.iter().zip(&settings).map(|(v, settings)| serde_json::json!({
                "id": v.id, "enabled": v.enabled, "rollover_strategy": v.rollover.as_ref().map(|r| &r.strategy_id), "contract_month":v.rollover.as_ref().map(|r| &r.contract_month), "strategy": v.strategy, "instrument": v.instrument,
                "token": v.instrument_token, "strategy_config": v.strategy_config,
                "strategy_settings": settings,
                "redis_journal": portfolio.key(&v.id, "journal").expect("validated"),
                "redis_owner": portfolio.key(&v.id, "owner").expect("validated"),
                "redis_commands": portfolio.keyspace(&v.id).and_then(|k| k.commands("YYYYMMDD-run")).expect("validated"),
                "redis_lease": portfolio.keyspace(&v.id).and_then(|k| k.lease("KITEUSER")).expect("validated"),
                "redis_order_budget": portfolio.keyspace(&v.id).and_then(|k| k.order_budget("KITEUSER")).expect("validated"),
                "nautilus_cache": format!("trader-{}:<run-uuid>:*", portfolio.trader_id(&v.id).expect("validated"))
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
        assert_eq!(p.validate().is_err(), !cfg!(feature = "live-orders"));
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
