use myelin_git::search_projection::{
    git_code_projection_spec, GIT_BLOB_ACL_OBJECT_TYPE, GIT_BLOB_TYPE, GIT_SUBSYSTEM,
};
use myelin_query::FieldType;

#[test]
fn producer_git_declares_the_frozen_6_3_shape() {
    let spec = git_code_projection_spec();
    assert_eq!(spec.subsystem, GIT_SUBSYSTEM);
    assert_eq!(spec.subsystem, "git");
    assert_eq!(spec.type_, GIT_BLOB_TYPE);
    assert_eq!(spec.type_, "blob");
    assert_eq!(spec.acl_object_type, GIT_BLOB_ACL_OBJECT_TYPE);
    assert_eq!(
        spec.acl_object_type, "repo",
        "a blob's reachability is its parent repo's"
    );
    assert!(
        !spec.semantic,
        "code is trigram/symbol full-text, not vector-embedded in v1"
    );
    assert_eq!(spec.struct_fields.get("path"), Some(&FieldType::Text));
    assert_eq!(spec.struct_fields.get("language"), Some(&FieldType::Text));
    assert_eq!(spec.struct_fields.get("blob_oid"), Some(&FieldType::Text));
    assert_eq!(
        spec.struct_fields.len(),
        3,
        "exactly the three structured code facets"
    );
}

#[test]
fn producer_spec_serializes_to_the_6_3_wire_shape() {
    let json = serde_json::to_value(git_code_projection_spec()).expect("the spec serializes");
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
    assert_eq!(obj["subsystem"], serde_json::json!("git"));
    assert_eq!(obj["type"], serde_json::json!("blob"));
    assert_eq!(obj["semantic"], serde_json::json!(false));
    assert_eq!(obj["acl_object_type"], serde_json::json!("repo"));
    assert_eq!(
        obj["struct_fields"],
        serde_json::json!({ "path": "Text", "language": "Text", "blob_oid": "Text" })
    );
}
