//! # The `consumer_dedup` ledger — the effectively-once anchor (contract 2.5)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/event-bus.md` §3.3 (the `consumer_dedup`
//! ledger) + §4.2 (at-least-once + idempotent consumers ≈ effectively-once; Helland 2007/2012 —
//! Myelin does **not** chase true exactly-once).
//!
//! **Contract-index:** row 2.5 (`consumer_dedup` ledger — `(consumer, event_id)` PK, presence ==
//! "already handled"). **EB-06 → global P-015.**
//!
//! ## What this module ships (the effectively-once anchor every consumer resolves through)
//! The [`DedupLedger`] (`(consumer, event_id)` PK; presence == "already handled") and the
//! `INSERT … ON CONFLICT DO NOTHING` primitive ([`DedupLedger::mark_handled`]) the EB-05
//! consumer template's idempotency rule (rule 1) calls. At-least-once delivery + an idempotent
//! handler + this ledger ≈ **effectively-once**: a redelivered `(consumer, event_id)` is a no-op
//! (the second insert returns "already present"), so the handler runs exactly once in effect.
//!
//! The PK is the **pair**, so:
//! - a redelivery to the SAME consumer is suppressed (the idempotency no-op), and
//! - the SAME event delivered to two DIFFERENT consumers is fresh for EACH (each processes it
//!   once) — the consumer dimension of the PK.
//!
//! ## RECONCILED IN PLACE (EB-06 / P-015, 2026-06-19) — coherence, EI-01 §7
//! EB-06 names the `consumer_dedup` ledger as its own deliverable, in a file `dedup.rs`. But the
//! ledger had **already shipped** as part of the idempotent-consumer runtime in **P-009 / P-S08**
//! (the substrate roadmap reached the consumer template — which DEPENDS on the ledger — first; the
//! event-bus roadmap reaches the ledger as its own EB-06 unit). Per the coherence rule (EI-01 §7:
//! never define a type twice, never build a parallel second implementation), EB-06 **reconciles
//! in place**: the `DedupLedger` + the frozen 2.5 DDL ([`CONSUMER_DEDUP_MIGRATION`]) were MOVED
//! verbatim out of `consumer.rs` into THIS file (the EB-06-named home) with **no name/type/unit/
//! semantics change**, and are re-exported from the crate root so every frozen public path
//! (`myelin_events::DedupLedger`, `::CONSUMER_DEDUP_MIGRATION`) is unchanged. The
//! [`crate::consumer::Consumer`] runtime keeps calling exactly the same `DedupLedger` API
//! (`mark_handled` is rule 1). What EB-06 ADDS is (a) the named-deliverable file home for the
//! effectively-once anchor and (b) the **standalone 2.5 CDC pair** + the focused unit tests
//! (idempotent re-delivery proven; the per-consumer PK proven) — the gate EB-06's DEFINITION OF
//! DONE names. The combined 2.4/2.5 consumer-side CDC in `tests/drills_sub_d2_consumer.rs`
//! (`cdc_2_4_2_5_*`) stays as the end-to-end relay→consumer pair; the new `cdc_2_5_*` here is the
//! ledger-focused pair the coverage scanner reads for row 2.5 specifically.
//!
//! ## DEVIATION / FLOOR — the in-memory ledger models the SQL `consumer_dedup` table
//! There is **no live OLTP DB in M0** (the OLTP tier client is **P-007 / P-ST-01**; the migration
//! runner is **P-S15**). So the `consumer_dedup` ledger is modeled as an **in-memory
//! [`DedupLedger`]** whose semantics are byte-for-byte the 2.5 contract: `(consumer, event_id)` is
//! the PK (a second insert of the same pair is the no-op idempotency check), per-consumer so two
//! consumers of the same event each process it once. The frozen DDL
//! ([`CONSUMER_DEDUP_MIGRATION`]) is the shape the runner applies.
//!
//! ## The same-transaction co-commit — DELIVERED, no longer a floor (peer-review #7 / MR-023b)
//! The real `INSERT … ON CONFLICT DO NOTHING` against the Storage pool is now executed in the SAME
//! transaction as the handler's state write (so the dedup mark and the side effect commit together
//! — the atomicity that makes idempotency real, not best-effort). This USED TO BE a named floor
//! ("lands when the consumer runtime threads a tx into the handler"); it is now the shipped seam:
//! [`DedupLedger::begin_co_commit`] opens ONE transaction, INSERTs the mark within it, and hands the
//! transaction-bound connection to the handler (via [`crate::HandlerTx`]) so the effect co-commits;
//! the [`crate::consumer::Consumer`] runtime commits on `Done` / rolls back on `Retry`/failure. A
//! crash before commit leaves NEITHER the mark NOR the effect → a redelivery re-runs
//! (exactly-once-with-effect), closing the old at-most-once floor. The durable impl is
//! `myelin_storage::events_durable::DurableDedupBacking::begin_co_commit`. The seam shape (the
//! `(consumer, event_id)` key, the `mark_handled` primitive) does NOT change.

