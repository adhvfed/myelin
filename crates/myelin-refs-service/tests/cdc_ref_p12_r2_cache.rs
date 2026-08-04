use std::sync::Arc;

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventHandler,
    EventId, EventType, HandleOutcome, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs_service::{
    bounded_stale, OwnerProjection, ProjectApi, ProjectApiError, ProjectOutcome, Projection,
    ProjectionCacheRead, R2ProjectionCache, RefsDekPin, RefsProjectionInvalidator, Resolution,
    ResolveMode, ResolveService, TombstoneReason,
};
use myelin_storage::{InMemoryCache, KmsEngine};
use myelin_substrate::{FailStaticAuthz, FailStaticThreshold};
use myelin_tenancy::{CellId, Region, TenantId};
use std::sync::Mutex;

use myelin_identity::{Decision, Permission};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn cell() -> CellId {
    CellId::from_token("cell-fr-par-1")
}
fn viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}

fn threshold() -> FailStaticThreshold {
    FailStaticThreshold {
        status: "OPEN - LEGAL".into(),
        owner: "DPO / Legal".into(),
        static_max_secs: None,
        static_max_default_secs: 300,
        agent_token_ttl_secs: 60,
        constraint: "static_max <= revocation-SLA AND static_max >= agent-token-TTL".into(),
    }
}
fn authz() -> Arc<FailStaticAuthz> {
    Arc::new(FailStaticAuthz::try_new(300, &threshold()).expect("valid bound"))
}

fn live_cache() -> R2ProjectionCache {
    let dek = Arc::new(RefsDekPin::new(Arc::new(KmsEngine::new())));
    R2ProjectionCache::new(Arc::new(InMemoryCache::new()), dek)
}

#[derive(Default)]
struct SyntheticOwner {
    allowed: Mutex<Vec<String>>,
    outcome: Mutex<Option<ProjectOutcome>>,
    project_calls: Mutex<u64>,
}
impl SyntheticOwner {
    fn allow(&self, id: &str) {
        self.allowed.lock().unwrap().push(id.into());
    }
    fn set_outcome(&self, o: ProjectOutcome) {
        *self.outcome.lock().unwrap() = Some(o);
    }
    fn project_call_count(&self) -> u64 {
        *self.project_calls.lock().unwrap()
    }
    fn live(title: &str) -> ProjectOutcome {
        ProjectOutcome::Live(OwnerProjection {
            title: title.into(),
            state: "open".into(),
            icon: "issue".into(),
            render_hint: "issue-card".into(),
            sub_anchor: None,
            flag: None,
        })
    }
}
impl ProjectApi for SyntheticOwner {
    fn check_view(
        &self,
        _t: &TenantId,
        _r: &Region,
        _o: &ArtifactRef,
        viewer: &Principal,
        _p: &Permission,
    ) -> Result<Decision, ProjectApiError> {
        if self
            .allowed
            .lock()
            .unwrap()
            .iter()
            .any(|a| a == &viewer.principal_id.0)
        {
            Ok(Decision::Allow)
        } else {
            Ok(Decision::Deny)
        }
    }
    fn project(
        &self,
        _t: &TenantId,
        _r: &Region,
        _ref: &ArtifactRef,
        _v: &Principal,
        _m: ResolveMode,
    ) -> Result<ProjectOutcome, ProjectApiError> {
        *self.project_calls.lock().unwrap() += 1;
        Ok(self
            .outcome
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| SyntheticOwner::live("default title")))
    }
}

fn lifecycle_event(id: &str, type_: &str, subject: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType(type_.into()),
        schema_ver: 1,
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId("p-opaque-1".into()),
            PrincipalKind::Human,
            tenant(),
        )),
        subject: ArtifactRef(subject.into()),
        aggregate: AggregateKey(format!("agg:{subject}")),
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
        payload: serde_json::json!({}),
    }
}

