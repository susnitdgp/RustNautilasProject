//! Opt-in, best-effort Slack alerts. Never perform HTTP on the trading thread.
use anyhow::{Result, anyhow, ensure};
use std::{io::Read, sync::mpsc, thread, time::Duration};
use zeroize::Zeroizing;

pub struct Alerts {
    sender: Option<mpsc::SyncSender<String>>,
    worker: Option<thread::JoinHandle<()>>,
}
fn validate(raw: &str) -> Result<()> {
    let url =
        reqwest::Url::parse(raw).map_err(|_| anyhow!("Invalid Slack webhook configuration"))?;
    ensure!(
        url.scheme() == "https"
            && url.host_str() == Some("hooks.slack.com")
            && url.port().is_none()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url.path().starts_with("/services/")
            && url.path().split('/').count() == 5,
        "Slack webhook must use the official HTTPS incoming-webhook endpoint"
    );
    Ok(())
}
fn client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(3))
        .build()
        .map_err(|_| anyhow!("Cannot initialize Slack HTTPS client"))
}
fn deliver(client: &reqwest::blocking::Client, endpoint: &str, text: &str) -> Result<()> {
    let response = client
        .post(endpoint)
        .header("Content-Type", "application/json")
        .body(serde_json::json!({"text": text}).to_string())
        .send()
        .map_err(|_| anyhow!("Slack delivery failed or timed out"))?;
    ensure!(
        response.status() == reqwest::StatusCode::OK,
        "Slack rejected alert"
    );
    let mut body = String::new();
    response
        .take(33)
        .read_to_string(&mut body)
        .map_err(|_| anyhow!("Slack response unreadable"))?;
    ensure!(body.trim() == "ok", "Slack did not acknowledge alert");
    Ok(())
}
impl Alerts {
    pub fn from_env(sim: bool) -> Result<Self> {
        let disabled = Self {
            sender: None,
            worker: None,
        };
        if sim {
            return Ok(disabled);
        }
        match std::env::var("KITE_SLACK_ALERTS").as_deref() {
            Err(std::env::VarError::NotPresent) | Ok("0") => return Ok(disabled),
            Ok("1") => {}
            _ => return Err(anyhow!("KITE_SLACK_ALERTS must be 0 or 1")),
        }
        let mut con =
            kite_journal::connection::connect(&kite_journal::connection::url_from_env()?)?;
        let value: Option<String> = redis::cmd("GET")
            .arg("susanta:slack_webhook_url")
            .query(&mut con)
            .map_err(|_| anyhow!("Cannot load Slack webhook from Redis"))?;
        let secret =
            Zeroizing::new(value.ok_or_else(|| anyhow!("Slack enabled but webhook is missing"))?);
        validate(&secret)?;
        let (sender, receiver) = mpsc::sync_channel::<String>(4);
        let worker = thread::Builder::new()
            .name("slack-alerts".into())
            .spawn(move || {
                let Ok(client) = client() else {
                    eprintln!("[ALERT] Slack client unavailable");
                    return;
                };
                for text in receiver {
                    if deliver(&client, &secret, &text).is_err() {
                        eprintln!("[ALERT] Slack delivery unconfirmed; inspect local logs");
                    }
                    // No retries: a timeout may follow a successfully posted message.
                    thread::sleep(Duration::from_secs(1));
                }
            })
            .map_err(|_| anyhow!("Cannot start Slack alert worker"))?;
        eprintln!("[ALERT] Slack enabled; delivery is best-effort");
        Ok(Self {
            sender: Some(sender),
            worker: Some(worker),
        })
    }
    pub fn emit(&self, text: String) {
        if let Some(sender) = &self.sender
            && sender.try_send(text).is_err()
        {
            eprintln!("[ALERT] Slack queue unavailable/full; alert not queued");
        }
    }
}
impl Drop for Alerts {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_non_slack_insecure_and_redirectable_configuration() {
        for raw in [
            "http://hooks.slack.com/services/T/B/S",
            "https://example.com/services/T/B/S",
            "https://hooks.slack.com.evil.test/services/T/B/S",
            "https://name@hooks.slack.com/services/T/B/S",
            "https://hooks.slack.com/services/T/B/S?extra=secret",
        ] {
            assert!(validate(raw).is_err());
        }
        assert!(validate("https://hooks.slack.com/services/TEST/TEST/NOT_A_SECRET").is_ok());
    }
    #[test]
    fn payload_escapes_text_and_requires_slack_acknowledgement() {
        use std::io::Write;
        for (status, body, expected) in [
            ("200 OK", "ok", true),
            ("200 OK", "invalid", false),
            ("429 Too Many Requests", "rate_limited", false),
            ("302 Found", "redirect", false),
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = format!("http://{}", listener.local_addr().unwrap());
            let text = "quote \" slash \\ newline\n";
            let payload = serde_json::json!({"text": text}).to_string();
            let server = thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut received = Vec::new();
                let mut buf = [0u8; 1024];
                while !received.ends_with(payload.as_bytes()) {
                    let n = socket.read(&mut buf).unwrap();
                    assert!(n > 0);
                    received.extend_from_slice(&buf[..n]);
                }
                write!(
                    socket,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            });
            assert_eq!(
                deliver(&client().unwrap(), &endpoint, text).is_ok(),
                expected
            );
            server.join().unwrap();
        }
    }
}
