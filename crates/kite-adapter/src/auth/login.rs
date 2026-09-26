//! Kite Connect interactive login and request-token exchange.
//!
//! Secrets are never printed. The resulting access token is durably written to
//! susanta:kite_access_token in Redis.
use crate::credentials::redis::{ACCESS_TOKEN_KEY, API_KEY};
use anyhow::{Result, anyhow, ensure};
use reqwest::{StatusCode, Url, blocking::Client};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    env,
    fmt::Write as _,
    io::{self, Read, Write},
    time::Duration,
};
use zeroize::Zeroizing;

const LOGIN_URL: &str = "https://kite.zerodha.com/connect/login";
const TOKEN_URL: &str = "https://api.kite.trade/session/token";
const API_SECRET_ENV: &str = "KITE_API_SECRET";
const API_SECRET_KEY: &str = "susanta:kite_api_secret";
const MAX_BODY: u64 = 65_536;

#[derive(Debug, PartialEq, Eq)]
pub struct ExchangeResult {
    pub user_id: String,
}

#[derive(Deserialize)]
struct TokenEnvelope {
    status: String,
    data: Option<TokenData>,
}

#[derive(Deserialize)]
struct TokenData {
    user_id: String,
    api_key: String,
    access_token: String,
}

pub fn login_url() -> Result<String> {
    let api_key = redis_secret(API_KEY, "Kite API key")?;
    login_url_for(&api_key)
}

fn login_url_for(api_key: &str) -> Result<String> {
    validate_secret(api_key, "Kite API key")?;
    let mut url = Url::parse(LOGIN_URL).map_err(|_| anyhow!("Invalid Kite login endpoint"))?;
    url.query_pairs_mut()
        .append_pair("v", "3")
        .append_pair("api_key", api_key);
    Ok(url.into())
}

pub fn interactive() -> Result<ExchangeResult> {
    // Resolve the permanent secret before asking the user to login, so a
    // missing setup fails early instead of wasting a short-lived request token.
    let api_secret = api_secret()?;
    let api_key = redis_secret(API_KEY, "Kite API key")?;
    let url = login_url_for(&api_key)?;

    println!("Open this URL in your browser and complete the Kite login:\n{url}");
    print!("Paste the returned request_token or full redirect URL, then press Enter: ");
    io::stdout()
        .flush()
        .map_err(|_| anyhow!("Could not write login prompt"))?;

    let mut input = Zeroizing::new(String::new());
    io::stdin()
        .read_line(&mut input)
        .map_err(|_| anyhow!("Could not read Kite request token"))?;
    exchange_and_store(&api_key, &api_secret, &input)
}

pub fn exchange_and_store(
    api_key: &str,
    api_secret: &str,
    request_token_or_url: &str,
) -> Result<ExchangeResult> {
    validate_secret(api_key, "Kite API key")?;
    validate_secret(api_secret, "Kite API secret")?;
    let request_token = Zeroizing::new(extract_request_token(request_token_or_url)?);
    let checksum = Zeroizing::new(checksum(api_key, &request_token, api_secret));

    let client = Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .https_only(true)
        .build()
        .map_err(|_| anyhow!("Could not initialize Kite authentication client"))?;

    let form = [
        ("api_key", api_key),
        ("request_token", request_token.as_str()),
        ("checksum", checksum.as_str()),
    ];
    let mut response = client
        .post(TOKEN_URL)
        .header("X-Kite-Version", "3")
        .form(&form)
        .send()
        .map_err(|_| anyhow!("Kite token exchange request failed"))?;

    if response.status() == StatusCode::UNAUTHORIZED || response.status() == StatusCode::FORBIDDEN {
        return Err(anyhow!(
            "Kite rejected the request token or checksum; obtain a fresh request_token"
        ));
    }
    ensure!(
        response.status().is_success(),
        "Kite token service returned an unsuccessful status"
    );

    let mut body = Zeroizing::new(Vec::new());
    response
        .by_ref()
        .take(MAX_BODY + 1)
        .read_to_end(&mut body)
        .map_err(|_| anyhow!("Could not read Kite token response"))?;
    ensure!(
        body.len() as u64 <= MAX_BODY,
        "Kite token response exceeds size limit"
    );

    let envelope: TokenEnvelope =
        serde_json::from_slice(&body).map_err(|_| anyhow!("Invalid Kite token response"))?;
    ensure!(envelope.status == "success", "Kite token exchange failed");
    let data = envelope
        .data
        .ok_or_else(|| anyhow!("Kite token response is missing data"))?;
    ensure!(
        data.api_key == api_key,
        "Kite token response API key mismatch"
    );
    ensure!(
        !data.user_id.trim().is_empty() && data.user_id.len() <= 64,
        "Kite token response is missing account identity"
    );
    let access_token = Zeroizing::new(data.access_token);
    validate_secret(&access_token, "Kite access token")?;
    store_access_token(&access_token)?;

    Ok(ExchangeResult {
        user_id: data.user_id,
    })
}

