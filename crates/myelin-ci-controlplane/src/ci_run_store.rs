use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use myelin_ci_sandbox::ResourceUsage;
use myelin_events::OutboxRow;
use myelin_storage::pgrelay::PgRelay;
use myelin_storage::{
    with_tenant_tx, with_tenant_tx_error, DurableCostLedger, MicroUsd, PgError, RunId as CostRunId,
    TenantScope,
};
use sqlx::postgres::PgPool;
use sqlx::types::Uuid;
use sqlx::Row;

use crate::ci_drive_manifest::CiDriveManifestStore;
use crate::job_accounting_store::{
    versioned_accounting_receipt, CiJobAccountingRecord, CiJobAccountingStore,
    CiJobAccountingWriteVersion, CiJobTerminalDisposition,
};

const PR_RUN_SUPERSESSION_LOCK_DOMAIN: &str = "myelin.ci.pr-run-supersession.v1";
const CI_RUN_IDEMPOTENCY_LOCK_DOMAIN: &str = "myelin.ci.run-idempotency.v1";

const LOCK_CI_RUN_IDEMPOTENCY_QUERY: &str = "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiRunInsert {
    pub tenant_id: String,
    pub region: String,
    pub run_id: String,
    pub project_id: String,
    pub pipeline_id: String,
    pub wf_run_id: String,
    pub definition_snapshot: String,
    pub trigger_kind: String,
    pub concurrency_group: Option<String>,
    pub pr_head_generation: Option<i64>,
    pub trust_tier: String,
    pub state: String,
    pub correlation_id: String,
    pub cause_event_id: Option<String>,
    pub cause_depth: i64,
    pub caused_by: Option<String>,
    pub repo_ref: Option<String>,
    pub source_ref: Option<String>,
    pub commit_oid: Option<String>,
    pub triggered_by: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiRunRecord {
    pub tenant_id: String,
    pub run_id: String,
    pub region: String,
    pub project_id: String,
    pub pipeline_id: String,
    pub wf_run_id: String,
    pub repo_ref: Option<String>,
    pub source_ref: Option<String>,
    pub commit_oid: Option<String>,
    pub cause_event_id: Option<String>,
    pub cause_depth: i64,
    pub caused_by: Option<String>,
    pub definition_snapshot: String,
    pub trigger_kind: String,
    pub concurrency_group: Option<String>,
    pub pr_head_generation: Option<i64>,
    pub trust_tier: String,
    pub state: String,
    pub correlation_id: String,
}

pub const INSERT_CI_RUN_QUERY: &str = "\
INSERT INTO ci_run (
  tenant_id, region, run_id, project_id, pipeline_id, wf_run_id,
  repo_ref, source_ref, commit_oid, cause_event_id, cause_depth, caused_by, definition_snapshot,
  trigger_kind, concurrency_group, pr_head_generation, triggered_by, trust_tier, state, correlation_id
) VALUES (
  $1, $2, $3::uuid, $4::uuid, $5::uuid, $6::uuid,
  $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20
)
ON CONFLICT (tenant_id, run_id) DO NOTHING
RETURNING run_id";

pub const VERIFY_CI_RUN_REPLAY_QUERY: &str = "\
SELECT
  region = $2                                      AS region_matches,
  project_id = $4::uuid                            AS project_id_matches,
  pipeline_id = $5::uuid                           AS pipeline_id_matches,
  wf_run_id = $6::uuid                             AS wf_run_id_matches,
  repo_ref IS NOT DISTINCT FROM $7::text           AS repo_ref_matches,
  source_ref IS NOT DISTINCT FROM $8::text          AS source_ref_matches,
  commit_oid IS NOT DISTINCT FROM $9::text          AS commit_oid_matches,
  cause_event_id IS NOT DISTINCT FROM $10::text     AS cause_event_id_matches,
  cause_depth = $11                                 AS cause_depth_matches,
  caused_by IS NOT DISTINCT FROM $12::text          AS caused_by_matches,
  definition_snapshot = $13                        AS definition_snapshot_matches,
  trigger_kind = $14                               AS trigger_kind_matches,
  concurrency_group IS NOT DISTINCT FROM $15::text AS concurrency_group_matches,
  pr_head_generation IS NOT DISTINCT FROM $16::bigint AS pr_head_generation_matches,
  trust_tier = $17                                 AS trust_tier_matches,
  correlation_id = $18                             AS correlation_id_matches,
  (triggered_by IS NOT DISTINCT FROM $19::text
    OR triggered_by = $20::text)                   AS triggered_by_matches
FROM ci_run
WHERE tenant_id = $1 AND region = $2 AND run_id = $3::uuid
FOR KEY SHARE";

pub const SELECT_CI_RUN_QUERY: &str = "\
SELECT
  tenant_id              AS tenant_id,
  run_id::text            AS run_id,
  region                  AS region,
  project_id::text        AS project_id,
  pipeline_id::text       AS pipeline_id,
  wf_run_id::text         AS wf_run_id,
  repo_ref                AS repo_ref,
  source_ref              AS source_ref,
  commit_oid              AS commit_oid,
  cause_event_id          AS cause_event_id,
  cause_depth             AS cause_depth,
  caused_by               AS caused_by,
  definition_snapshot     AS definition_snapshot,
  trigger_kind            AS trigger_kind,
  concurrency_group       AS concurrency_group,
  pr_head_generation      AS pr_head_generation,
  trust_tier              AS trust_tier,
  state                   AS state,
  correlation_id          AS correlation_id
FROM ci_run WHERE tenant_id = $1 AND run_id = $2::uuid";

pub const LOCK_CI_RUN_FOR_TOKEN_MINT_QUERY: &str = "\
SELECT
  tenant_id              AS tenant_id,
  run_id::text            AS run_id,
  region                  AS region,
  project_id::text        AS project_id,
  pipeline_id::text       AS pipeline_id,
  wf_run_id::text         AS wf_run_id,
  repo_ref                AS repo_ref,
  source_ref              AS source_ref,
  commit_oid              AS commit_oid,
  cause_event_id          AS cause_event_id,
  cause_depth             AS cause_depth,
  caused_by               AS caused_by,
  definition_snapshot     AS definition_snapshot,
  trigger_kind            AS trigger_kind,
  concurrency_group       AS concurrency_group,
  pr_head_generation      AS pr_head_generation,
  trust_tier              AS trust_tier,
  state                   AS state,
  correlation_id          AS correlation_id
FROM ci_run
WHERE tenant_id = $1 AND region = $2 AND run_id = $3::uuid AND wf_run_id = $4::uuid
FOR UPDATE";

