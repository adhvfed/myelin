use myelin_content::{Block, CalloutTone, HeadingLevel, InlineNode};
use myelin_events::ArtifactRef;
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_issues::{
    is_issue_block, paragraph_body, roundtrips_md, validate_subtree, ContentKind, IssueContent,
    ISSUES_EXCLUDED_BLOCKS,
};
use myelin_query::{FieldId, ViewSpec};
use myelin_tenancy::TenantId;

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
    let subset = issues_block_subset();
    assert!(
        validate_subtree(&subset).is_ok(),
        "the whole Issues subset is admitted"
    );
    assert!(
        subset.iter().all(is_issue_block),
        "every Issues block is in-subset"
    );
    assert!(IssueContent::new(ContentKind::Body, subset).is_ok());

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
    assert_eq!(body.structured_nodes().len(), 1);
    assert!(matches!(body.structured_nodes()[0], InlineNode::Mention(_)));
}
