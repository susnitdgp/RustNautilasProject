mod cli;
mod execution_command;
mod management_command;
mod market_data_command;
mod preflight_command;
mod rate_limit_command;
mod reconciliation_command;
mod reports_command;
mod runtime;
mod strategy_command;

fn main() -> anyhow::Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match cli::parse(&args)? {
        cli::Command::Strategy { config, input } => {
            strategy_command::run(&config, input.as_deref())
        }
        cli::Command::ReportsSim => reports_command::run(),
        cli::Command::ManagementSim => management_command::run(),
        cli::Command::RateLimitSim => rate_limit_command::run(),
        cli::Command::ExecutionSim { namespace } => execution_command::run(namespace.as_deref()),
        cli::Command::Reconcile { config } => reconciliation_command::run(&config),
        cli::Command::Capture {
            config,
            seconds,
            output,
        } => runtime::capture::run(&config, seconds, std::path::Path::new(&output)),
        cli::Command::Replay { input } => runtime::replay::run(std::path::Path::new(&input)),
        cli::Command::Preflight { config, csv } => preflight_command::run(&config, csv.as_deref()),
        cli::Command::SessionCheck { config } => market_data_command::session_check(&config),
        cli::Command::Stream { config, seconds } => market_data_command::stream(&config, seconds),
    }
}
