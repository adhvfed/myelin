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

use crate::relay::{BusTransport, DrainReport};
use crate::{
    derive_envelope, AggregateKey, EmitContext, EventDraft, EventEnvelope, EventId, OutboxError,
    OutboxTx, Result,
};
// Used only by the `test-support`-gated memory arm (MR-009b W3b.6).
#[cfg(any(test, feature = "test-support"))]
use std::collections::HashMap;
use std::collections::HashSet;
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

/// Durable quarantine for outbox rows that cannot be safely decoded or routed.
///
/// The original row remains in `outbox` and stays unpublished. This table deliberately copies no
/// envelope, payload, subject, tenant, or actor data: operators get a stable bounded reason while
/// remediation retains the one authoritative raw row behind the foreign key.
pub const OUTBOX_QUARANTINE_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS outbox_quarantine (
    event_id         TEXT        PRIMARY KEY REFERENCES outbox(event_id) ON DELETE RESTRICT,
    aggregate        TEXT        NOT NULL,
    seq              BIGINT      NOT NULL,
    reason_code      TEXT        NOT NULL,
    reason_detail    TEXT        NOT NULL,
    quarantined_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    acknowledged_at TIMESTAMPTZ,
    CONSTRAINT outbox_quarantine_reason_code_bounded CHECK (
        reason_code ~ '^[a-z0-9_]{1,64}$'
    ),
    CONSTRAINT outbox_quarantine_reason_detail_bounded CHECK (
        octet_length(reason_detail) BETWEEN 1 AND 256
    )
);
CREATE INDEX IF NOT EXISTS outbox_quarantine_aggregate_seq_idx
    ON outbox_quarantine (aggregate, seq);";

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

/// The shared inner state of the **`test-support`-gated in-memory arm** of an [`OutboxStore`]
/// (behind an `Arc<Mutex<…>>` so the store is a cloneable handle shared by the emitting
/// transactions, the relay, and the depth reader). `pub(crate)` (with `pub(crate)` fields)
/// because the relay's memory-arm claim/mark-sent/dead-letter mechanics ([`crate::relay`])
/// operate on it directly (modeling the SQL row updates).
///
/// **MR-009b W3b.6 — TEST DOUBLE (compiled ONLY under `#[cfg(any(test, feature =
/// "test-support"))]`).** The production store holds NO in-memory collection: the always-compiled
/// backend is `Durable(Arc<dyn DurableOutboxBacking>)` over the real PG `outbox` table
/// (`myelin_storage::PgOutboxBacking`), so committed events survive a process restart (SI-007).
#[cfg(any(test, feature = "test-support"))]
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
#[derive(Clone)]
pub struct OutboxStore {
    backend: OutboxBackend,
}

/// The `Default` store is the in-memory TEST DOUBLE — `#[cfg(any(test, feature =
/// "test-support"))]` (MR-009b W3b.6). Production constructs the `Durable` backing through
/// [`OutboxStore::durable`] (the W3b.4 composition roots: provider-from-env → foundation
/// migrations → `PgOutboxBacking`, fail-loud on missing durable config).
#[cfg(any(test, feature = "test-support"))]
impl Default for OutboxStore {
    fn default() -> Self {
        OutboxStore {
            backend: OutboxBackend::default(),
        }
    }
}

/// **The durable backing seam for the `outbox` table + relay (SI-007, MR-009b W3b).** A real
/// `outbox` table over the OLTP pool implements this so the transactional co-commit + the
/// unsent-row ledger + the relay drain **survive a process restart** (a committed-but-unsent row
/// after a restart is still relayed because it lives in Postgres, not a per-process
/// `Arc<Mutex<Inner>>`). The verbs mirror the in-memory store's: the durable arm of
/// [`OutboxTransaction::commit`] ([`commit_staged`](DurableOutboxBacking::commit_staged) — all
/// staged rows in ONE atomic commit) + the contract-1.8 read signals + the SINGLE composite relay
/// verb [`drain_once`](DurableOutboxBacking::drain_once). The trait is SYNC to match the emit /
/// relay call sites; a PG impl bridges to async internally (`block_in_place` + `block_on`), the
/// same bridge [`crate::nats::NatsJetStreamBus`] uses.
///
/// **The co-commit FLOOR (named, not silently skipped):** the real emit-iff-committed guarantee
/// (BUS-D4) is the outbox row inserted in the SAME transaction as the caller's state write. This
/// seam's [`commit_staged`](DurableOutboxBacking::commit_staged) takes the staged rows and commits
/// them atomically; threading the caller's OWN state-write transaction INTO the same commit is the
/// per-subsystem emit re-point (W3b.2+ / `PgRelay::co_commit_in_tx`) beyond this role-struct step.
/// THIS seam delivers the durable ledger + restart-surviving relay; the in-caller's-tx co-commit
/// stays the documented floor. `drain_once` is a SINGLE composite verb (claim →
/// publish → mark-sent / dead-letter in one call) — deliberately NOT decomposed into
/// claim/mark-sent primitives, so the durable impl owns the `FOR UPDATE SKIP LOCKED` atomicity.
pub trait DurableOutboxBacking: Send + Sync {
    /// The durable arm of [`OutboxTransaction::commit`]: durably + atomically commit every staged
    /// row (all-or-nothing — a partial commit would be silent data loss). Assigns the per-aggregate
    /// commit-order `seq` the same way the in-memory store does (gap-free, true commit order). A
    /// duplicate `event_id` REJECTS (returns `Err`) — reject-parity with the in-memory arm.
    fn commit_staged(&self, rows: Vec<OutboxRow>) -> Result<()>;

