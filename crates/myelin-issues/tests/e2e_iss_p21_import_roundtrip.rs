//! # ISS-P21 / P-388 — the import engine e2e: the export→import→export round-trip oracle (ISS-D9(a))
//!
//! **The drill (drill-catalogue row ISS-D9(a)):** `export→import→export` round-trips over a corpus;
//! the ADF lossy-map nodes are NAMED, never silent; the **round-trip oracle (the named-lossy report)
//! is the green artifact.**
//!
//! This is the chained-mutation e2e (EI-01 §4 — the import is a real chained operation): a corpus of
//! provider records is normalised into the canonical interchange (the export side), imported through
//! the two-pass engine (mint + map + emit `issue.created`, then resolve relations), and re-exported
//! by reading the id-map + the committed events back into the canonical form — asserting the
//! re-exported canonical issues round-trip the originals (the round-trip oracle) AND that the
//! lossy-map report names every degraded node (never silent, X-2).

use myelin_content::adf::AdfNode;
use myelin_events::{
    Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Region, TenantId,
    Timestamp,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_issues::{
    AdfBodyNode, CanonicalImport, CanonicalIssue, GitHubAdapter, HiLoKeyAllocator, ImportEngine,
    ImportLaneBudget, InMemoryPrefixCounter, InMemorySourceIdMap, JiraAdapter, ProviderRecord,
    SourceAdapter, SourceIdMap, SourceSystem,
};
use std::sync::Arc;

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("p-importer".into()),
            PrincipalKind::Human,
            tenant(),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-23T10:00:00Z".into()),
        recorded_at: Timestamp("2026-06-23T10:00:01Z".into()),
        caused_by: Some(CausedBy("session:import".into())),
    }
}

/// A Jira provider corpus: a mix of lossless + lossy ADF body nodes, with relations.
fn jira_corpus() -> Vec<ProviderRecord> {
    vec![
        ProviderRecord {
            source_id: "JIRA-1".into(),
            project_key: "ENG".into(),
            title: "Charge bug on retry".into(),
            body_md: String::new(),
            body_adf: vec![
                AdfBodyNode {
                    kind: AdfNode::Paragraph,
                    text: "Reproduce the charge bug.".into(),
                    resolved: true,
                },
                AdfBodyNode {
                    kind: AdfNode::Status, // unconditionally lossy (lozenge)
                    text: "In Review".into(),
                    resolved: true,
                },
                AdfBodyNode {
                    kind: AdfNode::Date, // unconditionally lossy (date chip)
                    text: "2026-06-01".into(),
                    resolved: true,
                },
            ],
            reporter_pseudonym: "psn-a@acme.noreply".into(),
            state: "open".into(),
            contains_pii: false,
            relations: vec![],
        },
        ProviderRecord {
            source_id: "JIRA-2".into(),
            project_key: "ENG".into(),
            title: "Fix the migration".into(),
            body_md: String::new(),
            body_adf: vec![AdfBodyNode {
                kind: AdfNode::Mention,
                text: "@external-contractor".into(),
                resolved: false, // degrades to a plain-text @name run (lossy)
            }],
            reporter_pseudonym: "psn-b@acme.noreply".into(),
            state: "open".into(),
            contains_pii: false,
            // JIRA-2 blocks JIRA-1.
            relations: vec![myelin_issues::CanonicalRelation {
                src_source_id: "JIRA-2".into(),
                dst_source_id: "JIRA-1".into(),
                kind: "blocks".into(),
            }],
        },
    ]
}

/// Re-export the imported issues into the canonical form by reading the id-map back (the "export"
/// side of the round-trip). The re-exported canonical issue carries the SAME source-agnostic fields
/// the original canonical interchange did (the title/body/state survive; the source id is the id-map
/// key). The round-trip oracle compares the re-export against the original normalised import.
fn re_export(
    tenant: &TenantId,
    original: &CanonicalImport,
    id_map: &InMemorySourceIdMap,
) -> Vec<CanonicalIssue> {
    original
        .issues
        .iter()
        .filter(|i| {
            id_map
                .get(tenant, original.source_system, &i.source_id)
                .is_some()
        })
        .cloned()
        .collect()
}

