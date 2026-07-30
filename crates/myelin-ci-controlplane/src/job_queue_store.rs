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
use sqlx::pool::PoolConnection;
use sqlx::postgres::PgPool;
use sqlx::types::Uuid;
use sqlx::{Acquire, Postgres, Row};

use crate::ci_claim_window::MAX_CI_JOB_CLAIM_WINDOW_SECS;
use crate::runner_bind::CI_RUNNER_EXECUTION_LEASE_TTL_SECS;
#[cfg(any(test, feature = "test-support"))]
use crate::scheduler::CANCEL_SUPERSEDED_QUERY;
use crate::scheduler::{
    EnqueueOutcome, Lane, AUTHORIZE_JOB_LAUNCH_QUERY, COMPLETE_JOB_QUERY, CONSUME_CLAIM_QUERY,
    CONSUME_PREPARATION_CLAIM_QUERY, HEARTBEAT_QUERY, INSERT_JOB_QUEUE_QUERY,
    READ_COMPLETION_DISPOSITION_QUERY, RENEW_PREPARATION_LEASE_QUERY, VERIFY_JOB_LAUNCH_LIVE_QUERY,
};

// =================================================================================================
// Typed, fail-loud error (no silent drop / coerce).
// =================================================================================================

/// A durable `job_queue`-store failure. Loud + typed — a claim/enqueue/reap NEVER silently drops or
/// coerces. Safe to log: carries only the structural fault, never PII beyond the opaque tenant/job
/// tokens the CI schema already keys on.
#[derive(Debug)]
pub enum JobQueueStoreError {
    /// Caller-supplied runtime bounds or scope are invalid; no database operation was attempted.
    InvalidInput(String),
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
            JobQueueStoreError::InvalidInput(e) => {
                write!(f, "durable job_queue input refused: {e}")
            }
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
    /// The immutable claim window (seconds) the claim sizes `claim_expires_at` from — derived from
    /// this dispatch's own launch template by
    /// [`claim_window_secs`](crate::ci_claim_window::claim_window_secs). NOT an `Option`: a NULL
    /// window is legacy-only, so a Rust writer that could produce one must not exist.
    pub claim_window_secs: i64,
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
    /// The durable claim window this generation's expiry was sized from, or `None` for a legacy row
    /// dispatched before the column existed (which the claim sized from the flat execution-lease TTL
    /// instead). A checkout-bearing job may never run under `None`: the resolver refuses it before
    /// mint, and the token issuer refuses it again inside the locked mint transaction.
    pub claim_window_secs: Option<i64>,
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
    pub claim_window_secs: Option<i64>,
    pub claim_is_live: bool,
}

/// Lock one exact scheduler row and recover its persisted initial claim generation. Job-queue lock
/// precedes the CI-run lock everywhere token minting needs both, matching reporter/reaper ownership.
/// `claim_window_secs` is returned so the issuer can prove the locked generation's expiry really was
/// sized from the durable window, and that the window really is what the dispatched spec derives.
pub const LOCK_JOB_CLAIM_FOR_TOKEN_MINT_QUERY: &str = "\
SELECT state, idem_token, stage, trust_tier, lease_owner, lease_epoch,
       claim_nonce::text AS claim_nonce, claim_window_secs,
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
    /// The other recognized receipt generation, accepted only by the already-terminal replay read.
    /// It is never written by the live-generation CAS.
    pub alternate_replay_receipt: Option<&'a str>,
}

/// Exact authority for the preparation-only `leased -> terminal` CAS.
pub(crate) struct PreparationClaimConsumeSpec<'a> {
    pub tenant_id: &'a str,
    pub region: &'a str,
    pub job_id: Uuid,
    pub wf_run_id: Uuid,
    pub ci_run_id: Uuid,
    pub idem_token: &'a str,
    pub lease_owner: &'a str,
    pub lease_epoch: i64,
    pub claim_nonce: Uuid,
    pub stage: &'a str,
    pub claim_started_at_epoch_secs: i64,
    pub claim_expires_at_epoch_secs: i64,
    pub reserve_handle: &'a str,
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

