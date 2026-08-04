use myelin_content::InlineNode;
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, CorrelationId, DataRole, EmitContextBase,
    EventEnvelope, EventId, EventType, IdMinter, MonotonicMinter, OutboxStore, Region, TenantId,
    Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs_service::{ChatEdgeProducer, EdgeProjection, EdgeRel};

use std::sync::Arc;

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn principal() -> Principal {
    Principal::stub(
        PrincipalId("p-opaque-7".into()),
        PrincipalKind::Human,
        tenant(),
    )
}

fn content_event(depth: u32) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId("01J-chat-msg".into()),
        type_: EventType("chat.message.created".into()),
        schema_ver: 1,
        tenant: tenant(),
        region: region(),
        actor: Actor(principal()),
        subject: ChatEdgeProducer::message_root("acme", "01HMSGCDC").expect("canonical chat root"),
        aggregate: AggregateKey("chat:message:01HMSGCDC".into()),
        causation_id: None,
        correlation_id: CorrelationId("01J-root-corr".into()),
        caused_by: Some(CausedBy("session:abc".into())),
        depth,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-23T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-23T00:00:01Z".into()),
        payload: serde_json::json!({ "body_ref": "r1" }),
    }
}

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: region(),
        actor: Actor(principal()),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-23T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-23T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:abc".into())),
    }
}

fn store_and_minter() -> (OutboxStore, Arc<dyn IdMinter>) {
    (
        OutboxStore::new(),
        Arc::new(MonotonicMinter::new()) as Arc<dyn IdMinter>,
    )
}

fn maximal_chat_body() -> Vec<InlineNode> {
    vec![
        InlineNode::Mention(principal()),
        InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/issue/issue/ENG-1".into())),
        InlineNode::Embed(ArtifactRef("myelin://acme/git/commit/core:deadbeef".into())),
        InlineNode::Embed(ArtifactRef("myelin://acme/ci/run/run-9".into())),
        InlineNode::Embed(ArtifactRef("myelin://acme/knowledge/page/42".into())),
    ]
}

#[test]
fn chat_unfurls_emit_commit_and_ingest_as_reference_edges() {
    let (store, minter) = store_and_minter();
    let producer = ChatEdgeProducer;
    let source = ChatEdgeProducer::message_root("acme", "01HMSGCDC").expect("canonical chat root");
    let content = content_event(2);
    let body = maximal_chat_body();

    let mut tx = store.begin(minter, ctx_base());
    tx.stage_state_change("chat message 01HMSGCDC written");
    let ids: Vec<EventId> = producer
        .emit_chat_edges(&mut tx, &source, &body, &content)
        .expect("emit chat unfurls");
    assert_eq!(ids.len(), 5, "five structured nodes → five unfurl edges");
    assert_eq!(
        store.outbox_depth(),
        0,
        "emit-iff-committed: nothing durable before commit"
    );
    tx.commit().expect("commit the message + its unfurl edges");
    assert_eq!(
        store.outbox_depth(),
        5,
        "five edge rows durable after commit"
    );

    let proj = EdgeProjection::new();
    let mut rels: Vec<EdgeRel> = Vec::new();
    for id in &ids {
        let row = store.row(id).expect("committed edge row");
        let env = &row.envelope;
        assert_eq!(env.type_, EventType("refs.edge.created".into()));
        assert_eq!(env.payload["rel_class"], "reference");
        assert_eq!(
            env.payload["source"], "myelin://acme/chat/message/01HMSGCDC",
            "every unfurl edge is sourced from the Chat message root"
        );
        assert_eq!(env.depth, content.depth + 1);
        assert_eq!(env.correlation_id, content.correlation_id);
        rels.push(match env.payload["rel"].as_str().unwrap() {
            "mentions" => EdgeRel::Mentions,
            "links" => EdgeRel::Links,
            "embeds" => EdgeRel::Embeds,
            other => panic!("unexpected rel {other}"),
        });
        let target = env.payload["target"].as_str().unwrap();
        let id_str = myelin_refs_service::edge_id(
            &tenant(),
            "myelin://acme/chat/message/01HMSGCDC",
            target,
            env.payload["rel"].as_str().unwrap(),
        );
        proj.upsert(
            &tenant(),
            &region(),
            myelin_refs_service::EdgeRow {
                edge_id: id_str.clone(),
                source: source.clone(),
                source_root: myelin_refs::strip_sub(&source),
                target: ArtifactRef(target.into()),
                target_root: myelin_refs::strip_sub(&ArtifactRef(target.into())),
                rel: env.payload["rel"].as_str().unwrap().into(),
                rel_class: myelin_refs_service::RelClass::Reference,
                origin_event: format!("evt-{id_str}"),
                origin_actor: "chat-pseudonym".into(),
                zookie: Some("zk-1".into()),
                tombstoned: false,
            },
        );
    }
    assert_eq!(
        rels,
        vec![
            EdgeRel::Mentions,
            EdgeRel::Links,
            EdgeRel::Embeds,
            EdgeRel::Embeds,
            EdgeRel::Embeds
        ]
    );

    let subsystems: std::collections::BTreeSet<String> = body
        .iter()
        .filter_map(|n| match n {
            InlineNode::ArtifactRefNode(r) | InlineNode::Embed(r) => {
                r.0.split('/').nth(3).map(|s| s.to_string())
            }
            InlineNode::Mention(_) => Some("identity".into()),
        })
        .collect();
    assert!(
        ["ci", "git", "issue", "knowledge"]
            .iter()
            .all(|s| subsystems.contains(*s)),
        "the maximal Chat message unfurls every prior producer class - traversal complete"
    );
}

#[test]
fn aborted_chat_message_emits_zero_unfurl_edges() {
    let (store, minter) = store_and_minter();
    let producer = ChatEdgeProducer;
    let source = ChatEdgeProducer::message_root("acme", "01HMSGCDC").expect("canonical chat root");
    let content = content_event(0);
    let body = maximal_chat_body();
    {
        let mut tx = store.begin(minter, ctx_base());
        tx.stage_state_change("chat message written");
        let ids = producer
            .emit_chat_edges(&mut tx, &source, &body, &content)
            .expect("emit ok");
        assert_eq!(
            ids.len(),
            5,
            "five edges buffered into the open transaction"
        );
    }
    assert_eq!(
        store.outbox_depth(),
        0,
        "aborted chat message-send → 0 unfurl edges"
    );
    assert_eq!(store.committed_count(), 0);
}
