//! # `ci_run_store` — CT-004d.2 chunk 4: the durable `ci_run` writer (the CI run-of-record)
//!
//! **Owning architecture doc (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md`
//! §1 (trigger → dispatch: match → dedup → trust-stamp → resolve → reserve/start) + arch 01 §3.1 (the
//! `ci_run` thin index over the myelin-flow workflow run).
//!
//! ## The gap this closes (grounded by the CT-004b scout)
//! The `ci-dispatch.trigger` consumer's reserve bundle is supposed to persist a durable `ci_run` row
//! (`state = queued`) — the run-of-record every downstream reader (the surfacing scan, the run-view,
//! the check-emitter) reads. But the PRODUCTION reserve store
//! ([`myelin_ci_dispatch::OutboxReserveStore`]) only `stage_state_change`'d a NOTE — it NEVER wrote
//! the row. The row was written durably ONLY in the integration test's `CoCommitReserveStore` (which
//! CAN name `sqlx`, so it rode the co-commit connection and proved the TRUE shape). This module is the
//! PRODUCTION durable `ci_run` writer that test proved: a `PgPool`-holding `…Store` running
//! byte-identical SQL, mirroring [`crate::job_spec_store::CiJobSpecStore`].
//!
//! ## The co-commit shape (the load-bearing invariant) — and the honest events-stay-absorb split
//! The `ci_run` ROW is the exactly-once RUN-OF-RECORD, so it MUST land atomically with the consumer's
//! dedup mark (the #7 / MR-023b floor): a crash (rollback) between them leaves NEITHER, a redelivery
//! re-runs and lands both exactly once. [`CiRunStore::co_commit_insert`] writes the row on the
//! consumer's co-commit `HandlerTx` connection (downcast `tx.connection::<sqlx::PgConnection>()`) — the
//! SAME `sqlx` transaction the dedup mark is in — so the row + the mark commit or roll back as ONE unit
//! (the runtime commits the tx on `Done`, rolls it back on `Retry`/failure — `myelin_events::consumer`).
//!
//! **HONEST SCOPE (the #7 H1 finding, unchanged):** the co-emitted `ci.run.started` / `ci.check.updated`
//! EVENTS still go through the OUTBOX (which owns its OWN pool — the ci-dispatch leaf crate cannot name
//! `sqlx` outside `--features integration`), so they stay on ABSORB mode (`commit_absorb` →
//! `ON CONFLICT (event_id) DO NOTHING`, idempotent on the deterministic ids). This chunk co-commits the
//! run-of-record ROW with the mark; the events remain absorb-idempotent. Forcing the outbox events onto
//! the external connection was H1's REJECTED path (the leaf crate has no `sqlx` there) — NOT re-opened.
//!
//! ## Idempotency + RLS (fail-closed)
//! `ON CONFLICT (tenant_id, run_id) DO NOTHING` — a redelivered trigger mints the SAME deterministic
//! `run_id` (derived from the triggering `event_id`), then a separate locking read verifies every
//! immutable field before classifying it as an exact replay. A collision is typed and loud. Every
//! write is `(tenant, region)`-scoped: the pool-based [`CiRunStore::insert_ci_run`] acquires through
//! [`with_tenant_tx_error`] so domain errors survive the RESHAPE-002 FORCE-RLS transaction convention;
//! [`CiRunStore::get_ci_run`] uses [`with_tenant_tx`], and the co-commit path rides the caller's scoped
//! transaction. Named `…Store` + carries a `PgPool` so the `no-in-memory-durable-store` scanner reads
//! it as a genuine durable store.
//!
//! ## Out of scope (named, not built — the CT-004d.2 chunk split)
//! This is ONLY the durable `ci_run` writer + its co-commit wiring. It does NOT register/drive the
//! `ci.pipeline` body (chunk 2), call `DurableExecutor::start` (chunk 3), or touch the
//! scheduler/runner (chunk 5). The `ci_run` row it writes carries the PRE-MINTED `wf_run_id` those
//! chunks start the workflow with.