fn api_secret() -> Result<Zeroizing<String>> {
    match env::var(API_SECRET_ENV) {
        Ok(value) => {
            let value = Zeroizing::new(value);
            validate_secret(&value, "Kite API secret")?;
            Ok(value)
        }
        Err(env::VarError::NotPresent) => redis_secret(API_SECRET_KEY, "Kite API secret").map_err(
            |_| {
                anyhow!(
                    "Kite API secret is unavailable; set KITE_API_SECRET or Redis key susanta:kite_api_secret"
                )
            },
        ),
        Err(_) => Err(anyhow!("KITE_API_SECRET is not valid Unicode")),
    }
}

fn redis_secret(key: &str, label: &str) -> Result<Zeroizing<String>> {
    let url = kite_journal::connection::url_from_env()?;
    let mut connection = kite_journal::connection::connect(&url)?;
    let value: Option<String> = ::redis::cmd("GET")
        .arg(key)
        .query(&mut connection)
        .map_err(|_| anyhow!("Redis credential read failed"))?;
    let value = Zeroizing::new(value.ok_or_else(|| anyhow!("{label} is missing"))?);
    validate_secret(&value, label)?;
    Ok(value)
}

fn store_access_token(access_token: &str) -> Result<()> {
    let url = kite_journal::connection::url_from_env()?;
    let mut connection = kite_journal::connection::connect(&url)?;
    ::redis::cmd("SET")
        .arg(ACCESS_TOKEN_KEY)
        .arg(access_token)
        .query::<()>(&mut connection)
        .map_err(|_| anyhow!("Redis access-token update failed"))?;
    kite_journal::connection::sync(&mut connection)
        .map_err(|_| anyhow!("Redis access-token durability confirmation failed"))?;
    Ok(())
}

fn validate_secret(value: &str, label: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 4096
            && value.bytes().all(|byte| byte.is_ascii_graphic()),
        "{label} is empty or invalid"
    );
    Ok(())
}

fn extract_request_token(input: &str) -> Result<String> {
    let input = input.trim();
    validate_secret(input, "Kite request token")?;
    if input.starts_with("https://") || input.starts_with("http://") {
        let url = Url::parse(input).map_err(|_| anyhow!("Invalid Kite redirect URL"))?;
        let token = url
            .query_pairs()
            .find_map(|(key, value)| (key == "request_token").then(|| value.into_owned()))
            .ok_or_else(|| anyhow!("Redirect URL does not contain request_token"))?;
        validate_secret(&token, "Kite request token")?;
        return Ok(token);
    }
    Ok(input.to_owned())
}

fn checksum(api_key: &str, request_token: &str, api_secret: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(api_key.as_bytes());
    hasher.update(request_token.as_bytes());
    hasher.update(api_secret.as_bytes());
    let digest = hasher.finalize();
    let mut output = String::with_capacity(64);
    for byte in digest {
        write!(&mut output, "{byte:02x}").expect("write to String");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_url_contains_only_public_api_key() {
        let url = login_url_for("public-key").unwrap();
        assert_eq!(
            url,
            "https://kite.zerodha.com/connect/login?v=3&api_key=public-key"
        );
    }

    #[test]
    fn extracts_token_from_token_or_redirect_url() {
        assert_eq!(extract_request_token("abc123").unwrap(), "abc123");
        assert_eq!(
            extract_request_token(
                "https://example.test/callback?request_token=abc%2D123&status=success"
            )
            .unwrap(),
            "abc-123"
        );
        assert!(extract_request_token("https://example.test/callback?status=success").is_err());
    }

    #[test]
    fn checksum_is_sha256_of_documented_concatenation() {
        assert_eq!(
            checksum("api", "request", "secret"),
            "257f5edc0415fc77bd14b16e08ca983df5e4d049db7c63e292f18f6d640402b5"
        );
    }

    #[test]
    fn secret_validation_rejects_empty_whitespace_and_controls() {
        for value in ["", "has space", "line\nbreak"] {
            assert!(validate_secret(value, "secret").is_err());
        }
        assert!(validate_secret("valid-_TOKEN.123", "secret").is_ok());
    }
}
