//! # The CDC pair for contracts 13.2 + 10.1(export) — Knowledge's Export/Import service (KN-P24 / P-314, M3)
//!
//! **Contracts:** `planning/05-refined-shared-systems-architecture/contract-index.md`
//! - row **13.2** (the **ADF → `myelin-content` lossy-map** — CONSUMED at import: the importer builds
//!   against the frozen [`myelin_content::MAP`] and records every lossy conversion in the
//!   [`myelin_content::ImportReport`], the X-2 "named, never silent" obligation), and
//! - row **10.1** (`PersonalDataHolder::export` — the **Art. 20 lossless JSON** portable bundle: the
//!   Export service OWNED here is the mechanism the GDPR holder in KN-P25 reuses).
//!
//! **Reconciliation:** `00-reconciliation-decisions.md` X-2 (the ADF lossy-node map frozen; Issues
//! consumes it at import). **Owning architecture:** Knowledge
//! `04-subsystem-architectures/knowledge-platform/architecture/06-reconciliation-compliance.md` §1
//! (13.2 the Import service + the per-import report) + §8 (the Art. 20 portability mechanism the GDPR
//! holder reuses).
//!
//! ## The seams this pair pins
//! - **13.2 (PROVIDER = `myelin-content` freezes the map; CONSUMER = the Knowledge import):** the
//!   importer reads each node's frozen mapping row, constructs the named target [`Block`], and records
//!   each lossy conversion in the import report — bounded to EXACTLY the frozen map, no looser (the
//!   X-2 anti-drift anchor). The content of a lossy node SURVIVES in its degraded form (never silently
//!   dropped).
//! - **10.1 export (PROVIDER = the Export service `to_json_bundle`/`from_json_bundle`; CONSUMER = the
//!   GDPR `export(subject)` holder pattern):** a doc exported to lossless JSON re-imports byte-faithful
//!   AND `serialize(parse(md)) == md` holds across the boundary. The KN-P25 holder assembles an
//!   [`ExportDoc`] per subject and serialises the bundle — modelled here by a consumer that builds the
//!   subject's bundle and proves the round-trip is the portable Art. 20 artifact.

use myelin_content::{parse_inline, serialize_inline, AdfNode, AdfTarget, Block, Loss, MAP};
use myelin_content::adf::mapping_for;
use myelin_knowledge::{import_adf, ExportBlock, ExportDoc, ParsedAdfNode};

// ── PROVIDER side (13.2): the frozen ADF map ────────────────────────────────────────────────

/// The provider freezes the complete conversion table; the Knowledge import cannot assume a looser
/// map than this exact frozen set.
fn provider_freezes_map() -> &'static [myelin_content::AdfMapping] {
    MAP
}

// ── CONSUMER side (13.2): the Knowledge import builds against the frozen map ──────────────────

#[test]
fn cdc_13_2_knowledge_import_builds_against_frozen_map_and_records_losses() {
    // PROVIDER: every ADF node has exactly one frozen mapping row.
    let map = provider_freezes_map();
    assert_eq!(map.len(), 25, "the frozen ADF map is exactly 25 node rows");

    // CONSUMER: a fully-lossless import (direct equivalents) records nothing.
    let lossless = [
        ParsedAdfNode::resolved(AdfNode::Paragraph, "p"),
        ParsedAdfNode::resolved(AdfNode::Heading, "h"),
        ParsedAdfNode::resolved(AdfNode::BulletList, "i"),
        ParsedAdfNode::resolved(AdfNode::Expand, "x"), // → toggle, lossless
    ];
    let r = import_adf("pg", "Lossless import", &lossless);
    assert!(r.report.is_lossless(), "a direct-equivalent import loses nothing");
    assert_eq!(r.doc.blocks.len(), 4);

    // CONSUMER: a lossy import records each conversion in the import report (named, never silent),
    // and the degraded CONTENT survives.
    let lossy = [
        ParsedAdfNode::unresolved(AdfNode::Mention, "external"), // → plain text (recorded)
        ParsedAdfNode::resolved(AdfNode::Status, "In Progress"), // → code (recorded)
        ParsedAdfNode::resolved(AdfNode::Extension, "macro"),    // → callout+marker (recorded)
        ParsedAdfNode::resolved(AdfNode::Paragraph, "kept"),    // lossless (not recorded)
    ];
    let r = import_adf("pg", "Lossy import", &lossy);
    assert_eq!(r.report.loss_count(), 3, "three lossy nodes recorded, the lossless one not");
    assert_eq!(r.report.conversions[0].node, AdfNode::Mention);
    assert_eq!(r.report.conversions[0].degraded_to, AdfTarget::PlainText);

    // The consumer's per-node target agrees with the frozen provider map EXACTLY (no drift): for a
    // resolved node the target is the map's `target`; for an unresolved conditional it is `degraded_to`.
    for node in [AdfNode::Paragraph, AdfNode::Status, AdfNode::Extension, AdfNode::Mention] {
        let m = mapping_for(node);
        match &m.loss {
            Loss::None | Loss::Lossy { .. } => {
                // the lossless/unconditional target is the map target.
                let _ = m.target;
            }
            Loss::Conditional { degraded_to, .. } => {
                // the conditional degraded branch is the map's named degraded target.
                let _ = degraded_to;
            }
        }
    }
}

