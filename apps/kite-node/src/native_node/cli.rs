use anyhow::{Result, bail};

pub fn dispatch(args: &[String]) -> Option<Result<()>> {
    let command = args.first()?.as_str();
    if !command.starts_with("native-") {
        return None;
    }
    Some(match(command,&args[1..]){
        ("native-ilrc-research",[name,token,lot,is_crude,open,close])=>ilrc_research(name,token,lot,is_crude,open,close),
        ("native-ilrc-date",[name,token,lot,is_crude,open,close,date])=>ilrc_date(name,token,lot,is_crude,open,close,date),
        ("native-ilrc-config-date",[config,date])=>super::ilrc_backtest::run_config_date(config,date),
        ("native-ilrc-production-check",[config,broker])=>super::ilrc_config::production_check(config,broker),
        ("native-ilrc-causal-audit",[config,file,date])=>super::ilrc_causal_audit::run(config,file,date),
        ("native-ilrc-timed-scenario",[config,fixture,date,slippage,kill])=>slippage.parse::<f64>().map_err(anyhow::Error::from).and_then(|v|kill.parse::<usize>().map_err(anyhow::Error::from).and_then(|k|super::ilrc_timed_mock::run_scenario(config,fixture,date,v,k))),
        ("native-ilrc-timed-mock",[config,fixture,date])=>super::ilrc_timed_mock::run(config,fixture,date),
        ("native-ilrc-mock-broker",[])=>super::ilrc_broker_replay::run(),
        ("native-ilrc-mock-execution",[])=>super::ilrc_mock_execution::run(),
        ("native-ilrc-live-readiness",[strategy,broker,policy])=>super::ilrc_live_readiness::check(strategy,broker,policy),
        ("native-ilrc-shadow",[config,seconds])=>seconds.parse::<u64>().map_err(anyhow::Error::from).and_then(|seconds|super::ilrc_shadow::run(config,seconds)),
        ("native-ilrc-quality-research",[name,token])=>token.parse::<u32>().map_err(anyhow::Error::from).and_then(|token|super::ilrc_backtest::run_quality_research(name,token)),
        ("native-ilrc-frequency-research",[name,token])=>token.parse::<u32>().map_err(anyhow::Error::from).and_then(|token|super::ilrc_backtest::run_frequency_research(name,token)),
        ("native-ilrc-continuation-research",[name,token])=>token.parse::<u32>().map_err(anyhow::Error::from).and_then(|token|super::ilrc_continuation_backtest::run(name,token)),
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
fn usage() -> Result<()> {
    bail!(
        "Usage: native-ilrc-config-date CONFIG DATE | native-ilrc-production-check CONFIG BROKER | native-ilrc-shadow CONFIG SECONDS | native-kite-auth | native-kite-status ACCOUNT"
    )
}
