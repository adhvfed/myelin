//! # `runner_bind` — CT-004c.2: bind the `RunnerAgent` to the DURABLE `job_queue` store + gVisor exec
//!
//! **Owning architecture (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/00-overview.md` §4 (the
//! runner agent = a small attested binary that "pulls leases, launches the sandbox via
//! `SandboxBackend`, streams frames, reports terminal via the `job.done` signal") + `02-internals-and-
//! algorithms.md` §2.1 (pull-leasing — the `FOR UPDATE SKIP LOCKED` claim + heartbeat) + §5.1 (the
//! unified sandbox behind one trait; gVisor the named backend). **Reconciliation:** §OQ-F (a
//! `SCHEDULE_AND_RUN_JOB` dispatch retries on a failed launch; the runner delivers `job.done` at-least-
//! once, the engine wakes once).
//!
//! ## What CT-004c.2 ships — the exec BINDING (WIRE, do not reinvent)
//! CT-004c.1 landed the durable [`CiJobQueueStore`](crate::CiJobQueueStore) (claim / heartbeat /
//! complete against live PG, tier+region eligible, SKIP LOCKED) but leased a row and launched NOTHING.
//! The sandbox crate's [`RunnerAgent::run_one`](myelin_ci_sandbox::RunnerAgent::run_one) is the COMPLETE
//! security-conscious claim → heartbeat-confirm → `SandboxBackend::launch` (four-guarantee hooks fire
//! inside, fail-closed) → firehose (references-not-payloads) → whole-guest kill → derive `passed` →
//! `job.done` (exactly-once) → settle cycle. CT-004c.2 does not touch that body; it supplies TWO
//! real backings:
//!
//! 1. **[`DurableLeaseAdapter`]** — the durable `CiJobQueueStore` behind the sandbox's
//!    [`LeaseStore`](myelin_ci_sandbox::LeaseStore) port `run_one` claims through. THE SECURITY SEAM:
//!    [`DurableLeaseAdapter::claim_for_labels`] forwards the runner's `allowed_tiers` + `region`
//!    STRAIGHT to [`CiJobQueueStore::claim`] UNCHANGED (never widened/defaulted/dropped) — the tier
//!    predicate the durable store proved (an `untrusted_fork` job is never leased by a trusted-only
//!    runner) is the ONLY gate, never re-derived in the agent.
//! 2. **[`CiRunnerLoop`]** — the bounded runner loop the service `main` spawns (mirroring
//!    [`JobQueueReaper`](crate::JobQueueReaper)'s periodic-driver shape): repeatedly `run_one` with
//!    backoff on `NoWork`, a clean retry on `LeaseLost`, a LOUD log + backoff on `LaunchFailed` (the
//!    dispatch activity retries per §OQ-F). It wires a REAL [`GvisorBackend`] — untrusted code runs in
//!    a real `runsc` guest (the AG-D4 gate; the four sandbox guarantees are preserved, nothing is
//!    weakened).
//!
//! ## The async→sync bridge (the SAME convention as the MR-022 durable stores)
//! [`RunnerAgent::run_one`] is SYNC (it blocks for the in-line compute job). The durable
//! `CiJobQueueStore` verbs are ASYNC (sqlx). The adapter bridges via the established
//! `block_in_place` + `block_on` convention (`myelin_storage::kms_durable`, `PgOutboxBacking`): the
//! runner loop runs on a DEDICATED thread OFF the tokio runtime, so the adapter's DB calls
//! `Handle::block_on` directly (a `try_current` guard falls back to `block_in_place` if ever driven on
//! a multi-thread worker). No change to the sandbox seam, no new lease implementation.
//!
//! ## The JobSpec-provisioning seam (the CT-004d handoff, named honestly)
//! The durable `job_queue` row is SCHEDULING metadata only (job_id / run_id / lane / labels /
//! trust_tier / fair_key / idem_token) — it carries NO digest-pinned [`JobSpec`] (command / image /
//! limits). The adapter resolves a leased row to its `JobSpec` via an injected [`JobSpecResolver`].
//! The DURABLE spec store (specs keyed by job_id, written by the `SCHEDULE_AND_RUN_JOB` dispatch) is
//! **CT-004d** — the resolver is the seam it fills. CT-004c.2's integration test injects a real
//! resolver (a compute `JobSpec` that runs in a real `runsc` guest), proving the whole
//! claim→exec→`job.done`→settle path end to end.

use std::sync::Arc;
use std::time::Duration;

use myelin_ci_sandbox::asset_registry::{GvisorAssetRegistry, RootfsAssetBinding};
use myelin_ci_sandbox::gvisor::GvisorBackend;
use myelin_ci_sandbox::{
    derive_checkout_authorization_scope, resolved_gvisor_rootfs, resolved_gvisor_rust_rootfs,
    verified_gvisor_git_rootfs, ImageRef, JobKind, JobSpec, LeaseStore, QueuedJob, RunnerError,
    RunnerHooks, TrustTier, GVISOR_GIT_ROOTFS_SHA256, LINUX_RUST_V1_ROOTFS_SHA256,
    LINUX_SMALL_V1_ROOTFS_SHA256,
};
use myelin_storage::s3blob::S3BlobStore;
use myelin_tenancy::{Region, TenantId};

/// **The founder-dogfood pipeline's REAL `GvisorAssetRegistry` (CT-007 gate 2/4).** This is the
/// single most safety-critical construction in this slice: it is what turns `spec.image` into the
/// rootfs the REAL production runner loop ([`CiRunnerLoop::run_until_shutdown`]) actually launches
/// against. Every entry here reuses an EXISTING resolver ([`resolved_gvisor_rootfs`] /
/// [`resolved_gvisor_rust_rootfs`] / [`verified_gvisor_git_rootfs`]) rather than hardcoding a path,
/// so the corresponding operator env-var overrides remain the path-selection mechanism while the
/// registry pin remains the authority.
///
/// **Registered today:**
/// - `myelin.local/linux-small-v1-rootfs@sha256:<LINUX_SMALL_V1_ROOTFS_SHA256>` → the base rootfs —
///   the EXACT image `.myelin/ci.toml` already pins for the live founder-dogfood pipeline
///   (`scripts/dogfood.sh verify-ci-rootfs` independently asserts this same digest against the same
///   directory by shelling out; this registry entry asserts it again, in-process, at every launch).
///   Before this change, production resolved the rootfs SOLELY from `MYELIN_GVISOR_ROOTFS`,
///   completely ignoring whatever image a job declared; this entry is what makes that declaration
///   the real authority going forward, for the SAME asset that already runs today — nothing about
///   which bytes execute changes, only that `spec.image` is now checked against them.
/// - `myelin.local/linux-rust-v1-rootfs@sha256:<LINUX_RUST_V1_ROOTFS_SHA256>` → the Rust-capable
///   rootfs (`runner-assets.toml`'s `linux-rust-v1` row) — registered for HONESTY (every ordinary,
///   non-git-wire job rootfs this runner currently launches is covered) even though no dispatched
///   job names it yet.
/// - `myelin.local/git-v1-rootfs@sha256:<GVISOR_GIT_ROOTFS_SHA256>` → the git-bearing checkout and
///   git-wire rootfs. Its resolver also validates the complete `/tmp`, `/workspace`, `/repo`, and
///   `/quarantine` destination set before registry construction hashes it.
///
/// **Composition-root placement, not `myelin-ci-sandbox`:** this function lives here (the CI
/// control-plane's composition root), not in `myelin-ci-sandbox`, because the SPECIFIC bindings
/// (which images exist, which asset backs the founder pipeline) are a deployment/composition fact —
/// `.myelin/ci.toml` and `runner-assets.toml` are both repo-root config files this crate's binary is
/// closer to than the generic sandbox-seam crate is. `myelin-ci-sandbox` supplies the MECHANISM
/// (`GvisorAssetRegistry`, the resolvers, the digest constants); this crate supplies the ONE real
/// binding set.
///
/// **Verifies for real, at construction, right here** ([`GvisorAssetRegistry::from_bindings`]) — this
/// is genuine hashing WORK over the staged asset directories (the >800MiB Rust asset takes several
/// real seconds on this host), done ONCE at runner startup, never per job launch. A runner that
/// cannot verify its own configured assets refuses to start (`.expect` panics LOUDLY here) rather
/// than silently limping along and discovering the problem mid-launch.
pub fn production_gvisor_registry() -> Arc<GvisorAssetRegistry> {
    Arc::new(
        GvisorAssetRegistry::from_bindings(vec![
            RootfsAssetBinding {
                image: ImageRef::pinned(format!(
                    "myelin.local/linux-small-v1-rootfs@sha256:{LINUX_SMALL_V1_ROOTFS_SHA256}"
                ))
                .expect("the founder-pipeline image reference is a well-formed digest pin"),
                rootfs: resolved_gvisor_rootfs(),
            },
            RootfsAssetBinding {
                image: ImageRef::pinned(format!(
                    "myelin.local/linux-rust-v1-rootfs@sha256:{LINUX_RUST_V1_ROOTFS_SHA256}"
                ))
                .expect("the linux-rust-v1 image reference is a well-formed digest pin"),
                rootfs: resolved_gvisor_rust_rootfs(),
            },
            RootfsAssetBinding {
                image: ImageRef::pinned(format!(
                    "myelin.local/git-v1-rootfs@sha256:{GVISOR_GIT_ROOTFS_SHA256}"
                ))
                .expect("the git rootfs image reference is a well-formed digest pin"),
                // This resolver verifies all four fixed OCI destinations and the same canonical
                // digest first; `from_bindings` then independently verifies the registry pin.
                rootfs: verified_gvisor_git_rootfs().expect(
                    "the git rootfs and every fixed OCI mountpoint must verify before runner startup",
                ),
            },
        ])
        .expect(
            "production runner assets must verify at startup — a runner that cannot prove its own \
             configured rootfs assets must refuse to start rather than launch jobs it cannot verify",
        ),
    )
}

