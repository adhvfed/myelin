//! Region-scoped discovery of queued CI runs for the production starter lane.
//!
//! A control-plane cell must discover which tenant owns the next queued run before it can construct
//! a tenant-bound [`crate::PgCiPipelineStarter`]. This is deliberately a tiny capability over the
//! constrained scheduler pool: it returns only an authoritative [`TenantId`], cannot read run
//! payloads, and runs under the same server-mapped region plus empty-tenant RLS boundary as queue
//! claim/reap.
//!
//! This file is a named `tenant-predicate` exclusion because the query is cross-tenant by design.
//! The database role and RLS restrict it to one server-owned region; the selected tenant is then the
//! input to the ordinary tenant-scoped starter.
//!
//! @residency-cell-pinned:file — every query binds the requested region inside the transaction and
//! RLS also requires it to equal `myelin_ci_scheduler_region()`.

use myelin_tenancy::TenantId;
use sqlx::postgres::PgPool;

use crate::job_queue_region::with_region_tx;
use crate::job_queue_store::JobQueueStoreError;

/// Bounded discovery: choose the tenant owning the globally oldest queued run in this region. The
/// stable `run_id` tiebreaker is evaluated in PostgreSQL but not returned, so the scheduler receives
/// no run metadata or customer-authored payload.
pub const DISCOVER_QUEUED_CI_RUN_TENANT_QUERY: &str = "\
SELECT tenant_id
  FROM ci_run
 WHERE region = $1
   AND state = 'queued'
 ORDER BY created_at, run_id
 LIMIT 1";

/// Column-minimal, region-wide queued-run discovery capability.
#[derive(Clone)]
pub struct CiRegionRunDiscovery {
    pool: PgPool,
}

impl CiRegionRunDiscovery {
    pub(crate) fn with_pg(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Return the authoritative tenant owning the oldest queued run in `region`.
    pub async fn next_queued_tenant(
        &self,
        region: &str,
    ) -> Result<Option<TenantId>, JobQueueStoreError> {
        let region_scope = region.to_owned();
        let region_query = region_scope.clone();
        let tenant = with_region_tx(&self.pool, &region_scope, move |conn| {
            Box::pin(async move {
                sqlx::query_scalar::<_, String>(DISCOVER_QUEUED_CI_RUN_TENANT_QUERY)
                    .bind(&region_query)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|error| myelin_storage::PgError::Query(error.to_string()))
            })
        })
        .await?;
        Ok(tenant.map(TenantId))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_is_region_queued_oldest_first_and_not_a_decorative_claim() {
        for required in [
            "SELECT tenant_id",
            "WHERE region = $1",
            "state = 'queued'",
            "ORDER BY created_at, run_id",
            "LIMIT 1",
        ] {
            assert!(
                DISCOVER_QUEUED_CI_RUN_TENANT_QUERY.contains(required),
                "discovery query pins `{required}`"
            );
        }
        assert!(
            !DISCOVER_QUEUED_CI_RUN_TENANT_QUERY.contains("SKIP LOCKED"),
            "discovery ends before the tenant starter transaction; exclusion belongs to the \
             starter's exact queued-row lock, not a lock released by this read"
        );
        assert_eq!(
            DISCOVER_QUEUED_CI_RUN_TENANT_QUERY
                .matches("SELECT")
                .count(),
            1,
            "the scheduler receives only the tenant id, never a run payload"
        );
    }
}
