use myelin_query::FieldType;
use myelin_search::{
    kn_db_row_index_spec, kn_index_specs, kn_page_index_spec, register_kn_index_specs,
    IncrementalIndexer, IndexSpec, MockEmbeddingAdapter, ProjectFetcher, FACET_ARTIFACT_REF,
    FACET_EMBED, FACET_MENTION,
};

struct NullFetcher;
impl ProjectFetcher for NullFetcher {
    fn project(
        &self,
        _t: &myelin_tenancy::TenantId,
        _r: &myelin_tenancy::Region,
        _a: &myelin_tenancy::ArtifactRef,
    ) -> Result<myelin_search::SearchProjection, myelin_search::ProjectFetchError> {
        Err(myelin_search::ProjectFetchError::Gone)
    }
}

#[test]
fn producer_kn_page_spec_is_the_frozen_6_3_shape() {
    let s = kn_page_index_spec();
    assert_eq!(s.subsystem, "knowledge");
    assert_eq!(s.type_, "page");
    assert_eq!(
        s.acl_object_type, "page",
        "a page's reachability is the page-tree's"
    );
    assert!(
        s.semantic,
        "a page is semantically indexed (vector-in-v1, §4.5)"
    );
    assert_eq!(
        s.struct_fields.len(),
        3,
        "the three structured inline-node reference facets"
    );
    for facet in [FACET_MENTION, FACET_ARTIFACT_REF, FACET_EMBED] {
        assert_eq!(
            s.struct_fields.get(facet),
            Some(&FieldType::Relation),
            "`{facet}` is a dependable reference facet (Relation, §3.1)"
        );
    }
}

#[test]
fn producer_kn_db_row_spec_is_the_gin_scan_facet_shape() {
    let s = kn_db_row_index_spec();
    assert_eq!(s.subsystem, "knowledge");
    assert_eq!(s.type_, "db_row");
    assert!(
        !s.semantic,
        "a db row is a structured record, not vector-embedded prose"
    );
    assert_eq!(s.struct_fields.get("priority"), Some(&FieldType::Select));
    assert_eq!(s.struct_fields.get("owner"), Some(&FieldType::Principal));
    assert_eq!(s.struct_fields.get("due"), Some(&FieldType::Date));
    assert_eq!(s.struct_fields.get("order_key"), Some(&FieldType::OrderKey));
    assert!(
        !s.struct_fields.contains_key("rollup"),
        "rollup is read-time, never a stored facet (KN-3)"
    );
    assert!(
        !s.struct_fields.contains_key("formula"),
        "formula is read-time, never a stored facet (KN-3)"
    );
}

#[test]
fn producer_kn_specs_serialize_to_the_6_3_wire_shape() {
    for s in kn_index_specs() {
        let json = serde_json::to_value(&s).expect("the spec serializes");
        let obj = json.as_object().expect("a JSON object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "acl_object_type",
                "semantic",
                "struct_fields",
                "subsystem",
                "type"
            ],
            "the frozen 6.3 wire key set"
        );
        assert_eq!(obj["subsystem"], serde_json::json!("knowledge"));
    }
    let page_json = serde_json::to_value(kn_page_index_spec()).unwrap();
    assert_eq!(
        page_json["struct_fields"],
        serde_json::json!({ "mention": "Relation", "artifact_ref": "Relation", "embed": "Relation" }),
        "the structured inline-node facets serialize to the typed columnar shape"
    );
}

#[test]
fn consumer_search_admits_the_kn_specs() {
    let _indexer = IncrementalIndexer::new(
        kn_index_specs(),
        std::sync::Arc::new(NullFetcher),
        std::sync::Arc::new(MockEmbeddingAdapter::new(16)),
    );
    let accepted: Vec<IndexSpec> = register_kn_index_specs();
    assert_eq!(
        accepted,
        kn_index_specs(),
        "Search accepts the declared KN specs verbatim"
    );
}

#[test]
fn consumer_kn_specs_coexist_with_another_producer() {
    let mut issue_fields = std::collections::BTreeMap::new();
    issue_fields.insert("status".to_string(), FieldType::Select);
    let issue = IndexSpec::new("issue", "issue", issue_fields);
    let mut specs = kn_index_specs();
    specs.push(issue);
    let _indexer = IncrementalIndexer::new(
        specs,
        std::sync::Arc::new(NullFetcher),
        std::sync::Arc::new(MockEmbeddingAdapter::new(16)),
    );
}
