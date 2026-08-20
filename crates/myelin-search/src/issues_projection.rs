use std::collections::BTreeMap;

use myelin_query::{FieldType, FieldValue, OrderKey};

use crate::engine::ORDER_KEY_FIELD;
use crate::indexer::{IndexSpec, SearchProjection};

pub const ISSUE_SUBSYSTEM: &str = "issue";

pub const ISSUE_TYPE: &str = "issue";

pub const ISSUE_ACL_OBJECT_TYPE: &str = "issue";

pub const FACET_STATE_CATEGORY: &str = "state_category";
pub const FACET_PRIORITY: &str = "priority";
pub const FACET_ASSIGNEE: &str = "assignee";
pub const FACET_TYPE_RANK: &str = "type_rank";
pub const FACET_PROJECT_ID: &str = "project_id";
pub const FACET_CYCLE_ID: &str = "cycle_id";

pub const ISSUE_PRODUCER_RANK_FACET: &str = "rank";

pub fn issue_index_spec() -> IndexSpec {
    let mut struct_fields: BTreeMap<String, FieldType> = BTreeMap::new();
    struct_fields.insert(FACET_STATE_CATEGORY.to_string(), FieldType::Select);
    struct_fields.insert(FACET_PRIORITY.to_string(), FieldType::Int);
    struct_fields.insert(FACET_ASSIGNEE.to_string(), FieldType::Principal);
    struct_fields.insert(FACET_TYPE_RANK.to_string(), FieldType::Int);
    struct_fields.insert(FACET_PROJECT_ID.to_string(), FieldType::Relation);
    struct_fields.insert(FACET_CYCLE_ID.to_string(), FieldType::Relation);
    struct_fields.insert(ORDER_KEY_FIELD.to_string(), FieldType::OrderKey);

    IndexSpec::new(ISSUE_SUBSYSTEM, ISSUE_TYPE, struct_fields)
        .with_acl_object_type(ISSUE_ACL_OBJECT_TYPE)
}

pub fn issue_index_specs() -> Vec<IndexSpec> {
    vec![issue_index_spec()]
}

pub fn register_issue_index_specs() -> Vec<IndexSpec> {
    let specs = issue_index_specs();
    let _accepted = crate::indexer::IncrementalIndexer::new(
        specs.clone(),
        std::sync::Arc::new(NullProjectFetcher),
        std::sync::Arc::new(crate::indexer::MockEmbeddingAdapter::new(8)),
    );
    specs
}

struct NullProjectFetcher;

impl crate::indexer::ProjectFetcher for NullProjectFetcher {
    fn project(
        &self,
        _tenant: &myelin_tenancy::TenantId,
        _region: &myelin_tenancy::Region,
        _ref_: &myelin_tenancy::ArtifactRef,
    ) -> Result<SearchProjection, crate::indexer::ProjectFetchError> {
        Err(crate::indexer::ProjectFetchError::Gone)
    }
}

#[derive(Clone, Debug, Default)]
pub struct IssueProjectionInput {
    pub body: String,
    pub state_category: Option<String>,
    pub priority: Option<i64>,
    pub assignee: Option<String>,
    pub type_rank: Option<i64>,
    pub project_id: Option<String>,
    pub cycle_id: Option<String>,
    pub rank: Option<OrderKey>,
    pub lang: Option<String>,
}

