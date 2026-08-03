//! Strict loader for a queued CI run's immutable, tenant-scoped execution-plan snapshot.
//!
//! This is intentionally a preparation boundary, not an execution boundary. The wire contains only
//! resolved DAG facts. Runtime authority (tokens, secrets, trust, mounts, egress, and resource
//! grants) must be supplied later by policy-aware components and cannot be smuggled through the CAS
//! document.

use std::collections::{BTreeMap, BTreeSet};

use myelin_ci_sandbox::gvisor::{CARGO_SOURCE_REPLACE_CONFIG, CARGO_VENDOR_DIRECTORY_CONFIG};
use myelin_ci_sandbox::ImageRef;
use myelin_storage::{BlobError, BlobStore, ContentHash};
use myelin_tenancy::TenantId;
use serde::{Deserialize, Serialize};

use crate::ci_run_store::CiRunRecord;

/// The legacy DAG-only schema version the current PostgreSQL starter can execute.
pub const RUN_PLAN_SCHEMA_V1: u32 = 1;
/// The resolved request schema that preserves authored stages and names the requested execution
/// profile without granting any runtime authority.
pub const RUN_PLAN_SCHEMA_V2: u32 = 2;
/// Version of the execution-profile request nested in a version-2 run plan.
pub const EXECUTION_REQUEST_SCHEMA_V1: u32 = 1;
/// Frozen domain for the version-1 launch-request digest.
pub const LAUNCH_REQUEST_DIGEST_V1_DOMAIN: &str = "myelin.ci.launch-request.v1";
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
/// Maximum argv entries in a structured build request (the tool itself is platform-supplied).
pub const MAX_STRUCTURED_BUILD_ARGS: usize = 16;
/// Maximum UTF-8 byte length of one structured build argument.
pub const MAX_STRUCTURED_BUILD_ARG_BYTES: usize = 256;
/// Server-controlled Cargo home used by structured Cargo build jobs. It is writable because it
/// lives under the sandbox's size-bounded `/tmp`; the server-owned `config.toml` inside it is a
/// separate read-only bind mount installed by the gVisor launch boundary.
pub const PLATFORM_CARGO_HOME: &str = myelin_ci_sandbox::gvisor::STRUCTURED_CARGO_HOME;
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

/// The execution profiles that a version-2 authored request can name.
///
/// This is a request, not a server grant: it carries no trust, token, secret, egress, workspace,
/// resource, scheduling, metering, or check authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CiExecutionProfileV1 {
    #[serde(rename = "linux-small-v1")]
    LinuxSmallV1,
    #[serde(rename = "linux-build-v1")]
    LinuxBuildV1,
}

impl CiExecutionProfileV1 {
    /// Parse a profile from its canonical `linux-<class>-v1` label (identical to the serde rename and
    /// to the runner label the launch authority stamps); `None` for any unrecognized label. The
    /// runner composition uses this to turn `MYELIN_CI_RUNNER_EXECUTION_PROFILES` into the profile set
    /// it advertises labels for.
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "linux-small-v1" => Some(Self::LinuxSmallV1),
            "linux-build-v1" => Some(Self::LinuxBuildV1),
            _ => None,
        }
    }
}

/// Versioned authored execution request nested in a version-2 resolved plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CiExecutionRequestV1 {
    /// Must equal [`EXECUTION_REQUEST_SCHEMA_V1`].
    pub schema_version: u32,
    /// Requested profile. Policy-aware control-plane code must still grant or refuse it later.
    pub profile: CiExecutionProfileV1,
}

/// Version-2 resolved run-plan wire. Field order is part of its canonical JSON representation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedRunPlanV2 {
    /// Must equal [`RUN_PLAN_SCHEMA_V2`].
    pub schema_version: u32,
    /// Authored profile request, never server launch authority.
    pub execution: CiExecutionRequestV1,
    /// Resolved jobs, sorted strictly by concrete [`ResolvedJobV2::name`].
    pub jobs: Vec<ResolvedJobV2>,
}