use crate::ConsumerName;
// `HashSet` stays ungated: it appears in `mem()`'s always-compiled return type. `Mutex` is used
// ONLY by the `test-support`-gated in-memory `Memory` arm + its `Default`, so it is gated too
// (MR-009b Wave 3 — the production ledger is the pool-backed `Durable` arm).
use std::collections::HashSet;
use std::sync::Arc;
#[cfg(any(test, feature = "test-support"))]
use std::sync::Mutex;

/// The frozen forward-only DDL for the `consumer_dedup` ledger (contract 2.5). This is the shape
/// the migration runner (P-S15) applies when the OLTP tier client (P-007) is wired; the in-memory
/// [`DedupLedger`] models exactly these semantics until then.
///
/// - `(consumer, event_id)` is the **PRIMARY KEY** — the idempotency key is per-consumer, so two
///   distinct consumers of the same event each process it exactly once, and a redelivery to the
///   SAME consumer is suppressed (`ON CONFLICT DO NOTHING`);
/// - `recorded_at` is when the consumer durably marked the event handled (read in the SAME
///   transaction as the handler's state write — the atomicity floor named in the module docs).
///
/// **Forward-only** (the `forward-only-migration` lint, P-S11): this is an `expand` migration (it
/// only adds the table); there is no destructive down-migration.
pub const CONSUMER_DEDUP_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS consumer_dedup (
    consumer    TEXT        NOT NULL,
    event_id    TEXT        NOT NULL,
    recorded_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT consumer_dedup_pk PRIMARY KEY (consumer, event_id)
);";

/// **Why a co-commit transaction failed (peer-review #7 / MR-023b).** Surfaced (never swallowed):
/// a failed commit means the dedup mark + the handler's effect did NOT land, so the consumer runtime
/// treats it like a `Retry` (do NOT ack — a redelivery re-runs; 0 lost).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoCommitError(pub String);

impl core::fmt::Display for CoCommitError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "consumer co-commit failed: {}", self.0)
    }
}

impl std::error::Error for CoCommitError {}

/// **The same-transaction co-commit handle (peer-review #7 / MR-023b — the atomicity that makes
/// idempotency real, not best-effort).** The consumer runtime opens ONE transaction via
/// [`DedupLedger::begin_co_commit`], which INSERTs the `(consumer, event_id)` dedup mark WITHIN it
/// (the freshness check) and returns this handle; the runtime hands the handler the
/// transaction-bound connection (via [`crate::HandlerTx`]) so the handler's durable state write runs
/// on the SAME transaction, then the runtime [`commit`](CoCommitTx::commit)s (mark + effect land
/// together) on `Done` or [`rollback`](CoCommitTx::rollback)s (both vanish) on `Retry`/failure. A
/// crash before commit leaves NEITHER, so a redelivery re-runs — exactly-once-with-effect, never a
/// committed mark with a lost effect (the old at-most-once floor).
///
/// The handle is boxed + `Send` so the sync consumer runtime can move it across the outcome match.
/// `myelin-events` is a §2.9 DAG SINK, so the connection is type-erased behind `&mut dyn Any`; the
/// durable PG impl lives in `myelin_storage::events_durable`.
pub trait CoCommitTx: Send {
    /// The type-erased, transaction-bound DB connection the handler runs its writes on — the SAME
    /// transaction the dedup mark is in. `None` on the in-memory model backend (no shared-DB write).
    fn connection(&mut self) -> Option<&mut dyn core::any::Any>;
    /// Commit — the dedup mark + the handler's writes become durable together (`Done`).
    fn commit(self: Box<Self>) -> Result<(), CoCommitError>;
    /// Roll back — the dedup mark + the handler's writes vanish so a redelivery re-runs (`Retry` /
    /// a `Deduplicated` no-op where nothing needs to persist).
    fn rollback(self: Box<Self>);
}