use myelin_storage::{with_tenant_tx, with_tenant_tx_error, PgError};
use sqlx::postgres::PgPool;
use sqlx::Row;

/// **The `ci_run` row a reserve/start bundle persists (all the `CREATE_CI_RUN_DDL` columns the writer
/// binds).** Owned `String`s so the durable store binds them directly (the `uuid` columns are bound as
/// text and cast `$n::uuid` in SQL — the SAME posture the CT-004b integration test proved). The
/// NULLABLE columns (`repo_ref` / `commit_oid` / `cause_event_id` / `triggered_by`) are `Option` — a
/// reserve bundle that does not carry them writes `NULL` (the DDL permits it), never a fabricated value.
///
/// `ci-dispatch` builds this from its `ArmedRun` (`ci_run_insert_from_armed`); the mapping lives THERE
/// (this crate cannot name `ArmedRun` — that edge would be a cycle).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiRunInsert {
    /// `ci_run.tenant_id` — the RLS/tenant partition key (the VERIFIED tenant, never a URL path).
    pub tenant_id: String,
    /// `ci_run.region` — the residency pin (the RLS `(tenant, region)` predicate half).
    pub region: String,
    /// `ci_run.run_id` (uuid) — the PK half; deterministic from the triggering `event_id` (the
    /// `ON CONFLICT` idempotency anchor).
    pub run_id: String,
    /// `ci_run.project_id` (uuid) — a deterministic placeholder from the repo ref (the repo→project
    /// registry is the named floor).
    pub project_id: String,
    /// `ci_run.pipeline_id` (uuid) — a deterministic placeholder (named floor).
    pub pipeline_id: String,
    /// `ci_run.wf_run_id` (uuid) — PRE-MINTED here; CT-004d.2 chunk 3 starts the workflow with it.
    pub wf_run_id: String,
    /// `ci_run.definition_snapshot` — the content-addressed CAS blob ref the run runs (NOT NULL).
    pub definition_snapshot: String,
    /// `ci_run.trigger_kind` — the CHECK token (`push`/`pull_request`/…), NOT NULL.
    pub trigger_kind: String,
    /// `ci_run.trust_tier` — the stamped CHECK token (`trusted`/`untrusted_fork`/`self_hosted`),
    /// NOT NULL; the SAME value stamped on every `ci.check.updated.trust_tier` (X-1).
    pub trust_tier: String,
    /// `ci_run.state` — the lifecycle state at reserve, always `queued`, NOT NULL.
    pub state: String,
    /// `ci_run.correlation_id` — the triggering envelope's correlation, NOT NULL.
    pub correlation_id: String,
    /// `ci_run.cause_event_id` (nullable) — the triggering `event_id` (the cause provenance).
    pub cause_event_id: Option<String>,
    /// Depth of the triggering envelope retained for saturating child derivation.
    pub cause_depth: i64,
    /// Originating human/session action inherited by later lifecycle facts.
    pub caused_by: Option<String>,
    /// `ci_run.repo_ref` (nullable) — the repo the run ran against (the check-seam / run-view key half).
    pub repo_ref: Option<String>,
    /// `ci_run.commit_oid` (nullable) — the commit the run ran against (the CheckStatus key half).
    pub commit_oid: Option<String>,
    /// `ci_run.triggered_by` (nullable) — the acting PSEUDONYM subject (contract 4.8), never a raw
    /// name/email. `None` if the reserve bundle does not carry it (the CT-004b proven shape).
    pub triggered_by: Option<String>,
}

