//! # `job_spec_store` — CT-004d.1: durable launch templates + dispatch co-persistence
//!
//! **Owning architecture doc (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md`
//! §3.3 (the `SCHEDULE_AND_RUN_JOB` handshake — the dispatch enqueues the stage's job) + §2.1 (the
//! pull-lease claim the runner resolves against) + §5.3 (the digest-pinned [`JobSpec`] is what the
//! sandbox EXECUTES). **Reconciliation:** §OQ-F (the deterministic `idem_token` producer/consumer
//! agree on — one token keys BOTH the `job_queue` row and this spec row).
//!
//! ## What CT-004d.1 ships — the missing half of the dispatch→durable→resolve bridge
//! CT-004c.1 landed the durable [`CiJobQueueStore`](crate::CiJobQueueStore) (enqueue/claim/reap) —
//! but the `job_queue` row is **scheduling metadata only** (job_id / run_id / lane / labels /
//! trust_tier / fair_key / idem_token); it carries NO digest-pinned launch template (image / command /
//! egress / limits / trust / workspace / authority / meter). CT-004c.2 wired the runner to claim from
//! the durable queue but resolved the spec through an INJECTED [`JobSpecResolver`](crate::JobSpecResolver)
//! that the production path left as a fail-closed no-op ([`crate::spec_store_unavailable_resolver`]).
//!
//! [`CiJobSpecStore`] is the real backing that resolver reads: it persists a dispatched stage's
//! non-launchable [`DurableCiJobLaunchTemplate`] keyed by `(tenant_id, job_id)` — the SAME identity
//! the leased `job_queue` row carries — so the runner can resolve a claimed row, mint under that
//! claim generation, attach the short-lived token, and execute it. It is the durable
//! system-of-record for "what a leased job runs".
//!
//! ## The template is a SINGLE `jsonb` column — a faithful round-trip
//! [`DurableCiJobLaunchTemplate`] derives `serde::{Serialize, Deserialize}`, so the whole value
//! round-trips through ONE `spec jsonb` column with NO lossy per-column projection and no token JTI.
//! The resolved template is what EXECUTES, so fidelity is load-bearing: a corrupt / missing template is a
//! **fail-closed** [`CiJobSpecStoreError`] (the runner then does NOT launch — the leased row is left for
//! the reaper), NEVER a fabricated default spec.
//!
//! ## Dispatch co-persists the spec + the `job_queue` row in ONE tx, idempotent on the `idem_token`
//! [`CiJobSpecStore::co_persist_dispatch`] writes BOTH the `job_queue` row (via the byte-identical
//! [`INSERT_JOB_QUEUE_QUERY`]) AND the `ci_job_spec` row in a single tenant-scoped transaction, so a
//! crash between the two cannot leave a claimable `job_queue` row with no resolvable spec (which would
//! wedge on the reaper forever). Both inserts are idempotent — the `job_queue` row on the
//! `(tenant_id, idem_token)` `jq_idem` unique, the spec row on the `(tenant_id, job_id)` PK — so a
//! re-dispatch (control-plane replay) or a reaper re-queue collapses to ONE of each (effectively-once).
//!
//! ## THE SECURITY INVARIANT — the `job_queue` row's `trust_tier` comes from the SPEC, never widened
//! The eligibility gate CT-004c.1/c.2 enforce is the `job_queue` row's `trust_tier` (an `untrusted_fork`
//! job is never leased by a trusted-only runner). [`co_persist_dispatch`](CiJobSpecStore::co_persist_dispatch)
//! **feeds that gate from the real dispatched spec**: it refuses fail-closed
//! ([`CiJobSpecStoreError::TrustTierMismatch`]) if the enqueue's declared `trust_tier` does not equal
//! `spec.trust_tier` — so the row that gates the claim can never carry a WIDER tier than the spec that
//! will execute. The residency `region` is the run's honest residency pin carried on the enqueue (no
//! global pool, no default). This does not weaken the gate — it wires the gate to the truth.
//!
//! ## The lease-TTL floor (the CT-004c.2 verifier's MEDIUM fix)
//! [`RunnerAgent::run_one`](myelin_ci_sandbox::RunnerAgent) BLOCKS for the whole in-line job and only
//! heartbeats BEFORE + AFTER the blocking launch (never mid-launch, without a structural change to the
//! sandbox launch). So a job whose wall-clock exceeds the lease TTL would lapse its lease mid-run → the
//! reaper re-queues it → a second runner double-executes. CT-004d.1 makes that impossible on TWO ends:
//! (1) [`MAX_JOB_TIMEOUT_SECS`] is the ceiling this store enforces at persist ([`CiJobSpecStoreError::TimeoutTooLong`]
//! fail-closed) — a spec whose `timeout_secs` exceeds it never becomes a claimable job; and (2) the
//! runner is wired with `lease_ttl_secs = ` [`CI_RUNNER_EXECUTION_LEASE_TTL_SECS`](crate::runner_bind::CI_RUNNER_EXECUTION_LEASE_TTL_SECS)
//! `> MAX_JOB_TIMEOUT_SECS`, so a leased job provably cannot outlive its lease. (The mid-launch
//! heartbeat is the tighter fix — named as the follow-on that would let the lease TTL shrink again.)

