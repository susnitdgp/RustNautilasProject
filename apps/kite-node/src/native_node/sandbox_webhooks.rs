//! Opt-in lifecycle hooks for the two external crude-oil sandbox strategies.
//!
//! This controller is intentionally used only by the sandbox runner. It never
//! runs for production, mock, paper, or market-data-only commands.
use anyhow::{Result, anyhow, ensure};
use kite_adapter::execution::native_client::production::{SandboxWebhook, SandboxWebhooks};
use serde::Serialize;
use serde_json::Value;
use std::{fs, time::Duration};

const ALLOWED_HOST: &str = "susbull.awsgoswami.com";
const START_MODE: &str = "sandbox";
const SHORT_NAME: &str = "CRUDEOIL-SHORT";
const LONG_NAME: &str = "CRUDEOIL-LONG";

#[derive(Clone, Debug)]
struct Endpoint {
    name: String,
    url: String,
}

#[derive(Debug)]
pub struct Controller {
    enabled: bool,
    endpoints: Vec<Endpoint>,
}

pub struct Session {
    client: Option<reqwest::Client>,
    endpoints: Vec<Endpoint>,
    started: Vec<usize>,
}

#[derive(Serialize)]
struct StartCommand {
    action: &'static str,
    mode: &'static str,
}

#[derive(Serialize)]
struct StopCommand {
    action: &'static str,
}

impl Controller {
    pub fn load(path: &str) -> Result<Self> {
        let text = fs::read_to_string(path)
            .map_err(|_| anyhow!("Cannot read sandbox webhook configuration"))?;
        let root: Value = serde_json::from_str(&text)
            .map_err(|_| anyhow!("Invalid sandbox webhook configuration"))?;
        ensure!(
            root.is_object(),
            "Sandbox webhook configuration must be a JSON object"
        );
        let hooks: SandboxWebhooks = match root.get("sandbox_webhooks") {
            None | Some(Value::Null) => SandboxWebhooks::default(),
            Some(value) => serde_json::from_value(value.clone())
                .map_err(|_| anyhow!("Invalid sandbox webhook configuration"))?,
        };

        let mut endpoints = Vec::with_capacity(hooks.strategies.len());
        for (name, endpoint) in hooks.strategies {
            ensure!(
                matches!(name.as_str(), SHORT_NAME | LONG_NAME),
                "Unsupported sandbox webhook strategy"
            );
            validate_url(&endpoint)?;
            endpoints.push(Endpoint {
                name,
                url: endpoint.url,
            });
        }
        endpoints.sort_by(|a, b| a.name.cmp(&b.name));
        if hooks.enabled {
            ensure!(
                endpoints.len() == 2
                    && endpoints.iter().any(|e| e.name == SHORT_NAME)
                    && endpoints.iter().any(|e| e.name == LONG_NAME),
                "Enabled sandbox webhooks require CRUDEOIL-SHORT and CRUDEOIL-LONG"
            );
        }
        Ok(Self {
            enabled: hooks.enabled,
            endpoints,
        })
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub async fn start(&self) -> Result<Session> {
        if !self.enabled {
            return Ok(Session {
                client: None,
                endpoints: self.endpoints.clone(),
                started: Vec::new(),
            });
        }
        let mut session = Session {
            client: Some(client()?),
            endpoints: self.endpoints.clone(),
            started: Vec::new(),
        };
        for index in 0..session.endpoints.len() {
            let endpoint = session.endpoints[index].clone();
            // A lost response may follow a successful start; include it in cleanup.
            session.started.push(index);
            if let Err(error) = post(
                session.client.as_ref().expect("enabled client"),
                &endpoint,
                &StartCommand {
                    action: "start",
                    mode: START_MODE,
                },
            )
            .await
            {
                let cleanup = session.stop().await;
                return Err(match cleanup {
                    Ok(()) => anyhow!(
                        "Sandbox strategy webhook start failed for {}: {error}",
                        endpoint.name
                    ),
                    Err(_) => anyhow!(
                        "Sandbox strategy webhook start and cleanup were not confirmed; review required"
                    ),
                });
            }
        }
        Ok(session)
    }
}

impl Session {
    pub async fn stop(&mut self) -> Result<()> {
        let Some(client) = &self.client else {
            self.started.clear();
            return Ok(());
        };
        let mut first_error = None;
        while let Some(index) = self.started.pop() {
            if let Err(error) = post(
                client,
                &self.endpoints[index],
                &StopCommand { action: "stop" },
            )
            .await
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        match first_error {
            Some(error) => Err(anyhow!(
                "Sandbox strategy webhook stop failed; review required: {error}"
            )),
            None => Ok(()),
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if !self.started.is_empty() {
            eprintln!("Sandbox strategy webhook session was not stopped; review required");
        }
    }
}

fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| anyhow!("Cannot initialize sandbox webhook client"))
}

async fn post<T: Serialize>(
    client: &reqwest::Client,
    endpoint: &Endpoint,
    command: &T,
) -> Result<()> {
    let body = serde_json::to_vec(command)
        .map_err(|_| anyhow!("Cannot encode sandbox strategy webhook command"))?;
    let response = client
        .post(&endpoint.url)
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|_| anyhow!("Sandbox strategy webhook request failed or timed out"))?;
    ensure!(
        response.status().is_success(),
        "Sandbox strategy webhook rejected request"
    );
    Ok(())
}

