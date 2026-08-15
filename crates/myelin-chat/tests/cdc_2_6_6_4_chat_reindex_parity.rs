use myelin_chat::events::CHAT_MESSAGE_SNAPSHOT;
use myelin_chat::replay::{
    reindex_parity_hash, ChatReadModelConsumer, ChatReindexSource, ChatReplayKind,
    MessageProjectFetcher, MessageProjection, NOTIF_REASON_MENTIONED,
};
use myelin_events::{
    reindex as bus_reindex, Actor, AggregateKey, CorrelationId, DataRole, EmitContextBase,
    EventEnvelope, EventType, OutboxStore, Region, ReindexSource, SnapshotScope, TenantId,
    Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use std::collections::BTreeMap;

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId("platform".into()),
            PrincipalKind::Service,
            tenant(),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-24T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-24T00:00:00Z".into()),
        caused_by: None,
    }
}

#[derive(Default)]
struct FakeProjector {
    bodies: BTreeMap<String, MessageProjection>,
}
impl MessageProjectFetcher for FakeProjector {
    fn project(&self, message_ref: &str) -> Option<MessageProjection> {
        self.bodies.get(message_ref).cloned()
    }
}

fn projector() -> FakeProjector {
    let mut f = FakeProjector::default();
    f.bodies.insert(
        "myelin://acme/chat/message/m1".into(),
        MessageProjection {
            body_text: "blocked on the deploy".into(),
            channel_id: "c1".into(),
            edges: vec![("myelin://acme/issue/issue/ENG-1".into(), "links".into())],
            mentions: vec!["myelin://acme/identity/member/alice".into()],
        },
    );
    f.bodies.insert(
        "myelin://acme/chat/message/m2".into(),
        MessageProjection {
            body_text: "shipping now".into(),
            channel_id: "c2".into(),
            edges: vec![],
            mentions: vec!["myelin://acme/identity/member/bob".into()],
        },
    );
    f
}

fn source() -> ChatReindexSource {
    let mut s = ChatReindexSource::new();
    for (mref, channel) in [
        ("myelin://acme/chat/message/m1", "c1"),
        ("myelin://acme/chat/message/m2", "c2"),
    ] {
        s.upsert(
            ChatReplayKind::Message,
            mref,
            1,
            mref,
            serde_json::json!({ "channel": channel }),
        );
    }
    s
}

fn live_envelope(message_ref: &str, version: u64) -> EventEnvelope {
    let agg = AggregateKey(message_ref.to_string());
    EventEnvelope {
        event_id: myelin_events::snapshot_event_id(&tenant(), &agg, version),
        type_: EventType(CHAT_MESSAGE_SNAPSHOT.into()),
        schema_ver: 1,
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId("platform".into()),
            PrincipalKind::Service,
            tenant(),
        )),
        subject: myelin_events::ArtifactRef(message_ref.to_string()),
        aggregate: agg,
        causation_id: None,
        correlation_id: CorrelationId(format!("corr-{message_ref}")),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Processor,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-24T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-24T00:00:00Z".into()),
        payload: serde_json::json!({ "version": version }),
    }
}

#[test]
fn cdc_2_6_6_4_chat_replay_parity_provider_reemits_consumer_rebuilds_one_path() {
    let proj = projector();
    let src = source();
    let scope = SnapshotScope::new("chat", "message:all");

    let mut live = ChatReadModelConsumer::new();
    for draft in src.replay(&scope, None) {
        live.ingest(&live_envelope(&draft.aggregate.0, draft.version), &proj);
    }

    let mut outbox = OutboxStore::new();
    let sources: &[&dyn ReindexSource] = &[&src];
    let receipt = bus_reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("reindex");
    assert_eq!(
        receipt.snapshots_emitted, 2,
        "the PROVIDER re-emits one chat.message.snapshot per durable message (2.6)"
    );

    let mut cold = ChatReadModelConsumer::new();
    for draft in src.replay(&scope, None) {
        let row = outbox
            .row(&draft.event_id(&tenant()))
            .expect("snapshot row at deterministic id");
        assert_eq!(
            row.envelope.type_.0, CHAT_MESSAGE_SNAPSHOT,
            "the re-emit carries the frozen chat.message.snapshot token"
        );
        cold.ingest(&row.envelope, &proj);
    }

    assert_eq!(
        reindex_parity_hash(&cold),
        reindex_parity_hash(&live),
        "steady-state and recovery share one path - the reindex-parity hash matches (CHAT-D15)"
    );
    assert_eq!(cold.search_len(), 2);
    assert_eq!(cold.refs_len(), 1, "the Refs read-model rebuilt (5.2)");
    assert_eq!(cold.notif_len(), 2, "the Notif read-model rebuilt (7.1)");
}

#[test]
fn cdc_2_6_chat_replay_rerun_emits_zero_new() {
    let src = source();
    let scope = SnapshotScope::new("chat", "message:all");
    let mut outbox = OutboxStore::new();
    let sources: &[&dyn ReindexSource] = &[&src];

    let r1 = bus_reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("first");
    assert_eq!(r1.snapshots_emitted, 2);
    let r2 = bus_reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("second");
    assert_eq!(r2.snapshots_emitted, 0, "a re-run emits 0 NEW (idempotent)");
    assert_eq!(r2.snapshots_skipped_duplicate, 2);
}

#[test]
fn cdc_2_6_chat_replay_erased_subject_is_a_tombstone_on_rebuild() {
    let mut proj = projector();
    let mut src = source();
    let scope = SnapshotScope::new("chat", "message:all");

    assert!(src.erase("myelin://acme/chat/message/m2"));
    proj.bodies.remove("myelin://acme/chat/message/m2");

    let mut outbox = OutboxStore::new();
    let sources: &[&dyn ReindexSource] = &[&src];
    bus_reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("post-erase reindex");

    let mut cold = ChatReadModelConsumer::new();
    for draft in src.replay(&scope, None) {
        let row = outbox
            .row(&draft.event_id(&tenant()))
            .expect("snapshot row");
        cold.ingest(&row.envelope, &proj);
    }
    assert_eq!(
        cold.search_len(),
        1,
        "only m1 rebuilt - the erased m2 did not resurrect"
    );
    assert!(!cold.search_indexes("myelin://acme/chat/message/m2"));
    assert_eq!(
        cold.notif_len(),
        1,
        "bob's @-mention row to the erased m2 is absent"
    );
}

#[test]
fn cdc_7_1_chat_notif_reason_is_the_frozen_mentioned_rule_key() {
    assert_eq!(NOTIF_REASON_MENTIONED, "chat.message.mentioned");
}
