pub mod asset_registry;
pub mod canonical_tar;
mod checkout_authorization;
pub mod checkout_orchestration;
mod dirlock;
pub mod escape_corpus;
pub mod events;
pub mod firecracker;
pub mod gvisor;

/// True when this test environment cannot express distinct-uid semantics
/// (euid 0 - e.g. this crate's own lib tests running inside myelin's CI
/// sandbox). Callers skip LOUDLY; MYELIN_REQUIRE_USERNS_TESTS=1 turns the
/// skip into a hard failure on hosts that must prove the semantics.
#[cfg(test)]
pub(crate) fn fake_root_test_environment_skip(context: &str) -> bool {
    if unsafe { libc::geteuid() } != 0 {
        return false;
    }
    if std::env::var_os("MYELIN_REQUIRE_USERNS_TESTS").is_some() {
        panic!("MYELIN_REQUIRE_USERNS_TESTS=1 but this environment reports euid=0 ({context})");
    }
    eprintln!("SKIP (loud, NOT a silent pass): euid=0 cannot express {context}");
    true
}

pub mod hardening;
mod launch_gate;
pub mod notif_rules;
pub mod redaction;
pub use redaction::{ResolvedJobSecrets, ResolvedSecretEnv, SecretInjectionError};
pub mod replay;
pub mod rootfs_overlay;
pub mod runner;
pub mod self_hosted;
pub mod snapshot_pool;
mod sync;
pub mod user_namespace;
mod workspace_intent;
pub mod workspace_manager;
pub mod workspace_storage;

pub use events::{
    ci_event_tokens, is_durable, register_ci_tokens, CI_DURABLE_TOKENS, CI_FIREHOSE_TOKENS,
};
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub use launch_gate::launch_gate_parent_death_probe;
pub use replay::CiReindexSource;

pub use notif_rules::{
    ci_notif_rules, ci_summary, register_ci_notif_rules, register_ci_summary_templates,
    summary_template_key, CheckVerdict, CiSummary, CI_CHECK_STATUS_RULE, CI_SUMMARY_TEMPLATES,
};

pub use escape_corpus::{
    build_corpus_script, parse_console, AttackFamily, AttackMarker, AttackOutcome, Backend,
    BackendRun, DrillReport, EscapeAttestation, CORPUS, CORPUS_VERSION,
};

pub use self_hosted::{
    mint_self_hosted_token, self_hosted_grant, AttestState, Attestation, AttestationVerifier,
    SelfHostedMintError, SelfHostedRunner, StructuralAttestationVerifier, TenantScopedToken,
    SELFHOSTED_GRANT_PREFIX,
};

pub use snapshot_pool::{
    AcquirePath, ModeledRestore, PoolStats, SnapshotPool, SnapshotRestore, WarmSandbox,
};

pub use gvisor::{
    build_gvisor_corpus_script, gvisor_drill_config_json, resolved_gvisor_rootfs,
    GVISOR_CORPUS_SCRIPT,
};

pub use asset_registry::{
    cargo_lock_sha256_hex, cargo_vendor_smoke_reference, cargo_vendor_workspace_reference,
    file_sha256_hex, resolved_gvisor_cargo_vendor, resolved_gvisor_cargo_vendor_workspace,
    select_registered_cargo_vendor, AssetRegistryError, CargoVendorAssetBinding,
    GvisorAssetRegistry, RootfsAssetBinding, VerifiedCargoVendor, VerifiedRootfs,
    CARGO_VENDOR_SMOKE_LOCK_SHA256, CARGO_VENDOR_SMOKE_TREE_SHA256,
    CARGO_VENDOR_WORKSPACE_LOCK_SHA256, CARGO_VENDOR_WORKSPACE_TREE_SHA256,
    ENV_GVISOR_CARGO_VENDOR, ENV_GVISOR_CARGO_VENDOR_WORKSPACE,
};
pub use canonical_tar::canonical_tree_sha256_hex;
pub use gvisor::{
    resolved_gvisor_rust_rootfs, ENV_GVISOR_RUST_ROOTFS, GVISOR_GIT_ROOTFS_SHA256,
    LINUX_RUST_V1_ROOTFS_SHA256, LINUX_SMALL_V1_ROOTFS_SHA256,
};

pub use gvisor::{
    assert_repo_under_root, resolve_bare_repo_path, resolved_gvisor_git_rootfs,
    validate_wire_repo_slug, validate_wire_segment, verified_gvisor_git_rootfs, GitWireSpec,
    MemoryCgroup, WireError, ENV_GVISOR_GIT_ROOTFS, ENV_RUNSC_BIN, WIRE_QUARANTINE_MOUNT,
    WIRE_REPO_MOUNT, WIRE_STDIN_BOUND,
};