use myelin_ci_sandbox::{JobSpecTemplate, TrustTier};
use myelin_storage::{with_tenant_tx, with_tenant_tx_error, PgError};
use sqlx::postgres::PgPool;
use sqlx::types::Uuid;
use sqlx::Row;

use crate::job_queue_store::{parse_id, trust_token};
use crate::scheduler::{EnqueueOutcome, INSERT_JOB_QUEUE_QUERY};
use crate::DurableEnqueue;

/// **The wall-clock ceiling a dispatched [`JobSpecTemplate`]'s timeout may not exceed.**
/// The runner's lease TTL is wired ABOVE this (see [`crate::runner_bind::CI_RUNNER_EXECUTION_LEASE_TTL_SECS`]),
/// so a leased job can never outlive its lease mid-run (the CT-004c.2 double-run guard, closed at the
/// dispatch). 6 h (GitHub-Actions parity) — comfortably above every real CI job (the workspace's
/// longest configured job is 2 h) so this rejects nothing legitimate; a spec above it is refused
/// fail-closed rather than admitted as a lease-outliving double-run hazard.
pub const MAX_JOB_TIMEOUT_SECS: u32 = 6 * 60 * 60;

/// **Persist a dispatched stage's [`JobSpecTemplate`] keyed `(tenant_id, job_id)`, idempotent on the PK
/// ([`CiJobSpecStore::co_persist_dispatch`] co-writes it with the `job_queue` row).** Binds:
/// `$1 tenant_id`, `$2 region`, `$3 job_id` (uuid), `$4 run_id` (uuid), `$5 idem_token`,
/// `$6 spec` (jsonb). `ON CONFLICT (tenant_id, job_id) DO NOTHING` makes a re-dispatch a no-op — the
/// stored spec is deterministic on the dispatch position, so a re-write would be identical anyway.
/// `RETURNING job_id` is present iff a fresh row was inserted (absent on the idempotent conflict).
pub const INSERT_JOB_SPEC_QUERY: &str = "\
INSERT INTO ci_job_spec (tenant_id, region, job_id, run_id, idem_token, spec, stage)
VALUES ($1, $2, $3, $4, $5, $6, $7)
ON CONFLICT (tenant_id, job_id) DO NOTHING
RETURNING job_id";

/// **Read a leased job's persisted [`JobSpecTemplate`] back (the runner's resolve path).** Keyed on the
/// `(tenant_id, job_id)` the leased `job_queue` row carries. Binds `$1 tenant_id`, `$2 job_id` (uuid).
/// Returns the `spec jsonb` for deserialization; NO row → the loud [`CiJobSpecStoreError::SpecNotFound`]
/// (fail-closed — the runner does not launch an unresolved job).
pub const SELECT_JOB_SPEC_QUERY: &str = "\
SELECT spec FROM ci_job_spec WHERE tenant_id = $1 AND job_id = $2";

/// **Read a dispatched job's durable claimed-identity (run_id, idem_token, stage, spec) for `(tenant, job_id)`
/// — the terminal reporter's fail-closed verification anchor (CT-004d.2 rewire).** The reporter derives
/// the completed job's `job_id` and reads this back to prove the presented `(run_id, idem_token)` match
/// the durable dispatch record BEFORE it signals a verdict — a forged/mis-keyed completion resolves no
/// row (or a divergent one) and is refused. `stage` is the durable verdict-attribution name (the
/// restart-safe replacement for the in-memory stage bridge). Binds `$1 tenant_id`, `$2 job_id` (uuid).
pub const SELECT_JOB_SPEC_IDENTITY_QUERY: &str = "\
SELECT run_id::text AS run_id, idem_token, stage, spec
FROM ci_job_spec WHERE tenant_id = $1 AND job_id = $2";

/// Exact replay readback for both halves of one durable dispatch. `ON CONFLICT DO NOTHING` is safe
/// only when the existing queue/spec pair is byte-equivalent to the requested dispatch; this query
/// makes that equivalence a checked invariant inside the same transaction.
const SELECT_EXACT_DISPATCH_QUERY: &str = "\
SELECT q.region, q.job_id::text AS queue_job_id, q.run_id::text AS queue_run_id,
       q.lane, q.labels, q.trust_tier, q.concurrency_group, q.fair_key,
       q.idem_token AS queue_idem_token, q.stage AS queue_stage,
       q.claim_window_secs AS queue_claim_window_secs,
       s.region AS spec_region, s.run_id::text AS spec_run_id,
       s.idem_token AS spec_idem_token, s.spec, s.stage AS spec_stage
FROM job_queue q
JOIN ci_job_spec s ON s.tenant_id = q.tenant_id AND s.job_id = q.job_id
WHERE q.tenant_id = $1 AND q.idem_token = $2";

