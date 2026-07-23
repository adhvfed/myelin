//! PostgreSQL-backed starter for queued CI runs.
//!
//! A starter is composed for one explicit `(tenant, region)` cell. It never discovers tenants and
//! never scans a region globally. The selected `ci_run` row, its canonical `ci_job` DAG ledger, the
//! pre-minted `workflow_run`, and the `queued -> running` transition are committed on one caller-owned
//! PostgreSQL transaction.
//!
//! The service main composes this starter through [`PgCiRunStarterFactory`] (built at the composition
//! root by [`crate::ci_run_starter_factory`]), behind the SAME explicit `MYELIN_CI_RUNNER=1`
//! activation seam the runner lane uses. Unset / `0` keeps the complete runner host dormant. A fresh
//! start accepts only a canonical V2 plan and requires an explicit
//! policy-aware [`CiLaunchAuthorityMaterializer`]; the production default refuses every fresh launch.
//! The starter co-commits the immutable runtime grants, check attempts, canonical job ledger, workflow,
//! and lifecycle transition, while an exact retry validates and reuses the frozen manifest without
//! consulting mutable current policy. A fresh start also co-emits one manifest-bound in-progress
//! check fact per authored context through the durable outbox. The region-wide poller is composed
//! separately and is driven only by that same opt-in host.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use myelin_events::{
    derive_envelope_from_persisted_cause, Actor, CausedBy, CorrelationId, EmitContext, EventId,
    HandlerTx, IdMinter, PersistedEventCause, Timestamp,
};
use myelin_flow::{partition_for_run_id, PgFlowExecutor, RunId, StartSpec, CI_PIPELINE_WF_TYPE};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs::ArtifactRef;
use myelin_storage::pgrelay::PgRelay;
use myelin_storage::{BlobStore, ContentHash, DurableCostLedger};
use myelin_tenancy::{Region, TenantId};
use sqlx::{PgPool, Row};

use crate::check_emitter::BUMP_CHECK_ATTEMPT_SQL;
use crate::ci_drive_manifest::{
    ci_check_context_v1, CiDriveManifestError, CiDriveManifestStore, CiDriveManifestV1,
    CiLaunchAuthorityV1, CiManifestTrustTierV1, CiManifestWorkspaceV1, GrantedCiJobV1,
};
use crate::ci_run_store::CiRunRecord;
use crate::ci_run_supersession::{HeadDecision, PgCiRunSupersession};
use crate::run_plan::{
    load_launch_run_plan_v2, PreparedRunPlanV2, RunPlanError, RUN_PLAN_SCHEMA_V2,
};
use crate::surfacing::{ci_artifact_ref, ci_run_ref};

const SELECT_QUEUED_RUN: &str = "\
SELECT tenant_id, run_id::text AS run_id, region, project_id::text AS project_id,
       pipeline_id::text AS pipeline_id, wf_run_id::text AS wf_run_id, repo_ref, commit_oid,
       cause_event_id, cause_depth, caused_by, definition_snapshot, trigger_kind, concurrency_group,
       pr_head_generation,
       triggered_by, trust_tier, state,
       cost_settled, correlation_id,
       to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS created_at,
       finished_at::text AS finished_at
FROM ci_run
WHERE tenant_id = $1 AND region = $2 AND state = 'queued'
ORDER BY created_at, run_id
LIMIT 1";

const LOCK_EXACT_QUEUED_RUN: &str = "\
SELECT tenant_id, run_id::text AS run_id, region, project_id::text AS project_id,
       pipeline_id::text AS pipeline_id, wf_run_id::text AS wf_run_id, repo_ref, commit_oid,
       cause_event_id, cause_depth, caused_by, definition_snapshot, trigger_kind, concurrency_group,
       pr_head_generation,
       triggered_by, trust_tier, state,
       cost_settled, correlation_id,
       to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS created_at,
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
/// V2 identity also binds the authored stage separately from the concrete matrix-expanded name.
pub const CI_JOB_ID_V2_DOMAIN: &str = "myelin.ci.job-id.v2";
/// Frozen deterministic event-id domain for a manifest-bound initial check fact.
pub const CI_INITIAL_CHECK_EVENT_V1_DOMAIN: &str = "myelin.ci.initial-check-event.v1";

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

pub fn ci_job_id_v2(
    tenant: &TenantId,
    run_id: sqlx::types::Uuid,
    stage: &str,
    concrete_name: &str,
    matrix_identity: &[u8],
) -> sqlx::types::Uuid {
    let mut hasher = blake3::Hasher::new_derive_key(CI_JOB_ID_V2_DOMAIN);
    for frame in [
        tenant.0.as_bytes(),
        run_id.as_bytes().as_slice(),
        stage.as_bytes(),
        concrete_name.as_bytes(),
        matrix_identity,
    ] {
        hasher.update(&(frame.len() as u64).to_be_bytes());
        hasher.update(frame);
    }
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest.as_bytes()[..16]);
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

/// A policy-aware server adapter that converts one verified customer plan into explicit runtime
/// grants. The adapter is invoked only after the starter locks the exact queued run. Implementations
/// must be deterministic and retry-safe for `record.run_id`: external reservation/token services
/// cannot join the PostgreSQL transaction, so a rolled-back retry must resolve the same stable handles
/// and must not create an irreversible duplicate. There is no permissive implementation.
pub trait CiLaunchAuthorityMaterializer: Send + Sync {
    fn materialize<'a>(
        &'a self,
        record: &'a CiRunRecord,
        prepared: &'a PreparedRunPlanV2,
        definition: &'a CiWorkflowDefinitionPin,
    ) -> Pin<
        Box<dyn Future<Output = Result<CiLaunchAuthorityV1, CiLaunchAuthorityError>> + Send + 'a>,
    >;

    /// Materialize on the starter's tenant-scoped transaction. The default preserves truly external
    /// authority providers; an in-database reservation adapter overrides this so reservation and
    /// manifest/workflow/job state co-commit.
    fn materialize_in_tx<'a>(
        &'a self,
        _conn: &'a mut sqlx::PgConnection,
        record: &'a CiRunRecord,
        prepared: &'a PreparedRunPlanV2,
        definition: &'a CiWorkflowDefinitionPin,
    ) -> Pin<
        Box<dyn Future<Output = Result<CiLaunchAuthorityV1, CiLaunchAuthorityError>> + Send + 'a>,
    > {
        self.materialize(record, prepared, definition)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiLaunchAuthorityError(pub String);

impl std::fmt::Display for CiLaunchAuthorityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CI launch authority refused: {}", self.0)
    }
}

impl std::error::Error for CiLaunchAuthorityError {}

#[derive(Clone, Debug, Default)]
struct UnavailableCiLaunchAuthority;

impl CiLaunchAuthorityMaterializer for UnavailableCiLaunchAuthority {
    fn materialize<'a>(
        &'a self,
        _record: &'a CiRunRecord,
        _prepared: &'a PreparedRunPlanV2,
        _definition: &'a CiWorkflowDefinitionPin,
    ) -> Pin<
        Box<dyn Future<Output = Result<CiLaunchAuthorityV1, CiLaunchAuthorityError>> + Send + 'a>,
    > {
        Box::pin(async {
            Err(CiLaunchAuthorityError(
                "no policy-aware launch-authority adapter is configured".into(),
            ))
        })
    }
}

/// Strict, Flow-safe decoding of the two references persisted as a CI workflow's claimed input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimedCiInput {
    tenant: TenantId,
    manifest_digest: String,
    ci_run_id: String,
}

impl ClaimedCiInput {
    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    pub fn manifest_digest(&self) -> &str {
        &self.manifest_digest
    }

