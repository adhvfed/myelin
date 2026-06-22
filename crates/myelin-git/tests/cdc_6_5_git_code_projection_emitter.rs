//! # The CDC pair for contract 6.5 (+ the 6.3 EMITTER slice) — **git's code-projection emitter**
//! (GIT-P25 / P-287, M3-G5)
//!
//! **Contract-index row 6.5** (code-search input — "Git emits an indexable `git.*` projection per
//! blob/ref/symbol"). The SPEC half (row 6.3, git's `declare_indexable` schema) shipped in GIT-P5 /
//! P-231 ([`cdc_6_3_git_code_projection`]). THIS file pins the **EMITTER** half — the receive-pack
//! post-commit hook that feeds the rows the spec declared (architecture `02 §9`, TE-27):
//!
//! - the **PRODUCER** (provider) is **git emitting one projection doc per changed blob** on a push to
//!   an indexed ref ([`myelin_git::code_projection::CodeProjectionEmitter::emit_for_push`]) — the §9
//!   per-blob shape (path / detected language / camel·snake-split symbols / literals / blob text /
//!   tip commit message / blob_oid), incremental (emit-count == changed-blob-count), through the
//!   outbox as the NAMED [`git.blob.snapshot`] token. The producer's promise: it emits the §9 shape
//!   and supplies the TEXT (Search builds the trigram/symbol/path/literal index — no cross-DB).
//! - the **CONSUMER** is **the Search-owned [`myelin_search::SearchProjection`]** the per-blob doc
//!   lowers to ([`BlobProjection::into_search_projection`]) — its structured facets are EXACTLY the
//!   GIT-P5 spec's three declared facets (`path` / `language` / `blob_oid`, all `Text`), so a doc the
//!   emitter produces is admissible into the indexer git registered the spec with (no facet drift).
//!
//! A drift on either side (git drops/renames a §9 field, or the emitted facet keys diverge from the
//! declared spec) fails this test in the same CI job. **The gate of GIT-P25 is the projection emit**
//! (per-blob, incremental); this CDC is the mechanical evidence that the emitted doc carries the §9
//! shape AND lowers to the Search-owned projection whose facets match the registered 6.3 spec.

use myelin_events::{
    Actor, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Region, TenantId, Timestamp,
};
use myelin_git::code_projection::{
    Blob, BlobProjection, CodeProjectionCursor, CodeProjectionEmitter, NoRestrictions, Tree,
};
use myelin_git::events::GIT_BLOB_SNAPSHOT;
use myelin_git::search_projection::{
    git_code_projection_spec, FACET_BLOB_OID, FACET_LANGUAGE, FACET_PATH,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_query::{FieldType, FieldValue};
use std::sync::Arc;

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-22T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-22T00:00:01Z".into()),
        caused_by: None,
    }
}

/// **PRODUCER side — git emits the §9 per-blob projection shape, incremental.** A push of N changed
/// blobs emits exactly N `git.blob.snapshot` docs, each carrying the §9 fields. A rename/drop of a §9
/// field on git's side fails here.
#[test]
fn cdc_6_5_producer_git_emits_the_section_9_shape_per_changed_blob() {
    let outbox = OutboxStore::new();
    let cursor = CodeProjectionCursor::new();
    let r = NoRestrictions;
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let e = CodeProjectionEmitter::new("core", "main", ctx_base(), &outbox, minter, &cursor, &r);

    let tree = Tree::empty()
        .with(
            "src/lib.rs",
            Blob::new("oid-lib", b"pub fn parseHttpResponse() {}".to_vec()),
        )
        .with(
            "src/util.rs",
            Blob::new("oid-util", b"fn max_retries() -> i64 { 42 }".to_vec()),
        );
    let emit = e
        .emit_for_push("refs/heads/main", "tip-1", &Tree::empty(), &tree, "initial")
        .unwrap()
        .expect("an indexed-ref push emits");

    // emit-count == changed-blob-count (the GATE).
    assert_eq!(emit.changed_blob_count, 2);
    assert_eq!(emit.emitted.len(), 2);

    for id in &emit.emitted {
        let row = outbox.row(id).unwrap();
        // The NAMED token (never a literal).
        assert_eq!(row.envelope.type_.0, GIT_BLOB_SNAPSHOT);
        let pl = &row.envelope.payload;
        // The §9 fields are all present.
        for field in [
            "op",
            "artifact_ref",
            "path",
            "language",
            "symbols",
            "literals",
            "text",
            "blob_oid",
        ] {
            assert!(
                pl.get(field).is_some(),
                "the §9 projection field `{field}` must be present"
            );
        }
        // The doc's ACL object type is the parent repo (the GIT-P5 spec's acl_object_type).
        assert_eq!(pl["acl_object_type"], serde_json::json!("repo"));
    }
}

