//! # `runner` — the runner agent + the lease/heartbeat handshake + the exactly-once
//! `job.done` terminal report (CI-P3 → P-238, M2)
//!
//! **Owning architecture docs (read in full before changing):**
//! - `planning/04-subsystem-architectures/continuous-integration/architecture/00-overview.md` §4
//!   (*the runner agent = a small attested Rust binary; hosted + self-hosted from one artifact*):
//!   it "pulls leases, launches the sandbox via `SandboxBackend`, streams frames, reports terminal
//!   via the `job.done` signal. Same binary hosted + self-hosted."
//! - `.../02-internals-and-algorithms.md` §2.1 (*pull-leasing — the assignment model*): a runner
//!   long-polls `job_queue`, claims the next eligible job for its labels via `FOR UPDATE SKIP
//!   LOCKED`, takes a **lease** (`lease_owner` + `lease_expires`), and **heartbeats** to extend it.
//!   "This reuses the platform's existing lease primitive (the outbox relay, the timer wheel) —
//!   proven, not novel." The dead-runner **reaper** is the OTHER side (CI-P12, named floor here).
//! - `.../03-events-contracts-and-glue.md` §1.1 / §2 (the `job.done` terminal report shape — a
//!   signal, idempotent on `idem_token`, references-not-payloads).
//! - `planning/05-refined-shared-systems-architecture/00-reconciliation-decisions.md` §OQ-F (the
//!   `SCHEDULE_AND_RUN_JOB` long-park: the runner "can deliver 'done' twice (at-least-once) and the
//!   workflow wakes once. The signal is **idempotent on `idem_token`**").
//!
//! **Contracts CONSUMED:** 9.2/9.4 (`job.done` — the terminal report the runner emits), 4.7
//! (`mint_run_token` — the runner CONSUMES the minted per-job/self-hosted token), 12.4
//! (`residency_verify` — the runner pool region; the claim is region-pinned, no global pool).
//!
//! ## What this prompt (CI-P3) ships — the runner SIDE only, RECONCILED with the engine
//!
//! Three things, all the runner half of an already-built contract — never a fork:
//!
//! 1. **The lease/heartbeat handshake (runner side).** [`JobLeaseStore`] models the platform's
//!    frozen `FOR UPDATE SKIP LOCKED` + heartbeat lease primitive — the SAME shape
//!    `myelin_flow::engine::RunStore::lease_runnable` models for the workflow dispatcher (the
//!    proven primitive, reused, not reinvented). The runner side is exactly three moves:
//!    [`JobLeaseStore::claim_for_labels`] (claim the highest-priority, in-region, label-eligible job
//!    via skip-locked) → [`JobLeaseStore::heartbeat`] (renew `lease_expires` while the job runs) →
//!    terminal report (then [`JobLeaseStore::settle`]). The **reaper/reclaim side** (sweep expired
//!    leases, re-queue) is **CI-P12** (named floor) — here a claim simply SKIPS a live lease and
//!    claims one whose lease has EXPIRED, the same skip-locked safety, so an expired lease is
//!    reclaimable by construction.
//!
//! 2. **The runner agent.** [`RunnerAgent::run_one`] performs the whole claim → launch → terminal
//!    cycle: claim a job for the runner's labels, launch the sandbox via
//!    [`SandboxBackend::launch`] (CI-P1/CI-P2 — the runner does NOT reimplement the sandbox), stream
//!    firehose frames (STUBBED — see [`FirehoseSink`]; the full log pipeline is **CI-P20**), and on
//!    terminal report `job.done` ECHOING the spec's `idem_token`. Same agent hosted + self-hosted —
//!    the self-hosted ATTESTATION GATE + the tenant-`SelfHosted`-scoped token mint is **CI-P4 →
//!    P-240** (named floor); here the runner CONSUMES the minted token off the `JobSpec.run_token`
//!    (4.7).
//!
//! 3. **The exactly-once terminal report — RECONCILED with the engine signal path.** The runner
//!    reports terminal by delivering the `job.done` durable signal through the ENGINE's
//!    [`myelin_flow::DurableExecutor::signal`] with `signal_name = JOB_DONE_SIGNAL` and
//!    `idem_key = idem_token` — whose `INSERT … ON CONFLICT (tenant, run_id, signal_name, idem_key)
//!    DO NOTHING` IS the exactly-once wake (recon §OQ-F; shipped P-FLOW-09 / P-205). The runner can
//!    deliver "done" TWICE (at-least-once); the engine buffers it ONCE; the parked workflow wakes
//!    ONCE. **There is NO second signal path** — [`TerminalReporter`] is a thin echo onto
//!    `DurableExecutor::signal`, and [`EngineTerminalReporter`] wraps a real
//!    [`myelin_flow::FlowExecutor`].
//!
//! ## FLOORS named (CI-P3)
//! - **Pre-warmed snapshot pools** (the cold-start mitigation), the **self-hosted attestation gate**,
//!   and the tenant-`SelfHosted`-scoped token mint → **CI-P4 (→ P-240)**. Here the runner CONSUMES a
//!   minted token; it does not mint or attest.
//! - **The firehose log pipeline** (the full `(job, step, byte-range)` index + the resume-cursor
//!   protocol + the `ci.log.available` coalesced pointer) → **CI-P20**. Here [`FirehoseSink`] is a
//!   STUB seam that counts frames; its durable implementation publishes the deeper
//!   `ci.log.available` pointer.
//! - **The reaper / dead-lease reclaim** (sweep expired leases → re-queue → fresh lease) → **CI-P12**.
//!   Here the claim SKIPS live leases and claims expired ones (so an expired lease is reclaimable);
//!   the active sweeper that re-queues a dead runner's job is CI-P12.
//!
//! ## MUTATION-SCORE FLOOR (mandatory-core)
//! The **terminal-report idempotency module** — [`TerminalReporter`] / [`EngineTerminalReporter`] +
//! the runner's terminal-report leg ([`RunnerAgent::run_one`] step 4 / [`RunnerAgent::report_done_again`])
//! — is **mandatory-core** (it carries the exactly-once-under-at-least-once property: a doubly-delivered
//! `job.done` must wake the parked workflow ONCE, double-effect = 0). Its cargo-mutants
//! mutation-score floor is **100% (zero surviving mutants)** — the same floor the engine's signal
//! idempotency carries (P-FLOW-09), because the runner reuses that exact path and a surviving mutant
//! here would mean a fork that silently double-wakes. The lease/heartbeat handshake module
//! ([`JobLeaseStore`]) carries a **≥ 90%** floor (the skip-locked / owner-only-renew / expiry-reclaim
//! invariants are load-bearing but not the irreversible-effect surface).
//!
//! ## DB-free / VM-free by default
//! [`JobLeaseStore`] and [`RunnerAgent`] are in-memory value/trait code modelling the frozen Postgres
//! `FOR UPDATE SKIP LOCKED` lease + the engine signal idempotency (the dev↔prod CONFIG SWAP — never a
//! code change). The REAL `FOR UPDATE SKIP LOCKED` claim + heartbeat + the exactly-once terminal
//! report run against LIVE Postgres ONLY in `tests/integration_runner_lease.rs` (the `integration`
//! feature). `cargo build --workspace` + the default `cargo test` stay DB-free AND VM-free.

use crate::{JobSpec, ResourceUsage, RunnerHooks, SandboxBackend, SandboxLaunch};
use myelin_flow::{
    DurableExecutor, ExecutorError, RunId, SignalOutcome, SignalSpec, JOB_DONE_SIGNAL,
};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

// =================================================================================================
// The lease/heartbeat handshake (runner side) — REUSES the FOR UPDATE SKIP LOCKED + heartbeat
// primitive (arch 02 §2.1). The reaper/reclaim side is CI-P12 (named floor).
// =================================================================================================