    /// **The ABSORB arm of [`OutboxTransaction::commit_absorb`] (H1 — peer-review #7 re-prosecution).**
    /// Like [`commit_staged`](Self::commit_staged), but a DETERMINISTIC duplicate `event_id` is
    /// ABSORBED (`ON CONFLICT (event_id) DO NOTHING`) instead of rejected — AFTER verifying the stored
    /// row is byte-identical (a divergent payload under the same id is a genuine collision and still
    /// `Err`s). This is what a handler emitting deterministic ids (the CI dispatcher's co-emitted
    /// `ci.run.started` / `ci.check.updated`) needs so a crash-window redelivery that re-runs the
    /// handler re-emits the SAME ids WITHOUT the reject-arm's `Err("duplicate emit")` → `Retry` →
    /// UNBOUNDED LIVELOCK (the H1 bug). The events stay present exactly once.
    ///
    /// **Default = the reject arm** (delegates to [`commit_staged`](Self::commit_staged)): a backing
    /// that has not implemented true absorb-mode keeps the safe reject behavior (fail-closed, never a
    /// silent absorb). The production PG backing (`myelin_storage::outbox_durable::PgOutboxBacking`)
    /// OVERRIDES this with the real `ON CONFLICT DO NOTHING` + payload-equality verification.
    fn commit_staged_absorb(&self, rows: Vec<OutboxRow>) -> Result<()> {
        self.commit_staged(rows)
    }

    /// The `outbox_depth` survival signal (contract 1.8): the number of unsent rows.
    fn outbox_depth(&self) -> usize;
    /// The dead-letter count survival signal (contract 1.8).
    fn dead_letter_count(&self) -> usize;
    /// The `recorded_at` of the oldest still-unsent row (the outbox-age anchor), or `None` when
    /// fully drained.
    fn oldest_unsent_recorded_at(&self) -> Option<crate::Timestamp>;
    /// The total committed-row count (sent + unsent) — the no-ghost assertion input.
    fn committed_count(&self) -> usize;
    /// Read a committed row by id (the relay's envelope re-hydration / a test read).
    fn row(&self, id: &EventId) -> Option<OutboxRow>;
    /// Snapshot the committed rows in per-aggregate commit order.
    fn committed_rows(&self) -> Vec<OutboxRow>;
    /// Snapshot the dead-lettered rows (the operator alert / a test).
    fn dead_letters(&self) -> Vec<OutboxRow>;

    /// **The SINGLE composite relay verb** — one drain pass: claim up to `batch` unsent rows with
    /// the `FOR UPDATE SKIP LOCKED` discipline, publish each via
    /// `transport.put(subject, envelope, dedup_id = event_id)`, mark sent on `Accepted`/
    /// `Deduplicated`, and dead-letter a row that exhausts the retry bound — returning a
    /// [`DrainReport`]. Composite BY DESIGN (not decomposed into claim/mark primitives) so the
    /// durable impl owns the claim atomicity in one transaction.
    fn drain_once(&self, transport: &dyn BusTransport, batch: usize) -> Result<DrainReport>;
}

/// The outbox-store backend (the MR-007/008/023 backend-enum pattern; mirrors [`crate::dedup`]'s
/// `DedupBackend`). `Memory` is the in-memory transactional model (the SUB-D1/BUS-D4 drill state +
/// the test/dev in-process floor); `Durable` is the production PG-backed seam the W3b.4
/// composition roots wire (`OutboxStore::durable(PgOutboxBacking)` over the MR-022 provider).
///
/// **MR-009b W3b.6 — THE FLIP: the `Memory` arm is a `test-support`-gated TEST DOUBLE.** The
/// production-compiled enum presents ONLY `Durable(Arc<dyn DurableOutboxBacking>)`, so the
/// `no-in-memory-durable-store` scanner (which strips `test-support`-gated variants as test
/// doubles) no longer fires on the [`OutboxStore`] holder — the `outbox.rs` baseline entry is
/// REMOVED. Chain: W3b.1 role-struct (scanner-neutral) → W3b.2 `PgOutboxBacking` + relay parity →
/// W3b.3 identity co-commit (BUS-2 exact) → W3b.4 durable composition roots (fail-loud mains) →
/// W3b.5 harness gating → THIS flip.
#[derive(Clone)]
enum OutboxBackend {
    /// **MR-009b W3b.6 — TEST DOUBLE (compiled ONLY under `#[cfg(any(test, feature =
    /// "test-support"))]`).** The in-memory transactional model: all state behind a shared lock.
    /// DB-free unit tests/drills reach it via the `myelin-events/test-support` dev-dependency.
    #[cfg(any(test, feature = "test-support"))]
    Memory(Arc<Mutex<Inner>>),
    /// The durable PG-backed seam (production): an `outbox` table that survives restart.
    Durable(Arc<dyn DurableOutboxBacking>),
}

/// The default backend is the in-memory TEST DOUBLE — `#[cfg(any(test, feature =
/// "test-support"))]` (MR-009b W3b.6). Production constructs the `Durable` backing through
/// [`OutboxStore::durable`] (the W3b.4 composition roots).
#[cfg(any(test, feature = "test-support"))]
impl Default for OutboxBackend {
    fn default() -> Self {
        OutboxBackend::Memory(Arc::new(Mutex::new(Inner::default())))
    }
}

impl OutboxStore {
    /// **MR-009b W3b.6 — TEST DOUBLE (compiled ONLY under `#[cfg(any(test, feature =
    /// "test-support"))]`).** A fresh, empty IN-MEMORY store (depth 0, no dead-letters) — the
    /// SUB-D1/BUS-D4 drill + in-process test floor. The production constructor is
    /// [`OutboxStore::durable`] (the W3b.4 composition roots; a production root that reaches for
    /// this breaks LOUDLY at compile time — never a silent in-memory fallback).
    #[cfg(any(test, feature = "test-support"))]
    pub fn new() -> Self {
        Self::default()
    }

    /// **Bind the store to a DURABLE backing** (SI-007) so the transactional co-commit + the
    /// unsent-row ledger + the relay drain survive a process restart. A later wave's composition
    /// root constructs this with the PG-backed `outbox` table; [`OutboxStore::new`] stays the
    /// in-memory default. Every public read/commit/drain method dispatches to the backing.
    pub fn durable(backing: Arc<dyn DurableOutboxBacking>) -> Self {
        OutboxStore {
            backend: OutboxBackend::Durable(backing),
        }
    }

