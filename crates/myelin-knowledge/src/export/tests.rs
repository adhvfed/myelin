//! Unit tests for the Export/Import service (KN-P24 / P-314) — the lossless JSON round-trip, the
//! Markdown/HTML/PDF/CSV exporters, and the ADF lossy-map import report.

use super::*;
use crate::database::FieldDef;
use myelin_content::{
    parse_inline, Block, CalloutTone, Cell, Column, HeadingLevel, InlineNode, ListItem, TaskItem,
};
use myelin_content::{AdfNode, AdfTarget};
use myelin_query::{FieldId, FieldType, FieldValue, OrderKey};
use myelin_tenancy::ArtifactRef;
use std::collections::BTreeMap;

fn para(md: &str) -> Block {
    Block::Paragraph {
        inline: parse_inline(md, &[]),
    }
}

/// A rich document exercising many of the frozen block variants + nested structure + inline marks +
/// a structured node — the round-trip fixture.
fn rich_doc() -> ExportDoc {
    let nodes = vec![InlineNode::ArtifactRefNode(ArtifactRef(
        "myelin://acme/issues/issue/PROJ-1".into(),
    ))];
    ExportDoc::new(
        "page-1",
        "My Runbook",
        Some(PageId("team".into())),
        vec![
            ExportBlock::leaf(
                "b1",
                Block::Heading {
                    level: HeadingLevel::new(2).unwrap(),
                    inline: parse_inline("**Incident** response", &[]),
                },
            ),
            ExportBlock::leaf(
                "b2",
                para("Plain text with `code` and *italic* and a \u{FFFC} ref"),
            ),
            ExportBlock::with_children(
                "b3",
                para("A paragraph with children"),
                vec![ExportBlock::leaf("b3a", para("a nested child"))],
            ),
            ExportBlock::leaf(
                "b4",
                Block::BulletList {
                    items: vec![
                        ListItem {
                            blocks: vec![para("first item")],
                        },
                        ListItem {
                            blocks: vec![para("second **bold** item")],
                        },
                    ],
                },
            ),
            ExportBlock::leaf(
                "b5",
                Block::TaskList {
                    items: vec![
                        TaskItem {
                            checked: true,
                            inline: parse_inline("done", &[]),
                        },
                        TaskItem {
                            checked: false,
                            inline: parse_inline("todo", &[]),
                        },
                    ],
                },
            ),
            ExportBlock::leaf(
                "b6",
                Block::CodeBlock {
                    lang: Some("rust".into()),
                    text: "let x = **not bold**;".into(),
                },
            ),
            ExportBlock::leaf(
                "b7",
                Block::Callout {
                    tone: CalloutTone::Warn,
                    blocks: vec![para("careful!")],
                },
            ),
            ExportBlock::leaf(
                "b8",
                Block::Table {
                    columns: vec![
                        Column {
                            header: parse_inline("Name", &[]),
                        },
                        Column {
                            header: parse_inline("Role", &[]),
                        },
                    ],
                    rows: vec![vec![
                        Cell {
                            blocks: vec![para("Alice")],
                        },
                        Cell {
                            blocks: vec![para("SRE")],
                        },
                    ]],
                },
            ),
            ExportBlock::leaf("b9", Block::Divider),
            // a paragraph carrying a structured ref node (positional bind)
            ExportBlock::leaf(
                "b10",
                Block::Paragraph {
                    inline: parse_inline("see \u{FFFC} here", &nodes),
                },
            ),
        ],
    )
}

// ── lossless JSON (Art. 20, 10.1) ───────────────────────────────────────────────────────────

#[test]
fn lossless_json_roundtrips_byte_faithful() {
    let doc = rich_doc();
    let json = doc.to_json_bundle();
    let back = ExportDoc::from_json_bundle(&json).expect("re-import parses");
    assert_eq!(
        back, doc,
        "the JSON bundle is a byte-faithful lossless round-trip (Art. 20)"
    );
}

#[test]
fn json_roundtrip_gate_is_green() {
    // THE round-trip gate: structural identity AND render(parse(md))===md across the boundary.
    let doc = rich_doc();
    assert!(
        doc.json_roundtrips(),
        "the export/import round-trip gate is green (KN-D2 + Art. 20)"
    );
}

