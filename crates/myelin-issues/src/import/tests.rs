//! Unit tests for the two-pass ID-remapped import engine (ISS-P21 / P-388) — the mandatory-core
//! resume/dedup tests (the id-map is data-loss-adjacent — the cargo-mutants mutation-score floor is
//! declared in the prompt report). They cover:
//! - the id-map idempotent re-create / resume (0 duplicate creates on a re-run) + rollback;
//! - the ADF lossy-map (each lossy node produces a report entry, never silent);
//! - the two-pass relation remap (pass 2 resolves both endpoints through the id-map; an unmapped
//!   endpoint is a named Unresolved gap);
//! - the dry run (reconciliation-report-first — no event emitted);
//! - the per-tenant in-flight cap (the import is processed in capped batches).

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

// ── 1. the id-map: idempotent re-create / resume (0 duplicate creates) ──────────────────────────

/// **A re-run over the SAME import SKIPS every already-mapped source id (0 duplicate creates).** The
/// first run creates 3; the second run (same source ids, same id-map) creates 0 and skips 3 — the
/// idempotency guarantee the id-map enforces (the resume/dedup core).
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

    // Re-run the SAME import: every source id is already mapped → 0 created, 3 skipped, NO new event.
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

/// **A crash mid-import, then a resume, creates each issue EXACTLY once (ISS-D9(b) — 0 dup).** With a
/// batch cap of 2 over 5 issues, a crash after the first batch leaves 2 created + 3 not; the resume
/// (a fresh run over the same import) creates the remaining 3 and skips the 2 — total 5, each once.
#[test]
fn crash_then_resume_creates_each_issue_exactly_once() {
    let alloc = HiLoKeyAllocator::new(InMemoryPrefixCounter::new());
    let id_map = InMemorySourceIdMap::new();
    let budget = ImportLaneBudget { max_in_flight: 2 };
    let engine = ImportEngine::new(&alloc, &id_map, budget);
    let store = OutboxStore::new();
    // ONE cell-global minter shared across the crash + the resume (monotonic across a restart —
    // the production minter is cell-global, not per-run).
    let m = minter();
    let import = import_of(&["P-1", "P-2", "P-3", "P-4", "P-5"], &[]);

    // Crash AFTER committing batch 0 (the first 2 issues are durable; 3 remain).
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

    // Resume: a fresh run over the SAME import — the 2 durable ones are SKIPPED, the 3 remaining
    // created. Total created across both runs = 5, each issue exactly once (0 dup).
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
        "exactly 5 mappings — each issue created exactly once"
    );
    assert_eq!(
        store.outbox_depth(),
        5,
        "exactly 5 issue.created — 0 duplicate"
    );
}

/// **Rollback deletes every id-map entry — a re-run then re-creates.** After a rollback the source
/// ids are no longer mapped (the id-map is the rollback ledger), so the next run creates them again.
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

    // A re-run after rollback re-creates (the ids are unmapped again). The same cell-global minter
    // continues monotonically (the rolled-back issues' canonical keys are re-minted fresh).
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

// ── 2. the ADF lossy-map: each lossy node produces a report entry, never silent ─────────────────

/// **The Jira adapter records EACH lossy ADF node in the import report (never silent, X-2).** A body
/// with a Jira status lozenge (unconditionally lossy) + an unresolved external mention (conditionally
/// lossy, degraded) + a lossless paragraph yields exactly 2 recorded lossy conversions; the paragraph
/// records none.
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
                resolved: false, // degrades to a plain-text @name run (lossy)
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
    // The degraded content SURVIVES in the body (the loss is named, not a silent drop).
    assert!(import.issues[0].body_md.contains("In Review"));
    assert!(import.issues[0].body_md.contains("@external"));
}

/// **A RESOLVED conditional node is lossless — no report entry.** A mention that resolves in-tenant
/// records no loss (the conditional row's lossless branch).
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
            resolved: true, // resolves in-tenant → lossless
        }],
        reporter_pseudonym: "psn@acme.noreply".into(),
        state: "open".into(),
        contains_pii: false,
        relations: vec![],
    };
    let import = JiraAdapter.normalise(&[record]);
    assert!(
        import.report.is_lossless(),
        "a resolved mention is lossless — no report entry"
    );
}

/// **A markdown-native adapter (Linear/GitHub/CSV) carries the body through losslessly.** No ADF, no
/// loss — the body_md is verbatim.
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

// ── 3. the two-pass relation remap ──────────────────────────────────────────────────────────────

/// **Pass 2 resolves both relation endpoints through the id-map and emits issue.relation.created.** A
/// blocks-relation between two imported issues resolves (both endpoints minted in pass 1).
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
    // 2 issue.created + 1 issue.relation.created.
    assert_eq!(store.outbox_depth(), 3);
}

/// **A relation to an endpoint OUTSIDE the import set is a NAMED Unresolved gap (never a silent
/// dangling edge).** A blocks-relation A→Z where Z is not in the import yields one unresolved gap.
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
    // no issue.relation.created emitted for an unresolved edge.
    assert_eq!(store.outbox_depth(), 1, "only the 1 issue.created");
}

// ── 4. dry-run (reconciliation-report-first — no event emitted) ──────────────────────────────────

/// **A dry run constructs the FULL reconciliation report WITHOUT emitting a single event.** It
/// predicts the created/skipped counts, the resolved relations, the unresolved gaps, the lossy-map,
/// and the permission-scheme legal leg — all before the live import runs.
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
    assert!(!dry.report.is_clean(), "lossy/unresolved/legal — not clean");

    // The dry run minted NO durable key + emitted NO event (the id-map is untouched).
    assert_eq!(
        id_map.count(&tenant(), SourceSystem::Jira),
        0,
        "a dry run mints nothing"
    );
}

/// **A fully-clean import dry-runs clean.** No lossy nodes, no unresolved relations, no permission
/// scheme → `is_clean()` is true.
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

// ── 5. the per-tenant in-flight cap (contract 1.11) ─────────────────────────────────────────────

/// **The lane budget splits a workload into capped batches (the import never exceeds the in-flight
/// cap).** 10 items at a cap of 4 → batches [4, 4, 2]; the human lane is admitted between each.
#[test]
fn lane_budget_caps_the_in_flight_batch_size() {
    let budget = ImportLaneBudget { max_in_flight: 4 };
    assert_eq!(budget.batches(10), vec![4, 4, 2]);
    assert_eq!(budget.batches(0), Vec::<usize>::new());
    assert_eq!(budget.batches(4), vec![4]);
    // a degenerate 0 cap is clamped to 1 (never an infinite loop).
    assert_eq!(
        ImportLaneBudget { max_in_flight: 0 }.batches(3),
        vec![1, 1, 1]
    );
}

/// **A large import under a small cap commits every issue exactly once across the batches.** 7 issues
/// at a cap of 3 → 3 batches, 7 created, 7 mappings, 7 events (the cap never drops or duplicates).
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

// ── 6. the import emits the NORMAL issue.created (one indexing path) ─────────────────────────────

/// **An imported issue emits the SAME issue.created token a hand-created issue emits (one indexing
/// path).** The event token + the imported marker + the canonical key are on the wire (the body is
/// NOT — references-not-payloads).
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
    // references-not-payloads: the title/body are NOT on the wire.
    let payload = serde_json::to_string(&rows[0].envelope.payload).unwrap();
    assert!(
        !payload.contains("imported PROJ-9"),
        "no title body on the wire"
    );
}

/// **A PII-bearing imported body flags the event (references-not-payloads).** `contains_pii` on the
/// canonical issue sets `contains_personal_data` on the emitted event.
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
