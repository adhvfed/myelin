//! PostgreSQL-backed starter for queued CI runs.
//!
//! A starter is composed for one explicit `(tenant, region)` cell. It never discovers tenants and
//! never scans a region globally. The selected `ci_run` row, the pre-minted `workflow_run`, and the
//! `queued -> running` transition are committed on one caller-owned PostgreSQL transaction.
//!
//! This module is deliberately not composed by the service main yet. The v1 plan and current CI
//! schemas do not durably provide the runner lane/labels, timeout and resource authority, egress and
//! workspace grants, per-run token, metering reservation, check-attempt/context facts, or restart-safe
//! stage identity required to execute/report a job. Starting this durable control record is a bounded
//! prerequisite; registering a production body or enabling runner dispatch remains gated on those
//! authoritative fields and on replacing the main's no-op runner hooks.

use std::sync::Arc;

use myelin_events::{HandlerTx, IdMinter};
use myelin_flow::{PgFlowExecutor, RunId, StartSpec, CI_PIPELINE_WF_TYPE};
use myelin_refs::ArtifactRef;
use myelin_storage::BlobStore;
use myelin_tenancy::{Region, TenantId};
use sqlx::{PgPool, Row};

use crate::ci_run_store::CiRunRecord;
use crate::run_plan::{load_resolved_run_plan, RunPlanError};
use crate::surfacing::{ci_artifact_ref, ci_run_ref};

const CLAIM_QUEUED_RUN: &str = "\
SELECT tenant_id, run_id::text AS run_id, region, project_id::text AS project_id,
       pipeline_id::text AS pipeline_id, wf_run_id::text AS wf_run_id, repo_ref, commit_oid,
       cause_event_id, definition_snapshot, trigger_kind, trust_tier, state, correlation_id
FROM ci_run
WHERE tenant_id = $1 AND region = $2 AND state = 'queued'
ORDER BY created_at, run_id
FOR UPDATE SKIP LOCKED
LIMIT 1";

/// One bounded starter pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartQueuedOutcome {
    /// No queued row exists in this exact configured cell.
    Idle,
    /// The row and workflow were atomically advanced.
    Started { run_id: String, wf_run_id: String },
}

/// Fail-closed starter errors. The transaction is rolled back for every variant.
#[derive(Debug)]
pub enum PgCiStarterError {
    InvalidScope(String),
    Database(String),
    CorruptRun(String),
    Workflow(myelin_flow::ExecutorError),
    Plan(RunPlanError),
    WorkflowIdentityMismatch { expected: String, actual: String },
}

impl std::fmt::Display for PgCiStarterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidScope(message) => write!(f, "invalid CI starter scope: {message}"),
            Self::Database(message) => write!(f, "CI starter database error: {message}"),
            Self::CorruptRun(message) => write!(f, "queued CI run refused: {message}"),
            Self::Workflow(error) => write!(f, "durable workflow start refused: {error}"),
            Self::Plan(error) => write!(f, "queued CI run plan refused: {error}"),
            Self::WorkflowIdentityMismatch { expected, actual } => write!(
                f,
                "durable workflow idempotency collision: queued run requires `{expected}` but key resolved to `{actual}`"
            ),
        }
    }
}

impl std::error::Error for PgCiStarterError {}

/// A bounded, exact-cell queued-run starter. Construct one instance for each explicitly configured
/// tenant and region; there is deliberately no tenant enumeration constructor or API.
#[derive(Clone)]
pub struct PgCiPipelineStarter {
    pool: PgPool,
    tenant: TenantId,
    region: Region,
    executor: PgFlowExecutor,
    blobs: Arc<dyn BlobStore + Send + Sync>,
}

impl PgCiPipelineStarter {
    pub fn new(
        pool: PgPool,
        rt: tokio::runtime::Handle,
        minter: Arc<dyn IdMinter>,
        tenant: TenantId,
        region: Region,
        blobs: Arc<dyn BlobStore + Send + Sync>,
    ) -> Result<Self, PgCiStarterError> {
        validate_scope("tenant", &tenant.0)?;
        validate_scope("region", &region.0)?;
        let executor =
            PgFlowExecutor::new(pool.clone(), rt, minter, tenant.clone(), region.clone());
        Ok(Self {
            pool,
            tenant,
            region,
            executor,
            blobs,
        })
    }

