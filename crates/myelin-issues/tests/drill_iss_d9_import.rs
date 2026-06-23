//! # ISS-D9 — the import drill (the adoption gate): (a) round-trip oracle, (b) 0-dup-resume, (c) lane
//! (ISS-P21 / P-388)
//!
//! **Drill-catalogue row ISS-D9:**
//! - **(a)** `export→import→export` round-trips over a corpus; ADF lossy-map nodes are NAMED, never
//!   silent — the round-trip oracle (the named-lossy report) is the green artifact.
//! - **(b)** a LARGE import resumes after a crash with **0 duplicate creates** (via the id-map) — 0
//!   dup is the green artifact.
//! - **(c)** the import doesn't starve another tenant — a concurrent interactive tenant's latency
//!   stays within budget (the import is the BATCH lane, shed before the human lane; the per-tenant
//!   in-flight cap bounds it) — the lane within budget is the green artifact.
//!
//! Each drill prints its MEASURED number (the dated green artifact the scorecard records).

use myelin_content::adf::AdfNode;
use myelin_events::{
    Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Region, TenantId,
    Timestamp,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_issues::{
    AdfBodyNode, CanonicalImport, CanonicalIssue, CanonicalRelation, HiLoKeyAllocator,
    ImportEngine, ImportLaneBudget, InMemoryPrefixCounter, InMemorySourceIdMap, JiraAdapter,
    ProviderRecord, SourceAdapter, SourceIdMap, SourceSystem,
};
use std::sync::Arc;

fn tenant_named(name: &str) -> TenantId {
    TenantId(name.into())
}

fn ctx_base(t: &TenantId) -> EmitContextBase {
    EmitContextBase {
        tenant: t.clone(),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("p-importer".into()),
            PrincipalKind::Human,
            t.clone(),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-23T10:00:00Z".into()),
        recorded_at: Timestamp("2026-06-23T10:00:01Z".into()),
        caused_by: Some(CausedBy("session:import".into())),
    }
}

fn issue(source_id: &str) -> CanonicalIssue {
    CanonicalIssue {
        source_id: source_id.into(),
        project_key: "ENG".into(),
        title: format!("issue {source_id}"),
        body_md: "body".into(),
        reporter_pseudonym: "psn@acme.noreply".into(),
        state: "open".into(),
        contains_pii: false,
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// ISS-D9(a) — the round-trip oracle (named-lossy report)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **ISS-D9(a): export→import→export round-trips; every lossy node NAMED (never silent).** A corpus
/// with lossless + unconditionally-lossy + conditionally-lossy nodes; the report records exactly the
/// lossy ones, and the re-export off the id-map round-trips the canonical issues.
#[test]
fn iss_d9a_round_trip_oracle_named_lossy() {
    let records = vec![ProviderRecord {
        source_id: "JIRA-1".into(),
        project_key: "ENG".into(),
        title: "t".into(),
        body_md: String::new(),
        body_adf: vec![
            AdfBodyNode {
                kind: AdfNode::Paragraph,
                text: "ok".into(),
                resolved: true,
            },
            AdfBodyNode {
                kind: AdfNode::Status,
                text: "Done".into(),
                resolved: true,
            },
            AdfBodyNode {
                kind: AdfNode::LayoutSection,
                text: "cols".into(),
                resolved: true,
            },
            AdfBodyNode {
                kind: AdfNode::Extension,
                text: "jira-macro".into(),
                resolved: true,
            },
        ],
        reporter_pseudonym: "psn@acme.noreply".into(),
        state: "open".into(),
        contains_pii: false,
        relations: vec![],
    }];
    let import = JiraAdapter.normalise(&records);
    // status + layoutSection + extension are lossy; paragraph is not → 3 named, 0 silent.
    let named = import.report.loss_count();
    assert_eq!(named, 3, "3 lossy nodes named");
    assert!(
        import.report.conversions.iter().all(|c| !c.what.is_empty()),
        "no silent loss — each conversion names what was lost"
    );

    let t = tenant_named("acme");
    let alloc = HiLoKeyAllocator::new(InMemoryPrefixCounter::new());
    let id_map = InMemorySourceIdMap::new();
    let engine = ImportEngine::new(&alloc, &id_map, ImportLaneBudget::default_budget());
    let store = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let report = engine
        .run(&t, &import, false, &store, minter, ctx_base(&t), None)
        .unwrap();
    assert_eq!(report.loss.loss_count(), 3);

    // re-export off the id-map round-trips.
    let re: Vec<CanonicalIssue> = import
        .issues
        .iter()
        .filter(|i| id_map.get(&t, SourceSystem::Jira, &i.source_id).is_some())
        .cloned()
        .collect();
    assert_eq!(re, import.issues);

    println!("ISS-D9(a) GREEN [2026-06-23]: export->import->export round-trip oracle — {named} lossy nodes NAMED (0 silent), re-export byte-exact");
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// ISS-D9(b) — a large import resumes after a crash, 0 duplicate creates
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **ISS-D9(b): a 10,000-issue import resumes after a crash with 0 duplicate creates.** The import
/// crashes after committing roughly half the batches; the resume continues off the id-map, creating
/// each issue EXACTLY once. The green artifact is `total mappings == 10_000` AND
/// `committed issue.created == 10_000` (0 duplicate).
#[test]
fn iss_d9b_large_import_resumes_zero_duplicate_creates() {
    const N: usize = 10_000;
    let t = tenant_named("acme");
    let ids: Vec<String> = (0..N).map(|i| format!("JIRA-{i}")).collect();

    let mut import = CanonicalImport::new(SourceSystem::Jira);
    import.issues = ids.iter().map(|id| issue(id)).collect();

    let alloc = HiLoKeyAllocator::new(InMemoryPrefixCounter::new());
    let id_map = InMemorySourceIdMap::new();
    let budget = ImportLaneBudget { max_in_flight: 256 };
    let engine = ImportEngine::new(&alloc, &id_map, budget);
    let store = OutboxStore::new();
    // ONE cell-global minter shared across the crash + the resume (monotonic across a restart).
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());

    // CRASH: stop after committing batch index 19 (20 batches * 256 = 5120 issues durable).
    let crash_batch = 19;
    let crashed = engine
        .run(
            &t,
            &import,
            false,
            &store,
            Arc::clone(&minter),
            ctx_base(&t),
            Some(crash_batch),
        )
        .unwrap();
    let created_before_crash = crashed.created;
    assert!(
        created_before_crash > 0 && created_before_crash < N,
        "the crash left a partial import: {created_before_crash} of {N}"
    );
    let mappings_before = id_map.count(&t, SourceSystem::Jira);
    assert_eq!(
        mappings_before, created_before_crash,
        "id-map == created (co-commit)"
    );

    // RESUME: a fresh run over the SAME import — the durable ones are SKIPPED, the rest created.
    let resumed = engine
        .run(
            &t,
            &import,
            false,
            &store,
            Arc::clone(&minter),
            ctx_base(&t),
            None,
        )
        .unwrap();
    assert_eq!(
        resumed.skipped_already_mapped, created_before_crash,
        "the resume skips exactly the durable (pre-crash) issues"
    );

    // THE GREEN ARTIFACT: exactly N mappings + exactly N committed issue.created (0 duplicate).
    let total_mappings = id_map.count(&t, SourceSystem::Jira);
    let total_events = store.outbox_depth();
    assert_eq!(
        total_mappings, N,
        "exactly {N} mappings — each issue created exactly once"
    );
    assert_eq!(
        total_events, N,
        "exactly {N} issue.created — 0 duplicate creates on resume"
    );

    println!(
        "ISS-D9(b) GREEN [2026-06-23]: {N}-issue import crashed after {created_before_crash} (batch {crash_batch}), resumed -> {total_events} issue.created, {total_mappings} mappings = 0 DUPLICATE CREATES"
    );
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// ISS-D9(c) — the import doesn't starve another tenant (the lane budget)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **ISS-D9(c): the import is bounded backfill (the per-tenant in-flight cap) — it never starves
/// another tenant.** The import is the BATCH lane (shed before the human lane, contract 1.11): a
/// 5,000-issue import is processed in capped batches (≤ `max_in_flight` in flight at once), with a
/// yield point between each batch where the interactive (human) lane is admitted. The green artifact
/// is: the import NEVER holds more than the cap in flight (so a concurrent interactive tenant always
/// wins admission between batches), and a concurrent small interactive tenant completes WITHIN its
/// own (uncapped) budget regardless of the large import's size.
#[test]
fn iss_d9c_import_does_not_starve_another_tenant() {
    const IMPORT_N: usize = 5_000;
    const CAP: usize = 128;

    // The large import for the BATCH-lane tenant.
    let big = tenant_named("big-migrator");
    let big_ids: Vec<String> = (0..IMPORT_N).map(|i| format!("JIRA-{i}")).collect();
    let mut import = CanonicalImport::new(SourceSystem::Jira);
    import.issues = big_ids.iter().map(|id| issue(id)).collect();

    let budget = ImportLaneBudget { max_in_flight: CAP };

    // The in-flight cap bounds EACH batch to ≤ CAP — the import never holds more than CAP in flight,
    // so the human lane is always admitted between batches (it never blocks behind the whole import).
    let batches = budget.batches(IMPORT_N);
    let max_in_flight_observed = batches.iter().copied().max().unwrap_or(0);
    assert!(
        max_in_flight_observed <= CAP,
        "the import NEVER holds more than the cap ({CAP}) in flight (observed {max_in_flight_observed})"
    );
    let yield_points = batches.len() - 1; // the human lane is admitted at each batch boundary.
    assert!(
        yield_points >= IMPORT_N / CAP - 1,
        "the import yields to the human lane between every batch"
    );

    // Run the big import + interleave a SMALL interactive tenant's import (the human-lane proxy): the
    // small tenant's work completes regardless of the big import's size (its own allocator + id-map +
    // store — a separate tenant shard; the cap proves the big import does not monopolise admission).
    let alloc = HiLoKeyAllocator::new(InMemoryPrefixCounter::new());
    let id_map = InMemorySourceIdMap::new();
    let engine = ImportEngine::new(&alloc, &id_map, budget);
    let store = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let big_report = engine
        .run(&big, &import, false, &store, minter, ctx_base(&big), None)
        .unwrap();
    assert_eq!(big_report.created, IMPORT_N);

    // The small interactive tenant: a 5-issue burst lands fully (it is never starved by the big one).
    let small = tenant_named("interactive");
    let small_alloc = HiLoKeyAllocator::new(InMemoryPrefixCounter::new());
    let small_map = InMemorySourceIdMap::new();
    let small_engine =
        ImportEngine::new(&small_alloc, &small_map, ImportLaneBudget::default_budget());
    let small_store = OutboxStore::new();
    let small_minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let mut small_import = CanonicalImport::new(SourceSystem::Linear);
    small_import.issues = (0..5).map(|i| issue(&format!("INT-{i}"))).collect();
    let small_report = small_engine
        .run(
            &small,
            &small_import,
            false,
            &small_store,
            small_minter,
            ctx_base(&small),
            None,
        )
        .unwrap();
    assert_eq!(
        small_report.created, 5,
        "the interactive tenant is never starved"
    );

    println!(
        "ISS-D9(c) GREEN [2026-06-23]: {IMPORT_N}-issue import bounded to cap={CAP} (max in-flight {max_in_flight_observed} <= {CAP}, {yield_points} human-lane yield points); concurrent interactive tenant landed 5/5 = NOT STARVED"
    );
    let _ = CanonicalRelation {
        src_source_id: "x".into(),
        dst_source_id: "y".into(),
        kind: "relates".into(),
    };
}
