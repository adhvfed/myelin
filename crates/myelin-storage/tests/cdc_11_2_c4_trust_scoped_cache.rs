//! Contract 11.2-C4 CDC pair — the **trust-scoped CI cache namespaces** (P-ST-28 / global P-330).
//!
//! The prompt requires "the provider+consumer pair for 11.2-C4 (the CI cache consumer)". This is the
//! consumer-driven contract test: the PROVIDER is `myelin-storage` (the [`CiCacheNamespace`] over the
//! unchanged content-addressed `BlobStore` trait this prompt ships); the CONSUMER is the **CI cache
//! client** — the CI subsystem stamps a run's `trust_tier` from its provenance and hands it (with the
//! run's own `pr_id`) to the cache on every write, never recomputing trust in Storage. The test pins
//! the frozen call shape CI relies on: if `put` / `get` / the [`TrustTier`] enum / the
//! [`CacheScope`] shape drift, it stops compiling/passing.
//!
//! **The load-bearing contract this pins (storage.md §3.2 C4):**
//! 1. CI stamps `trust_tier` from run provenance; Storage ENFORCES the write-scope rule (it does NOT
//!    recompute trust — the tier is an INPUT);
//! 2. an `untrusted_fork` run may **READ** the `trusted` scope (a cache hit is fine) but its **WRITE**
//!    to `trusted` is **REFUSED by the blob client** (the poisoned-cache defence);
//! 3. a fork's write lands only in `fork:<pr_id>` — a trusted read of the same name MISSES
//!    (0 cross-scope cache writes; `cache_scope_violation` = 0 LANDINGS).

use myelin_storage::{CacheScope, CacheScopeError, CiCacheNamespace, FsBlobStore, TrustTier};
use myelin_tenancy::TenantId;

/// The 11.2-C4 CONSUMER: a CI cache client. CI stamps a run's `trust_tier` from its provenance (a
/// fork PR → `UntrustedFork`; everything else → `Trusted`) and carries the run's own `pr_id`; it
/// hands BOTH to the cache on every write. It NEVER recomputes trust — that is exactly the
/// "Storage enforces, CI stamps" split the contract freezes.
struct CiCacheClient<'a, 'b> {
    /// The SHARED per-tenant cache namespace (one store across all of a tenant's CI runs — a fork's
    /// write and a trusted run's read hit the SAME namespace; the confinement is the scope, not a
    /// separate store).
    cache: &'a CiCacheNamespace<'b>,
    /// CI's stamp off run provenance (the INPUT Storage enforces against).
    trust_tier: TrustTier,
    /// The run's own PR id (the only scope a fork run may write is `fork:<this>`).
    run_pr_id: String,
}

impl<'a, 'b> CiCacheClient<'a, 'b> {
    /// Boot a CI cache client for a run whose `trust_tier` CI stamped off provenance, over the
    /// SHARED per-tenant cache namespace.
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

    /// Save a cache entry into `scope` — CI hands the stamped `trust_tier` + the run's `pr_id`; the
    /// provider enforces the write-scope rule.
    fn save(&self, scope: &CacheScope, name: &str, bytes: &[u8]) -> Result<(), CacheScopeError> {
        self.cache
            .put(self.trust_tier, &self.run_pr_id, scope, name, bytes)
            .map(|_| ())
    }

    /// Restore a cache entry from `scope` (a read — a fork may read the trusted scope).
    fn restore(&self, scope: &CacheScope, name: &str) -> Result<Vec<u8>, CacheScopeError> {
        self.cache.get(scope, name)
    }

    fn cache_scope_violation(&self) -> u64 {
        self.cache.telemetry().cache_scope_violation()
    }
}

/// THE CDC pair: a trusted CI run populates the trusted cache; an `untrusted_fork` run READS it (a
/// cache hit is fine) but its WRITE to the trusted scope is REFUSED — the provider honours the frozen
/// 11.2-C4 write-scope rule the CI consumer relies on.
#[test]
fn cdc_11_2_c4_fork_reads_trusted_but_its_write_to_trusted_is_refused() {
    let store = FsBlobStore::new();
    // The SHARED per-tenant cache namespace — all of the tenant's runs hit the same store.
    let cache = CiCacheNamespace::over(TenantId::from_token("acme"), &store);

    // A trusted run (CI stamped `Trusted` off provenance) populates the trusted build cache.
    let trusted_run = CiCacheClient::for_run(&cache, TrustTier::Trusted, "main");
    trusted_run
        .save(&CacheScope::Trusted, "deps", b"resolved-deps")
        .expect("a trusted run writes the trusted scope");

    // An untrusted_fork run (CI stamped `UntrustedFork` off provenance, PR 42).
    let fork_run = CiCacheClient::for_run(&cache, TrustTier::UntrustedFork, "42");

    // It may READ the trusted scope — a cache hit is fine (the asymmetric rule).
    assert_eq!(
        fork_run
            .restore(&CacheScope::Trusted, "deps")
            .expect("a fork may read the trusted scope"),
        b"resolved-deps"
    );

    // But its WRITE to the trusted scope is REFUSED by the blob client (the poisoned-cache defence).
    let refused = fork_run.save(&CacheScope::Trusted, "deps", b"poison");
    assert!(
        matches!(refused, Err(CacheScopeError::ForkWriteToTrusted { .. })),
        "the provider REFUSES a fork write to the trusted scope, got {refused:?}"
    );
    // The attempted poisoning is OBSERVED (telemetry fires on the refusal).
    assert_eq!(fork_run.cache_scope_violation(), 1);

    // 0 cross-scope LANDING: the trusted "deps" is still the trusted run's bytes, never the poison.
    assert_eq!(
        trusted_run.restore(&CacheScope::Trusted, "deps").unwrap(),
        b"resolved-deps"
    );
}

/// The fork's LEGITIMATE write — to its OWN `fork:<pr_id>` scope — succeeds, and is INVISIBLE to a
/// trusted read of the same name (0 cross-scope cache writes; the confinement the CI consumer leans
/// on so a fork build cache never poisons a trusted run).
#[test]
fn cdc_11_2_c4_fork_write_is_confined_to_its_own_scope() {
    let store = FsBlobStore::new();
    let cache = CiCacheNamespace::over(TenantId::from_token("acme"), &store);
    let fork_run = CiCacheClient::for_run(&cache, TrustTier::UntrustedFork, "42");
    let own = CacheScope::Fork {
        pr_id: "42".to_string(),
    };

    // The fork writes its OWN scope — permitted.
    fork_run
        .save(&own, "artifact", b"fork-artifact")
        .expect("a fork writes its own fork:<pr_id> scope");
    assert_eq!(fork_run.cache_scope_violation(), 0);

    // The fork reads its own scope back (a hit).
    assert_eq!(
        fork_run.restore(&own, "artifact").unwrap(),
        b"fork-artifact"
    );

    // A trusted run reading the SAME name in the TRUSTED scope MISSES — the fork's write never
    // reached the trusted scope (the confinement).
    let trusted_run = CiCacheClient::for_run(&cache, TrustTier::Trusted, "main");
    assert!(matches!(
        trusted_run.restore(&CacheScope::Trusted, "artifact"),
        Err(CacheScopeError::Miss { .. })
    ));
}
