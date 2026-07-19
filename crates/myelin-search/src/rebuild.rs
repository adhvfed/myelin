//! **The tenant+region index rebuild coordinator — the legacy→canonical identity migration.**
//!
//! Git blob projection identities moved from raw slash-delimited ids to canonical percent-encoded
//! [`ArtifactRef`](myelin_tenancy::ArtifactRef)s (see [`crate::canonical`]). Because the index is
//! keyed by the id STRING, that cutover does not rewrite anything: every document, vector and
//! metadata record written under a legacy id simply becomes unreachable by the new writer and
//! survives untouched. The consequences are all silent:
//!
//! - a live blob gets a SECOND document (legacy + canonical) and answers queries twice;
//! - a blob deleted after the cutover loses only its canonical document — the legacy twin keeps
//!   serving the deleted content;
//! - a blob whose subject is `restrict`ed keeps answering from the legacy id, which is precisely the
//!   content the restriction exists to suppress;
//! - a legacy VECTOR keeps answering semantic queries even where no document remains.
//!
//! There is no in-place fix: the legacy ids cannot be mapped back to `(repo, ref, path)` without
//! guessing (that ambiguity is why the identity changed). The only sound repair is to rebuild the
//! `(tenant, region)` index from owner truth.
//!
//! ## Why a coordinator, and not just a call to [`crate::reindex`]
//!
//! [`SearchReindexer::reindex`] is the right ENGINE and the wrong CONTROL: it wipes per call (so
//! replaying four owner corpora in sequence wipes three of them away), it holds its cursors in
//! process (so a crash resumes from a high-water mark describing a generation that no longer
//! exists), it has no notion of a fenced read, and it can be started twice for the same tenant. A
//! production migration needs the phases ordered, durable, and exclusive.
//!
//! ## The phase order, and why each edge is where it is
//!
//! ```text
//! Claimed  → the durable job row exists and this process holds an exclusive lease
//! Fenced   → the broker high-water mark is recorded AND reads are fail-empty
//! Wiped    → the index is destroyed exactly once
//! CursorsReset → every scope's applied-event/reindex cursor is cleared
//! Replayed → every registered owner corpus has been replayed from owner truth
//! CaughtUp → live events up to the recorded high-water mark have been applied
//! Verified → count/hash parity, vector parity, zero legacy ids, zero lag all green
//! Complete → reads reopen
//! ```
//!
//! - **Fence before wipe.** Wiping an index that is still serving reads turns a migration into an
//!   outage that looks like data loss: queries return partial results with no signal that anything
//!   is wrong. Fail-empty is the honest answer — "this tenant's index is rebuilding", not "there are
//!   no results".
//! - **Record the high-water mark before the wipe, not after.** The mark is what bounds catch-up. If
//!   it were taken after the replay, events that arrived DURING the replay would sit above it and
//!   never be applied — a permanent silent hole. Taken at fence time it is a ceiling the replay is
//!   guaranteed to sit below.
//! - **Reset cursors after the wipe.** The cursors describe the wiped generation. Resetting them
//!   first would leave a crash window in which a resumed replay trusts cursors for documents that
//!   still exist; resetting after means the cursors are only ever cleared once the docs they
//!   described are provably gone.
//! - **Verify before reopening.** A rebuild that half-succeeded must not be indistinguishable from
//!   one that succeeded. Reads stay fail-empty until every check passes.
//!
//! ## Crash convergence
//!
//! Every phase transition is journaled AFTER its action, and every action is idempotent. So a crash
//! between an action and its journal entry re-runs the action on restart (safe), and a crash after
//! the journal entry resumes at the next phase (correct). The phase gate is what makes re-running
//! safe: the wipe only executes while the journaled phase is below `Wiped`, so a crash during replay
//! cannot re-wipe the work already replayed.
//!
//! The lease carries a **fence epoch** bumped on every claim. A holder whose lease expired and was
//! stolen cannot journal a phase transition: its compare-and-set carries a stale epoch and is
//! refused. This is what stops a paused-then-resumed process from stomping on the rebuild that
//! replaced it.
//!
//! ## Tenant and region scoping
//!
//! Every operation is keyed on `(tenant, region)`. A rebuild for one tenant cannot claim, advance,
//! verify or wipe another's — and cannot pass its own verification on the strength of another's
//! index, because the inventory it verifies is read from the same `(tenant, region)` partition it
//! wiped. A region is likewise part of the key, not a filter applied afterwards.
//!
//! ## Disclosure posture
//!
//! Nothing in this module renders a tenant id, a region, a document id, a path, or source text into
//! an error, a `Display`, or a `Debug`. A legacy blob id embeds the raw repository name and file
//! path, and the restricted documents this migration exists to remove are exactly the ones whose
//! identity must not leak into an operator's log. Errors name the PHASE and the FAILING CHECK;
//! reports carry COUNTS and a digest. [`RebuildError`] and [`RebuildReport`] have hand-written
//! `Debug` impls so that a `{:?}` in a log line cannot become a disclosure.

use std::collections::BTreeSet;
use std::sync::Arc;

use myelin_events::{EmitContextBase, OutboxStore, ReindexSource, SnapshotScope};
use myelin_tenancy::{Region, TenantId};

use crate::canonical::is_legacy_blob_id;
use crate::reindex::{ReindexError, SearchReindexer};

// ───────────────────────────── the durable schema (contract 1.5, forward-only) ────────────────────

/// **The forward-only migration creating the rebuild job + lease table.**
///
/// One row per `(tenant, region)` — the job and its lease live together deliberately, so that
/// claiming the lease and reading the phase are ONE atomic row read. Splitting them into two tables
/// would admit a window where a process holds the lease but observes a phase written by its
/// predecessor.
///
/// Additive and forward-only: a `CREATE TABLE IF NOT EXISTS`, never a `DROP` or a destructive
/// `ALTER` (the migration runner rejects those at boot, §9.1).
pub const SEARCH_REBUILD_JOB_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS search_rebuild_job (
    tenant             TEXT   NOT NULL,
    region             TEXT   NOT NULL,
    phase              TEXT   NOT NULL,
    fence_epoch        BIGINT NOT NULL,
    high_water_mark    BIGINT,
    high_water_seqs    TEXT,
    pre_wipe_docs      BIGINT,
    owners_replayed    TEXT   NOT NULL DEFAULT '',
    lease_holder       TEXT,
    lease_expires_at   BIGINT NOT NULL DEFAULT 0,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant, region)
);";

/// **The forward-only migration indexing unfinished rebuilds.**
///
/// The read path asks "is this tenant rebuilding?" on every query, and an operator asks "what is
/// still in flight?" across the cell. Both are partial scans without this index. Additive: a
/// `CREATE INDEX IF NOT EXISTS`.
pub const SEARCH_REBUILD_ACTIVE_INDEX_MIGRATION: &str = "\
CREATE INDEX IF NOT EXISTS search_rebuild_job_active
    ON search_rebuild_job (tenant, region)
    WHERE phase <> 'complete';";

// ───────────────────────────── phases ─────────────────────────────────────────────────────────────

/// **A rebuild phase.** Ordered: [`RebuildPhase::rank`] gives the total order the phase gate
/// compares on, so "have we already wiped?" is `phase >= Wiped` rather than a set membership test
/// that a new phase could silently fall out of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RebuildPhase {
    /// The durable job row exists and a holder owns the lease. Nothing destructive has happened.
    Claimed,
    /// The broker/live-intake high-water mark is recorded and reads are fail-empty.
    Fenced,
    /// The `(tenant, region)` index has been destroyed. Exactly once per rebuild.
    Wiped,
    /// Every scope's durable applied-event/reindex cursor has been cleared.
    CursorsReset,
    /// Every registered owner corpus has been replayed from owner truth.
    Replayed,
    /// Live events up to the recorded high-water mark have been applied.
    CaughtUp,
    /// Parity, zero-legacy and zero-lag checks all passed.
    Verified,
    /// Reads have reopened. Terminal.
    Complete,
}

