//! # CDC — the vector/semantic seam (SRCH-P05 → P-168, **provider** side)
//!
//! **Architecture:** `search-and-indexing.md` §3.3 (the HNSW vector index — incremental insert,
//! soft-delete-then-compact, `model_ref` per vector) / §3.2 (all three shapes in ONE per-tenant
//! index space keyed by the same `doc_id`) / §4.5 (vector k-NN, ACL-filtered during traversal).
//!
//! - **PROVIDER** = the vector HNSW shape behind the `IndexBackend` trait
//!   ([`myelin_search::TantivyBackend::semantic`] + the co-located [`myelin_search::HnswVectorIndex`])
//!   — the seam Search exposes for the (future) semantic query path.
//! - **CONSUMER (stand-in)** = the **semantic query** that lands in **SRCH-P10/P11** (contract row
//!   6.2 `semantic(text|vec, viewer, k, filter)`). It holds the engine behind `dyn IndexBackend`,
//!   composes the ACL allow-set FIRST (the `search-requires-acl-filter` pre-filter), and reaches the
//!   vector shape only WITH the composed filter — exactly the filter-during-traversal shape SRCH-P11
//!   lowers the `list_objects` `SetExpr` into. The embedding adapter (text→vector) is the indexer's
//!   SRCH-P06 concern; this stand-in passes a vector directly.
//!
//! The dated green artifact: the consumer upserts a synthetic embedded corpus (one doc-id space —
//! the same docs the FT/structured CDC indexes), composes an allow-set ACL filter from a mock
//! `list_objects` result, runs a semantic k-NN through the engine, and observes ONLY visible docs —
//! the nearest hidden vector never enters the candidate set. If the vector-seam shape drifts, this
//! stops compiling/passing — that is the contract. (The RRF fusion + the real `list_objects` conjoin
//! is SRCH-P11; the indexer that feeds embedded upserts is SRCH-P06 — named floors.)

use std::collections::BTreeMap;

use myelin_query::{FieldType, FieldValue, OrderKey};
use myelin_search::{
    AclFilter, Embedding, IndexBackend, IndexDocument, ModelRef, TantivyBackend, VectorHit,
    ORDER_KEY_FIELD,
};

/// The CONSUMER stand-in (the SRCH-P10/P11 `semantic` shape): always composes the ACL filter from a
/// mock `list_objects` allow-set, then reaches the vector shape — never without a pre-filter.
struct SemanticStandIn<'a> {
    engine: &'a dyn IndexBackend,
}

impl SemanticStandIn<'_> {
    /// `semantic(vec, viewer, k, filter)` (contract 6.2): pre-filter FIRST (the allow-set), engine
    /// SECOND. The k visible nearest passages — the shape an agent's RAG retrieval consumes.
    fn semantic(&self, reachable: &[&str], query: &Embedding, k: usize) -> Vec<VectorHit> {
        let acl_filter = AclFilter::ids(reachable.iter().copied());
        self.engine.semantic(&acl_filter, query, k).expect("engine semantic")
    }
}

fn corpus_facets() -> BTreeMap<String, FieldType> {
    let mut m = BTreeMap::new();
    m.insert("status".to_string(), FieldType::Select);
    m.insert(ORDER_KEY_FIELD.to_string(), FieldType::OrderKey);
    m
}

fn embedded(id: &str, body: &str, v: Vec<f32>) -> IndexDocument {
    let k = OrderKey::bisect(None, None);
    IndexDocument::new(id, body)
        .with_field("status", FieldValue::Select("open".into()))
        .with_field(ORDER_KEY_FIELD, FieldValue::OrderKey(k))
        .with_embedding(Embedding::new(v), "text-embed-3@1")
}

