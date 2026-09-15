//! Authenticated reads and fixed virtual-contract-note calculation; no order mutation endpoints.
use crate::credentials::KiteCredentials;
use crate::execution::native_client::outage::ReadFailure;
use anyhow::{Result, anyhow, ensure};
use reqwest::{
    Client, StatusCode,
    header::{AUTHORIZATION, HeaderValue},
};
use serde::{Deserialize, de::DeserializeOwned};
use std::time::Duration;
use zeroize::Zeroizing;

pub(crate) enum Endpoint {
    Profile,
    Positions,
    Orders,
    Trades,
    CommodityMargins,
}
impl Endpoint {
    fn path(&self) -> &'static str {
        match self {
            Self::Profile => "/user/profile",
            Self::Positions => "/portfolio/positions",
            Self::Orders => "/orders",
            Self::Trades => "/trades",
            Self::CommodityMargins => "/user/margins/commodity",
        }
    }
}
pub(crate) struct ReadClient {
    #[cfg(test)]
    charge_test_url: Option<String>,
    root: &'static str,
    pace: tokio::sync::Mutex<tokio::time::Instant>,
    client: Client,
    authorization: HeaderValue,
}
impl ReadClient {
    pub(crate) fn new(credentials: &KiteCredentials) -> Result<Self> {
        let value = Zeroizing::new(format!(
            "token {}:{}",
            credentials.api_key(),
            credentials.access_token()
        ));
        let mut authorization = HeaderValue::from_str(&value)
            .map_err(|_| anyhow!("Invalid Kite authentication header"))?;
        authorization.set_sensitive(true);
        let client = Client::builder()
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| anyhow!("Kite read client initialization failed"))?;
        Ok(Self {
            #[cfg(test)]
            charge_test_url: None,
            root: "https://api.kite.trade",
            pace: tokio::sync::Mutex::new(tokio::time::Instant::now()),
            client,
            authorization,
        })
    }
    pub(crate) fn sandbox(credentials: &KiteCredentials) -> Result<Self> {
        let mut client = Self::new(credentials)?;
        client.root = "https://sandbox.kite.trade/oms";
        Ok(client)
    }
    pub(crate) async fn sandbox_quote<T: DeserializeOwned>(&self) -> Result<T> {
        ensure!(
            self.root == "https://sandbox.kite.trade/oms",
            "Sandbox quote requires sandbox client"
        );
        self.get_at("https://sandbox.kite.trade/oms/quote?i=MCX%3ACRUDEOIL26SEPFUT")
            .await
    }
    pub(crate) async fn get<T: DeserializeOwned>(&self, endpoint: Endpoint) -> Result<T> {
        self.get_at(&format!("{}{}", self.root, endpoint.path()))
            .await
    }
    #[cfg(test)]
    pub(crate) fn charges_test_url(mut self, url: String) -> Self {
        self.charge_test_url = Some(url);
        self
    }
    pub(crate) async fn charges<T: DeserializeOwned>(&self, payload: Vec<u8>) -> Result<T> {
        ensure!(
            self.root == "https://api.kite.trade",
            "Sandbox charge calculations are unavailable"
        );
        let url = "https://api.kite.trade/charges/orders";
        #[cfg(test)]
        let url = self.charge_test_url.as_deref().unwrap_or(url);

        self.read_response(
            self.client
                .post(url)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(payload),
        )
        .await
    }
    async fn get_at<T: DeserializeOwned>(&self, url: &str) -> Result<T> {
        self.read_response(self.client.get(url)).await
    }
    async fn read_response<T: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T> {
        let mut next = self.pace.lock().await;
        tokio::time::sleep_until(*next).await;
        *next = tokio::time::Instant::now() + Duration::from_millis(150);
        let mut response = request
            .header("X-Kite-Version", "3")
            .header(AUTHORIZATION, self.authorization.clone())
            .send()
            .await
            .map_err(|_| anyhow!(ReadFailure::Transient))?;
        if matches!(
            response.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ) {
            return Err(anyhow!(ReadFailure::SessionExpired));
        }
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            let seconds = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(10)
                .clamp(10, 86400);
            return Err(anyhow!(ReadFailure::RateLimited(seconds * 1000)));
        }
        if response.status().is_server_error() {
            return Err(anyhow!(ReadFailure::Transient));
        }
        ensure!(
            response.status().is_success(),
            "Kite read service returned an unsuccessful status"
        );
        let mut body = Zeroizing::new(Vec::new());
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| anyhow!(ReadFailure::Transient))?
        {
            ensure!(
                body.len() + chunk.len() <= 8 * 1024 * 1024,
                "Kite response exceeds size limit"
            );
            body.extend_from_slice(&chunk);
        }
        decode(&body)
    }
}
#[derive(Deserialize)]
struct Envelope<T> {
    status: String,
    data: Option<T>,
}
fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    let envelope: Envelope<T> =
        serde_json::from_slice(bytes).map_err(|_| anyhow!("Invalid Kite read response"))?;
    ensure!(
        envelope.status == "success",
        "Kite read response was unsuccessful"
    );
    envelope
        .data
        .ok_or_else(|| anyhow!("Kite response data is missing"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sandbox_read_root_is_fixed_and_separate_from_production() {
        let c = KiteCredentials::new(Some("sandbox-only".into()), Some("sandbox-token".into()))
            .unwrap();
        assert_eq!(
            ReadClient::sandbox(&c).unwrap().root,
            "https://sandbox.kite.trade/oms"
        );
        assert_eq!(ReadClient::new(&c).unwrap().root, "https://api.kite.trade");
    }
    #[test]
    fn rejects_malformed_missing_and_error_without_payload() {
        for bytes in [
            r#"{"status":"error","message":"private-sentinel"}"#,
            r#"{"status":"success","data":null}"#,
            "private-sentinel",
        ] {
            let error = decode::<Vec<String>>(bytes.as_bytes())
                .unwrap_err()
                .to_string();
            assert!(!error.contains("private-sentinel"));
        }
        assert!(
            decode::<Vec<String>>(br#"{"status":"success","data":[]}"#)
                .unwrap()
                .is_empty()
        );
    }
    #[tokio::test]
    async fn get_only_headers_and_auth_error_redaction() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/orders", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            while !bytes.ends_with(b"\r\n\r\n") {
                bytes.push(socket.read_u8().await.unwrap());
                assert!(bytes.len() < 8192);
            }
            let request = String::from_utf8(bytes).unwrap().to_ascii_lowercase();
            assert!(request.starts_with("get /orders "));
            assert!(request.contains("authorization: token test-key:test-token"));
            assert!(request.contains("x-kite-version: 3"));
            socket.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 16\r\nConnection: close\r\n\r\nprivate-sentinel").await.unwrap();
        });
        let credentials =
            KiteCredentials::new(Some("test-key".into()), Some("test-token".into())).unwrap();
        let error = ReadClient::new(&credentials)
            .unwrap()
            .get_at::<Vec<String>>(&url)
            .await
            .unwrap_err()
            .to_string();
        task.await.unwrap();
        assert_eq!(error, "Kite session rejected; renew the token in Redis");
    }
}