    /// Claim and start at most one row. `start_with_id_on_conn` receives the exact connection owned
    /// by this transaction; the lifecycle update follows on that connection before one commit.
    pub async fn run_once(&self) -> Result<StartQueuedOutcome, PgCiStarterError> {
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| PgCiStarterError::Database(format!("begin: {error}")))?;
        sqlx::query(
            "SELECT set_config('myelin.tenant_id', $1, true), \
                    set_config('myelin.region', $2, true)",
        )
        .bind(&self.tenant.0)
        .bind(&self.region.0)
        .execute(&mut *transaction)
        .await
        .map_err(|error| PgCiStarterError::Database(format!("scope transaction: {error}")))?;

        let row = sqlx::query(CLAIM_QUEUED_RUN)
            .bind(&self.tenant.0)
            .bind(&self.region.0)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|error| PgCiStarterError::Database(format!("claim queued run: {error}")))?;
        let Some(row) = row else {
            transaction.rollback().await.map_err(|error| {
                PgCiStarterError::Database(format!("rollback idle pass: {error}"))
            })?;
            return Ok(StartQueuedOutcome::Idle);
        };
        let record = decode_record(&row)?;
        validate_record(&self.tenant, &self.region, &record)?;
        // This bounded CAS head/get happens while the row lock is held. That deliberately favors a
        // single simple correctness boundary over a read/re-lock TOCTOU seam: the object is capped at
        // MAX_RUN_PLAN_BYTES, content-address verified on read, and the exact row cannot change until
        // its validated snapshot reference and workflow start commit together.
        load_resolved_run_plan(self.blobs.as_ref(), &record).map_err(PgCiStarterError::Plan)?;

        let workflow_input = workflow_input(&record)?;
        let started = {
            let mut handler_tx = HandlerTx::with_connection(&mut *transaction);
            self.executor
                .start_with_id_on_conn(
                    &mut handler_tx,
                    StartSpec {
                        wf_type: CI_PIPELINE_WF_TYPE.into(),
                        input: workflow_input.clone(),
                        budget: None,
                        idem_key: format!("ci:{}", record.run_id),
                    },
                    Some(RunId(record.wf_run_id.clone())),
                )
                .map_err(PgCiStarterError::Workflow)?
        };
        if started.0 != record.wf_run_id {
            return Err(PgCiStarterError::WorkflowIdentityMismatch {
                expected: record.wf_run_id,
                actual: started.0,
            });
        }
        verify_started_workflow(&mut transaction, &record, &workflow_input).await?;

        let updated = sqlx::query(
            "UPDATE ci_run SET state = 'running' \
             WHERE tenant_id = $1 AND region = $2 AND run_id = $3::uuid AND state = 'queued'",
        )
        .bind(&record.tenant_id)
        .bind(&record.region)
        .bind(&record.run_id)
        .execute(&mut *transaction)
        .await
        .map_err(|error| PgCiStarterError::Database(format!("mark run running: {error}")))?;
        if updated.rows_affected() != 1 {
            return Err(PgCiStarterError::Database(
                "queued-to-running compare-and-set affected no row".into(),
            ));
        }
        transaction
            .commit()
            .await
            .map_err(|error| PgCiStarterError::Database(format!("commit run start: {error}")))?;
        Ok(StartQueuedOutcome::Started {
            run_id: record.run_id,
            wf_run_id: started.0,
        })
    }
}

fn validate_scope(label: &str, value: &str) -> Result<(), PgCiStarterError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(PgCiStarterError::InvalidScope(format!(
            "{label} must be a non-empty bounded machine token"
        )));
    }
    Ok(())
}

fn validate_record(
    tenant: &TenantId,
    region: &Region,
    record: &CiRunRecord,
) -> Result<(), PgCiStarterError> {
    if record.tenant_id != tenant.0 || record.region != region.0 {
        return Err(PgCiStarterError::CorruptRun(
            "claimed row escaped the configured tenant/region scope".into(),
        ));
    }
    if record.state != "queued" {
        return Err(PgCiStarterError::CorruptRun(
            "claimed row is not queued".into(),
        ));
    }
    if record.repo_ref.as_deref().is_none_or(str::is_empty)
        || record.commit_oid.as_deref().is_none_or(str::is_empty)
        || record.definition_snapshot.is_empty()
    {
        return Err(PgCiStarterError::CorruptRun(
            "repository, commit, and definition snapshot provenance are required".into(),
        ));
    }
    Ok(())
}