pub const LOCK_CI_RUN_FOR_FINALIZE_QUERY: &str = "\
SELECT state, cost_settled,
       to_char(finished_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"')
         AS completed_at
FROM ci_run
WHERE tenant_id = $1 AND region = $2 AND run_id = $3::uuid AND wf_run_id = $4::uuid
FOR UPDATE";

pub const SELECT_CI_RUN_ACCOUNTING_QUERY: &str = "\
SELECT job_id::text AS job_id, reserve_handle, passed, timed_out, skipped
FROM ci_job_accounting
WHERE tenant_id = $1 AND region = $2 AND ci_run_id = $3::uuid AND wf_run_id = $4::uuid
ORDER BY job_id";

const SELECT_RESERVED_CHECK_ATTEMPT_QUERY: &str = "\
SELECT repo_ref, commit_oid, run_attempt FROM ci_run_check_attempt
WHERE tenant_id=$1 AND region=$2 AND run_id=$3 AND context=$4";

pub const FINALIZE_CI_RUN_QUERY: &str = "\
UPDATE ci_run
SET state = $5, cost_settled = true, finished_at = $6::timestamptz
WHERE tenant_id = $1 AND region = $2 AND run_id = $3::uuid AND wf_run_id = $4::uuid
  AND state = 'running' AND cost_settled = false AND finished_at IS NULL
RETURNING to_char(finished_at AT TIME ZONE 'UTC',
                  'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS completed_at";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiRunFinalizationJob {
    pub job_id: String,
    pub reserve_handle: String,
    pub flow_timed_out: bool,
    pub dispatched: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiRunTerminalState {
    Succeeded,
    Failed,
    TimedOut,
}

impl CiRunTerminalState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiRunFinalization {
    pub tenant_id: String,
    pub region: String,
    pub run_id: String,
    pub wf_run_id: String,
    pub terminal_state: CiRunTerminalState,
    pub completed_at: String,
    pub jobs: Vec<CiRunFinalizationJob>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiRunFinalizationWrite {
    Finalized,
    ExactReplay,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiRunFinalizationOutcome {
    pub write: CiRunFinalizationWrite,
    pub completed_at: String,
}

pub trait CiRunFinalizer: Send + Sync {
    fn finalize(
        &self,
        finalization: &CiRunFinalization,
    ) -> Result<CiRunFinalizationOutcome, CiRunStoreError>;
}

#[non_exhaustive]
#[derive(Debug, PartialEq, Eq)]
pub enum CiRunStoreError {
    Db(String),
    InvalidInitialState,
    InvalidCausalDepth,
    InvalidConcurrencyGroup,
    InvalidPrHeadGeneration,
    ReplayCollision { differing_fields: Vec<&'static str> },
    ConflictNotVisible,
    NoCoCommitTx,
    InvalidFinalization(&'static str),
    FinalizationRunNotFound,
    IncompleteTerminalAccounting,
    TerminalAccountingDivergence,
    TerminalVerdictDivergence,
    FinalizationStateDivergence,
    SkippedJobAccounting,
    FinalizationManifestDivergence,
}

impl core::fmt::Display for CiRunStoreError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CiRunStoreError::Db(e) => write!(f, "durable ci_run store error: {e}"),
            CiRunStoreError::InvalidInitialState => {
                write!(f, "durable ci_run insert requires the queued initial state")
            }
            CiRunStoreError::InvalidCausalDepth => write!(
                f,
                "durable ci_run insert requires cause_depth in the canonical u32 range"
            ),
            CiRunStoreError::InvalidConcurrencyGroup => write!(
                f,
                "durable ci_run insert requires a canonical PR concurrency group only for pull-request triggers"
            ),
            CiRunStoreError::InvalidPrHeadGeneration => write!(
                f,
                "durable ci_run insert requires a positive producer-authored head generation only for pull-request triggers"
            ),
            CiRunStoreError::ReplayCollision { differing_fields } => write!(
                f,
                "durable ci_run replay collided on immutable fields: {}",
                differing_fields.join(", ")
            ),
            CiRunStoreError::ConflictNotVisible => write!(
                f,
                "durable ci_run conflict could not be verified in the active tenant scope"
            ),
            CiRunStoreError::NoCoCommitTx => write!(
                f,
                "durable ci_run co-commit refused: the HandlerTx carried no co-commit connection \
                 (a durable handler fails closed rather than write the run-of-record outside the \
                 dedup mark's transaction - the #7 at-most-once floor)"
            ),
            CiRunStoreError::InvalidFinalization(field) => {
                write!(f, "durable ci_run finalization refused an invalid {field}")
            }
            CiRunStoreError::FinalizationRunNotFound => write!(
                f,
                "durable ci_run finalization could not find the exact scoped run identity"
            ),
            CiRunStoreError::IncompleteTerminalAccounting => write!(
                f,
                "durable ci_run finalization requires one accounting receipt per manifest job"
            ),
            CiRunStoreError::TerminalAccountingDivergence => write!(
                f,
                "durable ci_run finalization found divergent job accounting authority"
            ),
            CiRunStoreError::TerminalVerdictDivergence => write!(
                f,
                "durable ci_run terminal state disagrees with immutable job verdicts"
            ),
            CiRunStoreError::FinalizationStateDivergence => write!(
                f,
                "durable ci_run finalization collided with the stored lifecycle state"
            ),
            CiRunStoreError::SkippedJobAccounting => write!(
                f,
                "durable ci_run finalization could not close skipped-job accounting"
            ),
            CiRunStoreError::FinalizationManifestDivergence => write!(
                f,
                "durable ci_run finalization differs from immutable manifest authority"
            ),
        }
    }
}

impl std::error::Error for CiRunStoreError {}

impl From<PgError> for CiRunStoreError {
    fn from(e: PgError) -> Self {
        Self::Db(e.to_string())
    }
}

#[derive(Clone)]
pub struct CiRunStore {
    pool: PgPool,
    surface_cursor_key: Option<Arc<zeroize::Zeroizing<[u8; 32]>>>,
    #[cfg(any(test, feature = "integration"))]
    surface_detail_test_barrier: Option<Arc<tokio::sync::Barrier>>,
}

#[derive(Clone)]
pub struct DurableCiRunFinalizer {
    store: CiRunStore,
    ledger: DurableCostLedger,
    accounting: CiJobAccountingStore,
    manifest: CiDriveManifestStore,
    scope: TenantScope,
    rt: tokio::runtime::Handle,
}

impl DurableCiRunFinalizer {
    pub fn new(
        store: CiRunStore,
        ledger: DurableCostLedger,
        accounting: CiJobAccountingStore,
        manifest: CiDriveManifestStore,
        scope: TenantScope,
        rt: tokio::runtime::Handle,
    ) -> Self {
        Self {
            store,
            ledger,
            accounting,
            manifest,
            scope,
            rt,
        }
    }
}

impl CiRunFinalizer for DurableCiRunFinalizer {
    fn finalize(
        &self,
        finalization: &CiRunFinalization,
    ) -> Result<CiRunFinalizationOutcome, CiRunStoreError> {
        let tenant = self.scope.tenant().as_str().to_owned();
        let region = self.scope.region().as_str().to_owned();
        let store = self.store.clone();
        let ledger = self.ledger.clone();
        let accounting = self.accounting.clone();
        let manifest_store = self.manifest.clone();
        let scope = self.scope.clone();
        let finalization = finalization.clone();
        let pool = store.pool().clone();
        let future = with_tenant_tx_error(&pool, &tenant, &region, move |conn| {
            Box::pin(async move {
                let (manifest, _) = manifest_store
                    .load_by_wf_run_on_conn(conn, &finalization.wf_run_id)
                    .await
                    .map_err(|_| CiRunStoreError::FinalizationManifestDivergence)?
                    .ok_or(CiRunStoreError::FinalizationManifestDivergence)?;
                let expected: BTreeMap<&str, &str> = manifest
                    .jobs
                    .iter()
                    .map(|job| (job.job_id.as_str(), job.reserve_handle.as_str()))
                    .collect();
                if manifest.ci_run_id != finalization.run_id
                    || expected.len() != finalization.jobs.len()
                    || finalization.jobs.iter().any(|job| {
                        expected.get(job.job_id.as_str()).copied()
                            != Some(job.reserve_handle.as_str())
                    })
                {
                    return Err(CiRunStoreError::FinalizationManifestDivergence);
                }
                for job in finalization.jobs.iter().filter(|job| !job.dispatched) {
                    let refunded = ledger
                        .cancel_unstarted_in_tx(
                            conn,
                            scope.tenant(),
                            &CostRunId(job.reserve_handle.clone()),
                        )
                        .await
                        .map_err(|_| CiRunStoreError::SkippedJobAccounting)?;
                    let existing = accounting
                        .load_in_tx(conn, &scope, &job.job_id)
                        .await
                        .map_err(|_| CiRunStoreError::SkippedJobAccounting)?;
                    if let Some(existing) = existing {
                        let expected_v3 = skipped_accounting_record(
                            &scope,
                            &finalization,
                            job,
                            refunded,
                            CiJobAccountingWriteVersion::V3,
                        );
                        let expected_v4 = skipped_accounting_record(
                            &scope,
                            &finalization,
                            job,
                            refunded,
                            CiJobAccountingWriteVersion::V4,
                        );
                        let receipt_matches = match existing.disposition {
                            None => existing.completion_receipt == expected_v3.completion_receipt,
                            Some(_) => {
                                existing.disposition == expected_v4.disposition
                                    && existing.completion_receipt == expected_v4.completion_receipt
                            }
                        };
                        if !existing.skipped
                            || existing.wf_run_id != finalization.wf_run_id
                            || existing.ci_run_id != finalization.run_id
                            || existing.reserve_handle != job.reserve_handle
                            || existing.passed
                            || existing.timed_out
                            || existing.usage.cpu_seconds != 0
                            || existing.usage.mem_byte_seconds != 0
                            || existing.pricing_revision != "ci-skipped:v1"
                            || existing.billed != MicroUsd::ZERO
                            || existing.refunded != refunded
                            || !receipt_matches
                        {
                            return Err(CiRunStoreError::SkippedJobAccounting);
                        }
                        continue;
                    }
                    accounting
                        .record_in_tx(
                            conn,
                            &scope,
                            &skipped_accounting_record(
                                &scope,
                                &finalization,
                                job,
                                refunded,
                                accounting.write_version(),
                            ),
                        )
                        .await
                        .map_err(|_| CiRunStoreError::SkippedJobAccounting)?;
                }
                store
                    .finalize_ci_run_in_tx(conn, &scope, &finalization)
                    .await
            })
        });
        match tokio::runtime::Handle::try_current() {
            Ok(_) => tokio::task::block_in_place(|| self.rt.block_on(future)),
            Err(_) => self.rt.block_on(future),
        }
    }
}

impl CiRunStore {
    pub fn with_pg(pool: PgPool) -> CiRunStore {
        CiRunStore {
            pool,
            surface_cursor_key: None,
            #[cfg(any(test, feature = "integration"))]
            surface_detail_test_barrier: None,
        }
    }

    pub fn with_pg_surface_cursor_key(
        pool: PgPool,
        key: zeroize::Zeroizing<[u8; 32]>,
    ) -> CiRunStore {
        CiRunStore {
            pool,
            surface_cursor_key: Some(Arc::new(key)),
            #[cfg(any(test, feature = "integration"))]
            surface_detail_test_barrier: None,
        }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub(crate) fn surface_cursor_key(&self) -> Option<&[u8; 32]> {
        self.surface_cursor_key.as_deref().map(|key| &**key)
    }

    #[cfg(any(test, feature = "integration"))]
    pub fn with_surface_detail_test_barrier(
        mut self,
        barrier: Arc<tokio::sync::Barrier>,
    ) -> CiRunStore {
        self.surface_detail_test_barrier = Some(barrier);
        self
    }

    #[cfg(any(test, feature = "integration"))]
    pub(crate) fn surface_detail_test_barrier(&self) -> Option<Arc<tokio::sync::Barrier>> {
        self.surface_detail_test_barrier.clone()
    }

    pub fn co_commit_insert(
        &self,
        tx: &mut myelin_events::HandlerTx<'_>,
        row: &CiRunInsert,
        rt: &tokio::runtime::Handle,
    ) -> Result<bool, CiRunStoreError> {
        validate_initial_state(row)?;
        let conn = tx
            .connection::<sqlx::PgConnection>()
            .ok_or(CiRunStoreError::NoCoCommitTx)?;
        tokio::task::block_in_place(|| rt.block_on(insert_on_conn(conn, row)))
    }

    pub fn co_commit_reserve<F>(
        &self,
        tx: &mut myelin_events::HandlerTx<'_>,
        row: &CiRunInsert,
        contexts: &BTreeSet<String>,
        rt: &tokio::runtime::Handle,
        stage: F,
    ) -> Result<BTreeMap<String, u32>, CiRunStoreError>
    where
        F: FnOnce(&BTreeMap<String, u32>) -> Result<Vec<OutboxRow>, String>,
    {
        validate_initial_state(row)?;
        if contexts.is_empty() {
            return Err(CiRunStoreError::Db(
                "reserve requires at least one check context".into(),
            ));
        }
        let conn = tx
            .connection::<sqlx::PgConnection>()
            .ok_or(CiRunStoreError::NoCoCommitTx)?;
        tokio::task::block_in_place(|| {
            rt.block_on(async {
                insert_on_conn(conn, row).await?;
                let attempts = allocate_reserve_check_attempts(conn, row, contexts).await?;
                let rows = stage(&attempts).map_err(|error| {
                    CiRunStoreError::Db(format!("stage reserve events: {error}"))
                })?;
                PgRelay::co_commit_rows_in_tx(conn, &rows)
                    .await
                    .map_err(CiRunStoreError::from)?;
                Ok(attempts)
            })
        })
    }

    pub async fn insert_ci_run(&self, row: &CiRunInsert) -> Result<bool, CiRunStoreError> {
        validate_initial_state(row)?;
        let row = row.clone();
        let tenant = row.tenant_id.clone();
        let region = row.region.clone();
        with_tenant_tx_error(&self.pool, &tenant, &region, move |conn| {
            Box::pin(async move { insert_on_conn(conn, &row).await })
        })
        .await
    }

    pub async fn get_ci_run(
        &self,
        tenant_id: &str,
        region: &str,
        run_id: &str,
    ) -> Result<Option<CiRunRecord>, CiRunStoreError> {
        let tenant_id_owned = tenant_id.to_string();
        let run_owned = run_id.to_string();
        let row = with_tenant_tx(&self.pool, tenant_id, region, move |conn| {
            Box::pin(async move {
                sqlx::query(SELECT_CI_RUN_QUERY)
                    .bind(&tenant_id_owned)
                    .bind(&run_owned)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|e| PgError::Query(e.to_string()))
            })
        })
        .await
        .map_err(CiRunStoreError::from)?;

        Ok(row.map(ci_run_record_from_row))
    }

    pub(crate) async fn lock_for_token_mint_on_conn(
        connection: &mut sqlx::PgConnection,
        tenant_id: &str,
        region: &str,
        run_id: &str,
        wf_run_id: &str,
    ) -> Result<Option<CiRunRecord>, CiRunStoreError> {
        let row = sqlx::query(LOCK_CI_RUN_FOR_TOKEN_MINT_QUERY)
            .bind(tenant_id)
            .bind(region)
            .bind(run_id)
            .bind(wf_run_id)
            .fetch_optional(&mut *connection)
            .await
            .map_err(|error| CiRunStoreError::Db(format!("lock CI run for token mint: {error}")))?;
        Ok(row.map(ci_run_record_from_row))
    }

    pub async fn finalize_ci_run(
        &self,
        scope: &TenantScope,
        finalization: &CiRunFinalization,
    ) -> Result<CiRunFinalizationOutcome, CiRunStoreError> {
        validate_finalization_scope(scope, finalization)?;
        let tenant = scope.tenant().as_str().to_owned();
        let region = scope.region().as_str().to_owned();
        let store = self.clone();
        let scope = scope.clone();
        let finalization = finalization.clone();
        with_tenant_tx_error(&self.pool, &tenant, &region, move |conn| {
            Box::pin(async move {
                store
                    .finalize_ci_run_in_tx(conn, &scope, &finalization)
                    .await
            })
        })
        .await
    }

    async fn finalize_ci_run_in_tx(
        &self,
        conn: &mut sqlx::PgConnection,
        scope: &TenantScope,
        finalization: &CiRunFinalization,
    ) -> Result<CiRunFinalizationOutcome, CiRunStoreError> {
        validate_finalization_scope(scope, finalization)?;
        finalize_on_conn(conn, scope, finalization).await
    }
}

fn ci_run_record_from_row(row: sqlx::postgres::PgRow) -> CiRunRecord {
    CiRunRecord {
        tenant_id: row.get("tenant_id"),
        run_id: row.get("run_id"),
        region: row.get("region"),
        project_id: row.get("project_id"),
        pipeline_id: row.get("pipeline_id"),
        wf_run_id: row.get("wf_run_id"),
        repo_ref: row.get("repo_ref"),
        source_ref: row.get("source_ref"),
        commit_oid: row.get("commit_oid"),
        cause_event_id: row.get("cause_event_id"),
        cause_depth: row.get("cause_depth"),
        caused_by: row.get("caused_by"),
        definition_snapshot: row.get("definition_snapshot"),
        trigger_kind: row.get("trigger_kind"),
        concurrency_group: row.get("concurrency_group"),
        pr_head_generation: row.get("pr_head_generation"),
        trust_tier: row.get("trust_tier"),
        state: row.get("state"),
        correlation_id: row.get("correlation_id"),
    }
}

async fn finalize_on_conn(
    conn: &mut sqlx::PgConnection,
    scope: &TenantScope,
    finalization: &CiRunFinalization,
) -> Result<CiRunFinalizationOutcome, CiRunStoreError> {
    let locked = sqlx::query(LOCK_CI_RUN_FOR_FINALIZE_QUERY)
        .bind(scope.tenant().as_str())
        .bind(scope.region().as_str())
        .bind(&finalization.run_id)
        .bind(&finalization.wf_run_id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|_| CiRunStoreError::Db("ci_run finalization lock".into()))?
        .ok_or(CiRunStoreError::FinalizationRunNotFound)?;

    let rows = sqlx::query(SELECT_CI_RUN_ACCOUNTING_QUERY)
        .bind(scope.tenant().as_str())
        .bind(scope.region().as_str())
        .bind(&finalization.run_id)
        .bind(&finalization.wf_run_id)
        .fetch_all(&mut *conn)
        .await
        .map_err(|_| CiRunStoreError::Db("ci_run accounting verification".into()))?;
    if rows.len() != finalization.jobs.len() {
        return Err(CiRunStoreError::IncompleteTerminalAccounting);
    }

    let expected: BTreeMap<&str, &CiRunFinalizationJob> = finalization
        .jobs
        .iter()
        .map(|job| (job.job_id.as_str(), job))
        .collect();
    let mut all_passed = true;
    let mut any_timed_out = false;
    for row in &rows {
        let job_id: String = row.get("job_id");
        let reserve_handle: String = row.get("reserve_handle");
        let Some(job) = expected.get(job_id.as_str()).copied() else {
            return Err(CiRunStoreError::TerminalAccountingDivergence);
        };
        if job.reserve_handle != reserve_handle {
            return Err(CiRunStoreError::TerminalAccountingDivergence);
        }
        if row.get::<bool, _>("skipped") == job.dispatched {
            return Err(CiRunStoreError::TerminalAccountingDivergence);
        }
        all_passed &= row.get::<bool, _>("passed");
        any_timed_out |= job.flow_timed_out || row.get::<bool, _>("timed_out");
    }
    let derived = if any_timed_out {
        CiRunTerminalState::TimedOut
    } else if all_passed {
        CiRunTerminalState::Succeeded
    } else {
        CiRunTerminalState::Failed
    };
    if derived != finalization.terminal_state {
        return Err(CiRunStoreError::TerminalVerdictDivergence);
    }

    let state: String = locked.get("state");
    let cost_settled: bool = locked.get("cost_settled");
    let stored_completed_at: Option<String> = locked.get("completed_at");
    if state == finalization.terminal_state.as_str() && cost_settled {
        return Ok(CiRunFinalizationOutcome {
            write: CiRunFinalizationWrite::ExactReplay,
            completed_at: stored_completed_at
                .ok_or(CiRunStoreError::FinalizationStateDivergence)?,
        });
    }
    if state != "running" || cost_settled || stored_completed_at.is_some() {
        return Err(CiRunStoreError::FinalizationStateDivergence);
    }

    let updated = sqlx::query(FINALIZE_CI_RUN_QUERY)
        .bind(scope.tenant().as_str())
        .bind(scope.region().as_str())
        .bind(&finalization.run_id)
        .bind(&finalization.wf_run_id)
        .bind(finalization.terminal_state.as_str())
        .bind(&finalization.completed_at)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|_| CiRunStoreError::Db("ci_run terminal transition".into()))?;
    let updated = updated.ok_or(CiRunStoreError::FinalizationStateDivergence)?;
    Ok(CiRunFinalizationOutcome {
        write: CiRunFinalizationWrite::Finalized,
        completed_at: updated.get("completed_at"),
    })
}

fn skipped_accounting_record(
    scope: &TenantScope,
    finalization: &CiRunFinalization,
    job: &CiRunFinalizationJob,
    refunded: MicroUsd,
    write_version: CiJobAccountingWriteVersion,
) -> CiJobAccountingRecord {
    let disposition = CiJobTerminalDisposition::SkippedBeforeStart;
    let legacy_completion_receipt_v3 = skipped_completion_receipt(scope, finalization, job);
    let receipt =
        versioned_accounting_receipt(write_version, legacy_completion_receipt_v3, disposition);
    CiJobAccountingRecord {
        tenant: scope.tenant().clone(),
        job_id: job.job_id.clone(),
        wf_run_id: finalization.wf_run_id.clone(),
        ci_run_id: finalization.run_id.clone(),
        reserve_handle: job.reserve_handle.clone(),
        passed: false,
        timed_out: false,
        skipped: true,
        usage: ResourceUsage {
            cpu_seconds: 0,
            mem_byte_seconds: 0,
        },
        pricing_revision: "ci-skipped:v1".into(),
        billed: MicroUsd::ZERO,
        refunded,
        disposition: receipt.disposition,
        completion_receipt: receipt.completion_receipt,
        legacy_completion_receipt_v3: receipt.legacy_completion_receipt_v3,
    }
}

fn skipped_completion_receipt(
    scope: &TenantScope,
    finalization: &CiRunFinalization,
    job: &CiRunFinalizationJob,
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"myelin.ci.skipped-accounting.v1\0");
    for field in [
        scope.tenant().as_str(),
        scope.region().as_str(),
        finalization.run_id.as_str(),
        finalization.wf_run_id.as_str(),
        job.job_id.as_str(),
        job.reserve_handle.as_str(),
    ] {
        hasher.update(&(field.len() as u64).to_be_bytes());
        hasher.update(field.as_bytes());
    }
    format!("v3:{}", hasher.finalize().to_hex())
}

fn validate_finalization(finalization: &CiRunFinalization) -> Result<(), CiRunStoreError> {
    if finalization.tenant_id.is_empty()
        || finalization.tenant_id.len() > 512
        || finalization.region.is_empty()
        || finalization.region.len() > 512
    {
        return Err(CiRunStoreError::InvalidFinalization(
            "tenant or region scope",
        ));
    }
    Uuid::parse_str(&finalization.run_id)
        .map_err(|_| CiRunStoreError::InvalidFinalization("CI run id"))?;
    Uuid::parse_str(&finalization.wf_run_id)
        .map_err(|_| CiRunStoreError::InvalidFinalization("workflow run id"))?;
    if finalization.completed_at.is_empty() || finalization.completed_at.len() > 64 {
        return Err(CiRunStoreError::InvalidFinalization(
            "journaled completion timestamp",
        ));
    }
    if finalization.jobs.is_empty() || finalization.jobs.len() > 1_024 {
        return Err(CiRunStoreError::InvalidFinalization("manifest job set"));
    }
    let mut job_ids = BTreeSet::new();
    let mut reserve_handles = BTreeSet::new();
    for job in &finalization.jobs {
        Uuid::parse_str(&job.job_id).map_err(|_| CiRunStoreError::InvalidFinalization("job id"))?;
        if job.reserve_handle.is_empty() || job.reserve_handle.len() > 512 {
            return Err(CiRunStoreError::InvalidFinalization("reserve handle"));
        }
        if !job_ids.insert(job.job_id.as_str()) {
            return Err(CiRunStoreError::InvalidFinalization("duplicate job id"));
        }
        if !reserve_handles.insert(job.reserve_handle.as_str()) {
            return Err(CiRunStoreError::InvalidFinalization(
                "duplicate reserve handle",
            ));
        }
        if job.flow_timed_out && !job.dispatched {
            return Err(CiRunStoreError::InvalidFinalization(
                "timed-out undispatched job",
            ));
        }
    }
    Ok(())
}

fn validate_finalization_scope(
    scope: &TenantScope,
    finalization: &CiRunFinalization,
) -> Result<(), CiRunStoreError> {
    validate_finalization(finalization)?;
    if scope.tenant().as_str() != finalization.tenant_id
        || scope.region().as_str() != finalization.region
    {
        return Err(CiRunStoreError::InvalidFinalization(
            "tenant or region scope",
        ));
    }
    Ok(())
}

async fn allocate_reserve_check_attempts(
    conn: &mut sqlx::PgConnection,
    row: &CiRunInsert,
    contexts: &BTreeSet<String>,
) -> Result<BTreeMap<String, u32>, CiRunStoreError> {
    let repo_ref = row
        .repo_ref
        .as_deref()
        .ok_or_else(|| CiRunStoreError::Db("reserve lacks repository provenance".into()))?;
    let commit_oid = row
        .commit_oid
        .as_deref()
        .ok_or_else(|| CiRunStoreError::Db("reserve lacks commit provenance".into()))?;
    let run_id = Uuid::parse_str(&row.run_id)
        .map_err(|_| CiRunStoreError::Db("reserve run id is not a UUID".into()))?;
    let mut attempts = BTreeMap::new();
    for context in contexts {
        if let Some((stored_repo, stored_commit, stored_attempt)) =
            sqlx::query_as::<_, (String, String, i32)>(SELECT_RESERVED_CHECK_ATTEMPT_QUERY)
                .bind(&row.tenant_id)
                .bind(&row.region)
                .bind(run_id)
                .bind(context)
                .fetch_optional(&mut *conn)
                .await
                .map_err(|error| {
                    CiRunStoreError::Db(format!("read reserved check attempt: {error}"))
                })?
        {
            if stored_repo != repo_ref || stored_commit != commit_oid || stored_attempt <= 0 {
                return Err(CiRunStoreError::Db(
                    "reserved check attempt provenance diverged".into(),
                ));
            }
            attempts.insert(context.clone(), stored_attempt as u32);
            continue;
        }

        let attempt_i32: i32 = sqlx::query_scalar(crate::check_emitter::BUMP_CHECK_ATTEMPT_SQL)
            .bind(&row.tenant_id)
            .bind(&row.region)
            .bind(repo_ref)
            .bind(commit_oid)
            .bind(context)
            .bind(run_id)
            .fetch_one(&mut *conn)
            .await
            .map_err(|error| {
                CiRunStoreError::Db(format!("allocate reserve check attempt: {error}"))
            })?;
        let attempt = u32::try_from(attempt_i32)
            .map_err(|_| CiRunStoreError::Db("check attempt is not a positive u32".into()))?;
        if attempt == 0 {
            return Err(CiRunStoreError::Db(
                "check attempt allocation returned zero".into(),
            ));
        }
        let inserted: Option<i32> = sqlx::query_scalar(
            "INSERT INTO ci_run_check_attempt \
             (tenant_id,region,run_id,repo_ref,commit_oid,context,run_attempt) \
             VALUES ($1,$2,$3,$4,$5,$6,$7) \
             ON CONFLICT (tenant_id,run_id,context) DO NOTHING \
             RETURNING run_attempt",
        )
        .bind(&row.tenant_id)
        .bind(&row.region)
        .bind(run_id)
        .bind(repo_ref)
        .bind(commit_oid)
        .bind(context)
        .bind(attempt_i32)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|error| CiRunStoreError::Db(format!("persist reserved check attempt: {error}")))?;
        if inserted != Some(attempt_i32) {
            return Err(CiRunStoreError::Db(
                "reserved check attempt collided with divergent authority".into(),
            ));
        }
        attempts.insert(context.clone(), attempt);
    }
    Ok(attempts)
}

