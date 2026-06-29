//! # myelin-ci-sandbox — the runtime-agnostic sandbox seam (CI-P1 → P-129, M2)
//!
//! **Owning architecture doc (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/01-tech-and-data-model.md`
//! §2 ("The job spec — the runtime-agnostic seam (ADR-20 / X-6 / CI-1)") — the `JobSpec` struct,
//! the `SandboxBackend` trait, the `FleetProvider` trait. The four uniform guarantees are pinned
//! by `02-internals-and-algorithms.md` §5.2 and the reconciliation decision `X-6` in
//! `planning/05-refined-shared-systems-architecture/00-reconciliation-decisions.md`.
//! **Contracts:** `contract-index.md` row 8.4 (`ToolHands::exec` = the CI runner's `kind=agent`
//! job on the unified sandbox — the CI runner-seam half), with the consumed hooks 11.7
//! (reserve/settle), 4.7 (`mint_run_token`), 11.3 (KMS).
//!
//! ## What this crate IS (the one struct, two kinds seam — shapes only)
//! `JobSpec` is the **one struct, two kinds** seam (ADR-20 / X-6 / CI-1). `SandboxBackend` hides
//! Firecracker vs gVisor vs self-hosted; `FleetProvider` hides the EU provider. These trait
//! shapes are what the whole CI subsystem is built on. The central equivalence (X-6):
//! **`ToolHands::exec(Command)` (contract 8.4) IS `launch(JobSpec{ kind: Agent, .. })`** — the
//! same runner, the same hardening, the same drill, inheriting the **four uniform guarantees**
//! (cost gate, per-run-token attribution, HITL withhold, isolation floor + drill). The
//! [`agent_job`] constructor wires that equivalence so the agent fabric (AG-P8 → P-226)
//! dispatches a `kind=agent` job onto THIS exact runner; no bypass exists (the `no-host-exec`
//! lint, P-S10/P-017, runs live over this crate's `src` tree and admits it because there is no
//! host-execution path — all execution goes through [`SandboxBackend::launch`]).
//!
//! ## What this crate is NOT (named floors)
//! - **No backend implementation.** [`SandboxBackend`] / [`FleetProvider`] are trait SHAPES only.
//!   The **Firecracker default backend + the backend-independent mandatory hardening profile +
//!   the hardened-boot self-test land in CI-P2 (→ P-237)**. The **gVisor second backend** (the
//!   named-second, same trait, its own drill) is **CI-P28**. The **fleet impl** is **CI-P14**.
//! - **No escape drill here.** The ZERO-escapes real-kernel GATE (AG-D4 / CI-T1) is **CI-P5
//!   (→ P-239)**; the hardened-boot self-test is CI-P2. The [`RunnerHooks::isolation_floor`] hook
//!   is the seam that drill drives.
//! - **No live wallet / token / KMS.** The four-guarantee hooks ([`RunnerHooks`]) are typed SEAM
//!   functions wired here; the reserve/settle wallet body is Commercial (C-1) consumed via
//!   Storage 11.7, `mint_run_token` is Identity 4.7, KMS is Storage 11.3 — all consumed, not
//!   implemented, at P-129.
//!
//! ## DB-free / VM-free by default
//! The seam (this module) is a pure value/trait crate — `cargo build --workspace` and the default
//! `cargo test` stay DB-free AND VM-free. The real backends (CI-P2 → P-237) add no DB; their unit
//! tests inject a fake VMM/runtime so they never boot a guest. The REAL microVM boot lives ONLY in
//! the `integration`-feature self-test (`tests/hardened_boot_selftest.rs`), gated to SKIP gracefully
//! when `/dev/kvm` or `firecracker` is absent — so CI without KVM still passes.
//!
//! ## The CI-P2 backends + the named floor
//! - [`firecracker`] — the **Firecracker default** [`SandboxBackend`] (microVM = KVM + minimal VMM):
//!   the production default for untrusted code (arch 02 §5.1).
//! - [`gvisor`] — the **gVisor (`runsc`) named-second** [`SandboxBackend`] behind the SAME trait. The
//!   CI-P28 "gVisor second backend" floor is satisfied **early** here (the host has `runsc`; the
//!   handoff inverts the original deferral) so the AG-D4 escape drill (CI-P5 → P-239) can parametrize
//!   per available backend. gVisor uses the OCI/`runsc` path; Firecracker uses the microVM path. The
//!   drill governs which is the production default (microVM, §5.1).
//! - [`hardening`] — the **backend-independent mandatory hardening profile** (arch 02 §5.3) applied
//!   identically regardless of backend or kind, incl. the unit-tested egress-allowlist evaluator
//!   (metadata / control-plane / cross-tenant always denied).
//!
//! **FLOOR named (CI-P2):** ONE backend (Firecracker) goes through the escape drill first; gVisor is
//! the named second backend behind the same trait (its own drill). The ZERO-escapes real-kernel GATE
//! (AG-D4 / CI-T1) is CI-P5 (→ P-239); this prompt ships only the **hardened-boot self-test** (the
//! floor under that drill — proves the runner BOOTS hardened, not that it survives the corpus).

pub mod escape_corpus;
pub mod events;
pub mod firecracker;
pub mod gvisor;
pub mod hardening;
pub mod notif_rules;
pub mod replay;
pub mod runner;
pub mod self_hosted;
pub mod snapshot_pool;

pub use events::{
    ci_event_tokens, is_durable, register_ci_tokens, CI_DURABLE_TOKENS, CI_FIREHOSE_TOKENS,
};
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

