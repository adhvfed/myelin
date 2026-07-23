//! **The CI distributed scheduler — the pull-lease claim + concurrency groups + affinity + the
//! dead-runner reaper (CI-P12 / P-355, M4).**
//!
//! **Owning architecture doc (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md`
//! §2.1 (pull-leasing — the claim query with `FOR UPDATE SKIP LOCKED`, the lease, the heartbeat, the
//! dead-runner reaper), §2.3 (concurrency groups — `deploy:prod` serialize + `pr:web:42`
//! cancel-superseded, affinity labels); `01-tech-and-data-model.md` §3.3 (the scheduler tables —
//! `job_queue` + the claim indexes) + §3.4 (runners). **Contracts consumed:** 11.1 (OLTP — the claim
//! hot path + `FOR UPDATE SKIP LOCKED`), 1.8 (the telemetry signal set — scheduler queue-depth, claim
//! latency, lease-reap count).
//!
//! ## What CI-P12 ships here — the claim's WHOLE intelligence, as ONE query
//! The claim is the scheduler's entire decision (arch 02 §2.1): a runner long-polls and claims the
//! next eligible job via the `jq_claimable` `FOR UPDATE SKIP LOCKED` query. The claim encodes, as
//! predicates in ONE query, every scheduling rule:
//! - **RESIDENCY** — `region = $cell_region` (a runner claims only in-region; no global pool,
//!   residency by construction, arch 00 §5);
//! - **AFFINITY** — `labels <@ runner_labels` (the job's labels are a subset of the runner's);
//! - **TRUST** — `trust_tier = ANY($runner_allowed_tiers)` (an `untrusted_fork` job never reaches a
//!   self-hosted-trusted runner, arch 01 §2 / contract 4.9);
//! - **CONCURRENCY (serialize)** — `NOT EXISTS (… r.concurrency_group = q.concurrency_group AND
//!   r.state='running' AND q.concurrency_group LIKE 'deploy:%')` (one `deploy:prod` at a time, the
//!   `jq_serialize` partial unique index);
//! - **LANES** — `ORDER BY lane_priority(lane) DESC` (interactive > batch > deploy, the
//!   protected-human-lane analogue inside CI, arch 02 §2.3);
//! - **FAIRNESS** — `fair_deficit.deficit DESC` (the DRR term; the deficit ADVANCE/REPLENISH is
//!   CI-P13 — here the claim ORDERS on the term, see the floor);
//! - then **`enqueued_at ASC`** (oldest first within an equal key).
//!
//! On claim the row is leased: `lease_owner` / `lease_expires` set, `state='leased'`. The
//! **dead-runner reaper** sweeps expired leases → re-queues their jobs (`state='queued'`,
//! `lease_owner`/`lease_expires` cleared), which makes the run's `SCHEDULE_AND_RUN_JOB` activity
//! retry idempotently — the enqueue is idempotent on `idem_token` via the `jq_idem` unique, so the
//! re-dispatch is ONE row, never a duplicate.
//!
//! ## Concurrency groups (arch 02 §2.3)
//! - `deploy:prod` is a **serialization key** (the `jq_serialize` partial unique index — at most one
//!   `deploy:%` group running at a time; the claim's `NOT EXISTS` predicate holds it).
//! - `pr:web:42` is **cancel-superseded** ([`cancel_superseded`] — a new push to the PR cancels the
//!   in-flight `queued`/`leased` run for that group so only the latest head is tested).
//!
//! ## DB-free model + the live-stack proof (the binding data-layer policy)
//! This module carries the claim/reaper logic TWICE, in lock-step:
//! - the [`CLAIM_QUERY`] / [`CANCEL_SUPERSEDED_QUERY`] / [`REAP_QUERY`] **`&str` SQL** the live OLTP
//!   path runs (arch 02 §2.1 verbatim intent — the `FOR UPDATE SKIP LOCKED` claim, the serialize
//!   `NOT EXISTS`, the reaper sweep). The REAL apply against the dev-stack Postgres (a real claim
//!   under concurrency, the serialize index, the reaper re-queue) is
//!   `tests/integration_ci_p12_scheduler_claim.rs` behind the `integration` cargo feature;
//! - a **deterministic in-memory model** ([`SchedulerState`]) that implements the IDENTICAL predicate
//!   semantics — so the unit + drill tests (the claim predicates, the reaper re-queue idempotency, the
//!   serialize + cancel-superseded) are deterministic and DB-free, while the live test proves the SQL
//!   carries the same semantics. The two are the same algorithm; neither is a mock of the other.
//!
//! ## Floors named (VISION §3 / the prompt DoD)
//! - the **DRR fair-share** advance/replenish over `fair_key` + the **priority lanes** detail +
//!   **per-tenant backpressure** are **CI-P13** (P-356): this prompt's claim ORDERS on
//!   `fair_deficit.deficit DESC` (the term), but does NOT advance/replenish the counter — that is
//!   CI-P13. The lane ORDER BY is here (the strict precedence); the lane SHED budget under surge is
//!   CI-P13.
//! - the **flat-DRR → hierarchical** (per-tenant → per-project → per-pipeline) scheduler follow-on is
//!   **CI-P29** (measured-starvation-triggered, the per-`fair_key` wait-time histogram signal).

use std::collections::BTreeMap;

pub use myelin_ci_sandbox::TrustTier;

// =================================================================================================
// 1. The live OLTP claim/reaper SQL (arch 02 §2.1, verbatim intent). Held as `&str` so the lints do
//    not mistake the DDL/DML for live Rust; the live integration test runs the IDENTICAL query.
// =================================================================================================

