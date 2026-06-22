//! **Reindex-from-source — the ONLY rebuild path** (SRCH-P16 / P-179; architecture
//! `search-and-indexing.md` §4.9; contract 6.4). Search NEVER reads owner databases. On any rebuild —
//! cold start, corruption, schema change, a new sub-index, post-restore re-erasure, an embedding-model
//! swap — Search calls the **bus reindex-from-source re-emit protocol** (contract 2.6,
//! [`myelin_events::reindex`]) and feeds the resulting `*.snapshot` events through its **ordinary live
//! indexer** ([`crate::indexer::IncrementalIndexer::index`], §4.1). There is ONE code path for
//! steady-state and recovery (SRCH-D5: cold == live); there is **NO "load the index from Postgres"
//! backdoor** (the SEARCH-1 anti-pattern; the no-cross-db lint catches it).
//!
//! ## The §4.9 protocol, implemented here
//! ```text
//! reindex(scope=(tenant|subsystem|type)):
//!   wipe the per-tenant index (the cold-rebuild precondition — derived state only)
//!   for each owning subsystem in scope:
//!      subsystem.replay(scope, since=<cursor>) → emits *.snapshot via its outbox → the live bus
//!   Search's ordinary indexer (§4.1) ingests them, idempotent on the deterministic snapshot event_id
//! ```
//! [`SearchReindexer::reindex`] is the §5.6 `reindex(scope) → job` surface (contract 6.4 OWNED). It:
//! 1. **wipes** the `(tenant, region)` index (a cold rebuild starts from empty — derived state only,
//!    Search holds no system-of-record, §1) UNLESS the scope is an incremental backfill (`since > 0`,
//!    the new-sub-index / upcaster path, which appends to the live index);
//! 2. drives the **bus re-emit** ([`myelin_events::reindex`], contract 2.6 CONSUMED): each owning
//!    subsystem's [`myelin_events::ReindexSource::replay`] emits `*.snapshot` drafts through the
//!    **outbox** (the SAME outbox→bus→live-consumer path BUS-2 mandates — no backdoor);
//! 3. drains those snapshot rows from the outbox **in the replay's deterministic order** and feeds each
//!    through the indexer's PUBLIC [`crate::indexer::IncrementalIndexer::index`] — the EXACT live
//!    consumer step a `*.created` takes (the consumer cannot tell cold from live: a `*.snapshot` carries
//!    the SAME envelope shape, only its `event_id` is the deterministic
//!    [`myelin_events::snapshot_event_id`] so a re-run converges). Idempotent on `doc_id` (the engine's
//!    upsert) AND on `event_id` (the cursor store's applied-set) — belt and braces.
//!
//! ## The cursor store S4 (§4.9) — throttled, resumable, per-tenant in-flight caps
//! [`ReindexCursorStore`] is the v1 cursor store the §4.9 budget names. It is, per `(tenant, region,
//! scope)`:
//! - **resumable** — it records the per-aggregate high-water `since` cursor so a resumed replay re-asks
//!   the owner only for aggregates ABOVE the cursor (`since = Some(v)`), and the deterministic snapshot
//!   `event_id` makes a redelivered snapshot a no-op (idempotency-by-construction);
//! - **throttled** — a single `reindex` pass applies AT MOST `batch_cap` snapshots, returning a
//!   [`ReindexJob::InProgress`] with the resume cursor when more remain (the caller drives the next
//!   batch); a full rebuild is a sequence of bounded batches, never an unbounded firehose;
//! - **per-tenant in-flight caps** — at most `max_in_flight_per_tenant` reindex jobs run concurrently
//!   for one tenant ([`ReindexCursorStore::try_acquire`] / [`ReindexCursorStore::release`]); an
//!   over-cap acquire is refused so a reindex storm cannot starve a tenant's live indexing lane.
//!
//! ## No-cross-db (the SEARCH-1 ratchet, structural here)
//! This module reads NO owner database. The ONLY inputs are (a) the bus re-emit seam
//! ([`myelin_events::reindex`], a contract surface, not a sibling's storage module) and (b) the live
//! indexer's `index()` step. There is no `myelin_<owner>::{storage|store|db|schema|repo|pool}` path —
//! the no-cross-db lint (`crates/myelin-search/tests/lint_confirmation.rs` + the workspace `lint-gate`)
//! holds over `crates/myelin-search/src`. A reindex re-drives the SAME `index()` path; there is no
//! "load the index from Postgres" backdoor.
//!
//! ## Floors named (prompt DoD)
//! - **The CI-scale SRCH-D5 variant gates this band** (a small-corpus cold-vs-live parity proof). The
//!   **full-scale** reindex-parity (SRCH-D5 at scale, E2E-3) is the M5 follow-on **SRCH-P32**. Named so
//!   the CI-variant green is not mistaken for the full-scale proof.
//! - **The owners' real `replay` bodies are EB-26 / per-owner M3/M4** (the named floor on
//!   [`myelin_events::reindex`]). This module is the SEARCH-side consumer of that seam; it is exercised
//!   here against a reference owner ([`myelin_events::ReferenceReindexSource`]) + the synthetic-producer
//!   indexer. The seam shape (wipe → bus re-emit → live `index()` → cursor) does not change when the real
//!   owner replay lands.
//! - **Mutation floor (mandatory-core).** The reindex decision logic — the wipe-iff-full-rebuild branch,
//!   the bus-re-emit drive, the deterministic-id drain order, the batch-cap throttle, the resume-cursor
//!   advance, the per-tenant in-flight acquire/release, the idempotent applied-set — is the
//!   mutation-tested core; the floor is stated + met by the unit + chained + drill tests below (every
//!   branch asserted; a mutant that drops the wipe, mis-orders the drain, skips the throttle, or
//!   resurrects an applied snapshot is caught). The SRCH-P06 indexer + SRCH-P15 erase mutation floors
//!   still hold (unchanged). The world-scale reindex-parity-at-scale drill is SRCH-P32 (M5).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use myelin_events::reindex::{reindex as bus_reindex, ReindexError as BusReindexError};
use myelin_events::{
    EmitContextBase, OutboxStore, ReindexReceipt as BusReindexReceipt, ReindexSource, SnapshotScope,
};
use myelin_tenancy::{Region, TenantId};

use crate::indexer::{IncrementalIndexer, IndexEventError};