// CI-P28 (P-423): the gVisor escape-drill bundle builders (the corpus RE-RUNS on the gVisor backend
// — the permanent gate, contract 8.4). Exercised by tests/escape_drill_gvisor_test.rs against a real
// `runsc` sandbox; the host-side parser + attestation format are SHARED with the Firecracker drill.
pub use gvisor::{
    build_gvisor_corpus_script, gvisor_drill_config_json, resolved_gvisor_rootfs,
    GVISOR_CORPUS_SCRIPT,
};

pub use runner::{
    CountingFirehose, EngineTerminalReporter, FirehoseSink, JobLeaseStore, QueuedJob, RunOutcome,
    RunnerAgent, RunnerError, TerminalReport, TerminalReporter,
};

use serde::{Deserialize, Serialize};

// `Region` is the frozen (tenant, region) partition type (contract 12.1 / P-CP-01); the
// `FleetProvider` trait names it in provision/capacity (arch 01 §2).
pub use myelin_tenancy::Region;

// ---------------------------------------------------------------------------------------------
// §2 — The job spec (byte-authoritative to arch 01 §2)
// ---------------------------------------------------------------------------------------------

/// `JobSpec` — the **one struct, two kinds** seam (arch 01 §2; ADR-20 / X-6 / CI-1).
///
/// One value describes every execution on the unified runner — a CI run (`kind: Ci`) or an agent
/// `ToolHands::exec` (`kind: Agent`). The frozen field order/shape is arch 01 §2; a needed shape
/// change is a whole-workspace contract PR, escalated and written down (code-wins-over-docs).
///
/// **Construction is fail-closed.** Use [`JobSpec::new`] (validates the digest-pin-or-reject rule
/// — contract CI-1 / arch 01 §2 / arch 02 §5.3) rather than building the struct literally, so an
/// un-digested image can never reach the runner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobSpec {
    /// `Ci | Agent` — the UNIFY point (TE-31 = UNIFY; X-6). The ONLY thing that differs between a
    /// CI run and an agent `exec` on this runner.
    pub kind: JobKind,
    /// The image to run — **MUST be digest-pinned**; an un-digested tag is rejected fail-closed
    /// (CI-1; arch 02 §5.3). Enforced by [`JobSpec::new`] / [`ImageRef::digest_pinned`].
    pub image: ImageRef,
    /// The command line executed inside the guest.
    pub command: Vec<String>,
    /// Environment variables. **Secrets are NAMES here**, resolved inside the boundary (CI-1);
    /// see [`secret_refs`](JobSpec::secret_refs).
    pub env: Vec<EnvVar>,
    /// Secret references resolved by the **in-boundary broker**, scoped to THIS job only (CI-1;
    /// arch 02 §7.3) — never baked into images, never forwarded via the runtime.
    pub secret_refs: Vec<SecretRef>,
    /// Egress policy — **default-deny**, allowlist opt-in; the cloud-metadata endpoint, the
    /// control-plane/internal RPC, and any cross-tenant network are ALWAYS blocked (arch 02 §5.3).
    pub egress: EgressPolicy,
    /// Resource limits — cpu, mem, disk, `pids_max` (fork-bomb ceiling), `timeout`, **zero-swap**
    /// (arch 02 §5.3).
    pub limits: ResourceLimits,
    /// Workspace — checkout via the scoped job-token git wire; **read-only root + tmpfs scratch**.
    pub workspace: WorkspaceSpec,
    /// `Trusted | UntrustedFork | SelfHosted` — gates secrets/cache-scope/egress; the **SAME**
    /// value CI stamps onto `CheckStatus.trust_tier` (X-1). One value, stamped once (arch 01 §2).
    pub trust_tier: TrustTier,
    /// The per-job attenuated token (`Id::mint_run_token`, contract 4.7) — guarantee #2,
    /// attribution; life == run life, auto-revoked on teardown.
    pub run_token: RunTokenRef,
    /// The reserve this job settles against (run-level / agent-run-level) — guarantee #1, the cost
    /// gate (contract 11.7).
    pub meter_to: MeterTarget,
    /// Minted by the workflow at `SCHEDULE_AND_RUN_JOB` dispatch (OQ-F); the runner stamps it on
    /// the `job.done` signal — producer/consumer agree, no round-trip.
    pub idem_token: IdemToken,
}

/// `Ci | Agent` — the UNIFY point (arch 01 §2; X-6). The agent fabric's `ToolHands::exec` IS a
/// `JobSpec{ kind: Agent, .. }` launched on this runner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobKind {
    /// A CI run (a pipeline job).
    Ci,
    /// An agent `ToolHands::exec` (compute/external untrusted code; never privileged mutation —
    /// mutation goes through `EffectApi`, guarantee #3).
    Agent,
}

/// `Trusted | UntrustedFork | SelfHosted` (arch 01 §2). Evaluated once from run provenance
/// (member push vs fork PR vs self-hosted target) + the ReBAC ABAC edge `read & !is_untrusted_fork`
/// (contract 4.9); the SAME value gates the `JobSpec` AND is stamped onto `CheckStatus.trust_tier`
/// (X-1). Git never recomputes trust; it reads the fact.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrustTier {
    /// A trusted run (member push) — full secrets/cache/egress per policy.
    Trusted,
    /// An untrusted fork PR — restricted secrets, restricted cache scope, restricted egress.
    UntrustedFork,
    /// A self-hosted-runner job — the per-run token is scoped to ONE tenant's `SelfHosted` jobs
    /// (contract 4.7 sharpened; recon §1).
    SelfHosted,
}

/// A container/VM image reference. **MUST be digest-pinned** — an un-digested tag is rejected
/// fail-closed (CI-1; arch 01 §2 / arch 02 §5.3). Construct via [`ImageRef::pinned`] (which
/// enforces the rule) or check an existing ref with [`ImageRef::digest_pinned`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageRef {
    /// The full reference, e.g. `registry.example/foo@sha256:<64-hex>`.
    pub reference: String,
}

