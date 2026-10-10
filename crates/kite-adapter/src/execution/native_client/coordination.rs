//! Run coordination without persistent trading state (kite-adapter 0.4.0).
//!
//! * [`InstanceLock`]: an OS lock file per slot and account. A second process for the same
//!   slot and account cannot start while one runs. The OS releases the lock when the process
//!   exits for any reason, so a crash never blocks the next start: after a crash, square off
//!   in Kite if needed and start again (the run's own startup check requires a flat account
//!   with no open orders).
//! * [`OrderBudget`]: the Kite order-rate budget, kept in Redis so every process trading the
//!   account (e.g. Sniper and SATS slots) shares one limit.
use anyhow::{Context, Result, anyhow, ensure};
use kite_execution::rate_limit::{Decision, Limiter, policy::Policy};
use std::{
    fs::{File, OpenOptions, TryLockError},
    io::Write,
    path::{Path, PathBuf},
};

/// Directory for lock files: `KITE_LOCK_DIR`, else `$HOME/.local/state/kite-node/locks`.
fn lock_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("KITE_LOCK_DIR").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set; set KITE_LOCK_DIR"))?;
    Ok(PathBuf::from(home).join(".local/state/kite-node/locks"))
}

/// Lock file name from the slot's key space and the account, e.g.
/// `kite-prod-crudeoilm-sniper-202610-AB1234.lock`.
fn lock_name(keys: &super::keys::KeySpace, account: &str) -> Result<String> {
    let scope = keys.lock(account)?;
    let name: String = scope
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' })
        .collect();
    Ok(format!("{name}.lock"))
}

/// Held for the whole run; dropping it (or the process ending) releases the lock.
#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
    path: PathBuf,
}
impl InstanceLock {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Where the slot's lock file lives (for status output).
pub fn lock_file(keys: &super::keys::KeySpace, account: &str) -> Result<PathBuf> {
    Ok(lock_dir()?.join(lock_name(keys, account)?))
}

/// Takes the slot's lock or fails at once if another process holds it.
pub fn lock_instance(keys: &super::keys::KeySpace, account: &str) -> Result<InstanceLock> {
    lock_at(&lock_dir()?, &lock_name(keys, account)?)
}

/// Preflight: fails if another process holds the slot's lock; takes nothing.
pub fn check_unlocked(keys: &super::keys::KeySpace, account: &str) -> Result<()> {
    lock_instance(keys, account).map(drop)
}

fn lock_at(dir: &Path, name: &str) -> Result<InstanceLock> {
    std::fs::create_dir_all(dir).with_context(|| format!("Cannot create lock directory {}", dir.display()))?;
    let path = dir.join(name);
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("Cannot open lock file {}", path.display()))?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            let holder = std::fs::read_to_string(&path).unwrap_or_default();
            anyhow::bail!(
                "Another kite-node process is running this slot ({}; lock {}). Stop it first.",
                holder.trim(),
                path.display()
            );
        }
        Err(TryLockError::Error(e)) => {
            return Err(anyhow!(e)).with_context(|| format!("Cannot lock {}", path.display()));
        }
    }
    // Informational only: who holds the lock. The OS lock is what counts.
    file.set_len(0)?;
    write!(file, "pid {}", std::process::id())?;
    file.flush()?;
    Ok(InstanceLock { _file: file, path })
}

/// Conservative application budget per account; includes cancellations and failed requests.
const ORDER_POLICY: Policy = Policy {
    per_second: 5,
    per_minute: 100,
    per_day: 1000,
};

/// The account's Kite order-rate budget in Redis, shared by every process on the account.
///
/// A failed Redis call leaves the outcome unknown (the count may or may not have been
/// taken), which is harmless for a rate budget. So instead of staying unusable for the
/// rest of the run (the limiter refuses everything after one error), the connection is
/// dropped and reopened on the next call (kite-adapter 0.5.0). The caller decides what an
/// error means for the order: see `Dispatcher` (entries are denied, exits still go out).
pub(crate) struct OrderBudget {
    url: String,
    key: String,
    limiter: Option<Limiter>,
}
impl OrderBudget {
    pub fn open(keys: &super::keys::KeySpace, account: &str) -> Result<Self> {
        Self::open_at(&kite_journal::connection::url_from_env()?, keys, account)
    }
    pub fn open_at(url: &str, keys: &super::keys::KeySpace, account: &str) -> Result<Self> {
        ensure!(
            !account.is_empty()
                && account.len() <= 64
                && account.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
            "Invalid account"
        );
        let key = match keys.order_budget(account)? {
            // Portfolio budgets are shared by every slot on the account.
            Some(key) => key,
            None => Limiter::key(&format!("native-account-{account}"))?,
        };
        // Opened at start-up: a run does not start without a working budget.
        let limiter = Limiter::open_or_create_key(url, &key, ORDER_POLICY)?;
        Ok(Self {
            url: url.to_owned(),
            key,
            limiter: Some(limiter),
        })
    }
    fn limiter(&mut self) -> Result<&mut Limiter> {
        if self.limiter.is_none() {
            self.limiter = Some(
                Limiter::open_or_create_key(&self.url, &self.key, ORDER_POLICY)
                    .context("Order-rate budget: Redis reopen failed")?,
            );
        }
        Ok(self.limiter.as_mut().expect("limiter just set"))
    }
    pub fn reserve(&mut self) -> Result<()> {
        let decision = self.limiter()?.reserve();
        match decision {
            Ok(Decision::Allowed) => Ok(()),
            Ok(Decision::Deferred { retry_after_ms }) => {
                anyhow::bail!("order-rate budget exhausted (free again in {retry_after_ms} ms)")
            }
            Err(e) => {
                self.limiter = None;
                Err(e.context("order-rate budget unavailable (Redis); will reconnect"))
            }
        }
    }
    pub fn cooldown(&mut self, ms: u64) -> Result<()> {
        let result = self.limiter()?.cooldown(ms.clamp(10_000, 86_400_000));
        if result.is_err() {
            self.limiter = None;
        }
        result
    }
}

#[cfg(test)]
mod lock_tests {
    use super::*;
    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("kite-lock-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }
    #[test]
    fn second_holder_is_refused_and_release_frees_the_lock() {
        let d = dir("pair");
        let first = lock_at(&d, "slot.lock").unwrap();
        let error = lock_at(&d, "slot.lock").unwrap_err().to_string();
        assert!(error.contains("Another kite-node process"), "{error}");
        assert!(error.contains(&format!("pid {}", std::process::id())), "{error}");
        // another slot is independent
        let other = lock_at(&d, "other.lock").unwrap();
        drop(first);
        let again = lock_at(&d, "slot.lock").unwrap();
        assert!(again.path().ends_with("slot.lock"));
        drop((other, again));
        let _ = std::fs::remove_dir_all(&d);
    }
    #[test]
    fn lock_names_are_plain_file_names() {
        let keys = super::super::keys::KeySpace::portfolio("kite", "crudeoilm-sniper").unwrap();
        let name = lock_name(&keys, "AB1234").unwrap();
        assert!(name.ends_with(".lock"));
        assert!(name.contains("crudeoilm-sniper") && name.contains("AB1234"));
        assert!(name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)), "{name}");
    }
}