/// **The pull-lease claim (arch 02 §2.1) — the scheduler's whole intelligence as ONE query.** A
/// runner long-polls and claims the highest-priority, fairest, label-eligible, in-region, trust-
/// allowed, non-serialized job via `FOR UPDATE SKIP LOCKED` (so concurrent runners never block each
/// other; each takes a different row). Bind params: `$1 cell_region`, `$2 runner_labels text[]`,
/// `$3 runner_allowed_tiers text[]`, `$4 lease_owner`, `$5 lease_ttl_seconds`.
///
/// The `lane_priority` CASE is the strict lane ORDER (interactive > batch > deploy, arch 02 §2.3);
/// `fair_deficit.deficit DESC` is the DRR fairness term (the ADVANCE is CI-P13); `enqueued_at ASC`
/// breaks the tie oldest-first. The serialize `NOT EXISTS` is the `deploy:%` one-at-a-time hold. On
/// claim the row is updated to `leased` with the lease owner + expiry, and the claimed row returned.
/// The same PostgreSQL statement clock is returned beside the expiry so token issuance can bind a
/// retry-stable absolute lifetime to this exact claim generation without trusting a process clock.
///
/// **`FOR UPDATE OF q` (not a bare `FOR UPDATE`).** The `fair_deficit` join is a READ-ONLY fairness
/// hint (left-joined; a missing deficit row defaults to 0). Postgres refuses `FOR UPDATE` on the
/// nullable side of an outer join (`make_outerjoininfo`), and locking the fairness hint is wrong
/// anyway — only the claimed `job_queue` row must be locked SKIP-LOCKED so concurrent runners take
/// different rows. So the lock is scoped to `q` (the `job_queue` row) explicitly. (Production fix
/// proven by `tests/integration_ci_p12_scheduler_claim.rs` against live Postgres.)
pub const CLAIM_QUERY: &str = "\
WITH eligible AS (
  SELECT q.tenant_id, q.region, q.job_id
  FROM job_queue q
  LEFT JOIN fair_deficit f
    ON f.tenant_id = q.tenant_id AND f.region = q.region AND f.fair_key = q.fair_key
  WHERE q.state = 'queued'
    AND q.region = $1
    AND q.labels <@ $2
    AND q.trust_tier = ANY($3)
    AND (
      q.concurrency_group IS NULL
      OR q.concurrency_group NOT LIKE 'deploy:%'
      OR NOT EXISTS (
        SELECT 1 FROM job_queue r
        WHERE r.tenant_id = q.tenant_id
          AND r.concurrency_group = q.concurrency_group
          AND r.state = 'running'
      )
    )
  ORDER BY
    CASE q.lane WHEN 'interactive' THEN 2 WHEN 'batch' THEN 1 ELSE 0 END DESC,
    COALESCE(f.deficit, 0) DESC,
    q.enqueued_at ASC
  FOR UPDATE OF q SKIP LOCKED
  LIMIT 1
)
UPDATE job_queue j
SET state = 'leased',
    lease_owner = $4,
    lease_expires = statement_timestamp() + ($5 || ' seconds')::interval,
    lease_epoch = j.lease_epoch + 1,
    claim_nonce = gen_random_uuid(),
    claim_started_at = statement_timestamp(),
    claim_expires_at = statement_timestamp() + ($5 || ' seconds')::interval
FROM eligible e
WHERE j.tenant_id = e.tenant_id AND j.job_id = e.job_id
RETURNING j.tenant_id, j.job_id, j.run_id, j.lane, j.concurrency_group, j.fair_key, j.trust_tier,
          j.lease_epoch, j.claim_nonce::text AS claim_nonce,
          EXTRACT(EPOCH FROM j.claim_started_at)::bigint AS claim_started_at_epoch_secs,
          EXTRACT(EPOCH FROM j.claim_expires_at)::bigint AS claim_expires_at_epoch_secs";

/// **Cancel-superseded (arch 02 §2.3) — a new push to a PR cancels the in-flight run for that group.**
/// On a new enqueue for a `pr:%` concurrency group, the prior `queued`/`leased` rows for that group
/// are moved to `terminal` so only the latest head is tested. Bind: `$1 tenant_id`, `$2 region`,
/// `$3 concurrency_group`, `$4 keep_job_id` (the new head, never cancelled).
pub const CANCEL_SUPERSEDED_QUERY: &str = "\
UPDATE job_queue
SET state = 'terminal', lease_owner = NULL, lease_expires = NULL
WHERE tenant_id = $1
  AND region = $2
  AND concurrency_group = $3
  AND state IN ('queued', 'leased')
  AND job_id <> $4
RETURNING job_id";

/// **The dead-runner reaper (arch 02 §2.1) — sweep expired leases → re-queue.** A runner that died
/// mid-lease or after the final launch fence leaves a `leased`/`running` row whose `lease_expires`
/// has passed; the reaper moves it back to
/// `queued` (clearing the lease) so it is claimable again. The re-queue is idempotent (the same row
/// returns to `queued`); the run's `SCHEDULE_AND_RUN_JOB` activity re-dispatch is ONE row (the
/// `jq_idem` unique on `(tenant_id, idem_token)` rejects a duplicate enqueue). Bind: `$1 region`.
pub const REAP_QUERY: &str = "\
WITH candidates AS MATERIALIZED (
  SELECT tenant_id, region, job_id, state, lease_epoch, claim_nonce
  FROM job_queue
  WHERE region = $1
    AND state IN ('leased', 'running')
    AND lease_expires < now()
  FOR UPDATE SKIP LOCKED
),
expired AS (
  SELECT tenant_id, job_id
  FROM candidates
  WHERE state = 'leased'
     OR (
       state = 'running'
       AND pg_try_advisory_xact_lock(
         hashtextextended(
           jsonb_build_array(
             tenant_id::text,
             region::text,
             job_id::text,
             lease_epoch::text,
             claim_nonce::text
           )::text,
           0
         )
       )
     )
)
UPDATE job_queue j
SET state = 'queued', lease_owner = NULL, lease_expires = NULL, claim_nonce = NULL
FROM expired e
WHERE j.tenant_id = e.tenant_id AND j.job_id = e.job_id
RETURNING j.tenant_id, j.job_id";

/// **Final exact-generation launch fence.** Atomically move one still-live scheduler generation
/// from `leased` to `running` immediately before sandbox spawn. Cancellation and this CAS serialize
/// on the same row: cancellation winning first makes this match zero rows; this winning first makes
/// the row ineligible for cancel-superseded. Every persisted generation fact is compared, including
/// the original (heartbeat-independent) claim timestamps. The successful CAS installs a fresh
/// execution lease covering the admitted runtime while a session advisory lock protects the much
/// smaller commit→gated-release interval. Bind: `$1 tenant`, `$2 region`, `$3 job`, `$4 workflow
/// run`, `$5 owner`, `$6 epoch`, `$7 nonce`, `$8 claim start`, `$9 claim expiry`, `$10 execution
/// lease seconds`.
pub const AUTHORIZE_JOB_LAUNCH_QUERY: &str = "\
UPDATE job_queue
SET state = 'running',
    lease_expires = statement_timestamp() + ($10 || ' seconds')::interval
WHERE tenant_id = $1
  AND region = $2
  AND job_id = $3::uuid
  AND run_id = $4::uuid
  AND state = 'leased'
  AND lease_owner = $5
  AND lease_epoch = $6
  AND claim_nonce = $7::uuid
  AND EXTRACT(EPOCH FROM claim_started_at)::bigint = $8
  AND EXTRACT(EPOCH FROM claim_expires_at)::bigint = $9
  AND claim_expires_at > statement_timestamp()
  AND completion_receipt IS NULL
RETURNING job_id";

/// **The idempotent enqueue (arch 02 §2.1 / §3.2) — insert ONE schedulable `job_queue` row.** The
/// durable equivalent of [`SchedulerState::enqueue`]: a job the run's `SCHEDULE_AND_RUN_JOB`
/// activity dispatches becomes a `queued` row. Idempotent on the `jq_idem` unique
/// `(tenant_id, idem_token)` via `ON CONFLICT … DO NOTHING`, so a reaper re-queue + a redundant
/// re-dispatch of the same `(tenant_id, idem_token)` is ONE row, never a duplicate (0 duplicate
/// enqueues — the CI-D1 effectively-once floor). `enqueued_at` defaults to `now()` (the claim's
/// oldest-first tie-break). Bind: `$1 tenant_id`, `$2 region`, `$3 job_id` (uuid), `$4 run_id`
/// (uuid), `$5 lane`, `$6 labels text[]`, `$7 trust_tier`, `$8 concurrency_group` (nullable),
/// `$9 fair_key`, `$10 idem_token`, `$11 stage`. `RETURNING job_id` is present iff the row was inserted (absent on
/// the idempotent conflict) — so the store reads INSERTED vs DUPLICATE from the returned-row count.
pub const INSERT_JOB_QUEUE_QUERY: &str = "\
INSERT INTO job_queue
  (tenant_id, region, job_id, run_id, lane, labels, trust_tier, concurrency_group, fair_key, idem_token, stage, state)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, 'queued')