/// **The runner-lane pre-activation guard's probe (CT-004d.2 — the ROLLING-UPGRADE FLOOR).** Counts, in
/// a region, `job_queue` rows that are still NON-terminal but whose queue-authority `stage` is NULL
/// (a pre-rewire historical dispatch a rolling upgrade left un-back-filled). The ACTIVATION guard
/// refuses to start the (dormant) runner lane
/// while any such row is still live — so the invariant "no non-terminal NULL-stage job exists at
/// activation" is CHECKED, not assumed (CI has never been production-activated, so a healthy deploy
/// returns 0). Cross-tenant within a region (the runner lane claims cross-tenant), so it is NOT
/// tenant-scoped — it runs on the region-scheduler / admin path. Bind: `$1 region`.
pub const NON_TERMINAL_NULL_STAGE_JOBS_QUERY: &str = "\
SELECT count(*) FROM job_queue q \
WHERE q.region = $1 AND q.state <> 'terminal' AND q.stage IS NULL";

/// **The claim-window activation guard's probe (CT-007 lease/topology reconciliation).** Counts, in
/// a region, non-terminal `job_queue` rows whose `claim_window_secs` is still NULL — a dispatch
/// written by a binary older than the claim-window expand. Such a row is claimed under the flat
/// execution-lease fallback, which is correct for the workload-only topology it was dispatched under
/// but cannot hold a four-execution checkout composition. Every new Rust writer populates the
/// column, so a converged fleet returns 0; the checkout-composition activation path refuses while it
/// does not, and the resolver/issuer refuse per-job regardless, so this is a coarse operational
/// guard layered on top of enforcement, never the enforcement itself. Cross-tenant within a region
/// (the runner lane claims cross-tenant), so it runs on the region-scheduler path. Bind: `$1 region`.
pub const NON_TERMINAL_NULL_CLAIM_WINDOW_JOBS_QUERY: &str = "\
SELECT count(*) FROM job_queue q \
WHERE q.region = $1 AND q.state <> 'terminal' AND q.claim_window_secs IS NULL";

/// Durable, non-launchable job input. The scheduler may persist this for an arbitrary queue wait:
/// it contains the immutable sandbox template plus the stable Identity authority handle, but no JTI
/// or bearer material. The exact live claim resolver mints and attaches a short-lived token later.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DurableCiJobLaunchTemplate {
    pub spec: JobSpecTemplate,
    pub ci_run_id: String,
    pub token_authority_handle: String,
}

// =================================================================================================
// Typed, fail-loud error (no silent drop / coerce / fabricated-default spec).
// =================================================================================================

/// A durable launch-template-store failure. Loud + typed — persist/resolve never silently drops,
/// coerces, or fabricates a default template. Safe to log: carries only the structural fault + opaque
/// tenant/job/idem tokens the CI schema already keys on, never the spec's inner material.
#[derive(Debug)]
pub enum CiJobSpecStoreError {
    /// A durable-store DB error (the statement did NOT succeed) — never a silent partial write.
    Db(String),
    /// A `job_id`/`run_id` presented to the store is not a UUID (the durable column type) — refused
    /// loudly (never truncated/coerced), the SAME rule as the `job_queue` store.
    BadId {
        /// Which field failed (`job_id` | `run_id`).
        field: &'static str,
        /// The offending value (an opaque CI id token — not PII).
        value: String,
    },
    /// No spec row for `(tenant, job_id)` — the runner cannot resolve the leased job to a spec. A
    /// fail-closed resolve error (the row stays leased, the reaper recovers it), NEVER a default spec.
    SpecNotFound {
        /// The tenant partition of the missing spec.
        tenant_id: String,
        /// The job id whose spec is absent.
        job_id: String,
    },
    /// A stored `spec jsonb` did not deserialize to a launch template (a corrupt durable write).
    CorruptSpec {
        /// The job id whose spec failed to decode.
        job_id: String,
        /// The serde error (structural — not the spec's inner material).
        detail: String,
    },
    /// Serializing the [`JobSpecTemplate`] to jsonb failed (should not happen for a valid template) —
    /// refused loudly rather than persisting a partial/empty spec.
    SpecEncode(String),
    /// The enqueue's declared `trust_tier` does not equal the dispatched spec's `trust_tier` — the
    /// SECURITY invariant fail-closed: the `job_queue` row's gate tier MUST come from the spec that
    /// executes; a mismatch is a widening/narrowing attempt refused before any row is written.
    TrustTierMismatch {
        /// The tier the enqueue declared (the would-be `job_queue.trust_tier`).
        enqueue: &'static str,
        /// The dispatched spec's real tier (the truth the gate must carry).
        spec: &'static str,
    },
    /// The spec's `timeout_secs` exceeds [`MAX_JOB_TIMEOUT_SECS`] — refused fail-closed so a leased
    /// job can never outlive the runner's lease (the CT-004c.2 double-run guard, closed at dispatch).
    TimeoutTooLong {
        /// The spec's requested wall-clock timeout.
        requested: u32,
        /// The enforced ceiling.
        ceiling: u32,
    },
    /// A dispatch identity row exists but its `stage` column is NULL (a pre-rewire historical row the
    /// `ci_0015a` ALTER could not back-fill). The reporter fails closed rather than attribute a verdict
    /// to an unknown stage — a NULL stage can never be a fabricated pass.
    MissingStage {
        /// The job id whose durable stage is absent.
        job_id: String,
    },
    /// The enqueue's declared `claim_window_secs` does not equal what the dispatched spec's own
    /// topology derives — refused before any row is written, the same posture as
    /// [`Self::TrustTierMismatch`]. The durable scalar is CACHED authority, never a second authority
    /// source: a caller may not widen (or narrow) the immutable claim ceiling the runner will get.
    ClaimWindowMismatch {
        /// The window the enqueue declared (the would-be `job_queue.claim_window_secs`).
        enqueue: i64,
        /// The window the dispatched spec derives (the truth the queue row must carry).
        spec: i64,
    },
    /// The dispatched spec's claim window is underivable at all (a malformed checkout workspace or
    /// an over-ceiling timeout). Refused fail-closed rather than defaulted.
    ClaimWindowUnderivable(String),
}

