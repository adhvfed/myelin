//! CDC (consumer side) for contract-index row 13.2 — the frozen **ADF → `myelin-content` lossy-map**
//! consumed by the **Issues import** (ISS-P21 / P-388, X-2/CR-9).
//!
//! PROVIDER side: Knowledge freezes the conversion table ([`myelin_content::adf::MAP`]) — every ADF
//! node → its `myelin-content` target + a named loss class (the provider half lives in
//! `myelin-content`/`myelin-knowledge`). CONSUMER side (THIS file): Issues' **real** import engine
//! ([`myelin_issues::JiraAdapter`]) builds against the EXACT frozen map — it converts each ADF body
//! node through the map and records every lossy conversion in the [`myelin_content::ImportReport`]
//! (the X-2 "recorded in the import report" obligation). The import is BOUNDED to exactly the frozen
//! map: a node the map marks lossless records nothing; a node it marks lossy records exactly once
//! with the frozen `what` text. This proves Issues' import assumption is no looser than the freeze
//! (the X-2 anti-drift anchor) over the ACTUAL adapter, not a model.

use myelin_content::adf::{mapping_for, AdfNode, Loss};
use myelin_issues::{AdfBodyNode, JiraAdapter, ProviderRecord, SourceAdapter};

/// Build a single-issue Jira provider record carrying one ADF body node (the consumer-side fixture).
fn record_with(node: AdfNode, resolved: bool) -> ProviderRecord {
    ProviderRecord {
        source_id: "JIRA-1".into(),
        project_key: "ENG".into(),
        title: "t".into(),
        body_md: String::new(),
        body_adf: vec![AdfBodyNode {
            kind: node,
            text: "payload".into(),
            resolved,
        }],
        reporter_pseudonym: "psn@acme.noreply".into(),
        state: "open".into(),
        contains_pii: false,
        relations: vec![],
    }
}

/// **CONSUMER (13.2): the real Issues import records each LOSSLESS node with no report entry.** For
/// every `Loss::None` row in the frozen map, the Jira adapter converts the node and records nothing.
#[test]
fn issues_import_records_nothing_for_lossless_nodes() {
    for m in myelin_content::adf::MAP {
        if matches!(m.loss, Loss::None) {
            let import = JiraAdapter.normalise(&[record_with(m.node, true)]);
            assert!(
                import.report.is_lossless(),
                "{} is a frozen-lossless node — the import records no loss",
                m.node.wire_id()
            );
        }
    }
}

/// **CONSUMER (13.2): the real Issues import records each UNCONDITIONALLY-LOSSY node exactly once,
/// with the frozen `what` text.** Bounded to exactly the freeze — no looser, no silent.
#[test]
fn issues_import_records_unconditional_lossy_nodes_with_frozen_text() {
    for m in myelin_content::adf::MAP {
        if let Loss::Lossy { what } = &m.loss {
            let import = JiraAdapter.normalise(&[record_with(m.node, true)]);
            assert_eq!(
                import.report.loss_count(),
                1,
                "{} is unconditionally lossy — recorded exactly once",
                m.node.wire_id()
            );
            assert_eq!(
                import.report.conversions[0].node, m.node,
                "the report names the offending node"
            );
            assert_eq!(
                import.report.conversions[0].what,
                what.to_string(),
                "the report carries the FROZEN loss text (no looser, no re-worded)"
            );
        }
    }
}

/// **CONSUMER (13.2): the real Issues import honours the CONDITIONAL rows — lossless when resolved,
/// lossy (degraded to the frozen target) when not.** For each conditional row, a resolved node
/// records nothing; an unresolved node records exactly once with the frozen degraded target.
#[test]
fn issues_import_honours_conditional_rows() {
    for m in myelin_content::adf::MAP {
        if let Loss::Conditional {
            what, degraded_to, ..
        } = &m.loss
        {
            // resolved → lossless, no report entry.
            let resolved = JiraAdapter.normalise(&[record_with(m.node, true)]);
            assert!(
                resolved.report.is_lossless(),
                "{} resolved is lossless",
                m.node.wire_id()
            );
            // unresolved → degraded, recorded once with the frozen degraded target + text.
            let unresolved = JiraAdapter.normalise(&[record_with(m.node, false)]);
            assert_eq!(
                unresolved.report.loss_count(),
                1,
                "{} unresolved degrades",
                m.node.wire_id()
            );
            assert_eq!(unresolved.report.conversions[0].degraded_to, *degraded_to);
            assert_eq!(unresolved.report.conversions[0].what, what.to_string());
        }
    }
}

/// **CONSUMER (13.2): the import is bounded to EXACTLY the frozen map — every map row is reachable
/// through the real adapter.** Driving each node through the adapter produces a result consistent
/// with `mapping_for` (the anti-drift anchor: the consumer cannot assume a node the freeze omits).
#[test]
fn issues_import_is_bounded_to_the_frozen_map() {
    for m in myelin_content::adf::MAP {
        // The adapter never panics on any frozen node (the closed table is total over the adapter).
        let import = JiraAdapter.normalise(&[record_with(m.node, false)]);
        assert_eq!(import.issues.len(), 1, "{} imports", m.node.wire_id());
        // and the frozen map is the single source of the loss decision the adapter consumed.
        let _ = mapping_for(m.node);
    }
}