/// **The durable backing seam for the `consumer_dedup` ledger (SI-023, MR-023).** A real
/// `(consumer, event_id)` PK table over the OLTP pool implements this so consumer idempotency
/// **survives a process restart**: a redelivered event after a restart is still deduped because the
/// mark lives in Postgres, not a per-process `HashSet`. The verbs mirror the in-memory ledger's
/// (the `INSERT … ON CONFLICT DO NOTHING` primitive + the read/revert/forget verbs). The trait is
/// SYNC to match the consumer runtime's sync `mark_handled` call site; a PG impl bridges to async
/// internally (`block_in_place` + `block_on`), the same bridge [`crate::nats::NatsJetStreamBus`]
/// uses (the production impl is `myelin_storage::events_durable::DurableDedupBacking`).
///
/// **The same-tx atomicity is now DELIVERED, not a floor (peer-review #7 / MR-023b — FIXED):**
/// [`begin_co_commit`](DurableDedup::begin_co_commit) opens ONE transaction, sets the `(tenant,
/// region)` RLS scope, INSERTs the dedup mark within it, and returns a [`CoCommitTx`] the consumer
/// runtime commits AFTER the handler's write co-commits (or rolls back on failure). So a rolled-back
/// handler rolls back its dedup mark for free — the in-the-SAME-transaction-as-the-handler's-
/// state-write co-commit the module docs used to name only as a floor. Fail-direction: when a
/// durable [`DurableDedup::mark_handled`] / [`begin_co_commit`](DurableDedup::begin_co_commit)
/// cannot reach the DB it MUST report FRESH (run the handler), never a silent "already handled"
/// (which would be a skipped → lost event) — effectively-once degrades to at-least-once under a DB
/// outage, never to data loss.
pub trait DurableDedup: Send + Sync {
    /// `INSERT (consumer, event_id) ON CONFLICT DO NOTHING` → `true` iff freshly inserted.
    fn mark_handled(&self, consumer: &ConsumerName, event_id: &crate::EventId) -> bool;
    /// Read-only presence check.
    fn is_handled(&self, consumer: &ConsumerName, event_id: &crate::EventId) -> bool;
    /// Remove the pair (a `Retry` reverts its speculative mark so a redelivery re-runs).
    fn revert(&self, consumer: &ConsumerName, event_id: &crate::EventId);
    /// Forget the pair (the reindex-after-wipe path); `true` iff a mark was removed.
    fn forget(&self, consumer: &ConsumerName, event_id: &crate::EventId) -> bool;
    /// **Open the same-transaction co-commit (peer-review #7 / MR-023b).** Acquire a connection,
    /// BEGIN, set the `(tenant, region)` GUC transaction-scoped (RLS, the `with_tenant_tx`
    /// convention), `INSERT (consumer, event_id) ON CONFLICT DO NOTHING` WITHIN the transaction, and
    /// return `(handle, fresh)`: `fresh == true` iff the mark was newly inserted (run the handler on
    /// this tx). The runtime commits the handle on `Done` (mark + effect together) or rolls it back
    /// on `Retry`/failure (both vanish). Fail-direction: on a DB error report FRESH with a no-op
    /// handle (run the handler, at-least-once), NEVER a silent "already handled".
    fn begin_co_commit(
        &self,
        consumer: &ConsumerName,
        event_id: &crate::EventId,
        tenant: &crate::TenantId,
        region: &crate::Region,
    ) -> (Box<dyn CoCommitTx>, bool);
}