#[test]
fn structured_nodes_survive_the_json_roundtrip() {
    let doc = rich_doc();
    let back = ExportDoc::from_json_bundle(&doc.to_json_bundle()).unwrap();
    // b10 carries one ArtifactRefNode; it must survive positionally.
    let b10 = back
        .all_blocks()
        .into_iter()
        .find(|b| b.id.as_str() == "b10")
        .unwrap();
    match &b10.block {
        Block::Paragraph { inline } => {
            assert_eq!(inline.nodes.len(), 1, "the structured node survives");
            assert!(matches!(inline.nodes[0], InlineNode::ArtifactRefNode(_)));
        }
        _ => panic!("b10 is a paragraph"),
    }
}

#[test]
fn malformed_bundle_is_a_loud_error() {
    let err = ExportDoc::from_json_bundle("{not json").unwrap_err();
    assert!(matches!(err, ExportError::MalformedBundle(_)));
}

#[test]
fn code_block_text_is_verbatim_across_roundtrip() {
    let doc = rich_doc();
    let back = ExportDoc::from_json_bundle(&doc.to_json_bundle()).unwrap();
    let b6 = back
        .all_blocks()
        .into_iter()
        .find(|b| b.id.as_str() == "b6")
        .unwrap();
    match &b6.block {
        Block::CodeBlock { text, .. } => assert_eq!(text, "let x = **not bold**;"),
        _ => panic!("b6 is a code block"),
    }
}

// ── the markdown exporter ───────────────────────────────────────────────────────────────────

#[test]
fn markdown_exporter_renders_blocks() {
    let md = rich_doc().to_markdown();
    assert!(md.contains("# My Runbook"), "title is an H1: {md}");
    assert!(
        md.contains("## **Incident** response"),
        "heading renders: {md}"
    );
    assert!(md.contains("- first item"), "bullet list: {md}");
    assert!(md.contains("- [x] done"), "checked task: {md}");
    assert!(md.contains("- [ ] todo"), "unchecked task: {md}");
    assert!(md.contains("```rust"), "fenced code with lang: {md}");
    assert!(md.contains("let x = **not bold**;"), "verbatim code: {md}");
    assert!(md.contains("| Name | Role |"), "table header: {md}");
    assert!(md.contains("---"), "divider/table separator: {md}");
}

// ── the HTML exporter ───────────────────────────────────────────────────────────────────────

#[test]
fn html_exporter_is_a_self_contained_document() {
    let html = rich_doc().to_html();
    assert!(html.starts_with("<!DOCTYPE html>"), "a complete document");
    assert!(html.contains("<title>My Runbook</title>"));
    assert!(
        html.contains("<h2><strong>Incident</strong> response</h2>"),
        "marks → elements: {html}"
    );
    assert!(html.contains("<code>code</code>"), "inline code: {html}");
    assert!(html.contains("<em>italic</em>"), "italic: {html}");
    assert!(html.contains("<ul>"), "bullet list: {html}");
    assert!(
        html.contains("<input type=\"checkbox\" disabled checked>"),
        "checked task: {html}"
    );
    assert!(
        html.contains("<pre><code class=\"language-rust\">"),
        "code block lang class"
    );
    // code content is HTML-escaped (the `*` survives literally, no element injection)
    assert!(
        html.contains("let x = **not bold**;"),
        "code escaped verbatim: {html}"
    );
    assert!(html.contains("<table>"), "table: {html}");
    assert!(html.contains("<hr>"), "divider: {html}");
}

#[test]
fn html_escapes_dangerous_content() {
    let doc = ExportDoc::new(
        "p",
        "Title <script>",
        None,
        vec![ExportBlock::leaf("x", para("a < b & c > d"))],
    );
    let html = doc.to_html();
    assert!(
        !html.contains("<script>"),
        "the title is escaped, no injection"
    );
    assert!(html.contains("&lt;script&gt;"));
    assert!(
        html.contains("a &lt; b &amp; c &gt; d"),
        "body escaped: {html}"
    );
}

// ── the PDF exporter ────────────────────────────────────────────────────────────────────────

#[test]
fn pdf_exporter_emits_a_valid_pdf() {
    let pdf = rich_doc().to_pdf();
    assert!(pdf.starts_with(b"%PDF-1.4"), "the PDF header");
    let tail = &pdf[pdf.len().saturating_sub(8)..];
    assert!(
        tail.windows(5).any(|w| w == b"%%EOF"),
        "the PDF trailer/EOF"
    );
    let s = String::from_utf8_lossy(&pdf);
    assert!(s.contains("/Type /Catalog"), "catalog object");
    assert!(s.contains("/Type /Page"), "page object");
    assert!(s.contains("xref"), "the cross-reference table");
    assert!(s.contains("startxref"), "the startxref pointer");
    assert!(
        s.contains("My Runbook"),
        "the title is in the content stream"
    );
}

