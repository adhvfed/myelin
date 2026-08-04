use super::*;

#[test]
fn e2e_1_pr_pane_zero_leak_tombstone() {
    let a = run_e2e_1_pr_pane();
    assert_eq!(a.scenario, "E2E-1");
    assert_eq!(
        a.leaks, 0,
        "0 doc/count/IDF/RAG/title leak (the §4.2 pre-filter)"
    );
    assert!(a.green, "E2E-1 must be green: {}", a.evidence);
    assert!(a.is_green());
}

#[test]
fn e2e_3_spec_to_ship_reindex_byte_match() {
    let a = run_e2e_3_spec_to_ship();
    assert_eq!(a.scenario, "E2E-3");
    assert_eq!(a.leaks, 0);
    assert!(a.green, "E2E-3 must be green: {}", a.evidence);
    assert!(a.evidence.contains("byte_match=true"));
    assert!(a.is_green());
}

#[test]
fn e2e_4_dsar_fanout_zero_recoverable_including_backups() {
    let a = run_e2e_4_dsar_fanout();
    assert_eq!(a.scenario, "E2E-4");
    assert_eq!(
        a.leaks, 0,
        "0 recoverable PII incl. vectors incl. backups (GA-D1 spine)"
    );
    assert!(a.green, "E2E-4 must be green: {}", a.evidence);
    assert!(
        a.evidence.contains("is_h7=true"),
        "the receipt includes Search H7"
    );
    assert!(a.is_green());
}

#[test]
fn the_whole_search_wedge_is_green() {
    let arts = run_search_e2e_wedge();
    assert_eq!(arts.len(), 3, "E2E-1 / E2E-3 / E2E-4");
    let scenarios: Vec<&str> = arts.iter().map(|a| a.scenario).collect();
    assert_eq!(scenarios, E2E_SCENARIOS, "the three rows Search crosses");
    for a in &arts {
        assert!(a.is_green(), "{} must be green: {}", a.scenario, a.evidence);
    }
}

#[test]
fn e2e_1_confidential_is_reachable_when_acl_admits_it() {
    use crate::engine::{AclFilter, IndexBackend};
    let be = pane_corpus();
    let acl_filter = AclFilter::ids([PANE_CONFIDENTIAL]);
    let hits = be
        .search(&acl_filter, "acquisition", 50)
        .expect("ft under the granting ACL");
    assert!(
        hits.iter().any(|h| h.doc_id == PANE_CONFIDENTIAL),
        "under a granting ACL the confidential issue is reachable (the deny was the ACL, not a blanket)"
    );
}

#[test]
fn e2e_3_parity_hash_diverges_on_a_dropped_doc() {
    let tenant = e2e_tenant();
    let region = e2e_region();
    let lineage = spec_to_ship_lineage(&tenant.0);
    let scope = SnapshotScope::new("knowledge", "page:all");

    let fetcher = Arc::new(LineageFetcher::default());
    let mut owner = ReferenceReindexSource::new("knowledge", "page");
    for (agg, body) in &lineage {
        owner.upsert(agg, 1, serde_json::json!({ "kind": "page" }));
        fetcher.put(&lineage_snapshot_ref(agg), body);
    }
    let ix = Arc::new(IncrementalIndexer::new(
        vec![lineage_page_spec()],
        fetcher.clone(),
        Arc::new(MockEmbeddingAdapter::new(8)),
    ));
    let reindexer = SearchReindexer::new(ix.clone(), region.clone());
    let mut outbox = OutboxStore::new();
    let srcs: &[&dyn ReindexSource] = &[&owner];
    reindexer
        .reindex(&tenant, &scope, None, srcs, &mut outbox, e2e_ctx_base())
        .expect("full lineage");
    let full_hash = lineage_parity_hash(&ix, &tenant, &region);

    let short: Vec<(String, String)> = lineage[..lineage.len() - 1].to_vec();
    let fetcher2 = Arc::new(LineageFetcher::default());
    let mut owner2 = ReferenceReindexSource::new("knowledge", "page");
    for (agg, body) in &short {
        owner2.upsert(agg, 1, serde_json::json!({ "kind": "page" }));
        fetcher2.put(&lineage_snapshot_ref(agg), body);
    }
    let ix2 = Arc::new(IncrementalIndexer::new(
        vec![lineage_page_spec()],
        fetcher2.clone(),
        Arc::new(MockEmbeddingAdapter::new(8)),
    ));
    let reindexer2 = SearchReindexer::new(ix2.clone(), region.clone());
    let mut outbox2 = OutboxStore::new();
    let srcs2: &[&dyn ReindexSource] = &[&owner2];
    reindexer2
        .reindex(&tenant, &scope, None, srcs2, &mut outbox2, e2e_ctx_base())
        .expect("short lineage");
    let short_hash = lineage_parity_hash(&ix2, &tenant, &region);

    assert_ne!(
        full_hash, short_hash,
        "the parity hash diverges when the cold rebuild drops a doc (the byte-match is a true gate)"
    );
}

#[test]
fn e2e_4_backup_proof_is_not_vacuous() {
    let a = run_e2e_4_dsar_fanout();
    assert!(
        a.evidence.contains("recoverable 3→0"),
        "the backups held plaintext before the shred (3) and 0 after - a real shred: {}",
        a.evidence
    );
}
