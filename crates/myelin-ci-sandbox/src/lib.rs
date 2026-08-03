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

pub mod asset_registry;
pub mod canonical_tar;
mod checkout_authorization;
pub mod checkout_orchestration;
mod dirlock;
pub mod escape_corpus;
pub mod events;
pub mod firecracker;
pub mod gvisor;
pub mod hardening;
mod launch_gate;
pub mod notif_rules;
pub mod redaction;
pub use redaction::{
    ResolvedJobSecrets, ResolvedSecretEnv, SecretInjectionError,
};
pub mod replay;
pub mod rootfs_overlay;
pub mod runner;
pub mod self_hosted;
pub mod snapshot_pool;
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

// CI-P28 (P-423): the gVisor escape-drill bundle builders (the corpus RE-RUNS on the gVisor backend
// — the permanent gate, contract 8.4). Exercised by tests/escape_drill_gvisor_test.rs against a real
// `runsc` sandbox; the host-side parser + attestation format are SHARED with the Firecracker drill.
pub use gvisor::{
    build_gvisor_corpus_script, gvisor_drill_config_json, resolved_gvisor_rootfs,
    GVISOR_CORPUS_SCRIPT,
};

// CT-007 gate 2/4 (registry slice): `spec.image` is now the real launch authority for an ordinary
// (non-git-wire) gVisor launch — `GvisorBackend::new` takes a [`GvisorAssetRegistry`] and
// `launch_with` resolves + verifies `spec.image` against it BEFORE any resource is reserved. The
// rust-capable runner asset's own resolver is exported alongside the base/git ones above.
pub use asset_registry::{
    file_sha256_hex, resolved_gvisor_cargo_vendor, AssetRegistryError, CargoVendorAssetBinding,
    GvisorAssetRegistry, RootfsAssetBinding, VerifiedCargoVendor, VerifiedRootfs,
    CARGO_VENDOR_SMOKE_LOCK_SHA256, CARGO_VENDOR_SMOKE_TREE_SHA256, ENV_GVISOR_CARGO_VENDOR,
};
pub use canonical_tar::canonical_tree_sha256_hex;
pub use gvisor::{
    resolved_gvisor_rust_rootfs, ENV_GVISOR_RUST_ROOTFS, GVISOR_GIT_ROOTFS_SHA256,
    LINUX_RUST_V1_ROOTFS_SHA256, LINUX_SMALL_V1_ROOTFS_SHA256,
};

// CT-006a (GT-006 / SI-013): the SANDBOXED GIT-WIRE capability — canonical `git upload-pack`/
// `receive-pack` run in the hardened gVisor sandbox with the bare repo bound READ-ONLY at `/repo`, a
// writable `/quarantine`, bounded stdin (the stateless-rpc request body) + captured stdout. The
// (tenant, region, repo) locator is resolver-validated (the GT-001 cross-tenant boundary, replicated)
// before any mount. Exercised by tests/git_wire_prod_exec_test.rs against a real `runsc` sandbox.
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
#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// Ephemeral broker resolution coupled to its exact redaction plan. Private and non-serializable:
    /// durable templates and records retain only `secret_refs`.
    resolved_secrets: ResolvedJobSecrets,
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
    pub run_token: RunTokenCredential,
    /// Server-resolved facts the final attribution hook must bind the signed credential to. This
    /// value is deliberately ephemeral: durable templates omit it, and the claim-time resolver
    /// constructs it only after locking the exact scheduler generation.
    pub run_token_authorization: Option<RunTokenAuthorizationContext>,
    /// The reserve this job settles against (run-level / agent-run-level) — guarantee #1, the cost
    /// gate (contract 11.7).
    pub meter_to: MeterTarget,
    /// Minted by the workflow at `SCHEDULE_AND_RUN_JOB` dispatch (OQ-F); the runner stamps it on
    /// the `job.done` signal — producer/consumer agree, no round-trip.
    pub idem_token: IdemToken,
}

/// Immutable, non-launchable job template persisted while work waits in the durable queue.
///
/// It intentionally omits [`RunTokenCredential`]: per-job credentials are short-lived and must be minted
/// only after the scheduler has issued an exact live claim. A template becomes executable solely
/// through [`JobSpecTemplate::resolve`], which attaches that claim-generation token immediately
/// before launch. This keeps bearer/token lifetime equal to activity lifetime without persisting an
/// expiring JTI in queued work.
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
    /// a digest is `@<algo>:<hex>` where the hex part is non-empty, all-hex, AND its length matches
    /// the named algorithm (R0.7-B): `sha256` → 64 hex, `sha384` → 96, `sha512` → 128. An unknown
    /// algorithm is rejected fail-closed — an unrecognized digest algorithm is not a trustworthy pin
    /// (a short/truncated digest like `@sha256:ab` is no longer accepted as "pinned").
    pub fn digest_pinned(&self) -> bool {
        self.parse_digest().is_some()
    }

    /// Parse the digest algorithm and hex digest out of an `@<algo>:<hex>`-pinned reference, reusing
    /// the EXACT SAME strict validation [`digest_pinned`](Self::digest_pinned) enforces (hex length
    /// matches the named algorithm; an unknown/unrecognized algorithm ⇒ `None`, never a guess).
    /// `None` for an unpinned or malformed reference. Used by
    /// [`crate::asset_registry::GvisorAssetRegistry::from_bindings`] to recover the digest a
    /// registered rootfs must hash to, rather than re-deriving the parsing rule a second time.
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

    /// The exact hex-string length for a known digest algorithm (R0.7-B). `None` for an unknown
    /// algorithm — which [`ImageRef::digest_pinned`] treats as not-pinned (fail-closed).
    fn digest_hex_len(algo: &str) -> Option<usize> {
        match algo {
            "sha256" => Some(64),
            "sha384" => Some(96),
            "sha512" => Some(128),
            _ => None,
        }
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
    /// The disk-backed EPHEMERAL WORKSPACE quota, bytes — the real host-disk-backed writable
    /// workspace a job's checkout/build lives in (e.g. `cargo build` output, which can run into
    /// many GB of `target/`). NOT the RAM-backed `/tmp` scratch tmpfs — see [`Self::tmpfs_bytes`]
    /// for that. No disk-backed-workspace mount is wired up yet (a later step); this field is the
    /// type-level home for that quota in advance of the mount implementation.
    pub disk_bytes: u64,
    /// The RAM-backed `/tmp` scratch tmpfs ceiling, bytes (CT-003a). This bounds gVisor's
    /// otherwise-unbounded host-RAM-backed `/tmp` tmpfs, so a disk fill hits `ENOSPC` instead of
    /// consuming host RAM without limit. SHOULD be <= `mem_bytes` (it is carved out of the same
    /// host RAM the job's memory ceiling bounds) — not enforced yet, that's a later step.
    pub tmpfs_bytes: u64,
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
    /// The EXACT commit object id to check out (CT-007 slice 5b.3-1 — corrected stale doc: this is
    /// NOT a ref name or an abbreviated hash; `crate::workspace_intent::derive_workspace_intent`
    /// requires a full 40-character (SHA-1) or 64-character (SHA-256) lowercase-hex object id,
    /// content-addressed and non-ambiguous, since it becomes the wire `want` this crate's own
    /// checkout-preparation transport fetches).
    pub commit: Option<String>,
}

