use super::*;
use crate::keys::{HiLoKeyAllocator, InMemoryPrefixCounter};
use myelin_content::adf::AdfNode;
use myelin_events::{
    Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Region, TenantId,
    Timestamp,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
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

fn minter() -> Arc<dyn IdMinter> {
    Arc::new(MonotonicMinter::new())
}

fn issue(source_id: &str) -> CanonicalIssue {
    CanonicalIssue {
        source_id: source_id.into(),
        project_key: "ENG".into(),
        title: format!("imported {source_id}"),
        body_md: "a body".into(),
        reporter_pseudonym: "psn-reporter@acme.noreply".into(),
        state: "open".into(),
        contains_pii: false,
    }
}

fn import_of(ids: &[&str], rels: &[(&str, &str, &str)]) -> CanonicalImport {
    let mut import = CanonicalImport::new(SourceSystem::Jira);
    import.issues = ids.iter().map(|id| issue(id)).collect();
    import.relations = rels
        .iter()
        .map(|(s, d, k)| CanonicalRelation {
            src_source_id: (*s).into(),
            dst_source_id: (*d).into(),
            kind: (*k).into(),
        })
        .collect();
    import
}

#[test]
fn rerun_skips_already_mapped_zero_duplicate_creates() {
    let alloc = HiLoKeyAllocator::new(InMemoryPrefixCounter::new());
    let id_map = InMemorySourceIdMap::new();
    let engine = ImportEngine::new(&alloc, &id_map, ImportLaneBudget::default_budget());
    let store = OutboxStore::new();
    let import = import_of(&["PROJ-1", "PROJ-2", "PROJ-3"], &[]);

    let r1 = engine
        .run(
            &tenant(),
            &import,
            false,
            &store,
            minter(),
            ctx_base(),
            None,
        )
        .unwrap();
    assert_eq!(r1.created, 3);
    assert_eq!(r1.skipped_already_mapped, 0);
    assert_eq!(id_map.count(&tenant(), SourceSystem::Jira), 3);
    let depth_after_first = store.outbox_depth();
    assert_eq!(depth_after_first, 3, "3 issue.created emitted");

    let r2 = engine
        .run(
            &tenant(),
            &import,
            false,
            &store,
            minter(),
            ctx_base(),
            None,
        )
        .unwrap();
    assert_eq!(r2.created, 0, "0 duplicate creates on a re-run");
    assert_eq!(r2.skipped_already_mapped, 3);
    assert_eq!(
        id_map.count(&tenant(), SourceSystem::Jira),
        3,
        "still 3 mappings"
    );
    assert_eq!(
        store.outbox_depth(),
        depth_after_first,
        "a re-run emits NO new issue.created (idempotent)"
    );
}

#[test]
fn crash_then_resume_creates_each_issue_exactly_once() {
    let alloc = HiLoKeyAllocator::new(InMemoryPrefixCounter::new());
    let id_map = InMemorySourceIdMap::new();
    let budget = ImportLaneBudget { max_in_flight: 2 };
    let engine = ImportEngine::new(&alloc, &id_map, budget);
    let store = OutboxStore::new();
    let m = minter();
    let import = import_of(&["P-1", "P-2", "P-3", "P-4", "P-5"], &[]);

    let crashed = engine
        .run(
            &tenant(),
            &import,
            false,
            &store,
            Arc::clone(&m),
            ctx_base(),
            Some(0),
        )
        .unwrap();
    assert_eq!(crashed.created, 2, "first batch committed before the crash");
    assert_eq!(id_map.count(&tenant(), SourceSystem::Jira), 2);
    assert_eq!(store.outbox_depth(), 2);

    let resumed = engine
        .run(
            &tenant(),
            &import,
            false,
            &store,
            Arc::clone(&m),
            ctx_base(),
            None,
        )
        .unwrap();
    assert_eq!(
        resumed.created, 3,
        "the resume creates exactly the remaining 3"
    );
    assert_eq!(
        resumed.skipped_already_mapped, 2,
        "the 2 durable ones are skipped"
    );
    assert_eq!(
        id_map.count(&tenant(), SourceSystem::Jira),
        5,
        "exactly 5 mappings - each issue created exactly once"
    );
    assert_eq!(
        store.outbox_depth(),
        5,
        "exactly 5 issue.created - 0 duplicate"
    );
}

#[test]
fn rollback_clears_the_id_map_and_a_rerun_recreates() {
    let alloc = HiLoKeyAllocator::new(InMemoryPrefixCounter::new());
    let id_map = InMemorySourceIdMap::new();
    let engine = ImportEngine::new(&alloc, &id_map, ImportLaneBudget::default_budget());
    let store = OutboxStore::new();
    let m = minter();
    let import = import_of(&["X-1", "X-2"], &[]);

    engine
        .run(
            &tenant(),
            &import,
            false,
            &store,
            Arc::clone(&m),
            ctx_base(),
            None,
        )
        .unwrap();
    assert_eq!(id_map.count(&tenant(), SourceSystem::Jira), 2);

    let removed = engine.rollback(&tenant(), &import);
    assert_eq!(removed, 2, "rollback removed both mappings");
    assert_eq!(id_map.count(&tenant(), SourceSystem::Jira), 0);

    let r = engine
        .run(
            &tenant(),
            &import,
            false,
            &store,
            Arc::clone(&m),
            ctx_base(),
            None,
        )
        .unwrap();
    assert_eq!(r.created, 2, "a re-run after rollback re-creates");
}

#[test]
fn jira_adapter_records_each_lossy_node() {
    let record = ProviderRecord {
        source_id: "PROJ-1".into(),
        project_key: "ENG".into(),
        title: "t".into(),
        body_md: String::new(),
        body_adf: vec![
            AdfBodyNode {
                kind: AdfNode::Paragraph,
                text: "lossless prose".into(),
                resolved: true,
            },
            AdfBodyNode {
                kind: AdfNode::Status,
                text: "In Review".into(),
                resolved: true,
            },
            AdfBodyNode {
                kind: AdfNode::Mention,
                text: "@external".into(),
                resolved: false,
            },
        ],
        reporter_pseudonym: "psn@acme.noreply".into(),
        state: "open".into(),
        contains_pii: false,
        relations: vec![],
    };
    let import = JiraAdapter.normalise(&[record]);
    assert_eq!(
        import.report.loss_count(),
        2,
        "exactly 2 lossy nodes recorded (status + unresolved mention); the paragraph is lossless"
    );
    assert!(!import.report.is_lossless());
    assert_eq!(import.report.conversions[0].node, AdfNode::Status);
    assert_eq!(import.report.conversions[1].node, AdfNode::Mention);
    assert!(import.issues[0].body_md.contains("In Review"));
    assert!(import.issues[0].body_md.contains("@external"));
}

#[test]
fn resolved_conditional_node_is_lossless() {
    let record = ProviderRecord {
        source_id: "PROJ-2".into(),
        project_key: "ENG".into(),
        title: "t".into(),
        body_md: String::new(),
        body_adf: vec![AdfBodyNode {
            kind: AdfNode::Mention,
            text: "@alice".into(),
            resolved: true,
        }],
        reporter_pseudonym: "psn@acme.noreply".into(),
        state: "open".into(),
        contains_pii: false,
        relations: vec![],
    };
    let import = JiraAdapter.normalise(&[record]);
    assert!(
        import.report.is_lossless(),
        "a resolved mention is lossless - no report entry"
    );
}

#[test]
fn markdown_native_adapter_is_lossless() {
    let record = ProviderRecord {
        source_id: "ABC-1".into(),
        project_key: "ENG".into(),
        title: "t".into(),
        body_md: "**bold** markdown body".into(),
        body_adf: vec![],
        reporter_pseudonym: "psn@acme.noreply".into(),
        state: "open".into(),
        contains_pii: false,
        relations: vec![],
    };
    for import in [
        LinearAdapter.normalise(std::slice::from_ref(&record)),
        GitHubAdapter.normalise(std::slice::from_ref(&record)),
        CsvAdapter.normalise(std::slice::from_ref(&record)),
    ] {
        assert!(import.report.is_lossless());
        assert_eq!(import.issues[0].body_md, "**bold** markdown body");
    }
}

#[test]
fn pass2_resolves_relation_through_the_id_map() {
    let alloc = HiLoKeyAllocator::new(InMemoryPrefixCounter::new());
    let id_map = InMemorySourceIdMap::new();
    let engine = ImportEngine::new(&alloc, &id_map, ImportLaneBudget::default_budget());
    let store = OutboxStore::new();
    let import = import_of(&["A", "B"], &[("A", "B", "blocks")]);

    let r = engine
        .run(
            &tenant(),
            &import,
            false,
            &store,
            minter(),
            ctx_base(),
            None,
        )
        .unwrap();
    assert_eq!(r.created, 2);
    assert_eq!(r.relations_created, 1, "the blocks edge resolved");
    assert!(r.unresolved.is_empty());
    assert_eq!(store.outbox_depth(), 3);
}

#[test]
fn unresolved_relation_endpoint_is_a_named_gap() {
    let alloc = HiLoKeyAllocator::new(InMemoryPrefixCounter::new());
    let id_map = InMemorySourceIdMap::new();
    let engine = ImportEngine::new(&alloc, &id_map, ImportLaneBudget::default_budget());
    let store = OutboxStore::new();
    let import = import_of(&["A"], &[("A", "Z", "blocks")]);

    let r = engine
        .run(
            &tenant(),
            &import,
            false,
            &store,
            minter(),
            ctx_base(),
            None,
        )
        .unwrap();
    assert_eq!(r.created, 1);
    assert_eq!(r.relations_created, 0);
    assert_eq!(r.unresolved.len(), 1, "the dangling edge is a NAMED gap");
    assert!(r.unresolved[0].reason.contains("destination"));
    assert_eq!(store.outbox_depth(), 1, "only the 1 issue.created");
}

#[test]
fn dry_run_predicts_without_emitting() {
    let alloc = HiLoKeyAllocator::new(InMemoryPrefixCounter::new());
    let id_map = InMemorySourceIdMap::new();
    let engine = ImportEngine::new(&alloc, &id_map, ImportLaneBudget::default_budget());
    let import = import_of(&["A", "B"], &[("A", "B", "relates"), ("A", "Z", "blocks")]);

    let dry = engine.dry_run(&tenant(), &import, true);
    assert_eq!(dry.report.created, 2);
    assert_eq!(dry.report.skipped_already_mapped, 0);
    assert_eq!(dry.report.relations_created, 1, "A->B resolves in-set");
    assert_eq!(dry.report.unresolved.len(), 1, "A->Z is an unresolved gap");
    assert_eq!(
        dry.report.legal_review.len(),
        1,
        "the permission-scheme R-9 leg is named"
    );
    assert_eq!(dry.report.legal_review[0], UNSUPPORTED_PERMISSION_SCHEME);
    assert!(!dry.report.is_clean(), "lossy/unresolved/legal - not clean");

    assert_eq!(
        id_map.count(&tenant(), SourceSystem::Jira),
        0,
        "a dry run mints nothing"
    );
}

#[test]
fn fully_clean_import_is_clean() {
    let alloc = HiLoKeyAllocator::new(InMemoryPrefixCounter::new());
    let id_map = InMemorySourceIdMap::new();
    let engine = ImportEngine::new(&alloc, &id_map, ImportLaneBudget::default_budget());
    let import = import_of(&["A", "B"], &[("A", "B", "relates")]);

    let dry = engine.dry_run(&tenant(), &import, false);
    assert!(
        dry.report.is_clean(),
        "a lossless, fully-resolved import is clean"
    );
}

#[test]
fn lane_budget_caps_the_in_flight_batch_size() {
    let budget = ImportLaneBudget { max_in_flight: 4 };
    assert_eq!(budget.batches(10), vec![4, 4, 2]);
    assert_eq!(budget.batches(0), Vec::<usize>::new());
    assert_eq!(budget.batches(4), vec![4]);
    assert_eq!(
        ImportLaneBudget { max_in_flight: 0 }.batches(3),
        vec![1, 1, 1]
    );
}

#[test]
fn capped_batches_create_every_issue_exactly_once() {
    let alloc = HiLoKeyAllocator::new(InMemoryPrefixCounter::new());
    let id_map = InMemorySourceIdMap::new();
    let engine = ImportEngine::new(&alloc, &id_map, ImportLaneBudget { max_in_flight: 3 });
    let store = OutboxStore::new();
    let ids: Vec<String> = (0..7).map(|i| format!("P-{i}")).collect();
    let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let import = import_of(&id_refs, &[]);

    let r = engine
        .run(
            &tenant(),
            &import,
            false,
            &store,
            minter(),
            ctx_base(),
            None,
        )
        .unwrap();
    assert_eq!(r.created, 7);
    assert_eq!(id_map.count(&tenant(), SourceSystem::Jira), 7);
    assert_eq!(
        store.outbox_depth(),
        7,
        "every issue committed exactly once across the cap"
    );
}

#[test]
fn imported_issue_emits_normal_issue_created() {
    let alloc = HiLoKeyAllocator::new(InMemoryPrefixCounter::new());
    let id_map = InMemorySourceIdMap::new();
    let engine = ImportEngine::new(&alloc, &id_map, ImportLaneBudget::default_budget());
    let store = OutboxStore::new();
    let import = import_of(&["PROJ-9"], &[]);

    engine
        .run(
            &tenant(),
            &import,
            false,
            &store,
            minter(),
            ctx_base(),
            None,
        )
        .unwrap();
    let rows = store.committed_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].envelope.type_.0, events::ISSUE_CREATED);
    assert_eq!(rows[0].envelope.payload["imported"], true);
    assert!(rows[0].envelope.payload["canonical_key"]
        .as_str()
        .unwrap()
        .starts_with("ENG-"));
    let payload = serde_json::to_string(&rows[0].envelope.payload).unwrap();
    assert!(
        !payload.contains("imported PROJ-9"),
        "no title body on the wire"
    );
}

#[test]
fn pii_imported_issue_flags_the_event() {
    let alloc = HiLoKeyAllocator::new(InMemoryPrefixCounter::new());
    let id_map = InMemorySourceIdMap::new();
    let engine = ImportEngine::new(&alloc, &id_map, ImportLaneBudget::default_budget());
    let store = OutboxStore::new();
    let mut import = import_of(&["PII-1"], &[]);
    import.issues[0].contains_pii = true;

    engine
        .run(
            &tenant(),
            &import,
            false,
            &store,
            minter(),
            ctx_base(),
            None,
        )
        .unwrap();
    let rows = store.committed_rows();
    assert!(
        rows[0].envelope.contains_personal_data,
        "a PII body flags the event"
    );
}