/// The dedup-ledger backend (MR-023, the MR-007/008 backend-enum pattern). `Memory` is the
/// always-compiled in-memory **TEST-DOUBLE** (the model the unit suite + the in-process SUB-D1/D2
/// drills run against); `Durable` is the production PG-backed seam the events `serve()` composition
/// root (`myelin_storage::events_serve`) wires. `Memory` stays the default so the DB-free default
/// build is unchanged; the `no-in-memory-durable-store` scanner FOLLOWS this enum and still fires
/// on the `Memory` variant (still the default) — the baseline entry is supplemented, not removed,
/// until production wires `Durable` as the non-optional default (the MR-007/008 status).
#[derive(Clone)]
enum DedupBackend {
    /// The in-memory model / test-double: `(consumer, event_id)` set behind a shared lock.
    /// **MR-009b Wave 3 — TEST DOUBLE (compiled ONLY under `#[cfg(any(test, feature = "test-support"))]`).**
    /// NOT the production system-of-record: the production-compiled enum presents ONLY the pool-backed
    /// `Durable` variant (the `no-in-memory-durable-store` scanner strips this `test-support`-gated
    /// `Memory` arm as a test double), so `DedupLedger` no longer holds an in-memory collection in the
    /// production graph (SI-023 leaves the baseline).
    #[cfg(any(test, feature = "test-support"))]
    Memory(Arc<Mutex<HashSet<(ConsumerName, crate::EventId)>>>),
    /// The durable PG-backed seam (production): a `(consumer, event_id)` table that survives restart.
    /// **The PRODUCTION DEFAULT (MR-009b Wave 3) — always compiled.**
    Durable(Arc<dyn DurableDedup>),
}

/// The default backend is the in-memory TEST DOUBLE — compiled ONLY under
/// `#[cfg(any(test, feature = "test-support"))]` (MR-009b Wave 3). Production constructs the
/// `Durable` backing through [`DedupLedger::durable`] (the events `serve()` composition root).
#[cfg(any(test, feature = "test-support"))]
impl Default for DedupBackend {
    fn default() -> Self {
        DedupBackend::Memory(Arc::new(Mutex::new(HashSet::new())))
    }
}

/// **The in-memory co-commit handle (peer-review #7 / MR-023b) — TEST DOUBLE.** Models the
/// same-transaction atomicity of the durable path against the in-memory `HashSet`: `begin_co_commit`
/// already inserted the `(consumer, event_id)` mark; `commit` KEEPS it (the effect "landed"),
/// `rollback` REMOVES it iff this call inserted it (a `Retry`/failure re-runs on redelivery — this
/// IS the old `revert`, now structural). Carries no connection (an in-memory handler writes to its
/// own state, not a shared DB). Compiled only under `#[cfg(any(test, feature = "test-support"))]`.
#[cfg(any(test, feature = "test-support"))]
struct MemoryCoCommit {
    set: Arc<Mutex<HashSet<(ConsumerName, crate::EventId)>>>,
    key: (ConsumerName, crate::EventId),
    /// Whether THIS begin inserted the mark (fresh). A rollback only removes a mark it inserted, so
    /// rolling back a duplicate delivery never disturbs the pre-existing mark.
    inserted: bool,
}

#[cfg(any(test, feature = "test-support"))]
impl CoCommitTx for MemoryCoCommit {
    fn connection(&mut self) -> Option<&mut dyn core::any::Any> {
        None
    }
    fn commit(self: Box<Self>) -> Result<(), CoCommitError> {
        // Keep the mark — the (in-memory) effect and the mark "co-commit". Nothing to persist.
        Ok(())
    }
    fn rollback(self: Box<Self>) {
        if self.inserted {
            self.set
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&self.key);
        }
    }
}

