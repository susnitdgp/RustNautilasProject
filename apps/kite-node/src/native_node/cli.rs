use anyhow::{Result, bail};

pub fn dispatch(args: &[String]) -> Option<Result<()>> {
    let command = args.first()?.as_str();
    if !command.starts_with("native-") {
        return None;
    }
    Some(match(command,&args[1..]){
        ("native-portfolio-validate",[config])=>super::portfolio::inspect(config),
        ("native-sats-backtest",[portfolio,instance,from,to])=>super::sats_backtest::run(portfolio,instance,from,to),
        ("native-sniper-backtest",[config,from,to])=>super::sniper_backtest::run(config,from,to),
        ("native-sniper-paper",[portfolio,instance])=>super::sniper_live::run(portfolio,instance,None,super::sniper_live::Mode::Paper),
        ("native-sniper-live-preflight",[portfolio,instance,broker])=>super::sniper_live::preflight(portfolio,instance,broker),
        ("native-sniper-live",[portfolio,instance,broker])=>super::sniper_live::run(portfolio,instance,Some(broker),super::sniper_live::Mode::Live),
        ("native-sats-paper",[portfolio,instance])=>super::sats_live::run(portfolio,instance,None,super::sats_live::Mode::Paper),
        ("native-sats-live-preflight",[portfolio,instance,broker])=>super::sats_live::preflight(portfolio,instance,broker),
        ("native-sats-live",[portfolio,instance,broker])=>super::sats_live::run(portfolio,instance,Some(broker),super::sats_live::Mode::Live),
        ("native-kite-auth",[])=>kite_adapter::auth::login::interactive().map(|r|println!("{}",serde_json::json!({"event":"kite_access_token_updated","user_id":r.user_id,"redis_key":"susanta:kite_access_token","access_token_printed":false}))),
        ("native-kite-auth-login-url",[])=>kite_adapter::auth::login::login_url().map(|url|println!("{url}")),
        ("native-kite-margins-check",[config])=>tokio::runtime::Runtime::new().map_err(anyhow::Error::from).and_then(|rt|rt.block_on(kite_adapter::execution::native_client::margins::check(config))).map(|v|println!("{v}")),
        _=>usage(),
    })
}

fn usage() -> Result<()> {
    bail!("Usage: native-portfolio-validate CONFIG | native-sats-backtest PORTFOLIO INSTANCE FROM TO | native-sniper-backtest CONFIG FROM TO | native-sniper-paper PORTFOLIO INSTANCE | native-sniper-live-preflight PORTFOLIO INSTANCE BROKER | native-sniper-live PORTFOLIO INSTANCE BROKER | native-sats-paper PORTFOLIO INSTANCE | native-sats-live-preflight PORTFOLIO INSTANCE BROKER | native-sats-live PORTFOLIO INSTANCE BROKER | native-kite-auth")
}