impl core::fmt::Display for CiJobSpecStoreError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CiJobSpecStoreError::Db(e) => write!(f, "durable ci_job_spec store error: {e}"),
            CiJobSpecStoreError::BadId { field, value } => write!(
                f,
                "durable ci_job_spec op refused: {field} `{value}` is not a UUID (the \
                 ci_job_spec.{field} column is uuid — CI job/run ids are uuids in production)"
            ),
            CiJobSpecStoreError::SpecNotFound { tenant_id, job_id } => write!(
                f,
                "no durable launch template for tenant `{tenant_id}` job `{job_id}` — the runner \
                 cannot resolve the leased job (fail-closed; the row stays leased for the reaper)"
            ),
            CiJobSpecStoreError::CorruptSpec { job_id, detail } => write!(
                f,
                "corrupt durable launch template for job `{job_id}` (jsonb decode failed closed): \
                 {detail}"
            ),
            CiJobSpecStoreError::SpecEncode(e) => {
                write!(f, "durable ci_job_spec persist refused: launch template did not serialize to jsonb: {e}")
            }
            CiJobSpecStoreError::TrustTierMismatch { enqueue, spec } => write!(
                f,
                "durable dispatch refused (SECURITY): the job_queue row's trust_tier `{enqueue}` does \
                 not match the dispatched spec's trust_tier `{spec}` — the claim-gating tier MUST come \
                 from the spec that executes (no widening/defaulting)"
            ),
            CiJobSpecStoreError::TimeoutTooLong { requested, ceiling } => write!(
                f,
                "durable dispatch refused: spec timeout_secs {requested} exceeds the {ceiling}s \
                 ceiling — a job may not outlive the runner's lease (double-run guard, fail-closed)"
            ),
            CiJobSpecStoreError::MissingStage { job_id } => write!(
                f,
                "durable ci_job_spec for job `{job_id}` has a NULL stage (a pre-rewire historical row) \
                 — the reporter fails closed rather than attribute a verdict to an unknown stage"
            ),
            CiJobSpecStoreError::ClaimWindowMismatch { enqueue, spec } => write!(
                f,
                "durable dispatch refused: the job_queue row's claim_window_secs {enqueue} does not \
                 match the {spec}s window the dispatched spec's own topology derives — the immutable \
                 claim ceiling MUST come from the spec that executes (no widening/defaulting)"
            ),
            CiJobSpecStoreError::ClaimWindowUnderivable(detail) => write!(
                f,
                "durable dispatch refused: {detail}"
            ),
        }
    }
}

impl std::error::Error for CiJobSpecStoreError {}

impl From<PgError> for CiJobSpecStoreError {
    fn from(error: PgError) -> Self {
        Self::from_pg(error)
    }
}

impl CiJobSpecStoreError {
    /// Map a storage-layer [`PgError`] into the store's typed error (fail-loud, no swallow).
    fn from_pg(e: PgError) -> Self {
        CiJobSpecStoreError::Db(e.to_string())
    }
}

// =================================================================================================
// The outcome of a co-persist dispatch.
// =================================================================================================

/// The result of [`CiJobSpecStore::co_persist_dispatch`] — whether each of the two co-committed rows
/// was freshly INSERTED or an idempotent no-op (the effectively-once guarantee firing). Both being a
/// duplicate is a re-dispatch that correctly created nothing new.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DispatchOutcome {
    /// The `job_queue` enqueue outcome (Inserted | DuplicateIdem on `jq_idem`).
    pub enqueue: EnqueueOutcome,
    /// Whether the `ci_job_spec` row was freshly inserted (false = the `(tenant, job_id)` PK collapsed
    /// a re-dispatch to a no-op — the spec was already persisted).
    pub spec_inserted: bool,
}

// =================================================================================================
// The store.
// =================================================================================================

/// **The REAL durable CI `ci_job_spec` store (CT-004d.1).** Holds the OLTP [`PgPool`] and persists /
/// resolves a dispatched stage's [`JobSpec`] as a single `spec jsonb` column, each under the
/// tenant-/region-scoped transaction the FORCE-RLS `ci_job_spec` table requires (the CT-004c.1
/// [`with_tenant_tx`] pattern). Cloneable (the pool is an `Arc`-backed handle). Named `…Store` +
/// carries a `PgPool` field so the `no-in-memory-durable-store` scanner reads it as a genuine durable
/// store. The caller must have applied the CI control-plane migrations (which create `ci_job_spec` +
/// `job_queue`) — the ci-controlplane `serve(AppSpec)` boot migrate does this.
#[derive(Clone)]
pub struct CiJobSpecStore {
    pool: PgPool,
}

