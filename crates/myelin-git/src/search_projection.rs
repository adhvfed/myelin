//! # `search_projection` — git's `declare_indexable` code-projection spec (GIT-P5 / P-231)
//!
//! Git **owns what to index** (overview §1.1: "the indexable CODE PROJECTION … git hosting owns
//! *what* to index"). Search owns the *engine*; git declares — via the frozen contract-6.3
//! `declare_indexable(IndexSpec{ … })` surface ([`myelin_search::IndexSpec`]) — the SHAPE of the
//! `git.*` code projection so the indexer knows, per `(subsystem, type)`, which structured facets a
//! git blob doc carries, whether it is semantically indexed, and which ACL object type the per-viewer
//! reachability filter keys on.
//!
//! **This prompt (GIT-P5) ships the SPEC REGISTRATION only.** The code-projection EMITTER — the
//! receive-pack post-commit hook that walks the indexed ref's tree, builds the per-blob
//! [`myelin_search::SearchProjection`] (path / language / camel·snake-split symbols / literals /
//! commit message / text / blob_oid) and emits it through the outbox — is the **GIT-P25 / P-287**
//! follow-on (architecture `02 §9` TE-27). Here we register the spec and prove it serializes to the
//! 6.3 wire shape; no emitter, no push path.
//!
//! ## The projection shape (architecture `02 §9` — the indexable output git owns)
//!
//! Per blob, on a push to an indexed ref, the emitter (GIT-P25) will project:
//!
//! ```text
//! { artifact_ref: myelin://<tenant>/git/blob/<repo>:<ref>:<path>,
//!   path, language (detected),
//!   symbols:  [ identifiers split on camelCase/snake_case, def-like names ],
//!   literals: [ string/number literals ],
//!   text: <blob text>,                # Search builds trigrams (Cox 2012) over the text
//!   commit_message: <tip commit message>, blob_oid }
//! ```
//!
//! The frozen [`IndexSpec`](myelin_search::IndexSpec) carries the **structured/semantic/acl** half of
//! this; the **full-text + projection body** (`symbols` / `literals` / `commit_message` / `text`)
//! arrives at emit time in the index-time `SearchProjection.text`, not in the spec (the spec is the
//! schema, the projection is the row — `IndexSpec` doc, §4.1). So the structured facets git declares
//! are exactly the columnar/filterable fields a code-search query pins on:
//!
//! | facet      | [`FieldType`](myelin_query::FieldType) | why structured |
//! |------------|---------|----------------|
//! | `path`     | `Text` | exact/prefix path filtering ("find this path across the corpus", GF-3) |
//! | `language` | `Text` | per-language facet filter (the detected language tag) |
//! | `blob_oid` | `Text` | the content-addressed identity of the indexed blob (de-dup / pin) |
//!
//! `symbols` / `literals` / `commit_message` / the blob `text` are **full-text** content (the
//! inverted/trigram shape Search builds at index time over `SearchProjection.text`), NOT structured
//! facets — so they are deliberately absent from `struct_fields`.
//!
//! ## ACL object type — `repo` (NOT `blob`)
//!
//! A git blob doc's per-viewer reachability is decided by its parent **repository**, not by the
//! individual blob: there is no per-blob ACL in git (architecture §6 / the ReBAC `repo` object type,
//! `crate::rebac_fragment::object_types::REPO`). So the spec pins `acl_object_type = "repo"` while
//! `type_ = "blob"` — the query path's ACL conjoin then keys on `repo` (the Identity reverse-index
//! `repo` reachable-set), exactly the object type git's frozen ReBAC fragment exposes.
//!
//! ## Coherence (EI-01 §7)
//!
//! Git does NOT define a second indexing-contract shape: it constructs the ONE frozen
//! [`myelin_search::IndexSpec`] (owned by Search). The registration is "accepted" by Search exactly
//! when [`IncrementalIndexer::new`](myelin_search::IncrementalIndexer::new) admits the spec into its
//! per-tenant facet union without a schema mismatch — proven in this module's tests + the CDC.

