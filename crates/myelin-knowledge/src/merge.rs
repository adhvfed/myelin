//! # The per-block CAS merge floor (Layer 3a) — KN-P13 / P-303, M3 (KN-D3, the named-floor proof)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/knowledge-platform/architecture/02-internals-and-algorithms.md`
//! §1 (the layered collab stack — **Layer 3 MERGE: CAS floor (v1) → Yrs CRDT (promotion-triggered)**;
//! the merge engine slots OVER the [`crate::transport`] Layer-1 resume-cursor transport, it never
//! touches it), §3.2 (the CAS floor: per-block optimistic compare-and-swap on `block.version`;
//! `rows_affected == 0` → `Conflict{current}`; the loser RECONCILES, never silently overwritten;
//! different blocks edit freely in parallel; the `rows_affected == 0` rate is the CRDT-promotion
//! trigger metric), §3.3/§3.4 (the promotion trigger: the first true concurrent-edit conflict measured
//! via the per-doc CAS conflict rate, KQ-1).
//!
//! **Canon (read in full first):** ../../VISION.md §3 (name-your-floors — a floor that masquerades as
//! done is the failure); external-insights/04-hard-problems.md §2.1 (CRDT-after-CAS — the v1 floor
//! guarantees no SILENT overwrite, does not merge; the loser reconciles); external-insights/01 §3
//! (prove-it: 0 silent overwrites is a quantified gate).
//!
//! **Contract-index:** row **3.5** the firehose resume-cursor transport — **CONSUMED** (the CAS ops
//! RIDE the [`crate::transport`] transport; this module is the Layer-3 apply engine that turns an
//! [`crate::transport::OpKind`] edit op into a versioned block write — it does not re-implement the
//! transport). The Layer-2 per-op `Id.check` + the zookie new-enemy guard (4.2/4.10) is **KN-P14**;
//! this prompt is the merge GUARD only.
//!
//! ## What this module ships (KN-P13's owned work — Layer 3a)
//! - **[`CasStore`]** — the in-memory model of the `block` row's CONTENT + `version` columns
//!   (`inline` / `props` / `version`, [`crate::store`] §2.3). The [`crate::block_tree::BlockTree`] owns
//!   the load-bearing TREE columns (`parent_id` / `order_key`); this owns the per-block CONTENT the CAS
//!   guard versions. The two are the same `block` row split by concern (EI-01 §7 — one row, two views).
//! - **[`CasStore::edit_block`]** — the per-block optimistic compare-and-swap:
//!   `EDIT_BLOCK(block_id, expected_version, new_inline, new_props)` runs
//!   `UPDATE block SET inline=?, props=?, version=version+1 WHERE tenant=? AND block_id=? AND
//!   version=expected_version`; on `rows_affected == 0` returns [`CasOutcome::Conflict`] carrying the
//!   server's CURRENT block state — **the loser RECONCILES, never silently overwritten** (§3.2). The
//!   lowered SQL is the visible query-plan artifact [`cas_update_sql`].
//! - **Per-block independence** — the guard is `WHERE block_id=? AND version=?`, so two writers editing
//!   DIFFERENT blocks never collide (no false conflict; §3.2 "different blocks edit freely in
//!   parallel").
//! - **[`SoftLockTable`]** — advisory soft-locks ("someone is editing this block") over the
//!   ephemeral awareness channel ([`crate::transport::Presence`] tier). ADVISORY: a soft-lock does NOT
//!   gate the CAS write (the CAS guard is the real safety; the soft-lock is a UX courtesy that reduces
//!   the conflict RATE). Never persisted (§2.3).
//! - **[`CasStore::snapshot_block`] / [`CasStore::restore_block`]** — per-block snapshot/restore
//!   layered on the CAS guard (§3.2): capture a block's state, restore it later via a CAS write (so a
//!   restore that races a live edit is itself a conflict, never a silent clobber).
//! - **[`OfflineQueue`]** — offline = read + queued light-edit reconciled via the CAS floor (§3.2 /
//!   roadmap §3): a client that edited offline replays its queued edits through [`CasStore::edit_block`]
//!   on reconnect; an edit whose `expected_version` is stale gets a [`CasOutcome::Conflict`] to
//!   reconcile (it is NOT force-applied). The deep offline-first answer is KN-P29 (the CRDT).
//! - **[`ConflictMeter`]** — the CAS-conflict-rate metric (the `rows_affected == 0` fraction) emitted
//!   to telemetry — it is **the CRDT-promotion trigger metric (KQ-1)** KN-P29 reads (§3.4). A
//!   PII-free per-doc rate (committed / conflicted counts → fraction).
//!
//! ## FLOORS NAMED (VISION §3 — the failure is a floor that masquerades as done)
//! - **CAS — NO MERGE (the named floor).** This guarantees no SILENT overwrite; it does NOT blend two
//!   concurrent same-block edits — the loser is rejected with the current server state to reconcile
//!   (EI-04 §2.1). **Follow-on: the Yrs CRDT (KN-1, KN-P29, M5)**, which actually merges. **Trigger:
//!   the first true concurrent-edit conflict** measured via [`ConflictMeter`]'s conflict-rate crossing
//!   the KQ-1 threshold (§3.4). The CAS ops ride the SAME [`crate::transport`] transport the Yrs update
//!   bytes will (the [`crate::transport::OpKind::EnginePromote`] cutover marker) — the promotion is a
//!   Layer-3 swap, NOT a transport rewrite.
//! - **Offline = read + queued light-edit (the named floor).** A reconnecting client replays its queue
//!   through the CAS guard; a stale edit conflicts. **Follow-on: full offline-first** (the CRDT's
//!   convergent merge, KN-P29) — so two long-offline divergent edits MERGE instead of one losing.
//! - **The live `block` row PERSIST is the in-memory [`CasStore`] on the substrate floor** (no live
//!   Postgres in `cargo build`, P-S12). The `UPDATE … WHERE version = expected_version` CAS semantics
//!   are modelled byte-faithfully ([`cas_update_sql`] is the lowered SQL); the real Postgres co-commit
//!   rides the KN-P05 store ([`crate::store`]). The KN-D3 drill here proves the CAS PROPERTY (0 silent
//!   overwrites, per-block independence) over the in-process guard — the engine-agnostic substrate.
//!
//! ## MANDATORY-CORE MUTATION FLOOR (the KN-P13 cargo-mutants gate — TESTS field)
//! The CAS GUARD is mandatory-core: [`CasStore::edit_block`] (the
//! `version == expected_version` precondition + the `version + 1` bump + the `rows_affected == 0`
//! conflict arm). The stated floor: **100% mutation score on the CAS-guard path**. Every
//! comparison/arithmetic/branch mutant in `edit_block` is killed by the unit tests + the KN-D3 chained
//! drill: a mutated precondition (`==` → `!=` / `>=`), a dropped `+ 1` bump, or a swapped
//! Committed/Conflict arm all flip the no-silent-overwrite assertion (the loser would silently win, or
//! the winner would be rejected) or the per-block-independence assertion. The conflict-rate accessors /
//! Display arms are not core. Run: `cargo mutants -p myelin-knowledge -f merge.rs`.