use crate::ci_claim_token_issuer::LockedManifestCiJobTokenIssuer;
use crate::ci_identity_adapter::ci_job_authorization_context;
use crate::ci_manifest_job_runner::{
    resolve_claim_launch_secrets, secret_withhold_machine_reason, validate_run_token,
    CiJobSecretResolver, CiJobTokenIssuer, CiJobTokenRequest,
};
use crate::ci_pipeline_reporter_router::CiPipelineReporterRouter;
use crate::job_spec_store::MAX_JOB_TIMEOUT_SECS;
use crate::{
    CiJobQueueStore, CiJobSpecStore, CiRegionQueueStore, DurableLogPersist, LeasedJob,
    LogPipelineSink,
};

/// **The runner's EXECUTION lease TTL, wired ABOVE the max job timeout (CT-004d.1 — the CT-004c.2
/// verifier's MEDIUM fix).** [`RunnerAgent::run_one`](myelin_ci_sandbox::RunnerAgent) BLOCKS for the
/// whole in-line job and heartbeats only BEFORE + AFTER the blocking launch (never mid-launch, absent
/// a structural change to the sandbox launch). So if the lease TTL were below the job's wall-clock,
/// the lease would lapse mid-run → the reaper re-queues it → a second runner double-executes. Setting
/// the TTL strictly above [`MAX_JOB_TIMEOUT_SECS`] (which the spec store enforces as the per-job
/// ceiling) makes that impossible: a leased job provably finishes before its lease can lapse. The
/// tighter fix — a heartbeat thread DURING the blocking launch, which would let this shrink back
/// toward the reaper cadence — is the named CT-004d follow-on (it needs a structural hook in the
/// sandbox launch, out of scope for CT-004d.1, which must not touch `run_one`'s security body). The
/// `+ 600` is the margin over the ceiling (reaper-cadence + clock-skew headroom).
///
/// **This bounds ONE execution, not a whole claim generation** (CT-007 lease/topology
/// reconciliation): a checkout-bearing parent attempt legally contains four sequential executions,
/// and its hard per-generation ceiling is the immutable
/// [`claim_window_secs`](crate::ci_claim_window::claim_window_secs) instead. The two were the same
/// constant before that slice, which is why this one was renamed outright rather than aliased — a
/// time-authority constant whose meaning narrowed must not keep its old, now-ambiguous name.
pub const CI_RUNNER_EXECUTION_LEASE_TTL_SECS: i64 = MAX_JOB_TIMEOUT_SECS as i64 + 600;

/// **Resolve a leased `job_queue` row to its digest-pinned [`JobSpec`] (the CT-004d spec-store seam).**
/// The durable queue holds scheduling metadata, not the spec; this maps a claimed [`LeasedJob`] to the
/// `JobSpec` the sandbox launches. An `Err` (spec absent / not yet provisioned) makes the claim a
/// no-op (`None`) — the row stays leased and the dead-runner reaper re-queues it (safe, never a launch
/// of an unresolved job). In production the impl reads the durable spec store the dispatch writes
/// (CT-004d); the CT-004c.2 test injects a real compute spec.
pub type JobSpecResolver = Arc<dyn Fn(&LeasedJob) -> Result<JobSpec, String> + Send + Sync>;

trait SecretWithholdTerminalizer {
    fn terminalize(
        &self,
        claim: &CiJobTokenRequest,
        diagnostic: &str,
    ) -> Result<(), String>;
}

impl SecretWithholdTerminalizer for CiPipelineReporterRouter {
    fn terminalize(
        &self,
        claim: &CiJobTokenRequest,
        diagnostic: &str,
    ) -> Result<(), String> {
        use myelin_ci_sandbox::{
            PreparationPhase, PreparationReportClaim, PreparationTerminalDisposition,
            TerminalReporter,
        };

        let report_claim = PreparationReportClaim {
            tenant_id: claim.tenant_id.clone(),
            region: claim.region.clone(),
            project_id: claim.project_id.clone(),
            wf_run_id: claim.wf_run_id.clone(),
            ci_run_id: claim.ci_run_id.clone(),
            job_id: claim.job_id.clone(),
            token_authority_handle: claim.token_authority_handle.clone(),
            idem_token: claim.idem_token.clone(),
            lease_owner: claim.lease_owner.clone(),
            lease_epoch: claim.lease_epoch,
            claim_nonce: claim.claim_nonce.clone(),
            claim_started_at_epoch_secs: claim.claim_started_at_epoch_secs,
            claim_expires_at_epoch_secs: claim.claim_expires_at_epoch_secs,
        };
        self.report_preparation_terminal(
            &report_claim,
            PreparationTerminalDisposition::Failed {
                phase: PreparationPhase::SecretResolution,
            },
            Some(diagnostic),
        )
        .map(|_| ())
        .map_err(|error| format!("terminal secret-withhold settlement failed: {error}"))
    }
}

fn finish_claim_secret_resolution(
    resolution: Result<JobSpec, crate::SecretLaunchError>,
    claim: &CiJobTokenRequest,
    terminalizer: &impl SecretWithholdTerminalizer,
) -> Result<JobSpec, String> {
    match resolution {
        Ok(spec) => Ok(spec),
        Err(crate::SecretLaunchError::Withheld(withheld)) => {
            let diagnostic = secret_withhold_machine_reason(&withheld);
            terminalizer.terminalize(claim, &diagnostic)?;
            Err(format!(
                "secret-bearing claim settled terminally: {diagnostic}"
            ))
        }
        Err(error) => Err(error.to_string()),
    }
}

fn finish_v1_claim_secret_resolution(
    resolution: Result<JobSpec, crate::SecretLaunchError>,
    claim: &CiJobTokenRequest,
    terminalizer: &impl SecretWithholdTerminalizer,
) -> Result<JobSpec, String> {
    finish_claim_secret_resolution(resolution, claim, terminalizer)
}