impl CiJobSpecStore {
    /// Wrap the controlplane OLTP pool as the durable `ci_job_spec` store. The production composition
    /// root constructs this from the MR-022 `SubstrateProvider` pool ([`crate::ci_job_spec_store`]).
    pub fn with_pg(pool: PgPool) -> CiJobSpecStore {
        CiJobSpecStore { pool }
    }

    /// The pool this store is bound to.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// **Co-persist a dispatched stage: the `job_queue` row + its [`JobSpec`], in ONE tenant-scoped tx
    /// (arch §3.3 step 1).** Writes the `job_queue` enqueue (via the byte-identical
    /// [`INSERT_JOB_QUEUE_QUERY`]) AND the `ci_job_spec` row atomically, so a crash between them cannot
    /// leave a claimable job with no resolvable spec. Both are idempotent (the enqueue on
    /// `(tenant, idem_token)`, the spec on `(tenant, job_id)`) — a re-dispatch / reaper re-queue
    /// collapses to ONE of each.
    ///
    /// **SECURITY (the gate is fed, not bypassed):** the `job_queue` row's `trust_tier` is written as
    /// `enq.trust_tier`, and this refuses fail-closed ([`CiJobSpecStoreError::TrustTierMismatch`]) if
    /// that does not equal `spec.trust_tier` — so the claim-gating tier ALWAYS equals the tier of the
    /// spec that executes (an `untrusted_fork` spec can never be enqueued behind a widened `trusted`
    /// gate). The `region` is `enq.region` (the run's honest residency pin). The spec's `timeout_secs`
    /// is capped at [`MAX_JOB_TIMEOUT_SECS`] (else [`CiJobSpecStoreError::TimeoutTooLong`]) so a leased
    /// job can never outlive the runner's lease.
    pub async fn co_persist_dispatch(
        &self,
        enq: &DurableEnqueue,
        launch: &DurableCiJobLaunchTemplate,
        stage: &str,
    ) -> Result<DispatchOutcome, CiJobSpecStoreError> {
        self.co_persist_dispatch_inner(enq, launch, stage, false)
            .await
    }

    /// Co-persist a manifest dispatch only while its exact Flow run is durably active.
    ///
    /// The workflow row is locked before the queue row in this same transaction. Run
    /// supersession takes the same Flow→queue order, so either dispatch commits first and
    /// cancellation observes the row, or cancellation terminates Flow first and this write refuses.
    pub async fn co_persist_active_flow_dispatch(
        &self,
        enq: &DurableEnqueue,
        launch: &DurableCiJobLaunchTemplate,
        stage: &str,
    ) -> Result<DispatchOutcome, CiJobSpecStoreError> {
        self.co_persist_dispatch_inner(enq, launch, stage, true)
            .await
    }