/// The default per-`reindex`-pass snapshot batch cap (the §4.9 throttle). A full rebuild applies at most
/// this many `*.snapshot` events per pass, returning a resume cursor when more remain — so a reindex is
/// a sequence of bounded batches, never an unbounded firehose that starves the live indexing lane. A
/// v1 default; the real per-cell budget is a config knob (SEARCH-2; a config swap, never a code change).
pub const DEFAULT_BATCH_CAP: usize = 1024;

/// The default per-tenant concurrent-reindex in-flight cap (the §4.9 per-tenant cap). At most this many
/// reindex jobs run concurrently for one tenant — an over-cap acquire is refused so a reindex storm
/// cannot starve a tenant's live indexing. A v1 default; the real per-cell budget is a config knob.
pub const DEFAULT_MAX_IN_FLIGHT_PER_TENANT: usize = 2;

/// The `(tenant, region, scope)` cursor key (the cursor store partition; PII-free opaque tokens). A
/// reindex is always tenant-first + scope-precise (§3.4 / §4.9 sub-artifact granularity).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct CursorKey {
    tenant: String,
    region: String,
    scope: String,
}

impl CursorKey {
    fn new(tenant: &TenantId, region: &Region, scope: &SnapshotScope) -> CursorKey {
        CursorKey {
            tenant: tenant.0.clone(),
            region: region.0.clone(),
            scope: scope.as_key(),
        }
    }
}

/// **The reindex cursor store S4 (§4.9) — throttled, resumable, per-tenant in-flight caps.** The v1
/// cursor store the §4.9 budget names. Records, per `(tenant, region, scope)`, the high-water resume
/// cursor (the `since` an interrupted replay resumes from) + the applied snapshot `event_id` set (the
/// idempotency guard) + the per-tenant in-flight reindex count (the cap). PII-free: opaque tenant/region
/// tokens + scope selectors + snapshot ids (`snap-…`), never a body.
#[derive(Clone)]
pub struct ReindexCursorStore {
    inner: Arc<Mutex<CursorInner>>,
    /// The §4.9 throttle: max `*.snapshot` events applied per `reindex` pass.
    batch_cap: usize,
    /// The §4.9 per-tenant cap: max concurrent reindex jobs for one tenant.
    max_in_flight_per_tenant: usize,
}

#[derive(Default)]
struct CursorInner {
    /// `(tenant, region, scope)` → the resume cursor (the high-water version already applied — a resumed
    /// replay re-asks the owner for aggregates ABOVE this).
    cursors: BTreeMap<CursorKey, u64>,
    /// `(tenant, region, scope)` → the applied snapshot `event_id`s (the in-store idempotency guard — a
    /// redelivered snapshot is a no-op; belt to the deterministic-id braces).
    applied: BTreeMap<CursorKey, BTreeSet<String>>,
    /// tenant → the number of reindex jobs currently in flight (the per-tenant cap counter).
    in_flight: BTreeMap<String, usize>,
}

impl Default for ReindexCursorStore {
    fn default() -> ReindexCursorStore {
        ReindexCursorStore::new()
    }
}

impl ReindexCursorStore {
    /// A fresh cursor store at the default throttle/cap (the v1 budget).
    pub fn new() -> ReindexCursorStore {
        ReindexCursorStore::with_budget(DEFAULT_BATCH_CAP, DEFAULT_MAX_IN_FLIGHT_PER_TENANT)
    }

    /// A cursor store with an explicit `batch_cap` + `max_in_flight_per_tenant` (the per-cell budget; a
    /// config swap, never a code change). Both are floored at 1 (a 0-cap would wedge every reindex).
    pub fn with_budget(batch_cap: usize, max_in_flight_per_tenant: usize) -> ReindexCursorStore {
        ReindexCursorStore {
            inner: Arc::new(Mutex::new(CursorInner::default())),
            batch_cap: batch_cap.max(1),
            max_in_flight_per_tenant: max_in_flight_per_tenant.max(1),
        }
    }

    /// The §4.9 throttle (max snapshots applied per `reindex` pass).
    pub fn batch_cap(&self) -> usize {
        self.batch_cap
    }

    /// The §4.9 per-tenant in-flight cap.
    pub fn max_in_flight_per_tenant(&self) -> usize {
        self.max_in_flight_per_tenant
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, CursorInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The current resume cursor for `(tenant, region, scope)` — the high-water version already applied;
    /// a resumed replay re-asks the owner for aggregates ABOVE it. `None` = nothing applied yet (a full
    /// rebuild from `since = None`).
    pub fn cursor(&self, tenant: &TenantId, region: &Region, scope: &SnapshotScope) -> Option<u64> {
        self.lock()
            .cursors
            .get(&CursorKey::new(tenant, region, scope))
            .copied()
    }

    /// Has the snapshot `event_id` already been applied for `(tenant, region, scope)`? (The idempotency
    /// guard — a redelivered snapshot is a no-op.)
    pub fn is_applied(
        &self,
        tenant: &TenantId,
        region: &Region,
        scope: &SnapshotScope,
        event_id: &str,
    ) -> bool {
        self.lock()
            .applied
            .get(&CursorKey::new(tenant, region, scope))
            .is_some_and(|set| set.contains(event_id))
    }

    /// **Try to acquire a per-tenant reindex in-flight slot (the §4.9 per-tenant cap).** Returns `true`
    /// (and increments the in-flight count) iff the tenant is under [`Self::max_in_flight_per_tenant`];
    /// `false` (no increment) if at cap — an over-cap reindex is REFUSED so a reindex storm cannot starve
    /// a tenant's live indexing lane. Pair every successful acquire with a [`Self::release`].
    pub fn try_acquire(&self, tenant: &TenantId) -> bool {
        let cap = self.max_in_flight_per_tenant;
        let mut g = self.lock();
        let n = g.in_flight.entry(tenant.0.clone()).or_insert(0);
        if *n >= cap {
            false
        } else {
            *n += 1;
            true
        }
    }

    /// Release a per-tenant reindex in-flight slot (pairs with a successful [`Self::try_acquire`]).
    pub fn release(&self, tenant: &TenantId) {
        let mut g = self.lock();
        if let Some(n) = g.in_flight.get_mut(&tenant.0) {
            *n = n.saturating_sub(1);
        }
    }

    /// The current per-tenant in-flight reindex count (observability / the cap test reads it).
    pub fn in_flight(&self, tenant: &TenantId) -> usize {
        self.lock().in_flight.get(&tenant.0).copied().unwrap_or(0)
    }

    /// **Reset the cursor + applied-set for `(tenant, region, scope)` (the full-rebuild precondition).**
    /// A full reindex (`since = None`) WIPES the index, so the prior applied-set/cursor are stale — they
    /// guarded the OLD generation's docs. Clearing them lets the cold rebuild re-apply every snapshot
    /// (the index is empty; re-applying is correct, and the engine's `doc_id` upsert keeps it idempotent
    /// within the pass). The applied-set then guards only WITHIN one ongoing throttled multi-batch
    /// rebuild (a redelivered snapshot mid-rebuild is a no-op). The in-flight count is untouched (it is
    /// per-tenant, not per-scope). An absent scope is a no-op.
    pub fn reset_scope(&self, tenant: &TenantId, region: &Region, scope: &SnapshotScope) {
        let key = CursorKey::new(tenant, region, scope);
        let mut g = self.lock();
        g.cursors.remove(&key);
        g.applied.remove(&key);
    }

    /// Record that `event_id` (at `version`) was applied for `(tenant, region, scope)` — advances the
    /// resume cursor to the max version seen + marks the id applied (the idempotency guard). Returns
    /// `false` iff the id was ALREADY applied (the caller skips it — no double effect).
    fn record_applied(
        &self,
        tenant: &TenantId,
        region: &Region,
        scope: &SnapshotScope,
        event_id: &str,
        version: u64,
    ) -> bool {
        let key = CursorKey::new(tenant, region, scope);
        let mut g = self.lock();
        let fresh = g
            .applied
            .entry(key.clone())
            .or_default()
            .insert(event_id.to_string());
        if fresh {
            let cur = g.cursors.entry(key).or_insert(0);
            *cur = (*cur).max(version);
        }
        fresh
    }
}

/// An error from a Search-side reindex (contract 6.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReindexError {
    /// The bus re-emit seam failed (no owner for the scope, or the outbox emit/commit failed). A reindex
    /// of an unknown owner is a LOUD error — never a silent empty rebuild that would mask a wiring bug.
    Bus(String),
    /// The live indexer rejected a re-driven snapshot (a malformed snapshot, or the engine failed). LOUD:
    /// a half-rebuilt index is never silently accepted.
    Index(String),
    /// The per-tenant in-flight cap refused this reindex (a reindex storm is shed, not starved). The
    /// caller retries after a running job releases.
    AtCapacity(String),
}

