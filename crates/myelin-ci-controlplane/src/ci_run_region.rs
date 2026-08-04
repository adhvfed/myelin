use myelin_tenancy::TenantId;
use sqlx::postgres::PgPool;

use crate::job_queue_region::with_region_tx;
use crate::job_queue_store::JobQueueStoreError;

pub const MAX_ACTIVE_CI_RUN_PAGE: usize = 64;

pub const DISCOVER_QUEUED_CI_RUN_TENANT_QUERY: &str = "\
SELECT tenant_id
  FROM ci_run
 WHERE region = $1
   AND state = 'queued'
 ORDER BY created_at, run_id
 LIMIT 1";

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

pub const DISCOVER_SUPERSEDED_CI_PIPELINE_RUNS_QUERY: &str = "\
SELECT tenant_id, run_id
  FROM workflow_run
 WHERE region = $1
   AND wf_type = 'ci.pipeline'
   AND wf_version = $2
   AND state IN ('running', 'waiting')
 ORDER BY tenant_id, run_id
 LIMIT $3";

pub const MAX_SUPERSEDED_CI_PIPELINE_RUN_REPORT: usize = 16;

pub const MAX_SUPERSEDED_CI_PIPELINE_RUN_PROBE: usize = MAX_SUPERSEDED_CI_PIPELINE_RUN_REPORT + 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SupersededCiPipelineRun {
    pub tenant: TenantId,
    pub wf_run_id: String,
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

#[derive(Clone)]
pub struct CiRegionRunDiscovery {
    pool: PgPool,
}

impl CiRegionRunDiscovery {
    pub(crate) fn with_pg(pool: PgPool) -> Self {
        Self { pool }
    }

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

    pub async fn superseded_definition_runs(
        &self,
        region: &str,
        version: i32,
        limit: usize,
    ) -> Result<Vec<SupersededCiPipelineRun>, JobQueueStoreError> {
        if !(1..=MAX_SUPERSEDED_CI_PIPELINE_RUN_PROBE).contains(&limit) {
            return Err(JobQueueStoreError::InvalidInput(
                "superseded-definition discovery page bound is invalid".into(),
            ));
        }
        let region_scope = region.to_owned();
        let region_query = region_scope.clone();
        let rows = with_region_tx(&self.pool, &region_scope, move |conn| {
            Box::pin(async move {
                sqlx::query_as::<_, (String, String)>(DISCOVER_SUPERSEDED_CI_PIPELINE_RUNS_QUERY)
                    .bind(&region_query)
                    .bind(version)
                    .bind(limit as i64)
                    .fetch_all(&mut *conn)
                    .await
                    .map_err(|error| myelin_storage::PgError::Query(error.to_string()))
            })
        })
        .await?;
        Ok(rows
            .into_iter()
            .map(|(tenant, wf_run_id)| SupersededCiPipelineRun {
                tenant: TenantId(tenant),
                wf_run_id,
            })
            .collect())
    }

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
    fn the_superseded_run_predicate_matches_the_partial_index_byte_for_byte() {
        let index_predicate = crate::migrations::CREATE_CI_WORKFLOW_ACTIVE_REGION_INDEX_DDL
            .split("WHERE ")
            .nth(1)
            .expect("the active-region index is partial");
        assert_eq!(
            index_predicate,
            "wf_type = 'ci.pipeline' AND state IN ('running', 'waiting')"
        );
        for clause in ["wf_type = 'ci.pipeline'", "state IN ('running', 'waiting')"] {
            assert!(
                DISCOVER_SUPERSEDED_CI_PIPELINE_RUNS_QUERY.contains(clause),
                "the diagnostic must restate the index predicate clause `{clause}` verbatim"
            );
        }
        assert!(
            !DISCOVER_SUPERSEDED_CI_PIPELINE_RUNS_QUERY.contains("NOT IN"),
            "the negative terminal-state form is not index-eligible and must never come back"
        );
        for terminal in ["completed", "failed", "terminated", "nondeterministic"] {
            assert!(myelin_flow::run_state::is_terminal(terminal));
        }
        for live in ["running", "waiting"] {
            assert!(!myelin_flow::run_state::is_terminal(live));
        }
    }

    #[test]
    fn the_probe_ceiling_is_one_over_the_report_bound() {
        assert_eq!(MAX_SUPERSEDED_CI_PIPELINE_RUN_REPORT, 16);
        assert_eq!(
            MAX_SUPERSEDED_CI_PIPELINE_RUN_PROBE,
            MAX_SUPERSEDED_CI_PIPELINE_RUN_REPORT + 1
        );
    }

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
