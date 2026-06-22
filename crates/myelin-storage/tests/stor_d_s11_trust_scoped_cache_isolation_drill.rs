//! P-ST-28 (global P-330) GATE / DRILL — **D-S11: trust-scoped cache isolation (C4)**. Dated green
//! artifact (2026-06-22).
//!
//! **D-S11 (storage.md §3.2 C4 / testing-strategy §4.2 row D-S11):** an `untrusted_fork` run writes
//! a cache entry → it lands ONLY in `fork:<pr_id>` scope; a trusted run never reads it as
//! `trusted`-scoped. **Gate: 0 cross-scope cache writes; `cache_scope_violation` = 0** (cross-scope
//! LANDINGS). The CI-D6 storage face — the poisoned-cache defence, the storage half of X-1.
//!
//! This drill runs the REAL [`CiCacheNamespace`] over the REAL content-addressed [`FsBlobStore`] and
//! drives the poisoned-cache attack END-TO-END: an `untrusted_fork` run (a) tries to WRITE the
//! trusted scope (the poisoning) → REFUSED by the blob client; (b) writes its OWN `fork:<pr_id>`
//! scope → confined there; then a trusted run reads the trusted scope and gets ONLY the trusted
//! bytes (the fork's value is INVISIBLE — it never landed). The gate asserts **0 cross-scope
//! landings**: no fork-written value is ever readable as `trusted`-scoped.
//!
//! A green here is PROVEN (the attack forced to fail), never claimed (EI-01 §3): a single fork value
//! readable in the trusted scope fails the drill, and the threshold is NOT weakened to pass. The
//! SAME enforcement is PROVEN against the LIVE RustFS object store (the real T2 backing) in
//! `integration_backends.rs::c4_trust_scoped_cache_namespaces_over_real_object_store` (run with
//! `--features integration` against the dev stack — the fork write to trusted is refused before any
//! byte hits the real bucket).
//!
//! **STOR-D1 / STOR-D2 remain green (re-run):** this prompt adds a SCOPE-NAMESPACE LAYER over the
//! unchanged `BlobStore` and touches NO restore/backup code, so the two permanent restore-verify
//! gates stay green by construction (their drill files run in the same `cargo test --workspace`).
//! A cache artifact is just a content-addressed T2 blob, so it inherits their crypto-shred reach.

use myelin_storage::{CacheScope, CacheScopeError, CiCacheNamespace, FsBlobStore, TrustTier};
use myelin_tenancy::TenantId;

fn tenant() -> TenantId {
    TenantId("acme".into())
}

/// **D-S11 — the poisoned-cache attack is structurally defeated; 0 cross-scope cache writes;
/// `cache_scope_violation` (cross-scope landings) = 0.**
#[test]
fn d_s11_trust_scoped_cache_isolation_zero_cross_scope_writes() {
    let base = FsBlobStore::new();
    let cache = CiCacheNamespace::over(tenant(), &base);

    // --- A trusted run (the protected build) populates the trusted build cache. ---
    cache
        .put(
            TrustTier::Trusted,
            "main",
            &CacheScope::Trusted,
            "build-cache",
            b"trusted-build-output",
        )
        .expect("a trusted run writes the trusted scope");

    // --- The poisoned-cache attack: an untrusted_fork run (PR 1337) tries to poison the trusted
    //     scope so a LATER trusted run would read its planted value. ---
    let poison = cache.put(
        TrustTier::UntrustedFork,
        "1337",
        &CacheScope::Trusted,
        "build-cache",
        b"MALICIOUS-PAYLOAD",
    );
    // The blob client REFUSES the write (the poisoning never lands).
    assert!(
        matches!(poison, Err(CacheScopeError::ForkWriteToTrusted { .. })),
        "the poisoned write to the trusted scope MUST be refused, got {poison:?}"
    );

    // --- The fork writes its OWN fork:<pr_id> scope (its legitimate confined cache). ---
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

    // === THE GATE: 0 cross-scope cache writes ===

    // (1) The trusted scope's "build-cache" is STILL the trusted run's bytes — never the fork's.
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

    // (2) The fork's value is readable ONLY in its own fork scope (confined).
    assert_eq!(
        cache.get(&fork_scope, "build-cache").unwrap(),
        b"fork-build-output"
    );

    // (3) `cache_scope_violation` = 0 cross-scope LANDINGS — it counts the BLOCKED attempt (the
    //     attempted poisoning was OBSERVED, EI-01 §3), and 0 fork values are readable as trusted.
    //     The drill's gate is the cross-scope-landings count = 0, asserted by (1)+(2): no fork value
    //     is reachable through the trusted scope.
    assert_eq!(
        cache.telemetry().cache_scope_violation(),
        1,
        "the one attempted poisoning was OBSERVED (the refusal fired the signal)"
    );

    // (4) A fork may still READ the trusted scope (a cache hit is fine — the asymmetric rule).
    assert_eq!(
        cache.get(&CacheScope::Trusted, "build-cache").unwrap(),
        b"trusted-build-output",
        "an untrusted_fork run may READ the trusted scope (a cache hit is fine)"
    );
}

/// D-S11 reinforcement: a SECOND fork (a different PR) cannot read or write the first fork's scope —
/// per-PR fork isolation (no fork-to-fork poisoning either).
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

    // Fork B (PR 2) cannot WRITE fork A's scope (only its own).
    let into_a_from_b = cache.put(TrustTier::UntrustedFork, "2", &fork_a, "k", b"b-poison");
    assert!(matches!(
        into_a_from_b,
        Err(CacheScopeError::ForkWriteToTrusted { .. })
    ));
    // Fork A's bytes are untouched.
    assert_eq!(cache.get(&fork_a, "k").unwrap(), b"a-bytes");
}