ON CONFLICT (tenant_id, idem_token) DO NOTHING
RETURNING job_id";

/// **Complete a job — move it to `terminal` (the runner reported `job.done`, arch 02 §3.2).** The
/// durable equivalent of [`SchedulerState::complete_job`]: a `leased`/`running`/`queued` job whose
/// runner finished is moved to `terminal` (clearing the lease) so the reaper NEVER re-queues a
/// completed job (re-queuing a done job would double-run it). Idempotent: a re-complete of an
/// already-`terminal` row matches nothing (`state <> 'terminal'`) → 0 rows, the `job.done` side of
/// the effectively-once invariant (a double-delivered `job.done` terminates the row ONCE). Bind:
/// `$1 tenant_id`, `$2 job_id`. `RETURNING job_id` iff this call moved the row.
pub const COMPLETE_JOB_QUERY: &str = "\
UPDATE job_queue
SET state = 'terminal', lease_owner = NULL, lease_expires = NULL
WHERE tenant_id = $1
  AND job_id = $2
  AND state <> 'terminal'
RETURNING job_id";

/// **Heartbeat — a LIVE runner extends its lease (arch 02 §2.1).** The durable equivalent of
/// [`SchedulerState::heartbeat`]: while a job is `leased`/`running` by this owner, push
/// `lease_expires` forward
/// so a heart-beating runner is NOT swept by the [`REAP_QUERY`] (only a DEAD runner's expired lease
/// is reaped). Guarded to the lease OWNER (`lease_owner = $3`) so a stale/other runner cannot extend
/// a lease it does not hold. Bind: `$1 tenant_id`, `$2 job_id`, `$3 lease_owner`,
/// `$4 extend_seconds`. `RETURNING job_id` iff the lease was extended.
pub const HEARTBEAT_QUERY: &str = "\
UPDATE job_queue
SET lease_expires = now() + ($4 || ' seconds')::interval
WHERE tenant_id = $1
  AND job_id = $2
  AND state IN ('leased', 'running')
  AND lease_owner = $3
RETURNING job_id";

/// **The atomic prove-and-consume completion CAS (CT-004d.2 claim-bound completion).** Before any
/// verdict is signalled, the terminal reporter consumes the CLAIM: it moves the row to `terminal` and
/// records the deterministic `completion_receipt` ONLY IF the presented claim generation matches the
/// row's — `lease_owner = $3 AND lease_epoch = $4` — and the exact launch CAS already moved that
/// generation to `running`. A merely leased claimant cannot invent a terminal result for work that
/// never executed. A stale worker (reaped + re-claimed → higher epoch, or a different owner) matches
/// 0 rows and is refused; a forger with a valid token but no running claim matches 0 rows. Bind:
/// `$1 tenant_id`, `$2 job_id`, `$3 lease_owner`, `$4 lease_epoch`, `$5 claim_nonce`,
/// `$6 completion_receipt`, `$7 stage`. `RETURNING job_id` iff THIS call consumed the claim. The
/// receipt is idempotent evidence an exact redelivery reads.
pub const CONSUME_CLAIM_QUERY: &str = "\
UPDATE job_queue
SET state = 'terminal', completion_receipt = $6, lease_owner = NULL, lease_expires = NULL
WHERE tenant_id = $1
  AND job_id = $2
  AND lease_owner = $3
  AND lease_epoch = $4
  AND claim_nonce = $5::uuid
  AND stage = $7
  AND state = 'running'
  AND completion_receipt IS NULL
RETURNING job_id";

/// **Read a job's terminal disposition for the completion CAS's 0-row branch.** When
/// [`CONSUME_CLAIM_QUERY`] consumes nothing, this distinguishes an IDEMPOTENT redelivery (the row is
/// already `terminal` with the SAME `completion_receipt`) from a fail-closed REFUSAL (missing row,
/// stale claim generation, or a divergent receipt — e.g. a flipped-verdict replay). Bind:
/// `$1 tenant_id`, `$2 job_id`.
pub const READ_COMPLETION_DISPOSITION_QUERY: &str = "\
SELECT state, completion_receipt FROM job_queue WHERE tenant_id = $1 AND job_id = $2";

// =================================================================================================
// 2. The deterministic in-memory model — the IDENTICAL claim/reaper semantics, DB-free, so the unit
//    + drill tests are deterministic. The live SQL above carries the same algorithm against Postgres.
// =================================================================================================

/// The three lanes (arch 02 §2.3), strict precedence interactive > batch > deploy. The claim ORDER
/// BY's `lane_priority(lane) DESC` is exactly this enum's `priority()` (higher = claimed first).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    /// PR-check feedback — the protected-human lane; must never queue behind a batch matrix.
    Interactive,
    /// A nightly/batch matrix.
    Batch,
    /// A deploy job (serialized via the `deploy:%` concurrency group).
    Deploy,
}

impl Lane {
    /// The strict lane priority (higher = claimed first): interactive(2) > batch(1) > deploy(0). This
    /// is the `CASE … DESC` term in [`CLAIM_QUERY`] — interactive feedback never queues behind batch.
    pub fn priority(self) -> i32 {
        match self {
            Lane::Interactive => 2,
            Lane::Batch => 1,
            Lane::Deploy => 0,
        }
    }

    /// The `job_queue.lane` CHECK-constraint string value (arch 01 §3.3).
    pub fn as_str(self) -> &'static str {
        match self {
            Lane::Interactive => "interactive",
            Lane::Batch => "batch",
            Lane::Deploy => "deploy",
        }
    }

    /// Parse a `job_queue.lane` CHECK token back to a [`Lane`] (the read-back side of [`Lane::as_str`]).
    /// `None` for a token outside the frozen three-lane set — a corrupt durable row the store surfaces
    /// loudly (never silently coerced), the same posture as the metering store's meter/kind parse.
    pub fn from_token(token: &str) -> Option<Lane> {
        match token {
            "interactive" => Some(Lane::Interactive),
            "batch" => Some(Lane::Batch),
            "deploy" => Some(Lane::Deploy),
            _ => None,
        }
    }
}

/// The `job_queue.state` lifecycle (arch 01 §3.3 CHECK): `queued` → `leased` (claimed) → `running`
/// (runner started) → `terminal` (done/cancelled). The reaper moves an expired `leased` back to
/// `queued`; cancel-superseded moves `queued`/`leased` to `terminal`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobState {
    /// Claimable.
    Queued,
    /// Claimed by a runner; holds a lease (`lease_owner` + `lease_expires`).
    Leased,
    /// The runner has started executing.
    Running,
    /// Done or cancelled — no longer schedulable.
    Terminal,
}

impl JobState {
    fn as_str(self) -> &'static str {
        match self {
            JobState::Queued => "queued",
            JobState::Leased => "leased",
            JobState::Running => "running",
            JobState::Terminal => "terminal",
        }
    }
}