async fn insert_on_conn(
    conn: &mut sqlx::PgConnection,
    row: &CiRunInsert,
) -> Result<bool, CiRunStoreError> {
    validate_initial_state(row)?;
    if let Some(group) = &row.concurrency_group {
        lock_pr_concurrency_group_on_conn(conn, &row.tenant_id, &row.region, group)
            .await
            .map_err(|_| CiRunStoreError::Db("PR concurrency-group lock".into()))?;
    }
    lock_ci_run_idempotency_on_conn(conn, &row.tenant_id, &row.run_id).await?;
    if verify_existing_replay_on_conn(conn, row).await? {
        return Ok(false);
    }

    let inserted = sqlx::query(INSERT_CI_RUN_QUERY)
        .bind(&row.tenant_id)
        .bind(&row.region)
        .bind(&row.run_id)
        .bind(&row.project_id)
        .bind(&row.pipeline_id)
        .bind(&row.wf_run_id)
        .bind(&row.repo_ref)
        .bind(&row.source_ref)
        .bind(&row.commit_oid)
        .bind(&row.cause_event_id)
        .bind(row.cause_depth)
        .bind(&row.caused_by)
        .bind(&row.definition_snapshot)
        .bind(&row.trigger_kind)
        .bind(&row.concurrency_group)
        .bind(row.pr_head_generation)
        .bind(&row.triggered_by)
        .bind(&row.trust_tier)
        .bind(&row.state)
        .bind(&row.correlation_id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| CiRunStoreError::Db(e.to_string()))?;
    if inserted.is_some() {
        return Ok(true);
    }

    if verify_existing_replay_on_conn(conn, row).await? {
        Ok(false)
    } else {
        Err(CiRunStoreError::ConflictNotVisible)
    }
}