/// The per-consumer `consumer_dedup` ledger (contract 2.5). `(consumer, event_id)` is the PK:
/// [`DedupLedger::mark_handled`] records the pair and returns whether it was FRESH (newly inserted)
/// or a DUPLICATE (already present — the idempotency no-op). A cloneable handle so a reconnected
/// `Consumer` re-bound by the same name re-uses the SAME ledger (consumer-template rule 4) and the
/// redelivery is absorbed.
///
/// **Backend (MR-023):** [`DedupLedger::new`] is the in-memory test-double; [`DedupLedger::durable`]
/// binds a PG-backed [`DurableDedup`] so idempotency survives a process restart (SI-023). The public
/// API (`mark_handled`/`is_handled`/`revert`/`forget`) is identical on both backends — the consumer
/// runtime calls the same methods regardless.
///
/// This is **the effectively-once anchor**: every consumer's at-least-once delivery resolves to
/// exactly-once-in-effect through this ledger (Bus §4.2).
#[derive(Clone)]
pub struct DedupLedger {
    backend: DedupBackend,
}

/// The `Default` ledger is the in-memory TEST DOUBLE — `#[cfg(any(test, feature = "test-support"))]`
/// only (MR-009b Wave 3). Production builds the durable ledger through [`DedupLedger::durable`].
#[cfg(any(test, feature = "test-support"))]
impl Default for DedupLedger {
    fn default() -> Self {
        DedupLedger {
            backend: DedupBackend::default(),
        }
    }
}

impl DedupLedger {
    /// A fresh, empty IN-MEMORY ledger (the test-double / in-process-drill default).
    /// **MR-009b Wave 3 — TEST DOUBLE (compiled ONLY under `#[cfg(any(test, feature = "test-support"))]`).**
    /// The PRODUCTION constructor is [`DedupLedger::durable`] (the events `serve()` composition root
    /// wires the PG-backed `consumer_dedup` table); this `::new` is the DB-free unit-test entry point
    /// downstream crates reach via the `myelin-events/test-support` dev-dependency.
    #[cfg(any(test, feature = "test-support"))]
    pub fn new() -> Self {
        Self::default()
    }

    /// **MR-023: bind the ledger to a DURABLE PG backing** so consumer idempotency survives a
    /// process restart (a redelivered event after a restart is still deduped — SI-023). The events
    /// `serve()` composition root (`myelin_storage::events_serve::EventsRuntime`) constructs this
    /// with the PG-backed `consumer_dedup` table; [`DedupLedger::new`] stays the in-memory default.
    pub fn durable(backing: Arc<dyn DurableDedup>) -> Self {
        DedupLedger {
            backend: DedupBackend::Durable(backing),
        }
    }