/// A schedulable `job_queue` row (the in-memory mirror of the live table — the SAME columns the
/// claim orders on). PII-free: every field is an opaque id / label / vocabulary token.
#[derive(Clone, Debug)]
pub struct QueuedJob {
    /// The tenant partition (the first PK component; never crossed by a query path).
    pub tenant_id: String,
    /// The residency region — a runner claims only in-region (no global pool).
    pub region: String,
    /// The opaque job id (the `(tenant_id, job_id)` PK).
    pub job_id: String,
    /// The owning run id.
    pub run_id: String,
    /// The lane (the strict ORDER BY term).
    pub lane: Lane,
    /// The affinity labels — a job is claimable iff `labels ⊆ runner_labels`.
    pub labels: Vec<String>,
    /// The trust tier — a job is claimable iff `trust_tier ∈ runner_allowed_tiers`.
    pub trust_tier: TrustTier,
    /// The concurrency group (`deploy:prod` serialize, `pr:web:42` cancel-superseded) or `None`.
    pub concurrency_group: Option<String>,
    /// The DRR fairness key (`tenant` or `tenant:project`).
    pub fair_key: String,
    /// The idempotency token — the `jq_idem` unique `(tenant_id, idem_token)` makes a re-enqueue a
    /// no-op (the reaper re-dispatch is ONE row, never a duplicate).
    pub idem_token: String,
    /// A monotonic enqueue order (the `enqueued_at ASC` tie-break; lower = older).
    pub enqueued_seq: u64,
    /// The lifecycle state.
    pub state: JobState,
    /// The lease owner (a runner id) while `leased`/`running`, else `None`.
    pub lease_owner: Option<String>,
    /// The logical lease-expiry tick (compared against the reaper's `now`), else `None`.
    pub lease_expires: Option<u64>,
}

impl QueuedJob {
    /// A fresh `queued` job (the enqueue shape). `enqueued_seq` is the monotonic order the claim
    /// breaks ties on (`enqueued_at ASC`). The arguments mirror the `job_queue` columns the claim
    /// orders on; each is load-bearing (the claim filters/orders on every one), so
    /// `too_many_arguments` is allowed (the same posture as the search pipeline/cache constructors).
    #[allow(clippy::too_many_arguments)]
    pub fn enqueued(
        tenant_id: impl Into<String>,
        region: impl Into<String>,
        job_id: impl Into<String>,
        run_id: impl Into<String>,
        lane: Lane,
        trust_tier: TrustTier,
        fair_key: impl Into<String>,
        idem_token: impl Into<String>,
        enqueued_seq: u64,
    ) -> Self {
        QueuedJob {
            tenant_id: tenant_id.into(),
            region: region.into(),
            job_id: job_id.into(),
            run_id: run_id.into(),
            lane,
            labels: Vec::new(),
            trust_tier,
            concurrency_group: None,
            fair_key: fair_key.into(),
            idem_token: idem_token.into(),
            enqueued_seq,
            state: JobState::Queued,
            lease_owner: None,
            lease_expires: None,
        }
    }

    /// Builder: set the affinity labels.
    pub fn with_labels(mut self, labels: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.labels = labels.into_iter().map(Into::into).collect();
        self
    }

    /// Builder: set the concurrency group (`deploy:prod` serialize / `pr:web:42` cancel-superseded).
    pub fn with_concurrency_group(mut self, group: impl Into<String>) -> Self {
        self.concurrency_group = Some(group.into());
        self
    }
}

/// The runner's claim filter (arch 02 §2.1 bind params): the cell region (residency), the runner's
/// labels (affinity: the job's labels must be a subset), the trust tiers the runner is allowed to
/// execute (an untrusted-fork job never reaches a trusted self-hosted runner), and the lease the
/// runner takes on claim (owner + TTL).
#[derive(Clone, Debug)]
pub struct ClaimRequest {
    /// The runner's cell region — only in-region jobs are claimable (no global pool).
    pub cell_region: String,
    /// The runner's labels — a job is claimable iff its labels are a SUBSET of these.
    pub runner_labels: Vec<String>,
    /// The trust tiers this runner may execute — a job is claimable iff its tier is one of these.
    pub runner_allowed_tiers: Vec<TrustTier>,
    /// The runner id recorded as the lease owner on claim.
    pub lease_owner: String,
    /// The lease TTL (in logical ticks) — `lease_expires = now + lease_ttl`.
    pub lease_ttl: u64,
}

/// The result of a successful claim — the leased job's identity + the scheduling terms it was
/// claimed on (so a caller / drill can assert WHY this job won the claim).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Claimed {
    /// The claimed job's tenant.
    pub tenant_id: String,
    /// The claimed job id.
    pub job_id: String,
    /// The owning run.
    pub run_id: String,
    /// The lane it was claimed in.
    pub lane: Lane,
    /// The concurrency group (if any).
    pub concurrency_group: Option<String>,
}

/// The deterministic scheduler model: the `job_queue` rows + the per-`fair_key` deficit (read by the
/// claim's DRR term; the ADVANCE is CI-P13). `now` is a logical clock the reaper compares lease
/// expiries against — DB-free + deterministic, the SAME predicate semantics as the live SQL.
#[derive(Clone, Debug, Default)]
pub struct SchedulerState {
    jobs: Vec<QueuedJob>,
    /// The per-`(tenant, region, fair_key)` DRR deficit (the claim ORDER reads it; CI-P13 advances).
    fair_deficit: BTreeMap<(String, String, String), i64>,
    /// The logical clock (reaper compares `lease_expires < now`).
    now: u64,
}

/// Why an enqueue was rejected (the idempotency floor — `jq_idem` unique on `(tenant_id,
/// idem_token)`). A duplicate enqueue is a NO-OP, never a second row (the reaper re-dispatch relies
/// on this: re-queueing a reaped job, then a redundant `SCHEDULE_AND_RUN_JOB` retry, is one row).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnqueueOutcome {
    /// The job was inserted (a new `(tenant_id, idem_token)`).
    Inserted,
    /// A row with the same `(tenant_id, idem_token)` already exists — the insert was a no-op
    /// (idempotent enqueue, the `jq_idem` unique).
    DuplicateIdem,
}

impl SchedulerState {
    /// A fresh empty scheduler at logical tick 0.
    pub fn new() -> Self {
        SchedulerState::default()
    }

    /// The current logical tick.
    pub fn now(&self) -> u64 {
        self.now
    }

    /// Advance the logical clock by `ticks` (so a lease can expire for the reaper drill).
    pub fn advance(&mut self, ticks: u64) {
        self.now += ticks;
    }

    /// Set the DRR deficit for a `fair_key` (CI-P13 owns the ADVANCE/REPLENISH; the claim here only
    /// ORDERS on it — this lets the unit test prove the claim's DRR ordering term in isolation).
    pub fn set_deficit(&mut self, tenant_id: &str, region: &str, fair_key: &str, deficit: i64) {
        self.fair_deficit.insert(
            (
                tenant_id.to_string(),
                region.to_string(),
                fair_key.to_string(),
            ),
            deficit,
        );
    }

    fn deficit_of(&self, job: &QueuedJob) -> i64 {
        self.fair_deficit
            .get(&(
                job.tenant_id.clone(),
                job.region.clone(),
                job.fair_key.clone(),
            ))
            .copied()
            .unwrap_or(0)
    }