impl ImageRef {
    /// Construct a digest-pinned [`ImageRef`], rejecting an un-digested tag fail-closed (CI-1).
    /// Returns [`SpecError::UndigestedImage`] for a reference that is not pinned by digest.
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

    /// True iff the reference is pinned by a content digest (`...@<algo>:<hexdigest>`), the only
    /// admitted form. A bare `:tag` (or no tag) is NOT pinned. The check is deliberately strict:
    /// a digest is `@<algo>:<hex>` where the hex part is non-empty and all-hex (so `@sha256:`
    /// with an empty digest, or a non-hex digest, is rejected).
    pub fn digest_pinned(&self) -> bool {
        let Some((_, after_at)) = self.reference.rsplit_once('@') else {
            return false;
        };
        let Some((algo, digest)) = after_at.split_once(':') else {
            return false;
        };
        !algo.is_empty() && !digest.is_empty() && digest.chars().all(|c| c.is_ascii_hexdigit())
    }
}

/// An environment variable. Secrets are NAMES here (CI-1); the value of a secret env is resolved
/// in-boundary from a [`SecretRef`], never carried in clear in the spec.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvVar {
    /// The variable name.
    pub name: String,
    /// The literal (non-secret) value. Secret values are NOT carried here — they are resolved
    /// in-boundary from a [`SecretRef`] of the same `name`.
    pub value: String,
}

/// A reference to a secret resolved by the in-boundary broker, scoped to THIS job only (CI-1;
/// arch 02 §7.3). The clear value never appears in the `JobSpec`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretRef {
    /// The env var name the resolved secret is bound to inside the boundary.
    pub name: String,
    /// The opaque broker handle (a name/path, never the secret material).
    pub handle: String,
}

/// The egress policy — **default-deny**, allowlist opt-in (arch 02 §5.3). The cloud-metadata
/// endpoint, the control-plane/internal RPC, and any cross-tenant network are ALWAYS blocked,
/// regardless of allowlist (enforced by the backend's hardening profile, CI-P2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressPolicy {
    /// The host/CIDR allowlist. Empty == fully default-deny.
    pub allow: Vec<String>,
}

impl Default for EgressPolicy {
    /// Default-deny: no allowlist entries (arch 02 §5.3).
    fn default() -> Self {
        EgressPolicy { allow: Vec::new() }
    }
}

impl EgressPolicy {
    /// The default-deny policy (no allowlist). The safe default for every job (arch 02 §5.3).
    pub fn deny_all() -> EgressPolicy {
        EgressPolicy::default()
    }
}

/// Resource limits — cpu, mem, disk, `pids_max` (fork-bomb ceiling), `timeout`, **zero-swap**
/// (arch 01 §2; arch 02 §5.3). `swap` is structurally absent (there is no swap field): swap is
/// always zero by construction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// Millicpu ceiling (1000 == one core).
    pub cpu_millis: u32,
    /// Memory ceiling, bytes.
    pub mem_bytes: u64,
    /// Scratch-disk quota, bytes.
    pub disk_bytes: u64,
    /// The `pids.max` fork-bomb ceiling (arch 02 §5.3). MUST be > 0.
    pub pids_max: u32,
    /// The wall-clock timeout, seconds. MUST be > 0.
    pub timeout_secs: u32,
}

/// The workspace spec — checkout via the scoped job-token git wire; **read-only root + tmpfs
/// scratch** (arch 01 §2).
/// The empty default (`repo_ref: None, commit: None`) is the agent-`compute` case: no checkout.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSpec {
    /// The `ArtifactRef` of the repo to check out (a reference, not data), or `None` for an
    /// agent `compute` job that needs no checkout.
    pub repo_ref: Option<String>,
    /// The commit/ref to check out (content-addressed).
    pub commit: Option<String>,
}

/// The per-job attenuated run token reference (`mint_run_token`, contract 4.7) — guarantee #2.
/// Carries the token's `jti`/handle, not the token material.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunTokenRef {
    /// The token id (`jti`) of the per-run attenuated token. Life == run life; auto-revoked on
    /// teardown; re-mintable mid-workflow on resume (S-11).
    pub jti: String,
}

/// The reserve this job settles against (contract 11.7) — guarantee #1, the cost gate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeterTarget {
    /// The reserve id the dispatch reserved against and the `job.done` settles (run-level /
    /// agent-run-level).
    pub reserve_id: String,
}

/// The dispatch idempotency token (minted by the workflow at `SCHEDULE_AND_RUN_JOB`, OQ-F). The
/// runner stamps it on the `job.done` signal so producer/consumer agree with no round-trip.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdemToken(pub String);

/// A construction error — fail-closed (arch 01 §2 / CI-1). The only way to build a `JobSpec` that
/// would have violated a non-negotiable invariant is to get an `Err` instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpecError {
    /// The image is not pinned by digest — rejected fail-closed (CI-1; arch 02 §5.3).
    UndigestedImage {
        /// The offending reference (for a self-describing error).
        reference: String,
    },
    /// `pids_max` is zero — a fork-bomb ceiling MUST be set (arch 02 §5.3).
    NoPidsMax,
    /// `timeout_secs` is zero — every job MUST have a wall-clock timeout (arch 02 §5.3).
    NoTimeout,
}

