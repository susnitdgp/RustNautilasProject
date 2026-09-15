use super::{
    broker::{Broker, Snapshot},
    mock::MockBroker,
    outage::{self, ReadFailure},
    shutdown,
};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
struct Failing {
    calls: AtomicUsize,
    failures: usize,
    permanent: bool,
}
#[async_trait]
impl Broker for Failing {
    async fn verify(&self) -> Result<()> {
        Ok(())
    }
    async fn snapshot(&self) -> Result<Snapshot> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if n < self.failures {
            return Err(if self.permanent {
                anyhow!(ReadFailure::SessionExpired)
            } else {
                anyhow!(ReadFailure::Transient)
            });
        }
        MockBroker::new(144870151, "NRML").snapshot().await
    }
}
#[tokio::test]
async fn transient_reads_retry_but_permanent_errors_do_not_and_retry_budget_is_bounded() {
    let b = Failing {
        calls: AtomicUsize::new(0),
        failures: 2,
        permanent: false,
    };
    outage::snapshot(&b).await.unwrap();
    assert_eq!(b.calls.load(Ordering::SeqCst), 3);
    let b = Failing {
        calls: AtomicUsize::new(0),
        failures: 99,
        permanent: false,
    };
    assert!(outage::snapshot(&b).await.is_err());
    assert_eq!(b.calls.load(Ordering::SeqCst), 3);
    let b = Failing {
        calls: AtomicUsize::new(0),
        failures: 99,
        permanent: true,
    };
    assert!(outage::snapshot(&b).await.is_err());
    assert_eq!(b.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn shutdown_drains_finished_tasks_and_aborts_overdue_tasks_without_detaching() {
    shutdown::drain(vec![tokio::spawn(async { Ok(()) })], Duration::from_secs(1))
        .await
        .unwrap();
    let dropped = std::sync::Arc::new(AtomicUsize::new(0));
    struct Guard(std::sync::Arc<AtomicUsize>);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let flag = dropped.clone();
    let task = tokio::spawn(async move {
        let _g = Guard(flag);
        std::future::pending::<()>().await;
        Ok(())
    });
    tokio::task::yield_now().await;
    assert!(
        shutdown::drain(vec![task], Duration::from_millis(20))
            .await
            .is_err()
    );
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}
