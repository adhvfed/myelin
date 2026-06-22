//! # CDC — the `IndexBackend` engine seam (SRCH-P04 → P-167, provider side)
//!
//! **Architecture:** `search-and-indexing.md` §2.1 (Tantivy behind the `IndexBackend` trait
//! `open/upsert/delete/search/merge/snapshot`; the ACL pre-filter compiles to a posting-list-level
//! conjunctive clause) / §2.2 (the FT + structured shapes) / §3.1–§3.2 (one per-tenant index space
//! keyed by the same `doc_id`).
//!
//! - **PROVIDER** = the `IndexBackend` trait + its Tantivy v1 reference implementation
//!   ([`myelin_search::TantivyBackend`]) — the engine seam Search opens a per-tenant index behind.
//!   OpenSearch is the reserved per-cell upgrade behind the SAME trait (M5/§6.2); this CDC pins the
//!   trait shape so that swap is a config/impl change, not a contract change.
//! - **CONSUMER (stand-in)** = a minimal permission-aware query stand-in that holds the engine
//!   behind `dyn IndexBackend`, **composes the ACL filter FIRST** (the `search-requires-acl-filter`
//!   pre-filter, §4.2), and only then reaches the engine. This is the exact shape the real
//!   permission-aware query path (SRCH-P08) takes — it can never reach the engine without a composed
//!   filter (the filter is a mandatory parameter).
//!
//! The dated green artifact: the consumer upserts a synthetic corpus, composes an allow-set ACL
//! filter from a (mock) `list_objects` result, runs an FT search + a structured search through the
//! engine, and observes ONLY visible docs — no hidden doc enters the candidate set. If the
//! `IndexBackend` shape drifts, this stops compiling/passing — that is the contract. (The full
//! `SetExpr` lowering + the real `list_objects` conjoin is the SRCH-P08 follow-on; the vector branch
//! is SRCH-P05; the indexer that feeds upserts is SRCH-P06 — named floors.)

use std::collections::BTreeMap;

use myelin_query::{FieldType, FieldValue, OrderKey};
use myelin_search::{AclFilter, Hit, IndexBackend, IndexDocument, TantivyBackend, ORDER_KEY_FIELD};

/// The CONSUMER stand-in: the permission-aware query path takes the engine behind `dyn
/// IndexBackend` and ALWAYS composes the ACL filter (here from a mock `list_objects` allow-set)
/// before reaching it — the engine is never called without a pre-filter.
struct QueryStandIn<'a> {
    engine: &'a dyn IndexBackend,
}

impl QueryStandIn<'_> {
    /// Compose the ACL filter from the viewer's reachable set (the mock `list_objects` result), then
    /// run the FT search. This is the SRCH-P08 shape: pre-filter FIRST, engine SECOND.
    fn permission_aware_search(&self, reachable: &[&str], text: &str) -> Vec<Hit> {
        // Step 1 (mocked here): list_objects(viewer, read, T) → the allow-set.
        let acl_filter = AclFilter::ids(reachable.iter().copied());
        // Step 4: engine.search WITH the composed filter (pre-filter, never post-filter).
        self.engine
            .search(&acl_filter, text, 50)
            .expect("engine search")
    }
}

fn corpus_facets() -> BTreeMap<String, FieldType> {
    let mut m = BTreeMap::new();
    m.insert("status".to_string(), FieldType::Select);
    m.insert(ORDER_KEY_FIELD.to_string(), FieldType::OrderKey);
    m
}

/// **The provider/consumer CDC pair: the engine seam answers a permission-aware query and surfaces
/// ONLY visible docs.** The consumer composes the ACL filter first; a doc outside the allow-set
/// never enters the candidate set (no count/rank leak — the pre-filter crux, §4.2.1).
#[test]
fn indexbackend_seam_answers_a_pre_filtered_query() {
    let mut engine = TantivyBackend::open(&corpus_facets()).expect("open the per-tenant index");
    let k = OrderKey::bisect(None, None);
    for (id, body) in [
        ("acme/issue/ENG-1", "deadlock in the scheduler"),
        ("acme/issue/ENG-2", "deadlock in the indexer"),
        ("acme/issue/ENG-3", "deadlock SECRET ops runbook"),
    ] {
        let doc = IndexDocument::new(id, body)
            .with_field("status", FieldValue::Select("open".into()))
            .with_field(ORDER_KEY_FIELD, FieldValue::OrderKey(k.clone()));
        engine.upsert(&doc).expect("upsert");
    }

    // The viewer's reachable set EXCLUDES ENG-3 (the secret runbook).
    let consumer = QueryStandIn { engine: &engine };
    let hits =
        consumer.permission_aware_search(&["acme/issue/ENG-1", "acme/issue/ENG-2"], "deadlock");

    let ids: std::collections::BTreeSet<String> = hits.into_iter().map(|h| h.doc_id).collect();
    assert_eq!(
        ids,
        ["acme/issue/ENG-1", "acme/issue/ENG-2"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        "the engine surfaces only the visible docs; the secret runbook never enters the candidate set"
    );
}

/// **The structured-shape seam round-trips through `dyn IndexBackend` (the typed facet branch).**
/// The consumer composes the ACL filter, runs a typed-facet equality search, and gets the visible
/// matches — the structured/columnar shape behind the same trait.
#[test]
fn indexbackend_structured_seam_round_trips() {
    let mut engine = TantivyBackend::open(&corpus_facets()).expect("open");
    let k = OrderKey::bisect(None, None);
    for (id, status) in [("d1", "open"), ("d2", "closed"), ("d3", "open")] {
        let doc = IndexDocument::new(id, "body")
            .with_field("status", FieldValue::Select(status.into()))
            .with_field(ORDER_KEY_FIELD, FieldValue::OrderKey(k.clone()));
        engine.upsert(&doc).expect("upsert");
    }

    // The pre-filter is composed first; the engine answers the structured query behind the trait.
    let acl_filter = AclFilter::ids(["d1", "d2", "d3"]);
    let engine_ref: &dyn IndexBackend = &engine;
    let open = engine_ref
        .search_structured(
            &acl_filter,
            "status",
            &FieldValue::Select("open".into()),
            50,
        )
        .expect("structured search");
    let ids: std::collections::BTreeSet<String> = open.into_iter().map(|h| h.doc_id).collect();
    assert_eq!(ids, ["d1", "d3"].iter().map(|s| s.to_string()).collect());
}

/// **The merge/snapshot ops are part of the seam (the reindex/erase-compaction substrate).** A
/// consumer that deletes a doc and snapshots through `dyn IndexBackend` sees the live count drop —
/// the shape the erase (SRCH-P15) + reindex (SRCH-P16) paths call.
#[test]
fn indexbackend_merge_snapshot_seam() {
    let mut engine = TantivyBackend::open(&corpus_facets()).expect("open");
    let k = OrderKey::bisect(None, None);
    for i in 0..4 {
        let doc = IndexDocument::new(format!("d{i}"), "body")
            .with_field("status", FieldValue::Select("open".into()))
            .with_field(ORDER_KEY_FIELD, FieldValue::OrderKey(k.clone()));
        engine.upsert(&doc).expect("upsert");
    }
    assert_eq!(engine.snapshot().expect("snapshot"), 4);
    engine.delete("d1").expect("delete");
    engine.merge().expect("merge");
    assert_eq!(
        engine.snapshot().expect("snapshot after merge"),
        3,
        "the deleted doc is gone"
    );
}
