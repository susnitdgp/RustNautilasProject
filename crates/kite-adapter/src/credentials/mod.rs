//! Credential storage boundary. Values are intentionally not serializable.
pub mod redis;

use anyhow::{Result, ensure};
use std::fmt;
use zeroize::Zeroizing;

pub struct KiteCredentials {
    api_key: Zeroizing<String>,
    access_token: Zeroizing<String>,
}

impl KiteCredentials {
    pub(crate) fn new(api_key: Option<String>, access_token: Option<String>) -> Result<Self> {
        // Wrap both before validating, so owned values are cleared on failure too.
        let api_key = api_key.map(Zeroizing::new);
        let access_token = access_token.map(Zeroizing::new);
        let valid = |value: &Option<Zeroizing<String>>| {
            value.as_ref().is_some_and(|v| {
                !v.is_empty() && v.len() <= 4096 && v.bytes().all(|b| b.is_ascii_graphic())
            })
        };
        ensure!(
            valid(&api_key),
            "Redis key susanta:kite_api_key is missing, not a string, empty, or invalid"
        );
        ensure!(
            valid(&access_token),
            "Redis key susanta:kite_access_token is missing, not a string, empty, or invalid"
        );
        Ok(Self {
            api_key: api_key.expect("validated"),
            access_token: access_token.expect("validated"),
        })
    }

    /// Explicit access for future authenticated transports. Never log the return value.
    pub fn api_key(&self) -> &str {
        &self.api_key
    }
    /// Explicit access for future authenticated transports. Never log the return value.
    pub fn access_token(&self) -> &str {
        &self.access_token
    }
}

impl fmt::Debug for KiteCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("KiteCredentials { api_key: [REDACTED], access_token: [REDACTED] }")
    }
}
