use myelin_events::{
    reindex, snapshot_event_id, DerivedStore, EmitContextBase, OutboxStore, ReferenceReindexSource,
    ReindexReceipt, ReindexSource, SnapshotScope,
};
use myelin_events::{Actor, AggregateKey, Region, TenantId, Timestamp};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("platform".into()),
            PrincipalKind::Service,
            TenantId("acme".into()),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:00Z".into()),
        caused_by: None,
    }
}

fn provider_reindex() -> (
    ReindexReceipt,
    OutboxStore,
    ReferenceReindexSource,
    SnapshotScope,
) {
    let mut source = ReferenceReindexSource::new("knowledge", "page");
    source.upsert(
        "knowledge.page:home",
        2,
        serde_json::json!({ "title_ref": "r-home" }),
    );
    source.upsert(
        "knowledge.page:guide",
        1,
        serde_json::json!({ "title_ref": "r-guide" }),
    );
    let scope = SnapshotScope::new("knowledge", "page:subtree");

    let mut outbox = OutboxStore::new();
    let sources: &[&dyn ReindexSource] = &[&source];
    let receipt =
        reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("provider reindex");
    (receipt, outbox, source, scope)
}

#[test]
fn cdc_2_6_provider_reindex_consumer_rebuilds_cold_equals_live() {
    let (receipt, outbox, source, scope) = provider_reindex();
    assert_eq!(
        receipt.snapshots_emitted, 2,
        "provider re-emitted both aggregates"
    );
    assert_eq!(receipt.owners_replayed, vec!["knowledge".to_string()]);

    let home_id = snapshot_event_id(&AggregateKey("knowledge.page:home".into()), 2);
    let row = outbox
        .row(&home_id)
        .expect("the home snapshot is at its deterministic id");
    assert_eq!(row.envelope.type_.0, "knowledge.page.snapshot");

    let mut live = DerivedStore::new();
    for draft in source.replay(&scope, None) {
        live.ingest(&live_envelope(&draft));
    }
    let mut cold = DerivedStore::new();
    for draft in source.replay(&scope, None) {
        let r = outbox.row(&draft.event_id()).expect("snapshot row present");
        cold.ingest(&r.envelope);
    }
    assert_eq!(
        cold.parity_bytes(),
        live.parity_bytes(),
        "cold == live (the 2.6 contract)"
    );

    let before = cold.parity_bytes();
    for draft in source.replay(&scope, None) {
        let r = outbox.row(&draft.event_id()).expect("snapshot row present");
        assert!(
            !cold.ingest(&r.envelope),
            "a re-ingested snapshot is a no-op (idempotent)"
        );
    }
    assert_eq!(
        cold.parity_bytes(),
        before,
        "byte-stable across a re-ingest"
    );
}

fn live_envelope(draft: &myelin_events::SnapshotDraft) -> myelin_events::EventEnvelope {
    use myelin_events::{CorrelationId, EventId};
    let id = draft.event_id();
    myelin_events::EventEnvelope {
        event_id: EventId(id.0.clone()),
        type_: draft.type_.clone(),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("platform".into()),
            PrincipalKind::Service,
            TenantId("acme".into()),
        )),
        subject: draft.subject.clone(),
        aggregate: draft.aggregate.clone(),
        causation_id: None,
        correlation_id: CorrelationId(id.0),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: draft.data_role,
        visibility: draft.visibility,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:00Z".into()),
        payload: draft.payload.clone(),
    }
}