/// CT-007 slice 5b.3-2b: the ONLY sanctioned way an external crate (the control-plane authority
/// chain) may derive a job's checkout-authorization scope from its `kind`/`workspace`. Wraps the
/// private `workspace_intent` module's parsing/validation (Sol's review: an external caller must
/// never duplicate those rules itself, nor see the private `WorkspaceIntent`/
/// `ValidatedCheckoutRequest` types directly). `Ok(None)` for an ordinary compute job (`workspace`
/// sets neither field); `Ok(Some(scope))` for a syntactically-valid checkout-bearing job; `Err` for
/// a malformed `workspace` (mixed `Some`/`None`, an unparseable `repo_ref`, or a malformed
/// `commit`). This performs ONLY the same syntactic validation `derive_workspace_intent` always did
/// — no authority, tenant, or repo-read check; callers still need their own cross-check against
/// durable authority state before trusting the result for anything security-relevant.
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

/// The per-job attenuated run token reference (`mint_run_token`, contract 4.7) — guarantee #2.
/// Carries the token's `jti`/handle, not the token material.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunTokenRef {
    /// The token id (`jti`) of the per-run attenuated token. Life == run life; auto-revoked on
    /// teardown; re-mintable mid-workflow on resume (S-11).
    pub jti: String,
}

/// A freshly minted, executable per-job credential.
///
/// Unlike [`RunTokenRef`], this type carries the opaque bearer material needed at the final launch
/// boundary. It deliberately implements neither `Serialize` nor `Deserialize`, redacts both the
/// bearer and JTI from `Debug`, and zeroizes the bearer on drop. Durable queue/spec records must use
/// [`JobSpecTemplate`] plus a stable authority handle and may never contain this value.
#[derive(Clone, PartialEq, Eq)]
pub struct RunTokenCredential {
    /// Public revocation/attribution reference. The bearer remains private.
    pub jti: String,
    bearer: String,
    ttl_secs: u64,
}

/// Expected, non-secret facts for final-boundary run-token authorization.
///
/// This context grants nothing by itself. A launch hook must cryptographically verify the opaque
/// bearer and compare its signed claims with every field here immediately before sandbox launch.
/// It deliberately implements neither `Serialize` nor `Deserialize`, so queued work cannot persist
/// or customer-supply a claimed authorization result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunTokenAuthorizationContext {
    /// One exact hosted CI job under one live scheduler claim generation.
    CiJob(CiJobAuthorizationContext),
}

/// Durable-claim facts a signed CI job credential must match at the launch boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiJobAuthorizationContext {
    pub tenant_id: String,
    pub region: String,
    pub principal_id: String,
    /// Server-verified project scope used to derive the job-specific secret principal and binding.
    pub project_id: String,
    pub wf_run_id: String,
    pub job_id: String,
    pub lease_owner: String,
    pub lease_epoch: i64,
    pub claim_nonce: String,
    pub claim_started_at_epoch_secs: i64,
    pub claim_expires_at_epoch_secs: i64,
    /// Exact metering reservation attested by the signed capability vector. The launch boundary
    /// re-derives it from the in-hand `JobSpec.meter_to.reserve_id` and requires exact equality.
    pub reserve_id: String,
    pub required_capabilities: Vec<String>,
    /// CT-007 slice 5b.3-2c: the checkout target this job's durable claim was minted against, when
    /// it has one. `required_capabilities` carries both the dynamic `repo:<ref>#pull` grant and a
    /// non-operational `checkout-commit:<format>:<hex>#attest` fact. A launch hook must still
    /// re-derive the checkout scope from the in-hand `JobSpec.workspace`, require exact equality
    /// here, and require Identity's signed capability vector to match; neither the ephemeral
    /// context nor the signed bearer is sufficient alone.
    pub checkout_scope: Option<CheckoutAuthorizationScope>,
    /// **CT-007 phase-credential generations.** The exact durable credential generation this
    /// ephemeral context was resolved under, or `None` for the legacy V1 claim-bound shape whose
    /// signed `run_id` is the bare `job_id`. Present iff the resolver ran under
    /// `CiJobCredentialWriteVersion::V2PhaseBound`, so a rolling fleet keeps verifying legacy
    /// contexts through the unchanged job-id path while V2 contexts verify through the generation.
    pub credential_binding: Option<CiJobCredentialBinding>,
}

/// **CT-007 phase-credential generations: the ephemeral binding a V2 launch boundary re-verifies.**
///
/// Carries the durable generation's own facts PLUS the three immutable claim-identity fields the
/// generation digest binds that [`CiJobAuthorizationContext`] does not already hold (`ci_run_id`,
/// `token_authority_handle`, `idem_token`). Without those the boundary could only COMPARE the signed
/// `run_id` against a value it was handed; with them it RECOMPUTES the generation id from
/// server-resolved facts and requires the signed value to equal it — the same
/// "never trust, always recompute" posture `token_authority_handle` verification already has.
///
/// Like [`RunTokenAuthorizationContext`] this deliberately implements neither `Serialize` nor
/// `Deserialize`: a claimed authorization result must never be persistable or customer-suppliable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiJobCredentialBinding {
    pub binding_version: i16,
    /// The durable purpose vocabulary token (`checkout_advertise`, `checkout_fetch`,
    /// `checkout_materialization`, `workload`).
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

    /// Construct a bounded executable credential. Empty/overlong identifiers, empty bearer
    /// material, and non-positive TTLs are refused before a sandbox can receive the value.
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

    /// Expose the bearer only to the final-boundary verifier/broker that immediately consumes it.
    pub fn expose_bearer(&self) -> &str {
        &self.bearer
    }

    /// The short fail-static lifetime Identity minted this credential under.
    pub fn ttl_secs(&self) -> u64 {
        self.ttl_secs
    }

    /// Project the public attribution/revocation handle without exposing bearer material.
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

