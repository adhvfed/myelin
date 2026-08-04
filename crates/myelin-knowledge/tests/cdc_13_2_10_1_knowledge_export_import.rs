use myelin_content::adf::mapping_for;
use myelin_content::{parse_inline, serialize_inline, AdfNode, AdfTarget, Block, Loss, MAP};
use myelin_knowledge::{import_adf, ExportBlock, ExportDoc, ParsedAdfNode};

fn provider_freezes_map() -> &'static [myelin_content::AdfMapping] {
    MAP
}

#[test]
fn cdc_13_2_knowledge_import_builds_against_frozen_map_and_records_losses() {
    let map = provider_freezes_map();
    assert_eq!(map.len(), 25, "the frozen ADF map is exactly 25 node rows");

    let lossless = [
        ParsedAdfNode::resolved(AdfNode::Paragraph, "p"),
        ParsedAdfNode::resolved(AdfNode::Heading, "h"),
        ParsedAdfNode::resolved(AdfNode::BulletList, "i"),
        ParsedAdfNode::resolved(AdfNode::Expand, "x"),
    ];
    let r = import_adf("pg", "Lossless import", &lossless);
    assert!(
        r.report.is_lossless(),
        "a direct-equivalent import loses nothing"
    );
    assert_eq!(r.doc.blocks.len(), 4);

    let lossy = [
        ParsedAdfNode::unresolved(AdfNode::Mention, "external"),
        ParsedAdfNode::resolved(AdfNode::Status, "In Progress"),
        ParsedAdfNode::resolved(AdfNode::Extension, "macro"),
        ParsedAdfNode::resolved(AdfNode::Paragraph, "kept"),
    ];
    let r = import_adf("pg", "Lossy import", &lossy);
    assert_eq!(
        r.report.loss_count(),
        3,
        "three lossy nodes recorded, the lossless one not"
    );
    assert_eq!(r.report.conversions[0].node, AdfNode::Mention);
    assert_eq!(r.report.conversions[0].degraded_to, AdfTarget::PlainText);

    for node in [
        AdfNode::Paragraph,
        AdfNode::Status,
        AdfNode::Extension,
        AdfNode::Mention,
    ] {
        let m = mapping_for(node);
        match &m.loss {
            Loss::None | Loss::Lossy { .. } => {
                let _ = m.target;
            }
            Loss::Conditional { degraded_to, .. } => {
                let _ = degraded_to;
            }
        }
    }
}

fn provider_export_then_import(doc: &ExportDoc) -> ExportDoc {
    let json = doc.to_json_bundle();
    ExportDoc::from_json_bundle(&json).expect("the lossless bundle re-imports")
}

fn gdpr_export_subject_bundle(subject_page: &str) -> String {
    let doc = ExportDoc::new(
        subject_page,
        "Subject portable export",
        None,
        vec![
            ExportBlock::leaf(
                "b1",
                Block::Paragraph {
                    inline: parse_inline("some **content**", &[]),
                },
            ),
            ExportBlock::leaf(
                "b2",
                Block::Paragraph {
                    inline: parse_inline("a `code` run", &[]),
                },
            ),
        ],
    );
    doc.to_json_bundle()
}

#[test]
fn cdc_10_1_export_service_is_the_art20_lossless_mechanism() {
    let doc = ExportDoc::new(
        "page-x",
        "Runbook",
        None,
        vec![
            ExportBlock::leaf(
                "b1",
                Block::Paragraph {
                    inline: parse_inline("**bold** and *italic*", &[]),
                },
            ),
            ExportBlock::with_children(
                "b2",
                Block::Paragraph {
                    inline: parse_inline("parent", &[]),
                },
                vec![ExportBlock::leaf(
                    "b2a",
                    Block::Paragraph {
                        inline: parse_inline("child", &[]),
                    },
                )],
            ),
        ],
    );
    let back = provider_export_then_import(&doc);
    assert_eq!(
        back, doc,
        "the bundle is a byte-faithful lossless round-trip (Art. 20)"
    );
    assert!(
        doc.json_roundtrips(),
        "the export/import round-trip gate is green (render(parse(md))===md)"
    );

    let bundle = gdpr_export_subject_bundle("subject/page/1");
    let parsed =
        ExportDoc::from_json_bundle(&bundle).expect("the holder's bundle is a valid export");
    assert_eq!(parsed.page_id.as_str(), "subject/page/1");
    let md = parsed
        .all_blocks()
        .iter()
        .filter_map(|b| match &b.block {
            Block::Paragraph { inline } => Some(serialize_inline(inline)),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        md.contains("**content**"),
        "the subject's bold content survives the export: {md}"
    );
    assert!(
        md.contains("`code`"),
        "the subject's code run survives the export: {md}"
    );
}
