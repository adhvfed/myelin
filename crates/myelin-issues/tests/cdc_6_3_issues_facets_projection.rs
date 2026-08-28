use myelin_issues::declares::{issue_facets_projection_spec, ISSUE_SUBSYSTEM, ISSUE_TYPE};
use myelin_query::FieldType;

#[test]
fn producer_issues_declares_the_frozen_6_3_shape() {
    let spec = issue_facets_projection_spec();
    assert_eq!(spec.subsystem, ISSUE_SUBSYSTEM);
    assert_eq!(spec.subsystem, "issue");
    assert_eq!(spec.type_, ISSUE_TYPE);
    assert_eq!(spec.type_, "issue");
    assert_eq!(
        spec.acl_object_type, "issue",
        "an issue's reachability is its own ReBAC `view`"
    );
    assert!(
        !spec.semantic,
        "Issues is trigram-title + facet filter in v1, not vector-embedded"
    );
    assert_eq!(
        spec.struct_fields.get("state_category"),
        Some(&FieldType::Select)
    );
    assert_eq!(spec.struct_fields.get("priority"), Some(&FieldType::Int));
    assert_eq!(
        spec.struct_fields.get("assignee"),
        Some(&FieldType::Principal)
    );
    assert_eq!(spec.struct_fields.get("type_rank"), Some(&FieldType::Int));
    assert_eq!(
        spec.struct_fields.get("project_id"),
        Some(&FieldType::Relation)
    );
    assert_eq!(
        spec.struct_fields.get("cycle_id"),
        Some(&FieldType::Relation)
    );
    assert_eq!(spec.struct_fields.get("rank"), Some(&FieldType::OrderKey));
    assert_eq!(
        spec.struct_fields.len(),
        7,
        "exactly the seven structured issue facets"
    );
}

#[test]
fn producer_spec_serializes_to_the_6_3_wire_shape() {
    let json = serde_json::to_value(issue_facets_projection_spec()).expect("the spec serializes");
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
