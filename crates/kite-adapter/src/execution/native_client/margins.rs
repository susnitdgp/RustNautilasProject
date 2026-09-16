//! Select one funds ledger; never add shared equity and commodity balances.
use super::broker::Funds;
use anyhow::{Result, ensure};
use serde::Deserialize;
#[derive(Deserialize)]
pub(crate) struct Ledgers {
    pub equity: Funds,
    pub commodity: Funds,
}
pub(crate) fn select(mut funds: Ledgers, verified_mcx: bool) -> Result<Funds> {
    ensure!(
        verified_mcx,
        "Verified MCX account required before selecting funds"
    );
    let chosen = if funds.commodity.enabled {
        funds.commodity.ledger = Some("commodity");
        funds.commodity
    } else {
        ensure!(funds.equity.enabled, "No enabled trading funds ledger");
        funds.equity.ledger = Some("equity_unified");
        funds.equity
    };
    ensure!(
        chosen.utilised.debits >= rust_decimal::Decimal::ZERO,
        "Negative utilised debits unsupported"
    );
    Ok(chosen)
}
pub async fn check(path: &str) -> Result<serde_json::Value> {
    use crate::http::authenticated::{Endpoint, ReadClient};
    let settings: super::production::Settings =
        serde_json::from_str(&std::fs::read_to_string(path)?)?;
    let credentials = crate::credentials::redis::load_from_env()?;
    let read = ReadClient::new(&credentials)?;
    #[derive(Deserialize)]
    struct Profile {
        user_id: String,
        exchanges: Vec<String>,
        products: Vec<String>,
    }
    let profile: Profile = read.get(Endpoint::Profile).await?;
    ensure!(
        profile.user_id == settings.expected_user_id
            && profile.exchanges.iter().any(|v| v == "MCX")
            && profile.products.contains(&settings.product),
        "Kite identity or MCX/product permission mismatch"
    );
    let funds = select(read.get(Endpoint::Margins).await?, true)?;
    Ok(
        serde_json::json!({"event":"kite_funds_check","account_verified":true,
        "mcx_enabled":true,"product":settings.product,"funds_ledger":funds.ledger,
        "funds_enabled":funds.enabled,"debits_supported":true,"order_mutations":0}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    fn ledgers(equity: bool, commodity: bool) -> Ledgers {
        serde_json::from_value(serde_json::json!({
            "equity":{"enabled":equity,"net":9000,"utilised":{"debits":1000}},
            "commodity":{"enabled":commodity,"net":2000,"utilised":{"debits":500}}
        }))
        .unwrap()
    }
    #[test]
    fn unified_ledger_used_only_after_verified_mcx_and_disabled_commodity() {
        let funds = select(ledgers(true, false), true).unwrap();
        assert_eq!(funds.ledger, Some("equity_unified"));
        assert_eq!(funds.net, rust_decimal::Decimal::from(9000));
        assert!(select(ledgers(true, false), false).is_err());
        assert!(select(ledgers(false, false), true).is_err());
    }
    #[test]
    fn legacy_balance_is_not_combined_with_equity_or_replaced_on_invalid_debits() {
        let funds = select(ledgers(true, true), true).unwrap();
        assert_eq!(funds.ledger, Some("commodity"));
        assert_eq!(funds.net, rust_decimal::Decimal::from(2000));
        let mut invalid = ledgers(true, true);
        invalid.commodity.utilised.debits = rust_decimal::Decimal::from(-1);
        assert!(select(invalid, true).is_err());
    }
}