    /// Lock the in-memory set (only valid on the `Memory` backend; the `Durable` backend routes
    /// straight to its trait, never here).
    fn mem(&self) -> Option<std::sync::MutexGuard<'_, HashSet<(ConsumerName, crate::EventId)>>> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            DedupBackend::Memory(inner) => Some(inner.lock().unwrap_or_else(|e| e.into_inner())),
            DedupBackend::Durable(_) => None,
        }
    }

    /// Record `(consumer, event_id)` and report whether it was FRESH. This is the `INSERT …
    /// ON CONFLICT DO NOTHING` model: a fresh pair returns `true` (the handler should run); a pair
    /// already present returns `false` (a redelivery — the handler is SKIPped, the message is
    /// acked, 0 dup). The PK is the pair, so the SAME event delivered to two DIFFERENT consumers is
    /// fresh for each. On the `Durable` backend a restart-surviving PG row is the dedup state.
    pub fn mark_handled(&self, consumer: &ConsumerName, event_id: &crate::EventId) -> bool {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            DedupBackend::Memory(inner) => inner
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert((consumer.clone(), event_id.clone())),
            DedupBackend::Durable(d) => d.mark_handled(consumer, event_id),
        }
    }

    /// **Open the same-transaction co-commit for one delivery (peer-review #7 / MR-023b — FIXED).**
    /// Returns `(handle, fresh)`: the handle carries a transaction with the `(consumer, event_id)`
    /// dedup mark already INSERTed within it (not yet committed), and `fresh == true` iff the mark
    /// was newly inserted (the handler should run on this tx). The consumer runtime commits the
    /// handle on `Done` (the dedup mark + the handler's durable effect land together) or rolls it
    /// back on `Retry`/failure (both vanish, so a redelivery re-runs — no lost effect). On the
    /// in-memory backend the handle models this atomicity: commit keeps the mark, rollback removes
    /// it (the old `revert`, now structural). `tenant`/`region` scope the durable tx's RLS (ignored
    /// in memory).
    pub fn begin_co_commit(
        &self,
        consumer: &ConsumerName,
        event_id: &crate::EventId,
        tenant: &crate::TenantId,
        region: &crate::Region,
    ) -> (Box<dyn CoCommitTx>, bool) {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            DedupBackend::Memory(inner) => {
                let key = (consumer.clone(), event_id.clone());
                // The `INSERT … ON CONFLICT DO NOTHING` model: `insert` returns `true` iff the pair
                // was newly added (FRESH). A duplicate leaves the existing mark untouched.
                let fresh = inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(key.clone());
                (
                    Box::new(MemoryCoCommit {
                        set: Arc::clone(inner),
                        key,
                        inserted: fresh,
                    }),
                    fresh,
                )
            }
            DedupBackend::Durable(d) => d.begin_co_commit(consumer, event_id, tenant, region),
        }
    }

    /// Has `(consumer, event_id)` already been handled? (Read-only check; `mark_handled` is the
    /// transactional one.)
    pub fn is_handled(&self, consumer: &ConsumerName, event_id: &crate::EventId) -> bool {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            DedupBackend::Memory(inner) => inner
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains(&(consumer.clone(), event_id.clone())),
            DedupBackend::Durable(d) => d.is_handled(consumer, event_id),
        }
    }

    /// Remove `(consumer, event_id)` from the ledger. A consumer-template `Retry` is NOT a
    /// completed handle, so the runtime reverts the mark a delivery speculatively took — a later
    /// redelivery must re-run the handler (else a transient failure would be permanently swallowed:
    /// silent data loss). With the #7/MR-023b co-commit the runtime's `Retry` path rolls back the
    /// whole co-commit transaction (mark + effect together) rather than calling this — so this verb
    /// is now the standalone mirror for the reindex / manual paths.
    ///
    /// **Superseded internally by [`DedupLedger::begin_co_commit`] (#7/MR-023b):** the consumer
    /// runtime no longer speculatively marks-then-reverts — a `Retry` rolls back the co-commit
    /// transaction (which removes the mark atomically with the handler's effect). This verb is kept
    /// as the ledger's explicit mirror of [`DurableDedup::revert`] (the reindex / manual paths).
    pub fn revert(&self, consumer: &ConsumerName, event_id: &crate::EventId) {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            DedupBackend::Memory(inner) => {
                inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&(consumer.clone(), event_id.clone()));
            }
            DedupBackend::Durable(d) => d.revert(consumer, event_id),
        }
    }

    /// **Forget `(consumer, event_id)` so a later delivery re-runs the handler (the reindex-after-wipe
    /// path).** A FULL `reindex(scope)` WIPES the derived read-model and rebuilds it from source by
    /// re-driving the owner's `*.snapshot` events through the SAME live consumer. Those snapshots carry
    /// DETERMINISTIC ids ([`crate::snapshot_event_id`]); if a prior rebuild already marked them handled,
    /// the redelivery would be deduplicated and the wiped store would stay EMPTY. Forgetting the
    /// snapshot's mark for the scope being rebuilt lets the cold rebuild re-apply it (the within-pass
    /// idempotency is then the consumer's OWN write-time collapse, e.g. the inbox `(tenant, recipient,
    /// dedup_key)` UPSERT). This is the dedup-ledger analog of `reindex`'s cursor-store `reset_scope`
    /// (SRCH-P16): a full rebuild resets the applied guard for the generation it re-emits. In the OLTP
    /// binding this is a scoped `DELETE FROM consumer_dedup WHERE consumer = $1 AND event_id = $2`
    /// (forward-only; the snapshot id re-applies idempotently into the wiped store). Returns `true` iff
    /// a mark was present and removed.
    pub fn forget(&self, consumer: &ConsumerName, event_id: &crate::EventId) -> bool {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            DedupBackend::Memory(inner) => inner
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&(consumer.clone(), event_id.clone())),
            DedupBackend::Durable(d) => d.forget(consumer, event_id),
        }
    }

    /// How many `(consumer, event_id)` pairs the IN-MEMORY ledger holds (a depth read for tests /
    /// the in-process model). On the `Durable` backend the count is a DB query not exposed through
    /// this introspection helper — it returns the in-process view (`0`); production correctness
    /// rests on `mark_handled`/`is_handled`, not on this helper.
    pub fn len(&self) -> usize {
        self.mem().map(|s| s.len()).unwrap_or(0)
    }

    /// Whether the IN-MEMORY ledger is empty (see [`DedupLedger::len`] for the durable caveat).
    pub fn is_empty(&self) -> bool {
        self.mem().map(|s| s.is_empty()).unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EventId;

    fn consumer(name: &str) -> ConsumerName {
        ConsumerName(name.into())
    }

    // --- Unit: idempotent re-delivery (the one effect on double-delivery) ---

    /// **Idempotent re-delivery.** The SAME `(consumer, event_id)` inserted twice yields ONE
    /// recorded pair: the first `mark_handled` is FRESH (the handler should run), the second is a
    /// DUPLICATE (the handler is SKIPped). This is the `ON CONFLICT DO NOTHING` property — the
    /// effectively-once anchor (Bus §4.2). Greened transitively by SUB-D2 in EB-05; this is the
    /// structural unit proof EB-06's DoD names.
    #[test]
    fn same_consumer_event_inserted_twice_is_one_effect() {
        let ledger = DedupLedger::new();
        let c = consumer("indexer");
        let id = EventId("01J-1".into());

        assert!(
            ledger.mark_handled(&c, &id),
            "first delivery is FRESH → the handler runs"
        );
        assert!(
            !ledger.mark_handled(&c, &id),
            "redelivery is a DUPLICATE → the handler is skipped"
        );
        assert!(
            !ledger.mark_handled(&c, &id),
            "and again — still a duplicate"
        );
        assert_eq!(
            ledger.len(),
            1,
            "exactly ONE (consumer, event_id) pair recorded"
        );
        assert!(ledger.is_handled(&c, &id), "the pair is durably handled");
    }

    /// Two DISTINCT consumers each record the SAME `event_id` independently — the consumer
    /// dimension of the `(consumer, event_id)` PK. Each is fresh for its own consumer (each
    /// processes the event once); a redelivery to either is a duplicate. The key is per-consumer,
    /// not global.
    #[test]
    fn two_consumers_record_the_same_event_independently() {
        let ledger = DedupLedger::new();
        let a = consumer("indexer");
        let b = consumer("notifier");
        let id = EventId("01J-1".into());

        assert!(ledger.mark_handled(&a, &id), "fresh for consumer A");
        assert!(
            ledger.mark_handled(&b, &id),
            "ALSO fresh for consumer B (different PK)"
        );
        // and a redelivery to either is now a duplicate.
        assert!(
            !ledger.mark_handled(&a, &id),
            "redelivery to A is a duplicate"
        );
        assert!(
            !ledger.mark_handled(&b, &id),
            "redelivery to B is a duplicate"
        );
        assert_eq!(ledger.len(), 2, "two distinct (consumer, event_id) pairs");
        // each consumer sees only its OWN handled state.
        assert!(ledger.is_handled(&a, &id) && ledger.is_handled(&b, &id));
        assert!(
            !ledger.is_handled(&consumer("other"), &id),
            "a third consumer has not handled it"
        );
    }

    /// `is_empty` / `is_handled` track state precisely: empty before any mark, non-empty +
    /// `is_handled` true for the exact pair after; and a `revert` (a consumer-template `Retry`
    /// rolling back its speculative mark) returns the pair to unhandled so a redelivery re-runs.
    #[test]
    fn is_empty_is_handled_and_revert_track_state() {
        let ledger = DedupLedger::new();
        let c = consumer("indexer");
        let id = EventId("01J-1".into());
        assert!(ledger.is_empty(), "a fresh ledger is empty");
        assert!(!ledger.is_handled(&c, &id), "nothing handled yet");

        assert!(ledger.mark_handled(&c, &id), "first mark is fresh");
        assert!(!ledger.is_empty(), "no longer empty after a mark");
        assert!(ledger.is_handled(&c, &id), "the exact pair is handled");

        // a Retry reverts the speculative mark → unhandled again (a redelivery must re-run).
        ledger.revert(&c, &id);
        assert!(
            !ledger.is_handled(&c, &id),
            "after revert the pair is unhandled (a retry re-runs)"
        );
        assert!(ledger.is_empty(), "revert removed the only pair");
    }

    /// **`forget` returns whether a mark was actually removed (the reindex-after-wipe path).** The
    /// boolean is load-bearing: a FULL reindex forgets the snapshot's mark for the scope so the cold
    /// rebuild re-applies it. Forgetting a PRESENT mark returns `true` (and the pair is gone so a
    /// redelivery re-runs the handler); forgetting an ABSENT mark returns `false` (nothing to do).
    /// Pins the `true`/`false` return (a constant-`true` mutant would falsely claim a wipe happened
    /// on an empty scope; a constant-`false` would hide a real forget). (P-507 mutation gate.)
    #[test]
    fn forget_returns_whether_a_mark_was_removed_and_re_runs_the_handler() {
        let ledger = DedupLedger::new();
        let c = consumer("indexer");
        let id = EventId("01J-snapshot".into());

        // forgetting an ABSENT mark is a no-op → false.
        assert!(
            !ledger.forget(&c, &id),
            "forgetting a mark that was never present returns false"
        );

        // mark it handled, then forget it: a PRESENT mark → true, and the pair is gone.
        assert!(ledger.mark_handled(&c, &id), "first delivery is fresh");
        assert!(ledger.is_handled(&c, &id), "the pair is handled");
        assert!(
            ledger.forget(&c, &id),
            "forgetting a present mark returns true"
        );
        assert!(
            !ledger.is_handled(&c, &id),
            "after forget the pair is unhandled — the cold rebuild re-applies the snapshot"
        );
        // forgetting again (now absent) is false.
        assert!(
            !ledger.forget(&c, &id),
            "a second forget of the now-absent mark returns false"
        );
        // and a re-delivery after forget is FRESH again (the handler re-runs).
        assert!(
            ledger.mark_handled(&c, &id),
            "redelivery after forget is fresh → the handler re-runs (reindex-after-wipe)"
        );
    }

    /// The 2.5 migration is the frozen shape: the `(consumer, event_id)` PK + the columns are
    /// present; forward-only (no destructive DROP).
    #[test]
    fn migration_is_the_frozen_2_5_shape() {
        assert!(CONSUMER_DEDUP_MIGRATION.contains("CREATE TABLE IF NOT EXISTS consumer_dedup"));
        assert!(CONSUMER_DEDUP_MIGRATION.contains("PRIMARY KEY (consumer, event_id)"));
        for col in ["consumer", "event_id", "recorded_at"] {
            assert!(
                CONSUMER_DEDUP_MIGRATION.contains(col),
                "missing column {col}"
            );
        }
        assert!(
            !CONSUMER_DEDUP_MIGRATION.contains("DROP TABLE"),
            "forward-only: no destructive down"
        );
    }
}
