use super::request::{Command, broker_id};
use crate::credentials::KiteCredentials;
use anyhow::{Result, anyhow, ensure};
use reqwest::{
    Client, StatusCode,
    header::{AUTHORIZATION, HeaderValue},
};
use serde::Deserialize;
use std::time::Duration;
use zeroize::Zeroizing;
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Acknowledged { order_id: String },
    Rejected,
    SessionExpired,
    RateLimited { retry_after_ms: u64 },
    Unknown,
}
pub struct KiteOrderTransport {
    client: Client,
    authorization: HeaderValue,
}
impl KiteOrderTransport {
    pub fn new(credentials: &KiteCredentials) -> Result<Self> {
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
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| anyhow!("Kite order client initialization failed"))?;
        Ok(Self {
            client,
            authorization,
        })
    }
    pub async fn execute(&self, command: &Command) -> Result<Outcome> {
        ensure!(
            cfg!(feature = "live-orders"),
            "Live Kite order execution is disabled in this build"
        );
        self.send_at(command, "https://api.kite.trade").await
    }
    async fn send_at(&self, command: &Command, base: &str) -> Result<Outcome> {
        command.validate()?;
        let (method, path, fields) = command.wire();
        let request = self
            .client
            .request(method, format!("{base}{path}"))
            .header("X-Kite-Version", "3")
            .header(AUTHORIZATION, self.authorization.clone());
        let request = if fields.is_empty() {
            request
        } else {
            request.form(&fields)
        };
        let mut response = match request.send().await {
            Ok(r) => r,
            Err(_) => return Ok(Outcome::Unknown),
        };
        let status = response.status();
        if status == StatusCode::TOO_MANY_REQUESTS {
            let seconds = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(10)
                .clamp(1, 86400);
            return Ok(Outcome::RateLimited {
                retry_after_ms: seconds * 1000,
            });
        }
        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            return Ok(Outcome::SessionExpired);
        }
        // No automatic retries. Server/proxy failures may follow an accepted mutation.
        if !status.is_success() {
            return Ok(if matches!(status.as_u16(), 400 | 404 | 405 | 422) {
                Outcome::Rejected
            } else {
                Outcome::Unknown
            });
        }
        let mut body = Zeroizing::new(Vec::new());
        loop {
            match response.chunk().await {
                Ok(Some(bytes)) => {
                    if body.len() + bytes.len() > 65536 {
                        return Ok(Outcome::Unknown);
                    }
                    body.extend_from_slice(&bytes);
                }
                Ok(None) => break,
                Err(_) => return Ok(Outcome::Unknown),
            }
        }
        #[derive(Deserialize)]
        struct Envelope {
            status: String,
            data: Option<Data>,
        }
        #[derive(Deserialize)]
        struct Data {
            order_id: String,
        }
        let parsed: Envelope = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(_) => return Ok(Outcome::Unknown),
        };
        let Some(data) = parsed.data else {
            return Ok(Outcome::Unknown);
        };
        if parsed.status != "success"
            || broker_id(&data.order_id).is_err()
            || command.expected_id().is_some_and(|id| id != data.order_id)
        {
            return Ok(Outcome::Unknown);
        }
        Ok(Outcome::Acknowledged {
            order_id: data.order_id,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    fn creds() -> KiteCredentials {
        KiteCredentials::new(Some("test-key".into()), Some("test-token".into())).unwrap()
    }
    async fn check(command: Command, status: u16, body: &str, expected: &str) -> Outcome {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let body = body.to_owned();
        let expected = expected.to_owned();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut data = Vec::new();
            while !data.ends_with(b"\r\n\r\n") {
                data.push(stream.read_u8().await.unwrap());
                assert!(data.len() < 8192);
            }
            let headers = String::from_utf8(data).unwrap().to_ascii_lowercase();
            assert!(headers.starts_with(&expected));
            assert!(headers.contains("authorization: token test-key:test-token"));
            assert!(headers.contains("x-kite-version: 3"));
            let length = headers
                .lines()
                .find_map(|s| {
                    s.strip_prefix("content-length: ")
                        .and_then(|s| s.parse::<usize>().ok())
                })
                .unwrap_or(0);
            let mut form = vec![0; length];
            stream.read_exact(&mut form).await.unwrap();
            if length > 0 {
                let form = String::from_utf8(form).unwrap();
                assert!(form.contains("quantity=1"));
                assert!(form.contains("price=6000"));
                assert!(form.contains("order_type=LIMIT"));
            }
            let response = format!(
                "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nRetry-After: 12\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let result = KiteOrderTransport::new(&creds())
            .unwrap()
            .send_at(&command, &endpoint)
            .await
            .unwrap();
        task.await.unwrap();
        result
    }
    #[tokio::test]
    async fn methods_headers_form_and_acknowledgements() {
        let commands = [
            (
                Command::Place {
                    symbol: "CRUDEOIL26SEPFUT".into(),
                    side: "BUY".into(),
                    product: "NRML".into(),
                    quantity: 1,
                    price_rupees: 6000,
                    tag: "Test1".into(),
                },
                "post /orders/regular ",
            ),
            (
                Command::Modify {
                    order_id: "123".into(),
                    quantity: 1,
                    price_rupees: 6000,
                },
                "put /orders/regular/123 ",
            ),
            (
                Command::Cancel {
                    order_id: "123".into(),
                },
                "delete /orders/regular/123 ",
            ),
        ];
        for (cmd, method) in commands {
            assert_eq!(
                check(
                    cmd,
                    200,
                    r#"{"status":"success","data":{"order_id":"123"}}"#,
                    method
                )
                .await,
                Outcome::Acknowledged {
                    order_id: "123".into()
                }
            );
        }
    }
    #[tokio::test]
    async fn errors_and_bad_success_are_classified_without_body() {
        for (status, body, outcome) in [
            (
                429,
                "private",
                Outcome::RateLimited {
                    retry_after_ms: 12000,
                },
            ),
            (403, "private", Outcome::SessionExpired),
            (400, "private", Outcome::Rejected),
            (502, "private", Outcome::Unknown),
            (200, "private", Outcome::Unknown),
            (
                200,
                r#"{"status":"success","data":{"order_id":"456"}}"#,
                Outcome::Unknown,
            ),
        ] {
            assert_eq!(
                check(
                    Command::Cancel {
                        order_id: "123".into()
                    },
                    status,
                    body,
                    "delete /orders/regular/123 "
                )
                .await,
                outcome
            );
        }
    }
    #[tokio::test]
    async fn disabled_build_blocks_before_network() {
        if !cfg!(feature = "live-orders") {
            assert!(
                KiteOrderTransport::new(&creds())
                    .unwrap()
                    .execute(&Command::Cancel {
                        order_id: "123".into()
                    })
                    .await
                    .is_err()
            );
        }
    }
    #[tokio::test]
    async fn lost_response_is_unknown_without_retry() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            while !bytes.ends_with(b"\r\n\r\n") {
                bytes.push(stream.read_u8().await.unwrap());
            }
            drop(stream);
            assert!(
                tokio::time::timeout(Duration::from_millis(150), listener.accept())
                    .await
                    .is_err()
            );
        });
        let result = KiteOrderTransport::new(&creds())
            .unwrap()
            .send_at(
                &Command::Cancel {
                    order_id: "123".into(),
                },
                &endpoint,
            )
            .await
            .unwrap();
        assert_eq!(result, Outcome::Unknown);
        task.await.unwrap();
    }
}
