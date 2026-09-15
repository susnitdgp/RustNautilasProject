use anyhow::{Result, bail};
/// Parse the native entry points without silently ignoring extra arguments.
pub fn dispatch(args: &[String]) -> Option<Result<()>> {
    let command = args.first()?.as_str();
    if !command.starts_with("native-") {
        return None;
    }
    Some(match (command, &args[1..]) {
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
        ("native-backtest", []) => super::backtest::run("config/strategy-crossover.toml", None),
        ("native-backtest", [strategy]) => super::backtest::run(strategy, None),
        ("native-backtest", [strategy, catalog]) => super::backtest::run(strategy, Some(catalog)),
        ("native-emulator-sim", []) => super::components::run(false),
        ("native-twap-sim", []) => super::components::run(true),
        ("native-recover", [namespace]) => super::recovery::run(namespace),
        ("native-node-live", _) => Err(anyhow::anyhow!(
            "Live broker execution is not integrated into this native node; paper execution remains enforced"
        )),
        _ => usage(),
    })
}
fn usage() -> Result<()> {
    bail!(
        "Usage: native-node-sim [strategy.toml] | native-node-paper [instrument.toml strategy.toml [--seconds N]] | native-backtest [strategy.toml [catalog]] | native-emulator-sim | native-twap-sim | native-recover NAMESPACE"
    )
}