    async fn co_persist_dispatch_inner(
        &self,
        enq: &DurableEnqueue,
        launch: &DurableCiJobLaunchTemplate,
        stage: &str,
        require_active_flow: bool,
    ) -> Result<DispatchOutcome, CiJobSpecStoreError> {
        // ── the two fail-closed dispatch invariants (SECURITY trust-tier + the lease-TTL floor). ──
        validate_dispatch(enq.trust_tier, Some(enq.claim_window_secs), launch)?;

        let job_uuid = parse_id_local("job_id", &enq.job_id)?;
        let run_uuid = parse_id_local("run_id", &enq.run_id)?;
        let spec_json = serde_json::to_value(launch)
            .map_err(|e| CiJobSpecStoreError::SpecEncode(e.to_string()))?;
        let stage = stage.to_string();

        let labels = enq.labels.clone();
        let group = enq.concurrency_group.clone();
        let lane = enq.lane.as_str();
        let trust = trust_token(enq.trust_tier);
        let tenant_id = enq.tenant_id.clone();
        let region = enq.region.clone();
        let fair_key = enq.fair_key.clone();
        let idem = enq.idem_token.clone();
        let workflow_run_id = enq.run_id.clone();
        let claim_window_secs = enq.claim_window_secs;
        if enq.stage != stage {
            return Err(CiJobSpecStoreError::Db(
                "durable dispatch stage differs between queue authority and spec identity".into(),
            ));
        }

        let (enqueued, spec_inserted) =
            with_tenant_tx(&self.pool, &enq.tenant_id, &enq.region, move |conn| {
                Box::pin(async move {
                    if require_active_flow {
                        let state = sqlx::query_scalar::<_, String>(
                            "SELECT state FROM workflow_run \
                             WHERE tenant_id = $1 AND region = $2 AND run_id = $3 FOR UPDATE",
                        )
                        .bind(&tenant_id)
                        .bind(&region)
                        .bind(&workflow_run_id)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(|e| PgError::Query(e.to_string()))?;
                        if state.as_deref() != Some("running") {
                            return Err(PgError::Query(
                                "manifest dispatch refused: owning Flow run is not active".into(),
                            ));
                        }
                    }
                    // (1) the job_queue row (the eligibility gate) — idempotent on jq_idem.
                    let jq_row = sqlx::query(INSERT_JOB_QUEUE_QUERY)
                        .bind(&tenant_id) // $1 tenant_id (the RLS/tenant predicate)
                        .bind(&region) // $2 region
                        .bind(job_uuid) // $3 job_id
                        .bind(run_uuid) // $4 run_id
                        .bind(lane) // $5 lane
                        .bind(&labels) // $6 labels text[]
                        .bind(trust) // $7 trust_tier (== spec.trust_tier, verified above)
                        .bind(group.as_deref()) // $8 concurrency_group (nullable)
                        .bind(&fair_key) // $9 fair_key
                        .bind(&idem) // $10 idem_token
                        .bind(&stage) // $11 stage (regional guard + completion authority)
                        .bind(claim_window_secs) // $12 immutable dispatch-derived claim window
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(|e| PgError::Query(e.to_string()))?;
                    // (2) the spec row (what EXECUTES) — idempotent on the (tenant, job_id) PK.
                    let spec_row = sqlx::query(INSERT_JOB_SPEC_QUERY)
                        .bind(&tenant_id) // $1 tenant_id (the RLS/tenant predicate)
                        .bind(&region) // $2 region
                        .bind(job_uuid) // $3 job_id
                        .bind(run_uuid) // $4 run_id
                        .bind(&idem) // $5 idem_token (co-key with the queue row)
                        .bind(&spec_json) // $6 spec jsonb (the whole value — faithful round-trip)
                        .bind(&stage) // $7 stage (the durable verdict-attribution name)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(|e| PgError::Query(e.to_string()))?;
                    let exact = sqlx::query(SELECT_EXACT_DISPATCH_QUERY)
                        .bind(&tenant_id)
                        .bind(&idem)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(|e| PgError::Query(e.to_string()))?
                        .ok_or_else(|| {
                            PgError::Query(
                                "durable dispatch replay readback found no joined queue/spec row"
                                    .into(),
                            )
                        })?;
                    verify_exact_dispatch(
                        &exact,
                        &region,
                        job_uuid,
                        run_uuid,
                        lane,
                        &labels,
                        trust,
                        group.as_deref(),
                        &fair_key,
                        &idem,
                        &stage,
                        claim_window_secs,
                        &spec_json,
                    )?;
                    Ok((jq_row.is_some(), spec_row.is_some()))
                })
            })
            .await
            .map_err(CiJobSpecStoreError::from_pg)?;

        Ok(DispatchOutcome {
            enqueue: if enqueued {
                EnqueueOutcome::Inserted
            } else {
                EnqueueOutcome::DuplicateIdem
            },
            spec_inserted,
        })
    }

