//! # `job_queue_store` — CT-004c.1: the REAL durable `job_queue` store + the dead-runner reaper loop
//!
//! **Owning architecture doc (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md`
//! §2.1 (pull-leasing — the `FOR UPDATE SKIP LOCKED` claim, the lease + heartbeat, the dead-runner
//! reaper), §2.3 (concurrency groups — `deploy:%` serialize + `pr:%` cancel-superseded, affinity
//! labels); `01-tech-and-data-model.md` §3.3 (the `job_queue` table + the `jq_claimable`/
//! `jq_serialize`/`jq_idem` indexes). **Contracts:** 11.1 (OLTP — the claim hot path), 12.1 (the
//! `(tenant, region)` partition key), 1.6 (residency-pin).
//!
//! ## What CT-004c.1 ships — the SCHEDULER made durable (turning the model into a real store)
//! Before CT-004c.1 the scheduler existed in TWO forms: the live OLTP SQL ([`crate::scheduler`]'s
//! `CLAIM_QUERY`/`REAP_QUERY`/`CANCEL_SUPERSEDED_QUERY` bare `&str` constants — with the only live
//! apply a feature-gated test that string-replaces `job_queue` with a scratch table) and the DB-free
//! [`crate::scheduler::SchedulerState`] in-memory model. There was NO production-callable store that
//! ran the claim/reap against a real pool. [`CiJobQueueStore`] is that store: it holds a [`PgPool`],
//! executes the BYTE-IDENTICAL production SQL constants, and is the durable equivalent of BOTH the
//! controlplane `SchedulerState` and the sandbox `JobLeaseStore::claim_for_labels`.
//!
//! **This resolves the "two parallel in-memory lease implementations".** In CT-004c.2 the runner (and
//! the controlplane dispatch handshake) claim from THIS one durable store — the single system-of-
//! record for the leased-job lifecycle. `SchedulerState` stays as the deterministic in-memory model
//! the unit/drill tests exercise (the SAME predicate semantics); it is not a mock of this store nor
//! vice versa — they are the same algorithm, one DB-free and one pool-backed.
//!
//! ## Tenant-scoped RLS — the exact seam (do it RIGHT from the start)
//! The CI tables are FORCE-RLS `(tenant, region)` (CT-004m; `scripts/pg-init/00-rls-conventions.sql`).
//! CT-004a's cost store writes on a bare pool without the tenant GUC (a known CT-004d floor). This
//! store does NOT repeat that: every PER-TENANT statement runs under a transaction that sets the
//! `(tenant, region)` session GUC the RLS policy keys on, via the established MR-022
//! [`myelin_storage::with_tenant_tx`] convention (acquire → BEGIN → `set_config('myelin.tenant_id',
//! …, true)` + `set_config('myelin.region', …, true)` → op → COMMIT, GUC discarded on commit so no
//! cross-tenant bleed). The per-tenant ops — [`CiJobQueueStore::enqueue`],
//! [`CiJobQueueStore::cancel_superseded`], [`CiJobQueueStore::complete`],
//! [`CiJobQueueStore::heartbeat`] — carry a tenant, so they run tenant-scoped and are correct under
//! the app role (`myelin_app`, NOBYPASSRLS), not just as the admin/owner. (They bind `tenant_id`
//! explicitly, so the `tenant-predicate` lint reads the IDOR guard on every one.)
//!
//! ### Claim + reap are a separate REGION-scoped capability
//! Cross-tenant claim/reap live only on [`crate::job_queue_region::CiRegionQueueStore`], constructed
//! over the dedicated scheduler pool. They are deliberately absent from [`CiJobQueueStore`], making
//! it impossible to accidentally run them through the tenant application pool.
//!
//! ## Fail-loud, typed, no silent drop
//! Every DB error is a typed [`JobQueueStoreError::Db`] (never a swallowed drop). A `job_id`/`run_id`
//! that is not a UUID (the durable column type) is a loud [`JobQueueStoreError::BadId`]; a read-back
//! row whose `lane`/`trust_tier` token is outside the frozen CHECK set is a loud
//! [`JobQueueStoreError::CorruptRow`].
//!
//! ## CT-004c.2 HANDOFF (the runner binds to THIS store)
//! CT-004c.1 is the durability plumbing ONLY — it leases a row, it launches NOTHING. **CT-004c.2**
//! binds the `RunnerAgent` to this store and starts the pipeline body on the executor: the runner
//! long-polls [`CiJobQueueStore::claim`] (region + labels + trust tiers) to LEASE a row, then hands
//! the leased job to the AG-D4-gated sandbox to execute the untrusted body, heartbeats via
//! [`CiJobQueueStore::heartbeat`] while it runs, and [`CiJobQueueStore::complete`]s on `job.done`.
//! The security-load-bearing seam CT-004c.2's adversarial verifier must cover: the runner MUST pass
//! ONLY the trust tiers it is allowed to execute to `claim` (an `untrusted_fork` job must never be
//! claimed by a claim that lists only trusted tiers — the predicate this store proves), and the
//! sandbox-exec path (`myelin-ci-sandbox/src/runner.rs`) that CT-004c.1 deliberately does NOT touch.

