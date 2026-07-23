//! Shutdown-aware routing from region-wide queued-run discovery to exact-tenant CI starters.
//!
//! Discovery returns only the authoritative tenant id. Each pass then asks
//! [`crate::PgCiRunStarterFactory`] for a fresh starter bound to that tenant and the factory's one
//! region. The discovery read is intentionally not a claim: the exact queued-row lock in
//! [`crate::PgCiPipelineStarter::run_once`] remains the single-winner authority when pollers race.
//! Batches are bounded so a permanently busy CI lane cannot starve lifecycle work or shutdown.
//!
//! @residency-cell-pinned:file — every discovery binds the exact region captured by the validated
//! starter factory; each routed starter is constructed with that same [`myelin_tenancy::Region`].

use std::time::Duration;

use crate::{
    CiRegionRunDiscovery, CiWorkflowDefinitionPin, JobQueueStoreError, PgCiRunStarterFactory,
    PgCiStarterError, StartQueuedOutcome,
};

/// Maximum queued runs one poller tick may start before yielding to lifecycle control.
pub const MAX_CI_RUN_START_BATCH: usize = 64;

/// Result of one bounded starter-poller pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CiRunStarterBatch {
    pub started: usize,
    pub saturated: bool,
}

/// Fail-closed starter-poller errors. No variant contains database credentials.
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

/// Region-wide discovery plus exact-tenant starter routing.
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

    /// Discover one queued tenant and drive its exact-cell starter once.
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

    /// Drive until discovery is idle or the explicit fairness bound is reached.
    pub async fn run_until_idle(
        &self,
        max_starts: usize,
    ) -> Result<CiRunStarterBatch, CiRunStarterPollerError> {
        if !(1..=MAX_CI_RUN_START_BATCH).contains(&max_starts) {
            return Err(CiRunStarterPollerError::InvalidConfig);
        }
        let mut started = 0;
        let mut processed = 0;
        while processed < max_starts {
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
                // A stale row was durably consumed, so the bounded loop made progress even though
                // no workflow started. Do not count it as a start, but continue discovery.
                StartQueuedOutcome::Superseded { .. } => processed += 1,
            }
        }
        Ok(CiRunStarterBatch {
            started,
            saturated: true,
        })
    }

    /// Run bounded passes until explicit shutdown or sender closure.
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
            self.run_until_idle(max_starts).await?;
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