fn decode_record(row: &sqlx::postgres::PgRow) -> Result<CiRunRecord, PgCiStarterError> {
    macro_rules! field {
        ($name:literal) => {
            row.try_get($name).map_err(|error| {
                PgCiStarterError::CorruptRun(format!(
                    "cannot decode authoritative `{}` column: {error}",
                    $name
                ))
            })?
        };
    }
    Ok(CiRunRecord {
        tenant_id: field!("tenant_id"),
        run_id: field!("run_id"),
        region: field!("region"),
        project_id: field!("project_id"),
        pipeline_id: field!("pipeline_id"),
        wf_run_id: field!("wf_run_id"),
        repo_ref: field!("repo_ref"),
        commit_oid: field!("commit_oid"),
        cause_event_id: field!("cause_event_id"),
        definition_snapshot: field!("definition_snapshot"),
        trigger_kind: field!("trigger_kind"),
        trust_tier: field!("trust_tier"),
        state: field!("state"),
        correlation_id: field!("correlation_id"),
    })
}

fn workflow_input(record: &CiRunRecord) -> Result<Vec<ArtifactRef>, PgCiStarterError> {
    let prefix = format!("myelin://{}/ci/snapshot/", record.tenant_id);
    let address = record
        .definition_snapshot
        .strip_prefix(&prefix)
        .ok_or_else(|| {
            PgCiStarterError::CorruptRun(
                "validated snapshot reference no longer matches the authoritative tenant".into(),
            )
        })?;
    Ok(vec![
        // `snapshot` is not a frozen Bus artifact type. The injective, reversible id keeps the full
        // multihash (`snapshot-<algorithm>:<digest>`) under canonical `ci/artifact`; ci_run's
        // definition_snapshot remains the authoritative classed URI.
        ci_artifact_ref(&record.tenant_id, &format!("snapshot-{address}")),
        ci_run_ref(&record.tenant_id, &record.run_id),
    ])
}

async fn verify_started_workflow(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    record: &CiRunRecord,
    expected_input: &[ArtifactRef],
) -> Result<(), PgCiStarterError> {
    let row = sqlx::query(
        "SELECT wf_type, idem_key, input, state FROM workflow_run \
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3 FOR UPDATE",
    )
    .bind(&record.tenant_id)
    .bind(&record.region)
    .bind(&record.wf_run_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|error| PgCiStarterError::Database(format!("verify workflow start: {error}")))?
    .ok_or_else(|| {
        PgCiStarterError::CorruptRun(
            "workflow start returned a handle but no exact durable row exists".into(),
        )
    })?;
    let wf_type: String = row
        .try_get("wf_type")
        .map_err(|error| PgCiStarterError::CorruptRun(format!("decode wf_type: {error}")))?;
    let idem_key: String = row
        .try_get("idem_key")
        .map_err(|error| PgCiStarterError::CorruptRun(format!("decode idem_key: {error}")))?;
    let input: serde_json::Value = row
        .try_get("input")
        .map_err(|error| PgCiStarterError::CorruptRun(format!("decode workflow input: {error}")))?;
    let state: String = row
        .try_get("state")
        .map_err(|error| PgCiStarterError::CorruptRun(format!("decode workflow state: {error}")))?;
    let expected_input = serde_json::to_value(expected_input).map_err(|error| {
        PgCiStarterError::CorruptRun(format!("encode expected workflow input: {error}"))
    })?;
    if wf_type != CI_PIPELINE_WF_TYPE
        || idem_key != format!("ci:{}", record.run_id)
        || input != expected_input
        || !matches!(state.as_str(), "running" | "waiting")
    {
        return Err(PgCiStarterError::CorruptRun(format!(
            "existing workflow row diverges from queued run authority (wf_type={wf_type}, idem_key={idem_key}, state={state})"
        )));
    }
    Ok(())
}
