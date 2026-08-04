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

#[test]
fn cdc_s5_caches_the_typed_list_objects_result() {
    let (pin, key_ref) = pin_with_dek();
    let cache = FilterCache::new(CacheTtl::bounded(60, 300).unwrap(), pin);

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
    let hit = cache
        .get_or_compute(
            &tenant(),
            &region(),
            &subject(),
            &ty(),
            &bounded(5),
            &key_ref,
            || panic!("must be a hit - the typed object was cached"),
        )
        .unwrap();
    assert_eq!(hit, ids);

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

#[test]
fn cdc_s5_zookie_bypass_and_bucketing() {
    assert!(
        should_bypass(&strong(7)),
        "a strong read bypasses (4.10 read-your-writes)"
    );
    assert!(
        !should_bypass(&bounded(7)),
        "a bounded read may use the cache (degrade-not-cascade)"
    );
    assert_eq!(zookie_bucket("z@7"), 7);
    assert_ne!(
        zookie_bucket("z@7"),
        zookie_bucket("z@8"),
        "different buckets, different entries"
    );
}

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

#[test]
fn cdc_caches_crypto_shred_under_the_index_dek() {
    let (pin, key_ref) = pin_with_dek();
    let filter_cache = FilterCache::new(CacheTtl::bounded(60, 300).unwrap(), pin.clone());
    let result_cache = ResultCache::new(CacheTtl::bounded(60, 300).unwrap(), pin.clone());

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

    assert!(
        pin.destroy_tenant_index_dek(&tenant(), &region()),
        "the index DEK was present"
    );

    assert!(
        filter_cache
            .probe_recoverable(&tenant(), &region(), &subject(), &ty(), "z@5", &key_ref)
            .is_err(),
        "a destroyed per-tenant index DEK renders the S5 cache unrecoverable (crypto-shred)"
    );
    let sealed = result_cache.get_or_compute(
        &tenant(),
        &region(),
        &subject(),
        1,
        &bounded(5),
        &key_ref,
        || myelin_search::RankedResults {
            hits: vec![],
            zookie: "z@5".into(),
            post_fetch_fields: vec![],
        },
    );
    assert!(
        sealed.is_err(),
        "the result cache crypto-shreds under the same per-tenant index DEK (the seal fails loudly \
         on a destroyed key - never plaintext at rest without a key)"
    );
}
