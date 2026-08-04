use myelin_storage::{CacheScope, CacheScopeError, CiCacheNamespace, FsBlobStore, TrustTier};
use myelin_tenancy::TenantId;

struct CiCacheClient<'a, 'b> {
    cache: &'a CiCacheNamespace<'b>,
    trust_tier: TrustTier,
    run_pr_id: String,
}

impl<'a, 'b> CiCacheClient<'a, 'b> {
    fn for_run(
        cache: &'a CiCacheNamespace<'b>,
        trust_tier: TrustTier,
        run_pr_id: &str,
    ) -> CiCacheClient<'a, 'b> {
        CiCacheClient {
            cache,
            trust_tier,
            run_pr_id: run_pr_id.to_string(),
        }
    }

    fn save(&self, scope: &CacheScope, name: &str, bytes: &[u8]) -> Result<(), CacheScopeError> {
        self.cache
            .put(self.trust_tier, &self.run_pr_id, scope, name, bytes)
            .map(|_| ())
    }

    fn restore(&self, scope: &CacheScope, name: &str) -> Result<Vec<u8>, CacheScopeError> {
        self.cache.get(scope, name)
    }

    fn cache_scope_violation(&self) -> u64 {
        self.cache.telemetry().cache_scope_violation()
    }
}

#[test]
fn cdc_11_2_c4_fork_reads_trusted_but_its_write_to_trusted_is_refused() {
    let store = FsBlobStore::new();
    let cache = CiCacheNamespace::over(TenantId::from_token("acme"), &store);

    let trusted_run = CiCacheClient::for_run(&cache, TrustTier::Trusted, "main");
    trusted_run
        .save(&CacheScope::Trusted, "deps", b"resolved-deps")
        .expect("a trusted run writes the trusted scope");

    let fork_run = CiCacheClient::for_run(&cache, TrustTier::UntrustedFork, "42");

    assert_eq!(
        fork_run
            .restore(&CacheScope::Trusted, "deps")
            .expect("a fork may read the trusted scope"),
        b"resolved-deps"
    );

    let refused = fork_run.save(&CacheScope::Trusted, "deps", b"poison");
    assert!(
        matches!(refused, Err(CacheScopeError::ForkWriteToTrusted { .. })),
        "the provider REFUSES a fork write to the trusted scope, got {refused:?}"
    );
    assert_eq!(fork_run.cache_scope_violation(), 1);

    assert_eq!(
        trusted_run.restore(&CacheScope::Trusted, "deps").unwrap(),
        b"resolved-deps"
    );
}

#[test]
fn cdc_11_2_c4_fork_write_is_confined_to_its_own_scope() {
    let store = FsBlobStore::new();
    let cache = CiCacheNamespace::over(TenantId::from_token("acme"), &store);
    let fork_run = CiCacheClient::for_run(&cache, TrustTier::UntrustedFork, "42");
    let own = CacheScope::Fork {
        pr_id: "42".to_string(),
    };

    fork_run
        .save(&own, "artifact", b"fork-artifact")
        .expect("a fork writes its own fork:<pr_id> scope");
    assert_eq!(fork_run.cache_scope_violation(), 0);

    assert_eq!(
        fork_run.restore(&own, "artifact").unwrap(),
        b"fork-artifact"
    );

    let trusted_run = CiCacheClient::for_run(&cache, TrustTier::Trusted, "main");
    assert!(matches!(
        trusted_run.restore(&CacheScope::Trusted, "artifact"),
        Err(CacheScopeError::Miss { .. })
    ));
}