impl std::fmt::Display for SpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpecError::UndigestedImage { reference } => write!(
                f,
                "image `{reference}` is not digest-pinned — an un-digested tag is rejected \
                 fail-closed (CI-1; arch 02 §5.3). Pin by `@<algo>:<hexdigest>`."
            ),
            SpecError::NoPidsMax => write!(
                f,
                "ResourceLimits.pids_max is 0 — the fork-bomb ceiling MUST be set (arch 02 §5.3)."
            ),
            SpecError::NoTimeout => write!(
                f,
                "ResourceLimits.timeout_secs is 0 — every job MUST have a wall-clock timeout \
                 (arch 02 §5.3)."
            ),
        }
    }
}

impl std::error::Error for SpecError {}

impl JobSpec {
    /// Construct a `JobSpec`, enforcing the fail-closed non-negotiables (arch 01 §2; CI-1; arch
    /// 02 §5.3): the image MUST be digest-pinned, `pids_max` MUST be set, and `timeout_secs` MUST
    /// be set. Returns the first violated invariant as a [`SpecError`] — the un-digested-tag
    /// rejection is the headline (CI-1).
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
        run_token: RunTokenRef,
        meter_to: MeterTarget,
        idem_token: IdemToken,
    ) -> Result<JobSpec, SpecError> {
        if !image.digest_pinned() {
            return Err(SpecError::UndigestedImage {
                reference: image.reference,
            });
        }
        if limits.pids_max == 0 {
            return Err(SpecError::NoPidsMax);
        }
        if limits.timeout_secs == 0 {
            return Err(SpecError::NoTimeout);
        }
        Ok(JobSpec {
            kind,
            image,
            command,
            env,
            secret_refs,
            egress,
            limits,
            workspace,
            trust_tier,
            run_token,
            meter_to,
            idem_token,
        })
    }
}

// ---------------------------------------------------------------------------------------------
// §2 — The backend + fleet trait shapes (arch 01 §2). Shapes ONLY — no impl at P-129.
// ---------------------------------------------------------------------------------------------

/// A handle to a launched sandbox — opaque to callers; the backend's own identifier for the guest
/// it must be able to whole-guest-kill on teardown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxHandle {
    /// The backend's guest id (microVM id / runsc container id / self-hosted lease id).
    pub guest_id: String,
}

/// The **per-stream capture bound** for [`SandboxResult`] (RESHAPE-001 / CT-001). Captured guest
/// `stdout`/`stderr` are bounded to this many bytes EACH at the seam — a HEAD capture, so a runaway
/// guest cannot OOM the runner. The *full* stream rides the firehose → the `ci.log.available`
/// pointer (CI-P20), never the engine signal payload (references-not-payloads at the signal
/// boundary). The bound is enforced when CT-002 wires real capture; the SHAPE carries it now.
pub const SANDBOX_CAPTURE_BOUND: usize = 256 * 1024;

