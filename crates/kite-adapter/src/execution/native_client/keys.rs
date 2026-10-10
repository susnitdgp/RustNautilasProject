//! Names for a slot's shared state (kite-adapter 0.4.0: no order journal, no lease).
//!
//! `Portfolio` scopes a strategy slot under the manifest's naming scheme `<prefix>:v1:{<slot>}:<kind>` (the `{…}`
//! is a Redis Cluster hash tag, so one slot's keys stay together):
//!
//! | kind          | name                                                  |
//! |---------------|-------------------------------------------------------|
//! | order budget  | `<prefix>:v1:{account-<kite user>}:order-budget` (Redis) |
//! | live dashboard| `<prefix>:v1:{<slot>}:dash` (+ `:state`, `:events`, `:live`) (dashboard Redis) |
//! | instance lock | `<prefix>-<slot>-<kite user>.lock` (a file, not Redis) |
//!
//! The order budget is per Kite account (Kite's limits are per API key), so all
//! slots trading the same account share it; locks are per slot, so different slots
//! can run side by side on one account.
use anyhow::{Result, ensure};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeySpace {
    Portfolio { prefix: String, slot: String },
}

fn segment(value: &str, what: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 48
            && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "Unsafe Redis {what} segment"
    );
    Ok(())
}

pub fn account(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty() && value.len() <= 32 && value.bytes().all(|b| b.is_ascii_alphanumeric()),
        "Invalid native account scope"
    );
    Ok(())
}

impl KeySpace {
    /// Keys for one portfolio slot (`prefix` = manifest `redis_prefix`, `slot` = instance id).
    pub fn portfolio(prefix: &str, slot: &str) -> Result<Self> {
        segment(prefix, "prefix")?;
        segment(slot, "slot")?;
        Ok(Self::Portfolio { prefix: prefix.into(), slot: slot.into() })
    }

    /// Scope of the slot's instance lock file (one process per slot and account).
    pub fn lock(&self, user: &str) -> Result<String> {
        account(user)?;
        Ok(match self {
            Self::Portfolio { prefix, slot } => format!("{prefix}-{slot}-{user}"),
        })
    }

    /// Base key of the live dashboard data (written to the dashboard Redis, not the
    /// trading one); the writer appends `:state`, `:events` and `:live`.
    pub fn dashboard(&self) -> String {
        match self {
            Self::Portfolio { prefix, slot } => format!("{prefix}:v1:{{{slot}}}:dash"),
        }
    }

    /// Order-budget key, shared by every slot on the account.
    pub fn order_budget(&self, user: &str) -> Result<String> {
        account(user)?;
        Ok(match self {
            Self::Portfolio { prefix, .. } => format!("{prefix}:v1:{{account-{user}}}:order-budget"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portfolio_names_follow_the_manifest_scheme() {
        let k = KeySpace::portfolio("kite-prod", "crudeoilm-sats-202610").unwrap();
        assert_eq!(k.dashboard(), "kite-prod:v1:{crudeoilm-sats-202610}:dash");
        assert_eq!(k.lock("NVC171").unwrap(), "kite-prod-crudeoilm-sats-202610-NVC171");
        assert_eq!(
            k.order_budget("NVC171").unwrap(),
            "kite-prod:v1:{account-NVC171}:order-budget",
            "budget is shared by every slot on the account"
        );
        let other = KeySpace::portfolio("kite-prod", "gold-sats-202612").unwrap();
        assert_ne!(other.lock("NVC171").unwrap(), k.lock("NVC171").unwrap(), "locks are per slot");
        assert_eq!(other.order_budget("NVC171").unwrap(), k.order_budget("NVC171").unwrap());
    }

    #[test]
    fn rejects_injection() {
        assert!(KeySpace::portfolio("bad:prefix", "slot").is_err());
        assert!(KeySpace::portfolio("p", "slot}x").is_err());
        let k = KeySpace::portfolio("p", "s").unwrap();
        assert!(k.lock("NVC-171").is_err());
    }
}