use std::time::Duration;

use myelin_ci_sandbox::TrustTier;
use myelin_storage::{with_tenant_tx, PgError};
use sqlx::postgres::PgPool;
use sqlx::types::Uuid;
use sqlx::Row;

#[cfg(any(test, feature = "test-support"))]
use crate::scheduler::CANCEL_SUPERSEDED_QUERY;
use crate::scheduler::{
    EnqueueOutcome, Lane, AUTHORIZE_JOB_LAUNCH_QUERY, COMPLETE_JOB_QUERY, CONSUME_CLAIM_QUERY,
    HEARTBEAT_QUERY, INSERT_JOB_QUEUE_QUERY, READ_COMPLETION_DISPOSITION_QUERY,
};

// =================================================================================================
// Typed, fail-loud error (no silent drop / coerce).
// =================================================================================================

/// A durable `job_queue`-store failure. Loud + typed — a claim/enqueue/reap NEVER silently drops or
/// coerces. Safe to log: carries only the structural fault, never PII beyond the opaque tenant/job
/// tokens the CI schema already keys on.
#[derive(Debug)]
pub enum JobQueueStoreError {
    /// A durable-store DB error (the statement did NOT succeed) — never a silent partial write.
    Db(String),
    /// A `job_id`/`run_id` presented to the store is not a UUID (the durable column type). CI job/run
    /// ids ARE uuids in production; a non-uuid token is refused loudly (never truncated/coerced).
    BadId {
        /// Which field failed (`job_id` | `run_id`).
        field: &'static str,
        /// The offending value (an opaque CI id token — not PII).
        value: String,
    },
    /// A read-back row carries a `lane`/`trust_tier` token outside the frozen CHECK-constraint set — a
    /// corrupt durable write, surfaced loudly (never silently coerced).
    CorruptRow(String),
}

impl core::fmt::Display for JobQueueStoreError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            JobQueueStoreError::Db(e) => write!(f, "durable job_queue store error: {e}"),
            JobQueueStoreError::BadId { field, value } => write!(
                f,
                "durable job_queue op refused: {field} `{value}` is not a UUID (the \
                 job_queue.{field} column is uuid — CI job/run ids are uuids in production)"
            ),
            JobQueueStoreError::CorruptRow(e) => {
                write!(
                    f,
                    "corrupt durable job_queue row (outside the frozen token set): {e}"
                )
            }
        }
    }
}

impl std::error::Error for JobQueueStoreError {}

impl JobQueueStoreError {
    /// Map a storage-layer [`PgError`] into the store's typed error (fail-loud, no swallow).
    pub(crate) fn from_pg(e: PgError) -> Self {
        JobQueueStoreError::Db(e.to_string())
    }
}

// =================================================================================================
// The enqueue input + the leased-row result.
// =================================================================================================

/// A schedulable job to [`CiJobQueueStore::enqueue`] — the durable-write shape (the `job_queue`
/// columns the claim filters/orders on). PII-free: every field is an opaque id / label / vocabulary
/// token. `job_id`/`run_id` are the UUID string form (parsed to the `uuid` column type, loud on a
/// non-uuid). The arguments mirror the columns (each load-bearing for the claim), so it is a struct
/// rather than a wide positional constructor.
#[derive(Clone, Debug)]
pub struct DurableEnqueue {
    /// The tenant partition (the first PK component; the RLS/tenant-GUC scope of the write).
    pub tenant_id: String,
    /// The residency region — a runner claims only in-region (no global pool).
    pub region: String,
    /// The opaque job id (the `(tenant_id, job_id)` PK) — UUID string form.
    pub job_id: String,
    /// The owning run id — UUID string form.
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
    /// no-op (a reaper re-queue + a redundant re-dispatch = ONE row).
    pub idem_token: String,
    /// Durable pipeline stage attribution. Every new dispatch supplies it; NULL is reserved for
    /// historical pre-expand rows and blocks runner-lane activation.
    pub stage: String,
}

