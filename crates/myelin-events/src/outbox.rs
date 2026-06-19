//! # The `outbox` table + the same-transaction co-commit (contract 2.3, the provider half)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/00-platform-substrate.md` §3.3 (the
//! outbox relay — claims unsent rows with `FOR UPDATE SKIP LOCKED`, stamps the stable
//! `event_id` for broker-side dedup, publishes, marks sent, dead-letters after bounded
//! retries; the relay is the ONLY component on the broker publish side) and §2.10 (the
//! units the row carries).
//!
//! **Contract-index:** row 2.3 — `outbox` table `(event_id UNIQUE, aggregate, seq, subject,
//! envelope)`, `UNIQUE(aggregate, seq)` per-aggregate ordering. **P-S07 → global P-008.**
//!
//! ## Status (P-012 / EB-03, 2026-06-19) — per-aggregate ordering CORRECTNESS, reconciled in place
//! EB-03 ("The transactional outbox table + the `OutboxTx::emit` same-tx API, per-aggregate
//! ordering correctness") is the **event-bus ledger's framing of the SAME deliverable the
//! substrate roadmap already shipped** (P-S07/P-008 here, P-S06/P-006 for the emit causality):
//! the global run order interleaves the two roadmaps, so the outbox + emit surface is reached
//! from both. Per the coherence rule (EI-01 §7: never define a type twice, never build a
//! parallel second implementation), EB-03 **reconciles in place** — the frozen 2.3 table shape
//! ([`OUTBOX_MIGRATION`]), the 2.2 [`OutboxTx`] emit surface, the same-tx co-commit, and the
//! emit-iff-committed structure are UNCHANGED. What EB-03 ADDS / HARDENS:
//! - **Per-aggregate `seq` is now allocated at COMMIT time, not emit time** (see
//!   [`OutboxTransaction::commit`]). The substrate version allocated `seq` from `next_seq` at
//!   emit time; an aborted transaction that had emitted would then have BURNED a seq value,
//!   leaving a **gap** in the surviving committed sequence. EB-03's gate requires the
//!   per-aggregate seq be "monotonic AND **gap-free** under concurrent emitters", and arch §3.2
//!   requires it "reflect **true commit order**". Both are satisfied exactly by allocating at
//!   commit under the store lock: an aborted transaction consumes no seq, and the order
//!   transactions reach `commit` (true commit order) is the order seqs are assigned. This is a
//!   correctness improvement to the shared mechanism, documented per EI-01 §1 — the seam shape
//!   (the row columns, the `OutboxTx` trait, the depth signal) does NOT change.
//! - **The EB-03 GATE artifact** `tests::eb03_per_aggregate_seq_is_monotonic_and_gap_free_under_concurrent_emitters`
//!   (N threads race one hot aggregate → the committed seqs are exactly the contiguous set
//!   `{0..N}`, no gap, no dup) + `tests::eb03_aborted_transaction_leaves_no_seq_gap`.
//! - **FLOOR named:** proving this ordering AT PRODUCTION QPS under a hot-ref / hot-channel
//!   burst (BUS-D9, contract 2.3's "production QPS" clause) is the **M5 follow-on EB-29**. EB-03
//!   ships the correctness construction; EB-29 proves it under measured load.
//!
//! ## What this module ships (P-S07, the silent-data-loss floor — SUB-D1 / BUS-D4)
//! - The `outbox` table **migration** ([`OUTBOX_MIGRATION`]) to the frozen 2.3 shape: the
//!   forward-only DDL the migration runner (P-S15) applies. Expressed as a frozen DDL string
//!   constant here (the runner does not exist yet); the **floor** is named below.
//! - The **same-transaction co-commit** ([`OutboxStore`] + [`OutboxTransaction`]): an emit
//!   buffers the derived [`EventEnvelope`] into the SAME [`OutboxTransaction`] as the caller's
//!   state change, and the row becomes durable **iff** that transaction commits. A dropped
//!   (un-committed / aborted) transaction publishes nothing — **emit-iff-committed** (BUS-D4),
//!   correct-by-construction: there is no path that durably writes an event whose state change
//!   did not commit, and none that commits state without its event.
//! - `OutboxTx::emit` (the frozen 2.2 trait from [`crate`]) is **implemented** here on
//!   [`OutboxTransaction`]: it mints the stable ULID ([`Ulid`] via the injected [`IdMinter`]),
//!   pulls the ambient [`EmitContext`] from the transaction handle, calls [`derive_envelope`],
//!   and buffers the row — all in the open transaction.
//! - The `outbox_depth` survival signal ([`OutboxStore::outbox_depth`]) + the dead-letter
//!   count ([`OutboxStore::dead_letter_count`]): the contract-1.8 signals the SUB-D1 / BUS-D4
//!   drills assert against (`outbox_depth → 0` after the relay drains; both counts `0` on the
//!   no-loss path). The relay that drains the depth lives in [`crate::relay`].
//!
//! ## DEVIATION / FLOOR — the in-memory store models the SQL table (EI-01 §1, write it down)
//! There is **no live OLTP database in M0** (the OLTP tier client + the real Postgres binding
//! is the Storage tier client, **P-007 / P-ST-01**, and the migration runner is **P-S15**).
//! So the *mechanism* this prompt owns — the same-transaction co-commit + the unsent-row
//! ledger + the depth signal — is modeled as an **in-memory transactional store**
//! ([`OutboxStore`]) whose semantics are byte-for-byte the 2.3 contract: a row is
//! `(event_id UNIQUE, aggregate, seq UNIQUE-per-aggregate, subject, envelope, published_at)`,
//! a `begin → emit/co-commit → commit` transaction is atomic (all-or-nothing), and an
//! uncommitted transaction writes nothing. The frozen DDL ([`OUTBOX_MIGRATION`]) is the shape
//! the real table takes when the runner applies it. **Floor:** the real `INSERT … RETURNING`
//! against the Storage pool inside the caller's DB transaction lands when the OLTP tier client
//! is wired (P-007 + the `serve` lifecycle P-S12); the relay's `FOR UPDATE SKIP LOCKED` claim
//! is modeled here as an atomic claim over the in-memory rows (same observable property: two
//! relay workers never double-claim a row). The seam shape (the `OutboxTx` trait, the row
//! columns, the depth signal) does NOT change when the binding lands.
//!
//! ## FLOOR — the single-region event log (roadmap §3 floor table)
//! This is a single-region event log on a general-purpose store. The **column-store seam**
//! (the high-volume time-series tier) is the **post-M5 follow-on** (added only when volume is
//! measured; the Bus's `BusTransport` trait, [`crate::relay::BusTransport`], IS that seam —
//! built now behind the trait, promoted later; named in EB-31 / the post-M5 substrate
//! follow-on). Named in writing here, never silently assumed done.
//!
//! ## FLOOR — the ULID source
//! A real ULID is `time-ordered-prefix + randomness`. M0 has no shared clock/RNG (the clock is
//! initialised inside the `serve` lifecycle, P-S12). So id minting is an injected [`IdMinter`]
//! seam: the deterministic [`MonotonicMinter`] is the test/floor source (monotonic, so the
//! `(aggregate, seq)` ordering + the stable-id dedup property are testable without wall-clock
//! flakiness); the real wall-clock+random ULID source is wired at P-S12 and implements the
//! SAME [`IdMinter`] trait — the relay/store do not change.