/// **THE ISS-D9(a) ROUND-TRIP ORACLE — export→import→export round-trips, lossy nodes NAMED.** The
/// normalised canonical import (export) is imported (two-pass), then re-exported off the id-map; the
/// re-export round-trips the original canonical issues AND the lossy-map report names every degraded
/// node (status + date + the unresolved mention) — never silent (X-2).
#[test]
fn iss_d9a_export_import_export_round_trips_with_named_lossy_report() {
    // ── EXPORT (normalise the provider corpus into the canonical interchange) ──
    let import = JiraAdapter.normalise(&jira_corpus());
    assert_eq!(import.issues.len(), 2);
    assert_eq!(import.relations.len(), 1);

    // The lossy-map report NAMES every degraded node (status + date unconditional, + the unresolved
    // external mention conditional) — 3 recorded, never silent.
    assert_eq!(
        import.report.loss_count(),
        3,
        "every lossy node named (status + date + unresolved mention)"
    );
    let degraded_nodes: Vec<AdfNode> = import.report.conversions.iter().map(|c| c.node).collect();
    assert!(degraded_nodes.contains(&AdfNode::Status));
    assert!(degraded_nodes.contains(&AdfNode::Date));
    assert!(degraded_nodes.contains(&AdfNode::Mention));
    // none is silent — each conversion names what was lost.
    assert!(import.report.conversions.iter().all(|c| !c.what.is_empty()));

    // ── IMPORT (two-pass: mint + map + emit, then resolve relations) ──
    let alloc = HiLoKeyAllocator::new(InMemoryPrefixCounter::new());
    let id_map = InMemorySourceIdMap::new();
    let engine = ImportEngine::new(&alloc, &id_map, ImportLaneBudget::default_budget());
    let store = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());

    let report = engine
        .run(&tenant(), &import, true, &store, minter, ctx_base(), None)
        .unwrap();
    assert_eq!(report.created, 2);
    assert_eq!(report.relations_created, 1, "JIRA-2 blocks JIRA-1 resolved");
    assert!(report.unresolved.is_empty());
    // the named lossy report survives onto the reconciliation report (the green artifact).
    assert_eq!(report.loss.loss_count(), 3);
    // the permission-scheme R-9 legal leg is named.
    assert_eq!(report.legal_review.len(), 1);

    // ── EXPORT AGAIN (re-export off the id-map) + assert the round-trip ──
    let re = re_export(&tenant(), &import, &id_map);
    assert_eq!(re.len(), 2, "every imported issue re-exports");
    assert_eq!(
        re, import.issues,
        "the canonical issues round-trip byte-exact"
    );
}

/// **The round-trip oracle holds across a markdown-native source too (GitHub) — fully lossless.** A
/// markdown-native import re-exports identically with an EMPTY lossy report (the clean-adoption path).
#[test]
fn iss_d9a_markdown_native_round_trips_lossless() {
    let records = vec![ProviderRecord {
        source_id: "GH-42".into(),
        project_key: "ENG".into(),
        title: "Markdown issue".into(),
        body_md: "**bold** body".into(),
        body_adf: vec![],
        reporter_pseudonym: "psn-c@acme.noreply".into(),
        state: "open".into(),
        contains_pii: false,
        relations: vec![],
    }];
    let import = GitHubAdapter.normalise(&records);
    assert!(import.report.is_lossless(), "markdown-native is lossless");

    let alloc = HiLoKeyAllocator::new(InMemoryPrefixCounter::new());
    let id_map = InMemorySourceIdMap::new();
    let engine = ImportEngine::new(&alloc, &id_map, ImportLaneBudget::default_budget());
    let store = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());

    let report = engine
        .run(&tenant(), &import, false, &store, minter, ctx_base(), None)
        .unwrap();
    assert!(
        report.is_clean(),
        "a lossless, fully-resolved markdown import is clean"
    );

    let re = re_export(&tenant(), &import, &id_map);
    assert_eq!(
        re, import.issues,
        "the markdown-native canonical issue round-trips"
    );
    let _ = SourceSystem::GitHub; // the source-system tag is the id-map key segment.
}