/// A leased `job_queue` row (the `CLAIM_QUERY` `RETURNING` shape) — the claimed job's identity + the
/// scheduling terms it won on. Carries `trust_tier` so a caller/verifier can assert WHICH tier was
/// leased (the security seam CT-004c.2 depends on: a claim listing only trusted tiers never returns
/// an `untrusted_fork` row).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeasedJob {
    /// The claimed job's tenant.
    pub tenant_id: String,
    /// The claimed job id.
    pub job_id: Uuid,
    /// The owning run.
    pub run_id: Uuid,
    /// The lane it was claimed in.
    pub lane: Lane,
    /// The concurrency group (if any).
    pub concurrency_group: Option<String>,
    /// The DRR fairness key it was claimed on.
    pub fair_key: String,
    /// The trust tier of the leased job (the security-load-bearing fact).
    pub trust_tier: TrustTier,
    /// The runner identity that owns this exact live claim.
    pub lease_owner: String,
    /// **The claim generation** — the monotone `lease_epoch` the claim bumped (CT-004d.2 claim-bound
    /// completion). The runner carries this to `report_done`; the completion CAS refuses a stale claim
    /// (a lower epoch than the row's) so a reaped-and-re-claimed worker cannot win first delivery.
    pub lease_epoch: i64,
    /// Fresh unguessable authority minted for this exact claim generation.
    pub claim_nonce: String,
    /// PostgreSQL statement time that minted this claim, in Unix epoch seconds. Token issuance uses
    /// this durable claim fact rather than a process clock, so acknowledgement-loss retry is stable.
    pub claim_started_at_epoch_secs: i64,
    /// Initial claim expiry in Unix epoch seconds. A token authority may issue only within this
    /// bounded claim generation; heartbeat may extend execution but never rewrites mint identity.
    pub claim_expires_at_epoch_secs: i64,
}

/// Exact durable scheduler generation presented to the final pre-spawn launch fence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiJobLaunchClaim {
    pub tenant_id: String,
    pub region: String,
    pub wf_run_id: String,
    pub job_id: String,
    pub lease_owner: String,
    pub lease_epoch: i64,
    pub claim_nonce: String,
    pub claim_started_at_epoch_secs: i64,
    pub claim_expires_at_epoch_secs: i64,
}

/// Scheduler claim facts reloaded under a row lock immediately before token minting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LockedJobClaim {
    pub state: String,
    pub idem_token: String,
    pub stage: Option<String>,
    pub trust_tier: String,
    pub lease_owner: Option<String>,
    pub lease_epoch: i64,
    pub claim_nonce: Option<String>,
    pub claim_started_at_epoch_secs: Option<i64>,
    pub claim_expires_at_epoch_secs: Option<i64>,
    pub claim_is_live: bool,
}

/// Lock one exact scheduler row and recover its persisted initial claim generation. Job-queue lock
/// precedes the CI-run lock everywhere token minting needs both, matching reporter/reaper ownership.
pub const LOCK_JOB_CLAIM_FOR_TOKEN_MINT_QUERY: &str = "\
SELECT state, idem_token, stage, trust_tier, lease_owner, lease_epoch,
       claim_nonce::text AS claim_nonce,
       EXTRACT(EPOCH FROM claim_started_at)::bigint AS claim_started_at_epoch_secs,
       EXTRACT(EPOCH FROM claim_expires_at)::bigint AS claim_expires_at_epoch_secs,
       COALESCE(claim_expires_at > statement_timestamp(), false) AS claim_is_live
FROM job_queue
WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid AND run_id = $4::uuid
FOR UPDATE";

/// **The outcome of the claim CAS joined to durable signal delivery by the completion reporter.**
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClaimConsumeOutcome {
    /// THIS call won the claim — the row moved to `terminal` and the receipt was recorded. Proceed to
    /// signal the verdict.
    Consumed,
    /// The row is already `terminal` with the SAME receipt — an idempotent redelivery of the identical
    /// completion. Re-signalling is a harmless engine no-op (the `wf_signal` PK dedups).
    AlreadyConsumed,
    /// No live claim matched the presented generation — a missing row, a stale/other `(lease_owner,
    /// lease_epoch)`, or a divergent receipt (e.g. a flipped-verdict replay). Fail-closed: nothing
    /// changed, and the caller signals NO verdict.
    Refused,
}

/// Borrowed inputs for the caller-transaction completion CAS.
pub(crate) struct ClaimConsumeSpec<'a> {
    pub tenant_id: &'a str,
    pub job_id: Uuid,
    pub lease_owner: &'a str,
    pub lease_epoch: i64,
    pub claim_nonce: Uuid,
    pub stage: &'a str,
    pub completion_receipt: &'a str,
}

// =================================================================================================
// The store.
// =================================================================================================

/// **The REAL durable CI `job_queue` store (CT-004c.1).** Holds the OLTP [`PgPool`] and executes the
/// BYTE-IDENTICAL production SQL constants from [`crate::scheduler`] against it, each under the
/// tenant-/region-scoped transaction the FORCE-RLS `job_queue` table requires. Cloneable (the pool is
/// an `Arc`-backed handle). Named `…Store` + carries a `PgPool` field so the
/// `no-in-memory-durable-store` scanner reads it as a genuine durable store. The caller must have
/// applied the CI control-plane migrations (which create `job_queue` + `fair_deficit` + the three
/// claim indexes) — the ci-controlplane `serve(AppSpec)` boot migrate does this.
#[derive(Clone)]
pub struct CiJobQueueStore {
    pub(crate) pool: PgPool,
}

impl CiJobQueueStore {
    /// Wrap the controlplane OLTP pool as the durable `job_queue` store. The production composition
    /// root constructs this from the MR-022 `SubstrateProvider` pool
    /// ([`crate::ci_job_queue_store`]).
    pub fn with_pg(pool: PgPool) -> CiJobQueueStore {
        CiJobQueueStore { pool }
    }

