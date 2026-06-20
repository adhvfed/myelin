//! **REF-P12 / P-161 — the R2 projection cache, PROVEN against the live dev-stack Valkey.**
//!
//! Gated behind the `integration` cargo feature so the default `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free. Run against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-refs-service --features integration \
//!     --test integration_ref_p12_r2_cache -- --nocapture
//!
//! This is the REAL data-layer proof the binding policy requires for REF-P12 — the R2 projection cache
//! driving the REAL [`myelin_storage::ValkeyCache`] (the `Cache` seam, the BSD Valkey fork via `fred`),
//! with each entry sealed under the REAL per-tenant DEK ([`myelin_storage::KmsEngine`]). The drill is
//! registered red-until-proven and flips green ONLY here. We prove against the LIVE Valkey:
//!
//! - **fill → read round-trips through real Valkey** (the projection is SEALED in Valkey, never
//!   plaintext): a `GET` of the raw backing key returns `nonce || ciphertext`, and the cache decrypts
//!   it to the exact 5.6 projection (a HIT).
//! - **an `*.updated`/`*.erased` bust DELETEs the live Valkey key** → the next read MISSES (re-resolves;
//!   never the stale title) — the §3.6 invalidation against the real store.
//! - **crypto-shred**: destroying the per-tenant DEK makes the SURVIVING Valkey blob unrecoverable (the
//!   read MISSES, never plaintext) — the erasure-vs-immutability answer against the real store.
//! - **tenant isolation**: tenant B never reads tenant A's cached projection (`{tenant}:{key}`
//!   namespacing on the real server).
//!
//! `MYELIN_REGION=fr-par` is the dev posture; the cache is residency-pinned by riding the cell-local
//! Valkey (dev<->prod is a config swap, never a code change).
#![cfg(feature = "integration")]

use std::sync::Arc;
use std::time::Duration;

use myelin_events::ArtifactRef;
use myelin_refs_service::{
    ProjectionCache, ProjectionCacheRead, Projection, R2ProjectionCache, RefsDekPin,
};
use myelin_storage::valkey::ValkeyCache;
use myelin_storage::{KmsEngine, NONCE_LEN};
use myelin_tenancy::{Region, TenantId};

fn redis_url() -> String {
    std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6380".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
/// A per-process-unique tenant so parallel runs / reruns never collide on the real server.
fn tenant(tag: &str) -> TenantId {
    TenantId(format!("p161-{tag}-{}", std::process::id()))
}
fn projection(ref_: &str, title: &str) -> Projection {
    Projection {
        ref_: ArtifactRef(ref_.into()),
        title: title.into(),
        state: "open".into(),
        icon: "issue".into(),
        render_hint: "issue-card".into(),
        sub_anchor: None,
        flag: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r2_cache_fill_read_bust_and_crypto_shred_on_real_valkey() {
    let valkey = ValkeyCache::connect(&redis_url(), tokio::runtime::Handle::current())
        .expect("connect dev Valkey (is the stack up? docker compose -f docker-compose.dev.yml up -d --wait)");

    let dek = Arc::new(RefsDekPin::new(Arc::new(KmsEngine::new())));
    // Short TTL on this drill so a leaked key self-evicts quickly; the round-trip is synchronous.
    let cache = R2ProjectionCache::with_ttl(Arc::new(valkey.clone()), dek.clone(), Duration::from_secs(120));

    let t = tenant("rt");
    let ref_ = ArtifactRef("myelin://acme/issue/issue/ENG-1".into());
    let p = projection(&ref_.0, "TOP SECRET acquisition — Alice Liddell");

    // ── fill → read round-trips through REAL Valkey (HIT). ──
    tokio::task::block_in_place(|| cache.fill(&t, &region(), &ref_, &p))
        .expect("fill seals + SETs in real Valkey");
    let hit = tokio::task::block_in_place(|| ProjectionCacheRead::read(&cache, &t, &region(), &ref_));
    assert_eq!(hit, Some(p.clone()), "the live Valkey entry decrypts to the exact projection (HIT)");

    // ── the value SEALED in Valkey is nonce || ciphertext — never the plaintext title. ──
    use myelin_storage::Cache;
    let raw = tokio::task::block_in_place(|| valkey.get(&t, &R2ProjectionCache::cache_key(&ref_)))
        .expect("raw GET")
        .expect("a blob is stored");
    let as_text = String::from_utf8_lossy(&raw);
    assert!(!as_text.contains("Alice Liddell"), "the cached title is sealed in Valkey, never plaintext");
    assert!(raw.len() > NONCE_LEN, "the stored blob is nonce || ciphertext");

    // ── a *.updated/*.erased bust DELETEs the live key → the next read MISSES (re-resolves). ──
    cache.invalidate(&t, &region(), &ref_); // the §3.6 bust against real Valkey
    let after_bust = tokio::task::block_in_place(|| ProjectionCacheRead::read(&cache, &t, &region(), &ref_));
    assert!(after_bust.is_none(), "after the bust the real-Valkey read MISSES → re-resolves (never stale)");

    // ── crypto-shred: re-fill, then destroy the per-tenant DEK → the surviving Valkey blob is dead. ──
    tokio::task::block_in_place(|| cache.fill(&t, &region(), &ref_, &p)).expect("re-fill");
    assert!(
        tokio::task::block_in_place(|| ProjectionCacheRead::read(&cache, &t, &region(), &ref_)).is_some(),
        "decrypts while the DEK lives"
    );
    assert!(dek.destroy_tenant_dek(&t, &region()), "tenant offboard: the per-tenant DEK is shredded");
    let after_shred = tokio::task::block_in_place(|| ProjectionCacheRead::read(&cache, &t, &region(), &ref_));
    assert!(
        after_shred.is_none(),
        "a crypto-shredded cached title is unrecoverable on real Valkey — a MISS, never plaintext"
    );

    // ── tenant isolation on the real server: tenant B never reads tenant A's cached projection. ──
    let a = tenant("iso-a");
    let b = tenant("iso-b");
    let dek2 = Arc::new(RefsDekPin::new(Arc::new(KmsEngine::new())));
    let cache2 = R2ProjectionCache::with_ttl(Arc::new(valkey.clone()), dek2, Duration::from_secs(120));
    tokio::task::block_in_place(|| cache2.fill(&a, &region(), &ref_, &projection(&ref_.0, "a's title")))
        .expect("fill for tenant a");
    let cross = tokio::task::block_in_place(|| ProjectionCacheRead::read(&cache2, &b, &region(), &ref_));
    assert!(cross.is_none(), "tenant B never reads tenant A's cached projection (real-Valkey namespacing)");

    // cleanup our keys (best-effort).
    let _ = tokio::task::block_in_place(|| valkey.delete(&a, &R2ProjectionCache::cache_key(&ref_)));
    let _ = tokio::task::block_in_place(|| valkey.delete(&t, &R2ProjectionCache::cache_key(&ref_)));
}