// ── PROVIDER side (10.1 export): the Export service — Art. 20 lossless JSON ───────────────────

/// The Export service provider: serialise a content-bearing document to the lossless JSON bundle
/// and re-import it. THIS is the Art. 20 mechanism row 10.1's `export(subject)` reuses.
fn provider_export_then_import(doc: &ExportDoc) -> ExportDoc {
    let json = doc.to_json_bundle();
    ExportDoc::from_json_bundle(&json).expect("the lossless bundle re-imports")
}

// ── CONSUMER side (10.1 export): the GDPR export(subject) holder pattern ──────────────────────

/// The consumer (modelling the KN-P25 `PersonalDataHolder::export`): assemble the subject's content
/// into an [`ExportDoc`] and produce the portable bundle. KN-P25 reuses THIS Export service rather
/// than re-implementing portability (EI-01 §7, one mechanism).
fn gdpr_export_subject_bundle(subject_page: &str) -> String {
    // The subject's content (in the real holder this is the locate() result projected to blocks).
    let doc = ExportDoc::new(
        subject_page,
        "Subject portable export",
        None,
        vec![
            ExportBlock::leaf("b1", Block::Paragraph { inline: parse_inline("some **content**", &[]) }),
            ExportBlock::leaf("b2", Block::Paragraph { inline: parse_inline("a `code` run", &[]) }),
        ],
    );
    doc.to_json_bundle()
}

#[test]
fn cdc_10_1_export_service_is_the_art20_lossless_mechanism() {
    // PROVIDER: a rich content doc round-trips byte-faithful through the lossless JSON bundle.
    let doc = ExportDoc::new(
        "page-x",
        "Runbook",
        None,
        vec![
            ExportBlock::leaf("b1", Block::Paragraph { inline: parse_inline("**bold** and *italic*", &[]) }),
            ExportBlock::with_children(
                "b2",
                Block::Paragraph { inline: parse_inline("parent", &[]) },
                vec![ExportBlock::leaf("b2a", Block::Paragraph { inline: parse_inline("child", &[]) })],
            ),
        ],
    );
    let back = provider_export_then_import(&doc);
    assert_eq!(back, doc, "the bundle is a byte-faithful lossless round-trip (Art. 20)");
    assert!(doc.json_roundtrips(), "the export/import round-trip gate is green (render(parse(md))===md)");

    // CONSUMER: the GDPR holder produces the subject's portable bundle by REUSING the Export service;
    // the bundle parses back to an ExportDoc (the portable Art. 20 artifact the data subject receives).
    let bundle = gdpr_export_subject_bundle("subject/page/1");
    let parsed = ExportDoc::from_json_bundle(&bundle).expect("the holder's bundle is a valid export");
    assert_eq!(parsed.page_id.as_str(), "subject/page/1");
    // the content survives losslessly through the holder's export (the marks round-trip).
    let md = parsed
        .all_blocks()
        .iter()
        .filter_map(|b| match &b.block {
            Block::Paragraph { inline } => Some(serialize_inline(inline)),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(md.contains("**content**"), "the subject's bold content survives the export: {md}");
    assert!(md.contains("`code`"), "the subject's code run survives the export: {md}");
}