use crate::block_tree::BlockId;
use std::collections::{BTreeMap, HashMap, HashSet};

/// **The per-block CONTENT + `version` state (the `block` row's content columns, [`crate::store`]
/// §2.3).** The CAS guard versions THIS — the [`crate::block_tree::BlockTree`] owns the orthogonal
/// TREE columns (`parent_id` / `order_key`); a content edit (this) and a tree move (that) are
/// INDEPENDENT writes to the same row, which is exactly why a same-block content conflict and a
/// different-block move never interfere.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockState {
    /// The block's inline text (the `inline` column — the serialized inline run).
    pub inline: String,
    /// The block's non-inline properties (the `props` JSONB column — checkbox / callout colour / …),
    /// held opaque here (the schema validation of a prop op is KN-P14, Layer 2).
    pub props: String,
    /// **The CAS version (the `block.version` column — the optimistic-concurrency token).** A
    /// successful [`CasStore::edit_block`] bumps it by exactly 1; a writer must present the
    /// `expected_version` it last read, or it loses the CAS and reconciles.
    pub version: u64,
}

impl BlockState {
    /// A fresh block at `version = 1` (the first write; the [`crate::store`] DDL's `version bigint NOT
    /// NULL` is seeded to 1 at insert — a `version = 0` would mean "never written").
    pub fn new(inline: impl Into<String>, props: impl Into<String>) -> BlockState {
        BlockState { inline: inline.into(), props: props.into(), version: 1 }
    }
}

/// **The outcome of an `EDIT_BLOCK` CAS write (§3.2).** Either the writer held the current version and
/// COMMITTED (carrying the new state at the bumped version), or a concurrent writer had already moved
/// the version on and the writer LOST — handed the server's CURRENT state to reconcile, **never
/// silently overwritten**. A `Conflict` is NOT an error: it is the EXPECTED concurrent-edit case the
/// floor's whole point is to surface (EI-04 §2.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CasOutcome {
    /// The CAS held (the writer's `expected_version` matched): the write committed at the bumped
    /// version. Carries the new committed [`BlockState`] (`version = expected_version + 1`).
    Committed(BlockState),
    /// The CAS missed (`rows_affected == 0` — a concurrent writer won): the loser is REJECTED and
    /// handed the server's CURRENT state to reconcile against. **No silent overwrite happened.**
    Conflict {
        /// The server's current block state (the basis the loser reconciles its edit onto).
        current: BlockState,
    },
}

impl CasOutcome {
    /// `true` iff the CAS held (the write committed — the winner).
    pub fn committed(&self) -> bool {
        matches!(self, CasOutcome::Committed(_))
    }

    /// `true` iff the CAS missed (the writer lost and must reconcile — the loser).
    pub fn is_conflict(&self) -> bool {
        matches!(self, CasOutcome::Conflict { .. })
    }

    /// The resulting/current state either way (the committed state, or the current server state the
    /// loser reconciles onto).
    pub fn state(&self) -> &BlockState {
        match self {
            CasOutcome::Committed(s) => s,
            CasOutcome::Conflict { current } => current,
        }
    }
}

/// **Why a CAS write could not even be attempted (the typed LOUD pre-conditions — distinct from a
/// [`CasOutcome::Conflict`], which is a NORMAL concurrent-edit outcome).**
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CasError {
    /// `EDIT_BLOCK` named a `block_id` that has no content row yet (an edit before the insert — the
    /// insert is a [`crate::block_tree::BlockTree::insert_block`] + a [`CasStore::insert_block`]).
    NoSuchBlock(BlockId),
    /// An insert reused a `block_id` that already has a content row (content is inserted once per id).
    DuplicateBlock(BlockId),
}

impl std::fmt::Display for CasError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CasError::NoSuchBlock(b) => write!(f, "no content row for block {}", b.as_str()),
            CasError::DuplicateBlock(b) => {
                write!(f, "content row for block {} already exists (insert once)", b.as_str())
            }
        }
    }
}

impl std::error::Error for CasError {}