/// A `ci_run` row read back (the run-view / check-emitter resolve path). The durable columns as owned
/// `String`s (the `uuid` columns rendered `::text`). This is a thin read record — NOT the
/// PII-classification mirror ([`crate::schema::CiRunRow`]), which tags the same table for the GDPR lint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiRunRecord {
    /// `ci_run.tenant_id` — the authoritative tenant partition carried into the per-tenant
    /// pipeline starter. Omitting this made it possible for a composition root to stamp a
    /// synthetic/fixed tenant onto another tenant's durable run.
    pub tenant_id: String,
    /// `ci_run.run_id` (uuid rendered text).
    pub run_id: String,
    /// `ci_run.region`.
    pub region: String,
    /// `ci_run.project_id` (uuid rendered text).
    pub project_id: String,
    /// `ci_run.pipeline_id` (uuid rendered text).
    pub pipeline_id: String,
    /// `ci_run.wf_run_id` (uuid rendered text).
    pub wf_run_id: String,
    /// `ci_run.repo_ref` (nullable).
    pub repo_ref: Option<String>,
    /// `ci_run.commit_oid` (nullable).
    pub commit_oid: Option<String>,
    /// `ci_run.cause_event_id` (nullable).
    pub cause_event_id: Option<String>,
    /// `ci_run.cause_depth`.
    pub cause_depth: i64,
    /// `ci_run.caused_by` (nullable).
    pub caused_by: Option<String>,
    /// `ci_run.definition_snapshot`.
    pub definition_snapshot: String,
    /// `ci_run.trigger_kind`.
    pub trigger_kind: String,
    /// `ci_run.trust_tier`.
    pub trust_tier: String,
    /// `ci_run.state`.
    pub state: String,
    /// `ci_run.correlation_id`.
    pub correlation_id: String,
}

/// **INSERT a `ci_run` row, idempotent on the `(tenant_id, run_id)` PK.** Binds every column the writer
/// sets (the `uuid` columns cast `$n::uuid` from text); the NULLABLE columns bind `Option` (→ `NULL`
/// when `None`); `cost_settled` / `created_at` / `finished_at` take their DDL defaults.
/// `ON CONFLICT (tenant_id, run_id) DO NOTHING` makes the initial write non-destructive. On conflict,
/// [`VERIFY_CI_RUN_REPLAY_QUERY`] must verify the immutable identity in a separate statement before
/// the operation can return `false`; a divergent or invisible conflict is an error.
pub const INSERT_CI_RUN_QUERY: &str = "\
INSERT INTO ci_run (
  tenant_id, region, run_id, project_id, pipeline_id, wf_run_id,
  repo_ref, commit_oid, cause_event_id, cause_depth, caused_by, definition_snapshot,
  trigger_kind, triggered_by, trust_tier, state, correlation_id
) VALUES (
  $1, $2, $3::uuid, $4::uuid, $5::uuid, $6::uuid,
  $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17
)
ON CONFLICT (tenant_id, run_id) DO NOTHING
RETURNING run_id";

/// Read the immutable run identity after an `INSERT ... DO NOTHING` conflict. This deliberately is
/// a **second statement**: under PostgreSQL READ COMMITTED, the statement that lost a concurrent
/// insert race can observe the unique conflict without being able to read the winner in that same
/// statement's snapshot. A new statement receives a new snapshot after the winner commits.
///
/// The explicit `(tenant, region, run)` predicate is defence in depth alongside FORCE RLS. If the
/// unique conflict belongs to another residency region, the row remains invisible and the caller
/// returns [`CiRunStoreError::ConflictNotVisible`] without disclosing either region.
pub const VERIFY_CI_RUN_REPLAY_QUERY: &str = "\
SELECT
  region = $2                                      AS region_matches,
  project_id = $4::uuid                            AS project_id_matches,
  pipeline_id = $5::uuid                           AS pipeline_id_matches,
  wf_run_id = $6::uuid                             AS wf_run_id_matches,
  repo_ref IS NOT DISTINCT FROM $7::text           AS repo_ref_matches,
  commit_oid IS NOT DISTINCT FROM $8::text         AS commit_oid_matches,
  cause_event_id IS NOT DISTINCT FROM $9::text     AS cause_event_id_matches,
  cause_depth = $10                                 AS cause_depth_matches,
  caused_by IS NOT DISTINCT FROM $11::text          AS caused_by_matches,
  definition_snapshot = $12                        AS definition_snapshot_matches,
  trigger_kind = $13                               AS trigger_kind_matches,
  trust_tier = $14                                 AS trust_tier_matches,
  correlation_id = $15                             AS correlation_id_matches,
  (triggered_by IS NOT DISTINCT FROM $16::text
    OR triggered_by = $17::text)                   AS triggered_by_matches