/// **The provider/consumer CDC pair: the vector seam answers a permission-aware semantic query and
/// surfaces ONLY visible docs (filter-during-traversal).** The consumer composes the ACL filter
/// first; the nearest hidden vector never enters the candidate set (no count/rank leak — the
/// SRCH-D1 vector half property).
#[test]
fn vector_seam_answers_a_pre_filtered_semantic_query() {
    let mut engine = TantivyBackend::open(&corpus_facets()).expect("open the per-tenant index");
    // A corpus where `ENG-3` is the NEAREST to the query but is NOT in the viewer's reachable set.
    engine.upsert(&embedded("acme/issue/ENG-1", "deadlock in the scheduler", vec![0.8, 0.2, 0.0]))
        .expect("u");
    engine.upsert(&embedded("acme/issue/ENG-2", "deadlock in the indexer", vec![0.7, 0.3, 0.0]))
        .expect("u");
    engine.upsert(&embedded("acme/issue/ENG-3", "deadlock SECRET ops runbook", vec![1.0, 0.0, 0.0]))
        .expect("u");

    let consumer = SemanticStandIn { engine: &engine };
    // The viewer's reachable set EXCLUDES ENG-3 (the secret runbook) — even though it is nearest.
    let hits = consumer.semantic(
        &["acme/issue/ENG-1", "acme/issue/ENG-2"],
        &Embedding::new(vec![1.0, 0.0, 0.0]),
        2,
    );

    let ids: std::collections::BTreeSet<String> = hits.iter().map(|h| h.doc_id.clone()).collect();
    assert_eq!(
        ids,
        ["acme/issue/ENG-1", "acme/issue/ENG-2"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        "the engine surfaces only the visible vectors; the nearest secret never enters the candidate set"
    );
    // Every hit carries its model_ref (§3.3) — a fused result can be checked for model consistency.
    assert!(hits.iter().all(|h| h.model_ref == ModelRef("text-embed-3@1".into())));
}

/// **One-doc-id-space (no separate vector store): the SAME `doc_id` is reachable by FT, structured,
/// AND vector through the one engine (§3.2).** The consumer indexes a doc ONCE (all three shapes)
/// and reaches it three ways under one key.
#[test]
fn vector_shares_the_one_doc_id_space_with_ft_and_structured() {
    let mut engine = TantivyBackend::open(&corpus_facets()).expect("open");
    engine
        .upsert(&embedded("acme/page/7", "raft consensus protocol", vec![1.0, 0.0, 0.0]))
        .expect("u");
    let acl = AclFilter::ids(["acme/page/7"]);

    let ft = engine.search(&acl, "raft", 5).expect("ft");
    let st = engine
        .search_structured(&acl, "status", &FieldValue::Select("open".into()), 5)
        .expect("structured");
    let ve = engine.semantic(&acl, &Embedding::new(vec![0.99, 0.01, 0.0]), 1).expect("semantic");

    assert_eq!(ft[0].doc_id, "acme/page/7");
    assert_eq!(st[0].doc_id, "acme/page/7");
    assert_eq!(ve[0].doc_id, "acme/page/7");
    // All three shapes resolve the SAME doc_id — one index space, no separate vector store (§3.2).
    assert_eq!(ft[0].doc_id, ve[0].doc_id);
    assert_eq!(st[0].doc_id, ve[0].doc_id);
}

/// **Soft-delete-then-compact through the seam: a `delete` then `merge` leaves 0 orphan embedding
/// (the erasure-critical property the SRCH-P15 erase path rides — §3.3).**
#[test]
fn vector_seam_soft_delete_then_compact_zero_orphan() {
    let mut engine = TantivyBackend::open(&corpus_facets()).expect("open");
    engine.upsert(&embedded("keep", "alpha", vec![1.0, 0.0])).expect("u");
    engine.upsert(&embedded("erase", "beta", vec![0.0, 1.0])).expect("u");

    engine.delete("erase").expect("delete erases the doc + soft-deletes its vector");
    let acl = AclFilter::ids(["keep", "erase"]);
    let hits = engine.semantic(&acl, &Embedding::new(vec![0.0, 1.0]), 5).expect("semantic");
    assert!(!hits.iter().any(|h| h.doc_id == "erase"), "the erased vector never surfaces");

    engine.merge().expect("merge compacts");
    assert!(!engine.vectors().has_orphan_embedding(), "0 orphan embedding after compaction");
    assert_eq!(engine.vectors().live_len(), 1, "only the surviving vector remains");
}