/// **The per-block CAS content store for ONE doc (Layer 3a, §3.2 — the in-memory model of the `block`
/// row's content + `version` columns).** Holds each block's [`BlockState`]; [`Self::edit_block`] is the
/// optimistic compare-and-swap that GUARANTEES no silent overwrite. Engine-agnostic: a Yrs CRDT
/// (KN-P29) replaces THIS layer behind the same op stream, the transport unchanged.
///
/// The store is per-doc (the `(tenant, page_id)` partition the [`crate::transport`] pins). The
/// [`ConflictMeter`] rides alongside (every [`Self::edit_block`] feeds it the commit/conflict tally
/// for the CRDT-promotion trigger metric, §3.4).
#[derive(Debug, Default, Clone)]
pub struct CasStore {
    /// `block_id → its content + version` (the `block` row content columns, keyed by the stable id).
    blocks: BTreeMap<BlockId, BlockState>,
    /// The CAS-conflict-rate meter (the `rows_affected == 0` fraction — the CRDT-promotion trigger).
    meter: ConflictMeter,
}

impl CasStore {
    /// A fresh, empty content store (a brand-new doc).
    pub fn new() -> CasStore {
        CasStore::default()
    }

    /// **Insert a block's initial content row at `version = 1`** (the content half of an insert; the
    /// tree half is [`crate::block_tree::BlockTree::insert_block`]). The id is inserted ONCE.
    ///
    /// # Errors
    /// [`CasError::DuplicateBlock`] if a content row for `block_id` already exists.
    pub fn insert_block(
        &mut self,
        block_id: BlockId,
        inline: impl Into<String>,
        props: impl Into<String>,
    ) -> Result<&BlockState, CasError> {
        if self.blocks.contains_key(&block_id) {
            return Err(CasError::DuplicateBlock(block_id));
        }
        self.blocks.insert(block_id.clone(), BlockState::new(inline, props));
        Ok(self.blocks.get(&block_id).expect("just inserted"))
    }

    /// Read a block's current state (the basis a client presents an `expected_version` against).
    pub fn get(&self, block_id: &BlockId) -> Option<&BlockState> {
        self.blocks.get(block_id)
    }

    /// **`EDIT_BLOCK(block_id, expected_version, new_inline, new_props)` — the per-block optimistic
    /// compare-and-swap (§3.2, the load-bearing CAS guard).** Models
    /// `UPDATE block SET inline=?, props=?, version=version+1 WHERE tenant=? AND block_id=? AND
    /// version=expected_version`:
    /// - if the block's current `version == expected_version` → the CAS HOLDS: write the new content,
    ///   bump `version` by 1, return [`CasOutcome::Committed`] (the winner).
    /// - if it differs (a concurrent writer already bumped it → `rows_affected == 0`) → the CAS MISSES:
    ///   return [`CasOutcome::Conflict`] carrying the server's CURRENT state, **without writing** — the
    ///   loser reconciles, NEVER silently overwritten.
    ///
    /// Every call feeds the [`ConflictMeter`] (a commit or a conflict) — the CRDT-promotion trigger
    /// metric (§3.4). The guard is per-block (`WHERE block_id = ?`), so edits to DIFFERENT blocks never
    /// produce a false conflict (§3.2).
    ///
    /// # Errors
    /// [`CasError::NoSuchBlock`] if `block_id` has no content row (edit before insert).
    pub fn edit_block(
        &mut self,
        block_id: &BlockId,
        expected_version: u64,
        new_inline: impl Into<String>,
        new_props: impl Into<String>,
    ) -> Result<CasOutcome, CasError> {
        let current = self.blocks.get_mut(block_id).ok_or_else(|| CasError::NoSuchBlock(block_id.clone()))?;
        // ── THE CAS GUARD (the WHERE version = expected_version precondition) ──────────────────────
        if current.version == expected_version {
            // The CAS held → rows_affected == 1. Write the new content + bump the version by exactly 1.
            current.inline = new_inline.into();
            current.props = new_props.into();
            current.version += 1;
            let committed = current.clone();
            self.meter.record_commit();
            Ok(CasOutcome::Committed(committed))
        } else {
            // The CAS missed → rows_affected == 0 (a concurrent writer already moved the version on).
            // Reject the loser with the CURRENT server state — NO WRITE, never a silent overwrite.
            let conflict = current.clone();
            self.meter.record_conflict();
            Ok(CasOutcome::Conflict { current: conflict })
        }
    }

    /// **Snapshot a block's current state (§3.2 — snapshot/restore layered on the CAS guard).** A
    /// caller captures the [`BlockState`] to restore later via [`Self::restore_block`]. A snapshot is a
    /// pure read (it does not bump the version).
    ///
    /// # Errors
    /// [`CasError::NoSuchBlock`] if the block has no content row.
    pub fn snapshot_block(&self, block_id: &BlockId) -> Result<BlockState, CasError> {
        self.blocks.get(block_id).cloned().ok_or_else(|| CasError::NoSuchBlock(block_id.clone()))
    }

    /// **Restore a block to a previously-[`Self::snapshot_block`]ed state THROUGH the CAS guard
    /// (§3.2).** A restore is itself a CAS write at `expected_version` — so a restore that RACES a live
    /// edit (the version moved on since the caller decided to restore) is a [`CasOutcome::Conflict`],
    /// never a silent clobber of the concurrent edit. The restored content is the snapshot's
    /// inline/props; the version still bumps (a restore is a new revision, not a rewind of the
    /// counter).
    ///
    /// # Errors
    /// [`CasError::NoSuchBlock`] if the block has no content row.
    pub fn restore_block(
        &mut self,
        block_id: &BlockId,
        expected_version: u64,
        snapshot: &BlockState,
    ) -> Result<CasOutcome, CasError> {
        self.edit_block(block_id, expected_version, snapshot.inline.clone(), snapshot.props.clone())
    }

