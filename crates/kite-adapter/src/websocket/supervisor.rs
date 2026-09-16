use super::{models::BinaryFrame, parser, transport};
use crate::{
    credentials::KiteCredentials,
    mapping::market_data::{self, Snapshot},
};
use anyhow::{Result, anyhow, ensure};
use chrono::Utc;
use futures_util::SinkExt;
use serde::Serialize;
use tokio::time::{Duration, Instant, sleep_until, timeout_at};
use tokio_tungstenite::tungstenite::Message;

#[derive(Debug, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum FeedEvent {
    Connected { generation: u32 },
    Gap { generation: u32 },
    Snapshot(Box<Snapshot>),
}

#[derive(Debug, Default, Serialize)]
pub struct Summary {
    pub connection_generations: u32,
    pub gaps: u32,
    pub heartbeats: u64,
    pub ticks: u64,
    pub full_ticks: u64,
    pub fresh_full_ticks: u64,
    pub ignored_order_updates: u64,
    pub ignored_notices: u64,
    pub final_source_fresh: bool,
    pub live_orders_enabled: bool,
    #[serde(skip)]
    last_exchange_timestamp: Option<u32>,
    #[serde(skip)]
    current_generation_has_full: bool,
}

fn process_binary(
    bytes: &[u8],
    token: u32,
    generation: u32,
    summary: &mut Summary,
    on_event: &mut impl FnMut(FeedEvent),
) -> Result<()> {
    match parser::parse(bytes)? {
        BinaryFrame::Heartbeat => summary.heartbeats += 1,
        BinaryFrame::Ticks(ticks) => {
            for tick in ticks {
                ensure!(
                    tick.instrument_token == token,
                    "Received an unexpected instrument token"
                );
                summary.ticks += 1;
                let snapshot = market_data::snapshot(&tick, Utc::now(), generation);
                if tick.full.is_some() {
                    summary.full_ticks += 1;
                    summary.fresh_full_ticks += u64::from(snapshot.source_fresh);
                    summary.last_exchange_timestamp = snapshot.exchange_timestamp;
                    summary.current_generation_has_full = true;
                }
                on_event(FeedEvent::Snapshot(Box::new(snapshot)));
            }
        }
    }
    Ok(())
}

fn process_text(text: &str, summary: &mut Summary) -> Result<()> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|_| anyhow!("Invalid Kite WebSocket text message"))?;
    match value.get("type").and_then(|v| v.as_str()) {
        Some("error") => Err(anyhow!(
            "Kite WebSocket reported an error; check session or subscription"
        )),
        Some("order") => {
            summary.ignored_order_updates += 1;
            Ok(())
        }
        _ => {
            summary.ignored_notices += 1;
            Ok(())
        }
    }
}

/// One socket at a time; at most two reconnects. Initial/handshake failure is terminal.
/// This diagnostic reports observations; it does not declare a Nautilus node ready.
pub async fn observe(
    credentials: &KiteCredentials,
    token: u32,
    duration: Duration,
    on_event: impl FnMut(FeedEvent),
) -> Result<Summary> {
    observe_at(
        credentials,
        token,
        duration,
        on_event,
        "wss://ws.kite.trade",
    )
    .await
}