/// Drain a child stream (a guest pipe / serial console) into an owned buffer that is HARD-bounded to
/// `limit` bytes of **HEAD** capture (the first `limit` bytes — matching the existing head-bound
/// semantics), then KEEP READING the remainder into a small fixed throwaway buffer and DISCARD it to
/// EOF.
///
/// This is the CT-002c host-side memory-DoS fix shared by BOTH production-exec backends
/// (Firecracker serial console + gVisor `runsc` stdout/stderr). The previous code did
/// `read_to_end`-then-`truncate`, so an untrusted workload emitting at high rate until `timeout_secs`
/// could force this HOST drain thread to buffer multi-GB BEFORE the bound was applied (the guest's
/// cgroup mem limit does not bound a HOST allocation). Here host memory is bounded to
/// `limit` + the 64 KiB throwaway chunk REGARDLESS of how much the workload emits.
///
/// Crucially it keeps READING (and discarding) past the bound rather than stopping: if we stopped
/// reading, a chatty guest would fill the OS pipe buffer and BLOCK on write, hanging until the
/// timeout-kill — defeating prompt termination and risking a cross-stream deadlock. Draining-and-
/// discarding applies no backpressure, so the guest runs to its real exit / the timeout fires cleanly.
///
/// Returns the captured head bytes and whether any bytes beyond the bound were seen (`truncated`).
pub(crate) fn drain_capped<R: std::io::Read>(mut r: R, limit: usize) -> (Vec<u8>, bool) {
    let mut head = Vec::new();
    let mut truncated = false;
    let mut chunk = [0u8; 64 * 1024];
    loop {
        match r.read(&mut chunk) {
            Ok(0) => break, // EOF: the guest pipe closed (child exited or was whole-guest-killed).
            Ok(n) => {
                if head.len() < limit {
                    let take = (limit - head.len()).min(n);
                    head.extend_from_slice(&chunk[..take]);
                    if take < n {
                        truncated = true; // overflowed the head bound this read
                    }
                } else {
                    // Already at the bound: read into `chunk` and DISCARD — bounded host memory, but
                    // the pipe keeps draining so the guest never blocks on a full pipe.
                    truncated = true;
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break, // pipe error ⇒ treat as end-of-stream; the wait/kill loop owns lifecycle.
        }
    }
    (head, truncated)
}

/// **The result of running a job's command inside the sandbox (RESHAPE-001 / CT-001).** This is the
/// in-line compute outcome the runner DERIVES its [`TerminalReport`](crate::runner::TerminalReport)
/// from — `passed = exit_code == Some(0) && !timed_out`. Before this reshape the seam could carry
/// NOTHING about the command's outcome (the handle was `{ guest_id }` only and the runner took the
/// terminal report as an INPUT parameter); now the outcome flows back through the seam.
///
/// **References-not-payloads at the SIGNAL boundary:** `stdout`/`stderr` are BOUNDED capture (see
/// [`SANDBOX_CAPTURE_BOUND`]) for the runner to ship through the firehose as redacted frames; they
/// are NEVER placed in the `job.done` engine signal payload (that stays `ArtifactRef`s only — the
/// `ci.log.available` pointer the firehose pipeline publishes).
///
/// At CT-001 a backend returns a STUB value (exit 0, empty streams, stub usage) — the FIELD shape
/// must exist and FLOW. CT-002 fills in the real Firecracker boot + gVisor `runsc run` that runs
/// `spec.command`, captures the streams, enforces `spec.limits.timeout_secs` (setting `timed_out`),
/// and reports the measured [`ResourceUsage`] (it reuses the inner backend `wait` already present).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxResult {
    /// The guest command's exit code — `None` if the guest was killed by signal/timeout (no code).
    pub exit_code: Option<i32>,
    /// Whether the wall-clock `timeout_secs` ceiling fired (the whole guest was killed). A timed-out
    /// job is NOT a pass regardless of any partial exit code.
    pub timed_out: bool,
    /// The measured resource usage (the resource-seconds metering unit, arch §8) — handed to the
    /// guarantee-#1 settle hook so metering settles against what the job actually consumed. Stub at
    /// CT-001; measured at CT-002.
    pub usage: ResourceUsage,
    /// Bounded ([`SANDBOX_CAPTURE_BOUND`]) captured guest stdout — for the firehose/log pipeline, NOT
    /// the signal payload.
    pub stdout: Vec<u8>,
    /// Bounded ([`SANDBOX_CAPTURE_BOUND`]) captured guest stderr — for the firehose/log pipeline, NOT
    /// the signal payload.
    pub stderr: Vec<u8>,
}

impl SandboxResult {
    /// Whether the job PASSED — `exit_code == Some(0)` AND it did not time out. This is the single
    /// derivation point the runner uses to build `TerminalReport.passed` (no longer a parameter).
    pub fn passed(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out
    }

    /// A CT-001 STUB clean result (exit 0, no timeout, empty streams, the given `usage`). CT-002
    /// replaces this with the real captured outcome. Centralised so backends do not hand-roll the
    /// stub shape (anti-duplication).
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

/// **What [`SandboxBackend::launch`] returns (RESHAPE-001 / CT-001, shape A).** The teardown
/// [`SandboxHandle`] PLUS the command [`SandboxResult`]. `launch` BLOCKS for the in-line compute job
/// (matching the trait's documented semantics: it returns when the guest is up and, for an in-line
/// job, has run) and returns both — `kill(handle)` still tears the guest down (idempotent if the
/// guest already exited). This is preferred over a separate `wait()` method: the in-line compute
/// path is a single blocking call, so one return value (handle + result) keeps every call site's
/// control flow linear; the long-park path keeps using [`SandboxBackend::accept_async`] +
/// `job.done`, which is a different lifecycle entirely.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxLaunch {
    /// The teardown handle the caller MUST eventually [`kill`](SandboxBackend::kill).
    pub handle: SandboxHandle,
    /// The command's result — the runner DERIVES the terminal report from it.
    pub result: SandboxResult,
}

/// The sandbox backend (arch 01 §2): Firecracker (default) | Gvisor (named 2nd) | SelfHosted
/// (delegated). The **trait SHAPE only** at P-129 — the Firecracker impl is CI-P2 (→ P-237), the
/// gVisor impl is CI-P28, the self-hosted impl is CI-P4. `launch` carries the [`RunnerHooks`]
/// through which the four uniform guarantees are wired (X-6; arch 02 §5.2).
///
/// **`ToolHands::exec(Command)` (contract 8.4) IS `launch(JobSpec{ kind: Agent, .. }, hooks)`** —
/// the same runner, the same hardening, the same drill. The `no-host-exec` lint (P-S10/P-017)
/// forbids any platform host-exec bypass; ALL execution goes through this seam.
pub trait SandboxBackend {
    /// The backend's error type.
    type Error: std::error::Error;

    /// Launch a job in a fresh, ephemeral, one-job-per-sandbox guest, applying the mandatory
    /// hardening profile (arch 02 §5.3) and wiring the four-guarantee `hooks` (arch 02 §5.2). For an
    /// in-line compute job `launch` BLOCKS for the duration and returns a [`SandboxLaunch`] carrying
    /// BOTH the teardown [`SandboxHandle`] (the caller MUST eventually [`kill`](SandboxBackend::kill)
    /// it — idempotent if the guest already exited) AND the command's [`SandboxResult`]
    /// (exit/timeout/usage/captured-streams). The runner DERIVES its terminal report from the
    /// result; it is no longer supplied as an input (RESHAPE-001 / CT-001).
    fn launch(&self, spec: &JobSpec, hooks: &RunnerHooks) -> Result<SandboxLaunch, Self::Error>;

    /// Whole-guest kill on teardown (arch 01 §2): the guest is destroyed, never reused across
    /// tenants/jobs. Idempotent (killing an already-dead guest is a no-op success).
    fn kill(&self, h: &SandboxHandle) -> Result<(), Self::Error>;

    /// **The ASYNC-DISPATCH seam for the `SCHEDULE_AND_RUN_JOB` long-park idiom (arch agent-fabric
    /// §5.6; AG-P16 → P-228).** Where [`launch`](SandboxBackend::launch) BLOCKS for the duration of an
    /// in-line `compute` job (it returns the [`SandboxHandle`] only when the guest is up and the four
    /// guarantees have fired), `accept_async` DISPATCHES a long job (one that takes minutes-to-hours)
    /// and RETURNS immediately — the caller (the durable workflow) then PARKS holding no runtime and is
    /// resumed HOURS later by the durable `job.done` signal the runner delivers when the job finishes.
    ///
    /// It returns `Ok(())` if the runner ACCEPTED the job for asynchronous execution (the dispatch
    /// succeeded, NOT the job — completion arrives later as `job.done`), or `Err(..)` if the dispatch
    /// itself failed (the runner is unreachable / rejected the spec) — a dispatch failure surfaces LOUD
    /// so the workflow's dispatch activity RETRIES it on the SAME `idem_token` (the runner dedups a
    /// re-dispatched job on it). The `spec.idem_token` is the dispatch dedup key the runner echoes on
    /// the `job.done` signal (the no-coordination agreement).
    ///
    /// **Default:** accept the spec (return `Ok(())`). A backend that genuinely supports asynchronous
    /// dispatch (the real microVM fleet, CI-P2/CI-P14) overrides this to enqueue the guest and arrange
    /// the eventual `job.done` delivery; the default makes every backend usable on the long-park path
    /// without a second hardening profile (the long-park job is the SAME hardened `JobSpec`).
    fn accept_async(&self, spec: &JobSpec) -> Result<(), Self::Error> {
        let _ = spec;
        Ok(())
    }
}

/// A runner class (the label-class the scheduler/fleet sizes a warm buffer per — arch 02 §5.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerClass(pub String);

/// A provisioned runner host returned by a [`FleetProvider`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerHost {
    /// The provider's host id.
    pub host_id: String,
    /// The region the host is pinned to (residency — no cross-region runner placement).
    pub region: Region,
}

