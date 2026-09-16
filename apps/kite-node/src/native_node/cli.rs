use anyhow::{Result, bail};
/// Parse the native entry points without silently ignoring extra arguments.
pub fn dispatch(args: &[String]) -> Option<Result<()>> {
    let command = args.first()?.as_str();
    if !command.starts_with("native-") {
        return None;
    }
    Some(match (command, &args[1..]) {
        ("native-kite-custom-preflight", []) => custom_probe("config/kite-custom-sandbox.toml"),
        ("native-kite-custom-preflight", [config]) => custom_probe(config),
        ("native-kite-sandbox-preflight", []) => tokio::runtime::Runtime::new()
            .map_err(anyhow::Error::from)
            .and_then(|r| r.block_on(kite_adapter::execution::native_client::sandbox::preflight()))
            .map(|v| println!("{}", v)),
        ("native-kite-sandbox", []) => super::runner::run_kite_sandbox(
            "config/kite-sandbox.toml",
            "config/strategy-crossover.toml",
        ),
        ("native-kite-sandbox", [settings, strategy]) => {
            super::runner::run_kite_sandbox(settings, strategy)
        }
        ("native-kite-review", [namespace]) => {
            kite_adapter::execution::native_client::recovery::review(namespace)
                .map(|v| println!("{}", v))
        }
        ("native-full-audit", [catalog]) => super::catalog::audit(std::path::Path::new(catalog)),
        ("native-kite-status", [account]) => {
            kite_adapter::execution::native_client::coordination::status(account)
                .map(|v| println!("{}", v))
        }
        ("native-kite-mock-short", []) => {
            super::runner::run_kite_mock_short("config/strategy-crossover.toml")
        }
        ("native-kite-mock-short", [strategy]) => super::runner::run_kite_mock_short(strategy),
        ("native-kite-mock", []) => super::runner::run_kite_mock("config/strategy-crossover.toml"),
        ("native-kite-mock", [strategy]) => super::runner::run_kite_mock(strategy),
        ("native-node-sim", []) => super::runner::run(None, "config/strategy-crossover.toml", 30),
        ("native-node-sim", [strategy]) => super::runner::run(None, strategy, 30),
        ("native-node-paper", []) => super::runner::run(
            Some("config/crudeoil-september.toml"),
            "config/strategy-crossover.toml",
            30,
        ),
        ("native-node-paper", [instrument, strategy]) => {
            super::runner::run(Some(instrument), strategy, 30)
        }
        ("native-node-paper", [instrument, strategy, flag, seconds]) if flag == "--seconds" => {
            seconds
                .parse::<u64>()
                .map_err(anyhow::Error::from)
                .and_then(|seconds| {
                    anyhow::ensure!((1..=300).contains(&seconds), "Seconds must be 1..300");
                    super::runner::run(Some(instrument), strategy, seconds)
                })
        }
        ("native-vwap-backtest", [date]) => super::vwap_batch::run(date, None),
        ("native-vwap-backtest", [date, path]) => super::vwap_batch::run(date, Some(path)),
        ("native-vwap-session", [date, path, folder]) => {
            super::vwap_backtest::run(date, path, folder)
        }
        ("native-supertrend-backtest", [date]) => super::supertrend_backtest::run(date, None),
        ("native-supertrend-backtest", [date, path]) => {
            super::supertrend_backtest::run(date, Some(path))
        }
        ("native-backtest", []) => super::backtest::run("config/strategy-crossover.toml", None),
        ("native-backtest", [strategy]) => super::backtest::run(strategy, None),
        ("native-backtest", [strategy, catalog]) => super::backtest::run(strategy, Some(catalog)),
        ("native-emulator-sim", []) => super::components::run(false),
        ("native-twap-sim", []) => super::components::run(true),
        ("native-recover", [namespace]) => super::recovery::run(namespace),
        ("native-node-live", _) => Err(anyhow::anyhow!(
            "Real Kite broker orders remain disabled; paper execution remains enforced. Use native-kite-mock for adapter integration tests"
        )),
        _ => usage(),
    })
}
fn usage() -> Result<()> {
    bail!(
        "Usage: native-vwap-backtest YYYY-MM-DD [historical_input.json] | native-supertrend-backtest YYYY-MM-DD [candles.json] | native-kite-mock [strategy.toml] | native-node-sim [strategy.toml] | native-node-paper [instrument.toml strategy.toml [--seconds N]] | native-backtest [strategy.toml [catalog]] | native-emulator-sim | native-twap-sim | native-recover NAMESPACE"
    )
}

fn custom_probe(path: &str) -> Result<()> {
    let text = std::fs::read_to_string(path)?;
    let result = tokio::runtime::Runtime::new()?
        .block_on(kite_adapter::execution::native_client::custom_sandbox::probe(&text))?;
    println!("{}", result);
    anyhow::ensure!(
        result["read_shapes_compatible"] == true,
        "Custom sandbox is incompatible with native execution; see checks above"
    );
    Ok(())
}