async fn observe_impl(
    credentials: &KiteCredentials,
    token: u32,
    duration: Duration,
    mut on_event: impl FnMut(FeedEvent),
    endpoint: &str,
    initial_socket: Option<transport::Socket>,
) -> Result<Summary> {
    ensure!(token > 0, "Zero subscription token");
    ensure!(
        (1..=86400).contains(&duration.as_secs()),
        "Duration must be 1..86400 seconds"
    );
    let mut initial_socket = initial_socket;
    let deadline = Instant::now() + duration;
    let mut summary = Summary::default();
    'connections: loop {
        summary.connection_generations += 1;
        let generation = summary.connection_generations;
        let mut socket = if let Some(socket) = initial_socket.take() {
            socket
        } else {
            transport::connect_at(credentials, deadline, endpoint).await?
        };
        transport::subscribe(&mut socket, token, deadline).await?;
        summary.current_generation_has_full = false;
        summary.last_exchange_timestamp = None;
        on_event(FeedEvent::Connected { generation });
        let mut last_frame = Instant::now();
        let mut last_usable = Instant::now();
        loop {
            let message = tokio::select! {
                biased;
                _ = sleep_until(deadline) => {
                    transport::close(&mut socket).await;
                    break 'connections;
                }
                message = transport::next(&mut socket, (last_frame + Duration::from_secs(10)).min(last_usable+Duration::from_secs(10))) => message,
            };
            match message {
                Ok(Some(Message::Binary(bytes))) => {
                    last_frame = Instant::now();
                    if let Err(error) =
                        process_binary(&bytes, token, generation, &mut summary, &mut on_event)
                    {
                        transport::close(&mut socket).await;
                        return Err(error);
                    }
                    if market_data::is_fresh(
                        summary.last_exchange_timestamp,
                        Utc::now().timestamp(),
                    ) {
                        last_usable = Instant::now();
                    }
                }
                Ok(Some(Message::Text(text))) => {
                    last_frame = Instant::now();
                    if let Err(error) = process_text(&text, &mut summary) {
                        transport::close(&mut socket).await;
                        return Err(error);
                    }
                }
                Ok(Some(Message::Ping(bytes))) => {
                    last_frame = Instant::now();
                    if !matches!(
                        timeout_at(
                            deadline.min(Instant::now() + Duration::from_secs(3)),
                            socket.send(Message::Pong(bytes))
                        )
                        .await,
                        Ok(Ok(()))
                    ) {
                        break;
                    }
                }
                Ok(Some(Message::Pong(_))) => {
                    last_frame = Instant::now();
                }
                Ok(Some(Message::Close(_))) | Ok(None) | Err(_) => break,
                Ok(Some(_)) => {}
            }
        }
        transport::close(&mut socket).await;
        summary.gaps += 1;
        summary.current_generation_has_full = false;
        summary.last_exchange_timestamp = None;
        on_event(FeedEvent::Gap { generation });
        if Instant::now() >= deadline {
            break;
        }
        ensure!(
            generation < 3,
            "Kite stream disconnected repeatedly; reconnect budget exhausted"
        );
        let jitter_ms = u64::from(Utc::now().timestamp_subsec_millis() % 101);
        let backoff = Duration::from_millis(500 * u64::from(generation) + jitter_ms);
        sleep_until(deadline.min(Instant::now() + backoff)).await;
        if Instant::now() >= deadline {
            break;
        }
    }
    summary.final_source_fresh = summary.current_generation_has_full
        && market_data::is_fresh(summary.last_exchange_timestamp, Utc::now().timestamp());
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_order_payload_is_counted_without_display() {
        let mut summary = Summary::default();
        process_text(
            r#"{"type":"order","data":{"private":"sentinel"}}"#,
            &mut summary,
        )
        .unwrap();
        assert_eq!(summary.ignored_order_updates, 1);
        assert!(
            !serde_json::to_string(&summary)
                .unwrap()
                .contains("sentinel")
        );
    }
    #[test]
    fn error_payload_and_invalid_json_are_redacted() {
        for value in [
            r#"{"type":"error","data":"private-sentinel"}"#,
            "private-sentinel",
        ] {
            let error = process_text(value, &mut Summary::default())
                .unwrap_err()
                .to_string();
            assert!(!error.contains("private-sentinel"));
        }
    }
    #[test]
    fn unexpected_token_is_rejected() {
        let mut bytes = vec![0, 1, 0, 8];
        bytes.extend_from_slice(&123u32.to_be_bytes());
        bytes.extend_from_slice(&100i32.to_be_bytes());
        let mut summary = Summary::default();
        assert!(process_binary(&bytes, 456, 1, &mut summary, &mut |_| {}).is_err());
        assert_eq!(summary.ticks, 0);
    }
    #[test]
    fn heartbeat_does_not_imply_market_data_freshness() {
        let mut summary = Summary::default();
        process_binary(&[0], 123, 1, &mut summary, &mut |_| {}).unwrap();
        assert_eq!(summary.heartbeats, 1);
        assert_eq!(summary.full_ticks, 0);
        assert!(!summary.final_source_fresh);
    }
}

