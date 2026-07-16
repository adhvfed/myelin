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

use myelin_ci_sandbox::gvisor::GvisorBackend;
use myelin_ci_sandbox::{
    CountingFirehose, EngineTerminalReporter, JobSpec, LeaseStore, QueuedJob, RunnerError,
    RunnerHooks, TrustTier,
};
use myelin_flow::FlowExecutor;
use myelin_tenancy::{Region, TenantId};

use crate::{CiJobQueueStore, LeasedJob};

/// **Resolve a leased `job_queue` row to its digest-pinned [`JobSpec`] (the CT-004d spec-store seam).**
/// The durable queue holds scheduling metadata, not the spec; this maps a claimed [`LeasedJob`] to the
/// `JobSpec` the sandbox launches. An `Err` (spec absent / not yet provisioned) makes the claim a
/// no-op (`None`) — the row stays leased and the dead-runner reaper re-queues it (safe, never a launch
/// of an unresolved job). In production the impl reads the durable spec store the dispatch writes
/// (CT-004d); the CT-004c.2 test injects a real compute spec.
pub type JobSpecResolver = Arc<dyn Fn(&LeasedJob) -> Result<JobSpec, String> + Send + Sync>;

/// Bridge one async durable-store call to the sync [`LeaseStore`] port. The runner loop drives this
/// from a dedicated OFF-runtime thread, so `block_on` runs directly; the `try_current` guard falls
/// back to `block_in_place` only if ever driven on a multi-thread runtime worker (the SAME convention
/// as `myelin_storage::kms_durable`). NEVER call this from a current-thread runtime (drive the runner
/// on its own thread — `CiRunnerLoop::spawn`).
fn bridge<F: std::future::Future>(rt: &tokio::runtime::Handle, fut: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(_) => tokio::task::block_in_place(|| rt.block_on(fut)),
        Err(_) => rt.block_on(fut),
    }
}

/// **The durable `job_queue` store adapted to the sandbox [`LeaseStore`] port (CT-004c.2).** Wraps the
/// pool-backed [`CiJobQueueStore`] so the SAME [`RunnerAgent::run_one`](myelin_ci_sandbox::RunnerAgent)
/// body claims / heartbeats / settles against live Postgres instead of the in-memory floor. It holds
/// the store, the runner's residency `region` (the heartbeat/complete tenant-scoped tx keys on it), a
/// tokio [`Handle`](tokio::runtime::Handle) for the async→sync bridge, and the [`JobSpecResolver`].
///
/// **The security invariant (the adversarial-verifier surface):** [`claim_for_labels`](Self::claim_for_labels)
/// forwards `allowed_tiers` + `region` to [`CiJobQueueStore::claim`] EXACTLY as received — no widening,
/// no default, no drop — so the durable `trust_tier = ANY($tiers) AND region = $region` predicate is
/// the sole eligibility gate. This type carries a durable store handle (no in-memory collection of
/// record), so it is NOT an in-memory durable store.
pub struct DurableLeaseAdapter {
    store: CiJobQueueStore,
    region: String,
    rt: tokio::runtime::Handle,
    resolve: JobSpecResolver,
}

impl DurableLeaseAdapter {
    /// Adapt the durable `job_queue` store to the runner's lease port. `region` is the runner's
    /// residency region (the same region its claim filters on); `rt` is the runtime handle the sync
    /// port bridges its async DB calls onto; `resolve` maps a leased row to its `JobSpec`.
    pub fn new(
        store: CiJobQueueStore,
        region: impl Into<String>,
        rt: tokio::runtime::Handle,
        resolve: JobSpecResolver,
    ) -> DurableLeaseAdapter {
        DurableLeaseAdapter {
            store,
            region: region.into(),
            rt,
            resolve,
        }
    }
}