/// Structural refusal constructing an executable run-token credential. No variant carries token
/// material, so errors are safe to surface across the runner boundary.
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

    /// Split a resolved launch spec into its persistable template and ephemeral credential.
    /// Callers must not serialize or persist the returned credential alongside the template.
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

    /// Attach the complete broker resolution immediately before launch. The checked constructor is
    /// the only mutation path, so secret env and redaction coverage cannot be separated.
    pub fn with_resolved_secrets(
        mut self,
        bindings: Vec<ResolvedSecretEnv>,
    ) -> Result<Self, SecretInjectionError> {
        self.resolved_secrets = ResolvedJobSecrets::for_job(&self, bindings)?;
        Ok(self)
    }

    /// Validate the fail-closed injected-env ↔ redaction-plan equality before launch.
    pub fn validate_secret_coverage(&self) -> Result<(), SecretInjectionError> {
        self.resolved_secrets.validate_for_job(self)
    }

    /// Number of broker-resolved env entries attached to this ephemeral launch spec.
    pub fn resolved_secret_count(&self) -> usize {
        self.resolved_secrets.len()
    }

    pub(crate) fn resolved_secrets(&self) -> &ResolvedJobSecrets {
        &self.resolved_secrets
    }
}

impl JobSpecTemplate {
    /// Construct a durable launch template under the same fail-closed image/resource invariants as
    /// [`JobSpec::new`], but without minting or accepting a run token before the job is claimed.
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

    /// Attach the freshly minted claim-generation token and produce the only type a sandbox can
    /// launch. The template itself has no launch method and contains no credential.
    pub fn resolve(self, run_token: RunTokenCredential) -> JobSpec {
        self.resolve_with_authorization(run_token, None)
    }

    /// Attach a freshly minted credential plus its server-resolved final-boundary expectations.
    /// The durable template remains bearer- and authorization-context-free.
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
    // Delegates to the shared `ResourceLimits`-only validator (Sol's review: two independent
    // copies of the SAME two checks would be free to drift) — the specific `SpecError` variant is
    // still derived here, since the shared validator has no `JobSpec`-shaped error type to return.
    validate_execution_limits(limits).map_err(|_| {
        if limits.pids_max == 0 {
            SpecError::NoPidsMax
        } else {
            SpecError::NoTimeout
        }
    })
}

/// The `ResourceLimits`-only half of [`validate_job_shape`]'s fail-closed non-negotiables
/// (`pids_max`/`timeout_secs` MUST be set) — extracted (CT-007 slice 5b.2, Sol's review) so a
/// caller with no `ImageRef` to validate (the checkout-preparation runtime, which is deliberately
/// never a billed `JobSpec`) still enforces the SAME mandatory limits `JobSpec::new` would have,
/// rather than silently skipping them by bypassing `JobSpec` entirely.
pub(crate) fn validate_execution_limits(limits: &ResourceLimits) -> Result<(), String> {
    if limits.pids_max == 0 {
        return Err("pids_max (fork-bomb ceiling) must be set (> 0)".to_string());
    }
    if limits.timeout_secs == 0 {
        return Err("timeout_secs must be set (> 0)".to_string());
    }
    Ok(())
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
/// Returns the captured head bytes and whether any bytes beyond the bound were seen or the stream
/// could not be read through EOF (`truncated`). A read fault must not make a partial protocol payload
/// look complete to callers such as the Git wire adapter.
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
            Err(_) => {
                // The prefix is still useful for diagnostics, but it is not a complete stream. Mark
                // it truncated so protocol callers fail closed instead of serving partial bytes.
                truncated = true;
                break;
            }
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
    /// Whether every incremental output frame reached the runner-side consumer.
    ///
    /// A false value is still a measured, completed attempt: the runner must tear the sandbox down,
    /// durably accrue the attempt's usage, and requeue the exact claim without emitting `job.done`.
    /// Treating it as a bare launch error would discard the only value carrying measured usage and
    /// strand reporter-owned reservations.
    pub output_complete: bool,
}

/// Which captured guest stream produced an incremental output frame.
///
/// The stream identity stays inside the sandbox/runner seam. The current durable log is one ordered
/// job stream, so [`FirehoseSink`](crate::FirehoseSink) intentionally coalesces both variants after
/// the backend has preserved each stream's byte order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SandboxOutputStream {
    /// The command's standard output.
    Stdout,
    /// The command's standard error.
    Stderr,
}

/// A bounded incremental-output callback driven by a production sandbox while the command runs.
///
/// Implementations must consume or copy `frame` before returning. Production backends invoke this
/// from their pipe/console drain threads and apply boundary redaction before the callback. Returning
/// an error makes the launch fail loudly; the backend must still drain/kill/reap its guest so a
/// failed consumer cannot deadlock or orphan the sandbox.
pub trait SandboxOutputSink: Send + Sync {
    /// Consume one ordered frame from `stream`.
    fn emit(&self, stream: SandboxOutputStream, frame: &[u8]) -> Result<(), String>;
}

/// Shared cancellation edge from the runner's durable-output consumer to the live sandbox watchdog.
///
/// A persistence failure occurs on the runner thread while the backend owns the still-running guest.
/// This token lets the consumer request whole-guest teardown immediately, without waiting for the
/// command's ordinary timeout or requiring a handle before `launch_streaming` returns.
#[derive(Clone, Default)]
pub struct SandboxCancellation {
    cancelled: Arc<AtomicBool>,
}

impl SandboxCancellation {
    /// A fresh, not-cancelled execution token.
    pub fn new() -> SandboxCancellation {
        SandboxCancellation::default()
    }

    /// Request prompt whole-guest teardown. Idempotent.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Whether teardown has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// The atomic flag used by existing backend watchdog loops.
    pub(crate) fn as_atomic(&self) -> &AtomicBool {
        self.cancelled.as_ref()
    }
}

