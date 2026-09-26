use anyhow::{Result, bail};

/// Native Trend Ribbon and operational entry points.
pub fn dispatch(args: &[String]) -> Option<Result<()>> {
    let command = args.first()?.as_str();
    if !command.starts_with("native-") {
        return None;
    }

    Some(match (command, &args[1..]) {
        ("native-trend-ribbon-sim", [config]) => trend_ribbon_selection(config)
            .and_then(|_| super::trend_ribbon_live_runner::run(config, 30, true)),
        ("native-trend-ribbon-paper", [config, seconds]) => trend_ribbon_selection(config)
            .and_then(|_| seconds.parse::<u64>().map_err(anyhow::Error::from))
            .and_then(|seconds| super::trend_ribbon_live_runner::run(config, seconds, false)),
        ("native-trend-ribbon-kite-mock", [config]) => {
            trend_ribbon_selection(config).and_then(|_| {
                super::trend_ribbon_live_runner::run_with_execution(config, 30, true, true)
            })
        }
        ("native-trend-ribbon-replay", [config, catalog]) => trend_ribbon_selection(config)
            .and_then(|_| super::trend_ribbon_replay::run(config, catalog)),
        ("native-trend-ribbon-record", [config, seconds]) => trend_ribbon_selection(config)
            .and_then(|_| seconds.parse::<u64>().map_err(anyhow::Error::from))
            .and_then(|seconds| super::trend_ribbon_recorder::run(config, seconds)),
        ("native-trend-ribbon-backtest-fixture", [config, fixture]) => {
            trend_ribbon_selection(config)
                .and_then(|_| super::trend_ribbon_backtest::run(config, fixture))
        }
        ("native-trend-ribbon-dashboard-history", [config, date]) => trend_ribbon_selection(config)
            .and_then(|_| super::dashboard::run_history(config, date, false)),
        ("native-trend-ribbon-dashboard-snapshot", [config, date]) => {
            trend_ribbon_selection(config)
                .and_then(|_| super::dashboard::run_history(config, date, true))
        }
        ("native-trend-ribbon-dashboard-summary", [config, from, to]) => {
            trend_ribbon_selection(config)
                .and_then(|_| super::dashboard::run_summary(config, from, to, false))
        }
        ("native-trend-ribbon-dashboard-summary-snapshot", [config, from, to]) => {
            trend_ribbon_selection(config)
                .and_then(|_| super::dashboard::run_summary(config, from, to, true))
        }
        ("native-trend-ribbon-kite-production", [config, broker]) => trend_ribbon_selection(config)
            .and_then(|_| super::trend_ribbon_live_runner::run_broker(config, broker)),
        ("native-trend-ribbon-production-check", [config, broker]) => {
            trend_ribbon_selection(config)
                .and_then(|_| super::production::production_check(config, broker))
        }
        ("native-contract-check", [config]) => super::production::contract_check(config),
        ("native-production-preflight", [config]) => super::production::preflight(config),
        ("native-kite-auth", []) => kite_adapter::auth::login::interactive().map(|result| {
            println!(
                "{}",
                serde_json::json!({
                    "event":"kite_access_token_updated",
                    "user_id":result.user_id,
                    "redis_key":"susanta:kite_access_token",
                    "access_token_printed":false
                })
            );
        }),
        ("native-kite-auth-login-url", []) => {
            kite_adapter::auth::login::login_url().map(|url| println!("{url}"))
        }
        ("native-kite-margins-check", [config]) => tokio::runtime::Runtime::new()
            .map_err(anyhow::Error::from)
            .and_then(|runtime| {
                runtime.block_on(kite_adapter::execution::native_client::margins::check(
                    config,
                ))
            })
            .map(|value| println!("{value}")),
        ("native-kite-review", [namespace]) => {
            kite_adapter::execution::native_client::recovery::review(namespace)
                .map(|value| println!("{value}"))
        }
        ("native-kite-status", [account]) => {
            kite_adapter::execution::native_client::coordination::status(account)
                .map(|value| println!("{value}"))
        }
        ("native-kite-mock-release-reviewed", [namespace]) => release_reviewed_mock(namespace),
        ("native-full-audit", [catalog]) => super::catalog::audit(std::path::Path::new(catalog)),
        ("native-recover", [namespace]) => super::recovery::run(namespace),
        _ => usage(),
    })
}

fn usage() -> Result<()> {
    bail!(
        "Usage: native-trend-ribbon-sim CONFIG | native-trend-ribbon-paper CONFIG SECONDS | native-trend-ribbon-kite-mock CONFIG | native-trend-ribbon-replay CONFIG CATALOG | native-trend-ribbon-record CONFIG SECONDS | native-trend-ribbon-backtest-fixture CONFIG FIXTURE | native-trend-ribbon-dashboard-history CONFIG YYYY-MM-DD | native-trend-ribbon-dashboard-snapshot CONFIG YYYY-MM-DD | native-trend-ribbon-dashboard-summary CONFIG FROM_YYYY-MM-DD TO_YYYY-MM-DD | native-trend-ribbon-dashboard-summary-snapshot CONFIG FROM_YYYY-MM-DD TO_YYYY-MM-DD | native-trend-ribbon-production-check CONFIG BROKER | native-trend-ribbon-kite-production CONFIG BROKER | native-contract-check CONFIG | native-production-preflight CONFIG | native-kite-auth | native-kite-auth-login-url | native-kite-margins-check CONFIG | native-full-audit CATALOG | native-kite-review NAMESPACE | native-kite-status ACCOUNT | native-kite-mock-release-reviewed NAMESPACE | native-recover NAMESPACE"
    )
}

fn trend_ribbon_selection(path: &str) -> Result<()> {
    let selection = super::production::Selection::load(path)?;
    anyhow::ensure!(
        selection.strategy == "trend_ribbon_boswaves",
        "This command requires the Trend Ribbon v2.10 selection"
    );
    Ok(())
}

fn release_reviewed_mock(namespace: &str) -> Result<()> {
    let review = kite_adapter::execution::native_client::recovery::review(namespace)?;
    anyhow::ensure!(
        review["requires_review"] == false
            && review["unresolved"] == 0
            && review["journal_exposure"] == "0",
        "MOCK namespace recovery review is not clean enough to release"
    );
    kite_adapter::execution::native_client::coordination::release_reviewed_mock(namespace)?;
    let status = kite_adapter::execution::native_client::coordination::status("MOCK")?;
    println!(
        "{}",
        serde_json::json!({
            "event":"native_kite_mock_reviewed_release",
            "namespace":namespace,
            "account_status":status,
            "live_orders_enabled":false
        })
    );
    Ok(())
}