/// Committed launch ownership held only between durable CAS and gated child release. The session
/// advisory lock makes a paused continuation unreapable without hiding `running` or retaining a
/// transaction/row lock. Dropping without an explicit release closes the connection, which releases
/// the lock rather than leaking it into the pool.
pub(crate) struct RetainedCiJobLaunch {
    connection: Option<PoolConnection<Postgres>>,
    lock_key: i64,
}

impl RetainedCiJobLaunch {
    pub(crate) async fn validate(&mut self) -> Result<(), JobQueueStoreError> {
        let connection = self
            .connection
            .as_mut()
            .expect("launch ownership validates one database session");
        // @tenant-cross-scope: this inspects only the current PostgreSQL session's advisory-lock
        // state. The lock key was derived from the complete tenant-scoped generation by the
        // committed launch CAS; no tenant table is read here.
        let owned: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1
                FROM pg_locks
                WHERE pid = pg_backend_pid()
                  AND locktype = 'advisory'
                  AND granted
                  AND objsubid = 1
                  AND ((classid::bigint << 32) | objid::bigint) = $1
             )",
        )
        .bind(self.lock_key)
        .fetch_one(&mut **connection)
        .await
        .map_err(|error| {
            connection.close_on_drop();
            JobQueueStoreError::Db(format!("validate launch session lock: {error}"))
        })?;
        if !owned {
            connection.close_on_drop();
            return Err(JobQueueStoreError::Db(
                "launch session lock was lost before sandbox release".into(),
            ));
        }
        Ok(())
    }

    pub(crate) async fn release(mut self) -> Result<(), JobQueueStoreError> {
        let mut connection = self
            .connection
            .take()
            .expect("launch ownership releases one database session");
        // @tenant-cross-scope: this releases one PostgreSQL session advisory lock by its derived
        // generation key; it reads no tenant table and the immediately preceding launch CAS
        // already bound the complete tenant-scoped generation.
        let released: bool = sqlx::query_scalar(
            "SELECT pg_advisory_unlock($1) /* tenant_id generation verified by launch CAS */",
        )
        .bind(self.lock_key)
        .fetch_one(&mut *connection)
        .await
        .map_err(|error| {
            connection.close_on_drop();
            JobQueueStoreError::Db(format!("validate/release launch session lock: {error}"))
        })?;
        if !released {
            connection.close_on_drop();
            return Err(JobQueueStoreError::Db(
                "launch session lock was lost during sandbox release".into(),
            ));
        }
        Ok(())
    }
}