fn validate_url(endpoint: &SandboxWebhook) -> Result<()> {
    let url = reqwest::Url::parse(&endpoint.url)
        .map_err(|_| anyhow!("Invalid sandbox strategy webhook URL"))?;
    ensure!(
        matches!(url.scheme(), "http" | "https")
            && url.host_str() == Some(ALLOWED_HOST)
            && url.port().is_none()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "Sandbox strategy webhook must use the approved host without redirects or query data"
    );
    let segments: Vec<_> = url
        .path_segments()
        .ok_or_else(|| anyhow!("Invalid sandbox strategy webhook path"))?
        .collect();
    ensure!(
        segments.len() == 3
            && segments[0] == "webhook"
            && segments[1] == "strategy"
            && segments[2].starts_with("obwh_")
            && (6..=128).contains(&segments[2].len())
            && segments[2]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')),
        "Invalid sandbox strategy webhook path"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn endpoint(url: &str) -> SandboxWebhook {
        SandboxWebhook { url: url.into() }
    }

    #[test]
    fn validates_only_the_approved_strategy_webhook_shape() {
        assert!(
            validate_url(&endpoint(
                "http://susbull.awsgoswami.com/webhook/strategy/obwh_TEST_123"
            ))
            .is_ok()
        );
        for url in [
            "https://example.com/webhook/strategy/obwh_TEST",
            "http://susbull.awsgoswami.com/webhook/strategy/obwh_TEST?x=1",
            "http://susbull.awsgoswami.com/webhook/strategy/obwh_TEST/extra",
            "http://user@susbull.awsgoswami.com/webhook/strategy/obwh_TEST",
            "http://susbull.awsgoswami.com/other/obwh_TEST",
        ] {
            assert!(validate_url(&endpoint(url)).is_err());
        }
    }

    #[test]
    fn commands_are_exact_and_do_not_carry_production_mode() {
        assert_eq!(
            serde_json::to_value(StartCommand {
                action: "start",
                mode: START_MODE,
            })
            .unwrap(),
            serde_json::json!({"action":"start","mode":"sandbox"})
        );
        assert_eq!(
            serde_json::to_value(StopCommand { action: "stop" }).unwrap(),
            serde_json::json!({"action":"stop"})
        );
    }

    #[test]
    fn missing_webhook_section_is_disabled() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "{{}}").unwrap();
        let config = Controller::load(file.path().to_str().unwrap()).unwrap();
        assert!(!config.enabled());
    }

    #[test]
    fn enabled_configuration_requires_both_named_strategies() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            "{}",
            serde_json::json!({
                "sandbox_webhooks": {
                    "enabled": true,
                    "strategies": {
                        "CRUDEOIL-SHORT": {"url": "http://susbull.awsgoswami.com/webhook/strategy/obwh_SHORT"},
                        "CRUDEOIL-LONG": {"url": "http://susbull.awsgoswami.com/webhook/strategy/obwh_LONG"}
                    }
                }
            })
        )
        .unwrap();
        let config = Controller::load(file.path().to_str().unwrap()).unwrap();
        assert!(config.enabled());

        let mut incomplete = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            incomplete,
            "{}",
            serde_json::json!({
                "sandbox_webhooks": {
                    "enabled": true,
                    "strategies": {"CRUDEOIL-SHORT": {"url": "http://susbull.awsgoswami.com/webhook/strategy/obwh_SHORT"}}
                }
            })
        )
        .unwrap();
        assert!(Controller::load(incomplete.path().to_str().unwrap()).is_err());
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[tokio::test]
    async fn lost_start_response_stops_attempted_strategy_without_retry() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut bodies = Vec::new();
            for attempt in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                loop {
                    let mut byte = [0];
                    socket.read_exact(&mut byte).await.unwrap();
                    bytes.push(byte[0]);
                    if bytes.ends_with(b"\r\n\r\n") {
                        break;
                    }
                }
                let headers = String::from_utf8(bytes).unwrap();
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse().unwrap())
                    })
                    .unwrap();
                let mut body = vec![0; length];
                socket.read_exact(&mut body).await.unwrap();
                bodies.push(serde_json::from_slice::<Value>(&body).unwrap());
                if attempt == 1 {
                    socket
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        )
                        .await
                        .unwrap();
                }
                // First response deliberately lost after the request was received.
            }
            bodies
        });
        let controller = Controller {
            enabled: true,
            endpoints: vec![Endpoint {
                name: SHORT_NAME.into(),
                url: endpoint,
            }],
        };
        assert!(controller.start().await.is_err());
        let bodies = tokio::time::timeout(Duration::from_secs(10), server)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            bodies,
            vec![
                serde_json::json!({"action":"start","mode":"sandbox"}),
                serde_json::json!({"action":"stop"}),
            ]
        );
    }
}
