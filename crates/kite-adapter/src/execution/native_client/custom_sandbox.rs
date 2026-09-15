//! Credential-free compatibility probe for the user-selected Nordible mock.
//! This module deliberately has no execution transport or Redis credential loader.
use anyhow::{Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;
const ALLOWED_ROOT: &str = "http://94.136.191.37:3000";
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    base_url: String,
}
#[derive(Debug, Serialize)]
struct Check {
    path: String,
    http_status: Option<u16>,
    compatible: bool,
    reason: String,
}
fn assess(path: &str, status: u16, value: &Value) -> Check {
    let reason = if status != 200 {
        "missing_or_unsuccessful_route"
    } else if value.get("COLLABORATION-NEEDED").is_some() {
        "static_mock_incomplete_request_handling"
    } else if value["status"] != "success" {
        "invalid_response_envelope"
    } else {
        match path {
            "/user/profile"
                if value["data"]["user_id"]
                    .as_str()
                    .is_some_and(|s| !s.is_empty()) =>
            {
                "read_shape_matches"
            }
            "/user/margins/commodity"
                if value["data"]["enabled"].as_bool() == Some(true)
                    && value["data"]["net"].is_number() =>
            {
                "read_shape_matches"
            }
            "/quote?i=MCX%3ACRUDEOIL26SEPFUT"
                if value["data"]["MCX:CRUDEOIL26SEPFUT"]["instrument_token"].as_u64()
                    == Some(144870151) =>
            {
                "read_shape_matches"
            }
            "/orders" | "/trades" if value["data"].is_array() => "read_shape_matches",
            "/portfolio/positions"
                if value["data"]["net"]
                    .as_array()
                    .is_some_and(|ps| ps.iter().all(|p| p["quantity"].as_i64() == Some(0))) =>
            {
                "read_shape_matches"
            }
            _ => "missing_contract_fields_or_unmanaged_position",
        }
    };
    Check {
        path: path.into(),
        http_status: Some(status),
        compatible: reason == "read_shape_matches",
        reason: reason.into(),
    }
}
async fn check(client: &reqwest::Client, root: &str, path: &str) -> Result<Check> {
    // GET only; no Authorization/Cookie headers and redirects are disabled.
    let mut response = client
        .get(format!("{root}{path}"))
        .header("X-Kite-Version", "3")
        .send()
        .await
        .map_err(|_| anyhow!("Custom sandbox unavailable"))?;
    let status = response.status().as_u16();
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow!("Custom sandbox response unavailable"))?
    {
        ensure!(
            bytes.len() + chunk.len() <= 1024 * 1024,
            "Custom sandbox response exceeds limit"
        );
        bytes.extend_from_slice(&chunk);
    }
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Ok(assess(path, status, &value))
}
pub async fn probe(config_text: &str) -> Result<Value> {
    let config: Config = toml::from_str(config_text)?;
    ensure!(
        config.base_url == ALLOWED_ROOT,
        "Custom probe permits only the user-selected mock host"
    );
    let client = reqwest::Client::builder()
        .retry(reqwest::retry::never())
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(8))
        .build()?;
    let mut checks = Vec::new();
    for path in [
        "/user/profile",
        "/user/margins/commodity",
        "/orders",
        "/trades",
        "/portfolio/positions",
        "/quote?i=MCX%3ACRUDEOIL26SEPFUT",
    ] {
        checks.push(
            check(&client, &config.base_url, path)
                .await
                .unwrap_or_else(|_| Check {
                    path: path.into(),
                    http_status: None,
                    compatible: false,
                    reason: "request_failed_or_oversized".into(),
                }),
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let compatible = checks.iter().all(|c| c.compatible);
    Ok(
        serde_json::json!({"event":"custom_sandbox_compatibility","base_url":config.base_url,"provider":"user_selected_mock","read_shapes_compatible":compatible,"checks":checks,"credentials_loaded":false,"orders_sent":0,"execution_enabled":false,"live_orders_enabled":false,"next_requirement":if compatible {"Stateful lifecycle and fresh full-tick verification still required"}else{"Server must implement compatible account, contract and stateful trading APIs"}}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn static_samples_and_unrelated_quotes_cannot_pass_native_preflight() {
        assert!(!assess("/orders",200,&serde_json::json!({"status":"success","data":[],"COLLABORATION-NEEDED":"request handling missing"})).compatible);
        assert!(!assess("/quote?i=MCX%3ACRUDEOIL26SEPFUT",200,&serde_json::json!({"status":"success","data":{"NSE:INFY":{"instrument_token":408065}}})).compatible);
        assert!(!assess("/user/profile", 404, &Value::Null).compatible);
        assert!(
            !assess(
                "/portfolio/positions",
                200,
                &serde_json::json!({"status":"success","data":{"net":[{"quantity":-100}]}})
            )
            .compatible
        );
    }
    #[tokio::test]
    async fn refuses_other_hosts_before_any_network_request() {
        assert!(probe("base_url='https://api.kite.trade'").await.is_err());
    }
    #[tokio::test]
    async fn probe_sends_no_credentials_and_does_not_follow_redirects() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let root = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            while !bytes.ends_with(b"\r\n\r\n") {
                bytes.push(socket.read_u8().await.unwrap());
                assert!(bytes.len() < 8192);
            }
            let request = String::from_utf8(bytes).unwrap().to_lowercase();
            assert!(request.starts_with("get /user/profile "));
            assert!(!request.contains("authorization:"));
            assert!(!request.contains("cookie:"));
            socket.write_all(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/private\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
        });
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let r = check(&client, &root, "/user/profile").await.unwrap();
        assert_eq!(r.http_status, Some(302));
        assert!(!r.compatible);
        task.await.unwrap();
    }
}
