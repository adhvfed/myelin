use std::collections::BTreeMap;

use myelin_query::{FieldType, FieldValue};

use crate::analysis::{Analyzer, Language};
use crate::indexer::{IndexSpec, SearchProjection};

pub const GIT_SUBSYSTEM: &str = "git";

pub const GIT_BLOB_TYPE: &str = "blob";

pub const GIT_BLOB_ACL_OBJECT_TYPE: &str = "repo";

pub const FACET_PATH: &str = "path";
pub const FACET_LANGUAGE: &str = "language";
pub const FACET_BLOB_OID: &str = "blob_oid";

pub const TRIGRAM_N: usize = 3;

pub fn git_code_projection_spec() -> IndexSpec {
    let mut struct_fields: BTreeMap<String, FieldType> = BTreeMap::new();
    struct_fields.insert(FACET_PATH.to_string(), FieldType::Text);
    struct_fields.insert(FACET_LANGUAGE.to_string(), FieldType::Text);
    struct_fields.insert(FACET_BLOB_OID.to_string(), FieldType::Text);
    IndexSpec::new(GIT_SUBSYSTEM, GIT_BLOB_TYPE, struct_fields)
        .with_parent_acl_object_type(GIT_BLOB_ACL_OBJECT_TYPE, GIT_BLOB_ACL_OBJECT_TYPE)
}

pub fn git_index_specs() -> Vec<IndexSpec> {
    vec![git_code_projection_spec()]
}

pub fn register_git_index_specs() -> Vec<IndexSpec> {
    let specs = git_index_specs();
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitBlobProjectionInput {
    pub path: String,
    pub language: String,
    pub text: String,
    pub literals: Vec<String>,
    pub commit_message: String,
    pub blob_oid: String,
}

pub fn git_blob_search_projection(input: &GitBlobProjectionInput) -> SearchProjection {
    let code = Analyzer::for_language(Language::Code);

    let mut terms: Vec<String> = Vec::new();

    terms.extend(code.analyze(&input.text));

    for segment in input.path.split(['/', '\\', '.']) {
        if !segment.is_empty() {
            terms.extend(code.analyze(segment));
        }
    }

    for literal in &input.literals {
        terms.extend(code.analyze(literal));
    }

    terms.extend(code.analyze(&input.commit_message));

    for tg in trigrams(&input.text) {
        terms.push(trigram_token(&tg));
    }

    let text = terms.join(" ");

    let mut fields: BTreeMap<String, FieldValue> = BTreeMap::new();
    if !input.path.is_empty() {
        fields.insert(FACET_PATH.to_string(), FieldValue::Text(input.path.clone()));
    }
    if !input.language.is_empty() {
        fields.insert(
            FACET_LANGUAGE.to_string(),
            FieldValue::Text(input.language.clone()),
        );
    }
    if !input.blob_oid.is_empty() {
        fields.insert(
            FACET_BLOB_OID.to_string(),
            FieldValue::Text(input.blob_oid.clone()),
        );
    }

    SearchProjection {
        text,
        fields,
        lang: Some(Language::Code.tag().to_string()),
    }
}

pub fn trigrams(text: &str) -> Vec<String> {
    let normalized = normalize_for_trigrams(text);
    let chars: Vec<char> = normalized.chars().collect();
    if chars.len() < TRIGRAM_N {
        return Vec::new();
    }
    let mut out: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for window in chars.windows(TRIGRAM_N) {
        out.insert(window.iter().collect());
    }
    out.into_iter().collect()
}

pub fn trigram_query(substring: &str) -> Vec<String> {
    trigrams(substring)
        .iter()
        .map(|t| trigram_token(t))
        .collect()
}

fn trigram_token(trigram: &str) -> String {
    format!("t\u{00b7}{trigram}")
}

fn normalize_for_trigrams(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_was_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !last_was_space {
                out.push(' ');
                last_was_space = true;
            }
        } else {
            for low in ch.to_lowercase() {
                out.push(low);
            }
            last_was_space = false;
        }
    }
    out.trim().to_string()
}