pub fn issue_search_projection(input: &IssueProjectionInput) -> SearchProjection {
    let mut fields: BTreeMap<String, FieldValue> = BTreeMap::new();

    if let Some(sc) = &input.state_category {
        fields.insert(
            FACET_STATE_CATEGORY.to_string(),
            FieldValue::Select(sc.clone()),
        );
    }
    if let Some(p) = input.priority {
        fields.insert(FACET_PRIORITY.to_string(), FieldValue::Int(p));
    }
    if let Some(a) = &input.assignee {
        fields.insert(FACET_ASSIGNEE.to_string(), FieldValue::Principal(a.clone()));
    }
    if let Some(tr) = input.type_rank {
        fields.insert(FACET_TYPE_RANK.to_string(), FieldValue::Int(tr));
    }
    if let Some(pid) = &input.project_id {
        fields.insert(
            FACET_PROJECT_ID.to_string(),
            FieldValue::Relation(pid.clone()),
        );
    }
    if let Some(cid) = &input.cycle_id {
        fields.insert(
            FACET_CYCLE_ID.to_string(),
            FieldValue::Relation(cid.clone()),
        );
    }
    if let Some(rank) = &input.rank {
        fields.insert(
            ORDER_KEY_FIELD.to_string(),
            FieldValue::OrderKey(rank.clone()),
        );
    }

    SearchProjection {
        text: input.body.clone(),
        fields,
        lang: input.lang.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::IncrementalIndexer;

    #[test]
    fn issue_spec_is_the_consumed_6_3_shape() {
        let s = issue_index_spec();
        assert_eq!(s.subsystem, "issue");
        assert_eq!(s.type_, "issue");
        assert_eq!(
            s.acl_object_type, "issue",
            "an issue's reachability is its own ReBAC `view` (no parent ACL object)"
        );
        assert!(
            !s.semantic,
            "Issues is trigram-title + facet filter in v1, not vector-embedded"
        );
        assert_eq!(s.struct_fields.len(), 7, "exactly the seven issue facets");
        assert_eq!(
            s.struct_fields.get(FACET_STATE_CATEGORY),
            Some(&FieldType::Select)
        );
        assert_eq!(s.struct_fields.get(FACET_PRIORITY), Some(&FieldType::Int));
        assert_eq!(
            s.struct_fields.get(FACET_ASSIGNEE),
            Some(&FieldType::Principal)
        );
        assert_eq!(s.struct_fields.get(FACET_TYPE_RANK), Some(&FieldType::Int));
        assert_eq!(
            s.struct_fields.get(FACET_PROJECT_ID),
            Some(&FieldType::Relation)
        );
        assert_eq!(
            s.struct_fields.get(FACET_CYCLE_ID),
            Some(&FieldType::Relation)
        );
        assert_eq!(
            s.struct_fields.get(ORDER_KEY_FIELD),
            Some(&FieldType::OrderKey),
            "the board rank is the order_key columnar fast-field for sort (13.3)"
        );
    }

    #[test]
    fn fulltext_body_is_not_a_struct_facet() {
        let s = issue_index_spec();
        for absent in ["title", "body", "props", "comment", "description"] {
            assert!(
                !s.struct_fields.contains_key(absent),
                "`{absent}` is full-text projection body, not a structured facet"
            );
        }
    }

    #[test]
    fn issue_rank_facet_is_the_order_key_convention() {
        let s = issue_index_spec();
        assert_eq!(
            ISSUE_PRODUCER_RANK_FACET, "rank",
            "the producer's board-domain rank facet name is `rank` (myelin_issues::declares::FACET_RANK)"
        );
        assert_eq!(
            ORDER_KEY_FIELD, "order_key",
            "Search's index-doc order_key columnar-sort convention"
        );
        assert!(
            s.struct_fields.contains_key(ORDER_KEY_FIELD),
            "Search declares the rank under the order_key convention so the columnar sort serves it"
        );
        assert!(
            !s.struct_fields.contains_key(ISSUE_PRODUCER_RANK_FACET),
            "Search does NOT declare a `rank`-named facet - the engine sorts only on `order_key`"
        );
        assert_eq!(
            s.struct_fields.get(ORDER_KEY_FIELD),
            Some(&FieldType::OrderKey),
            "the order_key facet is byte-identical LexoRank (13.3), only the index-doc key is the convention"
        );
    }

    #[test]
    fn registration_is_accepted_by_search() {
        let accepted = register_issue_index_specs();
        assert_eq!(
            accepted,
            issue_index_specs(),
            "Search accepts the declared Issues spec verbatim"
        );
        let _ix = IncrementalIndexer::new(
            issue_index_specs(),
            std::sync::Arc::new(NullProjectFetcher),
            std::sync::Arc::new(crate::indexer::MockEmbeddingAdapter::new(8)),
        );
    }

    #[test]
    fn projection_builds_typed_facets_and_order_key() {
        let input = IssueProjectionInput {
            body: "scheduler deadlock at runtime".into(),
            state_category: Some("started".into()),
            priority: Some(2),
            assignee: Some("psn:alice".into()),
            type_rank: Some(1),
            project_id: Some("myelin://acme/issue/project/ENG".into()),
            cycle_id: None,
            rank: Some(OrderKey::bisect(None, None)),
            lang: Some("en".into()),
        };
        let p = issue_search_projection(&input);

        assert_eq!(p.text, "scheduler deadlock at runtime");
        assert_eq!(p.lang.as_deref(), Some("en"));

        assert_eq!(
            p.fields.get(FACET_STATE_CATEGORY),
            Some(&FieldValue::Select("started".into()))
        );
        assert_eq!(p.fields.get(FACET_PRIORITY), Some(&FieldValue::Int(2)));
        assert_eq!(
            p.fields.get(FACET_ASSIGNEE),
            Some(&FieldValue::Principal("psn:alice".into()))
        );
        assert_eq!(p.fields.get(FACET_TYPE_RANK), Some(&FieldValue::Int(1)));
        assert_eq!(
            p.fields.get(FACET_PROJECT_ID),
            Some(&FieldValue::Relation(
                "myelin://acme/issue/project/ENG".into()
            ))
        );
        assert!(
            !p.fields.contains_key(FACET_CYCLE_ID),
            "an absent nullable facet is not indexed as empty"
        );
        assert!(
            matches!(p.fields.get(ORDER_KEY_FIELD), Some(FieldValue::OrderKey(_))),
            "the rank is stamped under the order_key columnar-sort convention"
        );
        assert!(
            !p.fields.contains_key(ISSUE_PRODUCER_RANK_FACET),
            "the projection never stamps a `rank`-named facet"
        );

        let spec = issue_index_spec();
        for (name, value) in &p.fields {
            assert_eq!(
                value.field_type(),
                *spec.struct_fields.get(name).expect("facet is declared"),
                "facet `{name}` value type matches its spec declaration"
            );
        }
    }
}