impl RebuildPhase {
    /// The phase's position in the total order (the phase gate's comparison key).
    pub fn rank(self) -> u8 {
        match self {
            RebuildPhase::Claimed => 0,
            RebuildPhase::Fenced => 1,
            RebuildPhase::Wiped => 2,
            RebuildPhase::CursorsReset => 3,
            RebuildPhase::Replayed => 4,
            RebuildPhase::CaughtUp => 5,
            RebuildPhase::Verified => 6,
            RebuildPhase::Complete => 7,
        }
    }

    /// The stable wire token (the `phase` column value). A fixed vocabulary — never free text.
    pub fn token(self) -> &'static str {
        match self {
            RebuildPhase::Claimed => "claimed",
            RebuildPhase::Fenced => "fenced",
            RebuildPhase::Wiped => "wiped",
            RebuildPhase::CursorsReset => "cursors_reset",
            RebuildPhase::Replayed => "replayed",
            RebuildPhase::CaughtUp => "caught_up",
            RebuildPhase::Verified => "verified",
            RebuildPhase::Complete => "complete",
        }
    }

    /// Parse a wire token back to a phase. `None` for an unknown token — a journal row carrying an
    /// unrecognised phase is a LOUD failure, never coerced to `Claimed` (which would re-wipe a
    /// finished index).
    pub fn from_token(token: &str) -> Option<RebuildPhase> {
        [
            RebuildPhase::Claimed,
            RebuildPhase::Fenced,
            RebuildPhase::Wiped,
            RebuildPhase::CursorsReset,
            RebuildPhase::Replayed,
            RebuildPhase::CaughtUp,
            RebuildPhase::Verified,
            RebuildPhase::Complete,
        ]
        .into_iter()
        .find(|p| p.token() == token)
    }

    /// Whether reads must fail-empty in this phase. Everything before [`RebuildPhase::Complete`]
    /// fences reads: an index mid-rebuild is not a smaller index, it is an INCOMPLETE one, and
    /// serving from it silently returns wrong answers.
    pub fn fences_reads(self) -> bool {
        self != RebuildPhase::Complete
    }
}

// ───────────────────────────── the durable record ─────────────────────────────────────────────────

/// The `(tenant, region)` rebuild key. Every coordinator operation is scoped by one.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RebuildKey {
    /// The tenant whose index is being rebuilt.
    pub tenant: TenantId,
    /// The region the index is resident in (§3.4 — Search is region-pinned).
    pub region: Region,
}

impl RebuildKey {
    /// Build a key for `(tenant, region)`.
    pub fn new(tenant: &TenantId, region: &Region) -> RebuildKey {
        RebuildKey {
            tenant: tenant.clone(),
            region: region.clone(),
        }
    }
}

/// A `RebuildKey` names a tenant and a region — both customer-identifying. `Debug` renders neither.
impl std::fmt::Debug for RebuildKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RebuildKey(<redacted tenant/region>)")
    }
}

/// **The durable rebuild record** — one row of `search_rebuild_job`.
#[derive(Clone, PartialEq, Eq)]
pub struct RebuildRecord {
    /// The furthest phase durably reached.
    pub phase: RebuildPhase,
    /// Bumped on every claim. A holder journaling with a stale epoch has lost its lease and is
    /// refused — this is the fence that stops a resumed-after-pause process from stomping on the
    /// rebuild that replaced it.
    pub fence_epoch: u64,
    /// The broker/live-intake high-water mark recorded at fence time, as a COUNT. Reporting only —
    /// it is deliberately NOT what bounds catch-up. See [`Self::high_water_at`].
    pub high_water_mark: Option<u64>,
    /// **The catch-up ceiling: the per-aggregate `seq` watermark captured at fence time.**
    ///
    /// This bound went through two wrong shapes before this one, and both failure modes are worth
    /// keeping written down because they are the shapes a reviewer will reach for again.
    ///
    /// It was first POSITIONAL — take the first `high_water_mark` rows. That is correct only if the
    /// committed stream is a stable commit-ordered prefix, and the durable outbox does not promise
    /// that: `committed_live_rows` orders by `(aggregate, seq)`, aggregate-LEXICOGRAPHIC, while the
    /// count is over the whole live set. A positional take selected the N lexicographically-smallest
    /// -aggregate rows — an arbitrary mix — silently dropping every pre-fence event on a
    /// high-sorting aggregate while live intake was fenced.
    ///
    /// It was then a `recorded_at` TIMESTAMP. Also wrong, for two independent reasons. The field is
    /// an unvalidated `String` compared lexicographically, so mixed precision breaks ordering
    /// outright (`"…T10:00:00Z" > "…T10:00:00.500Z"` is `true`, skipping an event that is
    /// chronologically earlier), as do offsets and any non-RFC-3339 producer. And `recorded_at` is
    /// stamped from the PRODUCER's clock when its transaction opens, not assigned by the store at
    /// commit — so a fast producer clock puts a genuinely pre-fence event above the boundary, where
    /// catch-up skips it and redelivery never comes because it was already dedup-marked.
    ///
    /// `seq` has neither problem: it is assigned by the STORE inside the commit transaction
    /// (`COALESCE(MAX(seq) + 1, 0)` per aggregate), it is an integer, and it is monotone per
    /// aggregate — the true per-aggregate commit sequence. The watermark is the highest `seq` each
    /// aggregate had reached at fence time; catch-up applies a row iff its aggregate is in the map
    /// AND its `seq` is at or below that aggregate's mark.
    ///
    /// An aggregate ABSENT from the map had no committed rows at fence time, so every row it has now
    /// is post-fence. Absence must therefore mean "skip everything", which is why this is read
    /// through an explicit `Option` and never `unwrap_or(0)` — `seq` is 0-BASED, so a default of 0
    /// would admit each such aggregate's first event.
    pub high_water_seqs: std::collections::BTreeMap<String, u64>,
    /// **How many documents the index held at fence time, BEFORE the wipe.**
    ///
    /// The anchor that stops verification passing over a destroyed corpus. Every other leg compares
    /// the rebuilt index against what the replay produced, so when owner truth comes back EMPTY —
    /// an unregistered source, an owner whose truth was never loaded, a mis-scoped selector — the
    /// comparison is `0 == 0` and every leg passes over an index that has just been wiped clean.
    /// Reads then reopen on nothing.
    ///
    /// This is the one fact that cannot be derived from the rebuild's own output, so it has to be
    /// captured before the destruction: an index that HELD documents must not verify as rebuilt
    /// while holding none.
    pub pre_wipe_docs: Option<u64>,
    /// The owner tokens whose corpora have been replayed (so a resumed replay skips finished
    /// owners rather than redoing them).
    pub owners_replayed: BTreeSet<String>,
    /// The current lease holder's opaque id, if any.
    pub lease_holder: Option<String>,
    /// The tick the lease expires at. A holder past this has no lease.
    pub lease_expires_at: u64,
}

/// The record carries a holder id and owner tokens; `Debug` renders the phase and counts only.
impl std::fmt::Debug for RebuildRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RebuildRecord")
            .field("phase", &self.phase)
            .field("fence_epoch", &self.fence_epoch)
            .field("high_water_mark", &self.high_water_mark)
            .field("owners_replayed", &self.owners_replayed.len())
            .field("leased", &self.lease_holder.is_some())
            .finish()
    }
}

impl RebuildRecord {
    /// Whether `holder` owns a lease that has not expired at `now`.
    pub fn lease_held_by(&self, holder: &str, now: u64) -> bool {
        self.lease_holder.as_deref() == Some(holder) && now < self.lease_expires_at
    }

    /// Whether ANY unexpired lease is outstanding at `now`.
    pub fn leased(&self, now: u64) -> bool {
        self.lease_holder.is_some() && now < self.lease_expires_at
    }
}

// ───────────────────────────── the journal seam ───────────────────────────────────────────────────

