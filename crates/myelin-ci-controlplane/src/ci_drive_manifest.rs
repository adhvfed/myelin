use std::collections::{BTreeMap, BTreeSet};

use myelin_ci_sandbox::ImageRef;
use myelin_tenancy::{Region, TenantId};
use serde::{Deserialize, Serialize};
use sqlx::{PgConnection, PgPool, Row};

use crate::{secret_broker::parse_canonical_secret_handle, CI_PIPELINE_WF_TYPE};

pub const CI_DRIVE_MANIFEST_SCHEMA_V1: u32 = 1;
pub const CI_DRIVE_MANIFEST_DIGEST_V1_DOMAIN: &str = "myelin.ci.drive-manifest.v1";
pub const MAX_CI_DRIVE_MANIFEST_BYTES: usize = 16 * 1024 * 1024;
const MAX_MANIFEST_JOBS: usize = 1_024;
const MAX_TOKEN_BYTES: usize = 512;
const MAX_MANIFEST_COLLECTION_ITEMS: usize = 1_024;
const MAX_CPU_MILLIS: u32 = 64_000;
const MAX_MEMORY_BYTES: u64 = 256 * 1024 * 1024 * 1024;
const MAX_DISK_BYTES: u64 = 2 * 1024 * 1024 * 1024 * 1024;
const MAX_PIDS: u32 = 32_768;
const MAX_TIMEOUT_SECS: u32 = 24 * 60 * 60;

const INSERT_MANIFEST: &str = "\
INSERT INTO ci_drive_manifest (
  tenant_id, region, wf_run_id, ci_run_id, schema_version,
  source_snapshot_ref, manifest_digest, manifest_bytes
) VALUES ($1, $2, $3::uuid, $4::uuid, $5, $6, $7, $8)
ON CONFLICT DO NOTHING";

const SELECT_COLLIDING_MANIFESTS: &str = "\
SELECT tenant_id, region, wf_run_id::text AS wf_run_id, ci_run_id::text AS ci_run_id,
       schema_version, source_snapshot_ref, manifest_digest, manifest_bytes
FROM ci_drive_manifest
WHERE tenant_id = $1 AND region = $2
  AND (wf_run_id = $3::uuid OR ci_run_id = $4::uuid)";

const SELECT_EXPECTED_MANIFEST: &str = "\
SELECT tenant_id, region, wf_run_id::text AS wf_run_id, ci_run_id::text AS ci_run_id,
       schema_version, source_snapshot_ref, manifest_digest, manifest_bytes
