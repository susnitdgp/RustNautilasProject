//! Live dashboard publisher, decoupled from trading.
//!
//! Producers (the strategy, the stop watcher) push small `FnOnce(&mut Board)` updates
//! into their own lock-free SPSC queue (`rtrb`). A push never blocks: when a queue is
//! full the update is dropped and counted. One OS thread (`dashboard`) owns the
//! [`Board`], applies the updates and publishes it to the dashboard Redis from
//! `config/dashboard.json`, at most every 250 ms and at least once a second:
//!
//! | key              | type   | content                                                   |
//! |------------------|--------|-----------------------------------------------------------|
//! | `<base>:state`   | hash   | `snapshot` (board JSON), `updated_at_ms`, `seq`, `status` |
//! | `<base>:events`  | stream | one entry per event: `ts_ms`, `text` (capped ~1000)       |
//! | `<base>:live`    | pubsub | `seq` after every write, so a page can refresh at once    |
//!
//! `<base>` = `<prefix>:v1:{<slot>}:dash` (the portfolio key scheme). Redis being slow
//! or down only delays the dashboard: the writer retries every 5 s and keeps the
//! latest state and up to 500 unsent events. Nothing here can stall an order.
use super::sats_dashboard::{Board, note};
use anyhow::{Context, Result, ensure};
use rtrb::{Consumer, Producer, RingBuffer};
use serde::Deserialize;
use std::{
    cell::RefCell,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub type Update = Box<dyn FnOnce(&mut Board) + Send>;

/// Default location of the dashboard settings (gitignored: the URL holds a password).
pub const CONFIG_PATH: &str = "config/dashboard.json";
const QUEUE: usize = 4096;
const TICK: Duration = Duration::from_millis(50);
const MIN_FLUSH: Duration = Duration::from_millis(250);
const HEARTBEAT: Duration = Duration::from_secs(1);
const RETRY: Duration = Duration::from_secs(5);
const IO_TIMEOUT: Duration = Duration::from_secs(2);
const EVENTS_MAX: usize = 1000;
const TTL_SECS: u64 = 7 * 86_400;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DashboardConfig {
    pub redis_dashboard_url: String,
}

impl DashboardConfig {
    /// `None` when the file does not exist (dashboard publishing off).
    pub fn load(path: &str) -> Result<Option<Self>> {
        let raw = match std::fs::read(path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("Cannot read {path}")),
        };
        let c: Self = serde_json::from_slice(&raw).with_context(|| format!("Invalid dashboard config {path}"))?;
        let url = c.redis_dashboard_url.as_str();
        ensure!(
            url.starts_with("redis://") || url.starts_with("rediss://"),
            "redis_dashboard_url in {path} must start with redis:// or rediss://"
        );
        redis::Client::open(url).with_context(|| format!("redis_dashboard_url in {path} is not a valid Redis URL"))?;
        Ok(Some(c))
    }
}

/// Dashboard key names for one slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keys {
    pub state: String,
    pub events: String,
    pub live: String,
}

impl Keys {
    pub fn new(base: &str) -> Self {
        Self { state: format!("{base}:state"), events: format!("{base}:events"), live: format!("{base}:live") }
    }
}

/// One producer's end of a dashboard queue. `push` never blocks.
pub struct Feed {
    tx: RefCell<Producer<Update>>,
    dropped: Arc<AtomicU64>,
}

impl std::fmt::Debug for Feed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Feed").field("dropped", &self.dropped.load(Ordering::Relaxed)).finish_non_exhaustive()
    }
}

