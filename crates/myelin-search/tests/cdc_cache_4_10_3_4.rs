//! # CDC — the Search **caches consumer** (contract 4.10 zookie-bucketing + `TTL ≤ revocation SLA`;
//! §3.4 the typed `ListObjectsResult` S5 key; 11.3 the per-tenant index DEK) (SRCH-P13 → P-176).
//!
//! **Architecture:** `search-and-indexing.md` §4.10 (the `list_objects` filter cache S5 holding a
//! typed `ListObjectsResult`, `TTL ≤ revocation SLA`, bypassed for zookie-stamped queries; the
//! hot-query result cache zookie-bucketed + request-coalesced) + §3.4 tail (the S5 cache key
//! `(tenant, region, subject, type, zookie-bucket)`; the cached object is a `ListObjectsResult`, not
//! an opaque blob; never source of truth; residency-pinned + crypto-shred-able under the per-tenant
//! index DEK). Contracts 4.10 (the zookie-bucketing + the `TTL ≤ revocation SLA` bound), 11.3 (the
//! per-tenant index DEK the caches crypto-shred under).
//!
//! Search OWNS no contract here — it CONSUMES the frozen [`myelin_identity::ListObjectsResult`]
//! (4.3, the S5 cached object), the [`myelin_identity::Consistency`]/`ConsistencyMode` zookie shape
//! (4.10, the bucket + the bypass), and the [`myelin_storage`] per-tenant index DEK (11.3, the
//! crypto-shred unit). This CDC pins the CONSUMER side of all three at the cache seam: if the 4.3
//! `ListObjectsResult` shape, the 4.10 zookie/consistency shape, or the 11.3 DEK seal/destroy
//! contract drifts, this stops compiling/passing — that is the contract.
//!
//! The dated green artifact (2026-06-20): the Search S5 filter cache + the hot-query result cache
//! honour the 4.10 zookie-bucketing + `TTL ≤ revocation SLA` bound + the strong-read bypass, cache
//! the typed 4.3 `ListObjectsResult`, and crypto-shred under the 11.3 per-tenant index DEK.

use std::sync::Arc;

use myelin_identity::{
    Consistency, ConsistencyMode, ListObjectsResult, ObjectId, ObjectType, Principal, PrincipalId,
    PrincipalKind, SetExpr, Zookie,
};
use myelin_storage::KmsEngine;
use myelin_tenancy::{Region, TenantId};

use myelin_search::{
    should_bypass, zookie_bucket, CacheTtl, FilterCache, ResultCache, SearchDekPin,
};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn subject() -> Principal {
    Principal::stub(
        PrincipalId("p:alice".into()),
        PrincipalKind::Human,
        tenant(),
    )
}
fn ty() -> ObjectType {
    ObjectType("issue".into())
}
fn bounded(rev: u64) -> Consistency {
    Consistency {
        at_least: Zookie(format!("z@{rev}")),
        mode: ConsistencyMode::BoundedStale,
    }
}
fn strong(rev: u64) -> Consistency {
    Consistency {
        at_least: Zookie(format!("z@{rev}")),
        mode: ConsistencyMode::Strong,
    }
}

fn pin_with_dek() -> (SearchDekPin, myelin_storage::PiiKeyRef) {
    let pin = SearchDekPin::new(Arc::new(KmsEngine::new()));
    let key_ref = pin
        .reserve(&tenant(), &region())
        .expect("reserve per-tenant index DEK");
    (pin, key_ref)
}

/// **CONSUMER (§3.4 / 4.3): the S5 cache holds the TYPED `ListObjectsResult` (`Ids` OR
/// `Filter{set_expr}`), not an opaque blob — both variants round-trip byte-exact through the seal.**
/// If the frozen 4.3 `ListObjectsResult` shape drifts, the sealed round-trip breaks here.
#[test]
fn cdc_s5_caches_the_typed_list_objects_result() {
    let (pin, key_ref) = pin_with_dek();
    let cache = FilterCache::new(CacheTtl::bounded(60, 300).unwrap(), pin);

    // The Ids variant (the S4 materialised path).
    let ids = ListObjectsResult::Ids {
        ids: vec![ObjectId("d1".into()), ObjectId("d2".into())],
        zookie: Zookie("z@5".into()),
    };
    let got = cache
        .get_or_compute(
            &tenant(),
            &region(),
            &subject(),
            &ty(),
            &bounded(5),
            &key_ref,
            || ids.clone(),
        )
        .unwrap();
    assert_eq!(
        got, ids,
        "the S5 cache round-trips the typed Ids ListObjectsResult"
    );
    // Second read is a hit returning the byte-exact typed object (NOT an opaque blob).
    let hit = cache
        .get_or_compute(
            &tenant(),
            &region(),
            &subject(),
            &ty(),
            &bounded(5),
            &key_ref,
            || panic!("must be a hit — the typed object was cached"),
        )
        .unwrap();
    assert_eq!(hit, ids);

    // The Filter{set_expr} variant (the S8 push-down path) on a different type key.
    let filter = ListObjectsResult::Filter {
        set_expr: SetExpr::NotIds(vec![ObjectId("secret".into())]),
        zookie: Zookie("z@5".into()),
    };
    let other_ty = ObjectType("doc".into());
    let got = cache
        .get_or_compute(
            &tenant(),
            &region(),
            &subject(),
            &other_ty,
            &bounded(5),
            &key_ref,
            || filter.clone(),
        )
        .unwrap();
    assert_eq!(
        got, filter,
        "the S5 cache round-trips the typed Filter{{set_expr}} ListObjectsResult"
    );
}