FROM ci_run
WHERE tenant_id = $1 AND region = $2 AND run_id = $3::uuid
FOR KEY SHARE";

/// **Read a `ci_run` row back by `(tenant_id, run_id)` (the run-view / check-emitter resolve path).**
/// The `uuid` columns are rendered `::text` so the read record is a plain `String`. Keyed on the RLS
/// tenant predicate + the run PK; `region` is the RLS scope (the `(tenant, region)` GUC the tx sets).
pub const SELECT_CI_RUN_QUERY: &str = "\
SELECT
  tenant_id              AS tenant_id,
  run_id::text            AS run_id,
  region                  AS region,
  project_id::text        AS project_id,
  pipeline_id::text       AS pipeline_id,
  wf_run_id::text         AS wf_run_id,
  repo_ref                AS repo_ref,
  commit_oid              AS commit_oid,
  cause_event_id          AS cause_event_id,
  cause_depth             AS cause_depth,
  caused_by               AS caused_by,
  definition_snapshot     AS definition_snapshot,
  trigger_kind            AS trigger_kind,
  trust_tier              AS trust_tier,
  state                   AS state,
  correlation_id          AS correlation_id
FROM ci_run WHERE tenant_id = $1 AND run_id = $2::uuid";

/// A durable `ci_run`-store failure. Loud + typed — a write/read NEVER silently drops or coerces. Safe
/// to log: carries only structural faults and immutable field names, never replay values.
///
/// This enum is non-exhaustive because the durable store can gain new fail-closed checks. Callers
/// must retain a fallback arm instead of treating the current variants as the complete failure set.
#[non_exhaustive]
#[derive(Debug, PartialEq, Eq)]
pub enum CiRunStoreError {
    /// A durable-store DB error (the statement did NOT succeed) — never a silent partial write.
    Db(String),
    /// Reserve/start may create a run only in the canonical initial state (`queued`). Checked before
    /// opening a transaction or executing SQL.
    InvalidInitialState,
    /// The retained triggering-envelope depth cannot be represented by the canonical `u32`
    /// envelope depth. Checked before opening a transaction or executing SQL.
    InvalidCausalDepth,
    /// The primary key already exists, but its immutable run identity differs from this replay.
    /// Field names only: submitted and stored values are deliberately never exposed in the error.
    ReplayCollision {
        /// Immutable fields that differ, in stable schema order.
        differing_fields: Vec<&'static str>,
    },
    /// The insert proved a primary-key conflict, but the explicitly tenant/region-scoped verification
    /// read could not see the row. This fails closed without disclosing where the row resides.
    ConflictNotVisible,
    /// The co-commit [`myelin_events::HandlerTx`] carried NO connection (a durable handler on the
    /// in-memory / no-tx path). The writer FAILS CLOSED here (never a write outside the co-commit tx —
    /// that re-opens the at-most-once #7 bug), so the handler returns `Retry` and the redelivery
    /// re-runs on a real co-commit tx.
    NoCoCommitTx,
}

impl core::fmt::Display for CiRunStoreError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CiRunStoreError::Db(e) => write!(f, "durable ci_run store error: {e}"),
            CiRunStoreError::InvalidInitialState => {
                write!(f, "durable ci_run insert requires the queued initial state")
            }
            CiRunStoreError::InvalidCausalDepth => write!(
                f,
                "durable ci_run insert requires cause_depth in the canonical u32 range"
            ),
            CiRunStoreError::ReplayCollision { differing_fields } => write!(
                f,
                "durable ci_run replay collided on immutable fields: {}",
                differing_fields.join(", ")
            ),
            CiRunStoreError::ConflictNotVisible => write!(
                f,
                "durable ci_run conflict could not be verified in the active tenant scope"
            ),
            CiRunStoreError::NoCoCommitTx => write!(
                f,
                "durable ci_run co-commit refused: the HandlerTx carried no co-commit connection \
                 (a durable handler fails closed rather than write the run-of-record outside the \
                 dedup mark's transaction — the #7 at-most-once floor)"
            ),
        }
    }
}

