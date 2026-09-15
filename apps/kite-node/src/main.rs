mod cli;
mod market_data_command;
mod preflight_command;

fn main() -> anyhow::Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match cli::parse(&args)? {
        cli::Command::Preflight { config, csv } => preflight_command::run(&config, csv.as_deref()),
        cli::Command::SessionCheck { config } => market_data_command::session_check(&config),
        cli::Command::Stream { config, seconds } => market_data_command::stream(&config, seconds),
    }
}
