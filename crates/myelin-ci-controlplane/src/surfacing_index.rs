use std::collections::BTreeMap;

use myelin_query::FieldType;
use myelin_search::{IncrementalIndexer, IndexSpec, MockEmbeddingAdapter, ProjectFetcher};

pub use myelin_ci_sandbox::replay::{CiReindexSource, CiReplayKind};
pub use myelin_ci_sandbox::notif_rules::{
    ci_summary, register_ci_summary_templates, summary_template_key, CheckVerdict, CiSummary,
    CI_SUMMARY_TEMPLATES,
};

pub const CI_SUBSYSTEM: &str = "ci";

pub const CI_RUN_TYPE: &str = "run";

pub const CI_RUN_ACL_OBJECT_TYPE: &str = "ci_run";

pub const FACET_STATE: &str = "state";
pub const FACET_TRUST_TIER: &str = "trust_tier";
pub const FACET_ENV: &str = "env";
pub const FACET_ACTOR_PSEUDONYM: &str = "actor_pseudonym";
pub const FACET_CREATED_AT: &str = "created_at";
pub const FACET_REPO_REF: &str = "repo_ref";
pub const FACET_COMMIT_OID: &str = "commit_oid";

pub fn ci_run_index_spec() -> IndexSpec {
    let mut struct_fields: BTreeMap<String, FieldType> = BTreeMap::new();
    struct_fields.insert(FACET_STATE.to_string(), FieldType::Select);
    struct_fields.insert(FACET_TRUST_TIER.to_string(), FieldType::Select);
    struct_fields.insert(FACET_ENV.to_string(), FieldType::Select);
    struct_fields.insert(FACET_ACTOR_PSEUDONYM.to_string(), FieldType::Principal);
    struct_fields.insert(FACET_CREATED_AT.to_string(), FieldType::Date);
    struct_fields.insert(FACET_REPO_REF.to_string(), FieldType::Relation);
    struct_fields.insert(FACET_COMMIT_OID.to_string(), FieldType::Text);

    IndexSpec::new(CI_SUBSYSTEM, CI_RUN_TYPE, struct_fields)
        .semantic()
        .with_acl_object_type(CI_RUN_ACL_OBJECT_TYPE)
}

pub fn register_ci_run_index_spec() -> IndexSpec {
    let spec = ci_run_index_spec();
    let _accepted = IncrementalIndexer::new(
        vec![spec.clone()],
        std::sync::Arc::new(NullProjectFetcher),
        std::sync::Arc::new(MockEmbeddingAdapter::new(8)),
    );
    spec
}

struct NullProjectFetcher;

impl ProjectFetcher for NullProjectFetcher {
    fn project(
        &self,
        _tenant: &myelin_tenancy::TenantId,
        _region: &myelin_tenancy::Region,
        _ref_: &myelin_tenancy::ArtifactRef,
    ) -> Result<myelin_search::SearchProjection, myelin_search::ProjectFetchError> {
        Err(myelin_search::ProjectFetchError::Gone)
    }
}

pub fn run_doc_is_indexable(restricted: bool, erased: bool) -> bool {
    !restricted && !erased
}

#[cfg(test)]
#[path = "surfacing_index_tests.rs"]
mod tests;