    /// The pool this store is bound to (for a co-commit caller that wants to begin its own tx).
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Caller-transaction claim lock used by the production token issuer. It performs no fallback
    /// or coercion; the issuer compares every returned generation field before invoking Identity.
    pub(crate) async fn lock_for_token_mint_on_conn(
        connection: &mut sqlx::PgConnection,
        tenant_id: &str,
        region: &str,
        job_id: &str,
        run_id: &str,
    ) -> Result<Option<LockedJobClaim>, JobQueueStoreError> {
        let row = sqlx::query(LOCK_JOB_CLAIM_FOR_TOKEN_MINT_QUERY)
            .bind(tenant_id)
            .bind(region)
            .bind(job_id)
            .bind(run_id)
            .fetch_optional(&mut *connection)
            .await
            .map_err(|error| {
                JobQueueStoreError::Db(format!("lock job claim for token mint: {error}"))
            })?;
        Ok(row.map(|row| LockedJobClaim {
            state: row.get("state"),
            idem_token: row.get("idem_token"),
            stage: row.get("stage"),
            trust_tier: row.get("trust_tier"),
            lease_owner: row.get("lease_owner"),
            lease_epoch: row.get("lease_epoch"),
            claim_nonce: row.get("claim_nonce"),
            claim_started_at_epoch_secs: row.get("claim_started_at_epoch_secs"),
            claim_expires_at_epoch_secs: row.get("claim_expires_at_epoch_secs"),
            claim_is_live: row.get("claim_is_live"),
        }))
    }