use std::collections::BTreeMap;

use myelin_query::FieldType;
use myelin_search::IndexSpec;

/// The subsystem token git declares its projection under (`git`, the Bus §6.2 / overview token —
/// the same token [`crate::subs::GIT_SUBSYSTEM`] anchors).
pub const GIT_SUBSYSTEM: &str = "git";

/// The artifact type git's code projection indexes — a `blob` (a single indexed file at a path on an
/// indexed ref). The canonical ref is `myelin://<tenant>/git/blob/<repo>:<ref>:<path>` (subs §2).
pub const GIT_BLOB_TYPE: &str = "blob";

/// The ACL object type the blob doc's reachability filter pins on — the parent **`repo`** (there is
/// no per-blob ACL; the repository decides reachability — architecture §6). Equal to
/// [`crate::rebac_fragment::object_types::REPO`].
pub const GIT_BLOB_ACL_OBJECT_TYPE: &str = "repo";

/// The structured-facet key for the indexed blob's path within the repo (the columnar field a path
/// filter pins on — GF-3 "find this path").
pub const FACET_PATH: &str = "path";
/// The structured-facet key for the detected source language tag.
pub const FACET_LANGUAGE: &str = "language";
/// The structured-facet key for the content-addressed blob object id (the indexed blob's identity).
pub const FACET_BLOB_OID: &str = "blob_oid";

/// Build git's **`declare_indexable` code-projection spec** (contract 6.3) — the deliverable of
/// GIT-P5 / P-231. The returned [`IndexSpec`] is the frozen Search-owned shape git registers:
/// `subsystem = "git"`, `type = "blob"`, the three structured facets (`path` / `language` /
/// `blob_oid`), non-semantic (code is trigram/symbol full-text, not vector-embedded in v1 —
/// architecture `02 §9` GF-3), `acl_object_type = "repo"`.
///
/// The full-text projection body (symbols / literals / commit message / blob text) is NOT in the
/// spec — it arrives at emit time in the index-time `SearchProjection` (the GIT-P25 emitter). This
/// function registers the SCHEMA; the emitter ships the rows.
pub fn git_code_projection_spec() -> IndexSpec {
    let mut struct_fields: BTreeMap<String, FieldType> = BTreeMap::new();
    // The structured/columnar facets a code-search query filters on (the full-text body — symbols,
    // literals, commit message, text — is delivered via SearchProjection.text at emit time, NOT here).
    struct_fields.insert(FACET_PATH.to_string(), FieldType::Text);
    struct_fields.insert(FACET_LANGUAGE.to_string(), FieldType::Text);
    struct_fields.insert(FACET_BLOB_OID.to_string(), FieldType::Text);

    IndexSpec::new(GIT_SUBSYSTEM, GIT_BLOB_TYPE, struct_fields)
        // Code is NOT vector-embedded in v1 — trigram + symbol/path/literal full-text is the GF-3
        // floor (architecture 02 §9). Semantic code-embedding is a post-v1 follow-on.
        .with_acl_object_type(GIT_BLOB_ACL_OBJECT_TYPE)
}

/// **Register git's code-projection spec WITH Search (the GATE).** Builds the
/// [`git_code_projection_spec`] and proves Search **accepts** it by admitting it into a live
/// [`IncrementalIndexer`](myelin_search::IncrementalIndexer)'s per-tenant facet union — the only
/// honest definition of "accepted" (Search is the authority that admits; git does not get to assert
/// acceptance). Returns the spec that was accepted, so a caller can assert the registered shape.
///
/// In production the spec is handed to Search's `declare_indexable` registration at subsystem
/// boot; here we exercise that admission directly (the indexer constructor IS the build-time
/// declare_indexable registration surface — `IndexSpec` doc, §4.1). No fetcher/embedder is needed
/// for the SPEC registration; this proves the schema is admitted without a schema mismatch.
pub fn register_git_code_projection_spec() -> IndexSpec {
    let spec = git_code_projection_spec();
    // Admit it into a real indexer's facet union (the build-time declare_indexable surface). If the
    // facet types collided or the shape were malformed this would panic at construction; it does not.
    let _accepted = myelin_search::IncrementalIndexer::new(
        vec![spec.clone()],
        std::sync::Arc::new(NullProjectFetcher),
        std::sync::Arc::new(myelin_search::MockEmbeddingAdapter::new(8)),
    );
    spec
}