impl Feed {
    /// Queues an update for the dashboard thread; when the queue is full the update is
    /// dropped and counted instead of waiting.
    pub fn push(&self, f: impl FnOnce(&mut Board) + Send + 'static) {
        if self.tx.borrow_mut().push(Box::new(f)).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn channel(capacity: usize, dropped: Arc<AtomicU64>) -> (Feed, Consumer<Update>) {
    let (tx, rx) = RingBuffer::<Update>::new(capacity);
    (Feed { tx: RefCell::new(tx), dropped }, rx)
}

/// The running dashboard thread.
pub struct Publisher {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Board>>,
}

impl Publisher {
    /// Final drain and write, then stops the thread and hands back the board.
    pub fn close(mut self) -> Board {
        self.stop.store(true, Ordering::Release);
        self.thread.take().and_then(|t| t.join().ok()).unwrap_or_default()
    }
}

impl Drop for Publisher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

/// Starts the dashboard thread with `N` producer queues. `url` `None` = keep the board
/// (for the final summary) but publish nothing.
pub fn start<const N: usize>(mut board: Board, url: Option<String>, keys: Keys) -> Result<(Publisher, [Feed; N])> {
    board.status = "STARTING".into();
    let dropped = Arc::new(AtomicU64::new(0));
    let mut consumers = Vec::with_capacity(N);
    let feeds: [Feed; N] = std::array::from_fn(|_| {
        let (feed, rx) = channel(QUEUE, dropped.clone());
        consumers.push(rx);
        feed
    });
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let sink = url.map(|u| Sink::new(u, keys));
    let thread = thread::Builder::new()
        .name("dashboard".into())
        .spawn(move || run(board, consumers, dropped, sink, thread_stop))
        .context("Cannot start the dashboard thread")?;
    Ok((Publisher { stop, thread: Some(thread) }, feeds))
}

fn run(mut board: Board, mut rx: Vec<Consumer<Update>>, dropped: Arc<AtomicU64>, mut sink: Option<Sink>, stop: Arc<AtomicBool>) -> Board {
    let mut dirty = true;
    let mut last_flush: Option<Instant> = None;
    loop {
        let stopping = stop.load(Ordering::Acquire);
        for c in rx.iter_mut() {
            while let Ok(f) = c.pop() {
                f(&mut board);
                dirty = true;
            }
        }
        board.dropped = dropped.load(Ordering::Relaxed);
        match sink.as_mut() {
            Some(s) => {
                let since = last_flush.map_or(Duration::MAX, |t| t.elapsed());
                if (dirty && since >= MIN_FLUSH) || since >= HEARTBEAT || stopping {
                    s.flush(&mut board);
                    last_flush = Some(Instant::now());
                    dirty = false;
                }
            }
            None => board.pending.clear(),
        }
        if stopping {
            return board;
        }
        thread::sleep(TICK);
    }
}

/// The dashboard Redis connection, reconnected in the background when it drops.
struct Sink {
    client: Option<redis::Client>,
    con: Option<redis::Connection>,
    keys: Keys,
    retry_at: Option<Instant>,
    seq: u64,
    /// Last outcome was reported (`Some(true)` connected, `Some(false)` failing).
    reported: Option<bool>,
}

impl Sink {
    fn new(url: String, keys: Keys) -> Self {
        let client = redis::Client::open(url.as_str()).ok();
        if client.is_none() {
            note("Dashboard: invalid Redis URL; publishing disabled");
        }
        Self { client, con: None, keys, retry_at: None, seq: 0, reported: None }
    }

    fn report(&mut self, ok: bool, detail: &str) {
        if self.reported != Some(ok) {
            self.reported = Some(ok);
            note(&if ok { "Dashboard: publishing to Redis".to_owned() } else { format!("Dashboard: Redis unavailable ({detail}); retrying every 5 s") });
        }
    }

    fn connection(&mut self) -> Option<&mut redis::Connection> {
        if self.con.is_none() && self.retry_at.is_none_or(|t| Instant::now() >= t) {
            let attempt = self.client.as_ref()?.get_connection_with_timeout(IO_TIMEOUT).and_then(|c| {
                c.set_read_timeout(Some(IO_TIMEOUT))?;
                c.set_write_timeout(Some(IO_TIMEOUT))?;
                Ok(c)
            });
            match attempt {
                Ok(c) => {
                    self.con = Some(c);
                    self.retry_at = None;
                }
                Err(e) => {
                    self.retry_at = Some(Instant::now() + RETRY);
                    self.report(false, &e.kind_name());
                }
            }
        }
        self.con.as_mut()
    }

    fn flush(&mut self, board: &mut Board) {
        let seq = self.seq + 1;
        let pipe = commands(&self.keys, board, seq, chrono::Utc::now().timestamp_millis());
        let Some(con) = self.connection() else { return };
        match pipe.query::<()>(con) {
            Ok(()) => {
                self.seq = seq;
                board.pending.clear();
                self.report(true, "");
            }
            Err(e) => {
                self.con = None;
                self.retry_at = Some(Instant::now() + RETRY);
                self.report(false, &e.kind_name());
            }
        }
    }
}

trait KindName {
    fn kind_name(&self) -> String;
}

impl KindName for redis::RedisError {
    /// Error category only: the message could echo the URL with its password.
    fn kind_name(&self) -> String {
        if self.is_timeout() {
            "timeout".into()
        } else if self.is_connection_refusal() {
            "connection refused".into()
        } else if self.is_io_error() {
            "network error".into()
        } else {
            format!("{:?}", self.kind())
        }
    }
}

/// One pipelined write: state hash, new events, notification.
fn commands(keys: &Keys, board: &Board, seq: u64, now_ms: i64) -> redis::Pipeline {
    let snapshot = serde_json::json!({ "seq": seq, "updated_at_ms": now_ms, "board": board }).to_string();
    let mut p = redis::pipe();
    p.cmd("HSET")
        .arg(&keys.state)
        .arg("snapshot")
        .arg(snapshot)
        .arg("updated_at_ms")
        .arg(now_ms)
        .arg("seq")
        .arg(seq)
        .arg("status")
        .arg(&board.status)
        .ignore();
    p.cmd("EXPIRE").arg(&keys.state).arg(TTL_SECS).ignore();
    if !board.pending.is_empty() {
        for (ts_ms, text) in &board.pending {
            p.cmd("XADD")
                .arg(&keys.events)
                .arg("MAXLEN")
                .arg("~")
                .arg(EVENTS_MAX)
                .arg("*")
                .arg("ts_ms")
                .arg(*ts_ms)
                .arg("text")
                .arg(text)
                .ignore();
        }
        p.cmd("EXPIRE").arg(&keys.events).arg(TTL_SECS).ignore();
    }
    p.cmd("PUBLISH").arg(&keys.live).arg(seq).ignore();
    p
}

#[cfg(test)]
#[allow(dead_code)]
#[path = "../../../../crates/kite-journal/test-support/redis.rs"]
mod test_redis;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_queue_drops_and_counts_instead_of_blocking() {
        let dropped = Arc::new(AtomicU64::new(0));
        let (feed, mut rx) = channel(2, dropped.clone());
        for i in 0..5 {
            feed.push(move |b| b.lots = i);
        }
        assert_eq!(dropped.load(Ordering::Relaxed), 3, "two fit, three dropped");
        let mut b = Board::default();
        while let Ok(f) = rx.pop() {
            f(&mut b);
        }
        assert_eq!(b.lots, 1, "the queued updates apply in order");
    }

    #[test]
    fn the_thread_applies_every_producer_and_returns_the_board() {
        let (publisher, [strategy, watcher]) = start::<2>(Board { lots: 3, ..Board::default() }, None, Keys::new("t")).unwrap();
        strategy.push(|b| b.apply_fill(3.0, 8790.0));
        strategy.push(|b| b.event("BUY 3 lot".into()));
        watcher.push(|b| b.status = "STOPPING".into());
        std::thread::sleep(Duration::from_millis(200));
        let b = publisher.close();
        assert_eq!((b.position, b.status.as_str(), b.dropped), (3.0, "STOPPING", 0));
        assert!(b.events[0].ends_with("BUY 3 lot"));
        assert!(b.pending.is_empty(), "nothing to publish without a URL");
    }

    #[test]
    fn missing_config_means_off_and_a_bad_url_is_rejected() {
        assert!(DashboardConfig::load("/nonexistent/dashboard.json").unwrap().is_none());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("d.json");
        std::fs::write(&path, r#"{"redis_dashboard_url":"http://x"}"#).unwrap();
        assert!(DashboardConfig::load(path.to_str().unwrap()).is_err());
        std::fs::write(&path, r#"{"redis_dashboard_url":"redis://default:pw@localhost:6390"}"#).unwrap();
        assert!(DashboardConfig::load(path.to_str().unwrap()).unwrap().is_some());
    }

    /// End to end against a throwaway local redis-server.
    #[test]
    fn publishes_state_events_and_a_notification() {
        use redis::Commands;
        let r = test_redis::TestRedis::new();
        let keys = Keys::new(&format!("{}:dash", r.namespace()));
        let mut sub = r.connection();
        let mut pubsub = sub.as_pubsub();
        pubsub.subscribe(&keys.live).unwrap();
        pubsub.set_read_timeout(Some(Duration::from_secs(5))).unwrap();

        let board = Board { slot: "crudeoilm-sniper-202610".into(), point_value: 10.0, ..Board::default() };
        let (publisher, [strategy]) = start::<1>(board, Some(r.url.clone()), keys.clone()).unwrap();
        strategy.push(|b| {
            b.status = "RUNNING".into();
            b.apply_fill(1.0, 8790.0);
            b.event("BUY 1 lot @ 8790".into());
        });
        let seq: u64 = pubsub.get_message().unwrap().get_payload().unwrap();
        assert!(seq >= 1);
        strategy.push(|b| b.event("TP1 8808: close 1 lot".into()));
        let b = publisher.close();
        assert!(b.pending.is_empty(), "every event was written");

        let mut con = r.connection();
        let status: String = con.hget(&keys.state, "status").unwrap();
        assert_eq!(status, "RUNNING");
        let snapshot: String = con.hget(&keys.state, "snapshot").unwrap();
        let v: serde_json::Value = serde_json::from_str(&snapshot).unwrap();
        assert_eq!(v["board"]["slot"], "crudeoilm-sniper-202610");
        assert_eq!(v["board"]["position"], 1.0);
        let ttl: i64 = con.ttl(&keys.state).unwrap();
        assert!(ttl > 0, "state expires after the run");
        let events: Vec<(String, std::collections::HashMap<String, String>)> =
            redis::cmd("XRANGE").arg(&keys.events).arg("-").arg("+").query(&mut con).unwrap();
        let texts: Vec<&str> = events.iter().map(|(_, f)| f["text"].as_str()).collect();
        assert_eq!(texts, ["BUY 1 lot @ 8790", "TP1 8808: close 1 lot"]);
    }

    #[test]
    fn an_unreachable_redis_never_blocks_and_keeps_the_events() {
        // nothing listens on port 1
        let (publisher, [strategy]) = start::<1>(Board::default(), Some("redis://127.0.0.1:1".into()), Keys::new("t")).unwrap();
        let t = Instant::now();
        for i in 0..100 {
            strategy.push(move |b| b.event(format!("e{i}")));
        }
        assert!(t.elapsed() < Duration::from_millis(50), "pushing is never slowed by Redis");
        std::thread::sleep(Duration::from_millis(300));
        let b = publisher.close();
        assert_eq!(b.pending.len(), 100, "unsent events are kept for the next connection");
    }
}
