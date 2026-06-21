//! Contract 11.2-C3 CDC pair — git's **consumer half** of the within-EU CDN clone/bundle class via
//! the bundle-URI accelerated-clone path (GIT-P15 / global P-276, M3-G2).
//!
//! The storage-side CDC (`myelin-storage/src/cdn.rs` tests, P-254) owns the [`CdnCloneClass`]
//! PROVIDER (the content-addressed within-EU clone/bundle blob class). GIT-P15 lands the REAL git
//! consumer — the [`myelin_git::shed_clone::BundleUriClone`] — so this is the consumer-driven
//! contract test with the ACTUAL consumer type:
//!
//! - **PROVIDER:** `myelin-storage` — [`myelin_storage::cdn::CdnCloneClass`] over the unchanged
//!   content-addressed [`myelin_storage::blob::BlobStore`] (the bundle is a content-addressed blob;
//!   the address IS the cache-validity check — a tampered bundle is refused).
//! - **CONSUMER:** `myelin-git` — the [`myelin_git::shed_clone::BundleUriClone`] accelerated-clone
//!   path: publish a precomputed bundle → advertise a bundle-URI → serve a clone by content-address.
//!
//! The load-bearing contract this pins: a clone served a bundle-URI from the CDN class round-trips a
//! VALID clone (the accelerated-clone floor holds), and a tampered bundle is REFUSED (0 silent serve)
//! — the content-address is the cache-validity check, so the git consumer never serves corrupt clone
//! bytes off the edge. If the provider's CDN shapes drift, this stops compiling/passing.

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

/// **CDC: a bundle-URI clone round-trips a valid clone (the accelerated-clone floor — the contract).**
/// The serving tier publishes a precomputed bundle into the CDN class; the cloning client fetches by
/// the advertised content-address; the round-tripped bytes are the exact bundle.
#[test]
fn cdc_11_2_c3_bundle_uri_clone_round_trips_a_valid_clone() {
    let store = FsBlobStore::new();
    let path = BundleUriClone::new(eu_cdn(&store, "acme"));

    let bundle_bytes = b"PACK\0clone-bundle@refsnapshot";
    let uri = path.publish_bundle(bundle_bytes).expect("publish the bundle → bundle-URI");
    // the URI carries the content-address (the CDN cache key, `transfer.bundleURI`).
    assert_eq!(uri.content_hash, ContentHash::blake3(bundle_bytes));

    // the accelerated clone: fetch by the bundle-URI → the exact repo bytes.
    let cloned = path.clone_via_bundle_uri(&uri).expect("clone via the bundle-URI");
    assert_eq!(cloned, bundle_bytes, "the bundle-URI clone round-trips the exact bytes");
}

/// **CDC: the bundle is content-addressed — the SAME base BlobStore backs the CDN serve (the CDN is
/// a delivery layer over the unchanged store, not a new store).** The bytes published through the git
/// consumer are readable through the storage class by the same content-address.
#[test]
fn cdc_11_2_c3_the_bundle_rides_the_unchanged_content_addressed_store() {
    let store = FsBlobStore::new();
    let cdn = eu_cdn(&store, "acme");
    // the git consumer publishes through its accelerated-clone path...
    let path = BundleUriClone::new(eu_cdn(&store, "acme"));
    let uri = path.publish_bundle(b"shared-backing-bundle").expect("publish");
    // ...and the SAME storage CDN class serves the SAME bytes by the SAME content-address.
    let via_storage = cdn.bundle(&uri.content_hash).expect("the storage class has the bundle");
    assert_eq!(via_storage, b"shared-backing-bundle");
}

/// **CDC: a tampered bundle is REFUSED (0 silent serve — the content-address-as-validity contract).**
#[test]
fn cdc_11_2_c3_a_tampered_bundle_is_refused() {
    let store = FsBlobStore::new();
    let path = BundleUriClone::new(eu_cdn(&store, "acme"));
    let uri = path.publish_bundle(b"valid-clone-bundle").expect("publish");

    assert!(store.corrupt_for_drill(&tenant("acme"), &uri.content_hash), "bundle present to corrupt");
    let err = path.clone_via_bundle_uri(&uri).expect_err("a tampered bundle MUST be refused");
    assert!(matches!(err, BundleCloneError::Fetch { .. }), "0 silent serve: {err}");
}