impl std::error::Error for CiRunStoreError {}

impl From<PgError> for CiRunStoreError {
    fn from(e: PgError) -> Self {
        Self::Db(e.to_string())
    }
}

/// **The REAL durable CI `ci_run` store (CT-004d.2 chunk 4) — the run-of-record writer.** Holds the
/// OLTP [`PgPool`] and writes / reads the `ci_run` row, mirroring [`crate::job_spec_store::CiJobSpecStore`].
/// Two write paths, both exact-replay safe on `(tenant_id, run_id)`:
///
/// - **[`co_commit_insert`](CiRunStore::co_commit_insert) (the PRODUCTION reserve path):** writes the
///   row on the consumer's co-commit `HandlerTx` connection — the SAME tx as the dedup mark — so the
///   run-of-record + the mark are ATOMIC (the load-bearing invariant). Does NOT use the pool (it rides
///   the caller's connection); needs a `tokio` runtime handle to bridge the async `sqlx` write to the
///   sync `ReserveStore::persist` body.
/// - **[`insert_ci_run`](CiRunStore::insert_ci_run) (the pool-based standalone write):** acquires
///   through [`with_tenant_tx_error`] (the typed FORCE-RLS convention) — the round-trip /
///   control-plane-owned write.
///
/// Plus [`get_ci_run`](CiRunStore::get_ci_run), the run-view / check-emitter read. Cloneable (the pool
/// is an `Arc`-backed handle). The caller must have applied the CI durable migrations (which create
/// `ci_run` — [`crate::ci_durable_migrations`], applied at BOTH CI mains' boot).
#[derive(Clone)]
pub struct CiRunStore {
    pool: PgPool,
}

impl CiRunStore {
    /// Wrap the OLTP pool as the durable `ci_run` store (mirror [`crate::job_spec_store::CiJobSpecStore::with_pg`]).
    /// The production composition root constructs this from the MR-022 `SubstrateProvider` pool
    /// ([`crate::ci_run_store`]).
    pub fn with_pg(pool: PgPool) -> CiRunStore {
        CiRunStore { pool }
    }

    /// The pool this store is bound to.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// **Co-commit the `ci_run` ROW on the consumer's co-commit connection (the run-of-record ⇄ dedup
    /// mark atomicity — the load-bearing invariant).** Downcasts `tx.connection::<sqlx::PgConnection>()`
    /// (the SAME `sqlx` transaction `DedupLedger::begin_co_commit` opened + inserted the dedup mark
    /// within) and runs [`INSERT_CI_RUN_QUERY`] on it. So the row + the mark commit together (the
    /// runtime commits the tx on `Done`) or roll back together (on `Retry`/failure) — a crash between
    /// leaves NEITHER, and a redelivery re-runs + lands both exactly once. A primary-key conflict is
    /// accepted only after exact immutable replay verification.
    ///
    /// **Fail-closed:** if the handle carries NO connection ([`CiRunStoreError::NoCoCommitTx`]) the
    /// writer refuses — it NEVER writes the run-of-record outside the mark's tx (that re-opens the
    /// at-most-once #7 bug). The caller (`ReserveStore::persist`) maps this to `Retry`.
    ///
    /// **RLS:** the co-commit tx already `set_config('myelin.tenant_id'|'myelin.region', …, true)`
    /// (transaction-scoped) — see `DurableDedupBacking::begin_co_commit` — so this INSERT is
    /// `(tenant, region)`-scoped WITHOUT re-opening a nested tenant transaction. Returns `true` iff a
    /// fresh row was inserted and `false` only for a verified exact immutable replay.
    ///
    /// `rt` bridges the async `sqlx` write to the sync `persist` body (the `PgOutboxBacking` idiom); the
    /// downcast + the `block_on` are HERE (this crate names `sqlx`), so the ci-dispatch leaf crate only
    /// threads the type-erased `tx` through.
    pub fn co_commit_insert(
        &self,
        tx: &mut myelin_events::HandlerTx<'_>,
        row: &CiRunInsert,
        rt: &tokio::runtime::Handle,
    ) -> Result<bool, CiRunStoreError> {
        validate_initial_state(row)?;
        let conn = tx
            .connection::<sqlx::PgConnection>()
            .ok_or(CiRunStoreError::NoCoCommitTx)?;
        tokio::task::block_in_place(|| rt.block_on(insert_on_conn(conn, row)))
    }

