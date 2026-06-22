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
//! ([`CONSUMER_DEDUP_MIGRATION`]) is the shape the runner applies. **Floor:** the real
//! `INSERT … ON CONFLICT DO NOTHING` against the Storage pool, executed in the SAME transaction as
//! the handler's state write (so the dedup mark and the side effect commit together — the
//! atomicity that makes idempotency real, not best-effort), lands when the OLTP client is wired
//! (P-007) + the consumer runtime runs inside `serve` (**P-S12**). The seam shape (the
//! `(consumer, event_id)` key, the `mark_handled` primitive) does NOT change.

use crate::ConsumerName;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

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

/// The per-consumer `consumer_dedup` ledger (contract 2.5, the in-memory model). `(consumer,
/// event_id)` is the PK: [`DedupLedger::mark_handled`] records the pair and returns whether it was
/// FRESH (newly inserted) or a DUPLICATE (already present — the idempotency no-op). A cloneable
/// handle over shared state so a reconnected `Consumer` re-bound by the same name re-uses the SAME
/// ledger (consumer-template rule 4) and the redelivery is absorbed.
///
/// This is **the effectively-once anchor**: every consumer's at-least-once delivery resolves to
/// exactly-once-in-effect through this ledger (Bus §4.2).
#[derive(Clone, Default)]
pub struct DedupLedger {
    inner: Arc<Mutex<HashSet<(ConsumerName, crate::EventId)>>>,
}

impl DedupLedger {
    /// A fresh, empty ledger.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record `(consumer, event_id)` and report whether it was FRESH. This is the `INSERT …
    /// ON CONFLICT DO NOTHING` model: a fresh pair returns `true` (the handler should run); a pair
    /// already present returns `false` (a redelivery — the handler is SKIPped, the message is
    /// acked, 0 dup). The PK is the pair, so the SAME event delivered to two DIFFERENT consumers is
    /// fresh for each.
    pub fn mark_handled(&self, consumer: &ConsumerName, event_id: &crate::EventId) -> bool {
        let mut set = self.lock();
        set.insert((consumer.clone(), event_id.clone()))
    }

    /// Has `(consumer, event_id)` already been handled? (Read-only check; `mark_handled` is the
    /// transactional one.)
    pub fn is_handled(&self, consumer: &ConsumerName, event_id: &crate::EventId) -> bool {
        self.lock().contains(&(consumer.clone(), event_id.clone()))
    }

    /// Remove `(consumer, event_id)` from the ledger. A consumer-template `Retry` is NOT a
    /// completed handle, so the runtime reverts the mark a delivery speculatively took — a later
    /// redelivery must re-run the handler (else a transient failure would be permanently swallowed:
    /// silent data loss). The real `consumer_dedup` row is written in the SAME transaction as the
    /// handler's state write (P-007/P-S12), so a rolled-back handler rolls back its dedup mark for
    /// free — this models that atomicity. Crate-internal: only the runtime calls it.
    pub(crate) fn revert(&self, consumer: &ConsumerName, event_id: &crate::EventId) {
        self.lock().remove(&(consumer.clone(), event_id.clone()));
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
        self.lock().remove(&(consumer.clone(), event_id.clone()))
    }

    /// How many `(consumer, event_id)` pairs the ledger holds (for tests / a depth read).
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether the ledger is empty.
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashSet<(ConsumerName, crate::EventId)>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
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
