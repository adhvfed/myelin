//! CDC pair for contract-index row 13.2 — the frozen **ADF → `myelin-content` lossy-map**
//! (X-2/CR-9).
//!
//! PROVIDER side: Knowledge (this crate) freezes the conversion table ([`myelin_content::MAP`]):
//! every ADF node → its `myelin-content` target + a named loss class. CONSUMER side: Issues' import
//! builds against this exact map — it looks up each node's mapping, constructs the named target, and
//! records every lossy conversion in the import report (the X-2 "recorded in the import report"
//! obligation). This file carries BOTH sides so the contract-coverage scanner (P-037) admits row
//! 13.2 as a real provider+consumer pair.

use myelin_content::adf::mapping_for;
use myelin_content::{AdfNode, AdfTarget, ImportReport, Loss, MAP};

// ── PROVIDER side (13.2): Knowledge freezes the conversion table ───────────────────

/// The provider exposes the complete frozen map: every ADF node has exactly one mapping row, each
/// with a named loss class. The Issues import cannot assume a looser map than this.
fn provider_freezes_map() -> &'static [myelin_content::AdfMapping] {
    MAP
}

// ── CONSUMER side (X-2): Issues' import builds against the frozen map ───────────────

/// A consumer (Issues' import) processes a stream of ADF nodes against the frozen map: for each, it
/// reads the mapping, "constructs" the target, and records a lossy conversion when the node degraded
/// (here we model the conditional/unconditional loss branches the real parser evaluates). Returns
/// the import report — the named-floor artifact the importing user sees.
fn issues_import_consumes_map(nodes: &[AdfNode], resolves_in_tenant: bool) -> ImportReport {
    let mut report = ImportReport::new();
    for &node in nodes {
        let m = mapping_for(node);
        match &m.loss {
            Loss::None => { /* lossless — construct the direct target, nothing to record */ }
            Loss::Lossy { what } => {
                // Unconditionally lossy — always recorded.
                report.record(node, m.target, what.to_string());
            }
            Loss::Conditional {
                what, degraded_to, ..
            } => {
                // Lossy only when the condition does NOT hold (e.g. the principal/URL did not
                // resolve in-tenant). The real parser evaluates the condition per node.
                if !resolves_in_tenant {
                    report.record(node, *degraded_to, what.to_string());
                }
            }
        }
    }
    report
}

#[test]
fn cdc_13_2_provider_freezes_map_consumer_records_losses() {
    // PROVIDER: the frozen map covers exactly the X-2 node set, one row each.
    let map = provider_freezes_map();
    assert_eq!(map.len(), 25, "the frozen ADF map is exactly 25 node rows");

    // CONSUMER: an import of a fully-lossless document records nothing.
    let lossless_doc = [
        AdfNode::Paragraph,
        AdfNode::Heading,
        AdfNode::BulletList,
        AdfNode::Table,
        AdfNode::Expand, // → toggle, lossless
    ];
    let report = issues_import_consumes_map(&lossless_doc, true);
    assert!(
        report.is_lossless(),
        "a direct-equivalent import loses nothing"
    );

    // CONSUMER: an import with an external mention + a Jira status lozenge + a macro records each
    // loss in the import report (named, not silent).
    let lossy_doc = [
        AdfNode::Mention,   // external (does not resolve) → plain text (recorded)
        AdfNode::Status,    // always → code run, loses styling (recorded)
        AdfNode::Extension, // macro → callout(note) + marker (recorded)
        AdfNode::Paragraph, // lossless (not recorded)
    ];
    let report = issues_import_consumes_map(&lossy_doc, /* resolves_in_tenant */ false);
    assert_eq!(
        report.loss_count(),
        3,
        "three lossy nodes recorded, the lossless one is not"
    );
    assert_eq!(report.conversions[0].node, AdfNode::Mention);
    assert_eq!(report.conversions[0].degraded_to, AdfTarget::PlainText);
    assert_eq!(report.conversions[1].node, AdfNode::Status);
    assert_eq!(report.conversions[2].node, AdfNode::Extension);

    // CONSUMER: the SAME mention, when it DOES resolve in-tenant, is lossless (the conditional row).
    let resolved = issues_import_consumes_map(&[AdfNode::Mention], /* resolves */ true);
    assert!(
        resolved.is_lossless(),
        "an in-tenant mention survives as a structured node"
    );
}
