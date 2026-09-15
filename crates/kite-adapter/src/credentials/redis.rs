//! Synchronous Redis reads for preflight/startup, never for the live event loop.
use super::KiteCredentials;
use anyhow::{Result, anyhow};
use std::{env, time::Duration};
use zeroize::Zeroizing;

pub const API_KEY: &str = "susanta:kite_api_key";
pub const ACCESS_TOKEN_KEY: &str = "susanta:kite_access_token";
pub const DEFAULT_REDIS_URL: &str = "redis://127.0.0.1:6379/0";
const TIMEOUT: Duration = Duration::from_secs(3);

pub fn load_from_env() -> Result<KiteCredentials> {
    let url = Zeroizing::new(match env::var("KITE_REDIS_URL") {
        Ok(value) => value,
        Err(env::VarError::NotPresent) => DEFAULT_REDIS_URL.to_owned(),
        Err(_) => return Err(anyhow!("KITE_REDIS_URL is not valid Unicode")),
    });
    load_from_url(&url)
}

/// Reads both keys in one MGET. Never returns Redis error bodies or connection URLs.
pub fn load_from_url(url: &str) -> Result<KiteCredentials> {
    load_keys(url, API_KEY, ACCESS_TOKEN_KEY)
}
pub fn load_sandbox() -> Result<KiteCredentials> {
    let url = kite_journal::connection::url_from_env()?;
    load_sandbox_at(&url).map_err(|_| anyhow!("Sandbox Redis credentials missing or invalid"))
}
fn load_keys(url: &str, api_key_name: &str, token_name: &str) -> Result<KiteCredentials> {
    let client = ::redis::Client::open(url)
        .map_err(|_| anyhow!("Invalid Redis connection configuration"))?;
    let mut connection = client
        .get_connection_with_timeout(TIMEOUT)
        .map_err(|_| anyhow!("Redis connection or authentication failed"))?;
    connection
        .set_read_timeout(Some(TIMEOUT))
        .map_err(|_| anyhow!("Could not set Redis read timeout"))?;
    connection
        .set_write_timeout(Some(TIMEOUT))
        .map_err(|_| anyhow!("Could not set Redis write timeout"))?;
    let (api_key, access_token): (Option<String>, Option<String>) = ::redis::cmd("MGET")
        .arg(api_key_name)
        .arg(token_name)
        .query(&mut connection)
        .map_err(|_| anyhow!("Redis credential read failed"))?;
    KiteCredentials::new(api_key, access_token)
}

pub(crate) fn load_sandbox_at(url: &str) -> Result<KiteCredentials> {
    load_keys(url, "sandbox:kite_api_key", "sandbox:kite_access_token")
        .map_err(|_| anyhow!("Sandbox Redis credentials missing or invalid"))
}