/// **The durable rebuild journal.**
///
/// A sync trait, matching [`myelin_events::DurableOutboxBacking`]: the indexer and the coordinator
/// are synchronous, and the durable implementation bridges to async sqlx internally rather than
/// colouring this whole path.
///
/// [`Self::compare_and_store`] is the load-bearing method — it MUST be atomic against concurrent
/// callers (a conditional `UPDATE ... WHERE fence_epoch = $expected`). Everything the coordinator
/// guarantees about exclusivity rests on that atomicity; an implementation that read-then-wrote
/// would let two holders interleave a wipe and a replay.
pub trait RebuildJournal: Send + Sync {
    /// Read the record for `key`, or `None` if no rebuild has ever been started for it.
    fn load(&self, key: &RebuildKey) -> Result<Option<RebuildRecord>, RebuildError>;

    /// **Atomically** store `next` for `key` iff the currently-stored fence epoch equals
    /// `expected_epoch` (`None` = expect no row at all — the initial claim).
    ///
    /// Returns `Ok(true)` on success and `Ok(false)` if the precondition failed (someone else holds
    /// the lease). `Err` is reserved for the store being unreachable — which must NEVER be reported
    /// as `Ok(false)`, because a caller reads `false` as "another holder is doing this correctly"
    /// and would stand down rather than fail loudly.
    fn compare_and_store(
        &self,
        key: &RebuildKey,
        expected_epoch: Option<u64>,
        next: &RebuildRecord,
    ) -> Result<bool, RebuildError>;
}

/// An in-process [`RebuildJournal`] for tests and drills.
///
/// Gated behind `test-support` so the `no-in-memory-durable-store` scanner strips it from the
/// production graph — the durable journal is the production default, and this double must never be
/// reachable from a production dependency edge.
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Default)]
pub struct MemoryRebuildJournal {
    rows: Arc<std::sync::Mutex<std::collections::BTreeMap<RebuildKey, RebuildRecord>>>,
}

#[cfg(any(test, feature = "test-support"))]
impl MemoryRebuildJournal {
    /// A fresh empty journal.
    pub fn new() -> MemoryRebuildJournal {
        MemoryRebuildJournal::default()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl RebuildJournal for MemoryRebuildJournal {
    fn load(&self, key: &RebuildKey) -> Result<Option<RebuildRecord>, RebuildError> {
        Ok(self
            .rows
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .cloned())
    }

    fn compare_and_store(
        &self,
        key: &RebuildKey,
        expected_epoch: Option<u64>,
        next: &RebuildRecord,
    ) -> Result<bool, RebuildError> {
        let mut g = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        let current = g.get(key).map(|r| r.fence_epoch);
        if current != expected_epoch {
            return Ok(false);
        }
        g.insert(key.clone(), next.clone());
        Ok(true)
    }
}

// ───────────────────────────── errors ─────────────────────────────────────────────────────────────

/// Why a rebuild operation failed.
///
/// Every variant names the PHASE or the CHECK, never the tenant, the region, a document id, a path,
/// or any source text. `Debug` is hand-written to the same standard as `Display` so that a `{:?}` in
/// a log line cannot become a disclosure the `{}` form was careful to avoid.
#[derive(Clone, PartialEq, Eq)]
pub enum RebuildError {
    /// The durable journal is unreachable or returned an unusable row. Carries the store's own
    /// message, which is infrastructural (connection/serialization), never tenant data.
    Journal(String),
    /// Another holder owns the lease, or this holder's lease expired and was stolen. The rebuild is
    /// not this process's to run.
    LeaseLost,
    /// No rebuild job exists for this key — an operation was attempted without a claim.
    NoJob,
    /// Catch-up was reached without a recorded high-water instant. LOUD: there is no safe default
    /// bound — applying the unbounded tail never converges, applying nothing is a silent hole.
    MissingHighWaterMark,
    /// The journal row carries a phase token this build does not recognise (a forward-rolled schema
    /// read by an older binary). LOUD: coercing it would risk re-wiping a finished index.
    UnknownPhase,
    /// A phase was attempted out of order.
    PhaseOutOfOrder {
        /// The phase the caller tried to run.
        attempted: RebuildPhase,
        /// The phase durably reached.
        durable: RebuildPhase,
    },
    /// The underlying replay/reindex failed.
    Replay(ReindexError),
    /// The index engine failed during a rebuild step.
    ///
    /// Carries a fixed CATEGORY token, deliberately not the underlying message: the indexer's own
    /// errors interpolate the offending artifact ref (`"event subject `{ref}` is not a myelin://
    /// artifact ref"`), and a blob ref embeds the repository name and file path. Propagating that
    /// string would turn every malformed event during a rebuild into a disclosure. The detail stays
    /// available where it belongs — the indexer's own logging — and does not ride this error out to
    /// an operator-facing rebuild report.
    Engine(&'static str),
    /// Verification did not pass, so reads stay fenced.
    VerificationFailed(VerifyFailure),
}

impl std::fmt::Display for RebuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RebuildError::Journal(e) => write!(f, "rebuild: durable journal unavailable: {e}"),
            RebuildError::LeaseLost => write!(
                f,
                "rebuild: the exclusive lease is held elsewhere (or expired and was stolen)"
            ),
            RebuildError::NoJob => write!(f, "rebuild: no rebuild job is claimed for this scope"),
            RebuildError::MissingHighWaterMark => write!(
                f,
                "rebuild: catch-up reached without a recorded high-water instant — refusing to \
                 guess a bound"
            ),
            RebuildError::UnknownPhase => write!(
                f,
                "rebuild: the journal row carries an unrecognised phase — refusing to guess"
            ),
            RebuildError::PhaseOutOfOrder { attempted, durable } => write!(
                f,
                "rebuild: phase `{}` attempted while `{}` is the durable phase",
                attempted.token(),
                durable.token()
            ),
            // Scrubbed to a category — the inner message carries the offending artifact ref.
            RebuildError::Replay(e) => {
                write!(f, "rebuild: {}", replay_failure_category(e))
            }
            RebuildError::Engine(e) => write!(f, "rebuild: index engine failed: {e}"),
            RebuildError::VerificationFailed(v) => {
                write!(f, "rebuild: verification failed: {v}")
            }
        }
    }
}

/// Renders exactly what `Display` does — see the type docs. A derived `Debug` would print the inner
/// strings of variants without their framing, which is the same information, but the point of the
/// hand-written impl is that adding a tenant-carrying variant later cannot silently start leaking
/// through `{:?}` while the author is only reviewing `{}`.
impl std::fmt::Debug for RebuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RebuildError({self})")
    }
}

impl std::error::Error for RebuildError {}

impl From<ReindexError> for RebuildError {
    fn from(e: ReindexError) -> RebuildError {
        RebuildError::Replay(e)
    }
}

/// The per-aggregate `seq` watermark of a committed-row snapshot: the highest `seq` each aggregate
/// has reached. This is the fence-time ceiling catch-up runs to — see
/// [`RebuildRecord::high_water_seqs`] for why it is a store-assigned sequence rather than a row
/// position or a producer timestamp.
pub fn high_water_seqs(
    committed: &[myelin_events::OutboxRow],
) -> std::collections::BTreeMap<String, u64> {
    let mut out: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
    for row in committed {
        let mark = out.entry(row.aggregate.0.clone()).or_insert(row.seq);
        *mark = (*mark).max(row.seq);
    }
    out
}

/// Reduce a replay error to a fixed category token.
///
/// The catch-up leg already scrubs indexer errors; the replay leg reached the SAME messages by a
/// different route — `ReindexError::Index` is built by flattening `IndexEventError`'s payload, which
/// interpolates the offending artifact ref, and `RebuildError::Replay` rendered it verbatim. Same
/// leak, other door.
fn replay_failure_category(e: &ReindexError) -> &'static str {
    match e {
        ReindexError::Bus(_) => "replay: the bus re-emit failed",
        ReindexError::Index(_) => "replay: the live indexer rejected a snapshot",
        ReindexError::AtCapacity(_) => "replay: the per-tenant in-flight cap refused this pass",
    }
}