/// One resolved version-2 DAG node.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedJobV2 {
    /// Authored DAG-stage name and check context. Matrix instances intentionally share it.
    pub stage: String,
    /// Unique concrete matrix-expanded DAG-node name.
    pub name: String,
    /// Digest-pinned image request.
    pub image: String,
    /// Exact argv request. No shell or fallback executable is inferred.
    pub command: Vec<String>,
    /// Optional platform-invoked build recipe. Legacy V2 producers omit this field, preserving their
    /// canonical bytes exactly. Exactly one of a non-empty `command` or `build` is accepted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<StructuredBuildV1>,
    /// Concrete DAG-node dependencies, sorted strictly.
    pub needs: Vec<String>,
    /// Reserved generator marker. Version 2 refuses `true` until fragment ingestion exists.
    pub is_generator: bool,
    /// Deterministically ordered resolved matrix axes.
    pub matrix_key: BTreeMap<String, String>,
}

/// Bounded tool identifiers supported by the platform-owned structured build vehicle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StructuredBuildToolV1 {
    Cargo,
}

/// A tenant-authored build recipe whose executable, environment, and accepted argument grammar are
/// owned by the platform. The Cargo grammar is a strict closed allowlist
/// ([`CARGO_RECIPE_ALLOWLIST`]) of `--locked`, offline-safe recipes — `build`, unit `test --lib`
/// (optionally `--workspace`), and `clippy --all-targets -- -D warnings` — so tenant options such as
/// `--config` cannot reopen the Cargo boundary. [`Self::platform_argv`] inserts the server-owned
/// source overrides (before any `--` driver separator);
/// `[patch]`, `[replace]`, `paths`, and path/git dependencies can resolve only to code already in
/// the tenant's own workspace while offline, so their acceptability is an attestation-policy
/// concern rather than a dependency-fetch escape.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructuredBuildV1 {
    pub tool: StructuredBuildToolV1,
    pub args: Vec<String>,
}

impl StructuredBuildV1 {
    /// Validate this recipe against the platform's bounded, non-shell argument grammar.
    pub fn validate_for_job(&self, job_name: &str) -> Result<(), RunPlanError> {
        validate_structured_build(job_name, self)
    }

    /// Construct the exact direct argv executed by the sandbox. No shell executable or `-c` program
    /// is accepted from the tenant or inserted by this translation.
    ///
    /// The platform's two vendor `--config` pairs are inserted immediately BEFORE the first `--`
    /// separator in the recipe, if any. A `--` in a Cargo recipe (e.g. `clippy ... -- -D warnings`)
    /// forwards every following token to the compiler/driver, so appending `--config` after it would
    /// hand the platform source overrides to `rustc` instead of Cargo. Recipes without a `--`
    /// (`build`/`test`) keep the historical trailing-append output byte-for-byte.
    pub fn platform_argv(&self) -> Vec<String> {
        match self.tool {
            StructuredBuildToolV1::Cargo => {
                let vendor = [
                    "--config".to_owned(),
                    CARGO_SOURCE_REPLACE_CONFIG.to_owned(),
                    "--config".to_owned(),
                    CARGO_VENDOR_DIRECTORY_CONFIG.to_owned(),
                ];
                let mut argv = Vec::with_capacity(1 + self.args.len() + vendor.len());
                argv.push("cargo".to_owned());
                match self.args.iter().position(|arg| arg == "--") {
                    Some(split) => {
                        argv.extend(self.args[..split].iter().cloned());
                        argv.extend(vendor);
                        argv.extend(self.args[split..].iter().cloned());
                    }
                    None => {
                        argv.extend(self.args.iter().cloned());
                        argv.extend(vendor);
                    }
                }
                argv
            }
        }
    }
}

/// Public carrier for either canonical resolved-plan wire version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VersionedResolvedRunPlan {
    V1(ResolvedRunPlanV1),
    V2(ResolvedRunPlanV2),
}