    /// The CAS-conflict-rate meter (the CRDT-promotion trigger metric, §3.4 / KQ-1).
    pub fn meter(&self) -> &ConflictMeter {
        &self.meter
    }

    /// The number of blocks with a content row.
    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    /// `true` iff no block has a content row yet.
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
}

/// **The lowered CAS `UPDATE` SQL (§3.2 — the visible query-plan artifact).** The per-block
/// compare-and-swap is `UPDATE block SET … version = version + 1 WHERE (tenant, block_id) = (..) AND
/// version = expected_version`. The `WHERE version = expected_version` is the optimistic-concurrency
/// guard: on a precondition miss Postgres reports `rows_affected == 0` (no row matched the version),
/// which the engine maps to [`CasOutcome::Conflict`]. The leading `(tenant, block_id)` is the PK
/// equality (a single-row probe, never a scan); the per-block guard means a different block's edit
/// touches a DIFFERENT row, so two different-block writes never serialize against each other.
pub fn cas_update_sql() -> &'static str {
    "UPDATE block \
        SET inline = $4, props = $5, version = version + 1, edited_by = $6, edited_at = now() \
      WHERE tenant = $1 AND block_id = $2 AND version = $3"
}

/// **The advisory soft-lock table (§3.2 — "someone is editing this block," over the awareness
/// channel).** A soft-lock is ADVISORY: it is a UX courtesy that REDUCES the concurrent-edit rate (a
/// second editor sees "X is editing this block" and waits), but it does NOT gate the CAS write — the
/// CAS guard ([`CasStore::edit_block`]) is the real, mandatory safety. A soft-lock is ephemeral (rides
/// the [`crate::transport::Presence`] awareness tier, §2.3) and is NEVER persisted — there is no path
/// from a soft-lock to the [`crate::store`] `block` row.
#[derive(Debug, Default, Clone)]
pub struct SoftLockTable {
    /// `block_id → the opaque client/session id holding the advisory lock` (one holder at a time; a
    /// later acquire reports the EXISTING holder rather than stealing — advisory, not mandatory).
    locks: HashMap<BlockId, String>,
}

/// The outcome of a soft-lock acquire (advisory — never blocks a CAS write either way).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SoftLock {
    /// The caller acquired the advisory lock (no one else was editing this block).
    Acquired,
    /// Someone ELSE is already advisory-editing this block — carries the holder's opaque client id so
    /// the UI can show "X is editing this block." The caller MAY still edit (the CAS guard is the real
    /// safety); the soft-lock just lets a courteous client wait.
    Held {
        /// The opaque client/session id currently advisory-editing the block (PII-free).
        by: String,
    },
}

impl SoftLockTable {
    /// A fresh soft-lock table (no advisory locks held).
    pub fn new() -> SoftLockTable {
        SoftLockTable::default()
    }

    /// **Acquire the advisory soft-lock on a block for `client_id`.** If the block is free, the caller
    /// takes it ([`SoftLock::Acquired`]); if someone else holds it, returns [`SoftLock::Held`] naming
    /// them (the caller is NOT blocked — advisory). Re-acquiring one's OWN lock is idempotent
    /// ([`SoftLock::Acquired`]).
    pub fn acquire(&mut self, block_id: &BlockId, client_id: &str) -> SoftLock {
        match self.locks.get(block_id) {
            Some(holder) if holder != client_id => SoftLock::Held { by: holder.clone() },
            _ => {
                self.locks.insert(block_id.clone(), client_id.to_string());
                SoftLock::Acquired
            }
        }
    }

    /// Release the advisory soft-lock IFF `client_id` holds it (a client cannot release another's
    /// lock). Releasing a lock one does not hold is a no-op (advisory — never an error).
    pub fn release(&mut self, block_id: &BlockId, client_id: &str) {
        if self.locks.get(block_id).map(|h| h == client_id).unwrap_or(false) {
            self.locks.remove(block_id);
        }
    }

    /// The opaque client id advisory-editing `block_id`, if any (the "who is editing this" awareness
    /// read the UI renders).
    pub fn holder(&self, block_id: &BlockId) -> Option<&String> {
        self.locks.get(block_id)
    }
}

/// **One queued offline edit (§3.2 / roadmap §3 — offline = read + queued light-edit).** A client that
/// edited a block while offline records the edit + the `expected_version` it last SAW (when it went
/// offline). On reconnect the queue is replayed through the CAS guard ([`OfflineQueue::reconcile`]);
/// an edit whose base version has since moved on gets a [`CasOutcome::Conflict`] to reconcile — it is
/// NOT force-applied (no silent overwrite, even offline).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuedEdit {
    /// The block the offline edit targets.
    pub block_id: BlockId,
    /// The version the client last saw before going offline (the CAS `expected_version`).
    pub expected_version: u64,
    /// The new inline content the offline edit produced.
    pub new_inline: String,
    /// The new props the offline edit produced.
    pub new_props: String,
}

/// **The offline edit queue (§3.2 — the named "read + queued light-edit" floor).** A client buffers
/// its offline edits here and replays them through the CAS floor on reconnect. The deep offline-first
/// answer (two long-divergent edits MERGE convergently) is KN-P29 (the CRDT); this floor guarantees no
/// SILENT overwrite even for an offline edit (a stale queued edit conflicts and reconciles).
#[derive(Debug, Default, Clone)]
pub struct OfflineQueue {
    edits: Vec<QueuedEdit>,
}