/// A [`SandboxBackend::launch`]/[`launch_streaming`](SandboxBackend::launch_streaming) failure,
/// carrying whatever the backend can honestly prove about whether a durable claim on the attempt
/// exists — the caller MUST dispatch on this before it can correctly release, settle, or report
/// (CT-007 vertical-slice: this exists to close a real, pre-existing leak where a launch failure
/// after the durable launch CAS committed, or after the guest genuinely started executing, was
/// indistinguishable from an ordinary pre-commit refusal, so the caller's cost reservation for a
/// real (if failed) attempt was silently never released NOR settled).
///
/// Deliberately has **no** blanket `From<E>` impl. A caller must explicitly choose
/// [`SandboxLaunchError::Failed`] (or one of the other variants) rather than letting `?` silently
/// reclassify every new backend error as an ordinary pre-commit refusal — the exact failure mode
/// this type exists to make impossible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxLaunchError<E> {
    /// No retryable-attempt record is available through this return — either the failure is an
    /// ordinary pre-commit refusal (isolation floor, hardening, image resolution, cost exhaustion,
    /// final attribution), or the backend has already performed whatever hook cleanup it supports
    /// for a genuinely ambiguous/unattemptable case. The caller propagates this as an ordinary
    /// launch failure; no terminal report, no retryable-attempt record.
    Failed(E),

    /// A durable running claim existed for this attempt (a committed launch permit, or a process
    /// that genuinely started executing) and the sandbox has been fully killed/reaped by the time
    /// this returned. Reporter-owned completion accounting MUST durably record `usage` under
    /// `cause` and either requeue the exact claim OR terminalize it (a concurrent supersession can
    /// win first) — settling this any other way (including silently dropping it) reproduces the
    /// leak this type exists to close.
    RetryableAttempt {
        source: E,
        cause: crate::runner::RetryableAttemptCause,
        usage: ResourceUsage,
    },

    /// The durable launch CAS may or may not have committed — the backend cannot honestly tell
    /// (e.g. the store returned an error but the underlying commit may have already landed and lost
    /// its acknowledgement). The caller must NOT release, settle, or report a retryable attempt
    /// under either guess; it must surface this loudly and let durable reconciliation (the existing
    /// lease/claim reaper) resolve the ambiguity instead.
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

/// The sandbox backend (arch 01 §2): Firecracker (default) | Gvisor (named 2nd) | SelfHosted
/// (delegated). The **trait SHAPE only** at P-129 — the Firecracker impl is CI-P2 (→ P-237), the
/// gVisor impl is CI-P28, the self-hosted impl is CI-P4. `launch` carries the [`RunnerHooks`]
/// through which the four uniform guarantees are wired (X-6; arch 02 §5.2).
///
/// **`ToolHands::exec(Command)` (contract 8.4) IS `launch(JobSpec{ kind: Agent, .. }, hooks)`** —
/// the same runner, the same hardening, the same drill. The `no-host-exec` lint (P-S10/P-017)
/// forbids any platform host-exec bypass; ALL execution goes through this seam.
pub trait SandboxBackend: Sync {
    /// The backend's error type.
    type Error: std::error::Error + Send;

    /// Launch a job in a fresh, ephemeral, one-job-per-sandbox guest, applying the mandatory
    /// hardening profile (arch 02 §5.3) and wiring the four-guarantee `hooks` (arch 02 §5.2). For an
    /// in-line compute job `launch` BLOCKS for the duration and returns a [`SandboxLaunch`] carrying
    /// BOTH the teardown [`SandboxHandle`] (the caller MUST eventually [`kill`](SandboxBackend::kill)
    /// it — idempotent if the guest already exited) AND the command's [`SandboxResult`]
    /// (exit/timeout/usage/captured-streams). The runner DERIVES its terminal report from the
    /// result; it is no longer supplied as an input (RESHAPE-001 / CT-001).
    fn launch(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
    ) -> Result<SandboxLaunch, SandboxLaunchError<Self::Error>>;

    /// Launch while delivering boundary-redacted output incrementally during execution.
    ///
    /// The default is a compatibility adapter for test doubles and delegated backends: it performs
    /// the ordinary blocking launch and emits the bounded result captures afterward. It therefore
    /// does **not** prove during-execution delivery. The production gVisor and Firecracker backends
    /// override this method at their real pipe/console drain boundaries.
    fn launch_streaming(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
        output: Arc<dyn SandboxOutputSink>,
        _cancellation: SandboxCancellation,
    ) -> Result<SandboxLaunch, SandboxLaunchError<Self::Error>> {
        let launch = self.launch(spec, hooks)?;
        if !launch.result.stdout.is_empty() {
            let _ = output.emit(SandboxOutputStream::Stdout, &launch.result.stdout);
        }
        if !launch.result.stderr.is_empty() {
            let _ = output.emit(SandboxOutputStream::Stderr, &launch.result.stderr);
        }
        Ok(launch)
    }

