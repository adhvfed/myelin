use std::collections::{BTreeMap, BTreeSet};

use myelin_ci_sandbox::gvisor::{is_admitted_structured_cargo_recipe, platform_cargo_argv};
use myelin_ci_sandbox::ImageRef;
use myelin_storage::{BlobError, BlobStore, ContentHash};
use myelin_tenancy::TenantId;
use serde::{Deserialize, Serialize};

use crate::ci_run_store::CiRunRecord;

pub const RUN_PLAN_SCHEMA_V1: u32 = 1;
pub const RUN_PLAN_SCHEMA_V2: u32 = 2;
pub const EXECUTION_REQUEST_SCHEMA_V1: u32 = 1;
pub const LAUNCH_REQUEST_DIGEST_V1_DOMAIN: &str = "myelin.ci.launch-request.v1";
pub const MAX_RUN_PLAN_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_RUN_PLAN_JOBS: usize = 1_024;
pub const MAX_JOB_NAME_BYTES: usize = 128;
pub const MAX_IMAGE_BYTES: usize = 2_048;
pub const MAX_COMMAND_ARGS: usize = 64;
pub const MAX_COMMAND_BYTES: usize = 32 * 1024;
pub const MAX_STRUCTURED_BUILD_ARGS: usize = 16;
pub const MAX_STRUCTURED_BUILD_ARG_BYTES: usize = 256;
pub const PLATFORM_CARGO_HOME: &str = myelin_ci_sandbox::gvisor::STRUCTURED_CARGO_HOME;
pub const MAX_MATRIX_AXES: usize = 16;
pub const MAX_MATRIX_KEY_BYTES: usize = 64;
pub const MAX_MATRIX_VALUE_BYTES: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedRunPlanV1 {
    pub schema_version: u32,
    pub jobs: Vec<ResolvedJobV1>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedJobV1 {
    pub name: String,
    pub image: String,
    pub command: Vec<String>,
    pub needs: Vec<String>,
    pub is_generator: bool,
    pub matrix_key: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CiExecutionProfileV1 {
    #[serde(rename = "linux-small-v1")]
    LinuxSmallV1,
    #[serde(rename = "linux-build-v1")]
    LinuxBuildV1,
}

impl CiExecutionProfileV1 {
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "linux-small-v1" => Some(Self::LinuxSmallV1),
            "linux-build-v1" => Some(Self::LinuxBuildV1),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CiExecutionRequestV1 {
    pub schema_version: u32,
    pub profile: CiExecutionProfileV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedRunPlanV2 {
    pub schema_version: u32,
    pub execution: CiExecutionRequestV1,
    pub jobs: Vec<ResolvedJobV2>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedJobV2 {
    pub stage: String,
    pub name: String,
    pub image: String,
    pub command: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<StructuredBuildV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_cargo_vendor: Option<String>,
    pub needs: Vec<String>,
    pub is_generator: bool,
    pub matrix_key: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StructuredBuildToolV1 {
    Cargo,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructuredBuildV1 {
    pub tool: StructuredBuildToolV1,
    pub args: Vec<String>,
}

impl StructuredBuildV1 {
    pub fn validate_for_job(&self, job_name: &str) -> Result<(), RunPlanError> {
        validate_structured_build(job_name, self)
    }

    pub fn platform_argv(&self) -> Vec<String> {
        match self.tool {
            StructuredBuildToolV1::Cargo => platform_cargo_argv(&self.args),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VersionedResolvedRunPlan {
    V1(ResolvedRunPlanV1),
    V2(ResolvedRunPlanV2),
}

pub fn derive_concrete_job_name(stage: &str, matrix_key: &BTreeMap<String, String>) -> String {
    if matrix_key.is_empty() {
        return stage.to_string();
    }
    let mut identity = Vec::new();
    identity.extend_from_slice(&(stage.len() as u64).to_be_bytes());
    identity.extend_from_slice(stage.as_bytes());
    identity.extend_from_slice(&(matrix_key.len() as u64).to_be_bytes());
    for (key, value) in matrix_key {
        identity.extend_from_slice(&(key.len() as u64).to_be_bytes());
        identity.extend_from_slice(key.as_bytes());
        identity.extend_from_slice(&(value.len() as u64).to_be_bytes());
        identity.extend_from_slice(value.as_bytes());
    }
    let digest = blake3::hash(&identity).to_hex();
    let prefix: String = stage
        .bytes()
        .filter(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        .take(61)
        .map(char::from)
        .collect();
    format!("{prefix}--{digest}")
}

impl ResolvedJobV1 {
    pub fn matrix_identity(&self) -> Vec<u8> {
        let mut encoded = Vec::new();
        encoded.extend_from_slice(&(self.matrix_key.len() as u64).to_be_bytes());
        for (key, value) in &self.matrix_key {
            encoded.extend_from_slice(&(key.len() as u64).to_be_bytes());
            encoded.extend_from_slice(key.as_bytes());
            encoded.extend_from_slice(&(value.len() as u64).to_be_bytes());
            encoded.extend_from_slice(value.as_bytes());
        }
        encoded
    }
}

impl ResolvedJobV2 {
    pub fn matrix_identity(&self) -> Vec<u8> {
        let mut encoded = Vec::new();
        encoded.extend_from_slice(&(self.matrix_key.len() as u64).to_be_bytes());
        for (key, value) in &self.matrix_key {
            encoded.extend_from_slice(&(key.len() as u64).to_be_bytes());
            encoded.extend_from_slice(key.as_bytes());
            encoded.extend_from_slice(&(value.len() as u64).to_be_bytes());
            encoded.extend_from_slice(value.as_bytes());
        }
        encoded
    }
}

impl ResolvedRunPlanV1 {
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, RunPlanError> {
        validate_plan(self)?;
        let bytes = serde_json::to_vec(self).map_err(|error| RunPlanError::WireMalformed {
            detail: error.to_string(),
        })?;
        if bytes.len() > MAX_RUN_PLAN_BYTES {
            return Err(RunPlanError::SnapshotTooLarge {
                actual: bytes.len(),
                maximum: MAX_RUN_PLAN_BYTES,
            });
        }
        Ok(bytes)
    }
}

impl ResolvedRunPlanV2 {
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, RunPlanError> {
        validate_plan_v2(self)?;
        canonical_json(self)
    }

    pub fn launch_request_digest_v1(&self) -> Result<String, RunPlanError> {
        validate_plan_v2(self)?;
        let mut hasher = blake3::Hasher::new_derive_key(LAUNCH_REQUEST_DIGEST_V1_DOMAIN);
        let request = canonical_json_unbounded(&self.execution)?;
        update_digest_frame(&mut hasher, &request);
        for job in &self.jobs {
            let bytes = canonical_json_unbounded(job)?;
            update_digest_frame(&mut hasher, &bytes);
        }
        Ok(format!("blake3:{}", hasher.finalize().to_hex()))
    }
}

impl VersionedResolvedRunPlan {
    pub fn schema_version(&self) -> u32 {
        match self {
            Self::V1(plan) => plan.schema_version,
            Self::V2(plan) => plan.schema_version,
        }
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, RunPlanError> {
        match self {
            Self::V1(plan) => plan.canonical_bytes(),
            Self::V2(plan) => plan.canonical_bytes(),
        }
    }

    pub fn as_v1(&self) -> Option<&ResolvedRunPlanV1> {
        match self {
            Self::V1(plan) => Some(plan),
            Self::V2(_) => None,
        }
    }

    pub fn as_v2(&self) -> Option<&ResolvedRunPlanV2> {
        match self {
            Self::V1(_) => None,
            Self::V2(plan) => Some(plan),
        }
    }
}

fn canonical_json<T: Serialize>(value: &T) -> Result<Vec<u8>, RunPlanError> {
    let bytes = canonical_json_unbounded(value)?;
    if bytes.len() > MAX_RUN_PLAN_BYTES {
        return Err(RunPlanError::SnapshotTooLarge {
            actual: bytes.len(),
            maximum: MAX_RUN_PLAN_BYTES,
        });
    }
    Ok(bytes)
}

fn canonical_json_unbounded<T: Serialize>(value: &T) -> Result<Vec<u8>, RunPlanError> {
    serde_json::to_vec(value).map_err(|error| RunPlanError::WireMalformed {
        detail: error.to_string(),
    })
}

fn update_digest_frame(hasher: &mut blake3::Hasher, frame: &[u8]) {
    hasher.update(&(frame.len() as u64).to_be_bytes());
    hasher.update(frame);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedRunPlan {
    tenant: TenantId,
    content_hash: ContentHash,
    plan: ResolvedRunPlanV1,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedRunPlanV2 {
    tenant: TenantId,
    content_hash: ContentHash,
    plan: ResolvedRunPlanV2,
}

impl PreparedRunPlanV2 {
    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    pub fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }

    pub fn plan(&self) -> &ResolvedRunPlanV2 {
        &self.plan
    }
}

impl PreparedRunPlan {
    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    pub fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }

    pub fn plan(&self) -> &ResolvedRunPlanV1 {
        &self.plan
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RedispatchReason {
    LegacyUnversioned,
    UnsupportedVersion(u64),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunPlanError {
    ProvenanceRefused { detail: String },
    TenantMismatch { record: String, reference: String },
    RedispatchRequired(RedispatchReason),
    LaunchAuthorityRequired { version: u32 },
    Blob(BlobError),
    SnapshotTooLarge { actual: usize, maximum: usize },
    MetadataAddressMismatch,
    WireMalformed { detail: String },
    InvalidPlan { detail: String },
}

impl RunPlanError {
    pub fn requires_redispatch(&self) -> bool {
        matches!(self, RunPlanError::RedispatchRequired(_))
    }

    pub fn is_dependency_failure(&self) -> bool {
        matches!(self, RunPlanError::Blob(BlobError::Backend(_)))
    }
}

impl std::fmt::Display for RunPlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunPlanError::ProvenanceRefused { detail } => {
                write!(f, "CI run-plan provenance refused before blob access: {detail}")
            }
            RunPlanError::TenantMismatch { record, reference } => write!(
                f,
                "CI run-plan tenant mismatch: run record `{record}` does not match snapshot `{reference}`"
            ),
            RunPlanError::RedispatchRequired(RedispatchReason::LegacyUnversioned) => write!(
                f,
                "legacy unversioned CI run plan is not executable; re-dispatch is required"
            ),
            RunPlanError::RedispatchRequired(RedispatchReason::UnsupportedVersion(version)) => {
                write!(
                    f,
                    "CI run-plan schema version {version} is unsupported; re-dispatch is required"
                )
            }
            RunPlanError::LaunchAuthorityRequired { version } => write!(
                f,
                "CI run-plan schema version {version} is a valid request but cannot execute until durable launch authority is materialized"
            ),
            RunPlanError::Blob(error) => write!(f, "CI run-plan blob access failed: {error}"),
            RunPlanError::SnapshotTooLarge { actual, maximum } => write!(
                f,
                "CI run-plan snapshot is {actual} bytes, above the {maximum}-byte limit"
            ),
            RunPlanError::MetadataAddressMismatch => {
                write!(f, "CI run-plan CAS metadata address mismatch")
            }
            RunPlanError::WireMalformed { detail } => {
                write!(f, "malformed CI run-plan wire: {detail}")
            }
            RunPlanError::InvalidPlan { detail } => write!(f, "invalid CI run plan: {detail}"),
        }
    }
}

impl std::error::Error for RunPlanError {}

impl From<BlobError> for RunPlanError {
    fn from(value: BlobError) -> Self {
        RunPlanError::Blob(value)
    }
}

pub fn load_resolved_run_plan<B: BlobStore + ?Sized>(
    blobs: &B,
    run: &CiRunRecord,
) -> Result<PreparedRunPlan, RunPlanError> {
    let (tenant, content_hash, versioned) = load_versioned_run_plan(blobs, run)?;
    let plan = match versioned {
        VersionedResolvedRunPlan::V1(plan) => plan,
        VersionedResolvedRunPlan::V2(plan) => {
            return Err(RunPlanError::LaunchAuthorityRequired {
                version: plan.schema_version,
            })
        }
    };

    Ok(PreparedRunPlan {
        tenant,
        content_hash,
        plan,
    })
}

pub fn load_launch_run_plan_v2<B: BlobStore + ?Sized>(
    blobs: &B,
    run: &CiRunRecord,
) -> Result<PreparedRunPlanV2, RunPlanError> {
    let (tenant, content_hash, versioned) = load_versioned_run_plan(blobs, run)?;
    let plan = match versioned {
        VersionedResolvedRunPlan::V2(plan) => plan,
        VersionedResolvedRunPlan::V1(plan) => {
            return Err(RunPlanError::InvalidPlan {
                detail: format!(
                    "manifest-backed launch requires run-plan schema V2; received V{}",
                    plan.schema_version
                ),
            })
        }
    };
    Ok(PreparedRunPlanV2 {
        tenant,
        content_hash,
        plan,
    })
}

fn load_versioned_run_plan<B: BlobStore + ?Sized>(
    blobs: &B,
    run: &CiRunRecord,
) -> Result<(TenantId, ContentHash, VersionedResolvedRunPlan), RunPlanError> {
    let (tenant, content_hash) = parse_snapshot_ref(run)?;

    let metadata = blobs.head(&tenant, &content_hash)?;
    if metadata.hash != content_hash {
        return Err(RunPlanError::MetadataAddressMismatch);
    }
    if metadata.stored_len > MAX_RUN_PLAN_BYTES {
        return Err(RunPlanError::SnapshotTooLarge {
            actual: metadata.stored_len,
            maximum: MAX_RUN_PLAN_BYTES,
        });
    }

    let bytes = blobs.get(&tenant, &content_hash)?;
    if bytes.len() > MAX_RUN_PLAN_BYTES {
        return Err(RunPlanError::SnapshotTooLarge {
            actual: bytes.len(),
            maximum: MAX_RUN_PLAN_BYTES,
        });
    }
    let plan = decode_resolved_run_plan(&bytes)?;
    Ok((tenant, content_hash, plan))
}

fn parse_snapshot_ref(run: &CiRunRecord) -> Result<(TenantId, ContentHash), RunPlanError> {
    if !valid_tenant_token(&run.tenant_id) {
        return Err(RunPlanError::ProvenanceRefused {
            detail: "ci_run.tenant_id is empty, overlong, or not an opaque machine token".into(),
        });
    }
    if run
        .repo_ref
        .as_deref()
        .is_none_or(|value| value.trim().is_empty())
    {
        return Err(RunPlanError::ProvenanceRefused {
            detail: "ci_run.repo_ref is absent or empty".into(),
        });
    }
    if run
        .commit_oid
        .as_deref()
        .is_none_or(|value| value.trim().is_empty())
    {
        return Err(RunPlanError::ProvenanceRefused {
            detail: "ci_run.commit_oid is absent or empty".into(),
        });
    }
    let remainder = run
        .definition_snapshot
        .strip_prefix("myelin://")
        .ok_or_else(|| RunPlanError::ProvenanceRefused {
            detail: "definition_snapshot must use `myelin://<tenant>/ci/snapshot/<hash>`".into(),
        })?;
    let parts: Vec<_> = remainder.split('/').collect();
    if parts.len() != 4 || parts[1] != "ci" || parts[2] != "snapshot" {
        return Err(RunPlanError::ProvenanceRefused {
            detail: "definition_snapshot path must be exactly `/ci/snapshot/`".into(),
        });
    }
    let reference_tenant = parts[0];
    if !valid_tenant_token(reference_tenant) {
        return Err(RunPlanError::ProvenanceRefused {
            detail: "definition_snapshot tenant is not an opaque machine token".into(),
        });
    }
    if reference_tenant != run.tenant_id {
        return Err(RunPlanError::TenantMismatch {
            record: run.tenant_id.clone(),
            reference: reference_tenant.to_string(),
        });
    }
    let address = parts[3];
    let Some(digest) = address.strip_prefix("blake3:") else {
        return Err(RunPlanError::ProvenanceRefused {
            detail: "definition_snapshot must use a BLAKE3 content address".into(),
        });
    };
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(RunPlanError::ProvenanceRefused {
            detail:
                "definition_snapshot digest must be exactly 64 lowercase hexadecimal characters"
                    .into(),
        });
    }
    let content_hash =
        ContentHash::parse(address).map_err(|error| RunPlanError::ProvenanceRefused {
            detail: format!("definition_snapshot address is malformed: {error}"),
        })?;
    Ok((TenantId::from_token(run.tenant_id.clone()), content_hash))
}

fn valid_tenant_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

pub fn decode_resolved_run_plan(bytes: &[u8]) -> Result<VersionedResolvedRunPlan, RunPlanError> {
    if bytes.len() > MAX_RUN_PLAN_BYTES {
        return Err(RunPlanError::SnapshotTooLarge {
            actual: bytes.len(),
            maximum: MAX_RUN_PLAN_BYTES,
        });
    }
    let envelope: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|error| RunPlanError::WireMalformed {
            detail: error.to_string(),
        })?;
    let object = envelope
        .as_object()
        .ok_or_else(|| RunPlanError::WireMalformed {
            detail: "top-level value must be an object".into(),
        })?;
    let version = object
        .get("schema_version")
        .ok_or(RunPlanError::RedispatchRequired(
            RedispatchReason::LegacyUnversioned,
        ))?;
    let version = version
        .as_u64()
        .ok_or_else(|| RunPlanError::WireMalformed {
            detail: "schema_version must be an unsigned integer".into(),
        })?;
    let plan = match version {
        version if version == u64::from(RUN_PLAN_SCHEMA_V1) => {
            let plan: ResolvedRunPlanV1 =
                serde_json::from_value(envelope).map_err(|error| RunPlanError::WireMalformed {
                    detail: error.to_string(),
                })?;
            validate_plan(&plan)?;
            VersionedResolvedRunPlan::V1(plan)
        }
        version if version == u64::from(RUN_PLAN_SCHEMA_V2) => {
            let plan: ResolvedRunPlanV2 =
                serde_json::from_value(envelope).map_err(|error| RunPlanError::WireMalformed {
                    detail: error.to_string(),
                })?;
            validate_plan_v2(&plan)?;
            VersionedResolvedRunPlan::V2(plan)
        }
        version => {
            return Err(RunPlanError::RedispatchRequired(
                RedispatchReason::UnsupportedVersion(version),
            ))
        }
    };
    let canonical = plan.canonical_bytes()?;
    if bytes != canonical {
        return Err(RunPlanError::WireMalformed {
            detail: "snapshot bytes are not canonical compact JSON".into(),
        });
    }
    Ok(plan)
}

fn validate_plan(plan: &ResolvedRunPlanV1) -> Result<(), RunPlanError> {
    if plan.schema_version != RUN_PLAN_SCHEMA_V1 {
        return Err(RunPlanError::RedispatchRequired(
            RedispatchReason::UnsupportedVersion(u64::from(plan.schema_version)),
        ));
    }
    if plan.jobs.is_empty() {
        return invalid("a run plan must contain at least one job");
    }
    if plan.jobs.len() > MAX_RUN_PLAN_JOBS {
        return invalid(format!(
            "plan has {} jobs, above the {MAX_RUN_PLAN_JOBS}-job limit",
            plan.jobs.len()
        ));
    }

    let mut names = BTreeSet::new();
    let mut previous_name: Option<&str> = None;
    for job in &plan.jobs {
        if !valid_machine_token(&job.name, MAX_JOB_NAME_BYTES) {
            return invalid(format!(
                "job name `{}` is not a bounded machine token",
                job.name
            ));
        }
        if !names.insert(job.name.as_str()) {
            return invalid(format!("duplicate job name `{}`", job.name));
        }
        if previous_name.is_some_and(|previous| previous >= job.name.as_str()) {
            return invalid("jobs must be sorted strictly by name for deterministic snapshots");
        }
        previous_name = Some(&job.name);

        if job.image.is_empty() || job.image.len() > MAX_IMAGE_BYTES {
            return invalid(format!(
                "job `{}` image reference is empty or overlong",
                job.name
            ));
        }
        if !(ImageRef {
            reference: job.image.clone(),
        })
        .digest_pinned()
        {
            return invalid(format!("job `{}` image is not digest-pinned", job.name));
        }
        if job.command.is_empty() || job.command.len() > MAX_COMMAND_ARGS {
            return invalid(format!(
                "job `{}` command must contain 1..={MAX_COMMAND_ARGS} arguments",
                job.name
            ));
        }
        if job.command[0].is_empty() {
            return invalid(format!("job `{}` command executable is empty", job.name));
        }
        let command_bytes = job.command.iter().try_fold(0usize, |total, argument| {
            if argument.contains('\0') {
                None
            } else {
                total.checked_add(argument.len())
            }
        });
        let Some(command_bytes) = command_bytes else {
            return invalid(format!(
                "job `{}` command contains NUL or overflows",
                job.name
            ));
        };
        if command_bytes > MAX_COMMAND_BYTES {
            return invalid(format!(
                "job `{}` command is {command_bytes} bytes, above {MAX_COMMAND_BYTES}",
                job.name
            ));
        }
        if job.is_generator {
            return invalid(format!(
                "job `{}` is a dynamic generator; fragment ingestion is not implemented",
                job.name
            ));
        }
        validate_matrix(job)?;
    }

    let mut indegree = BTreeMap::<&str, usize>::new();
    let mut dependents = BTreeMap::<&str, Vec<&str>>::new();
    for job in &plan.jobs {
        let mut seen_needs = BTreeSet::new();
        let mut previous_need: Option<&str> = None;
        for need in &job.needs {
            if need == &job.name {
                return invalid(format!("job `{}` depends on itself", job.name));
            }
            if !names.contains(need.as_str()) {
                return invalid(format!("job `{}` needs unknown job `{need}`", job.name));
            }
            if !seen_needs.insert(need.as_str()) {
                return invalid(format!("job `{}` repeats need `{need}`", job.name));
            }
            if previous_need.is_some_and(|previous| previous >= need.as_str()) {
                return invalid(format!(
                    "job `{}` needs must be sorted strictly for deterministic snapshots",
                    job.name
                ));
            }
            previous_need = Some(need);
            dependents
                .entry(need.as_str())
                .or_default()
                .push(job.name.as_str());
        }
        indegree.insert(job.name.as_str(), job.needs.len());
    }

    let mut ready: BTreeSet<&str> = indegree
        .iter()
        .filter_map(|(name, degree)| (*degree == 0).then_some(*name))
        .collect();
    let mut visited = 0usize;
    while let Some(name) = ready.pop_first() {
        visited += 1;
        for dependent in dependents.get(name).into_iter().flatten() {
            let Some(degree) = indegree.get_mut(dependent) else {
                return invalid(format!(
                    "job dependency graph lost validated job `{dependent}`"
                ));
            };
            let Some(next_degree) = degree.checked_sub(1) else {
                return invalid(format!(
                    "job dependency graph counted duplicate edge to `{dependent}`"
                ));
            };
            *degree = next_degree;
            if *degree == 0 {
                ready.insert(dependent);
            }
        }
    }
    if visited != plan.jobs.len() {
        return invalid("job dependency graph contains a cycle");
    }
    Ok(())
}

fn validate_plan_v2(plan: &ResolvedRunPlanV2) -> Result<(), RunPlanError> {
    if plan.schema_version != RUN_PLAN_SCHEMA_V2 {
        return Err(RunPlanError::RedispatchRequired(
            RedispatchReason::UnsupportedVersion(u64::from(plan.schema_version)),
        ));
    }
    if plan.execution.schema_version != EXECUTION_REQUEST_SCHEMA_V1 {
        return invalid(format!(
            "execution request schema version {} is unsupported",
            plan.execution.schema_version
        ));
    }
    if plan
        .jobs
        .iter()
        .any(|job| !valid_machine_token(&job.stage, MAX_JOB_NAME_BYTES))
    {
        return invalid("every version-2 authored stage must be a bounded machine token");
    }
    for job in &plan.jobs {
        let expected = derive_concrete_job_name(&job.stage, &job.matrix_key);
        if job.name != expected {
            return invalid(format!(
                "version-2 concrete job name `{}` does not match stage `{}` and its matrix identity; expected `{expected}`",
                job.name, job.stage
            ));
        }
        match &job.build {
            Some(build) => {
                if !job.command.is_empty() {
                    return invalid(format!(
                        "version-2 job `{}` must declare either command or build, never both",
                        job.name
                    ));
                }
                build.validate_for_job(&job.name)?;
            }
            None if job.command.is_empty() => {
                return invalid(format!(
                    "version-2 job `{}` must declare either command or build",
                    job.name
                ));
            }
            None => {}
        }
        if job.selected_cargo_vendor.is_some() && job.build.is_none() {
            return invalid(format!(
                "version-2 job `{}` carries a Cargo vendor selection without a structured build",
                job.name
            ));
        }
    }
    let compatibility = ResolvedRunPlanV1 {
        schema_version: RUN_PLAN_SCHEMA_V1,
        jobs: plan
            .jobs
            .iter()
            .map(|job| ResolvedJobV1 {
                name: job.name.clone(),
                image: job.image.clone(),
                command: job
                    .build
                    .as_ref()
                    .map_or_else(|| job.command.clone(), StructuredBuildV1::platform_argv),
                needs: job.needs.clone(),
                is_generator: job.is_generator,
                matrix_key: job.matrix_key.clone(),
            })
            .collect(),
    };
    validate_plan(&compatibility)
}

fn validate_structured_build(
    job_name: &str,
    build: &StructuredBuildV1,
) -> Result<(), RunPlanError> {
    if build.args.is_empty() || build.args.len() > MAX_STRUCTURED_BUILD_ARGS {
        return invalid(format!(
            "job `{job_name}` structured build args must contain 1..={MAX_STRUCTURED_BUILD_ARGS} entries"
        ));
    }
    for argument in &build.args {
        if argument.is_empty() || argument.len() > MAX_STRUCTURED_BUILD_ARG_BYTES {
            return invalid(format!(
                "job `{job_name}` structured build argument is empty or exceeds {MAX_STRUCTURED_BUILD_ARG_BYTES} bytes"
            ));
        }
        if !argument.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/' | b'=')
        }) {
            return invalid(format!(
                "job `{job_name}` structured build argument contains shell metacharacters or unsupported bytes"
            ));
        }
    }
    match build.tool {
        StructuredBuildToolV1::Cargo => {
            if is_admitted_structured_cargo_recipe(&build.args) {
                Ok(())
            } else {
                invalid(format!(
                    "job `{job_name}` Cargo recipe is not in the platform allowlist; admitted recipes are \
                     `build --locked`, workspace or library test runs, `test --locked -p <package>`, \
                     and workspace or root clippy runs"
                ))
            }
        }
    }
}

fn validate_matrix(job: &ResolvedJobV1) -> Result<(), RunPlanError> {
    if job.matrix_key.len() > MAX_MATRIX_AXES {
        return invalid(format!(
            "job `{}` has more than {MAX_MATRIX_AXES} matrix axes",
            job.name
        ));
    }
    for (key, value) in &job.matrix_key {
        if !valid_machine_token(key, MAX_MATRIX_KEY_BYTES) {
            return invalid(format!(
                "job `{}` matrix key `{key}` is not a bounded machine token",
                job.name
            ));
        }
        if !valid_machine_token(value, MAX_MATRIX_VALUE_BYTES) {
            return invalid(format!(
                "job `{}` matrix value for `{key}` is not a bounded machine token",
                job.name
            ));
        }
    }
    Ok(())
}

fn valid_machine_token(value: &str, maximum: usize) -> bool {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    value.len() <= maximum
        && first.is_ascii_alphanumeric()
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn invalid<T>(detail: impl Into<String>) -> Result<T, RunPlanError> {
    Err(RunPlanError::InvalidPlan {
        detail: detail.into(),
    })
}

#[cfg(test)]
#[path = "run_plan_tests.rs"]
mod tests;