/// The result of reconciling one queued offline edit on reconnect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconcileResult {
    /// The edit that was reconciled.
    pub edit: QueuedEdit,
    /// Its CAS outcome (committed if the base version still held, else a conflict to reconcile).
    pub outcome: CasOutcome,
}

impl OfflineQueue {
    /// A fresh, empty offline queue (the client is online / has flushed).
    pub fn new() -> OfflineQueue {
        OfflineQueue::default()
    }

    /// **Queue a light-edit made while offline** (the block + the new content + the `expected_version`
    /// the client last saw). Held until [`Self::reconcile`] replays it on reconnect.
    pub fn queue(
        &mut self,
        block_id: BlockId,
        expected_version: u64,
        new_inline: impl Into<String>,
        new_props: impl Into<String>,
    ) {
        self.edits.push(QueuedEdit {
            block_id,
            expected_version,
            new_inline: new_inline.into(),
            new_props: new_props.into(),
        });
    }

    /// The number of queued offline edits awaiting reconcile.
    pub fn len(&self) -> usize {
        self.edits.len()
    }

    /// `true` iff there are no queued offline edits.
    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    /// **Reconcile the queued offline edits through the CAS floor on reconnect (§3.2).** Each queued
    /// edit is replayed via [`CasStore::edit_block`] at the `expected_version` it captured offline: if
    /// the block has not changed since, the edit COMMITS; if a concurrent writer moved the version on
    /// while the client was offline, the edit gets a [`CasOutcome::Conflict`] (the offline editor
    /// reconciles, NEVER a silent overwrite). DRAINS the queue (returns one [`ReconcileResult`] per
    /// queued edit, in order). An edit whose block was deleted while offline surfaces its
    /// [`CasError`] LOUDLY (it is collected as a per-edit error, not a silent drop).
    pub fn reconcile(&mut self, store: &mut CasStore) -> Vec<Result<ReconcileResult, CasError>> {
        let drained = std::mem::take(&mut self.edits);
        drained
            .into_iter()
            .map(|edit| {
                store
                    .edit_block(
                        &edit.block_id,
                        edit.expected_version,
                        edit.new_inline.clone(),
                        edit.new_props.clone(),
                    )
                    .map(|outcome| ReconcileResult { edit, outcome })
            })
            .collect()
    }
}

/// **The CAS-conflict-rate meter (§3.4 / KQ-1 — the CRDT-promotion trigger metric).** Counts CAS
/// COMMITS vs CONFLICTS per doc; the conflict RATE (the `rows_affected == 0` fraction) is the signal
/// KN-P29 reads to decide the Yrs CRDT promotion (the first true concurrent-edit conflict, sustained
/// above a threshold, triggers the engine swap). PII-free (two counts → a fraction); the metric NAME
/// is [`CAS_CONFLICT_RATE_METRIC`], emitted to the telemetry port (the harness §10.2 signal surface).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ConflictMeter {
    /// CAS writes that COMMITTED (`rows_affected == 1` — the winner / a no-contention edit).
    committed: u64,
    /// CAS writes that CONFLICTED (`rows_affected == 0` — the loser reconciles). The NUMERATOR of the
    /// conflict rate.
    conflicted: u64,
}

/// **The canonical metric NAME for the CAS conflict rate (the CRDT-promotion trigger, §3.4).** Lines
/// up with the harness §10.2 `SignalName` convention (`<subsystem>.<signal>`) so the telemetry port
/// reads Knowledge's emit as the same signal KN-P29's promotion logic asserts against. PII-free.
pub const CAS_CONFLICT_RATE_METRIC: &str = "knowledge.cas_conflict_rate";

impl ConflictMeter {
    /// A fresh meter (no edits recorded).
    pub fn new() -> ConflictMeter {
        ConflictMeter::default()
    }

    /// Record a CAS COMMIT (a winning / uncontended edit — the rate denominator grows, numerator does
    /// not). Called by [`CasStore::edit_block`] on the held-CAS path.
    pub fn record_commit(&mut self) {
        self.committed += 1;
    }

    /// Record a CAS CONFLICT (a losing edit — both the numerator AND the denominator grow). Called by
    /// [`CasStore::edit_block`] on the missed-CAS path. This is the event KN-P29 watches.
    pub fn record_conflict(&mut self) {
        self.conflicted += 1;
    }

    /// CAS commits recorded.
    pub fn committed(&self) -> u64 {
        self.committed
    }

    /// CAS conflicts recorded (the `rows_affected == 0` count — the conflict-rate numerator).
    pub fn conflicted(&self) -> u64 {
        self.conflicted
    }

    /// Total CAS edit attempts (commits + conflicts — the conflict-rate denominator).
    pub fn attempts(&self) -> u64 {
        self.committed + self.conflicted
    }

    /// **The CAS-conflict-rate (the `rows_affected == 0` fraction) — the CRDT-promotion trigger metric
    /// value (§3.4 / KQ-1).** `conflicted / (committed + conflicted)`, in `[0.0, 1.0]`. A fresh doc
    /// (0 attempts) has a rate of `0.0` (no contention observed yet — never a divide-by-zero). KN-P29
    /// promotes to the Yrs CRDT when this crosses the measured threshold (the first true concurrent
    /// conflict).
    pub fn conflict_rate(&self) -> f64 {
        let attempts = self.attempts();
        if attempts == 0 {
            0.0
        } else {
            self.conflicted as f64 / attempts as f64
        }
    }