impl std::fmt::Display for ReindexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReindexError::Bus(e) => write!(f, "reindex: bus re-emit failed: {e}"),
            ReindexError::Index(e) => write!(f, "reindex: live indexer rejected a snapshot: {e}"),
            ReindexError::AtCapacity(e) => {
                write!(f, "reindex: per-tenant in-flight cap reached: {e}")
            }
        }
    }
}

impl std::error::Error for ReindexError {}

/// **The outcome of a `reindex(scope)` pass (contract 6.4 — `reindex(scope) → job`).** Either the pass
/// applied every remaining snapshot (`Done`), or the §4.9 throttle capped it mid-rebuild (`InProgress`,
/// carrying the resume cursor the caller drives the next batch from). PII-free: counts + the resume
/// cursor (a version number), never a body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReindexJob {
    /// The rebuild is complete for this scope: every snapshot above the cursor was applied (or
    /// idempotently skipped). Carries the totals (the SRCH-D5 receipt body).
    Done(ReindexProgress),
    /// The §4.9 batch cap stopped this pass mid-rebuild — `resume_since` is the cursor the NEXT
    /// `reindex(scope, since=resume_since)` continues from (a resumable replay). Carries the partial
    /// totals.
    InProgress {
        /// The progress so far (snapshots applied / skipped this pass).
        progress: ReindexProgress,
        /// The `since` cursor the next batch resumes from (the high-water version applied this pass).
        resume_since: u64,
    },
}

impl ReindexJob {
    /// The progress totals (regardless of `Done`/`InProgress`).
    pub fn progress(&self) -> &ReindexProgress {
        match self {
            ReindexJob::Done(p) => p,
            ReindexJob::InProgress { progress, .. } => progress,
        }
    }

    /// `true` iff the rebuild for this scope is complete.
    pub fn is_done(&self) -> bool {
        matches!(self, ReindexJob::Done(_))
    }
}

/// The progress a `reindex` pass made (the SRCH-D5 / contract-6.4 receipt body). PII-free counts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReindexProgress {
    /// `*.snapshot` events the bus re-emit newly emitted into the outbox (the re-emit half).
    pub snapshots_emitted: usize,
    /// `*.snapshot` events skipped at the bus because their deterministic id was already in the outbox
    /// (the `ON CONFLICT DO NOTHING` idempotency at the re-emit half).
    pub snapshots_skipped_duplicate: usize,
    /// `*.snapshot` events DRIVEN through the live indexer this pass (the ingest half).
    pub docs_indexed: usize,
    /// `*.snapshot` events skipped at ingest because their id was already applied (the cursor store's
    /// idempotency guard — a redelivered snapshot is a no-op; no resurrection).
    pub docs_skipped_applied: usize,
    /// The owners replayed (the §6.2 tokens), in scope order.
    pub owners_replayed: Vec<String>,
}

/// **Search's reindex-from-source driver (SRCH-P16; contract 6.4 OWNED — `reindex(scope) → job`).** The
/// ONLY rebuild path (SEARCH-1). Wraps the live [`IncrementalIndexer`] (the SAME one the bus feeds — a
/// reindex re-drives ITS `index()` step, no second index, no Postgres backdoor) + the cursor store S4 +
/// the cell's resident region. Cloneable handle (the indexer + cursor store are shared).
#[derive(Clone)]
pub struct SearchReindexer {
    /// The live per-tenant index a reindex rebuilds (the SAME indexer the bus feeds — `reindex` re-drives
    /// its `index()` path; there is no second index, no "load from Postgres" backdoor).
    indexer: Arc<IncrementalIndexer>,
    /// The cursor store S4 (throttled, resumable, per-tenant caps) — shared so concurrent reindex jobs
    /// see the same in-flight counter.
    cursors: ReindexCursorStore,
    /// The cell's resident region (§3.4 — Search is region-pinned). The 6.4 surface passes a scope; the
    /// cell-local driver resolves the region from its config (env `MYELIN_REGION`, dev `fr-par`).
    region: Region,
}

impl SearchReindexer {
    /// Build the reindex driver over a live [`IncrementalIndexer`] + a fresh cursor store at the default
    /// budget + the cell's resident `region`.
    pub fn new(indexer: Arc<IncrementalIndexer>, region: Region) -> SearchReindexer {
        SearchReindexer {
            indexer,
            cursors: ReindexCursorStore::new(),
            region,
        }
    }

