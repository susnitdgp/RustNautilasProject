use anyhow::{Result, ensure};
use serde::Serialize;
#[derive(Clone, Copy, Serialize)]
pub struct Policy {
    pub per_second: u32,
    pub per_minute: u32,
    pub per_day: u32,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            per_second: 10,
            per_minute: 400,
            per_day: 5000,
        }
    }
}
impl Policy {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=10).contains(&self.per_second)
                && (1..=400).contains(&self.per_minute)
                && (1..=5000).contains(&self.per_day),
            "Order rate policy exceeds supported limits or is zero"
        );
        Ok(())
    }
}
