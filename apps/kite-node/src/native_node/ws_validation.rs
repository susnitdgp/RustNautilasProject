//! Read-only live Kite WebSocket vs broker historical candle diagnostic.
//! Does not construct the Nautilus live node or execution client.
use super::ws_candles::Aggregator;
use anyhow::{Result, ensure};
use chrono::{FixedOffset, Utc};
use kite_adapter::{
    credentials::redis,
    http::historical::{self, Interval},
    websocket::{
        supervisor::{self, FeedEvent},
        transport,
    },
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::time::Instant;

pub fn run(token: u32, seconds: u64) -> Result<()> {
    ensure!(
        token > 0 && (200..=1200).contains(&seconds),
        "validation duration must be 200..1200 seconds"
    );
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async move {
        let creds=redis::load_from_env()?;
        let socket=transport::connect(&creds,Instant::now()+Duration::from_secs(10)).await?;
        let state=Arc::new(Mutex::new((Aggregator::new(0),Vec::new(),0u64,0u64,None::<String>)));
        let captured=state.clone();
        let summary=supervisor::observe_connected(&creds,token,Duration::from_secs(seconds),move |event| {
            let mut guard=captured.lock().expect("capture lock");
            match event {
                FeedEvent::Connected{..}=>{},
                FeedEvent::Gap{..}=>guard.4=Some("Kite WebSocket gap".into()),
                FeedEvent::Snapshot(s)=>{
                    guard.2+=1;
                    if guard.4.is_some(){return;}
                    match guard.0.observe(&s) {
                        Ok(Some(c))=>{ guard.3+=1; guard.1.push(c); },
                        Ok(None)=>{},
                        Err(e)=>guard.4=Some(e.to_string()),
                    }
                }
            }
        },socket).await?;
        let (bars,ticks,emitted,failure)={let g=state.lock().expect("capture lock");(g.1.clone(),g.2,g.3,g.4.clone())};
        println!("{}",serde_json::json!({"event":"websocket_validation_capture","ticks":ticks,"bars":emitted,"error":failure,"summary_debug":format!("{summary:?}")}));
        ensure!(failure.is_none(),"WebSocket data invalid: {:?}",failure);
        ensure!(!bars.is_empty(),"no full streaming candle observed; keep live trading stopped");
        // Allow historical API time to publish finalized candles, without polling for signals.
        tokio::time::sleep(Duration::from_secs(45)).await;
        let today=Utc::now().with_timezone(&FixedOffset::east_opt(19800).unwrap()).date_naive();
        let broker=historical::fetch_window_for(token,today,2,Interval::ThreeMinute).await?;
        let mut matches=0usize;
        let mut mismatches=Vec::new();
        for observed in &bars {
            let found=broker.iter().find(|c| c.time().ok().map(|v|v.timestamp())==observed.time().ok().map(|v|v.timestamp()));
            match found {
                None=>mismatches.push(serde_json::json!({"start":observed.timestamp,"reason":"broker candle not published"})),
                Some(b)=>{
                    let close_ok=(b.close-observed.close).abs()<0.011;
                    let open_ok=(b.open-observed.open).abs()<0.011;
                    let high_ok=(b.high-observed.high).abs()<0.011;
                    let low_ok=(b.low-observed.low).abs()<0.011;
                    let volume_ok=b.volume==observed.volume;
                    if close_ok && open_ok && high_ok && low_ok && volume_ok {matches+=1;} else {
                        mismatches.push(serde_json::json!({"start":observed.timestamp,"actual":observed,"broker":b,"close_ok":close_ok,"open_ok":open_ok,"high_ok":high_ok,"low_ok":low_ok,"volume_ok":volume_ok}));
                    }
                }
            }
        }
        println!("{}",serde_json::json!({"event":"websocket_historical_comparison","observed_bars":bars.len(),"exact_ohlcv_matches":matches,"mismatches":mismatches,"orders_submitted":0,"execution_client_loaded":false}));
        ensure!(mismatches.is_empty(),"WebSocket vs historical OHLCV mismatch: validation FAILED");
        Ok(())
    })
}