/// Reduce an indexer error to a fixed category token.
///
/// The indexer's messages interpolate the offending artifact ref, and a Git blob ref embeds the
/// repository name and file path — so the message itself is a disclosure. An operator still needs to
/// know WHICH KIND of failure stopped the rebuild (a poison event is a different response from a
/// flaky owner fetch), and the category carries that without the payload.
fn catch_up_failure_category(e: &crate::indexer::IndexEventError) -> &'static str {
    match e {
        crate::indexer::IndexEventError::Malformed(_) => {
            "catch-up hit a malformed event (non-retryable poison)"
        }
        crate::indexer::IndexEventError::Engine(_) => "catch-up hit an index engine failure",
        crate::indexer::IndexEventError::Transient(_) => {
            "catch-up hit a transient owner-projection failure"
        }
    }
}

/// Which verification check failed, and by how much.
///
/// Counts only. Naming the offending documents would defeat the migration's whole purpose on the
/// restricted-content case: the ids that fail the legacy check are exactly the ones whose repo name
/// and path must not reach a log.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum VerifyFailure {
    /// The rebuilt document count disagrees with what the replay indexed.
    DocCountMismatch {
        /// Documents the replay indexed.
        expected: usize,
        /// Documents present in the index.
        found: usize,
    },
    /// The rebuilt document SET disagrees with what the replay indexed (same count, different
    /// members — the failure a count-only check would pass).
    DocDigestMismatch,
    /// The right NUMBER of embeddings exists, but on the wrong subjects (the failure a count-only
    /// vector check passes).
    VectorDigestMismatch,
    /// A document carries a live embedding it should not, or is missing one it should have.
    VectorParityMismatch {
        /// Documents expected to carry a live vector.
        expected: usize,
        /// Documents carrying one.
        found: usize,
    },
    /// Identities under the retired legacy grammar survived the rebuild.
    LegacyIdentitiesSurvived {
        /// How many, across documents, vectors and metadata records.
        count: usize,
    },
    /// The index held documents before the wipe and holds none after the rebuild — or the rebuild's
    /// expectation is empty, which would make every parity leg a vacuous `0 == 0`.
    CorpusDestroyed {
        /// Documents the index held at fence time.
        before: u64,
        /// Documents it holds now.
        after: usize,
        /// Documents the rebuild expected to produce.
        expected: usize,
    },
    /// The replay and/or catch-up reported driving documents, but the index is empty — the
    /// signature of a wipe that landed after the replay.
    EmptyAfterNonEmptyReplay {
        /// Documents the replay reported indexing.
        replayed: usize,
        /// Documents catch-up reported applying.
        caught_up: usize,
    },
    /// The indexer still has unprojected events in flight.
    NonZeroLag {
        /// The outstanding count.
        lag: u64,
    },
}

impl std::fmt::Display for VerifyFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerifyFailure::DocCountMismatch { expected, found } => write!(
                f,
                "document count parity: replay indexed {expected}, index holds {found}"
            ),
            VerifyFailure::DocDigestMismatch => write!(
                f,
                "document set parity: the index holds the right NUMBER of documents but not the \
                 right ones"
            ),
            VerifyFailure::VectorDigestMismatch => write!(
                f,
                "vector set parity: the index holds the right NUMBER of embeddings but not on the \
                 right documents"
            ),
            VerifyFailure::VectorParityMismatch { expected, found } => write!(
                f,
                "vector parity: {expected} documents should carry a live embedding, {found} do"
            ),
            VerifyFailure::LegacyIdentitiesSurvived { count } => write!(
                f,
                "{count} identities under the retired legacy grammar survived the rebuild"
            ),
            VerifyFailure::CorpusDestroyed {
                before,
                after,
                expected,
            } => write!(
                f,
                "the index held {before} document(s) before the wipe and holds {after} now, with an \
                 expectation of {expected} — owner truth came back empty, so the rebuild destroyed \
                 the corpus instead of rebuilding it"
            ),
            VerifyFailure::EmptyAfterNonEmptyReplay {
                replayed,
                caught_up,
            } => write!(
                f,
                "the index is EMPTY after a replay that reported {replayed} document(s) and a \
                 catch-up that reported {caught_up} — the rebuild's work was destroyed after it ran"
            ),
            VerifyFailure::NonZeroLag { lag } => {
                write!(f, "index lag is {lag}, expected 0 before reopening reads")
            }
        }
    }
}

impl std::fmt::Debug for VerifyFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "VerifyFailure({self})")
    }
}

// ───────────────────────────── the read gate ──────────────────────────────────────────────────────

/// What a read of this tenant's index is permitted to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadMode {
    /// Serve normally.
    Open,
    /// **Fail EMPTY, explicitly.** The index is mid-rebuild: incomplete, not small. A caller must
    /// surface this as "rebuilding" and must not present the empty result as an answer — the whole
    /// point of the distinction is that "no results" and "cannot answer yet" are different claims.
    FailEmptyRebuilding,
}

impl ReadMode {
    /// Whether a query may be served.
    pub fn serves(self) -> bool {
        self == ReadMode::Open
    }
}

/// **The read gate the query path consults per `(tenant, region)`.**
///
/// Cheap and shared: a clone reads the same journal. The gate is deliberately fail-CLOSED — if the
/// journal cannot be reached, reads are fenced rather than served, because the alternative is
/// serving a half-rebuilt index during exactly the incident that made the journal unreachable.
#[derive(Clone)]
pub struct RebuildReadGate {
    journal: Arc<dyn RebuildJournal>,
}

impl RebuildReadGate {
    /// Build a gate over the durable journal.
    pub fn new(journal: Arc<dyn RebuildJournal>) -> RebuildReadGate {
        RebuildReadGate { journal }
    }

    /// The read mode for `(tenant, region)`.
    ///
    /// `Open` iff there is no rebuild record, or the record is `Complete`. Fail-closed on a journal
    /// error and on an unrecognised phase token.
    pub fn read_mode(&self, tenant: &TenantId, region: &Region) -> ReadMode {
        let key = RebuildKey::new(tenant, region);
        match self.journal.load(&key) {
            Ok(None) => ReadMode::Open,
            Ok(Some(record)) if !record.phase.fences_reads() => ReadMode::Open,
            // A rebuild in any pre-Complete phase, OR a journal we cannot read: fence.
            Ok(Some(_)) | Err(_) => ReadMode::FailEmptyRebuilding,
        }
    }

    /// **Whether ordinary live intake may apply an event to `(tenant, region)`.**
    ///
    /// False while a rebuild is in flight. This is not an optimisation: the coordinator owns the
    /// index across the wipe→replay→catch-up window, and a concurrent live apply would either be
    /// erased by the wipe (a lost update) or land on top of a partially-replayed corpus and then be
    /// counted by the parity check as a document the replay did not index. Events keep accumulating
    /// in the outbox meanwhile — nothing is dropped; catch-up applies everything up to the recorded
    /// high-water mark and ordinary intake resumes above it once reads reopen.
    pub fn admits_intake(&self, tenant: &TenantId, region: &Region) -> bool {
        self.read_mode(tenant, region).serves()
    }
}

// ───────────────────────────── the report ─────────────────────────────────────────────────────────