/// Bridge one async durable-store call to the sync [`LeaseStore`] port. The runner loop drives this
/// from a dedicated OFF-runtime thread, so `block_on` runs directly; the `try_current` guard falls
/// back to `block_in_place` only if ever driven on a multi-thread runtime worker (the SAME convention
/// as `myelin_storage::kms_durable`). NEVER call this from a current-thread runtime (drive the runner
/// on its own thread — `CiRunnerLoop::spawn`).
///
/// **CT-007 5b.3-6d STEP 3:** this is the ONE shared off-runtime bridge;
/// [`ci_checkout_composition`](crate::ci_checkout_composition)'s dormant `DurableAttemptAuthority` and
/// parent-attempt reserve hook reuse it (both are driven from this same `CiRunnerLoop::spawn` dedicated
/// off-runtime thread), rather than forking a second, subtly-different copy.
pub(crate) fn bridge<F: std::future::Future>(rt: &tokio::runtime::Handle, fut: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(_) => tokio::task::block_in_place(|| rt.block_on(fut)),
        Err(_) => rt.block_on(fut),
    }
}

/// **The split durable queue capabilities adapted to the sandbox [`LeaseStore`] port (CT-004c.2).**
/// [`CiRegionQueueStore`] claims through the dedicated scheduler role; [`CiJobQueueStore`] performs
/// tenant-scoped heartbeat/complete through the application role. Keeping both handles explicit
/// prevents either credential from acquiring the other's mutation surface.
///
/// **The security invariant (the adversarial-verifier surface):** [`claim_for_labels`](Self::claim_for_labels)
/// forwards `allowed_tiers` + `region` to [`CiRegionQueueStore::claim`] EXACTLY as received — no widening,
/// no default, no drop — so the durable `trust_tier = ANY($tiers) AND region = $region` predicate is
/// the sole eligibility gate. This type carries a durable store handle (no in-memory collection of
/// record), so it is NOT an in-memory durable store.
pub struct DurableLeaseAdapter {
    region_store: CiRegionQueueStore,
    tenant_store: CiJobQueueStore,
    region: String,
    rt: tokio::runtime::Handle,
    resolve: JobSpecResolver,
}

impl DurableLeaseAdapter {
    /// Adapt the separate scheduler/tenant queue stores to the runner's lease port.
    pub fn new(
        region_store: CiRegionQueueStore,
        tenant_store: CiJobQueueStore,
        region: impl Into<String>,
        rt: tokio::runtime::Handle,
        resolve: JobSpecResolver,
    ) -> DurableLeaseAdapter {
        DurableLeaseAdapter {
            region_store,
            tenant_store,
            region: region.into(),
            rt,
            resolve,
        }
    }
}

impl LeaseStore for DurableLeaseAdapter {
    /// **Claim through the durable `FOR UPDATE SKIP LOCKED` predicate (arch 02 §2.1).** Forwards
    /// `region` + `runner_labels` + `allowed_tiers` UNCHANGED to [`CiRegionQueueStore::claim`]. On a
    /// leased row, resolves the [`JobSpec`] and builds the sandbox [`QueuedJob`]. `None` when nothing
    /// is eligible OR the spec is unresolved (the row stays leased; the reaper re-queues it — never a
    /// launch of an unresolved/ineligible job). A DB error is LOUD (never a silent drop) and yields
    /// `None` (fail-closed: no launch on a claim that did not clearly succeed).
    fn claim_for_labels(
        &self,
        worker: &str,
        runner_labels: &[String],
        allowed_tiers: &[TrustTier],
        region: &Region,
        now: i64,
        lease_ttl_secs: i64,
    ) -> Option<QueuedJob> {
        let ttl = lease_ttl_secs.max(0) as u64;
        // THE SECURITY PASS-THROUGH: region + labels + allowed_tiers go to the durable claim verbatim.
        // The eligibility gate (`trust_tier = ANY($tiers) AND region = $region`) is the store's.
        let claimed = bridge(
            &self.rt,
            self.region_store
                .claim(&region.0, runner_labels, allowed_tiers, worker, ttl),
        );
        let leased = match claimed {
            Ok(Some(l)) => l,
            Ok(None) => return None,
            Err(e) => {
                eprintln!(
                    "ci-runner[{worker}]: durable claim FAILED in region `{}` (no launch; will \
                     retry): {e}",
                    region.0
                );
                return None;
            }
        };
        // Resolve the leased row to its digest-pinned spec (the CT-004d spec-store seam). An
        // unresolved spec is a no-op claim — the row stays leased and the reaper re-queues it.
        let spec = match (self.resolve)(&leased) {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "ci-runner[{worker}]: leased job {} has no resolvable JobSpec (CT-004d spec \
                     store); leaving it leased for the reaper: {e}",
                    leased.job_id
                );
                return None;
            }
        };
        Some(QueuedJob {
            tenant: TenantId(leased.tenant_id.clone()),
            region: region.clone(),
            run_id: leased.run_id.to_string(),
            job_id: leased.job_id.to_string(),
            labels: runner_labels.to_vec(),
            spec,
            lease_owner: Some(worker.to_string()),
            lease_expires: Some(now + lease_ttl_secs),
            // The claim generation the durable CLAIM bumped — carried to the completion CAS so a stale
            // reaped worker (lower epoch) is refused.
            lease_epoch: leased.lease_epoch,
            claim_nonce: leased.claim_nonce,
        })
    }

    /// **Heartbeat-renew through the durable owner-guarded UPDATE (arch 02 §2.1).** `false` if the
    /// caller is not the current lease owner — which is EXACTLY the lost-lease detection
    /// [`RunnerAgent::run_one`](myelin_ci_sandbox::RunnerAgent) Step-2 stops launching on (another
    /// worker reclaimed a stolen lease → this runner must not double-run). A DB error is LOUD and
    /// returns `false` (fail-closed: treat as lost, do not run untrusted code on an unconfirmed lease).
    fn heartbeat(
        &self,
        worker: &str,
        tenant: &TenantId,
        job_id: &str,
        _now: i64,
        lease_ttl_secs: i64,
    ) -> bool {
        let ttl = lease_ttl_secs.max(0) as u64;
        match bridge(
            &self.rt,
            self.tenant_store
                .heartbeat(&tenant.0, &self.region, job_id, worker, ttl),
        ) {
            Ok(extended) => extended,
            Err(e) => {
                eprintln!(
                    "ci-runner[{worker}]: heartbeat FAILED for job {job_id} (treating as \
                     lease-lost, fail-closed): {e}"
                );
                false
            }
        }
    }

    /// **Settle on terminal — complete the durable row (arch 02 §3.2).** Best-effort: the engine
    /// signal idempotency (not this complete) is what makes the `job.done` wake exactly-once, so a
    /// complete after a redelivered report is a harmless no-op. A DB error is LOUD (never a silent
    /// drop); the reaper will not re-queue a heart-beat-less row past its lease, and a re-delivered
    /// `job.done` still wakes once.
    fn settle(&self, tenant: &TenantId, job_id: &str) {
        if let Err(e) = bridge(
            &self.rt,
            self.tenant_store.complete(&tenant.0, &self.region, job_id),
        ) {
            eprintln!(
                "ci-runner: settle/complete FAILED for job {job_id} in region `{}` (the job.done \
                 idempotency still holds; the reaper reconciles): {e}",
                self.region
            );
        }
    }
}