    /// **The typed sandbox/runner CYCLE seam (activated by CT-007 slice 5b.3-6e.2).**
    ///
    /// [`SandboxLaunch`] can only ever describe a launched workload; it cannot represent a checkout
    /// PREPARATION that terminalized (`AttemptsExhausted`, a Hop-B failure), requeued, or demanded
    /// reconciliation before any workload ran. [`SandboxCycleOutcome`] is the strictly wider result a
    /// single sandbox cycle can produce once the checkout path is selected.
    ///
    /// **The default is a byte-faithful compatibility wrapper:** it performs the ordinary streaming
    /// launch and maps its `Ok` into [`SandboxCycleOutcome::WorkloadLaunched`], so EVERY existing
    /// backend and test double satisfies the seam without changing a line. Production
    /// [`RunnerAgent`](crate::runner::RunnerAgent) calls this method; gVisor overrides it to produce
    /// preparation variants for checkout-bearing specs.
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

/// **The typed outcome of one activated sandbox cycle.**
///
/// The strictly-wider return of [`SandboxBackend::run_cycle`]. A compute cycle and a
/// checkout-preparation cycle both flow through the same runner lane, but only a WORKLOAD produces a
/// [`SandboxLaunch`]; a preparation can terminalize, requeue, or demand fail-closed reconciliation
/// BEFORE any workload exists. This enum is the seam where the runner routes each of those.
///
/// It mirrors [`CheckoutContinuationOutcome`](crate::checkout_orchestration::CheckoutContinuationOutcome)'s
/// variants exactly and offers a total [`From`] conversion, so the gVisor backend's future checkout
/// `run_cycle` override (5b.3-6e.2) can lift the orchestration's own result into this interface type
/// without re-deriving the disposition vocabulary. `#[allow(dead_code)]` because — like every 6e.1
/// chassis symbol — nothing production reads these fields until the activating slice selects the
/// checkout path.
#[derive(Debug)]
#[allow(dead_code)]
pub enum SandboxCycleOutcome {
    /// Preparation (if any) succeeded and the workload launched — the ordinary [`SandboxLaunch`]
    /// firehose/kill/report/settlement tail takes over.
    WorkloadLaunched(SandboxLaunch),
    /// The workload ran but failed retryably — reporter-owned workload retry accounting requeues the
    /// exact claim and durably accounts the measured usage without emitting `job.done`.
    WorkloadRetryable {
        /// The typed retryable cause.
        cause: crate::runner::RetryableAttemptCause,
        /// The measured usage the retry transaction accounts.
        usage: ResourceUsage,
        /// The operator-facing detail.
        message: String,
    },
    /// Preparation reached a terminal disposition (`Failed`/`TimedOut`/`AttemptsExhausted`) before any
    /// workload — the preparation reporter settles it; the ordinary workload reporter and
    /// `LeaseStore::settle` are NOT called.
    PreparationTerminal {
        /// The preparation reporting identity to settle against.
        claim: crate::runner::PreparationReportClaim,
        /// The terminal disposition (its own accounting/signal semantics).
        disposition: crate::runner::PreparationTerminalDisposition,
        /// Retained operator-safe detail explaining the preparation failure, when one exists.
        diagnostic: Option<String>,
    },
    /// Preparation failed retryably — the preparation requeue reporter re-queues the exact claim and
    /// emits no `job.done`.
    PreparationRetryable {
        /// The preparation reporting identity to requeue.
        claim: crate::runner::PreparationReportClaim,
        /// The phase the retryable failure occurred in.
        phase: crate::runner::PreparationPhase,
    },
    /// Preparation left resources in an unproven state (teardown unproven, usage unrepresentable, or
    /// quarantine required) — the runner lane must stop FAIL-CLOSED and leave durable recovery to the
    /// reaper; it must not keep claiming work with possibly-live/quarantined resources.
    ReconciliationRequired {
        /// The phase reconciliation is required for.
        phase: crate::runner::PreparationPhase,
        /// The guest teardown could not be proven complete.
        teardown_unproven: bool,
        /// The measured usage could not be represented for settlement.
        usage_unrepresentable: bool,
        /// The workspace/identity must be quarantined before reuse.
        quarantine_required: bool,
    },
}

impl From<crate::checkout_orchestration::CheckoutContinuationOutcome> for SandboxCycleOutcome {
    /// Lift a checkout orchestration's own outcome into the runner-cycle interface type. Total and
    /// variant-for-variant, so a future exhaustive `match` on either type stays in lock-step.
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
/// 1. **Universal cost gate** ([`reserve`](RunnerHooks::reserve) /
///    [`settle_completed`](RunnerHooks::settle_completed)) — reserve at dispatch,
///    refuse-on-exhaustion, settle on completion, never interrupt in-flight; CI runs and agent runs
///    meter into the SAME ledger (contract 11.7). Completion settlement has one explicit owner:
///    either this hook or the terminal reporter's atomic transaction.
/// 2. **Attribution** ([`attribute`](RunnerHooks::attribute)) — the job runs under a per-run
///    attenuated token (`mint_run_token`, contract 4.7); after reserve, this is the final pre-spawn
///    authorization boundary. A refusal settles the unused reserve at zero and starts no guest.
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
    /// The one component allowed to commit successful completion settlement. Generic agent execution
    /// settles through the hook; CI defers to its claim/receipt/signal reporter transaction.
    completion_settlement_owner: CompletionSettlementOwner,
    /// Guarantee #1a: reserve budget at dispatch; `Err` == exhausted → refuse-to-start (contract
    /// 11.7, `reserve_budget`). Returns the reserve handle on success.
    reserve: ReserveHook,
    /// Guarantee #1b: settle the reserve on completion (contract 11.7, `settle_budget`); the
    /// unused reserve is released, never interrupting in-flight.
    settle: SettleHook,
    /// Guarantee #2: reauthorize the per-run attenuated token and retain any durable launch fence
    /// until a sandbox child exists but remains mechanically unable to execute.
    attribute: AttributionHook,
    /// Guarantee #4: the isolation-floor hook the escape drill (CI-P5) drives — apply + verify the
    /// mandatory hardening profile (arch 02 §5.3) before any untrusted code runs.
    isolation_floor: IsolationFloorHook,
    /// CT-007 slice 5b.3-2: the pre-Hop-A checkout-authorization hook. `None` for every ordinary
    /// (non-checkout) caller, and for any caller that has not yet configured one via
    /// [`Self::with_checkout_authorization`] — [`Self::authorize_checkout`] refuses outright on
    /// `None` rather than silently treating a missing hook as authorization granted.
    checkout_authorization: Option<CheckoutAuthorizationHook>,
    /// CT-007 phase-credential generations: the V2 per-phase authorization hook. Legacy callers
    /// leave it `None`; the activated V2 runner wiring installs it. A missing hook refuses outright
    /// and never falls back to the claim-bound hook.
    checkout_phase_authorization: Option<CheckoutPhaseAuthorizationHook>,
    /// CT-007 slice 5b.3-6c: the V2 parent-attempt reservation mode. Legacy callers leave it `None`;
    /// the activated V2 runner wiring installs it. [`Self::reserve_parent_attempt`] refuses outright
    /// on `None`.
    parent_attempt_reserve: Option<ParentAttemptReserveHook>,
}

/// Single durable owner of successful completion settlement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionSettlementOwner {
    /// The sandbox completion hook commits settlement before returning the result.
    Hook,
    /// The terminal reporter co-commits settlement with claim consumption, receipt, and signal.
    TerminalReporter,
}

/// Guarantee #1a hook type (contract 11.7 reserve_budget): begin the exact reservation carried by
/// the fully resolved job, `Err` == unavailable/exhausted. The complete [`JobSpec`] is required so a
/// durable implementation can bind the reservation to the same tenant, region, workflow, job, and
/// claim generation as final attribution; a bare caller-controlled meter string is insufficient.
pub type ReserveHook = Box<dyn Fn(&JobSpec) -> Result<ReserveHandle, HookError> + Send + Sync>;
/// **CT-007 slice 5b.3-6c: the V2 parent-attempt reservation mode, alongside the legacy
/// [`ReserveHook`].** Unlike `ReserveHook` (which returns a bare handle), this one performs the exact
/// claim/v2-reservation validation, transitions the reservation to inflight, and inserts/replays the
/// exact parent-attempt row — all in one tenant transaction — returning a
/// [`ParentAttemptAdmission`](crate::checkout_orchestration::ParentAttemptAdmission) that also carries
/// the opaque per-attempt [`AttemptAuthority`](crate::checkout_orchestration::AttemptAuthority). The
/// activated V2 wiring installs this hook; the checkout orchestrator refuses outright when absent.
pub type ParentAttemptReserveHook = Box<
    dyn Fn(&JobSpec) -> Result<crate::checkout_orchestration::ParentAttemptAdmission, HookError>
        + Send
        + Sync,