    /// **INSERT a `ci_run` row on the store's OWN pool under a tenant-scoped tx (the standalone /
    /// round-trip write).** Acquires through [`with_tenant_tx_error`] (BEGIN → set the `(tenant,
    /// region)` GUC transaction-scoped → INSERT/verify → COMMIT), so it is RLS-isolated, leaves no
    /// residual scope, and preserves typed collision errors. Returns `true` iff fresh and `false`
    /// only for a verified exact immutable replay.
    pub async fn insert_ci_run(&self, row: &CiRunInsert) -> Result<bool, CiRunStoreError> {
        validate_initial_state(row)?;
        let row = row.clone();
        let tenant = row.tenant_id.clone();
        let region = row.region.clone();
        with_tenant_tx_error(&self.pool, &tenant, &region, move |conn| {
            Box::pin(async move { insert_on_conn(conn, &row).await })
        })
        .await
    }

    /// **Read a `ci_run` row back by `(tenant, run_id)` (the run-view / check-emitter resolve path).**
    /// Under a tenant-scoped tx (RLS). `Ok(None)` iff there is no such row for this tenant (a clean
    /// absent — the caller decides whether that is fail-closed for its use).
    pub async fn get_ci_run(
        &self,
        tenant_id: &str,
        region: &str,
        run_id: &str,
    ) -> Result<Option<CiRunRecord>, CiRunStoreError> {
        let tenant_id_owned = tenant_id.to_string();
        let run_owned = run_id.to_string();
        let row = with_tenant_tx(&self.pool, tenant_id, region, move |conn| {
            Box::pin(async move {
                sqlx::query(SELECT_CI_RUN_QUERY)
                    .bind(&tenant_id_owned) // $1 tenant_id (the RLS/tenant predicate)
                    .bind(&run_owned)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|e| PgError::Query(e.to_string()))
            })
        })
        .await
        .map_err(CiRunStoreError::from)?;

        Ok(row.map(|r| CiRunRecord {
            tenant_id: r.get("tenant_id"),
            run_id: r.get("run_id"),
            region: r.get("region"),
            project_id: r.get("project_id"),
            pipeline_id: r.get("pipeline_id"),
            wf_run_id: r.get("wf_run_id"),
            repo_ref: r.get("repo_ref"),
            commit_oid: r.get("commit_oid"),
            cause_event_id: r.get("cause_event_id"),
            cause_depth: r.get("cause_depth"),
            caused_by: r.get("caused_by"),
            definition_snapshot: r.get("definition_snapshot"),
            trigger_kind: r.get("trigger_kind"),
            trust_tier: r.get("trust_tier"),
            state: r.get("state"),
            correlation_id: r.get("correlation_id"),
        }))
    }
}