/// Derive the exact concrete DAG-node name from an authored stage and its sorted matrix identity.
/// Empty matrix identities retain the stage byte-for-byte; matrix identities are length-framed and
/// BLAKE3-bound so distinct assignments cannot alias.
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

impl ResolvedJobV2 {
    /// Collision-safe deterministic identity bytes for this resolved matrix assignment.
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

impl ResolvedRunPlanV2 {
    /// Validate semantics and return the deterministic compact JSON representation.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, RunPlanError> {
        validate_plan_v2(self)?;
        canonical_json(self)
    }

    /// Digest the complete ordered launch request without converting it into runtime authority.
    ///
    /// The BLAKE3 derive-key input is one `u64`-big-endian length-prefixed frame for the canonical
    /// execution request followed by one frame for each canonical job in deterministic plan order.
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

/// A fully parsed and validated plan, still carrying its authoritative tenant and content address.
/// Its fields are private so callers cannot accidentally replace validation with struct literals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedRunPlan {
    tenant: TenantId,
    content_hash: ContentHash,
    plan: ResolvedRunPlanV1,
}

/// Validated version-2 launch request. This is still customer input, not authority: callers must
/// combine it with a policy-produced launch grant before any job can be materialized.
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
    /// The current V1-only starter encountered a valid V2 request but launch authority is absent.
    LaunchAuthorityRequired { version: u32 },
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

/// Load and prepare the exact snapshot referenced by a durable [`CiRunRecord`].
///
/// URI and tenant provenance checks complete before `head`; `head` completes and its size is checked
/// before `get`. The storage implementation then provides re-hash-on-read integrity verification.
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

/// Load the exact canonical V2 request for the launch-authority boundary. V1 remains a compatibility
/// wire but cannot enter the manifest-backed production path because it lacks authored stage and
/// execution-profile semantics.
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

/// Decode either supported canonical plan wire without granting it execution authority.
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
            let recipe: Vec<&str> = build.args.iter().map(String::as_str).collect();
            if CARGO_RECIPE_ALLOWLIST.contains(&recipe.as_slice()) {
                Ok(())
            } else {
                invalid(format!(
                    "job `{job_name}` Cargo recipe is not in the platform allowlist; admitted recipes are \
                     `build --locked`, `test --locked --lib`, `test --locked --lib --workspace`, and \
                     `clippy --locked --all-targets -- -D warnings`"
                ))
            }
        }
    }
}

/// The exact, closed set of tenant-supplied Cargo recipes the platform will lower and run. Every
/// entry is `--locked` (no network resolution), offline-safe under `--network=none`, and cannot
/// reopen the Cargo source boundary: the tenant supplies only this leading recipe and
/// [`StructuredBuildV1::platform_argv`] owns the vendor `--config` suffix. Widen ONLY by adding a
/// fixed vector here; never by admitting free-form tokens, `--config`, `--target-dir`, path/patch
/// options, or any tenant-chosen flag.
///
/// - `build --locked` — the original compile recipe (argv unchanged).
/// - `test --locked --lib` — unit tests only. `--lib` is REQUIRED: integration/`--test` targets
///   routinely need live network backends, which the `--network=none` sandbox blocks, so they are
///   deliberately not admitted.
/// - `test --locked --lib --workspace` — the same, fanned across every workspace member's lib tests.
/// - `clippy --locked --all-targets -- -D warnings` — lint every target and fail on any warning. The
///   `-- -D warnings` tail is a compiler-driver flag; `platform_argv` inserts the vendor `--config`
///   pairs before the `--` so they reach Cargo, not `rustc`.
const CARGO_RECIPE_ALLOWLIST: &[&[&str]] = &[
    &["build", "--locked"],
    &["test", "--locked", "--lib"],
    &["test", "--locked", "--lib", "--workspace"],
    &[
        "clippy",
        "--locked",
        "--all-targets",
        "--",
        "-D",
        "warnings",
    ],
];

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
