use super::subscription;
use crate::credentials::KiteCredentials;
use anyhow::{Result, anyhow, ensure};
use futures_util::{SinkExt, StreamExt};
use tokio::{
    net::TcpStream,
    time::{Duration, Instant, timeout_at},
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_with_config,
    tungstenite::{Message, client::IntoClientRequest, protocol::WebSocketConfig},
};

pub type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// No Debug/Display of URLs, requests, error bodies or close reasons.
pub(crate) async fn connect_at(
    credentials: &KiteCredentials,
    deadline: Instant,
    endpoint: &str,
) -> Result<Socket> {
    let mut url =
        reqwest::Url::parse(endpoint).map_err(|_| anyhow!("Invalid WebSocket endpoint"))?;
    url.query_pairs_mut()
        .append_pair("api_key", credentials.api_key())
        .append_pair("access_token", credentials.access_token());
    let request = url
        .as_str()
        .into_client_request()
        .map_err(|_| anyhow!("Could not create Kite WebSocket request"))?;
    let config = WebSocketConfig::default()
        .max_message_size(Some(1_048_576))
        .max_frame_size(Some(1_048_576));
    let connection = timeout_at(
        deadline.min(Instant::now() + Duration::from_secs(10)),
        connect_async_with_config(request, Some(config), true),
    )
    .await;
    match connection {
        Ok(Ok((socket, _))) => Ok(socket),
        // All handshake failures are terminal for this diagnostic run:
        // no repeated auth/permission/quota requests.
        _ => Err(anyhow!("Kite WebSocket connection failed or timed out")),
    }
}

pub async fn subscribe(socket: &mut Socket, token: u32, deadline: Instant) -> Result<()> {
    ensure!(token != 0, "Cannot subscribe a zero instrument token");
    for message in subscription::messages(token) {
        timeout_at(
            deadline.min(Instant::now() + Duration::from_secs(3)),
            socket.send(Message::Text(message.into())),
        )
        .await
        .map_err(|_| anyhow!("Kite subscription timed out"))?
        .map_err(|_| anyhow!("Kite subscription send failed"))?;
    }
    Ok(())
}

pub async fn next(socket: &mut Socket, deadline: Instant) -> Result<Option<Message>> {
    timeout_at(deadline, socket.next())
        .await
        .map_err(|_| anyhow!("Kite stream idle timeout"))?
        .transpose()
        .map_err(|_| anyhow!("Kite stream receive failed"))
}

pub async fn close(socket: &mut Socket) {
    let _ = tokio::time::timeout(Duration::from_secs(1), socket.close(None)).await;
}

/// Connect to the fixed Kite market-data endpoint for native node adapters.
pub async fn connect(credentials: &KiteCredentials, deadline: Instant) -> Result<Socket> {
    connect_at(credentials, deadline, "wss://ws.kite.trade").await
}
