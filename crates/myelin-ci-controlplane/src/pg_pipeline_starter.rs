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
use myelin_flow::{partition_for_run_id, PgFlowExecutor, RunId, StartSpec, CI_PIPELINE_WF_TYPE};
use myelin_refs::ArtifactRef;
use myelin_storage::{BlobStore, ContentHash, HashAlgo};
use myelin_tenancy::{Region, TenantId};
use sqlx::{PgPool, Row};

use crate::ci_run_store::CiRunRecord;
use crate::run_plan::{load_resolved_run_plan, RunPlanError};
use crate::surfacing::{ci_artifact_ref, ci_run_ref};

const SELECT_QUEUED_RUN: &str = "\
SELECT tenant_id, run_id::text AS run_id, region, project_id::text AS project_id,
       pipeline_id::text AS pipeline_id, wf_run_id::text AS wf_run_id, repo_ref, commit_oid,
       cause_event_id, definition_snapshot, trigger_kind, triggered_by, trust_tier, state,
       cost_settled, correlation_id, created_at::text AS created_at,
       finished_at::text AS finished_at
FROM ci_run
WHERE tenant_id = $1 AND region = $2 AND state = 'queued'
ORDER BY created_at, run_id
LIMIT 1";

const LOCK_EXACT_QUEUED_RUN: &str = "\
SELECT tenant_id, run_id::text AS run_id, region, project_id::text AS project_id,
       pipeline_id::text AS pipeline_id, wf_run_id::text AS wf_run_id, repo_ref, commit_oid,
       cause_event_id, definition_snapshot, trigger_kind, triggered_by, trust_tier, state,
       cost_settled, correlation_id, created_at::text AS created_at,
       finished_at::text AS finished_at
FROM ci_run
WHERE tenant_id = $1 AND region = $2 AND run_id = $3::uuid AND state = 'queued'
FOR UPDATE";

/// Immutable code identity this starter is allowed to bind. `ci_run` does not yet carry this pin,
/// so the bounded composition must supply the exact deployed body version and hash explicitly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiWorkflowDefinitionPin {
    version: i32,
    code_hash: String,
}

impl CiWorkflowDefinitionPin {
    pub fn new(version: i32, code_hash: impl Into<String>) -> Result<Self, PgCiStarterError> {
        let code_hash = code_hash.into();
        if version <= 0 {
            return Err(PgCiStarterError::InvalidScope(
                "workflow definition version must be positive".into(),
            ));
        }
        if code_hash.trim().is_empty() || code_hash.len() > 256 {
            return Err(PgCiStarterError::InvalidScope(
                "workflow definition code hash must be non-empty and at most 256 bytes".into(),
            ));
        }
        Ok(Self { version, code_hash })
    }

    pub fn version(&self) -> i32 {
        self.version
    }

    pub fn code_hash(&self) -> &str {
        &self.code_hash
    }
}

/// Strict, Flow-safe decoding of the two references persisted as a CI workflow's claimed input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimedCiInput {
    tenant: TenantId,
    snapshot: ContentHash,
    run_id: String,
}

impl ClaimedCiInput {
    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    pub fn snapshot(&self) -> &ContentHash {
        &self.snapshot
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimedCiInputError(pub String);

impl std::fmt::Display for ClaimedCiInputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid claimed CI workflow input: {}", self.0)
    }
}

impl std::error::Error for ClaimedCiInputError {}

