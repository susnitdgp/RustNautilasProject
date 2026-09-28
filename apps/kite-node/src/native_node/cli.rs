use anyhow::{Result, bail};

/// Native Pure Squeeze Momentum v2.28.3 and operational entry points.
pub fn dispatch(args: &[String]) -> Option<Result<()>> {
    let command = args.first()?.as_str();
    if !command.starts_with("native-") {
        return None;
    }

    Some(match (command, &args[1..]) {
        ("native-squeeze-momentum-sim", [config]) => squeeze_selection(config)
            .and_then(|_| super::squeeze_momentum_live_runner::run(config, 30, true)),
        ("native-squeeze-momentum-paper", [config, seconds]) => squeeze_selection(config)
            .and_then(|_| seconds.parse::<u64>().map_err(anyhow::Error::from))
            .and_then(|seconds| super::squeeze_momentum_live_runner::run(config, seconds, false)),
        ("native-squeeze-momentum-kite-mock", [config]) => {
            squeeze_selection(config).and_then(|_| {
                super::squeeze_momentum_live_runner::run_with_execution(config, 30, true, true)
            })
        }
        ("native-squeeze-momentum-record", [config, seconds]) => squeeze_selection(config)
            .and_then(|_| seconds.parse::<u64>().map_err(anyhow::Error::from))
            .and_then(|seconds| super::squeeze_momentum_recorder::run(config, seconds)),
        ("native-squeeze-momentum-backtest-fixture", [config, fixture]) => {
            squeeze_selection(config)
                .and_then(|_| super::squeeze_momentum_backtest::run(config, fixture))
        }
        ("native-squeeze-momentum-dashboard-live", [config, seconds]) => squeeze_selection(config)
            .and_then(|_| seconds.parse::<u64>().map_err(anyhow::Error::from))
            .and_then(|seconds| {
                super::squeeze_momentum_live_runner::run_dashboard(config, seconds)
            }),
        ("native-squeeze-momentum-dashboard-history", [config, date]) => squeeze_selection(config)
            .and_then(|_| super::dashboard::run_history(config, date, false)),
        ("native-squeeze-momentum-dashboard-snapshot", [config, date]) => squeeze_selection(config)
            .and_then(|_| super::dashboard::run_history(config, date, true)),
        ("native-squeeze-momentum-dashboard-summary", [config, from, to]) => {
            squeeze_selection(config)
                .and_then(|_| super::dashboard::run_summary(config, from, to, false))
        }
        ("native-squeeze-momentum-dashboard-summary-snapshot", [config, from, to]) => {
            squeeze_selection(config)
                .and_then(|_| super::dashboard::run_summary(config, from, to, true))
        }
        ("native-squeeze-momentum-kite-production", [config, broker]) => squeeze_selection(config)
            .and_then(|_| super::squeeze_momentum_live_runner::run_broker(config, broker)),
        ("native-squeeze-momentum-production-check", [config, broker]) => squeeze_selection(config)
            .and_then(|_| super::production::production_check(config, broker)),
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
        "Usage: native-squeeze-momentum-sim CONFIG | native-squeeze-momentum-paper CONFIG SECONDS | native-squeeze-momentum-kite-mock CONFIG | native-squeeze-momentum-record CONFIG SECONDS | native-squeeze-momentum-backtest-fixture CONFIG FIXTURE | native-squeeze-momentum-dashboard-live CONFIG SECONDS | native-squeeze-momentum-dashboard-history CONFIG YYYY-MM-DD | native-squeeze-momentum-dashboard-snapshot CONFIG YYYY-MM-DD | native-squeeze-momentum-dashboard-summary CONFIG FROM_YYYY-MM-DD TO_YYYY-MM-DD | native-squeeze-momentum-dashboard-summary-snapshot CONFIG FROM_YYYY-MM-DD TO_YYYY-MM-DD | native-squeeze-momentum-production-check CONFIG BROKER | native-squeeze-momentum-kite-production CONFIG BROKER | native-contract-check CONFIG | native-production-preflight CONFIG | native-kite-auth | native-kite-auth-login-url | native-kite-margins-check CONFIG | native-full-audit CATALOG | native-kite-review NAMESPACE | native-kite-status ACCOUNT | native-kite-mock-release-reviewed NAMESPACE | native-recover NAMESPACE"
    )
}

fn squeeze_selection(path: &str) -> Result<()> {
    let selection = super::production::Selection::load(path)?;
    anyhow::ensure!(
        selection.strategy == "squeeze_momentum_lazybear_v2283",
        "This command requires MCX Crude PURE Squeeze Momentum v2.28.3"
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