    /// **The emitted telemetry sample: the metric NAME + the current conflict-rate value (§3.4).** The
    /// telemetry port reads this; KN-P29's promotion logic watches the named signal. A PII-free
    /// `(name, rate)` pair — no per-doc identity, no content.
    pub fn telemetry_sample(&self) -> (&'static str, f64) {
        (CAS_CONFLICT_RATE_METRIC, self.conflict_rate())
    }
}

/// **A page-wide set of blocks currently observed under SIMULTANEOUS multi-author presence (§3.4 — the
/// second CRDT-promotion trigger: "sustained multi-author simultaneous presence on one block").** A
/// helper for KN-P29's trigger: alongside the conflict RATE, sustained simultaneous presence on the
/// SAME block is the leading indicator of imminent true concurrent edits. Built from the ephemeral
/// [`crate::transport::Presence`] tier (never persisted).
#[derive(Debug, Default, Clone)]
pub struct SimultaneousPresence {
    /// `block_id → the set of opaque client ids currently present on it` (ephemeral, presence-tier).
    present: HashMap<BlockId, HashSet<String>>,
}

impl SimultaneousPresence {
    /// A fresh presence index.
    pub fn new() -> SimultaneousPresence {
        SimultaneousPresence::default()
    }

    /// Record that `client_id` is present (cursor/selection) on `block_id` (an awareness frame).
    pub fn enter(&mut self, block_id: &BlockId, client_id: &str) {
        self.present.entry(block_id.clone()).or_default().insert(client_id.to_string());
    }

    /// Record that `client_id` left `block_id` (presence is ephemeral; a drop is fine).
    pub fn leave(&mut self, block_id: &BlockId, client_id: &str) {
        if let Some(set) = self.present.get_mut(block_id) {
            set.remove(client_id);
            if set.is_empty() {
                self.present.remove(block_id);
            }
        }
    }