use crate::{
    derive_envelope, AggregateKey, EmitContext, EventDraft, EventEnvelope, EventId, OutboxError,
    OutboxTx, Result,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The frozen forward-only DDL for the `outbox` table (contract 2.3). This is the shape the
/// migration runner (P-S15) applies when the OLTP tier client (P-007) is wired; the in-memory
/// [`OutboxStore`] in this module models exactly these semantics until then. The columns +
/// constraints are the contract:
/// - `event_id` is the ULID idempotency key and is **UNIQUE** (broker-side dedup, ADR-04.1);
/// - `(aggregate, seq)` is **UNIQUE** — the per-aggregate ordering key (the relay drains a
///   given aggregate in `seq` order so per-ref / per-conversation ordering holds, D-9);
/// - `subject` + `envelope` carry the row's reference + the canonical [`EventEnvelope`] body
///   (references-not-payloads — the envelope's `payload` holds IDs/refs, never a PII body);
/// - `published_at` is NULL until the relay publishes the row (an unsent row is `NULL`; the
///   `outbox_depth` survival signal is the count of `published_at IS NULL` rows);
/// - `attempts` bounds the relay's retries before a dead-letter.
///
/// **Forward-only** (the `forward-only-migration` lint, P-S11): this is an `expand` migration
/// (it only adds the table); there is no destructive down-migration.
pub const OUTBOX_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS outbox (
    event_id     TEXT        NOT NULL,
    aggregate    TEXT        NOT NULL,
    seq          BIGINT      NOT NULL,
    subject      TEXT        NOT NULL,
    envelope     JSONB       NOT NULL,
    published_at TIMESTAMPTZ,
    attempts     INT         NOT NULL DEFAULT 0,
    CONSTRAINT outbox_event_id_unique UNIQUE (event_id),
    CONSTRAINT outbox_aggregate_seq_unique UNIQUE (aggregate, seq)
);
-- the relay claims unsent rows ordered (aggregate, seq) with FOR UPDATE SKIP LOCKED:
CREATE INDEX IF NOT EXISTS outbox_unsent_idx ON outbox (aggregate, seq) WHERE published_at IS NULL;";

/// A stable ULID — the `event_id` (the idempotency / broker-side-dedup key, ADR-04.1). A
/// distinct newtype from [`EventId`] at the minting boundary; `From<Ulid> for EventId` carries
/// it onto the envelope. "Stable" = the SAME row always carries the SAME id across every
/// re-claim / redelivery, which is exactly what makes broker-side dedup
/// (`Nats-Msg-Id = event_id`) suppress a duplicate publish (0 ghost).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Ulid(pub String);

impl From<Ulid> for EventId {
    fn from(u: Ulid) -> Self {
        EventId(u.0)
    }
}

/// The id-minting seam (the floor named in the module docs). The outbox mints the stable ULID
/// for each emitted event; the source is injected so the store/relay stay deterministically
/// testable before a shared clock exists (P-S12). A real wall-clock+random ULID source
/// implements this same trait at P-S12; the store does not change.
pub trait IdMinter: Send + Sync {
    /// Mint the next stable, monotonically-increasing ULID. Monotonic so a later emit sorts
    /// after an earlier one (ULID time-ordering) — the property the relay's `(aggregate, seq)`
    /// ordered drain and the audit walk rely on.
    fn mint(&self) -> Ulid;
}

/// A deterministic monotonic ULID minter (the test/floor source). Emits `01J-<n>` with a
/// zero-padded counter so lexical order == mint order (ULID time-ordering, without a
/// wall-clock). The real source (wall-clock + randomness) lands at P-S12 behind [`IdMinter`].
#[derive(Default)]
pub struct MonotonicMinter {
    next: AtomicU64,
}

impl MonotonicMinter {
    /// A fresh minter starting at 0.
    pub fn new() -> Self {
        Self::default()
    }
}

impl IdMinter for MonotonicMinter {
    fn mint(&self) -> Ulid {
        let n = self.next.fetch_add(1, Ordering::SeqCst);
        // Zero-pad to a fixed width so lexical order equals numeric order (time-ordering).
        Ulid(format!("01J{n:020}"))
    }
}

/// One durably-stored outbox row (contract 2.3). Mirrors the [`OUTBOX_MIGRATION`] columns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutboxRow {
    /// The stable ULID idempotency key (UNIQUE; the broker-side dedup id).
    pub event_id: EventId,
    /// The per-aggregate ordering key.
    pub aggregate: AggregateKey,
    /// The monotonically-increasing per-aggregate sequence (`UNIQUE(aggregate, seq)`).
    pub seq: u64,
    /// The row's subject ref (the envelope's `subject`, hoisted for the broker subject).
    pub subject: crate::ArtifactRef,
    /// The canonical event body.
    pub envelope: EventEnvelope,
    /// `None` until the relay publishes the row (the `outbox_depth` signal counts `None`s).
    pub published_at: Option<crate::Timestamp>,
    /// How many publish attempts the relay has made (bounded before a dead-letter).
    pub attempts: u32,
}