/// The ONE `ci_run` INSERT execution — bound identically whether it runs on the co-commit connection or
/// the pool's tenant-scoped tx (so the durable write is authored EXACTLY ONCE, no drift). Returns `true`
/// iff a fresh row was inserted (`RETURNING run_id` present). On conflict it executes a mandatory
/// second statement and returns `false` only for a verified exact immutable replay.
async fn insert_on_conn(
    conn: &mut sqlx::PgConnection,
    row: &CiRunInsert,
) -> Result<bool, CiRunStoreError> {
    validate_initial_state(row)?;
    let inserted = sqlx::query(INSERT_CI_RUN_QUERY)
        .bind(&row.tenant_id) // $1 tenant_id (RLS/tenant predicate)
        .bind(&row.region) // $2 region
        .bind(&row.run_id) // $3 run_id ::uuid (PK half)
        .bind(&row.project_id) // $4 project_id ::uuid
        .bind(&row.pipeline_id) // $5 pipeline_id ::uuid
        .bind(&row.wf_run_id) // $6 wf_run_id ::uuid
        .bind(&row.repo_ref) // $7 repo_ref (nullable)
        .bind(&row.commit_oid) // $8 commit_oid (nullable)
        .bind(&row.cause_event_id) // $9 cause_event_id (nullable)
        .bind(row.cause_depth) // $10 cause_depth
        .bind(&row.caused_by) // $11 caused_by (nullable)
        .bind(&row.definition_snapshot) // $12 definition_snapshot
        .bind(&row.trigger_kind) // $13 trigger_kind
        .bind(&row.triggered_by) // $14 triggered_by (nullable pseudonym)
        .bind(&row.trust_tier) // $15 trust_tier
        .bind(&row.state) // $16 state
        .bind(&row.correlation_id) // $17 correlation_id
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| CiRunStoreError::Db(e.to_string()))?;
    if inserted.is_some() {
        return Ok(true);
    }

    // This must remain a separate statement. A data-modifying CTE would reuse the losing INSERT's
    // snapshot and can miss a concurrently committed winner under READ COMMITTED.
    let stored = sqlx::query(VERIFY_CI_RUN_REPLAY_QUERY)
        .bind(&row.tenant_id)
        .bind(&row.region)
        .bind(&row.run_id)
        .bind(&row.project_id)
        .bind(&row.pipeline_id)
        .bind(&row.wf_run_id)
        .bind(&row.repo_ref)
        .bind(&row.commit_oid)
        .bind(&row.cause_event_id)
        .bind(row.cause_depth)
        .bind(&row.caused_by)
        .bind(&row.definition_snapshot)
        .bind(&row.trigger_kind)
        .bind(&row.trust_tier)
        .bind(&row.correlation_id)
        .bind(&row.triggered_by)
        .bind(crate::ERASED_PSEUDONYM)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| CiRunStoreError::Db(e.to_string()))?
        .ok_or(CiRunStoreError::ConflictNotVisible)?;

    let mut differing_fields = Vec::new();
    for (field, matches) in [
        ("region", stored.get::<bool, _>("region_matches")),
        ("project_id", stored.get::<bool, _>("project_id_matches")),
        ("pipeline_id", stored.get::<bool, _>("pipeline_id_matches")),
        ("wf_run_id", stored.get::<bool, _>("wf_run_id_matches")),
        ("repo_ref", stored.get::<bool, _>("repo_ref_matches")),
        ("commit_oid", stored.get::<bool, _>("commit_oid_matches")),
        (
            "cause_event_id",
            stored.get::<bool, _>("cause_event_id_matches"),
        ),
        ("cause_depth", stored.get::<bool, _>("cause_depth_matches")),
        ("caused_by", stored.get::<bool, _>("caused_by_matches")),
        (
            "definition_snapshot",
            stored.get::<bool, _>("definition_snapshot_matches"),
        ),
        (
            "trigger_kind",
            stored.get::<bool, _>("trigger_kind_matches"),
        ),
        ("trust_tier", stored.get::<bool, _>("trust_tier_matches")),
        (
            "correlation_id",
            stored.get::<bool, _>("correlation_id_matches"),
        ),
        (
            "triggered_by",
            stored.get::<bool, _>("triggered_by_matches"),
        ),
    ] {
        if !matches {
            differing_fields.push(field);
        }
    }

    if differing_fields.is_empty() {
        Ok(false)
    } else {
        Err(CiRunStoreError::ReplayCollision { differing_fields })
    }
}

