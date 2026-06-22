//! **REF-P15 / P-164 — the structural-erasure holder, PROVEN against the live dev-stack Valkey.**
//!
//! Gated behind the `integration` cargo feature so the default `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free. Run against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-refs-service --features integration \
//!     --test integration_ref_p15_holder_erase -- --nocapture
//!
//! This is the REAL data-layer proof the binding policy requires for the REF-P15 cache-PII purge: the
//! holder `erase(subject)` drives the §4.6 purge through the REAL [`myelin_storage::ValkeyCache`] (the
//! SAME `invalidate` eviction path the `*.erased` consumer drives — one path, no backdoor). We prove
//! against the LIVE Valkey (REF-D5 CI variant, real-store half):
//!
//! - warm the subject's cached projection titles (a name) into REAL Valkey (sealed under the per-tenant
//!   DEK);
//! - `RefsCacheHolder::erase(Subject)` → the live Valkey keys for the refs the subject touches are
//!   DELETED → a subsequent read MISSES (0 recoverable PII, never the stale name);
//! - tenant isolation: another tenant's cached title is untouched by the erase.
//!
//! `MYELIN_REGION=fr-par` is the dev posture; the cache is residency-pinned by riding the cell-local
//! Valkey (dev<->prod is a config swap, never a code change). The full backup-level 0-recoverable shred
//! (REF-D5 at scale) is REF-P25 (named floor).
#![cfg(feature = "integration")]

use std::sync::Arc;
use std::time::Duration;

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventHandler,
    EventId, EventType, Timestamp, Visibility,
};
use myelin_gdpr::{EraseScope, PersonalDataHolder, SubjectRef, TenantId as GdprTenantId};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs_service::{
    EdgeProjection, Projection, ProjectionCacheRead, R2ProjectionCache, RefsCacheHolder,
    RefsDekPin, RefsEdgeBuilder,
};
use myelin_storage::valkey::ValkeyCache;
use myelin_storage::KmsEngine;
use myelin_tenancy::{Region, TenantId};

fn redis_url() -> String {
    std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6380".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
/// A per-process-unique tenant so parallel runs / reruns never collide on the real server.
fn ttenant(tag: &str) -> TenantId {
    TenantId(format!("p164-{tag}-{}", std::process::id()))
}
fn gtenant(t: &TenantId) -> GdprTenantId {
    // gdpr's TenantId IS myelin_tenancy::TenantId (a type alias) — pass it straight through.
    t.clone()
}
fn subject(id: &str, t: &TenantId) -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        t.clone(),
    ))
}
fn principal(id: &str, t: &TenantId) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, t.clone())
}
fn projection(ref_: &str, title: &str) -> Projection {
    Projection {
        ref_: ArtifactRef(ref_.into()),
        title: title.into(),
        state: "open".into(),
        icon: "doc".into(),
        render_hint: "card".into(),
        sub_anchor: None,
        flag: None,
    }
}

fn edge_event(t: &TenantId, id: &str, actor: &str, source: &str, target: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType("refs.edge.created".into()),
        schema_ver: 1,
        tenant: t.clone(),
        region: region(),
        actor: Actor(principal(actor, t)),
        subject: ArtifactRef(source.into()),
        aggregate: AggregateKey(format!("edge:{source}->{target}")),
        causation_id: None,
        correlation_id: CorrelationId(id.into()),
        caused_by: None,
        depth: 1,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
        payload: serde_json::json!({ "source": source, "target": target, "rel": "mentions" }),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn holder_erase_purges_cache_pii_on_real_valkey_zero_recoverable() {
    let valkey = ValkeyCache::connect(&redis_url(), tokio::runtime::Handle::current())
        .expect("connect dev Valkey (is the stack up? docker compose -f docker-compose.dev.yml up -d --wait)");
    let dek = Arc::new(RefsDekPin::new(Arc::new(KmsEngine::new())));
    let cache = Arc::new(R2ProjectionCache::with_ttl(
        Arc::new(valkey.clone()),
        dek.clone(),
        Duration::from_secs(120),
    ));

    let t = ttenant("erase");

    // the subject p-erase-me authored an edge whose SOURCE cached title holds their name.
    let builder = RefsEdgeBuilder::new(EdgeProjection::new());
    let src = "myelin://acme/chat/message/m1";
    let tgt = "myelin://acme/knowledge/page/7c2";
    builder.handle(&edge_event(&t, "e1", "p-erase-me", src, tgt));
    let projection_handle = builder.projection().clone();

    // warm the subject's cached title (a name) into REAL Valkey, sealed under the per-tenant DEK.
    cache
        .fill(
            &t,
            &region(),
            &ArtifactRef(src.into()),
            &projection(src, "Alice Smith"),
        )
        .expect("warm the cached title in Valkey");
    assert!(
        cache
            .read(&t, &region(), &ArtifactRef(src.into()))
            .is_some(),
        "the cached title (a name) is present in live Valkey before erase"
    );

    // a DIFFERENT tenant's cached title — must be untouched by the erase (tenant isolation).
    let t_other = ttenant("other");
    cache
        .fill(
            &t_other,
            &region(),
            &ArtifactRef(src.into()),
            &projection(src, "Bob Other"),
        )
        .expect("warm the other tenant's title");

    // REF-P15: the holder erase drives the §4.6 purge through the live Valkey (the ONE eviction path).
    let holder = RefsCacheHolder::with_cache(cache.clone(), projection_handle);
    let receipt = holder
        .erase(EraseScope::Subject {
            subject: subject("p-erase-me", &t),
            tenant: gtenant(&t),
        })
        .expect("holder erase succeeds");
    assert_eq!(receipt.receipt.operation, "erase");

    // 0 recoverable PII: the subject's cached title is GONE from live Valkey (a read MISSES → re-resolve,
    // never the stale name).
    assert!(
        cache
            .read(&t, &region(), &ArtifactRef(src.into()))
            .is_none(),
        "0 recoverable PII: the subject's cached title is purged from live Valkey"
    );
    // tenant isolation: the OTHER tenant's title is untouched.
    assert!(
        cache
            .read(&t_other, &region(), &ArtifactRef(src.into()))
            .is_some(),
        "tenant isolation: another tenant's cached title is untouched by the erase"
    );
}
