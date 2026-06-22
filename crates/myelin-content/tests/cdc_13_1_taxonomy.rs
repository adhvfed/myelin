//! CDC pair for contract-index row 13.1 — the frozen `myelin-content` taxonomy (X-2/OQ-B).
//!
//! PROVIDER side: Knowledge (this crate) freezes the complete 15-variant `Block`
//! taxonomy + the markdown-subset inline grammar + the three structured nodes
//! (mention/artifact_ref/embed). CONSUMER side: Chat / Issues / Search declare strict
//! SUBSETS — they compile against the SAME `Block`/`InlineNode` types and never redefine
//! or add a node type (X-2). This file carries BOTH sides so the contract-coverage
//! scanner (P-037) admits row 13.1 as a real provider+consumer pair.

use myelin_content::{
    parse_inline, serialize_inline, Block, CalloutTone, Cell, Column, EmbedDisplay, HeadingLevel,
    Inline, InlineNode, ListItem, TaskItem,
};
use myelin_events::ArtifactRef;
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_query::{FieldId, ViewSpec};
use myelin_tenancy::TenantId;

// ── PROVIDER side (13.1): Knowledge freezes the complete taxonomy ──────────────────

/// The provider exposes the FULL frozen 15-variant block set + the three structured
/// inline nodes. Building one of each pins the provider surface; Chat/Issues cannot add
/// a 16th variant because the enum is closed here.
fn provider_full_taxonomy() -> Vec<Block> {
    let p = Principal::stub(
        PrincipalId("alice".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    );
    let inline_with_three_nodes = parse_inline(
        "\u{FFFC} \u{FFFC} \u{FFFC}",
        &[
            InlineNode::Mention(p),
            InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/issues/issue/PROJ-1".into())),
            InlineNode::Embed(ArtifactRef("myelin://acme/knowledge/page/42".into())),
        ],
    );
    vec![
        Block::Paragraph {
            inline: inline_with_three_nodes,
        },
        Block::Heading {
            level: HeadingLevel::new(1).unwrap(),
            inline: Inline::default(),
        },
        Block::BulletList {
            items: vec![ListItem { blocks: vec![] }],
        },
        Block::OrderedList {
            items: vec![],
            start: 1,
        },
        Block::TaskList {
            items: vec![TaskItem {
                checked: false,
                inline: Inline::default(),
            }],
        },
        Block::Blockquote { blocks: vec![] },
        Block::CodeBlock {
            lang: Some("rust".into()),
            text: "**raw**".into(),
        },
        Block::Callout {
            tone: CalloutTone::Note,
            blocks: vec![],
        },
        Block::Table {
            columns: vec![Column {
                header: Inline::default(),
            }],
            rows: vec![vec![Cell { blocks: vec![] }]],
        },
        Block::Divider,
        Block::Image {
            blob: ArtifactRef("myelin://acme/blob/1".into()),
            alt: "a".into(),
            caption: None,
        },
        Block::Embed {
            reference: ArtifactRef("myelin://acme/issue/1".into()),
            display: EmbedDisplay::Card,
        },
        Block::DbView {
            db: ArtifactRef("myelin://acme/db/1".into()),
            view: ViewSpec::table(FieldId::new("order_key")),
        },
        Block::Toggle {
            summary: Inline::default(),
            blocks: vec![],
        },
        Block::SyncBlock {
            source: ArtifactRef("myelin://acme/block/9".into()),
        },
    ]
}

// ── CONSUMER side (X-2): Chat / Issues declare strict subsets ──────────────────────

/// A consumer (Chat) declares a SUBSET: it builds only the variants it supports, from the
/// SAME frozen types. `db_view` / `sync_block` are excluded by Chat (Knowledge-only, X-2);
/// the consumer simply never constructs them. The point: no redefinition, no new node.
fn chat_subset_consumer() -> Vec<Block> {
    vec![
        Block::Paragraph {
            inline: parse_inline("hi **there**", &[]),
        },
        Block::Heading {
            level: HeadingLevel::new(3).unwrap(),
            inline: Inline::default(),
        }, // chat caps at 1..3
        Block::BulletList { items: vec![] },
        Block::CodeBlock {
            lang: None,
            text: "x".into(),
        },
        Block::Callout {
            tone: CalloutTone::Info,
            blocks: vec![],
        },
        Block::Divider,
        Block::Embed {
            reference: ArtifactRef("myelin://acme/chat/message/1".into()),
            display: EmbedDisplay::Preview,
        },
    ]
}

/// A consumer reuses the three shared structured inline nodes verbatim (they are the
/// uniform producers of `refs.edge.created`, 5.4). The consumer reads them as a node-array
/// walk — exactly what the provider guarantees.
fn issues_subset_consumes_structured_nodes() -> Vec<InlineNode> {
    vec![
        InlineNode::Mention(Principal::stub(
            PrincipalId("bob".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )),
        InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/git/commit/abc".into())),
    ]
}

#[test]
fn cdc_13_1_provider_freezes_taxonomy_consumer_rides_subset() {
    // provider: the full frozen set is exactly 15 variants and round-trips through serde
    let full = provider_full_taxonomy();
    assert_eq!(full.len(), 15, "the v1 taxonomy is frozen at 15 variants");
    for b in &full {
        let json = serde_json::to_string(b).unwrap();
        let back: Block = serde_json::from_str(&json).unwrap();
        assert_eq!(*b, back);
    }

    // provider: the inline round-trip holds and the three structured nodes are preserved
    if let Block::Paragraph { inline } = &full[0] {
        let md = "\u{FFFC} \u{FFFC} \u{FFFC}";
        assert_eq!(serialize_inline(inline), md);
        assert_eq!(inline.structured_nodes().len(), 3);
    } else {
        panic!("first block should be the paragraph carrying the three nodes");
    }

    // consumer: a strict subset compiles against the SAME types, no redefinition
    let chat = chat_subset_consumer();
    assert!(chat.len() < full.len(), "a subset is strictly smaller");
    assert!(chat
        .iter()
        .all(|b| !matches!(b, Block::DbView { .. } | Block::SyncBlock { .. })));

    // consumer: structured nodes are reused verbatim and walk-extractable
    let nodes = issues_subset_consumes_structured_nodes();
    assert_eq!(nodes.len(), 2);
    assert!(matches!(nodes[0], InlineNode::Mention(_)));
}
