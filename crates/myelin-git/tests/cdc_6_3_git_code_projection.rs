//! # The CDC pair for contract 6.3 — **git's owned `declare_indexable` code-projection spec** (GIT-P5 / P-231)
//!
//! **Contract-index row 6.3** (`declare_indexable(IndexSpec{ … })` — per-subsystem projection; each
//! subsystem declares its projection, Search owns the engine + the `IndexSpec` type). The Search
//! engine + admit-contract half is exercised by Search's own synthetic-producer drills
//! (`crates/myelin-search/src/indexer.rs` tests, SRCH-P06); THIS file pins the **git slice** of the
//! same row — the freeze GIT-P5 ships:
//!
//! (In contract-index terms the spec **PROVIDER**/producer is the subsystem — git here — and the
//! **CONSUMER** is Search; the two markers below carry the provider+consumer pair for the coverage
//! scanner.)
//!
//! - the **PRODUCER** (the provider side) is **git declaring its code-projection spec at build time**
//!   ([`myelin_git::search_projection::git_code_projection_spec`]) — the frozen Search-owned
//!   [`myelin_search::IndexSpec`] git registers (`git`/`blob`, the three structured facets,
//!   non-semantic, `acl_object_type = "repo"`). The producer's promise: it declares exactly the
//!   architecture `02 §9` code-projection shape and NO second indexing-contract shape (EI-01 §7).
//! - the **CONSUMER** is **Search admitting the spec** into a live
//!   [`IncrementalIndexer`](myelin_search::IncrementalIndexer)'s per-tenant facet union without a
//!   schema mismatch — the only honest definition of "accepted" (Search is the authority).
//!
//! The two sides are pinned here so a drift on either (git drops/renames a facet or the acl object
//! type; Search renames an `IndexSpec` field) fails this test in the same CI job. **The gate of
//! GIT-P5 is the build-time spec registration** — Search admits git's spec; this CDC is the
//! mechanical evidence that the frozen 6.3 shape REGISTERS (well-formed) and is ACCEPTED. The
//! code-projection EMITTER that feeds the projection at push time is the GIT-P25 / P-287 follow-on.

use myelin_git::search_projection::{
    git_code_projection_spec, register_git_code_projection_spec, GIT_BLOB_ACL_OBJECT_TYPE,
    GIT_BLOB_TYPE, GIT_SUBSYSTEM,
};
use myelin_query::FieldType;
use myelin_search::{IncrementalIndexer, IndexSpec, MockEmbeddingAdapter, ProjectFetcher};

/// A do-nothing fetcher — the SPEC registration never indexes (no emitter here; GIT-P25). It exists
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

/// **PRODUCER side — git declares the frozen 6.3 code-projection shape.** Pins every field of the
/// spec git registers (the architecture `02 §9` shape): a rename/drop on git's side fails here.
#[test]
fn producer_git_declares_the_frozen_6_3_shape() {
    let spec = git_code_projection_spec();
    assert_eq!(spec.subsystem, GIT_SUBSYSTEM);
    assert_eq!(spec.subsystem, "git");
    assert_eq!(spec.type_, GIT_BLOB_TYPE);
    assert_eq!(spec.type_, "blob");
    assert_eq!(spec.acl_object_type, GIT_BLOB_ACL_OBJECT_TYPE);
    assert_eq!(spec.acl_object_type, "repo", "a blob's reachability is its parent repo's");
    assert!(!spec.semantic, "code is trigram/symbol full-text, not vector-embedded in v1");
    assert_eq!(spec.struct_fields.get("path"), Some(&FieldType::Text));
    assert_eq!(spec.struct_fields.get("language"), Some(&FieldType::Text));
    assert_eq!(spec.struct_fields.get("blob_oid"), Some(&FieldType::Text));
    assert_eq!(spec.struct_fields.len(), 3, "exactly the three structured code facets");
}

/// **PRODUCER side — the spec serializes to the 6.3 wire shape (0 schema mismatches).** The frozen
/// key set + values. A wire rename of any `IndexSpec` key (e.g. `type` → `type_`) fails here.
#[test]
fn producer_spec_serializes_to_the_6_3_wire_shape() {
    let json = serde_json::to_value(git_code_projection_spec()).expect("the spec serializes");
    let obj = json.as_object().expect("a JSON object");
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec!["acl_object_type", "semantic", "struct_fields", "subsystem", "type"],
        "the frozen 6.3 wire key set"
    );
    assert_eq!(obj["subsystem"], serde_json::json!("git"));
    assert_eq!(obj["type"], serde_json::json!("blob"));
    assert_eq!(obj["semantic"], serde_json::json!(false));
    assert_eq!(obj["acl_object_type"], serde_json::json!("repo"));
    assert_eq!(
        obj["struct_fields"],
        serde_json::json!({ "path": "Text", "language": "Text", "blob_oid": "Text" })
    );
}

/// **CONSUMER side — Search ADMITS git's spec.** The registration is accepted: a live indexer takes
/// the spec into its per-tenant facet union without a schema mismatch (no panic at construction),
/// and the helper returns the spec byte-equal to the declared one.
#[test]
fn consumer_search_admits_the_spec() {
    let spec = git_code_projection_spec();
    // Search admits it (the build-time declare_indexable surface — the indexer constructor).
    let _indexer = IncrementalIndexer::new(
        vec![spec.clone()],
        std::sync::Arc::new(NullFetcher),
        std::sync::Arc::new(MockEmbeddingAdapter::new(8)),
    );
    // The git-side registration helper proves the same admission + returns the accepted spec.
    let accepted: IndexSpec = register_git_code_projection_spec();
    assert_eq!(accepted, spec, "Search accepts the declared spec verbatim (no mutation/rejection)");
}

/// **CONSUMER side — git's spec COEXISTS with another subsystem's spec in the same facet union.**
/// Search admits git's `blob` spec alongside a synthetic `issue` spec (distinct `(subsystem, type)`
/// keys) — proving git's registration is additive, not a clobber of the shared facet space.
#[test]
fn consumer_git_spec_coexists_with_another_producer() {
    let git = git_code_projection_spec();
    let mut issue_fields = std::collections::BTreeMap::new();
    issue_fields.insert("status".to_string(), FieldType::Select);
    let issue = IndexSpec::new("issue", "issue", issue_fields);
    // Both admitted into one indexer (the per-tenant schema is the UNION of the registered specs).
    let _indexer = IncrementalIndexer::new(
        vec![git, issue],
        std::sync::Arc::new(NullFetcher),
        std::sync::Arc::new(MockEmbeddingAdapter::new(8)),
    );
}