pub use runner::{
    CompletionClaim, CountingFirehose, EngineTerminalReporter, FirehoseSink, JobLeaseStore,
    LeaseStore, PreparationAttemptDisposition, PreparationLeaseCheckpoint, PreparationLeaseLost,
    PreparationOutcomeDispatch, PreparationPhase, PreparationReportClaim, PreparationRetryReport,
    PreparationTerminalDisposition, QueuedJob, RetryableAttemptCause, RetryableAttemptFailure,
    RetryableAttemptOutcome, RunOutcome, RunnerAgent, RunnerCycleOutcome, RunnerError,
    TerminalReport, TerminalReporter,
};

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub use myelin_tenancy::Region;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobSpec {
    pub kind: JobKind,
    pub image: ImageRef,
    pub command: Vec<String>,
    pub env: Vec<EnvVar>,
    pub secret_refs: Vec<SecretRef>,
    resolved_secrets: ResolvedJobSecrets,
    pub egress: EgressPolicy,
    pub limits: ResourceLimits,
    pub workspace: WorkspaceSpec,
    pub trust_tier: TrustTier,
    pub run_token: RunTokenCredential,
    pub run_token_authorization: Option<RunTokenAuthorizationContext>,
    pub meter_to: MeterTarget,
    pub idem_token: IdemToken,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobSpecTemplate {
    pub kind: JobKind,
    pub image: ImageRef,
    pub command: Vec<String>,
    pub env: Vec<EnvVar>,
    pub secret_refs: Vec<SecretRef>,
    pub egress: EgressPolicy,
    pub limits: ResourceLimits,
    pub workspace: WorkspaceSpec,
    pub trust_tier: TrustTier,
    pub meter_to: MeterTarget,
    pub idem_token: IdemToken,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobKind {
    Ci,
    Agent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrustTier {
    Trusted,
    UntrustedFork,
    SelfHosted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageRef {
    pub reference: String,
}

impl ImageRef {
    pub fn pinned(reference: impl Into<String>) -> Result<ImageRef, SpecError> {
        let reference = reference.into();
        let r = ImageRef { reference };
        if r.digest_pinned() {
            Ok(r)
        } else {
            Err(SpecError::UndigestedImage {
                reference: r.reference,
            })
        }
    }

    pub fn digest_pinned(&self) -> bool {
        self.parse_digest().is_some()
    }

    pub(crate) fn parse_digest(&self) -> Option<(&str, &str)> {
        let (_, after_at) = self.reference.rsplit_once('@')?;
        let (algo, digest) = after_at.split_once(':')?;
        let expected_hex_len = Self::digest_hex_len(algo)?;
        if !digest.is_empty()
            && digest.len() == expected_hex_len
            && digest.chars().all(|c| c.is_ascii_hexdigit())
        {
            Some((algo, digest))
        } else {
            None
        }
    }

    fn digest_hex_len(algo: &str) -> Option<usize> {
        match algo {
            "sha256" => Some(64),
            "sha384" => Some(96),
            "sha512" => Some(128),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvVar {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretRef {
    pub name: String,
    pub handle: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressPolicy {
    pub allow: Vec<String>,
}

impl EgressPolicy {
    pub fn deny_all() -> EgressPolicy {
        EgressPolicy::default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceLimits {
    pub cpu_millis: u32,
    pub mem_bytes: u64,
    pub disk_bytes: u64,
    pub tmpfs_bytes: u64,
    pub pids_max: u32,
    pub timeout_secs: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSpec {
    pub repo_ref: Option<String>,
    pub commit: Option<String>,
}

pub fn derive_checkout_authorization_scope(
    kind: JobKind,
    workspace: &WorkspaceSpec,
) -> Result<Option<CheckoutAuthorizationScope>, String> {
    match workspace_intent::derive_workspace_intent(kind, workspace)? {
        workspace_intent::WorkspaceIntent::Compute => Ok(None),
        workspace_intent::WorkspaceIntent::Checkout(request) => {
            Ok(Some(request.to_authorization_scope()))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunTokenRef {
    pub jti: String,
}

#[derive(Clone, PartialEq, Eq)]
pub struct RunTokenCredential {
    pub jti: String,
    bearer: String,
    ttl_secs: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunTokenAuthorizationContext {
    CiJob(CiJobAuthorizationContext),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiJobAuthorizationContext {
    pub tenant_id: String,
    pub region: String,
    pub principal_id: String,
    pub project_id: String,
    pub wf_run_id: String,
    pub job_id: String,
    pub lease_owner: String,
    pub lease_epoch: i64,
    pub claim_nonce: String,
    pub claim_started_at_epoch_secs: i64,
    pub claim_expires_at_epoch_secs: i64,
    pub reserve_id: String,
    pub required_capabilities: Vec<String>,
    pub checkout_scope: Option<CheckoutAuthorizationScope>,
    pub credential_binding: Option<CiJobCredentialBinding>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiJobCredentialBinding {
    pub binding_version: i16,
    pub purpose: String,
    pub generation_id: String,
    pub issued_at_epoch_secs: i64,
    pub expires_at_epoch_secs: i64,
    pub ci_run_id: String,
    pub token_authority_handle: String,
    pub idem_token: String,
}

impl RunTokenCredential {
    const MAX_BEARER_BYTES: usize = 64 * 1024;

    pub fn new(
        bearer: impl Into<String>,
        jti: impl Into<String>,
        ttl_secs: u64,
    ) -> Result<Self, RunTokenCredentialError> {
        use zeroize::Zeroize;

        let mut bearer = bearer.into();
        let mut jti = jti.into();
        let invalid = if bearer.trim().is_empty() {
            Some(RunTokenCredentialError::EmptyBearer)
        } else if bearer.len() > Self::MAX_BEARER_BYTES {
            Some(RunTokenCredentialError::BearerTooLong)
        } else if jti.trim().is_empty() || jti.len() > 512 {
            Some(RunTokenCredentialError::InvalidJti)
        } else if ttl_secs == 0 {
            Some(RunTokenCredentialError::NonPositiveTtl)
        } else {
            None
        };
        if let Some(error) = invalid {
            bearer.zeroize();
            jti.zeroize();
            return Err(error);
        }
        Ok(Self {
            jti,
            bearer,
            ttl_secs,
        })
    }

    pub fn expose_bearer(&self) -> &str {
        &self.bearer
    }

    pub fn ttl_secs(&self) -> u64 {
        self.ttl_secs
    }

    pub fn reference(&self) -> RunTokenRef {
        RunTokenRef {
            jti: self.jti.clone(),
        }
    }
}

impl std::fmt::Debug for RunTokenCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunTokenCredential")
            .field("jti", &"<redacted>")
            .field("bearer", &"<redacted>")
            .field("ttl_secs", &self.ttl_secs)
            .finish()
    }
}

impl Drop for RunTokenCredential {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.bearer.zeroize();
        self.jti.zeroize();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunTokenCredentialError {
    EmptyBearer,
    BearerTooLong,
    InvalidJti,
    NonPositiveTtl,
}

impl std::fmt::Display for RunTokenCredentialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyBearer => f.write_str("run-token bearer material must be non-empty"),
            Self::BearerTooLong => {
                f.write_str("run-token bearer material must be at most 65536 bytes")
            }
            Self::InvalidJti => {
                f.write_str("run-token JTI must be non-empty and at most 512 bytes")
            }
            Self::NonPositiveTtl => f.write_str("run-token TTL must be positive"),
        }
    }
}

impl std::error::Error for RunTokenCredentialError {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeterTarget {
    pub reserve_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdemToken(pub String);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpecError {
    UndigestedImage { reference: String },
    NoPidsMax,
    NoTimeout,
}

impl std::fmt::Display for SpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpecError::UndigestedImage { reference } => write!(
                f,
                "image `{reference}` is not digest-pinned - an un-digested tag is rejected \
                 fail-closed (CI-1; arch 02 §5.3). Pin by `@<algo>:<hexdigest>`."
            ),
            SpecError::NoPidsMax => write!(
                f,
                "ResourceLimits.pids_max is 0 - the fork-bomb ceiling MUST be set (arch 02 §5.3)."
            ),
            SpecError::NoTimeout => write!(
                f,
                "ResourceLimits.timeout_secs is 0 - every job MUST have a wall-clock timeout \
                 (arch 02 §5.3)."
            ),
        }
    }
}

impl std::error::Error for SpecError {}

impl JobSpec {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        kind: JobKind,
        image: ImageRef,
        command: Vec<String>,
        env: Vec<EnvVar>,
        secret_refs: Vec<SecretRef>,
        egress: EgressPolicy,
        limits: ResourceLimits,
        workspace: WorkspaceSpec,
        trust_tier: TrustTier,
        run_token: RunTokenCredential,
        meter_to: MeterTarget,
        idem_token: IdemToken,
    ) -> Result<JobSpec, SpecError> {
        validate_job_shape(&image, &limits)?;
        Ok(JobSpec {
            kind,
            image,
            command,
            env,
            secret_refs,
            resolved_secrets: ResolvedJobSecrets::default(),
            egress,
            limits,
            workspace,
            trust_tier,
            run_token,
            run_token_authorization: None,
            meter_to,
            idem_token,
        })
    }

    pub fn into_template(self) -> (JobSpecTemplate, RunTokenCredential) {
        let template = JobSpecTemplate {
            kind: self.kind,
            image: self.image,
            command: self.command,
            env: self.env,
            secret_refs: self.secret_refs,
            egress: self.egress,
            limits: self.limits,
            workspace: self.workspace,
            trust_tier: self.trust_tier,
            meter_to: self.meter_to,
            idem_token: self.idem_token,
        };
        (template, self.run_token)
    }

    pub fn with_resolved_secrets(
        mut self,
        bindings: Vec<ResolvedSecretEnv>,
    ) -> Result<Self, SecretInjectionError> {
        self.resolved_secrets = ResolvedJobSecrets::for_job(&self, bindings)?;
        Ok(self)
    }

    pub fn validate_secret_coverage(&self) -> Result<(), SecretInjectionError> {
        self.resolved_secrets.validate_for_job(self)
    }

    pub fn resolved_secret_count(&self) -> usize {
        self.resolved_secrets.len()
    }

    pub(crate) fn resolved_secrets(&self) -> &ResolvedJobSecrets {
        &self.resolved_secrets
    }
}

impl JobSpecTemplate {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        kind: JobKind,
        image: ImageRef,
        command: Vec<String>,
        env: Vec<EnvVar>,
        secret_refs: Vec<SecretRef>,
        egress: EgressPolicy,
        limits: ResourceLimits,
        workspace: WorkspaceSpec,
        trust_tier: TrustTier,
        meter_to: MeterTarget,
        idem_token: IdemToken,
    ) -> Result<Self, SpecError> {
        validate_job_shape(&image, &limits)?;
        Ok(Self {
            kind,
            image,
            command,
            env,
            secret_refs,
            egress,
            limits,
            workspace,
            trust_tier,
            meter_to,
            idem_token,
        })
    }

    pub fn resolve(self, run_token: RunTokenCredential) -> JobSpec {
        self.resolve_with_authorization(run_token, None)
    }

    pub fn resolve_with_authorization(
        self,
        run_token: RunTokenCredential,
        run_token_authorization: Option<RunTokenAuthorizationContext>,
    ) -> JobSpec {
        JobSpec {
            kind: self.kind,
            image: self.image,
            command: self.command,
            env: self.env,
            secret_refs: self.secret_refs,
            resolved_secrets: ResolvedJobSecrets::default(),
            egress: self.egress,
            limits: self.limits,
            workspace: self.workspace,
            trust_tier: self.trust_tier,
            run_token,
            run_token_authorization,
            meter_to: self.meter_to,
            idem_token: self.idem_token,
        }
    }
}

fn validate_job_shape(image: &ImageRef, limits: &ResourceLimits) -> Result<(), SpecError> {
    if !image.digest_pinned() {
        return Err(SpecError::UndigestedImage {
            reference: image.reference.clone(),
        });
    }
    validate_execution_limits(limits).map_err(|_| {
        if limits.pids_max == 0 {
            SpecError::NoPidsMax
        } else {
            SpecError::NoTimeout
        }
    })
}

pub(crate) fn validate_execution_limits(limits: &ResourceLimits) -> Result<(), String> {
    if limits.pids_max == 0 {
        return Err("pids_max (fork-bomb ceiling) must be set (> 0)".to_string());
    }
    if limits.timeout_secs == 0 {
        return Err("timeout_secs must be set (> 0)".to_string());
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxHandle {
    pub guest_id: String,
}

pub const SANDBOX_CAPTURE_BOUND: usize = 256 * 1024;

pub(crate) fn drain_capped<R: std::io::Read>(mut r: R, limit: usize) -> (Vec<u8>, bool) {
    let mut head = Vec::new();
    let mut truncated = false;
    let mut chunk = [0u8; 64 * 1024];
    loop {
        match r.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if head.len() < limit {
                    let take = (limit - head.len()).min(n);
                    head.extend_from_slice(&chunk[..take]);
                    if take < n {
                        truncated = true;
                    }
                } else {
                    truncated = true;
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => {
                truncated = true;
                break;
            }
        }
    }
    (head, truncated)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxResult {
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub usage: ResourceUsage,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl SandboxResult {
    pub fn passed(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out
    }

    pub fn stub_ok(usage: ResourceUsage) -> SandboxResult {
        SandboxResult {
            exit_code: Some(0),
            timed_out: false,
            usage,
            stdout: Vec::new(),
            stderr: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxLaunch {
    pub handle: SandboxHandle,
    pub result: SandboxResult,
    pub output_complete: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SandboxOutputStream {
    Stdout,
    Stderr,
}

pub trait SandboxOutputSink: Send + Sync {
    fn emit(&self, stream: SandboxOutputStream, frame: &[u8]) -> Result<(), String>;
}

#[derive(Clone, Default)]
pub struct SandboxCancellation {
    cancelled: Arc<AtomicBool>,
}

impl SandboxCancellation {
    pub fn new() -> SandboxCancellation {
        SandboxCancellation::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub(crate) fn as_atomic(&self) -> &AtomicBool {
        self.cancelled.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxLaunchError<E> {
    Failed(E),

    RetryableAttempt {
        source: E,
        cause: crate::runner::RetryableAttemptCause,
        usage: ResourceUsage,
    },

    DurableOutcomeUnknown(E),
}

impl<E: std::fmt::Display> std::fmt::Display for SandboxLaunchError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SandboxLaunchError::Failed(e) => write!(f, "{e}"),
            SandboxLaunchError::RetryableAttempt { source, cause, .. } => {
                write!(
                    f,
                    "retryable sandbox infrastructure attempt ({}): {source}",
                    cause.as_storage_token()
                )
            }
            SandboxLaunchError::DurableOutcomeUnknown(e) => {
                write!(
                    f,
                    "durable launch outcome unknown (needs reconciliation): {e}"
                )
            }
        }
    }
}

impl<E: std::fmt::Debug + std::fmt::Display> std::error::Error for SandboxLaunchError<E> {}

pub trait SandboxBackend: Sync {
    type Error: std::error::Error + Send;

    fn launch(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
    ) -> Result<SandboxLaunch, SandboxLaunchError<Self::Error>>;

    fn launch_streaming(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
        output: Arc<dyn SandboxOutputSink>,
        _cancellation: SandboxCancellation,
    ) -> Result<SandboxLaunch, SandboxLaunchError<Self::Error>> {
        let mut launch = self.launch(spec, hooks)?;
        let stdout_complete = launch.result.stdout.is_empty()
            || output
                .emit(SandboxOutputStream::Stdout, &launch.result.stdout)
                .is_ok();
        let stderr_complete = launch.result.stderr.is_empty()
            || output
                .emit(SandboxOutputStream::Stderr, &launch.result.stderr)
                .is_ok();
        launch.output_complete &= stdout_complete && stderr_complete;
        Ok(launch)
    }

    fn run_cycle(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
        output: Arc<dyn SandboxOutputSink>,
        cancellation: SandboxCancellation,
    ) -> Result<SandboxCycleOutcome, SandboxLaunchError<Self::Error>> {
        self.launch_streaming(spec, hooks, output, cancellation)
            .map(SandboxCycleOutcome::WorkloadLaunched)
    }

    fn kill(&self, h: &SandboxHandle) -> Result<(), Self::Error>;

    fn accept_async(&self, spec: &JobSpec) -> Result<(), Self::Error> {
        let _ = spec;
        Ok(())
    }
}

#[derive(Debug)]
pub enum SandboxCycleOutcome {
    WorkloadLaunched(SandboxLaunch),
    WorkloadRetryable {
        cause: crate::runner::RetryableAttemptCause,
        usage: ResourceUsage,
        message: String,
    },
    PreparationTerminal {
        claim: crate::runner::PreparationReportClaim,
        disposition: crate::runner::PreparationTerminalDisposition,
        diagnostic: Option<String>,
    },
    PreparationRetryable {
        claim: crate::runner::PreparationReportClaim,
        phase: crate::runner::PreparationPhase,
    },
    ReconciliationRequired {
        phase: crate::runner::PreparationPhase,
        teardown_unproven: bool,
        usage_unrepresentable: bool,
        quarantine_required: bool,
    },
}

impl From<crate::checkout_orchestration::CheckoutContinuationOutcome> for SandboxCycleOutcome {
    fn from(outcome: crate::checkout_orchestration::CheckoutContinuationOutcome) -> Self {
        use crate::checkout_orchestration::CheckoutContinuationOutcome as Cco;
        match outcome {
            Cco::WorkloadLaunched(launch) => Self::WorkloadLaunched(launch),
            Cco::WorkloadRetryable {
                cause,
                usage,
                message,
            } => Self::WorkloadRetryable {
                cause,
                usage,
                message,
            },
            Cco::PreparationTerminal {
                claim,
                disposition,
                diagnostic,
            } => Self::PreparationTerminal {
                claim,
                disposition,
                diagnostic,
            },
            Cco::PreparationRetryable { claim, phase } => {
                Self::PreparationRetryable { claim, phase }
            }
            Cco::ReconciliationRequired {
                phase,
                teardown_unproven,
                usage_unrepresentable,
                quarantine_required,
            } => Self::ReconciliationRequired {
                phase,
                teardown_unproven,
                usage_unrepresentable,
                quarantine_required,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerClass(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerHost {
    pub host_id: String,
    pub region: Region,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capacity {
    pub provisioned: u32,
    pub available: u32,
}

pub trait FleetProvider {
    type Error: std::error::Error;

    fn provision(
        &self,
        class: RunnerClass,
        n: u32,
        region: Region,
    ) -> Result<Vec<RunnerHost>, Self::Error>;

    fn deprovision(&self, hosts: &[RunnerHost]) -> Result<(), Self::Error>;

    fn capacity(&self, region: Region) -> Result<Capacity, Self::Error>;
}

pub struct RunnerHooks {
    completion_settlement_owner: CompletionSettlementOwner,
    reserve: ReserveHook,
    settle: SettleHook,
    attribute: AttributionHook,
    isolation_floor: IsolationFloorHook,
    checkout_phase_authorization: Option<CheckoutPhaseAuthorizationHook>,
    parent_attempt_reserve: Option<ParentAttemptReserveHook>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionSettlementOwner {
    Hook,
    TerminalReporter,
}

pub type ReserveHook = Box<dyn Fn(&JobSpec) -> Result<ReserveHandle, HookError> + Send + Sync>;
pub type ParentAttemptReserveHook = Box<
    dyn Fn(&JobSpec) -> Result<crate::checkout_orchestration::ParentAttemptAdmission, HookError>
        + Send
        + Sync,
>;
pub type SettleHook =
    Box<dyn Fn(&JobSpec, &ReserveHandle, ResourceUsage) -> Result<(), HookError> + Send + Sync>;
pub type AttributeHook = Box<dyn Fn(&JobSpec) -> Result<(), HookError> + Send + Sync>;
pub type LaunchFenceHook = Box<dyn Fn(&JobSpec) -> Result<LaunchPermit, HookError> + Send + Sync>;
pub type IsolationFloorHook = Box<dyn Fn(&JobSpec) -> Result<(), HookError> + Send + Sync>;

pub use crate::workspace_intent::GitObjectFormat;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckoutAuthorizationScope {
    tenant: myelin_tenancy::TenantId,
    repo_ref: myelin_events::ArtifactRef,
    repo_id: String,
    commit_hex: String,
    commit_format: GitObjectFormat,
}

impl CheckoutAuthorizationScope {
    pub(crate) fn new(
        tenant: myelin_tenancy::TenantId,
        repo_ref: myelin_events::ArtifactRef,
        repo_id: String,
        commit_hex: String,
        commit_format: GitObjectFormat,
    ) -> Self {
        Self {
            tenant,
            repo_ref,
            repo_id,
            commit_hex,
            commit_format,
        }
    }

    pub fn tenant(&self) -> &myelin_tenancy::TenantId {
        &self.tenant
    }

    pub fn repo_ref(&self) -> &myelin_events::ArtifactRef {
        &self.repo_ref
    }

    pub fn repo_id(&self) -> &str {
        &self.repo_id
    }

    pub fn commit_hex(&self) -> &str {
        &self.commit_hex
    }

    pub fn commit_format(&self) -> GitObjectFormat {
        self.commit_format
    }
}

pub type CheckoutPhaseAuthorizationHook = Box<
    dyn Fn(&JobSpec, &CheckoutAuthorizationScope, CheckoutPhase) -> Result<LaunchPermit, HookError>
        + Send
        + Sync,
>;

pub use checkout_authorization::CheckoutPhase;
pub(crate) use checkout_authorization::PhaseAuthorization;

enum AttributionHook {
    Immediate(AttributeHook),
    Fenced(LaunchFenceHook),
}

pub struct LaunchPermit {
    commit: Box<dyn FnOnce() -> Result<LaunchOwnership, HookError> + Send>,
}

impl LaunchPermit {
    pub fn immediate() -> Self {
        Self {
            commit: Box::new(|| Ok(LaunchOwnership::immediate())),
        }
    }

    pub fn retained(
        commit: impl FnOnce() -> Result<LaunchOwnership, HookError> + Send + 'static,
    ) -> Self {
        Self {
            commit: Box::new(commit),
        }
    }

    pub fn commit(self) -> Result<LaunchOwnership, HookError> {
        (self.commit)()
    }

    pub fn commit_and_release(self) -> Result<(), HookError> {
        self.commit()?.validate()?.release()
    }
}

#[must_use = "launch ownership must be released only at the sandbox exec boundary"]
pub struct LaunchOwnership {
    validate: Box<dyn FnOnce() -> Result<ValidatedLaunchOwnership, HookError> + Send>,
}

impl LaunchOwnership {
    pub fn immediate() -> Self {
        Self {
            validate: Box::new(|| Ok(ValidatedLaunchOwnership::immediate())),
        }
    }

    pub fn retained(
        validate: impl FnOnce() -> Result<ValidatedLaunchOwnership, HookError> + Send + 'static,
    ) -> Self {
        Self {
            validate: Box::new(validate),
        }
    }

    pub fn validate(self) -> Result<ValidatedLaunchOwnership, HookError> {
        (self.validate)()
    }
}

#[must_use = "validated launch ownership must be released after the sandbox gate opens"]
pub struct ValidatedLaunchOwnership {
    release: Box<dyn FnOnce() -> Result<(), HookError> + Send>,
}

impl ValidatedLaunchOwnership {
    pub fn immediate() -> Self {
        Self {
            release: Box::new(|| Ok(())),
        }
    }

    pub fn retained(release: impl FnOnce() -> Result<(), HookError> + Send + 'static) -> Self {
        Self {
            release: Box::new(release),
        }
    }

    pub fn release(self) -> Result<(), HookError> {
        (self.release)()
    }
}

impl RunnerHooks {
    pub fn new(
        completion_settlement_owner: CompletionSettlementOwner,
        reserve: ReserveHook,
        settle: SettleHook,
        attribute: AttributeHook,
        isolation_floor: IsolationFloorHook,
    ) -> Self {
        Self {
            completion_settlement_owner,
            reserve,
            settle,
            attribute: AttributionHook::Immediate(attribute),
            isolation_floor,
            checkout_phase_authorization: None,
            parent_attempt_reserve: None,
        }
    }

    pub fn new_with_launch_fence(
        completion_settlement_owner: CompletionSettlementOwner,
        reserve: ReserveHook,
        settle: SettleHook,
        attribute: LaunchFenceHook,
        isolation_floor: IsolationFloorHook,
    ) -> Self {
        Self {
            completion_settlement_owner,
            reserve,
            settle,
            attribute: AttributionHook::Fenced(attribute),
            isolation_floor,
            checkout_phase_authorization: None,
            parent_attempt_reserve: None,
        }
    }

    pub fn with_checkout_phase_authorization(
        mut self,
        hook: CheckoutPhaseAuthorizationHook,
    ) -> Self {
        self.checkout_phase_authorization = Some(hook);
        self
    }

    pub fn with_parent_attempt_reservation(mut self, hook: ParentAttemptReserveHook) -> Self {
        self.parent_attempt_reserve = Some(hook);
        self
    }

    pub(crate) fn reserve_parent_attempt(
        &self,
        spec: &JobSpec,
    ) -> Result<crate::checkout_orchestration::ParentAttemptAdmission, HookError> {
        match &self.parent_attempt_reserve {
            Some(hook) => hook(spec),
            None => Err(HookError(
                "checkout parent-attempt admission requires a configured parent-attempt reservation \
                 hook, but none was provided (this RunnerHooks selects the legacy reserve mode)"
                    .to_string(),
            )),
        }
    }

    pub fn completion_settlement_owner(&self) -> CompletionSettlementOwner {
        self.completion_settlement_owner
    }

    pub fn enforce_isolation_floor(&self, spec: &JobSpec) -> Result<(), HookError> {
        (self.isolation_floor)(spec)
    }

    pub fn reserve(&self, spec: &JobSpec) -> Result<ReserveHandle, HookError> {
        (self.reserve)(spec)
    }

    pub fn attribute(&self, spec: &JobSpec) -> Result<(), HookError> {
        self.acquire_launch_permit(spec)?.commit_and_release()
    }

    pub fn acquire_launch_permit(&self, spec: &JobSpec) -> Result<LaunchPermit, HookError> {
        match &self.attribute {
            AttributionHook::Immediate(attribute) => {
                attribute(spec)?;
                Ok(LaunchPermit::immediate())
            }
            AttributionHook::Fenced(attribute) => attribute(spec),
        }
    }

    pub fn with_attribute_guard(
        mut self,
        guard: impl Fn(&JobSpec) -> Result<(), HookError> + Send + Sync + 'static,
    ) -> Self {
        self.attribute = match self.attribute {
            AttributionHook::Immediate(attribute) => {
                AttributionHook::Immediate(Box::new(move |spec| {
                    guard(spec)?;
                    attribute(spec)
                }))
            }
            AttributionHook::Fenced(attribute) => AttributionHook::Fenced(Box::new(move |spec| {
                guard(spec)?;
                attribute(spec)
            })),
        };
        self
    }

    pub fn release_unused(&self, spec: &JobSpec, reserve: &ReserveHandle) -> Result<(), HookError> {
        (self.settle)(
            spec,
            reserve,
            ResourceUsage {
                cpu_seconds: 0,
                mem_byte_seconds: 0,
            },
        )
    }

    pub fn settle_completed(
        &self,
        spec: &JobSpec,
        reserve: &ReserveHandle,
        usage: ResourceUsage,
    ) -> Result<(), HookError> {
        match self.completion_settlement_owner {
            CompletionSettlementOwner::Hook => (self.settle)(spec, reserve, usage),
            CompletionSettlementOwner::TerminalReporter => Ok(()),
        }
    }
}

impl std::fmt::Debug for RunnerHooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunnerHooks")
            .field(
                "completion_settlement_owner",
                &self.completion_settlement_owner,
            )
            .field("reserve", &"<fn>")
            .field("settle", &"<fn>")
            .field("attribute", &"<fn>")
            .field("isolation_floor", &"<fn>")
            .finish()
    }
}

pub const fn hitl_withhold_note() -> &'static str {
    "side-effecting mutation never goes through the sandbox runner; it goes through \
     EffectApi::apply (contract 8.2) - the routing split is the safety boundary (X-6 #3 / AG-8)"
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReserveHandle(pub String);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceUsage {
    pub cpu_seconds: u64,
    pub mem_byte_seconds: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookError(pub String);

impl std::fmt::Display for HookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "runner hook failed: {}", self.0)
    }
}

impl std::error::Error for HookError {}

#[allow(clippy::too_many_arguments)]
pub fn agent_job(
    image: ImageRef,
    command: Vec<String>,
    env: Vec<EnvVar>,
    secret_refs: Vec<SecretRef>,
    egress: EgressPolicy,
    limits: ResourceLimits,
    trust_tier: TrustTier,
    run_token: RunTokenCredential,
    meter_to: MeterTarget,
    idem_token: IdemToken,
) -> Result<JobSpec, SpecError> {
    JobSpec::new(
        JobKind::Agent,
        image,
        command,
        env,
        secret_refs,
        egress,
        limits,
        WorkspaceSpec::default(),
        trust_tier,
        run_token,
        meter_to,
        idem_token,
    )
}

#[cfg(test)]
pub(crate) fn checkout_job_spec_for_tests() -> JobSpec {
    let mut spec = JobSpec::new(
        JobKind::Ci,
        ImageRef::pinned(format!("test.local/checkout@sha256:{}", "a".repeat(64))).unwrap(),
        vec!["true".into()],
        vec![],
        vec![],
        EgressPolicy { allow: vec![] },
        ResourceLimits {
            cpu_millis: 1000,
            mem_bytes: 256 << 20,
            disk_bytes: 1 << 30,
            tmpfs_bytes: 1 << 30,
            pids_max: 64,
            timeout_secs: 120,
        },
        WorkspaceSpec {
            repo_ref: Some("myelin://acme/git/repo/widgets".to_string()),
            commit: Some("a".repeat(40)),
        },
        TrustTier::UntrustedFork,
        RunTokenCredential::new("test-bearer", "advertise-jti", 300).unwrap(),
        MeterTarget {
            reserve_id: "r".into(),
        },
        IdemToken("idem-checkout-6c".into()),
    )
    .unwrap();
    spec.run_token_authorization = Some(RunTokenAuthorizationContext::CiJob(
        CiJobAuthorizationContext {
            tenant_id: "acme".to_string(),
            region: "fr-par".to_string(),
            principal_id: "p".to_string(),
            project_id: "00000000-0000-0000-0000-000000000001".to_string(),
            wf_run_id: "wf".to_string(),
            job_id: "j".to_string(),
            lease_owner: "o".to_string(),
            lease_epoch: 1,
            claim_nonce: "n".to_string(),
            claim_started_at_epoch_secs: 0,
            claim_expires_at_epoch_secs: 1,
            reserve_id: "r".to_string(),
            required_capabilities: vec![],
            checkout_scope: None,
            credential_binding: None,
        },
    ));
    spec
}

#[cfg(test)]
mod tests {
    use super::*;

    struct PrefixThenReadFault {
        prefix: Option<Vec<u8>>,
    }

    impl std::io::Read for PrefixThenReadFault {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if let Some(prefix) = self.prefix.take() {
                buf[..prefix.len()].copy_from_slice(&prefix);
                Ok(prefix.len())
            } else {
                Err(std::io::Error::other("injected stream read fault"))
            }
        }
    }

    #[test]
    fn capped_drain_marks_a_read_fault_as_incomplete() {
        let (head, truncated) = drain_capped(
            PrefixThenReadFault {
                prefix: Some(b"partial".to_vec()),
            },
            SANDBOX_CAPTURE_BOUND,
        );

        assert_eq!(head, b"partial");
        assert!(truncated);
    }

    fn digest() -> ImageRef {
        ImageRef::pinned("registry.example/img@sha256:abc123def4567890abc123def4567890abc123def4567890abc123def4567890").unwrap()
    }

    fn limits() -> ResourceLimits {
        ResourceLimits {
            cpu_millis: 1000,
            mem_bytes: 512 << 20,
            disk_bytes: 1 << 30,
            tmpfs_bytes: 1 << 30,
            pids_max: 256,
            timeout_secs: 600,
        }
    }

    fn credential(jti: &str) -> RunTokenCredential {
        RunTokenCredential::new(format!("test-bearer:{jti}"), jti, 300).unwrap()
    }

    #[test]
    fn queued_template_contains_no_token_and_resolves_only_at_claim() {
        let template = JobSpecTemplate::new(
            JobKind::Ci,
            digest(),
            vec!["test".into()],
            Vec::new(),
            Vec::new(),
            EgressPolicy::default(),
            limits(),
            WorkspaceSpec::default(),
            TrustTier::Trusted,
            MeterTarget {
                reserve_id: "reserve:job".into(),
            },
            IdemToken("wf/job".into()),
        )
        .expect("valid template");
        let wire = serde_json::to_value(&template).expect("serialize template");
        assert!(wire.get("run_token").is_none());
        assert!(!wire.to_string().contains("jti"));

        let spec = template.resolve(credential("claim-jti"));
        assert_eq!(spec.run_token.jti, "claim-jti");
        assert_eq!(spec.run_token.expose_bearer(), "test-bearer:claim-jti");
    }

    fn ci_spec() -> JobSpec {
        JobSpec::new(
            JobKind::Ci,
            digest(),
            vec!["cargo".into(), "test".into()],
            vec![EnvVar {
                name: "CI".into(),
                value: "1".into(),
            }],
            vec![SecretRef {
                name: "NPM_TOKEN".into(),
                handle: "broker://job/npm".into(),
            }],
            EgressPolicy::deny_all(),
            limits(),
            WorkspaceSpec::default(),
            TrustTier::Trusted,
            credential("jti-1"),
            MeterTarget {
                reserve_id: "res-1".into(),
            },
            IdemToken("idem-1".into()),
        )
        .unwrap()
    }

    #[test]
    fn digest_pinned_accepts_a_sha_digest_and_rejects_a_bare_tag() {
        let sha256_64 = "a".repeat(64);
        assert!(ImageRef {
            reference: format!("img@sha256:{sha256_64}")
        }
        .digest_pinned());
        assert!(!ImageRef {
            reference: "img:latest".into()
        }
        .digest_pinned());
        assert!(!ImageRef {
            reference: "img".into()
        }
        .digest_pinned());
        assert!(!ImageRef {
            reference: "img@sha256:".into()
        }
        .digest_pinned());
        assert!(!ImageRef {
            reference: format!("img@sha256:{}", "z".repeat(64))
        }
        .digest_pinned());
        assert!(!ImageRef {
            reference: "img@sha256:deadbeef".into()
        }
        .digest_pinned());
        assert!(!ImageRef {
            reference: format!("img@sha256:{}", "a".repeat(65))
        }
        .digest_pinned());
        assert!(!ImageRef {
            reference: format!("img@md5:{}", "a".repeat(64))
        }
        .digest_pinned());
        assert!(ImageRef {
            reference: format!("img@sha512:{}", "b".repeat(128))
        }
        .digest_pinned());
    }

    #[test]
    fn image_ref_pinned_rejects_undigested_fail_closed() {
        let err = ImageRef::pinned("registry/img:latest").unwrap_err();
        assert!(matches!(err, SpecError::UndigestedImage { .. }));
        assert!(ImageRef::pinned(format!("registry/img@sha256:{}", "c".repeat(64))).is_ok());
        assert!(ImageRef::pinned("registry/img@sha256:abc123").is_err());
    }

    #[test]
    fn jobspec_new_rejects_an_undigested_image() {
        let r = JobSpec::new(
            JobKind::Ci,
            ImageRef {
                reference: "img:latest".into(),
            },
            vec![],
            vec![],
            vec![],
            EgressPolicy::deny_all(),
            limits(),
            WorkspaceSpec::default(),
            TrustTier::Trusted,
            credential("j"),
            MeterTarget {
                reserve_id: "r".into(),
            },
            IdemToken("i".into()),
        );
        assert_eq!(
            r.unwrap_err(),
            SpecError::UndigestedImage {
                reference: "img:latest".into()
            }
        );
    }

    #[test]
    fn jobspec_new_rejects_zero_pids_max_and_zero_timeout() {
        let mut l = limits();
        l.pids_max = 0;
        let r = JobSpec::new(
            JobKind::Ci,
            digest(),
            vec![],
            vec![],
            vec![],
            EgressPolicy::deny_all(),
            l,
            WorkspaceSpec::default(),
            TrustTier::Trusted,
            credential("j"),
            MeterTarget {
                reserve_id: "r".into(),
            },
            IdemToken("i".into()),
        );
        assert_eq!(r.unwrap_err(), SpecError::NoPidsMax);

        let mut l = limits();
        l.timeout_secs = 0;
        let r = JobSpec::new(
            JobKind::Ci,
            digest(),
            vec![],
            vec![],
            vec![],
            EgressPolicy::deny_all(),
            l,
            WorkspaceSpec::default(),
            TrustTier::Trusted,
            credential("j"),
            MeterTarget {
                reserve_id: "r".into(),
            },
            IdemToken("i".into()),
        );
        assert_eq!(r.unwrap_err(), SpecError::NoTimeout);
    }

    #[test]
    fn executable_credential_is_bounded_and_debug_redacted() {
        let credential = credential("secret-jti");
        let rendered = format!("{credential:?}");
        assert!(!rendered.contains("test-bearer"));
        assert!(!rendered.contains("secret-jti"));
        assert!(rendered.contains("<redacted>"));
        assert_eq!(credential.ttl_secs(), 300);
        assert_eq!(credential.reference().jti, "secret-jti");
        assert_eq!(
            RunTokenCredential::new("", "jti", 1).unwrap_err(),
            RunTokenCredentialError::EmptyBearer
        );
        assert_eq!(
            RunTokenCredential::new("x".repeat(64 * 1024 + 1), "jti", 1).unwrap_err(),
            RunTokenCredentialError::BearerTooLong
        );
        assert_eq!(
            RunTokenCredential::new("bearer", "", 1).unwrap_err(),
            RunTokenCredentialError::InvalidJti
        );
        assert_eq!(
            RunTokenCredential::new("bearer", "jti", 0).unwrap_err(),
            RunTokenCredentialError::NonPositiveTtl
        );
    }

    #[test]
    fn egress_default_is_deny_all() {
        assert!(EgressPolicy::default().allow.is_empty());
        assert!(EgressPolicy::deny_all().allow.is_empty());
    }

    #[test]
    fn agent_job_builds_a_kind_agent_spec_with_the_same_invariants() {
        let spec = agent_job(
            digest(),
            vec!["python".into(), "-c".into(), "print(1)".into()],
            vec![],
            vec![],
            EgressPolicy::deny_all(),
            limits(),
            TrustTier::UntrustedFork,
            credential("agent-jti"),
            MeterTarget {
                reserve_id: "agent-res".into(),
            },
            IdemToken("agent-idem".into()),
        )
        .unwrap();
        assert_eq!(spec.kind, JobKind::Agent);
        assert_eq!(spec.workspace, WorkspaceSpec::default());
        let bad = agent_job(
            ImageRef {
                reference: "img:latest".into(),
            },
            vec![],
            vec![],
            vec![],
            EgressPolicy::deny_all(),
            limits(),
            TrustTier::UntrustedFork,
            credential("j"),
            MeterTarget {
                reserve_id: "r".into(),
            },
            IdemToken("i".into()),
        );
        assert!(matches!(bad, Err(SpecError::UndigestedImage { .. })));
    }

    struct NoopBackend;
    impl SandboxBackend for NoopBackend {
        type Error = HookError;
        fn launch(
            &self,
            spec: &JobSpec,
            hooks: &RunnerHooks,
        ) -> Result<SandboxLaunch, SandboxLaunchError<Self::Error>> {
            (|| -> Result<SandboxLaunch, HookError> {
                hooks.enforce_isolation_floor(spec)?;
                let res = hooks.reserve(spec)?;
                if let Err(attribute_error) = hooks.attribute(spec) {
                    hooks.release_unused(spec, &res)?;
                    return Err(attribute_error);
                }
                let mut result = SandboxResult::stub_ok(ResourceUsage {
                    cpu_seconds: 1,
                    mem_byte_seconds: 1,
                });
                result.stdout = b"the workload spoke".to_vec();
                hooks.settle_completed(spec, &res, result.usage)?;
                Ok(SandboxLaunch {
                    handle: SandboxHandle {
                        guest_id: "noop-guest".into(),
                    },
                    result,
                    output_complete: true,
                })
            })()
            .map_err(SandboxLaunchError::Failed)
        }
        fn kill(&self, _h: &SandboxHandle) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    fn test_hooks() -> RunnerHooks {
        RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(|spec| {
                Ok(ReserveHandle(format!(
                    "reserved:{}",
                    spec.meter_to.reserve_id
                )))
            }),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        )
    }

    #[test]
    fn derive_checkout_authorization_scope_is_none_for_an_ordinary_compute_workspace() {
        let workspace = WorkspaceSpec {
            repo_ref: None,
            commit: None,
        };
        assert_eq!(
            derive_checkout_authorization_scope(JobKind::Agent, &workspace).unwrap(),
            None
        );
    }

    #[test]
    fn derive_checkout_authorization_scope_derives_the_exact_scope_for_a_valid_checkout_workspace()
    {
        let workspace = WorkspaceSpec {
            repo_ref: Some("myelin://acme/git/repo/widgets".to_string()),
            commit: Some("a".repeat(40)),
        };
        let scope = derive_checkout_authorization_scope(JobKind::Ci, &workspace)
            .unwrap()
            .expect("a full repo_ref + commit pair must derive Some(scope)");
        assert_eq!(scope.tenant().0, "acme");
        assert_eq!(scope.repo_ref().0, "myelin://acme/git/repo/widgets");
        assert_eq!(scope.repo_id(), "widgets");
        assert_eq!(scope.commit_hex(), "a".repeat(40));
        assert_eq!(scope.commit_format(), GitObjectFormat::Sha1);
    }

    #[test]
    fn derive_checkout_authorization_scope_refuses_a_malformed_workspace() {
        let mixed = WorkspaceSpec {
            repo_ref: Some("myelin://acme/git/repo/widgets".to_string()),
            commit: None,
        };
        assert!(derive_checkout_authorization_scope(JobKind::Ci, &mixed).is_err());
    }

    #[test]
    fn sandbox_backend_launch_drives_the_four_guarantee_hooks() {
        let backend = NoopBackend;
        let hooks = test_hooks();
        let launch = backend.launch(&ci_spec(), &hooks).unwrap();
        assert_eq!(launch.handle.guest_id, "noop-guest");
        assert_eq!(launch.result.exit_code, Some(0));
        assert!(!launch.result.timed_out);
        assert!(launch.result.passed());
        backend.kill(&launch.handle).unwrap();
    }

    #[test]
    fn run_cycle_default_wraps_launch_streaming_as_workload_launched() {
        struct NoopSink;
        impl SandboxOutputSink for NoopSink {
            fn emit(&self, _stream: SandboxOutputStream, _frame: &[u8]) -> Result<(), String> {
                Ok(())
            }
        }
        let backend = NoopBackend;
        let outcome = backend
            .run_cycle(
                &ci_spec(),
                &test_hooks(),
                std::sync::Arc::new(NoopSink),
                SandboxCancellation::default(),
            )
            .unwrap();
        match outcome {
            SandboxCycleOutcome::WorkloadLaunched(launch) => {
                assert_eq!(launch.handle.guest_id, "noop-guest");
                backend.kill(&launch.handle).unwrap();
            }
            other => panic!(
                "the default run_cycle must wrap launch_streaming as WorkloadLaunched, got {other:?}"
            ),
        }
    }

    #[test]
    fn default_streaming_marks_output_incomplete_when_the_sink_refuses_a_frame() {
        struct RefusingSink;
        impl SandboxOutputSink for RefusingSink {
            fn emit(&self, _stream: SandboxOutputStream, _frame: &[u8]) -> Result<(), String> {
                Err("durable output is unavailable".into())
            }
        }

        let outcome = NoopBackend
            .run_cycle(
                &ci_spec(),
                &test_hooks(),
                std::sync::Arc::new(RefusingSink),
                SandboxCancellation::default(),
            )
            .unwrap();

        let SandboxCycleOutcome::WorkloadLaunched(launch) = outcome else {
            panic!("ordinary launches remain workload outcomes after an output refusal");
        };
        assert!(!launch.output_complete);
    }

    #[test]
    fn sandbox_cycle_outcome_lifts_every_checkout_continuation_variant() {
        use crate::checkout_orchestration::CheckoutContinuationOutcome as Cco;
        use crate::runner::{
            PreparationPhase, PreparationReportClaim, PreparationTerminalDisposition,
            RetryableAttemptCause,
        };
        let claim = || PreparationReportClaim {
            tenant_id: "t".into(),
            region: "fr-par".into(),
            project_id: "00000000-0000-0000-0000-000000000001".into(),
            wf_run_id: "wf".into(),
            ci_run_id: "ci".into(),
            job_id: "job".into(),
            token_authority_handle: "tah".into(),
            idem_token: "idem".into(),
            lease_owner: "owner".into(),
            lease_epoch: 1,
            claim_nonce: "nonce".into(),
            claim_started_at_epoch_secs: 10,
            claim_expires_at_epoch_secs: 20,
        };
        assert!(matches!(
            SandboxCycleOutcome::from(Cco::WorkloadRetryable {
                cause: RetryableAttemptCause::SandboxInfrastructure,
                usage: ResourceUsage { cpu_seconds: 2, mem_byte_seconds: 3 },
                message: "m".into(),
            }),
            SandboxCycleOutcome::WorkloadRetryable { cause: RetryableAttemptCause::SandboxInfrastructure, usage, .. }
                if usage.cpu_seconds == 2 && usage.mem_byte_seconds == 3
        ));
        assert!(matches!(
            SandboxCycleOutcome::from(Cco::PreparationTerminal {
                claim: claim(),
                disposition: PreparationTerminalDisposition::AttemptsExhausted,
                diagnostic: None,
            }),
            SandboxCycleOutcome::PreparationTerminal {
                disposition: PreparationTerminalDisposition::AttemptsExhausted,
                ..
            }
        ));
        assert!(matches!(
            SandboxCycleOutcome::from(Cco::PreparationRetryable {
                claim: claim(),
                phase: PreparationPhase::CheckoutTransport,
            }),
            SandboxCycleOutcome::PreparationRetryable {
                phase: PreparationPhase::CheckoutTransport,
                ..
            }
        ));
        assert!(matches!(
            SandboxCycleOutcome::from(Cco::ReconciliationRequired {
                phase: PreparationPhase::CheckoutMaterialization,
                teardown_unproven: true,
                usage_unrepresentable: false,
                quarantine_required: true,
            }),
            SandboxCycleOutcome::ReconciliationRequired {
                phase: PreparationPhase::CheckoutMaterialization,
                teardown_unproven: true,
                usage_unrepresentable: false,
                quarantine_required: true,
            }
        ));
    }

    #[test]
    fn final_launch_hook_receives_the_ephemeral_bearer_ttl_and_expected_facts() {
        let backend = NoopBackend;
        let seen = std::sync::Arc::new(std::sync::Mutex::new(None));
        let seen_at_boundary = seen.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(move |spec| {
                let credential = &spec.run_token;
                *seen_at_boundary.lock().unwrap() = Some((
                    credential.expose_bearer().to_owned(),
                    credential.jti.clone(),
                    credential.ttl_secs(),
                    spec.run_token_authorization.clone(),
                ));
                Ok(())
            }),
            Box::new(|_s| Ok(())),
        );

        let mut spec = ci_spec();
        let expected = RunTokenAuthorizationContext::CiJob(CiJobAuthorizationContext {
            tenant_id: "acme".into(),
            region: "eu-west".into(),
            principal_id: "svc:ci".into(),
            project_id: "00000000-0000-0000-0000-000000000001".into(),
            wf_run_id: "11111111-1111-4111-8111-111111111111".into(),
            job_id: "job-1".into(),
            lease_owner: "runner-1".into(),
            lease_epoch: 1,
            claim_nonce: "44444444-4444-4444-8444-444444444444".into(),
            claim_started_at_epoch_secs: 1_785_000_000,
            claim_expires_at_epoch_secs: 1_785_000_030,
            reserve_id: "reserve:job".into(),
            required_capabilities: vec!["job.launch".into()],
            checkout_scope: None,
            credential_binding: None,
        });
        spec.run_token_authorization = Some(expected.clone());
        backend.launch(&spec, &hooks).unwrap();
        assert_eq!(
            *seen.lock().unwrap(),
            Some((
                "test-bearer:jti-1".into(),
                "jti-1".into(),
                300,
                Some(expected)
            ))
        );
    }

    #[test]
    fn the_cost_gate_hook_can_refuse_to_start_on_exhaustion() {
        let backend = NoopBackend;
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(|_spec| Err(HookError("wallet exhausted".into()))),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );
        let r = backend.launch(&ci_spec(), &hooks);
        assert_eq!(
            r.unwrap_err(),
            SandboxLaunchError::Failed(HookError("wallet exhausted".into()))
        );
    }

    #[test]
    fn the_isolation_floor_hook_gates_launch() {
        let backend = NoopBackend;
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Err(HookError("hardening profile not met".into()))),
        );
        let r = backend.launch(&ci_spec(), &hooks);
        assert!(r.is_err());
    }

    struct NoopFleet;
    impl FleetProvider for NoopFleet {
        type Error = HookError;
        fn provision(
            &self,
            _class: RunnerClass,
            n: u32,
            region: Region,
        ) -> Result<Vec<RunnerHost>, Self::Error> {
            Ok((0..n)
                .map(|i| RunnerHost {
                    host_id: format!("host-{i}"),
                    region: region.clone(),
                })
                .collect())
        }
        fn deprovision(&self, _hosts: &[RunnerHost]) -> Result<(), Self::Error> {
            Ok(())
        }
        fn capacity(&self, _region: Region) -> Result<Capacity, Self::Error> {
            Ok(Capacity {
                provisioned: 0,
                available: 0,
            })
        }
    }

    #[test]
    fn fleet_provider_shape_compiles_and_is_region_pinned() {
        let fleet = NoopFleet;
        let region = Region("fr-par".into());
        let hosts = fleet
            .provision(RunnerClass("ci".into()), 2, region.clone())
            .unwrap();
        assert_eq!(hosts.len(), 2);
        assert!(hosts.iter().all(|h| h.region == region));
        fleet.deprovision(&hosts).unwrap();
        assert_eq!(fleet.capacity(region).unwrap().provisioned, 0);
    }
}
