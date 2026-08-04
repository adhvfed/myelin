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
fn ttenant(tag: &str) -> TenantId {
    TenantId(format!("p164-{tag}-{}", std::process::id()))
}
fn gtenant(t: &TenantId) -> GdprTenantId {
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

    let builder = RefsEdgeBuilder::new(EdgeProjection::new());
    let src = "myelin://acme/chat/message/m1";
    let tgt = "myelin://acme/knowledge/page/7c2";
    builder.handle(&edge_event(&t, "e1", "p-erase-me", src, tgt), &mut myelin_events::HandlerTx::none());
    let projection_handle = builder.projection().clone();

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

    let t_other = ttenant("other");
    cache
        .fill(
            &t_other,
            &region(),
            &ArtifactRef(src.into()),
            &projection(src, "Bob Other"),
        )
        .expect("warm the other tenant's title");

    let holder = RefsCacheHolder::with_cache(cache.clone(), projection_handle);
    let receipt = holder
        .erase(EraseScope::Subject {
            subject: subject("p-erase-me", &t),
            tenant: gtenant(&t),
        })
        .expect("holder erase succeeds");
    assert_eq!(receipt.receipt.operation, "erase");

    assert!(
        cache
            .read(&t, &region(), &ArtifactRef(src.into()))
            .is_none(),
        "0 recoverable PII: the subject's cached title is purged from live Valkey"
    );
    assert!(
        cache
            .read(&t_other, &region(), &ArtifactRef(src.into()))
            .is_some(),
        "tenant isolation: another tenant's cached title is untouched by the erase"
    );
}