#[test]
fn pdf_escapes_parens_in_text() {
    let doc = ExportDoc::new(
        "p",
        "f(x) and (paren)",
        None,
        vec![ExportBlock::leaf("x", para("a (b) c"))],
    );
    let pdf = doc.to_pdf();
    let s = String::from_utf8_lossy(&pdf);
    assert!(
        s.contains("f\\(x\\)"),
        "parens escaped in the title literal: missing in {s:?}"
    );
}

// ── the CSV exporter (flexible DB) ──────────────────────────────────────────────────────────

fn schema() -> FieldSchema {
    FieldSchema::of(vec![
        FieldDef::new("name", FieldType::Text),
        FieldDef::new("count", FieldType::Int),
        FieldDef::new("active", FieldType::Bool),
    ])
    .unwrap()
}

fn row(id: &str, name: &str, count: i64, active: bool) -> DbRow {
    let mut props: BTreeMap<FieldId, FieldValue> = BTreeMap::new();
    props.insert(FieldId::new("name"), FieldValue::Text(name.into()));
    props.insert(FieldId::new("count"), FieldValue::Int(count));
    props.insert(FieldId::new("active"), FieldValue::Bool(active));
    DbRow::new(id, props, OrderKey::bisect(None, None))
}

#[test]
fn csv_exporter_renders_header_and_rows_in_schema_order() {
    let csv = export_rows_to_csv(
        &schema(),
        &[row("r1", "Alice", 3, true), row("r2", "Bob", 0, false)],
    );
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(lines[0], "name,count,active", "header in declared order");
    assert_eq!(lines[1], "Alice,3,true");
    assert_eq!(lines[2], "Bob,0,false");
}

#[test]
fn csv_quotes_special_characters_rfc4180() {
    let mut props: BTreeMap<FieldId, FieldValue> = BTreeMap::new();
    props.insert(
        FieldId::new("name"),
        FieldValue::Text("Smith, \"Bob\"".into()),
    );
    props.insert(FieldId::new("count"), FieldValue::Int(1));
    props.insert(FieldId::new("active"), FieldValue::Bool(true));
    let r = DbRow::new("r", props, OrderKey::bisect(None, None));
    let csv = export_rows_to_csv(&schema(), &[r]);
    assert!(
        csv.contains("\"Smith, \"\"Bob\"\"\""),
        "comma+quote escaped per RFC-4180: {csv}"
    );
}

#[test]
fn csv_absent_field_is_empty_cell() {
    let mut props: BTreeMap<FieldId, FieldValue> = BTreeMap::new();
    props.insert(FieldId::new("name"), FieldValue::Text("Only".into()));
    // count + active absent
    let r = DbRow::new("r", props, OrderKey::bisect(None, None));
    let csv = export_rows_to_csv(&schema(), &[r]);
    let data = csv.lines().nth(1).unwrap();
    assert_eq!(
        data, "Only,,",
        "absent fields are empty cells, schema order preserved"
    );
}

// ── the ADF lossy-map import (13.2) ─────────────────────────────────────────────────────────

#[test]
fn adf_import_lossless_document_records_nothing() {
    let nodes = [
        ParsedAdfNode::resolved(AdfNode::Heading, "Title"),
        ParsedAdfNode::resolved(AdfNode::Paragraph, "Some body text"),
        ParsedAdfNode::resolved(AdfNode::BulletList, "an item"),
        ParsedAdfNode::resolved(AdfNode::Expand, "details"), // → toggle, lossless
    ];
    let result = import_adf("p1", "Imported", &nodes);
    assert!(
        result.report.is_lossless(),
        "a direct-equivalent import records no loss"
    );
    assert_eq!(result.doc.blocks.len(), 4, "every node becomes a block");
    // the heading became a heading, the paragraph a paragraph
    assert!(matches!(result.doc.blocks[0].block, Block::Heading { .. }));
    assert!(matches!(
        result.doc.blocks[1].block,
        Block::Paragraph { .. }
    ));
    assert!(matches!(result.doc.blocks[3].block, Block::Toggle { .. }));
}