    /// **Enqueue a job, idempotent on `(tenant_id, idem_token)` (arch 02 §2.1; [`INSERT_JOB_QUEUE_QUERY`]).**
    /// Runs tenant-scoped ([`with_tenant_tx`]) so it is correct under the app role. Returns
    /// [`EnqueueOutcome::Inserted`] iff a NEW row was inserted, or [`EnqueueOutcome::DuplicateIdem`]
    /// when the `jq_idem` unique made the insert a no-op (a reaper re-queue + a redundant re-dispatch
    /// = ONE row, never a duplicate — the CI-D1 effectively-once floor).
    pub async fn enqueue(
        &self,
        job: &DurableEnqueue,
    ) -> Result<EnqueueOutcome, JobQueueStoreError> {
        let job_uuid = parse_id("job_id", &job.job_id)?;
        let run_uuid = parse_id("run_id", &job.run_id)?;
        let labels = job.labels.clone();
        let group = job.concurrency_group.clone();
        let lane = job.lane.as_str();
        let trust = trust_token(job.trust_tier);
        let tenant_id = job.tenant_id.clone();
        let region = job.region.clone();
        let fair_key = job.fair_key.clone();
        let idem = job.idem_token.clone();
        let stage = job.stage.clone();
        let inserted = with_tenant_tx(&self.pool, &job.tenant_id, &job.region, move |conn| {
            Box::pin(async move {
                let row = sqlx::query(INSERT_JOB_QUEUE_QUERY)
                    .bind(&tenant_id) // $1 tenant_id (the RLS/tenant predicate)
                    .bind(&region) // $2 region
                    .bind(job_uuid) // $3 job_id
                    .bind(run_uuid) // $4 run_id
                    .bind(lane) // $5 lane
                    .bind(&labels) // $6 labels text[]
                    .bind(trust) // $7 trust_tier
                    .bind(group.as_deref()) // $8 concurrency_group (nullable)
                    .bind(&fair_key) // $9 fair_key
                    .bind(&idem) // $10 idem_token
                    .bind(&stage) // $11 durable pipeline stage
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|e| PgError::Query(e.to_string()))?;
                Ok(row.is_some())
            })
        })
        .await
        .map_err(JobQueueStoreError::from_pg)?;
        Ok(if inserted {
            EnqueueOutcome::Inserted
        } else {
            EnqueueOutcome::DuplicateIdem
        })
    }

    /// **Cancel-superseded (arch 02 §2.3; [`CANCEL_SUPERSEDED_QUERY`]) — a new push to a PR cancels
    /// the in-flight run for that group.** Tenant-scoped ([`with_tenant_tx`]). Moves the prior
    /// `queued`/`leased` rows of `group` to `terminal`, keeping `keep_job_id` (the new head), so only
    /// the latest PR head is tested. Returns the cancelled job ids.
    #[cfg(any(test, feature = "test-support"))]
    pub async fn cancel_superseded(
        &self,
        tenant_id: &str,
        region: &str,
        group: &str,
        keep_job_id: &str,
    ) -> Result<Vec<Uuid>, JobQueueStoreError> {
        let keep_uuid = parse_id("job_id", keep_job_id)?;
        let tenant_id_owned = tenant_id.to_string();
        let region_owned = region.to_string();
        let group_owned = group.to_string();
        let rows = with_tenant_tx(&self.pool, tenant_id, region, move |conn| {
            Box::pin(async move {
                sqlx::query(CANCEL_SUPERSEDED_QUERY)
                    .bind(&tenant_id_owned) // $1 tenant_id (the RLS/tenant predicate)
                    .bind(&region_owned) // $2 region
                    .bind(&group_owned) // $3 concurrency_group
                    .bind(keep_uuid) // $4 keep_job_id
                    .fetch_all(&mut *conn)
                    .await
                    .map_err(|e| PgError::Query(e.to_string()))
            })
        })
        .await
        .map_err(JobQueueStoreError::from_pg)?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            out.push(r.get::<Uuid, _>("job_id"));
        }
        Ok(out)
    }

    /// **Complete a job — move it to `terminal` ([`COMPLETE_JOB_QUERY`]).** Tenant-scoped. Idempotent:
    /// a re-complete of an already-`terminal` row returns `false` (the `job.done` side of the
    /// effectively-once invariant). Returns `true` iff this call moved the row to terminal.
    pub async fn complete(
        &self,
        tenant_id: &str,
        region: &str,
        job_id: &str,
    ) -> Result<bool, JobQueueStoreError> {
        let job_uuid = parse_id("job_id", job_id)?;
        let tenant_id_owned = tenant_id.to_string();
        let moved = with_tenant_tx(&self.pool, tenant_id, region, move |conn| {
            Box::pin(async move {
                let row = sqlx::query(COMPLETE_JOB_QUERY)
                    .bind(&tenant_id_owned) // $1 tenant_id (the RLS/tenant predicate)
                    .bind(job_uuid) // $2 job_id
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|e| PgError::Query(e.to_string()))?;
                Ok(row.is_some())
            })
        })
        .await
        .map_err(JobQueueStoreError::from_pg)?;
        Ok(moved)
    }

    /// Atomically authorize one exact, still-live claim generation for launch. This is a one-shot
    /// `leased` → `running` CAS, not a read/check: cancellation, reaping, and launch serialize on the
    /// row, so no cancellation can slip between a successful check and sandbox spawn.
    pub async fn authorize_launch(
        &self,
        claim: &CiJobLaunchClaim,
    ) -> Result<bool, JobQueueStoreError> {
        let job_id = parse_id("job_id", &claim.job_id)?;
        let wf_run_id = parse_id("run_id", &claim.wf_run_id)?;
        let claim_nonce = parse_id("claim_nonce", &claim.claim_nonce)?;
        let tenant_id = claim.tenant_id.clone();
        let region = claim.region.clone();
        let lease_owner = claim.lease_owner.clone();
        let lease_epoch = claim.lease_epoch;
        let claim_started_at = claim.claim_started_at_epoch_secs;
        let claim_expires_at = claim.claim_expires_at_epoch_secs;
        let authorized = with_tenant_tx(&self.pool, &claim.tenant_id, &claim.region, move |conn| {
            Box::pin(async move {
                let row = sqlx::query(AUTHORIZE_JOB_LAUNCH_QUERY)
                    .bind(&tenant_id)
                    .bind(&region)
                    .bind(job_id)
                    .bind(wf_run_id)
                    .bind(&lease_owner)
                    .bind(lease_epoch)
                    .bind(claim_nonce)
                    .bind(claim_started_at)
                    .bind(claim_expires_at)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|error| PgError::Query(error.to_string()))?;
                Ok(row.is_some())
            })
        })
        .await
        .map_err(JobQueueStoreError::from_pg)?;
        Ok(authorized)
    }

    /// Claim CAS for the durable completion reporter. This is intentionally caller-transaction-only:
    /// the claim transition and `PgFlowExecutor::signal_typed_on_conn` must share this exact connection
    /// and commit boundary, so no public helper can accidentally reintroduce a crash gap.
    pub(crate) async fn consume_claim_on_conn(
        conn: &mut sqlx::PgConnection,
        spec: ClaimConsumeSpec<'_>,
    ) -> Result<ClaimConsumeOutcome, PgError> {
        let consumed = sqlx::query(CONSUME_CLAIM_QUERY)
            .bind(spec.tenant_id) // $1 tenant_id
            .bind(spec.job_id) // $2 job_id
            .bind(spec.lease_owner) // $3 lease_owner
            .bind(spec.lease_epoch) // $4 lease_epoch
            .bind(spec.claim_nonce) // $5 unguessable claim nonce
            .bind(spec.completion_receipt) // $6 canonical completion receipt
            .bind(spec.stage) // $7 durable queue/spec stage authority
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| PgError::Query(e.to_string()))?;
        if consumed.is_some() {
            return Ok(ClaimConsumeOutcome::Consumed);
        }
        let disposition = sqlx::query(READ_COMPLETION_DISPOSITION_QUERY)
            .bind(spec.tenant_id)
            .bind(spec.job_id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| PgError::Query(e.to_string()))?;
        let Some(row) = disposition else {
            return Ok(ClaimConsumeOutcome::Refused);
        };
        let state: String = row.get("state");
        let stored_receipt: Option<String> = row.get("completion_receipt");
        if state == "terminal" && stored_receipt.as_deref() == Some(spec.completion_receipt) {
            Ok(ClaimConsumeOutcome::AlreadyConsumed)
        } else {
            Ok(ClaimConsumeOutcome::Refused)
        }
    }

    /// **Heartbeat — a live runner extends its lease ([`HEARTBEAT_QUERY`]).** Tenant-scoped. Only the
    /// lease OWNER (while `leased` or `running`) can extend, so a heart-beating runner is NOT reaped (only a DEAD
    /// runner's expired lease is swept). Returns `true` iff the lease was extended.
    pub async fn heartbeat(
        &self,
        tenant_id: &str,
        region: &str,
        job_id: &str,
        lease_owner: &str,
        extend_secs: u64,
    ) -> Result<bool, JobQueueStoreError> {
        let job_uuid = parse_id("job_id", job_id)?;
        let tenant_id_owned = tenant_id.to_string();
        let owner = lease_owner.to_string();
        let ttl = extend_secs.to_string();
        let extended = with_tenant_tx(&self.pool, tenant_id, region, move |conn| {
            Box::pin(async move {
                let row = sqlx::query(HEARTBEAT_QUERY)
                    .bind(&tenant_id_owned) // $1 tenant_id (the RLS/tenant predicate)
                    .bind(job_uuid) // $2 job_id
                    .bind(&owner) // $3 lease_owner
                    .bind(&ttl) // $4 extend_seconds
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|e| PgError::Query(e.to_string()))?;
                Ok(row.is_some())
            })
        })
        .await
        .map_err(JobQueueStoreError::from_pg)?;
        Ok(extended)
    }
}