/// A digest over a set of document ids — the "hash parity" leg of verification.
///
/// FNV-1a over the sorted, length-delimited id set. Length delimiting matters: without it the sets
/// `{"ab","c"}` and `{"a","bc"}` digest identically, and those are exactly the near-miss shapes a
/// delimiter-encoding migration produces. This is a corruption check, not a security primitive; it
/// is one-way in practice, so a digest may be logged where an id may not.
pub fn doc_set_digest<'a, I: IntoIterator<Item = &'a str>>(ids: I) -> u64 {
    let mut sorted: Vec<&str> = ids.into_iter().collect();
    sorted.sort_unstable();
    sorted.dedup();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for id in sorted {
        for byte in (id.len() as u64).to_le_bytes().iter().chain(id.as_bytes()) {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

/// The receipt of a completed rebuild. Counts and a digest — never an id, a path, or a tenant.
#[derive(Clone, PartialEq, Eq)]
pub struct RebuildReport {
    /// The high-water mark catch-up ran to.
    pub high_water_mark: u64,
    /// Owner corpora replayed.
    pub owners_replayed: usize,
    /// Documents the replay indexed.
    pub docs_replayed: usize,
    /// Live events applied during catch-up.
    pub docs_caught_up: usize,
    /// Documents in the rebuilt index.
    pub docs_indexed: usize,
    /// Live vectors in the rebuilt index.
    pub vectors_indexed: usize,
    /// The rebuilt document-set digest.
    pub digest: u64,
    /// Legacy identities surviving. Zero on a green rebuild.
    pub legacy_identities: usize,
}

impl std::fmt::Debug for RebuildReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RebuildReport")
            .field("high_water_mark", &self.high_water_mark)
            .field("owners_replayed", &self.owners_replayed)
            .field("docs_replayed", &self.docs_replayed)
            .field("docs_caught_up", &self.docs_caught_up)
            .field("docs_indexed", &self.docs_indexed)
            .field("vectors_indexed", &self.vectors_indexed)
            .field("legacy_identities", &self.legacy_identities)
            .finish()
    }
}

// ───────────────────────────── the coordinator ────────────────────────────────────────────────────

/// The default lease duration, in ticks. A holder must renew within this or lose the rebuild.
pub const DEFAULT_LEASE_TICKS: u64 = 300;

/// **The rebuild coordinator.** Holds the durable journal and the reindex driver; every method is
/// scoped to one `(tenant, region)` and gated on the phase + the lease.
#[derive(Clone)]
pub struct RebuildCoordinator {
    journal: Arc<dyn RebuildJournal>,
    reindexer: SearchReindexer,
    lease_ticks: u64,
}

impl RebuildCoordinator {
    /// Build a coordinator over the durable journal and the live reindex driver.
    pub fn new(journal: Arc<dyn RebuildJournal>, reindexer: SearchReindexer) -> RebuildCoordinator {
        RebuildCoordinator {
            journal,
            reindexer,
            lease_ticks: DEFAULT_LEASE_TICKS,
        }
    }

    /// Set the lease duration in ticks (a per-cell budget; a config swap, never a code change).
    pub fn with_lease_ticks(mut self, ticks: u64) -> RebuildCoordinator {
        self.lease_ticks = ticks.max(1);
        self
    }

    /// A read gate sharing this coordinator's journal.
    pub fn read_gate(&self) -> RebuildReadGate {
        RebuildReadGate::new(Arc::clone(&self.journal))
    }

    /// The durable record for `key`, if any.
    pub fn record(&self, key: &RebuildKey) -> Result<Option<RebuildRecord>, RebuildError> {
        self.journal.load(key)
    }

    /// **Claim (or renew) the exclusive rebuild lease for `key`.**
    ///
    /// Succeeds when there is no record, when this holder already owns the lease (a renewal), or
    /// when the incumbent's lease has EXPIRED at `now` (a takeover after a crashed holder). Fails
    /// with [`RebuildError::LeaseLost`] when another holder's lease is live — including for a
    /// different tenant or region, which are simply different keys and therefore never contend.
    ///
    /// A successful claim bumps the fence epoch, so any transition the previous holder attempts
    /// afterwards is refused.
    ///
    /// A claim on a `Complete` record starts a FRESH rebuild from [`RebuildPhase::Claimed`] — that
    /// is what makes the migration re-runnable.
    pub fn claim(&self, key: &RebuildKey, holder: &str, now: u64) -> Result<u64, RebuildError> {
        let current = self.journal.load(key)?;
        let (expected_epoch, next_epoch, phase, hwm, hwm_seqs, pre_wipe, owners) = match &current {
            None => (
                None,
                1,
                RebuildPhase::Claimed,
                None,
                std::collections::BTreeMap::new(),
                None,
                BTreeSet::new(),
            ),
            Some(rec) => {
                let mine = rec.lease_held_by(holder, now);
                if rec.leased(now) && !mine {
                    return Err(RebuildError::LeaseLost);
                }
                if rec.phase == RebuildPhase::Complete {
                    // A finished rebuild: a new claim starts a fresh one.
                    (
                        Some(rec.fence_epoch),
                        rec.fence_epoch + 1,
                        RebuildPhase::Claimed,
                        None,
                        std::collections::BTreeMap::new(),
                        None,
                        BTreeSet::new(),
                    )
                } else {
                    (
                        Some(rec.fence_epoch),
                        rec.fence_epoch + 1,
                        rec.phase,
                        rec.high_water_mark,
                        // The recorded ceiling SURVIVES a takeover. Re-taking it on resume would
                        // move the boundary upward after the wipe — reintroducing the very hole the
                        // fence-time capture exists to close.
                        rec.high_water_seqs.clone(),
                        rec.pre_wipe_docs,
                        rec.owners_replayed.clone(),
                    )
                }
            }
        };
        let next = RebuildRecord {
            phase,
            fence_epoch: next_epoch,
            high_water_mark: hwm,
            high_water_seqs: hwm_seqs,
            pre_wipe_docs: pre_wipe,
            owners_replayed: owners,
            lease_holder: Some(holder.to_string()),
            lease_expires_at: now.saturating_add(self.lease_ticks),
        };
        if self.journal.compare_and_store(key, expected_epoch, &next)? {
            Ok(next_epoch)
        } else {
            // Someone else moved the epoch between our load and our store.
            Err(RebuildError::LeaseLost)
        }
    }

    /// Load the record and confirm `holder` still owns the lease at `now`.
    fn checked(
        &self,
        key: &RebuildKey,
        holder: &str,
        now: u64,
    ) -> Result<RebuildRecord, RebuildError> {
        let rec = self.journal.load(key)?.ok_or(RebuildError::NoJob)?;
        if !rec.lease_held_by(holder, now) {
            return Err(RebuildError::LeaseLost);
        }
        Ok(rec)
    }

    /// **Re-assert the lease through the JOURNAL immediately before a destructive action.**
    ///
    /// [`Self::checked`] only READS the record, so on its own it leaves the classic
    /// fence-token-at-the-wrong-boundary hole: a holder can pass the read, stall (a GC pause, a disk
    /// stall), lose its lease to a takeover that then wipes and replays for an hour, wake up, and
    /// execute its own destructive action — destroying the replacement's work. The `advance` CAS
    /// afterwards would return `LeaseLost`, but only after the damage.
    ///
    /// This performs a compare-and-set FIRST, so a displaced holder is refused BEFORE it acts. The
    /// write is a no-op in content (the same phase, a renewed lease); its whole purpose is that it
    /// goes through the epoch predicate, which a displaced holder cannot satisfy.
    ///
    /// **Residual, named:** this narrows the window, it does not close it. The index wipe is an
    /// in-process operation and the journal is an external store, so there remains a window between
    /// the CAS returning and the wipe executing. Closing it properly requires the index write path
    /// itself to carry the fence epoch (an index generation stamped on each write, rejected if
    /// stale) — a larger change than this migration, and recorded as a follow-on rather than
    /// papered over.
    fn reassert_lease(
        &self,
        key: &RebuildKey,
        rec: &RebuildRecord,
        holder: &str,
        now: u64,
    ) -> Result<(), RebuildError> {
        self.advance(key, rec, holder, now, |_| {})
    }

    /// Journal `next` under the holder's current fence epoch, renewing the lease.
    fn advance(
        &self,
        key: &RebuildKey,
        current: &RebuildRecord,
        holder: &str,
        now: u64,
        mutate: impl FnOnce(&mut RebuildRecord),
    ) -> Result<(), RebuildError> {
        let mut next = current.clone();
        mutate(&mut next);
        next.lease_holder = Some(holder.to_string());
        next.lease_expires_at = now.saturating_add(self.lease_ticks);
        // The epoch is NOT bumped by a phase advance — only a claim bumps it. So the compare is
        // against the epoch this holder claimed under: if a rival claimed in the meantime the epoch
        // moved and this store is refused.
        if self
            .journal
            .compare_and_store(key, Some(current.fence_epoch), &next)?
        {
            Ok(())
        } else {
            Err(RebuildError::LeaseLost)
        }
    }

    /// **Abandon a rebuild, lifting the fence.**
    ///
    /// The remedy an operator needs when a rebuild is blocking something that must proceed — chiefly
    /// a tenant decommission, which `SearchEraseHolder::erase_tenant` refuses mid-rebuild because
    /// the in-flight replay would re-index the corpus after the shred.
    ///
    /// This does NOT repair the index: it marks the job `Complete`, so reads reopen over whatever
    /// the abandoned rebuild left behind — which after a wipe is a partial corpus. That is only
    /// acceptable when the index is about to be destroyed anyway, or when the operator intends to
    /// immediately re-run the rebuild. It is deliberately not called "cancel": nothing is rolled
    /// back.
    pub fn abandon(&self, key: &RebuildKey, holder: &str, now: u64) -> Result<(), RebuildError> {
        let rec = self.checked(key, holder, now)?;
        self.advance(key, &rec, holder, now, |next| {
            next.phase = RebuildPhase::Complete;
        })
    }

    /// **Phase 1 — fence: record the high-water mark and close reads.**
    ///
    /// `high_water_mark` is the broker/live-intake position at this instant (the committed outbox
    /// depth). Recording it BEFORE the wipe is what makes catch-up sound: every event the replay
    /// could race with is at or below this mark, so applying up to it closes the window with no
    /// hole above and no unbounded tail.
    ///
    /// Idempotent: re-running while already fenced re-affirms the ALREADY-RECORDED mark rather than
    /// taking a new one. Taking a fresh mark on a retry would move the ceiling upward after the
    /// wipe, which is precisely the hole the ordering exists to prevent.
    pub fn fence(
        &self,
        key: &RebuildKey,
        holder: &str,
        now: u64,
        committed: &[myelin_events::OutboxRow],
    ) -> Result<u64, RebuildError> {
        let rec = self.checked(key, holder, now)?;
        if rec.phase >= RebuildPhase::Fenced {
            // Already fenced: re-affirm the RECORDED mark. Re-deriving it from the current stream
            // would move the ceiling upward after the wipe — reintroducing the hole the fence-time
            // capture exists to close.
            return Ok(rec.high_water_mark.unwrap_or(0));
        }
        let seqs = high_water_seqs(committed);
        let count = committed.len() as u64;
        // Capture the corpus size BEFORE the wipe destroys it — see `pre_wipe_docs`.
        let pre_wipe = self
            .reindexer
            .indexer()
            .inventory(&key.tenant, &key.region)
            .map_err(|_| RebuildError::Engine("index inventory read failed"))?
            .doc_ids
            .len() as u64;
        self.advance(key, &rec, holder, now, |next| {
            next.phase = RebuildPhase::Fenced;
            next.high_water_mark = Some(count);
            next.high_water_seqs = seqs;
            next.pre_wipe_docs = Some(pre_wipe);
        })?;
        Ok(count)
    }

    /// **Phase 2 — wipe the `(tenant, region)` index, exactly once.**
    ///
    /// Gated on the phase: the wipe runs only while the durable phase is below
    /// [`RebuildPhase::Wiped`]. That gate is what makes a crash during a LATER phase safe to resume
    /// — a restart re-enters at the journaled phase and does not destroy the replayed corpus.
    ///
    /// Refuses to run before the fence: wiping an index that is still serving reads converts a
    /// migration into a silent wrong-answer window.
    pub fn wipe(&self, key: &RebuildKey, holder: &str, now: u64) -> Result<bool, RebuildError> {
        let rec = self.checked(key, holder, now)?;
        if rec.phase >= RebuildPhase::Wiped {
            return Ok(false); // already wiped — not again.
        }
        if rec.phase < RebuildPhase::Fenced {
            return Err(RebuildError::PhaseOutOfOrder {
                attempted: RebuildPhase::Wiped,
                durable: rec.phase,
            });
        }
        // Re-assert the lease through the journal BEFORE destroying anything — a displaced holder
        // must be refused before the wipe, not after it.
        self.reassert_lease(key, &rec, holder, now)?;
        self.reindexer.indexer().wipe(&key.tenant, &key.region);
        self.advance(key, &rec, holder, now, |next| {
            next.phase = RebuildPhase::Wiped;
        })?;
        Ok(true)
    }

    /// **Phase 3 — reset every durable applied-event / reindex cursor for the partition.**
    ///
    /// After the wipe, every cursor describes documents that no longer exist. Clearing them all at
    /// once (rather than per-scope as each owner replays) means no crash window leaves a
    /// not-yet-replayed corpus with a cursor pointing into the wiped generation — which would resume
    /// above its own documents and skip the corpus silently.
    pub fn reset_cursors(
        &self,
        key: &RebuildKey,
        holder: &str,
        now: u64,
    ) -> Result<usize, RebuildError> {
        let rec = self.checked(key, holder, now)?;
        if rec.phase >= RebuildPhase::CursorsReset {
            return Ok(0);
        }
        if rec.phase < RebuildPhase::Wiped {
            return Err(RebuildError::PhaseOutOfOrder {
                attempted: RebuildPhase::CursorsReset,
                durable: rec.phase,
            });
        }
        self.reassert_lease(key, &rec, holder, now)?;
        let reset = self
            .reindexer
            .cursors()
            .reset_all_scopes(&key.tenant, &key.region);
        self.advance(key, &rec, holder, now, |next| {
            next.phase = RebuildPhase::CursorsReset;
        })?;
        Ok(reset)
    }

    /// **Phase 4 — replay EVERY registered owner corpus, without wiping between scopes.**
    ///
    /// `scopes` must cover every registered owner corpus, not only Git blobs: the wipe destroyed the
    /// whole `(tenant, region)` index, so issues, knowledge and chat documents are gone too and a
    /// Git-only replay would silently ship a rebuild that lost three corpora.
    ///
    /// Each scope goes through [`SearchReindexer::replay_scope_without_wipe`]. Scopes already marked
    /// replayed in the journal are skipped, so a resumed rebuild continues rather than restarting —
    /// and because each scope is journaled as it completes, a crash mid-replay loses at most the
    /// scope in flight (which is idempotent to redo).
    #[allow(clippy::too_many_arguments)]
    pub fn replay_all(
        &self,
        key: &RebuildKey,
        holder: &str,
        now: u64,
        scopes: &[SnapshotScope],
        sources: &[&dyn ReindexSource],
        outbox: &mut OutboxStore,
        ctx_base: EmitContextBase,
    ) -> Result<ReplayOutcome, RebuildError> {
        let mut rec = self.checked(key, holder, now)?;
        // Past `Replayed` the corpus is frozen: catch-up has begun applying live events on top of
        // it, and replaying a scope now would re-drive snapshots BEHIND events that already
        // superseded them. Stop.
        if rec.phase > RebuildPhase::Replayed {
            return Ok(ReplayOutcome::default());
        }
        if rec.phase < RebuildPhase::CursorsReset {
            return Err(RebuildError::PhaseOutOfOrder {
                attempted: RebuildPhase::Replayed,
                durable: rec.phase,
            });
        }
        // At EXACTLY `Replayed` we do not early-return. The phase is a floor, not the truth about
        // which corpora are done — that is `owners_replayed`. A caller that previously passed an
        // incomplete scope set (a partial replay, or a resume that hadn't yet loaded the full
        // registry) would otherwise have permanently pinned the phase to `Replayed` with corpora
        // still missing, and every later attempt to finish the job would no-op silently. Topping up
        // the missing scopes here is what makes the phase converge to its own meaning.

        self.reassert_lease(key, &rec, holder, now)?;
        rec = self.checked(key, holder, now)?;
        let mut outcome = ReplayOutcome::default();
        for scope in scopes {
            let scope_key = scope.as_key();
            // Ask the owner what this scope's truth IS — for EVERY scope in the set, including ones
            // an earlier pass already replayed. These subjects are the independent expectation: they
            // come from OWNER TRUTH, not from the index, so comparing the rebuilt index against them
            // is a real check rather than a restatement. It is the same deterministic
            // `replay(scope, None)` the driver below re-drives, so the two agree by construction.
            //
            // Collected BEFORE the already-replayed skip, deliberately: on a resumed rebuild the
            // index legitimately holds the corpora an earlier pass replayed, so an expectation
            // covering only THIS pass's scopes would fail a correct resume.
            for source in sources {
                if source.owner_token() == scope.owner {
                    for draft in source.replay(scope, None) {
                        outcome.replayed_subjects.insert(draft.subject.0.clone());
                    }
                }
            }
            if rec.owners_replayed.contains(&scope_key) {
                continue; // already driven in an earlier attempt.
            }
            let job = self.reindexer.replay_scope_without_wipe(
                &key.tenant,
                scope,
                sources,
                outbox,
                ctx_base.clone(),
            )?;
            outcome.docs_indexed += job.progress().docs_indexed;
            // Journal the scope as done BEFORE moving on, so a crash resumes after it.
            let done = scope_key.clone();
            self.advance(key, &rec, holder, now, |next| {
                next.owners_replayed.insert(done);
            })?;
            rec = self.checked(key, holder, now)?;
        }

        if rec.phase < RebuildPhase::Replayed {
            self.advance(key, &rec, holder, now, |next| {
                next.phase = RebuildPhase::Replayed;
            })?;
        }
        Ok(outcome)
    }

    /// **Phase 5 — apply live events up to the recorded high-water mark.**
    ///
    /// `rows` is the committed live-event stream in commit order. Only the prefix at or below the
    /// recorded mark is applied: events above it arrived after the fence and are ordinary live
    /// intake's business once reads reopen. Applying them here instead would move the finish line
    /// every time a producer wrote, and the rebuild would never converge under load.
    ///
    /// Idempotent — every apply is an upsert keyed on the document id, so a redelivered or re-run
    /// prefix converges to the same index.
    pub fn catch_up(
        &self,
        key: &RebuildKey,
        holder: &str,
        now: u64,
        rows: &[myelin_events::OutboxRow],
    ) -> Result<CatchUpOutcome, RebuildError> {
        let rec = self.checked(key, holder, now)?;
        if rec.phase >= RebuildPhase::CaughtUp {
            return Ok(CatchUpOutcome::default());
        }
        if rec.phase < RebuildPhase::Replayed {
            return Err(RebuildError::PhaseOutOfOrder {
                attempted: RebuildPhase::CaughtUp,
                durable: rec.phase,
            });
        }
        // The ceiling is the fence-time per-aggregate `seq` watermark — a STORE-assigned position,
        // not a row position and not a producer clock. See `high_water_seqs`.
        //
        // The fence records a watermark on every rebuild, so an empty map is only reachable when the
        // outbox held nothing at fence time. That is legitimate (a fresh cell), and it correctly
        // means "apply nothing" — every aggregate is absent, so every row is post-fence.
        if rec.phase < RebuildPhase::Fenced {
            return Err(RebuildError::MissingHighWaterMark);
        }
        let watermark = rec.high_water_seqs.clone();
        self.reassert_lease(key, &rec, holder, now)?;
        let mut outcome = CatchUpOutcome::default();
        for row in rows.iter() {
            // At or below this aggregate's fence-time mark. An ABSENT aggregate had no committed
            // rows at fence time, so everything it holds now is post-fence: skip. Explicit `Option`
            // rather than `unwrap_or(0)` — `seq` is 0-based, so a 0 default would admit each such
            // aggregate's first event.
            match watermark.get(&row.aggregate.0) {
                Some(mark) if row.seq <= *mark => {}
                _ => continue,
            }
            // Tenant-first: a rebuild applies only ITS partition's events. A row for another tenant
            // or region rides the same shared outbox and must not be projected here.
            if row.envelope.tenant != key.tenant || row.envelope.region != key.region {
                continue;
            }
            self.reindexer
                .indexer()
                .index(&row.envelope)
                .map_err(|e| RebuildError::Engine(catch_up_failure_category(&e)))?;
            outcome.applied += 1;
            outcome.applied_subjects.insert(row.envelope.subject.0.clone());
        }
        self.advance(key, &rec, holder, now, |next| {
            next.phase = RebuildPhase::CaughtUp;
        })?;
        Ok(outcome)
    }

    /// **Phase 6 — verify, then reopen reads.**
    ///
    /// Checks, in order:
    /// 1. **document count parity** — the index holds as many documents as the rebuild indexed;
    /// 2. **document set parity** — and the SAME ones (a digest, because equal counts over different
    ///    sets is exactly the failure a count check passes);
    /// 3. **vector parity** — every document that should carry a live embedding does, and none that
    ///    should not;
    /// 4. **zero legacy identities** — across all three id spaces, not just documents;
    /// 5. **zero lag** — no event is still in flight through the indexer.
    ///
    /// Only on a clean sweep does the phase advance to `Complete` and reads reopen. A failure leaves
    /// the record fenced, which is the honest state: the rebuild did not finish, so the index must
    /// not be served.
    pub fn verify_and_open(
        &self,
        key: &RebuildKey,
        holder: &str,
        now: u64,
        expected: &ExpectedCorpus,
    ) -> Result<RebuildReport, RebuildError> {
        let rec = self.checked(key, holder, now)?;
        if rec.phase < RebuildPhase::CaughtUp {
            return Err(RebuildError::PhaseOutOfOrder {
                attempted: RebuildPhase::Verified,
                durable: rec.phase,
            });
        }

        let indexer = self.reindexer.indexer();
        let inventory = indexer
            .inventory(&key.tenant, &key.region)
            .map_err(|_| RebuildError::Engine("index inventory read failed"))?;

        // An INDEPENDENT leg that survives a self-consistency expectation: the replay and catch-up
        // reported driving documents through the indexer, yet the index is empty. That is the
        // signature of a wipe that ran after the replay (a displaced holder, a re-entered phase) and
        // it is invisible to a count/digest comparison built from the index itself.
        // **The corpus-destroyed leg.** An index that HELD documents at fence time must not verify
        // as rebuilt while holding none — and must not verify against an EMPTY expectation either,
        // because an empty expectation makes every other leg `0 == 0`. This is the shape a missing
        // or unregistered owner source produces: the wipe runs, the replay finds no truth, and
        // without this check verification passes and reads reopen over nothing.
        let found_docs = inventory.doc_ids.len();
        if let Some(before) = rec.pre_wipe_docs {
            if before > 0 && (found_docs == 0 || expected.doc_ids.is_empty()) {
                return Err(RebuildError::VerificationFailed(
                    VerifyFailure::CorpusDestroyed {
                        before,
                        after: found_docs,
                        expected: expected.doc_ids.len(),
                    },
                ));
            }
        }
        if found_docs == 0 && (expected.docs_replayed > 0 || expected.docs_caught_up > 0) {
            return Err(RebuildError::VerificationFailed(
                VerifyFailure::EmptyAfterNonEmptyReplay {
                    replayed: expected.docs_replayed,
                    caught_up: expected.docs_caught_up,
                },
            ));
        }
        if found_docs != expected.doc_ids.len() {
            return Err(RebuildError::VerificationFailed(
                VerifyFailure::DocCountMismatch {
                    expected: expected.doc_ids.len(),
                    found: found_docs,
                },
            ));
        }

        let digest = doc_set_digest(inventory.doc_ids.iter().map(String::as_str));
        let expected_digest = doc_set_digest(expected.doc_ids.iter().map(String::as_str));
        if digest != expected_digest {
            return Err(RebuildError::VerificationFailed(
                VerifyFailure::DocDigestMismatch,
            ));
        }

        // Vector parity by COUNT and by SET. A count-only check passes when the right NUMBER of
        // embeddings exists on the wrong subjects — the same failure the document digest exists to
        // catch, and the vector space deserves the same treatment because it answers queries
        // independently of the document space.
        let found_vectors = inventory.vector_doc_ids.len();
        if found_vectors != expected.vector_doc_ids.len() {
            return Err(RebuildError::VerificationFailed(
                VerifyFailure::VectorParityMismatch {
                    expected: expected.vector_doc_ids.len(),
                    found: found_vectors,
                },
            ));
        }
        let vector_digest = doc_set_digest(inventory.vector_doc_ids.iter().map(String::as_str));
        let expected_vector_digest =
            doc_set_digest(expected.vector_doc_ids.iter().map(String::as_str));
        if vector_digest != expected_vector_digest {
            return Err(RebuildError::VerificationFailed(
                VerifyFailure::VectorDigestMismatch,
            ));
        }

        // All three id spaces — a legacy identity surviving only as a vector or only as a metadata
        // record is still a legacy identity surviving.
        let legacy = inventory
            .all_ids()
            .into_iter()
            .filter(|id| is_legacy_blob_id(id))
            .count();
        if legacy != 0 {
            return Err(RebuildError::VerificationFailed(
                VerifyFailure::LegacyIdentitiesSurvived { count: legacy },
            ));
        }

        let lag = indexer.index_lag();
        if lag != 0 {
            return Err(RebuildError::VerificationFailed(VerifyFailure::NonZeroLag {
                lag,
            }));
        }

        // Green: journal Verified then Complete. Reads reopen only at Complete, so a crash between
        // the two leaves reads fenced — the safe side.
        self.advance(key, &rec, holder, now, |next| {
            next.phase = RebuildPhase::Verified;
        })?;
        let rec = self.checked(key, holder, now)?;
        self.advance(key, &rec, holder, now, |next| {
            next.phase = RebuildPhase::Complete;
        })?;

        Ok(RebuildReport {
            high_water_mark: rec.high_water_mark.unwrap_or(0),
            owners_replayed: rec.owners_replayed.len(),
            docs_replayed: expected.docs_replayed,
            docs_caught_up: expected.docs_caught_up,
            docs_indexed: found_docs,
            vectors_indexed: found_vectors,
            digest,
            legacy_identities: 0,
        })
    }
}

/// What a [`RebuildCoordinator::replay_all`] pass drove, INDEPENDENT of the index it drove it into.
///
/// `replayed_subjects` is read from the owners' `replay` drafts — owner truth — which is what makes
/// it usable as a verification expectation. Building the expectation from the index instead makes
/// count, digest and vector parity `x == x`; see [`ExpectedCorpus::from_index`].
#[derive(Clone, Default)]
pub struct ReplayOutcome {
    /// Snapshots driven through the live indexer this pass.
    pub docs_indexed: usize,
    /// Every subject the owners replayed — the document set owner truth says should exist.
    pub replayed_subjects: BTreeSet<String>,
}

impl std::fmt::Debug for ReplayOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Subjects are artifact refs (a blob ref embeds repo + path) — counts only.
        f.debug_struct("ReplayOutcome")
            .field("docs_indexed", &self.docs_indexed)
            .field("replayed_subjects", &self.replayed_subjects.len())
            .finish()
    }
}