/// **A queued job row the runner claims a lease over (the runner's view of `job_queue`, arch 02
/// §2.1 / arch 01 §3).** The fairness/lanes/concurrency predicates of the FULL scheduler claim are
/// CI-P11/CI-P12; the runner side cares about the lease columns + the residency/affinity/trust
/// predicates that decide whether THIS runner may claim it. References-not-payloads — the `spec` is
/// the digest-pinned [`JobSpec`] (no PII; secrets are names).
#[derive(Clone, Debug)]
pub struct QueuedJob {
    /// `(tenant, region)` partition key — the residency pin (12.1). A runner claims ONLY in-region
    /// jobs (no global pool, 12.4).
    pub tenant: TenantId,
    /// `(tenant, region)` residency pin — the claim predicate `q.region = $cell_region`.
    pub region: Region,
    /// The durable run id of the parked workflow this job belongs to (the `job.done` target).
    pub run_id: String,
    /// The job's id within the queue (the lease row key).
    pub job_id: String,
    /// The job's required labels — the affinity predicate `q.labels <@ $runner_labels` (job labels
    /// MUST be a subset of the runner's labels).
    pub labels: Vec<String>,
    /// The digest-pinned hardened [`JobSpec`] the runner launches. Its `idem_token` (minted by the
    /// workflow at `SCHEDULE_AND_RUN_JOB` dispatch) is ECHOED on the terminal `job.done` (the
    /// no-coordination dedup agreement, §OQ-F).
    pub spec: JobSpec,
    /// The worker currently holding the lease (`None` = unleased, claimable). The skip-locked claim
    /// stamps it.
    pub lease_owner: Option<String>,
    /// The lease deadline (epoch seconds in this in-memory model; a `timestamptz` in PG). A claim
    /// SKIPS a row whose lease is LIVE (`lease_expires > now`) and may claim one whose lease has
    /// EXPIRED (`lease_expires <= now`) — the crash-recovery / reclaim seam the CI-P12 reaper drives.
    pub lease_expires: Option<i64>,
    /// **The claim generation** — the monotone epoch the claim bumped (CT-004d.2 claim-bound
    /// completion). Carried to `report_done` so the durable completion CAS refuses a stale claim: a
    /// worker whose lease was reaped and re-claimed holds a LOWER epoch than the row and cannot win
    /// first delivery. `0` for a fresh, unclaimed row.
    pub lease_epoch: i64,
    /// Opaque authority for this exact claim generation. Production mints an unguessable UUID in
    /// the claim statement; the in-memory model uses a deterministic non-authoritative token.
    pub claim_nonce: String,
}

impl QueuedJob {
    /// A fresh, unclaimed queued job (no lease). The full scheduler enqueues these (CI-P11); the
    /// runner-side test seeds them directly.
    pub fn new(
        tenant: TenantId,
        region: Region,
        run_id: impl Into<String>,
        job_id: impl Into<String>,
        labels: Vec<String>,
        spec: JobSpec,
    ) -> Self {
        Self {
            tenant,
            region,
            run_id: run_id.into(),
            job_id: job_id.into(),
            labels,
            spec,
            lease_owner: None,
            lease_expires: None,
            lease_epoch: 0,
            claim_nonce: String::new(),
        }
    }
}

/// **The in-memory `job_queue` lease store — the runner-side `FOR UPDATE SKIP LOCKED` + heartbeat
/// primitive (arch 02 §2.1), REUSED not reinvented.** A cloneable handle over the shared queue (an
/// `Arc<Mutex<…>>`), modelling EXACTLY the lease columns + skip-locked claim that
/// `myelin_flow::engine::RunStore::lease_runnable` models for the workflow dispatcher (the proven
/// platform primitive). The runner side is three moves: [`claim_for_labels`](Self::claim_for_labels)
/// → [`heartbeat`](Self::heartbeat) → [`settle`](Self::settle). The REAPER (sweep expired → re-queue)
/// is CI-P12.
///
/// **Live binding:** the real apply is the frozen `job_queue` `FOR UPDATE SKIP LOCKED` claim +
/// `UPDATE … SET lease_expires = now() + ttl` heartbeat in `tests/integration_runner_lease.rs` (the
/// skip-locked IS the no-double-claim safety, never an application-level check) — the dev↔prod config
/// swap, never a code change.
#[derive(Clone, Default)]
pub struct JobLeaseStore {
    inner: Arc<Mutex<HashMap<(String, String), QueuedJob>>>,
}

impl JobLeaseStore {
    /// A fresh, empty job-queue lease store.
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<(String, String), QueuedJob>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn key(j: &QueuedJob) -> (String, String) {
        (j.tenant.0.clone(), j.job_id.clone())
    }

    /// Enqueue a job (the full scheduler's role, CI-P11; here the test/runner seeds it).
    pub fn enqueue(&self, job: QueuedJob) {
        self.lock().insert(Self::key(&job), job);
    }

    /// Read a job row by `(tenant, job_id)`.
    pub fn get(&self, tenant: &TenantId, job_id: &str) -> Option<QueuedJob> {
        self.lock()
            .get(&(tenant.0.clone(), job_id.to_string()))
            .cloned()
    }

    /// **Claim ONE eligible job for `worker` with `runner_labels` in `region` (the `FOR UPDATE SKIP
    /// LOCKED` claim, arch 02 §2.1).** Returns the first job that is (a) in `region` (residency, no
    /// global pool, 12.4), (b) **label-eligible** — `job.labels ⊆ runner_labels` (affinity), (c)
    /// **trust-eligible** — `job.trust_tier ∈ allowed_tiers` (an untrusted job never reaches a
    /// runner that does not admit its tier), AND (d) whose lease is FREE (unleased OR EXPIRED at
    /// `now`), stamping `lease_owner = worker` + `lease_expires = now + lease_ttl_secs`. A job
    /// another worker holds a LIVE lease on is SKIPPED (no two runners run the same job — the
    /// skip-locked safety). Returns `None` if no eligible job awaits a lease.
    ///
    /// **The expiry re-claim is the reclaim seam (arch 02 §2.1):** a runner that DIED holds a lease
    /// that EXPIRES; once expired, this hands the job to another runner. The ACTIVE reaper that
    /// re-queues a dead runner's job (so its `SCHEDULE_AND_RUN_JOB` activity retries) is CI-P12; here
    /// the claim simply admits an expired-lease job, so an expired lease IS reclaimable.
    pub fn claim_for_labels(
        &self,
        worker: &str,
        runner_labels: &[String],
        allowed_tiers: &[crate::TrustTier],
        region: &Region,
        now: i64,
        lease_ttl_secs: i64,
    ) -> Option<QueuedJob> {
        let mut q = self.lock();
        // Deterministic scan order (by job_id) so the claim is stable across runners — models the
        // `ORDER BY` of the claim query (arch 02 §2.1); the full fairness/lane order is CI-P11.
        let mut keys: Vec<_> = q.keys().cloned().collect();
        keys.sort();
        for k in keys {
            let job = q.get_mut(&k).expect("key from the same map");
            // RESIDENCY (12.4): in-region only — no global pool.
            if &job.region != region {
                continue;
            }
            // AFFINITY: job labels ⊆ runner labels (job.labels <@ $runner_labels).
            if !job.labels.iter().all(|l| runner_labels.contains(l)) {
                continue;
            }
            // TRUST: the runner must admit the job's trust tier (an untrusted_fork job never reaches
            // a runner that does not admit it; the self-hosted scope is CI-P4).
            if !allowed_tiers.contains(&job.spec.trust_tier) {
                continue;
            }
            let lease_free = match job.lease_expires {
                None => true,
                Some(exp) => exp <= now, // EXPIRED — the dead runner's lease lapsed (reclaim seam).
            };
            if lease_free {
                job.lease_owner = Some(worker.to_string());
                job.lease_expires = Some(now + lease_ttl_secs);
                // Bump the claim generation (models the durable `lease_epoch = lease_epoch + 1`) so a
                // reaped-then-re-claimed job carries a higher epoch and a stale worker's completion is
                // refused by the CAS.
                job.lease_epoch += 1;
                job.claim_nonce = format!("memory:{worker}:{}", job.lease_epoch);
                return Some(job.clone());
            }
            // else: a LIVE lease another runner holds — SKIP it (skip-locked; no double-run).
        }
        None
    }

    /// **Heartbeat-renew the lease `worker` holds on `job_id` (arch 02 §2.1).** Extends
    /// `lease_expires` to `now + lease_ttl_secs` so a long-running job's lease does not lapse mid-run
    /// (the reaper would otherwise reclaim it). Returns `true` if the renew applied — `false` if the
    /// job is gone OR the caller is NOT the current lease owner (a runner can only heartbeat its OWN
    /// lease; a runner whose lease already lapsed and was reclaimed by another worker CANNOT renew it,
    /// which is exactly the lost-lease detection the runner needs to stop launching). This is the
    /// `UPDATE job_queue SET lease_expires = $now + $ttl WHERE job_id = $j AND lease_owner = $worker`.
    pub fn heartbeat(
        &self,
        worker: &str,
        tenant: &TenantId,
        job_id: &str,
        now: i64,
        lease_ttl_secs: i64,
    ) -> bool {
        let mut q = self.lock();
        match q.get_mut(&(tenant.0.clone(), job_id.to_string())) {
            Some(job) if job.lease_owner.as_deref() == Some(worker) => {
                job.lease_expires = Some(now + lease_ttl_secs);
                true
            }
            // not the owner (or reclaimed by another worker, or gone) — the renew is refused.
            _ => false,
        }
    }

    /// **Settle a claimed job — remove it from the queue on terminal (the lease is released by
    /// deleting the row).** The runner calls this AFTER the terminal report is delivered; the engine
    /// signal idempotency (not this delete) is what makes the wake exactly-once, so a re-delivered
    /// report on a settled job is still a harmless engine no-op. A no-op if the job is absent.
    pub fn settle(&self, tenant: &TenantId, job_id: &str) {
        self.lock().remove(&(tenant.0.clone(), job_id.to_string()));
    }