impl LeaseStore for DurableLeaseAdapter {
    /// **Claim through the durable `FOR UPDATE SKIP LOCKED` predicate (arch 02 §2.1).** Forwards
    /// `region` + `runner_labels` + `allowed_tiers` UNCHANGED to [`CiJobQueueStore::claim`]. On a
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
            self.store
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
            self.store
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
            self.store.complete(&tenant.0, &self.region, job_id),
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
/// the AG-D4 gate), the firehose stub, the `job.done` reporter over the composition's [`FlowExecutor`],
/// and the four-guarantee hooks. [`run`](Self::run) constructs the [`RunnerAgent`] on its own thread
/// and loops `run_one` with backoff. Mirrors [`JobQueueReaper`](crate::JobQueueReaper): a bounded
/// background driver, no new `AppSpec` field, LOUD on failure and resilient (a launch failure logs and
/// continues; the §OQ-F dispatch retry re-runs the job).
///
/// **Reporter note (the CT-004d handoff):** the terminal `job.done` is delivered to `executor` — the
/// SAME durable executor the `ci.pipeline` body parks on. Starting that body on a shared executor (so
/// the report wakes a real parked run) is CT-004d; here the reporter is the ONE signal path
/// (`EngineTerminalReporter` → `DurableExecutor::signal`, exactly-once on `idem_token`), proven end to
/// end in the CT-004c.2 integration test.
pub struct CiRunnerLoop {
    worker_id: String,
    labels: Vec<String>,
    allowed_tiers: Vec<TrustTier>,
    region: String,
    lease_ttl_secs: i64,
    store: CiJobQueueStore,
    rt: tokio::runtime::Handle,
    resolve: JobSpecResolver,
    executor: FlowExecutor,
    hooks: RunnerHooks,
    idle_backoff: Duration,
    error_backoff: Duration,
}

impl CiRunnerLoop {
    /// Construct the runner loop. `worker_id`/`labels`/`allowed_tiers`/`region`/`lease_ttl_secs` are
    /// the claim predicates + lease TTL; `store` is the durable `job_queue` store; `rt` bridges the
    /// sync runner onto the async pool; `resolve` provides the `JobSpec` for a leased row (CT-004d
    /// seam); `executor` is the `job.done` target; `hooks` are the four-guarantee wiring.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        worker_id: impl Into<String>,
        labels: Vec<String>,
        allowed_tiers: Vec<TrustTier>,
        region: impl Into<String>,
        lease_ttl_secs: i64,
        store: CiJobQueueStore,
        rt: tokio::runtime::Handle,
        resolve: JobSpecResolver,
        executor: FlowExecutor,
        hooks: RunnerHooks,
    ) -> CiRunnerLoop {
        CiRunnerLoop {
            worker_id: worker_id.into(),
            labels,
            allowed_tiers,
            region: region.into(),
            lease_ttl_secs,
            store,
            rt,
            resolve,
            executor,
            hooks,
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
    /// keeps `block_on` correct and never starves a tokio worker. Returns the join handle (the loop
    /// runs until the process exits, like the reaper).
    pub fn spawn(self) -> std::thread::JoinHandle<()> {
        std::thread::Builder::new()
            .name("ci-runner".into())
            .spawn(move || self.run())
            .expect("spawn the ci-runner thread")
    }

    /// **Run the claim → launch → `job.done` cycle forever (until the process stops).** Constructs the
    /// [`RunnerAgent`] over the gVisor backend + the durable lease adapter, then loops: `NoWork` sleeps
    /// `idle_backoff`; `LeaseLost` is a clean immediate retry (another worker owns it now); a
    /// `LaunchFailed`/`ReportFailed` is logged LOUD and sleeps `error_backoff` (the dispatch activity
    /// retries the job, §OQ-F). Never reimplements any sandbox logic.
    pub fn run(self) {
        let CiRunnerLoop {
            worker_id,
            labels,
            allowed_tiers,
            region,
            lease_ttl_secs,
            store,
            rt,
            resolve,
            executor,
            hooks,
            idle_backoff,
            error_backoff,
        } = self;

        let backend = GvisorBackend::new();
        let firehose = CountingFirehose::new();
        let reporter = EngineTerminalReporter::new(executor);
        let adapter = DurableLeaseAdapter::new(store, region.clone(), rt, resolve);
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
            match agent.run_one(now_secs()) {
                Ok(o) => {
                    eprintln!(
                        "ci-runner[{worker_id}]: ran job {} for run {} (passed={}, job.done={:?})",
                        o.job_id, o.run_id, o.report.passed, o.signal_outcome
                    );
                }
                Err(RunnerError::NoWork) => std::thread::sleep(idle_backoff),
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
                    std::thread::sleep(error_backoff);
                }
                Err(e @ RunnerError::ReportFailed(_)) => {
                    eprintln!("ci-runner[{worker_id}]: terminal report FAILED (surfaced): {e}");
                    std::thread::sleep(error_backoff);
                }
            }
        }
    }
}

/// The runner's clock (epoch seconds) — the `now` the claim/heartbeat lease arithmetic uses.
fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// **The production spec-resolver seam (CT-004d floor).** Returns `Err` for every leased row — the
/// durable spec store (specs keyed by `job_id`, written by the `SCHEDULE_AND_RUN_JOB` dispatch) is
/// CT-004d. Until then a `main`-wired runner claims nothing it can launch (the row stays leased and is
/// reaped) — so enabling the runner before CT-004d is a safe no-op, never an unresolved launch. The
/// CT-004c.2 integration test injects a REAL resolver instead (proving the exec path on a real spec).
pub fn spec_store_unavailable_resolver() -> JobSpecResolver {
    Arc::new(|leased: &LeasedJob| {
        Err(format!(
            "no durable JobSpec store yet (CT-004d) for job {}; runner cannot launch an unresolved \
             job — leaving it leased for the reaper",
            leased.job_id
        ))
    })
}