    /// All jobs (read-only) — for assertions/telemetry (the scheduler queue-depth signal, 1.8).
    pub fn jobs(&self) -> &[QueuedJob] {
        &self.jobs
    }

    fn find(&self, tenant_id: &str, job_id: &str) -> Option<usize> {
        self.jobs
            .iter()
            .position(|j| j.tenant_id == tenant_id && j.job_id == job_id)
    }

    /// **Enqueue a job, idempotent on `(tenant_id, idem_token)` (the `jq_idem` unique).** A duplicate
    /// idem token is a NO-OP — this is the floor the reaper re-dispatch + the
    /// `SCHEDULE_AND_RUN_JOB` retry rely on (re-queue then redundant retry = ONE row).
    pub fn enqueue(&mut self, job: QueuedJob) -> EnqueueOutcome {
        let dup = self
            .jobs
            .iter()
            .any(|j| j.tenant_id == job.tenant_id && j.idem_token == job.idem_token);
        if dup {
            return EnqueueOutcome::DuplicateIdem;
        }
        self.jobs.push(job);
        EnqueueOutcome::Inserted
    }

    /// **Enqueue with cancel-superseded for a `pr:%` group (arch 02 §2.3).** Enqueue the new head,
    /// then cancel the prior `queued`/`leased` rows of the same concurrency group (so only the latest
    /// head is tested). Returns the enqueue outcome; the cancellation is applied as a side effect (it
    /// mirrors [`CANCEL_SUPERSEDED_QUERY`] run on the new enqueue).
    pub fn enqueue_superseding(&mut self, job: QueuedJob) -> EnqueueOutcome {
        let group = job.concurrency_group.clone();
        let new_job_id = job.job_id.clone();
        let tenant = job.tenant_id.clone();
        let region = job.region.clone();
        let outcome = self.enqueue(job);
        if outcome == EnqueueOutcome::Inserted {
            if let Some(group) = group {
                self.cancel_superseded(&tenant, &region, &group, &new_job_id);
            }
        }
        outcome
    }

    /// **Cancel-superseded ([`CANCEL_SUPERSEDED_QUERY`]): move the prior `queued`/`leased` rows of a
    /// concurrency group to `terminal`, keeping `keep_job_id`.** Returns the cancelled job ids.
    pub fn cancel_superseded(
        &mut self,
        tenant_id: &str,
        region: &str,
        group: &str,
        keep_job_id: &str,
    ) -> Vec<String> {
        let mut cancelled = Vec::new();
        for j in &mut self.jobs {
            if j.tenant_id == tenant_id
                && j.region == region
                && j.concurrency_group.as_deref() == Some(group)
                && j.job_id != keep_job_id
                && matches!(j.state, JobState::Queued | JobState::Leased)
            {
                j.state = JobState::Terminal;
                j.lease_owner = None;
                j.lease_expires = None;
                cancelled.push(j.job_id.clone());
            }
        }
        cancelled
    }

    /// Is a `deploy:%` group already running (the serialize `NOT EXISTS` — the `jq_serialize` partial
    /// unique index, at most one running per `deploy:%` group)?
    fn deploy_group_running(&self, tenant_id: &str, group: &str) -> bool {
        self.jobs.iter().any(|j| {
            j.tenant_id == tenant_id
                && j.concurrency_group.as_deref() == Some(group)
                && j.state == JobState::Running
        })
    }

    /// Whether a job is eligible for a given claim request — the EXACT predicate conjunction of
    /// [`CLAIM_QUERY`]'s `WHERE`: queued, in-region, labels ⊆ runner_labels, trust ∈ allowed, and the
    /// `deploy:%` serialize hold.
    fn eligible(&self, job: &QueuedJob, req: &ClaimRequest) -> bool {
        if job.state != JobState::Queued {
            return false;
        }
        // RESIDENCY: in-region only.
        if job.region != req.cell_region {
            return false;
        }
        // AFFINITY: job labels ⊆ runner labels.
        if !job
            .labels
            .iter()
            .all(|l| req.runner_labels.iter().any(|rl| rl == l))
        {
            return false;
        }
        // TRUST: job tier ∈ runner-allowed tiers.
        if !req.runner_allowed_tiers.contains(&job.trust_tier) {
            return false;
        }
        // CONCURRENCY (serialize): a `deploy:%` group with one already running is held.
        if let Some(group) = &job.concurrency_group {
            if group.starts_with("deploy:") && self.deploy_group_running(&job.tenant_id, group) {
                return false;
            }
        }
        true
    }

    /// **The pull-lease claim (arch 02 §2.1; [`CLAIM_QUERY`]).** Pick the single highest-priority,
    /// fairest, label-eligible, in-region, trust-allowed, non-serialized job and lease it. The pick
    /// order is the claim's `ORDER BY`: lane priority DESC, then DRR `deficit` DESC, then
    /// `enqueued_seq` ASC. On claim the row is set `leased` with the lease owner + expiry
    /// (`now + lease_ttl`). Returns `None` if no job is eligible (the runner long-polls again).
    ///
    /// `FOR UPDATE SKIP LOCKED` in the live SQL means concurrent runners never block each other (each
    /// claims a different row); the model is single-threaded, so the claim is a deterministic pick —
    /// the same row the live query would return under no contention.
    pub fn claim(&mut self, req: &ClaimRequest) -> Option<Claimed> {
        let mut best: Option<usize> = None;
        for (i, job) in self.jobs.iter().enumerate() {
            if !self.eligible(job, req) {
                continue;
            }
            best = Some(match best {
                None => i,
                Some(b) => {
                    let cur = &self.jobs[b];
                    // ORDER BY lane_priority DESC, deficit DESC, enqueued_seq ASC.
                    let key_new = (
                        job.lane.priority(),
                        self.deficit_of(job),
                        // enqueued_seq ascending → negate for a single max comparison.
                        -(job.enqueued_seq as i64),
                    );
                    let key_cur = (
                        cur.lane.priority(),
                        self.deficit_of(cur),
                        -(cur.enqueued_seq as i64),
                    );
                    if key_new > key_cur {
                        i
                    } else {
                        b
                    }
                }
            });
        }
        let idx = best?;
        let now = self.now;
        let ttl = req.lease_ttl;
        let owner = req.lease_owner.clone();
        let job = &mut self.jobs[idx];
        job.state = JobState::Leased;
        job.lease_owner = Some(owner);
        job.lease_expires = Some(now + ttl);
        Some(Claimed {
            tenant_id: job.tenant_id.clone(),
            job_id: job.job_id.clone(),
            run_id: job.run_id.clone(),
            lane: job.lane,
            concurrency_group: job.concurrency_group.clone(),
        })
    }

    /// Mark a leased job `running` (the runner started). Used to prove the `deploy:%` serialize hold:
    /// once a `deploy:prod` job is running, a second `deploy:prod` job is NOT claimable.
    pub fn mark_running(&mut self, tenant_id: &str, job_id: &str) -> bool {
        if let Some(i) = self.find(tenant_id, job_id) {
            if self.jobs[i].state == JobState::Leased {
                self.jobs[i].state = JobState::Running;
                return true;
            }
        }
        false
    }