/// **The bounded CI runner loop (CT-004c.2) — the service `main` spawns it (arch 00 §4).** Owns the
/// durable lease adapter's inputs, a REAL [`GvisorBackend`] (untrusted code runs in a `runsc` guest —
/// the AG-D4 gate), the firehose stub, the PostgreSQL-only `job.done` reporter,
/// and the four-guarantee hooks. [`run_until_shutdown`](Self::run_until_shutdown) constructs the
/// [`RunnerAgent`] on its own thread and loops `run_one` with shutdown-aware backoff. Mirrors
/// [`JobQueueReaper`](crate::JobQueueReaper): a bounded background driver, no new `AppSpec` field,
/// LOUD on failure and resilient (a launch failure logs and continues; the §OQ-F dispatch retry
/// re-runs the job).
///
/// The reporter re-encodes the runner-derived verdict and atomically buffers it in PostgreSQL. A
/// tenant/region/partition-scoped [`myelin_flow::PgFlowWorker`] wakes and consumes that one durable
/// signal path; no process-local executor mirror participates in a production build.
pub struct CiRunnerLoop {
    worker_id: String,
    labels: Vec<String>,
    allowed_tiers: Vec<TrustTier>,
    region: String,
    lease_ttl_secs: i64,
    region_store: CiRegionQueueStore,
    tenant_store: CiJobQueueStore,
    rt: tokio::runtime::Handle,
    resolve: JobSpecResolver,
    reporter: CiPipelineReporterRouter,
    hooks: RunnerHooks,
    idle_backoff: Duration,
    error_backoff: Duration,
    // CT-004f sub-step 5: the durable log path the live runner seals captured job output through. The
    // pool + S3 config build the `LogPipelineSink` (per-(tenant,run,job) LogPipeline over the real
    // S3 CAS) + `DurableLogPersist` (the log_segment/log_anchor writer + ci.log.available outbox emit)
    // INSIDE the run driver on the dedicated runner thread (the LogPipeline is non-Send).
    pool: sqlx::postgres::PgPool,
    s3: myelin_config::S3Config,
    /// CT-007 slice 4: the OWNED, already-preflighted workspace-activation level (`main`'s
    /// `prepare_runner_host` parsed and preflighted this exact value before PostgreSQL bootstrap —
    /// see that function's own doc). A MANDATORY constructor parameter, not an optional builder
    /// (Sol's design review): this is security-sensitive composition state, so omitting it must be
    /// a compile error, not a silent default.
    gvisor_workspace_config: myelin_ci_sandbox::gvisor::GvisorWorkspaceConfig,
    /// The independently boot-validated Hop-A bare-repository root. Stage B requires Enabled; the
    /// opaque type makes an unchecked path unrepresentable here.
    gvisor_checkout_config: myelin_ci_sandbox::gvisor::GvisorCheckoutConfig,
}

/// Terminal reason returned by the owned runner thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiRunnerLoopExit {
    /// The lifecycle signal arrived before another claim. An in-flight sandbox, if any, completed
    /// under its durable job timeout before this result was returned.
    Shutdown,
    /// Reporter and hook settlement ownership disagreed. Retrying cannot repair this static
    /// composition error, so the complete runner host must stop fail-closed.
    SettlementOwnerMismatch,
    /// A launched job could not co-commit its terminal report/accounting. The durable claim remains
    /// recoverable, but accepting more work would accumulate completed, unsettled jobs.
    TerminalReportFailed,
    /// CT-007 slice 4: `GvisorBackend::try_new` refused the preflighted `gvisor_workspace_config`
    /// (e.g. the `WorkspaceManager`/`UserNamespaceAllocator` construction that only happens HERE,
    /// on this dedicated thread, found the host no longer matches what `main`'s preflight observed).
    /// This is an expected, diagnosable operational failure (Sol's design review) — never a panic —
    /// surfaced before a single claim is ever attempted.
    SandboxBackendInitializationFailed,
}

