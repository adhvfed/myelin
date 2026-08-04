use myelin_storage::{CacheScope, CacheScopeError, CiCacheNamespace, FsBlobStore, TrustTier};
use myelin_tenancy::TenantId;

fn tenant() -> TenantId {
    TenantId("acme".into())
}

#[test]
fn d_s11_trust_scoped_cache_isolation_zero_cross_scope_writes() {
    let base = FsBlobStore::new();
    let cache = CiCacheNamespace::over(tenant(), &base);

    cache
        .put(
            TrustTier::Trusted,
            "main",
            &CacheScope::Trusted,
            "build-cache",
            b"trusted-build-output",
        )
        .expect("a trusted run writes the trusted scope");

    let poison = cache.put(
        TrustTier::UntrustedFork,
        "1337",
        &CacheScope::Trusted,
        "build-cache",
        b"MALICIOUS-PAYLOAD",
    );
    assert!(
        matches!(poison, Err(CacheScopeError::ForkWriteToTrusted { .. })),
        "the poisoned write to the trusted scope MUST be refused, got {poison:?}"
    );

    let fork_scope = CacheScope::Fork {
        pr_id: "1337".to_string(),
    };
    cache
        .put(
            TrustTier::UntrustedFork,
            "1337",
            &fork_scope,
            "build-cache",
            b"fork-build-output",
        )
        .expect("a fork writes its own fork:<pr_id> scope");

    let trusted_read = cache
        .get(&CacheScope::Trusted, "build-cache")
        .expect("the trusted scope still has the trusted run's entry");
    assert_eq!(
        trusted_read, b"trusted-build-output",
        "0 cross-scope landing: a trusted read returns ONLY the trusted bytes, never the fork's"
    );
    assert_ne!(
        trusted_read, b"MALICIOUS-PAYLOAD",
        "the poison NEVER reached the trusted scope"
    );

    assert_eq!(
        cache.get(&fork_scope, "build-cache").unwrap(),
        b"fork-build-output"
    );

    assert_eq!(
        cache.telemetry().cache_scope_violation(),
        1,
        "the one attempted poisoning was OBSERVED (the refusal fired the signal)"
    );

    assert_eq!(
        cache.get(&CacheScope::Trusted, "build-cache").unwrap(),
        b"trusted-build-output",
        "an untrusted_fork run may READ the trusted scope (a cache hit is fine)"
    );
}

#[test]
fn d_s11_per_pr_fork_isolation() {
    let base = FsBlobStore::new();
    let cache = CiCacheNamespace::over(tenant(), &base);

    let fork_a = CacheScope::Fork {
        pr_id: "1".to_string(),
    };
    cache
        .put(TrustTier::UntrustedFork, "1", &fork_a, "k", b"a-bytes")
        .expect("fork A writes its own scope");

    let into_a_from_b = cache.put(TrustTier::UntrustedFork, "2", &fork_a, "k", b"b-poison");
    assert!(matches!(
        into_a_from_b,
        Err(CacheScopeError::ForkWriteToTrusted { .. })
    ));
    assert_eq!(cache.get(&fork_a, "k").unwrap(), b"a-bytes");
}
