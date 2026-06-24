//! # CDC 2.6 / 6.4 — Chat replay(scope, since) full parity (Search/Refs/Notif rebuild; ONE path)
//! (CHAT-P21 / P-416, M4-C7)
//!
//! **Contract 2.6** — `reindex-from-source` / `replay(scope, since)` (OWNED by the Bus seam; each owner
//! ships its `replay` body). This CDC pins BOTH sides of the Chat leg of the seam:
//! - **PROVIDER (chat)** — `myelin_chat::replay::ChatReindexSource::replay` re-emits one
//!   `chat.message.snapshot` per durable message through the Bus OUTBOX (`myelin_events::reindex`), at
//!   the DETERMINISTIC `snapshot_event_id(aggregate, version)` (idempotent re-run).
//! - **CONSUMER (chat read-models)** — `myelin_chat::replay::ChatReadModelConsumer::ingest` re-applies
//!   each `*.snapshot` through the SAME step the steady-state live event takes, materializing the three
//!   Chat-fed read-models (Search row ∥ Refs edge ∥ Notif reason). Steady-state and recovery share ONE
//!   path (0 recovery-only code paths) → the `reindex_parity_hash` of a cold rebuild == the live one.
//!
//! **Contract 6.4** — `reindex(scope) -> job` (Search). **CONSUMED** by Chat: the chat message
//! read-model is rebuilt the §4.9 ONLY way — the Bus re-emit (2.6) drives the SAME live indexer step
//! (CONSUMED here through `ChatReadModelConsumer`, the Search-engine-backed leg is the CHAT-D15 drill).
//!
//! Coherence (EI-01 §7): chat owns NO second rebuild path. The rebuild reuses the ONE Bus reindex seam
//! plus the ONE consumer ingest step; the parity is inherited from the cold==live invariant, not
//! re-built. The erased-subject tombstone (X-7) is the SAME `ChatReindexSource::erase` skip the
//! skeleton proves.

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

/// The owner's `project(message)` (5.6) over an in-memory truth — the SAME fetcher serves the live emit
/// and the cold replay (cold == live). An ABSENT (erased) message returns `None` (no row, X-7).
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
        event_id: myelin_events::snapshot_event_id(&agg, version),
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

/// **The 2.6 / 6.4 pair, end-to-end: the PROVIDER (chat replay re-emits) + the CONSUMER (the read-models
/// rebuild) — steady-state and recovery share ONE path (the parity hash matches).**
#[test]
fn cdc_2_6_6_4_chat_replay_parity_provider_reemits_consumer_rebuilds_one_path() {
    let proj = projector();
    let src = source();
    let scope = SnapshotScope::new("chat", "message:all");

    // CONSUMER (live): the steady-state path ingests the live message events.
    let mut live = ChatReadModelConsumer::new();
    for draft in src.replay(&scope, None) {
        live.ingest(&live_envelope(&draft.aggregate.0, draft.version), &proj);
    }

    // PROVIDER (recovery): chat's replay re-emits each chat.message.snapshot through the Bus outbox at
    // its deterministic id. CONSUMER (recovery): a WIPED read-model rebuilds from those re-emits through
    // the SAME ingest step.
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
            .row(&draft.event_id())
            .expect("snapshot row at deterministic id");
        assert_eq!(
            row.envelope.type_.0, CHAT_MESSAGE_SNAPSHOT,
            "the re-emit carries the frozen chat.message.snapshot token"
        );
        cold.ingest(&row.envelope, &proj);
    }

    // THE DATED GREEN ARTIFACT: the reindex-parity hash matches (cold == live across Search/Refs/Notif;
    // one path, 0 recovery-only code paths).
    assert_eq!(
        reindex_parity_hash(&cold),
        reindex_parity_hash(&live),
        "steady-state and recovery share one path — the reindex-parity hash matches (CHAT-D15)"
    );
    assert_eq!(cold.search_len(), 2);
    assert_eq!(cold.refs_len(), 1, "the Refs read-model rebuilt (5.2)");
    assert_eq!(cold.notif_len(), 2, "the Notif read-model rebuilt (7.1)");
}

/// **PROVIDER idempotence (the deterministic id) — a re-run emits 0 new snapshots.** The re-emit is a
/// no-op on the deterministic `snapshot_event_id`; the consumer's rebuild is unchanged.
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

/// **CONSUMER erased-skip (X-7) — an erased subject emits a tombstone on rebuild (no resurrection).**
/// m2 is erased at the owner; the rebuild SKIPS it (its Search/Notif rows are absent).
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
        let row = outbox.row(&draft.event_id()).expect("snapshot row");
        cold.ingest(&row.envelope, &proj);
    }
    assert_eq!(
        cold.search_len(),
        1,
        "only m1 rebuilt — the erased m2 did not resurrect"
    );
    assert!(!cold.search_indexes("myelin://acme/chat/message/m2"));
    assert_eq!(
        cold.notif_len(),
        1,
        "bob's @-mention row to the erased m2 is absent"
    );
}

/// The Notif notify-reason token is the frozen `mentioned` rule-key (contract 7.6) — pinned, not a literal.
#[test]
fn cdc_7_1_chat_notif_reason_is_the_frozen_mentioned_rule_key() {
    assert_eq!(NOTIF_REASON_MENTIONED, "chat.message.mentioned");
}