/// The shared inner state of an [`OutboxStore`] (behind an `Arc<Mutex<…>>` so the store is a
/// cloneable handle shared by the emitting transactions, the relay, and the depth reader).
/// `pub(crate)` (with `pub(crate)` fields) because the relay's claim/mark-sent/dead-letter
/// mechanics ([`crate::relay`]) operate on it directly (modeling the SQL row updates).
#[derive(Default)]
pub(crate) struct Inner {
    /// All committed rows, keyed by `event_id` (the UNIQUE constraint). Insertion-ordered via
    /// `order` so the relay drains deterministically.
    pub(crate) rows: HashMap<EventId, OutboxRow>,
    /// The order rows were committed (so the relay claims oldest-first, `(aggregate, seq)`).
    pub(crate) order: Vec<EventId>,
    /// The next committed `seq` per aggregate (models the `UNIQUE(aggregate, seq)` monotonic
    /// counter). **Allocated at COMMIT time, not emit time** (EB-03): a `seq` is consumed only
    /// when the transaction durably commits, so an aborted transaction leaves NO gap — the
    /// committed sequence is `0, 1, 2, …` gap-free, and it reflects TRUE COMMIT ORDER (arch
    /// §3.2), which is exactly what the per-aggregate ordering invariant requires.
    pub(crate) next_seq: HashMap<AggregateKey, u64>,
    /// Rows the relay gave up on (dead-lettered after bounded retries). Drained out of `rows`.
    pub(crate) dead_letters: Vec<OutboxRow>,
    /// event_ids the relay has currently CLAIMED (modeling `FOR UPDATE SKIP LOCKED`: a second
    /// relay worker SKIPs a claimed row). A claim is released on publish (the row leaves the
    /// unsent set) or on a failed attempt (so it can be retried).
    pub(crate) claimed: std::collections::HashSet<EventId>,
}