    /// **The dead-runner reaper ([`REAP_QUERY`], arch 02 §2.1): re-queue every expired lease.** A
    /// `leased`/`running` job whose `lease_expires < now` is moved back to `queued` (clearing the lease) so it
    /// is claimable again. The re-queue is idempotent — a job already `queued` is untouched; a job
    /// whose lease has NOT expired is untouched. Returns the re-queued `(tenant_id, job_id)` set.
    ///
    /// 0 orphans: every expired lease is re-queued. 0 duplicate enqueues: the reaper updates the
    /// EXISTING row in place (it never inserts), and any subsequent `SCHEDULE_AND_RUN_JOB` retry is
    /// idempotent on `idem_token` (the `jq_idem` unique).
    pub fn reap(&mut self) -> Vec<(String, String)> {
        let now = self.now;
        let mut reaped = Vec::new();
        for j in &mut self.jobs {
            if matches!(j.state, JobState::Leased | JobState::Running)
                && j.lease_expires.is_some_and(|e| e < now)
            {
                j.state = JobState::Queued;
                j.lease_owner = None;
                j.lease_expires = None;
                reaped.push((j.tenant_id.clone(), j.job_id.clone()));
            }
        }
        reaped
    }

    /// Heartbeat: a live runner extends its lease (`lease_expires = now + ttl`) — proves a
    /// heart-beating runner is NOT reaped (only a DEAD runner's expired lease is swept). Returns true
    /// if the lease was extended (the job is leased/running under this owner).
    pub fn heartbeat(&mut self, tenant_id: &str, job_id: &str, owner: &str, ttl: u64) -> bool {
        let now = self.now;
        if let Some(i) = self.find(tenant_id, job_id) {
            let j = &mut self.jobs[i];
            if matches!(j.state, JobState::Leased | JobState::Running)
                && j.lease_owner.as_deref() == Some(owner)
            {
                j.lease_expires = Some(now + ttl);
                return true;
            }
        }
        false
    }

    /// **Complete a job — move it to `terminal` (the runner reported `job.done`).** A `leased` /
    /// `running` job whose runner finished is moved to `terminal` (clearing the lease) so the reaper
    /// NEVER re-queues a completed job (a completed job is not a dead-runner orphan — re-queuing it
    /// would double-run it). Idempotent: a re-complete of an already-`terminal` job is a NO-OP
    /// (returns false) — this is the `job.done` side of the effectively-once invariant (a double-
    /// delivered `job.done` terminates the row ONCE). Returns true iff this call moved the row to
    /// terminal. A `queued` job (not yet claimed) reporting done is unusual but also terminates (the
    /// runner finished a job the scheduler had not yet observed as leased).
    pub fn complete_job(&mut self, tenant_id: &str, job_id: &str) -> bool {
        if let Some(i) = self.find(tenant_id, job_id) {
            let j = &mut self.jobs[i];
            if j.state != JobState::Terminal {
                j.state = JobState::Terminal;
                j.lease_owner = None;
                j.lease_expires = None;
                return true;
            }
        }
        false
    }

    /// The state of a job (for assertions).
    pub fn state_of(&self, tenant_id: &str, job_id: &str) -> Option<JobState> {
        self.find(tenant_id, job_id).map(|i| self.jobs[i].state)
    }

    /// The number of `queued` (claimable) jobs — the scheduler queue-depth telemetry signal (1.8).
    pub fn queue_depth(&self) -> usize {
        self.jobs
            .iter()
            .filter(|j| j.state == JobState::Queued)
            .count()
    }
}

/// The SQL `lane` CHECK token for a [`Lane`] — used by the live integration test to seed rows whose
/// `lane` matches the model's `priority()` ordering. (Kept here so the two stay in lock-step.)
pub fn lane_token(lane: Lane) -> &'static str {
    lane.as_str()
}

