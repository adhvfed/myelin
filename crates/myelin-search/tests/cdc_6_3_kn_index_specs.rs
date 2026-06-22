//! # The CDC pair for contract 6.3 — **Knowledge's owned `declare_indexable` IndexSpecs** (SRCH-P17 / P-260)
//!
//! **Contract-index row 6.3** (`declare_indexable(IndexSpec{ … })` — per-subsystem projection; each
//! subsystem declares its projection, Search owns the engine + the `IndexSpec` type). The git slice
//! (GIT-P5) and the Issues slice (ISS-P04) pin their producers; THIS file pins the **Knowledge
//! slice** — the KN page + db_row specs SRCH-P17 wires:
//!
//! - the **PRODUCER** (provider side) is **Knowledge declaring its index specs**
//!   ([`myelin_search::kn_page_index_spec`] / [`myelin_search::kn_db_row_index_spec`]) — the frozen
//!   Search-owned [`IndexSpec`]s KN registers (`knowledge`/`page` semantic + the three structured
//!   inline-node reference facets; `knowledge`/`db_row` non-semantic + the custom DB-field GIN-scan
//!   facets + the order_key sort). KN has NO service crate yet (unlike git/`myelin-git`), so KN's
//!   owned specs are modelled in the Search consumer crate against the frozen `myelin-content`
//!   taxonomy it consumes — the same posture git's emitter-less GIT-P5 took. The producer's promise:
//!   it declares exactly the §3.1/§4.6.1 KN projection shape and NO second indexing-contract shape.
//! - the **CONSUMER** is **Search admitting the specs** into a live
//!   [`IncrementalIndexer`](myelin_search::IncrementalIndexer)'s per-tenant facet union without a
//!   schema mismatch — the only honest definition of "accepted" (Search is the authority).
//!
//! A drift on either side (KN drops/renames a facet or the acl object type; Search renames an
//! `IndexSpec` field) fails this test in the same CI job. The page/db_row projection BUILDER
//! (`page_search_projection`) + the real-corpus indexing GATE live in
//! `tests/integration_srch_p17_kn_indexing.rs`; this CDC is the mechanical 6.3 wire-shape evidence.

use myelin_query::FieldType;
use myelin_search::{
    kn_db_row_index_spec, kn_index_specs, kn_page_index_spec, register_kn_index_specs,
    IncrementalIndexer, IndexSpec, MockEmbeddingAdapter, ProjectFetcher, FACET_ARTIFACT_REF,
    FACET_EMBED, FACET_MENTION,
};

/// A do-nothing fetcher — the SPEC registration never indexes (no emitter here; the KN service
/// crate's emitter is the M3 follow-on). It exists only so the consumer (Search) can ADMIT the spec.
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

/// **PRODUCER side — KN's page spec is the frozen 6.3 shape.** Pins every field; a rename/drop on the
/// KN side (the structured inline-node facet set, the semantic flag, the acl object type) fails here.
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

/// **PRODUCER side — KN's db_row spec is the GIN-scan facet shape (§4.6.1).** The custom DB fields +
/// the order_key sort; non-semantic. rollup/formula are NOT stored facets (KN-3).
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

/// **PRODUCER side — both KN specs serialize to the 6.3 wire shape (0 schema mismatches).** The
/// frozen key set (`subsystem`/`type`/`struct_fields`/`semantic`/`acl_object_type`). A wire rename
/// of any `IndexSpec` key (e.g. `type` → `type_`) fails here.
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
    // The page spec's structured facets serialize to the typed reference-facet shape (13.3).
    let page_json = serde_json::to_value(kn_page_index_spec()).unwrap();
    assert_eq!(
        page_json["struct_fields"],
        serde_json::json!({ "mention": "Relation", "artifact_ref": "Relation", "embed": "Relation" }),
        "the structured inline-node facets serialize to the typed columnar shape"
    );
}

/// **CONSUMER side — Search ADMITS both KN specs.** A live indexer takes them into its per-tenant
/// facet union without a schema mismatch (no panic at construction), and the registration helper
/// returns them byte-equal to the declared set.
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

/// **CONSUMER side — the KN specs COEXIST with another subsystem's spec in one facet union.** Search
/// admits KN's page+db_row alongside a synthetic `issue` spec (distinct `(subsystem, type)` keys) —
/// proving KN's registration is additive, not a clobber of the shared facet space.
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
