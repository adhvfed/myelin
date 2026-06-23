//! # CI-D6 drill — fork-cannot-poison-trusted-cache (CI-P22 / P-365).
//!
//! The whole-system drill (07 D-6 / drill catalogue row CI-D6): an adversarial `UntrustedFork` run
//! attempts to write the default-branch (trusted) cache scope; the **trust-tier/branch-scoped
//! namespace holds STRUCTURALLY** → **0 trusted-cache writes from a fork-tier run**. The quantified
//! gate: `0 fork→trusted writes`.
//!
//! This is the CI-side failure-injection scenario over the two halves CI-P22 composes:
//! - CI [`derive_cache_scope`] — a fork run derives ONLY its `fork:<pr_id>` scope (it cannot even
//!   NAME the trusted scope from its provenance);
//! - storage `CiCacheNamespace::put` — even when the adversary FORCES the trusted scope into the put,
//!   the storage write-scope rule REFUSES it (the structural defence).
//!
//! Emits a dated green artifact line on pass (the prompt's "CI-D6 (0 fork→trusted writes) emits its
//! dated green artifact").

use myelin_ci_controlplane::artifact_cache::{
    derive_cache_scope, ForkPoisonOutcome, RunProvenance,
};
use myelin_storage::blob::FsBlobStore;
use myelin_storage::ci_cache_scope::{CacheScope, CacheScopeError, CiCacheNamespace};
use myelin_tenancy::TenantId;

fn tenant() -> TenantId {
    TenantId::from_token("acme")
}

/// **CI-D6: 0 fork→trusted writes.** An adversarial fork run makes MULTIPLE poisoning attempts
/// (the trusted scope, a protected `branch:` scope, ANOTHER fork's scope, a re-attempt) — every one
/// is REFUSED; nothing lands in the trusted scope; a later TRUSTED read of the same name is a clean
/// MISS (the fork's bytes never reached the trusted cache).
#[test]
fn ci_d6_fork_cannot_poison_the_trusted_cache() {
    let base = FsBlobStore::new();
    let cache = CiCacheNamespace::over(tenant(), &base);

    // The adversary's fork run — CI derives ONLY its confined fork:42 scope.
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

    // The adversary's poisoning targets — every scope it is NOT allowed to write.
    let poison_targets = [
        CacheScope::Trusted,
        CacheScope::Branch {
            name: "main".into(),
        },
        CacheScope::Fork { pr_id: "99".into() }, // another fork's scope
        CacheScope::Trusted,                     // a re-attempt
    ];

    for target in &poison_targets {
        outcome.fork_to_trusted_attempts += 1;
        let attempt = cache.put(tier, &run_pr, target, "build-cache", b"poison");
        match attempt {
            // REFUSED by the storage write-scope rule (the structural defence).
            Err(CacheScopeError::ForkWriteToTrusted { .. }) => {}
            // A landing is the breach — count it (the gate then fails).
            Ok(_) => outcome.fork_to_trusted_landings += 1,
            other => panic!("unexpected put result: {other:?}"),
        }
    }

    // The fork's LEGITIMATE write — its OWN scope still works (the defence is asymmetric, not a wall).
    cache
        .put(tier, &run_pr, &own_scope, "build-cache", b"fork-bytes")
        .expect("the fork writes its OWN scope");

    // ---- the quantified gate: 0 fork→trusted writes -----------------------------------------------
    assert_eq!(outcome.fork_to_trusted_attempts, 4);
    assert_eq!(
        outcome.fork_to_trusted_landings, 0,
        "CI-D6 gate: 0 fork→trusted writes"
    );
    assert!(
        outcome.is_green(),
        "CI-D6 must be GREEN (0 trusted-cache poisonings landed)"
    );

    // Every blocked attempt was OBSERVED (not silently dropped — EI-01 §3).
    assert_eq!(cache.telemetry().cache_scope_violation(), 4);
    // Nothing landed in the trusted scope; a trusted read of the poisoned name is a clean MISS.
    assert!(!cache.contains(&CacheScope::Trusted, "build-cache"));
    let trusted_read = cache.get(&CacheScope::Trusted, "build-cache");
    assert!(
        matches!(trusted_read, Err(CacheScopeError::Miss { .. })),
        "a trusted read of the fork-written name must MISS (0 cross-scope landings)"
    );
    // The fork's own scope round-trips (its legitimate cache is intact).
    assert_eq!(
        cache
            .get(&own_scope, "build-cache")
            .expect("fork reads own"),
        b"fork-bytes"
    );

    // The dated green artifact (the prompt's "CI-D6 emits its dated green artifact").
    println!(
        "[CI-D6 GREEN 2026-06-23] fork-cannot-poison-trusted-cache: {} poisoning attempts, \
         {} fork→trusted LANDINGS (gate: 0). cache_scope_violation={} (every attempt observed). \
         The trust-tier/branch-scoped namespace held STRUCTURALLY.",
        outcome.fork_to_trusted_attempts,
        outcome.fork_to_trusted_landings,
        cache.telemetry().cache_scope_violation(),
    );
}
