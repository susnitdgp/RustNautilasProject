//! Process signals stop the native node so clients can drain and report recovery status.
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
pub async fn wait(done: Arc<AtomicBool>, seconds: u64) {
    let completion = async {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds + 60);
        while !done.load(Ordering::Acquire) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    #[cfg(unix)]
    {
        let signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate());
        if let Ok(mut terminate) = signal {
            tokio::select! {_=completion=>{},_=tokio::signal::ctrl_c()=>{},_=terminate.recv()=>{}};
            return;
        }
    }
    tokio::select! {_=completion=>{},_=tokio::signal::ctrl_c()=>{}}
}
