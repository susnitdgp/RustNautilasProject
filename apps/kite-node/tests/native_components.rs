use std::{
    net::TcpListener,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};
struct Redis {
    child: Child,
    dir: tempfile::TempDir,
    url: String,
}
impl Redis {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let dir = tempfile::tempdir().unwrap();
        let child = Command::new("redis-server")
            .args([
                "--bind",
                "127.0.0.1",
                "--port",
                &port.to_string(),
                "--appendonly",
                "yes",
                "--appendfsync",
                "everysec",
                "--save",
                "",
                "--maxmemory-policy",
                "noeviction",
                "--dir",
                dir.path().to_str().unwrap(),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("redis-server required");
        let server = Self {
            child,
            dir,
            url: format!("redis://127.0.0.1:{port}/"),
        };
        for _ in 0..100 {
            if redis::Client::open(server.url.as_str())
                .unwrap()
                .get_connection()
                .is_ok()
            {
                return server;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("Redis did not start");
    }
    fn run(&self, args: &[&str]) -> serde_json::Value {
        let output = Command::new(env!("CARGO_BIN_EXE_kite-node"))
            .args(args)
            .env("KITE_REDIS_URL", &self.url)
            .current_dir(self.dir.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "command {:?}: {}\n{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let logs = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!logs.contains("[ERROR]"), "Native runtime error: {logs}");
        assert!(
            !logs.contains("InvalidStateTrigger"),
            "Native event sequence failure: {logs}"
        );
        assert!(
            !logs.contains("on_start` handler was called when not overridden"),
            "Missing lifecycle hook: {logs}"
        );
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .rev()
            .find_map(|line| serde_json::from_str(line).ok())
            .expect("JSON result")
    }
}
impl Drop for Redis {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
#[test]
fn native_nodes_matching_algorithms_catalog_and_redis_reconstruction() {
    let redis = Redis::start();
    let config = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../config/strategy-crossover.toml")
        .canonicalize()
        .unwrap();
    for cmd in [
        "native-backtest",
        "native-node-sim",
        "native-kite-mock",
        "native-kite-mock-short",
    ] {
        let result = redis.run(&[cmd, config.to_str().unwrap()]);
        assert_eq!(result["strategy_ticks"], 15);
        assert_eq!(result["signals"], 2, "{cmd}: {result}");
        assert_eq!(result["fills"], 2);
        assert_eq!(result["open_contracts"].as_f64(), Some(0.0));
        assert_eq!(result["live_orders_enabled"], false);
        if cmd == "native-node-sim" {
            assert_eq!(result["full_catalog_roundtrip_verified"], true);
            assert_eq!(result["captured_full_packets"], 15);
            let replay = redis.run(&[
                "native-backtest",
                config.to_str().unwrap(),
                result["catalog"].as_str().unwrap(),
            ]);
            assert_eq!(replay["full_packet_replay"], true);
            assert_eq!(replay["replayed_full_packets"], 15);
            assert_eq!(replay["strategy_ticks"], 15);
            assert_eq!(replay["signals"], 2);
            assert_eq!(replay["fills"], 2);
            assert_eq!(replay["open_contracts"].as_f64(), Some(0.0));
        }
        if cmd.starts_with("native-kite-mock") {
            let health = redis.run(&["native-kite-status", "MOCK"]);
            assert_eq!(health["state"], "Clean");
            let review = redis.run(&["native-kite-review", result["namespace"].as_str().unwrap()]);
            assert_eq!(review["unresolved"], 0);
            assert_eq!(review["journal_exposure"], "0");
            let audit = redis.run(&["native-full-audit", result["catalog"].as_str().unwrap()]);
            assert_eq!(audit["packets"], 15);
            assert_eq!(audit["full_fields_present"], true);
            assert_eq!(result["native_kite_execution_client"], true);
            assert_eq!(result["native_kite_mock_broker"], true);
            assert_eq!(result["native_matching_engine"], false);
            assert_eq!(result["broker_orders_accessed"], false);
            let key = [
                "susanta:nautilus:native-kite:commands:{",
                result["namespace"].as_str().unwrap(),
                "}",
            ]
            .concat();
            let mut connection = redis::Client::open(redis.url.as_str())
                .unwrap()
                .get_connection()
                .unwrap();
            let records: std::collections::BTreeMap<String, String> = redis::cmd("HGETALL")
                .arg(key)
                .query(&mut connection)
                .unwrap();
            assert_eq!(records.len(), 5);
            for (_, record) in records.iter().filter(|(key, _)| key.starts_with("order:")) {
                let r: serde_json::Value = serde_json::from_str(record).unwrap();
                assert_eq!(r["outcome"], "Observed");
                assert_eq!(r["events"].as_array().unwrap().len(), 4);
                assert!(r["broker_id"].as_str().is_some());
                assert_eq!(r["tag"].as_str().unwrap().len(), 20);
            }
        }
        if cmd == "native-kite-mock-short" {
            assert_eq!(result["signal_counts"]["SELL"], 1);
            assert_eq!(result["signal_counts"]["SELL_EXIT"], 1);
            let replay = redis.run(&[
                "native-backtest",
                config.to_str().unwrap(),
                result["catalog"].as_str().unwrap(),
            ]);
            assert_eq!(replay["signal_counts"]["SELL"], 1);
            assert_eq!(replay["signal_counts"]["SELL_EXIT"], 1);
            assert_eq!(replay["fills"], 2);
            assert_eq!(replay["open_contracts"].as_f64(), Some(0.0));
        }
        let recovered = redis.run(&["native-recover", result["namespace"].as_str().unwrap()]);
        assert_eq!(recovered["orders"], 2);
        assert_eq!(recovered["accounts"], 1);
        assert_eq!(recovered["open_orders"], 0);
        assert_eq!(recovered["open_contracts"].as_f64(), Some(0.0));
        assert_eq!(recovered["resubmissions"], 0);
        for events in recovered["order_event_sequences"]
            .as_object()
            .unwrap()
            .values()
        {
            assert_eq!(
                *events,
                serde_json::json!(["Initialized", "Submitted", "Accepted", "Filled"])
            );
        }
    }
    let emulated = redis.run(&["native-emulator-sim"]);
    assert_eq!(emulated["emulated"], 1);
    assert_eq!(emulated["fills"], 2);
    let recovered = redis.run(&["native-recover", emulated["namespace"].as_str().unwrap()]);
    assert_eq!(recovered["open_contracts"].as_f64(), Some(0.0));
    let twap = redis.run(&["native-twap-sim"]);
    assert!(twap["spawned_children"].as_u64().unwrap() > 0);
    assert_eq!(twap["fills"], 3);
    let recovered = redis.run(&["native-recover", twap["namespace"].as_str().unwrap()]);
    assert_eq!(recovered["open_contracts"].as_f64(), Some(0.0));
    let result = Command::new(env!("CARGO_BIN_EXE_kite-node"))
        .arg("native-node-live")
        .env("KITE_REDIS_URL", &redis.url)
        .current_dir(redis.dir.path())
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("paper execution remains enforced"));
}

#[test]
fn sigterm_drains_native_node_and_leaves_flat_account_restartable() {
    let redis = Redis::start();
    let config = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../config/strategy-crossover.toml")
        .canonicalize()
        .unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_kite-node"))
        .args(["native-kite-mock", config.to_str().unwrap()])
        .env("KITE_REDIS_URL", &redis.url)
        .current_dir(redis.dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut c = redis::Client::open(redis.url.as_str())
        .unwrap()
        .get_connection()
        .unwrap();
    let key = "susanta:nautilus:native-kite:account:{MOCK}";
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        let state: Option<String> = redis::cmd("HGET")
            .arg(key)
            .arg("state")
            .query(&mut c)
            .unwrap();
        if state.as_deref() == Some("Running") {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_secs(3));
    assert!(
        Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(redis.run(&["native-kite-status", "MOCK"])["state"], "Clean");
}

#[test]
fn selected_supertrend_live_node_trades_both_directions_and_flattens() {
    let redis = Redis::start();
    let config = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../config/production-supertrend.json")
        .canonicalize()
        .unwrap();
    let result = redis.run(&["native-supertrend-sim", config.to_str().unwrap()]);
    assert_eq!(result["status"], "Clean");
    assert_eq!(result["runtime"], "LiveNode");
    assert_eq!(result["live_orders_enabled"], false);
    assert_eq!(result["open_contracts"].as_f64(), Some(0.));
    assert!(result["fills"].as_u64().unwrap() >= 4);
    let folder = redis
        .dir
        .path()
        .join(result["report_directory"].as_str().unwrap());
    let signals: Vec<serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(folder.join("signals.json")).unwrap())
            .unwrap();
    for intent in ["BUY", "BUY_EXIT", "SELL", "SELL_EXIT"] {
        assert!(
            signals.iter().any(|s| s["intent"] == intent),
            "{intent} missing"
        );
    }
    let mut con = redis::Client::open(redis.url.as_str())
        .unwrap()
        .get_connection()
        .unwrap();
    let id = result["namespace"].as_str().unwrap();
    let owner: Option<String> = redis::cmd("GET")
        .arg(format!("kite:paper:supertrend:sim:{id}:owner"))
        .query(&mut con)
        .unwrap();
    assert!(owner.is_none());
    let health: String = redis::cmd("HGET")
        .arg(format!("kite:paper:supertrend:{id}:health"))
        .arg("state")
        .query(&mut con)
        .unwrap();
    assert_eq!(health, "Clean");
}

#[cfg(unix)]
#[test]
fn selected_supertrend_sigterm_flattens_before_stopping_node() {
    use std::io::{BufRead, BufReader};
    let redis = Redis::start();
    let config = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../config/production-supertrend.json")
        .canonicalize()
        .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_kite-node"))
        .args(["native-supertrend-sim", config.to_str().unwrap()])
        .env("KITE_REDIS_URL", &redis.url)
        .current_dir(redis.dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut first = String::new();
    reader.read_line(&mut first).unwrap();
    assert!(first.contains("supertrend_live_started"));
    std::thread::sleep(Duration::from_secs(3));
    assert!(
        Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let mut result = None;
    for line in reader.lines() {
        let line = line.unwrap();
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
            result = Some(value);
        }
    }
    assert!(child.wait().unwrap().success());
    let result = result.unwrap();
    assert_eq!(result["status"], "Clean");
    assert_eq!(result["open_contracts"].as_f64(), Some(0.));
    assert!(result["fills"].as_u64().unwrap() >= 2);
    let path = redis
        .dir
        .path()
        .join(result["report_directory"].as_str().unwrap())
        .join("signals.json");
    let signals: Vec<serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert!(signals.iter().any(|s| s["reason"] == "shutdown"));
}

#[test]
fn selected_strategy_uses_native_kite_protected_market_dispatch() {
    let redis = Redis::start();
    let config = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../config/production-supertrend.json")
        .canonicalize()
        .unwrap();
    let result = redis.run(&["native-supertrend-kite-mock", config.to_str().unwrap()]);
    let selection: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
    assert_eq!(result["instrument"], selection["instrument"]);
    assert_eq!(result["instrument_token"], selection["instrument_token"]);
    let fills: Vec<serde_json::Value> = serde_json::from_slice(
        &std::fs::read(
            redis
                .dir
                .path()
                .join(result["report_directory"].as_str().unwrap())
                .join("fills.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(!fills.is_empty());
    assert!(
        fills
            .iter()
            .all(|fill| fill["instrument_id"] == selection["instrument"])
    );
    assert_eq!(result["execution"], "Kite native mock");
    assert_eq!(result["status"], "Clean");
    assert_eq!(result["live_orders_enabled"], false);
    assert_eq!(result["open_contracts"].as_f64(), Some(0.));
    assert!(result["fills"].as_u64().unwrap() >= 4);
    let recovered = redis.run(&["native-recover", result["namespace"].as_str().unwrap()]);
    assert_eq!(recovered["requires_review"], false);
    assert_eq!(recovered["orders"], result["fills"]);
}

#[test]
fn revised_history_rebuilds_in_live_node_without_replaying_orders() {
    let redis = Redis::start();
    let config = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../config/production-supertrend.json")
        .canonicalize()
        .unwrap();
    let result = redis.run(&["native-supertrend-recovery-sim", config.to_str().unwrap()]);
    assert_eq!(result["status"], "Clean");
    assert_eq!(result["indicator_rebuilds"], 1);
    assert_eq!(result["bars"], 160);
    assert_eq!(result["fills"], 6);
    assert_eq!(result["open_contracts"].as_f64(), Some(0.));
}

#[cfg(unix)]
mod unattended {
    use super::*;
    use std::io::{BufRead, BufReader};
    fn start(redis: &Redis, command: &str) -> (Child, String) {
        let config = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../config/production-supertrend.json")
            .canonicalize()
            .unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_kite-node"))
            .args([command, config.to_str().unwrap()])
            .env("KITE_REDIS_URL", &redis.url)
            .current_dir(redis.dir.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut line = String::new();
        let mut reader = BufReader::new(child.stdout.take().unwrap());
        reader.read_line(&mut line).unwrap();
        child.stdout = Some(reader.into_inner());
        let event: serde_json::Value = serde_json::from_str(&line).unwrap();
        (child, event["namespace"].as_str().unwrap().to_owned())
    }
    fn bounded_wait(child: &mut Child) -> std::process::ExitStatus {
        let deadline = std::time::Instant::now() + Duration::from_secs(40);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                return status;
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("unattended fault did not stop within 40 seconds");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    #[test]
    fn selected_owner_loss_stops_and_preserves_foreign_owner() {
        let redis = Redis::start();
        let (mut child, id) = start(&redis, "native-supertrend-sim");
        let mut con = redis::Client::open(redis.url.as_str())
            .unwrap()
            .get_connection()
            .unwrap();
        let key = format!("kite:paper:supertrend:sim:{id}:owner");
        redis::cmd("SET")
            .arg(&key)
            .arg("replacement-owner")
            .query::<()>(&mut con)
            .unwrap();
        assert!(!bounded_wait(&mut child).success());
        let owner: String = redis::cmd("GET").arg(&key).query(&mut con).unwrap();
        assert_eq!(owner, "replacement-owner");
        let summary: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(
                redis
                    .dir
                    .path()
                    .join(format!("data/supertrend-live/{id}/summary.json")),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(summary["status"], "ReviewRequired");
        assert_eq!(summary["live_orders_enabled"], false);
        assert!(
            summary["feed_fault"]
                .as_str()
                .unwrap()
                .contains("ownership")
        );
    }
    #[test]
    fn selected_hard_crash_blocks_account_restart_without_resubmission() {
        let redis = Redis::start();
        let (mut child, id) = start(&redis, "native-supertrend-kite-mock");
        let mut con = redis::Client::open(redis.url.as_str())
            .unwrap()
            .get_connection()
            .unwrap();
        let key = "susanta:nautilus:native-kite:account:{MOCK}";
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            let attempts: Option<u64> = redis::cmd("HGET")
                .arg(key)
                .arg("command_attempts")
                .query(&mut con)
                .unwrap();
            if attempts.unwrap_or(0) > 0 {
                break;
            }
            if std::time::Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("mock never dispatched");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        child.kill().unwrap();
        child.wait().unwrap();
        let attempts: u64 = redis::cmd("HGET")
            .arg(key)
            .arg("command_attempts")
            .query(&mut con)
            .unwrap();
        let health = redis.run(&["native-kite-status", "MOCK"]);
        assert_eq!(health["restart_blocked"], true);
        assert_eq!(health["owner"], id);
        let config = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../config/production-supertrend.json");
        let rejected = Command::new(env!("CARGO_BIN_EXE_kite-node"))
            .args(["native-supertrend-kite-mock", config.to_str().unwrap()])
            .env("KITE_REDIS_URL", &redis.url)
            .current_dir(redis.dir.path())
            .output()
            .unwrap();
        assert!(!rejected.status.success());
        assert!(String::from_utf8_lossy(&rejected.stderr).contains("Account restart blocked"));
        assert!(!String::from_utf8_lossy(&rejected.stdout).contains("supertrend_live_started"));
        let after: u64 = redis::cmd("HGET")
            .arg(key)
            .arg("command_attempts")
            .query(&mut con)
            .unwrap();
        assert_eq!(after, attempts, "restart submitted an additional command");
        let health = redis.run(&["native-kite-status", "MOCK"]);
        assert_eq!(health["owner"], id);
        assert_eq!(health["restart_blocked"], true);
        let recovered = redis.run(&["native-recover", &id]);
        assert_eq!(recovered["resubmissions"], 0);
    }
    #[test]
    fn selected_redis_outage_stops_without_claiming_clean_shutdown() {
        let mut redis = Redis::start();
        let (mut child, id) = start(&redis, "native-supertrend-sim");
        redis.child.kill().unwrap();
        redis.child.wait().unwrap();
        assert!(!bounded_wait(&mut child).success());
        let report = redis
            .dir
            .path()
            .join(format!("data/supertrend-live/{id}/summary.json"));
        if report.exists() {
            let summary: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(report).unwrap()).unwrap();
            assert_ne!(summary["status"], "Clean");
        }
    }
}

#[test]
fn selected_blocked_account_does_not_create_strategy_owner_or_journal() {
    let redis = Redis::start();
    let mut con = redis::Client::open(redis.url.as_str())
        .unwrap()
        .get_connection()
        .unwrap();
    let key = "susanta:nautilus:native-kite:account:{MOCK}";
    redis::cmd("HSET")
        .arg(key)
        .arg("scope")
        .arg("NATIVE_DISABLED_V1")
        .arg("state")
        .arg("ReviewRequired")
        .arg("owner")
        .arg("previous-run")
        .arg("unresolved")
        .arg("0")
        .arg("position")
        .arg("0")
        .query::<()>(&mut con)
        .unwrap();
    let before: std::collections::BTreeMap<String, String> =
        redis::cmd("HGETALL").arg(key).query(&mut con).unwrap();
    let config =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config/production-supertrend.json");
    let output = Command::new(env!("CARGO_BIN_EXE_kite-node"))
        .args(["native-supertrend-kite-mock", config.to_str().unwrap()])
        .env("KITE_REDIS_URL", &redis.url)
        .current_dir(redis.dir.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("owner=previous-run"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("supertrend_live_started"));
    let after: std::collections::BTreeMap<String, String> =
        redis::cmd("HGETALL").arg(key).query(&mut con).unwrap();
    assert_eq!(before, after);
    assert_eq!(
        redis::cmd("DBSIZE").query::<usize>(&mut con).unwrap(),
        1,
        "Rejected startup created new persistence or strategy ownership"
    );
}

#[test]
fn selected_initialization_failure_reports_unknown_order_count() {
    let redis = Redis::start();
    let mut con = redis::Client::open(redis.url.as_str())
        .unwrap()
        .get_connection()
        .unwrap();
    // A clean-looking coordinator without its durable budget fails during client
    // creation, after admission. It must not manufacture a one-order report.
    redis::cmd("HSET")
        .arg("susanta:nautilus:native-kite:account:{MOCK}")
        .arg("scope")
        .arg("NATIVE_DISABLED_V1")
        .arg("state")
        .arg("Clean")
        .arg("owner")
        .arg("")
        .arg("unresolved")
        .arg("0")
        .arg("position")
        .arg("0")
        .query::<()>(&mut con)
        .unwrap();
    let config =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config/production-supertrend.json");
    let output = Command::new(env!("CARGO_BIN_EXE_kite-node"))
        .args(["native-supertrend-kite-mock", config.to_str().unwrap()])
        .env("KITE_REDIS_URL", &redis.url)
        .current_dir(redis.dir.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let result: serde_json::Value = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str(line).ok())
        .expect("Failure summary");
    assert_eq!(result["status"], "ReviewRequired");
    assert!(result["open_orders"].is_null());
    assert!(result["open_contracts"].is_null());
    assert_eq!(result["signals"], 0);
    assert_eq!(result["fills"], 0);
    assert!(result["run_error"].as_str().unwrap().contains("Rate-limit"));
}

#[test]
fn pivot_point_simulation_and_kite_mock_reverse_and_square_off_without_confirmation_filter() {
    let redis = Redis::start();
    let config = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../config/pivot-point-supertrend.json")
        .canonicalize()
        .unwrap();
    for command in ["native-pivot-sim", "native-pivot-kite-mock"] {
        let result = redis.run(&[command, config.to_str().unwrap()]);
        assert_eq!(result["strategy"], "pivot_point_supertrend");
        assert_eq!(result["status"], "Clean");
        assert_eq!(result["live_orders_enabled"], false);
        assert_eq!(result["open_orders"], 0);
        assert_eq!(result["open_contracts"].as_f64(), Some(0.));
        let folder = redis
            .dir
            .path()
            .join(result["report_directory"].as_str().unwrap());
        let signals: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(folder.join("signals.json")).unwrap()).unwrap();
        for intent in ["BUY", "BUY_EXIT", "SELL", "SELL_EXIT"] {
            assert!(
                signals.iter().any(|s| s["intent"] == intent),
                "{command}: missing {intent}: {signals:?}"
            );
        }
        assert!(signals.iter().all(|s| s["entry_filtered"] == false));
        assert!(signals.iter().any(|s| s["reason"] == "session_end"));
        let indicators: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(folder.join("indicators.json")).unwrap())
                .unwrap();
        assert!(indicators.iter().any(|v| !v["center"].is_null()));
        assert!(indicators.iter().all(|v| v.get("confirmation").is_none()));
        let recovered = redis.run(&["native-recover", result["namespace"].as_str().unwrap()]);
        assert_eq!(recovered["requires_review"], false);
        assert_eq!(recovered["resubmissions"], 0);
    }
}

#[test]
fn pivot_point_cannot_activate_production_or_load_the_wrong_strategy() {
    let redis = Redis::start();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    for args in [
        vec![
            "native-supertrend-kite-production".to_owned(),
            root.join("config/pivot-point-supertrend.json")
                .display()
                .to_string(),
            "missing-private-broker-settings.json".to_owned(),
        ],
        vec![
            "native-pivot-sim".to_owned(),
            root.join("config/production-supertrend.json")
                .display()
                .to_string(),
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_kite-node"))
            .args(args)
            .env("KITE_REDIS_URL", &redis.url)
            .current_dir(redis.dir.path())
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("Pivot")
                || String::from_utf8_lossy(&output.stderr).contains("pivot_point_supertrend")
        );
    }
    let mut con = redis::Client::open(redis.url.as_str())
        .unwrap()
        .get_connection()
        .unwrap();
    assert_eq!(
        redis::cmd("DBSIZE").query::<usize>(&mut con).unwrap(),
        0,
        "Rejected activation created Redis state"
    );
}