/// Decode exactly `[ci/artifact/snapshot-<full-multihash>, ci/run/<uuid>]`. No extra reference,
/// suffix, foreign tenant, abbreviated digest, or different canonical artifact type is accepted.
pub fn decode_ci_claimed_input(
    expected_tenant: &TenantId,
    input: &[ArtifactRef],
) -> Result<ClaimedCiInput, ClaimedCiInputError> {
    if input.len() != 2 {
        return Err(ClaimedCiInputError(
            "expected exactly snapshot artifact then CI run reference".into(),
        ));
    }
    let snapshot_ref = myelin_refs::parse_scoped(&input[0].0)
        .map_err(|error| ClaimedCiInputError(format!("snapshot reference: {error}")))?;
    let run_ref = myelin_refs::parse_scoped(&input[1].0)
        .map_err(|error| ClaimedCiInputError(format!("run reference: {error}")))?;
    if snapshot_ref.tenant != *expected_tenant
        || run_ref.tenant != *expected_tenant
        || snapshot_ref.sub.is_some()
        || run_ref.sub.is_some()
        || snapshot_ref.subsystem != "ci"
        || snapshot_ref.type_ != "artifact"
        || run_ref.subsystem != "ci"
        || run_ref.type_ != "run"
    {
        return Err(ClaimedCiInputError(
            "references must be unsubscripted canonical CI artifact/run refs for the expected tenant"
                .into(),
        ));
    }
    let multihash = snapshot_ref.id.strip_prefix("snapshot-").ok_or_else(|| {
        ClaimedCiInputError("snapshot artifact id lacks `snapshot-` class".into())
    })?;
    let snapshot = ContentHash::parse(multihash)
        .map_err(|error| ClaimedCiInputError(format!("snapshot multihash: {error}")))?;
    if snapshot.algo != HashAlgo::Blake3
        || snapshot.digest_hex.len() != 64
        || !snapshot
            .digest_hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ClaimedCiInputError(
            "snapshot must be a canonical lowercase 32-byte BLAKE3 address".into(),
        ));
    }
    let run_id = sqlx::types::Uuid::parse_str(&run_ref.id)
        .map_err(|error| ClaimedCiInputError(format!("run id is not a UUID: {error}")))?
        .to_string();
    let canonical_snapshot = ci_artifact_ref(
        &expected_tenant.0,
        &format!("snapshot-{}", snapshot.to_multihash_string()),
    );
    let canonical_run = ci_run_ref(&expected_tenant.0, &run_id);
    if input[0] != canonical_snapshot || input[1] != canonical_run {
        return Err(ClaimedCiInputError(
            "claimed references are parseable but not byte-canonical".into(),
        ));
    }
    Ok(ClaimedCiInput {
        tenant: expected_tenant.clone(),
        snapshot,
        run_id,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StarterCandidate {
    record: CiRunRecord,
    triggered_by: Option<String>,
    cost_settled: bool,
    created_at: String,
    finished_at: Option<String>,
}

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
    definition: CiWorkflowDefinitionPin,
}

impl PgCiPipelineStarter {
    pub fn new(
        pool: PgPool,
        rt: tokio::runtime::Handle,
        minter: Arc<dyn IdMinter>,
        tenant: TenantId,
        region: Region,
        blobs: Arc<dyn BlobStore + Send + Sync>,
        definition: CiWorkflowDefinitionPin,
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
            definition,
        })
    }

    /// Validate one preflight candidate outside a database lock, then re-lock and byte-compare that
    /// exact row before starting it. `start_with_id_on_conn` receives the exact caller transaction;
    /// the workflow identity proof and lifecycle update follow before one commit.
    pub async fn run_once(&self) -> Result<StartQueuedOutcome, PgCiStarterError> {
        let Some(candidate) = self.preflight_candidate().await? else {
            return Ok(StartQueuedOutcome::Idle);
        };
        validate_candidate(&self.tenant, &self.region, &candidate)?;
        load_resolved_run_plan(self.blobs.as_ref(), &candidate.record)
            .map_err(PgCiStarterError::Plan)?;
        let workflow_input = workflow_input(&candidate.record)?;

        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| PgCiStarterError::Database(format!("begin: {error}")))?;
        scope_transaction(&mut transaction, &self.tenant, &self.region).await?;
        let tenant_id = &self.tenant.0;
        let row = sqlx::query(LOCK_EXACT_QUEUED_RUN)
            .bind(tenant_id)
            .bind(&self.region.0)
            .bind(&candidate.record.run_id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|error| PgCiStarterError::Database(format!("re-lock queued run: {error}")))?;
        let Some(row) = row else {
            transaction.rollback().await.map_err(|error| {
                PgCiStarterError::Database(format!("rollback concurrent-winner pass: {error}"))
            })?;
            return Ok(StartQueuedOutcome::Idle);
        };
        let locked = decode_candidate(&row)?;
        if locked != candidate {
            return Err(PgCiStarterError::CorruptRun(
                "authoritative ci_run changed between plan preflight and exact row lock".into(),
            ));
        }
        validate_candidate(&self.tenant, &self.region, &locked)?;
        let record = locked.record;
        let replay = lock_existing_exact_workflow(&mut transaction, &record).await?;
        validate_definition_pin(&mut transaction, &self.definition, replay).await?;
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
        verify_started_workflow(&mut transaction, &record, &workflow_input, &self.definition)
            .await?;

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

    async fn preflight_candidate(&self) -> Result<Option<StarterCandidate>, PgCiStarterError> {
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| PgCiStarterError::Database(format!("begin preflight: {error}")))?;
        scope_transaction(&mut transaction, &self.tenant, &self.region).await?;
        let tenant_id = &self.tenant.0;
        let row = sqlx::query(SELECT_QUEUED_RUN)
            .bind(tenant_id)
            .bind(&self.region.0)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|error| {
                PgCiStarterError::Database(format!("select queued preflight candidate: {error}"))
            })?;
        transaction.commit().await.map_err(|error| {
            PgCiStarterError::Database(format!("commit preflight selection: {error}"))
        })?;
        row.as_ref().map(decode_candidate).transpose()
    }
}

