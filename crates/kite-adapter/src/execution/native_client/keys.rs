//! Redis key names for native execution state.
//!
//! `Legacy` keeps the historical single-account names used by the existing
//! review/status tooling. `Portfolio` scopes everything a strategy slot owns
//! under the portfolio manifest's naming scheme `<prefix>:v1:{<slot>}:<kind>`
//! (the `{…}` is a Redis Cluster hash tag, so one slot's keys stay together):
//!
//! | kind          | key                                                   |
//! |---------------|-------------------------------------------------------|
//! | command ledger| `<prefix>:v1:{<slot>}:commands:<run namespace>`       |
//! | slot lease    | `<prefix>:v1:{<slot>}:lease:<kite user>`              |
//! | order budget  | `<prefix>:v1:{account-<kite user>}:order-budget`      |
//! | live dashboard| `<prefix>:v1:{<slot>}:dash` (+ `:state`, `:events`, `:live`) |
//!
//! The order budget is per Kite account (Kite's limits are per API key), so all
//! slots of a portfolio trading the same account share it; leases are per slot,
//! so different slots can run side by side on one account.
use anyhow::{Result, ensure};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeySpace {
    Legacy,
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

pub fn namespace(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty() && value.len() <= 64 && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
        "Invalid native command namespace"
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

    pub fn commands(&self, ns: &str) -> Result<String> {
        namespace(ns)?;
        Ok(match self {
            Self::Legacy => format!("susanta:nautilus:native-kite:commands:{{{ns}}}"),
            Self::Portfolio { prefix, slot } => format!("{prefix}:v1:{{{slot}}}:commands:{ns}"),
        })
    }

    pub fn lease(&self, user: &str) -> Result<String> {
        account(user)?;
        Ok(match self {
            Self::Legacy => format!("susanta:nautilus:native-kite:account:{{{user}}}"),
            Self::Portfolio { prefix, slot } => format!("{prefix}:v1:{{{slot}}}:lease:{user}"),
        })
    }

    /// Base key of the live dashboard data (written to the dashboard Redis, not the
    /// trading one); the writer appends `:state`, `:events` and `:live`.
    pub fn dashboard(&self) -> String {
        match self {
            Self::Legacy => "susanta:nautilus:native-kite:dash".into(),
            Self::Portfolio { prefix, slot } => format!("{prefix}:v1:{{{slot}}}:dash"),
        }
    }

    /// Full order-budget key for portfolio key spaces; `None` for legacy, which
    /// keeps the rate limiter's own `native-account-<user>` scope.
    pub fn order_budget(&self, user: &str) -> Result<Option<String>> {
        account(user)?;
        Ok(match self {
            Self::Legacy => None,
            Self::Portfolio { prefix, .. } => Some(format!("{prefix}:v1:{{account-{user}}}:order-budget")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_names_are_unchanged() {
        let k = KeySpace::Legacy;
        assert_eq!(k.commands("run-1").unwrap(), "susanta:nautilus:native-kite:commands:{run-1}");
        assert_eq!(k.lease("NVC171").unwrap(), "susanta:nautilus:native-kite:account:{NVC171}");
        assert_eq!(k.order_budget("NVC171").unwrap(), None);
    }

    #[test]
    fn portfolio_names_follow_the_manifest_scheme() {
        let k = KeySpace::portfolio("kite-prod", "crudeoilm-sats-202610").unwrap();
        assert_eq!(k.dashboard(), "kite-prod:v1:{crudeoilm-sats-202610}:dash");
        assert_eq!(
            k.commands("20261009-f03e92db").unwrap(),
            "kite-prod:v1:{crudeoilm-sats-202610}:commands:20261009-f03e92db"
        );
        assert_eq!(k.lease("NVC171").unwrap(), "kite-prod:v1:{crudeoilm-sats-202610}:lease:NVC171");
        assert_eq!(
            k.order_budget("NVC171").unwrap().unwrap(),
            "kite-prod:v1:{account-NVC171}:order-budget",
            "budget is shared by every slot on the account"
        );
        let other = KeySpace::portfolio("kite-prod", "gold-sats-202612").unwrap();
        assert_ne!(other.lease("NVC171").unwrap(), k.lease("NVC171").unwrap(), "leases are per slot");
        assert_eq!(other.order_budget("NVC171").unwrap(), k.order_budget("NVC171").unwrap());
    }

    #[test]
    fn rejects_injection() {
        assert!(KeySpace::portfolio("bad:prefix", "slot").is_err());
        assert!(KeySpace::portfolio("p", "slot}x").is_err());
        let k = KeySpace::portfolio("p", "s").unwrap();
        assert!(k.commands("a:b").is_err());
        assert!(k.lease("NVC-171").is_err());
    }
}