#[test]
fn adf_import_records_each_lossy_conversion() {
    let nodes = [
        ParsedAdfNode::unresolved(AdfNode::Mention, "external-user"), // → plain text (recorded)
        ParsedAdfNode::resolved(AdfNode::Status, "In Progress"),      // always → code (recorded)
        ParsedAdfNode::resolved(AdfNode::Extension, "jira-macro"), // macro → callout+marker (recorded)
        ParsedAdfNode::resolved(AdfNode::Paragraph, "ok"),         // lossless (not recorded)
    ];
    let result = import_adf("p1", "Imported", &nodes);
    assert_eq!(
        result.report.loss_count(),
        3,
        "three lossy nodes recorded, the lossless one not"
    );
    assert_eq!(result.report.conversions[0].node, AdfNode::Mention);
    assert_eq!(
        result.report.conversions[0].degraded_to,
        AdfTarget::PlainText
    );
    assert_eq!(result.report.conversions[1].node, AdfNode::Status);
    assert_eq!(result.report.conversions[2].node, AdfNode::Extension);
    // the macro degraded to a callout(note) carrying the marker (named, not silent)
    match &result.doc.blocks[2].block {
        Block::Callout { tone, blocks } => {
            assert_eq!(*tone, CalloutTone::Note);
            let md = blocks
                .iter()
                .map(|b| match b {
                    Block::Paragraph { inline } => myelin_content::serialize_inline(inline),
                    _ => String::new(),
                })
                .collect::<Vec<_>>()
                .join(" ");
            assert!(
                md.contains("unsupported macro: jira-macro"),
                "the marker is in-content: {md}"
            );
        }
        other => panic!("a macro degrades to a callout(note), got {other:?}"),
    }
}

#[test]
fn adf_import_conditional_node_is_lossless_when_resolved() {
    // The SAME mention, when it resolves in-tenant, survives as a structured node (no loss).
    let resolved = import_adf(
        "p",
        "t",
        &[ParsedAdfNode::resolved(AdfNode::Mention, "alice")],
    );
    assert!(
        resolved.report.is_lossless(),
        "an in-tenant mention is lossless"
    );
    match &resolved.doc.blocks[0].block {
        Block::Paragraph { inline } => {
            assert_eq!(
                inline.nodes.len(),
                1,
                "the structured mention node survives"
            );
            assert!(matches!(inline.nodes[0], InlineNode::Mention(_)));
        }
        _ => panic!("a resolved mention is a paragraph carrying the structured node"),
    }
}

#[test]
fn adf_import_degraded_content_survives_not_dropped() {
    // An unresolved mention degrades to plain text — the loss is recorded BUT the @name text is
    // never silently dropped (EI-04 §4 named, never silent).
    let r = import_adf(
        "p",
        "t",
        &[ParsedAdfNode::unresolved(AdfNode::Mention, "ext-user")],
    );
    assert_eq!(r.report.loss_count(), 1);
    match &r.doc.blocks[0].block {
        Block::Paragraph { inline } => {
            let txt = myelin_content::serialize_inline(inline);
            assert!(
                txt.contains("ext-user"),
                "the degraded text survives: {txt}"
            );
            assert!(inline.nodes.is_empty(), "no structured node (it degraded)");
        }
        _ => panic!("a degraded mention is a plain paragraph"),
    }
}

#[test]
fn adf_imported_doc_json_roundtrips() {
    // An imported document is itself a lossless export bundle (the import → export pipeline).
    let nodes = [
        ParsedAdfNode::resolved(AdfNode::Heading, "Imported title"),
        ParsedAdfNode::unresolved(AdfNode::Mention, "x"),
        ParsedAdfNode::resolved(AdfNode::Extension, "m"),
    ];
    let result = import_adf("p", "Imported", &nodes);
    assert!(
        result.doc.json_roundtrips(),
        "the imported doc round-trips losslessly as a bundle"
    );
}

// ── the format enum ─────────────────────────────────────────────────────────────────────────

#[test]
fn export_format_extensions() {
    assert_eq!(ExportFormat::Json.extension(), "json");
    assert_eq!(ExportFormat::Markdown.extension(), "md");
    assert_eq!(ExportFormat::Html.extension(), "html");
    assert_eq!(ExportFormat::Pdf.extension(), "pdf");
    assert_eq!(ExportFormat::Csv.extension(), "csv");
}
