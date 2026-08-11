use std::sync::Arc;

use myelin_content::{InlineNode, OBJ};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, CorrelationId, DataRole, EmitContextBase,
    EventEnvelope, EventId, EventType, IdMinter, MonotonicMinter, OutboxStore, Region, TenantId,
    Timestamp, Visibility,
};
use myelin_git::body::{emit_body_edges, extract_body_edges, Body, EdgeRel};
use myelin_git::events::GIT_COMMENT_CREATED;
use myelin_git::subs::mint_pr_comment;
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

fn consumer_edge_id(tenant: &str, source: &str, target: &str, rel: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"myelin.refs.edge.v2");
    for field in [
        tenant.as_bytes(),
        source.as_bytes(),
        target.as_bytes(),
        rel.as_bytes(),
    ] {
        hasher.update(&(field.len() as u64).to_be_bytes());
        hasher.update(field);
    }
    format!("blake3:{}", hasher.finalize().to_hex())
}

#[derive(Debug, PartialEq, Eq)]
struct DecodedEdge {
    edge_id: String,
    source: String,
    target: String,
    rel: String,
    rel_class: String,
}

fn consumer_decode(env: &EventEnvelope) -> Result<DecodedEdge, String> {
    assert_eq!(
        env.type_.0, "refs.edge.created",
        "the consumer only ingests refs.edge.created here"
    );
    let p = &env.payload;
    let get = |k: &str| p.get(k).and_then(|v| v.as_str()).map(|s| s.to_string());
    let source = get("source").ok_or_else(|| "source".to_string())?;
    let target = get("target").ok_or_else(|| "target".to_string())?;
    let rel = get("rel").ok_or_else(|| "rel".to_string())?;
    let rel_class = get("rel_class").ok_or_else(|| "rel_class".to_string())?;
    let edge_id = consumer_edge_id(&env.tenant.0, &source, &target, &rel);
    Ok(DecodedEdge {
        edge_id,
        source,
        target,
        rel,
        rel_class,
    })
}

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("p-author".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:c1".into())),
    }
}

fn alice() -> Principal {
    Principal::stub(
        PrincipalId("p-alice".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

fn content_event(source: &ArtifactRef) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId("01J-comment".into()),
        type_: EventType(GIT_COMMENT_CREATED.into()),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(alice()),
        subject: source.clone(),
        aggregate: AggregateKey("git/pr/repo7:42".into()),
        causation_id: None,
        correlation_id: CorrelationId("01J-comment-corr".into()),
        caused_by: Some(CausedBy("session:c1".into())),
        depth: 1,
        contains_personal_data: false,
        data_role: DataRole::Processor,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        payload: serde_json::json!({ "comment_ref": source.0 }),
    }
}

#[test]
fn git_body_edges_decode_through_the_refs_consumer_with_the_right_edge_id() {
    let outbox = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let source = mint_pr_comment("acme", "repo7", 42, "cAbc").unwrap();

    let nodes = vec![
        InlineNode::Mention(alice()),
        InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/issue/issue/ENG-1".into())),
        InlineNode::Embed(ArtifactRef("myelin://acme/knowledge/page/7c2".into())),
    ];

    let ce = content_event(&source);
    let mut tx = outbox.begin(Arc::clone(&minter), ctx_base());
    tx.stage_state_change("git comment cAbc body written");
    let ids = emit_body_edges(&mut tx, &source, &nodes, &ce).unwrap();
    tx.commit().unwrap();

    let provider_edges = extract_body_edges(&source, &nodes);
    assert_eq!(provider_edges.len(), 3);

    for (id, p_edge) in ids.iter().zip(provider_edges.iter()) {
        let env = outbox.row(id).unwrap().envelope;
        let decoded = consumer_decode(&env).expect("the consumer decodes the Git edge");
        assert_eq!(
            decoded.rel_class, "reference",
            "content-node edges are reference-class"
        );
        assert_eq!(
            decoded.rel,
            p_edge.rel.as_str(),
            "the provider rel == the consumer-decoded rel"
        );
        assert_eq!(decoded.source, source.0);
        assert_eq!(decoded.target, p_edge.target.0);
        let expected = consumer_edge_id("acme", &source.0, &p_edge.target.0, p_edge.rel.as_str());
        assert_eq!(
            decoded.edge_id, expected,
            "the deterministic edge_id is provider/consumer-stable"
        );
    }
}

#[test]
fn the_three_node_kinds_map_to_the_frozen_rels() {
    let outbox = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let source = mint_pr_comment("acme", "repo7", 42, "cAbc").unwrap();

    let cases = [
        (InlineNode::Mention(alice()), EdgeRel::Mentions, "mentions"),
        (
            InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/issue/issue/ENG-1".into())),
            EdgeRel::Links,
            "links",
        ),
        (
            InlineNode::Embed(ArtifactRef("myelin://acme/knowledge/page/7c2".into())),
            EdgeRel::Embeds,
            "embeds",
        ),
    ];

    for (node, rel, wire) in cases {
        assert_eq!(rel.as_str(), wire);
        let ce = content_event(&source);
        let mut tx = outbox.begin(Arc::clone(&minter), ctx_base());
        tx.stage_state_change("git comment cAbc body written");
        let ids = emit_body_edges(&mut tx, &source, std::slice::from_ref(&node), &ce).unwrap();
        tx.commit().unwrap();
        let env = outbox.row(&ids[0]).unwrap().envelope;
        assert_eq!(
            consumer_decode(&env).unwrap().rel,
            wire,
            "{node:?} → `{wire}`"
        );
    }
}

#[test]
fn git_body_is_the_frozen_content_subset_and_round_trips() {
    let body = Body::new(
        format!("**ship it** - see {OBJ} and `cargo test` per [doc](https://x.test/d)"),
        vec![InlineNode::Embed(ArtifactRef(
            "myelin://acme/knowledge/page/7c2".into(),
        ))],
    );
    assert!(
        body.round_trips(),
        "render(parse(md)) === md (the 13.1 gate on git bodies)"
    );
    assert_eq!(body.parse().nodes.len(), 1);
}