/// Capacity report for a region (arch 01 §2; the autoscaler input, CI-P10/CI-P14).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capacity {
    /// Currently provisioned runner slots in the region.
    pub provisioned: u32,
    /// Currently idle (available) runner slots in the region.
    pub available: u32,
}

/// The EU fleet provider (arch 01 §2): Hetzner | OVH | Scaleway | BareMetal-PXE | K8s(customer) |
/// SelfHosted. The **trait SHAPE only** at P-129 — the fleet impl + autoscaler is CI-P14/CI-P10.
/// Every adapter is EU-runnable; nothing requires a hyperscaler-proprietary primitive (arch 01
/// §1.1).
pub trait FleetProvider {
    /// The provider's error type.
    type Error: std::error::Error;

    /// Provision `n` runner hosts of `class` in `region` (region-pinned — residency).
    fn provision(
        &self,
        class: RunnerClass,
        n: u32,
        region: Region,
    ) -> Result<Vec<RunnerHost>, Self::Error>;

    /// Deprovision the given hosts.
    fn deprovision(&self, hosts: &[RunnerHost]) -> Result<(), Self::Error>;

    /// Report capacity for a region.
    fn capacity(&self, region: Region) -> Result<Capacity, Self::Error>;
}

// ---------------------------------------------------------------------------------------------
// §5.2 — The four uniform guarantees wiring SEAM (X-6).
// ---------------------------------------------------------------------------------------------

/// The four-uniform-guarantee wiring seam (arch 02 §5.2; X-6) — passed to
/// [`SandboxBackend::launch`] so EVERY execution (CI run OR agent `exec`) inherits the guarantees
/// **by construction**, no subsystem re-implementing them (contract 8.4). The hooks are typed
/// SEAM functions wired here at P-129; their bodies are consumed contracts:
///
/// 1. **Universal cost gate** ([`reserve`](RunnerHooks::reserve) / [`settle`](RunnerHooks::settle))
///    — reserve at dispatch, refuse-on-exhaustion, settle on completion, never interrupt
///    in-flight; CI runs and agent runs meter into the SAME wallet (contract 11.7).
/// 2. **Attribution** ([`attribute`](RunnerHooks::attribute)) — the job runs under a per-run
///    attenuated token (`mint_run_token`, contract 4.7); life == run life, auto-revoked on
///    teardown, re-mintable on resume (S-11).
/// 3. **HITL withhold (plan-then-apply)** — encoded structurally, not as a hook: side-effecting
///    mutation NEVER goes through this runner; it goes through `EffectApi::apply` (contract 8.2).
///    `exec` carries ONLY `compute`/`external` untrusted code. See [`hitl_withhold_note`].
/// 4. **Isolation floor + drill** ([`isolation_floor`](RunnerHooks::isolation_floor)) — the
///    hardening-profile + real-kernel escape-drill hook CI-P5's escape drill drives (the
///    ZERO-escapes GATE is CI-P5 → P-239; the hardened-boot self-test is CI-P2 → P-237).
///
/// At P-129 a `RunnerHooks` value is a bundle of boxed closures; a backend (CI-P2) calls them at
/// the right lifecycle points. This keeps the four guarantees a single, typed, testable seam.
pub struct RunnerHooks {
    /// Guarantee #1a: reserve budget at dispatch; `Err` == exhausted → refuse-to-start (contract
    /// 11.7, `reserve_budget`). Returns the reserve handle on success.
    pub reserve: ReserveHook,
    /// Guarantee #1b: settle the reserve on completion (contract 11.7, `settle_budget`); the
    /// unused reserve is released, never interrupting in-flight.
    pub settle: SettleHook,
    /// Guarantee #2: confirm/attach the per-run attenuated token (contract 4.7); the job runs
    /// under it, life == run life.
    pub attribute: AttributeHook,
    /// Guarantee #4: the isolation-floor hook the escape drill (CI-P5) drives — apply + verify the
    /// mandatory hardening profile (arch 02 §5.3) before any untrusted code runs.
    pub isolation_floor: IsolationFloorHook,
}

/// Guarantee #1a hook type (contract 11.7 reserve_budget): reserve at dispatch, `Err` == exhausted.
pub type ReserveHook = Box<dyn Fn(&MeterTarget) -> Result<ReserveHandle, HookError> + Send + Sync>;
/// Guarantee #1b hook type (contract 11.7 settle_budget): settle on completion.
pub type SettleHook =
    Box<dyn Fn(&ReserveHandle, ResourceUsage) -> Result<(), HookError> + Send + Sync>;