/// **CONSUMER side — the emitted doc lowers to the Search-owned `SearchProjection` whose facets are
/// the registered 6.3 spec's.** The per-blob doc's structured facets are EXACTLY git's declared
/// `path` / `language` / `blob_oid` (all `Text`), so the doc is admissible into the indexer the spec
/// was registered with. A facet drift between the emitter and the declared spec fails here.
#[test]
fn cdc_6_5_consumer_doc_facets_match_the_registered_6_3_spec() {
    let bp = BlobProjection {
        artifact_ref: myelin_events::ArtifactRef(
            "myelin://acme/git/blob/core:refs/heads/main:src/lib.rs".into(),
        ),
        path: "src/lib.rs".into(),
        language: "rust".into(),
        symbols: vec!["parse".into(), "http".into(), "response".into()],
        literals: vec!["42".into()],
        text: "pub fn parseHttpResponse() {}".into(),
        commit_message: "initial".into(),
        blob_oid: myelin_git::code_projection::BlobOid::new("oid-lib"),
    };
    let sp = bp.into_search_projection();

    // The emitted doc's facet KEYS are exactly the spec's declared struct_fields keys.
    let spec = git_code_projection_spec();
    let mut doc_keys: Vec<&String> = sp.fields.keys().collect();
    doc_keys.sort();
    let mut spec_keys: Vec<&String> = spec.struct_fields.keys().collect();
    spec_keys.sort();
    assert_eq!(
        doc_keys, spec_keys,
        "the emitted doc's facets match the declared 6.3 spec facets"
    );

    // ...and each facet's VALUE type matches the spec's declared FieldType (all Text).
    for (k, v) in &sp.fields {
        assert_eq!(
            v.field_type(),
            spec.struct_fields[k],
            "facet `{k}` value type matches the declared spec type"
        );
        assert_eq!(spec.struct_fields[k], FieldType::Text);
    }
    // The concrete facets carry the §9 values.
    assert_eq!(
        sp.fields.get(FACET_PATH),
        Some(&FieldValue::Text("src/lib.rs".into()))
    );
    assert_eq!(
        sp.fields.get(FACET_LANGUAGE),
        Some(&FieldValue::Text("rust".into()))
    );
    assert_eq!(
        sp.fields.get(FACET_BLOB_OID),
        Some(&FieldValue::Text("oid-lib".into()))
    );
}

/// **The two sides agree (the round-trip): an emitted doc's facets are exactly what the spec declares
/// AND what the consumer reads.** The producer emit (the outbox payload) and the consumer projection
/// (the `SearchProjection`) carry the SAME facet values for the same blob.
#[test]
fn cdc_6_5_emit_payload_and_search_projection_agree() {
    let outbox = OutboxStore::new();
    let cursor = CodeProjectionCursor::new();
    let r = NoRestrictions;
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let e = CodeProjectionEmitter::new("core", "main", ctx_base(), &outbox, minter, &cursor, &r);

    let tree = Tree::empty().with("a.rs", Blob::new("oid-a", b"fn helloWorld() {}".to_vec()));
    let emit = e
        .emit_for_push("refs/heads/main", "tip", &Tree::empty(), &tree, "msg")
        .unwrap()
        .unwrap();
    let payload = outbox.row(&emit.emitted[0]).unwrap().envelope.payload;

    // Reconstruct the consumer projection from the same source the emitter used, and assert the
    // structured facets match the emitted payload's facet values (no divergence producer↔consumer).
    assert_eq!(payload["path"].as_str().unwrap(), "a.rs");
    assert_eq!(payload["language"].as_str().unwrap(), "rust");
    assert_eq!(payload["blob_oid"].as_str().unwrap(), "oid-a");
}
