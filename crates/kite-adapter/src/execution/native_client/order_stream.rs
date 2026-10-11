//! Production order notifications trigger reconciliation (order book + trade book, one
//! round trip); payloads never create fills. Every 2 s while an owned order is unresolved
//! and every 15 s otherwise the book is read anyway (a lost update is covered), and every
//! 15 s the account audit checks positions. Dedicated socket: no market-data
//! subscription and no broker mutations.
use super::dispatch::Dispatcher;
use crate::{credentials::KiteCredentials, websocket::transport};
use anyhow::{Result, anyhow, bail, ensure};
use futures_util::{SinkExt, StreamExt};
use nautilus_common::messages::ExecutionEvent;
use serde::Deserialize;
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
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
    /// No frame at all (heartbeat included) for this long: send a liveness Ping.
    idle: Duration,
    /// After the Ping, no frame for this long: the connection is dead (half-open).
    probe: Duration,
    /// Base delay before a reconnect attempt (multiplied by the attempt number).
    reconnect: Duration,
    /// Sliding window for `MAX_RECONNECTS_PER_WINDOW`.
    window: Duration,
}
impl Default for Timing {
    fn default() -> Self {
        Self {
            fallback: Duration::from_secs(15),
            pending: Duration::from_secs(2),
            idle: Duration::from_secs(10),
            probe: Duration::from_secs(5),
            reconnect: Duration::from_millis(500),
            window: Duration::from_secs(600),
        }
    }
}

/// Drops allowed inside `Timing::window` before the run stops (kite-adapter 0.5.0; until
/// then it was 2 reconnects per run, so the 3rd drop of a 14-hour session ended it).
const MAX_RECONNECTS_PER_WINDOW: usize = 3;
/// Connection attempts per drop, with a growing delay, before the run stops.
const CONNECT_ATTEMPTS: u32 = 3;

/// Records a reconnect at `now` if fewer than `MAX_RECONNECTS_PER_WINDOW` happened in the
/// last `window`; returns whether it is allowed.
fn reconnect_allowed(history: &mut VecDeque<Instant>, now: Instant, window: Duration) -> bool {
    while history.front().is_some_and(|t| now.duration_since(*t) >= window) {
        history.pop_front();
    }
    if history.len() >= MAX_RECONNECTS_PER_WINDOW {
        return false;
    }
    history.push_back(now);
    true
}

/// A stderr line that never panics (unlike `eprintln!` once the terminal is gone).
fn note(line: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "{line}");
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Connection {
    generation: u32,
    connected: bool,
}

/// A validated order notification for this account. Only its arrival matters (it
/// triggers reconciliation); the fields are validated and kept for the tests.
#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
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

