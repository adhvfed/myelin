use std::sync::Arc;

use myelin_chat::content::{
    emit_body_edges, extract_body_edges, validate_subtree, EdgeRel, MessageBody, SubsetError,
};
use myelin_chat::events::CHAT_MESSAGE_CREATED;
use myelin_chat::subs::mint_message;
use myelin_content::{
    parse_inline, serialize_inline, Block, Cell, Column, HeadingLevel, InlineNode, ListItem, OBJ,
};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, CorrelationId, DataRole, EmitContextBase,
    EventEnvelope, EventId, EventType, IdMinter, MonotonicMinter, OutboxStore, Region, TenantId,
    Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

fn consumer_edge_id(tenant: &str, source: &str, target: &str, rel: &str) -> String {
    let mut h: u128 = 0x6c62272e07bb014262b821756295c58d;
    const PRIME: u128 = 0x0000000001000000000000000000013b;
    let mut feed = |bytes: &[u8]| {
        for &b in bytes {
            h ^= b as u128;
            h = h.wrapping_mul(PRIME);
        }
        h ^= 0x00;
        h = h.wrapping_mul(PRIME);
    };
    feed(tenant.as_bytes());
    feed(source.as_bytes());
    feed(target.as_bytes());
    feed(rel.as_bytes());
    format!("{h:032x}")
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
        actor: Actor(alice()),
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
        event_id: EventId("01J-msg".into()),
        type_: EventType(CHAT_MESSAGE_CREATED.into()),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(alice()),
        subject: source.clone(),
        aggregate: AggregateKey("chat:conv:01J0CONV".into()),
        causation_id: None,
        correlation_id: CorrelationId("01J-msg-corr".into()),
        caused_by: Some(CausedBy("session:c1".into())),
        depth: 1,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        payload: serde_json::json!({ "message": source.0 }),
    }
}

#[test]
fn chat_body_edges_decode_through_the_refs_consumer_with_the_right_edge_id() {
    let outbox = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let source = mint_message("acme", "01J0MSGULID").unwrap();

    let nodes = vec![
        InlineNode::Mention(alice()),
        InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/issue/issue/ENG-1".into())),
        InlineNode::Embed(ArtifactRef("myelin://acme/knowledge/page/7c2".into())),
    ];

    let ce = content_event(&source);
    let mut tx = outbox.begin(Arc::clone(&minter), ctx_base());
    tx.stage_state_change("chat message 01J0MSGULID body written");
    let ids = emit_body_edges(&mut tx, &source, &nodes, &ce).unwrap();
    tx.commit().unwrap();

    let provider_edges = extract_body_edges(&source, &nodes);
    assert_eq!(provider_edges.len(), 3);

    for (id, p_edge) in ids.iter().zip(provider_edges.iter()) {
        let env = outbox.row(id).unwrap().envelope;
        let decoded = consumer_decode(&env).expect("the consumer decodes the Chat edge");
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
    let source = mint_message("acme", "01J0MSGULID").unwrap();

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
        tx.stage_state_change("chat message body written");
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
fn plain_prose_message_emits_no_edges() {
    let outbox = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let source = mint_message("acme", "01J0MSGULID").unwrap();
    let ce = content_event(&source);
    let mut tx = outbox.begin(Arc::clone(&minter), ctx_base());
    tx.stage_state_change("plain message");
    let body = MessageBody::new(vec![Block::Paragraph {
        inline: parse_inline("ping @alice - see myelin://acme/issue/ENG-1", &[]),
    }])
    .unwrap();
    let nodes: Vec<InlineNode> = body.structured_nodes().into_iter().cloned().collect();
    let ids = emit_body_edges(&mut tx, &source, &nodes, &ce).unwrap();
    tx.commit().unwrap();
    assert!(ids.is_empty(), "a prose reference is NOT a content edge");
    assert_eq!(outbox.committed_count(), 0, "0 edge events committed");
}

#[test]
fn chat_body_is_the_frozen_content_subset_and_round_trips() {
    let body = MessageBody::new(vec![
        Block::Heading {
            level: HeadingLevel::new(2).unwrap(),
            inline: parse_inline("**Status**", &[]),
        },
        Block::Paragraph {
            inline: parse_inline(
                &format!("ship it - see {OBJ} and `cargo test` per [doc](https://x.test/d)"),
                &[InlineNode::Embed(ArtifactRef(
                    "myelin://acme/knowledge/page/7c2".into(),
                ))],
            ),
        },
        Block::BulletList {
            items: vec![ListItem {
                blocks: vec![Block::Paragraph {
                    inline: parse_inline("a *list* item", &[]),
                }],
            }],
        },
    ])
    .unwrap();
    assert!(
        body.round_trips(),
        "render(parse(md)) === md (the 13.1 gate on chat bodies)"
    );
}

#[test]
fn chat_subset_excludes_the_three_knowledge_only_nodes() {
    use myelin_query::{FieldId, ViewSpec};
    let excluded: [(Block, &str); 3] = [
        (
            Block::DbView {
                db: ArtifactRef("myelin://acme/db/1".into()),
                view: ViewSpec::table(FieldId::new("k")),
            },
            "db_view",
        ),
        (
            Block::SyncBlock {
                source: ArtifactRef("myelin://acme/block/9".into()),
            },
            "sync_block",
        ),
        (
            Block::Toggle {
                summary: parse_inline("more", &[]),
                blocks: vec![],
            },
            "toggle",
        ),
    ];
    for (block, name) in excluded {
        let err: SubsetError = validate_subtree(std::slice::from_ref(&block)).unwrap_err();
        assert_eq!(
            err.excluded, name,
            "the chat parser rejects + names `{name}`"
        );
        assert!(
            MessageBody::new(vec![block.clone()]).is_err(),
            "MessageBody::new rejects `{name}`"
        );
        let nested = Block::Table {
            columns: vec![Column {
                header: parse_inline("c", &[]),
            }],
            rows: vec![vec![Cell {
                blocks: vec![block],
            }]],
        };
        assert_eq!(validate_subtree(&[nested]).unwrap_err().excluded, name);
    }
}

#[test]
fn stored_ast_round_trip_is_canonical() {
    let body = MessageBody::new(vec![Block::Paragraph {
        inline: parse_inline("a*b", &[]),
    }])
    .unwrap();
    assert!(
        body.round_trips(),
        "the stored (canonical) AST is a fixed point"
    );
    if let Block::Paragraph { inline } = &body.blocks[0] {
        assert_eq!(serialize_inline(inline), r"a\*b");
    } else {
        panic!("expected a paragraph");
    }
}
