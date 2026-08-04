use myelin_events::{
    Actor, AggregateKey, ArtifactRef, BusTransport, DataRole, EmitContextBase, EventDraft,
    EventHandler, EventType, HandleOutcome, IdMinter, InProcessBus, MonotonicMinter, OutboxStore,
    OutboxTx, Relay, Timestamp, Visibility,
};
use myelin_gdpr_service::AuditConsumer;
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::TenantId;
use std::sync::Arc;

const IDENTITY_TUPLE_WRITTEN: &str = "identity.tuple.written";

fn provider_emits_action(actor: Principal, subject: &str) -> OutboxStore {
    let outbox = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let ctx_base = EmitContextBase {
        tenant: actor.tenant.clone(),
        region: actor.region.clone(),
        actor: Actor(actor),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-19T00:00:01Z".into()),
        caused_by: None,
    };
    let mut tx = outbox.begin(minter, ctx_base);
    let draft = EventDraft {
        type_: EventType(IDENTITY_TUPLE_WRITTEN.into()),
        subject: ArtifactRef(subject.into()),
        aggregate: AggregateKey("identity:acme".into()),
        payload: serde_json::json!({ "real_name": "Alice Example", "email": "alice@example.test" }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    };
    tx.stage_state_change("tuple org:acme#member@p:alice written");
    tx.emit(draft, None)
        .expect("the action emits via the outbox");
    tx.commit().expect("the action + its state co-commit");
    outbox
}

#[test]
fn cdc_10_6_provider_emits_via_outbox_consumer_appends_minimised_hash_chained_entry() {
    let actor = Principal::stub(
        PrincipalId("u-42".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    );

    let outbox = provider_emits_action(actor, "myelin://acme/identity/tuple/t1");
    assert_eq!(
        outbox.outbox_depth(),
        1,
        "exactly one event for the one committed action"
    );

    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
    relay.drain_to_empty();
    let published = bus.consume("");
    assert_eq!(
        published.len(),
        1,
        "the relay published exactly the one action event"
    );
    assert_eq!(published[0].type_.0, IDENTITY_TUPLE_WRITTEN);

    let audit = AuditConsumer::new();
    assert_eq!(
        audit.handle(&published[0], &mut myelin_events::HandlerTx::none()),
        HandleOutcome::Done,
        "the audit consumer appends + acks"
    );

    let tenant = TenantId("acme".into());
    let entries = audit.log().entries_for(&tenant);
    assert_eq!(
        entries.len(),
        1,
        "one delivered action → one appended audit entry"
    );
    let e = &entries[0];

    assert_eq!(
        e.actor.actor, "u-42@acme.noreply",
        "actor is the minimised pseudonym grammar (4.8)"
    );
    assert_eq!(e.actor.actor_kind, "human");
    assert_eq!(e.action, IDENTITY_TUPLE_WRITTEN);
    assert_eq!(e.subject, ArtifactRef("myelin://acme/identity/tuple/t1".into()));
    assert!(
        e.prev_hash.starts_with("blake3:"),
        "hash-chain link present"
    );
    assert!(e.leaf_hash.starts_with("blake3:"), "Merkle leaf present");
    assert_eq!(
        e.correlation_id, published[0].correlation_id.0,
        "correlation (root) carried verbatim"
    );
    assert_eq!(
        e.causation_id, None,
        "a root action has no immediate parent"
    );

    let serialized = serde_json::to_string(e).expect("entry serialises");
    assert!(
        !serialized.contains("Alice Example"),
        "no real name reaches the audit entry"
    );
    assert!(
        !serialized.contains("alice@example.test"),
        "no email reaches the audit entry"
    );

    assert!(
        audit.log().verify_chain(&tenant),
        "the appended chain verifies intact"
    );
    assert!(
        audit.log().root(&tenant).is_some(),
        "a per-tenant Merkle root exists (the STH input)"
    );

    assert_eq!(
        audit.append_lag(),
        0,
        "audit_append_lag reads green (0) after the append"
    );
}