FROM ci_drive_manifest
WHERE tenant_id = $1 AND region = $2 AND wf_run_id = $3::uuid";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CiManifestTrustTierV1 {
    Trusted,
    UntrustedFork,
    SelfHosted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CiManifestLaneV1 {
    Interactive,
    Batch,
    Deploy,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CiManifestLimitsV1 {
    pub cpu_millis: u32,
    pub mem_bytes: u64,
    pub disk_bytes: u64,
    pub pids_max: u32,
    pub timeout_secs: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CiManifestWorkspaceV1 {
    pub repo_ref: String,
    pub commit_oid: String,
    pub read_only_root: bool,
    pub tmpfs_scratch: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CiManifestSchedulingV1 {
    pub lane: CiManifestLaneV1,
    pub labels: Vec<String>,
    pub concurrency_group: Option<String>,
    pub fair_key: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CiJobLaunchGrantV1 {
    pub concrete_name: String,
    pub env: BTreeMap<String, String>,
    pub secret_handles: BTreeMap<String, String>,
    pub egress_allow: Vec<String>,
    pub limits: CiManifestLimitsV1,
    pub scheduling: CiManifestSchedulingV1,
    pub reserve_handle: String,
    pub token_authority_handle: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CiLaunchAuthorityV1 {
    pub policy_revision: String,
    pub jobs: Vec<CiJobLaunchGrantV1>,
    pub merge_waiter: Option<CiMergeWaiterV1>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantedCiJobV1 {
    pub job_id: String,
    pub stage: String,
    pub name: String,
    pub check_context: String,
    pub needs: Vec<String>,
    pub matrix_key: BTreeMap<String, String>,
    pub image: String,
    pub command: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub secret_handles: BTreeMap<String, String>,
    pub egress_allow: Vec<String>,
    pub limits: CiManifestLimitsV1,
    pub workspace: CiManifestWorkspaceV1,
    pub scheduling: CiManifestSchedulingV1,
    pub reserve_handle: String,
    pub token_authority_handle: String,
    pub continue_on_error: bool,
}

pub fn ci_check_context_v1(authored_stage: &str) -> String {
    authored_stage.to_owned()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CiMergeWaiterV1 {
    pub workflow_run_id: String,
    pub idem_token: String,
    pub required_contexts: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CiDriveManifestV1 {
    pub schema_version: u32,
    pub tenant_id: String,
    pub region: String,
    pub project_id: String,
    pub wf_run_id: String,
    pub ci_run_id: String,
    pub source_snapshot_ref: String,
    pub source_plan_schema_version: u32,
    pub launch_request_digest: String,
    pub workflow_type: String,
    pub workflow_definition_version: i32,
    pub workflow_code_hash: String,
    pub authority_policy_revision: String,
    pub repo_ref: String,
    pub commit_oid: String,
    pub run_ref: String,
    pub started_at: String,
    pub trust_tier: CiManifestTrustTierV1,
    pub check_attempts: BTreeMap<String, u32>,
    pub merge_waiter: Option<CiMergeWaiterV1>,
    pub jobs: Vec<GrantedCiJobV1>,
}

impl CiDriveManifestV1 {
    pub fn validate(&self) -> Result<(), CiDriveManifestError> {
        if self.schema_version != CI_DRIVE_MANIFEST_SCHEMA_V1 {
            return invalid("unsupported manifest schema version");
        }
        validate_scope("tenant", &self.tenant_id)?;
        validate_scope("region", &self.region)?;
        validate_uuid("project_id", &self.project_id)?;
        validate_uuid("wf_run_id", &self.wf_run_id)?;
        validate_uuid("ci_run_id", &self.ci_run_id)?;
        validate_canonical_ref(
            "source_snapshot_ref",
            &self.source_snapshot_ref,
            &self.tenant_id,
            "ci",
            "artifact",
        )?;
        let snapshot = myelin_refs::parse_scoped(&self.source_snapshot_ref).map_err(|error| {
            CiDriveManifestError::Invalid(format!("source_snapshot_ref: {error}"))
        })?;
        let Some(snapshot_digest) = snapshot.id.strip_prefix("snapshot-blake3:") else {
            return invalid("source_snapshot_ref is not a snapshot BLAKE3 artifact");
        };
        if snapshot_digest.len() != 64
            || !snapshot_digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return invalid("source_snapshot_ref has a noncanonical BLAKE3 digest");
        }
        validate_canonical_ref("repo_ref", &self.repo_ref, &self.tenant_id, "git", "repo")?;
        validate_canonical_ref("run_ref", &self.run_ref, &self.tenant_id, "ci", "run")?;
        let run = myelin_refs::parse_scoped(&self.run_ref)
            .map_err(|error| CiDriveManifestError::Invalid(format!("run_ref: {error}")))?;
        if run.id != self.ci_run_id {
            return invalid("run_ref does not name ci_run_id");
        }
        if self.source_plan_schema_version != crate::RUN_PLAN_SCHEMA_V2 {
            return invalid("drive manifests require run-plan schema V2");
        }
        validate_digest("launch_request_digest", &self.launch_request_digest)?;
        if self.workflow_type != CI_PIPELINE_WF_TYPE {
            return invalid("workflow type is not ci.pipeline");
        }
        if self.workflow_definition_version <= 0 {
            return invalid("workflow definition version must be positive");
        }
        validate_bounded("workflow_code_hash", &self.workflow_code_hash)?;
        validate_bounded("authority_policy_revision", &self.authority_policy_revision)?;
        validate_bounded("commit_oid", &self.commit_oid)?;
        validate_rfc3339_utc("started_at", &self.started_at)?;
        if self.jobs.is_empty() || self.jobs.len() > MAX_MANIFEST_JOBS {
            return invalid("manifest job count is outside 1..=1024");
        }
        if self.check_attempts.is_empty() {
            return invalid("manifest has no check attempts");
        }
        for (context, attempt) in &self.check_attempts {
            validate_bounded("check context", context)?;
            if *attempt == 0 {
                return invalid("check attempts are one-based");
            }
        }

        let mut prior_name: Option<&str> = None;
        let mut ids = BTreeSet::new();
        let mut names = BTreeSet::new();
        let mut reserve_handles = BTreeSet::new();
        let mut job_contexts = BTreeSet::new();
        for job in &self.jobs {
            if prior_name.is_some_and(|prior| prior >= job.name.as_str()) {
                return invalid("jobs must be strictly sorted by concrete name");
            }
            prior_name = Some(&job.name);
            validate_uuid("job_id", &job.job_id)?;
            validate_machine_token("stage", &job.stage)?;
            validate_machine_token("job name", &job.name)?;
            validate_bounded("check context", &job.check_context)?;
            if job.check_context != ci_check_context_v1(&job.stage) {
                return invalid("job check context must be derived from its authored stage");
            }
            if !ids.insert(job.job_id.clone()) || !names.insert(job.name.clone()) {
                return invalid("manifest job ids and names must be unique");
            }
            job_contexts.insert(job.check_context.clone());
            validate_strictly_sorted("needs", &job.needs)?;
            validate_collection_len("needs", job.needs.len())?;
            for dependency in &job.needs {
                validate_uuid("dependency job id", dependency)?;
                if dependency == &job.job_id {
                    return invalid("a job cannot depend on itself");
                }
            }
            for (axis, value) in &job.matrix_key {
                validate_machine_token("matrix axis", axis)?;
                validate_bounded("matrix value", value)?;
            }
            validate_collection_len("matrix axes", job.matrix_key.len())?;
            if job.image.is_empty() || job.image.len() > crate::run_plan::MAX_IMAGE_BYTES {
                return invalid("job image is empty or overlong");
            }
            ImageRef::pinned(job.image.clone()).map_err(|_| {
                CiDriveManifestError::Invalid("job image is not digest-pinned".into())
            })?;
            if job.command.is_empty() || job.command.len() > crate::run_plan::MAX_COMMAND_ARGS {
                return invalid("job command length is outside 1..=64");
            }
            if job.command[0].is_empty() {
                return invalid("job command executable is empty");
            }
            let command_bytes = job.command.iter().try_fold(0usize, |total, argument| {
                if argument.contains('\0') {
                    None
                } else {
                    total.checked_add(argument.len())
                }
            });
            if command_bytes.is_none_or(|bytes| bytes > crate::run_plan::MAX_COMMAND_BYTES) {
                return invalid("job command contains NUL, overflows, or exceeds 32 KiB");
            }
            for (name, value) in &job.env {
                validate_machine_token("environment name", name)?;
                validate_bounded("environment value", value)?;
            }
            validate_collection_len("environment", job.env.len())?;
            for (name, handle) in &job.secret_handles {
                validate_machine_token("secret environment name", name)?;
                validate_tenant_secret_handle(handle, &self.tenant_id)?;
                if job.env.contains_key(name) {
                    return invalid("a secret name cannot also carry a literal environment value");
                }
            }
            validate_collection_len("secret handles", job.secret_handles.len())?;
            validate_strictly_sorted("egress allowlist", &job.egress_allow)?;
            validate_strictly_sorted("runner labels", &job.scheduling.labels)?;
            validate_collection_len("egress allowlist", job.egress_allow.len())?;
            validate_collection_len("runner labels", job.scheduling.labels.len())?;
            validate_limits(&job.limits)?;
            validate_workspace(self, &job.workspace)?;
            validate_bounded("fair key", &job.scheduling.fair_key)?;
            if let Some(group) = &job.scheduling.concurrency_group {
                validate_bounded("concurrency group", group)?;
            }
            validate_bounded("reserve handle", &job.reserve_handle)?;
            if !reserve_handles.insert(job.reserve_handle.clone()) {
                return invalid("manifest reserve handles must be unique per job");
            }
            validate_bounded("token authority handle", &job.token_authority_handle)?;
            if job.continue_on_error {
                return invalid(
                    "continue_on_error is unavailable until authored by the V2 plan contract",
                );
            }
        }
        if job_contexts != self.check_attempts.keys().cloned().collect() {
            return invalid("check_attempts must exactly cover the distinct job contexts");
        }
        for job in &self.jobs {
            for dependency in &job.needs {
                if !ids.contains(dependency) {
                    return invalid("job dependency does not exist in the manifest");
                }
            }
        }
        validate_acyclic(&self.jobs)?;
        if let Some(waiter) = &self.merge_waiter {
            validate_uuid("merge workflow_run_id", &waiter.workflow_run_id)?;
            validate_bounded("merge idem_token", &waiter.idem_token)?;
            if waiter.required_contexts.is_empty() {
                return invalid("merge waiter has no required contexts");
            }
            validate_strictly_sorted("merge required contexts", &waiter.required_contexts)?;
            for context in &waiter.required_contexts {
                if !self.check_attempts.contains_key(context) {
                    return invalid("merge waiter names an unknown check context");
                }
            }
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, CiDriveManifestError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self)
            .map_err(|error| CiDriveManifestError::Wire(error.to_string()))?;
        if bytes.len() > MAX_CI_DRIVE_MANIFEST_BYTES {
            return invalid("manifest exceeds the 16 MiB ceiling");
        }
        Ok(bytes)
    }

    pub fn digest(&self) -> Result<String, CiDriveManifestError> {
        let bytes = self.canonical_bytes()?;
        let mut hasher = blake3::Hasher::new_derive_key(CI_DRIVE_MANIFEST_DIGEST_V1_DOMAIN);
        hasher.update(&(bytes.len() as u64).to_be_bytes());
        hasher.update(&bytes);
        Ok(format!("blake3:{}", hasher.finalize().to_hex()))
    }

    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, CiDriveManifestError> {
        if bytes.len() > MAX_CI_DRIVE_MANIFEST_BYTES {
            return invalid("manifest exceeds the 16 MiB ceiling");
        }
        let manifest: Self = serde_json::from_slice(bytes)
            .map_err(|error| CiDriveManifestError::Wire(error.to_string()))?;
        let canonical = manifest.canonical_bytes()?;
        if canonical != bytes {
            return invalid("manifest bytes are not canonical compact JSON");
        }
        Ok(manifest)
    }
}

#[derive(Clone)]
pub struct CiDriveManifestStore {
    pool: PgPool,
    tenant: TenantId,
    region: Region,
}

impl CiDriveManifestStore {
    pub fn new(
        pool: PgPool,
        tenant: TenantId,
        region: Region,
    ) -> Result<Self, CiDriveManifestError> {
        validate_scope("tenant", &tenant.0)?;
        validate_scope("region", &region.0)?;
        Ok(Self {
            pool,
            tenant,
            region,
        })
    }

    pub async fn insert(
        &self,
        manifest: &CiDriveManifestV1,
    ) -> Result<String, CiDriveManifestError> {
        self.require_scope(manifest)?;
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| database("begin insert", error))?;
        scope_connection(&mut transaction, &self.tenant, &self.region).await?;
        let digest = self.insert_on_conn(&mut transaction, manifest).await?;
        transaction
            .commit()
            .await
            .map_err(|error| database("commit insert", error))?;
        Ok(digest)
    }

    pub async fn load_by_identity(
        &self,
        wf_run_id: &str,
        ci_run_id: &str,
    ) -> Result<Option<(CiDriveManifestV1, String)>, CiDriveManifestError> {
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| database("begin identity load", error))?;
        scope_connection(&mut transaction, &self.tenant, &self.region).await?;
        let loaded = self
            .load_by_identity_on_conn(&mut transaction, wf_run_id, ci_run_id)
            .await?;
        transaction
            .commit()
            .await
            .map_err(|error| database("commit identity load", error))?;
        Ok(loaded)
    }

    pub async fn insert_on_conn(
        &self,
        connection: &mut PgConnection,
        manifest: &CiDriveManifestV1,
    ) -> Result<String, CiDriveManifestError> {
        self.require_scope(manifest)?;
        let bytes = manifest.canonical_bytes()?;
        let digest = manifest.digest()?;
        sqlx::query(INSERT_MANIFEST)
            .bind(&manifest.tenant_id)
            .bind(&manifest.region)
            .bind(&manifest.wf_run_id)
            .bind(&manifest.ci_run_id)
            .bind(manifest.schema_version as i32)
            .bind(&manifest.source_snapshot_ref)
            .bind(&digest)
            .bind(&bytes)
            .execute(&mut *connection)
            .await
            .map_err(|error| database("insert", error))?;
        let rows = sqlx::query(SELECT_COLLIDING_MANIFESTS)
            .bind(&manifest.tenant_id)
            .bind(&manifest.region)
            .bind(&manifest.wf_run_id)
            .bind(&manifest.ci_run_id)
            .fetch_all(&mut *connection)
            .await
            .map_err(|error| database("verify insert", error))?;
        if rows.len() != 1 {
            return Err(CiDriveManifestError::Conflict);
        }
        verify_row(&rows[0], manifest, &bytes, &digest)?;
        Ok(digest)
    }

    pub async fn load_by_identity_on_conn(
        &self,
        connection: &mut PgConnection,
        wf_run_id: &str,
        ci_run_id: &str,
    ) -> Result<Option<(CiDriveManifestV1, String)>, CiDriveManifestError> {
        validate_uuid("wf_run_id", wf_run_id)?;
        validate_uuid("ci_run_id", ci_run_id)?;
        let tenant_id = self.tenant.0.as_str();
        let rows = sqlx::query(SELECT_COLLIDING_MANIFESTS)
            .bind(tenant_id)
            .bind(&self.region.0)
            .bind(wf_run_id)
            .bind(ci_run_id)
            .fetch_all(&mut *connection)
            .await
            .map_err(|error| database("load identity", error))?;
        if rows.is_empty() {
            return Ok(None);
        }
        if rows.len() != 1 {
            return Err(CiDriveManifestError::Conflict);
        }
        let row = &rows[0];
        let bytes: Vec<u8> = row.get("manifest_bytes");
        let digest: String = row.get("manifest_digest");
        let manifest = CiDriveManifestV1::decode_canonical(&bytes)?;
        verify_row(row, &manifest, &bytes, &digest)?;
        if manifest.tenant_id != self.tenant.0
            || manifest.region != self.region.0
            || manifest.wf_run_id != wf_run_id
            || manifest.ci_run_id != ci_run_id
        {
            return Err(CiDriveManifestError::IdentityMismatch);
        }
        Ok(Some((manifest, digest)))
    }

    pub async fn load_by_wf_run_on_conn(
        &self,
        connection: &mut PgConnection,
        wf_run_id: &str,
    ) -> Result<Option<(CiDriveManifestV1, String)>, CiDriveManifestError> {
        validate_uuid("wf_run_id", wf_run_id)?;
        let tenant_id = self.tenant.0.as_str();
        let row = sqlx::query(SELECT_EXPECTED_MANIFEST)
            .bind(tenant_id)
            .bind(&self.region.0)
            .bind(wf_run_id)
            .fetch_optional(&mut *connection)
            .await
            .map_err(|error| database("load workflow authority", error))?;
        let Some(row) = row else {
            return Ok(None);
        };
        let bytes: Vec<u8> = row.get("manifest_bytes");
        let digest: String = row.get("manifest_digest");
        let manifest = CiDriveManifestV1::decode_canonical(&bytes)?;
        verify_row(&row, &manifest, &bytes, &digest)?;
        if manifest.tenant_id != self.tenant.0
            || manifest.region != self.region.0
            || manifest.wf_run_id != wf_run_id
        {
            return Err(CiDriveManifestError::IdentityMismatch);
        }
        Ok(Some((manifest, digest)))
    }

    pub async fn load_expected(
        &self,
        wf_run_id: &str,
        ci_run_id: &str,
        expected_digest: &str,
    ) -> Result<CiDriveManifestV1, CiDriveManifestError> {
        validate_uuid("wf_run_id", wf_run_id)?;
        validate_uuid("ci_run_id", ci_run_id)?;
        validate_digest("manifest digest", expected_digest)?;
        let tenant_id = self.tenant.0.as_str();
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| database("begin load", error))?;
        scope_connection(&mut transaction, &self.tenant, &self.region).await?;
        let row = sqlx::query(SELECT_EXPECTED_MANIFEST)
            .bind(tenant_id)
            .bind(&self.region.0)
            .bind(wf_run_id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|error| database("load", error))?
            .ok_or(CiDriveManifestError::NotFound)?;
        let bytes: Vec<u8> = row.get("manifest_bytes");
        let manifest = CiDriveManifestV1::decode_canonical(&bytes)?;
        verify_row(&row, &manifest, &bytes, expected_digest)?;
        if manifest.ci_run_id != ci_run_id {
            return Err(CiDriveManifestError::IdentityMismatch);
        }
        transaction
            .commit()
            .await
            .map_err(|error| database("commit load", error))?;
        Ok(manifest)
    }

    fn require_scope(&self, manifest: &CiDriveManifestV1) -> Result<(), CiDriveManifestError> {
        if manifest.tenant_id != self.tenant.0 || manifest.region != self.region.0 {
            return Err(CiDriveManifestError::ScopeMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum CiDriveManifestError {
    Invalid(String),
    Wire(String),
    Database {
        operation: &'static str,
        detail: String,
    },
    ScopeMismatch,
    IdentityMismatch,
    DigestMismatch,
    Conflict,
    NotFound,
}

impl std::fmt::Display for CiDriveManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(detail) => write!(f, "invalid CI drive manifest: {detail}"),
            Self::Wire(detail) => write!(f, "malformed CI drive manifest wire: {detail}"),
            Self::Database { operation, detail } => {
                write!(
                    f,
                    "CI drive manifest database error during {operation}: {detail}"
                )
            }
            Self::ScopeMismatch => write!(f, "CI drive manifest scope mismatch"),
            Self::IdentityMismatch => write!(f, "CI drive manifest identity mismatch"),
            Self::DigestMismatch => write!(f, "CI drive manifest digest mismatch"),
            Self::Conflict => write!(f, "CI drive manifest uniqueness collision"),
            Self::NotFound => write!(f, "CI drive manifest not found"),
        }
    }
}

impl std::error::Error for CiDriveManifestError {}

fn verify_row(
    row: &sqlx::postgres::PgRow,
    manifest: &CiDriveManifestV1,
    bytes: &[u8],
    digest: &str,
) -> Result<(), CiDriveManifestError> {
    let stored_bytes: Vec<u8> = row.get("manifest_bytes");
    let stored_digest: String = row.get("manifest_digest");
    let stored_version: i32 = row.get("schema_version");
    let exact = row.get::<String, _>("tenant_id") == manifest.tenant_id
        && row.get::<String, _>("region") == manifest.region
        && row.get::<String, _>("wf_run_id") == manifest.wf_run_id
        && row.get::<String, _>("ci_run_id") == manifest.ci_run_id
        && stored_version == manifest.schema_version as i32
        && row.get::<String, _>("source_snapshot_ref") == manifest.source_snapshot_ref
        && stored_bytes == bytes;
    if !exact {
        return Err(CiDriveManifestError::IdentityMismatch);
    }
    if stored_digest != digest || manifest.digest()? != digest {
        return Err(CiDriveManifestError::DigestMismatch);
    }
    Ok(())
}

async fn scope_connection(
    connection: &mut PgConnection,
    tenant: &TenantId,
    region: &Region,
) -> Result<(), CiDriveManifestError> {
    sqlx::query("SELECT set_config('myelin.tenant_id', $1, true)")
        .bind(&tenant.0)
        .execute(&mut *connection)
        .await
        .map_err(|error| database("scope tenant", error))?;
    // @tenant-cross-scope: this sets the second half of the already tenant-scoped session context;
    sqlx::query("SELECT set_config('myelin.region', $1, true)")
        .bind(&region.0)
        .execute(&mut *connection)
        .await
        .map_err(|error| database("scope region", error))?;
    Ok(())
}

fn validate_acyclic(jobs: &[GrantedCiJobV1]) -> Result<(), CiDriveManifestError> {
    let index: BTreeMap<&str, usize> = jobs
        .iter()
        .enumerate()
        .map(|(position, job)| (job.job_id.as_str(), position))
        .collect();
    let mut colors = vec![0_u8; jobs.len()];
    fn visit(
        at: usize,
        jobs: &[GrantedCiJobV1],
        index: &BTreeMap<&str, usize>,
        colors: &mut [u8],
    ) -> Result<(), CiDriveManifestError> {
        if colors[at] == 1 {
            return invalid("manifest job graph contains a cycle");
        }
        if colors[at] == 2 {
            return Ok(());
        }
        colors[at] = 1;
        for dependency in &jobs[at].needs {
            visit(index[dependency.as_str()], jobs, index, colors)?;
        }
        colors[at] = 2;
        Ok(())
    }
    for at in 0..jobs.len() {
        visit(at, jobs, &index, &mut colors)?;
    }
    Ok(())
}

fn validate_workspace(
    manifest: &CiDriveManifestV1,
    workspace: &CiManifestWorkspaceV1,
) -> Result<(), CiDriveManifestError> {
    if workspace.repo_ref != manifest.repo_ref || workspace.commit_oid != manifest.commit_oid {
        return invalid("job workspace provenance differs from manifest provenance");
    }
    if !workspace.read_only_root || !workspace.tmpfs_scratch {
        return invalid("job workspace must use a read-only root and tmpfs scratch");
    }
    Ok(())
}

fn validate_limits(limits: &CiManifestLimitsV1) -> Result<(), CiDriveManifestError> {
    if limits.cpu_millis == 0
        || limits.mem_bytes == 0
        || limits.disk_bytes == 0
        || limits.pids_max == 0
        || limits.timeout_secs == 0
    {
        return invalid("all job resource limits must be positive");
    }
    if limits.cpu_millis > MAX_CPU_MILLIS
        || limits.mem_bytes > MAX_MEMORY_BYTES
        || limits.disk_bytes > MAX_DISK_BYTES
        || limits.pids_max > MAX_PIDS
        || limits.timeout_secs > MAX_TIMEOUT_SECS
    {
        return invalid("job resource grant exceeds the platform hard ceiling");
    }
    Ok(())
}

fn validate_collection_len(label: &str, len: usize) -> Result<(), CiDriveManifestError> {
    if len > MAX_MANIFEST_COLLECTION_ITEMS {
        return invalid(format!("{label} exceeds the 1024-item ceiling"));
    }
    Ok(())
}

fn validate_strictly_sorted(label: &str, values: &[String]) -> Result<(), CiDriveManifestError> {
    for value in values {
        validate_bounded(label, value)?;
    }
    if values.windows(2).any(|pair| pair[0] >= pair[1]) {
        return invalid(format!("{label} must be sorted and unique"));
    }
    Ok(())
}

fn validate_machine_token(label: &str, value: &str) -> Result<(), CiDriveManifestError> {
    validate_bounded(label, value)?;
    if !value.as_bytes()[0].is_ascii_alphanumeric()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return invalid(format!("{label} is not a machine token"));
    }
    Ok(())
}

fn validate_bounded(label: &str, value: &str) -> Result<(), CiDriveManifestError> {
    if value.is_empty() || value.len() > MAX_TOKEN_BYTES || value.trim() != value {
        return invalid(format!(
            "{label} must be non-empty, trimmed, and at most 512 bytes"
        ));
    }
    Ok(())
}

fn validate_rfc3339_utc(label: &str, value: &str) -> Result<(), CiDriveManifestError> {
    validate_bounded(label, value)?;
    if !value.ends_with('Z')
        || value.len() < 20
        || value.as_bytes().get(4) != Some(&b'-')
        || value.as_bytes().get(7) != Some(&b'-')
        || value.as_bytes().get(10) != Some(&b'T')
        || value.as_bytes().get(13) != Some(&b':')
        || value.as_bytes().get(16) != Some(&b':')
        || value.bytes().any(|byte| byte.is_ascii_whitespace())
    {
        return invalid(format!("{label} is not a canonical UTC RFC3339 timestamp"));
    }
    Ok(())
}

fn validate_scope(label: &str, value: &str) -> Result<(), CiDriveManifestError> {
    validate_bounded(label, value)?;
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return invalid(format!("{label} contains an invalid byte"));
    }
    Ok(())
}

fn validate_uuid(label: &str, value: &str) -> Result<(), CiDriveManifestError> {
    let parsed = sqlx::types::Uuid::parse_str(value)
        .map_err(|_| CiDriveManifestError::Invalid(format!("{label} is not a UUID")))?;
    if parsed.to_string() != value {
        return invalid(format!("{label} is not a canonical lowercase UUID"));
    }
    Ok(())
}

fn validate_digest(label: &str, value: &str) -> Result<(), CiDriveManifestError> {
    let Some(hex) = value.strip_prefix("blake3:") else {
        return invalid(format!("{label} is not a BLAKE3 digest"));
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return invalid(format!("{label} is not a canonical 32-byte BLAKE3 digest"));
    }
    Ok(())
}

fn validate_canonical_ref(
    label: &str,
    value: &str,
    tenant: &str,
    subsystem: &str,
    type_: &str,
) -> Result<(), CiDriveManifestError> {
    let parsed = myelin_refs::parse_scoped(value)
        .map_err(|error| CiDriveManifestError::Invalid(format!("{label}: {error}")))?;
    if parsed.tenant.0 != tenant
        || parsed.subsystem != subsystem
        || parsed.type_ != type_
        || parsed.sub.is_some()
        || value != format!("myelin://{tenant}/{subsystem}/{type_}/{}", parsed.id)
    {
        return invalid(format!(
            "{label} is not the expected canonical tenant-scoped reference"
        ));
    }
    Ok(())
}

fn validate_tenant_secret_handle(handle: &str, tenant: &str) -> Result<(), CiDriveManifestError> {
    validate_bounded("secret handle", handle)?;
    let Some(parsed) = parse_canonical_secret_handle(handle) else {
        return invalid("secret handle is not a strict canonical tenant-scoped secret identity");
    };
    if parsed.tenant != tenant {
        return invalid("secret handle is not bound to the manifest tenant");
    }
    Ok(())
}

fn invalid<T>(detail: impl Into<String>) -> Result<T, CiDriveManifestError> {
    Err(CiDriveManifestError::Invalid(detail.into()))
}

fn database(operation: &'static str, error: sqlx::Error) -> CiDriveManifestError {
    CiDriveManifestError::Database {
        operation,
        detail: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(byte: char) -> String {
        format!("blake3:{}", byte.to_string().repeat(64))
    }

    fn job(id: &str, name: &str, needs: Vec<String>) -> GrantedCiJobV1 {
        GrantedCiJobV1 {
            job_id: id.into(),
            stage: name.into(),
            name: name.into(),
            check_context: name.into(),
            needs,
            matrix_key: BTreeMap::new(),
            image: format!("registry.example/{name}@sha256:{}", "a".repeat(64)),
            command: vec!["/bin/true".into()],
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
            workspace: CiManifestWorkspaceV1 {
                repo_ref: "myelin://acme/git/repo/core".into(),
                commit_oid: "deadbeef".into(),
                read_only_root: true,
                tmpfs_scratch: true,
            },
            scheduling: CiManifestSchedulingV1 {
                lane: CiManifestLaneV1::Batch,
                labels: vec!["linux".into()],
                concurrency_group: None,
                fair_key: "project:core".into(),
            },
            reserve_handle: format!("reserve:{id}"),
            token_authority_handle: format!("mint:{id}"),
            continue_on_error: false,
        }
    }

    fn manifest() -> CiDriveManifestV1 {
        let build = "11111111-1111-8111-8111-111111111111";
        let test = "22222222-2222-8222-8222-222222222222";
        CiDriveManifestV1 {
            schema_version: 1,
            tenant_id: "acme".into(),
            region: "fr-par".into(),
            project_id: "55555555-5555-4555-8555-555555555555".into(),
            wf_run_id: "33333333-3333-8333-8333-333333333333".into(),
            ci_run_id: "44444444-4444-8444-8444-444444444444".into(),
            source_snapshot_ref: format!("myelin://acme/ci/artifact/snapshot-{}", digest('a')),
            source_plan_schema_version: 2,
            launch_request_digest: digest('b'),
            workflow_type: CI_PIPELINE_WF_TYPE.into(),
            workflow_definition_version: 1,
            workflow_code_hash: digest('c'),
            authority_policy_revision: "ci-policy-2026-07-21".into(),
            repo_ref: "myelin://acme/git/repo/core".into(),
            commit_oid: "deadbeef".into(),
            run_ref: "myelin://acme/ci/run/44444444-4444-8444-8444-444444444444".into(),
            started_at: "2026-07-21T12:34:56.000000Z".into(),
            trust_tier: CiManifestTrustTierV1::Trusted,
            check_attempts: BTreeMap::from([("build".into(), 7), ("test".into(), 4)]),
            merge_waiter: None,
            jobs: vec![
                job(build, "build", vec![]),
                job(test, "test", vec![build.into()]),
            ],
        }
    }

    #[test]
    fn canonical_round_trip_and_digest_are_stable() {
        let manifest = manifest();
        let bytes = manifest.canonical_bytes().unwrap();
        assert_eq!(
            CiDriveManifestV1::decode_canonical(&bytes).unwrap(),
            manifest
        );
        assert_eq!(manifest.digest().unwrap(), manifest.digest().unwrap());
        assert!(manifest.digest().unwrap().starts_with("blake3:"));
        assert!(!String::from_utf8(bytes).unwrap().contains("token_jti"));
    }

    #[test]
    fn ordinary_runs_do_not_fabricate_merge_authority() {
        let manifest = manifest();
        assert!(manifest.merge_waiter.is_none());
        assert!(!String::from_utf8(manifest.canonical_bytes().unwrap())
            .unwrap()
            .contains("merge:"));
    }

    #[test]
    fn rejects_cycles_unpinned_images_and_context_drift() {
        let mut cyclic = manifest();
        cyclic.jobs[0].needs = vec![cyclic.jobs[1].job_id.clone()];
        assert!(cyclic.validate().unwrap_err().to_string().contains("cycle"));

        let mut unpinned = manifest();
        unpinned.jobs[0].image = "registry.example/build:latest".into();
        assert!(unpinned
            .validate()
            .unwrap_err()
            .to_string()
            .contains("digest-pinned"));

        let mut drifted = manifest();
        drifted.check_attempts.remove("test");
        assert!(drifted
            .validate()
            .unwrap_err()
            .to_string()
            .contains("exactly cover"));
    }

    #[test]
    fn rejects_one_budget_reservation_reused_by_multiple_jobs() {
        let mut reused = manifest();
        reused.jobs[1].reserve_handle = reused.jobs[0].reserve_handle.clone();
        assert!(reused
            .validate()
            .unwrap_err()
            .to_string()
            .contains("reserve handles must be unique"));
    }

    #[test]
    fn secret_handles_are_bound_to_the_manifest_tenant() {
        let mut valid = manifest();
        valid.jobs[0]
            .secret_handles
            .insert("DEPLOY_KEY".into(), "myelin://acme/ci/secret/deploy".into());
        valid.validate().unwrap();

        valid.jobs[0].secret_handles.insert(
            "DEPLOY_KEY".into(),
            "myelin://victim/ci/secret/deploy".into(),
        );
        assert!(valid
            .validate()
            .unwrap_err()
            .to_string()
            .contains("manifest tenant"));
    }

    #[test]
    fn secret_handles_require_a_canonical_manifest_project_binding_identity() {
        let mut manifest = manifest();
        manifest.jobs[0]
            .secret_handles
            .insert("DEPLOY_KEY".into(), "myelin://acme/ci/secret/deploy".into());
        manifest.validate().unwrap();

        manifest.project_id = "project-b-alias".into();
        assert!(manifest
            .validate()
            .unwrap_err()
            .to_string()
            .contains("project_id"));
    }

    #[test]
    fn secret_handles_reject_every_noncanonical_alias_vector() {
        for handle in [
            "myelin://acme/ci/secret/secret:deploy",
            "myelin://acme/ci/secret/..",
            "myelin://acme/ci/secret/%2f",
            "myelin://acme/ci/secret/deploy\0",
            "junkmyelin://acme/ci/secret/deploy",
            "myelin://acme/ci/secret/deploy/",
            "myelin:///ci/secret/deploy",
            "myelin://acme/ci/secret/",
        ] {
            let mut invalid = manifest();
            invalid.jobs[0]
                .secret_handles
                .insert("DEPLOY_KEY".into(), handle.into());
            assert!(
                invalid.validate().is_err(),
                "manifest admitted noncanonical secret handle {handle:?}"
            );
        }
    }

    #[test]
    fn rejects_noncanonical_wire_and_unknown_fields() {
        let manifest = manifest();
        let mut bytes = manifest.canonical_bytes().unwrap();
        bytes.push(b' ');
        assert!(CiDriveManifestV1::decode_canonical(&bytes)
            .unwrap_err()
            .to_string()
            .contains("canonical"));

        let mut value = serde_json::to_value(manifest).unwrap();
        value["token_jti"] = serde_json::Value::String("must-not-land".into());
        assert!(CiDriveManifestV1::decode_canonical(&serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn authority_limits_provenance_and_collection_ceilings_are_fail_closed() {
        let mut wrong_snapshot_class = manifest();
        wrong_snapshot_class.source_snapshot_ref =
            format!("myelin://acme/ci/artifact/plan-{}", digest('a'));
        assert!(wrong_snapshot_class.validate().is_err());

        let mut wrong_plan_version = manifest();
        wrong_plan_version.source_plan_schema_version = 1;
        assert!(wrong_plan_version.validate().is_err());

        let mut excessive = manifest();
        excessive.jobs[0].limits.cpu_millis = MAX_CPU_MILLIS + 1;
        assert!(excessive
            .validate()
            .unwrap_err()
            .to_string()
            .contains("hard ceiling"));

        let mut too_many_env = manifest();
        too_many_env.jobs[0].env = (0..=MAX_MANIFEST_COLLECTION_ITEMS)
            .map(|index| (format!("V{index}"), "value".into()))
            .collect();
        assert!(too_many_env
            .validate()
            .unwrap_err()
            .to_string()
            .contains("1024-item ceiling"));

        let mut forged_context = manifest();
        forged_context.jobs[0].check_context = "ci:other".into();
        forged_context.check_attempts.remove("build");
        forged_context.check_attempts.insert("ci:other".into(), 7);
        assert!(forged_context
            .validate()
            .unwrap_err()
            .to_string()
            .contains("authored stage"));

        let mut prefixed_token = manifest();
        prefixed_token.jobs[0].stage = "_build".into();
        assert!(prefixed_token
            .validate()
            .unwrap_err()
            .to_string()
            .contains("machine token"));

        let mut exact_argv = manifest();
        exact_argv.jobs[0].image = format!(
            "registry.example/{}@sha256:{}",
            "a".repeat(500),
            "b".repeat(64)
        );
        exact_argv.jobs[0].command = vec!["/bin/tool".into(), "".into(), "  exact  ".into()];
        assert!(exact_argv.validate().is_ok());

        let mut rewritten_failure_semantics = manifest();
        rewritten_failure_semantics.jobs[0].continue_on_error = true;
        assert!(rewritten_failure_semantics
            .validate()
            .unwrap_err()
            .to_string()
            .contains("until authored"));
    }
}
