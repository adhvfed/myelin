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
                    kind: AdfNode::Status,
                    text: "In Review".into(),
                    resolved: true,
                },
                AdfBodyNode {
                    kind: AdfNode::Date,
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
                resolved: false,
            }],
            reporter_pseudonym: "psn-b@acme.noreply".into(),
            state: "open".into(),
            contains_pii: false,
            relations: vec![myelin_issues::CanonicalRelation {
                src_source_id: "JIRA-2".into(),
                dst_source_id: "JIRA-1".into(),
                kind: "blocks".into(),
            }],
        },
    ]
}

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

#[test]
fn iss_d9a_export_import_export_round_trips_with_named_lossy_report() {
    let import = JiraAdapter.normalise(&jira_corpus());
    assert_eq!(import.issues.len(), 2);
    assert_eq!(import.relations.len(), 1);

    assert_eq!(
        import.report.loss_count(),
        3,
        "every lossy node named (status + date + unresolved mention)"
    );
    let degraded_nodes: Vec<AdfNode> = import.report.conversions.iter().map(|c| c.node).collect();
    assert!(degraded_nodes.contains(&AdfNode::Status));
    assert!(degraded_nodes.contains(&AdfNode::Date));
    assert!(degraded_nodes.contains(&AdfNode::Mention));
    assert!(import.report.conversions.iter().all(|c| !c.what.is_empty()));

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
    assert_eq!(report.loss.loss_count(), 3);
    assert_eq!(report.legal_review.len(), 1);

    let re = re_export(&tenant(), &import, &id_map);
    assert_eq!(re.len(), 2, "every imported issue re-exports");
    assert_eq!(
        re, import.issues,
        "the canonical issues round-trip byte-exact"
    );
}

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
    let _ = SourceSystem::GitHub;
}