    /// The number of CLAIMABLE jobs for `runner_labels` in `region` at `now` (the queue-depth signal
    /// the autoscaler reads, arch 02 §5.4 — and the runner long-poll's "is there work" check). A job
    /// is claimable if it is in-region, label-eligible, and its lease is free (unleased or expired).
    pub fn claimable_depth(&self, runner_labels: &[String], region: &Region, now: i64) -> usize {
        self.lock()
            .values()
            .filter(|j| {
                &j.region == region
                    && j.labels.iter().all(|l| runner_labels.contains(l))
                    && j.lease_expires.map(|e| e <= now).unwrap_or(true)
            })
            .count()
    }
}

// =================================================================================================
// The lease/heartbeat PORT (CT-004c.2) — the seam [`RunnerAgent::run_one`] claims/heartbeats/settles
// through, so the SAME agent drives BOTH the in-memory floor ([`JobLeaseStore`]) AND the DURABLE
// `job_queue` store (`myelin_ci_controlplane::CiJobQueueStore`, adapted). Three moves, EXACTLY the
// signatures `run_one` already calls — so binding the durable store is a NEW impl of this port, NOT a
// change to the security-load-bearing claim→launch→report body.
// =================================================================================================

/// **The runner-side lease port (CT-004c.2) — claim / heartbeat / settle, the three moves
/// [`RunnerAgent::run_one`] performs.** Modelled EXACTLY on the in-memory [`JobLeaseStore`]'s method
/// shapes so the runner body is unchanged: the durable adapter
/// (`myelin_ci_controlplane`'s `DurableLeaseAdapter`) implements THIS trait over the pool-backed
/// `CiJobQueueStore`, passing the runner's `allowed_tiers` + `region` STRAIGHT THROUGH to the durable
/// `FOR UPDATE SKIP LOCKED` claim UNCHANGED (an `untrusted_fork` job is never claimable by a
/// trusted-only runner — the tier/region predicate is the durable store's, never re-derived here).
///
/// **The security contract of an impl:** [`claim_for_labels`](LeaseStore::claim_for_labels) MUST
/// forward `allowed_tiers` and `region` to its claim WITHOUT widening, defaulting, or dropping either
/// — the eligibility gate lives in the store the impl delegates to, never in the agent.
pub trait LeaseStore {
    /// Claim ONE eligible job for `worker`, in `region`, whose labels ⊆ `runner_labels` and whose
    /// `trust_tier ∈ allowed_tiers`, taking a lease `= now + lease_ttl_secs`. `None` when nothing is
    /// eligible (the long-poll found no work). The tier/region filter is forwarded UNCHANGED.
    fn claim_for_labels(
        &self,
        worker: &str,
        runner_labels: &[String],
        allowed_tiers: &[crate::TrustTier],
        region: &Region,
        now: i64,
        lease_ttl_secs: i64,
    ) -> Option<QueuedJob>;

    /// Renew the lease `worker` holds on `(tenant, job_id)`. `false` if the caller is NOT the owner
    /// (the lost-lease detection [`RunnerAgent::run_one`] Step-2 stops on — fail-closed).
    fn heartbeat(
        &self,
        worker: &str,
        tenant: &TenantId,
        job_id: &str,
        now: i64,
        lease_ttl_secs: i64,
    ) -> bool;

    /// Settle a claimed job on terminal (the engine signal idempotency — not this — is what makes the
    /// wake exactly-once, so a settle after a redelivered report is a harmless no-op).
    fn settle(&self, tenant: &TenantId, job_id: &str);
}

/// The in-memory floor IS a [`LeaseStore`] (the dev↔prod config swap). Each method delegates to the
/// inherent method of the same name — method-call syntax resolves to the INHERENT method (inherent
/// wins over a trait method), so this is a thin forward, never a recursion.
impl LeaseStore for JobLeaseStore {
    fn claim_for_labels(
        &self,
        worker: &str,
        runner_labels: &[String],
        allowed_tiers: &[crate::TrustTier],
        region: &Region,
        now: i64,
        lease_ttl_secs: i64,
    ) -> Option<QueuedJob> {
        // Inherent `JobLeaseStore::claim_for_labels` (method-call → inherent, not this trait method).
        self.claim_for_labels(
            worker,
            runner_labels,
            allowed_tiers,
            region,
            now,
            lease_ttl_secs,
        )
    }

    fn heartbeat(
        &self,
        worker: &str,
        tenant: &TenantId,
        job_id: &str,
        now: i64,
        lease_ttl_secs: i64,
    ) -> bool {
        self.heartbeat(worker, tenant, job_id, now, lease_ttl_secs)
    }

    fn settle(&self, tenant: &TenantId, job_id: &str) {
        self.settle(tenant, job_id)
    }
}

// =================================================================================================
// The firehose STUB seam — the full log pipeline is CI-P20 (named floor).
// =================================================================================================

/// **The firehose frame sink the runner streams to (the CI-P20 live-log seam).** The runner ships
/// each captured stdout/stderr chunk as a frame keyed by `(run, job)` within a `tenant` (arch 02
/// §7.1), then calls [`finish`](FirehoseSink::finish) once the job's output is complete so the sink
/// can seal + index + emit the `ci.log.available` pointer. The live tail rides the resume-cursor
/// protocol and sealed segments flush to the T2 log tier with the `(job, step, byte-range)` index.
///
/// **Cycle-safe seam (CT-004f F1/F2):** this trait lives in `ci-sandbox` (the LOWER crate — the
/// runner cannot depend on `ci-controlplane`'s `LogPipeline`). The real
/// `LogPipelineSink` in `ci-controlplane` (the HIGHER crate, which CAN name both) implements this
/// and drives the pipeline. `tenant` is threaded per frame because a runner is MULTI-TENANT (its
/// claim has no tenant filter), so the shared sink opens a per-`(tenant, run, job)` pipeline lazily;
/// `region` is NOT on the seam (a runner serves ONE region — the sink holds it at construction).
///
/// **Redaction is NOT this seam's job (CT-004f F3):** the frames the runner ships MUST already be
/// redacted at the sandbox BOUNDARY (where the broker resolved the plaintext) — the least-privilege
/// runner holds only opaque `SecretRef`s, so it cannot mask here. The pipeline's redactor is empty
/// defence-in-depth. See `planning/system-reviews/2026-07-17-ct004f-log-pipeline-scoping.md`.
pub trait FirehoseSink {
    /// Ship one firehose frame (a captured stdout/stderr chunk) for `(run_id, job_id)` within
    /// `tenant`. The STUB counts; the real sink appends to the open segment + publishes the live tail.
    fn ship_frame(&self, run_id: &str, job_id: &str, tenant: &TenantId, frame: &[u8]);
    /// The job's output is complete — CLOSE the step anchor with the job's verdict (`passed`), seal
    /// the open segment, flush the `(job, step, byte-range)` index, and emit the coalesced
    /// `ci.log.available` pointer. `passed` becomes the anchor status (`passed`/`failed`) the
    /// jump-to-failure deep-link reads (a single-command job = one step, so the job verdict IS the
    /// step verdict). The STUB is a no-op; the real sink drives `LogPipeline::flush_job` + drains
    /// pointers to the outbox. Idempotent (a re-delivered terminal report calls this again → no
    /// double seal).
    fn finish(&self, run_id: &str, job_id: &str, tenant: &TenantId, passed: bool);
}

/// A counting [`FirehoseSink`] stub — the test floor. Counts frames shipped so a test can assert the
/// runner streamed; the real firehose transport ([`LogPipelineSink`](../../myelin_ci_controlplane))
/// replaces it with NO runner change. `finish` is a no-op here (nothing to seal in a counter).
#[derive(Clone, Default)]
pub struct CountingFirehose {
    count: Arc<Mutex<u64>>,
    finished: Arc<Mutex<u64>>,
}