/// Guarantee #2 hook type (contract 4.7 mint_run_token): per-run attenuated-token attribution.
pub type AttributeHook = Box<dyn Fn(&RunTokenRef) -> Result<(), HookError> + Send + Sync>;
/// Guarantee #4 hook type (arch 02 §5.3): apply + verify the mandatory hardening profile.
pub type IsolationFloorHook = Box<dyn Fn(&JobSpec) -> Result<(), HookError> + Send + Sync>;

impl std::fmt::Debug for RunnerHooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunnerHooks")
            .field("reserve", &"<fn>")
            .field("settle", &"<fn>")
            .field("attribute", &"<fn>")
            .field("isolation_floor", &"<fn>")
            .finish()
    }
}

/// Guarantee #3, written once as the routing-split invariant (arch 02 §5.2 #3; X-6 #3; AG-8):
/// **side-effecting mutation never goes through this runner.** A `JobSpec` (`kind: Ci` OR
/// `kind: Agent`) carries ONLY `compute`/`external` untrusted code; privileged mutation goes
/// through `EffectApi::apply` (contract 8.2), which enforces schema → capability → delegation →
/// tenant → budget → **HITL gate** → apply-via-public-endpoint → meter. A gated tool whose name
/// is not in the approved set is **withheld** there (returns `Denied`, does not mutate). The
/// routing split IS the safety boundary — encoded structurally (there is no mutation API on the
/// sandbox seam), so this is a const note, not a runtime hook.
pub const fn hitl_withhold_note() -> &'static str {
    "side-effecting mutation never goes through the sandbox runner; it goes through \
     EffectApi::apply (contract 8.2) — the routing split is the safety boundary (X-6 #3 / AG-8)"
}

/// An opaque reserve handle from guarantee #1's reserve hook (contract 11.7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReserveHandle(pub String);

/// Resource usage reported at settle time (the metering unit is resource-seconds, arch §8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceUsage {
    /// CPU-seconds consumed.
    pub cpu_seconds: u64,
    /// Memory-byte-seconds consumed.
    pub mem_byte_seconds: u64,
}

/// A four-guarantee hook failure (cost-exhausted, token-rejected, isolation-floor-not-met, …).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookError(pub String);

impl std::fmt::Display for HookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "runner hook failed: {}", self.0)
    }
}

impl std::error::Error for HookError {}

// ---------------------------------------------------------------------------------------------
// The X-6 equivalence: ToolHands::exec IS launch(JobSpec{kind:Agent}).
// ---------------------------------------------------------------------------------------------

