use std::time::Duration;

use crate::{
    CiRegionRunDiscovery, CiWorkflowDefinitionPin, JobQueueStoreError, PgCiRunStarterFactory,
    PgCiStarterError, StartQueuedOutcome,
};

pub const MAX_CI_RUN_START_BATCH: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CiRunStarterBatch {
    pub started: usize,
    pub saturated: bool,
}

#[derive(Debug)]
pub enum CiRunStarterPollerError {
    InvalidConfig,
    Discovery(JobQueueStoreError),
    Starter(PgCiStarterError),
}

impl std::fmt::Display for CiRunStarterPollerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfig => f.write_str(
                "CI run starter poll interval and batch bound must be positive and bounded",
            ),
            Self::Discovery(error) => write!(f, "queued CI run discovery failed: {error}"),
            Self::Starter(error) => write!(f, "queued CI run start failed: {error}"),
        }
    }
}

impl std::error::Error for CiRunStarterPollerError {}

#[derive(Clone)]
pub struct PgCiRunStarterPoller {
    discovery: CiRegionRunDiscovery,
    starters: PgCiRunStarterFactory,
    definition: CiWorkflowDefinitionPin,
}

impl PgCiRunStarterPoller {
    pub fn new(
        discovery: CiRegionRunDiscovery,
        starters: PgCiRunStarterFactory,
        definition: CiWorkflowDefinitionPin,
    ) -> Self {
        Self {
            discovery,
            starters,
            definition,
        }
    }

    pub async fn run_once(&self) -> Result<StartQueuedOutcome, CiRunStarterPollerError> {
        let Some(tenant) = self
            .discovery
            .next_queued_tenant(&self.starters.region().0)
            .await
            .map_err(CiRunStarterPollerError::Discovery)?
        else {
            return Ok(StartQueuedOutcome::Idle);
        };
        self.starters
            .starter_for(tenant, self.definition.clone())
            .map_err(CiRunStarterPollerError::Starter)?
            .run_once()
            .await
            .map_err(CiRunStarterPollerError::Starter)
    }

    pub async fn run_until_idle(
        &self,
        max_starts: usize,
    ) -> Result<CiRunStarterBatch, CiRunStarterPollerError> {
        self.run_until_idle_inner(max_starts, None).await
    }

    async fn run_until_idle_or_shutdown(
        &self,
        max_starts: usize,
        shutdown: &tokio::sync::watch::Receiver<bool>,
    ) -> Result<CiRunStarterBatch, CiRunStarterPollerError> {
        self.run_until_idle_inner(max_starts, Some(shutdown)).await
    }

    async fn run_until_idle_inner(
        &self,
        max_starts: usize,
        shutdown: Option<&tokio::sync::watch::Receiver<bool>>,
    ) -> Result<CiRunStarterBatch, CiRunStarterPollerError> {
        if !(1..=MAX_CI_RUN_START_BATCH).contains(&max_starts) {
            return Err(CiRunStarterPollerError::InvalidConfig);
        }
        let mut started = 0;
        let mut processed = 0;
        while processed < max_starts {
            if shutdown.is_some_and(|receiver| *receiver.borrow()) {
                return Ok(CiRunStarterBatch {
                    started,
                    saturated: true,
                });
            }
            match self.run_once().await? {
                StartQueuedOutcome::Idle => {
                    return Ok(CiRunStarterBatch {
                        started,
                        saturated: false,
                    });
                }
                StartQueuedOutcome::Started { .. } => {
                    started += 1;
                    processed += 1;
                }
                StartQueuedOutcome::Superseded { .. } => processed += 1,
            }
        }
        Ok(CiRunStarterBatch {
            started,
            saturated: true,
        })
    }

    pub async fn run_until_shutdown(
        &self,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
        poll_interval: Duration,
        max_starts: usize,
    ) -> Result<(), CiRunStarterPollerError> {
        if poll_interval.is_zero() || !(1..=MAX_CI_RUN_START_BATCH).contains(&max_starts) {
            return Err(CiRunStarterPollerError::InvalidConfig);
        }
        loop {
            if *shutdown.borrow() {
                return Ok(());
            }
            self.run_until_idle_or_shutdown(max_starts, &shutdown)
                .await?;
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return Ok(());
                    }
                }
                _ = tokio::time::sleep(poll_interval) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use myelin_events::MonotonicMinter;
    use myelin_storage::FsBlobStore;
    use myelin_tenancy::Region;
    use sqlx::postgres::PgPoolOptions;

    use super::*;

    fn dormant_poller() -> PgCiRunStarterPoller {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
            .expect("syntactically valid lazy pool");
        let discovery = CiRegionRunDiscovery::with_pg(pool.clone());
        let starters = PgCiRunStarterFactory::new(
            pool,
            tokio::runtime::Handle::current(),
            Arc::new(MonotonicMinter::new()),
            Region("fr-par".into()),
            Arc::new(FsBlobStore::new()),
        );
        PgCiRunStarterPoller::new(
            discovery,
            starters,
            CiWorkflowDefinitionPin::new(1, "blake3:ci-body-v1").unwrap(),
        )
    }

    #[tokio::test]
    async fn shutdown_is_observed_before_any_database_read() {
        let poller = dormant_poller();
        let (_shutdown, receiver) = tokio::sync::watch::channel(true);
        poller
            .run_until_shutdown(receiver, Duration::from_millis(1), 1)
            .await
            .expect("pre-signalled shutdown is clean without connecting");
    }

    #[tokio::test]
    async fn runtime_bounds_are_rejected_before_any_database_read() {
        let poller = dormant_poller();
        for (interval, batch) in [
            (Duration::ZERO, 1),
            (Duration::from_millis(1), 0),
            (Duration::from_millis(1), MAX_CI_RUN_START_BATCH + 1),
        ] {
            let (_shutdown, receiver) = tokio::sync::watch::channel(true);
            assert!(matches!(
                poller.run_until_shutdown(receiver, interval, batch).await,
                Err(CiRunStarterPollerError::InvalidConfig)
            ));
        }
        assert!(matches!(
            poller.run_until_idle(0).await,
            Err(CiRunStarterPollerError::InvalidConfig)
        ));
    }
}
