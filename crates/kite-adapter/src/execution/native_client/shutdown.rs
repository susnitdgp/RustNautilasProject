//! Stop admission before draining; bound the entire drain and abort overdue tasks.
use anyhow::{Result, anyhow};
use std::time::Duration;
use tokio::{
    task::JoinHandle,
    time::{Instant, timeout_at},
};
pub(crate) async fn drain(tasks: Vec<JoinHandle<Result<()>>>, limit: Duration) -> Result<()> {
    let deadline = Instant::now() + limit;
    let mut failed = false;
    for mut task in tasks {
        match timeout_at(deadline, &mut task).await {
            Ok(Ok(Ok(()))) => {}
            Ok(_) => failed = true,
            Err(_) => {
                task.abort();
                let _ = task.await;
                failed = true;
            }
        }
    }
    if failed {
        Err(anyhow!(
            "Native task drain failed or timed out; recovery review required"
        ))
    } else {
        Ok(())
    }
}
