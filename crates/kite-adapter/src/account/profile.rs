use anyhow::{Result, ensure};
use serde::Deserialize;
#[derive(Deserialize)]
pub(crate) struct Profile {
    user_id: String,
    exchanges: Vec<String>,
    pub products: Vec<String>,
}
impl Profile {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.user_id.trim().is_empty(),
            "Kite account identity is missing"
        );
        ensure!(
            self.exchanges.iter().any(|x| x == "MCX"),
            "MCX is not enabled on this account"
        );
        Ok(())
    }
}