#[derive(Debug, Clone, Copy)]
pub struct ScipLsifFindUsagesFloor;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::IncrementalIndexer;

    #[test]
    fn spec_is_gits_owned_6_5_shape() {
        let s = git_code_projection_spec();
        assert_eq!(s.subsystem, "git");
        assert_eq!(s.type_, "blob");
        assert_eq!(
            s.acl_object_type, "repo",
            "a blob's reachability is its parent repo's"
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
        for facet in [FACET_PATH, FACET_LANGUAGE, FACET_BLOB_OID] {
            assert_eq!(
                s.struct_fields.get(facet),
                Some(&FieldType::Text),
                "`{facet}` is a typed columnar code facet (Text)"
            );
        }
    }

    #[test]
    fn spec_serializes_to_the_6_5_wire_shape() {
        let s = git_code_projection_spec();
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
            "the 6.5 wire key set"
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

    #[test]
    fn registration_is_accepted_by_search() {
        let accepted = register_git_index_specs();
        assert_eq!(
            accepted,
            git_index_specs(),
            "Search accepts the declared git spec verbatim"
        );
        let _ix = IncrementalIndexer::new(
            git_index_specs(),
            std::sync::Arc::new(NullProjectFetcher),
            std::sync::Arc::new(crate::indexer::MockEmbeddingAdapter::new(8)),
        );
    }

    fn rust_blob() -> GitBlobProjectionInput {
        GitBlobProjectionInput {
            path: "src/scheduler/deadlock.rs".into(),
            language: "rust".into(),
            text: "fn detectDeadlock(graph: &WaitForGraph) -> bool {\n    \
                   let msg = \"cycle detected\";\n    graph.has_cycle()\n}"
                .into(),
            literals: vec!["cycle detected".into()],
            commit_message: "fix: resolve the scheduler deadlock detection".into(),
            blob_oid: "blob-oid-abc123".into(),
        }
    }

    #[test]
    fn blob_projection_tokenizes_symbols_camel_snake_operators() {
        let p = git_blob_search_projection(&rust_blob());
        let toks: std::collections::BTreeSet<&str> = p.text.split(' ').collect();

        assert!(toks.contains("detect"), "camel part: {:?}", p.text);
        assert!(toks.contains("deadlock"), "camel part");
        assert!(
            toks.contains("detectdeadlock"),
            "whole identifier kept (exact-identifier hit)"
        );
        assert!(toks.contains("has"));
        assert!(toks.contains("cycle"));
        assert!(toks.contains("->"), "the -> operator is searchable");
        assert_eq!(p.lang.as_deref(), Some("code"));
    }

    #[test]
    fn blob_projection_indexes_path_as_facet_and_fulltext() {
        let p = git_blob_search_projection(&rust_blob());
        assert_eq!(
            p.fields.get(FACET_PATH),
            Some(&FieldValue::Text("src/scheduler/deadlock.rs".into()))
        );
        assert_eq!(
            p.fields.get(FACET_LANGUAGE),
            Some(&FieldValue::Text("rust".into()))
        );
        assert_eq!(
            p.fields.get(FACET_BLOB_OID),
            Some(&FieldValue::Text("blob-oid-abc123".into()))
        );
        let toks: std::collections::BTreeSet<&str> = p.text.split(' ').collect();
        assert!(
            toks.contains("scheduler"),
            "a path segment is full-text searchable"
        );
        assert!(toks.contains("deadlock"));
    }

    #[test]
    fn blob_projection_indexes_literals_and_commit_message() {
        let p = git_blob_search_projection(&rust_blob());
        let toks: std::collections::BTreeSet<&str> = p.text.split(' ').collect();
        assert!(toks.contains("cycle"), "literal token");
        assert!(toks.contains("detected"), "literal token");
        assert!(toks.contains("resolve"), "commit-message token");
        assert!(toks.contains("scheduler"), "commit-message token");
    }

    #[test]
    fn trigram_substring_query_admits_the_blob() {
        let p = git_blob_search_projection(&rust_blob());
        let body_tokens: std::collections::BTreeSet<&str> = p.text.split(' ').collect();

        let q = trigram_query("adlo");
        assert!(!q.is_empty(), "a 4-char substring yields trigrams");
        assert!(
            q.iter().all(|t| body_tokens.contains(t.as_str())),
            "every query trigram is in the blob's trigram set (candidate admit): q={q:?}"
        );

        let absent = trigram_query("zxqwv");
        assert!(
            !absent.iter().all(|t| body_tokens.contains(t.as_str())),
            "a substring absent from the code is not falsely admitted"
        );
    }

    #[test]
    fn trigrams_are_overlapping_char_windows() {
        assert_eq!(trigrams("abcd"), vec!["abc", "bcd"]);
        assert_eq!(trigrams("ABCD"), vec!["abc", "bcd"]);
        assert!(trigrams("ab").is_empty());
        assert_eq!(trigrams("a\n\n  b"), vec!["a b"]);
        assert_eq!(trigrams("café"), vec!["afé", "caf"]);
        assert_eq!(trigrams("aaaa"), vec!["aaa"]);
    }

    #[test]
    fn substring_shorter_than_trigram_yields_no_conjunction() {
        assert!(
            trigram_query("ab").is_empty(),
            "a <3-char substring cannot index - the caller scans"
        );
        assert!(trigram_query("a").is_empty());
    }

    #[test]
    fn trigram_namespace_is_disjoint_from_symbols() {
        let tg = trigram_token("foo");
        assert_ne!(
            tg, "foo",
            "the trigram token is namespaced apart from the identifier token"
        );
        assert!(tg.contains("foo"));
        assert_eq!(trigram_query("foo"), vec![tg]);
    }

    #[test]
    fn raw_code_is_verbatim_x2() {
        let input = GitBlobProjectionInput {
            text: "let `not_markdown` = **value**;".into(),
            ..Default::default()
        };
        let p = git_blob_search_projection(&input);
        let toks: std::collections::BTreeSet<&str> = p.text.split(' ').collect();
        assert!(
            toks.contains("not"),
            "raw code tokenized verbatim (X-2): {:?}",
            p.text
        );
        assert!(toks.contains("markdown"));
        assert!(toks.contains("value"));
    }

    #[test]
    fn empty_blob_projects_empty() {
        let p = git_blob_search_projection(&GitBlobProjectionInput::default());
        assert!(p.text.is_empty(), "no inputs ⇒ no searchable body");
        assert!(p.fields.is_empty(), "no inputs ⇒ no structured facets");
        assert_eq!(
            p.lang.as_deref(),
            Some("code"),
            "still analyzed under the code chain"
        );
    }

    #[test]
    fn the_named_floor_is_constructible() {
        let _floor = ScipLsifFindUsagesFloor;
    }
}