    pub fn ci_run_id(&self) -> &str {
        &self.ci_run_id
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

/// Decode exactly `[ci/artifact/drive-manifest-blake3:<64-lower-hex>, ci/run/<uuid>]`. No extra
/// reference, suffix, foreign tenant, abbreviated digest, or different artifact class is accepted.
pub fn decode_ci_claimed_input(
    expected_tenant: &TenantId,
    input: &[ArtifactRef],
) -> Result<ClaimedCiInput, ClaimedCiInputError> {
    if input.len() != 2 {
        return Err(ClaimedCiInputError(
            "expected exactly drive-manifest artifact then CI run reference".into(),
        ));
    }
    let manifest_ref = myelin_refs::parse_scoped(&input[0].0)
        .map_err(|error| ClaimedCiInputError(format!("manifest reference: {error}")))?;
    let run_ref = myelin_refs::parse_scoped(&input[1].0)
        .map_err(|error| ClaimedCiInputError(format!("run reference: {error}")))?;
    if manifest_ref.tenant != *expected_tenant
        || run_ref.tenant != *expected_tenant
        || manifest_ref.sub.is_some()
        || run_ref.sub.is_some()
        || manifest_ref.subsystem != "ci"
        || manifest_ref.type_ != "artifact"
        || run_ref.subsystem != "ci"
        || run_ref.type_ != "run"
    {
        return Err(ClaimedCiInputError(
            "references must be unsubscripted canonical CI artifact/run refs for the expected tenant"
                .into(),
        ));
    }
    let manifest_digest = manifest_ref
        .id
        .strip_prefix("drive-manifest-")
        .ok_or_else(|| {
            ClaimedCiInputError("manifest artifact id lacks `drive-manifest-` class".into())
        })?;
    let Some(digest_hex) = manifest_digest.strip_prefix("blake3:") else {
        return Err(ClaimedCiInputError(
            "manifest must use a BLAKE3 digest".into(),
        ));
    };
    if digest_hex.len() != 64
        || !digest_hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ClaimedCiInputError(
            "manifest must be a canonical lowercase 32-byte BLAKE3 digest".into(),
        ));
    }
    let ci_run_id = sqlx::types::Uuid::parse_str(&run_ref.id)
        .map_err(|error| ClaimedCiInputError(format!("run id is not a UUID: {error}")))?
        .to_string();
    let canonical_manifest = ci_artifact_ref(
        &expected_tenant.0,
        &format!("drive-manifest-{manifest_digest}"),
    );
    let canonical_run = ci_run_ref(&expected_tenant.0, &ci_run_id);
    if input[0] != canonical_manifest || input[1] != canonical_run {
        return Err(ClaimedCiInputError(
            "claimed references are parseable but not byte-canonical".into(),
        ));
    }
    Ok(ClaimedCiInput {
        tenant: expected_tenant.clone(),
        manifest_digest: manifest_digest.into(),
        ci_run_id,
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
    /// The queued row was atomically terminalized because a higher producer generation exists.
    Superseded { run_id: String },
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
    LaunchAuthority(CiLaunchAuthorityError),
    Manifest(CiDriveManifestError),
    Supersession(crate::CiRunSupersessionError),
    SupersessionUnavailable,
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
            Self::LaunchAuthority(error) => write!(f, "{error}"),
            Self::Manifest(error) => write!(f, "CI drive manifest refused: {error}"),
            Self::Supersession(error) => write!(f, "CI run supersession refused: {error}"),
            Self::SupersessionUnavailable => f.write_str(
                "pull-request CI start refused: no durable run-supersession authority is configured",
            ),
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
    launch_authority: Arc<dyn CiLaunchAuthorityMaterializer>,
    supersession: Option<PgCiRunSupersession>,
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
        Self::new_with_authority(
            pool,
            rt,
            minter,
            tenant,
            region,
            blobs,
            definition,
            Arc::new(UnavailableCiLaunchAuthority),
        )
    }

    /// Construct a starter with an explicit policy-aware authority adapter but no PR supersession
    /// authority. This is useful for non-PR tests; any pull-request row fails closed.
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_authority(
        pool: PgPool,
        rt: tokio::runtime::Handle,
        minter: Arc<dyn IdMinter>,
        tenant: TenantId,
        region: Region,
        blobs: Arc<dyn BlobStore + Send + Sync>,
        definition: CiWorkflowDefinitionPin,
        launch_authority: Arc<dyn CiLaunchAuthorityMaterializer>,
    ) -> Result<Self, PgCiStarterError> {
        Self::new_with_components(
            pool,
            rt,
            minter,
            tenant,
            region,
            blobs,
            definition,
            launch_authority,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_with_authority_and_supersession(
        pool: PgPool,
        rt: tokio::runtime::Handle,
        minter: Arc<dyn IdMinter>,
        tenant: TenantId,
        region: Region,
        blobs: Arc<dyn BlobStore + Send + Sync>,
        definition: CiWorkflowDefinitionPin,
        launch_authority: Arc<dyn CiLaunchAuthorityMaterializer>,
        supersession: PgCiRunSupersession,
    ) -> Result<Self, PgCiStarterError> {
        Self::new_with_components(
            pool,
            rt,
            minter,
            tenant,
            region,
            blobs,
            definition,
            launch_authority,
            Some(supersession),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new_with_components(
        pool: PgPool,
        rt: tokio::runtime::Handle,
        minter: Arc<dyn IdMinter>,
        tenant: TenantId,
        region: Region,
        blobs: Arc<dyn BlobStore + Send + Sync>,
        definition: CiWorkflowDefinitionPin,
        launch_authority: Arc<dyn CiLaunchAuthorityMaterializer>,
        supersession: Option<PgCiRunSupersession>,
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
            launch_authority,
            supersession,
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
        if let Some(outcome) = self.cancel_if_already_superseded(&candidate).await? {
            return Ok(outcome);
        }
        let manifest_store =
            CiDriveManifestStore::new(self.pool.clone(), self.tenant.clone(), self.region.clone())
                .map_err(PgCiStarterError::Manifest)?;
        let manifest_preflight = manifest_store
            .load_by_identity(&candidate.record.wf_run_id, &candidate.record.run_id)
            .await
            .map_err(PgCiStarterError::Manifest)?;
        // A frozen manifest is the complete replay authority. Only a genuinely fresh launch reads
        // the source CAS; exact repair must survive source retention and current-policy changes.
        let prepared = if manifest_preflight.is_some() {
            None
        } else {
            Some(
                load_launch_run_plan_v2(self.blobs.as_ref(), &candidate.record)
                    .map_err(PgCiStarterError::Plan)?,
            )
        };

        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| PgCiStarterError::Database(format!("begin: {error}")))?;
        scope_transaction(&mut transaction, &self.tenant, &self.region).await?;
        if let Some((group, _)) = pr_supersession_identity(&candidate.record) {
            self.supersession()?
                .lock_group_on_conn(&mut transaction, group)
                .await
                .map_err(PgCiStarterError::Supersession)?;
        }
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
        if let Some((group, generation)) = pr_supersession_identity(&locked.record) {
            let supersession = self.supersession()?;
            if supersession
                .classify_on_conn(&mut transaction, &locked.record.run_id, group, generation)
                .await
                .map_err(PgCiStarterError::Supersession)?
                == HeadDecision::Superseded
            {
                let run_id = locked.record.run_id.clone();
                supersession
                    .cancel_stale_queued_on_conn(
                        &mut transaction,
                        &locked.record.run_id,
                        &locked.record.wf_run_id,
                    )
                    .await
                    .map_err(PgCiStarterError::Supersession)?;
                transaction.commit().await.map_err(|error| {
                    PgCiStarterError::Database(format!("commit stale run cancellation: {error}"))
                })?;
                return Ok(StartQueuedOutcome::Superseded { run_id });
            }
        }
        let started_at = locked.created_at.clone();
        let record = locked.record;
        let replay = lock_existing_exact_workflow(&mut transaction, &record).await?;
        let existing = manifest_store
            .load_by_identity_on_conn(&mut transaction, &record.wf_run_id, &record.run_id)
            .await
            .map_err(PgCiStarterError::Manifest)?;
        let (manifest, manifest_digest, mut expected_jobs, definition) =
            if let Some((existing, digest)) = existing {
                if !replay {
                    return Err(PgCiStarterError::CorruptRun(
                        "drive manifest exists without its atomically-started workflow".into(),
                    ));
                }
                validate_replay_manifest(&record, &started_at, &existing)?;
                verify_replay_attempts(&mut transaction, &record, &existing).await?;
                let definition = CiWorkflowDefinitionPin::new(
                    existing.workflow_definition_version,
                    existing.workflow_code_hash.clone(),
                )?;
                validate_definition_pin(&mut transaction, &definition, true).await?;
                let expected_jobs = expected_ci_jobs_from_manifest(&record, &existing)?;
                (existing, digest, expected_jobs, definition)
            } else {
                if replay {
                    return Err(PgCiStarterError::CorruptRun(
                        "existing CI workflow has no immutable drive manifest".into(),
                    ));
                }
                let prepared = prepared.as_ref().ok_or_else(|| {
                    PgCiStarterError::CorruptRun(
                        "drive manifest disappeared after immutable preflight".into(),
                    )
                })?;
                validate_definition_pin(&mut transaction, &self.definition, false).await?;
                // The exact queued row is locked before consulting policy. Same-database reservation
                // adapters co-commit here; truly external adapters must remain retry-safe by run identity.
                let authority = self
                    .launch_authority
                    .materialize_in_tx(&mut transaction, &record, prepared, &self.definition)
                    .await
                    .map_err(PgCiStarterError::LaunchAuthority)?;
                let expected_jobs = expected_ci_jobs_v2(&record, prepared)?;
                let granted_jobs = granted_jobs_v2(&record, prepared, &expected_jobs, &authority)?;
                let contexts = granted_jobs
                    .iter()
                    .map(|job| job.check_context.clone())
                    .collect::<BTreeSet<_>>();
                let attempts =
                    allocate_check_attempts(&mut transaction, &record, &contexts).await?;
                let manifest = build_drive_manifest_v1(
                    &record,
                    prepared,
                    &self.definition,
                    &authority,
                    granted_jobs,
                    attempts,
                    &started_at,
                )?;
                let digest = manifest_store
                    .insert_on_conn(&mut transaction, &manifest)
                    .await
                    .map_err(PgCiStarterError::Manifest)?;
                (manifest, digest, expected_jobs, self.definition.clone())
            };
        let workflow_input = workflow_input(&record, &manifest_digest)?;
        for job in &mut expected_jobs {
            job.spec_ref = workflow_input[0].0.clone();
        }
        materialize_ci_jobs(
            &mut transaction,
            &record,
            &expected_jobs,
            &workflow_input[0].0,
            replay,
        )
        .await?;
        if !replay {
            emit_initial_checks(&mut transaction, &record, &manifest, &manifest_digest).await?;
        }
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
        verify_started_workflow(&mut transaction, &record, &workflow_input, &definition).await?;
        if manifest.digest().map_err(PgCiStarterError::Manifest)? != manifest_digest {
            return Err(PgCiStarterError::CorruptRun(
                "co-committed manifest digest changed before workflow verification".into(),
            ));
        }

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
        if let Some((group, generation)) = pr_supersession_identity(&record) {
            self.supersession()?
                .cancel_older_on_conn(&mut transaction, &record.run_id, group, generation)
                .await
                .map_err(PgCiStarterError::Supersession)?;
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

    async fn cancel_if_already_superseded(
        &self,
        candidate: &StarterCandidate,
    ) -> Result<Option<StartQueuedOutcome>, PgCiStarterError> {
        let Some((group, generation)) = pr_supersession_identity(&candidate.record) else {
            return Ok(None);
        };
        let supersession = self.supersession()?;
        let mut transaction =
            self.pool.begin().await.map_err(|error| {
                PgCiStarterError::Database(format!("begin head guard: {error}"))
            })?;
        scope_transaction(&mut transaction, &self.tenant, &self.region).await?;
        supersession
            .lock_group_on_conn(&mut transaction, group)
            .await
            .map_err(PgCiStarterError::Supersession)?;
        let tenant_id = &self.tenant.0;
        let row = sqlx::query(LOCK_EXACT_QUEUED_RUN)
            .bind(tenant_id)
            .bind(&self.region.0)
            .bind(&candidate.record.run_id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|error| {
                PgCiStarterError::Database(format!("lock head-guard candidate: {error}"))
            })?;
        let Some(row) = row else {
            transaction.rollback().await.map_err(|error| {
                PgCiStarterError::Database(format!("rollback lost head-guard candidate: {error}"))
            })?;
            return Ok(Some(StartQueuedOutcome::Idle));
        };
        let locked = decode_candidate(&row)?;
        if &locked != candidate {
            return Err(PgCiStarterError::CorruptRun(
                "authoritative ci_run changed before head-generation guard".into(),
            ));
        }
        let decision = supersession
            .classify_on_conn(
                &mut transaction,
                &candidate.record.run_id,
                group,
                generation,
            )
            .await
            .map_err(PgCiStarterError::Supersession)?;
        if decision == HeadDecision::Superseded {
            supersession
                .cancel_stale_queued_on_conn(
                    &mut transaction,
                    &candidate.record.run_id,
                    &candidate.record.wf_run_id,
                )
                .await
                .map_err(PgCiStarterError::Supersession)?;
            transaction.commit().await.map_err(|error| {
                PgCiStarterError::Database(format!("commit head-guard cancellation: {error}"))
            })?;
            return Ok(Some(StartQueuedOutcome::Superseded {
                run_id: candidate.record.run_id.clone(),
            }));
        }
        transaction.commit().await.map_err(|error| {
            PgCiStarterError::Database(format!("commit current-head guard: {error}"))
        })?;
        Ok(None)
    }

    fn supersession(&self) -> Result<&PgCiRunSupersession, PgCiStarterError> {
        self.supersession
            .as_ref()
            .ok_or(PgCiStarterError::SupersessionUnavailable)
    }
}

fn pr_supersession_identity(record: &CiRunRecord) -> Option<(&str, Option<i64>)> {
    (record.trigger_kind == "pull_request").then(|| {
        (
            record
                .concurrency_group
                .as_deref()
                .expect("validated pull-request run has a concurrency group"),
            record.pr_head_generation,
        )
    })
}

async fn emit_initial_checks(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    record: &CiRunRecord,
    manifest: &CiDriveManifestV1,
    manifest_digest: &str,
) -> Result<(), PgCiStarterError> {
    let tenant = TenantId(manifest.tenant_id.clone());
    let cause_event_id = record.cause_event_id.clone().ok_or_else(|| {
        PgCiStarterError::CorruptRun("queued run lacks durable triggering-event provenance".into())
    })?;
    let cause_depth = u32::try_from(record.cause_depth).map_err(|_| {
        PgCiStarterError::CorruptRun(
            "queued run carries causal depth outside the canonical u32 range".into(),
        )
    })?;
    let cause = PersistedEventCause {
        event_id: EventId(cause_event_id),
        correlation_id: CorrelationId(record.correlation_id.clone()),
        caused_by: record.caused_by.clone().map(CausedBy),
        depth: cause_depth,
    };
    // `started_at` is immutable run provenance carried by the payload. Envelope clocks describe
    // this transaction's actual state transition / durable acceptance, so take one PostgreSQL
    // wall-clock timestamp for the whole context set rather than pretending the run was accepted
    // when its queued row was originally created.
    // @tenant-cross-scope: PostgreSQL's clock is cell infrastructure with no tenant-owned rows;
    // the caller-owned transaction is already tenant-scoped before this read.
    let emitted_at: String = sqlx::query_scalar(
        "SELECT to_char(clock_timestamp() AT TIME ZONE 'UTC', \
                        'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"')",
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(|error| {
        PgCiStarterError::Database(format!("read initial check emission timestamp: {error}"))
    })?;
    let trust_tier = manifest_check_trust_tier(manifest.trust_tier);
    for (context, attempt) in &manifest.check_attempts {
        let emit_context = crate::check_emitter::CheckEmitContext {
            tenant: manifest.tenant_id.clone(),
            repo: manifest.repo_ref.clone(),
            commit_oid: manifest.commit_oid.clone(),
            run_ref: manifest.run_ref.clone(),
            run_attempt: *attempt,
            trust_tier,
            started_at: manifest.started_at.clone(),
            completed_at: None,
        };
        let draft = crate::check_emitter::assemble_check_status(
            &emit_context,
            crate::check_emitter::CheckProvider::Ci,
            context,
            crate::check_emitter::CheckState::InProgress,
            true,
            crate::check_emitter::CostPosture::Unsettled,
            None,
        );
        let timestamp = Timestamp(emitted_at.clone());
        let envelope = derive_envelope_from_persisted_cause(
            draft,
            EmitContext {
                event_id: initial_check_event_id(manifest, manifest_digest, context, *attempt),
                tenant: tenant.clone(),
                region: Region(manifest.region.clone()),
                actor: Actor(Principal::stub(
                    PrincipalId("ci-controlplane".into()),
                    PrincipalKind::Service,
                    tenant.clone(),
                )),
                schema_ver: 1,
                occurred_at: timestamp.clone(),
                recorded_at: timestamp,
                caused_by: None,
            },
            Some(&cause),
        );
        let aggregate = envelope.aggregate.0.clone();
        PgRelay::co_commit_in_tx(transaction, &aggregate, &envelope)
            .await
            .map_err(|error| {
                PgCiStarterError::Database(format!("emit initial check fact: {error}"))
            })?;
    }
    Ok(())
}

fn manifest_check_trust_tier(tier: CiManifestTrustTierV1) -> crate::check_emitter::TrustTier {
    match tier {
        CiManifestTrustTierV1::Trusted | CiManifestTrustTierV1::SelfHosted => {
            crate::check_emitter::TrustTier::Trusted
        }
        CiManifestTrustTierV1::UntrustedFork => crate::check_emitter::TrustTier::UntrustedFork,
    }
}

fn initial_check_event_id(
    manifest: &CiDriveManifestV1,
    manifest_digest: &str,
    context: &str,
    attempt: u32,
) -> EventId {
    let mut hasher = blake3::Hasher::new_derive_key(CI_INITIAL_CHECK_EVENT_V1_DOMAIN);
    for frame in [
        manifest.tenant_id.as_bytes(),
        manifest.ci_run_id.as_bytes(),
        manifest_digest.as_bytes(),
        context.as_bytes(),
        &attempt.to_be_bytes(),
    ] {
        hasher.update(&(frame.len() as u64).to_be_bytes());
        hasher.update(frame);
    }
    EventId(format!("ci-check-start-{}", hasher.finalize().to_hex()))
}

/// **The per-(tenant, region) starter composition seam (the region-wide poller's router).** A
/// [`PgCiPipelineStarter`] is bound to ONE explicit `(tenant, region)` cell and deliberately exposes no
/// tenant enumeration. A control-plane node, however, serves a whole region: the runner lane claims
/// across every tenant in its region, so the ci_run-poll autonomy wire must DISCOVER a queued run's
/// authoritative tenant and route it to a starter composed for exactly that tenant — it may never reuse
/// a synthetic service identity. This factory is that router: it captures the shared runtime
/// dependencies (the runtime pool, the id minter, the blob CAS, the cell region) ONCE at the
/// composition root and mints a fresh exact-cell starter for an explicit authoritative [`TenantId`] on
/// demand. It never enumerates tenants itself. Constructing the factory wraps the pool + blob client
/// only; no query runs until a minted starter's [`PgCiPipelineStarter::run_once`] is driven.
#[derive(Clone)]
pub struct PgCiRunStarterFactory {
    pool: PgPool,
    rt: tokio::runtime::Handle,
    minter: Arc<dyn IdMinter>,
    region: Region,
    blobs: Arc<dyn BlobStore + Send + Sync>,
    launch_authority: Arc<dyn CiLaunchAuthorityMaterializer>,
    supersession_ledger: Option<DurableCostLedger>,
}

impl PgCiRunStarterFactory {
    /// Test-support constructor with the explicit fail-closed authority. Production composition uses
    /// [`Self::new_with_authority_and_supersession`], so a PR lane cannot silently omit newest-head
    /// ordering and cancellation.
    /// The `region` is the residency boundary every minted starter polls (and never crosses); `blobs`
    /// is the plan CAS the resolved run plan is loaded from; `minter` mints durable workflow ids.
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(
        pool: PgPool,
        rt: tokio::runtime::Handle,
        minter: Arc<dyn IdMinter>,
        region: Region,
        blobs: Arc<dyn BlobStore + Send + Sync>,
    ) -> PgCiRunStarterFactory {
        Self::new_with_authority(
            pool,
            rt,
            minter,
            region,
            blobs,
            Arc::new(UnavailableCiLaunchAuthority),
        )
    }

    pub fn new_with_authority(
        pool: PgPool,
        rt: tokio::runtime::Handle,
        minter: Arc<dyn IdMinter>,
        region: Region,
        blobs: Arc<dyn BlobStore + Send + Sync>,
        launch_authority: Arc<dyn CiLaunchAuthorityMaterializer>,
    ) -> PgCiRunStarterFactory {
        Self::new_with_components(pool, rt, minter, region, blobs, launch_authority, None)
    }

    pub fn new_with_authority_and_supersession(
        pool: PgPool,
        rt: tokio::runtime::Handle,
        minter: Arc<dyn IdMinter>,
        region: Region,
        blobs: Arc<dyn BlobStore + Send + Sync>,
        launch_authority: Arc<dyn CiLaunchAuthorityMaterializer>,
        supersession_ledger: DurableCostLedger,
    ) -> PgCiRunStarterFactory {
        Self::new_with_components(
            pool,
            rt,
            minter,
            region,
            blobs,
            launch_authority,
            Some(supersession_ledger),
        )
    }

    fn new_with_components(
        pool: PgPool,
        rt: tokio::runtime::Handle,
        minter: Arc<dyn IdMinter>,
        region: Region,
        blobs: Arc<dyn BlobStore + Send + Sync>,
        launch_authority: Arc<dyn CiLaunchAuthorityMaterializer>,
        supersession_ledger: Option<DurableCostLedger>,
    ) -> PgCiRunStarterFactory {
        PgCiRunStarterFactory {
            pool,
            rt,
            minter,
            region,
            blobs,
            launch_authority,
            supersession_ledger,
        }
    }

    /// The cell region every minted starter is bound to (never crossed).
    pub fn region(&self) -> &Region {
        &self.region
    }

    /// **Mint the exact-cell starter for one authoritative tenant.** `tenant` MUST be read from the
    /// queued `ci_run.tenant_id` the poller discovered (never a synthetic/service identity), so the
    /// minted starter only ever polls and starts THAT tenant's queued runs in this factory's region;
    /// `definition` pins the immutable deployed body version + code hash the start is allowed to bind.
    /// Fails closed on an invalid tenant/region scope — never a widened cell.
    pub fn starter_for(
        &self,
        tenant: TenantId,
        definition: CiWorkflowDefinitionPin,
    ) -> Result<PgCiPipelineStarter, PgCiStarterError> {
        if let Some(ledger) = &self.supersession_ledger {
            let supersession = PgCiRunSupersession::new(
                self.pool.clone(),
                ledger.clone(),
                tenant.clone(),
                self.region.clone(),
                self.rt.clone(),
            )
            .map_err(PgCiStarterError::Supersession)?;
            PgCiPipelineStarter::new_with_authority_and_supersession(
                self.pool.clone(),
                self.rt.clone(),
                self.minter.clone(),
                tenant,
                self.region.clone(),
                self.blobs.clone(),
                definition,
                self.launch_authority.clone(),
                supersession,
            )
        } else {
            PgCiPipelineStarter::new_with_authority(
                self.pool.clone(),
                self.rt.clone(),
                self.minter.clone(),
                tenant,
                self.region.clone(),
                self.blobs.clone(),
                definition,
                self.launch_authority.clone(),
            )
        }
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
    match (
        candidate.record.trigger_kind.as_str(),
        candidate.record.concurrency_group.as_deref(),
        candidate.record.pr_head_generation,
    ) {
        ("pull_request", Some(group), generation)
            if crate::ci_run_store::valid_pr_concurrency_group(group)
                && generation.is_none_or(|value| value > 0) => {}
        ("pull_request", _, _) => {
            return Err(PgCiStarterError::CorruptRun(
                "pull-request run lacks canonical supersession authority".into(),
            ))
        }
        (_, None, None) => {}
        _ => {
            return Err(PgCiStarterError::CorruptRun(
                "non-PR run carries PR supersession authority".into(),
            ))
        }
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
            cause_depth: field!("cause_depth"),
            caused_by: field!("caused_by"),
            definition_snapshot: field!("definition_snapshot"),
            trigger_kind: field!("trigger_kind"),
            concurrency_group: field!("concurrency_group"),
            pr_head_generation: field!("pr_head_generation"),
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

fn workflow_input(
    record: &CiRunRecord,
    manifest_digest: &str,
) -> Result<Vec<ArtifactRef>, PgCiStarterError> {
    let input = vec![
        ci_artifact_ref(
            &record.tenant_id,
            &format!("drive-manifest-{manifest_digest}"),
        ),
        ci_run_ref(&record.tenant_id, &record.run_id),
    ];
    let decoded = decode_ci_claimed_input(&TenantId(record.tenant_id.clone()), &input)
        .map_err(|error| PgCiStarterError::CorruptRun(error.to_string()))?;
    if decoded.manifest_digest != manifest_digest || decoded.ci_run_id != record.run_id {
        return Err(PgCiStarterError::CorruptRun(
            "claimed-input encoding did not round-trip the authoritative manifest and run".into(),
        ));
    }
    Ok(input)
}

#[cfg(test)]
fn expected_ci_jobs_v1(
    record: &CiRunRecord,
    prepared: &crate::PreparedRunPlan,
) -> Result<Vec<ExpectedCiJobV1>, PgCiStarterError> {
    expected_ci_jobs_v1_with(record, prepared, ci_job_id_v1)
}

fn expected_ci_jobs_v2(
    record: &CiRunRecord,
    prepared: &PreparedRunPlanV2,
) -> Result<Vec<ExpectedCiJobV1>, PgCiStarterError> {
    let tenant = TenantId(record.tenant_id.clone());
    let expected_snapshot = format!(
        "myelin://{}/ci/snapshot/{}",
        tenant.0,
        prepared.content_hash().to_multihash_string()
    );
    if prepared.tenant() != &tenant || record.definition_snapshot != expected_snapshot {
        return Err(PgCiStarterError::CorruptRun(
            "prepared V2 plan provenance diverges from the locked ci_run".into(),
        ));
    }
    let run_id = sqlx::types::Uuid::parse_str(&record.run_id).map_err(|error| {
        PgCiStarterError::CorruptRun(format!("locked ci_run.run_id is not a UUID: {error}"))
    })?;
    let mut ids_by_name = BTreeMap::new();
    let mut unique_ids = BTreeSet::new();
    for job in &prepared.plan().jobs {
        let job_id = ci_job_id_v2(
            &tenant,
            run_id,
            &job.stage,
            &job.name,
            &job.matrix_identity(),
        );
        if !unique_ids.insert(job_id) || ids_by_name.insert(job.name.clone(), job_id).is_some() {
            return Err(PgCiStarterError::CorruptRun(format!(
                "deterministic version-2 ci_job identity collision at `{}`",
                job.name
            )));
        }
    }
    prepared
        .plan()
        .jobs
        .iter()
        .map(|job| {
            let needs = job
                .needs
                .iter()
                .map(|need| {
                    ids_by_name.get(need).copied().ok_or_else(|| {
                        PgCiStarterError::CorruptRun(format!(
                            "validated V2 node `{}` needs unmapped node `{need}`",
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
                        "encode V2 matrix identity for `{}`: {error}",
                        job.name
                    ))
                })?)
            };
            Ok(ExpectedCiJobV1 {
                tenant_id: record.tenant_id.clone(),
                region: record.region.clone(),
                job_id: ids_by_name[&job.name],
                run_id,
                stage: job.stage.clone(),
                name: job.name.clone(),
                needs,
                matrix_key,
                spec_ref: record.definition_snapshot.clone(),
                state: "queued".into(),
                attempt: 1,
                result_summary: None,
            })
        })
        .collect()
}

fn validate_replay_manifest(
    record: &CiRunRecord,
    started_at: &str,
    manifest: &CiDriveManifestV1,
) -> Result<(), PgCiStarterError> {
    manifest.validate().map_err(PgCiStarterError::Manifest)?;
    let snapshot_prefix = format!("myelin://{}/ci/snapshot/", record.tenant_id);
    let snapshot_digest = record
        .definition_snapshot
        .strip_prefix(&snapshot_prefix)
        .ok_or_else(|| {
            PgCiStarterError::CorruptRun(
                "locked definition snapshot is outside the tenant CI snapshot class".into(),
            )
        })?;
    let content_hash = ContentHash::parse(snapshot_digest).map_err(|error| {
        PgCiStarterError::CorruptRun(format!("locked snapshot digest is invalid: {error}"))
    })?;
    if !snapshot_digest.starts_with("blake3:")
        || record.definition_snapshot != format!("{snapshot_prefix}{snapshot_digest}")
    {
        return Err(PgCiStarterError::CorruptRun(
            "locked definition snapshot is not the canonical tenant V2 source".into(),
        ));
    }
    let trust_tier = match record.trust_tier.as_str() {
        "trusted" => CiManifestTrustTierV1::Trusted,
        "untrusted_fork" => CiManifestTrustTierV1::UntrustedFork,
        "self_hosted" => CiManifestTrustTierV1::SelfHosted,
        token => {
            return Err(PgCiStarterError::CorruptRun(format!(
                "locked run carries unknown trust tier `{token}`"
            )))
        }
    };
    let expected_source = ci_artifact_ref(
        &record.tenant_id,
        &format!("snapshot-{}", content_hash.to_multihash_string()),
    )
    .0;
    if manifest.tenant_id != record.tenant_id
        || manifest.region != record.region
        || manifest.wf_run_id != record.wf_run_id
        || manifest.ci_run_id != record.run_id
        || manifest.source_snapshot_ref != expected_source
        || manifest.repo_ref != record.repo_ref.as_deref().unwrap_or_default()
        || manifest.commit_oid != record.commit_oid.as_deref().unwrap_or_default()
        || manifest.run_ref != ci_run_ref(&record.tenant_id, &record.run_id).0
        || manifest.started_at != started_at
        || manifest.trust_tier != trust_tier
    {
        return Err(PgCiStarterError::CorruptRun(
            "immutable drive manifest diverges from the locked ci_run".into(),
        ));
    }
    Ok(())
}

fn matrix_identity(matrix_key: &BTreeMap<String, String>) -> Vec<u8> {
    let mut encoded = Vec::new();
    encoded.extend_from_slice(&(matrix_key.len() as u64).to_be_bytes());
    for (key, value) in matrix_key {
        encoded.extend_from_slice(&(key.len() as u64).to_be_bytes());
        encoded.extend_from_slice(key.as_bytes());
        encoded.extend_from_slice(&(value.len() as u64).to_be_bytes());
        encoded.extend_from_slice(value.as_bytes());
    }
    encoded
}

fn expected_ci_jobs_from_manifest(
    record: &CiRunRecord,
    manifest: &CiDriveManifestV1,
) -> Result<Vec<ExpectedCiJobV1>, PgCiStarterError> {
    let tenant = TenantId(record.tenant_id.clone());
    let run_id = sqlx::types::Uuid::parse_str(&record.run_id).map_err(|error| {
        PgCiStarterError::CorruptRun(format!("locked ci_run.run_id is not a UUID: {error}"))
    })?;
    let mut names_by_id = BTreeMap::new();
    for job in &manifest.jobs {
        let job_id = sqlx::types::Uuid::parse_str(&job.job_id).map_err(|error| {
            PgCiStarterError::CorruptRun(format!("manifest job id is not a UUID: {error}"))
        })?;
        let derived = ci_job_id_v2(
            &tenant,
            run_id,
            &job.stage,
            &job.name,
            &matrix_identity(&job.matrix_key),
        );
        if job_id != derived {
            return Err(PgCiStarterError::CorruptRun(format!(
                "manifest job `{}` has a noncanonical V2 identity",
                job.name
            )));
        }
        names_by_id.insert(job_id, job.name.as_str());
    }
    manifest
        .jobs
        .iter()
        .map(|job| {
            let job_id = sqlx::types::Uuid::parse_str(&job.job_id).map_err(|error| {
                PgCiStarterError::CorruptRun(format!("manifest job id is not a UUID: {error}"))
            })?;
            let mut needs = job
                .needs
                .iter()
                .map(|dependency| {
                    sqlx::types::Uuid::parse_str(dependency).map_err(|error| {
                        PgCiStarterError::CorruptRun(format!(
                            "manifest dependency id is not a UUID: {error}"
                        ))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            needs.sort_by_key(|dependency| names_by_id.get(dependency).copied());
            let matrix_key = (!job.matrix_key.is_empty())
                .then(|| serde_json::to_value(&job.matrix_key))
                .transpose()
                .map_err(|error| {
                    PgCiStarterError::CorruptRun(format!(
                        "encode manifest matrix identity for `{}`: {error}",
                        job.name
                    ))
                })?;
            Ok(ExpectedCiJobV1 {
                tenant_id: record.tenant_id.clone(),
                region: record.region.clone(),
                job_id,
                run_id,
                stage: job.stage.clone(),
                name: job.name.clone(),
                needs,
                matrix_key,
                spec_ref: String::new(),
                state: "queued".into(),
                attempt: 1,
                result_summary: None,
            })
        })
        .collect()
}

async fn verify_replay_attempts(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    record: &CiRunRecord,
    manifest: &CiDriveManifestV1,
) -> Result<(), PgCiStarterError> {
    let run_id = sqlx::types::Uuid::parse_str(&record.run_id).map_err(|error| {
        PgCiStarterError::CorruptRun(format!("locked ci_run.run_id is not a UUID: {error}"))
    })?;
    for (context, issued) in &manifest.check_attempts {
        let row: Option<(i32, Option<sqlx::types::Uuid>)> = sqlx::query_as(
            "SELECT next_attempt, current_run FROM check_attempt \
             WHERE tenant_id=$1 AND region=$2 AND repo_ref=$3 AND commit_oid=$4 AND context=$5 \
             FOR SHARE",
        )
        .bind(&record.tenant_id)
        .bind(&record.region)
        .bind(&manifest.repo_ref)
        .bind(&manifest.commit_oid)
        .bind(context)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(|error| {
            PgCiStarterError::Database(format!("verify replay check attempt: {error}"))
        })?;
        let (next, current) = row.ok_or_else(|| {
            PgCiStarterError::CorruptRun(format!(
                "manifest check context `{context}` has no allocation ledger"
            ))
        })?;
        let issued = i32::try_from(*issued).map_err(|_| {
            PgCiStarterError::CorruptRun("manifest check attempt exceeds PostgreSQL i32".into())
        })?;
        let minimum_next = issued.checked_add(1).ok_or_else(|| {
            PgCiStarterError::CorruptRun("manifest check attempt overflows PostgreSQL i32".into())
        })?;
        let valid = if current == Some(run_id) {
            next == minimum_next
        } else {
            current.is_some() && next > minimum_next
        };
        if !valid {
            return Err(PgCiStarterError::CorruptRun(format!(
                "manifest check attempt for `{context}` diverges from the allocation ledger"
            )));
        }
    }
    Ok(())
}

fn granted_jobs_v2(
    record: &CiRunRecord,
    prepared: &PreparedRunPlanV2,
    expected: &[ExpectedCiJobV1],
    authority: &CiLaunchAuthorityV1,
) -> Result<Vec<GrantedCiJobV1>, PgCiStarterError> {
    if authority.jobs.len() != prepared.plan().jobs.len()
        || expected.len() != prepared.plan().jobs.len()
    {
        return Err(PgCiStarterError::LaunchAuthority(CiLaunchAuthorityError(
            "authority must grant every concrete V2 job exactly once".into(),
        )));
    }
    let repo_ref = record.repo_ref.clone().ok_or_else(|| {
        PgCiStarterError::CorruptRun("locked run lacks repository provenance".into())
    })?;
    let commit_oid = record
        .commit_oid
        .clone()
        .ok_or_else(|| PgCiStarterError::CorruptRun("locked run lacks commit provenance".into()))?;
    prepared
        .plan()
        .jobs
        .iter()
        .zip(expected)
        .zip(&authority.jobs)
        .map(|((job, expected), grant)| {
            if grant.concrete_name != job.name || expected.name != job.name {
                return Err(PgCiStarterError::LaunchAuthority(CiLaunchAuthorityError(
                    "authority jobs must be strictly plan-ordered with no missing or extra name"
                        .into(),
                )));
            }
            let mut needs = expected
                .needs
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            needs.sort();
            Ok(GrantedCiJobV1 {
                job_id: expected.job_id.to_string(),
                stage: job.stage.clone(),
                name: job.name.clone(),
                check_context: ci_check_context_v1(&job.stage),
                needs,
                matrix_key: job.matrix_key.clone(),
                image: job.image.clone(),
                command: job.command.clone(),
                env: grant.env.clone(),
                secret_handles: grant.secret_handles.clone(),
                egress_allow: grant.egress_allow.clone(),
                limits: grant.limits.clone(),
                workspace: CiManifestWorkspaceV1 {
                    repo_ref: repo_ref.clone(),
                    commit_oid: commit_oid.clone(),
                    read_only_root: true,
                    tmpfs_scratch: true,
                },
                scheduling: grant.scheduling.clone(),
                reserve_handle: grant.reserve_handle.clone(),
                token_authority_handle: grant.token_authority_handle.clone(),
                continue_on_error: false,
            })
        })
        .collect()
}

fn build_drive_manifest_v1(
    record: &CiRunRecord,
    prepared: &PreparedRunPlanV2,
    definition: &CiWorkflowDefinitionPin,
    authority: &CiLaunchAuthorityV1,
    jobs: Vec<GrantedCiJobV1>,
    check_attempts: BTreeMap<String, u32>,
    started_at: &str,
) -> Result<CiDriveManifestV1, PgCiStarterError> {
    let trust_tier = match record.trust_tier.as_str() {
        "trusted" => CiManifestTrustTierV1::Trusted,
        "untrusted_fork" => CiManifestTrustTierV1::UntrustedFork,
        "self_hosted" => CiManifestTrustTierV1::SelfHosted,
        token => {
            return Err(PgCiStarterError::CorruptRun(format!(
                "locked run carries unknown trust tier `{token}`"
            )))
        }
    };
    let repo_ref = record.repo_ref.clone().ok_or_else(|| {
        PgCiStarterError::CorruptRun("locked run lacks repository provenance".into())
    })?;
    let commit_oid = record
        .commit_oid
        .clone()
        .ok_or_else(|| PgCiStarterError::CorruptRun("locked run lacks commit provenance".into()))?;
    let manifest = CiDriveManifestV1 {
        schema_version: 1,
        tenant_id: record.tenant_id.clone(),
        region: record.region.clone(),
        wf_run_id: record.wf_run_id.clone(),
        ci_run_id: record.run_id.clone(),
        source_snapshot_ref: ci_artifact_ref(
            &record.tenant_id,
            &format!("snapshot-{}", prepared.content_hash().to_multihash_string()),
        )
        .0,
        source_plan_schema_version: RUN_PLAN_SCHEMA_V2,
        launch_request_digest: prepared
            .plan()
            .launch_request_digest_v1()
            .map_err(PgCiStarterError::Plan)?,
        workflow_type: CI_PIPELINE_WF_TYPE.into(),
        workflow_definition_version: definition.version(),
        workflow_code_hash: definition.code_hash().into(),
        authority_policy_revision: authority.policy_revision.clone(),
        repo_ref,
        commit_oid,
        run_ref: ci_run_ref(&record.tenant_id, &record.run_id).0,
        started_at: started_at.into(),
        trust_tier,
        check_attempts,
        merge_waiter: authority.merge_waiter.clone(),
        jobs,
    };
    manifest.validate().map_err(PgCiStarterError::Manifest)?;
    Ok(manifest)
}

async fn allocate_check_attempts(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    record: &CiRunRecord,
    contexts: &BTreeSet<String>,
) -> Result<BTreeMap<String, u32>, PgCiStarterError> {
    let repo_ref = record.repo_ref.as_deref().ok_or_else(|| {
        PgCiStarterError::CorruptRun("locked run lacks repository provenance".into())
    })?;
    let commit_oid = record
        .commit_oid
        .as_deref()
        .ok_or_else(|| PgCiStarterError::CorruptRun("locked run lacks commit provenance".into()))?;
    let run_id = sqlx::types::Uuid::parse_str(&record.run_id).map_err(|error| {
        PgCiStarterError::CorruptRun(format!("locked run id is not a UUID: {error}"))
    })?;
    let mut attempts = BTreeMap::new();
    for context in contexts {
        let attempt: i32 = sqlx::query_scalar(BUMP_CHECK_ATTEMPT_SQL)
            .bind(&record.tenant_id)
            .bind(&record.region)
            .bind(repo_ref)
            .bind(commit_oid)
            .bind(context)
            .bind(run_id)
            .fetch_one(&mut **transaction)
            .await
            .map_err(|error| {
                PgCiStarterError::Database(format!("allocate check attempt: {error}"))
            })?;
        let attempt = u32::try_from(attempt).map_err(|_| {
            PgCiStarterError::CorruptRun("check attempt is not a positive u32".into())
        })?;
        if attempt == 0 || attempts.insert(context.clone(), attempt).is_some() {
            return Err(PgCiStarterError::CorruptRun(
                "check attempt allocation returned zero or a duplicate context".into(),
            ));
        }
    }
    Ok(attempts)
}

#[cfg(test)]
fn expected_ci_jobs_v1_with<F>(
    record: &CiRunRecord,
    prepared: &crate::PreparedRunPlan,
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

async fn materialize_ci_jobs(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    record: &CiRunRecord,
    expected: &[ExpectedCiJobV1],
    manifest_ref: &str,
    replay: bool,
) -> Result<(), PgCiStarterError> {
    if !replay {
        for job in expected {
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
            .bind(manifest_ref)
            .execute(&mut **transaction)
            .await
            .map_err(|error| PgCiStarterError::Database(format!("materialize ci_job: {error}")))?;
        }
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
            Some(actual)
                if actual.tenant_id == job.tenant_id
                    && actual.region == job.region
                    && actual.job_id == job.job_id
                    && actual.run_id == job.run_id
                    && actual.stage == job.stage
                    && actual.name == job.name
                    && actual.needs == job.needs
                    && actual.matrix_key == job.matrix_key
                    && actual.spec_ref == job.spec_ref
                    && if replay {
                        actual.attempt > 0
                    } else {
                        actual.state == job.state
                            && actual.attempt == job.attempt
                            && actual.result_summary == job.result_summary
                    } => {}
            Some(_) => {
                return Err(PgCiStarterError::CorruptRun(format!(
                    "durable ci_job `{}` diverges from immutable manifest authority",
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
    use crate::{
        CiExecutionProfileV1, CiExecutionRequestV1, CiJobLaunchGrantV1, CiManifestLaneV1,
        CiManifestLimitsV1, CiManifestSchedulingV1, ResolvedJobV1, ResolvedJobV2,
        ResolvedRunPlanV2,
    };

    const PINNED_IMAGE: &str =
        "registry.example/build@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    fn canonical_input() -> Vec<ArtifactRef> {
        vec![
            ci_artifact_ref("acme", &format!("drive-manifest-blake3:{}", "a".repeat(64))),
            ci_run_ref("acme", "10000000-0000-0000-0000-000000000001"),
        ]
    }

    fn prepared_plan(
        tenant_id: &str,
        run_id: &str,
        jobs: Vec<ResolvedJobV1>,
    ) -> (CiRunRecord, crate::PreparedRunPlan) {
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
            cause_depth: 0,
            caused_by: None,
            definition_snapshot: format!(
                "myelin://{tenant_id}/ci/snapshot/{}",
                hash.to_multihash_string()
            ),
            trigger_kind: "push".into(),
            concurrency_group: None,
            pr_head_generation: None,
            trust_tier: "trusted".into(),
            state: "queued".into(),
            correlation_id: run_id.into(),
        };
        let prepared =
            crate::load_resolved_run_plan(&blobs, &record).expect("load prepared test plan");
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

    fn prepared_plan_v2() -> (CiRunRecord, PreparedRunPlanV2) {
        let tenant = tenant();
        let plan = ResolvedRunPlanV2 {
            schema_version: RUN_PLAN_SCHEMA_V2,
            execution: CiExecutionRequestV1 {
                schema_version: 1,
                profile: CiExecutionProfileV1::LinuxSmallV1,
            },
            jobs: vec![
                ResolvedJobV2 {
                    stage: "build".into(),
                    name: "build".into(),
                    image: PINNED_IMAGE.into(),
                    command: vec!["/bin/build".into()],
                    needs: Vec::new(),
                    is_generator: false,
                    matrix_key: BTreeMap::new(),
                },
                ResolvedJobV2 {
                    stage: "test".into(),
                    name: "test".into(),
                    image: PINNED_IMAGE.into(),
                    command: vec!["/bin/test".into()],
                    needs: vec!["build".into()],
                    is_generator: false,
                    matrix_key: BTreeMap::new(),
                },
            ],
        };
        let bytes = plan.canonical_bytes().expect("canonical V2 test plan");
        let blobs = myelin_storage::FsBlobStore::new();
        let hash = blobs.put(&tenant, &bytes).expect("store V2 test plan");
        let record = CiRunRecord {
            tenant_id: tenant.0,
            run_id: "10000000-0000-0000-0000-000000000001".into(),
            region: "fr-par".into(),
            project_id: "22222222-2222-2222-2222-222222222222".into(),
            pipeline_id: "33333333-3333-3333-3333-333333333333".into(),
            wf_run_id: "20000000-0000-0000-0000-000000000001".into(),
            repo_ref: Some("myelin://acme/git/repo/core".into()),
            commit_oid: Some("deadbeef".into()),
            cause_event_id: None,
            cause_depth: 0,
            caused_by: None,
            definition_snapshot: format!(
                "myelin://acme/ci/snapshot/{}",
                hash.to_multihash_string()
            ),
            trigger_kind: "push".into(),
            concurrency_group: None,
            pr_head_generation: None,
            trust_tier: "trusted".into(),
            state: "queued".into(),
            correlation_id: "10000000-0000-0000-0000-000000000001".into(),
        };
        let prepared =
            load_launch_run_plan_v2(&blobs, &record).expect("load prepared V2 test plan");
        (record, prepared)
    }

    fn launch_grant(name: &str) -> CiJobLaunchGrantV1 {
        CiJobLaunchGrantV1 {
            concrete_name: name.into(),
            env: BTreeMap::new(),
            secret_handles: BTreeMap::new(),
            egress_allow: Vec::new(),
            limits: CiManifestLimitsV1 {
                cpu_millis: 1_000,
                mem_bytes: 1_073_741_824,
                disk_bytes: 2_147_483_648,
                pids_max: 128,
                timeout_secs: 600,
            },
            scheduling: CiManifestSchedulingV1 {
                lane: CiManifestLaneV1::Batch,
                labels: vec!["linux".into()],
                concurrency_group: None,
                fair_key: "project:22222222-2222-2222-2222-222222222222".into(),
            },
            reserve_handle: format!("reserve:run:{name}"),
            token_authority_handle: "mint:run".into(),
        }
    }

    #[test]
    fn claimed_input_round_trips_exact_manifest_tenant_and_run() {
        let input = canonical_input();
        let decoded = decode_ci_claimed_input(&tenant(), &input).expect("canonical claimed input");
        assert_eq!(decoded.tenant(), &tenant());
        assert_eq!(
            decoded.manifest_digest(),
            format!("blake3:{}", "a".repeat(64))
        );
        assert_eq!(decoded.ci_run_id(), "10000000-0000-0000-0000-000000000001");
        assert_eq!(
            input,
            vec![
                ci_artifact_ref(
                    "acme",
                    &format!("drive-manifest-{}", decoded.manifest_digest())
                ),
                ci_run_ref("acme", decoded.ci_run_id())
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
                ci_artifact_ref("acme", &format!("drive-manifest-sha256:{}", "a".repeat(64))),
                base[1].clone(),
            ],
            vec![
                ci_artifact_ref(
                    "other",
                    &format!("drive-manifest-blake3:{}", "a".repeat(64)),
                ),
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

    #[test]
    fn v2_launch_authority_requires_exact_plan_order_and_cardinality() {
        let (record, prepared) = prepared_plan_v2();
        let expected = expected_ci_jobs_v2(&record, &prepared).unwrap();
        for grants in [
            vec![launch_grant("build")],
            vec![
                launch_grant("build"),
                launch_grant("test"),
                launch_grant("extra"),
            ],
        ] {
            let authority = CiLaunchAuthorityV1 {
                policy_revision: "policy-v1".into(),
                jobs: grants,
                merge_waiter: None,
            };
            assert!(granted_jobs_v2(&record, &prepared, &expected, &authority)
                .expect_err("missing or extra authority job must fail closed")
                .to_string()
                .contains("exactly once"));
        }

        let reversed = CiLaunchAuthorityV1 {
            policy_revision: "policy-v1".into(),
            jobs: vec![launch_grant("test"), launch_grant("build")],
            merge_waiter: None,
        };
        assert!(granted_jobs_v2(&record, &prepared, &expected, &reversed)
            .expect_err("reordered authority jobs must fail closed")
            .to_string()
            .contains("strictly plan-ordered"));
    }

    #[test]
    fn initial_check_event_identity_is_stable_and_binds_manifest_context_and_attempt() {
        let (record, prepared) = prepared_plan_v2();
        let expected = expected_ci_jobs_v2(&record, &prepared).unwrap();
        let authority = CiLaunchAuthorityV1 {
            policy_revision: "policy-v1".into(),
            jobs: vec![launch_grant("build"), launch_grant("test")],
            merge_waiter: None,
        };
        let jobs = granted_jobs_v2(&record, &prepared, &expected, &authority).unwrap();
        let manifest = build_drive_manifest_v1(
            &record,
            &prepared,
            &CiWorkflowDefinitionPin::new(1, "blake3:ci-body-v1").unwrap(),
            &authority,
            jobs,
            BTreeMap::from([("build".into(), 1), ("test".into(), 1)]),
            "2026-07-21T12:34:56.000000Z",
        )
        .unwrap();
        let digest = manifest.digest().unwrap();
        let base = initial_check_event_id(&manifest, &digest, "build", 1);
        assert_eq!(
            initial_check_event_id(&manifest, &digest, "build", 1),
            base,
            "the same committed manifest fact derives the same outbox identity"
        );

        let mut other_tenant = manifest.clone();
        other_tenant.tenant_id = "other".into();
        let mut other_run = manifest.clone();
        other_run.ci_run_id = "10000000-0000-0000-0000-000000000002".into();
        let variants = [
            initial_check_event_id(&other_tenant, &digest, "build", 1),
            initial_check_event_id(&other_run, &digest, "build", 1),
            initial_check_event_id(&manifest, &format!("blake3:{}", "a".repeat(64)), "build", 1),
            initial_check_event_id(&manifest, &digest, "test", 1),
            initial_check_event_id(&manifest, &digest, "build", 2),
        ];
        assert!(variants.into_iter().all(|candidate| candidate != base));
        assert!(base.0.starts_with("ci-check-start-"));
    }

    #[test]
    fn manifest_execution_trust_projects_to_the_same_two_way_git_tier_as_dispatch() {
        assert_eq!(
            manifest_check_trust_tier(CiManifestTrustTierV1::Trusted),
            crate::check_emitter::TrustTier::Trusted
        );
        assert_eq!(
            manifest_check_trust_tier(CiManifestTrustTierV1::SelfHosted),
            crate::check_emitter::TrustTier::Trusted,
            "a self-hosted member run is trusted code for Git's two-way check gate"
        );
        assert_eq!(
            manifest_check_trust_tier(CiManifestTrustTierV1::UntrustedFork),
            crate::check_emitter::TrustTier::UntrustedFork
        );
    }
}
