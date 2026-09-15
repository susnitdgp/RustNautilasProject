use anyhow::{Result, bail, ensure};

pub enum Command {
    PaperRecover {
        namespace: String,
    },
    PaperFlowSim {
        config: String,
    },
    PaperLive {
        instrument: String,
        strategy: String,
        seconds: u64,
    },
    NativePaper {
        config: String,
    },
    Strategy {
        config: String,
        input: Option<String>,
    },
    ReportsSim,
    ManagementSim,
    RateLimitSim,
    ExecutionSim {
        namespace: Option<String>,
    },
    Reconcile {
        config: String,
    },
    Capture {
        config: String,
        seconds: u64,
        output: String,
    },
    Replay {
        input: String,
    },
    Preflight {
        config: String,
        csv: Option<String>,
    },
    SessionCheck {
        config: String,
    },
    Stream {
        config: String,
        seconds: u64,
    },
}

pub fn parse(args: &[String]) -> Result<Command> {
    match args {
        [command, namespace] if command == "paper-recover" => Ok(Command::PaperRecover {
            namespace: namespace.clone(),
        }),
        [command, config] if command == "paper-flow-sim" => Ok(Command::PaperFlowSim {
            config: config.clone(),
        }),
        [command, instrument, strategy, flag, seconds]
            if command == "paper-live" && flag == "--seconds" =>
        {
            let seconds = seconds
                .parse::<u64>()
                .map_err(|_| anyhow::anyhow!("Invalid paper duration"))?;
            ensure!(
                (1..=300).contains(&seconds),
                "Paper duration must be 1..300 seconds"
            );
            Ok(Command::PaperLive {
                instrument: instrument.clone(),
                strategy: strategy.clone(),
                seconds,
            })
        }
        [command, config] if command == "nautilus-paper-sim" => Ok(Command::NativePaper {
            config: config.clone(),
        }),
        [command, config] if command == "strategy-sim" => Ok(Command::Strategy {
            config: config.clone(),
            input: None,
        }),
        [command, config, flag, input] if command == "strategy-replay" && flag == "--input" => {
            Ok(Command::Strategy {
                config: config.clone(),
                input: Some(input.clone()),
            })
        }
        [command] if command == "reports-sim" => Ok(Command::ReportsSim),
        [command] if command == "order-management-sim" => Ok(Command::ManagementSim),
        [command] if command == "rate-limit-sim" => Ok(Command::RateLimitSim),
        [command] if command == "execution-sim" => Ok(Command::ExecutionSim { namespace: None }),
        [command, flag, namespace] if command == "execution-sim" && flag == "--namespace" => {
            Ok(Command::ExecutionSim {
                namespace: Some(namespace.clone()),
            })
        }
        [command, config] if command == "reconcile" => Ok(Command::Reconcile {
            config: config.clone(),
        }),
        [command, input] if command == "replay" => Ok(Command::Replay {
            input: input.clone(),
        }),
        [command, config, flag, seconds, out_flag, output]
            if command == "capture" && flag == "--seconds" && out_flag == "--output" =>
        {
            let seconds = seconds
                .parse::<u64>()
                .map_err(|_| anyhow::anyhow!("Invalid duration"))?;
            ensure!(
                (1..=300).contains(&seconds),
                "Duration must be 1..300 seconds"
            );
            Ok(Command::Capture {
                config: config.clone(),
                seconds,
                output: output.clone(),
            })
        }
        [command, config, source] if command == "preflight" && source == "--download" => {
            Ok(Command::Preflight {
                config: config.clone(),
                csv: None,
            })
        }
        [command, config, source, file] if command == "preflight" && source == "--csv" => {
            Ok(Command::Preflight {
                config: config.clone(),
                csv: Some(file.clone()),
            })
        }
        [command, config] if command == "session-check" => Ok(Command::SessionCheck {
            config: config.clone(),
        }),
        [command, config, flag, seconds] if command == "stream" && flag == "--seconds" => {
            let seconds = seconds
                .parse::<u64>()
                .map_err(|_| anyhow::anyhow!("Invalid duration"))?;
            ensure!(
                (1..=300).contains(&seconds),
                "Duration must be 1..300 seconds"
            );
            Ok(Command::Stream {
                config: config.clone(),
                seconds,
            })
        }
        _ => bail!(
            "Usage: kite-node preflight CONFIG --download | preflight CONFIG --csv FILE | session-check CONFIG | stream CONFIG --seconds 15 | capture CONFIG --seconds 15 --output FILE | replay FILE | reconcile CONFIG | execution-sim [--namespace NEW_NAME] | rate-limit-sim | order-management-sim | reports-sim | paper-flow-sim CONFIG | paper-live INSTRUMENT_CONFIG STRATEGY_CONFIG --seconds 30 | paper-recover NAMESPACE | nautilus-paper-sim CONFIG | strategy-sim CONFIG | strategy-replay CONFIG --input FILE"
        ),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn validates_duration_and_rejects_order_commands() {
        for args in [
            vec!["stream", "config", "--seconds", "0"],
            vec!["stream", "config", "--seconds", "301"],
            vec!["stream", "config", "--seconds", "secret-sentinel"],
            vec!["order", "BUY"],
        ] {
            let args = args.into_iter().map(String::from).collect::<Vec<_>>();
            let error = super::parse(&args).err().unwrap().to_string();
            assert!(!error.contains("secret-sentinel"));
        }
    }
}
