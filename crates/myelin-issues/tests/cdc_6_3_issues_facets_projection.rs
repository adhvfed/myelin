//! # The CDC pair for contract 6.3 — **Issues' owned `declare_indexable` facets-projection spec** (ISS-P04 / P-243)
//!
//! **Contract-index row 6.3** (`declare_indexable(IndexSpec{ … })` — per-subsystem projection; each
//! subsystem declares its projection, Search owns the engine + the `IndexSpec` type). The Search
//! engine + admit-contract half is exercised by Search's own synthetic-producer drills (SRCH-P06)
//! and the git slice by `crates/myelin-git/tests/cdc_6_3_git_code_projection.rs` (GIT-P5); THIS file
//! pins the **Issues slice** — the freeze ISS-P04 ships.
//!
//! (In contract-index terms the spec **PROVIDER**/producer is the subsystem — Issues here — and the
//! **CONSUMER** is Search; the two markers below carry the provider+consumer pair for the coverage
//! scanner.)
//!
//! - the **PRODUCER** (the provider side) is **Issues declaring its facets-projection spec at build
//!   time** ([`myelin_issues::declares::issue_facets_projection_spec`]) — the frozen Search-owned
//!   [`myelin_search::IndexSpec`] Issues registers (`issue`/`issue`, the seven structured board/list/
//!   search facets, non-semantic, `acl_object_type = "issue"`). The producer's promise: it declares
//!   exactly the architecture `01 §6.1` facets shape and NO second indexing-contract shape (EI-01 §7).
//! - the **CONSUMER** is **Search admitting the spec** into a live
//!   [`IncrementalIndexer`](myelin_search::IncrementalIndexer)'s per-tenant facet union without a
//!   schema mismatch — the only honest definition of "accepted" (Search is the authority).
//!
//! The two sides are pinned here so a drift on either (Issues drops/renames a facet or the acl
//! object type; Search renames an `IndexSpec` field) fails this test in the same CI job. **The gate
//! of ISS-P04 is the build-time spec registration** — Search admits Issues' spec; this CDC is the
//! mechanical evidence that the frozen 6.3 shape REGISTERS (well-formed) and is ACCEPTED. The
//! `issue.*` projection EMITTER that feeds the rows at write time is the ISS-P17 follow-on.

use myelin_issues::declares::{
    issue_facets_projection_spec, register_issue_facets_projection_spec, ISSUE_SUBSYSTEM, ISSUE_TYPE,
};
use myelin_query::FieldType;
use myelin_search::{IncrementalIndexer, IndexSpec, MockEmbeddingAdapter, ProjectFetcher};

/// A do-nothing fetcher — the SPEC registration never indexes (no emitter here; ISS-P17). It exists
/// only so the consumer (Search) can ADMIT the spec into a live indexer for the acceptance check.
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

/// **PRODUCER side — Issues declares the frozen 6.3 facets-projection shape.** Pins every field of
/// the spec Issues registers (the architecture `01 §6.1` shape): a rename/drop on Issues' side fails here.
#[test]
fn producer_issues_declares_the_frozen_6_3_shape() {
    let spec = issue_facets_projection_spec();
    assert_eq!(spec.subsystem, ISSUE_SUBSYSTEM);
    assert_eq!(spec.subsystem, "issue");
    assert_eq!(spec.type_, ISSUE_TYPE);
    assert_eq!(spec.type_, "issue");
    assert_eq!(spec.acl_object_type, "issue", "an issue's reachability is its own ReBAC `view`");
    assert!(!spec.semantic, "Issues is trigram-title + facet filter in v1, not vector-embedded");
    assert_eq!(spec.struct_fields.get("state_category"), Some(&FieldType::Select));
    assert_eq!(spec.struct_fields.get("priority"), Some(&FieldType::Int));
    assert_eq!(spec.struct_fields.get("assignee"), Some(&FieldType::Principal));
    assert_eq!(spec.struct_fields.get("type_rank"), Some(&FieldType::Int));
    assert_eq!(spec.struct_fields.get("project_id"), Some(&FieldType::Relation));
    assert_eq!(spec.struct_fields.get("cycle_id"), Some(&FieldType::Relation));
    assert_eq!(spec.struct_fields.get("rank"), Some(&FieldType::OrderKey));
    assert_eq!(spec.struct_fields.len(), 7, "exactly the seven structured issue facets");
}

/// **PRODUCER side — the spec serializes to the 6.3 wire shape (0 schema mismatches).** The frozen
/// key set + values. A wire rename of any `IndexSpec` key (e.g. `type` → `type_`) fails here.
#[test]
fn producer_spec_serializes_to_the_6_3_wire_shape() {
    let json = serde_json::to_value(issue_facets_projection_spec()).expect("the spec serializes");
    let obj = json.as_object().expect("a JSON object");
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec!["acl_object_type", "semantic", "struct_fields", "subsystem", "type"],
        "the frozen 6.3 wire key set"
    );
    assert_eq!(obj["subsystem"], serde_json::json!("issue"));
    assert_eq!(obj["type"], serde_json::json!("issue"));
    assert_eq!(obj["semantic"], serde_json::json!(false));
    assert_eq!(obj["acl_object_type"], serde_json::json!("issue"));
    assert_eq!(
        obj["struct_fields"],
        serde_json::json!({
            "state_category": "Select",
            "priority": "Int",
            "assignee": "Principal",
            "type_rank": "Int",
            "project_id": "Relation",
            "cycle_id": "Relation",
            "rank": "OrderKey",
        })
    );
}

/// **CONSUMER side — Search ADMITS Issues' spec.** The registration is accepted: a live indexer
/// takes the spec into its per-tenant facet union without a schema mismatch (no panic at
/// construction), and the helper returns the spec byte-equal to the declared one.
#[test]
fn consumer_search_admits_the_spec() {
    let spec = issue_facets_projection_spec();
    // Search admits it (the build-time declare_indexable surface — the indexer constructor).
    let _indexer = IncrementalIndexer::new(
        vec![spec.clone()],
        std::sync::Arc::new(NullFetcher),
        std::sync::Arc::new(MockEmbeddingAdapter::new(8)),
    );
    // The Issues-side registration helper proves the same admission + returns the accepted spec.
    let accepted: IndexSpec = register_issue_facets_projection_spec();
    assert_eq!(accepted, spec, "Search accepts the declared spec verbatim (no mutation/rejection)");
}

/// **CONSUMER side — Issues' spec COEXISTS with another subsystem's spec in the same facet union.**
/// Search admits Issues' `issue` spec alongside a synthetic `git`/`blob` spec (distinct
/// `(subsystem, type)` keys) — proving Issues' registration is additive, not a clobber of the
/// shared facet space.
#[test]
fn consumer_issue_spec_coexists_with_another_producer() {
    let issue = issue_facets_projection_spec();
    let mut blob_fields = std::collections::BTreeMap::new();
    blob_fields.insert("path".to_string(), FieldType::Text);
    let blob = IndexSpec::new("git", "blob", blob_fields).with_acl_object_type("repo");
    // Both admitted into one indexer (the per-tenant schema is the UNION of the registered specs).
    let _indexer = IncrementalIndexer::new(
        vec![issue, blob],
        std::sync::Arc::new(NullFetcher),
        std::sync::Arc::new(MockEmbeddingAdapter::new(8)),
    );
}
