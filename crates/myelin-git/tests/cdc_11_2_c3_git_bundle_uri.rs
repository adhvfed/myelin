use myelin_git::shed_clone::{BundleCloneError, BundleUriClone};
use myelin_storage::blob::{ContentHash, FsBlobStore};
use myelin_storage::cdn::CdnCloneClass;
use myelin_tenancy::{Region, TenantId};

fn tenant(s: &str) -> TenantId {
    TenantId::from_token(s)
}

fn eu_cdn<'a>(store: &'a FsBlobStore, t: &str) -> CdnCloneClass<'a> {
    CdnCloneClass::over(tenant(t), Region::new("fr-par"), true, store)
}

#[test]
fn cdc_11_2_c3_bundle_uri_clone_round_trips_a_valid_clone() {
    let store = FsBlobStore::new();
    let path = BundleUriClone::new(eu_cdn(&store, "acme"));

    let bundle_bytes = b"PACK\0clone-bundle@refsnapshot";
    let uri = path
        .publish_bundle(bundle_bytes)
        .expect("publish the bundle → bundle-URI");
    assert_eq!(uri.content_hash, ContentHash::blake3(bundle_bytes));

    let cloned = path
        .clone_via_bundle_uri(&uri)
        .expect("clone via the bundle-URI");
    assert_eq!(
        cloned, bundle_bytes,
        "the bundle-URI clone round-trips the exact bytes"
    );
}

#[test]
fn cdc_11_2_c3_the_bundle_rides_the_unchanged_content_addressed_store() {
    let store = FsBlobStore::new();
    let cdn = eu_cdn(&store, "acme");
    let path = BundleUriClone::new(eu_cdn(&store, "acme"));
    let uri = path
        .publish_bundle(b"shared-backing-bundle")
        .expect("publish");
    let via_storage = cdn
        .bundle(&uri.content_hash)
        .expect("the storage class has the bundle");
    assert_eq!(via_storage, b"shared-backing-bundle");
}

#[test]
fn cdc_11_2_c3_a_tampered_bundle_is_refused() {
    let store = FsBlobStore::new();
    let path = BundleUriClone::new(eu_cdn(&store, "acme"));
    let uri = path.publish_bundle(b"valid-clone-bundle").expect("publish");

    assert!(
        store.corrupt_for_drill(&tenant("acme"), &uri.content_hash),
        "bundle present to corrupt"
    );
    let err = path
        .clone_via_bundle_uri(&uri)
        .expect_err("a tampered bundle MUST be refused");
    assert!(
        matches!(err, BundleCloneError::Fetch { .. }),
        "0 silent serve: {err}"
    );
}
