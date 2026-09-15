use crate::credentials::KiteCredentials;
use anyhow::{Result, anyhow, ensure};
use reqwest::{
    Client, StatusCode,
    header::{AUTHORIZATION, HeaderValue},
};
use serde::Deserialize;
use std::time::Duration;
use zeroize::Zeroizing;

const PROFILE_URL: &str = "https://api.kite.trade/user/profile";

#[derive(Deserialize)]
struct Envelope {
    status: String,
    data: Option<Profile>,
}
#[derive(Deserialize)]
struct Profile {
    user_id: String,
    exchanges: Vec<String>,
}

fn validate_body(bytes: &[u8], required_exchange: &str) -> Result<()> {
    let response: Envelope =
        serde_json::from_slice(bytes).map_err(|_| anyhow!("Invalid Kite profile response"))?;
    ensure!(
        response.status == "success",
        "Kite session validation failed"
    );
    let profile = response
        .data
        .ok_or_else(|| anyhow!("Kite profile data is missing"))?;
    ensure!(
        !profile.user_id.trim().is_empty(),
        "Kite account identity is missing"
    );
    ensure!(
        profile.exchanges.iter().any(|e| e == required_exchange),
        "Required exchange is not enabled on this Kite account"
    );
    Ok(())
}

/// Validates the session and exchange permission; never prints account details.
pub async fn validate(credentials: &KiteCredentials, required_exchange: &str) -> Result<()> {
    validate_at(credentials, required_exchange, PROFILE_URL).await
}

async fn validate_at(credentials: &KiteCredentials, exchange: &str, url: &str) -> Result<()> {
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| anyhow!("Could not initialize Kite session client"))?;
    let value = Zeroizing::new(format!(
        "token {}:{}",
        credentials.api_key(),
        credentials.access_token()
    ));
    let mut authorization =
        HeaderValue::from_str(&value).map_err(|_| anyhow!("Invalid Kite authentication header"))?;
    authorization.set_sensitive(true);
    let mut response = client
        .get(url)
        .header("X-Kite-Version", "3")
        .header(AUTHORIZATION, authorization)
        .send()
        .await
        .map_err(|_| anyhow!("Kite session request failed"))?;
    if matches!(
        response.status(),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ) {
        return Err(anyhow!("Kite session rejected; renew the token in Redis"));
    }
    ensure!(
        response.status().is_success(),
        "Kite session service returned an unsuccessful status"
    );
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow!("Kite profile read failed"))?
    {
        ensure!(
            body.len() + chunk.len() <= 65536,
            "Kite profile exceeds size limit"
        );
        body.extend_from_slice(&chunk);
    }
    validate_body(&body, exchange)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_enabled_mcx() {
        assert!(
            validate_body(
                br#"{"status":"success","data":{"user_id":"test","exchanges":["MCX"]}}"#,
                "MCX"
            )
            .is_ok()
        );
    }
    #[test]
    fn denies_missing_account_or_exchange() {
        for body in [
            r#"{"status":"success","data":{"user_id":"","exchanges":["MCX"]}}"#,
            r#"{"status":"success","data":{"user_id":"test","exchanges":["NSE"]}}"#,
            r#"{"status":"success","data":null}"#,
        ] {
            assert!(validate_body(body.as_bytes(), "MCX").is_err());
        }
    }
    #[test]
    fn never_echoes_profile_or_error_payload() {
        for body in [
            r#"{"status":"error","message":"private-sentinel","data":null}"#,
            r#"{"status":"success","data":{"user_id":"private-sentinel"}}"#,
            "private-sentinel",
        ] {
            let error = format!("{:#}", validate_body(body.as_bytes(), "MCX").unwrap_err());
            assert!(!error.contains("private-sentinel"));
        }
    }
    #[tokio::test]
    async fn sends_headers_and_redacts_http_auth_failure() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/user/profile", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            loop {
                let byte = socket.read_u8().await.unwrap();
                bytes.push(byte);
                if bytes.ends_with(b"\r\n\r\n") {
                    break;
                }
                assert!(bytes.len() < 8192);
            }
            let request = String::from_utf8(bytes).unwrap().to_ascii_lowercase();
            assert!(request.starts_with("get /user/profile "));
            assert!(request.contains("authorization: token test-key:test-token"));
            assert!(request.contains("x-kite-version: 3"));
            socket.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 16\r\nConnection: close\r\n\r\nprivate-sentinel").await.unwrap();
        });
        let creds =
            KiteCredentials::new(Some("test-key".into()), Some("test-token".into())).unwrap();
        let error = validate_at(&creds, "MCX", &endpoint)
            .await
            .unwrap_err()
            .to_string();
        task.await.unwrap();
        assert_eq!(error, "Kite session rejected; renew the token in Redis");
    }
}
