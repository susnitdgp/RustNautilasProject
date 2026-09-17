//! Production order notifications trigger REST reconciliation; payloads never create fills.
//! Dedicated socket: no market-data subscription and no broker mutations.
use super::dispatch::Dispatcher;
use crate::{credentials::KiteCredentials, websocket::transport};
use anyhow::{Result, anyhow, ensure};
use futures_util::SinkExt;
use nautilus_common::messages::ExecutionEvent;
use serde::Deserialize;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::{
    sync::{Mutex, Notify, mpsc::UnboundedSender, watch},
    time::{Duration, Instant, MissedTickBehavior, interval_at, sleep, timeout},
};
use tokio_tungstenite::tungstenite::Message;

const ENDPOINT: &str = "wss://ws.kite.trade";

#[derive(Clone, Copy)]
struct Timing {
    fallback: Duration,
    pending: Duration,
    idle: Duration,
    reconnect: Duration,
}
impl Default for Timing {
    fn default() -> Self {
        Self {
            fallback: Duration::from_secs(15),
            pending: Duration::from_secs(5),
            idle: Duration::from_secs(10),
            reconnect: Duration::from_millis(500),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Connection {
    generation: u32,
    connected: bool,
}

/// Do not expose private payloads in parse errors or logs. Notifications from
/// manual orders also trigger account reconciliation, but never establish ownership.
fn is_order(text: &str, user_id: &str) -> Result<bool> {
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(rename = "type")]
        kind: String,
        data: Option<serde_json::Value>,
    }
    #[derive(Deserialize)]
    struct Update {
        user_id: String,
        order_id: String,
        status: String,
    }
    let envelope: Envelope =
        serde_json::from_str(text).map_err(|_| anyhow!("Invalid Kite order-stream message"))?;
    match envelope.kind.as_str() {
        "order" => {
            let update: Update = serde_json::from_value(
                envelope
                    .data
                    .ok_or_else(|| anyhow!("Missing Kite order update"))?,
            )
            .map_err(|_| anyhow!("Invalid Kite order update"))?;
            ensure!(
                update.user_id == user_id,
                "Kite order-update account mismatch"
            );
            super::super::request::broker_id(&update.order_id)?;
            ensure!(!update.status.is_empty(), "Missing Kite order status");
            Ok(true)
        }
        "error" => Err(anyhow!(
            "Kite order stream reported an error; review session"
        )),
        _ => Ok(false),
    }
}

pub(super) async fn connect(credentials: &KiteCredentials) -> Result<transport::Socket> {
    transport::connect(credentials, Instant::now() + Duration::from_secs(10)).await
}

pub(super) struct Monitor {
    pub dispatcher: Arc<Mutex<Dispatcher>>,
    pub tx: UnboundedSender<ExecutionEvent>,
    pub active: Arc<AtomicBool>,
    pub ready: Arc<AtomicBool>,
    pub credentials: Arc<KiteCredentials>,
    pub user_id: String,
}
impl Monitor {
    pub async fn run(self, socket: transport::Socket) -> Result<()> {
        self.run_at(socket, ENDPOINT, Timing::default()).await
    }

    async fn run_at(
        self,
        mut socket: transport::Socket,
        endpoint: &str,
        timing: Timing,
    ) -> Result<()> {
        let notify = Notify::new(); // One pending notification coalesces bursts.
        let (connection_tx, connection_rx) = watch::channel(Connection {
            generation: 1,
            connected: true,
        });
        // Both futures belong to this task: no detached reader on stop/failure.
        let result = tokio::select! {
            result = self.read(&mut socket, endpoint, timing, &notify, connection_tx) => result,
            result = self.reconcile(timing, &notify, connection_rx) => result,
            _ = async {
                while self.active.load(Ordering::Acquire) {
                    sleep(Duration::from_millis(100)).await;
                }
            } => Ok(()),
        };
        self.ready.store(false, Ordering::Release);
        if result.is_err() {
            self.active.store(false, Ordering::Release);
        }
        transport::close(&mut socket).await;
        result
    }

    async fn read(
        &self,
        socket: &mut transport::Socket,
        endpoint: &str,
        timing: Timing,
        notify: &Notify,
        connection: watch::Sender<Connection>,
    ) -> Result<()> {
        let mut generation = 1;
        loop {
            match transport::next(socket, Instant::now() + timing.idle).await {
                Ok(Some(Message::Text(text))) => {
                    if is_order(&text, &self.user_id)? {
                        notify.notify_one();
                    }
                }
                Ok(Some(Message::Binary(bytes))) => {
                    ensure!(
                        bytes.len() == 1,
                        "Unexpected market data on order-only stream"
                    );
                }
                Ok(Some(Message::Ping(bytes))) => {
                    timeout(Duration::from_secs(3), socket.send(Message::Pong(bytes)))
                        .await
                        .map_err(|_| anyhow!("Kite order-stream pong timed out"))?
                        .map_err(|_| anyhow!("Kite order-stream pong failed"))?;
                }
                Ok(Some(Message::Pong(_))) => {}
                Ok(Some(Message::Close(_))) | Ok(None) | Err(_) => {
                    connection.send_replace(Connection {
                        generation,
                        connected: false,
                    });
                    self.ready.store(false, Ordering::Release);
                    eprintln!("Kite order stream disconnected; new entries paused, reconciling");
                    transport::close(socket).await;
                    ensure!(
                        generation < 3,
                        "Kite order-stream reconnect budget exhausted"
                    );
                    sleep(timing.reconnect * generation).await;
                    // Authentication/handshake failure is terminal, never retried blindly.
                    *socket = transport::connect_at(
                        &self.credentials,
                        Instant::now() + Duration::from_secs(10),
                        endpoint,
                    )
                    .await?;
                    generation += 1;
                    connection.send_replace(Connection {
                        generation,
                        connected: true,
                    });
                    eprintln!("Kite order stream reconnected; validating broker state");
                }
                Ok(Some(_)) => {}
            }
        }
    }

    async fn reconcile(
        &self,
        timing: Timing,
        notify: &Notify,
        mut connection: watch::Receiver<Connection>,
    ) -> Result<()> {
        let mut fallback = interval_at(Instant::now() + timing.fallback, timing.fallback);
        let mut pending = interval_at(Instant::now() + timing.pending, timing.pending);
        fallback.set_missed_tick_behavior(MissedTickBehavior::Skip);
        pending.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            let pending_check = tokio::select! {
                _ = notify.notified() => false,
                _ = fallback.tick() => false,
                _ = pending.tick() => true,
                changed = connection.changed() => {
                    changed.map_err(|_| anyhow!("Kite order-stream monitor closed"))?;
                    false
                },
            };
            if !self.active.load(Ordering::Acquire) {
                return Ok(());
            }
            let mut service = self.dispatcher.lock().await;
            if pending_check && !service.has_unresolved() {
                // Keep the existing durable account heartbeat fresh without a REST request.
                service.heartbeat()?;
                continue;
            }
            let before = *connection.borrow_and_update();
            service.refresh(&self.tx).await?;
            // Never reopen admission from a snapshot taken before a disconnect/reconnect.
            let after = connection.borrow();
            self.ready.store(
                before == *after && after.connected && self.active.load(Ordering::Acquire),
                Ordering::Release,
            );
        }
    }
}

#[cfg(test)]
#[path = "order_stream_tests.rs"]
mod tests;
