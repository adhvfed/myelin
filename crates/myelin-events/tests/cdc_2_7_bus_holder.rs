use myelin_events::{Actor, AggregateKey, EmitContext, EventId};
use myelin_events::{
    ArtifactRef, BusEventLog, BusHolder, DataRole, EraseReceipt, EventDraft, EventType, IdMinter,
    InMemoryShredder, InlinePiiShredder, MonotonicMinter, OutboxStore, PiiKeyRef, Timestamp,
    Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}

fn inline_pii_event(event_id: &str, subject: &str) -> myelin_events::EventEnvelope {
    let key = PiiKeyRef(format!("kms://acme/0/subject:{subject}"));
    let draft = EventDraft {
        type_: EventType("chat.message.created".into()),
        subject: ArtifactRef(format!("myelin://acme/chat/message/{event_id}")),
        aggregate: AggregateKey(format!("chat.message:{event_id}")),
        payload: serde_json::json!({ "ref": format!("myelin://acme/chat/message/{event_id}") }),
        data_role: DataRole::Processor,
        visibility: Visibility::Internal,
        contains_personal_data: true,
        pii_key_ref: Some(key),
    };
    let ctx = EmitContext {
        event_id: EventId(event_id.into()),
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId(subject.into()),
            PrincipalKind::Human,
            tenant(),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-19T00:00:00Z".into()),
        caused_by: None,
    };
    myelin_events::derive_envelope(draft, ctx, None)
}

fn provider_erase(subject: &str) -> (EraseReceipt, InMemoryShredder, PiiKeyRef) {
    let mut log = BusEventLog::new();
    let shredder = InMemoryShredder::new();
    let ev = inline_pii_event("01J-1", subject);
    let key = ev.pii_key_ref.clone().expect("inline-PII event has a key");
    shredder.seal(&key);
    log.append(ev);

    let holder = BusHolder::new(tenant(), region(), shredder.clone());
    let mut outbox = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let receipt = holder
        .erase(subject, &mut log, &mut outbox, minter)
        .expect("provider erase");
    assert_eq!(
        outbox.committed_count(),
        1,
        "provider emitted the tombstone via the outbox"
    );
    (receipt, shredder, key)
}

#[test]
fn cdc_2_7_provider_crypto_shred_consumer_reads_zero_recoverable_receipt() {
    let (receipt, shredder, key) = provider_erase("u42");

    assert_eq!(receipt.subject, "u42");
    assert_eq!(
        receipt.recoverable_remaining, 0,
        "consumer requires 0 recoverable inline-PII"
    );
    assert_eq!(receipt.keys_shredded, 1);
    assert!(
        receipt.tombstones_emitted >= 1,
        "consumer requires the tombstone present"
    );
    assert!(
        !shredder.is_live(&key),
        "consumer confirms the per-subject DEK is crypto-shredded"
    );
}
