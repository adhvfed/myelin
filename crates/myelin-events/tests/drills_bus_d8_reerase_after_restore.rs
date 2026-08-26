use myelin_events::{Actor, AggregateKey, EmitContext, EventId};
use myelin_events::{
    ArtifactRef, BusErasureLedger, BusEventLog, BusHolder, BusObservations, BusSignals, DataRole,
    EventDraft, EventType, IdMinter, InMemoryShredder, InProcessBus, InlinePiiShredder,
    MonotonicMinter, OutboxStore, PiiKeyRef, Relay, Timestamp, Visibility,
};
use myelin_harness::{Predicate, SignalName, SignalSource};
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
    Timestamp("2026-06-19T00:00:04Z".into())
}
fn clock() -> Timestamp {
    Timestamp("2026-06-19T00:00:05Z".into())
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
        occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-19T00:00:00Z".into()),
        caused_by: None,
    };
    myelin_events::derive_envelope(draft, ctx, None)
}

fn minter() -> Arc<dyn IdMinter> {
    Arc::new(MonotonicMinter::new())
}

#[test]
fn bus_post_restore_re_erasure_zero_resurrected_keys_nothing_lost() {
    let mut live_log = BusEventLog::new();
    let shredder = InMemoryShredder::new();
    let ev = inline_pii("01J-1", "u42");
    if let Some(k) = &ev.pii_key_ref {
        shredder.seal(k);
    }
    live_log.append(ev);

    let holder = BusHolder::new(tenant(), region(), shredder.clone());
    let ledger = BusErasureLedger::new(tenant(), region());
    let mut live_outbox = OutboxStore::new();
    holder
        .erase_and_record(
            "u42",
            &mut live_log,
            &mut live_outbox,
            minter(),
            &ledger,
            now(),
        )
        .expect("live erase + ledger record");

    let key = PiiKeyRef("kms://acme/0/subject:u42".into());
    assert!(
        !shredder.is_live(&key),
        "precondition: the DEK is dead in the live cell"
    );
    assert!(
        ledger
            .is_erased("u42")
            .expect("in-memory erasure ledger is available"),
        "the PII-free ledger remembers the erasure"
    );

    let mut restored_log = BusEventLog::new();
    restored_log.append(inline_pii("01J-1", "u42"));
    shredder.seal(&key);
    assert!(
        shredder.is_live(&key),
        "the restore RESURRECTED u42's inline-PII DEK"
    );

    let mut reerase_outbox = OutboxStore::new();
    let receipt = holder
        .re_erase_after_restore(
            &ledger,
            &mut restored_log,
            &mut reerase_outbox,
            minter(),
            now(),
        )
        .expect("re-erase after restore (KMS reachable)");

    assert_eq!(
        receipt.resurrected, 0,
        "STOR-D1/D2: 0 resurrected inline-PII keys post-restore"
    );
    assert!(receipt.is_green(), "the Bus's restore-verify leg is GREEN");
    assert_eq!(
        receipt.keys_resurrected_by_restore, 1,
        "the restore brought the key back (honest)"
    );
    assert_eq!(receipt.re_erased_subjects, 1);
    assert!(
        receipt.tombstones_re_emitted >= 1,
        "re-tombstoned the restored row"
    );
    assert!(
        !shredder.is_live(&key),
        "the key STAYS destroyed across the restore"
    );
    assert!(
        restored_log.is_tombstoned("01J-1"),
        "the restored row carries a tombstone again"
    );

    let bus = InProcessBus::new();
    let relay = Relay::new(reerase_outbox.clone(), bus.clone(), clock);
    let drain = relay.drain_to_empty();
    assert!(
        drain.published >= 1,
        "the relay published the re-emitted *.erased tombstone"
    );

    let obs = BusObservations::default();
    let sig = BusSignals::snapshot(&reerase_outbox, &drain, &obs, &now(), 0)
        .expect("outbox telemetry is readable");
    let mut rec = myelin_events::MetricRecorder::new();
    sig.emit_to(&mut rec);

    let mut src = SignalSource::new();
    if let Some(v) = rec.scalar(myelin_events::BusSignal::OutboxDepth) {
        src.set_scalar(SignalName::OutboxDepth, v);
    }
    if let Some(v) = rec.scalar(myelin_events::BusSignal::DeadLetterCount) {
        src.set_scalar(SignalName::DeadLetterCount, v);
    }
    let depth_ok = src.assert_signal(SignalName::OutboxDepth, Predicate::Eq(0));
    let dlq_ok = src.assert_signal(SignalName::DeadLetterCount, Predicate::Eq(0));
    assert!(
        depth_ok.is_green(),
        "outbox drained after re-erasing across the restore: {depth_ok:?}"
    );
    assert!(
        dlq_ok.is_green(),
        "no tombstone dead-lettered re-erasing: {dlq_ok:?}"
    );
}

#[test]
fn bus_post_restore_re_erasure_is_loud_on_kms_failure() {
    let mut live_log = BusEventLog::new();
    let shredder = InMemoryShredder::new();
    let ev = inline_pii("01J-1", "u42");
    if let Some(k) = &ev.pii_key_ref {
        shredder.seal(k);
    }
    live_log.append(ev);
    let holder = BusHolder::new(tenant(), region(), shredder.clone());
    let ledger = BusErasureLedger::new(tenant(), region());
    let mut live_outbox = OutboxStore::new();
    holder
        .erase_and_record(
            "u42",
            &mut live_log,
            &mut live_outbox,
            minter(),
            &ledger,
            now(),
        )
        .expect("live erase + record");

    let mut restored = BusEventLog::new();
    restored.append(inline_pii("01J-1", "u42"));
    let key = PiiKeyRef("kms://acme/0/subject:u42".into());
    shredder.seal(&key);
    shredder.make_unreachable(&key);

    let mut ro = OutboxStore::new();
    let err = holder
        .re_erase_after_restore(&ledger, &mut restored, &mut ro, minter(), now())
        .expect_err("re-erase is loud on a KMS failure (never assume re-erased)");
    assert!(matches!(
        err,
        myelin_events::BusErasureError::Shred(myelin_events::ShredError::KmsUnavailable(_))
    ));
}
