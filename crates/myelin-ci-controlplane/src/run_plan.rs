//! Strict loader for a queued CI run's immutable, tenant-scoped execution-plan snapshot.
//!
//! This is intentionally a preparation boundary, not an execution boundary. The wire contains only
//! resolved DAG facts. Runtime authority (tokens, secrets, trust, mounts, egress, and resource
//! grants) must be supplied later by policy-aware components and cannot be smuggled through the CAS
//! document.

use std::collections::{BTreeMap, BTreeSet};

use myelin_ci_sandbox::ImageRef;
use myelin_storage::{BlobError, BlobStore, ContentHash};
use myelin_tenancy::TenantId;
use serde::{Deserialize, Serialize};

use crate::ci_run_store::CiRunRecord;

/// The only schema version this loader can execute.
pub const RUN_PLAN_SCHEMA_V1: u32 = 1;
/// Maximum plaintext or stored snapshot size accepted by the execution boundary.
pub const MAX_RUN_PLAN_BYTES: usize = 16 * 1024 * 1024;
/// Maximum number of resolved jobs in one plan.
pub const MAX_RUN_PLAN_JOBS: usize = 1_024;
/// Maximum UTF-8 byte length of a resolved job name.
pub const MAX_JOB_NAME_BYTES: usize = 128;
/// Maximum UTF-8 byte length of an image reference.
pub const MAX_IMAGE_BYTES: usize = 2_048;
/// Maximum argv entries in one command, including argv[0].
pub const MAX_COMMAND_ARGS: usize = 64;
/// Maximum aggregate UTF-8 bytes across one command vector.
pub const MAX_COMMAND_BYTES: usize = 32 * 1024;
/// Maximum matrix axes attached to a resolved job.
pub const MAX_MATRIX_AXES: usize = 16;
/// Maximum UTF-8 byte length of a matrix axis name.
pub const MAX_MATRIX_KEY_BYTES: usize = 64;
/// Maximum UTF-8 byte length of a matrix axis value.
pub const MAX_MATRIX_VALUE_BYTES: usize = 128;

/// Version-1 resolved run-plan wire. Field order is part of the canonical JSON representation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedRunPlanV1 {
    /// Must equal [`RUN_PLAN_SCHEMA_V1`].
    pub schema_version: u32,
    /// Resolved jobs, sorted strictly by `name`.
    pub jobs: Vec<ResolvedJobV1>,
}

/// One resolved job in the version-1 wire.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedJobV1 {
    /// Unique bounded machine-token DAG node name.
    pub name: String,
    /// Digest-pinned image reference.
    pub image: String,
    /// Exact argv. No shell or fallback executable is inferred.
    pub command: Vec<String>,
    /// Existing DAG node names this job depends on, sorted strictly.
    pub needs: Vec<String>,
    /// Dynamic generators are represented on the wire but refused until ingestion is implemented.
    pub is_generator: bool,
    /// Deterministically ordered resolved matrix axes.
    pub matrix_key: BTreeMap<String, String>,
}

impl ResolvedJobV1 {
    /// Collision-safe, deterministic identity bytes for the matrix assignment.
    ///
    /// Each string is length-prefixed, so assignments such as `a=bc` and `ab=c` cannot collide.
    /// The map's `BTreeMap` order makes the encoding stable across processes.
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
    /// Validate semantics and return the deterministic compact JSON representation.
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

/// A fully parsed and validated plan, still carrying its authoritative tenant and content address.
/// Its fields are private so callers cannot accidentally replace validation with struct literals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedRunPlan {
    tenant: TenantId,
    content_hash: ContentHash,
    plan: ResolvedRunPlanV1,
}

impl PreparedRunPlan {
    /// Authoritative tenant derived from [`CiRunRecord`], never from caller input.
    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    /// Verified content address loaded from the tenant-scoped CAS keyspace.
    pub fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }

    /// The validated DAG. This is not a sequential [`crate::ci_pipeline::PipelineRun`].
    pub fn plan(&self) -> &ResolvedRunPlanV1 {
        &self.plan
    }
}

/// Version failures that require the trigger to be re-dispatched through a current resolver.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RedispatchReason {
    /// The historical document has no explicit schema version.
    LegacyUnversioned,
    /// The document uses a schema this binary cannot safely interpret.
    UnsupportedVersion(u64),
}

/// Fail-closed preparation errors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunPlanError {
    /// The run record lacks a usable authoritative tenant or exact provenance-bearing CAS ref.
    ProvenanceRefused { detail: String },
    /// The URI tenant does not equal the authoritative run-record tenant.
    TenantMismatch { record: String, reference: String },
    /// A legacy or unsupported snapshot must be rebuilt by current dispatch.
    RedispatchRequired(RedispatchReason),
    /// The CAS metadata/read operation failed.
    Blob(BlobError),
    /// The advertised or returned object is above the hard size ceiling.
    SnapshotTooLarge { actual: usize, maximum: usize },
    /// The CAS metadata did not describe the requested address.
    MetadataAddressMismatch,
    /// Version-1 JSON was malformed, non-canonical, or carried an unknown field.
    WireMalformed { detail: String },
    /// A semantic plan invariant failed.
    InvalidPlan { detail: String },
}

impl RunPlanError {
    /// Whether retrying the same snapshot can never make it executable and a fresh dispatch is
    /// required.
    pub fn requires_redispatch(&self) -> bool {
        matches!(self, RunPlanError::RedispatchRequired(_))
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
            RunPlanError::Blob(error) => write!(f, "CI run-plan blob access failed: {error}"),
            RunPlanError::SnapshotTooLarge { actual, maximum } => write!(
                f,
                "CI run-plan snapshot is {actual} bytes, above the {maximum}-byte limit"
            ),
            RunPlanError::MetadataAddressMismatch => {
                write!(f, "CI run-plan CAS metadata address mismatch")
            }
            RunPlanError::WireMalformed { detail } => {
                write!(f, "malformed CI run-plan v1 wire: {detail}")
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

/// Load and prepare the exact snapshot referenced by a durable [`CiRunRecord`].
///
/// URI and tenant provenance checks complete before `head`; `head` completes and its size is checked
/// before `get`. The storage implementation then provides re-hash-on-read integrity verification.
pub fn load_resolved_run_plan<B: BlobStore + ?Sized>(
    blobs: &B,
    run: &CiRunRecord,
) -> Result<PreparedRunPlan, RunPlanError> {
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
    let plan = decode_plan(&bytes)?;

    Ok(PreparedRunPlan {
        tenant,
        content_hash,
        plan,
    })
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

fn decode_plan(bytes: &[u8]) -> Result<ResolvedRunPlanV1, RunPlanError> {
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
    if version != u64::from(RUN_PLAN_SCHEMA_V1) {
        return Err(RunPlanError::RedispatchRequired(
            RedispatchReason::UnsupportedVersion(version),
        ));
    }

    let plan: ResolvedRunPlanV1 =
        serde_json::from_value(envelope).map_err(|error| RunPlanError::WireMalformed {
            detail: error.to_string(),
        })?;
    validate_plan(&plan)?;
    let canonical = serde_json::to_vec(&plan).map_err(|error| RunPlanError::WireMalformed {
        detail: error.to_string(),
    })?;
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
            let degree = indegree
                .get_mut(dependent)
                .expect("dependent names were validated above");
            *degree -= 1;
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
