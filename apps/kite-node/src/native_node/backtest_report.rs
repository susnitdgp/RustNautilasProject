//! Immutable per-run result folders, separate from Redis application/order state.
use anyhow::Result;
use std::path::Path;
pub fn json(path: &Path, name: &str, value: &impl serde::Serialize) -> Result<()> {
    let file = path.join(name);
    let temporary = path.join(format!("{name}.tmp"));
    std::fs::write(&temporary, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(temporary, file)?;
    Ok(())
}
