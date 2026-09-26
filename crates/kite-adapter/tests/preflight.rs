use chrono::NaiveDate;
use kite_adapter::{config::Config, preflight};

const CONFIG: &str = include_str!("fixtures/crudeoil-september.toml");
const HEADER: &str = "instrument_token,tradingsymbol,name,expiry,tick_size,lot_size,instrument_type,segment,exchange\n";
const ROW: &str = "144870151,CRUDEOIL26SEPFUT,CRUDEOIL,2026-09-21,1,1,FUT,MCX-FUT,MCX\n";

fn check(csv: &str) -> anyhow::Result<preflight::Report> {
    preflight::run(
        &Config::parse(CONFIG)?,
        csv.as_bytes(),
        NaiveDate::from_ymd_opt(2026, 9, 15).unwrap(),
    )
}
fn master(row: &str) -> String {
    format!("{HEADER}{row}")
}

#[test]
fn resolves_into_real_nautilus_identity_without_enabling_orders() {
    let report = check(&master(ROW)).unwrap();
    assert_eq!(report.instrument_id, "CRUDEOIL26SEPFUT.MCX");
    assert_eq!(report.instrument_token, 144870151);
    assert_eq!(report.tick_size, "1");
    assert!(!report.live_orders_enabled);
    assert!(!report.contract_multiplier_verified);
    assert!(!report.engine_started);
}
#[test]
fn live_mode_cannot_be_enabled() {
    assert!(Config::parse(&CONFIG.replace("data_only", "live")).is_err());
}
#[test]
fn rejects_unknown_configuration_instead_of_ignoring_typo() {
    assert!(Config::parse(&format!("{CONFIG}\norder_enabled=true")).is_err());
}
#[test]
fn rejects_inconsistent_configured_month() {
    assert!(Config::parse(&CONFIG.replace("expiry_month = 9", "expiry_month = 10")).is_err());
}
#[test]
fn rejects_wrong_venue_config() {
    assert!(Config::parse(&CONFIG.replace("\"MCX\"", "\"NSE\"")).is_err());
}
#[test]
fn rejects_missing_contract() {
    assert!(check(&master(&ROW.replace("CRUDEOIL", "CRUDEOILM"))).is_err());
}
#[test]
fn rejects_ambiguous_matching_contracts() {
    assert!(check(&master(&format!("{ROW}{ROW}"))).is_err());
}
#[test]
fn rejects_unexpected_symbol() {
    assert!(
        check(&master(
            &ROW.replace("CRUDEOIL26SEPFUT", "CRUDEOIL26SEPALT")
        ))
        .is_err()
    );
}
#[test]
fn rejects_unexpected_expiry() {
    assert!(check(&master(&ROW.replace("2026-09-21", "2026-09-22"))).is_err());
}
#[test]
fn rejects_expired_contract_without_auto_roll() {
    let config = Config::parse(CONFIG).unwrap();
    assert!(
        preflight::run(
            &config,
            master(ROW).as_bytes(),
            NaiveDate::from_ymd_opt(2026, 9, 22).unwrap()
        )
        .is_err()
    );
}
#[test]
fn rejects_token_reused_by_another_row() {
    let other = ROW.replace("CRUDEOIL", "NATURALGAS");
    assert!(check(&master(&format!("{ROW}{other}"))).is_err());
}
#[test]
fn rejects_zero_token() {
    assert!(check(&master(&ROW.replace("144870151", "0"))).is_err());
}
#[test]
fn rejects_zero_lot_size() {
    assert!(check(&master(&ROW.replace(",1,1,FUT", ",1,0,FUT"))).is_err());
}
#[test]
fn rejects_invalid_tick_sizes() {
    for tick in ["0", "-1", "NaN", "inf", "invalid"] {
        assert!(check(&master(&ROW.replace(",1,1,FUT", &format!(",{tick},1,FUT")))).is_err());
    }
}
#[test]
fn rejects_missing_and_duplicate_headers() {
    assert!(check(&master(ROW).replace("instrument_token,", "wrong_name,")).is_err());
    let csv = format!("instrument_token,{HEADER}144870151,{ROW}");
    assert!(check(&csv).is_err());
}
#[test]
fn rejects_html_error_and_empty_response() {
    assert!(check("<html>Unavailable</html>").is_err());
    assert!(check("").is_err());
    assert!(check(HEADER).is_err());
}
#[test]
fn rejects_malformed_csv_or_expiry() {
    assert!(check(&master("1,broken\n")).is_err());
    assert!(check(&master(&ROW.replace("2026-09-21", "not-a-date"))).is_err());
}
#[test]
fn ignores_mini_options_and_other_expiries() {
    let mini = ROW
        .replace("144870151", "123")
        .replace("CRUDEOIL", "CRUDEOILM");
    let option = ROW.replace("144870151", "124").replace(",FUT,", ",CE,");
    let october = ROW
        .replace("144870151", "125")
        .replace("2026-09-21", "2026-10-19")
        .replace("26SEP", "26OCT");
    let report = check(&master(&format!("{mini}{option}{october}{ROW}"))).unwrap();
    assert_eq!(report.instrument_id, "CRUDEOIL26SEPFUT.MCX");
}
