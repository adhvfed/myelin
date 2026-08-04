use myelin_content::adf::{mapping_for, AdfNode, Loss};
use myelin_issues::{AdfBodyNode, JiraAdapter, ProviderRecord, SourceAdapter};

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

#[test]
fn issues_import_records_nothing_for_lossless_nodes() {
    for m in myelin_content::adf::MAP {
        if matches!(m.loss, Loss::None) {
            let import = JiraAdapter.normalise(&[record_with(m.node, true)]);
            assert!(
                import.report.is_lossless(),
                "{} is a frozen-lossless node - the import records no loss",
                m.node.wire_id()
            );
        }
    }
}

#[test]
fn issues_import_records_unconditional_lossy_nodes_with_frozen_text() {
    for m in myelin_content::adf::MAP {
        if let Loss::Lossy { what } = &m.loss {
            let import = JiraAdapter.normalise(&[record_with(m.node, true)]);
            assert_eq!(
                import.report.loss_count(),
                1,
                "{} is unconditionally lossy - recorded exactly once",
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

#[test]
fn issues_import_honours_conditional_rows() {
    for m in myelin_content::adf::MAP {
        if let Loss::Conditional {
            what, degraded_to, ..
        } = &m.loss
        {
            let resolved = JiraAdapter.normalise(&[record_with(m.node, true)]);
            assert!(
                resolved.report.is_lossless(),
                "{} resolved is lossless",
                m.node.wire_id()
            );
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

#[test]
fn issues_import_is_bounded_to_the_frozen_map() {
    for m in myelin_content::adf::MAP {
        let import = JiraAdapter.normalise(&[record_with(m.node, false)]);
        assert_eq!(import.issues.len(), 1, "{} imports", m.node.wire_id());
        let _ = mapping_for(m.node);
    }
}
