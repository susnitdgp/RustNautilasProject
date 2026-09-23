//! CLI and broker-path checks use a dedicated disposable Redis instance.
include!("support/redis.rs");

fn config(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../config/backup")
        .join(name)
}
#[test]
fn production_gates_and_configuration_check_do_not_start_trading_or_write_redis() {
    let redis = Redis::start();
    let broker = redis.dir.path().join("broker.json");
    let mut settings = serde_json::json!({"expected_user_id":"TEST123","product":"MIS",
        "instrument_token":1,"live_orders_enabled":false,"market_protection":-1});
    std::fs::write(&broker, serde_json::to_vec(&settings).unwrap()).unwrap();
    for (command, selection, expected) in [
        (
            "native-pivot-kite-production",
            "pivot-point-supertrend.json",
            "Pivot production requires",
        ),
        (
            "native-pivot-kite-production",
            "production-supertrend.json",
            "requires a pivot_point_supertrend",
        ),
        (
            "native-supertrend-kite-production",
            "production-pivot-supertrend.json",
            "Pivot Point production requires",
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_kite-node"))
            .args([
                command,
                config(selection).to_str().unwrap(),
                broker.to_str().unwrap(),
            ])
            .env("KITE_REDIS_URL", &redis.url)
            .env("KITE_SLACK_ALERTS", "0")
            .current_dir(redis.dir.path())
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains(expected));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("supertrend_live_started"));
    }
    let output = Command::new(env!("CARGO_BIN_EXE_kite-node"))
        .args([
            "native-pivot-kite-production",
            config("production-pivot-supertrend.json").to_str().unwrap(),
            broker.to_str().unwrap(),
        ])
        .env("KITE_REDIS_URL", &redis.url)
        .env("KITE_SLACK_ALERTS", "0")
        .current_dir(redis.dir.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("disabled in broker settings") || error.contains("live-orders build"));
    settings["live_orders_enabled"] = true.into();
    std::fs::write(&broker, serde_json::to_vec(&settings).unwrap()).unwrap();
    let gate: kite_adapter::execution::native_client::production::Settings =
        serde_json::from_value(settings).unwrap();
    if gate.validate().is_ok() {
        let check = redis.run(&[
            "native-pivot-production-check",
            config("production-pivot-supertrend.json").to_str().unwrap(),
            broker.to_str().unwrap(),
        ]);
        assert_eq!(check["configuration_valid"], true);
        assert_eq!(check["engine_started"], false);
        assert_eq!(check["account_checked"], false);
        assert_eq!(check["broker_orders_sent"], false);
        let selection: serde_json::Value = serde_json::from_slice(
            &std::fs::read(config("production-pivot-supertrend.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(check["instrument_token"], selection["instrument_token"]);
        assert_eq!(check["market_protection"], -1);
    }
    let mut con = redis::Client::open(redis.url.as_str())
        .unwrap()
        .get_connection()
        .unwrap();
    assert_eq!(redis::cmd("DBSIZE").query::<usize>(&mut con).unwrap(), 0);
}

#[test]
fn production_pivot_selection_uses_native_mock_and_reconciles_both_sides() {
    let redis = Redis::start();
    let result = redis.run(&[
        "native-pivot-kite-mock",
        config("production-pivot-supertrend.json").to_str().unwrap(),
    ]);
    assert_eq!(result["execution"], "Kite native mock");
    assert_eq!(result["status"], "Clean");
    assert_eq!(result["live_orders_enabled"], false);
    assert_eq!(result["broker_orders_sent"], false);
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
            "{intent}: {signals:?}"
        );
    }
    assert!(signals.iter().any(|s| s["reason"] == "session_end"));
    let recovered = redis.run(&["native-recover", result["namespace"].as_str().unwrap()]);
    assert_eq!(recovered["requires_review"], false);
    assert_eq!(recovered["resubmissions"], 0);
    assert_eq!(recovered["orders"], result["fills"]);
    let health = redis.run(&["native-kite-status", "MOCK"]);
    assert_eq!(health["state"], "Clean");
}

#[cfg(unix)]
#[test]
fn pivot_native_mock_shutdown_confirms_reducing_fill_before_clean_stop() {
    use std::io::{BufRead, BufReader};
    let redis = Redis::start();
    let mut child = Command::new(env!("CARGO_BIN_EXE_kite-node"))
        .args([
            "native-pivot-kite-mock",
            config("production-pivot-supertrend.json").to_str().unwrap(),
        ])
        .env("KITE_REDIS_URL", &redis.url)
        .env("KITE_SLACK_ALERTS", "0")
        .current_dir(redis.dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let started: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(started["execution"], "Kite native mock");
    assert_eq!(started["live_orders_enabled"], false);
    child.stdout = Some(reader.into_inner());
    let mut con = redis::Client::open(redis.url.as_str())
        .unwrap()
        .get_connection()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        let position: Option<String> = redis::cmd("HGET")
            .arg("susanta:nautilus:native-kite:account:{MOCK}")
            .arg("position")
            .query(&mut con)
            .unwrap();
        if position
            .as_deref()
            .and_then(|s| s.parse::<f64>().ok())
            .is_some_and(|p| p != 0.)
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Mock never opened a position"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while child.try_wait().unwrap().is_none() {
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            panic!("Shutdown did not finish");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str(line).ok())
        .unwrap();
    assert_eq!(result["status"], "Clean");
    assert_eq!(result["open_contracts"].as_f64(), Some(0.));
    assert_eq!(result["open_orders"], 0);
    assert!(result["fills"].as_u64().unwrap() >= 2);
    let folder = redis
        .dir
        .path()
        .join(result["report_directory"].as_str().unwrap());
    let signals: Vec<serde_json::Value> =
        serde_json::from_slice(&std::fs::read(folder.join("signals.json")).unwrap()).unwrap();
    assert!(
        signals.iter().any(|s| s["reason"] == "shutdown"
            && (s["intent"] == "BUY_EXIT" || s["intent"] == "SELL_EXIT"))
    );
    assert_eq!(redis.run(&["native-kite-status", "MOCK"])["state"], "Clean");
}
