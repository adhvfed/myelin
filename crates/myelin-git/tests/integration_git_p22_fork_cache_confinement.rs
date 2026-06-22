//! # GIT-P22 / P-284 — the `fork:<pr_id>` cache confinement, PROVEN against the live dev-stack Valkey.
//!
//! **Contract:** `contract-index.md` row 11.2 C4 (the **trust-tier/branch-scoped cache namespaces** — an
//! `UntrustedFork` write cannot reach the trusted cache scope) + row 5.9 (the fork-endorsement seam).
//! Owning architecture: `git-hosting/architecture/02-internals-and-algorithms.md` §6.3 (the storage-tier
//! half of the poisoned-pipeline defence — fork cache confined to `fork:<pr_id>`). **Reconciliation:**
//! §8 (the scope-key convention over the per-tenant cache — the poisoned-cache defence).
//!
//! This is the REAL data-layer proof the binding policy requires for the 11.2 C4 contract leg: the
//! [`myelin_git::fork_gate::ScopedCache`] confinement driving the REAL [`myelin_storage::ValkeyCache`]
//! (the `Cache` seam — the BSD Valkey fork via `fred`), NOT the in-memory floor. The drill is registered
//! red-until-proven and flips green ONLY here, against the LIVE Valkey:
//!
//! - **0 fork writes in the trusted scope** — a fork run (`fork:<pr_id>`) writes a key on the real
//!   server; a later TRUSTED run reading the same logical key MISSES (the fork cannot poison the trusted
//!   scope).
//! - **per-PR fork isolation** — PR 42's fork scope is invisible to PR 99's fork scope on the real
//!   server.
//! - **the trusted scope round-trips** — the confinement does not break the legitimate build-cache reuse
//!   across trusted runs.
//!
//! `MYELIN_REGION=fr-par` is the dev posture; the cache rides the cell-local Valkey (dev<->prod is a
//! config swap, never a code change). Run against the dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-git --features integration \
//!     --test integration_git_p22_fork_cache_confinement -- --nocapture
#![cfg(feature = "integration")]

use myelin_git::check_status::TrustTier;
use myelin_git::fork_gate::{ScopedCache, TrustScope};
use myelin_storage::valkey::ValkeyCache;
use myelin_tenancy::TenantId;
use std::time::Duration;

fn redis_url() -> String {
    std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6380".into())
}

/// A per-process-unique tenant so parallel runs / reruns never collide on the real server.
fn tenant(tag: &str) -> TenantId {
    TenantId(format!("p284-{tag}-{}", std::process::id()))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fork_cache_confinement_holds_on_real_valkey() {
    let valkey = ValkeyCache::connect(&redis_url(), tokio::runtime::Handle::current())
        .expect("connect dev Valkey (is the stack up? docker compose -f docker-compose.dev.yml up -d --wait)");
    let t = tenant("confine");
    let ttl = Duration::from_secs(120);

    // ── 0 fork writes in the trusted scope (the poisoned-cache defence on the REAL server) ──
    // A fork run derives fork:<pr_id> from its CI-stamped trust tier — NEVER trusted.
    let fork_scope = TrustScope::for_run(TrustTier::UntrustedFork, "42");
    assert!(
        !fork_scope.is_trusted(),
        "a fork run is structurally never the trusted scope"
    );
    let fork = ScopedCache::new(&valkey, fork_scope);
    fork.set(&t, "dep-graph", b"attacker-controlled", ttl)
        .expect("fork write to the real Valkey");

    // A trusted run reading the SAME logical key MISSES on the real server (the fork is confined).
    let trusted = ScopedCache::new(&valkey, TrustScope::Trusted);
    assert_eq!(
        trusted.get(&t, "dep-graph").expect("real-Valkey get"),
        None,
        "0 fork writes in the trusted scope (a fork cannot poison a trusted run on real Valkey)"
    );
    // The fork run itself reads back its own write (confinement isolates, it does not lose data).
    assert_eq!(
        fork.get(&t, "dep-graph").expect("real-Valkey get"),
        Some(b"attacker-controlled".to_vec()),
        "the fork run reads back its own fork-scoped write"
    );

    // ── per-PR fork isolation on the real server ──
    let f99 = ScopedCache::new(&valkey, TrustScope::for_run(TrustTier::UntrustedFork, "99"));
    assert_eq!(
        f99.get(&t, "dep-graph").expect("real-Valkey get"),
        None,
        "PR 99's fork scope cannot read PR 42's fork-scoped key"
    );

    // ── the legitimate trusted-scope path still round-trips across trusted runs ──
    let t1 = ScopedCache::new(&valkey, TrustScope::Trusted);
    t1.set(&t, "build-cache", b"shared-trusted", ttl)
        .expect("trusted write to the real Valkey");
    let t2 = ScopedCache::new(&valkey, TrustScope::Trusted);
    assert_eq!(
        t2.get(&t, "build-cache").expect("real-Valkey get"),
        Some(b"shared-trusted".to_vec()),
        "two trusted runs share the trusted scope (build-cache reuse is preserved)"
    );

    // Clean up the per-process keys (best-effort; the TTL self-evicts regardless).
    let _ = fork.delete(&t, "dep-graph");
    let _ = t1.delete(&t, "build-cache");
}