impl CountingFirehose {
    /// A fresh counting firehose stub.
    pub fn new() -> Self {
        Self::default()
    }
    /// The number of frames shipped (the CI-P20 floor's observable).
    pub fn frames_shipped(&self) -> u64 {
        *self.count.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// The number of `finish` calls (the terminal-flush observable — one per completed job).
    pub fn jobs_finished(&self) -> u64 {
        *self.finished.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl FirehoseSink for CountingFirehose {
    fn ship_frame(&self, _run_id: &str, _job_id: &str, _tenant: &TenantId, _frame: &[u8]) {
        *self.count.lock().unwrap_or_else(|e| e.into_inner()) += 1;
    }
    fn finish(&self, _run_id: &str, _job_id: &str, _tenant: &TenantId, _passed: bool) {
        *self.finished.lock().unwrap_or_else(|e| e.into_inner()) += 1;
    }
}

// =================================================================================================
// The exactly-once `job.done` terminal report — RECONCILED with the engine signal path (no fork).
// =================================================================================================

/// **A job's terminal outcome (the `job.done` payload, arch 03 §1.1 / §2).** References-not-payloads:
/// `result_refs` are canonical scoped `ArtifactRef`s from result publishers, never log bytes
/// or a PII body. `passed` is the pass/fail the parked workflow's DAG proceeds on. `usage` and
/// `timed_out` are bounded accounting/verdict metadata derived by the sandbox, not caller input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalReport {
    /// Whether the job passed (the DAG-walk decision the workflow resumes on).
    pub passed: bool,
    /// Whether the sandbox deadline terminated the job. A timed-out job can never pass.
    pub timed_out: bool,
    /// Actual resource usage measured by the sandbox backend for terminal accounting.
    pub usage: ResourceUsage,
    /// The job's canonical scoped result refs (references-not-payloads, §3.4). The CI-P20 firehose
    /// owns its deeper log pointer; this field never contains log bytes.
    pub result_refs: Vec<ArtifactRef>,
}

/// Claimed authority for one terminal delivery. Keeping these fields together prevents callers from
/// omitting or reordering an owner/epoch/nonce component at the reporting boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionClaim {
    pub tenant: TenantId,
    pub run: RunId,
    pub job_id: String,
    pub idem_token: String,
    pub lease_owner: String,
    pub lease_epoch: i64,
    pub claim_nonce: String,
}

/// **The terminal-report sink — the runner ECHOES `job.done` through it (the ONE signal path).** A
/// thin seam so the runner depends on an abstraction, with the production impl
/// ([`EngineTerminalReporter`]) routing onto the ENGINE's [`DurableExecutor::signal`] — there is NO
/// second signal mechanism. The runner delivers `signal_name = JOB_DONE_SIGNAL`, `idem_key =
/// idem_token`; the engine's `INSERT … ON CONFLICT DO NOTHING` makes a double-delivery wake once.
pub trait TerminalReporter {
    /// Report `job.done`, carrying the CLAIMED job's durable identity — `tenant` + `job_id` (the leased
    /// `job_queue` row's authority) alongside `run` + `idem_token` (the workflow-minted dispatch token
    /// the `idem_key` echoes). A verifying reporter (the CI pipeline reporter) checks
    /// `(tenant, run, job_id, idem_token)` against the durable dispatch record BEFORE it signals a
    /// verdict, so a caller cannot forge a completion for a job it does not own; a generic reporter
    /// ([`EngineTerminalReporter`]) delivers straight through. The runner passes the CLAIMED row's
    /// `tenant`/`job_id` — never a value derived from the result refs. Returns whether THIS delivery was
    /// the FIRST ([`SignalOutcome::Buffered`]), a DUPLICATE ([`SignalOutcome::Duplicate`]), or an
    /// acknowledged late-completion to an already-terminal run ([`SignalOutcome::TerminalNoOp`]) — all
    /// `Ok`, so the runner settles its lease on any of them. [`ExecutorError`] surfaces a delivery to a
    /// phantom run or a refused forgery (never silently dropped).
    fn report_done(
        &self,
        claim: &CompletionClaim,
        report: &TerminalReport,
    ) -> Result<SignalOutcome, ExecutorError>;
}

/// **The production terminal reporter — `job.done` onto the ENGINE's [`DurableExecutor::signal`]
/// (recon §OQ-F, contracts 9.2/9.4).** Wraps any [`DurableExecutor`] (the real
/// [`myelin_flow::FlowExecutor`]); `report_done` builds the [`SignalSpec`] with the FROZEN
/// `JOB_DONE_SIGNAL` name and `idem_key = idem_token` and delivers it. The exactly-once wake is the
/// engine's `wf_signal` PK / ON CONFLICT DO NOTHING — the runner reuses it, never a fork.
pub struct EngineTerminalReporter<E: DurableExecutor> {
    executor: E,
}

impl<E: DurableExecutor> EngineTerminalReporter<E> {
    /// Build a reporter over the durable executor the parked workflow runs on (the SAME executor that
    /// buffers + consumes the `job.done` signal — one signal path).
    pub fn new(executor: E) -> Self {
        Self { executor }
    }
}

impl<E: DurableExecutor> TerminalReporter for EngineTerminalReporter<E> {
    fn report_done(
        &self,
        claim: &CompletionClaim,
        report: &TerminalReport,
    ) -> Result<SignalOutcome, ExecutorError> {
        // The generic reporter is already bound to one tenant's executor and performs no
        // claimed-job verification or claim consumption (that is the CI pipeline reporter's job);
        // `tenant`/`job_id`/`lease_owner`/`lease_epoch` are the claim authority a VERIFYING reporter
        // consumes, ignored here.
        // The ONE signal path: deliver `job.done` keyed (run, JOB_DONE_SIGNAL, idem_token) — the
        // engine's INSERT … ON CONFLICT (tenant, run_id, signal_name, idem_key) DO NOTHING IS the
        // exactly-once wake. The runner can deliver this TWICE (at-least-once); the engine buffers
        // ONCE; the workflow wakes ONCE. The payload is references-not-payloads (the result refs);
        // `passed` rides as a leading marker ref so the resumed body reads the DAG decision without
        // a PII body (the full structured result is the CI-P20 pointer the refs name).
        let mut payload = Vec::with_capacity(report.result_refs.len() + 1);
        payload.push(ArtifactRef(format!(
            "myelin://job-done/passed-{}",
            report.passed
        )));
        payload.extend(report.result_refs.iter().cloned());
        self.executor.signal(SignalSpec {
            run: claim.run.clone(),
            signal_name: JOB_DONE_SIGNAL.to_string(),
            idem_key: claim.idem_token.clone(),
            payload,
            payload_key_ref: None,
        })
    }
}

// =================================================================================================
// The runner agent — claim → launch → heartbeat → terminal report.
// =================================================================================================

/// **The runner agent's view of a completed claim-to-terminal cycle.** Records what happened so a
/// caller (and a test) sees the claim landed, the sandbox launched + was torn down, the firehose
/// streamed, and the terminal report's idempotency outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunOutcome {
    /// The job that was claimed + run.
    pub job_id: String,
    /// The run the `job.done` was reported to.
    pub run_id: String,
    /// The terminal report delivered.
    pub report: TerminalReport,
    /// Whether the terminal report's `job.done` delivery was the FIRST wake (`Buffered`) or a
    /// DUPLICATE (`Duplicate`) — a re-report wakes the workflow ONCE (the engine dedup).
    pub signal_outcome: SignalOutcome,
    /// The claim generation (`lease_epoch`) this cycle completed under — carried so an at-least-once
    /// re-delivery ([`RunnerAgent::report_done_again`]) presents the SAME claim the CAS already
    /// consumed (its receipt is idempotent evidence), never a fresh/forged generation.
    pub lease_epoch: i64,
    /// Opaque claim nonce echoed on an exact terminal-report retry.
    pub claim_nonce: String,
}

/// An error a runner cycle can surface — loud, never swallowed (EI-02 §4). A backend launch failure
/// and a phantom-run terminal report are both observable.
#[derive(Debug)]
pub enum RunnerError {
    /// No claimable job for the runner's labels in its region (the long-poll found no work).
    NoWork,
    /// The sandbox backend failed to launch the job (fail-closed — the four-guarantee hooks refused,
    /// or the boot failed). Carries a self-describing message.
    LaunchFailed(String),
    /// The lease was LOST mid-run (a heartbeat was refused — another worker reclaimed it after this
    /// runner stalled). The runner MUST NOT report terminal for a job it no longer holds (the reaper,
    /// CI-P12, re-queued it; a fresh runner owns it now).
    LeaseLost {
        /// the job whose lease lapsed.
        job_id: String,
    },
    /// The terminal report failed (a `job.done` to a phantom run — surfaced, never dropped).
    ReportFailed(ExecutorError),
}

impl std::fmt::Display for RunnerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunnerError::NoWork => write!(f, "no claimable job for the runner's labels/region"),
            RunnerError::LaunchFailed(m) => write!(f, "sandbox launch failed (fail-closed): {m}"),
            RunnerError::LeaseLost { job_id } => write!(
                f,
                "lease LOST mid-run for job {job_id} — another worker reclaimed it (the reaper \
                 re-queued it; this runner must not report terminal)"
            ),
            RunnerError::ReportFailed(e) => write!(f, "terminal job.done report failed: {e}"),
        }
    }
}

impl std::error::Error for RunnerError {}

