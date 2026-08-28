use std::collections::BTreeMap;

use myelin_query::FieldType;
use myelin_search::IndexSpec;

pub const GIT_SUBSYSTEM: &str = "git";

pub const GIT_BLOB_TYPE: &str = "blob";

pub const GIT_BLOB_ACL_OBJECT_TYPE: &str = "repo";

pub const FACET_PATH: &str = "path";
pub const FACET_LANGUAGE: &str = "language";
pub const FACET_BLOB_OID: &str = "blob_oid";

pub fn git_code_projection_spec() -> IndexSpec {
    let mut struct_fields: BTreeMap<String, FieldType> = BTreeMap::new();
    struct_fields.insert(FACET_PATH.to_string(), FieldType::Text);
    struct_fields.insert(FACET_LANGUAGE.to_string(), FieldType::Text);
    struct_fields.insert(FACET_BLOB_OID.to_string(), FieldType::Text);

    IndexSpec::new(GIT_SUBSYSTEM, GIT_BLOB_TYPE, struct_fields)
        .with_parent_acl_object_type(GIT_BLOB_ACL_OBJECT_TYPE, GIT_BLOB_ACL_OBJECT_TYPE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_is_gits_owned_6_3_shape() {
        let s = git_code_projection_spec();
        assert_eq!(
            s.subsystem, "git",
            "git owns the `git` subsystem projection"
        );
        assert_eq!(s.type_, "blob", "the indexed artifact type is a blob");
        assert_eq!(
            s.acl_object_type, "repo",
            "a blob's reachability is decided by its parent repo (no per-blob ACL)"
        );
        assert_eq!(
            s.acl_object_type,
            crate::rebac_fragment::object_types::REPO,
            "the acl_object_type is exactly git's frozen ReBAC `repo` object type"
        );
        assert!(
            !s.semantic,
            "code is trigram/symbol full-text, not vector-embedded in v1 (GF-3)"
        );
        assert_eq!(
            s.struct_fields.len(),
            3,
            "exactly the three structured code facets"
        );
        assert_eq!(s.struct_fields.get("path"), Some(&FieldType::Text));
        assert_eq!(s.struct_fields.get("language"), Some(&FieldType::Text));
        assert_eq!(s.struct_fields.get("blob_oid"), Some(&FieldType::Text));
    }

    #[test]
    fn fulltext_body_is_not_a_struct_facet() {
        let s = git_code_projection_spec();
        for absent in ["symbols", "literals", "commit_message", "text"] {
            assert!(
                !s.struct_fields.contains_key(absent),
                "`{absent}` is full-text projection body, not a structured facet"
            );
        }
    }

    #[test]
    fn spec_serializes_to_the_6_3_wire_shape() {
        let s = git_code_projection_spec();
        let json = serde_json::to_value(&s).expect("the spec serializes");
        let obj = json.as_object().expect("the spec is a JSON object");

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
            "the 6.3 wire key set"
        );

        assert_eq!(obj["subsystem"], serde_json::json!("git"));
        assert_eq!(obj["type"], serde_json::json!("blob"));
        assert_eq!(obj["semantic"], serde_json::json!(false));
        assert_eq!(obj["acl_object_type"], serde_json::json!("repo"));
        assert_eq!(
            obj["struct_fields"],
            serde_json::json!({ "path": "Text", "language": "Text", "blob_oid": "Text" }),
            "the structured facets serialize to the typed columnar shape (13.3)"
        );
    }
}
