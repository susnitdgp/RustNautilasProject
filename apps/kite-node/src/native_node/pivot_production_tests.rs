use super::*;

fn selection() -> Selection {
    serde_json::from_str(include_str!(
        "../../../../config/production-pivot-supertrend.json"
    ))
    .unwrap()
}
fn ns(s: &str) -> u64 {
    chrono::DateTime::parse_from_rfc3339(s)
        .unwrap()
        .timestamp_nanos_opt()
        .unwrap() as u64
}
#[test]
fn pivot_production_respects_configured_end_and_existing_market_buffer() {
    let mut s = selection();
    s.validate().unwrap();
    let date = NaiveDate::from_ymd_opt(2026, 9, 22).unwrap();
    assert_eq!(
        s.execution_bounds(date, true).unwrap(),
        (
            ns("2026-09-22T09:00:00+05:30"),
            ns("2026-09-22T23:00:00+05:30")
        )
    );
    s.pivot_point.as_mut().unwrap().session.end = "23:15:00".parse().unwrap();
    assert_eq!(
        s.execution_bounds(date, false).unwrap().1,
        ns("2026-09-22T23:15:00+05:30")
    );
    assert_eq!(
        s.execution_bounds(date, true).unwrap().1,
        ns("2026-09-22T23:00:00+05:30")
    );
    s.pivot_point.as_mut().unwrap().session.end = "22:00:00".parse().unwrap();
    assert_eq!(
        s.execution_bounds(date, true).unwrap().1,
        ns("2026-09-22T22:00:00+05:30")
    );
}
#[test]
fn pivot_production_rejects_late_start_and_honors_calendar_overrides() {
    let s = selection();
    assert_eq!(
        s.production_duration(ns("2026-09-22T09:00:00+05:30"))
            .unwrap(),
        14 * 3600
    );
    assert_eq!(
        s.production_duration(ns("2026-09-22T22:59:44.5+05:30"))
            .unwrap(),
        16
    );
    for time in [
        "2026-09-22T08:59:59+05:30",
        "2026-09-22T22:59:45+05:30",
        "2026-09-22T23:00:00+05:30",
        "2026-10-02T09:00:00+05:30",
    ] {
        assert!(s.production_duration(ns(time)).is_err(), "{time}");
    }
    let mut raw: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../config/production-pivot-supertrend.json"
    ))
    .unwrap();
    raw["session_calendar"]["overrides"]["2026-09-22"] =
        serde_json::json!({"open":"17:00:00","close":"21:00:00"});
    let shortened: Selection = serde_json::from_value(raw.clone()).unwrap();
    shortened.validate().unwrap();
    assert_eq!(
        shortened
            .execution_bounds(NaiveDate::from_ymd_opt(2026, 9, 22).unwrap(), true)
            .unwrap(),
        (
            ns("2026-09-22T17:00:00+05:30"),
            ns("2026-09-22T20:30:00+05:30")
        )
    );
    raw["pivot_point"]["session"]["start"] = "20:45:00".into();
    let too_short: Selection = serde_json::from_value(raw).unwrap();
    assert!(
        too_short
            .production_duration(ns("2026-09-22T20:45:00+05:30"))
            .is_err()
    );
}
#[test]
fn pivot_production_requires_both_config_gates_and_uses_selected_token() {
    let paper: Selection = serde_json::from_str(include_str!(
        "../../../../config/pivot-point-supertrend.json"
    ))
    .unwrap();
    assert!(
        paper
            .broker_settings("missing-broker.json")
            .unwrap_err()
            .to_string()
            .contains("Pivot production requires live_orders_enabled=true")
    );
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("broker.json");
    let mut broker = serde_json::json!({"expected_user_id":"TEST123","product":"MIS",
        "instrument_token":1,"live_orders_enabled":false,"market_protection":-1});
    std::fs::write(&path, serde_json::to_vec(&broker).unwrap()).unwrap();
    let s = selection();
    assert!(s.broker_settings(path.to_str().unwrap()).is_err());
    broker["live_orders_enabled"] = true.into();
    std::fs::write(&path, serde_json::to_vec(&broker).unwrap()).unwrap();
    let gate: kite_adapter::execution::native_client::production::Settings =
        serde_json::from_value(broker.clone()).unwrap();
    if gate.validate().is_ok() {
        let ready = s.broker_settings(path.to_str().unwrap()).unwrap();
        assert_eq!(ready.instrument_token, s.instrument_token);
        assert_eq!(ready.product, "MIS");
        assert_eq!(ready.market_protection, -1);
    } else {
        assert!(
            s.broker_settings(path.to_str().unwrap())
                .unwrap_err()
                .to_string()
                .contains("live-orders build")
        );
    }
    for (key, value) in [
        ("product", serde_json::json!("NRML")),
        ("market_protection", serde_json::json!(0)),
        ("expected_user_id", serde_json::json!("REPLACE_ME")),
    ] {
        let mut invalid = broker.clone();
        invalid[key] = value;
        std::fs::write(&path, serde_json::to_vec(&invalid).unwrap()).unwrap();
        assert!(s.broker_settings(path.to_str().unwrap()).is_err());
    }
}
#[test]
fn original_strategy_keeps_its_existing_activation_and_exit_behavior() {
    let s: Selection = serde_json::from_str(include_str!(
        "../../../../config/production-supertrend.json"
    ))
    .unwrap();
    s.validate().unwrap();
    assert_eq!(
        s.production_duration(ns("2026-09-22T09:00:00+05:30"))
            .unwrap(),
        14 * 3600
    );
    let paper: Selection = serde_json::from_str(include_str!(
        "../../../../config/pivot-point-supertrend.json"
    ))
    .unwrap();
    assert_eq!(
        paper
            .session_duration(ns("2026-09-22T09:00:00+05:30"))
            .unwrap(),
        14 * 3600 + 15 * 60
    );
}
