use myelin_content::{
    parse_inline, serialize_inline, wasm, Block, Cell, Column, Inline, InlineNode, ListItem,
};
use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EventDraft, EventEnvelope, EventId, EventType, OutboxTx,
    Result as BusResult, Visibility,
};
use myelin_identity::Principal;

pub const CHAT_EXCLUDED_BLOCKS: [&str; 3] = ["db_view", "sync_block", "toggle"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubsetError {
    pub excluded: &'static str,
}

impl std::fmt::Display for SubsetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "block `{}` is Knowledge-only and not in the Chat content subset (X-2) - rejected, not dropped",
            self.excluded
        )
    }
}

impl std::error::Error for SubsetError {}

pub fn is_chat_block(block: &Block) -> bool {
    !matches!(
        block,
        Block::DbView { .. } | Block::SyncBlock { .. } | Block::Toggle { .. }
    )
}

fn excluded_name(block: &Block) -> Option<&'static str> {
    match block {
        Block::DbView { .. } => Some("db_view"),
        Block::SyncBlock { .. } => Some("sync_block"),
        Block::Toggle { .. } => Some("toggle"),
        _ => None,
    }
}

pub fn validate_subtree(blocks: &[Block]) -> Result<(), SubsetError> {
    for block in blocks {
        if let Some(excluded) = excluded_name(block) {
            return Err(SubsetError { excluded });
        }
        match block {
            Block::Blockquote { blocks } | Block::Callout { blocks, .. } => {
                validate_subtree(blocks)?;
            }
            Block::BulletList { items } | Block::OrderedList { items, .. } => {
                for ListItem { blocks } in items {
                    validate_subtree(blocks)?;
                }
            }
            Block::Table { rows, columns } => {
                let _ = columns;
                for row in rows {
                    for Cell { blocks } in row {
                        validate_subtree(blocks)?;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn inline_runs(blocks: &[Block]) -> Vec<&Inline> {
    let mut out = Vec::new();
    collect_inlines(blocks, &mut out);
    out
}

fn collect_inlines<'a>(blocks: &'a [Block], out: &mut Vec<&'a Inline>) {
    for block in blocks {
        match block {
            Block::Paragraph { inline } | Block::Heading { inline, .. } => out.push(inline),
            Block::TaskList { items } => {
                for item in items {
                    out.push(&item.inline);
                }
            }
            Block::Blockquote { blocks } | Block::Callout { blocks, .. } => {
                collect_inlines(blocks, out)
            }
            Block::BulletList { items } | Block::OrderedList { items, .. } => {
                for ListItem { blocks } in items {
                    collect_inlines(blocks, out);
                }
            }
            Block::Table { columns, rows } => {
                for Column { header } in columns {
                    out.push(header);
                }
                for row in rows {
                    for Cell { blocks } in row {
                        collect_inlines(blocks, out);
                    }
                }
            }
            Block::Image {
                caption: Some(caption),
                ..
            } => out.push(caption),
            _ => {}
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct MessageBody {
    pub blocks: Vec<Block>,
}

impl MessageBody {
    pub fn new(blocks: Vec<Block>) -> Result<MessageBody, SubsetError> {
        validate_subtree(&blocks)?;
        Ok(MessageBody { blocks })
    }

    pub fn empty() -> MessageBody {
        MessageBody::default()
    }

    pub fn round_trips(&self) -> bool {
        inline_runs(&self.blocks)
            .iter()
            .copied()
            .all(roundtrips_inline)
    }

    pub fn structured_nodes(&self) -> Vec<&InlineNode> {
        inline_runs(&self.blocks)
            .into_iter()
            .flat_map(|inline| inline.structured_nodes().iter())
            .collect()
    }
}

fn roundtrips_inline(inline: &Inline) -> bool {
    let md = serialize_inline(inline);
    let reparsed = wasm::render_parse(&md, &inline.nodes);
    wasm::render_serialize(&reparsed) == md
}

pub fn roundtrips_md(md: &str, nodes: &[InlineNode]) -> bool {
    let parsed = wasm::render_parse(md, nodes);
    wasm::render_serialize(&parsed) == md
}

pub fn paragraph_body(md: &str, nodes: Vec<InlineNode>) -> MessageBody {
    let inline = parse_inline(md, &nodes);
    MessageBody {
        blocks: vec![Block::Paragraph { inline }],
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EdgeRel {
    Mentions,
    Links,
    Embeds,
}

impl EdgeRel {
    pub fn as_str(self) -> &'static str {
        match self {
            EdgeRel::Mentions => "mentions",
            EdgeRel::Links => "links",
            EdgeRel::Embeds => "embeds",
        }
    }
}

pub const REL_CLASS_REFERENCE: &str = "reference";

pub const REFS_EDGE_CREATED: &str = "refs.edge.created";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BodyEdge {
    pub source: ArtifactRef,
    pub target: ArtifactRef,
    pub rel: EdgeRel,
}

pub fn edge_aggregate_key(source: &ArtifactRef, target: &ArtifactRef) -> AggregateKey {
    myelin_refs::edge_aggregate_key(source, target)
}

fn principal_member_ref(p: &Principal) -> ArtifactRef {
    ArtifactRef(format!(
        "myelin://{}/identity/member/{}",
        p.tenant.0, p.principal_id.0
    ))
}

pub fn extract_body_edges(source: &ArtifactRef, nodes: &[InlineNode]) -> Vec<BodyEdge> {
    nodes
        .iter()
        .map(|node| {
            let (target, rel) = match node {
                InlineNode::Mention(principal) => {
                    (principal_member_ref(principal), EdgeRel::Mentions)
                }
                InlineNode::ArtifactRefNode(target) => (target.clone(), EdgeRel::Links),
                InlineNode::Embed(target) => (target.clone(), EdgeRel::Embeds),
            };
            BodyEdge {
                source: source.clone(),
                target,
                rel,
            }
        })
        .collect()
}

pub fn extract_message_edges(source: &ArtifactRef, body: &MessageBody) -> Vec<BodyEdge> {
    let nodes: Vec<InlineNode> = body.structured_nodes().into_iter().cloned().collect();
    extract_body_edges(source, &nodes)
}

pub(crate) fn edge_event_draft(edge: &BodyEdge) -> EventDraft {
    EventDraft {
        type_: EventType(REFS_EDGE_CREATED.into()),
        subject: edge.source.clone(),
        aggregate: edge_aggregate_key(&edge.source, &edge.target),
        payload: serde_json::json!({
            "source": edge.source.0,
            "target": edge.target.0,
            "rel": edge.rel.as_str(),
            "rel_class": REL_CLASS_REFERENCE,
        }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

pub fn emit_body_edges(
    tx: &mut dyn OutboxTx,
    source: &ArtifactRef,
    nodes: &[InlineNode],
    content_event: &EventEnvelope,
) -> BusResult<Vec<EventId>> {
    let edges = extract_body_edges(source, nodes);
    let mut ids = Vec::with_capacity(edges.len());
    for edge in &edges {
        let id = tx.emit(edge_event_draft(edge), Some(content_event))?;
        ids.push(id);
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_content::{CalloutTone, HeadingLevel, TaskItem, OBJ};
    use myelin_events::{
        Actor, AggregateKey, ArtifactRef, CausedBy, CorrelationId, EmitContextBase, EventEnvelope,
        MonotonicMinter, OutboxStore, Region, TenantId, Timestamp,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use std::sync::Arc;

    fn para(md: &str, nodes: Vec<InlineNode>) -> Block {
        Block::Paragraph {
            inline: parse_inline(md, &nodes),
        }
    }

    fn alice() -> Principal {
        Principal::stub(
            PrincipalId("p-opaque-alice".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    fn message_source() -> ArtifactRef {
        crate::subs::mint_message("acme", "01J0MSGULID").unwrap()
    }

    #[test]
    fn admitted_chat_subset_is_valid_and_round_trips() {
        let blocks = vec![
            para("a **rich** message", vec![]),
            Block::Heading {
                level: HeadingLevel::new(3).unwrap(),
                inline: parse_inline("**Heading**", &[]),
            },
            Block::BulletList {
                items: vec![ListItem {
                    blocks: vec![para("item", vec![])],
                }],
            },
            Block::TaskList {
                items: vec![TaskItem {
                    checked: true,
                    inline: parse_inline("done", &[]),
                }],
            },
            Block::Blockquote {
                blocks: vec![para("quoted", vec![])],
            },
            Block::CodeBlock {
                lang: Some("rust".into()),
                text: "let x = **not bold**;".into(),
            },
            Block::Callout {
                tone: CalloutTone::Warn,
                blocks: vec![para("note", vec![])],
            },
            Block::Table {
                columns: vec![Column {
                    header: parse_inline("col", &[]),
                }],
                rows: vec![vec![Cell {
                    blocks: vec![para("cell", vec![])],
                }]],
            },
            Block::Divider,
            Block::Image {
                blob: ArtifactRef("myelin://acme/blob/1".into()),
                alt: "a".into(),
                caption: Some(parse_inline("*cap*", &[])),
            },
        ];
        assert!(validate_subtree(&blocks).is_ok(), "the subset is admitted");
        let body = MessageBody::new(blocks).expect("in-subset");
        assert!(
            body.round_trips(),
            "render(parse(md)) === md over the subtree"
        );
    }

    #[test]
    fn knowledge_only_blocks_are_rejected() {
        use myelin_query::{FieldId, ViewSpec};
        let excluded = [
            Block::DbView {
                db: ArtifactRef("myelin://acme/db/1".into()),
                view: ViewSpec::table(FieldId::new("order_key")),
            },
            Block::SyncBlock {
                source: ArtifactRef("myelin://acme/block/9".into()),
            },
            Block::Toggle {
                summary: parse_inline("more", &[]),
                blocks: vec![],
            },
        ];
        for (block, name) in excluded.into_iter().zip(CHAT_EXCLUDED_BLOCKS) {
            assert!(!is_chat_block(&block), "{name} is out of subset");
            let err = validate_subtree(std::slice::from_ref(&block)).unwrap_err();
            assert_eq!(err.excluded, name, "the error names the excluded variant");
            assert!(MessageBody::new(vec![block]).is_err());
        }
    }

    #[test]
    fn nested_knowledge_only_block_is_rejected() {
        let smuggled = Block::Blockquote {
            blocks: vec![
                para("ok", vec![]),
                Block::SyncBlock {
                    source: ArtifactRef("myelin://acme/block/9".into()),
                },
            ],
        };
        assert_eq!(
            validate_subtree(&[smuggled]).unwrap_err().excluded,
            "sync_block"
        );

        let in_list = Block::BulletList {
            items: vec![ListItem {
                blocks: vec![Block::Toggle {
                    summary: parse_inline("x", &[]),
                    blocks: vec![],
                }],
            }],
        };
        assert_eq!(validate_subtree(&[in_list]).unwrap_err().excluded, "toggle");

        let in_table = Block::Table {
            columns: vec![Column {
                header: parse_inline("c", &[]),
            }],
            rows: vec![vec![Cell {
                blocks: vec![Block::DbView {
                    db: ArtifactRef("myelin://acme/db/1".into()),
                    view: myelin_query::ViewSpec::table(myelin_query::FieldId::new("k")),
                }],
            }]],
        };
        assert_eq!(
            validate_subtree(&[in_table]).unwrap_err().excluded,
            "db_view"
        );
    }

    #[test]
    fn subset_error_display_names_the_offender() {
        let msg = SubsetError {
            excluded: "sync_block",
        }
        .to_string();
        assert!(msg.contains("sync_block"), "names the offender: {msg}");
        assert!(
            msg.to_lowercase().contains("rejected") && !msg.to_lowercase().contains("dropped, ok"),
            "loud about not dropping: {msg}"
        );
    }

    #[test]
    fn body_round_trips_byte_identical_via_wasm_path() {
        let md = format!("**bold** and *italic* with `code` and a {OBJ} mention");
        let nodes = vec![InlineNode::Mention(alice())];
        assert!(
            roundtrips_md(&md, &nodes),
            "render(parse(md)) === md via the WASM path"
        );
        let body = paragraph_body(&md, nodes);
        assert!(body.round_trips());
    }

    #[test]
    fn empty_body_round_trips_and_has_no_edges() {
        let body = MessageBody::empty();
        assert!(body.round_trips());
        assert!(extract_message_edges(&message_source(), &body).is_empty());
    }

    #[test]
    fn round_trips_md_is_false_on_non_canonical_body() {
        assert!(
            !roundtrips_md("a*b", &[]),
            "a non-canonical source body must NOT round-trip byte-exact"
        );
        assert!(
            roundtrips_md(r"a\*b", &[]),
            "the canonical form IS a fixed point"
        );
    }

    #[test]
    fn message_body_round_trips_is_ast_idempotent() {
        let from_non_canonical = paragraph_body("a*b", vec![]);
        assert!(
            from_non_canonical.round_trips(),
            "the stored (canonical) AST is always a fixed point"
        );
        if let Block::Paragraph { inline } = &from_non_canonical.blocks[0] {
            assert_eq!(serialize_inline(inline), r"a\*b");
        } else {
            panic!("expected a paragraph body");
        }
    }

    #[test]
    fn wasm_path_is_identical_to_native_parse() {
        let md = "**a** `b` ~~c~~ [t](u)";
        let native = serialize_inline(&parse_inline(md, &[]));
        let via_wasm = wasm::render_serialize(&wasm::render_parse(md, &[]));
        assert_eq!(native, via_wasm, "one renderer, native === wasm path");
        assert_eq!(native, md, "and it round-trips");
    }

    #[test]
    fn each_node_kind_yields_one_edge_with_correct_rel_and_target() {
        let src = message_source();
        let page = ArtifactRef("myelin://acme/knowledge/page/7c2".into());
        let issue = ArtifactRef("myelin://acme/issue/issue/ENG-1".into());
        let nodes = vec![
            InlineNode::Mention(alice()),
            InlineNode::ArtifactRefNode(issue.clone()),
            InlineNode::Embed(page.clone()),
        ];
        let edges = extract_body_edges(&src, &nodes);
        assert_eq!(edges.len(), 3, "N structured nodes → N edges (1 per node)");

        assert_eq!(edges[0].rel, EdgeRel::Mentions);
        assert_eq!(edges[0].rel.as_str(), "mentions");
        assert_eq!(edges[0].source, src);
        assert_eq!(
            edges[0].target.0, "myelin://acme/identity/member/p-opaque-alice",
            "mention target is the pseudonymous member URN, never the name"
        );

        assert_eq!(edges[1].rel, EdgeRel::Links);
        assert_eq!(edges[1].rel.as_str(), "links");
        assert_eq!(edges[1].target, issue);

        assert_eq!(edges[2].rel, EdgeRel::Embeds);
        assert_eq!(edges[2].rel.as_str(), "embeds");
        assert_eq!(edges[2].target, page);
    }

    #[test]
    fn prose_reference_is_not_a_content_edge() {
        let body = paragraph_body("see myelin://acme/issue/ENG-1 and ping @alice", vec![]);
        assert!(body.round_trips());
        let edges = extract_message_edges(&message_source(), &body);
        assert!(edges.is_empty(), "a prose reference is NOT a content edge");
    }

    #[test]
    fn structured_nodes_walks_the_whole_subtree() {
        let page = ArtifactRef("myelin://acme/knowledge/page/7".into());
        let body = MessageBody::new(vec![
            para(&format!("hi {OBJ}"), vec![InlineNode::Mention(alice())]),
            Block::BulletList {
                items: vec![ListItem {
                    blocks: vec![para(
                        &format!("see {OBJ}"),
                        vec![InlineNode::Embed(page.clone())],
                    )],
                }],
            },
        ])
        .unwrap();
        let nodes = body.structured_nodes();
        assert_eq!(nodes.len(), 2, "both structured nodes are reached");
        assert!(matches!(nodes[0], InlineNode::Mention(_)));
        assert!(matches!(nodes[1], InlineNode::Embed(_)));

        let edges = extract_message_edges(&message_source(), &body);
        assert_eq!(edges.len(), 2);
        assert_eq!(edges[0].rel, EdgeRel::Mentions);
        assert_eq!(edges[1].rel, EdgeRel::Embeds);
    }

    #[test]
    fn edge_event_draft_is_refs_edge_created_with_the_triple() {
        let src = message_source();
        let target = ArtifactRef("myelin://acme/knowledge/page/7c2#block-3".into());
        let edge = BodyEdge {
            source: src.clone(),
            target: target.clone(),
            rel: EdgeRel::Embeds,
        };
        let draft = edge_event_draft(&edge);
        assert_eq!(draft.type_.0, "refs.edge.created");
        assert_eq!(draft.subject, src, "the subject is the referencing body");
        assert_eq!(draft.payload["source"], src.0);
        assert_eq!(draft.payload["target"], target.0);
        assert_eq!(draft.payload["rel"], "embeds");
        assert_eq!(draft.payload["rel_class"], "reference");
        assert_eq!(draft.aggregate, edge_aggregate_key(&src, &target));
        assert!(
            !draft.contains_personal_data,
            "references-not-payloads: no inline PII"
        );
        assert!(draft.pii_key_ref.is_none());
        assert_eq!(draft.data_role, DataRole::Controller);
    }

    #[test]
    fn frozen_tokens_match_the_refs_wire_shape() {
        assert_eq!(REFS_EDGE_CREATED, "refs.edge.created");
        assert_eq!(REL_CLASS_REFERENCE, "reference");
        assert_eq!(EdgeRel::Mentions.as_str(), "mentions");
        assert_eq!(EdgeRel::Links.as_str(), "links");
        assert_eq!(EdgeRel::Embeds.as_str(), "embeds");
    }

    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: TenantId("acme".into()),
            region: Region("fr-par".into()),
            actor: Actor(alice()),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-21T10:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T10:00:01Z".into()),
            caused_by: Some(CausedBy("session:abc".into())),
        }
    }

    fn content_event(source: &ArtifactRef) -> EventEnvelope {
        EventEnvelope {
            event_id: myelin_events::EventId("01J-msg".into()),
            type_: EventType(crate::events::CHAT_MESSAGE_CREATED.into()),
            schema_ver: 1,
            tenant: TenantId("acme".into()),
            region: Region("fr-par".into()),
            actor: Actor(alice()),
            subject: source.clone(),
            aggregate: AggregateKey("chat:conv:01J0CONV".into()),
            causation_id: None,
            correlation_id: CorrelationId("01J-msg-corr".into()),
            caused_by: Some(CausedBy("session:abc".into())),
            depth: 1,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-21T10:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T10:00:01Z".into()),
            payload: serde_json::json!({ "message": source.0 }),
        }
    }

    #[test]
    fn message_edges_co_commit_with_the_content_event() {
        let store = OutboxStore::new();
        let minter: Arc<dyn myelin_events::IdMinter> = Arc::new(MonotonicMinter::new());
        let src = message_source();

        let body = MessageBody::new(vec![para(
            &format!("hi {OBJ} see {OBJ}"),
            vec![
                InlineNode::Mention(alice()),
                InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/issue/issue/ENG-1".into())),
            ],
        )])
        .unwrap();

        let cause = content_event(&src);
        let mut tx = store.begin(minter, ctx_base());
        tx.stage_state_change("chat message 01J0MSGULID body written");
        let nodes: Vec<InlineNode> = body.structured_nodes().into_iter().cloned().collect();
        let ids = emit_body_edges(&mut tx, &src, &nodes, &cause).unwrap();
        tx.commit().unwrap();

        assert_eq!(ids.len(), 2, "one edge event per structured node");
        for id in &ids {
            let row = store.row(id).expect("the committed edge row is present");
            assert_eq!(row.envelope.type_.0, "refs.edge.created");
            assert_eq!(
                row.envelope.correlation_id, cause.correlation_id,
                "the edge inherits the message's correlation root"
            );
            assert_eq!(
                row.envelope.causation_id.as_ref().unwrap().0,
                cause.event_id.0
            );
            assert_eq!(
                row.envelope.depth,
                cause.depth + 1,
                "depth+1 loop-guard stamp"
            );
            let payload = serde_json::to_string(&row.envelope.payload).unwrap();
            assert!(!payload.contains("hi "), "no inline body on the wire");
        }
    }
}