    /// **`true` iff `block_id` has 2+ simultaneous authors present (the multi-author trigger
    /// condition, §3.4).** KN-P29 watches this alongside [`ConflictMeter::conflict_rate`].
    pub fn is_contended(&self, block_id: &BlockId) -> bool {
        self.present.get(block_id).map(|s| s.len() >= 2).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bid(s: &str) -> BlockId {
        BlockId(s.to_string())
    }

    fn store_with_block() -> (CasStore, BlockId) {
        let mut s = CasStore::new();
        let b = bid("b1");
        s.insert_block(b.clone(), "hello", "{}").unwrap();
        (s, b)
    }

    // ── the CAS guard: winner commits, loser gets Conflict{current} ──────────────────────────────

    /// **The winner commits: a CAS at the current version writes + bumps the version by 1.**
    #[test]
    fn cas_winner_commits_and_bumps_version() {
        let (mut s, b) = store_with_block();
        assert_eq!(s.get(&b).unwrap().version, 1, "a fresh block is at version 1");
        let out = s.edit_block(&b, 1, "hello world", "{}").unwrap();
        assert!(out.committed(), "the CAS at the current version commits");
        assert_eq!(out.state().version, 2, "the version bumped by exactly 1");
        assert_eq!(out.state().inline, "hello world", "the new content was written");
        assert_eq!(s.get(&b).unwrap().version, 2, "the store reflects the committed write");
    }

    /// **THE NAMED-FLOOR PROPERTY: the loser is rejected with current state, NEVER silently
    /// overwritten (EI-04 §2.1).** A second writer presenting a STALE `expected_version` (it read v1,
    /// but v1 was already bumped to v2) gets `Conflict{current}` — and the store STILL holds the
    /// winner's content (the loser's bytes were never written).
    #[test]
    fn cas_loser_gets_conflict_current_zero_silent_overwrite() {
        let (mut s, b) = store_with_block();
        // Writer A reads v1, commits → v2.
        let a = s.edit_block(&b, 1, "A's edit", "{}").unwrap();
        assert!(a.committed());
        // Writer B ALSO read v1 (before A landed), now tries to write at expected_version = 1 → STALE.
        let bout = s.edit_block(&b, 1, "B's edit", "{}").unwrap();
        assert!(bout.is_conflict(), "the stale writer LOSES the CAS (rows_affected == 0)");
        match &bout {
            CasOutcome::Conflict { current } => {
                assert_eq!(current.version, 2, "the loser is handed the CURRENT server version");
                assert_eq!(current.inline, "A's edit", "the loser sees the WINNER's content to reconcile");
            }
            other => panic!("expected Conflict, got {other:?}"),
        }
        // THE 0-SILENT-OVERWRITE INVARIANT: the store STILL holds A's content, NOT B's.
        assert_eq!(
            s.get(&b).unwrap().inline,
            "A's edit",
            "B's edit was REJECTED, not silently applied — 0 silent overwrites"
        );
        assert_eq!(s.get(&b).unwrap().version, 2, "the version did NOT advance on the conflict");
    }

    /// **After a conflict, the loser RECONCILES by re-reading and re-applying at the current version
    /// — and then commits (the floor's whole contract: reject → reconcile → commit).**
    #[test]
    fn loser_reconciles_at_current_version_and_commits() {
        let (mut s, b) = store_with_block();
        s.edit_block(&b, 1, "A's edit", "{}").unwrap(); // → v2
        let conflict = s.edit_block(&b, 1, "B's edit", "{}").unwrap();
        let current = match conflict {
            CasOutcome::Conflict { current } => current,
            other => panic!("expected conflict, got {other:?}"),
        };
        // B reconciles: re-bases its edit on the current state + version, re-submits.
        let reconciled = s.edit_block(&b, current.version, "A's edit + B's reconciled edit", "{}").unwrap();
        assert!(reconciled.committed(), "the reconciled edit at the current version now commits");
        assert_eq!(reconciled.state().version, 3, "the reconciled commit bumps to v3");
    }

    // ── per-block independence: different blocks no false conflict ────────────────────────────────

    /// **PER-BLOCK INDEPENDENCE: two writers editing DIFFERENT blocks NEVER conflict (§3.2 — the guard
    /// is per-block).** Both commit; neither sees the other's version.
    #[test]
    fn different_blocks_edit_in_parallel_no_false_conflict() {
        let mut s = CasStore::new();
        let (b1, b2) = (bid("b1"), bid("b2"));
        s.insert_block(b1.clone(), "one", "{}").unwrap();
        s.insert_block(b2.clone(), "two", "{}").unwrap();
        // Writer X edits b1 at v1; Writer Y edits b2 at v1 — different blocks, both at v1.
        let x = s.edit_block(&b1, 1, "one edited", "{}").unwrap();
        let y = s.edit_block(&b2, 1, "two edited", "{}").unwrap();
        assert!(x.committed(), "b1's edit commits");
        assert!(y.committed(), "b2's edit commits — NO false conflict with b1");
        assert_eq!(s.get(&b1).unwrap().inline, "one edited");
        assert_eq!(s.get(&b2).unwrap().inline, "two edited");
        // 0 conflicts recorded — both were independent commits.
        assert_eq!(s.meter().conflicted(), 0, "different-block edits produce 0 false conflicts");
        assert_eq!(s.meter().committed(), 2, "both committed");
    }

    /// An edit before the content row exists is a LOUD error (not a silent create).
    #[test]
    fn edit_before_insert_errors_loudly() {
        let mut s = CasStore::new();
        assert_eq!(
            s.edit_block(&bid("ghost"), 1, "x", "{}").unwrap_err(),
            CasError::NoSuchBlock(bid("ghost"))
        );
    }

    /// A duplicate content insert is refused (content inserted once per id).
    #[test]
    fn duplicate_content_insert_refused() {
        let (mut s, b) = store_with_block();
        assert_eq!(s.insert_block(b.clone(), "x", "{}").unwrap_err(), CasError::DuplicateBlock(b));
    }

    // ── the soft-lock advisory ───────────────────────────────────────────────────────────────────

    /// **The advisory soft-lock: first acquire wins; a second client sees the holder but is NOT
    /// blocked (advisory — the CAS guard is the real safety).**
    #[test]
    fn soft_lock_is_advisory_not_mandatory() {
        let mut locks = SoftLockTable::new();
        let b = bid("b1");
        assert_eq!(locks.acquire(&b, "client-A"), SoftLock::Acquired, "A takes the advisory lock");
        assert_eq!(locks.holder(&b), Some(&"client-A".to_string()), "A is the advisory holder");
        // B sees A is editing — but is NOT prevented from editing (advisory).
        assert_eq!(
            locks.acquire(&b, "client-B"),
            SoftLock::Held { by: "client-A".into() },
            "B sees A editing (the UX courtesy) but is not blocked"
        );
        // A re-acquiring its own lock is idempotent.
        assert_eq!(locks.acquire(&b, "client-A"), SoftLock::Acquired);
        // A releases; B can now take it.
        locks.release(&b, "client-A");
        assert_eq!(locks.acquire(&b, "client-B"), SoftLock::Acquired, "after release B acquires");
        // B cannot release A-held… (B holds it now). A trying to release B's lock is a no-op.
        locks.release(&b, "client-A");
        assert_eq!(locks.holder(&b), Some(&"client-B".to_string()), "A cannot release B's lock");
    }

    /// **A soft-lock does NOT change the CAS outcome — even WITHOUT a lock, the CAS guard is the real
    /// safety (the soft-lock only reduces the conflict RATE, it is not the mechanism).**
    #[test]
    fn soft_lock_does_not_gate_the_cas_write() {
        let (mut s, b) = store_with_block();
        // No soft-lock at all: the CAS still protects against a silent overwrite.
        s.edit_block(&b, 1, "A", "{}").unwrap();
        let conflict = s.edit_block(&b, 1, "B", "{}").unwrap();
        assert!(conflict.is_conflict(), "the CAS guard protects regardless of any soft-lock");
    }

    // ── snapshot / restore ───────────────────────────────────────────────────────────────────────

    /// **Snapshot/restore layered on the CAS guard: a restore goes THROUGH the CAS (a restore racing
    /// a live edit conflicts, never silently clobbers).**
    #[test]
    fn snapshot_restore_through_the_cas_guard() {
        let (mut s, b) = store_with_block(); // v1, "hello"
        let snap = s.snapshot_block(&b).unwrap();
        assert_eq!(snap.inline, "hello");
        // Edit forward a couple of revisions.
        s.edit_block(&b, 1, "edited once", "{}").unwrap(); // v2
        s.edit_block(&b, 2, "edited twice", "{}").unwrap(); // v3
        // Restore at the CURRENT version (v3) → commits, content reverts to the snapshot, version → v4.
        let restored = s.restore_block(&b, 3, &snap).unwrap();
        assert!(restored.committed(), "a restore at the current version commits");
        assert_eq!(restored.state().inline, "hello", "the content reverted to the snapshot");
        assert_eq!(restored.state().version, 4, "the restore is a new revision (v4), not a counter rewind");

        // A restore at a STALE version (someone edited since) conflicts — never a silent clobber.
        s.edit_block(&b, 4, "live edit after restore", "{}").unwrap(); // v5
        let stale_restore = s.restore_block(&b, 4, &snap).unwrap();
        assert!(stale_restore.is_conflict(), "a restore racing a live edit conflicts, never clobbers");
        assert_eq!(
            s.get(&b).unwrap().inline,
            "live edit after restore",
            "the live edit survived the stale restore (0 silent overwrite)"
        );
    }

    // ── the offline queued-edit reconcile ────────────────────────────────────────────────────────

    /// **Offline = read + queued light-edit reconciled via the CAS floor: a queued edit whose base
    /// version still holds COMMITS on reconnect.**
    #[test]
    fn offline_queued_edit_commits_when_base_holds() {
        let (mut s, b) = store_with_block(); // v1
        let mut q = OfflineQueue::new();
        // The client went offline at v1, made a light-edit.
        q.queue(b.clone(), 1, "offline edit", "{}");
        assert_eq!(q.len(), 1);
        // Reconnect: no one else edited → the queued edit commits.
        let results = q.reconcile(&mut s);
        assert_eq!(results.len(), 1);
        let r = results[0].as_ref().unwrap();
        assert!(r.outcome.committed(), "the offline edit committed (base version still held)");
        assert_eq!(s.get(&b).unwrap().inline, "offline edit");
        assert!(q.is_empty(), "the queue drained on reconcile");
    }

    /// **A STALE offline edit (someone edited the block while the client was offline) CONFLICTS on
    /// reconnect — the offline editor reconciles, NEVER a silent overwrite (the named offline floor).**
    #[test]
    fn stale_offline_edit_conflicts_on_reconnect() {
        let (mut s, b) = store_with_block(); // v1
        let mut q = OfflineQueue::new();
        // The client went offline at v1.
        q.queue(b.clone(), 1, "offline edit", "{}");
        // Meanwhile ANOTHER client edited the block online → v2.
        s.edit_block(&b, 1, "online edit while peer offline", "{}").unwrap();
        // Reconnect: the offline edit's base (v1) is stale → conflict, not a silent clobber.
        let results = q.reconcile(&mut s);
        let r = results[0].as_ref().unwrap();
        assert!(r.outcome.is_conflict(), "the stale offline edit conflicts (reconcile, not overwrite)");
        assert_eq!(
            s.get(&b).unwrap().inline,
            "online edit while peer offline",
            "the online edit survived — the offline edit did NOT silently overwrite it"
        );
    }

    /// An offline edit to a block deleted while offline surfaces its error LOUDLY (not a silent drop).
    #[test]
    fn offline_edit_to_missing_block_errors_loudly() {
        let mut s = CasStore::new();
        let mut q = OfflineQueue::new();
        q.queue(bid("gone"), 1, "x", "{}");
        let results = q.reconcile(&mut s);
        assert_eq!(results[0].as_ref().unwrap_err(), &CasError::NoSuchBlock(bid("gone")));
    }

    // ── the conflict-rate metric (the CRDT-promotion trigger) ─────────────────────────────────────

    /// **The CAS-conflict-rate metric is emitted (the CRDT-promotion trigger, §3.4 / KQ-1).** A fresh
    /// doc has rate 0.0 (no divide-by-zero); commits + conflicts produce the `rows_affected == 0`
    /// fraction; the telemetry sample carries the canonical metric name.
    #[test]
    fn conflict_rate_metric_is_emitted() {
        let (mut s, b) = store_with_block();
        assert_eq!(s.meter().conflict_rate(), 0.0, "a fresh doc has 0 conflict rate (no divide-by-zero)");
        // 3 commits (A, then reconciled B twice) and 1 conflict.
        s.edit_block(&b, 1, "a", "{}").unwrap(); // commit → v2
        s.edit_block(&b, 1, "stale", "{}").unwrap(); // CONFLICT (stale v1)
        s.edit_block(&b, 2, "b", "{}").unwrap(); // commit → v3
        assert_eq!(s.meter().committed(), 2);
        assert_eq!(s.meter().conflicted(), 1);
        assert_eq!(s.meter().attempts(), 3);
        // rate = 1 / 3.
        assert!((s.meter().conflict_rate() - (1.0 / 3.0)).abs() < 1e-9, "the conflict rate is 1/3");
        let (name, rate) = s.meter().telemetry_sample();
        assert_eq!(name, "knowledge.cas_conflict_rate", "the canonical CRDT-promotion-trigger metric name");
        assert!((rate - (1.0 / 3.0)).abs() < 1e-9);
    }

    /// **The simultaneous-presence trigger (§3.4 — the second promotion signal): a block with 2+
    /// present authors is contended.**
    #[test]
    fn simultaneous_presence_marks_a_block_contended() {
        let mut p = SimultaneousPresence::new();
        let b = bid("b1");
        p.enter(&b, "A");
        assert!(!p.is_contended(&b), "one author present is not contended");
        p.enter(&b, "B");
        assert!(p.is_contended(&b), "two simultaneous authors → contended (the CRDT-promotion signal)");
        p.leave(&b, "A");
        assert!(!p.is_contended(&b), "back to one author → no longer contended");
    }

    /// **The lowered CAS SQL has the `WHERE version = expected_version` optimistic guard + the
    /// single-row PK probe (the visible query-plan artifact).**
    #[test]
    fn cas_sql_carries_the_optimistic_guard() {
        let sql = cas_update_sql();
        assert!(sql.contains("version = version + 1"), "the version is bumped: {sql}");
        assert!(sql.contains("WHERE tenant = $1 AND block_id = $2 AND version = $3"), "the CAS guard: {sql}");
        // a single-row PK probe (tenant, block_id) + the version precondition — never a scan.
        assert!(!sql.contains("WHERE TRUE"), "the write is a bounded single-row CAS, never unguarded");
    }
}
