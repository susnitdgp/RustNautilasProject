use anyhow::{Result, bail};

pub fn dispatch(args: &[String]) -> Option<Result<()>> {
    let command = args.first()?.as_str();
    if !command.starts_with("native-") {
        return None;
    }
    Some(match(command,&args[1..]){
        ("native-smbc-sim",[config])=>smbc_selection(config).and_then(|_|super::smbc_live_runner::run(config,30,true)),
        ("native-smbc-paper",[config,seconds])=>smbc_selection(config).and_then(|_|seconds.parse::<u64>().map_err(anyhow::Error::from)).and_then(|s|super::smbc_live_runner::run(config,s,false)),
        ("native-smbc-kite-mock",[config])=>smbc_selection(config).and_then(|_|super::smbc_live_runner::run_with_execution(config,30,true,true)),
        ("native-smbc-record",[config,seconds])=>smbc_selection(config).and_then(|_|seconds.parse::<u64>().map_err(anyhow::Error::from)).and_then(|s|super::smbc_recorder::run(config,s)),
        ("native-smbc-backtest-fixture",[config,fixture])=>smbc_selection(config).and_then(|_|super::smbc_backtest::run(config,fixture)),
        ("native-smbc-backtest-date",[config,date])=>smbc_selection(config).and_then(|_|super::smbc_backtest::run_date(config,date)),
        ("native-smbc-backtest-month",[config,month])=>smbc_selection(config).and_then(|_|super::smbc_backtest::run_month(config,month)),
        ("native-smbc-scan",[config])=>smbc_selection(config).and_then(|_|super::smbc_backtest::run_scan(config)),
        ("native-smbc-equity",[config])=>smbc_selection(config).and_then(|_|super::smbc_backtest::run_equity(config)),
        ("native-smbc-timeframes",[config])=>smbc_selection(config).and_then(|_|super::smbc_backtest::run_timeframes(config)),
        ("native-smbc-index-timeframes",[config,name,token,lot])=>smbc_selection(config).and_then(|_|token.parse::<u32>().map_err(anyhow::Error::from)).and_then(|token|lot.parse::<u32>().map_err(anyhow::Error::from).and_then(|lot|super::smbc_backtest::run_index_timeframes(config,name,token,lot))),
        ("native-ilrc-research",[name,token,lot,is_crude,open,close])=>ilrc_research(name,token,lot,is_crude,open,close),
        ("native-ilrc-date",[name,token,lot,is_crude,open,close,date])=>ilrc_date(name,token,lot,is_crude,open,close,date),
        ("native-ilrc-config-date",[config,date])=>super::ilrc_backtest::run_config_date(config,date),
        ("native-ilrc-production-check",[config,broker])=>super::ilrc_config::production_check(config,broker),
        ("native-ilrc-shadow",[config,seconds])=>seconds.parse::<u64>().map_err(anyhow::Error::from).and_then(|seconds|super::ilrc_shadow::run(config,seconds)),
        ("native-liquidity-pools-research",[name,token])=>token.parse::<u32>().map_err(anyhow::Error::from).and_then(|token|super::liquidity_pools_backtest::run(name,token)),
        ("native-sats-research",[name,token])=>token.parse::<u32>().map_err(anyhow::Error::from).and_then(|token|super::sats_backtest::run(name,token)),
        ("native-mirage-research",[name,token])=>token.parse::<u32>().map_err(anyhow::Error::from).and_then(|token|super::mirage_backtest::run(name,token)),
        ("native-ilrc-quality-research",[name,token])=>token.parse::<u32>().map_err(anyhow::Error::from).and_then(|token|super::ilrc_backtest::run_quality_research(name,token)),
        ("native-ilrc-frequency-research",[name,token])=>token.parse::<u32>().map_err(anyhow::Error::from).and_then(|token|super::ilrc_backtest::run_frequency_research(name,token)),
        ("native-ilrc-continuation-research",[name,token])=>token.parse::<u32>().map_err(anyhow::Error::from).and_then(|token|super::ilrc_continuation_backtest::run(name,token)),
        ("native-smbc-kite-production",[config,broker])=>smbc_selection(config).and_then(|_|super::smbc_live_runner::run_broker(config,broker)),
        ("native-smbc-production-check",[config,broker])=>smbc_selection(config).and_then(|_|super::production::production_check(config,broker)),
        ("native-contract-check",[config])=>super::production::contract_check(config),
        ("native-production-preflight",[config])=>super::production::preflight(config),
        ("native-kite-auth",[])=>kite_adapter::auth::login::interactive().map(|r|println!("{}",serde_json::json!({"event":"kite_access_token_updated","user_id":r.user_id,"redis_key":"susanta:kite_access_token","access_token_printed":false}))),
        ("native-kite-auth-login-url",[])=>kite_adapter::auth::login::login_url().map(|url|println!("{url}")),
        ("native-kite-margins-check",[config])=>tokio::runtime::Runtime::new().map_err(anyhow::Error::from).and_then(|rt|rt.block_on(kite_adapter::execution::native_client::margins::check(config))).map(|v|println!("{v}")),
        ("native-kite-review",[namespace])=>kite_adapter::execution::native_client::recovery::review(namespace).map(|v|println!("{v}")),
        ("native-kite-status",[account])=>kite_adapter::execution::native_client::coordination::status(account).map(|v|println!("{v}")),
        ("native-full-audit",[catalog])=>super::catalog::audit(std::path::Path::new(catalog)),
        ("native-recover",[namespace])=>super::recovery::run(namespace),
        _=>usage(),
    })
}

fn ilrc_date(
    name: &str,
    token: &str,
    lot: &str,
    is_crude: &str,
    open: &str,
    close: &str,
    date: &str,
) -> Result<()> {
    super::ilrc_backtest::run_date(
        name,
        token.parse()?,
        lot.parse()?,
        is_crude.parse()?,
        open.parse()?,
        close.parse()?,
        date,
    )
}
fn ilrc_research(
    name: &str,
    token: &str,
    lot: &str,
    is_crude: &str,
    open: &str,
    close: &str,
) -> Result<()> {
    super::ilrc_backtest::run_research(
        name,
        token.parse()?,
        lot.parse()?,
        is_crude.parse()?,
        open.parse()?,
        close.parse()?,
    )
}
fn smbc_selection(path: &str) -> Result<()> {
    let s = super::production::Selection::load(path)?;
    anyhow::ensure!(
        s.strategy == "smart_money_breakout_channels_v17",
        "This command requires Smart Money Breakout Channels v1.7"
    );
    Ok(())
}
fn usage() -> Result<()> {
    bail!(
        "Usage: native-smbc-sim CONFIG | native-smbc-paper CONFIG SECONDS | native-smbc-kite-mock CONFIG | native-smbc-record CONFIG SECONDS | native-smbc-backtest-fixture CONFIG FIXTURE | native-smbc-production-check CONFIG BROKER | native-smbc-kite-production CONFIG BROKER | native-contract-check CONFIG | native-production-preflight CONFIG | native-kite-auth | native-kite-auth-login-url | native-kite-margins-check CONFIG | native-kite-review NAMESPACE | native-kite-status ACCOUNT | native-full-audit CATALOG | native-recover NAMESPACE"
    )
}
