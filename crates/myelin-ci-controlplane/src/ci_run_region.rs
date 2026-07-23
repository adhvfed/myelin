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

/// Maximum running CI rows returned by one keyset page.
pub const MAX_ACTIVE_CI_RUN_PAGE: usize = 64;

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

/// Keyset-paged recovery route for already-started CI workflows.
pub const DISCOVER_ACTIVE_CI_RUNS_QUERY: &str = "\
SELECT tenant_id,
       run_id,
       partition,
       to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"')
  FROM workflow_run
 WHERE region = $1
   AND wf_type = 'ci.pipeline'
   AND state IN ('running', 'waiting')
   AND (
     $2::timestamptz IS NULL
     OR (created_at, tenant_id, run_id) > ($2::timestamptz, $3::text, $4::text)
   )
 ORDER BY created_at, tenant_id, run_id
 LIMIT $5";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiActiveRunCursor {
    pub created_at: String,
    pub tenant_id: String,
    pub run_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiActiveRunRoute {
    pub tenant: TenantId,
    pub wf_run_id: String,
    pub partition: i16,
    pub cursor: CiActiveRunCursor,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiActiveRunPage {
    pub routes: Vec<CiActiveRunRoute>,
    pub next_cursor: Option<CiActiveRunCursor>,
}

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

    /// Return one bounded keyset page of active workflow routes for restart-safe worker fan-out.
    pub async fn active_run_page(
        &self,
        region: &str,
        after: Option<&CiActiveRunCursor>,
        limit: usize,
    ) -> Result<CiActiveRunPage, JobQueueStoreError> {
        if !(1..=MAX_ACTIVE_CI_RUN_PAGE).contains(&limit) {
            return Err(JobQueueStoreError::InvalidInput(
                "active CI run discovery page bound is invalid".into(),
            ));
        }
        let region_scope = region.to_owned();
        let region_query = region_scope.clone();
        let after_created_at = after.map(|cursor| cursor.created_at.clone());
        let after_tenant_id = after.map(|cursor| cursor.tenant_id.clone());
        let after_run_id = after.map(|cursor| cursor.run_id.clone());
        let rows = with_region_tx(&self.pool, &region_scope, move |conn| {
            Box::pin(async move {
                sqlx::query_as::<_, (String, String, i16, String)>(DISCOVER_ACTIVE_CI_RUNS_QUERY)
                    .bind(&region_query)
                    .bind(after_created_at)
                    .bind(after_tenant_id)
                    .bind(after_run_id)
                    .bind(limit as i64)
                    .fetch_all(&mut *conn)
                    .await
                    .map_err(|error| myelin_storage::PgError::Query(error.to_string()))
            })
        })
        .await?;
        let routes = rows
            .into_iter()
            .map(
                |(tenant, wf_run_id, partition, created_at)| CiActiveRunRoute {
                    cursor: CiActiveRunCursor {
                        created_at,
                        tenant_id: tenant.clone(),
                        run_id: wf_run_id.clone(),
                    },
                    tenant: TenantId(tenant),
                    wf_run_id,
                    partition,
                },
            )
            .collect::<Vec<_>>();
        let next_cursor = routes.last().map(|route| route.cursor.clone());
        Ok(CiActiveRunPage {
            routes,
            next_cursor,
        })
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

    #[test]
    fn active_discovery_is_keyset_bounded_and_column_minimal() {
        for required in [
            "SELECT tenant_id",
            "run_id",
            "partition",
            "WHERE region = $1",
            "wf_type = 'ci.pipeline'",
            "state IN ('running', 'waiting')",
            "(created_at, tenant_id, run_id) >",
            "ORDER BY created_at, tenant_id, run_id",
            "LIMIT $5",
        ] {
            assert!(DISCOVER_ACTIVE_CI_RUNS_QUERY.contains(required));
        }
        assert!(!DISCOVER_ACTIVE_CI_RUNS_QUERY.contains("OFFSET"));
        assert!(!DISCOVER_ACTIVE_CI_RUNS_QUERY.contains("ci_run"));
    }

    #[tokio::test]
    async fn active_discovery_rejects_unbounded_pages_before_database_access() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
            .unwrap();
        let discovery = CiRegionRunDiscovery::with_pg(pool);
        for limit in [0, MAX_ACTIVE_CI_RUN_PAGE + 1] {
            assert!(matches!(
                discovery.active_run_page("fr-par", None, limit).await,
                Err(JobQueueStoreError::InvalidInput(_))
            ));
        }
    }
}
