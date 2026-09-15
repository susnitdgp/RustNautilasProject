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
    for cmd in ["native-backtest", "native-node-sim", "native-kite-mock"] {
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
        if cmd == "native-kite-mock" {
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