>;
/// Guarantee #1b hook type (contract 11.7 settle_budget): settle/release the exact scoped job
/// reservation. The complete [`JobSpec`] remains present at both successful completion and
/// pre-spawn refusal, so the hook never has to recover tenant scope from an opaque handle.
pub type SettleHook =
    Box<dyn Fn(&JobSpec, &ReserveHandle, ResourceUsage) -> Result<(), HookError> + Send + Sync>;
/// Guarantee #2 hook type (contract 4.7 mint_run_token): final-boundary attribution over the
/// credential and the server-resolved expected launch facts carried by the complete spec.
pub type AttributeHook = Box<dyn Fn(&JobSpec) -> Result<(), HookError> + Send + Sync>;
/// Production launch-fence hook. The returned permit retains the durable generation fence until
/// the backend has spawned a gated child. Dropping it before commit rolls the fence back.
pub type LaunchFenceHook = Box<dyn Fn(&JobSpec) -> Result<LaunchPermit, HookError> + Send + Sync>;
/// Guarantee #4 hook type (arch 02 §5.3): apply + verify the mandatory hardening profile.
pub type IsolationFloorHook = Box<dyn Fn(&JobSpec) -> Result<(), HookError> + Send + Sync>;

/// Re-exported from the private `workspace_intent` module (CT-007 slice 5b.3-2a, Sol's review: ONE
/// enum, never a duplicated public mirror that could drift from it) — see
/// [`workspace_intent::GitObjectFormat`](crate::workspace_intent::GitObjectFormat) for the full doc.
pub use crate::workspace_intent::GitObjectFormat;

/// A narrow, read-only view of a checkout-bearing job's target (CT-007 slice 5b.3-2a) — handed to
/// the control-plane's [`CheckoutAuthorizationHook`], deliberately NOT the full
/// `workspace_intent::ValidatedCheckoutRequest` (Sol's review: the authorizer needs exactly the
/// repo/commit dimension, nothing else). Carries plain, already-validated data — no secrets, no
/// filesystem paths (the opaque `repo_id` is never resolved to one here).
///
/// Fields are private (Sol's review): the redundant `tenant`/`repo_ref`/`repo_id` and
/// `commit_hex`/`commit_format` pairs could otherwise be constructed disagreeing with each other by
/// any public-field literal. `Self::new` is `pub(crate)` — the ONLY real caller is
/// `workspace_intent::ValidatedCheckoutRequest::to_authorization_scope`, which always derives every
/// field from the SAME already-validated request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckoutAuthorizationScope {
    tenant: myelin_tenancy::TenantId,
    repo_ref: myelin_events::ArtifactRef,
    repo_id: String,
    commit_hex: String,
    commit_format: GitObjectFormat,
}

impl CheckoutAuthorizationScope {
    #[allow(dead_code)]
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

/// The pre-Hop-A checkout-authorization hook (CT-007 slice 5b.3-2): a READ-ONLY check (never a
/// state transition — the real workload's `leased -> running` CAS stays `attribute`/
/// `acquire_launch_permit`'s job, committed only later, after a successful Hop B) that the job's
/// durably authorized claim actually grants read access to the EXACT repo/commit its
/// `WorkspaceIntent` names. `None` on a [`RunnerHooks`] that never configured one (e.g. every
/// ordinary compute-job caller) — `RunnerHooks::authorize_checkout` refuses outright rather than
/// silently treating a missing hook as authorized.
pub type CheckoutAuthorizationHook =
    Box<dyn Fn(&JobSpec, &CheckoutAuthorizationScope) -> Result<(), HookError> + Send + Sync>;

/// **CT-007 phase-credential generations: the V2 per-phase authorization hook.** Unlike
/// [`CheckoutAuthorizationHook`] (read-only, returns `()`), this one RETURNS the retained durable
/// [`LaunchPermit`] that the raw spawn gate must consume — so a preparation container can only ever
/// be launched through a permit the control plane's own durable phase gate produced, never through
/// an internally-minted [`LaunchPermit::immediate`].
pub type CheckoutPhaseAuthorizationHook = Box<
    dyn Fn(&JobSpec, &CheckoutAuthorizationScope, CheckoutPhase) -> Result<LaunchPermit, HookError>
        + Send
        + Sync,
>;

/// An unforgeable, one-shot proof that [`RunnerHooks::authorize_checkout`] genuinely succeeded for
/// an EXACT [`CheckoutAuthorizationScope`] AND an exact token generation (CT-007 slice 5b.3-2a,
/// Sol's review). Defined in the private `checkout_authorization` module — NOT here at the crate
/// root — because Rust's privacy rules make a private field visible to every descendant module of
/// its defining module; if this type lived here, `gvisor.rs` (a descendant of the crate root, same
/// as every module in this crate) could forge one via a struct literal, defeating the whole
/// capability guarantee. Living in its own sibling module means only that module — the one that
/// actually calls the hook — can ever construct one.
#[allow(unused_imports)]
pub(crate) use checkout_authorization::CheckoutAuthorizationProof;
/// CT-007 round-1 blocker 2: the fused, non-constructible phase authorization. Same module-privacy
/// reasoning as `CheckoutAuthorizationProof` — only `checkout_authorization` can build one.
#[allow(unused_imports)]
pub(crate) use checkout_authorization::PhaseAuthorization;
/// CT-007 phase-credential generations: the preparation-boundary vocabulary. PUBLIC (unlike the
/// proof itself) because the control plane supplies the hook that receives it.
pub use checkout_authorization::CheckoutPhase;

enum AttributionHook {
    Immediate(AttributeHook),
    Fenced(LaunchFenceHook),
}

/// Opaque durable launch-generation fence committed at the gated child-spawn boundary.
pub struct LaunchPermit {
    commit: Option<Box<dyn FnOnce() -> Result<LaunchOwnership, HookError> + Send>>,
}

impl LaunchPermit {
    /// A permit for non-durable/test hooks that completed attribution immediately.
    pub fn immediate() -> Self {
        Self {
            commit: Some(Box::new(|| Ok(LaunchOwnership::immediate()))),
        }
    }

    /// Wrap a lazy durable fence. The closure runs only after the launch guard is armed and returns
    /// session ownership that prevents a paused post-commit continuation from being reaped.
    pub fn retained(
        commit: impl FnOnce() -> Result<LaunchOwnership, HookError> + Send + 'static,
    ) -> Self {
        Self {
            commit: Some(Box::new(commit)),
        }
    }

    /// Commit the launch generation while the spawned child is mechanically blocked from execing
    /// the sandbox runtime. The launch gate releases the child only after this returns successfully.
    pub fn commit(mut self) -> Result<LaunchOwnership, HookError> {
        self.commit
            .take()
            .expect("launch permit commit is single-use")()
    }