/// **The runner agent (the small attested Rust binary's core, arch 00 §4).** Holds its identity
/// (`worker_id`), its labels + admitted trust tiers + region (the claim predicates), the lease TTL,
/// and the seams it drives: the [`JobLeaseStore`] (claim/heartbeat/settle), the [`SandboxBackend`]
/// (launch/kill — CI-P1/CI-P2, NOT reimplemented), the [`FirehoseSink`] (frame streaming — CI-P20
/// stub), and the [`TerminalReporter`] (the `job.done` echo onto the engine — the one signal path).
///
/// **Same binary hosted + self-hosted.** The self-hosted attestation gate + the tenant-scoped token
/// mint is CI-P4 (→ P-240); here the runner CONSUMES the minted token off `JobSpec.run_token` (4.7).
pub struct RunnerAgent<
    'a,
    B: SandboxBackend,
    F: FirehoseSink,
    T: TerminalReporter,
    L: LeaseStore = JobLeaseStore,
> {
    /// the runner's worker id (the lease owner + the heartbeat principal).
    worker_id: String,
    /// the runner's labels — a job is claimable iff its labels ⊆ these (affinity).
    labels: Vec<String>,
    /// the trust tiers this runner admits — an untrusted job never reaches a runner that does not
    /// admit its tier (the self-hosted scope is CI-P4).
    allowed_tiers: Vec<crate::TrustTier>,
    /// the runner's region — it claims ONLY in-region jobs (12.4; no global pool).
    region: Region,
    /// the lease TTL seconds — `claim` sets `lease_expires = now + ttl`; `heartbeat` renews it.
    lease_ttl_secs: i64,
    /// the lease PORT (CT-004c.2): the in-memory floor ([`JobLeaseStore`]) OR the durable
    /// `job_queue` store adapted to [`LeaseStore`] — the FOR UPDATE SKIP LOCKED claim + heartbeat
    /// primitive (arch 02 §2.1). The claim's tier/region filter is the port impl's, forwarded
    /// unchanged.
    leases: L,
    /// the unified sandbox backend (CI-P1/CI-P2) — `launch`/`kill`; the runner does NOT reimplement
    /// the sandbox.
    backend: &'a B,
    /// the firehose frame sink (CI-P20 stub).
    firehose: &'a F,
    /// the terminal reporter (the `job.done` echo onto the engine — the one signal path).
    reporter: &'a T,
    /// the four-guarantee hooks passed to every launch (X-6; arch 02 §5.2).
    hooks: RunnerHooks,
}