    /// **Resolve a leased job's persisted launch template (the runner's resolve path — arch §5.3).**
    /// Reads the `spec jsonb` for `(tenant, job_id)` under the tenant-scoped tx and deserializes the
    /// whole non-launchable template with no lossy projection. Fail-closed: no row →
    /// [`CiJobSpecStoreError::SpecNotFound`]; an un-decodable jsonb → [`CiJobSpecStoreError::CorruptSpec`].
    /// NEVER a fabricated default. `region` is the runner's
    /// residency region (the RLS/tenant-tx scope).
    pub async fn get_launch_template(
        &self,
        tenant_id: &str,
        region: &str,
        job_id: &str,
    ) -> Result<DurableCiJobLaunchTemplate, CiJobSpecStoreError> {
        let job_uuid = parse_id_local("job_id", job_id)?;
        let tenant_id_owned = tenant_id.to_string();
        let row = with_tenant_tx(&self.pool, tenant_id, region, move |conn| {
            Box::pin(async move {
                sqlx::query(SELECT_JOB_SPEC_QUERY)
                    .bind(&tenant_id_owned)
                    .bind(job_uuid)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|error| PgError::Query(error.to_string()))
            })
        })
        .await
        .map_err(CiJobSpecStoreError::from_pg)?;
        let row = row.ok_or_else(|| CiJobSpecStoreError::SpecNotFound {
            tenant_id: tenant_id.to_string(),
            job_id: job_id.to_string(),
        })?;
        let spec_json: serde_json::Value = row
            .try_get("spec")
            .map_err(|error| CiJobSpecStoreError::Db(error.to_string()))?;
        decode_launch_template(job_id, spec_json)
    }

    /// Resolve the complete immutable launch template inside a caller-owned tenant transaction.
    /// Final pre-spawn authorization uses this form so executable-spec verification and the exact
    /// scheduler-generation lock are read under one scoped transaction.
    pub(crate) async fn get_launch_template_on_conn(
        &self,
        conn: &mut sqlx::PgConnection,
        tenant_id: &str,
        job_id: &str,
    ) -> Result<DurableCiJobLaunchTemplate, CiJobSpecStoreError> {
        let job_uuid = parse_id_local("job_id", job_id)?;
        let row = sqlx::query(SELECT_JOB_SPEC_QUERY)
            .bind(tenant_id)
            .bind(job_uuid)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|error| CiJobSpecStoreError::Db(error.to_string()))?
            .ok_or_else(|| CiJobSpecStoreError::SpecNotFound {
                tenant_id: tenant_id.to_string(),
                job_id: job_id.to_string(),
            })?;
        let spec_json: serde_json::Value = row
            .try_get("spec")
            .map_err(|e| CiJobSpecStoreError::Db(e.to_string()))?;
        decode_launch_template(job_id, spec_json)
    }

    /// **Read a dispatched job's durable claimed-identity for `(tenant, job_id)` — the reporter's
    /// fail-closed verification anchor (CT-004d.2 rewire).** Returns the `(run_id, idem_token, stage)`
    /// the dispatch persisted, or `None` if no row exists (a forged/mis-keyed completion). The reporter
    /// derives the completed job's `job_id`, reads this, and refuses unless the presented `(run_id,
    /// idem_token)` match the durable record — proving the caller owns a real dispatched job before any
    /// verdict is signalled. `stage` is the durable verdict-attribution name (a fresh reporter after a
    /// restart reads it here — never an in-memory map). A row with a NULL `stage` (a pre-rewire
    /// historical dispatch) surfaces as [`CiJobSpecStoreError::MissingStage`], never a fabricated verdict.
    pub async fn get_dispatch_identity(
        &self,
        tenant_id: &str,
        region: &str,
        job_id: &str,
    ) -> Result<Option<ClaimedDispatchIdentity>, CiJobSpecStoreError> {
        let job_uuid = parse_id_local("job_id", job_id)?;
        let tenant_id_owned = tenant_id.to_string();
        let job_id_owned = job_id.to_string();
        let store = self.clone();
        let row = with_tenant_tx_error(&self.pool, tenant_id, region, move |conn| {
            Box::pin(async move {
                store
                    .get_dispatch_identity_on_conn(conn, &tenant_id_owned, job_uuid, &job_id_owned)
                    .await
            })
        })
        .await?;
        Ok(row)
    }

    pub(crate) async fn get_dispatch_identity_on_conn(
        &self,
        conn: &mut sqlx::PgConnection,
        tenant_id: &str,
        job_id: Uuid,
        job_id_text: &str,
    ) -> Result<Option<ClaimedDispatchIdentity>, CiJobSpecStoreError> {
        let row = sqlx::query(SELECT_JOB_SPEC_IDENTITY_QUERY)
            .bind(tenant_id)
            .bind(job_id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| CiJobSpecStoreError::Db(e.to_string()))?;
        let Some(row) = row else {
            return Ok(None);
        };
        let run_id: String = row
            .try_get("run_id")
            .map_err(|e| CiJobSpecStoreError::Db(e.to_string()))?;
        let idem_token: String = row
            .try_get("idem_token")
            .map_err(|e| CiJobSpecStoreError::Db(e.to_string()))?;
        let stage: Option<String> = row
            .try_get("stage")
            .map_err(|e| CiJobSpecStoreError::Db(e.to_string()))?;
        let stage = stage.ok_or_else(|| CiJobSpecStoreError::MissingStage {
            job_id: job_id_text.to_string(),
        })?;
        let spec_json: serde_json::Value = row
            .try_get("spec")
            .map_err(|e| CiJobSpecStoreError::Db(e.to_string()))?;
        let launch = decode_launch_template(job_id_text, spec_json)?;
        Ok(Some(ClaimedDispatchIdentity {
            run_id,
            idem_token,
            stage,
            reserve_handle: launch.spec.meter_to.reserve_id,
        }))
    }
}

#[allow(clippy::too_many_arguments)]
fn verify_exact_dispatch(
    row: &sqlx::postgres::PgRow,
    region: &str,
    job_id: Uuid,
    run_id: Uuid,
    lane: &str,
    labels: &[String],
    trust: &str,
    concurrency_group: Option<&str>,
    fair_key: &str,
    idem_token: &str,
    stage: &str,
    claim_window_secs: i64,
    spec: &serde_json::Value,
) -> Result<(), PgError> {
    let exact = row.get::<Option<i64>, _>("queue_claim_window_secs") == Some(claim_window_secs)
        && row.get::<String, _>("region") == region
        && row.get::<String, _>("queue_job_id") == job_id.to_string()
        && row.get::<String, _>("queue_run_id") == run_id.to_string()
        && row.get::<String, _>("lane") == lane
        && row.get::<Vec<String>, _>("labels") == labels
        && row.get::<String, _>("trust_tier") == trust
        && row.get::<Option<String>, _>("concurrency_group").as_deref() == concurrency_group
        && row.get::<String, _>("fair_key") == fair_key
        && row.get::<String, _>("queue_idem_token") == idem_token
        && row.get::<Option<String>, _>("queue_stage").as_deref() == Some(stage)
        && row.get::<String, _>("spec_region") == region
        && row.get::<String, _>("spec_run_id") == run_id.to_string()
        && row.get::<String, _>("spec_idem_token") == idem_token
        && row.get::<serde_json::Value, _>("spec") == *spec
        && row.get::<Option<String>, _>("spec_stage").as_deref() == Some(stage);
    if exact {
        Ok(())
    } else {
        Err(PgError::Query(
            "durable dispatch replay conflicts with the existing queue/spec identity".into(),
        ))
    }
}