/// The transactional `outbox` store (contract 2.3 provider half) — the same-transaction
/// co-commit + the unsent-row ledger + the depth signal. A cloneable handle over shared state
/// so the emitting code, the relay, and a telemetry reader all observe one truth.
///
/// **The co-commit invariant (BUS-D4, emit-iff-committed):** a row enters `rows` ONLY through
/// [`OutboxTransaction::commit`]. An [`OutboxTransaction`] buffers its emitted rows and the
/// caller's state mutation together; if it is dropped without `commit` (an abort / a crash
/// between buffering and commit) NOTHING is written — so there is no event without its state,
/// and (because the caller's state mutation is in the same buffer) no committed state without
/// its event. This is the silent-data-loss floor, correct-by-construction.
#[derive(Clone, Default)]
pub struct OutboxStore {
    inner: Arc<Mutex<Inner>>,
}

impl OutboxStore {
    /// A fresh, empty store (depth 0, no dead-letters).
    pub fn new() -> Self {
        Self::default()
    }

    /// Begin a transaction. Everything emitted on the returned [`OutboxTransaction`] — together
    /// with the caller's own state mutation, registered via
    /// [`OutboxTransaction::stage_state_change`] — is durable iff [`OutboxTransaction::commit`]
    /// is called. The `minter` (an [`Arc`] so the same source is shared across transactions)
    /// supplies the stable ULID; `ctx_base` carries the ambient tenant/region/actor/clock the
    /// emit derives from.
    pub fn begin(&self, minter: Arc<dyn IdMinter>, ctx_base: EmitContextBase) -> OutboxTransaction {
        OutboxTransaction {
            store: self.clone(),
            minter,
            ctx_base,
            staged_rows: Vec::new(),
            state_committed: Arc::new(Mutex::new(None)),
        }
    }

    /// The `outbox_depth` survival signal (contract 1.8): the number of **unsent** rows
    /// (`published_at IS NULL`). The SUB-D1 drill asserts this `→ 0` once the relay drains.
    pub fn outbox_depth(&self) -> usize {
        let inner = self.lock();
        inner
            .order
            .iter()
            .filter(|id| {
                inner
                    .rows
                    .get(*id)
                    .is_some_and(|r| r.published_at.is_none())
            })
            .count()
    }

    /// The dead-letter count survival signal (contract 1.8): rows the relay gave up on after
    /// bounded retries. The no-loss path asserts this `== 0`.
    pub fn dead_letter_count(&self) -> usize {
        self.lock().dead_letters.len()
    }

    /// The **`recorded_at` of the oldest still-unsent row** — the input to the contract-1.8
    /// *outbox age* survival signal (§4.11 "outbox depth **+ age**"). Depth alone says "how
    /// many are stuck"; age says "how LONG the oldest has been stuck" — a relay that is keeping
    /// up holds a tiny depth AND a near-now age, while a wedged relay shows age climbing even at
    /// constant depth. Returns `None` when the outbox is fully drained (no unsent row → no age).
    ///
    /// Rows are committed oldest-first into `order`, so the first unsent row in `order` is the
    /// oldest unsent one; its envelope's `recorded_at` (RFC-3339 UTC, the frozen unit §2.10) is
    /// the age anchor. The age-in-seconds is computed against a caller-supplied `now` in
    /// [`crate::telemetry`] (M0 has no shared wall-clock until `serve`, P-S12 — named floor).
    pub fn oldest_unsent_recorded_at(&self) -> Option<crate::Timestamp> {
        let inner = self.lock();
        inner.order.iter().find_map(|id| {
            inner
                .rows
                .get(id)
                .filter(|r| r.published_at.is_none())
                .map(|r| r.envelope.recorded_at.clone())
        })
    }

    /// The total committed-row count (sent + unsent), for the no-ghost assertion (every
    /// committed event is delivered exactly once; an aborted transaction adds nothing here).
    pub fn committed_count(&self) -> usize {
        self.lock().order.len()
    }

    /// Read a row by id (for tests / the relay's re-hydration of the envelope to publish).
    pub fn row(&self, id: &EventId) -> Option<OutboxRow> {
        self.lock().rows.get(id).cloned()
    }

