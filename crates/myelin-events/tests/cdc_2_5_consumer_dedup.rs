//! # The CDC pair for contract 2.5 — the `consumer_dedup` ledger (EB-06 → P-015)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 2.5
//! (`consumer_dedup` ledger — `(consumer, event_id)` PK, presence == "already handled"). Owning
//! architecture: `event-bus.md` §3.3 + §4.2 (at-least-once + idempotent ≈ effectively-once).
//!
//! ## The contract this pair pins (the effectively-once anchor)
//! The `consumer_dedup` ledger is the seam between the side that **records** an event as handled
//! (the PROVIDER of the dedup fact) and the side that **reads** that fact to suppress a redelivery
//! (the CONSUMER of it). The frozen shape both sides agree on:
//!
//! - the key is the **pair** `(consumer, event_id)` (the PK) — not the `event_id` alone;
//! - `mark_handled(consumer, event_id)` is `INSERT … ON CONFLICT DO NOTHING`: it returns FRESH
//!   (`true`) the first time the pair is seen and DUPLICATE (`false`) thereafter;
//! - presence (`is_handled`) means "already handled" — the redelivery is a no-op.
//!
//! This is the **dedicated 2.5 pair** EB-06's DEFINITION OF DONE names (the combined end-to-end
//! 2.4/2.5 relay→consumer pair stays in `drills_sub_d2_consumer.rs::cdc_2_4_2_5_*`). The dedup
//! property is greened transitively by SUB-D2 in EB-05/P-009; this pair pins the ledger contract
//! ITSELF, independent of the runtime, so a future SQL `consumer_dedup` implementation has the
//! frozen behaviour to conform to.

use myelin_events::{ConsumerName, DedupLedger, EventId, CONSUMER_DEDUP_MIGRATION};

/// The PROVIDER side of contract 2.5: the side that handled an event records the
/// `(consumer, event_id)` dedup fact, returning whether the handle was the FRESH one (it should
/// run the effect) or a DUPLICATE (a redelivery — skip). This is the `INSERT … ON CONFLICT DO
/// NOTHING` primitive.
fn provider_record_handled(ledger: &DedupLedger, consumer: &ConsumerName, event_id: &EventId) -> bool {
    ledger.mark_handled(consumer, event_id)
}

/// The CONSUMER side of contract 2.5: the side resolving a (re)delivery reads the ledger's
/// presence to decide whether to run the handler or skip. Presence == "already handled".
fn consumer_should_skip(ledger: &DedupLedger, consumer: &ConsumerName, event_id: &EventId) -> bool {
    ledger.is_handled(consumer, event_id)
}

/// **CDC 2.5 — idempotent re-delivery: one effect on a double-delivery.** Provider records a
/// handle (FRESH); the consumer, on the SAME (consumer, event_id), reads "already handled" and a
/// second provider record is a DUPLICATE. The contract's core promise: the same pair inserted
/// twice yields ONE recorded row and the handler runs once in effect (the effectively-once
/// anchor, Bus §4.2).
#[test]
fn cdc_2_5_same_pair_inserted_twice_is_one_effect() {
    let ledger = DedupLedger::new();
    let consumer = ConsumerName("indexer".into());
    let id = EventId("01J-1".into());

    // Before any handle, the consumer side sees "not handled" → it WOULD run the handler.
    assert!(!consumer_should_skip(&ledger, &consumer, &id), "nothing handled yet");

    // Provider records the first handle: FRESH → the effect runs exactly this once.
    assert!(provider_record_handled(&ledger, &consumer, &id), "first handle is FRESH");

    // The consumer side now reads "already handled" → it skips on a redelivery.
    assert!(consumer_should_skip(&ledger, &consumer, &id), "the pair is now handled → skip redelivery");

    // A second provider record of the SAME pair is a DUPLICATE (ON CONFLICT DO NOTHING).
    assert!(!provider_record_handled(&ledger, &consumer, &id), "redelivery is a DUPLICATE");
    assert_eq!(ledger.len(), 1, "exactly ONE (consumer, event_id) row — one effect");
}

/// **CDC 2.5 — the consumer dimension of the PK.** The SAME `event_id` recorded by TWO distinct
/// consumers is fresh for EACH (each runs its own effect once); a redelivery to either is a
/// duplicate. The key is the PAIR, not the event alone — so two consumers of the same event each
/// process it exactly once.
#[test]
fn cdc_2_5_two_consumers_record_the_same_event_independently() {
    let ledger = DedupLedger::new();
    let a = ConsumerName("indexer".into());
    let b = ConsumerName("notifier".into());
    let id = EventId("01J-1".into());

    // Each consumer's first handle of the shared event is FRESH (different PK).
    assert!(provider_record_handled(&ledger, &a, &id), "fresh for consumer A");
    assert!(provider_record_handled(&ledger, &b, &id), "ALSO fresh for consumer B (different PK)");

    // A redelivery to either consumer is a duplicate; a THIRD consumer has not handled it.
    assert!(!provider_record_handled(&ledger, &a, &id), "redelivery to A is a duplicate");
    assert!(!provider_record_handled(&ledger, &b, &id), "redelivery to B is a duplicate");
    assert!(!consumer_should_skip(&ledger, &ConsumerName("auditor".into()), &id), "C has not handled it");
    assert_eq!(ledger.len(), 2, "two distinct (consumer, event_id) rows");
}

/// **CDC 2.5 — the frozen DDL is the shape both sides agree on.** The migration the SQL
/// `consumer_dedup` implementation will apply carries the exact `(consumer, event_id)` PK + the
/// `recorded_at` column, and is forward-only (no destructive down). This pins the wire/storage
/// shape so the future Postgres ledger (P-007/P-S15) conforms to the same contract the in-memory
/// model proves above.
#[test]
fn cdc_2_5_migration_is_the_frozen_pk_shape() {
    assert!(CONSUMER_DEDUP_MIGRATION.contains("CREATE TABLE IF NOT EXISTS consumer_dedup"));
    assert!(
        CONSUMER_DEDUP_MIGRATION.contains("PRIMARY KEY (consumer, event_id)"),
        "the PK is the (consumer, event_id) PAIR — the consumer dimension is contractual"
    );
    for col in ["consumer", "event_id", "recorded_at"] {
        assert!(CONSUMER_DEDUP_MIGRATION.contains(col), "missing contractual column {col}");
    }
    assert!(!CONSUMER_DEDUP_MIGRATION.contains("DROP TABLE"), "forward-only: no destructive down");
}