/// What a [`RebuildCoordinator::catch_up`] pass applied, INDEPENDENT of the index.
///
/// The pre-fence live events belong in the verification expectation alongside owner truth: they
/// were applied to the generation the wipe destroyed and re-applied here, so the rebuilt index
/// legitimately holds documents the owners' `replay` did not name.
#[derive(Clone, Default)]
pub struct CatchUpOutcome {
    /// Live events applied.
    pub applied: usize,
    /// The subjects those events addressed.
    pub applied_subjects: BTreeSet<String>,
}

impl std::fmt::Debug for CatchUpOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CatchUpOutcome")
            .field("applied", &self.applied)
            .field("applied_subjects", &self.applied_subjects.len())
            .finish()
    }
}

/// **What the rebuild believes it produced** — the expectation side of parity verification.
///
/// Built by the caller from what the replay and catch-up actually indexed. Comparing the index
/// against this (rather than against a recount of itself) is what makes the check meaningful: it
/// catches both a document that failed to land and a document that survived the wipe.
#[derive(Clone, Default)]
pub struct ExpectedCorpus {
    /// Every document id the rebuild indexed.
    pub doc_ids: BTreeSet<String>,
    /// Every document id that should carry a live embedding.
    pub vector_doc_ids: BTreeSet<String>,
    /// Documents indexed during replay (reporting only).
    pub docs_replayed: usize,
    /// Documents applied during catch-up (reporting only).
    pub docs_caught_up: usize,
}