/// The SQL `state` token for a [`JobState`] — used by the live integration test to seed/assert rows.
pub fn state_token(state: JobState) -> &'static str {
    state.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(region: &str, owner: &str) -> ClaimRequest {
        ClaimRequest {
            cell_region: region.into(),
            runner_labels: vec!["linux".into(), "arm64".into(), "gpu".into()],
            runner_allowed_tiers: vec![TrustTier::Trusted, TrustTier::UntrustedFork],
            lease_owner: owner.into(),
            lease_ttl: 30,
        }
    }

    fn job(id: &str, lane: Lane, seq: u64) -> QueuedJob {
        QueuedJob::enqueued(
            "tenantA",
            "fr-par",
            id,
            format!("run-{id}"),
            lane,
            TrustTier::Trusted,
            "tenantA",
            format!("idem-{id}"),
            seq,
        )
    }

    // ── CLAIM PREDICATES ───────────────────────────────────────────────────────────────────────

    /// **RESIDENCY: a runner claims only in-region jobs (no global pool, arch 00 §5).** An
    /// out-of-region job is never claimed by an in-region runner.
    #[test]
    fn claim_residency_in_region_only() {
        let mut s = SchedulerState::new();
        let mut j = job("j1", Lane::Batch, 0);
        j.region = "us-east".into();
        s.enqueue(j);
        assert!(
            s.claim(&req("fr-par", "r1")).is_none(),
            "an out-of-region job is NOT claimable (residency by construction)"
        );
        // Same job in-region IS claimable.
        s.enqueue(job("j2", Lane::Batch, 1));
        let c = s
            .claim(&req("fr-par", "r1"))
            .expect("the in-region job claims");
        assert_eq!(c.job_id, "j2");
    }

    /// **AFFINITY: a job is claimable iff its labels ⊆ the runner's labels (arch 02 §2.3).** A job
    /// requiring a label the runner lacks is never claimed.
    #[test]
    fn claim_affinity_labels_subset() {
        let mut s = SchedulerState::new();
        s.enqueue(job("jneed", Lane::Batch, 0).with_labels(["windows"]));
        assert!(
            s.claim(&req("fr-par", "r1")).is_none(),
            "a job needing a label the runner lacks is NOT claimable"
        );
        s.enqueue(job("jok", Lane::Batch, 1).with_labels(["linux", "gpu"]));
        let c = s
            .claim(&req("fr-par", "r1"))
            .expect("the label-eligible job claims");
        assert_eq!(c.job_id, "jok", "labels ⊆ runner_labels claims");
    }

    /// **TRUST: an untrusted-fork job never reaches a runner that does not allow that tier (arch 01
    /// §2 / contract 4.9).** A `SelfHosted` job is not claimable by a runner allowing only
    /// trusted/untrusted-fork tiers.
    #[test]
    fn claim_trust_tier_membership() {
        let mut s = SchedulerState::new();
        let mut j = job("jself", Lane::Batch, 0);
        j.trust_tier = TrustTier::SelfHosted;
        s.enqueue(j);
        assert!(
            s.claim(&req("fr-par", "r1")).is_none(),
            "a SelfHosted job is NOT claimable by a runner that doesn't allow that tier"
        );
        // A runner that DOES allow SelfHosted claims it.
        let mut r = req("fr-par", "r1");
        r.runner_allowed_tiers = vec![TrustTier::SelfHosted];
        let c = s.claim(&r).expect("the self-hosted-allowed runner claims");
        assert_eq!(c.job_id, "jself");
    }

    /// **LANES: interactive is claimed before batch before deploy (the strict ORDER BY, arch 02
    /// §2.3).** The protected-human-lane analogue — interactive PR feedback never queues behind a
    /// batch matrix even when the batch job is older.
    #[test]
    fn claim_lane_priority_strict() {
        let mut s = SchedulerState::new();
        // The batch job is OLDER (seq 0); the interactive job is newer (seq 1) — lane wins over age.
        s.enqueue(job("jbatch", Lane::Batch, 0));
        s.enqueue(job("jinteractive", Lane::Interactive, 1));
        let c = s.claim(&req("fr-par", "r1")).expect("a job claims");
        assert_eq!(
            c.job_id, "jinteractive",
            "interactive is claimed before an OLDER batch job (lane priority is strict)"
        );
    }

    /// **FAIRNESS term: among same-lane jobs, the higher DRR deficit is claimed first (the CI-P13
    /// term the claim ORDERS on).** Proves the claim reads `fair_deficit.deficit DESC`; CI-P13 owns
    /// the advance/replenish.
    #[test]
    fn claim_fairness_deficit_orders() {
        let mut s = SchedulerState::new();
        let mut ja = job("ja", Lane::Batch, 0);
        ja.fair_key = "tenantA".into();
        let mut jb = job("jb", Lane::Batch, 1);
        jb.fair_key = "tenantB".into();
        jb.tenant_id = "tenantA".into(); // same tenant partition, different fair_key for the test
        s.enqueue(ja);
        s.enqueue(jb);
        // tenantB's fair_key has the higher deficit → it is claimed first despite being newer.
        s.set_deficit("tenantA", "fr-par", "tenantB", 100);
        s.set_deficit("tenantA", "fr-par", "tenantA", 1);
        let c = s.claim(&req("fr-par", "r1")).expect("a job claims");
        assert_eq!(
            c.job_id, "jb",
            "the higher-deficit fair_key is claimed first (the DRR ORDER BY term)"
        );
    }

    /// **ENQUEUED_AT tie-break: same lane + same deficit → oldest (lowest seq) first.**
    #[test]
    fn claim_oldest_first_within_equal_key() {
        let mut s = SchedulerState::new();
        s.enqueue(job("jnew", Lane::Batch, 5));
        s.enqueue(job("jold", Lane::Batch, 1));
        let c = s.claim(&req("fr-par", "r1")).expect("a job claims");
        assert_eq!(
            c.job_id, "jold",
            "the oldest job (lowest enqueued_seq) claims"
        );
    }

    // ── CONCURRENCY: serialize + cancel-superseded ─────────────────────────────────────────────

    /// **CONCURRENCY (serialize): at most ONE `deploy:prod` runs at a time (the `jq_serialize`
    /// partial unique — the claim's serialize `NOT EXISTS`).** Two `deploy:prod` jobs: claim+run the
    /// first; the second is NOT claimable until the first leaves `running`.
    #[test]
    fn concurrency_deploy_serialize_one_at_a_time() {
        let mut s = SchedulerState::new();
        s.enqueue(job("d1", Lane::Deploy, 0).with_concurrency_group("deploy:prod"));
        s.enqueue(job("d2", Lane::Deploy, 1).with_concurrency_group("deploy:prod"));

        // Claim + start the first deploy.
        let c1 = s
            .claim(&req("fr-par", "r1"))
            .expect("the first deploy claims");
        assert_eq!(c1.job_id, "d1");
        assert!(
            s.mark_running("tenantA", "d1"),
            "the first deploy is running"
        );

        // The second deploy:prod is NOT claimable while the first runs (the serialize hold).
        assert!(
            s.claim(&req("fr-par", "r2")).is_none(),
            "a second deploy:prod is NOT claimable while the first runs (serialize)"
        );

        // Once the first is terminal, the second is claimable.
        let d1_idx = s.find("tenantA", "d1").unwrap();
        s.jobs[d1_idx].state = JobState::Terminal;
        let c2 = s
            .claim(&req("fr-par", "r2"))
            .expect("the second deploy now claims");
        assert_eq!(
            c2.job_id, "d2",
            "the second deploy:prod claims once the first is done"
        );
    }

    /// **A non-deploy concurrency group does NOT serialize (only `deploy:%` is the serialize key).**
    /// Two `pr:web:42` jobs are both claimable (cancel-superseded is the PR rule, not serialize).
    #[test]
    fn concurrency_non_deploy_group_does_not_serialize() {
        let mut s = SchedulerState::new();
        s.enqueue(job("p1", Lane::Interactive, 0).with_concurrency_group("pr:web:42"));
        let c1 = s.claim(&req("fr-par", "r1")).expect("first claims");
        s.mark_running("tenantA", &c1.job_id);
        s.enqueue(job("p2", Lane::Interactive, 1).with_concurrency_group("pr:web:99"));
        assert!(
            s.claim(&req("fr-par", "r2")).is_some(),
            "a non-deploy group does not serialize"
        );
    }

    /// **CONCURRENCY (cancel-superseded): a new push to a PR cancels the prior in-flight run for that
    /// group (arch 02 §2.3).** Enqueue `pr:web:42` head 1, then head 2 superseding — head 1 goes
    /// `terminal`, only head 2 remains schedulable.
    #[test]
    fn concurrency_cancel_superseded_keeps_latest_head() {
        let mut s = SchedulerState::new();
        s.enqueue(job("head1", Lane::Interactive, 0).with_concurrency_group("pr:web:42"));
        // A new push: head2 supersedes head1 for the same group.
        let out = s.enqueue_superseding(
            job("head2", Lane::Interactive, 1).with_concurrency_group("pr:web:42"),
        );
        assert_eq!(out, EnqueueOutcome::Inserted);
        assert_eq!(
            s.state_of("tenantA", "head1"),
            Some(JobState::Terminal),
            "the prior head is cancelled (cancel-superseded)"
        );
        // Only head2 is claimable.
        let c = s
            .claim(&req("fr-par", "r1"))
            .expect("the latest head claims");
        assert_eq!(c.job_id, "head2", "only the latest PR head is tested");
        assert!(
            s.claim(&req("fr-par", "r2")).is_none(),
            "no other head remains schedulable"
        );
    }

    /// **Cancel-superseded also cancels a LEASED prior head (a push lands while the old head is
    /// claimed but not yet running).**
    #[test]
    fn cancel_superseded_cancels_a_leased_prior_head() {
        let mut s = SchedulerState::new();
        s.enqueue(job("h1", Lane::Interactive, 0).with_concurrency_group("pr:web:7"));
        let c = s.claim(&req("fr-par", "r1")).expect("h1 leases");
        assert_eq!(c.job_id, "h1");
        s.enqueue_superseding(job("h2", Lane::Interactive, 1).with_concurrency_group("pr:web:7"));
        assert_eq!(
            s.state_of("tenantA", "h1"),
            Some(JobState::Terminal),
            "a leased prior head is also cancelled by a new push"
        );
    }

    // ── THE REAPER DRILL ───────────────────────────────────────────────────────────────────────

    /// **THE REAPER-RECOVERY DRILL (the prompt GATE): kill a runner mid-lease → the reaper re-queues
    /// within the lease TTL with 0 orphans and 0 duplicate enqueues.** A runner claims a job (lease
    /// TTL 30), then dies (no heartbeat); the clock advances past the lease; the reaper sweeps → the
    /// job is `queued` again (re-claimable), the re-dispatch is ONE row (the `jq_idem` unique rejects
    /// a duplicate enqueue), and there are 0 orphaned (`leased`-forever) jobs.
    #[test]
    fn reaper_recovery_within_lease_ttl_zero_orphans_zero_dup_enqueue() {
        let mut s = SchedulerState::new();
        s.enqueue(job("j1", Lane::Batch, 0));
        let total_before = s.jobs().len();

        // A runner claims the job and takes a lease (now=0, ttl=30 → expires at 30).
        let c = s
            .claim(&req("fr-par", "dead-runner"))
            .expect("the job is claimed");
        assert_eq!(c.job_id, "j1");
        assert_eq!(s.state_of("tenantA", "j1"), Some(JobState::Leased));

        // The runner dies — no heartbeat. The clock advances PAST the lease TTL.
        s.advance(31);

        // The reaper sweeps: the expired lease is re-queued (0 orphans).
        let reaped = s.reap();
        assert_eq!(
            reaped,
            vec![("tenantA".into(), "j1".into())],
            "the dead lease is reaped"
        );
        assert_eq!(
            s.state_of("tenantA", "j1"),
            Some(JobState::Queued),
            "the reaped job is re-queued (claimable again) — 0 orphans"
        );

        // 0 duplicate enqueues: the reaper updated the EXISTING row; the count is unchanged. A
        // redundant SCHEDULE_AND_RUN_JOB retry (same idem_token) is a no-op.
        assert_eq!(
            s.jobs().len(),
            total_before,
            "the reaper inserts no new row"
        );
        let retry = s.enqueue(job("j1", Lane::Batch, 0)); // same (tenant, idem_token)
        assert_eq!(
            retry,
            EnqueueOutcome::DuplicateIdem,
            "the re-dispatch is idempotent on idem_token — ONE enqueue row, never a duplicate"
        );
        assert_eq!(
            s.jobs().len(),
            total_before,
            "still ONE row after the idempotent retry"
        );

        // A fresh runner re-claims the re-queued job (recovery complete).
        let c2 = s
            .claim(&req("fr-par", "live-runner"))
            .expect("the re-queued job re-claims");
        assert_eq!(c2.job_id, "j1", "a live runner picks up the recovered job");
    }

    /// **A HEART-BEATING runner is NOT reaped (only a DEAD runner's expired lease is swept).** The
    /// reaper is targeted: a live runner that extends its lease keeps the job; the reaper finds 0 to
    /// sweep.
    #[test]
    fn heartbeat_keeps_a_live_lease_off_the_reaper() {
        let mut s = SchedulerState::new();
        s.enqueue(job("j1", Lane::Batch, 0));
        s.claim(&req("fr-par", "live")).expect("claimed");
        // The clock advances, but the runner heartbeats BEFORE the lease expires.
        s.advance(20);
        assert!(
            s.heartbeat("tenantA", "j1", "live", 30),
            "the live runner extends its lease"
        );
        s.advance(11); // now=31, but lease was extended to 20+30=50.
        let reaped = s.reap();
        assert!(
            reaped.is_empty(),
            "a heart-beating runner's lease is NOT reaped (0 swept)"
        );
        assert_eq!(
            s.state_of("tenantA", "j1"),
            Some(JobState::Leased),
            "the live job stays leased"
        );
    }

    /// **The reaper is idempotent across repeated sweeps (re-running reap does not re-process an
    /// already-re-queued job).** After the first sweep the job is `queued`; a second sweep finds 0.
    #[test]
    fn reaper_is_idempotent_across_sweeps() {
        let mut s = SchedulerState::new();
        s.enqueue(job("j1", Lane::Batch, 0));
        s.claim(&req("fr-par", "dead")).expect("claimed");
        s.advance(31);
        assert_eq!(s.reap().len(), 1, "first sweep re-queues the dead lease");
        assert!(
            s.reap().is_empty(),
            "a second sweep finds nothing (idempotent)"
        );
    }

    // ── THE LIVE-SQL LOCK-STEP CHECK ───────────────────────────────────────────────────────────

    /// **The live claim SQL encodes the SAME predicate conjunction the model implements.** Pins the
    /// [`CLAIM_QUERY`] text so a model/SQL drift is loud: the claim is `FOR UPDATE SKIP LOCKED`, keys
    /// on region/labels/trust, holds the `deploy:%` serialize, and ORDERs lane→deficit→enqueued_at.
    #[test]
    fn the_live_claim_sql_matches_the_model_predicates() {
        assert!(
            CLAIM_QUERY.contains("FOR UPDATE OF q SKIP LOCKED"),
            "the claim is non-blocking + locks only the job_queue row (not the read-only fairness join)"
        );
        assert!(
            CLAIM_QUERY.contains("q.region = $1"),
            "RESIDENCY: in-region only"
        );
        assert!(
            CLAIM_QUERY.contains("q.labels <@ $2"),
            "AFFINITY: labels ⊆ runner_labels"
        );
        assert!(
            CLAIM_QUERY.contains("q.trust_tier = ANY($3)"),
            "TRUST: trust_tier ∈ runner_allowed_tiers"
        );
        assert!(
            CLAIM_QUERY.contains("LIKE 'deploy:%'") && CLAIM_QUERY.contains("NOT EXISTS"),
            "CONCURRENCY: the deploy:% serialize NOT EXISTS"
        );
        assert!(
            CLAIM_QUERY.contains("WHEN 'interactive' THEN 2")
                && CLAIM_QUERY.contains("COALESCE(f.deficit, 0) DESC")
                && CLAIM_QUERY.contains("q.enqueued_at ASC"),
            "ORDER BY lane DESC, deficit DESC, enqueued_at ASC"
        );
        assert!(
            CLAIM_QUERY.contains("SET state = 'leased'"),
            "on claim → leased"
        );
        // The reaper SQL re-queues an expired lease in place (no INSERT → 0 duplicate enqueues).
        assert!(
            REAP_QUERY.contains("SET state = 'queued'")
                && REAP_QUERY.contains("lease_expires < now()")
                && REAP_QUERY.contains("FOR UPDATE SKIP LOCKED")
                && REAP_QUERY.contains("pg_try_advisory_xact_lock")
                && REAP_QUERY
                    .trim_start()
                    .starts_with("WITH candidates AS MATERIALIZED"),
            "the reaper UPDATEs an expired lease in place (no INSERT)"
        );
        // Cancel-superseded terminalises the prior heads, keeping the new one.
        assert!(
            CANCEL_SUPERSEDED_QUERY.contains("SET state = 'terminal'")
                && CANCEL_SUPERSEDED_QUERY.contains("job_id <> $4"),
            "cancel-superseded terminalises prior heads, keeps the new head"
        );
        // The lane tokens match the model's CHECK-constraint strings.
        assert_eq!(lane_token(Lane::Interactive), "interactive");
        assert_eq!(state_token(JobState::Leased), "leased");
    }
}