// =================================================================================================
// The dead-runner reaper loop — the periodic driver wired into the controlplane serve lifecycle.
// =================================================================================================

/// **The dead-runner reaper loop (arch 02 §2.1) — the lease-driven periodic driver.** A bounded
/// background task the ci-controlplane `main` spawns onto the serve runtime (minimal-impact wiring —
/// no new `AppSpec` field): every `interval` it calls [`crate::CiRegionQueueStore::reap`] for the cell region,
/// re-queuing expired leases so a dead runner's job becomes claimable again. This is the SAME
/// lease-driven periodic shape as `WorkflowEngine::tick` (a lease sweep on a timer). The reaper is
/// SAFE: it only moves expired `leased`/`running` rows back to `queued`; it launches nothing.
///
/// The loop is resilient by design (a reaper that dies on one transient DB blip is worse than one
/// that retries): a reap error is logged LOUDLY (never a silent drop — the typed error is surfaced to
/// stderr) and the loop continues to the next tick. The first sweep is delayed one `interval` so the
/// serve boot-migrate (which creates `job_queue`) has completed.
pub struct JobQueueReaper {
    store: crate::CiRegionQueueStore,
    region: String,
    interval: Duration,
}

impl JobQueueReaper {
    /// Construct the reaper for a cell region + sweep interval. `region` is the cell's residency
    /// region (the `SubstrateProvider` config `region`); `interval` is the sweep cadence (a few
    /// multiples of the runner heartbeat — a lease expiry is caught within one interval).
    pub fn new(
        store: crate::CiRegionQueueStore,
        region: impl Into<String>,
        interval: Duration,
    ) -> Self {
        JobQueueReaper {
            store,
            region: region.into(),
            interval,
        }
    }

    /// The cell region this reaper sweeps.
    pub fn region(&self) -> &str {
        &self.region
    }

    /// The sweep interval.
    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// **One reap sweep** — re-queue every expired lease in the region; returns the count re-queued.
    /// Exposed so a test/drill can drive a single deterministic sweep (the loop just calls this on a
    /// timer).
    pub async fn reap_once(&self) -> Result<u64, JobQueueStoreError> {
        self.store.reap(&self.region).await
    }

    /// Legacy forever-loop driver retained for deterministic callers. Production uses
    /// [`Self::run_until_shutdown`] so the task is joined before process exit.
    pub async fn run(self) {
        loop {
            tokio::time::sleep(self.interval).await;
            match self.reap_once().await {
                Ok(0) => {}
                Ok(n) => {
                    eprintln!(
                        "ci-controlplane reaper: re-queued {n} expired lease(s) in region \
                         `{}` (dead-runner recovery)",
                        self.region
                    );
                }
                Err(e) => {
                    // Fail LOUD (never a silent drop), but keep sweeping — a reaper must be resilient
                    // to a transient DB blip; the NEXT sweep re-queues any lease this one missed.
                    eprintln!(
                        "ci-controlplane reaper: sweep in region `{}` FAILED (will retry next \
                         interval): {e}",
                        self.region
                    );
                }
            }
        }
    }

