use std::collections::BTreeMap;

use myelin_query::{FieldType, FieldValue, OrderKey};
use myelin_search::{
    AclFilter, Embedding, IndexBackend, IndexDocument, ModelRef, TantivyBackend, VectorHit,
    ORDER_KEY_FIELD,
};

struct SemanticStandIn<'a> {
    engine: &'a dyn IndexBackend,
}

impl SemanticStandIn<'_> {
    fn semantic(&self, reachable: &[&str], query: &Embedding, k: usize) -> Vec<VectorHit> {
        let acl_filter = AclFilter::ids(reachable.iter().copied());
        self.engine
            .semantic(&acl_filter, query, k)
            .expect("engine semantic")
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

#[test]
fn vector_seam_answers_a_pre_filtered_semantic_query() {
    let mut engine = TantivyBackend::open(&corpus_facets()).expect("open the per-tenant index");
    engine
        .upsert(&embedded(
            "acme/issue/ENG-1",
            "deadlock in the scheduler",
            vec![0.8, 0.2, 0.0],
        ))
        .expect("u");
    engine
        .upsert(&embedded(
            "acme/issue/ENG-2",
            "deadlock in the indexer",
            vec![0.7, 0.3, 0.0],
        ))
        .expect("u");
    engine
        .upsert(&embedded(
            "acme/issue/ENG-3",
            "deadlock SECRET ops runbook",
            vec![1.0, 0.0, 0.0],
        ))
        .expect("u");

    let consumer = SemanticStandIn { engine: &engine };
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
    assert!(hits
        .iter()
        .all(|h| h.model_ref == ModelRef("text-embed-3@1".into())));
}

#[test]
fn vector_shares_the_one_doc_id_space_with_ft_and_structured() {
    let mut engine = TantivyBackend::open(&corpus_facets()).expect("open");
    engine
        .upsert(&embedded(
            "acme/page/7",
            "raft consensus protocol",
            vec![1.0, 0.0, 0.0],
        ))
        .expect("u");
    let acl = AclFilter::ids(["acme/page/7"]);

    let ft = engine.search(&acl, "raft", 5).expect("ft");
    let st = engine
        .search_structured(&acl, "status", &FieldValue::Select("open".into()), 5)
        .expect("structured");
    let ve = engine
        .semantic(&acl, &Embedding::new(vec![0.99, 0.01, 0.0]), 1)
        .expect("semantic");

    assert_eq!(ft[0].doc_id, "acme/page/7");
    assert_eq!(st[0].doc_id, "acme/page/7");
    assert_eq!(ve[0].doc_id, "acme/page/7");
    assert_eq!(ft[0].doc_id, ve[0].doc_id);
    assert_eq!(st[0].doc_id, ve[0].doc_id);
}

#[test]
fn vector_seam_soft_delete_then_compact_zero_orphan() {
    let mut engine = TantivyBackend::open(&corpus_facets()).expect("open");
    engine
        .upsert(&embedded("keep", "alpha", vec![1.0, 0.0]))
        .expect("u");
    engine
        .upsert(&embedded("erase", "beta", vec![0.0, 1.0]))
        .expect("u");

    engine
        .delete("erase")
        .expect("delete erases the doc + soft-deletes its vector");
    let acl = AclFilter::ids(["keep", "erase"]);
    let hits = engine
        .semantic(&acl, &Embedding::new(vec![0.0, 1.0]), 5)
        .expect("semantic");
    assert!(
        !hits.iter().any(|h| h.doc_id == "erase"),
        "the erased vector never surfaces"
    );

    engine.merge().expect("merge compacts");
    assert!(
        !engine.vectors().has_orphan_embedding(),
        "0 orphan embedding after compaction"
    );
    assert_eq!(
        engine.vectors().live_len(),
        1,
        "only the surviving vector remains"
    );
}
