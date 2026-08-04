use std::collections::BTreeMap;

use myelin_query::{FieldType, FieldValue, OrderKey};
use myelin_search::{AclFilter, Hit, IndexBackend, IndexDocument, TantivyBackend, ORDER_KEY_FIELD};

struct QueryStandIn<'a> {
    engine: &'a dyn IndexBackend,
}

impl QueryStandIn<'_> {
    fn permission_aware_search(&self, reachable: &[&str], text: &str) -> Vec<Hit> {
        let acl_filter = AclFilter::ids(reachable.iter().copied());
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