impl Drop for RetainedCiJobLaunch {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.as_mut() {
            connection.close_on_drop();
        }
    }
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
            claim_window_secs: row.get("claim_window_secs"),
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
        let claim_window_secs = job.claim_window_secs;
        if !(1..=MAX_CI_JOB_CLAIM_WINDOW_SECS as i64).contains(&claim_window_secs) {
            return Err(JobQueueStoreError::InvalidInput(format!(
                "claim window {claim_window_secs}s is outside the durable 1..={MAX_CI_JOB_CLAIM_WINDOW_SECS}s bound"
            )));
        }
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
                    .bind(claim_window_secs) // $12 immutable dispatch-derived claim window
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
        let Some(mut launch) = self.authorize_launch_retained(claim).await? else {
            return Ok(false);
        };
        launch.validate().await?;
        launch.release().await?;
        Ok(true)
    }

    /// Win and commit the exact launch CAS while retaining a session advisory lock through the
    /// gated-child release. The lock key is a fail-closed hash of the complete durable generation:
    /// a collision can delay an unrelated launch/reap but can never admit one.
    pub(crate) async fn authorize_launch_retained(
        &self,
        claim: &CiJobLaunchClaim,
    ) -> Result<Option<RetainedCiJobLaunch>, JobQueueStoreError> {
        let job_id = parse_id("job_id", &claim.job_id)?;
        let wf_run_id = parse_id("run_id", &claim.wf_run_id)?;
        let claim_nonce = parse_id("claim_nonce", &claim.claim_nonce)?;
        let mut connection = self.pool.acquire().await.map_err(|error| {
            JobQueueStoreError::Db(format!("acquire launch fence session: {error}"))
        })?;
        // @tenant-cross-scope: PostgreSQL session-lock state is connection infrastructure, not a
        // tenant table. A pooled session carrying any advisory lock is unsafe here because
        // pg_try_advisory_lock is re-entrant; close it rather than mistaking stale ownership for a
        // fresh launch fence.
        let clean_session: bool = sqlx::query_scalar(
            "SELECT NOT EXISTS (
                SELECT 1
                FROM pg_locks
                WHERE pid = pg_backend_pid() AND locktype = 'advisory'
             )",
        )
        .fetch_one(&mut *connection)
        .await
        .map_err(|error| {
            connection.close_on_drop();
            JobQueueStoreError::Db(format!("inspect launch fence session: {error}"))
        })?;
        if !clean_session {
            connection.close_on_drop();
            return Err(JobQueueStoreError::Db(
                "launch fence session retained an advisory lock; refusing re-entrant ownership"
                    .into(),
            ));
        }
        let mut transaction = connection.begin().await.map_err(|error| {
            JobQueueStoreError::Db(format!("begin launch fence transaction: {error}"))
        })?;
        sqlx::query(
            "SELECT set_config('myelin.tenant_id', $1, true),
                    set_config('myelin.region', $2, true)",
        )
        .bind(&claim.tenant_id)
        .bind(&claim.region)
        .execute(&mut *transaction)
        .await
        .map_err(|error| JobQueueStoreError::Db(format!("scope retained launch fence: {error}")))?;
        let row = sqlx::query(AUTHORIZE_JOB_LAUNCH_QUERY)
            .bind(&claim.tenant_id)
            .bind(&claim.region)
            .bind(job_id)
            .bind(wf_run_id)
            .bind(&claim.lease_owner)
            .bind(claim.lease_epoch)
            .bind(claim_nonce)
            .bind(claim.claim_started_at_epoch_secs)
            .bind(claim.claim_expires_at_epoch_secs)
            .bind(CI_RUNNER_EXECUTION_LEASE_TTL_SECS)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|error| JobQueueStoreError::Db(format!("authorize launch fence: {error}")))?;
        if row.is_none() {
            transaction.rollback().await.map_err(|error| {
                JobQueueStoreError::Db(format!("rollback refused launch fence: {error}"))
            })?;
            return Ok(None);
        }
        let lock_key: i64 = sqlx::query_scalar(
            "SELECT hashtextextended(
                jsonb_build_array($1::text, $2::text, $3::text, $4::text, $5::text)::text,
                0
             )",
        )
        .bind(&claim.tenant_id)
        .bind(&claim.region)
        .bind(job_id)
        .bind(claim.lease_epoch)
        .bind(claim_nonce)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|error| JobQueueStoreError::Db(format!("derive launch session lock: {error}")))?;
        // @tenant-cross-scope: this acquires one PostgreSQL session advisory lock by its derived
        // generation key; it reads no tenant table and remains inside the transaction whose
        // launch CAS bound the complete tenant-scoped generation.
        let locked_result: Result<bool, sqlx::Error> = sqlx::query_scalar(
            "SELECT pg_try_advisory_lock($1) /* tenant_id generation verified by launch CAS */",
        )
        .bind(lock_key)
        .fetch_one(&mut *transaction)
        .await;
        let locked = match locked_result {
            Ok(locked) => locked,
            Err(error) => {
                let _ = transaction.rollback().await;
                // The server may have acquired the session lock before its acknowledgement was
                // lost. Never return that session to the pool under ambiguity.
                connection.close_on_drop();
                return Err(JobQueueStoreError::Db(format!(
                    "acquire launch session lock: {error}"
                )));
            }
        };
        if !locked {
            transaction.rollback().await.map_err(|error| {
                JobQueueStoreError::Db(format!("rollback colliding launch fence: {error}"))
            })?;
            return Err(JobQueueStoreError::Db(
                "launch session lock is already owned; refusing a colliding or duplicate launch"
                    .into(),
            ));
        }
        if let Err(error) = transaction.commit().await {
            connection.close_on_drop();
            return Err(JobQueueStoreError::Db(format!(
                "commit launch fence: {error}"
            )));
        }
        Ok(Some(RetainedCiJobLaunch {
            connection: Some(connection),
            lock_key,
        }))
    }

    /// **The exact-generation preparation-lease renewal ([`RENEW_PREPARATION_LEASE_QUERY`]).** Push
    /// `lease_expires` forward by one execution slot, capped at the immutable claim expiry, for a
    /// generation that is still `leased`, still owns the exact durable parent attempt, and whose
    /// public surface has not yet crossed to `running`.
    ///
    /// Returns `false` when NOTHING matched — the generation was reaped, cancelled, terminalized, or
    /// its workload launch already won. That is ownership loss, not a benign no-op: the caller MUST
    /// abort before spawning anything else under this claim.
    pub async fn renew_preparation_lease(
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
        let claim_started_at_epoch_secs = claim.claim_started_at_epoch_secs;
        let claim_expires_at_epoch_secs = claim.claim_expires_at_epoch_secs;
        let execution_lease = CI_RUNNER_EXECUTION_LEASE_TTL_SECS.to_string();
        let renewed = with_tenant_tx(&self.pool, &claim.tenant_id, &claim.region, move |conn| {
            Box::pin(async move {
                let row = sqlx::query(RENEW_PREPARATION_LEASE_QUERY)
                    .bind(&tenant_id)
                    .bind(&region)
                    .bind(job_id)
                    .bind(wf_run_id)
                    .bind(&lease_owner)
                    .bind(lease_epoch)
                    .bind(claim_nonce)
                    .bind(claim_started_at_epoch_secs)
                    .bind(claim_expires_at_epoch_secs)
                    .bind(&execution_lease)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|e| PgError::Query(e.to_string()))?;
                Ok(row.is_some())
            })
        })
        .await
        .map_err(JobQueueStoreError::from_pg)?;
        Ok(renewed)
    }

    /// **CT-007 slice 5b.3-2c: read-only sibling of [`Self::authorize_launch_retained`].** Checks
    /// the EXACT SAME generation predicate ([`VERIFY_JOB_LAUNCH_LIVE_QUERY`], the same bound fields
    /// in the same order) but never mutates `job_queue`/`ci_job` and never holds an advisory lock or
    /// connection past this call. For the pre-Hop-A checkout-authorization hook, which must confirm
    /// the durable claim is still live WITHOUT performing (or pre-empting) the real workload's
    /// `leased -> running` CAS — that CAS remains `authorize_launch_retained`'s alone to commit.
    pub(crate) async fn verify_launch_live(
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
        let claim_started_at_epoch_secs = claim.claim_started_at_epoch_secs;
        let claim_expires_at_epoch_secs = claim.claim_expires_at_epoch_secs;
        let live = with_tenant_tx(&self.pool, &claim.tenant_id, &claim.region, move |conn| {
            Box::pin(async move {
                let row: Option<i32> = sqlx::query_scalar(VERIFY_JOB_LAUNCH_LIVE_QUERY)
                    .bind(&tenant_id)
                    .bind(&region)
                    .bind(job_id)
                    .bind(wf_run_id)
                    .bind(&lease_owner)
                    .bind(lease_epoch)
                    .bind(claim_nonce)
                    .bind(claim_started_at_epoch_secs)
                    .bind(claim_expires_at_epoch_secs)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|e| PgError::Query(e.to_string()))?;
                Ok(row.is_some())
            })
        })
        .await
        .map_err(JobQueueStoreError::from_pg)?;
        Ok(live)
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
        if state == "terminal"
            && (stored_receipt.as_deref() == Some(spec.completion_receipt)
                || stored_receipt.as_deref() == spec.alternate_replay_receipt)
        {
            Ok(ClaimConsumeOutcome::AlreadyConsumed)
        } else {
            Ok(ClaimConsumeOutcome::Refused)
        }
    }

    /// Consume an exact still-leased parent attempt after checkout preparation terminated. Replay is
    /// v4-only: there was no historical preparation writer whose legacy receipt is authoritative.
    pub(crate) async fn consume_preparation_claim_on_conn(
        conn: &mut sqlx::PgConnection,
        spec: PreparationClaimConsumeSpec<'_>,
    ) -> Result<ClaimConsumeOutcome, PgError> {
        let consumed = sqlx::query(CONSUME_PREPARATION_CLAIM_QUERY)
            .bind(spec.tenant_id)
            .bind(spec.region)
            .bind(spec.job_id)
            .bind(spec.wf_run_id)
            .bind(spec.idem_token)
            .bind(spec.lease_owner)
            .bind(spec.lease_epoch)
            .bind(spec.claim_nonce)
            .bind(spec.stage)
            .bind(spec.claim_started_at_epoch_secs)
            .bind(spec.claim_expires_at_epoch_secs)
            .bind(spec.ci_run_id)
            .bind(spec.reserve_handle)
            .bind(spec.completion_receipt)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|error| PgError::Query(error.to_string()))?;
        if consumed.is_some() {
            return Ok(ClaimConsumeOutcome::Consumed);
        }
        let replay = sqlx::query_scalar::<_, i32>(
            "SELECT 1
             FROM job_queue q
             WHERE q.tenant_id = $1 AND q.region = $2 AND q.job_id = $3::uuid
               AND q.run_id = $4::uuid AND q.idem_token = $5 AND q.stage = $6
               AND q.state = 'terminal' AND q.completion_receipt = $7
               AND EXISTS (
                 SELECT 1 FROM ci_job_parent_attempt p
                 WHERE p.tenant_id = q.tenant_id AND p.region = q.region
                   AND p.job_id = q.job_id AND p.wf_run_id = q.run_id
                   AND p.ci_run_id = $8::uuid AND p.reserve_handle = $9
                   AND p.lease_owner = $10 AND p.lease_epoch = $11
                   AND p.claim_nonce = $12::uuid
                   AND p.claim_started_at_epoch_secs = $13
                   AND p.claim_expires_at_epoch_secs = $14
               )",
        )
        .bind(spec.tenant_id)
        .bind(spec.region)
        .bind(spec.job_id)
        .bind(spec.wf_run_id)
        .bind(spec.idem_token)
        .bind(spec.stage)
        .bind(spec.completion_receipt)
        .bind(spec.ci_run_id)
        .bind(spec.reserve_handle)
        .bind(spec.lease_owner)
        .bind(spec.lease_epoch)
        .bind(spec.claim_nonce)
        .bind(spec.claim_started_at_epoch_secs)
        .bind(spec.claim_expires_at_epoch_secs)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|error| PgError::Query(error.to_string()))?;
        Ok(if replay.is_some() {
            ClaimConsumeOutcome::AlreadyConsumed
        } else {
            ClaimConsumeOutcome::Refused
        })
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
/// no new `AppSpec` field). Every `interval` it re-queues expired leases whose Flow/CI owners remain
/// active. When configured with cancelled accounting, it also terminalizes a bounded, rotating
/// keyset page of expired launched jobs whose owners are already cancelled, using their immutable
/// manifest ceiling so superseded work is never made claimable again. Candidate failures are
/// isolated and reported after the rest of the page, so one corrupt tenant cannot block later
/// tenants. The reaper launches nothing.
///
/// The loop is resilient by design (a reaper that dies on one transient DB blip is worse than one
/// that retries): a reap error is logged LOUDLY (never a silent drop — the typed error is surfaced to
/// stderr) and the loop continues to the next tick. The first sweep is delayed one `interval` so the
/// serve boot-migrate (which creates `job_queue`) has completed.
pub struct JobQueueReaper {
    store: crate::CiRegionQueueStore,
    region: String,
    interval: Duration,
    cancelled_accounting: Option<(sqlx::PgPool, myelin_storage::DurableCostLedger)>,
    cancelled_cursor: std::sync::Mutex<Option<crate::job_queue_region::AbandonedCancelledCursor>>,
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
            cancelled_accounting: None,
            cancelled_cursor: std::sync::Mutex::new(None),
        }
    }

    /// Attach the tenant-scoped durable settlement path for expired launched jobs whose owners have
    /// already terminated. Production always configures this; queue-only tests may omit it.
    pub fn with_cancelled_accounting(
        mut self,
        pool: sqlx::PgPool,
        ledger: myelin_storage::DurableCostLedger,
    ) -> Self {
        self.cancelled_accounting = Some((pool, ledger));
        self
    }

    /// The cell region this reaper sweeps.
    pub fn region(&self) -> &str {
        &self.region
    }

    /// The sweep interval.
    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// **One recovery sweep** — seal expired prelaunch phases, re-queue expired active leases, and
    /// reconcile one bounded, rotating page of expired cancelled launches. The prelaunch deadline
    /// is topology-aware and independent of the flat queue lease. Sealing and lease reaping are
    /// attempted independently so a transient failure in either recovery surface cannot suppress
    /// the other. Returns the total rows changed, or reports accumulated failures after every
    /// independently safe operation was attempted.
    pub async fn reap_once(&self) -> Result<u64, JobQueueStoreError> {
        let mut changed = 0_u64;
        let mut failures = 0_u64;
        let mut first_failure = None;
        match self.store.seal_expired_prelaunch_usage(&self.region).await {
            Ok(sealed) => changed = changed.saturating_add(sealed),
            Err(error) => {
                failures = failures.saturating_add(1);
                first_failure
                    .get_or_insert_with(|| format!("prelaunch-usage sealing failed: {error}"));
            }
        }
        match self.store.reap(&self.region).await {
            Ok(reaped) => changed = changed.saturating_add(reaped),
            Err(error) => {
                failures = failures.saturating_add(1);
                first_failure.get_or_insert_with(|| format!("lease recovery failed: {error}"));
            }
        }
        let Some((pool, ledger)) = &self.cancelled_accounting else {
            return if failures == 0 {
                Ok(changed)
            } else {
                Err(JobQueueStoreError::Db(format!(
                    "{failures} reaper operation(s) failed after {changed} row(s) were recovered; \
                     first failure: {}",
                    first_failure.unwrap_or_else(|| "unknown recovery failure".into())
                )))
            };
        };
        let after = self
            .cancelled_cursor
            .lock()
            .map_err(|_| JobQueueStoreError::Db("cancelled-recovery cursor lock poisoned".into()))?
            .clone();
        let mut candidates = self
            .store
            .abandoned_cancelled(&self.region, after.as_ref())
            .await?;
        if candidates.is_empty() && after.is_some() {
            candidates = self.store.abandoned_cancelled(&self.region, None).await?;
        }
        let next_cursor =
            candidates.last().map(
                |candidate| crate::job_queue_region::AbandonedCancelledCursor {
                    tenant_id: candidate.tenant_id.clone(),
                    job_id: candidate.job_id.clone(),
                },
            );
        *self.cancelled_cursor.lock().map_err(|_| {
            JobQueueStoreError::Db("cancelled-recovery cursor lock poisoned".into())
        })? = next_cursor;

        let mut cancelled_failures = 0_u64;
        for candidate in candidates {
            let authority = match crate::PgCiRunSupersession::new(
                pool.clone(),
                ledger.clone(),
                myelin_tenancy::TenantId(candidate.tenant_id),
                myelin_tenancy::Region(self.region.clone()),
                tokio::runtime::Handle::current(),
            ) {
                Ok(authority) => authority,
                Err(error) => {
                    failures = failures.saturating_add(1);
                    cancelled_failures = cancelled_failures.saturating_add(1);
                    first_failure.get_or_insert_with(|| error.to_string());
                    continue;
                }
            };
            match authority
                .reconcile_abandoned_job(&candidate.wf_run_id, &candidate.job_id)
                .await
            {
                Ok(true) => changed = changed.saturating_add(1),
                Ok(false) => {}
                Err(error) => {
                    failures = failures.saturating_add(1);
                    cancelled_failures = cancelled_failures.saturating_add(1);
                    first_failure.get_or_insert_with(|| error.to_string());
                }
            }
        }
        if failures > 0 {
            let first = first_failure.unwrap_or_else(|| "unknown reconciliation failure".into());
            return if failures == cancelled_failures {
                Err(JobQueueStoreError::Db(format!(
                    "{cancelled_failures} cancelled recovery candidate(s) failed after {changed} \
                     row(s) were recovered; first failure: {first}"
                )))
            } else {
                Err(JobQueueStoreError::Db(format!(
                    "{failures} recovery operation(s) failed after {changed} row(s) were recovered; \
                     first failure: {first}"
                )))
            };
        }
        Ok(changed)
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
                        "ci-controlplane reaper: recovered {n} expired lease(s) in region \
                         `{}` (prelaunch sealing, active requeue, or cancelled settlement)",
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
                                "ci-controlplane reaper: recovered {n} expired lease(s) in region \
                                 `{}` (prelaunch sealing, active requeue, or cancelled settlement)",
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
        CONSUME_CLAIM_QUERY, CONSUME_PREPARATION_CLAIM_QUERY, HEARTBEAT_QUERY,
        INSERT_JOB_QUEUE_QUERY, REAP_QUERY, RENEW_PREPARATION_LEASE_QUERY,
    };

    /// **The store's SQL constants are well-formed against the real DDL (bind arity + column names).**
    /// This is the DB-free half of the gate: the constants the store binds carry the exact param
    /// count the store binds and the exact column names the row helpers read — a drift (a renamed
    /// column, a changed bind order) is loud here, before the live integration test.
    #[test]
    fn the_bound_sql_matches_the_store_binds() {
        // INSERT: twelve binds ($1..$12), including queue-authority stage + the claim window.
        assert!(INSERT_JOB_QUEUE_QUERY.contains("$12") && !INSERT_JOB_QUEUE_QUERY.contains("$13"));
        assert!(INSERT_JOB_QUEUE_QUERY.contains("claim_window_secs"));
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
            "j.claim_window_secs",
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
        assert!(CLAIM_QUERY.contains("COALESCE(j.claim_window_secs::text, $5)"));
        assert!(CLAIM_QUERY.contains("w.state IN ('running', 'waiting')"));
        assert!(CLAIM_QUERY.contains("c.state = 'running'"));
        assert!(CLAIM_QUERY.contains("c.wf_run_id = q.run_id"));
        // Mint authority reloads the exact persisted claim and treats legacy/null expiry as dead.
        assert!(LOCK_JOB_CLAIM_FOR_TOKEN_MINT_QUERY.contains("$4::uuid"));
        assert!(LOCK_JOB_CLAIM_FOR_TOKEN_MINT_QUERY.contains("FOR UPDATE"));
        assert!(LOCK_JOB_CLAIM_FOR_TOKEN_MINT_QUERY
            .contains("COALESCE(claim_expires_at > statement_timestamp(), false)"));
        // Completion binds nonce + stage and records the receipt only for that exact claim.
        assert!(CONSUME_CLAIM_QUERY.contains("$7") && !CONSUME_CLAIM_QUERY.contains("$8"));
        assert!(CONSUME_CLAIM_QUERY.contains("claim_nonce = $5::uuid"));
        assert!(CONSUME_CLAIM_QUERY.contains("stage = $7"));
        assert!(CONSUME_CLAIM_QUERY.contains("state = 'running'"));
        assert!(!CONSUME_CLAIM_QUERY.contains("state IN ('leased','running')"));
        for predicate in [
            "q.state = 'leased'",
            "q.region = $2",
            "q.run_id = $4::uuid",
            "q.idem_token = $5",
            "q.lease_owner = $6",
            "q.lease_epoch = $7",
            "q.claim_nonce = $8::uuid",
            "q.stage = $9",
            "q.claim_expires_at > statement_timestamp()",
            "FROM ci_job_parent_attempt AS parent",
            "parent.ci_run_id = $12::uuid",
            "parent.reserve_handle = $13",
            "surface.state IN ('queued', 'leased')",
        ] {
            assert!(
                CONSUME_PREPARATION_CLAIM_QUERY.contains(predicate),
                "missing preparation authority predicate `{predicate}`"
            );
        }
        assert!(!CONSUME_PREPARATION_CLAIM_QUERY.contains("q.state = 'running'"));
        assert!(!CONSUME_PREPARATION_CLAIM_QUERY.contains("q.state IN"));
        // Final launch is a one-shot exact-generation CAS, including original claim times.
        assert!(AUTHORIZE_JOB_LAUNCH_QUERY.contains("$10"));
        assert!(!AUTHORIZE_JOB_LAUNCH_QUERY.contains("$11"));
        assert!(AUTHORIZE_JOB_LAUNCH_QUERY.contains("WITH launched AS"));
        assert!(AUTHORIZE_JOB_LAUNCH_QUERY.contains("SET state = 'running'"));
        assert!(AUTHORIZE_JOB_LAUNCH_QUERY.contains("UPDATE ci_job AS surface"));
        assert!(AUTHORIZE_JOB_LAUNCH_QUERY.contains("surface.state IN ('queued', 'leased')"));
        assert!(AUTHORIZE_JOB_LAUNCH_QUERY.contains("lease_expires = LEAST("));
        assert!(AUTHORIZE_JOB_LAUNCH_QUERY.contains("state = 'leased'"));
        assert!(AUTHORIZE_JOB_LAUNCH_QUERY.contains("claim_nonce = $7::uuid"));
        assert!(AUTHORIZE_JOB_LAUNCH_QUERY.contains("claim_expires_at > statement_timestamp()"));
        // RENEW: ten binds ($1..$10), leased-only, capped at the immutable claim expiry.
        assert!(
            RENEW_PREPARATION_LEASE_QUERY.contains("$10")
                && !RENEW_PREPARATION_LEASE_QUERY.contains("$11")
        );
        assert!(RENEW_PREPARATION_LEASE_QUERY.contains("SET lease_expires = LEAST("));
        assert!(RENEW_PREPARATION_LEASE_QUERY.contains("q.state = 'leased'"));
        // REAP: one bind ($1 region), an in-place UPDATE (no INSERT → 0 duplicate enqueues).
        assert!(REAP_QUERY.contains("$1") && !REAP_QUERY.contains("$2"));
        assert!(REAP_QUERY
            .trim_start()
            .starts_with("WITH candidates AS MATERIALIZED"));
        assert!(REAP_QUERY.contains("FOR UPDATE SKIP LOCKED"));
        assert!(REAP_QUERY.contains("pg_try_advisory_xact_lock"));
        assert!(REAP_QUERY.contains("w.state IN ('running', 'waiting')"));
        assert!(REAP_QUERY.contains("c.state = 'running'"));
        assert!(REAP_QUERY.contains("c.wf_run_id = job_queue.run_id"));
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