    /// Run periodic sweeps until explicit shutdown or sender closure. Shutdown wins over a
    /// simultaneously-ready timer, so drain never begins a fresh sweep.
    pub async fn run_until_shutdown(self, mut shutdown: tokio::sync::watch::Receiver<bool>) {
        loop {
            if *shutdown.borrow() {
                return;
            }
            tokio::select! {
                biased;
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return;
                    }
                }
                _ = tokio::time::sleep(self.interval) => {
                    match self.reap_once().await {
                        Ok(0) => {}
                        Ok(n) => {
                            eprintln!(
                                "ci-controlplane reaper: re-queued {n} expired lease(s) in region \
                                 `{}` (dead-runner recovery)",
                                self.region
                            );
                        }
                        Err(e) => {
                            eprintln!(
                                "ci-controlplane reaper: sweep in region `{}` FAILED (will retry \
                                 next interval): {e}",
                                self.region
                            );
                        }
                    }
                }
            }
        }
    }
}

// =================================================================================================
// Row helpers (loud on a corrupt / non-uuid value — never a silent coerce). Shared with the
// cross-tenant claim/reap file (`crate::job_queue_region`).
// =================================================================================================

/// Parse a `job_id`/`run_id` token into the durable `uuid` column type. A non-uuid is a loud refusal.
pub(crate) fn parse_id(field: &'static str, value: &str) -> Result<Uuid, JobQueueStoreError> {
    Uuid::parse_str(value).map_err(|_| JobQueueStoreError::BadId {
        field,
        value: value.to_string(),
    })
}

/// The `job_queue.trust_tier` CHECK token for a sandbox [`TrustTier`] (the frozen three-tier
/// vocabulary — arch 01 §3.3). The read-back inverse is [`trust_from_token`].
pub(crate) fn trust_token(t: TrustTier) -> &'static str {
    match t {
        TrustTier::Trusted => "trusted",
        TrustTier::UntrustedFork => "untrusted_fork",
        TrustTier::SelfHosted => "self_hosted",
    }
}

