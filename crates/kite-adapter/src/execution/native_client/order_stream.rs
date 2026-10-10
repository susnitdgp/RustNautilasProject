//! Production order notifications trigger REST reconciliation; payloads never create fills.
//! A COMPLETE notification for an owned order first takes the postback fast path: that
//! order and its trades are read over REST (one round trip) and its fills emitted with
//! real Kite trade IDs; the full account snapshot follows and must confirm them.
//! Dedicated socket: no market-data subscription and no broker mutations.
use super::dispatch::Dispatcher;
use crate::{credentials::KiteCredentials, websocket::transport};
use anyhow::{Result, anyhow, bail, ensure};
use futures_util::{SinkExt, StreamExt};
use nautilus_common::messages::ExecutionEvent;
use serde::Deserialize;
use std::sync::{
    Arc, Mutex as StdMutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use tokio::{
    sync::{Mutex, Notify, mpsc::UnboundedSender, watch},
    time::{Duration, Instant, MissedTickBehavior, interval_at, sleep, timeout, timeout_at},
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

/// A validated order notification for this account.
#[derive(Debug, PartialEq, Eq)]
struct Postback {
    order_id: String,
    status: String,
}

/// Do not expose private payloads in parse errors or logs. Notifications from
/// manual orders also trigger account reconciliation, but never establish ownership.
/// `Ok(None)`: not an order notification (ignored).
fn is_order(text: &str, user_id: &str) -> Result<Option<Postback>> {
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(rename = "type")]
        kind: String,
        data: Option<serde_json::Value>,
    }
    #[derive(Deserialize)]
    struct Update {
        #[serde(default)]
        user_id: Option<String>,
        #[serde(default)]
        account_id: Option<String>,
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
            let identity = match (update.user_id.as_deref(), update.account_id.as_deref()) {
                (Some(a), Some(b)) if a != b => {
                    bail!("Kite order-update account identities conflict")
                }
                (Some(a), _) | (_, Some(a)) => a,
                (None, None) => bail!("Missing Kite order-update account identity"),
            };
            ensure!(identity == user_id, "Kite order-update account mismatch");
            super::super::request::broker_id(&update.order_id)?;
            ensure!(!update.status.is_empty(), "Missing Kite order status");
            Ok(Some(Postback {
                order_id: update.order_id,
                status: update.status,
            }))
        }
        "error" => Err(anyhow!(
            "Kite order stream reported an error; review session"
        )),
        _ => Ok(None),
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
        // Bumped on every order update / connection change, without the dispatcher lock,
        // so an order admitted from cached state sees it immediately.
        let doorbell = self.dispatcher.lock().await.doorbell();
        // Order ids of COMPLETE notifications not yet offered to the fast path.
        let completed = StdMutex::new(Vec::<String>::new());
        let (connection_tx, connection_rx) = watch::channel(Connection {
            generation: 1,
            connected: true,
        });
        // Both futures belong to this task: no detached reader on stop/failure.
        let result = tokio::select! {
            result = self.read(&mut socket, endpoint, timing, &notify, &doorbell, &completed, connection_tx) => result,
            result = self.reconcile(timing, &notify, &completed, connection_rx) => result,
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
        doorbell: &AtomicU64,
        completed: &StdMutex<Vec<String>>,
        connection: watch::Sender<Connection>,
    ) -> Result<()> {
        let mut generation = 1;
        loop {
            match timeout_at(Instant::now() + timing.idle, socket.next()).await {
                // A quiet order stream is healthy. REST fallback/pending timers continue
                // independently; an idle timeout must not consume reconnect budget.
                Err(_) => continue,
                Ok(Some(Ok(Message::Text(text)))) => {
                    if let Some(postback) = is_order(&text, &self.user_id)? {
                        if postback.status == "COMPLETE" {
                            let mut queue = completed
                                .lock()
                                .map_err(|_| anyhow!("Kite postback queue unavailable"))?;
                            if !queue.contains(&postback.order_id) {
                                queue.push(postback.order_id);
                            }
                        }
                        doorbell.fetch_add(1, Ordering::AcqRel);
                        notify.notify_one();
                    }
                }
                Ok(Some(Ok(Message::Binary(bytes)))) => {
                    ensure!(
                        bytes.len() == 1,
                        "Unexpected market data on order-only stream"
                    );
                }
                Ok(Some(Ok(Message::Ping(bytes)))) => {
                    timeout(Duration::from_secs(3), socket.send(Message::Pong(bytes)))
                        .await
                        .map_err(|_| anyhow!("Kite order-stream pong timed out"))?
                        .map_err(|_| anyhow!("Kite order-stream pong failed"))?;
                }
                Ok(Some(Ok(Message::Pong(_)))) => {}
                Ok(Some(Ok(Message::Close(_))) | Some(Err(_)) | None) => {
                    // Updates may be missed while disconnected: no cached admission until
                    // a reconciliation started after the reconnect.
                    doorbell.fetch_add(1, Ordering::AcqRel);
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
                    doorbell.fetch_add(1, Ordering::AcqRel);
                    connection.send_replace(Connection {
                        generation,
                        connected: true,
                    });
                    eprintln!("Kite order stream reconnected; validating broker state");
                }
                Ok(Some(Ok(_))) => {}
            }
        }
    }

    async fn reconcile(
        &self,
        timing: Timing,
        notify: &Notify,
        completed: &StdMutex<Vec<String>>,
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
            // Fast path first: the strategy gets the fill as soon as one REST round trip
            // confirms it, before the full snapshot below starts. An order the strategy
            // sends on that fill waits for the snapshot, which then also backs it.
            let ids = std::mem::take(
                &mut *completed
                    .lock()
                    .map_err(|_| anyhow!("Kite postback queue unavailable"))?,
            );
            for id in ids {
                self.dispatcher.lock().await.fast_fill(&id, &self.tx).await?;
            }
            let mut service = self.dispatcher.lock().await;
            if pending_check && !service.needs_refresh() {
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