    /// Snapshot the dead-lettered rows (for the operator alert / a test).
    pub fn dead_letters(&self) -> Vec<OutboxRow> {
        self.lock().dead_letters.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Crate-internal lock accessor so the relay mechanics ([`crate::relay`]) can drive the
    /// claim/mark-sent/dead-letter row updates against the shared state.
    pub(crate) fn lock_inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The ambient fields an [`OutboxTransaction`] supplies to every emit on it (the per-actor /
/// per-clock context, minus the per-event minted id which the outbox mints). The
/// [`EmitContext`] the pure [`derive_envelope`] reads is assembled from this base + a freshly
/// minted [`Ulid`] for each emit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmitContextBase {
    /// The partition + residency key (ADR-11) — first-class, never optional.
    pub tenant: crate::TenantId,
    pub region: crate::Region,
    /// The acting principal incl. on_behalf_of (ADR-13.3).
    pub actor: crate::Actor,
    /// schema version of the emitted type (forward-only; upcasters bridge at consume).
    pub schema_ver: u32,
    /// RFC-3339 UTC; when the action happened.
    pub occurred_at: crate::Timestamp,
    /// RFC-3339 UTC; when the log durably accepted it.
    pub recorded_at: crate::Timestamp,
    /// The distinct human-action / session ref (BUS-5).
    pub caused_by: Option<crate::CausedBy>,
}

/// An open outbox transaction — the same-transaction co-commit handle. Implements the frozen
/// [`OutboxTx`] trait: `emit(draft, cause)` derives causality correct-by-construction (via the
/// pure [`derive_envelope`]), mints the stable ULID, assigns the per-aggregate `seq`, and
/// **buffers** the row. The buffered rows + the staged state change become durable together on
/// [`commit`](Self::commit), and **not at all** if the transaction is dropped without it
/// (emit-iff-committed, BUS-D4).
pub struct OutboxTransaction {
    store: OutboxStore,
    minter: Arc<dyn IdMinter>,
    ctx_base: EmitContextBase,
    /// The rows emitted-but-not-yet-committed (the buffer the co-commit makes durable atomically).
    staged_rows: Vec<OutboxRow>,
    /// The caller's staged state change (a description the test can read), proving the event +
    /// the state mutation share the transaction. `None` if the caller staged none.
    state_committed: Arc<Mutex<Option<String>>>,
}

impl OutboxTransaction {
    /// Stage the caller's own state mutation into THIS transaction (the "state change" half of
    /// the co-commit). In a real service this is the row the handler writes to its own table in
    /// the same DB transaction the outbox row is inserted into; here it is recorded so a test
    /// can assert the state and the event commit together (and that an abort writes neither).
    pub fn stage_state_change(&mut self, change: impl Into<String>) {
        *self.state_committed.lock().unwrap_or_else(|e| e.into_inner()) = Some(change.into());
    }

    /// Commit the transaction: every staged outbox row + the staged state change become durable
    /// **atomically**. After this, the rows are visible to the relay and counted in
    /// `outbox_depth`.
    ///
    /// **The per-aggregate `seq` is assigned HERE, at commit time, under the store lock**
    /// (EB-03, per-aggregate ordering correctness). This is what makes the committed sequence
    /// for each aggregate `0, 1, 2, …` **gap-free AND in true commit order** (arch §3.2):
    /// - because the seq is consumed only when the transaction durably commits, an **aborted**
    ///   transaction that emitted (and was dropped) consumes NO seq → **no gap**;
    /// - because the whole commit (read-the-counter → assign → bump) runs while this thread
    ///   holds the single store lock, two **concurrent** transactions committing to the same
    ///   aggregate are serialized → distinct, monotonic, contiguous seqs → **no dup, no gap**;
    /// - the order in which transactions reach `commit` (true commit order) is the order in
    ///   which their seqs are assigned, so outbox order == state-change-commit order, which is
    ///   the source-of-truth ordering the relay drains.
    ///
    /// In the real OLTP binding (P-007) this is the `INSERT … RETURNING` whose `seq` is
    /// `COALESCE(MAX(seq)+1, 0)` for the aggregate inside the caller's transaction, protected by
    /// the `UNIQUE(aggregate, seq)` constraint (a racing commit retries) — same observable
    /// property. The in-memory store models exactly that under the store lock.
    pub fn commit(mut self) -> Result<()> {
        let mut inner = self.store.lock();
        // First pass: every staged row's event_id must be unique (the UNIQUE(event_id)
        // constraint) — reject the WHOLE commit loudly before mutating anything (atomicity: a
        // partial commit would be silent data loss). The minter is monotonic so this cannot
        // happen on the happy path; a collision is a programming error.
        for row in &self.staged_rows {
            if inner.rows.contains_key(&row.event_id) {
                return Err(OutboxError(format!(
                    "outbox UNIQUE(event_id) violation on {:?} — duplicate emit",
                    row.event_id
                )));
            }
        }
        // Second pass: assign the per-aggregate commit-order seq and durably insert. Staged
        // rows keep the emit order, so within one transaction the seqs are assigned in emit
        // order; across transactions they are assigned in commit order (this lock).
        for mut row in self.staged_rows.drain(..) {
            let slot = inner.next_seq.entry(row.aggregate.clone()).or_insert(0);
            row.seq = *slot;
            *slot += 1;
            inner.order.push(row.event_id.clone());
            inner.rows.insert(row.event_id.clone(), row);
        }
        Ok(())
    }

    /// Whether the caller staged a state change (for the co-commit assertion in tests).
    pub fn staged_state(&self) -> Option<String> {
        self.state_committed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// The number of rows currently buffered (emitted but not yet committed).
    pub fn staged_len(&self) -> usize {
        self.staged_rows.len()
    }
}

// **Emit-iff-committed (BUS-D4) is structural, not a Drop hook.** There is deliberately NO
// `Drop` impl that flushes buffered rows: [`OutboxTransaction::commit`] is the ONLY code path
// that writes a row into the [`OutboxStore`]. So an un-committed transaction (an abort, or a
// crash between buffering and commit) writes NOTHING — its `staged_rows` are simply dropped
// with the value, the store untouched, and the staged state change never reaches durable
// storage either. There is no event without its state, and (because the caller's state change
// is staged in the SAME transaction) no committed state without its event. The
// `dropped_transaction_emits_nothing_emit_iff_committed` test pins this.

impl OutboxTx for OutboxTransaction {
    fn emit(&mut self, draft: EventDraft, cause: Option<&EventEnvelope>) -> Result<EventId> {
        // Mint the stable ULID for this event (the broker-side dedup id).
        let id: EventId = self.minter.mint().into();
        // The per-aggregate `seq` is NOT allocated here — it is assigned at COMMIT time (see
        // `OutboxTransaction::commit`) so an aborted transaction consumes no seq and the
        // committed sequence stays gap-free + in true commit order (EB-03). The staged row
        // carries a placeholder `seq` that `commit` overwrites under the store lock.
        let aggregate = draft.aggregate.clone();
        let subject = draft.subject.clone();
        // Build the ambient context for the pure derivation (id + the transaction's base).
        let ctx = EmitContext {
            event_id: id.clone(),
            tenant: self.ctx_base.tenant.clone(),
            region: self.ctx_base.region.clone(),
            actor: self.ctx_base.actor.clone(),
            schema_ver: self.ctx_base.schema_ver,
            occurred_at: self.ctx_base.occurred_at.clone(),
            recorded_at: self.ctx_base.recorded_at.clone(),
            caused_by: self.ctx_base.caused_by.clone(),
        };
        // Causality correct-by-construction (P-S06): root carries its own correlation; a caused
        // event inherits parent provenance + depth+1.
        let envelope = derive_envelope(draft, ctx, cause);
        // BUFFER the row (not committed yet — emit-iff-committed). It becomes durable on commit.
        self.staged_rows.push(OutboxRow {
            event_id: id.clone(),
            aggregate,
            // Placeholder; the real per-aggregate commit-order seq is stamped by `commit`.
            seq: 0,
            subject,
            envelope,
            published_at: None,
            attempts: 0,
        });
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Actor, ArtifactRef, CausedBy, DataRole, EventType, Region, TenantId, Timestamp,
        Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn principal() -> Principal {
        Principal::stub(PrincipalId("p".into()), PrincipalKind::Human, TenantId("acme".into()))
    }

    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: TenantId("acme".into()),
            region: Region("eu-west".into()),
            actor: Actor(principal()),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-19T00:00:01Z".into()),
            caused_by: Some(CausedBy("session:abc".into())),
        }
    }

    fn draft(type_: &str, aggregate: &str) -> EventDraft {
        EventDraft {
            type_: EventType(type_.into()),
            subject: ArtifactRef(format!("myelin://acme/issues/issue/{aggregate}")),
            aggregate: AggregateKey(aggregate.into()),
            payload: serde_json::json!({ "ref": aggregate }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        }
    }

    fn store_and_minter() -> (OutboxStore, Arc<dyn IdMinter>) {
        (
            OutboxStore::new(),
            Arc::new(MonotonicMinter::new()) as Arc<dyn IdMinter>,
        )
    }

    /// The migration is the frozen 2.3 shape: the UNIQUE(event_id) + UNIQUE(aggregate, seq)
    /// constraints + the columns are present (a drift fails this).
    #[test]
    fn migration_is_the_frozen_2_3_shape() {
        assert!(OUTBOX_MIGRATION.contains("CREATE TABLE IF NOT EXISTS outbox"));
        assert!(OUTBOX_MIGRATION.contains("UNIQUE (event_id)"));
        assert!(OUTBOX_MIGRATION.contains("UNIQUE (aggregate, seq)"));
        for col in ["event_id", "aggregate", "seq", "subject", "envelope", "published_at"] {
            assert!(OUTBOX_MIGRATION.contains(col), "migration is missing column {col}");
        }
        // forward-only: no destructive DROP on the down path (there is no down path).
        assert!(!OUTBOX_MIGRATION.contains("DROP TABLE"));
    }

    /// A committed transaction makes its staged event + state change durable together — the
    /// co-commit happy path. After commit, `outbox_depth` reflects exactly the events emitted.
    #[test]
    fn commit_makes_event_and_state_durable_together() {
        let (store, minter) = store_and_minter();
        let mut tx = store.begin(minter, ctx_base());
        tx.stage_state_change("issue PROJ-1 created");
        let id = tx.emit(draft("issues.issue.created", "issue:PROJ-1"), None).unwrap();
        assert_eq!(tx.staged_len(), 1, "one event buffered");
        let id2 = tx.emit(draft("issues.issue.updated", "issue:PROJ-1"), None).unwrap();
        // before commit: nothing durable (emit-iff-committed) — depth still 0.
        assert_eq!(store.outbox_depth(), 0, "an open transaction has written nothing");
        assert_eq!(tx.staged_len(), 2, "two events buffered (not a constant)");
        assert_eq!(tx.staged_state().as_deref(), Some("issue PROJ-1 created"));

        tx.commit().unwrap();
        // after commit: both event rows are durable + unsent (depth 2).
        assert_eq!(store.outbox_depth(), 2);
        assert_eq!(store.committed_count(), 2);
        let row = store.row(&id).expect("committed row is present");
        assert_eq!(row.seq, 0, "first event for the aggregate is seq 0");
        assert!(row.published_at.is_none(), "a freshly committed row is unsent");
        assert_eq!(store.row(&id2).unwrap().seq, 1, "second event is seq 1");
    }

    /// **BUS-D4 (emit-iff-committed), the structural half.** A transaction DROPPED without
    /// commit (an abort / a crash between state-commit and publish) writes NOTHING: no outbox
    /// row, no ghost event. There is no event without its state.
    #[test]
    fn dropped_transaction_emits_nothing_emit_iff_committed() {
        let (store, minter) = store_and_minter();
        {
            let mut tx = store.begin(minter, ctx_base());
            tx.stage_state_change("issue PROJ-9 created");
            tx.emit(draft("issues.issue.created", "issue:PROJ-9"), None).unwrap();
            assert_eq!(tx.staged_len(), 1, "buffered, not committed");
            // tx dropped here WITHOUT commit (the crash point).
        }
        // emit-iff-committed: the aborted transaction published nothing.
        assert_eq!(store.outbox_depth(), 0, "an aborted transaction writes no event");
        assert_eq!(store.committed_count(), 0, "no ghost row from an abort");
        assert_eq!(store.dead_letter_count(), 0);
    }

    /// `emit` derives causality through the trait (a caused event sets depth = parent+1 and
    /// carries the root) AND, on commit, the rows carry a monotonic per-aggregate seq. Proves
    /// the trait impl wires `derive_envelope` and the commit-time ordering key together.
    #[test]
    fn emit_derives_causality_and_assigns_monotonic_seq_per_aggregate() {
        let (store, minter) = store_and_minter();
        let mut tx = store.begin(minter, ctx_base());

        let root_id = tx.emit(draft("issues.issue.created", "issue:PROJ-1"), None).unwrap();
        let root_env = store_envelope(&tx, 0);
        assert_eq!(root_env.depth, 0);
        assert_eq!(root_env.correlation_id.0, root_id.0, "root carries its own correlation");

        let child_id = tx
            .emit(draft("refs.edge.created", "issue:PROJ-1"), Some(&root_env))
            .unwrap();
        let child_env = store_envelope(&tx, 1);
        assert_eq!(child_env.depth, 1, "caused event is depth parent+1");
        assert_eq!(child_env.causation_id, Some(root_id.clone()));
        assert_ne!(root_id, child_id);

        // The seq is assigned at COMMIT (gap-free, true-commit-order). After commit the durable
        // rows for the same aggregate carry monotonic seqs 0, 1 in emit order.
        tx.commit().unwrap();
        assert_eq!(store.row(&root_id).unwrap().seq, 0);
        assert_eq!(store.row(&child_id).unwrap().seq, 1);
        assert_eq!(store.row(&root_id).unwrap().aggregate, store.row(&child_id).unwrap().aggregate);
    }

    fn store_envelope(tx: &OutboxTransaction, i: usize) -> EventEnvelope {
        tx.staged_rows[i].envelope.clone()
    }

    /// Distinct aggregates get independent seq counters (each starts at 0) — the per-aggregate
    /// ordering is per-aggregate, not global. Asserted on the committed rows (seq is a
    /// commit-time property).
    #[test]
    fn seq_is_independent_per_aggregate() {
        let (store, minter) = store_and_minter();
        let mut tx = store.begin(minter, ctx_base());
        let a0 = tx.emit(draft("issues.issue.created", "issue:A"), None).unwrap();
        let b0 = tx.emit(draft("issues.issue.created", "issue:B"), None).unwrap();
        let a1 = tx.emit(draft("issues.issue.updated", "issue:A"), None).unwrap();
        tx.commit().unwrap();
        // A: 0, 1 ; B: 0
        assert_eq!(store.row(&a0).unwrap().seq, 0); // A
        assert_eq!(store.row(&b0).unwrap().seq, 0); // B
        assert_eq!(store.row(&a1).unwrap().seq, 1); // A again
    }

    /// **EB-03 GATE — per-aggregate seq is monotonic + gap-free under CONCURRENT emitters to
    /// the SAME aggregate (no gaps, no dups).** This is the per-aggregate ordering CORRECTNESS
    /// the prompt ships (proving it AT PRODUCTION QPS under a hot-ref/hot-channel burst, BUS-D9,
    /// is the M5 follow-on EB-29). N threads each open a transaction, emit one event to the one
    /// shared hot aggregate, and commit; the committed seqs MUST be exactly the contiguous set
    /// {0, 1, …, N-1} — every value present once (no gap), none repeated (no dup) — because the
    /// commit-time allocation under the store lock serializes the racing commits.
    #[test]
    fn eb03_per_aggregate_seq_is_monotonic_and_gap_free_under_concurrent_emitters() {
        use std::sync::Arc as StdArc;
        let store = OutboxStore::new();
        let minter: StdArc<dyn IdMinter> = StdArc::new(MonotonicMinter::new());
        const N: u64 = 64;
        let hot = "issue:HOT"; // the one hot aggregate every thread races on.

        let mut handles = Vec::new();
        for _ in 0..N {
            let store = store.clone();
            let minter = StdArc::clone(&minter);
            handles.push(std::thread::spawn(move || {
                let mut tx = store.begin(minter, ctx_base());
                let id = tx.emit(draft("issues.issue.updated", hot), None).unwrap();
                tx.commit().unwrap();
                id
            }));
        }
        let ids: Vec<EventId> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        // Collect the committed seqs for the hot aggregate.
        let mut seqs: Vec<u64> = ids.iter().map(|id| store.row(id).unwrap().seq).collect();
        seqs.sort_unstable();
        // Gap-free + no-dup: exactly the contiguous set {0, 1, …, N-1}.
        let expected: Vec<u64> = (0..N).collect();
        assert_eq!(seqs, expected, "concurrent emitters must yield contiguous, unique seqs");
        assert_eq!(store.committed_count(), N as usize, "every committed event is present once");
    }

    /// **EB-03 — an ABORTED transaction consumes NO seq → the committed sequence stays gap-free.**
    /// This is why the seq is allocated at COMMIT, not at emit: a transaction that emits to an
    /// aggregate and is then dropped (abort / crash) must NOT burn a seq value, or the surviving
    /// committed sequence would have a hole. Commit A (seq 0), abort B, commit C (must be seq 1,
    /// not seq 2).
    #[test]
    fn eb03_aborted_transaction_leaves_no_seq_gap() {
        let (store, minter) = store_and_minter();
        let agg = "issue:GAPCHECK";

        // A commits → seq 0.
        let mut ta = store.begin(Arc::clone(&minter), ctx_base());
        let a = ta.emit(draft("issues.issue.created", agg), None).unwrap();
        ta.commit().unwrap();
        assert_eq!(store.row(&a).unwrap().seq, 0);

        // B emits to the SAME aggregate but is dropped WITHOUT commit (abort).
        {
            let mut tb = store.begin(Arc::clone(&minter), ctx_base());
            tb.emit(draft("issues.issue.updated", agg), None).unwrap();
            // tb dropped here — no commit, no seq consumed.
        }

        // C commits → must be seq 1 (the abort left no gap), not seq 2.
        let mut tc = store.begin(Arc::clone(&minter), ctx_base());
        let c = tc.emit(draft("issues.issue.updated", agg), None).unwrap();
        tc.commit().unwrap();
        assert_eq!(store.row(&c).unwrap().seq, 1, "abort must not burn a seq → gap-free");
        assert_eq!(store.committed_count(), 2, "only the two committed events exist");
    }

    /// The minted id is a stable ULID stamped onto the envelope (the broker-side dedup key).
    /// Monotonic minting → lexical order == mint order (ULID time-ordering).
    #[test]
    fn minted_ids_are_stable_and_monotonic() {
        let minter = MonotonicMinter::new();
        let a = minter.mint();
        let b = minter.mint();
        assert_ne!(a, b);
        assert!(a < b, "ULIDs are monotonic (time-ordered): {a:?} < {b:?}");
        // From<Ulid> for EventId carries the id unchanged.
        let id: EventId = a.clone().into();
        assert_eq!(id.0, a.0);
    }
}