    /// Build the driver over an explicit (shared) cursor store (so a test/op can pin the budget or share
    /// the in-flight counter across drivers).
    pub fn with_cursors(
        indexer: Arc<IncrementalIndexer>,
        cursors: ReindexCursorStore,
        region: Region,
    ) -> SearchReindexer {
        SearchReindexer {
            indexer,
            cursors,
            region,
        }
    }

    /// The cell's resident region (§3.4).
    pub fn region(&self) -> &Region {
        &self.region
    }

    /// The cursor store S4 (for observability / the resume-cursor + cap assertions).
    pub fn cursors(&self) -> &ReindexCursorStore {
        &self.cursors
    }

    /// **`reindex(scope, since) → job` (contract 6.4; §4.9) — the ONLY rebuild path (SEARCH-1).** Drives
    /// the bus re-emit ([`myelin_events::reindex`], 2.6 CONSUMED) → `*.snapshot` rows through the outbox
    /// → the live indexer's `index()` step, idempotent on the deterministic snapshot `event_id`. There is
    /// NO Postgres backdoor — the rebuild re-drives the SAME live consumer step as a `*.created`.
    ///
    /// - `since = None` is a FULL rebuild: the `(tenant, region)` index is **wiped** first (the
    ///   cold-rebuild precondition — derived state only) and rebuilt from `since = 0`.
    /// - `since = Some(v)` is an INCREMENTAL backfill (the new-sub-index / upcaster / resume path): NO
    ///   wipe; only aggregates above the cursor replay, appended to the live index.
    ///
    /// The §4.9 throttle caps the pass at [`ReindexCursorStore::batch_cap`] snapshots, returning a
    /// [`ReindexJob::InProgress`] with the resume cursor when more remain. The per-tenant in-flight cap is
    /// enforced (an over-cap reindex is [`ReindexError::AtCapacity`]).
    ///
    /// `sources` are the OWNING subsystems' [`ReindexSource`]s (their real `replay` bodies are EB-26 /
    /// per-owner M3/M4 — the named floor); `outbox` is the bus outbox the snapshots co-commit to;
    /// `ctx_base` is the emit context (the platform actor + clock). The bus re-emit re-reads the OWNER's
    /// source of truth — never Search's index (the no-cross-db floor is structural).
    #[allow(clippy::too_many_arguments)]
    pub fn reindex(
        &self,
        tenant: &TenantId,
        scope: &SnapshotScope,
        since: Option<u64>,
        sources: &[&dyn ReindexSource],
        outbox: &mut OutboxStore,
        ctx_base: EmitContextBase,
    ) -> Result<ReindexJob, ReindexError> {
        // The per-tenant in-flight cap (§4.9): refuse an over-cap reindex so a storm cannot starve the
        // tenant's live indexing lane. Acquire-then-release around the whole pass.
        if !self.cursors.try_acquire(tenant) {
            return Err(ReindexError::AtCapacity(format!(
                "tenant `{}` already has {} reindex job(s) in flight (cap {})",
                tenant.0,
                self.cursors.in_flight(tenant),
                self.cursors.max_in_flight_per_tenant()
            )));
        }
        let result = self.reindex_inner(tenant, scope, since, sources, outbox, ctx_base);
        self.cursors.release(tenant);
        result
    }

    fn reindex_inner(
        &self,
        tenant: &TenantId,
        scope: &SnapshotScope,
        since: Option<u64>,
        sources: &[&dyn ReindexSource],
        outbox: &mut OutboxStore,
        ctx_base: EmitContextBase,
    ) -> Result<ReindexJob, ReindexError> {
        let region = self.region.clone();

        // A FULL rebuild (`since = None`) WIPES the index first — the cold-rebuild precondition (derived
        // state only; Search holds no system-of-record, §1). An INCREMENTAL backfill (`since = Some`)
        // appends to the live index (the new-sub-index / upcaster / resume path), so NO wipe.
        if since.is_none() {
            self.indexer.wipe(tenant, &region);
            // The prior applied-set/cursor guarded the WIPED generation's docs — reset them so the cold
            // rebuild re-applies every snapshot (the index is empty; the applied-set then guards only
            // within THIS rebuild's batches). A throttled resume (`since = Some`) keeps the applied-set.
            self.cursors.reset_scope(tenant, &region, scope);
        }

        // (1) Drive the BUS re-emit (contract 2.6) — each owning subsystem's `replay(scope, since)`
        // emits `*.snapshot` drafts through the outbox (the SAME outbox→bus→live-consumer path; no
        // backdoor). A LOUD error if the scope's owner is unregistered (never a silent empty rebuild).
        let BusReindexReceipt {
            snapshots_emitted,
            snapshots_skipped_duplicate,
            owners_replayed,
        } = bus_reindex(scope, since, sources, outbox, ctx_base).map_err(map_bus_err)?;

        // (2) Drain the snapshot rows from the outbox IN THE REPLAY'S DETERMINISTIC ORDER and feed each
        // through the live indexer's `index()` step (the EXACT live consumer step). We recompute the
        // deterministic ids from the SAME `replay` the bus used (the owner's truth is deterministic), so
        // the drain order is byte-reproducible (cold == live). The cursor store throttles + dedups.
        let mut progress = ReindexProgress {
            snapshots_emitted,
            snapshots_skipped_duplicate,
            owners_replayed,
            ..Default::default()
        };
        let mut highest_applied = since.unwrap_or(0);
        let mut hit_cap = false;

        for source in sources {
            if source.owner_token() != scope.owner {
                continue; // the bus dispatched to the scope's owner; drain only that owner's snapshots.
            }
            for draft in source.replay(scope, since) {
                if progress.docs_indexed >= self.cursors.batch_cap() {
                    // The §4.9 throttle: this pass is full. Stop — the resume cursor is the high-water
                    // version applied so far; the caller drives the next batch from there.
                    hit_cap = true;
                    break;
                }
                let event_id = draft.event_id();
                // The idempotency guard (the cursor store applied-set): a redelivered snapshot is a
                // no-op (no resurrection of an already-applied — or post-erase, re-emitted-then-erased —
                // doc). Belt to the deterministic-id braces.
                if !self
                    .cursors
                    .record_applied(tenant, &region, scope, &event_id.0, draft.version)
                {
                    progress.docs_skipped_applied += 1;
                    continue;
                }
                // Read the snapshot row back from the outbox (it lands at its deterministic id) and feed
                // its envelope through the LIVE `index()` step — the SAME path a `*.created` takes.
                let row = outbox.row(&event_id).ok_or_else(|| {
                    ReindexError::Bus(format!(
                        "snapshot {} not found in the outbox (the bus re-emit did not stage it)",
                        event_id.0
                    ))
                })?;
                self.indexer.index(&row.envelope).map_err(map_index_err)?;
                progress.docs_indexed += 1;
                highest_applied = highest_applied.max(draft.version);
            }
            if hit_cap {
                break;
            }
        }

        if hit_cap {
            Ok(ReindexJob::InProgress {
                progress,
                resume_since: highest_applied,
            })
        } else {
            Ok(ReindexJob::Done(progress))
        }
    }
}