async fn lock_ci_run_idempotency_on_conn(
    conn: &mut sqlx::PgConnection,
    tenant_id: &str,
    run_id: &str,
) -> Result<(), CiRunStoreError> {
    let identity = format!(
        "{CI_RUN_IDEMPOTENCY_LOCK_DOMAIN}:{}:{tenant_id}:{}:{run_id}",
        tenant_id.len(),
        run_id.len()
    );
    sqlx::query(LOCK_CI_RUN_IDEMPOTENCY_QUERY)
        .bind(identity)
        .execute(&mut *conn)
        .await
        .map_err(|error| {
            CiRunStoreError::Db(format!("acquire ci_run idempotency lock: {error}"))
        })?;
    Ok(())
}

async fn verify_existing_replay_on_conn(
    conn: &mut sqlx::PgConnection,
    row: &CiRunInsert,
) -> Result<bool, CiRunStoreError> {
    let stored = sqlx::query(VERIFY_CI_RUN_REPLAY_QUERY)
        .bind(&row.tenant_id)
        .bind(&row.region)
        .bind(&row.run_id)
        .bind(&row.project_id)
        .bind(&row.pipeline_id)
        .bind(&row.wf_run_id)
        .bind(&row.repo_ref)
        .bind(&row.source_ref)
        .bind(&row.commit_oid)
        .bind(&row.cause_event_id)
        .bind(row.cause_depth)
        .bind(&row.caused_by)
        .bind(&row.definition_snapshot)
        .bind(&row.trigger_kind)
        .bind(&row.concurrency_group)
        .bind(row.pr_head_generation)
        .bind(&row.trust_tier)
        .bind(&row.correlation_id)
        .bind(&row.triggered_by)
        .bind(crate::ERASED_PSEUDONYM)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| CiRunStoreError::Db(e.to_string()))?;
    let Some(stored) = stored else {
        return Ok(false);
    };

    let mut differing_fields = Vec::new();
    for (field, matches) in [
        ("region", stored.get::<bool, _>("region_matches")),
        ("project_id", stored.get::<bool, _>("project_id_matches")),
        ("pipeline_id", stored.get::<bool, _>("pipeline_id_matches")),
        ("wf_run_id", stored.get::<bool, _>("wf_run_id_matches")),
        ("repo_ref", stored.get::<bool, _>("repo_ref_matches")),
        ("source_ref", stored.get::<bool, _>("source_ref_matches")),
        ("commit_oid", stored.get::<bool, _>("commit_oid_matches")),
        (
            "cause_event_id",
            stored.get::<bool, _>("cause_event_id_matches"),
        ),
        ("cause_depth", stored.get::<bool, _>("cause_depth_matches")),
        ("caused_by", stored.get::<bool, _>("caused_by_matches")),
        (
            "definition_snapshot",
            stored.get::<bool, _>("definition_snapshot_matches"),
        ),
        (
            "trigger_kind",
            stored.get::<bool, _>("trigger_kind_matches"),
        ),
        (
            "concurrency_group",
            stored.get::<bool, _>("concurrency_group_matches"),
        ),
        (
            "pr_head_generation",
            stored.get::<bool, _>("pr_head_generation_matches"),
        ),
        ("trust_tier", stored.get::<bool, _>("trust_tier_matches")),
        (
            "correlation_id",
            stored.get::<bool, _>("correlation_id_matches"),
        ),
        (
            "triggered_by",
            stored.get::<bool, _>("triggered_by_matches"),
        ),
    ] {
        if !matches {
            differing_fields.push(field);
        }
    }

    if differing_fields.is_empty() {
        Ok(true)
    } else {
        Err(CiRunStoreError::ReplayCollision { differing_fields })
    }
}