    /// Compatibility path for immediate/non-spawning checks.
    pub fn commit_and_release(self) -> Result<(), HookError> {
        self.commit()?.validate()?.release()
    }
}

/// Ownership retained after durable launch commit, validated before the gate, and held across the
/// exact gate write. Production uses a PostgreSQL session advisory lock: a paused runner keeps the
/// row unreapable, while process/connection death releases the lock fail-closed.
#[must_use = "launch ownership must be released only at the sandbox exec boundary"]
pub struct LaunchOwnership {
    validate: Option<Box<dyn FnOnce() -> Result<ValidatedLaunchOwnership, HookError> + Send>>,
}

impl LaunchOwnership {
    /// No-op ownership for immediate/test attribution hooks.
    pub fn immediate() -> Self {
        Self {
            validate: Some(Box::new(|| Ok(ValidatedLaunchOwnership::immediate()))),
        }
    }

    /// Wrap production session-lock validation. A successful validation returns the still-held
    /// ownership that the launch gate releases only after it has delivered the exec byte.
    pub fn retained(
        validate: impl FnOnce() -> Result<ValidatedLaunchOwnership, HookError> + Send + 'static,
    ) -> Self {
        Self {
            validate: Some(Box::new(validate)),
        }
    }

    /// Validate that ownership is still live while retaining it across the child-gate write.
    pub fn validate(mut self) -> Result<ValidatedLaunchOwnership, HookError> {
        self.validate
            .take()
            .expect("launch ownership validation is single-use")()
    }
}

/// A validated durable launch ownership retained across the exact child-gate write.
#[must_use = "validated launch ownership must be released after the sandbox gate opens"]
pub struct ValidatedLaunchOwnership {
    release: Option<Box<dyn FnOnce() -> Result<(), HookError> + Send>>,
}

impl ValidatedLaunchOwnership {
    /// No-op ownership for immediate/test attribution hooks.
    pub fn immediate() -> Self {
        Self {
            release: Some(Box::new(|| Ok(()))),
        }
    }

    /// Wrap the production session-lock release.
    pub fn retained(release: impl FnOnce() -> Result<(), HookError> + Send + 'static) -> Self {
        Self {
            release: Some(Box::new(release)),
        }
    }

    /// Release ownership after the already-gated child has received its exec byte.
    pub fn release(mut self) -> Result<(), HookError> {
        self.release
            .take()
            .expect("validated launch ownership release is single-use")()
    }
}

impl RunnerHooks {
    /// Build the lifecycle hook bundle with one explicit successful-completion settlement owner.
    ///
    /// The closures stay private so a backend cannot bypass [`settle_completed`](Self::settle_completed)
    /// and accidentally settle in both the sandbox hook and terminal reporter.
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
            checkout_authorization: None,
            checkout_phase_authorization: None,
            parent_attempt_reserve: None,
        }
    }

    /// Build a lifecycle bundle whose final authorization is committed at the gated child-spawn
    /// boundary.
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
            checkout_authorization: None,
            checkout_phase_authorization: None,
            parent_attempt_reserve: None,
        }
    }

    /// CT-007 slice 5b.3-2: attach the checkout-authorization hook. Consuming builder — every
    /// existing `new`/`new_with_launch_fence` caller is byte-unchanged (`checkout_authorization`
    /// defaults to `None`); only a caller that actually dispatches checkout-bearing jobs needs to
    /// call this.
    #[allow(dead_code)]
    pub fn with_checkout_authorization(mut self, hook: CheckoutAuthorizationHook) -> Self {
        self.checkout_authorization = Some(hook);
        self
    }

    /// CT-007 phase-credential generations: attach the V2 per-phase authorization hook. Additive in
    /// exactly the same way — every existing caller stays byte-unchanged and keeps `None`, so the
    /// whole phase-bound API is unreachable until a caller explicitly opts in.
    #[allow(dead_code)]
    pub fn with_checkout_phase_authorization(
        mut self,
        hook: CheckoutPhaseAuthorizationHook,
    ) -> Self {
        self.checkout_phase_authorization = Some(hook);
        self
    }

    /// CT-007 slice 5b.3-6c: attach the V2 parent-attempt reservation mode. Additive exactly like the
    /// other builders — every existing caller stays byte-unchanged and keeps `None`, so the whole
    /// parent-attempt admission path is unreachable until a caller (5b.3-6e) explicitly opts in.
    #[allow(dead_code)]
    pub fn with_parent_attempt_reservation(mut self, hook: ParentAttemptReserveHook) -> Self {
        self.parent_attempt_reserve = Some(hook);
        self
    }

    /// CT-007 slice 5b.3-6c: admit/reserve a parent attempt through the V2 reservation mode. Refuses
    /// outright when no parent-attempt reserve hook was configured — a missing hook is never treated
    /// as "admitted", and the legacy [`Self::reserve`] is deliberately NOT a fallback (a bare reserve
    /// handle carries no parent-attempt row, which every V2 settlement owner requires).
    #[allow(dead_code)]
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

    /// The one configured owner of successful completion settlement.
    pub fn completion_settlement_owner(&self) -> CompletionSettlementOwner {
        self.completion_settlement_owner
    }

    /// Apply and verify the mandatory isolation floor before any reservation or guest launch.
    pub fn enforce_isolation_floor(&self, spec: &JobSpec) -> Result<(), HookError> {
        (self.isolation_floor)(spec)
    }

    /// Reserve operational capacity before final attribution and guest launch.
    pub fn reserve(&self, spec: &JobSpec) -> Result<ReserveHandle, HookError> {
        (self.reserve)(spec)
    }

    /// Perform the final launch-attribution check.
    pub fn attribute(&self, spec: &JobSpec) -> Result<(), HookError> {
        self.acquire_launch_permit(spec)?.commit_and_release()
    }

    /// Authorize and retain the exact launch generation for a sandbox backend.
    pub fn acquire_launch_permit(&self, spec: &JobSpec) -> Result<LaunchPermit, HookError> {
        match &self.attribute {
            AttributionHook::Immediate(attribute) => {
                attribute(spec)?;
                Ok(LaunchPermit::immediate())
            }
            AttributionHook::Fenced(attribute) => attribute(spec),
        }
    }

    /// Prepend a final attribution guard while preserving the configured settlement owner and all
    /// other lifecycle hooks.
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

    /// Release a reservation after final attribution refuses before spawn. There will be no terminal
    /// report in this path, so the real hook always owns the zero-usage release.
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

    /// Settle completed usage exactly once, or explicitly defer it to the terminal reporter.
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