fn wall_ns() -> u64 {
    nautilus_core::time::get_atomic_clock_realtime().get_time_ns().as_u64()
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
        // Wall-clock arrival (ns) of the latest order update, for the latency log.
        let update_at = AtomicU64::new(0);
        let (connection_tx, connection_rx) = watch::channel(Connection {
            generation: 1,
            connected: true,
        });
        // Both futures belong to this task: no detached reader on stop/failure.
        let result = tokio::select! {
            result = self.read(&mut socket, endpoint, timing, &notify, &update_at, connection_tx) => result,
            result = self.reconcile(timing, &notify, &update_at, connection_rx) => result,
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
        update_at: &AtomicU64,
        connection: watch::Sender<Connection>,
    ) -> Result<()> {
        let mut generation = 1;
        let mut reconnects = VecDeque::new();
        // Liveness (kite-adapter 0.8.1): Kite sends a 1-byte heartbeat every couple of
        // seconds on a quiet connection. After `timing.idle` with no frame at all, a Ping
        // probes the link; still no frame (no heartbeat, no Pong) within `timing.probe`
        // means the connection is dead without having closed (half-open TCP). That is
        // handled exactly like a drop: entries pause, reconnect, reconcile, reopen.
        let mut probing = false;
        loop {
            let wait = if probing { timing.probe } else { timing.idle };
            let frame = match timeout_at(Instant::now() + wait, socket.next()).await {
                Err(_) if !probing => {
                    probing = true;
                    let sent = timeout(
                        Duration::from_secs(3),
                        socket.send(Message::Ping(Vec::new().into())),
                    )
                    .await;
                    if matches!(sent, Ok(Ok(()))) {
                        continue;
                    }
                    note("Kite order stream: liveness ping could not be sent");
                    None
                }
                Err(_) => {
                    note(&format!(
                        "Kite order stream silent for {} s (no heartbeat, no pong): treating it as dropped",
                        (timing.idle + timing.probe).as_secs()
                    ));
                    None
                }
                Ok(frame) => {
                    probing = false;
                    Some(frame)
                }
            };
            match frame {
                Some(Some(Ok(Message::Text(text)))) => {
                    if is_order(&text, &self.user_id)?.is_some() {
                        update_at.store(wall_ns(), Ordering::Release);
                        notify.notify_one();
                    }
                }
                Some(Some(Ok(Message::Binary(bytes)))) => {
                    ensure!(
                        bytes.len() == 1,
                        "Unexpected market data on order-only stream"
                    );
                }
                Some(Some(Ok(Message::Ping(bytes)))) => {
                    timeout(Duration::from_secs(3), socket.send(Message::Pong(bytes)))
                        .await
                        .map_err(|_| anyhow!("Kite order-stream pong timed out"))?
                        .map_err(|_| anyhow!("Kite order-stream pong failed"))?;
                }
                Some(Some(Ok(Message::Pong(_)))) => {}
                None | Some(Some(Ok(Message::Close(_))) | Some(Err(_)) | None) => {
                    probing = false;
                    // Updates may be missed while disconnected: entries stay paused until a
                    // reconciliation started after the reconnect.
                    connection.send_replace(Connection {
                        generation,
                        connected: false,
                    });
                    self.ready.store(false, Ordering::Release);
                    note("Kite order stream disconnected; new entries paused, reconciling");
                    transport::close(socket).await;
                    ensure!(
                        reconnect_allowed(&mut reconnects, Instant::now(), timing.window),
                        "Kite order stream dropped {MAX_RECONNECTS_PER_WINDOW} times within {} min",
                        timing.window.as_secs() / 60
                    );
                    // A few spaced attempts, then give up: a bad session (expired token)
                    // fails every attempt and must not be hammered.
                    let mut attempt = 0;
                    *socket = loop {
                        attempt += 1;
                        sleep(timing.reconnect * attempt).await;
                        match transport::connect_at(
                            &self.credentials,
                            Instant::now() + Duration::from_secs(10),
                            endpoint,
                        )
                        .await
                        {
                            Ok(fresh) => break fresh,
                            Err(e) if attempt < CONNECT_ATTEMPTS => {
                                note(&format!(
                                    "Kite order-stream reconnect attempt {attempt}/{CONNECT_ATTEMPTS} failed: {e:#}"
                                ));
                            }
                            Err(e) => {
                                return Err(e.context(format!(
                                    "Kite order stream: reconnect failed {CONNECT_ATTEMPTS} times"
                                )));
                            }
                        }
                    };
                    generation += 1;
                    connection.send_replace(Connection {
                        generation,
                        connected: true,
                    });
                    note("Kite order stream reconnected; validating broker state");
                }
                Some(Some(Ok(_))) => {}
            }
        }
    }

    async fn reconcile(
        &self,
        timing: Timing,
        notify: &Notify,
        update_at: &AtomicU64,
        mut connection: watch::Receiver<Connection>,
    ) -> Result<()> {
        let mut fallback = interval_at(Instant::now() + timing.fallback, timing.fallback);
        let mut pending = interval_at(Instant::now() + timing.pending, timing.pending);
        fallback.set_missed_tick_behavior(MissedTickBehavior::Skip);
        pending.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            // (pending tick, account audit, what triggered this pass)
            let (pending_check, audit, trigger) = tokio::select! {
                _ = notify.notified() => (false, false, "order_update"),
                _ = fallback.tick() => (false, true, "fallback_15s"),
                _ = pending.tick() => (true, false, "pending_timer"),
                changed = connection.changed() => {
                    changed.map_err(|_| anyhow!("Kite order-stream monitor closed"))?;
                    (false, true, "reconnect")
                },
            };
            let triggered_at = match trigger {
                "order_update" => update_at.load(Ordering::Acquire),
                _ => wall_ns(),
            };
            if !self.active.load(Ordering::Acquire) {
                return Ok(());
            }
            let mut service = self.dispatcher.lock().await;
            if pending_check && !service.has_unresolved() {
                // Nothing in flight: order updates and the 15 s fallback keep it current.
                continue;
            }
            let before = *connection.borrow_and_update();
            service.note_trigger(triggered_at, trigger);
            service.refresh(&self.tx).await?;
            if audit {
                service.audit().await?;
            }
            // Never reopen entries from a reconciliation started before a disconnect.
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
