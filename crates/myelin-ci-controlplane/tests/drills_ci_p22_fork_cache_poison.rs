use myelin_ci_controlplane::artifact_cache::{
    derive_cache_scope, ForkPoisonOutcome, RunProvenance,
};
use myelin_storage::blob::FsBlobStore;
use myelin_storage::ci_cache_scope::{CacheScope, CacheScopeError, CiCacheNamespace};
use myelin_tenancy::TenantId;

fn tenant() -> TenantId {
    TenantId::from_token("acme")
}

#[test]
fn ci_d6_fork_cannot_poison_the_trusted_cache() {
    let base = FsBlobStore::new();
    let cache = CiCacheNamespace::over(tenant(), &base);

    let prov = RunProvenance {
        trust_tier: "untrusted_fork".into(),
        protected_branch: None,
        pr_id: Some("42".into()),
    };
    let (tier, own_scope, run_pr) = derive_cache_scope(&prov).expect("fork derives its own scope");

    let mut outcome = ForkPoisonOutcome {
        fork_to_trusted_attempts: 0,
        fork_to_trusted_landings: 0,
    };

    let poison_targets = [
        CacheScope::Trusted,
        CacheScope::Branch {
            name: "main".into(),
        },
        CacheScope::Fork { pr_id: "99".into() },
        CacheScope::Trusted,
    ];

    for target in &poison_targets {
        outcome.fork_to_trusted_attempts += 1;
        let attempt = cache.put(tier, &run_pr, target, "build-cache", b"poison");
        match attempt {
            Err(CacheScopeError::ForkWriteToTrusted { .. }) => {}
            Ok(_) => outcome.fork_to_trusted_landings += 1,
            other => panic!("unexpected put result: {other:?}"),
        }
    }

    cache
        .put(tier, &run_pr, &own_scope, "build-cache", b"fork-bytes")
        .expect("the fork writes its OWN scope");

    assert_eq!(outcome.fork_to_trusted_attempts, 4);
    assert_eq!(
        outcome.fork_to_trusted_landings, 0,
        "CI-D6 gate: 0 fork→trusted writes"
    );
    assert!(
        outcome.is_green(),
        "CI-D6 must be GREEN (0 trusted-cache poisonings landed)"
    );

    assert_eq!(cache.telemetry().cache_scope_violation(), 4);
    assert!(!cache.contains(&CacheScope::Trusted, "build-cache").unwrap());
    let trusted_read = cache.get(&CacheScope::Trusted, "build-cache");
    assert!(
        matches!(trusted_read, Err(CacheScopeError::Miss { .. })),
        "a trusted read of the fork-written name must MISS (0 cross-scope landings)"
    );
    assert_eq!(
        cache
            .get(&own_scope, "build-cache")
            .expect("fork reads own"),
        b"fork-bytes"
    );

    println!(
        "[CI-D6 GREEN 2026-06-23] fork-cannot-poison-trusted-cache: {} poisoning attempts, \
         {} fork→trusted LANDINGS (gate: 0). cache_scope_violation={} (every attempt observed). \
         The trust-tier/branch-scoped namespace held STRUCTURALLY.",
        outcome.fork_to_trusted_attempts,
        outcome.fork_to_trusted_landings,
        cache.telemetry().cache_scope_violation(),
    );
}