fn validate_initial_state(row: &CiRunInsert) -> Result<(), CiRunStoreError> {
    if row.state != "queued" {
        return Err(CiRunStoreError::InvalidInitialState);
    }
    u32::try_from(row.cause_depth)
        .map(|_| ())
        .map_err(|_| CiRunStoreError::InvalidCausalDepth)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_row() -> CiRunInsert {
        CiRunInsert {
            tenant_id: "acme".into(),
            region: "fr-par".into(),
            run_id: "11111111-1111-1111-1111-111111111111".into(),
            project_id: "22222222-2222-2222-2222-222222222222".into(),
            pipeline_id: "33333333-3333-3333-3333-333333333333".into(),
            wf_run_id: "44444444-4444-4444-4444-444444444444".into(),
            definition_snapshot: "blake3:abcd".into(),
            trigger_kind: "push".into(),
            trust_tier: "trusted".into(),
            state: "queued".into(),
            correlation_id: "corr-1".into(),
            cause_event_id: Some("ev-push-1".into()),
            cause_depth: 0,
            caused_by: None,
            repo_ref: Some("web".into()),
            commit_oid: Some("deadbeef".into()),
            triggered_by: None,
        }
    }

    /// The INSERT binds all 17 columns + is idempotent on the `(tenant_id, run_id)` PK (the DB-free
    /// shape assertions; the live round-trip + co-commit atomicity are the integration proofs).
    #[test]
    fn insert_query_is_idempotent_on_the_pk_and_binds_every_column() {
        assert!(
            INSERT_CI_RUN_QUERY.contains("ON CONFLICT (tenant_id, run_id) DO NOTHING"),
            "idempotent on the run-of-record PK"
        );
        // 17 bind placeholders ($1..$17) for the 17 writer-set columns.
        for n in 1..=17 {
            assert!(INSERT_CI_RUN_QUERY.contains(&format!("${n}")), "binds ${n}");
        }
        assert!(
            !INSERT_CI_RUN_QUERY.contains("$18"),
            "no over-bind past $17"
        );
        // The uuid columns are cast from text (the CT-004b proven posture).
        assert!(INSERT_CI_RUN_QUERY.contains("$3::uuid"));
        // The row is constructable with every NOT-NULL column set + state = queued (the reserve state).
        let r = sample_row();
        assert_eq!(r.state, "queued");
        assert_eq!(r.trigger_kind, "push");
        assert!(
            r.triggered_by.is_none(),
            "the proven shape leaves triggered_by NULL"
        );
    }

    #[test]
    fn replay_verification_is_a_region_bound_locking_statement() {
        assert!(VERIFY_CI_RUN_REPLAY_QUERY.contains("tenant_id = $1"));
        assert!(VERIFY_CI_RUN_REPLAY_QUERY.contains("region = $2"));
        assert!(VERIFY_CI_RUN_REPLAY_QUERY.contains("run_id = $3::uuid"));
        assert!(VERIFY_CI_RUN_REPLAY_QUERY.contains("FOR KEY SHARE"));
        for field in [
            "region",
            "project_id",
            "pipeline_id",
            "wf_run_id",
            "repo_ref",
            "commit_oid",
            "cause_event_id",
            "cause_depth",
            "caused_by",
            "definition_snapshot",
            "trigger_kind",
            "trust_tier",
            "correlation_id",
            "triggered_by",
        ] {
            assert!(
                VERIFY_CI_RUN_REPLAY_QUERY.contains(field),
                "selects {field}"
            );
        }
        for mutable in ["state", "cost_settled", "finished_at", "created_at"] {
            assert!(
                !VERIFY_CI_RUN_REPLAY_QUERY.contains(mutable),
                "excludes mutable {mutable}"
            );
        }
    }

    #[test]
    fn non_queued_insert_is_rejected_before_sql() {
        let mut r = sample_row();
        r.state = "running".into();
        assert_eq!(
            validate_initial_state(&r),
            Err(CiRunStoreError::InvalidInitialState)
        );
    }

    #[test]
    fn non_canonical_causal_depth_is_rejected_before_sql() {
        let mut r = sample_row();
        r.cause_depth = -1;
        assert_eq!(
            validate_initial_state(&r),
            Err(CiRunStoreError::InvalidCausalDepth)
        );

        r.cause_depth = i64::from(u32::MAX) + 1;
        assert_eq!(
            validate_initial_state(&r),
            Err(CiRunStoreError::InvalidCausalDepth)
        );
    }
}