/// **CONSUMER (4.10): the S5 cache is bypassed for a zookie-stamped strong read + zookie-bucketed
/// (no cross-zookie bleed).** The frozen `ConsistencyMode` (4.10) drives the bypass; the zookie
/// revision suffix drives the bucket — a post-revocation read (newer bucket) never reads a
/// pre-revocation entry.
#[test]
fn cdc_s5_zookie_bypass_and_bucketing() {
    // The consumer's bypass decision is the frozen 4.10 mode split.
    assert!(
        should_bypass(&strong(7)),
        "a strong read bypasses (4.10 read-your-writes)"
    );
    assert!(
        !should_bypass(&bounded(7)),
        "a bounded read may use the cache (degrade-not-cascade)"
    );
    // The bucket is the zookie's monotone revision suffix (the no-cross-zookie-bleed key partition).
    assert_eq!(zookie_bucket("z@7"), 7);
    assert_ne!(
        zookie_bucket("z@7"),
        zookie_bucket("z@8"),
        "different buckets, different entries"
    );
}

/// **CONSUMER (4.10 / 1.10): `TTL ≤ revocation SLA` is a structural construct-time bound — a TTL
/// over the revocation SLA does NOT construct.** A revoked grant can never be served from cache past
/// N. This is the SAME constraint the substrate `FailStatic` constructor enforces (1.10 / §8.2),
/// applied to the Search caches.
#[test]
fn cdc_cache_ttl_bounded_by_revocation_sla() {
    assert!(
        CacheTtl::bounded(300, 300).is_ok(),
        "TTL == SLA is the inclusive boundary"
    );
    assert!(
        CacheTtl::bounded(301, 300).is_err(),
        "TTL > revocation SLA must be rejected (no stale-allow past N)"
    );
}

/// **CONSUMER (11.3 / §3.4): both caches crypto-shred under the per-tenant index DEK — a DEK-destroy
/// renders a cached object UNRECOVERABLE.** The frozen 11.3 DEK seal/destroy contract is what makes
/// the cache crypto-shred-able; if it drifts (e.g. a destroyed key silently returns plaintext), this
/// fails.
#[test]
fn cdc_caches_crypto_shred_under_the_index_dek() {
    let (pin, key_ref) = pin_with_dek();
    let filter_cache = FilterCache::new(CacheTtl::bounded(60, 300).unwrap(), pin.clone());
    let result_cache = ResultCache::new(CacheTtl::bounded(60, 300).unwrap(), pin.clone());

    // Cache an S5 filter entry + a result entry.
    filter_cache
        .get_or_compute(
            &tenant(),
            &region(),
            &subject(),
            &ty(),
            &bounded(5),
            &key_ref,
            || ListObjectsResult::Ids {
                ids: vec![ObjectId("d1".into())],
                zookie: Zookie("z@5".into()),
            },
        )
        .unwrap();
    assert!(filter_cache
        .probe_recoverable(&tenant(), &region(), &subject(), &ty(), "z@5", &key_ref)
        .unwrap());

    // Tenant-decommission crypto-shred: destroy the per-tenant index DEK (11.3 / §3.4).
    assert!(
        pin.destroy_tenant_index_dek(&tenant(), &region()),
        "the index DEK was present"
    );

    // The cached S5 entry is now UNRECOVERABLE — a loud KmsError, never a silent plaintext.
    assert!(
        filter_cache
            .probe_recoverable(&tenant(), &region(), &subject(), &ty(), "z@5", &key_ref)
            .is_err(),
        "a destroyed per-tenant index DEK renders the S5 cache unrecoverable (crypto-shred)"
    );
    // And the result cache seals under the SAME DEK — a fresh write now fails the seal loudly
    // (the value is still computed from source, but it cannot be sealed at rest without the key).
    let sealed = result_cache.get_or_compute(
        &tenant(),
        &region(),
        &subject(),
        1,
        &bounded(5),
        &key_ref,
        || myelin_search::RankedResults {
            rebuilding: false,
            hits: vec![],
            zookie: "z@5".into(),
            post_fetch_fields: vec![],
        },
    );
    assert!(
        sealed.is_err(),
        "the result cache crypto-shreds under the same per-tenant index DEK (the seal fails loudly \
         on a destroyed key — never plaintext at rest without a key)"
    );
}