impl ExpectedCorpus {
    /// **Build the expectation from OWNER TRUTH — the form in which verification is a real check.**
    ///
    /// `replay` carries the subjects the owners said should exist; `caught_up` is the number of
    /// live events catch-up applied. Comparing the rebuilt index against this catches what
    /// [`Self::from_index`] structurally cannot: a corpus that never replayed, a replay that indexed
    /// nothing, a wipe that landed after the replay, and a Git-only replay that lost three
    /// subsystems.
    ///
    /// `semantic_subjects` names the subset expected to carry a live embedding (a corpus whose spec
    /// is semantic). Pass an empty set when no corpus is semantic.
    ///
    /// Note the deliberate strictness: a subject the owner replayed but whose projection resolved
    /// `Gone` will be missing from the index and FAIL this check. That is correct — the owner's
    /// truth and its projection disagreeing is a real inconsistency, and failing loud beats
    /// reopening reads over a corpus nobody can account for.
    pub fn from_replay(
        replay: &ReplayOutcome,
        catch_up: &CatchUpOutcome,
        semantic_subjects: &BTreeSet<String>,
    ) -> ExpectedCorpus {
        // Owner truth PLUS the pre-fence live events catch-up re-applied. Those were applied to the
        // generation the wipe destroyed, so the rebuilt index legitimately holds documents the
        // owners' `replay` did not name; omitting them would fail a correct rebuild.
        let doc_ids: BTreeSet<String> = replay
            .replayed_subjects
            .union(&catch_up.applied_subjects)
            .cloned()
            .collect();
        ExpectedCorpus {
            vector_doc_ids: doc_ids.intersection(semantic_subjects).cloned().collect(),
            doc_ids,
            docs_replayed: replay.docs_indexed,
            docs_caught_up: catch_up.applied,
        }
    }