/// Parse a `job_queue.trust_tier` token back to a [`TrustTier`] — a token outside the frozen set is a
/// loud [`JobQueueStoreError::CorruptRow`] (never silently coerced).
pub(crate) fn trust_from_token(token: &str) -> Result<TrustTier, JobQueueStoreError> {
    match token {
        "trusted" => Ok(TrustTier::Trusted),
        "untrusted_fork" => Ok(TrustTier::UntrustedFork),
        "self_hosted" => Ok(TrustTier::SelfHosted),
        other => Err(JobQueueStoreError::CorruptRow(format!(
            "unknown trust_tier token `{other}`"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::{
        AUTHORIZE_JOB_LAUNCH_QUERY, CANCEL_SUPERSEDED_QUERY, CLAIM_QUERY, COMPLETE_JOB_QUERY,
        CONSUME_CLAIM_QUERY, HEARTBEAT_QUERY, INSERT_JOB_QUEUE_QUERY, REAP_QUERY,
    };

    /// **The store's SQL constants are well-formed against the real DDL (bind arity + column names).**
    /// This is the DB-free half of the gate: the constants the store binds carry the exact param
    /// count the store binds and the exact column names the row helpers read — a drift (a renamed
    /// column, a changed bind order) is loud here, before the live integration test.
    #[test]
    fn the_bound_sql_matches_the_store_binds() {
        // INSERT: eleven binds ($1..$11), including queue-authority stage.
        assert!(INSERT_JOB_QUEUE_QUERY.contains("$11") && !INSERT_JOB_QUEUE_QUERY.contains("$12"));
        assert!(INSERT_JOB_QUEUE_QUERY.contains("ON CONFLICT (tenant_id, idem_token) DO NOTHING"));
        assert!(INSERT_JOB_QUEUE_QUERY.contains("RETURNING job_id"));
        // CLAIM: five binds ($1..$5), RETURNING every column leased_from_row reads.
        assert!(CLAIM_QUERY.contains("$5") && !CLAIM_QUERY.contains("$6"));
        for col in [
            "j.tenant_id",
            "j.job_id",
            "j.run_id",
            "j.lane",
            "j.concurrency_group",
            "j.fair_key",
            "j.trust_tier",
            "j.lease_epoch",
            "j.claim_nonce",
            "claim_started_at_epoch_secs",
            "claim_expires_at_epoch_secs",
        ] {
            assert!(
                CLAIM_QUERY.contains(col),
                "the claim RETURNING carries `{col}` (leased_from_row reads it)"
            );
        }
        // The claim BUMPS the monotone claim generation so a stale re-claim is a higher epoch.
        assert!(CLAIM_QUERY.contains("lease_epoch = j.lease_epoch + 1"));
        assert!(CLAIM_QUERY.contains("claim_nonce = gen_random_uuid()"));
        assert!(CLAIM_QUERY.contains("claim_started_at = statement_timestamp()"));
        assert!(CLAIM_QUERY
            .contains("claim_expires_at = statement_timestamp() + ($5 || ' seconds')::interval"));
        // Mint authority reloads the exact persisted claim and treats legacy/null expiry as dead.
        assert!(LOCK_JOB_CLAIM_FOR_TOKEN_MINT_QUERY.contains("$4::uuid"));
        assert!(LOCK_JOB_CLAIM_FOR_TOKEN_MINT_QUERY.contains("FOR UPDATE"));
        assert!(LOCK_JOB_CLAIM_FOR_TOKEN_MINT_QUERY
            .contains("COALESCE(claim_expires_at > statement_timestamp(), false)"));
        // Completion binds nonce + stage and records the receipt only for that exact claim.
        assert!(CONSUME_CLAIM_QUERY.contains("$7") && !CONSUME_CLAIM_QUERY.contains("$8"));
        assert!(CONSUME_CLAIM_QUERY.contains("claim_nonce = $5::uuid"));
        assert!(CONSUME_CLAIM_QUERY.contains("stage = $7"));
        // Final launch is a one-shot exact-generation CAS, including original claim times.
        assert!(AUTHORIZE_JOB_LAUNCH_QUERY.contains("$9"));
        assert!(!AUTHORIZE_JOB_LAUNCH_QUERY.contains("$10"));
        assert!(AUTHORIZE_JOB_LAUNCH_QUERY.contains("SET state = 'running'"));
        assert!(AUTHORIZE_JOB_LAUNCH_QUERY.contains("state = 'leased'"));
        assert!(AUTHORIZE_JOB_LAUNCH_QUERY.contains("claim_nonce = $7::uuid"));
        assert!(AUTHORIZE_JOB_LAUNCH_QUERY.contains("claim_expires_at > statement_timestamp()"));
        // REAP: one bind ($1 region), an in-place UPDATE (no INSERT → 0 duplicate enqueues).
        assert!(REAP_QUERY.contains("$1") && !REAP_QUERY.contains("$2"));
        assert!(REAP_QUERY.trim_start().starts_with("UPDATE"));
        // CANCEL: four binds ($1..$4), keeping the new head.
        assert!(CANCEL_SUPERSEDED_QUERY.contains("$4") && !CANCEL_SUPERSEDED_QUERY.contains("$5"));
        assert!(CANCEL_SUPERSEDED_QUERY.contains("job_id <> $4"));
        // COMPLETE: two binds, idempotent on state <> 'terminal'.
        assert!(COMPLETE_JOB_QUERY.contains("$2") && !COMPLETE_JOB_QUERY.contains("$3"));
        assert!(COMPLETE_JOB_QUERY.contains("state <> 'terminal'"));
        // HEARTBEAT: four binds, owner-guarded, leased/running only.
        assert!(HEARTBEAT_QUERY.contains("$4") && !HEARTBEAT_QUERY.contains("$5"));
        assert!(
            HEARTBEAT_QUERY.contains("lease_owner = $3")
                && HEARTBEAT_QUERY.contains("state IN ('leased', 'running')")
        );
    }

    /// **The trust-tier token round-trips through the frozen CHECK vocabulary (the security seam's
    /// serialization).** Every tier maps to its DB token and back; an unknown token is a loud corrupt
    /// row, never silently coerced. This underpins the CT-004c.2 property (a claim's tier list is the
    /// exact DB predicate).
    #[test]
    fn trust_tier_tokens_round_trip_and_reject_unknown() {
        for t in [
            TrustTier::Trusted,
            TrustTier::UntrustedFork,
            TrustTier::SelfHosted,
        ] {
            let token = trust_token(t);
            assert_eq!(trust_from_token(token).unwrap(), t);
        }
        assert_eq!(trust_token(TrustTier::UntrustedFork), "untrusted_fork");
        assert!(matches!(
            trust_from_token("root"),
            Err(JobQueueStoreError::CorruptRow(_))
        ));
    }

    /// **A non-uuid job/run id is refused LOUDLY (never coerced).** The durable columns are `uuid`; a
    /// synthetic non-uuid token (e.g. a drill's `"ci/job/0"`) never reaches the durable row.
    #[test]
    fn a_non_uuid_id_is_a_loud_refusal() {
        let e = parse_id("job_id", "not-a-uuid").unwrap_err();
        assert!(matches!(
            e,
            JobQueueStoreError::BadId {
                field: "job_id",
                ..
            }
        ));
        // A real uuid parses.
        assert!(parse_id("run_id", "00000000-0000-0000-0000-000000000001").is_ok());
    }

    /// **The lane token round-trips through the frozen three-lane CHECK vocabulary.** A read-back
    /// `lane` outside the set would be a loud corrupt row in `leased_from_row`.
    #[test]
    fn lane_tokens_round_trip() {
        for l in [Lane::Interactive, Lane::Batch, Lane::Deploy] {
            assert_eq!(Lane::from_token(l.as_str()), Some(l));
        }
        assert_eq!(Lane::from_token("nonsense"), None);
    }
}
