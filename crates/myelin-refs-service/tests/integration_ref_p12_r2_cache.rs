#![cfg(feature = "integration")]

use std::sync::Arc;
use std::time::Duration;

use myelin_events::ArtifactRef;
use myelin_refs_service::{
    Projection, ProjectionCache, ProjectionCacheRead, R2ProjectionCache, RefsDekPin,
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
    let cache = R2ProjectionCache::with_ttl(
        Arc::new(valkey.clone()),
        dek.clone(),
        Duration::from_secs(120),
    );

    let t = tenant("rt");
    let ref_ = ArtifactRef("myelin://acme/issue/issue/ENG-1".into());
    let p = projection(&ref_.0, "TOP SECRET acquisition - Alice Liddell");

    tokio::task::block_in_place(|| cache.fill(&t, &region(), &ref_, &p))
        .expect("fill seals + SETs in real Valkey");
    let hit =
        tokio::task::block_in_place(|| ProjectionCacheRead::read(&cache, &t, &region(), &ref_));
    assert_eq!(
        hit,
        Some(p.clone()),
        "the live Valkey entry decrypts to the exact projection (HIT)"
    );

    use myelin_storage::Cache;
    let raw = tokio::task::block_in_place(|| valkey.get(&t, &R2ProjectionCache::cache_key(&ref_)))
        .expect("raw GET")
        .expect("a blob is stored");
    let as_text = String::from_utf8_lossy(&raw);
    assert!(
        !as_text.contains("Alice Liddell"),
        "the cached title is sealed in Valkey, never plaintext"
    );
    assert!(
        raw.len() > NONCE_LEN,
        "the stored blob is nonce || ciphertext"
    );

    cache.invalidate(&t, &region(), &ref_);
    let after_bust =
        tokio::task::block_in_place(|| ProjectionCacheRead::read(&cache, &t, &region(), &ref_));
    assert!(
        after_bust.is_none(),
        "after the bust the real-Valkey read MISSES → re-resolves (never stale)"
    );

    tokio::task::block_in_place(|| cache.fill(&t, &region(), &ref_, &p)).expect("re-fill");
    assert!(
        tokio::task::block_in_place(|| ProjectionCacheRead::read(&cache, &t, &region(), &ref_))
            .is_some(),
        "decrypts while the DEK lives"
    );
    assert!(
        dek.destroy_tenant_dek(&t, &region()),
        "tenant offboard: the per-tenant DEK is shredded"
    );
    let after_shred =
        tokio::task::block_in_place(|| ProjectionCacheRead::read(&cache, &t, &region(), &ref_));
    assert!(
        after_shred.is_none(),
        "a crypto-shredded cached title is unrecoverable on real Valkey - a MISS, never plaintext"
    );

    let a = tenant("iso-a");
    let b = tenant("iso-b");
    let dek2 = Arc::new(RefsDekPin::new(Arc::new(KmsEngine::new())));
    let cache2 =
        R2ProjectionCache::with_ttl(Arc::new(valkey.clone()), dek2, Duration::from_secs(120));
    tokio::task::block_in_place(|| {
        cache2.fill(&a, &region(), &ref_, &projection(&ref_.0, "a's title"))
    })
    .expect("fill for tenant a");
    let cross =
        tokio::task::block_in_place(|| ProjectionCacheRead::read(&cache2, &b, &region(), &ref_));
    assert!(
        cross.is_none(),
        "tenant B never reads tenant A's cached projection (real-Valkey namespacing)"
    );

    let _ = tokio::task::block_in_place(|| valkey.delete(&a, &R2ProjectionCache::cache_key(&ref_)));
    let _ = tokio::task::block_in_place(|| valkey.delete(&t, &R2ProjectionCache::cache_key(&ref_)));
}