#[allow(clippy::too_many_arguments)]
impl<'a, B: SandboxBackend, F: FirehoseSink, T: TerminalReporter, L: LeaseStore>
    RunnerAgent<'a, B, F, T, L>
{
    /// Build a runner agent. `hooks` are the four-guarantee wiring seam (reserve/settle 11.7,
    /// attribute 4.7, isolation-floor) every launch drives (X-6). `leases` is any [`LeaseStore`] —
    /// the in-memory floor OR the durable adapter (CT-004c.2); the security body is identical.
    pub fn new(
        worker_id: impl Into<String>,
        labels: Vec<String>,
        allowed_tiers: Vec<crate::TrustTier>,
        region: Region,
        lease_ttl_secs: i64,
        leases: L,
        backend: &'a B,
        firehose: &'a F,
        reporter: &'a T,
        hooks: RunnerHooks,
    ) -> Self {
        Self {
            worker_id: worker_id.into(),
            labels,
            allowed_tiers,
            region,
            lease_ttl_secs,
            leases,
            backend,
            firehose,
            reporter,
            hooks,
        }
    }

    /// The runner's worker id (the lease owner).
    pub fn worker_id(&self) -> &str {
        &self.worker_id
    }

    /// **Claim → launch → heartbeat → terminal report — one runner cycle (arch 00 §4 / §2.1).**
    ///
    /// 1. **Claim** a job for the runner's labels in its region via the `FOR UPDATE SKIP LOCKED`
    ///    handshake ([`JobLeaseStore::claim_for_labels`]) — `Err(NoWork)` if the long-poll finds
    ///    none.
    /// 2. **Heartbeat** to confirm the lease is still held BEFORE launching (a runner that stalled
    ///    between claim and launch must not run a job another worker reclaimed). A refused heartbeat
    ///    is `Err(LeaseLost)` — the reaper (CI-P12) re-queued it.
    /// 3. **Launch → run → collect** the sandbox via [`SandboxBackend::launch`] (CI-P1/CI-P2),
    ///    driving the four-guarantee hooks. `launch` BLOCKS for the in-line compute job and returns a
    ///    [`SandboxLaunch`] carrying the command's [`SandboxResult`](crate::SandboxResult)
    ///    (exit/timeout/usage/captured-streams). The runner ships the captured `stdout`/`stderr`
    ///    through the firehose as redacted frames (CI-P20 stub sink), then **kills** the guest on
    ///    teardown (one-job-per-sandbox, ephemeral). A launch failure is `Err(LaunchFailed)`
    ///    fail-closed (no terminal report — the dispatch activity retries the job, §OQ-F).
    /// 4. **DERIVE + report terminal.** The runner DERIVES the [`TerminalReport`] from the result
    ///    (`passed = result.exit_code == Some(0) && !result.timed_out`; result refs are canonical
    ///    references-not-payloads, NEVER the captured bytes) and delivers `job.done` ECHOING the spec's
    ///    `idem_token` through the
    ///    engine signal path (the exactly-once wake), then **settles** the lease. The [`RunOutcome`]
    ///    carries the derived report + the delivery's idempotency outcome.
    ///
    /// `now` is the runner's clock (epoch seconds). The terminal report is no longer an input
    /// (RESHAPE-001 / CT-001): the runner derives it from the [`SandboxResult`](crate::SandboxResult)
    /// the seam now carries back.
    pub fn run_one(&self, now: i64) -> Result<RunOutcome, RunnerError> {
        // ── Step 1: CLAIM (the FOR UPDATE SKIP LOCKED handshake). Region + label + trust eligible,
        // lease-free (unleased or expired). A live lease another runner holds is skipped.
        let job = self
            .leases
            .claim_for_labels(
                &self.worker_id,
                &self.labels,
                &self.allowed_tiers,
                &self.region,
                now,
                self.lease_ttl_secs,
            )
            .ok_or(RunnerError::NoWork)?;

        // ── Step 2: HEARTBEAT-confirm the lease is still ours before launching untrusted code. A
        // refused renew means we lost the lease (another worker reclaimed it) — STOP, never run a
        // job we no longer hold (the reaper re-queued it; CI-P12).
        let held = self.leases.heartbeat(
            &self.worker_id,
            &job.tenant,
            &job.job_id,
            now,
            self.lease_ttl_secs,
        );
        if !held {
            return Err(RunnerError::LeaseLost {
                job_id: job.job_id.clone(),
            });
        }

        // ── Step 3: LAUNCH → RUN → COLLECT via the unified backend (CI-P1/CI-P2). The four-guarantee
        // hooks fire inside launch (reserve/attribute/isolation-floor → settle); a refusal fails the
        // launch CLOSED — no terminal report, the dispatch activity retries (§OQ-F). The runner does
        // NOT reimplement the sandbox; it drives the seam. `launch` BLOCKS for the in-line compute job
        // and returns the command's result (RESHAPE-001 / CT-001).
        let SandboxLaunch { handle, result } = self
            .backend
            .launch(&job.spec, &self.hooks)
            .map_err(|e| RunnerError::LaunchFailed(e.to_string()))?;

        // Stream the captured guest stdout/stderr through the firehose as redacted frames (CI-P20
        // STUB sink). references-not-payloads at the SIGNAL boundary: the raw bytes go to the firehose
        // (→ the `ci.log.available` pointer), NEVER into the `job.done` engine signal payload. We ship
        // a frame per non-empty stream; the full pipeline (byte-range index, resume cursor) is CI-P20.
        if !result.stdout.is_empty() {
            self.firehose
                .ship_frame(&job.run_id, &job.job_id, &job.tenant, &result.stdout);
        }
        if !result.stderr.is_empty() {
            self.firehose
                .ship_frame(&job.run_id, &job.job_id, &job.tenant, &result.stderr);
        }
        // The job's output is complete — CLOSE the step anchor with the derived verdict, seal the
        // open segment, flush the (job, step, byte-range) index, and emit the `ci.log.available`
        // pointer (the real sink; a no-op in the counting stub). `passed` is derived here (a clean
        // exit that did not time out) — the SAME value the terminal report carries below. Called
        // BEFORE the terminal report so the pointer the report references is durably backed.
        // Idempotent — a re-delivered report path does not re-seal.
        let passed = result.passed();
        self.firehose
            .finish(&job.run_id, &job.job_id, &job.tenant, passed);
        // Re-heartbeat (a long job renews its lease so it does not lapse mid-run).
        self.leases.heartbeat(
            &self.worker_id,
            &job.tenant,
            &job.job_id,
            now,
            self.lease_ttl_secs,
        );

        // Whole-guest kill on teardown (one-job-per-sandbox, ephemeral, never reused — arch 02 §5.3).
        self.backend
            .kill(&handle)
            .map_err(|e| RunnerError::LaunchFailed(e.to_string()))?;

        // ── Step 4: DERIVE the terminal report from the command result, then REPORT TERMINAL.
        // `passed` is derived (NOT an input): a clean exit that did not time out. The firehose owns
        // the durable deep log pointer; it is not a canonical scoped ArtifactRef and therefore is not
        // smuggled through the typed verdict. Concrete artifact publishers can add scoped refs later.
        let report = TerminalReport {
            passed,
            timed_out: result.timed_out,
            usage: result.usage,
            result_refs: vec![],
        };

        // `job.done` ECHOING the spec's idem_token through the ENGINE signal path (the exactly-once
        // wake). The runner can deliver this twice (at-least-once); the engine buffers once; the
        // workflow wakes once. NO second signal path.
        let run = RunId(job.run_id.clone());
        // The runner carries the CLAIMED row's generation (this worker as lease owner + the epoch the
        // claim bumped) so the reporter's completion CAS proves ownership — a stale reaped worker holds
        // a lower epoch and is refused. Never derived from the result refs.
        let claim = CompletionClaim {
            tenant: job.tenant.clone(),
            run,
            job_id: job.job_id.clone(),
            idem_token: job.spec.idem_token.0.clone(),
            lease_owner: self.worker_id.clone(),
            lease_epoch: job.lease_epoch,
            claim_nonce: job.claim_nonce.clone(),
        };
        let outcome = self
            .reporter
            .report_done(&claim, &report)
            .map_err(RunnerError::ReportFailed)?;

        // Settle the lease (remove the claimed row). The engine signal idempotency — not this delete
        // — is what makes the wake exactly-once, so a re-delivered report is still a harmless no-op.
        self.leases.settle(&job.tenant, &job.job_id);

        Ok(RunOutcome {
            job_id: job.job_id,
            run_id: job.run_id,
            report,
            signal_outcome: outcome,
            lease_epoch: job.lease_epoch,
            claim_nonce: job.claim_nonce,
        })
    }

    /// **Report the SAME terminal `job.done` AGAIN (the at-least-once re-delivery, §OQ-F).** A runner
    /// can deliver "done" twice (a retry after an ack it never saw). This re-echoes the SAME
    /// `idem_token`; the engine's ON CONFLICT DO NOTHING makes it a [`SignalOutcome::Duplicate`] — the
    /// workflow wakes ONCE. Used to PROVE double-effect = 0 (the gate). Presents the SAME claim
    /// generation (`lease_owner`/`lease_epoch`) `run_one` completed under — a verifying reporter reads
    /// its recorded completion receipt (idempotent evidence) rather than re-consuming the claim.
    pub fn report_done_again(
        &self,
        claim: &CompletionClaim,
        report: &TerminalReport,
    ) -> Result<SignalOutcome, RunnerError> {
        self.reporter
            .report_done(claim, report)
            .map_err(RunnerError::ReportFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        EgressPolicy, HookError, IdemToken, ImageRef, JobKind, MeterTarget, ReserveHandle,
        ResourceLimits, ResourceUsage, RunTokenRef, SandboxHandle, SandboxLaunch, SandboxResult,
        TrustTier, WorkspaceSpec,
    };
    use myelin_events::MonotonicMinter;
    use myelin_flow::{job_idem_token, FlowExecutor, StartSpec};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }

    fn pinned() -> ImageRef {
        ImageRef::pinned("registry.example/runner@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef").unwrap()
    }
    fn limits() -> ResourceLimits {
        ResourceLimits {
            cpu_millis: 1000,
            mem_bytes: 256 << 20,
            disk_bytes: 1 << 30,
            pids_max: 128,
            timeout_secs: 600,
        }
    }

    fn ci_spec(idem: &str) -> JobSpec {
        JobSpec::new(
            JobKind::Ci,
            pinned(),
            vec!["cargo".into(), "test".into()],
            vec![],
            vec![],
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
            IdemToken(idem.into()),
        )
        .unwrap()
    }

    fn test_hooks() -> RunnerHooks {
        RunnerHooks {
            reserve: Box::new(|m| Ok(ReserveHandle(format!("reserved:{}", m.reserve_id)))),
            settle: Box::new(|_h, _u| Ok(())),
            attribute: Box::new(|_t| Ok(())),
            isolation_floor: Box::new(|_s| Ok(())),
        }
    }

    /// A recording sandbox backend — proves the runner DRIVES the seam (launch + kill), counts
    /// launches/kills, and runs the four-guarantee hooks exactly as a real backend must. NO host-exec
    /// path (no `process::Command`) — the launch trait is the only execution seam (the no-host-exec
    /// lint admits this, 1.6).
    struct RecordingBackend {
        launches: AtomicUsize,
        kills: AtomicUsize,
        fail_launch: bool,
        /// The command result the seam carries back — the runner DERIVES the terminal report from
        /// it (RESHAPE-001 / CT-001). Defaults to a clean pass with a stub stdout frame.
        result: SandboxResult,
    }
    impl Default for RecordingBackend {
        fn default() -> Self {
            Self {
                launches: AtomicUsize::new(0),
                kills: AtomicUsize::new(0),
                fail_launch: false,
                result: SandboxResult {
                    exit_code: Some(0),
                    timed_out: false,
                    usage: ResourceUsage {
                        cpu_seconds: 1,
                        mem_byte_seconds: 1,
                    },
                    stdout: b"<stub guest stdout>".to_vec(),
                    stderr: Vec::new(),
                },
            }
        }
    }
    impl SandboxBackend for RecordingBackend {
        type Error = HookError;
        fn launch(
            &self,
            spec: &JobSpec,
            hooks: &RunnerHooks,
        ) -> Result<SandboxLaunch, Self::Error> {
            if self.fail_launch {
                return Err(HookError("backend refused".into()));
            }
            // Drive the four-guarantee seam exactly as a real backend must (X-6).
            (hooks.isolation_floor)(spec)?;
            (hooks.attribute)(&spec.run_token)?;
            let res = (hooks.reserve)(&spec.meter_to)?;
            (hooks.settle)(&res, self.result.usage)?;
            self.launches.fetch_add(1, Ordering::SeqCst);
            Ok(SandboxLaunch {
                handle: SandboxHandle {
                    guest_id: format!("guest-{}", spec.idem_token.0),
                },
                result: self.result.clone(),
            })
        }
        fn kill(&self, _h: &SandboxHandle) -> Result<(), Self::Error> {
            self.kills.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    /// Build a FlowExecutor with one registered `ci.pipeline` definition + a started run, returning
    /// (executor, run_id). The runner reports `job.done` onto THIS executor — the one signal path.
    fn started_run() -> (FlowExecutor, RunId) {
        let ex = FlowExecutor::new(Arc::new(MonotonicMinter::new()), tenant(), region());
        ex.register_definition("ci.pipeline");
        let run = ex
            .start(StartSpec {
                wf_type: "ci.pipeline".into(),
                input: vec![],
                budget: None,
                idem_key: "ci:run-1".into(),
            })
            .expect("start the ci.pipeline run");
        (ex, run)
    }

    // ───────────────────────────── the lease/heartbeat handshake ─────────────────────────────────

    /// **A claimed lease is renewed by heartbeat; an expired lease is reclaimable (the GATE, arch 02
    /// §2.1).** worker-1 claims a job (lease_expires = now + ttl); a heartbeat RENEWS it; a SECOND
    /// worker cannot claim it while the lease is LIVE (skip-locked); once the lease EXPIRES, worker-2
    /// reclaims it (the reaper seam, CI-P12).
    #[test]
    fn lease_claim_heartbeat_renew_and_expiry_reclaim() {
        let q = JobLeaseStore::new();
        q.enqueue(QueuedJob::new(
            tenant(),
            region(),
            "run-1",
            "job-1",
            vec!["linux".into()],
            ci_spec("idem-1"),
        ));
        let tiers = [TrustTier::Trusted];

        // worker-1 claims at t=1000 with a 30s TTL → lease_expires = 1030.
        let claimed = q
            .claim_for_labels("worker-1", &["linux".into()], &tiers, &region(), 1000, 30)
            .expect("worker-1 claims the eligible job");
        assert_eq!(claimed.job_id, "job-1");
        assert_eq!(q.get(&tenant(), "job-1").unwrap().lease_expires, Some(1030));

        // a heartbeat at t=1020 RENEWS the lease → lease_expires = 1050 (the job did not lapse).
        assert!(q.heartbeat("worker-1", &tenant(), "job-1", 1020, 30));
        assert_eq!(q.get(&tenant(), "job-1").unwrap().lease_expires, Some(1050));

        // worker-2 CANNOT claim it while the lease is LIVE (skip-locked — no double-run).
        assert!(
            q.claim_for_labels("worker-2", &["linux".into()], &tiers, &region(), 1040, 30)
                .is_none(),
            "a live lease is skipped — no two runners run the same job"
        );

        // worker-2's heartbeat on a lease it does NOT own is REFUSED (only the owner renews).
        assert!(
            !q.heartbeat("worker-2", &tenant(), "job-1", 1040, 30),
            "a non-owner cannot heartbeat the lease"
        );

        // once the lease EXPIRES (t > 1050), worker-2 RECLAIMS it (the reaper seam, CI-P12).
        let reclaimed = q
            .claim_for_labels("worker-2", &["linux".into()], &tiers, &region(), 1100, 30)
            .expect("worker-2 reclaims the EXPIRED lease");
        assert_eq!(reclaimed.job_id, "job-1");
        assert_eq!(reclaimed.lease_owner.as_deref(), Some("worker-2"));
        assert_eq!(q.get(&tenant(), "job-1").unwrap().lease_expires, Some(1130));
    }

    /// **The claim is residency/affinity/trust eligible (arch 02 §2.1 / 12.4).** A job out-of-region,
    /// or with labels the runner lacks, or a trust tier the runner does not admit, is NOT claimed.
    #[test]
    fn claim_respects_region_affinity_and_trust() {
        let q = JobLeaseStore::new();
        // out-of-region job — never claimed by an fr-par runner (no global pool, 12.4).
        q.enqueue(QueuedJob::new(
            tenant(),
            Region("de-fra".into()),
            "run-x",
            "job-region",
            vec!["linux".into()],
            ci_spec("i1"),
        ));
        // a job needing a label the runner lacks (affinity).
        q.enqueue(QueuedJob::new(
            tenant(),
            region(),
            "run-y",
            "job-label",
            vec!["gpu".into()],
            ci_spec("i2"),
        ));
        // an untrusted_fork job — a runner admitting only Trusted does not claim it.
        let mut fork_spec = ci_spec("i3");
        fork_spec.trust_tier = TrustTier::UntrustedFork;
        q.enqueue(QueuedJob::new(
            tenant(),
            region(),
            "run-z",
            "job-trust",
            vec!["linux".into()],
            fork_spec,
        ));

        let none = q.claim_for_labels(
            "worker-1",
            &["linux".into()],
            &[TrustTier::Trusted],
            &region(),
            1000,
            30,
        );
        assert!(
            none.is_none(),
            "no eligible job: out-of-region / wrong-label / untrusted are all skipped"
        );

        // a runner that admits the fork tier AND has gpu CAN claim the gpu+fork job.
        q.enqueue(QueuedJob::new(
            tenant(),
            region(),
            "run-ok",
            "job-ok",
            vec!["linux".into()],
            ci_spec("i4"),
        ));
        let ok = q
            .claim_for_labels(
                "worker-2",
                &["linux".into(), "gpu".into()],
                &[TrustTier::Trusted, TrustTier::UntrustedFork],
                &region(),
                1000,
                30,
            )
            .expect("an eligible job is claimable");
        // job-label (gpu/Trusted) sorts before job-ok and job-trust; the broad runner claims the
        // first eligible by deterministic order.
        assert!(["job-label", "job-ok", "job-trust"].contains(&ok.job_id.as_str()));
    }

    // ───────────────────── the runner agent: claim → launch → terminal report ────────────────────

    /// **The runner agent runs a job end-to-end: claim → launch (four guarantees) → kill → terminal
    /// `job.done` (arch 00 §4).** One claim, one launch, one whole-guest kill, frames streamed, and a
    /// FIRST `job.done` delivery (Buffered) that wakes the parked workflow.
    #[test]
    fn runner_agent_claims_launches_and_reports_terminal() {
        let (ex, run) = started_run();
        let idem = job_idem_token(&run.0, "ci.pipeline:0");
        let q = JobLeaseStore::new();
        q.enqueue(QueuedJob::new(
            tenant(),
            region(),
            &run.0,
            "job-1",
            vec!["linux".into()],
            ci_spec(&idem),
        ));

        let backend = RecordingBackend::default();
        let firehose = CountingFirehose::new();
        let reporter = EngineTerminalReporter::new(ex.clone());
        let agent = RunnerAgent::new(
            "worker-1",
            vec!["linux".into()],
            vec![TrustTier::Trusted],
            region(),
            30,
            q.clone(),
            &backend,
            &firehose,
            &reporter,
            test_hooks(),
        );

        let outcome = agent
            .run_one(1000)
            .expect("the runner runs the job and reports terminal");

        assert_eq!(outcome.job_id, "job-1");
        assert_eq!(outcome.run_id, run.0);
        // the runner DERIVED the report from the backend's clean (exit 0) result.
        assert!(
            outcome.report.passed,
            "a clean exit (0, not timed out) derives passed=true"
        );
        assert!(outcome.report.result_refs.is_empty());
        assert_eq!(
            outcome.signal_outcome,
            SignalOutcome::Buffered,
            "the FIRST job.done delivery wakes the parked workflow"
        );
        // exactly one launch + one whole-guest kill (one-job-per-sandbox, ephemeral).
        assert_eq!(backend.launches.load(Ordering::SeqCst), 1);
        assert_eq!(backend.kills.load(Ordering::SeqCst), 1);
        // the firehose streamed the captured stdout as one redacted frame (stderr was empty).
        assert_eq!(firehose.frames_shipped(), 1);
        // the lease was settled (the claimed row removed).
        assert!(
            q.get(&tenant(), "job-1").is_none(),
            "the lease is settled on terminal"
        );
        // the engine buffered EXACTLY ONE job.done for the run.
        assert_eq!(ex.signals().count_for_run(&tenant(), &run.0), 1);
    }

    /// **GATE — a runner that delivers `job.done` TWICE wakes the parked workflow EXACTLY ONCE
    /// (idempotent on `idem_token`; double-effect = 0).** The runner runs the job (first `job.done` =
    /// Buffered), then RE-delivers the SAME terminal report (at-least-once) — the engine's ON CONFLICT
    /// DO NOTHING makes it a Duplicate; the buffered count stays ONE. The exactly-once terminal report
    /// is the engine's signal idempotency, REUSED — no second signal path, no fork.
    #[test]
    fn double_delivered_job_done_wakes_the_workflow_exactly_once() {
        let (ex, run) = started_run();
        let idem = job_idem_token(&run.0, "ci.pipeline:0");
        let q = JobLeaseStore::new();
        q.enqueue(QueuedJob::new(
            tenant(),
            region(),
            &run.0,
            "job-1",
            vec!["linux".into()],
            ci_spec(&idem),
        ));

        let backend = RecordingBackend::default();
        let firehose = CountingFirehose::new();
        let reporter = EngineTerminalReporter::new(ex.clone());
        let agent = RunnerAgent::new(
            "worker-1",
            vec!["linux".into()],
            vec![TrustTier::Trusted],
            region(),
            30,
            q.clone(),
            &backend,
            &firehose,
            &reporter,
            test_hooks(),
        );

        // first cycle: the FIRST job.done is Buffered (the workflow wakes). The runner DERIVES the
        // report from the backend result (no longer an input).
        let first = agent.run_one(1000).expect("first cycle");
        assert_eq!(first.signal_outcome, SignalOutcome::Buffered);

        // the runner RE-delivers the SAME job.done (at-least-once) — keyed on the SAME idem_token,
        // passing the derived report directly (the job row is already settled after run_one).
        let again = agent
            .report_done_again(
                &CompletionClaim {
                    tenant: tenant(),
                    run: run.clone(),
                    job_id: "job-1".into(),
                    idem_token: idem.clone(),
                    lease_owner: "worker-1".into(),
                    lease_epoch: first.lease_epoch,
                    claim_nonce: first.claim_nonce.clone(),
                },
                &first.report,
            )
            .expect("re-delivery is the idempotency working, not an error");
        assert_eq!(
            again,
            SignalOutcome::Duplicate,
            "the SECOND job.done is a no-op (ON CONFLICT DO NOTHING — double-effect = 0)"
        );

        // EXACTLY ONE buffered job.done row — the workflow woke ONCE under at-least-once delivery.
        assert_eq!(
            ex.signals().count_for_run(&tenant(), &run.0),
            1,
            "double-effect = 0: a doubly-delivered job.done buffers ONCE (the workflow wakes once)"
        );
    }

    /// **A launch refusal fails CLOSED — no terminal report (the dispatch activity retries, §OQ-F).**
    /// The backend refuses the launch (a four-guarantee hook could equally refuse); the runner
    /// surfaces `LaunchFailed` and NEVER reports `job.done` (the parked workflow is not falsely woken;
    /// the reaper/retry re-runs it).
    #[test]
    fn a_launch_refusal_fails_closed_with_no_terminal_report() {
        let (ex, run) = started_run();
        let idem = job_idem_token(&run.0, "ci.pipeline:0");
        let q = JobLeaseStore::new();
        q.enqueue(QueuedJob::new(
            tenant(),
            region(),
            &run.0,
            "job-1",
            vec!["linux".into()],
            ci_spec(&idem),
        ));

        let backend = RecordingBackend {
            fail_launch: true,
            ..Default::default()
        };
        let firehose = CountingFirehose::new();
        let reporter = EngineTerminalReporter::new(ex.clone());
        let agent = RunnerAgent::new(
            "worker-1",
            vec!["linux".into()],
            vec![TrustTier::Trusted],
            region(),
            30,
            q.clone(),
            &backend,
            &firehose,
            &reporter,
            test_hooks(),
        );

        let err = agent
            .run_one(1000)
            .expect_err("a launch refusal fails closed");
        assert!(matches!(err, RunnerError::LaunchFailed(_)));
        // NO job.done was reported — the parked workflow is not falsely woken (§OQ-F retry).
        assert_eq!(
            ex.signals().count_for_run(&tenant(), &run.0),
            0,
            "a failed launch reports NO terminal — the dispatch activity retries (no false wake)"
        );
    }

    /// **`NoWork` when no claimable job (the long-poll found nothing).** A runner whose labels/region
    /// match no queued job surfaces `NoWork` (it does not launch or report).
    #[test]
    fn no_claimable_job_surfaces_no_work() {
        let (ex, _run) = started_run();
        let q = JobLeaseStore::new(); // empty queue.
        let backend = RecordingBackend::default();
        let firehose = CountingFirehose::new();
        let reporter = EngineTerminalReporter::new(ex.clone());
        let agent = RunnerAgent::new(
            "worker-1",
            vec!["linux".into()],
            vec![TrustTier::Trusted],
            region(),
            30,
            q,
            &backend,
            &firehose,
            &reporter,
            test_hooks(),
        );
        let err = agent
            .run_one(1000)
            .expect_err("an empty queue surfaces NoWork");
        assert!(matches!(err, RunnerError::NoWork));
        assert_eq!(
            backend.launches.load(Ordering::SeqCst),
            0,
            "nothing launched on NoWork"
        );
    }

    /// **The claimable-depth signal counts in-region label-eligible free leases (the long-poll / the
    /// autoscaler queue-depth source, arch 02 §5.4).** A live-leased job does not count; an expired
    /// one does.
    #[test]
    fn claimable_depth_counts_free_eligible_leases() {
        let q = JobLeaseStore::new();
        q.enqueue(QueuedJob::new(
            tenant(),
            region(),
            "r",
            "j1",
            vec!["linux".into()],
            ci_spec("a"),
        ));
        q.enqueue(QueuedJob::new(
            tenant(),
            region(),
            "r",
            "j2",
            vec!["linux".into()],
            ci_spec("b"),
        ));
        assert_eq!(q.claimable_depth(&["linux".into()], &region(), 1000), 2);

        // claim one → claimable depth drops to 1 (the live lease no longer counts).
        q.claim_for_labels(
            "w1",
            &["linux".into()],
            &[TrustTier::Trusted],
            &region(),
            1000,
            30,
        );
        assert_eq!(q.claimable_depth(&["linux".into()], &region(), 1010), 1);
        // once that lease expires it counts again (reclaimable).
        assert_eq!(q.claimable_depth(&["linux".into()], &region(), 2000), 2);
    }

    /// **The terminal report is references-not-payloads + echoes the idem_token (§3.4 / §OQ-F).** The
    /// buffered `job.done` payload carries the `passed` marker + the result `ArtifactRef`s, NEVER log
    /// bytes; it is keyed on the spec's `idem_token` (the runner echoes it).
    #[test]
    fn terminal_report_is_references_not_payloads_keyed_on_idem_token() {
        let (ex, run) = started_run();
        let idem = job_idem_token(&run.0, "ci.pipeline:0");
        let q = JobLeaseStore::new();
        q.enqueue(QueuedJob::new(
            tenant(),
            region(),
            &run.0,
            "job-1",
            vec!["linux".into()],
            ci_spec(&idem),
        ));
        // a backend whose result is a NON-zero exit + captured stderr — the runner DERIVES
        // passed=false and ships the stderr to the firehose (NOT the signal payload).
        let backend = RecordingBackend {
            result: SandboxResult {
                exit_code: Some(2),
                timed_out: false,
                usage: ResourceUsage {
                    cpu_seconds: 1,
                    mem_byte_seconds: 1,
                },
                stdout: Vec::new(),
                stderr: b"compile error: E0001".to_vec(),
            },
            ..Default::default()
        };
        let firehose = CountingFirehose::new();
        let reporter = EngineTerminalReporter::new(ex.clone());
        let agent = RunnerAgent::new(
            "worker-1",
            vec!["linux".into()],
            vec![TrustTier::Trusted],
            region(),
            30,
            q,
            &backend,
            &firehose,
            &reporter,
            test_hooks(),
        );
        agent.run_one(1000).expect("run");

        // the captured stderr rode the FIREHOSE (one frame), never the signal payload.
        assert_eq!(firehose.frames_shipped(), 1);

        // the buffered job.done is keyed on the idem_token the runner echoed.
        let row = ex
            .signals()
            .get(&tenant(), &run.0, JOB_DONE_SIGNAL, &idem)
            .expect("the job.done buffered under the echoed idem_token");
        // The DERIVED passed marker is present; captured stderr and the firehose-owned deep log pointer
        // never enter the typed signal payload.
        assert_eq!(
            row.payload[0],
            ArtifactRef("myelin://job-done/passed-false".into())
        );
        assert_eq!(row.payload.len(), 1);
        assert_eq!(row.payload_key_ref, None, "no inline PII payload");
        // the raw stderr bytes never appear in the signal payload (references-not-payloads).
        for r in &row.payload {
            assert!(
                !r.0.contains("compile error"),
                "captured stream bytes must NEVER enter the engine signal payload"
            );
        }
    }

    /// **GATE — the runner DERIVES the terminal report from the SandboxResult (RESHAPE-001 / CT-001),
    /// it is no longer an input.** A clean exit (0, not timed out) ⇒ passed=true; a non-zero exit ⇒
    /// passed=false; a timeout ⇒ passed=false (regardless of exit code). This is the seam carrying a
    /// command's outcome back, the defect RESHAPE-001 fixes.
    #[test]
    fn runner_derives_terminal_report_from_the_sandbox_result() {
        fn run_with(result: SandboxResult) -> TerminalReport {
            let (ex, run) = started_run();
            let idem = job_idem_token(&run.0, "ci.pipeline:0");
            let q = JobLeaseStore::new();
            q.enqueue(QueuedJob::new(
                tenant(),
                region(),
                &run.0,
                "job-1",
                vec!["linux".into()],
                ci_spec(&idem),
            ));
            let backend = RecordingBackend {
                result,
                ..Default::default()
            };
            let firehose = CountingFirehose::new();
            let reporter = EngineTerminalReporter::new(ex.clone());
            let agent = RunnerAgent::new(
                "worker-1",
                vec!["linux".into()],
                vec![TrustTier::Trusted],
                region(),
                30,
                q,
                &backend,
                &firehose,
                &reporter,
                test_hooks(),
            );
            agent.run_one(1000).expect("run").report
        }

        fn usage() -> ResourceUsage {
            ResourceUsage {
                cpu_seconds: 1,
                mem_byte_seconds: 1,
            }
        }

        // exit 0, not timed out ⇒ PASS.
        let passed = run_with(SandboxResult {
            exit_code: Some(0),
            timed_out: false,
            usage: usage(),
            stdout: Vec::new(),
            stderr: Vec::new(),
        });
        assert!(passed.passed);
        assert!(!passed.timed_out);
        assert_eq!(passed.usage, usage());
        // exit 1 ⇒ FAIL.
        assert!(
            !run_with(SandboxResult {
                exit_code: Some(1),
                timed_out: false,
                usage: usage(),
                stdout: Vec::new(),
                stderr: Vec::new(),
            })
            .passed
        );
        // timed out ⇒ FAIL even though no exit code (killed by the timeout).
        let timed_out = run_with(SandboxResult {
            exit_code: None,
            timed_out: true,
            usage: usage(),
            stdout: Vec::new(),
            stderr: Vec::new(),
        });
        assert!(!timed_out.passed);
        assert!(timed_out.timed_out);
        assert_eq!(timed_out.usage, usage());
        // timed out with a stale 0 exit ⇒ still FAIL (timeout dominates).
        assert!(
            !run_with(SandboxResult {
                exit_code: Some(0),
                timed_out: true,
                usage: usage(),
                stdout: Vec::new(),
                stderr: Vec::new(),
            })
            .passed
        );
    }
}