async fn scope_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant: &TenantId,
    region: &Region,
) -> Result<(), PgCiStarterError> {
    sqlx::query(
        "SELECT set_config('myelin.tenant_id', $1, true), \
                set_config('myelin.region', $2, true)",
    )
    .bind(&tenant.0)
    .bind(&region.0)
    .execute(&mut **transaction)
    .await
    .map_err(|error| PgCiStarterError::Database(format!("scope transaction: {error}")))?;
    Ok(())
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

fn validate_candidate(
    tenant: &TenantId,
    region: &Region,
    candidate: &StarterCandidate,
) -> Result<(), PgCiStarterError> {
    validate_record(tenant, region, &candidate.record)?;
    if candidate.cost_settled
        || candidate.finished_at.is_some()
        || candidate.created_at.trim().is_empty()
    {
        return Err(PgCiStarterError::CorruptRun(
            "queued ci_run has contradictory settled/finished/creation lifecycle facts".into(),
        ));
    }
    Ok(())
}

fn decode_candidate(row: &sqlx::postgres::PgRow) -> Result<StarterCandidate, PgCiStarterError> {
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
    Ok(StarterCandidate {
        record: CiRunRecord {
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
        },
        triggered_by: field!("triggered_by"),
        cost_settled: field!("cost_settled"),
        created_at: field!("created_at"),
        finished_at: field!("finished_at"),
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
    let input = vec![
        // `snapshot` is not a frozen Bus artifact type. The injective, reversible id keeps the full
        // multihash (`snapshot-<algorithm>:<digest>`) under canonical `ci/artifact`; ci_run's
        // definition_snapshot remains the authoritative classed URI.
        ci_artifact_ref(&record.tenant_id, &format!("snapshot-{address}")),
        ci_run_ref(&record.tenant_id, &record.run_id),
    ];
    let decoded = decode_ci_claimed_input(&TenantId(record.tenant_id.clone()), &input)
        .map_err(|error| PgCiStarterError::CorruptRun(error.to_string()))?;
    if decoded.snapshot.to_multihash_string() != address || decoded.run_id != record.run_id {
        return Err(PgCiStarterError::CorruptRun(
            "claimed-input encoding did not round-trip the authoritative snapshot and run".into(),
        ));
    }
    Ok(input)
}

async fn lock_existing_exact_workflow(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    record: &CiRunRecord,
) -> Result<bool, PgCiStarterError> {
    sqlx::query_scalar::<_, i32>(
        "SELECT 1 FROM workflow_run \
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3 FOR UPDATE",
    )
    .bind(&record.tenant_id)
    .bind(&record.region)
    .bind(&record.wf_run_id)
    .fetch_optional(&mut **transaction)
    .await
    .map(|row| row.is_some())
    .map_err(|error| PgCiStarterError::Database(format!("lock existing workflow: {error}")))
}

async fn validate_definition_pin(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    pin: &CiWorkflowDefinitionPin,
    replay: bool,
) -> Result<(), PgCiStarterError> {
    // Global code registry: tenant_id/region do not apply because definitions contain no tenant
    // data. This is the same loud annotation used by PgFlowExecutor's registry queries.
    let tenant_id_not_applicable = sqlx::query(
        "SELECT code_hash, status FROM wf_definition \
         WHERE wf_type = $1 AND version = $2 FOR SHARE \
         /* global registry: tenant_id and region do not apply */",
    );
    let row = tenant_id_not_applicable
        .bind(CI_PIPELINE_WF_TYPE)
        .bind(pin.version)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(|error| {
            PgCiStarterError::Database(format!("lock workflow definition pin: {error}"))
        })?
        .ok_or_else(|| {
            PgCiStarterError::CorruptRun(format!(
                "pinned workflow definition {}@{} is absent",
                CI_PIPELINE_WF_TYPE, pin.version
            ))
        })?;
    let code_hash: String = row
        .try_get("code_hash")
        .map_err(|error| PgCiStarterError::CorruptRun(format!("decode code_hash: {error}")))?;
    let status: String = row.try_get("status").map_err(|error| {
        PgCiStarterError::CorruptRun(format!("decode definition status: {error}"))
    })?;
    if code_hash != pin.code_hash {
        return Err(PgCiStarterError::CorruptRun(
            "pinned workflow definition code hash differs from deployed registry".into(),
        ));
    }
    // A replay pinned to existing code may finish while that version drains. A fresh start must use
    // an active definition. Retired/unknown states are never resurrected.
    if (replay && !matches!(status.as_str(), "active" | "draining"))
        || (!replay && status != "active")
    {
        return Err(PgCiStarterError::CorruptRun(format!(
            "pinned workflow definition status `{status}` is not eligible for this start"
        )));
    }
    if !replay {
        // Same global-registry annotation as the exact pinned-definition lookup above.
        let tenant_id_not_applicable = sqlx::query_scalar::<_, i32>(
            "SELECT version FROM wf_definition WHERE wf_type = $1 AND status = 'active' \
             ORDER BY version DESC LIMIT 1 \
             /* global registry: tenant_id and region do not apply */",
        );
        let selected = tenant_id_not_applicable
            .bind(CI_PIPELINE_WF_TYPE)
            .fetch_optional(&mut **transaction)
            .await
            .map_err(|error| {
                PgCiStarterError::Database(format!(
                    "resolve active workflow definition pin: {error}"
                ))
            })?;
        if selected != Some(pin.version) {
            return Err(PgCiStarterError::CorruptRun(format!(
                "active workflow selection does not equal pinned version {}",
                pin.version
            )));
        }
    }
    Ok(())
}

async fn verify_started_workflow(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    record: &CiRunRecord,
    expected_input: &[ArtifactRef],
    pin: &CiWorkflowDefinitionPin,
) -> Result<(), PgCiStarterError> {
    let row = sqlx::query(
        "SELECT wf_type, wf_version, idem_key, input, state, budget, correlation_id, \
                causation_id, caused_by, depth, partition \
         FROM workflow_run \
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
    let wf_version: i32 = row
        .try_get("wf_version")
        .map_err(|error| PgCiStarterError::CorruptRun(format!("decode wf_version: {error}")))?;
    let idem_key: String = row
        .try_get("idem_key")
        .map_err(|error| PgCiStarterError::CorruptRun(format!("decode idem_key: {error}")))?;
    let input: serde_json::Value = row
        .try_get("input")
        .map_err(|error| PgCiStarterError::CorruptRun(format!("decode workflow input: {error}")))?;
    let state: String = row
        .try_get("state")
        .map_err(|error| PgCiStarterError::CorruptRun(format!("decode workflow state: {error}")))?;
    let budget: Option<serde_json::Value> = row.try_get("budget").map_err(|error| {
        PgCiStarterError::CorruptRun(format!("decode workflow budget: {error}"))
    })?;
    let correlation_id: String = row.try_get("correlation_id").map_err(|error| {
        PgCiStarterError::CorruptRun(format!("decode workflow correlation_id: {error}"))
    })?;
    let causation_id: Option<String> = row.try_get("causation_id").map_err(|error| {
        PgCiStarterError::CorruptRun(format!("decode workflow causation_id: {error}"))
    })?;
    let caused_by: Option<String> = row.try_get("caused_by").map_err(|error| {
        PgCiStarterError::CorruptRun(format!("decode workflow caused_by: {error}"))
    })?;
    let depth: i32 = row
        .try_get("depth")
        .map_err(|error| PgCiStarterError::CorruptRun(format!("decode workflow depth: {error}")))?;
    let partition: i16 = row.try_get("partition").map_err(|error| {
        PgCiStarterError::CorruptRun(format!("decode workflow partition: {error}"))
    })?;
    let expected_input = serde_json::to_value(expected_input).map_err(|error| {
        PgCiStarterError::CorruptRun(format!("encode expected workflow input: {error}"))
    })?;
    if wf_type != CI_PIPELINE_WF_TYPE
        || wf_version != pin.version
        || idem_key != format!("ci:{}", record.run_id)
        || input != expected_input
        || !matches!(state.as_str(), "running" | "waiting")
        || budget.is_some()
        || correlation_id != record.wf_run_id
        || causation_id.is_some()
        || caused_by.is_some()
        || depth != 0
        || partition != partition_for_run_id(&record.wf_run_id)
    {
        return Err(PgCiStarterError::CorruptRun(format!(
            "existing workflow row diverges from queued run authority (wf_type={wf_type}, idem_key={idem_key}, state={state})"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    fn canonical_input() -> Vec<ArtifactRef> {
        vec![
            ci_artifact_ref("acme", &format!("snapshot-blake3:{}", "a".repeat(64))),
            ci_run_ref("acme", "10000000-0000-0000-0000-000000000001"),
        ]
    }

    #[test]
    fn claimed_input_round_trips_exact_snapshot_tenant_and_run() {
        let input = canonical_input();
        let decoded = decode_ci_claimed_input(&tenant(), &input).expect("canonical claimed input");
        assert_eq!(decoded.tenant(), &tenant());
        assert_eq!(
            decoded.snapshot().to_multihash_string(),
            format!("blake3:{}", "a".repeat(64))
        );
        assert_eq!(decoded.run_id(), "10000000-0000-0000-0000-000000000001");
        assert_eq!(
            input,
            vec![
                ci_artifact_ref(
                    "acme",
                    &format!("snapshot-{}", decoded.snapshot().to_multihash_string())
                ),
                ci_run_ref("acme", decoded.run_id())
            ]
        );
    }

    #[test]
    fn claimed_input_rejects_noncanonical_wrong_algorithm_scope_order_and_suffix() {
        let base = canonical_input();
        let cases = vec![
            vec![base[0].clone()],
            vec![base[1].clone(), base[0].clone()],
            vec![
                ci_artifact_ref("acme", &format!("snapshot-sha256:{}", "a".repeat(64))),
                base[1].clone(),
            ],
            vec![
                ci_artifact_ref("other", &format!("snapshot-blake3:{}", "a".repeat(64))),
                base[1].clone(),
            ],
            vec![
                base[0].clone(),
                ArtifactRef(
                    "myelin://acme/ci/run/10000000-0000-0000-0000-000000000001#step-1".into(),
                ),
            ],
            vec![
                base[0].clone(),
                ci_run_ref("acme", "10000000-0000-0000-0000-00000000000A"),
            ],
        ];
        for input in cases {
            assert!(decode_ci_claimed_input(&tenant(), &input).is_err());
        }
    }
}
