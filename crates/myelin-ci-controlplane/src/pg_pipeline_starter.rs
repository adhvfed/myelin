//! PostgreSQL-backed starter for queued CI runs.
//!
//! A starter is composed for one explicit `(tenant, region)` cell. It never discovers tenants and
//! never scans a region globally. The selected `ci_run` row, its canonical `ci_job` DAG ledger, the
//! pre-minted `workflow_run`, and the `queued -> running` transition are committed on one caller-owned
//! PostgreSQL transaction.
//!
//! This module is deliberately not composed by the service main yet. The v1 plan and current CI
//! schemas do not durably provide the runner lane/labels, timeout and resource authority, egress and
//! workspace grants, per-run token, metering reservation, or check-attempt/context facts required to
//! execute/report a job. This starter now freezes restart-safe version-1 stage/DAG identity only;
//! registering a production body or enabling runner dispatch remains gated on the other authoritative
//! fields and on replacing the main's no-op runner hooks.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use myelin_events::{HandlerTx, IdMinter};
use myelin_flow::{partition_for_run_id, PgFlowExecutor, RunId, StartSpec, CI_PIPELINE_WF_TYPE};
use myelin_refs::ArtifactRef;
use myelin_storage::{BlobStore, ContentHash, HashAlgo};
use myelin_tenancy::{Region, TenantId};
use sqlx::{PgPool, Row};

use crate::ci_run_store::CiRunRecord;
use crate::run_plan::{load_resolved_run_plan, PreparedRunPlan, RunPlanError};
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

const LOCK_EXACT_CI_JOB_LEDGER: &str = "\
SELECT tenant_id, region, job_id, run_id, stage, name, needs, matrix_key, spec_ref,
       state, attempt, result_summary
FROM ci_job
WHERE tenant_id = $1 AND region = $2
  AND (run_id = $3 OR job_id = ANY($4::uuid[]))
FOR UPDATE";

/// Frozen BLAKE3 derive-key context for the version-1 canonical CI DAG-node identity.
///
/// The hash input is four ordered `u64::to_be_bytes()` length-prefixed frames: tenant id, the
/// RFC-ordered 16 bytes of `ci_run.run_id`, the concrete resolved job name, and
/// [`crate::ResolvedJobV1::matrix_identity`]. The first 16 digest bytes become an RFC 9562 UUIDv8 by setting
/// the version and variant bits. Changing any byte of this contract requires a new versioned helper.
pub const CI_JOB_ID_V1_DOMAIN: &str = "myelin.ci.job-id.v1";