pub(crate) async fn lock_pr_concurrency_group_on_conn(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    group: &str,
) -> Result<(), sqlx::Error> {
    // @tenant-cross-scope: advisory locking reads no tenant rows. The framed key contains the
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "{PR_RUN_SUPERSESSION_LOCK_DOMAIN}:{tenant}:{region}:{group}"
        ))
        .execute(&mut *conn)
        .await?;
    Ok(())
}

fn validate_initial_state(row: &CiRunInsert) -> Result<(), CiRunStoreError> {
    if row.state != "queued" {
        return Err(CiRunStoreError::InvalidInitialState);
    }
    match (
        row.trigger_kind.as_str(),
        row.concurrency_group.as_deref(),
        row.pr_head_generation,
    ) {
        ("pull_request", Some(group), Some(generation))
            if valid_pr_concurrency_group(group) && generation > 0 => {}
        ("pull_request", group, _) if !group.is_some_and(valid_pr_concurrency_group) => {
            return Err(CiRunStoreError::InvalidConcurrencyGroup);
        }
        ("pull_request", _, _) => {
            return Err(CiRunStoreError::InvalidPrHeadGeneration);
        }
        (_, None, None) => {}
        (_, Some(_), _) => {
            return Err(CiRunStoreError::InvalidConcurrencyGroup);
        }
        (_, None, Some(_)) => return Err(CiRunStoreError::InvalidPrHeadGeneration),
    }
    u32::try_from(row.cause_depth)
        .map(|_| ())
        .map_err(|_| CiRunStoreError::InvalidCausalDepth)
}

