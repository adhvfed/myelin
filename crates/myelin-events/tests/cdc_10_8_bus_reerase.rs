use myelin_events::{Actor, AggregateKey, EmitContext, EventId};
use myelin_events::{
    ArtifactRef, BusErasureLedger, BusEventLog, BusHolder, DataRole, EventDraft, EventType,
    IdMinter, InMemoryShredder, InlinePiiShredder, MonotonicMinter, OutboxStore, PiiKeyRef,
    ReErasureReceipt, Timestamp, Visibility,
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
fn now() -> Timestamp {
    Timestamp("2026-06-19T00:00:00Z".into())
}
fn actor_for(id: &str) -> Actor {
    Actor(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        tenant(),
    ))
}

fn inline_pii(event_id: &str, subject: &str) -> myelin_events::EventEnvelope {
    let draft = EventDraft {
        type_: EventType("chat.message.created".into()),
        subject: ArtifactRef(format!("myelin://acme/chat/message/{event_id}")),
        aggregate: AggregateKey(format!("chat.message:{event_id}")),
        payload: serde_json::json!({ "ref": format!("myelin://acme/chat/message/{event_id}") }),
        data_role: DataRole::Processor,
        visibility: Visibility::Internal,
        contains_personal_data: true,
        pii_key_ref: Some(PiiKeyRef(format!("kms://acme/0/subject:{subject}"))),
    };
    let ctx = EmitContext {
        event_id: EventId(event_id.into()),
        tenant: tenant(),
        region: region(),
        actor: actor_for(subject),
        schema_ver: 1,
        occurred_at: now(),
        recorded_at: now(),
        caused_by: None,
    };
    myelin_events::derive_envelope(draft, ctx, None)
}

fn minter() -> Arc<dyn IdMinter> {
    Arc::new(MonotonicMinter::new())
}

#[test]
fn cdc_10_8_provider_restores_consumer_re_erases_zero_resurrected() {
    let mut live_log = BusEventLog::new();
    let shredder = InMemoryShredder::new();
    let ev = inline_pii("01J-1", "u42");
    if let Some(k) = &ev.pii_key_ref {
        shredder.seal(k);
    }
    live_log.append(ev);
    let holder = BusHolder::new(tenant(), region(), shredder.clone());
    let ledger = BusErasureLedger::new(tenant(), region());
    let mut outbox = OutboxStore::new();
    holder
        .erase_and_record("u42", &mut live_log, &mut outbox, minter(), &ledger, now())
        .expect("provider: live erase + ledger record");

    let key = PiiKeyRef("kms://acme/0/subject:u42".into());
    let mut restored_log = BusEventLog::new();
    let ev2 = inline_pii("01J-1", "u42");
    restored_log.append(ev2);
    shredder.seal(&key);

    let mut reerase_outbox = OutboxStore::new();
    let receipt: ReErasureReceipt = holder
        .re_erase_after_restore(
            &ledger,
            &mut restored_log,
            &mut reerase_outbox,
            minter(),
            now(),
        )
        .expect("consumer: re-erase after restore");

    assert!(
        receipt.is_green(),
        "consumer requires 0 resurrected inline-PII keys post-restore"
    );
    assert_eq!(receipt.resurrected, 0);
    assert_eq!(
        receipt.keys_resurrected_by_restore, 1,
        "the restore resurrected one key"
    );
    assert_eq!(receipt.re_erased_subjects, 1);
    assert!(
        !shredder.is_live(&key),
        "the key STAYS destroyed across the restore"
    );
    assert!(ledger.is_erased("u42"));
}