/// Derive the canonical durable `ci_job.job_id` for one resolved version-1 DAG node.
///
/// The caller must pass authority read from the locked `ci_run` and validated plan. In particular,
/// `concrete_name` is the resolved node name (including any matrix suffix), never an authored alias.
pub fn ci_job_id_v1(
    tenant: &TenantId,
    run_id: sqlx::types::Uuid,
    concrete_name: &str,
    matrix_identity: &[u8],
) -> sqlx::types::Uuid {
    let mut hasher = blake3::Hasher::new_derive_key(CI_JOB_ID_V1_DOMAIN);
    for frame in [
        tenant.0.as_bytes(),
        run_id.as_bytes().as_slice(),
        concrete_name.as_bytes(),
        matrix_identity,
    ] {
        hasher.update(&(frame.len() as u64).to_be_bytes());
        hasher.update(frame);
    }
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest.as_bytes()[..16]);
    // RFC 9562: custom UUID version 8 and the RFC 4122/9562 variant (`10xx`).
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    sqlx::types::Uuid::from_bytes(bytes)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ExpectedCiJobV1 {
    tenant_id: String,
    region: String,
    job_id: sqlx::types::Uuid,
    run_id: sqlx::types::Uuid,
    stage: String,
    name: String,
    needs: Vec<sqlx::types::Uuid>,
    matrix_key: Option<serde_json::Value>,
    spec_ref: String,
    state: String,
    attempt: i32,
    result_summary: Option<serde_json::Value>,
}

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
    /// exact row before materializing its canonical DAG and starting it. The exact `ci_job` ledger,
    /// `start_with_id_on_conn` workflow, identity proof, and lifecycle update share one transaction.
    pub async fn run_once(&self) -> Result<StartQueuedOutcome, PgCiStarterError> {
        let Some(candidate) = self.preflight_candidate().await? else {
            return Ok(StartQueuedOutcome::Idle);
        };
        validate_candidate(&self.tenant, &self.region, &candidate)?;
        let prepared = load_resolved_run_plan(self.blobs.as_ref(), &candidate.record)
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
        materialize_ci_jobs_v1(&mut transaction, &record, &prepared).await?;
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

fn expected_ci_jobs_v1(
    record: &CiRunRecord,
    prepared: &PreparedRunPlan,
) -> Result<Vec<ExpectedCiJobV1>, PgCiStarterError> {
    expected_ci_jobs_v1_with(record, prepared, ci_job_id_v1)
}

fn expected_ci_jobs_v1_with<F>(
    record: &CiRunRecord,
    prepared: &PreparedRunPlan,
    mut derive_id: F,
) -> Result<Vec<ExpectedCiJobV1>, PgCiStarterError>
where
    F: FnMut(&TenantId, sqlx::types::Uuid, &str, &[u8]) -> sqlx::types::Uuid,
{
    let tenant = TenantId(record.tenant_id.clone());
    let expected_snapshot = format!(
        "myelin://{}/ci/snapshot/{}",
        tenant.0,
        prepared.content_hash().to_multihash_string()
    );
    if prepared.tenant() != &tenant || record.definition_snapshot != expected_snapshot {
        return Err(PgCiStarterError::CorruptRun(
            "prepared plan provenance diverges from the locked ci_run".into(),
        ));
    }
    let run_id = sqlx::types::Uuid::parse_str(&record.run_id).map_err(|error| {
        PgCiStarterError::CorruptRun(format!("locked ci_run.run_id is not a UUID: {error}"))
    })?;

    // Pass one freezes every node id before any dependency is translated. The BTreeMap preserves
    // the plan's canonical name language; the set catches a digest truncation collision loudly.
    let mut ids_by_name = BTreeMap::new();
    let mut unique_ids = BTreeSet::new();
    for job in &prepared.plan().jobs {
        let job_id = derive_id(&tenant, run_id, &job.name, &job.matrix_identity());
        if !unique_ids.insert(job_id) {
            return Err(PgCiStarterError::CorruptRun(format!(
                "deterministic version-1 ci_job id collision at resolved node `{}`",
                job.name
            )));
        }
        if ids_by_name.insert(job.name.clone(), job_id).is_some() {
            return Err(PgCiStarterError::CorruptRun(format!(
                "validated plan repeated resolved node `{}`",
                job.name
            )));
        }
    }

    let mut expected = Vec::with_capacity(prepared.plan().jobs.len());
    for job in &prepared.plan().jobs {
        // V1 COMPATIBILITY CONTRACT: `stage` is the concrete resolved node name because the v1 wire
        // does not preserve a separate authored-stage identity. A future distinction requires v2.
        let stage = job.name.clone();
        // Needs are validated as strictly name-sorted by the run-plan loader. Translate in that exact
        // canonical order; UUID byte order is deliberately not a second ordering authority.
        let needs = job
            .needs
            .iter()
            .map(|need| {
                ids_by_name.get(need).copied().ok_or_else(|| {
                    PgCiStarterError::CorruptRun(format!(
                        "validated node `{}` needs unmapped node `{need}`",
                        job.name
                    ))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let matrix_key = if job.matrix_key.is_empty() {
            None
        } else {
            Some(serde_json::to_value(&job.matrix_key).map_err(|error| {
                PgCiStarterError::CorruptRun(format!(
                    "encode matrix identity for `{}`: {error}",
                    job.name
                ))
            })?)
        };
        expected.push(ExpectedCiJobV1 {
            tenant_id: record.tenant_id.clone(),
            region: record.region.clone(),
            job_id: ids_by_name[&job.name],
            run_id,
            stage,
            name: job.name.clone(),
            needs,
            matrix_key,
            // V1 COMPATIBILITY CONTRACT: this is the whole locked resolved-plan CAS object that
            // contains the job, not a per-job executable JobSpec. Runtime JobSpec authority remains
            // deliberately disabled and belongs to the later `ci_job_spec` dispatch boundary.
            spec_ref: record.definition_snapshot.clone(),
            state: "queued".into(),
            attempt: 1,
            result_summary: None,
        });
    }
    Ok(expected)
}

async fn materialize_ci_jobs_v1(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    record: &CiRunRecord,
    prepared: &PreparedRunPlan,
) -> Result<(), PgCiStarterError> {
    let expected = expected_ci_jobs_v1(record, prepared)?;
    for job in &expected {
        sqlx::query(
            "INSERT INTO ci_job (tenant_id, region, job_id, run_id, stage, name, needs, \
                                matrix_key, spec_ref, state, attempt, result_summary) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'queued', 1, NULL) \
             ON CONFLICT (tenant_id, job_id) DO NOTHING",
        )
        .bind(&job.tenant_id)
        .bind(&job.region)
        .bind(job.job_id)
        .bind(job.run_id)
        .bind(&job.stage)
        .bind(&job.name)
        .bind(&job.needs)
        .bind(&job.matrix_key)
        .bind(&job.spec_ref)
        .execute(&mut **transaction)
        .await
        .map_err(|error| PgCiStarterError::Database(format!("materialize ci_job: {error}")))?;
    }

    let expected_ids = expected.iter().map(|job| job.job_id).collect::<Vec<_>>();
    let run_id = expected.first().map(|job| job.run_id).ok_or_else(|| {
        PgCiStarterError::CorruptRun("validated run plan materialized no ci_job rows".into())
    })?;
    let rows = sqlx::query(LOCK_EXACT_CI_JOB_LEDGER)
        .bind(&record.tenant_id)
        .bind(&record.region)
        .bind(run_id)
        .bind(&expected_ids)
        .fetch_all(&mut **transaction)
        .await
        .map_err(|error| {
            PgCiStarterError::Database(format!("lock exact ci_job ledger: {error}"))
        })?;

    let mut actual_by_id = BTreeMap::new();
    for row in rows {
        let actual = decode_ci_job(&row)?;
        let id = actual.job_id;
        if actual_by_id.insert(id, actual).is_some() {
            return Err(PgCiStarterError::CorruptRun(format!(
                "durable ci_job ledger repeated job id `{id}`"
            )));
        }
    }
    if actual_by_id.len() != expected.len() {
        return Err(PgCiStarterError::CorruptRun(format!(
            "durable ci_job ledger has {} rows but resolved plan requires {}",
            actual_by_id.len(),
            expected.len()
        )));
    }
    for job in expected {
        match actual_by_id.get(&job.job_id) {
            Some(actual) if actual == &job => {}
            Some(_) => {
                return Err(PgCiStarterError::CorruptRun(format!(
                    "durable ci_job `{}` diverges from locked version-1 run-plan authority",
                    job.job_id
                )))
            }
            None => {
                return Err(PgCiStarterError::CorruptRun(format!(
                    "durable ci_job `{}` is missing after materialization",
                    job.job_id
                )))
            }
        }
    }
    Ok(())
}

fn decode_ci_job(row: &sqlx::postgres::PgRow) -> Result<ExpectedCiJobV1, PgCiStarterError> {
    macro_rules! field {
        ($name:literal) => {
            row.try_get($name).map_err(|error| {
                PgCiStarterError::CorruptRun(format!(
                    "cannot decode authoritative ci_job `{}` column: {error}",
                    $name
                ))
            })?
        };
    }
    Ok(ExpectedCiJobV1 {
        tenant_id: field!("tenant_id"),
        region: field!("region"),
        job_id: field!("job_id"),
        run_id: field!("run_id"),
        stage: field!("stage"),
        name: field!("name"),
        needs: field!("needs"),
        matrix_key: field!("matrix_key"),
        spec_ref: field!("spec_ref"),
        state: field!("state"),
        attempt: field!("attempt"),
        result_summary: field!("result_summary"),
    })
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
    use crate::ResolvedJobV1;

    const PINNED_IMAGE: &str =
        "registry.example/build@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    fn canonical_input() -> Vec<ArtifactRef> {
        vec![
            ci_artifact_ref("acme", &format!("snapshot-blake3:{}", "a".repeat(64))),
            ci_run_ref("acme", "10000000-0000-0000-0000-000000000001"),
        ]
    }

    fn prepared_plan(
        tenant_id: &str,
        run_id: &str,
        jobs: Vec<ResolvedJobV1>,
    ) -> (CiRunRecord, PreparedRunPlan) {
        let tenant = TenantId(tenant_id.into());
        let plan = crate::ResolvedRunPlanV1 {
            schema_version: 1,
            jobs,
        };
        let bytes = plan.canonical_bytes().expect("canonical test plan");
        let blobs = myelin_storage::FsBlobStore::new();
        let hash = blobs.put(&tenant, &bytes).expect("store test plan");
        let record = CiRunRecord {
            tenant_id: tenant_id.into(),
            run_id: run_id.into(),
            region: "fr-par".into(),
            project_id: "22222222-2222-2222-2222-222222222222".into(),
            pipeline_id: "33333333-3333-3333-3333-333333333333".into(),
            wf_run_id: "20000000-0000-0000-0000-000000000001".into(),
            repo_ref: Some("repo-1".into()),
            commit_oid: Some("deadbeef".into()),
            cause_event_id: None,
            definition_snapshot: format!(
                "myelin://{tenant_id}/ci/snapshot/{}",
                hash.to_multihash_string()
            ),
            trigger_kind: "push".into(),
            trust_tier: "trusted".into(),
            state: "queued".into(),
            correlation_id: run_id.into(),
        };
        let prepared = load_resolved_run_plan(&blobs, &record).expect("load prepared test plan");
        (record, prepared)
    }

    fn resolved_job(
        name: &str,
        needs: Vec<&str>,
        matrix_key: BTreeMap<String, String>,
    ) -> ResolvedJobV1 {
        ResolvedJobV1 {
            name: name.into(),
            image: PINNED_IMAGE.into(),
            command: vec!["/bin/build".into()],
            needs: needs.into_iter().map(str::to_string).collect(),
            is_generator: false,
            matrix_key,
        }
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

    #[test]
    fn job_id_v1_known_answers_pin_domain_framing_version_and_variant() {
        let tenant = TenantId("acme".into());
        let run_id = sqlx::types::Uuid::parse_str("10000000-0000-0000-0000-000000000001")
            .expect("test UUID");
        let empty_matrix = ResolvedJobV1 {
            name: "build".into(),
            image: PINNED_IMAGE.into(),
            command: vec!["/bin/build".into()],
            needs: vec![],
            is_generator: false,
            matrix_key: BTreeMap::new(),
        };
        let mut axes = BTreeMap::new();
        axes.insert("arch".into(), "x86_64".into());
        axes.insert("os".into(), "linux".into());
        let matrix = resolved_job("test-linux-x86_64", vec!["build"], axes);

        let first = ci_job_id_v1(
            &tenant,
            run_id,
            &empty_matrix.name,
            &empty_matrix.matrix_identity(),
        );
        let second = ci_job_id_v1(&tenant, run_id, &matrix.name, &matrix.matrix_identity());
        assert_eq!(first.to_string(), "114cfd80-99c2-8e5b-a51d-008f7176782a");
        assert_eq!(second.to_string(), "f7b98ab0-9967-8d37-95a9-3ef7f3cc95e3");
        for id in [first, second] {
            assert_eq!(id.as_bytes()[6] >> 4, 8, "RFC 9562 UUID version 8");
            assert_eq!(id.as_bytes()[8] >> 6, 2, "RFC variant bits are 10");
        }
    }

    #[test]
    fn exact_job_ledger_lock_is_region_explicit_and_bind_ordered() {
        assert!(LOCK_EXACT_CI_JOB_LEDGER.contains("tenant_id = $1 AND region = $2"));
        assert!(LOCK_EXACT_CI_JOB_LEDGER.contains("run_id = $3"));
        assert!(LOCK_EXACT_CI_JOB_LEDGER.contains("job_id = ANY($4::uuid[])"));
        assert!(LOCK_EXACT_CI_JOB_LEDGER.ends_with("FOR UPDATE"));
    }

    #[test]
    fn job_id_v1_length_frames_and_every_authoritative_field_are_load_bearing() {
        let run = sqlx::types::Uuid::parse_str("10000000-0000-0000-0000-000000000001").unwrap();
        let other_run =
            sqlx::types::Uuid::parse_str("10000000-0000-0000-0000-000000000002").unwrap();
        let matrix_a = resolved_job("build", vec![], BTreeMap::from([("ab".into(), "c".into())]));
        let matrix_b = resolved_job("build", vec![], BTreeMap::from([("a".into(), "bc".into())]));
        let base = ci_job_id_v1(
            &TenantId("ab".into()),
            run,
            "c",
            &matrix_a.matrix_identity(),
        );
        let variants = [
            ci_job_id_v1(
                &TenantId("a".into()),
                run,
                "bc",
                &matrix_a.matrix_identity(),
            ),
            ci_job_id_v1(
                &TenantId("ab".into()),
                other_run,
                "c",
                &matrix_a.matrix_identity(),
            ),
            ci_job_id_v1(
                &TenantId("ab".into()),
                run,
                "different",
                &matrix_a.matrix_identity(),
            ),
            ci_job_id_v1(
                &TenantId("ab".into()),
                run,
                "c",
                &matrix_b.matrix_identity(),
            ),
        ];
        assert!(variants.into_iter().all(|candidate| candidate != base));
    }

    #[test]
    fn v1_materialization_freezes_stage_snapshot_needs_and_refuses_id_collision() {
        let mut matrix = BTreeMap::new();
        matrix.insert("os".into(), "linux".into());
        let (record, prepared) = prepared_plan(
            "acme",
            "10000000-0000-0000-0000-000000000001",
            vec![
                resolved_job("build", vec![], BTreeMap::new()),
                resolved_job("test-linux", vec!["build"], matrix),
            ],
        );
        let jobs = expected_ci_jobs_v1(&record, &prepared).expect("materialized identities");
        assert_eq!(jobs.len(), 2);
        assert!(jobs.iter().all(|job| job.stage == job.name));
        assert!(jobs
            .iter()
            .all(|job| job.spec_ref == record.definition_snapshot));
        assert_eq!(jobs[1].needs, vec![jobs[0].job_id]);
        assert_eq!(jobs[0].matrix_key, None);
        assert_eq!(jobs[1].matrix_key, Some(serde_json::json!({"os": "linux"})));

        let error =
            expected_ci_jobs_v1_with(&record, &prepared, |_, _, _, _| sqlx::types::Uuid::nil())
                .expect_err("two nodes may not collapse to one truncated digest");
        assert!(error.to_string().contains("id collision"));
    }
}