/// **The durable claimed-identity a dispatch persisted (CT-004d.2 rewire) — the reporter's fail-closed
/// verification record.** Read by `(tenant, job_id)`; the reporter refuses a completion whose presented
/// `(run_id, idem_token)` do not equal these, and attributes the verdict to `stage` (a durable read, not
/// an in-memory map — restart-safe).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimedDispatchIdentity {
    /// The run the dispatched job belongs to (the workflow the verdict wakes).
    pub run_id: String,
    /// The engine dispatch `idem_token` (the `job.done` idempotency key the runner echoes).
    pub idem_token: String,
    /// The pipeline stage name the verdict is attributed to (the restart-safe stage resolution).
    pub stage: String,
    /// The exact reservation handle embedded in the sandbox spec that executed.
    pub reserve_handle: String,
}

/// **The two fail-closed dispatch invariants (pure, so the unit suite proves them DB-free).**
/// (1) SECURITY — the `job_queue` row's `trust_tier` (`enq_trust`) MUST equal `spec.trust_tier`, so the
/// claim-gating tier always equals the tier of the spec that executes (no widen/default). (2) the
/// lease-TTL floor — `spec.limits.timeout_secs` may not exceed [`MAX_JOB_TIMEOUT_SECS`], so a leased
/// job can never outlive the runner's lease. A violation is a typed fail-closed error, NEVER coerced.
fn validate_dispatch(
    enq_trust: TrustTier,
    enq_claim_window_secs: Option<i64>,
    launch: &DurableCiJobLaunchTemplate,
) -> Result<(), CiJobSpecStoreError> {
    if launch.token_authority_handle.trim().is_empty() || launch.token_authority_handle.len() > 512
    {
        return Err(CiJobSpecStoreError::SpecEncode(
            "token authority handle is empty or overlong".into(),
        ));
    }
    if enq_trust != launch.spec.trust_tier {
        return Err(CiJobSpecStoreError::TrustTierMismatch {
            enqueue: trust_token(enq_trust),
            spec: trust_token(launch.spec.trust_tier),
        });
    }
    if launch.spec.limits.timeout_secs > MAX_JOB_TIMEOUT_SECS {
        return Err(CiJobSpecStoreError::TimeoutTooLong {
            requested: launch.spec.limits.timeout_secs,
            ceiling: MAX_JOB_TIMEOUT_SECS,
        });
    }
    // (3) the immutable claim ceiling — recomputed from `launch.spec`, exactly the pattern the
    // trust-tier invariant above uses. Deliberately WRITE-PATH ONLY: `None` (the resolve-side
    // decode, which has no caller-supplied window to compare) skips the derivation entirely, so an
    // already-persisted row whose window is underivable stays READABLE. The reporter's settlement
    // path decodes the same template, and refusing to read a dispatched job's identity would strand
    // it; the resolver's own `derive_checkout_authorization_scope` call already fails such a spec
    // closed before launch.
    if let Some(declared) = enq_claim_window_secs {
        let derived = crate::ci_claim_window::claim_window_secs_for_template(&launch.spec)
            .map_err(|error| CiJobSpecStoreError::ClaimWindowUnderivable(error.to_string()))?;
        if declared != derived {
            return Err(CiJobSpecStoreError::ClaimWindowMismatch {
                enqueue: declared,
                spec: derived,
            });
        }
    }
    Ok(())
}

/// **Decode stored `spec jsonb` to a [`DurableCiJobLaunchTemplate`] fail closed.**
/// Pure over the jsonb value so the unit suite proves both the faithful round-trip AND the corrupt →
/// fail-closed behaviour DB-free. An un-decodable value is [`CiJobSpecStoreError::CorruptSpec`] (the
/// stored spec is what executes, so a corrupt one fails the resolve closed) — NEVER a default spec.
fn decode_launch_template(
    job_id: &str,
    spec_json: serde_json::Value,
) -> Result<DurableCiJobLaunchTemplate, CiJobSpecStoreError> {
    let launch = serde_json::from_value::<DurableCiJobLaunchTemplate>(spec_json).map_err(|e| {
        CiJobSpecStoreError::CorruptSpec {
            job_id: job_id.to_string(),
            detail: e.to_string(),
        }
    })?;
    validate_dispatch(launch.spec.trust_tier, None, &launch)?;
    Ok(launch)
}

/// Parse a `job_id`/`run_id` token into the durable `uuid` column type — a non-uuid is a loud refusal.
/// (Delegates to the `job_queue` store's `parse_id`, re-mapping the error into this store's type so the
/// UUID-column rule is authored ONCE.)
fn parse_id_local(
    field: &'static str,
    value: &str,
) -> Result<sqlx::types::Uuid, CiJobSpecStoreError> {
    parse_id(field, value).map_err(|_| CiJobSpecStoreError::BadId {
        field,
        value: value.to_string(),
    })
}

#[cfg(test)]
#[path = "job_spec_store_tests.rs"]
mod tests;
