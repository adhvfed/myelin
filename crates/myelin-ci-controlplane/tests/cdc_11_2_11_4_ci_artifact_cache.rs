use myelin_ci_controlplane::artifact_cache::{
    derive_cache_scope, select_log_segment_dek, RunProvenance, SegmentPii,
};
use myelin_storage::blob::FsBlobStore;
use myelin_storage::ci_cache_scope::{CacheScope, CacheScopeError, CiCacheNamespace, TrustTier};
use myelin_storage::kms::KeyClass;
use myelin_tenancy::TenantId;

fn tenant() -> TenantId {
    TenantId::from_token("acme")
}

#[test]
fn ci_derived_fork_scope_writes_own_scope_and_is_refused_at_trusted() {
    let base = FsBlobStore::new();
    let cache = CiCacheNamespace::over(tenant(), &base);

    let prov = RunProvenance {
        trust_tier: "untrusted_fork".into(),
        protected_branch: None,
        pr_id: Some("42".into()),
    };
    let (tier, own_scope, run_pr) = derive_cache_scope(&prov).expect("fork derives its scope");
    assert_eq!(tier, TrustTier::UntrustedFork);
    assert_eq!(own_scope, CacheScope::Fork { pr_id: "42".into() });

    cache
        .put(tier, &run_pr, &own_scope, "deps", b"fork-deps")
        .expect("fork writes its own scope (the two halves agree)");
    assert!(cache.contains(&own_scope, "deps").unwrap());

    let refused = cache.put(tier, &run_pr, &CacheScope::Trusted, "deps", b"poison");
    assert!(matches!(
        refused,
        Err(CacheScopeError::ForkWriteToTrusted { .. })
    ));
    assert!(!cache.contains(&CacheScope::Trusted, "deps").unwrap());
}

#[test]
fn ci_derived_trusted_scope_writes_trusted_and_branch() {
    let base = FsBlobStore::new();
    let cache = CiCacheNamespace::over(tenant(), &base);

    let trusted = RunProvenance {
        trust_tier: "trusted".into(),
        protected_branch: None,
        pr_id: None,
    };
    let (tier, scope, pr) = derive_cache_scope(&trusted).expect("trusted derives");
    cache
        .put(tier, &pr, &scope, "deps", b"trusted-deps")
        .expect("trusted writes trusted");
    assert!(cache.contains(&CacheScope::Trusted, "deps").unwrap());

    let branch = RunProvenance {
        trust_tier: "trusted".into(),
        protected_branch: Some("main".into()),
        pr_id: None,
    };
    let (btier, bscope, bpr) = derive_cache_scope(&branch).expect("branch derives");
    assert_eq!(
        bscope,
        CacheScope::Branch {
            name: "main".into()
        }
    );
    cache
        .put(btier, &bpr, &bscope, "deps", b"branch-deps")
        .expect("trusted writes branch");
}

#[test]
fn ci_dek_selection_matches_the_storage_pii_key_ref_grammar() {
    let subj = select_log_segment_dek(
        &tenant(),
        3,
        &SegmentPii::IsolableSubject {
            subject_id: "u-42".into(),
        },
    );
    assert_eq!(subj.class, KeyClass::Subject("u-42".into()));
    assert_eq!(subj.to_uri(), "kms://acme/3/subject:u-42");

    let bulk = select_log_segment_dek(&tenant(), 0, &SegmentPii::NotIsolable);
    assert_eq!(bulk.class, KeyClass::Tenant);
    assert_eq!(bulk.to_uri(), "kms://acme/0/tenant");
}