pub(crate) fn valid_pr_concurrency_group(group: &str) -> bool {
    if group.len() > 512 || group.chars().any(char::is_control) {
        return false;
    }
    let Some(rest) = group.strip_prefix("pr:") else {
        return false;
    };
    let Some((repo, number)) = rest.rsplit_once(':') else {
        return false;
    };
    let Ok(parsed_number) = number.parse::<u64>() else {
        return false;
    };
    if parsed_number == 0 || parsed_number.to_string() != number {
        return false;
    }
    let mut pieces = repo.split('/');
    let mut saw_piece = false;
    for piece in &mut pieces {
        saw_piece = true;
        if piece.is_empty()
            || piece == "."
            || piece == ".."
            || !piece
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        {
            return false;
        }
    }
    saw_piece
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::{Region, TenantId};

    fn scope() -> TenantScope {
        let principal = Principal::stub(
            PrincipalId("ci-finalizer-test".into()),
            PrincipalKind::Service,
            TenantId("acme".into()),
        );
        TenantScope::from_verified_token(&principal, Region("fr-par".into()))
    }

    fn sample_row() -> CiRunInsert {
        CiRunInsert {
            tenant_id: "acme".into(),
            region: "fr-par".into(),
            run_id: "11111111-1111-1111-1111-111111111111".into(),
            project_id: "22222222-2222-2222-2222-222222222222".into(),
            pipeline_id: "33333333-3333-3333-3333-333333333333".into(),
            wf_run_id: "44444444-4444-4444-4444-444444444444".into(),
            definition_snapshot: "blake3:abcd".into(),
            trigger_kind: "push".into(),
            concurrency_group: None,
            pr_head_generation: None,
            trust_tier: "trusted".into(),
            state: "queued".into(),
            correlation_id: "corr-1".into(),
            cause_event_id: Some("ev-push-1".into()),
            cause_depth: 0,
            caused_by: None,
            repo_ref: Some("web".into()),
            source_ref: Some("refs/heads/main".into()),
            commit_oid: Some("deadbeef".into()),
            triggered_by: None,
        }
    }

    fn sample_finalization() -> CiRunFinalization {
        CiRunFinalization {
            tenant_id: "acme".into(),
            region: "fr-par".into(),
            run_id: "11111111-1111-1111-1111-111111111111".into(),
            wf_run_id: "44444444-4444-4444-4444-444444444444".into(),
            terminal_state: CiRunTerminalState::Succeeded,
            completed_at: "2026-07-21T13:00:00Z".into(),
            jobs: vec![CiRunFinalizationJob {
                job_id: "55555555-5555-5555-5555-555555555555".into(),
                reserve_handle: "reserve:job-1".into(),
                flow_timed_out: false,
                dispatched: true,
            }],
        }
    }

    #[test]
    fn insert_query_is_idempotent_on_the_pk_and_binds_every_column() {
        assert!(
            INSERT_CI_RUN_QUERY.contains("ON CONFLICT (tenant_id, run_id) DO NOTHING"),
            "idempotent on the run-of-record PK"
        );
        for n in 1..=20 {
            assert!(INSERT_CI_RUN_QUERY.contains(&format!("${n}")), "binds ${n}");
        }
        assert!(
            !INSERT_CI_RUN_QUERY.contains("$21"),
            "no over-bind past $20"
        );
        assert!(INSERT_CI_RUN_QUERY.contains("$3::uuid"));
        let r = sample_row();
        assert_eq!(r.state, "queued");
        assert_eq!(r.trigger_kind, "push");
        assert!(
            r.triggered_by.is_none(),
            "the proven shape leaves triggered_by NULL"
        );
    }

    #[test]
    fn replay_verification_is_a_region_bound_locking_statement() {
        assert!(VERIFY_CI_RUN_REPLAY_QUERY.contains("tenant_id = $1"));
        assert!(VERIFY_CI_RUN_REPLAY_QUERY.contains("region = $2"));
        assert!(VERIFY_CI_RUN_REPLAY_QUERY.contains("run_id = $3::uuid"));
        assert!(VERIFY_CI_RUN_REPLAY_QUERY.contains("FOR KEY SHARE"));
        for field in [
            "region",
            "project_id",
            "pipeline_id",
            "wf_run_id",
            "repo_ref",
            "source_ref",
            "commit_oid",
            "cause_event_id",
            "cause_depth",
            "caused_by",
            "definition_snapshot",
            "trigger_kind",
            "concurrency_group",
            "pr_head_generation",
            "trust_tier",
            "correlation_id",
            "triggered_by",
        ] {
            assert!(
                VERIFY_CI_RUN_REPLAY_QUERY.contains(field),
                "selects {field}"
            );
        }
        for mutable in ["state", "cost_settled", "finished_at", "created_at"] {
            assert!(
                !VERIFY_CI_RUN_REPLAY_QUERY.contains(mutable),
                "excludes mutable {mutable}"
            );
        }
    }

    #[test]
    fn run_idempotency_lock_frames_the_whole_tenant_and_run_identity() {
        assert_eq!(
            LOCK_CI_RUN_IDEMPOTENCY_QUERY,
            "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))"
        );
        assert!(CI_RUN_IDEMPOTENCY_LOCK_DOMAIN.ends_with(".v1"));
    }

    #[test]
    fn non_queued_insert_is_rejected_before_sql() {
        let mut r = sample_row();
        r.state = "running".into();
        assert_eq!(
            validate_initial_state(&r),
            Err(CiRunStoreError::InvalidInitialState)
        );
    }

    #[test]
    fn non_canonical_causal_depth_is_rejected_before_sql() {
        let mut r = sample_row();
        r.cause_depth = -1;
        assert_eq!(
            validate_initial_state(&r),
            Err(CiRunStoreError::InvalidCausalDepth)
        );

        r.cause_depth = i64::from(u32::MAX) + 1;
        assert_eq!(
            validate_initial_state(&r),
            Err(CiRunStoreError::InvalidCausalDepth)
        );
    }

    #[test]
    fn new_run_concurrency_identity_is_pr_only_and_canonical() {
        let mut row = sample_row();
        row.trigger_kind = "pull_request".into();
        assert_eq!(
            validate_initial_state(&row),
            Err(CiRunStoreError::InvalidConcurrencyGroup),
            "new PR rows cannot omit the supersession identity"
        );

        row.concurrency_group = Some("pr:team/core:42".into());
        row.pr_head_generation = Some(7);
        assert!(validate_initial_state(&row).is_ok());

        for invalid in [
            "pr:team//core:42",
            "pr:../core:42",
            "pr:team/core:0",
            "pr:team/core:042",
            "pr:team/core:not-a-number",
            "deploy:prod",
        ] {
            row.concurrency_group = Some(invalid.into());
            assert_eq!(
                validate_initial_state(&row),
                Err(CiRunStoreError::InvalidConcurrencyGroup),
                "invalid group {invalid:?} is refused"
            );
        }

        row.trigger_kind = "push".into();
        row.concurrency_group = Some("pr:team/core:42".into());
        assert_eq!(
            validate_initial_state(&row),
            Err(CiRunStoreError::InvalidConcurrencyGroup),
            "non-PR rows cannot smuggle PR scheduler authority"
        );

        row.concurrency_group = None;
        row.pr_head_generation = Some(7);
        assert_eq!(
            validate_initial_state(&row),
            Err(CiRunStoreError::InvalidPrHeadGeneration),
            "non-PR rows cannot smuggle PR ordering authority"
        );

        row.trigger_kind = "pull_request".into();
        row.concurrency_group = Some("pr:team/core:42".into());
        for invalid in [None, Some(0), Some(-1)] {
            row.pr_head_generation = invalid;
            assert_eq!(
                validate_initial_state(&row),
                Err(CiRunStoreError::InvalidPrHeadGeneration),
                "PR generation {invalid:?} is refused"
            );
        }
    }

    #[test]
    fn finalization_queries_lock_identity_verify_complete_receipts_and_cas_running() {
        for query in [
            LOCK_CI_RUN_FOR_FINALIZE_QUERY,
            SELECT_CI_RUN_ACCOUNTING_QUERY,
            FINALIZE_CI_RUN_QUERY,
        ] {
            for predicate in [
                "tenant_id = $1",
                "region = $2",
                "run_id = $3::uuid",
                "wf_run_id = $4::uuid",
            ] {
                assert!(query.contains(predicate), "query must bind {predicate}");
            }
        }
        assert!(LOCK_CI_RUN_FOR_FINALIZE_QUERY.contains("FOR UPDATE"));
        assert!(
            !SELECT_CI_RUN_ACCOUNTING_QUERY.contains("FOR "),
            "immutable accounting reads need only the production SELECT grant"
        );
        assert!(SELECT_CI_RUN_ACCOUNTING_QUERY.contains("skipped"));
        for guard in [
            "state = 'running'",
            "cost_settled = false",
            "finished_at IS NULL",
        ] {
            assert!(FINALIZE_CI_RUN_QUERY.contains(guard));
        }
        assert!(FINALIZE_CI_RUN_QUERY.contains("cost_settled = true"));
        assert!(FINALIZE_CI_RUN_QUERY.contains("finished_at = $6::timestamptz"));
    }

    #[test]
    fn immutable_reserved_attempt_reads_do_not_require_update_authority() {
        assert!(
            !SELECT_RESERVED_CHECK_ATTEMPT_QUERY.contains("FOR "),
            "ci_run_check_attempt is immutable and the runtime role intentionally lacks UPDATE"
        );
        for predicate in ["tenant_id=$1", "region=$2", "run_id=$3", "context=$4"] {
            assert!(
                SELECT_RESERVED_CHECK_ATTEMPT_QUERY.contains(predicate),
                "immutable replay remains scoped by {predicate}"
            );
        }
    }

    #[test]
    fn finalization_rejects_duplicate_job_or_reservation_authority_before_sql() {
        let mut duplicate_job = sample_finalization();
        duplicate_job.jobs.push(duplicate_job.jobs[0].clone());
        assert_eq!(
            validate_finalization(&duplicate_job),
            Err(CiRunStoreError::InvalidFinalization("duplicate job id"))
        );

        let mut duplicate_reserve = sample_finalization();
        duplicate_reserve.jobs.push(CiRunFinalizationJob {
            job_id: "66666666-6666-6666-6666-666666666666".into(),
            reserve_handle: duplicate_reserve.jobs[0].reserve_handle.clone(),
            flow_timed_out: false,
            dispatched: true,
        });
        assert_eq!(
            validate_finalization(&duplicate_reserve),
            Err(CiRunStoreError::InvalidFinalization(
                "duplicate reserve handle"
            ))
        );
    }

    #[test]
    fn skipped_receipt_is_stable_across_acknowledgement_loss_retries() {
        let first = sample_finalization();
        let first_receipt = skipped_completion_receipt(&scope(), &first, &first.jobs[0]);
        let mut retry = first.clone();
        retry.completed_at = "2026-07-21T13:00:01Z".into();
        retry.terminal_state = CiRunTerminalState::Failed;

        assert_eq!(
            skipped_completion_receipt(&scope(), &retry, &retry.jobs[0]),
            first_receipt
        );
    }
}
