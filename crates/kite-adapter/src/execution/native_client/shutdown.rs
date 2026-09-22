//! Stop admission before draining; bound the entire drain and abort overdue tasks.
use anyhow::{Result, anyhow};
use std::time::Duration;
use tokio::{
    task::JoinHandle,
    time::{Instant, timeout_at},
};
pub(crate) async fn drain(tasks: Vec<JoinHandle<Result<()>>>, limit: Duration) -> Result<()> {
    let deadline = Instant::now() + limit;
    let mut first_failure: Option<anyhow::Error> = None;
    for mut task in tasks {
        match timeout_at(deadline, &mut task).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(error))) => {
                if first_failure.is_none() {
                    first_failure = Some(error);
                }
            }
            Ok(Err(_)) => {
                if first_failure.is_none() {
                    first_failure =
                        Some(anyhow!("Native task join failed; recovery review required"));
                }
            }
            Err(_) => {
                task.abort();
                let _ = task.await;
                if first_failure.is_none() {
                    first_failure = Some(anyhow!(
                        "Native task drain timed out; recovery review required"
                    ));
                }
            }
        }
    }
    match first_failure {
        Some(error) => Err(anyhow!(
            "Native task drain failed; recovery review required: {error:#}"
        )),
        None => Ok(()),
    }
}
