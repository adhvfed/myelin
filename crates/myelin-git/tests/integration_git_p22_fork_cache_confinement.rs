#![cfg(feature = "integration")]

use myelin_git::check_status::TrustTier;
use myelin_git::fork_gate::{ScopedCache, TrustScope};
use myelin_storage::valkey::ValkeyCache;
use myelin_tenancy::TenantId;
use std::time::Duration;

fn redis_url() -> String {
    std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6380".into())
}

fn tenant(tag: &str) -> TenantId {
    TenantId(format!("p284-{tag}-{}", std::process::id()))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fork_cache_confinement_holds_on_real_valkey() {
    let valkey = ValkeyCache::connect(&redis_url(), tokio::runtime::Handle::current())
        .expect("connect dev Valkey (is the stack up? docker compose -f docker-compose.dev.yml up -d --wait)");
    let t = tenant("confine");
    let ttl = Duration::from_secs(120);

    let fork_scope = TrustScope::for_run(TrustTier::UntrustedFork, "42");
    assert!(
        !fork_scope.is_trusted(),
        "a fork run is structurally never the trusted scope"
    );
    let fork = ScopedCache::new(&valkey, fork_scope);
    fork.set(&t, "dep-graph", b"attacker-controlled", ttl)
        .expect("fork write to the real Valkey");

    let trusted = ScopedCache::new(&valkey, TrustScope::Trusted);
    assert_eq!(
        trusted.get(&t, "dep-graph").expect("real-Valkey get"),
        None,
        "0 fork writes in the trusted scope (a fork cannot poison a trusted run on real Valkey)"
    );
    assert_eq!(
        fork.get(&t, "dep-graph").expect("real-Valkey get"),
        Some(b"attacker-controlled".to_vec()),
        "the fork run reads back its own fork-scoped write"
    );

    let f99 = ScopedCache::new(&valkey, TrustScope::for_run(TrustTier::UntrustedFork, "99"));
    assert_eq!(
        f99.get(&t, "dep-graph").expect("real-Valkey get"),
        None,
        "PR 99's fork scope cannot read PR 42's fork-scoped key"
    );

    let t1 = ScopedCache::new(&valkey, TrustScope::Trusted);
    t1.set(&t, "build-cache", b"shared-trusted", ttl)
        .expect("trusted write to the real Valkey");
    let t2 = ScopedCache::new(&valkey, TrustScope::Trusted);
    assert_eq!(
        t2.get(&t, "build-cache").expect("real-Valkey get"),
        Some(b"shared-trusted".to_vec()),
        "two trusted runs share the trusted scope (build-cache reuse is preserved)"
    );

    let _ = fork.delete(&t, "dep-graph");
    let _ = t1.delete(&t, "build-cache");
}