/// CT-007 slice 5b.3-6c: a crate-level `#[cfg(test)]` checkout-bearing [`JobSpec`] carrying the
/// resolved run-token authorization context (region) the dormant orchestrator derives — shared by the
/// `gvisor` and `checkout_orchestration` unit tests. Uses a bare digest-pinned image (no rootfs
/// fixture needed — these tests never launch a real `runsc`).
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

    // --- The digest-pin-or-reject rule (CI-1; the headline non-negotiable) ---

    #[test]
    fn digest_pinned_accepts_a_sha_digest_and_rejects_a_bare_tag() {
        // A correct 64-hex sha256 digest IS pinned.
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
        // `@sha256:` with an empty digest is NOT pinned (fail-closed).
        assert!(!ImageRef {
            reference: "img@sha256:".into()
        }
        .digest_pinned());
        // a non-hex digest is NOT pinned.
        assert!(!ImageRef {
            reference: format!("img@sha256:{}", "z".repeat(64))
        }
        .digest_pinned());
        // R0.7-B: a too-SHORT sha256 digest (all-hex but only 8 chars) is NOT pinned — the length
        // must match the algorithm; a truncated digest is not a trustworthy pin.
        assert!(!ImageRef {
            reference: "img@sha256:deadbeef".into()
        }
        .digest_pinned());
        // R0.7-B: a too-LONG sha256 digest (65 hex) is NOT pinned.
        assert!(!ImageRef {
            reference: format!("img@sha256:{}", "a".repeat(65))
        }
        .digest_pinned());
        // R0.7-B: an UNKNOWN algorithm is NOT pinned (fail-closed), even with an all-hex 64-char body.
        assert!(!ImageRef {
            reference: format!("img@md5:{}", "a".repeat(64))
        }
        .digest_pinned());
        // A correct 128-hex sha512 digest IS pinned.
        assert!(ImageRef {
            reference: format!("img@sha512:{}", "b".repeat(128))
        }
        .digest_pinned());
    }

    #[test]
    fn image_ref_pinned_rejects_undigested_fail_closed() {
        let err = ImageRef::pinned("registry/img:latest").unwrap_err();
        assert!(matches!(err, SpecError::UndigestedImage { .. }));
        // R0.7-B: a full 64-hex sha256 digest is accepted; a short one is rejected.
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
            credential("agent-jti"),
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
            credential("j"),
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
        ) -> Result<SandboxLaunch, SandboxLaunchError<Self::Error>> {
            (|| -> Result<SandboxLaunch, HookError> {
                // Drive the four-guarantee seam exactly as a real backend must:
                hooks.enforce_isolation_floor(spec)?; // #4 isolation floor
                let res = hooks.reserve(spec)?; // #1a cost gate (reserve)
                if let Err(attribute_error) = hooks.attribute(spec) {
                    hooks.release_unused(spec, &res)?;
                    return Err(attribute_error);
                }
                // ... the guest would run here (a real backend launches the hardened VM) ...
                // CT-001: the seam now carries the command result; the metering settle (guarantee
                // #1) settles against `result.usage`.
                let result = SandboxResult::stub_ok(ResourceUsage {
                    cpu_seconds: 1,
                    mem_byte_seconds: 1,
                });
                hooks.settle_completed(spec, &res, result.usage)?; // #1b settle or explicit reporter deferral
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

    fn checkout_scope() -> CheckoutAuthorizationScope {
        CheckoutAuthorizationScope::new(
            myelin_tenancy::TenantId("acme".to_string()),
            myelin_events::ArtifactRef("myelin://acme/git/repo/widgets".to_string()),
            "widgets".to_string(),
            "a".repeat(40),
            GitObjectFormat::Sha1,
        )
    }

    #[test]
    fn authorize_checkout_refuses_when_no_hook_is_configured() {
        let hooks = test_hooks();
        let err = hooks
            .authorize_checkout(&ci_spec(), checkout_scope())
            .unwrap_err();
        assert!(err.0.contains("no hook was configured") || err.0.contains("none was provided"));
    }

    #[test]
    fn authorize_checkout_mints_a_proof_carrying_the_exact_scope_on_success() {
        let hooks = test_hooks().with_checkout_authorization(Box::new(|_spec, _scope| Ok(())));
        let proof = hooks
            .authorize_checkout(&ci_spec(), checkout_scope())
            .expect("configured hook returning Ok must mint a proof");
        assert_eq!(proof.scope(), &checkout_scope());
        assert_eq!(proof.run_token_jti(), "jti-1");
    }

    #[test]
    fn authorize_checkout_propagates_the_hook_error_and_mints_no_proof() {
        let hooks = test_hooks()
            .with_checkout_authorization(Box::new(|_spec, _scope| {
                Err(HookError("repo not authorized for this claim".to_string()))
            }));
        let err = hooks
            .authorize_checkout(&ci_spec(), checkout_scope())
            .unwrap_err();
        assert_eq!(err.0, "repo not authorized for this claim");
    }

    #[test]
    fn authorize_checkout_hands_the_hook_the_exact_scope_it_was_given() {
        let seen = Arc::new(std::sync::Mutex::new(None));
        let seen_in_hook = Arc::clone(&seen);
        let hooks = test_hooks().with_checkout_authorization(Box::new(move |_spec, scope| {
            *seen_in_hook.lock().unwrap() = Some(scope.clone());
            Ok(())
        }));
        hooks
            .authorize_checkout(&ci_spec(), checkout_scope())
            .expect("must succeed");
        assert_eq!(seen.lock().unwrap().as_ref(), Some(&checkout_scope()));
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

    /// CT-007 slice 5b.3-6e.1: the default `run_cycle` is a byte-faithful compatibility wrapper — it
    /// performs the ordinary streaming launch and maps its `Ok` into `WorkloadLaunched`, so EVERY
    /// existing backend/test double satisfies the seam unchanged.
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

    /// CT-007 slice 5b.3-6e.1: the `From<CheckoutContinuationOutcome>` bridge is total and
    /// variant-for-variant, so 6e.2's gVisor checkout `run_cycle` override can lift the orchestration's
    /// own result into the runner-cycle interface type without re-deriving the disposition vocabulary.
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
            SandboxCycleOutcome::PreparationRetryable { phase: PreparationPhase::CheckoutTransport, .. }
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
        // Guarantee #1: reserve refuses on exhaustion → launch fails fail-closed (never starts).
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
        // Guarantee #4: if the hardening profile cannot be applied/verified, launch fails closed
        // BEFORE any untrusted code runs (the seam CI-P5's escape drill drives).
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