    /// Crate-internal memory-arm accessor (**`test-support`-gated with the [`Inner`] it
    /// guards**, MR-009b W3b.6): the in-memory [`Inner`] guard on the `Memory` backend; `None`
    /// on the `Durable` backend (whose reads + relay mechanics route straight to the trait,
    /// never here). The relay's memory-arm claim/mark-sent/dead-letter/GC mechanics
    /// ([`crate::relay`]) and the memory-arm reads/commit obtain their guard through this.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn mem(&self) -> Option<std::sync::MutexGuard<'_, Inner>> {
        match &self.backend {
            OutboxBackend::Memory(inner) => Some(inner.lock().unwrap_or_else(|e| e.into_inner())),
            OutboxBackend::Durable(_) => None,
        }
    }

    /// Crate-internal durable-backing accessor: `Some(backing)` on the `Durable` backend (so the
    /// commit dispatch + the relay drain route to it), `None` on the (`test-support`-gated)
    /// `Memory` backend. In the production build the enum presents ONLY `Durable`, so this is
    /// always `Some` there (the `None` arm exists only under test/test-support).
    pub(crate) fn durable_backing(&self) -> Option<Arc<dyn DurableOutboxBacking>> {
        match &self.backend {
            OutboxBackend::Durable(b) => Some(Arc::clone(b)),
            #[cfg(any(test, feature = "test-support"))]
            OutboxBackend::Memory(_) => None,
        }
    }

    /// Begin a transaction. Everything emitted on the returned [`OutboxTransaction`] — together
    /// with the caller's own state mutation, registered via
    /// [`OutboxTransaction::stage_state_change`] — is durable iff [`OutboxTransaction::commit`]
    /// is called. The `minter` (an [`Arc`] so the same source is shared across transactions)
    /// supplies the stable ULID; `ctx_base` carries the ambient tenant/region/actor/clock the
    /// emit derives from.
    pub fn begin(&self, minter: Arc<dyn IdMinter>, ctx_base: EmitContextBase) -> OutboxTransaction {
        OutboxTransaction {
            store: Some(self.clone()),
            minter,
            ctx_base,
            staged_rows: Vec::new(),
            state_committed: Arc::new(Mutex::new(None)),
        }
    }

    /// The `outbox_depth` survival signal (contract 1.8): the number of **unsent** rows
    /// (`published_at IS NULL`). The SUB-D1 drill asserts this `→ 0` once the relay drains.
    pub fn outbox_depth(&self) -> usize {
        match &self.backend {
            OutboxBackend::Durable(b) => b.outbox_depth(),
            #[cfg(any(test, feature = "test-support"))]
            OutboxBackend::Memory(_) => {
                let inner = self.mem().expect("memory backend");
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
        }
    }

    /// The dead-letter count survival signal (contract 1.8): rows the relay gave up on after
    /// bounded retries. The no-loss path asserts this `== 0`.
    pub fn dead_letter_count(&self) -> usize {
        match &self.backend {
            OutboxBackend::Durable(b) => b.dead_letter_count(),
            #[cfg(any(test, feature = "test-support"))]
            OutboxBackend::Memory(_) => self.mem().expect("memory backend").dead_letters.len(),
        }
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
        match &self.backend {
            OutboxBackend::Durable(b) => b.oldest_unsent_recorded_at(),
            #[cfg(any(test, feature = "test-support"))]
            OutboxBackend::Memory(_) => {
                let inner = self.mem().expect("memory backend");
                inner.order.iter().find_map(|id| {
                    inner
                        .rows
                        .get(id)
                        .filter(|r| r.published_at.is_none())
                        .map(|r| r.envelope.recorded_at.clone())
                })
            }
        }
    }

    /// The total committed-row count (sent + unsent), for the no-ghost assertion (every
    /// committed event is delivered exactly once; an aborted transaction adds nothing here).
    pub fn committed_count(&self) -> usize {
        match &self.backend {
            OutboxBackend::Durable(b) => b.committed_count(),
            #[cfg(any(test, feature = "test-support"))]
            OutboxBackend::Memory(_) => self.mem().expect("memory backend").order.len(),
        }
    }

    /// Read a row by id (for tests / the relay's re-hydration of the envelope to publish).
    pub fn row(&self, id: &EventId) -> Option<OutboxRow> {
        match &self.backend {
            OutboxBackend::Durable(b) => b.row(id),
            #[cfg(any(test, feature = "test-support"))]
            OutboxBackend::Memory(_) => self.mem().expect("memory backend").rows.get(id).cloned(),
        }
    }

    /// Snapshot the dead-lettered rows (for the operator alert / a test).
    pub fn dead_letters(&self) -> Vec<OutboxRow> {
        match &self.backend {
            OutboxBackend::Durable(b) => b.dead_letters(),
            #[cfg(any(test, feature = "test-support"))]
            OutboxBackend::Memory(_) => self.mem().expect("memory backend").dead_letters.clone(),
        }
    }

    /// Snapshot the committed rows in per-aggregate commit order (for a producer test / a consumer
    /// re-hydration). The `order` vector is the commit sequence; each id resolves to its row. Used by
    /// a producer drill to assert WHICH events a workflow body emitted (e.g. the CI-P15 `ci.pipeline`
    /// body's terminal `ci.check.updated` / `ci.run.*` / `ci.result` facts).
    pub fn committed_rows(&self) -> Vec<OutboxRow> {
        match &self.backend {
            OutboxBackend::Durable(b) => b.committed_rows(),
            #[cfg(any(test, feature = "test-support"))]
            OutboxBackend::Memory(_) => {
                let inner = self.mem().expect("memory backend");
                inner
                    .order
                    .iter()
                    .filter_map(|id| inner.rows.get(id).cloned())
                    .collect()
            }
        }
    }

    /// **Restore seam: insert a committed outbox row directly (bypassing a transaction) — the PITR-target
    /// re-hydration the durable-workflow restore-verify (FLOW-D10 / P-FLOW-25,
    /// `myelin_flow::restore_verify`) populates the RESTORED outbox from.** Models `pg_restore` re-loading
    /// the retained `outbox` rows (all `seq <= T`) into a clean target: the row is re-inserted at its
    /// original `(event_id, aggregate, seq)`, preserving the commit order so the cross-seam offset
    /// reconcile reads the SAME committed sequence the live store held. Idempotent on `event_id` (the
    /// UNIQUE key) — a re-load of an already-present row is a no-op. NOT a production write path: the ONE
    /// production write path is [`OutboxTransaction::commit`].
    ///
    /// **MR-009b W3b.6 — `test-support`-gated with the memory arm it writes into** (the durable
    /// arm re-hydrates via `pg_restore`, never through this seam).
    #[doc(hidden)]
    #[cfg(any(test, feature = "test-support"))]
    pub fn restore_committed_row_for_test(&self, row: OutboxRow) {
        // Memory-arm test/restore seam; the durable arm re-hydrates via `pg_restore`, not here.
        let Some(mut inner) = self.mem() else { return };
        if inner.rows.contains_key(&row.event_id) {
            return;
        }
        let next = inner.next_seq.entry(row.aggregate.clone()).or_insert(0);
        *next = (*next).max(row.seq + 1);
        inner.order.push(row.event_id.clone());
        inner.rows.insert(row.event_id.clone(), row);
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
    /// The backing used by the ordinary commit APIs. `None` is a detached staging buffer whose
    /// caller must move the rows into a larger caller-owned database transaction.
    store: Option<OutboxStore>,
    minter: Arc<dyn IdMinter>,
    ctx_base: EmitContextBase,
    /// The rows emitted-but-not-yet-committed (the buffer the co-commit makes durable atomically).
    staged_rows: Vec<OutboxRow>,
    /// The caller's staged state change (a description the test can read), proving the event +
    /// the state mutation share the transaction. `None` if the caller staged none.
    state_committed: Arc<Mutex<Option<String>>>,
}

impl OutboxTransaction {
    /// Open a detached outbox staging buffer. This derives canonical envelopes through the same
    /// [`OutboxTx`] implementation as an ordinary transaction but owns no persistence backing;
    /// [`into_staged_rows`](Self::into_staged_rows) is the only successful terminal operation.
    /// Durable workflow engines use this to move emitted rows into their journal/run-state
    /// transaction, avoiding a second outbox commit and preserving emit-iff-domain-committed.
    pub fn detached(minter: Arc<dyn IdMinter>, ctx_base: EmitContextBase) -> Self {
        Self {
            store: None,
            minter,
            ctx_base,
            staged_rows: Vec::new(),
            state_committed: Arc::new(Mutex::new(None)),
        }
    }

    /// Consume a detached buffer and return its canonical unpublished rows. A transaction created
    /// by [`OutboxStore::begin`] cannot use this escape hatch: it must commit through its backing.
    pub fn into_staged_rows(mut self) -> Result<Vec<OutboxRow>> {
        if self.store.is_some() {
            return Err(OutboxError(
                "only a detached outbox transaction may export staged rows".into(),
            ));
        }
        let mut event_ids = HashSet::with_capacity(self.staged_rows.len());
        for row in &self.staged_rows {
            if !event_ids.insert(row.event_id.clone()) {
                return Err(OutboxError(
                    "detached outbox batch contains a duplicate event_id".into(),
                ));
            }
            if row.seq != 0 || row.published_at.is_some() || row.attempts != 0 {
                return Err(OutboxError(
                    "detached outbox rows must retain the unallocated, unpublished staging shape"
                        .into(),
                ));
            }
            if row.event_id != row.envelope.event_id
                || row.aggregate != row.envelope.aggregate
                || row.subject != row.envelope.subject
            {
                return Err(OutboxError(
                    "detached outbox row routing must exactly match its canonical envelope".into(),
                ));
            }
        }
        Ok(self.staged_rows.drain(..).collect())
    }

    /// Stage the caller's own state mutation into THIS transaction (the "state change" half of
    /// the co-commit). In a real service this is the row the handler writes to its own table in
    /// the same DB transaction the outbox row is inserted into; here it is recorded so a test
    /// can assert the state and the event commit together (and that an abort writes neither).
    pub fn stage_state_change(&mut self, change: impl Into<String>) {
        *self
            .state_committed
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(change.into());
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
    /// **Commit in ABSORB mode (H1 — peer-review #7 re-prosecution).** Identical to
    /// [`commit`](Self::commit) EXCEPT a DETERMINISTIC duplicate `event_id` (a re-emit whose id is
    /// derived from a triggering `event_id`) is ABSORBED (`ON CONFLICT (event_id) DO NOTHING`, after
    /// verifying byte-identical payload) instead of rejected. A handler that emits deterministic ids
    /// (the CI dispatcher) uses THIS on a crash-window redelivery so the re-emit does not `Err` into an
    /// unbounded `Retry` livelock. The rows still commit exactly once. On the in-memory arm the absorb
    /// is modeled by skipping an already-present byte-identical id (see below); a divergent payload
    /// under the same id still rejects.
    pub fn commit_absorb(mut self) -> Result<()> {
        let store = self.store.take().ok_or_else(|| {
            OutboxError("detached outbox rows require a caller-owned atomic commit".into())
        })?;
        if let Some(backing) = store.durable_backing() {
            let rows: Vec<OutboxRow> = self.staged_rows.drain(..).collect();
            return backing.commit_staged_absorb(rows);
        }
        // Memory arm (test-support): absorb a byte-identical already-present id; reject a divergent one.
        #[cfg(any(test, feature = "test-support"))]
        {
            let mut inner = store.mem().expect("memory backend");
            for row in &self.staged_rows {
                if let Some(existing) = inner.rows.get(&row.event_id) {
                    if existing.envelope != row.envelope {
                        return Err(OutboxError(format!(
                            "outbox event_id {:?} already present with a DIFFERENT payload — genuine collision",
                            row.event_id
                        )));
                    }
                }
            }
            for mut row in self.staged_rows.drain(..) {
                if inner.rows.contains_key(&row.event_id) {
                    continue; // deterministic re-emit — absorb (already present, byte-identical).
                }
                let slot = inner.next_seq.entry(row.aggregate.clone()).or_insert(0);
                row.seq = *slot;
                *slot += 1;
                inner.order.push(row.event_id.clone());
                inner.rows.insert(row.event_id.clone(), row);
            }
            return Ok(());
        }
        #[cfg(not(any(test, feature = "test-support")))]
        unreachable!(
            "a production OutboxStore is Durable-only (the Memory arm is test-support-gated)"
        )
    }

    pub fn commit(mut self) -> Result<()> {
        let store = self.store.take().ok_or_else(|| {
            OutboxError("detached outbox rows require a caller-owned atomic commit".into())
        })?;
        // Durable dispatch: the whole staged buffer commits atomically through the backing (the
        // durable arm of the co-commit). The backing assigns the per-aggregate seq the same way.
        if let Some(backing) = store.durable_backing() {
            let rows: Vec<OutboxRow> = self.staged_rows.drain(..).collect();
            return backing.commit_staged(rows);
        }
        // Memory arm (MR-009b W3b.6: the `test-support`-gated test double; in the production
        // build `durable_backing()` is always `Some` — the enum presents only `Durable`).
        #[cfg(any(test, feature = "test-support"))]
        {
            let mut inner = store.mem().expect("memory backend");
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
        #[cfg(not(any(test, feature = "test-support")))]
        // Structurally unreachable in the production build: `OutboxBackend` has only the
        // `Durable` variant there, so `durable_backing()` returned `Some` above.
        unreachable!(
            "a production OutboxStore is Durable-only (the Memory arm is test-support-gated)"
        )
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

    /// **Emit with a caller-supplied DETERMINISTIC `event_id` (peer-review #8).** Identical to
    /// [`OutboxTx::emit`] (same causality derivation via [`derive_envelope`]) EXCEPT the stable id is
    /// the supplied `id` instead of a freshly-minted ULID. A reaction whose id is derived
    /// deterministically from its triggering `event_id` (e.g. the CI dispatcher's co-emitted
    /// `ci.run.started` / `ci.check.updated`) mints the SAME id on a re-run.
    ///
    /// **H1 (peer-review #7 re-prosecution) — the dedup is NOT automatic on `commit`.** The default
    /// [`commit`](OutboxTransaction::commit) is the REJECT arm: a duplicate `event_id` returns
    /// `Err("duplicate emit")` (reject-parity). So a deterministic re-emit committed with plain
    /// `commit` would `Err` → the handler `Retry`s → an UNBOUNDED LIVELOCK. To get the "dedup the
    /// re-emit, no duplicate events" behavior, the caller MUST commit the deterministic-id transaction
    /// with [`commit_absorb`](OutboxTransaction::commit_absorb) (the `ON CONFLICT (event_id) DO
    /// NOTHING` + payload-equality path). A prior doc here WRONGLY claimed the reject-arm `commit`
    /// itself did `ON CONFLICT DO NOTHING` — it does not; that was the H1 livelock. This is NOT a
    /// raw-publish (it stages into the same co-commit buffer as `emit`; the `no-raw-publish` lint
    /// guards `publish_now` / `BusTransport::put`, not the emit family).
    pub fn emit_with_id(
        &mut self,
        id: EventId,
        draft: EventDraft,
        cause: Option<&EventEnvelope>,
    ) -> Result<EventId> {
        let aggregate = draft.aggregate.clone();
        let subject = draft.subject.clone();
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
        let envelope = derive_envelope(draft, ctx, cause);
        self.staged_rows.push(OutboxRow {
            event_id: id.clone(),
            aggregate,
            seq: 0,
            subject,
            envelope,
            published_at: None,
            attempts: 0,
        });
        Ok(id)
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

/// Crockford's base32 alphabet (excludes I, L, O, U) — the canonical ULID rendering alphabet.
/// Rendering a `u128` as 26 fixed-width Crockford digits preserves order: a numerically greater
/// value renders to a lexically greater string (the `IdMinter` monotonicity property).
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// **The production ULID source — the P-S12 stand-in (MR-009b W3b.4).** A real ULID: a 48-bit
/// wall-clock millisecond timestamp + 80 bits of per-process randomness, rendered as 26
/// Crockford-base32 characters, with a **monotonic guard** within the process (two mints in the
/// same millisecond never violate lexical order — the second bumps the previous value instead of
/// re-rolling). Implements the SAME [`IdMinter`] trait the store was built against, exactly as the
/// module-level "FLOOR — the ULID source" note promised: the store/relay do not change.
///
/// **Why composition roots MUST wire this (the W3b.3 named condition, P-S12 minter floor):** the
/// deterministic [`MonotonicMinter`] resets its counter to `0` per instance, so two production
/// roots (two stores, two processes, or one process across a restart) mint COLLIDING `event_id`s —
/// and the durable co-commit path's `ON CONFLICT (event_id) DO NOTHING`
/// (`PgRelay::co_commit_in_tx`) then SILENTLY DROPS the later event (probe-proven in W3b.3).
/// This source seeds its 80 random bits from OS entropy (`RandomState`) + the process id + the
/// nanosecond clock, so ids are unique across stores, processes, and restarts — the collision
/// path is closed. [`MonotonicMinter`] remains the deterministic TEST source (seeded, no
/// wall-clock flakiness); this is the source every PRODUCTION root injects.
///
/// **Stand-in honesty:** this is the P-S12 stand-in, not a distributed-uniqueness proof — the
/// 80-bit randomness gives the standard ULID collision bound (~2^40 mints per ms for a 50%
/// birthday collision), which is the accepted production posture for `Nats-Msg-Id`-class dedup
/// keys. A shared-entropy/coordinated scheme is deliberately NOT built (no measured need).
pub struct UlidMinter {
    /// The last minted 128-bit value — the monotonic-within-process guard (lexical order ==
    /// mint order even under a same-ms burst or a clock step backwards).
    last: Mutex<u128>,
    /// The per-process random seed: OS entropy (`RandomState`'s per-instance random keys) mixed
    /// with the process id, so two processes started in the same nanosecond still diverge.
    seed: u64,
    /// A per-mint disambiguator mixed into the random bits (concurrent mints diverge pre-guard).
    bump: AtomicU64,
}

impl Default for UlidMinter {
    fn default() -> Self {
        UlidMinter::new()
    }
}

impl UlidMinter {
    /// A fresh production ULID source seeded from OS entropy + the process id + the clock.
    pub fn new() -> UlidMinter {
        use std::hash::{BuildHasher, Hasher};
        // `RandomState` carries genuinely random per-instance keys from the OS — the workspace's
        // no-new-dependency entropy source (no `rand` crate edge added to the events sink).
        let os_entropy = std::collections::hash_map::RandomState::new()
            .build_hasher()
            .finish();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        UlidMinter {
            last: Mutex::new(0),
            seed: os_entropy ^ (u64::from(std::process::id()).rotate_left(32)) ^ nanos,
            bump: AtomicU64::new(0),
        }
    }

    fn now_ms() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    }

    /// 80 pseudo-random bits for the ULID tail: a splitmix64-style avalanche over the per-process
    /// seed, the per-mint counter, and the nanosecond clock. Randomness only has to make ids
    /// unlikely-to-collide ACROSS processes within a millisecond; ORDER comes from the timestamp
    /// + the monotonic guard, never from these bits.
    fn rand80(&self) -> u128 {
        fn splitmix64(mut z: u64) -> u64 {
            z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }
        let n = self.bump.fetch_add(1, Ordering::SeqCst);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let hi = splitmix64(self.seed ^ n ^ nanos);
        let lo = splitmix64(hi ^ self.seed.rotate_left(17));
        (u128::from(hi) << 64 | u128::from(lo)) & ((1u128 << 80) - 1)
    }

    /// Render a 128-bit ULID value as its canonical 26-char Crockford-base32 string (fixed width,
    /// most-significant first — lexical order == numeric order).
    fn render(value: u128) -> String {
        let mut buf = [0u8; 26];
        let mut v = value;
        for slot in buf.iter_mut().rev() {
            *slot = CROCKFORD[(v & 0x1f) as usize];
            v >>= 5;
        }
        String::from_utf8(buf.to_vec()).expect("crockford bytes are ASCII")
    }
}

impl IdMinter for UlidMinter {
    fn mint(&self) -> Ulid {
        let candidate = (Self::now_ms() << 80) | self.rand80();
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        // The monotonic guard: never return a value <= the last one (same-ms burst or a clock
        // that stepped backwards bumps the previous value by 1 — order preserved, id still unique
        // within this process; cross-process uniqueness rides the random tail).
        let value = if candidate > *last {
            candidate
        } else {
            last.wrapping_add(1)
        };
        *last = value;
        Ulid(Self::render(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Actor, ArtifactRef, CausedBy, DataRole, EventType, Region, TenantId, Timestamp, Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn principal() -> Principal {
        Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
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
        for col in [
            "event_id",
            "aggregate",
            "seq",
            "subject",
            "envelope",
            "published_at",
        ] {
            assert!(
                OUTBOX_MIGRATION.contains(col),
                "migration is missing column {col}"
            );
        }
        // forward-only: no destructive DROP on the down path (there is no down path).
        assert!(!OUTBOX_MIGRATION.contains("DROP TABLE"));
    }

    #[test]
    fn quarantine_migration_keeps_payload_in_the_original_outbox_only() {
        assert!(OUTBOX_QUARANTINE_MIGRATION.contains("PRIMARY KEY REFERENCES outbox(event_id)"));
        for col in [
            "aggregate",
            "seq",
            "reason_code",
            "reason_detail",
            "acknowledged_at",
        ] {
            assert!(OUTBOX_QUARANTINE_MIGRATION.contains(col));
        }
        assert!(!OUTBOX_QUARANTINE_MIGRATION.contains("envelope"));
        assert!(!OUTBOX_QUARANTINE_MIGRATION.contains("payload"));
        assert!(!OUTBOX_QUARANTINE_MIGRATION.contains("subject"));
        assert!(!OUTBOX_QUARANTINE_MIGRATION.contains("DROP TABLE"));
    }

    /// A committed transaction makes its staged event + state change durable together — the
    /// co-commit happy path. After commit, `outbox_depth` reflects exactly the events emitted.
    #[test]
    fn commit_makes_event_and_state_durable_together() {
        let (store, minter) = store_and_minter();
        let mut tx = store.begin(minter, ctx_base());
        tx.stage_state_change("issue PROJ-1 created");
        let id = tx
            .emit(draft("issues.issue.created", "issue:PROJ-1"), None)
            .unwrap();
        assert_eq!(tx.staged_len(), 1, "one event buffered");
        let id2 = tx
            .emit(draft("issues.issue.updated", "issue:PROJ-1"), None)
            .unwrap();
        // before commit: nothing durable (emit-iff-committed) — depth still 0.
        assert_eq!(
            store.outbox_depth(),
            0,
            "an open transaction has written nothing"
        );
        assert_eq!(tx.staged_len(), 2, "two events buffered (not a constant)");
        assert_eq!(tx.staged_state().as_deref(), Some("issue PROJ-1 created"));

        tx.commit().unwrap();
        // after commit: both event rows are durable + unsent (depth 2).
        assert_eq!(store.outbox_depth(), 2);
        assert_eq!(store.committed_count(), 2);
        let row = store.row(&id).expect("committed row is present");
        assert_eq!(row.seq, 0, "first event for the aggregate is seq 0");
        assert!(
            row.published_at.is_none(),
            "a freshly committed row is unsent"
        );
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
            tx.emit(draft("issues.issue.created", "issue:PROJ-9"), None)
                .unwrap();
            assert_eq!(tx.staged_len(), 1, "buffered, not committed");
            // tx dropped here WITHOUT commit (the crash point).
        }
        // emit-iff-committed: the aborted transaction published nothing.
        assert_eq!(
            store.outbox_depth(),
            0,
            "an aborted transaction writes no event"
        );
        assert_eq!(store.committed_count(), 0, "no ghost row from an abort");
        assert_eq!(store.dead_letter_count(), 0);
    }

    #[test]
    fn detached_transaction_exports_only_canonical_unallocated_rows() {
        let (_, minter) = store_and_minter();
        let mut tx = OutboxTransaction::detached(minter, ctx_base());
        let id = tx
            .emit(draft("issues.issue.created", "issue:DETACHED"), None)
            .unwrap();

        let rows = tx.into_staged_rows().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].event_id, id);
        assert_eq!(rows[0].event_id, rows[0].envelope.event_id);
        assert_eq!(
            rows[0].seq, 0,
            "the durable co-commit owns sequence allocation"
        );
        assert!(rows[0].published_at.is_none());
        assert_eq!(rows[0].attempts, 0);
    }

    #[test]
    fn detached_transaction_rejects_commit_and_preallocated_sequence() {
        let (_, minter) = store_and_minter();
        let mut commit_tx = OutboxTransaction::detached(Arc::clone(&minter), ctx_base());
        commit_tx
            .emit(draft("issues.issue.created", "issue:DETACHED"), None)
            .unwrap();
        assert!(
            commit_tx.commit().is_err(),
            "there is no second publish path"
        );

        let mut staged_tx = OutboxTransaction::detached(minter, ctx_base());
        staged_tx
            .emit(draft("issues.issue.created", "issue:DETACHED"), None)
            .unwrap();
        staged_tx.staged_rows[0].seq = 7;
        assert!(staged_tx.into_staged_rows().is_err());
    }

    #[test]
    fn detached_transaction_rejects_duplicate_event_ids_within_one_drive() {
        struct ConstantMinter;
        impl IdMinter for ConstantMinter {
            fn mint(&self) -> Ulid {
                Ulid("01DETACHEDDUPLICATE0000000".into())
            }
        }

        let mut tx = OutboxTransaction::detached(Arc::new(ConstantMinter), ctx_base());
        tx.emit(draft("issues.issue.created", "issue:ONE"), None)
            .unwrap();
        tx.emit(draft("issues.issue.created", "issue:TWO"), None)
            .unwrap();
        assert!(tx.into_staged_rows().is_err());
    }

    #[test]
    fn store_backed_transaction_cannot_export_around_its_commit_boundary() {
        let (store, minter) = store_and_minter();
        let mut tx = store.begin(minter, ctx_base());
        tx.emit(draft("issues.issue.created", "issue:BACKED"), None)
            .unwrap();
        assert!(tx.into_staged_rows().is_err());
        assert_eq!(store.outbox_depth(), 0);
    }

    /// `emit` derives causality through the trait (a caused event sets depth = parent+1 and
    /// carries the root) AND, on commit, the rows carry a monotonic per-aggregate seq. Proves
    /// the trait impl wires `derive_envelope` and the commit-time ordering key together.
    #[test]
    fn emit_derives_causality_and_assigns_monotonic_seq_per_aggregate() {
        let (store, minter) = store_and_minter();
        let mut tx = store.begin(minter, ctx_base());

        let root_id = tx
            .emit(draft("issues.issue.created", "issue:PROJ-1"), None)
            .unwrap();
        let root_env = store_envelope(&tx, 0);
        assert_eq!(root_env.depth, 0);
        assert_eq!(
            root_env.correlation_id.0, root_id.0,
            "root carries its own correlation"
        );

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
        assert_eq!(
            store.row(&root_id).unwrap().aggregate,
            store.row(&child_id).unwrap().aggregate
        );
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
        let a0 = tx
            .emit(draft("issues.issue.created", "issue:A"), None)
            .unwrap();
        let b0 = tx
            .emit(draft("issues.issue.created", "issue:B"), None)
            .unwrap();
        let a1 = tx
            .emit(draft("issues.issue.updated", "issue:A"), None)
            .unwrap();
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
        assert_eq!(
            seqs, expected,
            "concurrent emitters must yield contiguous, unique seqs"
        );
        assert_eq!(
            store.committed_count(),
            N as usize,
            "every committed event is present once"
        );
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
        assert_eq!(
            store.row(&c).unwrap().seq,
            1,
            "abort must not burn a seq → gap-free"
        );
        assert_eq!(
            store.committed_count(),
            2,
            "only the two committed events exist"
        );
    }

    /// **The production `UlidMinter` (P-S12 stand-in) satisfies the W3b.3 named condition:**
    /// two independent minters (modeling two composition roots / two processes / a restart) mint
    /// DISJOINT ids — unlike two `MonotonicMinter`s, which both start at `01J…0` and collide
    /// (the collision the durable `ON CONFLICT (event_id) DO NOTHING` silently drops).
    #[test]
    fn ulid_minter_two_instances_mint_disjoint_ids() {
        // First, pin the hazard this closes: two default MonotonicMinters DO collide.
        assert_eq!(
            MonotonicMinter::new().mint(),
            MonotonicMinter::new().mint(),
            "the deterministic test minter resets per instance (the named hazard)"
        );
        let a = UlidMinter::new();
        let b = UlidMinter::new();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1_000 {
            assert!(seen.insert(a.mint()), "minter A repeated an id");
            assert!(
                seen.insert(b.mint()),
                "minter B collided with A or repeated"
            );
        }
    }

    /// The production `UlidMinter` is monotonic within the process (lexical order == mint order,
    /// the `IdMinter` contract) even under a same-millisecond burst, and renders the canonical
    /// 26-char Crockford-base32 ULID form.
    #[test]
    fn ulid_minter_is_monotonic_and_canonical_within_process() {
        let m = UlidMinter::new();
        let mut prev = m.mint();
        assert_eq!(prev.0.len(), 26, "canonical 26-char ULID rendering");
        for _ in 0..1_000 {
            let next = m.mint();
            assert_eq!(next.0.len(), 26);
            assert!(
                next.0.bytes().all(|b| CROCKFORD.contains(&b)),
                "canonical Crockford alphabet only"
            );
            assert!(
                prev < next,
                "same-ms burst must stay monotonic: {prev:?} < {next:?}"
            );
            prev = next;
        }
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

    // ============================================================================================
    // MR-009b W3b.1 — the `DurableOutboxBacking` role-struct dispatch (scanner-neutral reshape).
    // A test-local mock backing proves the `Durable` arm routes commit + reads + the composite
    // drain verb to the trait, while the always-compiled `Memory` arm's behavior is unchanged
    // (every memory-arm test above keeps passing byte-for-byte).
    // ============================================================================================

    /// A test-local mock [`DurableOutboxBacking`]: records the rows handed to `commit_staged` and
    /// answers every read from that recorded set, so a `Durable` store's dispatch is observable.
    /// Its `drain_once` is the SINGLE composite verb — it marks the unsent rows published and
    /// reports them (recording the batch bound it was called with).
    #[derive(Default)]
    struct MockBacking {
        committed: Mutex<Vec<OutboxRow>>,
        drain_calls: Mutex<Vec<usize>>,
    }

    impl DurableOutboxBacking for MockBacking {
        fn commit_staged(&self, rows: Vec<OutboxRow>) -> Result<()> {
            self.committed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .extend(rows);
            Ok(())
        }
        fn outbox_depth(&self) -> usize {
            self.committed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .filter(|r| r.published_at.is_none())
                .count()
        }
        fn dead_letter_count(&self) -> usize {
            0
        }
        fn oldest_unsent_recorded_at(&self) -> Option<Timestamp> {
            self.committed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .find(|r| r.published_at.is_none())
                .map(|r| r.envelope.recorded_at.clone())
        }
        fn committed_count(&self) -> usize {
            self.committed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .len()
        }
        fn row(&self, id: &EventId) -> Option<OutboxRow> {
            self.committed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .find(|r| &r.event_id == id)
                .cloned()
        }
        fn committed_rows(&self) -> Vec<OutboxRow> {
            self.committed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }
        fn dead_letters(&self) -> Vec<OutboxRow> {
            Vec::new()
        }
        fn drain_once(&self, _transport: &dyn BusTransport, batch: usize) -> Result<DrainReport> {
            self.drain_calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(batch);
            let mut rows = self.committed.lock().unwrap_or_else(|e| e.into_inner());
            let mut published = 0;
            for r in rows.iter_mut().filter(|r| r.published_at.is_none()) {
                r.published_at = Some(Timestamp("2026-06-19T00:00:09Z".into()));
                published += 1;
            }
            Ok(DrainReport {
                published,
                ..Default::default()
            })
        }
    }

    /// **Commit dispatch routes the whole staged buffer to `commit_staged` (the durable arm of the
    /// co-commit).** A `Durable` store's transaction stages rows, and `commit()` hands them — in
    /// ONE atomic call — to the backing (nothing reaches it before commit; emit-iff-committed
    /// holds on the durable arm too). The store's reads then route to the backing.
    #[test]
    fn commit_dispatches_staged_rows_to_the_durable_backing() {
        let backing = Arc::new(MockBacking::default());
        let store = OutboxStore::durable(backing.clone());
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());

        let mut tx = store.begin(minter, ctx_base());
        tx.stage_state_change("state");
        let a = tx
            .emit(draft("issues.issue.created", "issue:A"), None)
            .unwrap();
        let b = tx
            .emit(draft("issues.issue.updated", "issue:A"), None)
            .unwrap();
        assert_eq!(tx.staged_len(), 2, "two rows buffered");
        // emit-iff-committed: nothing reaches the backing before commit.
        assert_eq!(
            backing.committed_count(),
            0,
            "an open tx wrote nothing durable"
        );

        tx.commit().unwrap();
        // the staged rows routed through `commit_staged` (one atomic call, both rows).
        let committed = backing.committed_rows();
        assert_eq!(committed.len(), 2, "both staged rows handed to the backing");
        assert_eq!(committed[0].event_id, a);
        assert_eq!(committed[1].event_id, b);
        // and the durable store's reads route to the backing.
        assert_eq!(store.committed_count(), 2);
        assert_eq!(store.outbox_depth(), 2);
        assert_eq!(store.row(&a).unwrap().event_id, a);
    }

    /// **A Durable-only store's reads route to the backing.** A `Durable` store has NO in-memory
    /// `Inner` (`mem()` is `None`), so if a read did not dispatch it would panic on the missing
    /// memory arm; every read returning the backing's view proves the dispatch.
    #[test]
    fn durable_store_reads_route_to_the_backing() {
        // Build realistic rows via a MEMORY store, then hand them to the backing directly.
        let (mem, minter) = store_and_minter();
        let mut tx = mem.begin(minter, ctx_base());
        let a = tx
            .emit(draft("issues.issue.created", "issue:A"), None)
            .unwrap();
        tx.emit(draft("issues.issue.updated", "issue:B"), None)
            .unwrap();
        tx.commit().unwrap();
        let rows = mem.committed_rows();

        let backing = Arc::new(MockBacking::default());
        backing.commit_staged(rows.clone()).unwrap();
        let store = OutboxStore::durable(backing.clone());

        assert_eq!(store.committed_count(), 2);
        assert_eq!(store.outbox_depth(), 2, "both rows unsent in the backing");
        assert_eq!(store.committed_rows().len(), 2);
        assert_eq!(store.row(&a).unwrap().event_id, a);
        assert!(store.dead_letters().is_empty());
        assert_eq!(store.dead_letter_count(), 0);
        assert_eq!(
            store.oldest_unsent_recorded_at(),
            Some(rows[0].envelope.recorded_at.clone()),
            "oldest-unsent age anchor read off the backing"
        );
    }

    /// **The relay's drain dispatches to the backing's SINGLE composite verb.** `Relay::drain_once`
    /// over a `Durable` store calls `backing.drain_once(transport, batch)` (never the in-memory
    /// claim/mark mechanics), passing the default batch bound; the report + the resulting depth
    /// reflect the backing.
    #[test]
    fn relay_drain_routes_to_the_durable_backing_composite_verb() {
        use crate::relay::{InProcessBus, Relay, DEFAULT_DRAIN_BATCH};
        let backing = Arc::new(MockBacking::default());
        let store = OutboxStore::durable(backing.clone());
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let mut tx = store.begin(minter, ctx_base());
        tx.emit(draft("issues.issue.created", "issue:A"), None)
            .unwrap();
        tx.emit(draft("issues.issue.updated", "issue:A"), None)
            .unwrap();
        tx.commit().unwrap();
        assert_eq!(store.outbox_depth(), 2);

        let relay = Relay::new(store.clone(), InProcessBus::new(), || {
            Timestamp("2026-06-19T00:00:09Z".into())
        });
        let report = relay.drain_once();
        assert_eq!(report.published, 2, "drain routed to backing.drain_once");
        assert_eq!(
            store.outbox_depth(),
            0,
            "the backing marked the rows published"
        );
        assert_eq!(
            backing
                .drain_calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_slice(),
            &[DEFAULT_DRAIN_BATCH],
            "the composite verb was called once with the default batch bound"
        );
    }
}
