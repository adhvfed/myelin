//! # CDC 13.1 — Issues CONSUMES the frozen `myelin-content` block subset (ISS-P10 / P-376)
//!
//! The CONSUMER leg of contract-index row 13.1 from the Issues side (the provider leg is
//! `crates/myelin-content/tests/cdc_13_1_taxonomy.rs`; Knowledge LEADS + freezes the taxonomy). This
//! pins the Issues-side agreement (X-2): Issues consumes a **strict SUBSET** of the frozen [`Block`]
//! taxonomy — the full set MINUS the three Knowledge-only nodes (`db_view`/`sync_block`/`toggle`) —
//! and it does so by LINKING the frozen types (`myelin_content::Block`/`Inline`/`InlineNode`), never
//! re-defining a node (EI-01 §7). The drift-killer: if Knowledge changed the taxonomy (added/removed
//! a node, renamed a variant) or if Issues' subset policy drifted, this CDC fails alongside the
//! provider's.
//!
//! It also pins that Issues' body/comment content round-trips `render(parse(md)) === md` through the
//! ONE WASM render path (the SAME parse/serialize compiled native + wasm32 — read + edit use the
//! identical parser, no second renderer) and that the three structured inline ref nodes are reused
//! verbatim (the uniform producers of `refs.edge.created`, 5.4 — a node-array walk, never a regex).

use myelin_content::{Block, CalloutTone, HeadingLevel, InlineNode};
use myelin_events::ArtifactRef;
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_issues::{
    is_issue_block, paragraph_body, roundtrips_md, validate_subtree, ContentKind, IssueContent,
    ISSUES_EXCLUDED_BLOCKS,
};
use myelin_query::{FieldId, ViewSpec};
use myelin_tenancy::TenantId;

/// The Issues consumed block SUBSET — the variants an issue body/comment authors, all built from the
/// SAME frozen [`Block`] types (no redefinition). It EXCLUDES the three Knowledge-only nodes.
fn issues_block_subset() -> Vec<Block> {
    vec![
        Block::Paragraph {
            inline: myelin_content::parse_inline("the **description** body", &[]),
        },
        Block::Heading {
            level: HeadingLevel::new(2).unwrap(),
            inline: myelin_content::parse_inline("Steps", &[]),
        },
        Block::BulletList { items: vec![] },
        Block::OrderedList {
            items: vec![],
            start: 1,
        },
        Block::TaskList { items: vec![] },
        Block::Blockquote { blocks: vec![] },
        Block::CodeBlock {
            lang: Some("rust".into()),
            text: "let x = 1;".into(),
        },
        Block::Callout {
            tone: CalloutTone::Note,
            blocks: vec![],
        },
        Block::Table {
            columns: vec![],
            rows: vec![],
        },
        Block::Divider,
        Block::Image {
            blob: ArtifactRef("myelin://acme/blob/1".into()),
            alt: "screenshot".into(),
            caption: None,
        },
        Block::Embed {
            reference: ArtifactRef("myelin://acme/issue/issue/ENG-9".into()),
            display: myelin_content::EmbedDisplay::Card,
        },
    ]
}

/// The three Knowledge-only blocks Issues' subset EXCLUDES (X-2) — built from the frozen types to
/// prove Issues rejects them, not that it cannot construct them.
fn knowledge_only_blocks() -> Vec<Block> {
    vec![
        Block::DbView {
            db: ArtifactRef("myelin://acme/db/1".into()),
            view: ViewSpec::table(FieldId::new("order_key")),
        },
        Block::SyncBlock {
            source: ArtifactRef("myelin://acme/block/9".into()),
        },
        Block::Toggle {
            summary: myelin_content::parse_inline("more", &[]),
            blocks: vec![],
        },
    ]
}

#[test]
fn cdc_13_1_issues_consumes_strict_subset_and_rejects_knowledge_only() {
    // CONSUMER: the Issues subset is built from the SAME frozen types + is admitted by the subset
    // validator (a strict subset — every variant is in-subset).
    let subset = issues_block_subset();
    assert!(
        validate_subtree(&subset).is_ok(),
        "the whole Issues subset is admitted"
    );
    assert!(
        subset.iter().all(is_issue_block),
        "every Issues block is in-subset"
    );
    // and a content document built from it is in-subset.
    assert!(IssueContent::new(ContentKind::Body, subset).is_ok());

    // CONSUMER (X-2): the three Knowledge-only nodes are REJECTED, never silently dropped — and the
    // excluded-name set is exactly {db_view, sync_block, toggle}.
    let excluded = knowledge_only_blocks();
    assert_eq!(excluded.len(), ISSUES_EXCLUDED_BLOCKS.len());
    for (block, name) in excluded.into_iter().zip(ISSUES_EXCLUDED_BLOCKS) {
        assert!(
            !is_issue_block(&block),
            "{name} is out of the Issues subset"
        );
        assert_eq!(
            validate_subtree(&[block]).unwrap_err().excluded,
            name,
            "the rejection names the excluded variant {name}"
        );
    }
}

#[test]
fn cdc_13_1_issues_body_round_trips_through_the_one_wasm_render_path() {
    use myelin_content::OBJ;
    // read + edit use the IDENTICAL WASM parser (the editor entry). render(parse(md)) === md.
    let md = format!("a **bold** {OBJ} body with `code`");
    let nodes = vec![InlineNode::Mention(Principal::stub(
        PrincipalId("p-opaque-alice".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    ))];
    assert!(
        roundtrips_md(&md, &nodes),
        "issue body round-trips via the ONE WASM render path"
    );
    let body = paragraph_body(&md, &nodes);
    assert!(body.round_trips());
    // the three structured nodes are reused verbatim + walk-extractable (5.4 — node-array walk).
    assert_eq!(body.structured_nodes().len(), 1);
    assert!(matches!(body.structured_nodes()[0], InlineNode::Mention(_)));
}