#[cfg(test)]
mod connection_tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;

    fn credentials() -> KiteCredentials {
        KiteCredentials::new(Some("test-key".into()), Some("test-token".into())).unwrap()
    }

    fn full_frame() -> Vec<u8> {
        let mut packet = vec![0; 184];
        packet[..4].copy_from_slice(&123u32.to_be_bytes());
        packet[4..8].copy_from_slice(&612300i32.to_be_bytes());
        packet[60..64].copy_from_slice(&(Utc::now().timestamp() as u32).to_be_bytes());
        let mut frame = vec![0, 1, 0, 184];
        frame.extend(packet);
        frame
    }

    #[tokio::test]
    async fn reconnect_replays_subscription_and_reports_gap() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("ws://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            for generation in 1..=2 {
                let (tcp, _) = listener.accept().await.unwrap();
                let mut socket = accept_async(tcp).await.unwrap();
                for expected in crate::websocket::subscription::messages(123) {
                    let actual = socket.next().await.unwrap().unwrap();
                    assert_eq!(actual.into_text().unwrap().as_str(), expected);
                }
                socket
                    .send(Message::Binary(full_frame().into()))
                    .await
                    .unwrap();
                if generation == 1 {
                    socket.close(None).await.unwrap();
                } else {
                    // Wait for deadline-driven client shutdown.
                    let message = socket.next().await.unwrap().unwrap();
                    assert!(matches!(message, Message::Close(_)));
                }
            }
        });
        let mut gaps = 0;
        let summary = observe_at(
            &credentials(),
            123,
            Duration::from_secs(2),
            |event| {
                if matches!(event, FeedEvent::Gap { .. }) {
                    gaps += 1;
                }
            },
            &endpoint,
        )
        .await
        .unwrap();
        server.await.unwrap();
        assert_eq!(summary.connection_generations, 2);
        assert_eq!(summary.gaps, 1);
        assert_eq!(gaps, 1);
        assert_eq!(summary.full_ticks, 2);
        assert!(summary.final_source_fresh);
    }

    #[tokio::test]
    async fn stops_after_three_socket_generations() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("ws://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            for _ in 0..3 {
                let (tcp, _) = listener.accept().await.unwrap();
                let mut socket = accept_async(tcp).await.unwrap();
                for _ in 0..2 {
                    socket.next().await.unwrap().unwrap();
                }
                socket.close(None).await.unwrap();
            }
        });
        let error = observe_at(
            &credentials(),
            123,
            Duration::from_secs(5),
            |_| {},
            &endpoint,
        )
        .await
        .unwrap_err()
        .to_string();
        server.await.unwrap();
        assert!(error.contains("reconnect budget exhausted"));
    }

    #[tokio::test]
    async fn heartbeat_only_run_is_not_data_ready() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("ws://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(tcp).await.unwrap();
            for _ in 0..2 {
                socket.next().await.unwrap().unwrap();
            }
            socket.send(Message::Binary(vec![0].into())).await.unwrap();
            socket.next().await;
        });
        let summary = observe_at(
            &credentials(),
            123,
            Duration::from_secs(1),
            |_| {},
            &endpoint,
        )
        .await
        .unwrap();
        server.await.unwrap();
        assert_eq!(summary.full_ticks, 0);
        assert_eq!(summary.heartbeats, 1);
        assert!(!summary.final_source_fresh);
    }
}

async fn observe_at(
    credentials: &KiteCredentials,
    token: u32,
    duration: Duration,
    on_event: impl FnMut(FeedEvent),
    endpoint: &str,
) -> Result<Summary> {
    observe_impl(credentials, token, duration, on_event, endpoint, None).await
}
pub async fn observe_connected(
    credentials: &KiteCredentials,
    token: u32,
    duration: Duration,
    on_event: impl FnMut(FeedEvent),
    socket: transport::Socket,
) -> Result<Summary> {
    observe_impl(
        credentials,
        token,
        duration,
        on_event,
        "wss://ws.kite.trade",
        Some(socket),
    )
    .await
}

pub async fn observe_sandbox_connected(
    credentials: &KiteCredentials,
    user_id: &str,
    token: u32,
    duration: Duration,
    on_event: impl FnMut(FeedEvent),
    socket: transport::Socket,
) -> Result<Summary> {
    observe_impl(
        credentials,
        token,
        duration,
        on_event,
        &transport::sandbox_endpoint(user_id)?,
        Some(socket),
    )
    .await
}