    /// **A SELF-CONSISTENCY snapshot of the index — NOT an independent expectation.**
    ///
    /// Read this warning before using it. The count, digest and vector legs of
    /// [`RebuildCoordinator::verify_and_open`] compare the index against this value; building it by
    /// reading the same index makes those three legs `x == x`. Verification then cannot catch:
    /// a corpus that never replayed, a replay that indexed nothing, a wipe that ran twice, a
    /// catch-up that applied nothing, or a concurrent holder that emptied the index. Only the
    /// zero-legacy-identity and zero-lag legs carry information against this expectation.
    ///
    /// It exists because the coordinator genuinely has no second copy of owner truth — Search never
    /// reads an owner database, so it cannot independently enumerate what SHOULD be there. What it
    /// can do is pin the built index so later divergence is caught, which is what this provides.
    ///
    /// **For a real migration, construct [`ExpectedCorpus`] directly from an independent source**
    /// (an operator's manifest, or the owner's own count) — that is the only form in which claim
    /// "count/hash parity verified" is a real check rather than a restatement. The adversarial
    /// drill's `a_git_only_replay_is_caught_as_lossy` demonstrates the difference: with an
    /// independent expectation a Git-only replay FAILS verification; with this one it passes.
    ///
    /// **Named follow-on:** the sound fix is for the replay to REPORT the document set it drove
    /// (the drafts it replayed, minus those the owner resolved `Gone`), so the coordinator can build
    /// an expectation that is independent of the index without reading an owner database. That is a
    /// change to the [`ReindexSource`] seam, not to this module.
    pub fn from_index(
        reindexer: &SearchReindexer,
        key: &RebuildKey,
        docs_replayed: usize,
        docs_caught_up: usize,
    ) -> Result<ExpectedCorpus, RebuildError> {
        let inv = reindexer
            .indexer()
            .inventory(&key.tenant, &key.region)
            .map_err(|_| RebuildError::Engine("index inventory read failed"))?;
        Ok(ExpectedCorpus {
            doc_ids: inv.doc_ids.iter().cloned().collect(),
            vector_doc_ids: inv.vector_doc_ids.iter().cloned().collect(),
            docs_replayed,
            docs_caught_up,
        })
    }
}