/// A do-nothing [`ProjectFetcher`](myelin_search::ProjectFetcher) used ONLY to admit the spec into a
/// live indexer for the registration GATE (the SPEC half ships here; the real owner-`project` fetch
/// is the GIT-P25 emitter / GIT-P9 project surface). It never fetches — registration does not index.
struct NullProjectFetcher;

impl myelin_search::ProjectFetcher for NullProjectFetcher {
    fn project(
        &self,
        _tenant: &myelin_tenancy::TenantId,
        _region: &myelin_tenancy::Region,
        _ref_: &myelin_tenancy::ArtifactRef,
    ) -> Result<myelin_search::SearchProjection, myelin_search::ProjectFetchError> {
        // The SPEC registration never fetches a projection (no emitter here — GIT-P25). A blob that
        // is never asked-for projects to nothing; this is the registration GATE, not the index path.
        Err(myelin_search::ProjectFetchError::Gone)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The spec is git's owned 6.3 shape.** Pins every field of the frozen `IndexSpec` git
    /// registers (a rename of a Search field would break this — the registrant catches it).
    #[test]
    fn spec_is_gits_owned_6_3_shape() {
        let s = git_code_projection_spec();
        assert_eq!(s.subsystem, "git", "git owns the `git` subsystem projection");
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
        assert!(!s.semantic, "code is trigram/symbol full-text, not vector-embedded in v1 (GF-3)");
        // The structured facets: path / language / blob_oid (the columnar filter shape).
        assert_eq!(s.struct_fields.len(), 3, "exactly the three structured code facets");
        assert_eq!(s.struct_fields.get("path"), Some(&FieldType::Text));
        assert_eq!(s.struct_fields.get("language"), Some(&FieldType::Text));
        assert_eq!(s.struct_fields.get("blob_oid"), Some(&FieldType::Text));
    }

    /// **The full-text projection body is NOT a structured facet.** `symbols` / `literals` /
    /// `commit_message` / `text` arrive at emit time in `SearchProjection.text` (GIT-P25), so they
    /// must be absent from `struct_fields` (the schema is the columnar half, not the body).
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

    /// **The spec serializes to the 6.3 wire shape (0 schema mismatches — the build-time gate).**
    /// Asserts the serialized JSON key set + values against the frozen contract-6.3 keys
    /// (`subsystem` / `type` / `struct_fields` / `semantic` / `acl_object_type`). A wire rename of
    /// any key is caught here (this IS the CDC of git's owned spec half).
    #[test]
    fn spec_serializes_to_the_6_3_wire_shape() {
        let s = git_code_projection_spec();
        let json = serde_json::to_value(&s).expect("the spec serializes");
        let obj = json.as_object().expect("the spec is a JSON object");

        // The exact frozen key set (the `type` rename, not `type_`, is the wire contract).
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["acl_object_type", "semantic", "struct_fields", "subsystem", "type"],
            "the 6.3 wire key set"
        );

        // The values.
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

    /// **The registration is ACCEPTED by Search (the GATE).** Search admits the spec into a live
    /// indexer's per-tenant facet union without a schema mismatch — the returned accepted spec is
    /// byte-equal to the declared one (registration neither mutates nor rejects the shape).
    #[test]
    fn registration_is_accepted_by_search() {
        let accepted = register_git_code_projection_spec();
        assert_eq!(accepted, git_code_projection_spec(), "Search accepts the declared spec verbatim");
    }
}