#[test]
fn chained_hit_then_updated_then_miss_then_re_resolve_through_the_chokepoint() {
    let cache = live_cache();
    let owner = Arc::new(SyntheticOwner::default());
    owner.allow("insider");
    owner.set_outcome(SyntheticOwner::live("v1 title"));

    let svc = ResolveService::new(authz(), Arc::new(cache.clone()), owner.clone(), cell());
    let ref_ = ArtifactRef("myelin://acme/issue/issue/ENG-1".into());
    let root = ref_.clone();

    let r1 = svc.resolve(
        &tenant(),
        &region(),
        &ref_,
        &root,
        &viewer("insider"),
        ResolveMode::Live,
        &bounded_stale(),
        false,
    );
    assert!(r1.is_projection(), "first resolve projects");
    assert_eq!(
        owner.project_call_count(),
        1,
        "the first (miss) resolve reached project"
    );
    let (_h, _m, fills) = cache.counters();
    assert_eq!(fills, 1, "the chokepoint filled the cache after the miss");

    let r2 = svc.resolve(
        &tenant(),
        &region(),
        &ref_,
        &root,
        &viewer("insider"),
        ResolveMode::Live,
        &bounded_stale(),
        false,
    );
    assert!(r2.is_projection(), "second resolve is a HIT");
    if let Resolution::Projection(p) = &r2 {
        assert_eq!(p.title, "v1 title", "the cache served the v1 projection");
    }
    assert_eq!(
        owner.project_call_count(),
        1,
        "the HIT short-circuited the owner (no second project)"
    );

    owner.set_outcome(SyntheticOwner::live("v2 title (fresh)"));
    let inv = RefsProjectionInvalidator::with_cache(Arc::new(cache.clone()));
    assert_eq!(
        inv.handle(&lifecycle_event("01J-u", "issue.issue.updated", &ref_.0), &mut myelin_events::HandlerTx::none()),
        HandleOutcome::Done,
        "the *.updated busts the live cache entry"
    );

    let r3 = svc.resolve(
        &tenant(),
        &region(),
        &ref_,
        &root,
        &viewer("insider"),
        ResolveMode::Live,
        &bounded_stale(),
        false,
    );
    assert!(r3.is_projection(), "the post-bust resolve re-resolves");
    if let Resolution::Projection(p) = &r3 {
        assert_eq!(
            p.title, "v2 title (fresh)",
            "after the bust the cache re-resolves the FRESH title, never the stale v1"
        );
    }
    assert_eq!(
        owner.project_call_count(),
        2,
        "the post-bust miss reached project again (re-resolve)"
    );
}

#[test]
fn on_erasure_the_cache_re_resolves_never_serving_stale() {
    let cache = live_cache();
    let owner = Arc::new(SyntheticOwner::default());
    owner.allow("insider");
    owner.set_outcome(SyntheticOwner::live("SECRET soon-erased"));

    let svc = ResolveService::new(authz(), Arc::new(cache.clone()), owner.clone(), cell());
    let ref_ = ArtifactRef("myelin://acme/issue/issue/ENG-secret".into());
    let root = ref_.clone();

    let _ = svc.resolve(
        &tenant(),
        &region(),
        &ref_,
        &root,
        &viewer("insider"),
        ResolveMode::Live,
        &bounded_stale(),
        false,
    );
    let warm = svc.resolve(
        &tenant(),
        &region(),
        &ref_,
        &root,
        &viewer("insider"),
        ResolveMode::Live,
        &bounded_stale(),
        false,
    );
    assert!(warm.is_projection(), "warm HIT before erasure");

    let inv = RefsProjectionInvalidator::with_cache(Arc::new(cache.clone()));
    assert_eq!(
        inv.handle(&lifecycle_event("01J-e", "issue.issue.erased", &ref_.0), &mut myelin_events::HandlerTx::none()),
        HandleOutcome::Done,
        "the *.erased busts the cached title"
    );
    owner.set_outcome(ProjectOutcome::Erased);

    let after = svc.resolve(
        &tenant(),
        &region(),
        &ref_,
        &root,
        &viewer("insider"),
        ResolveMode::Live,
        &bounded_stale(),
        false,
    );
    assert!(
        after.is_tombstone(),
        "on erasure the cache re-resolves to a tombstone, never the stale title"
    );
    assert_eq!(after.tombstone_reason(), Some(TombstoneReason::Erased));
}

#[test]
fn cdc_r2_holder_is_dek_sealed_and_crypto_shred_able() {
    let dek = Arc::new(RefsDekPin::new(Arc::new(KmsEngine::new())));
    let cache = R2ProjectionCache::new(Arc::new(InMemoryCache::new()), dek.clone());
    let ref_ = ArtifactRef("myelin://acme/issue/issue/ENG-1".into());

    let projection = Projection {
        ref_: ref_.clone(),
        title: "a name in a title".into(),
        state: "open".into(),
        icon: "issue".into(),
        render_hint: "issue-card".into(),
        sub_anchor: None,
        flag: None,
    };
    cache
        .fill(&tenant(), &region(), &ref_, &projection)
        .expect("fill seals under the per-tenant DEK");
    assert!(
        ProjectionCacheRead::read(&cache, &tenant(), &region(), &ref_).is_some(),
        "the title decrypts while the DEK lives"
    );

    assert!(
        dek.destroy_tenant_dek(&tenant(), &region()),
        "the per-tenant DEK is shredded"
    );
    assert!(
        ProjectionCacheRead::read(&cache, &tenant(), &region(), &ref_).is_none(),
        "a crypto-shredded cached title is unrecoverable - a MISS, never plaintext (10.1 cache half)"
    );
}
