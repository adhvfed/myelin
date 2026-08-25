use myelin_events::{ConsumerName, DedupLedger, EventId, CONSUMER_DEDUP_MIGRATION};

fn provider_record_handled(
    ledger: &DedupLedger,
    consumer: &ConsumerName,
    event_id: &EventId,
) -> bool {
    ledger
        .mark_handled(consumer, event_id)
        .expect("in-memory dedup storage is available")
}

fn consumer_should_skip(ledger: &DedupLedger, consumer: &ConsumerName, event_id: &EventId) -> bool {
    ledger
        .is_handled(consumer, event_id)
        .expect("in-memory dedup storage is available")
}

#[test]
fn cdc_2_5_same_pair_inserted_twice_is_one_effect() {
    let ledger = DedupLedger::new();
    let consumer = ConsumerName("indexer".into());
    let id = EventId("01J-1".into());

    assert!(
        !consumer_should_skip(&ledger, &consumer, &id),
        "nothing handled yet"
    );

    assert!(
        provider_record_handled(&ledger, &consumer, &id),
        "first handle is FRESH"
    );

    assert!(
        consumer_should_skip(&ledger, &consumer, &id),
        "the pair is now handled → skip redelivery"
    );

    assert!(
        !provider_record_handled(&ledger, &consumer, &id),
        "redelivery is a DUPLICATE"
    );
    assert_eq!(
        ledger.len(),
        1,
        "exactly ONE (consumer, event_id) row - one effect"
    );
}

#[test]
fn cdc_2_5_two_consumers_record_the_same_event_independently() {
    let ledger = DedupLedger::new();
    let a = ConsumerName("indexer".into());
    let b = ConsumerName("notifier".into());
    let id = EventId("01J-1".into());

    assert!(
        provider_record_handled(&ledger, &a, &id),
        "fresh for consumer A"
    );
    assert!(
        provider_record_handled(&ledger, &b, &id),
        "ALSO fresh for consumer B (different PK)"
    );

    assert!(
        !provider_record_handled(&ledger, &a, &id),
        "redelivery to A is a duplicate"
    );
    assert!(
        !provider_record_handled(&ledger, &b, &id),
        "redelivery to B is a duplicate"
    );
    assert!(
        !consumer_should_skip(&ledger, &ConsumerName("auditor".into()), &id),
        "C has not handled it"
    );
    assert_eq!(ledger.len(), 2, "two distinct (consumer, event_id) rows");
}

#[test]
fn cdc_2_5_migration_is_the_frozen_pk_shape() {
    assert!(CONSUMER_DEDUP_MIGRATION.contains("CREATE TABLE IF NOT EXISTS consumer_dedup"));
    assert!(
        CONSUMER_DEDUP_MIGRATION.contains("PRIMARY KEY (consumer, event_id)"),
        "the PK is the (consumer, event_id) PAIR - the consumer dimension is contractual"
    );
    for col in ["consumer", "event_id", "recorded_at"] {
        assert!(
            CONSUMER_DEDUP_MIGRATION.contains(col),
            "missing contractual column {col}"
        );
    }
    assert!(
        !CONSUMER_DEDUP_MIGRATION.contains("DROP TABLE"),
        "forward-only: no destructive down"
    );
}