/// Build the `kind=agent` `JobSpec` the agent fabric's `ToolHands::exec` (contract 8.4) dispatches
/// onto THIS runner (X-6). This is the wired equivalence: `ToolHands::exec(Command)` IS
/// `launch(JobSpec{ kind: Agent, .. }, hooks)` — the same runner, the same hardening, the same
/// drill, inheriting the four uniform guarantees by construction. An agent `compute`/`external`
/// job carries no repo checkout by default and default-deny egress; the caller (AG-P8 → P-226)
/// supplies the per-run token, meter target, idem token, image, and limits.
///
/// Fail-closed: the same [`JobSpec::new`] invariants apply (digest-pin, pids_max, timeout), so an
/// agent job can no more bypass the non-negotiables than a CI job can.
#[allow(clippy::too_many_arguments)]
pub fn agent_job(
    image: ImageRef,
    command: Vec<String>,
    env: Vec<EnvVar>,
    secret_refs: Vec<SecretRef>,
    egress: EgressPolicy,
    limits: ResourceLimits,
    trust_tier: TrustTier,
    run_token: RunTokenRef,
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
mod tests {
    use super::*;

    fn digest() -> ImageRef {
        ImageRef::pinned("registry.example/img@sha256:abc123def4567890").unwrap()
    }

    fn limits() -> ResourceLimits {
        ResourceLimits {
            cpu_millis: 1000,
            mem_bytes: 512 << 20,
            disk_bytes: 1 << 30,
            pids_max: 256,
            timeout_secs: 600,
        }
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
            RunTokenRef {
                jti: "jti-1".into(),
            },
            MeterTarget {
                reserve_id: "res-1".into(),
            },
            IdemToken("idem-1".into()),
        )
        .unwrap()
    }

    // --- The digest-pin-or-reject rule (CI-1; the headline non-negotiable) ---

    #[test]
    fn digest_pinned_accepts_a_sha_digest_and_rejects_a_bare_tag() {
        assert!(ImageRef {
            reference: "img@sha256:deadbeef".into()
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
        // `@sha256:` with an empty digest is NOT pinned (fail-closed).
        assert!(!ImageRef {
            reference: "img@sha256:".into()
        }
        .digest_pinned());
        // a non-hex digest is NOT pinned.
        assert!(!ImageRef {
            reference: "img@sha256:nothex!!".into()
        }
        .digest_pinned());
    }

    #[test]
    fn image_ref_pinned_rejects_undigested_fail_closed() {
        let err = ImageRef::pinned("registry/img:latest").unwrap_err();
        assert!(matches!(err, SpecError::UndigestedImage { .. }));
        assert!(ImageRef::pinned("registry/img@sha256:abc123").is_ok());
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
            RunTokenRef { jti: "j".into() },
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
            RunTokenRef { jti: "j".into() },
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
            RunTokenRef { jti: "j".into() },
            MeterTarget {
                reserve_id: "r".into(),
            },
            IdemToken("i".into()),
        );
        assert_eq!(r.unwrap_err(), SpecError::NoTimeout);
    }

    // --- The JobSpec round-trip (serde) ---

    #[test]
    fn jobspec_round_trips_through_json() {
        let spec = ci_spec();
        let json = serde_json::to_string(&spec).unwrap();
        let back: JobSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, back);
    }

    #[test]
    fn egress_default_is_deny_all() {
        assert!(EgressPolicy::default().allow.is_empty());
        assert!(EgressPolicy::deny_all().allow.is_empty());
    }

    // --- The X-6 equivalence: ToolHands::exec IS launch(JobSpec{kind:Agent}) ---

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
            RunTokenRef {
                jti: "agent-jti".into(),
            },
            MeterTarget {
                reserve_id: "agent-res".into(),
            },
            IdemToken("agent-idem".into()),
        )
        .unwrap();
        assert_eq!(spec.kind, JobKind::Agent);
        // No checkout by default for an agent compute job.
        assert_eq!(spec.workspace, WorkspaceSpec::default());
        // Same fail-closed invariants — an un-digested agent image is rejected too.
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
            RunTokenRef { jti: "j".into() },
            MeterTarget {
                reserve_id: "r".into(),
            },
            IdemToken("i".into()),
        );
        assert!(matches!(bad, Err(SpecError::UndigestedImage { .. })));
    }

    // --- The trait shapes compile (no impl at P-129; the four-guarantee hooks wire) ---

    /// A no-op backend proving the SandboxBackend trait shape compiles + the four-guarantee hooks
    /// are reachable from `launch`. The REAL backend (Firecracker) is CI-P2 (→ P-237); this is a
    /// shape-asserting stub, NOT a host-exec path (the `no-host-exec` lint admits this file
    /// because there is no `std::process::Command` etc — all execution goes through this seam).
    struct NoopBackend;
    impl SandboxBackend for NoopBackend {
        type Error = HookError;
        fn launch(
            &self,
            spec: &JobSpec,
            hooks: &RunnerHooks,
        ) -> Result<SandboxLaunch, Self::Error> {
            // Drive the four-guarantee seam exactly as a real backend must:
            (hooks.isolation_floor)(spec)?; // #4 isolation floor
            (hooks.attribute)(&spec.run_token)?; // #2 attribution
            let res = (hooks.reserve)(&spec.meter_to)?; // #1a cost gate (reserve)
                                                        // ... the guest would run here (a real backend launches the hardened VM) ...
            // CT-001: the seam now carries the command result; the metering settle (guarantee #1)
            // settles against `result.usage`.
            let result = SandboxResult::stub_ok(ResourceUsage {
                cpu_seconds: 1,
                mem_byte_seconds: 1,
            });
            (hooks.settle)(&res, result.usage)?; // #1b settle (against the result's usage)
            Ok(SandboxLaunch {
                handle: SandboxHandle {
                    guest_id: "noop-guest".into(),
                },
                result,
            })
        }
        fn kill(&self, _h: &SandboxHandle) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    fn test_hooks() -> RunnerHooks {
        RunnerHooks {
            reserve: Box::new(|m| Ok(ReserveHandle(format!("reserved:{}", m.reserve_id)))),
            settle: Box::new(|_h, _u| Ok(())),
            attribute: Box::new(|_t| Ok(())),
            isolation_floor: Box::new(|_s| Ok(())),
        }
    }

    #[test]
    fn sandbox_backend_launch_drives_the_four_guarantee_hooks() {
        let backend = NoopBackend;
        let hooks = test_hooks();
        let launch = backend.launch(&ci_spec(), &hooks).unwrap();
        assert_eq!(launch.handle.guest_id, "noop-guest");
        // The seam now carries the command result back (CT-001 stub): a clean exit, not timed out.
        assert_eq!(launch.result.exit_code, Some(0));
        assert!(!launch.result.timed_out);
        assert!(launch.result.passed());
        backend.kill(&launch.handle).unwrap();
    }

    #[test]
    fn the_cost_gate_hook_can_refuse_to_start_on_exhaustion() {
        // Guarantee #1: reserve refuses on exhaustion → launch fails fail-closed (never starts).
        let backend = NoopBackend;
        let hooks = RunnerHooks {
            reserve: Box::new(|_m| Err(HookError("wallet exhausted".into()))),
            settle: Box::new(|_h, _u| Ok(())),
            attribute: Box::new(|_t| Ok(())),
            isolation_floor: Box::new(|_s| Ok(())),
        };
        let r = backend.launch(&ci_spec(), &hooks);
        assert_eq!(r.unwrap_err(), HookError("wallet exhausted".into()));
    }

    #[test]
    fn the_isolation_floor_hook_gates_launch() {
        // Guarantee #4: if the hardening profile cannot be applied/verified, launch fails closed
        // BEFORE any untrusted code runs (the seam CI-P5's escape drill drives).
        let backend = NoopBackend;
        let hooks = RunnerHooks {
            reserve: Box::new(|m| Ok(ReserveHandle(m.reserve_id.clone()))),
            settle: Box::new(|_h, _u| Ok(())),
            attribute: Box::new(|_t| Ok(())),
            isolation_floor: Box::new(|_s| Err(HookError("hardening profile not met".into()))),
        };
        let r = backend.launch(&ci_spec(), &hooks);
        assert!(r.is_err());
    }

    /// The FleetProvider trait shape compiles (no impl at P-129; the fleet impl is CI-P14).
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

    #[test]
    fn hitl_withhold_note_states_the_routing_split() {
        // Guarantee #3 is structural (no mutation API on the seam) + documented once.
        assert!(hitl_withhold_note().contains("EffectApi::apply"));
    }
}