impl CiRunnerLoop {
    /// Construct the runner loop. `worker_id`/`labels`/`allowed_tiers`/`region`/`lease_ttl_secs` are
    /// the claim predicates + lease TTL; `store` is the durable `job_queue` store; `rt` bridges the
    /// sync runner onto the async pool; `resolve` provides the `JobSpec` for a leased row (CT-004d
    /// seam); `reporter` is the region router that constructs an exact-tenant, PostgreSQL-only
    /// [`crate::CiPipelineReporter`] which atomically consumes the
    /// claim and buffers the typed verdict; `hooks` are the four-guarantee wiring. `pool` +
    /// `s3` back the live log sink (CT-004f sub-step 5): the OLTP pool writes the `log_segment`/
    /// `log_anchor` index + the `ci.log.available` outbox pointer; `s3` is the CAS the sealed log
    /// segments flush to (the same shared pool + object store the rest of the control plane uses).
    /// `gvisor_workspace_config` (CT-007 slice 4) is the OWNED, already-preflighted workspace-
    /// activation level `main`'s `prepare_runner_host` produced before PostgreSQL bootstrap — a
    /// MANDATORY parameter (never an optional builder default) since this is security-sensitive
    /// sandbox composition state.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        worker_id: impl Into<String>,
        labels: Vec<String>,
        allowed_tiers: Vec<TrustTier>,
        region: impl Into<String>,
        lease_ttl_secs: i64,
        region_store: CiRegionQueueStore,
        tenant_store: CiJobQueueStore,
        rt: tokio::runtime::Handle,
        resolve: JobSpecResolver,
        reporter: CiPipelineReporterRouter,
        hooks: RunnerHooks,
        pool: sqlx::postgres::PgPool,
        s3: myelin_config::S3Config,
        gvisor_workspace_config: myelin_ci_sandbox::gvisor::GvisorWorkspaceConfig,
        gvisor_checkout_config: myelin_ci_sandbox::gvisor::GvisorCheckoutConfig,
    ) -> CiRunnerLoop {
        CiRunnerLoop {
            worker_id: worker_id.into(),
            labels,
            allowed_tiers,
            region: region.into(),
            lease_ttl_secs,
            region_store,
            tenant_store,
            rt,
            resolve,
            reporter,
            hooks,
            pool,
            s3,
            gvisor_workspace_config,
            gvisor_checkout_config,
            idle_backoff: Duration::from_millis(500),
            error_backoff: Duration::from_secs(2),
        }
    }

    /// Override the idle (`NoWork`) + error (`LaunchFailed`/`ReportFailed`) backoffs (defaults 500 ms /
    /// 2 s). A test drives a single `run_one` directly; the loop's backoff is for the long-poll cadence.
    pub fn with_backoff(mut self, idle: Duration, error: Duration) -> CiRunnerLoop {
        self.idle_backoff = idle;
        self.error_backoff = error;
        self
    }

    /// **Spawn the loop on a DEDICATED OS thread (off the tokio runtime).** The runner blocks for the
    /// whole in-line `runsc` job and the adapter bridges its DB calls onto `rt`; running off-runtime
    /// keeps `block_on` correct and never starves a tokio worker. This legacy forever-loop entry is
    /// retained for deterministic callers; production uses [`Self::spawn_until_shutdown`].
    pub fn spawn(self) -> std::thread::JoinHandle<CiRunnerLoopExit> {
        std::thread::Builder::new()
            .name("ci-runner".into())
            .spawn(move || self.run())
            .expect("spawn the ci-runner thread")
    }

    /// Spawn the production loop on a dedicated thread with an explicit lifecycle signal. A
    /// shutdown received during a sandbox launch lets that in-flight job finish, then prevents the
    /// next claim. Idle/error backoff observes shutdown within
    /// [`RUNNER_SHUTDOWN_POLL_INTERVAL`].
    pub fn spawn_until_shutdown(
        self,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> std::thread::JoinHandle<CiRunnerLoopExit> {
        self.try_spawn_until_shutdown(shutdown)
            .expect("spawn the shutdown-aware ci-runner thread")
    }

    /// Fallible production spawn used by the coordinated runner host. Thread-resource exhaustion is
    /// a typed startup refusal rather than a panic after other host lanes have started.
    pub(crate) fn try_spawn_until_shutdown(
        self,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> std::io::Result<std::thread::JoinHandle<CiRunnerLoopExit>> {
        std::thread::Builder::new()
            .name("ci-runner".into())
            .spawn(move || self.run_until_shutdown(shutdown))
    }

    /// **Run the claim → launch → `job.done` cycle forever (until the process stops).** Constructs the
    /// [`RunnerAgent`] over the gVisor backend + the durable lease adapter, then loops: `NoWork` sleeps
    /// `idle_backoff`; `LeaseLost` is a clean immediate retry (another worker owns it now); a
    /// `LaunchFailed`/`ReportFailed` is logged LOUD and sleeps `error_backoff` (the dispatch activity
    /// retries the job, §OQ-F). Never reimplements any sandbox logic.
    pub fn run(self) -> CiRunnerLoopExit {
        let (_shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        self.run_until_shutdown(shutdown_rx)
    }

    /// Run claim → launch → `job.done` cycles until explicit shutdown or sender closure. Shutdown is
    /// checked before constructing live adapters and before every claim. It never kills a job midway:
    /// an already-running sandbox remains bounded by its job timeout and drains before this loop
    /// returns.
    pub fn run_until_shutdown(
        self,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> CiRunnerLoopExit {
        if runner_shutdown_requested(&mut shutdown) {
            return CiRunnerLoopExit::Shutdown;
        }
        let CiRunnerLoop {
            worker_id,
            labels,
            allowed_tiers,
            region,
            lease_ttl_secs,
            region_store,
            tenant_store,
            rt,
            resolve,
            reporter,
            hooks,
            pool,
            s3,
            gvisor_workspace_config,
            gvisor_checkout_config,
            idle_backoff,
            error_backoff,
        } = self;

        // CT-007 slice 4: the ONE real construction of the security-load-bearing managers
        // (`WorkspaceManager`/`UserNamespaceAllocator`) `GvisorWorkspaceConfig::Enabled` names --
        // `main`'s `prepare_runner_host` already preflighted this exact configuration before
        // PostgreSQL bootstrap, but construction itself only happens HERE, on this dedicated
        // thread (mirroring `production_gvisor_registry()`'s own "verify once, at runner startup"
        // shape). A failure here is an expected, diagnosable operational failure (Sol's design
        // review) -- never a panic -- surfaced as a typed loop exit before a single claim is ever
        // attempted, so `CiRunnerHost` can trigger its own coordinated shutdown exactly as it does
        // for every other lane failure.
        let incident_sink: myelin_ci_sandbox::workspace_manager::IncidentSink = {
            let worker_id = worker_id.clone();
            Arc::new(move |message: &str| {
                eprintln!("ci-runner[{worker_id}]: GVISOR SECURITY INCIDENT: {message}");
            })
        };
        let backend = match GvisorBackend::try_new(
            production_gvisor_registry(),
            gvisor_workspace_config,
            incident_sink,
        ) {
            Ok(backend) => backend.with_checkout_config(gvisor_checkout_config),
            Err(error) => {
                eprintln!(
                    "ci-runner[{worker_id}]: sandbox backend initialization FAILED (fail-closed, \
                     no claim attempted): {error}"
                );
                return CiRunnerLoopExit::SandboxBackendInitializationFailed;
            }
        };
        // CT-004f sub-step 5: the LIVE log sink (was the `CountingFirehose` stub). Built HERE on the
        // dedicated runner thread because the per-job `LogPipeline` is non-Send (its firehose uses Rc);
        // the pool + rt handle are cheap Clones. Captured guest stdout/stderr → redacted at the sandbox
        // boundary (the `RedactionPlan` seam, empty today) → sealed to the real S3 CAS → `log_segment`/
        // `log_anchor` index + the `ci.log.available` outbox pointer, all tenant-scoped (FORCE-RLS).
        let firehose = LogPipelineSink::new(
            Region(region.clone()),
            S3BlobStore::connect(&s3, rt.clone()),
            DurableLogPersist::with_pg(pool, rt.clone()),
        );
        let adapter =
            DurableLeaseAdapter::new(region_store, tenant_store, region.clone(), rt, resolve);
        let agent = myelin_ci_sandbox::RunnerAgent::new(
            worker_id.clone(),
            labels,
            allowed_tiers,
            Region(region.clone()),
            lease_ttl_secs,
            adapter,
            &backend,
            &firehose,
            &reporter,
            hooks,
        );

        eprintln!(
            "ci-runner[{worker_id}]: started (region `{region}`, lease TTL {lease_ttl_secs}s) — \
             claiming from the durable job_queue + executing in gVisor (AG-D4)"
        );
        loop {
            if runner_shutdown_requested(&mut shutdown) {
                return CiRunnerLoopExit::Shutdown;
            }
            match agent.run_one_cycle(now_secs()) {
                Ok(myelin_ci_sandbox::RunnerCycleOutcome::Workload(o)) => {
                    eprintln!(
                        "ci-runner[{worker_id}]: ran job {} for run {} (passed={}, job.done={:?})",
                        o.job_id, o.run_id, o.report.passed, o.signal_outcome
                    );
                }
                Ok(myelin_ci_sandbox::RunnerCycleOutcome::PreparationTerminal {
                    job_id,
                    run_id,
                    signal_outcome,
                    diagnostic,
                }) => match diagnostic {
                    Some(diagnostic) => eprintln!(
                        "ci-runner[{worker_id}]: preparation terminalized job {job_id} for run \
                             {run_id} (job.done={signal_outcome:?}, diagnostic={diagnostic})"
                    ),
                    None => eprintln!(
                        "ci-runner[{worker_id}]: preparation terminalized job {job_id} for run \
                             {run_id} (job.done={signal_outcome:?})"
                    ),
                },
                Ok(myelin_ci_sandbox::RunnerCycleOutcome::PreparationRetryable {
                    job_id,
                    report,
                }) => {
                    eprintln!(
                        "ci-runner[{worker_id}]: preparation requeued job {job_id} ({report:?})"
                    );
                }
                Err(RunnerError::NoWork) => {
                    if runner_sleep_until_shutdown(&mut shutdown, idle_backoff) {
                        return CiRunnerLoopExit::Shutdown;
                    }
                }
                Err(RunnerError::LeaseLost { job_id }) => {
                    // Another worker reclaimed the lease (the reaper re-queued it) — a clean retry;
                    // this runner did NOT run the job (the Step-2 double-run guard, fail-closed).
                    eprintln!(
                        "ci-runner[{worker_id}]: lease LOST for job {job_id} mid-claim — retrying \
                         (no double-run)"
                    );
                }
                Err(e @ RunnerError::LaunchFailed(_)) => {
                    // Fail LOUD, keep looping — the §OQ-F dispatch activity retries the job.
                    eprintln!(
                        "ci-runner[{worker_id}]: launch FAILED (fail-closed, no terminal report; \
                         the dispatch retries): {e}"
                    );
                    if runner_sleep_until_shutdown(&mut shutdown, error_backoff) {
                        return CiRunnerLoopExit::Shutdown;
                    }
                }
                Err(e @ RunnerError::RetryableAttemptRecorded { .. }) => {
                    // The claim-aware reporter already accrued measured usage and returned the
                    // exact generation to `queued` without emitting job.done. Back off before
                    // reclaiming so a persistent log outage cannot hot-loop paid execution.
                    eprintln!("ci-runner[{worker_id}]: {e}");
                    if runner_sleep_until_shutdown(&mut shutdown, error_backoff) {
                        return CiRunnerLoopExit::Shutdown;
                    }
                }
                Err(e @ RunnerError::ReportFailed(_)) => {
                    eprintln!(
                        "ci-runner[{worker_id}]: terminal report FAILED; stopping host intake so the \
                         durable claim can be recovered: {e}"
                    );
                    return CiRunnerLoopExit::TerminalReportFailed;
                }
                Err(e @ RunnerError::SettlementOwnerMismatch { .. }) => {
                    // Static composition failure: retrying cannot make an unowned/double settlement
                    // safe. Stop this runner lane fail-closed before it claims any work.
                    eprintln!("ci-runner[{worker_id}]: CONFIGURATION REFUSED: {e}");
                    return CiRunnerLoopExit::SettlementOwnerMismatch;
                }
                Err(
                    e @ (RunnerError::PreparationRoutingFailed { .. }
                    | RunnerError::ReconciliationRequired { .. }),
                ) => {
                    eprintln!(
                        "ci-runner[{worker_id}]: checkout recovery REQUIRED; stopping host intake: {e}"
                    );
                    return CiRunnerLoopExit::TerminalReportFailed;
                }
            }
        }
    }
}

/// Maximum time an idle runner can take to observe shutdown. Active jobs drain under their own
/// timeout instead of being killed midway.
pub const RUNNER_SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(50);

fn runner_shutdown_requested(shutdown: &mut tokio::sync::watch::Receiver<bool>) -> bool {
    if *shutdown.borrow_and_update() {
        return true;
    }
    match shutdown.has_changed() {
        Ok(true) => *shutdown.borrow_and_update(),
        Ok(false) => false,
        Err(_) => true,
    }
}

fn runner_sleep_until_shutdown(
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
    duration: Duration,
) -> bool {
    let started = std::time::Instant::now();
    loop {
        if runner_shutdown_requested(shutdown) {
            return true;
        }
        let remaining = duration.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return false;
        }
        std::thread::sleep(remaining.min(RUNNER_SHUTDOWN_POLL_INTERVAL));
    }
}

/// The runner's clock (epoch seconds) — the `now` the claim/heartbeat lease arithmetic uses.
fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Historical fail-closed resolver retained for callers that deliberately have no durable spec
/// authority. It returns `Err` for every leased row, leaving the claim for the reaper rather than
/// inventing a launch. Production uses [`durable_spec_resolver`].
pub fn spec_store_unavailable_resolver() -> JobSpecResolver {
    Arc::new(|leased: &LeasedJob| {
        Err(format!(
            "no durable JobSpec store yet (CT-004d) for job {}; runner cannot launch an unresolved \
             job — leaving it leased for the reaper",
            leased.job_id
        ))
    })
}

/// **The REAL production spec-resolver over the durable `ci_job_spec` store (CT-004d.1).** Replaces
/// [`spec_store_unavailable_resolver`]: resolves a leased `job_queue` row to the exact durable launch
/// template, mints under the live claim generation, and only then constructs the [`JobSpec`] the
/// sandbox may execute. The template is read via [`CiJobSpecStore::get_launch_template`] keyed on
/// `(leased.tenant_id, leased.job_id)`.
/// `region` is the runner's residency region (the tenant-scoped-tx scope the read runs under); `rt` is
/// the runtime handle the sync resolver bridges its async DB read onto (the SAME off-runtime
/// `block_on` convention the lease adapter uses — the resolver is called INSIDE
/// [`DurableLeaseAdapter::claim_for_labels`], already on the runner's dedicated thread).
///
/// **Fail-closed by construction:** a missing spec ([`CiJobSpecStoreError::SpecNotFound`]) or a corrupt
/// one ([`CiJobSpecStoreError::CorruptSpec`]) returns `Err` — the claim becomes a no-op (`None`), the
/// row stays leased, and the reaper recovers it. The runner NEVER launches a fabricated/default spec;
/// the stored spec is the only thing that executes. A typed secret withhold is different: like the V2
/// resolver, V1 reports `SecretResolution` failure through `secret_terminal_reporter`, whose
/// exact-generation CAS atomically terminalizes the queue row, emits failed `job.done`, settles
/// accounting, and persists only the material-free secret name/reason diagnostic.
pub fn durable_spec_resolver(
    store: CiJobSpecStore,
    region: impl Into<String>,
    rt: tokio::runtime::Handle,
    token_issuer: LockedManifestCiJobTokenIssuer,
    secrets: CiJobSecretResolver,
    secret_terminal_reporter: CiPipelineReporterRouter,
) -> JobSpecResolver {
    durable_spec_resolver_with_issuer(
        store,
        region,
        rt,
        Arc::new(token_issuer),
        secrets,
        secret_terminal_reporter,
    )
}

/// Legacy V1 fixture seam under the crate's established dev-only `test-support` boundary. The
/// default production dependency graph cannot name it or pass an issuer that bypasses the locked
/// manifest verifier to [`durable_spec_resolver`].
#[cfg(any(test, feature = "test-support"))]
pub fn durable_spec_resolver_test_support(
    store: CiJobSpecStore,
    region: impl Into<String>,
    rt: tokio::runtime::Handle,
    token_issuer: Arc<dyn CiJobTokenIssuer>,
    secrets: CiJobSecretResolver,
    secret_terminal_reporter: CiPipelineReporterRouter,
) -> JobSpecResolver {
    durable_spec_resolver_with_issuer(
        store,
        region,
        rt,
        token_issuer,
        secrets,
        secret_terminal_reporter,
    )
}

fn durable_spec_resolver_with_issuer(
    store: CiJobSpecStore,
    region: impl Into<String>,
    rt: tokio::runtime::Handle,
    token_issuer: Arc<dyn CiJobTokenIssuer>,
    secrets: CiJobSecretResolver,
    secret_terminal_reporter: CiPipelineReporterRouter,
) -> JobSpecResolver {
    let region = region.into();
    Arc::new(move |leased: &LeasedJob| {
        let launch = bridge(
            &rt,
            store.get_launch_template(&leased.tenant_id, &region, &leased.job_id.to_string()),
        )
        .map_err(|e| e.to_string())?;
        if launch.spec.trust_tier != leased.trust_tier {
            return Err("claimed trust tier differs from the durable launch template".into());
        }
        // CT-007 lease/topology reconciliation: THE mechanical per-job null-window check. A
        // checkout-bearing job claimed on a legacy row has no durable four-execution ceiling, so its
        // claim would expire mid-preparation; refuse here, before the mint, so no resolved `JobSpec`
        // for such a row ever reaches `RunnerAgent`/`launch_with`. The row stays leased and the
        // reaper recovers it. The token issuer repeats this refusal under its own row lock, and the
        // regional activation guard counts such rows — enforcement is per-job, not procedural.
        if leased.claim_window_secs.is_none()
            && crate::ci_claim_window::is_checkout_bearing(launch.spec.kind, &launch.spec.workspace)
                .map_err(|e| e.to_string())?
        {
            return Err(
                "checkout-bearing job was claimed on a legacy row with no durable claim window; \
                 refusing before mint (its claim would expire mid-preparation)"
                    .into(),
            );
        }
        let request = CiJobTokenRequest {
            tenant_id: leased.tenant_id.clone(),
            region: region.clone(),
            project_id: launch.project_id.clone(),
            wf_run_id: leased.run_id.to_string(),
            ci_run_id: launch.ci_run_id,
            job_id: leased.job_id.to_string(),
            token_authority_handle: launch.token_authority_handle.clone(),
            idem_token: launch.spec.idem_token.0.clone(),
            lease_owner: leased.lease_owner.clone(),
            lease_epoch: leased.lease_epoch,
            claim_nonce: leased.claim_nonce.clone(),
            claim_started_at_epoch_secs: leased.claim_started_at_epoch_secs,
            claim_expires_at_epoch_secs: leased.claim_expires_at_epoch_secs,
        };
        request.validate().map_err(|e| e.to_string())?;
        // CT-007 slice 5b.3-2c: derive the checkout scope from the SAME already-loaded, immutable
        // `launch.spec.workspace` the claim-time issuer itself locked and validated this row
        // against (5b.3-2b) -- via the ONE sanctioned facade, so this can never parse differently
        // than what was actually authorized.
        let checkout = derive_checkout_authorization_scope(JobKind::Ci, &launch.spec.workspace)
            .map_err(|e| e.to_string())?;
        let authorization = ci_job_authorization_context(
            &request,
            &launch.spec.meter_to.reserve_id,
            checkout.as_ref(),
        );
        let run_token = bridge(&rt, token_issuer.mint(request.clone())).map_err(|e| e.to_string())?;
        validate_run_token(&run_token, &launch.token_authority_handle).map_err(|e| e.0)?;
        let resolution = resolve_claim_launch_secrets(
            &TenantId(leased.tenant_id.clone()),
            launch.spec,
            run_token,
            authorization,
            &secrets,
        );
        finish_v1_claim_secret_resolution(resolution, &request, &secret_terminal_reporter)
    })
}

/// **CT-007 slice 5b.3-6e.2: the activated V2 spec resolver.** Identical claim reconstruction,
/// trust/null-window checks, and scope derivation as [`durable_spec_resolver`], but the resolved
/// spec's INITIAL run-token authorization is minted through the V2 phase-credential path:
/// [`V2CheckoutComposition::mint_initial_phase_credential`](crate::ci_checkout_composition::V2CheckoutComposition::mint_initial_phase_credential)
/// mints a `CheckoutAdvertise` generation for a checkout-bearing job (or a `Workload` generation for a
/// compute job) and returns the phase-authorization context — with the credential binding — that the
/// parent-attempt reserve hook later reconstructs the exact durable claim from. Stage B selects it
/// only through `ci_runner_v2_wiring`.
pub fn durable_v2_spec_resolver(
    store: CiJobSpecStore,
    region: impl Into<String>,
    rt: tokio::runtime::Handle,
    checkout_composition: crate::ci_checkout_composition::V2CheckoutComposition,
    secrets: CiJobSecretResolver,
    secret_terminal_reporter: CiPipelineReporterRouter,
) -> JobSpecResolver {
    let region = region.into();
    Arc::new(move |leased: &LeasedJob| {
        let launch = bridge(
            &rt,
            store.get_launch_template(&leased.tenant_id, &region, &leased.job_id.to_string()),
        )
        .map_err(|e| e.to_string())?;
        if launch.spec.trust_tier != leased.trust_tier {
            return Err("claimed trust tier differs from the durable launch template".into());
        }
        // The SAME mechanical per-job null-window refusal the V1 resolver performs: a checkout-bearing
        // job claimed on a legacy row has no durable four-execution ceiling — refuse before the mint.
        if leased.claim_window_secs.is_none()
            && crate::ci_claim_window::is_checkout_bearing(launch.spec.kind, &launch.spec.workspace)
                .map_err(|e| e.to_string())?
        {
            return Err(
                "checkout-bearing job was claimed on a legacy row with no durable claim window; \
                 refusing before mint (its claim would expire mid-preparation)"
                    .into(),
            );
        }
        let request = CiJobTokenRequest {
            tenant_id: leased.tenant_id.clone(),
            region: region.clone(),
            project_id: launch.project_id.clone(),
            wf_run_id: leased.run_id.to_string(),
            ci_run_id: launch.ci_run_id,
            job_id: leased.job_id.to_string(),
            token_authority_handle: launch.token_authority_handle.clone(),
            idem_token: launch.spec.idem_token.0.clone(),
            lease_owner: leased.lease_owner.clone(),
            lease_epoch: leased.lease_epoch,
            claim_nonce: leased.claim_nonce.clone(),
            claim_started_at_epoch_secs: leased.claim_started_at_epoch_secs,
            claim_expires_at_epoch_secs: leased.claim_expires_at_epoch_secs,
        };
        request.validate().map_err(|e| e.to_string())?;
        let checkout_scope =
            derive_checkout_authorization_scope(JobKind::Ci, &launch.spec.workspace)
                .map_err(|e| e.to_string())?;
        // The V2 initial mint: CheckoutAdvertise for a checkout job, Workload for a compute job. The
        // returned authorization context carries the credential binding the parent-attempt reserve
        // hook reconstructs the exact durable claim from.
        let (minted, authorization) = checkout_composition
            .mint_initial_phase_credential(
                &request,
                &launch.spec.meter_to.reserve_id,
                checkout_scope.as_ref(),
            )
            .map_err(|e| e.to_string())?;
        validate_run_token(&minted.credential, &launch.token_authority_handle).map_err(|e| e.0)?;
        let resolution = resolve_claim_launch_secrets(
            &TenantId(leased.tenant_id.clone()),
            launch.spec,
            minted.credential,
            authorization,
            &secrets,
        );
        finish_claim_secret_resolution(resolution, &request, &secret_terminal_reporter)
    })
}

/// **The durable backing for the sandbox's preparation-lease checkpoint seam (CT-007 lease/topology
/// reconciliation).** Binds ONE exact claim generation and renews its execution lease through
/// [`CiJobQueueStore::renew_preparation_lease`], bridging the sync sandbox call onto the runner
/// thread's runtime with the same off-runtime `block_on` convention [`DurableLeaseAdapter`] uses.
///
/// **Deliberately dormant.** The transport takes `Option<&dyn PreparationLeaseCheckpoint>` and every
/// caller passes `None` today; 5b.3-6's composition is what constructs this and adds the later Hop
/// A→B and B→workload checkpoints. It exists here so the composition slice wires an already-proven
/// seam instead of inventing durable ownership semantics inline.
pub struct DurablePreparationLeaseCheckpoint {
    store: CiJobQueueStore,
    claim: crate::job_queue_store::CiJobLaunchClaim,
    rt: tokio::runtime::Handle,
}

impl DurablePreparationLeaseCheckpoint {
    /// Bind the checkpoint to one exact durable claim generation.
    pub fn new(
        store: CiJobQueueStore,
        claim: crate::job_queue_store::CiJobLaunchClaim,
        rt: tokio::runtime::Handle,
    ) -> DurablePreparationLeaseCheckpoint {
        DurablePreparationLeaseCheckpoint { store, claim, rt }
    }
}

impl myelin_ci_sandbox::PreparationLeaseCheckpoint for DurablePreparationLeaseCheckpoint {
    /// A DB error is treated as lost ownership, not as success: continuing to the next execution on
    /// an unconfirmed renewal is exactly the double-run the lease exists to prevent.
    fn renew(&self) -> Result<(), myelin_ci_sandbox::PreparationLeaseLost> {
        match bridge(&self.rt, self.store.renew_preparation_lease(&self.claim)) {
            Ok(true) => Ok(()),
            Ok(false) => Err(myelin_ci_sandbox::PreparationLeaseLost(format!(
                "no live leased generation matched job {} epoch {} nonce {}",
                self.claim.job_id, self.claim.lease_epoch, self.claim.claim_nonce
            ))),
            Err(error) => Err(myelin_ci_sandbox::PreparationLeaseLost(format!(
                "renewal query failed (treated as lost ownership, fail-closed): {error}"
            ))),
        }
    }
}

#[cfg(test)]
mod shutdown_tests {
    use super::*;

    #[test]
    fn pre_signalled_shutdown_interrupts_backoff_without_sleeping() {
        let (_sender, mut receiver) = tokio::sync::watch::channel(true);
        let started = std::time::Instant::now();
        assert!(runner_sleep_until_shutdown(
            &mut receiver,
            Duration::from_secs(2)
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn sender_closure_is_shutdown() {
        let (sender, mut receiver) = tokio::sync::watch::channel(false);
        drop(sender);
        assert!(runner_shutdown_requested(&mut receiver));
    }
}

#[cfg(test)]
mod secret_withhold_terminal_tests {
    use super::*;
    use crate::{SecretLaunchError, WithheldSecret, WithholdReason};
    use myelin_ci_sandbox::{
        CiJobAuthorizationContext, EgressPolicy, IdemToken, JobSpecTemplate, MeterTarget,
        ResourceLimits, RunTokenAuthorizationContext, RunTokenCredential, SecretRef, WorkspaceSpec,
    };
    use std::sync::Mutex;

    struct RecordingTerminalizer {
        terminal: Mutex<bool>,
        failed_job_done: Mutex<bool>,
        accounting_settled: Mutex<bool>,
        reaper_eligible: Mutex<bool>,
        diagnostic: Mutex<Option<String>>,
    }

    impl Default for RecordingTerminalizer {
        fn default() -> Self {
            Self {
                terminal: Mutex::new(false),
                failed_job_done: Mutex::new(false),
                accounting_settled: Mutex::new(false),
                reaper_eligible: Mutex::new(true),
                diagnostic: Mutex::new(None),
            }
        }
    }

    impl SecretWithholdTerminalizer for RecordingTerminalizer {
        fn terminalize(
            &self,
            _claim: &CiJobTokenRequest,
            diagnostic: &str,
        ) -> Result<(), String> {
            *self.terminal.lock().unwrap() = true;
            *self.failed_job_done.lock().unwrap() = true;
            *self.accounting_settled.lock().unwrap() = true;
            *self.reaper_eligible.lock().unwrap() = false;
            *self.diagnostic.lock().unwrap() = Some(diagnostic.to_owned());
            Ok(())
        }
    }

    fn claim() -> CiJobTokenRequest {
        CiJobTokenRequest {
            tenant_id: "acme".into(),
            region: "fr-par".into(),
            project_id: "55555555-5555-4555-8555-555555555555".into(),
            wf_run_id: "10000000-0000-0000-0000-000000000001".into(),
            ci_run_id: "20000000-0000-0000-0000-000000000001".into(),
            job_id: "30000000-0000-0000-0000-000000000001".into(),
            token_authority_handle: "authority:job".into(),
            idem_token: "idem:job".into(),
            lease_owner: "runner:1".into(),
            lease_epoch: 1,
            claim_nonce: "40000000-0000-0000-0000-000000000001".into(),
            claim_started_at_epoch_secs: 1_000,
            claim_expires_at_epoch_secs: 1_300,
        }
    }

    #[test]
    fn unavailable_secret_resolution_is_terminal_and_not_claimable_again() {
        let terminalizer = RecordingTerminalizer::default();
        let resolution = Err(SecretLaunchError::Withheld(vec![WithheldSecret {
            name: "DEPLOY_KEY".into(),
            reason: WithholdReason::CapabilityUnavailable,
        }]));

        let error = finish_claim_secret_resolution(resolution, &claim(), &terminalizer)
            .expect_err("withhold settles failed and never produces a launch spec");
        assert!(error.contains("settled terminally"));
        assert_eq!(
            terminalizer.diagnostic.lock().unwrap().as_deref(),
            Some("secret_withheld:DEPLOY_KEY=capability_unavailable")
        );
        assert!(
            *terminalizer.terminal.lock().unwrap(),
            "the exact claim is terminal, so a lease/reaper predicate cannot select it again"
        );

        let query = crate::scheduler::CONSUME_SECRET_WITHHELD_CLAIM_QUERY;
        assert!(query.contains("SET state = 'terminal'"));
        assert!(query.contains("q.state = 'leased'"));
        assert!(query.contains("q.lease_epoch = $7"));
        assert!(query.contains("q.claim_nonce = $8::uuid"));
        assert!(query.contains("AND NOT EXISTS ("));
    }

    #[test]
    fn v1_unavailable_secret_job_is_terminally_settled_and_not_re_leased_or_reaped() {
        let claim = claim();
        let template = JobSpecTemplate::new(
            JobKind::Ci,
            ImageRef::pinned(format!("registry.example/job@sha256:{}", "a".repeat(64))).unwrap(),
            vec!["/bin/true".into()],
            Vec::new(),
            vec![SecretRef {
                name: "DEPLOY_KEY".into(),
                handle: "myelin://acme/ci/secret/opaque-handle".into(),
            }],
            EgressPolicy::deny_all(),
            ResourceLimits {
                cpu_millis: 1_000,
                mem_bytes: 256 * 1024 * 1024,
                disk_bytes: 1024 * 1024 * 1024,
                tmpfs_bytes: 64 * 1024 * 1024,
                pids_max: 64,
                timeout_secs: 30,
            },
            WorkspaceSpec::default(),
            TrustTier::Trusted,
            MeterTarget {
                reserve_id: "reserve:secret-test".into(),
            },
            IdemToken(claim.idem_token.clone()),
        )
        .unwrap();
        let authorization = RunTokenAuthorizationContext::CiJob(CiJobAuthorizationContext {
            tenant_id: claim.tenant_id.clone(),
            region: claim.region.clone(),
            principal_id: "ci-job".into(),
            project_id: claim.project_id.clone(),
            wf_run_id: claim.wf_run_id.clone(),
            job_id: claim.job_id.clone(),
            lease_owner: claim.lease_owner.clone(),
            lease_epoch: claim.lease_epoch,
            claim_nonce: claim.claim_nonce.clone(),
            claim_started_at_epoch_secs: claim.claim_started_at_epoch_secs,
            claim_expires_at_epoch_secs: claim.claim_expires_at_epoch_secs,
            reserve_id: "reserve:secret-test".into(),
            required_capabilities: Vec::new(),
            checkout_scope: None,
            credential_binding: None,
        });
        let resolution = resolve_claim_launch_secrets(
            &TenantId(claim.tenant_id.clone()),
            template,
            RunTokenCredential::new("bearer", "jti:v1-secret-test", 60).unwrap(),
            authorization,
            &crate::unavailable_ci_job_secret_resolver(),
        );
        let terminalizer = RecordingTerminalizer::default();

        let error = finish_v1_claim_secret_resolution(resolution, &claim, &terminalizer)
            .expect_err("V1 withhold must settle terminally and never produce a launch spec");
        assert!(error.contains("settled terminally"));
        assert!(*terminalizer.terminal.lock().unwrap());
        assert!(*terminalizer.failed_job_done.lock().unwrap());
        assert!(*terminalizer.accounting_settled.lock().unwrap());
        assert!(!*terminalizer.reaper_eligible.lock().unwrap());
        let diagnostic = terminalizer.diagnostic.lock().unwrap();
        assert_eq!(
            diagnostic.as_deref(),
            Some("secret_withheld:DEPLOY_KEY=capability_unavailable")
        );
        assert!(!diagnostic.as_deref().unwrap().contains("opaque-handle"));
    }
}

/// A direct test for [`production_gvisor_registry`] itself — before this, nothing called it; every
/// test built its own ad-hoc registry. All three real staged assets (`linux-small-v1`,
/// `linux-rust-v1`, `git-v1`)
/// are present on the founder-dogfood host this was written on, so this is a REAL, non-skipped
/// assertion here (matching this repo's existing runsc/KVM graceful-skip convention, this test would
/// need to skip on a host without the assets staged — but per the CT-007 gate 2/4 brief, this host
/// has both, so it is exercised for real).
#[cfg(test)]
mod production_gvisor_registry_tests {
    use super::*;

    #[test]
    fn constructs_and_resolves_all_real_images_to_their_expected_paths() {
        let base_dir = resolved_gvisor_rootfs();
        let rust_dir = resolved_gvisor_rust_rootfs();
        let git_dir = myelin_ci_sandbox::resolved_gvisor_git_rootfs();
        if !base_dir.is_dir() || !rust_dir.is_dir() || !git_dir.is_dir() {
            eprintln!(
                "constructs_and_resolves_all_real_images_to_their_expected_paths: SKIPPED — a \
                 staged base ({}) / rust ({}) / git ({}) rootfs is absent on this machine",
                base_dir.display(),
                rust_dir.display(),
                git_dir.display()
            );
            return;
        }

        let registry = production_gvisor_registry();

        let small_image = ImageRef::pinned(format!(
            "myelin.local/linux-small-v1-rootfs@sha256:{LINUX_SMALL_V1_ROOTFS_SHA256}"
        ))
        .unwrap();
        let rust_image = ImageRef::pinned(format!(
            "myelin.local/linux-rust-v1-rootfs@sha256:{LINUX_RUST_V1_ROOTFS_SHA256}"
        ))
        .unwrap();
        let git_image = ImageRef::pinned(format!(
            "myelin.local/git-v1-rootfs@sha256:{GVISOR_GIT_ROOTFS_SHA256}"
        ))
        .unwrap();

        let verified_small = registry
            .resolve(&small_image)
            .expect("the production registry must resolve linux-small-v1");
        let verified_rust = registry
            .resolve(&rust_image)
            .expect("the production registry must resolve linux-rust-v1");
        let verified_git = registry
            .resolve(&git_image)
            .expect("the production registry must resolve git-v1");

        assert_eq!(
            verified_small.path(),
            std::fs::canonicalize(&base_dir).unwrap(),
            "linux-small-v1 must resolve to the SAME canonicalized path resolved_gvisor_rootfs() names"
        );
        assert_eq!(
            verified_rust.path(),
            std::fs::canonicalize(&rust_dir).unwrap(),
            "linux-rust-v1 must resolve to the SAME canonicalized path resolved_gvisor_rust_rootfs() names"
        );
        assert_eq!(
            verified_git.path(),
            std::fs::canonicalize(&git_dir).unwrap(),
            "git-v1 must resolve to the same verified canonical path used by checkout"
        );
    }
}