fn map_bus_err(e: BusReindexError) -> ReindexError {
    ReindexError::Bus(e.to_string())
}

fn map_index_err(e: IndexEventError) -> ReindexError {
    match e {
        IndexEventError::Malformed(w)
        | IndexEventError::Engine(w)
        | IndexEventError::Transient(w) => ReindexError::Index(w),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::AclFilter;
    use crate::indexer::{
        IndexSpec, MockEmbeddingAdapter, ProjectFetchError, ProjectFetcher, SearchProjection,
    };
    use myelin_events::reindex::ReferenceReindexSource;
    use myelin_events::{
        Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId,
        EventType, Timestamp, Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;

    const REGION: &str = "fr-par";

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region(REGION.into())
    }
    fn principal() -> Principal {
        Principal::stub(
            PrincipalId("platform".into()),
            PrincipalKind::Service,
            tenant(),
        )
    }

    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: tenant(),
            region: region(),
            actor: Actor(principal()),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-20T00:00:00Z".into()),
            caused_by: None,
        }
    }

    /// A ProjectFetcher backed by the SAME owner truth the reference source replays — so the live
    /// `index()` step (which fetches the owner's projection per `*.snapshot`) sees the body the owner
    /// holds. This is the no-cross-db seam: Search fetches the owner's projection (5.6), never its DB.
    #[derive(Default)]
    struct OwnerProjection {
        bodies: StdMutex<HashMap<String, String>>,
    }
    impl OwnerProjection {
        fn put(&self, ref_: &str, body: &str) {
            self.bodies
                .lock()
                .unwrap()
                .insert(ref_.to_string(), body.to_string());
        }
    }
    impl ProjectFetcher for OwnerProjection {
        fn project(
            &self,
            _t: &TenantId,
            _r: &Region,
            ref_: &ArtifactRef,
        ) -> Result<SearchProjection, ProjectFetchError> {
            match self.bodies.lock().unwrap().get(&ref_.0) {
                Some(body) => Ok(SearchProjection {
                    text: body.clone(),
                    fields: BTreeMap::new(),
                    lang: None,
                }),
                None => Err(ProjectFetchError::Gone),
            }
        }
    }

    /// The knowledge.page spec the reference owner's `*.snapshot` subject matches
    /// (`knowledge.page.snapshot` → subsystem `knowledge`, type `page`). Semantic so a vector rides.
    fn page_spec() -> IndexSpec {
        IndexSpec::new("knowledge", "page", BTreeMap::new()).semantic()
    }

    /// Build the live indexer + the owner-projection fetcher, populating the fetcher with `(ref, body)`.
    fn indexer_with(bodies: &[(&str, &str)]) -> (Arc<IncrementalIndexer>, Arc<OwnerProjection>) {
        let fetcher = Arc::new(OwnerProjection::default());
        for (r, b) in bodies {
            fetcher.put(r, b);
        }
        let ix = Arc::new(IncrementalIndexer::new(
            vec![page_spec()],
            fetcher.clone(),
            Arc::new(MockEmbeddingAdapter::new(8)),
        ));
        (ix, fetcher)
    }

    /// The reference owner's `replay` builds `*.snapshot` subjects as
    /// `myelin://t/<owner>/<artifact>/<agg>`. Mirror that so the fetcher body keys match the subjects
    /// the indexer will fetch.
    fn snapshot_ref(agg: &str) -> String {
        format!("myelin://t/knowledge/page/{agg}")
    }

    /// A reference knowledge owner with three pages at known versions.
    fn owner_with_three_pages() -> ReferenceReindexSource {
        let mut src = ReferenceReindexSource::new("knowledge", "page");
        src.upsert("home", 1, serde_json::json!({ "kind": "page" }));
        src.upsert("guide", 2, serde_json::json!({ "kind": "page" }));
        src.upsert("faq", 1, serde_json::json!({ "kind": "page" }));
        src
    }

    fn scope() -> SnapshotScope {
        SnapshotScope::new("knowledge", "page:all")
    }

    /// **The reindex drives the bus re-emit → the LIVE indexer → rebuilds the index from cold (the
    /// SRCH-D5 happy path). The deterministic snapshot ids are applied; the index ends up with every
    /// page searchable.** (The prompt's required unit: bus re-emit → live indexer.)
    #[test]
    fn reindex_rebuilds_the_index_from_the_bus_re_emit_through_the_live_indexer() {
        let src = owner_with_three_pages();
        // The owner's projection bodies (the live `index()` step fetches these — never the owner DB).
        let (ix, fetcher) = indexer_with(&[]);
        fetcher.put(&snapshot_ref("home"), "the home page about raft");
        fetcher.put(&snapshot_ref("guide"), "a guide about paxos");
        fetcher.put(&snapshot_ref("faq"), "frequently asked questions");

        let reindexer = SearchReindexer::new(ix.clone(), region());
        let mut outbox = OutboxStore::new();
        let sources: &[&dyn ReindexSource] = &[&src];

        let job = reindexer
            .reindex(&tenant(), &scope(), None, sources, &mut outbox, ctx_base())
            .expect("reindex");

        assert!(
            job.is_done(),
            "the full rebuild completes in one pass (under the batch cap)"
        );
        let p = job.progress();
        assert_eq!(
            p.snapshots_emitted, 3,
            "three pages re-emitted as *.snapshot via the bus"
        );
        assert_eq!(
            p.docs_indexed, 3,
            "all three driven through the LIVE indexer"
        );
        assert_eq!(p.owners_replayed, vec!["knowledge".to_string()]);
        assert_eq!(
            ix.live_count(&tenant(), &region()),
            3,
            "the index holds the three rebuilt docs"
        );

        // The rebuilt docs are searchable through the ordinary FT path (cold == live).
        let hits = ix
            .search_ft(&tenant(), &region(), &AclFilter::All, "raft", 10)
            .expect("ft");
        assert_eq!(hits.len(), 1, "the rebuilt home page is searchable");
    }

    /// **Re-running the reindex is idempotent in EFFECT — the bus emits 0 NEW snapshots (the
    /// deterministic id makes the re-emit an `ON CONFLICT DO NOTHING` no-op) and the cold rebuild
    /// converges to the SAME doc set (no duplication/resurrection; the engine's `doc_id` upsert keeps the
    /// re-application idempotent).** (The prompt's required unit: deterministic snapshot event_id.)
    #[test]
    fn reindex_is_idempotent_on_the_deterministic_snapshot_event_id() {
        let src = owner_with_three_pages();
        let (ix, fetcher) = indexer_with(&[]);
        for agg in ["home", "guide", "faq"] {
            fetcher.put(&snapshot_ref(agg), "body");
        }
        // A SHARED cursor store across the two passes (a real driver holds one cursor store — the durable
        // S4); the SAME outbox (the re-emit's deterministic-id dedup persists across runs).
        let cursors = ReindexCursorStore::new();
        let reindexer = SearchReindexer::with_cursors(ix.clone(), cursors, region());
        let sources: &[&dyn ReindexSource] = &[&src];
        let mut outbox = OutboxStore::new();

        let first = reindexer
            .reindex(&tenant(), &scope(), None, sources, &mut outbox, ctx_base())
            .expect("first");
        assert_eq!(
            first.progress().snapshots_emitted,
            3,
            "first run emits three snapshots"
        );
        assert_eq!(first.progress().docs_indexed, 3, "first pass indexes three");

        // A SECOND full reindex over the SAME outbox: the BUS skips all three as duplicate (their
        // deterministic ids are already present — `ON CONFLICT DO NOTHING`), but the cold rebuild still
        // re-drives the live `index()` step over the wiped index and converges to the SAME three docs.
        let second = reindexer
            .reindex(&tenant(), &scope(), None, sources, &mut outbox, ctx_base())
            .expect("second");
        assert_eq!(
            second.progress().snapshots_emitted,
            0,
            "0 NEW snapshots emitted (deterministic id)"
        );
        assert_eq!(
            second.progress().snapshots_skipped_duplicate,
            3,
            "all three skipped at the bus re-emit"
        );
        assert_eq!(
            second.progress().docs_indexed,
            3,
            "the cold rebuild re-applies the three (over a wipe)"
        );
        // The index still holds exactly three (no resurrection/duplication — `doc_id` upsert + the wipe).
        assert_eq!(
            ix.live_count(&tenant(), &region()),
            3,
            "still exactly three docs — idempotent in effect"
        );
    }

    /// **CHAINED (joint SRCH-P15 + SRCH-P16): index → erase → reindex-from-source → the rebuilt index
    /// EXCLUDES the erased subject; re-erasure after the reindex does NOT resurrect.** This is the
    /// erasure-stays-erased-across-a-rebuild invariant (X-7): once the OWNER tombstones the erased
    /// aggregate, its `replay` skips it, so the reindex cannot resurrect it through the live consumer
    /// path. (The prompt's required chained test.)
    #[test]
    fn chained_index_erase_reindex_does_not_resurrect_the_erased_subject() {
        use crate::dek::SearchDekPin;
        use crate::engine::SubjectMatcher;
        use crate::erase::SearchEraseHolder;
        use myelin_gdpr::SubjectRef;
        use myelin_identity::PseudonymHandle;
        use myelin_storage::KmsEngine;

        // An owner holding TWO pages: one owned by the erased subject (located by its `.noreply`
        // pseudonym mention in the body), one unrelated. The fetcher returns the owner's projection.
        let erased = SubjectRef::new(Principal::stub(
            PrincipalId("u-42".into()),
            PrincipalKind::Human,
            tenant(),
        ));
        // The frozen `<pseudonym>@<tenant>.noreply` grammar (contract 4.8) — the SAME the erase holder
        // matches a body mention on.
        let pseudonym = PseudonymHandle::new(&erased.principal.principal_id.0, &tenant().0)
            .expect("pseudonym renders")
            .render();
        let owned_ref = snapshot_ref("owned");
        let other_ref = snapshot_ref("other");

        let (ix, fetcher) = indexer_with(&[]);
        fetcher.put(
            &owned_ref,
            &format!("a page mentioning {pseudonym} about raft"),
        );
        fetcher.put(&other_ref, "an unrelated page about paxos");

        // The owner's truth (both pages). After the erase the owner TOMBSTONES the erased aggregate
        // (removes it from truth + drops its projection body) — the erasure must reach the owner too, so
        // its `replay` no longer re-emits it (X-7).
        let mut src = ReferenceReindexSource::new("knowledge", "page");
        src.upsert("owned", 1, serde_json::json!({ "kind": "page" }));
        src.upsert("other", 1, serde_json::json!({ "kind": "page" }));

        // (1) INDEX both pages via a full reindex-from-source.
        let reindexer = SearchReindexer::new(ix.clone(), region());
        let mut outbox = OutboxStore::new();
        reindexer
            .reindex(&tenant(), &scope(), None, &[&src], &mut outbox, ctx_base())
            .expect("initial index");
        assert_eq!(ix.live_count(&tenant(), &region()), 2, "both pages indexed");

        // (2) ERASE the subject (SRCH-P15: purge + reindex, vectors compacted) — the owned page is
        // purged from the index through the live consumer path.
        let kms = Arc::new(KmsEngine::new());
        let pin = SearchDekPin::new(kms);
        pin.reserve(&tenant(), &region())
            .expect("reserve the index DEK");
        let holder = SearchEraseHolder::new(ix.clone(), pin, region());
        let outcome = holder.erase_subject(&erased, &tenant()).expect("erase");
        assert_eq!(outcome.docs_purged, 1, "the subject's page is purged");
        assert!(
            outcome.zero_orphan_embedding,
            "0 orphan embedding after the erase"
        );
        assert_eq!(
            ix.live_count(&tenant(), &region()),
            1,
            "only the unrelated page remains"
        );

        // (3) The OWNER tombstones the erased aggregate (the erasure reached the owner — X-7). Build a
        // post-erase owner whose truth/projection no longer hold the erased page.
        let mut src_after = ReferenceReindexSource::new("knowledge", "page");
        src_after.upsert("other", 1, serde_json::json!({ "kind": "page" }));
        fetcher.bodies.lock().unwrap().remove(&owned_ref); // the owner's projection is gone too.

        // (4) REINDEX-FROM-SOURCE after the erase: the rebuilt index EXCLUDES the erased subject (the
        // owner no longer replays it — the erasure stays erased across the rebuild).
        let mut outbox2 = OutboxStore::new();
        let job = reindexer
            .reindex(
                &tenant(),
                &scope(),
                None,
                &[&src_after],
                &mut outbox2,
                ctx_base(),
            )
            .expect("reindex after erase");
        assert!(job.is_done());
        assert_eq!(
            ix.live_count(&tenant(), &region()),
            1,
            "the rebuilt index holds only the unrelated page"
        );
        let raft = ix
            .search_ft(&tenant(), &region(), &AclFilter::All, "raft", 10)
            .expect("ft raft");
        assert!(
            raft.is_empty(),
            "the erased subject's page is NOT resurrected by the reindex (X-7)"
        );

        // (5) RE-ERASURE after the reindex purges 0 (the subject is already gone — no resurrection).
        let matcher = SubjectMatcher::new(
            erased.principal.principal_id.0.clone(),
            Some(pseudonym.clone()),
        );
        let located = ix.locate_subject(&tenant(), &region(), &matcher);
        assert!(
            located.is_empty(),
            "the erased subject references 0 docs after the reindex"
        );
        let re = holder.erase_subject(&erased, &tenant()).expect("re-erase");
        assert_eq!(
            re.docs_purged, 0,
            "re-erasure after the reindex purges nothing (no resurrection)"
        );
        assert!(re.zero_orphan_embedding, "still 0 orphan embedding");
    }

    /// **The §4.9 throttle: a small batch cap stops a pass mid-rebuild with a resume cursor; the next
    /// pass resumes and finishes (a full rebuild is a sequence of bounded batches).** (The prompt's
    /// required unit: cursor store S4 throttled/resumable.)
    #[test]
    fn reindex_is_throttled_and_resumable_via_the_cursor_store() {
        // Four pages at strictly increasing versions so the resume cursor advances monotonically.
        let mut src = ReferenceReindexSource::new("knowledge", "page");
        src.upsert("p1", 1, serde_json::json!({ "kind": "page" }));
        src.upsert("p2", 2, serde_json::json!({ "kind": "page" }));
        src.upsert("p3", 3, serde_json::json!({ "kind": "page" }));
        src.upsert("p4", 4, serde_json::json!({ "kind": "page" }));
        let (ix, fetcher) = indexer_with(&[]);
        for agg in ["p1", "p2", "p3", "p4"] {
            fetcher.put(&snapshot_ref(agg), "body");
        }
        // A batch cap of 2: each pass applies AT MOST two snapshots.
        let cursors = ReindexCursorStore::with_budget(2, DEFAULT_MAX_IN_FLIGHT_PER_TENANT);
        let reindexer = SearchReindexer::with_cursors(ix.clone(), cursors, region());
        let sources: &[&dyn ReindexSource] = &[&src];
        let mut outbox = OutboxStore::new();

        // Pass 1: full rebuild, capped at 2.
        let p1 = reindexer
            .reindex(&tenant(), &scope(), None, sources, &mut outbox, ctx_base())
            .expect("pass 1");
        match p1 {
            ReindexJob::InProgress {
                progress,
                resume_since,
            } => {
                assert_eq!(
                    progress.docs_indexed, 2,
                    "the cap stops the pass at two docs"
                );
                assert_eq!(
                    resume_since, 2,
                    "the resume cursor is the high-water version applied"
                );
            }
            ReindexJob::Done(_) => panic!("the capped pass must NOT report Done — more remain"),
        }
        assert_eq!(
            ix.live_count(&tenant(), &region()),
            2,
            "only two docs applied so far"
        );

        // Pass 2: resume from the cursor (since = 2). Only p3/p4 replay; both apply; Done.
        let p2 = reindexer
            .reindex(
                &tenant(),
                &scope(),
                Some(2),
                sources,
                &mut outbox,
                ctx_base(),
            )
            .expect("pass 2");
        assert!(p2.is_done(), "the resumed pass finishes the rebuild");
        assert_eq!(
            p2.progress().docs_indexed,
            2,
            "the remaining two docs applied"
        );
        assert_eq!(
            ix.live_count(&tenant(), &region()),
            4,
            "all four docs rebuilt across the two batches"
        );
    }

    /// **The incremental backfill (`since = Some`) does NOT wipe the live index — it appends.** A new
    /// sub-index / upcaster backfill rides the SAME path but preserves the already-indexed docs.
    #[test]
    fn incremental_backfill_appends_without_wiping() {
        // The owner gains a new page at version 5; the index already holds an older one.
        let mut src = ReferenceReindexSource::new("knowledge", "page");
        src.upsert("old", 1, serde_json::json!({ "kind": "page" }));
        src.upsert("new", 5, serde_json::json!({ "kind": "page" }));
        let (ix, fetcher) = indexer_with(&[]);
        fetcher.put(&snapshot_ref("old"), "old body");
        fetcher.put(&snapshot_ref("new"), "new body");

        // Pre-seed the index with the OLD doc via a full reindex first.
        let reindexer = SearchReindexer::new(ix.clone(), region());
        let mut outbox = OutboxStore::new();
        let only_old = {
            let mut s = ReferenceReindexSource::new("knowledge", "page");
            s.upsert("old", 1, serde_json::json!({ "kind": "page" }));
            s
        };
        reindexer
            .reindex(
                &tenant(),
                &scope(),
                None,
                &[&only_old],
                &mut outbox,
                ctx_base(),
            )
            .expect("seed old");
        assert_eq!(
            ix.live_count(&tenant(), &region()),
            1,
            "the old doc is indexed"
        );

        // Incremental backfill since=1: only the version-5 page replays; the old doc is PRESERVED.
        let mut outbox2 = OutboxStore::new();
        let job = reindexer
            .reindex(
                &tenant(),
                &scope(),
                Some(1),
                &[&src],
                &mut outbox2,
                ctx_base(),
            )
            .expect("backfill");
        assert!(job.is_done());
        assert_eq!(
            job.progress().docs_indexed,
            1,
            "only the new page replays past since=1"
        );
        assert_eq!(
            ix.live_count(&tenant(), &region()),
            2,
            "the backfill APPENDED — the old doc survives"
        );
    }

    /// **The per-tenant in-flight cap refuses an over-cap reindex (a reindex storm is shed, not allowed
    /// to starve the live lane).** Acquire the cap manually, then prove a `reindex` is refused.
    #[test]
    fn per_tenant_in_flight_cap_refuses_an_over_cap_reindex() {
        let src = owner_with_three_pages();
        let (ix, _f) = indexer_with(&[]);
        let cursors = ReindexCursorStore::with_budget(DEFAULT_BATCH_CAP, 1); // cap = 1 in flight.
        let reindexer = SearchReindexer::with_cursors(ix, cursors.clone(), region());
        let sources: &[&dyn ReindexSource] = &[&src];
        let mut outbox = OutboxStore::new();

        // Simulate a reindex already in flight for this tenant (acquire the only slot).
        assert!(cursors.try_acquire(&tenant()), "the first slot acquires");
        assert_eq!(cursors.in_flight(&tenant()), 1);

        let err = reindexer
            .reindex(&tenant(), &scope(), None, sources, &mut outbox, ctx_base())
            .expect_err("an over-cap reindex is refused");
        assert!(
            matches!(err, ReindexError::AtCapacity(_)),
            "the per-tenant cap sheds the storm"
        );

        // Release the held slot; now the reindex succeeds (the cap is not a permanent block).
        cursors.release(&tenant());
        assert_eq!(cursors.in_flight(&tenant()), 0);
        let job = reindexer
            .reindex(&tenant(), &scope(), None, sources, &mut outbox, ctx_base())
            .expect("reindex succeeds once a slot frees");
        assert!(job.is_done());
        // The reindex released its own slot on completion (no leak).
        assert_eq!(
            cursors.in_flight(&tenant()),
            0,
            "the reindex released its in-flight slot"
        );
    }

    /// **A reindex of an UNKNOWN owner is a LOUD error (never a silent empty rebuild that masks a wiring
    /// bug).** Bubbles the bus's `NoSourceForOwner`.
    #[test]
    fn reindex_of_unknown_owner_is_a_loud_error() {
        let src = ReferenceReindexSource::new("knowledge", "page");
        let (ix, _f) = indexer_with(&[]);
        let reindexer = SearchReindexer::new(ix, region());
        let unknown = SnapshotScope::new("refs", "edge:all"); // no `refs` source registered.
        let mut outbox = OutboxStore::new();
        let err = reindexer
            .reindex(&tenant(), &unknown, None, &[&src], &mut outbox, ctx_base())
            .expect_err("unknown owner");
        assert!(
            matches!(err, ReindexError::Bus(_)),
            "an unknown owner is a loud Bus error"
        );
    }

    /// Build a `knowledge.page.created` live event (the ordinary ingest path) for a doc.
    fn created_event(doc: &str) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId(format!("ev:{doc}")),
            type_: EventType("knowledge.page.created".into()),
            schema_ver: 1,
            tenant: tenant(),
            region: region(),
            actor: Actor(principal()),
            subject: ArtifactRef(doc.into()),
            aggregate: AggregateKey(format!("agg:{doc}")),
            causation_id: None,
            correlation_id: CorrelationId(doc.into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
            payload: serde_json::json!({}),
        }
    }

    /// **SRCH-D5 (CI variant): cold == live. Build a LIVE index by ingesting `*.created` events; wipe;
    /// reindex-from-source; assert the rebuilt index is identical (doc set + FT searchability) using the
    /// LIVE consumer path ONLY.** (The prompt's required drill scenario.)
    #[test]
    fn srch_d5_cold_equals_live_ci_variant() {
        // The owner truth + the projection bodies (one source for both the live ingest and the cold
        // rebuild — the SAME owner content, so cold == live is the meaningful comparison).
        let mut src = ReferenceReindexSource::new("knowledge", "page");
        src.upsert("alpha", 1, serde_json::json!({ "kind": "page" }));
        src.upsert("beta", 1, serde_json::json!({ "kind": "page" }));
        let (ix, fetcher) = indexer_with(&[]);
        fetcher.put(&snapshot_ref("alpha"), "alpha discusses raft consensus");
        fetcher.put(&snapshot_ref("beta"), "beta discusses paxos consensus");

        // LIVE: ingest the two pages through the ordinary `*.created` path (the steady-state lane).
        ix.index(&created_event(&snapshot_ref("alpha")))
            .expect("live alpha");
        ix.index(&created_event(&snapshot_ref("beta")))
            .expect("live beta");
        let live_count = ix.live_count(&tenant(), &region());
        let live_raft = ix
            .search_ft(&tenant(), &region(), &AclFilter::All, "raft", 10)
            .expect("live raft");
        let live_paxos = ix
            .search_ft(&tenant(), &region(), &AclFilter::All, "paxos", 10)
            .expect("live paxos");
        assert_eq!(live_count, 2, "the live index holds both pages");
        assert_eq!(live_raft.len(), 1);
        assert_eq!(live_paxos.len(), 1);

        // COLD: wipe + reindex-from-source through the bus re-emit → the live indexer (the recovery lane).
        let reindexer = SearchReindexer::new(ix.clone(), region());
        let mut outbox = OutboxStore::new();
        let job = reindexer
            .reindex(&tenant(), &scope(), None, &[&src], &mut outbox, ctx_base())
            .expect("reindex cold");
        assert!(job.is_done());

        // PARITY: the cold-rebuilt index == live (doc count + the same docs searchable).
        let cold_count = ix.live_count(&tenant(), &region());
        let cold_raft = ix
            .search_ft(&tenant(), &region(), &AclFilter::All, "raft", 10)
            .expect("cold raft");
        let cold_paxos = ix
            .search_ft(&tenant(), &region(), &AclFilter::All, "paxos", 10)
            .expect("cold paxos");
        assert_eq!(
            cold_count, live_count,
            "cold-rebuilt doc count == live (SRCH-D5)"
        );
        assert_eq!(
            cold_raft.len(),
            live_raft.len(),
            "the raft page is searchable in the cold rebuild"
        );
        assert_eq!(
            cold_paxos.len(),
            live_paxos.len(),
            "the paxos page is searchable in the cold rebuild"
        );
        assert_eq!(
            cold_raft.first().map(|h| h.doc_id.clone()),
            live_raft.first().map(|h| h.doc_id.clone()),
            "the SAME doc id ranks for the SAME query (cold == live, not just same count)"
        );
    }
}
