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

    assert_eq!(emit.changed_blob_count, 2);
    assert_eq!(emit.emitted.len(), 2);

    for id in &emit.emitted {
        let row = outbox.row(id).unwrap();
        assert_eq!(row.envelope.type_.0, GIT_BLOB_SNAPSHOT);
        let pl = &row.envelope.payload;
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
        assert_eq!(pl["acl_object_type"], serde_json::json!("repo"));
    }
}

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

    let spec = git_code_projection_spec();
    let mut doc_keys: Vec<&String> = sp.fields.keys().collect();
    doc_keys.sort();
    let mut spec_keys: Vec<&String> = spec.struct_fields.keys().collect();
    spec_keys.sort();
    assert_eq!(
        doc_keys, spec_keys,
        "the emitted doc's facets match the declared 6.3 spec facets"
    );

    for (k, v) in &sp.fields {
        assert_eq!(
            v.field_type(),
            spec.struct_fields[k],
            "facet `{k}` value type matches the declared spec type"
        );
        assert_eq!(spec.struct_fields[k], FieldType::Text);
    }
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

    assert_eq!(payload["path"].as_str().unwrap(), "a.rs");
    assert_eq!(payload["language"].as_str().unwrap(), "rust");
    assert_eq!(payload["blob_oid"].as_str().unwrap(), "oid-a");
}
