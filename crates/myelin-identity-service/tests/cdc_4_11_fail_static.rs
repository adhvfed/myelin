use myelin_client::{CallError, Idempotency, ResilientClient, Target};
use myelin_events::{OutboxStore, Timestamp};
use myelin_identity::{
    Consistency, ConsistencyMode, ObjectId, Permission, Principal, PrincipalId, PrincipalKind,
    RelName, RelationTuple, TupleDelta, Zookie,
};
use myelin_identity_service::{Served, StoreBackedCheck, TupleStore};
use myelin_storage::TenantScope;
use myelin_substrate::{thresholds::FailStaticThreshold, Seconds};
use myelin_tenancy::{ArtifactRef, Region, TenantId};

fn revocation_sla_secs() -> Seconds {
    let t = myelin_substrate::Thresholds::load_canonical().expect("load thresholds");
    (t.revocation.sla_mins * 60) as Seconds
}

fn fail_static_threshold() -> FailStaticThreshold {
    myelin_substrate::Thresholds::load_canonical()
        .expect("load thresholds")
        .fail_static
}

fn scope(tenant: &str) -> TenantScope {
    let p = Principal::stub(
        PrincipalId("p-admin".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    TenantScope::from_verified_token(&p, Region("eu-west".into()))
}

fn subject(id: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    );
    p.region = Region("eu-west".into());
    p
}

fn grant(object: &str, relation: &str, subject: &str) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    })
}

fn bounded_stale() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::BoundedStale,
    }
}

fn strong() -> Consistency {
    Consistency {
        at_least: Zookie("z1".into()),
        mode: ConsistencyMode::Strong,
    }
}

fn ts(s: &str) -> Timestamp {
    Timestamp(s.into())
}

fn provider(s: &TenantScope) -> StoreBackedCheck {
    let store = TupleStore::new(OutboxStore::new());
    store
        .write_tuples(
            s,
            &subject("p-admin"),
            &[grant("repo:core", "view", "p:alice")],
            None,
            None,
            ts("2026-06-19T00:00:00Z"),
        )
        .expect("seed grant");
    StoreBackedCheck::new(store)
}

#[test]
fn cdc_4_11_provider_enforces_the_bound() {
    let svc = provider(&scope("acme"));
    let s6 = svc
        .failstatic_cache(revocation_sla_secs(), &fail_static_threshold())
        .expect("S6 constructs against a valid thresholds-file bound");
    assert!(
        s6.static_max() <= revocation_sla_secs(),
        "the fail-static window sits under the revocation SLA (4.11): static_max={} ≤ SLA={}",
        s6.static_max(),
        revocation_sla_secs()
    );
}

#[test]
fn cdc_4_11_consumer_wraps_check_in_resilient_client_and_survives_a_hiccup() {
    let acme = scope("acme");
    let svc = provider(&acme);
    let s6 = svc
        .failstatic_cache_with_clock(
            revocation_sla_secs(),
            &fail_static_threshold(),
            myelin_substrate::TestClock::at(1_000),
        )
        .expect("S6 constructs");
    let alice = subject("p:alice");
    let obj = ArtifactRef("repo:core".into());

    let client = ResilientClient::default();
    let target = Target("identity.check".into());

    let phase1 = consumer_check(
        &client,
        &target,
        &svc,
        &s6,
        &acme,
        &alice,
        &Permission("view".into()),
        &obj,
        &bounded_stale(),
        &ts("2026-06-19T00:00:01Z"),
         true,
    );
    assert!(
        matches!(phase1.served, Served::Fresh),
        "healthy: served fresh through the client"
    );
    assert!(
        phase1.is_allow(),
        "alice's live grant is allowed (and cached)"
    );

    s6.clock().advance(31);
    let phase2 = consumer_check(
        &client,
        &target,
        &svc,
        &s6,
        &acme,
        &alice,
        &Permission("view".into()),
        &obj,
        &bounded_stale(),
        &ts("2026-06-19T00:00:02Z"),
         false,
    );
    assert!(
        matches!(phase2.served, Served::Static),
        "hiccup: the consumer survives on the S6 bounded-staleness grant (no cascade)"
    );
    assert!(
        phase2.is_allow(),
        "authenticated traffic survives the hiccup (still Allow)"
    );
    assert!(
        phase2.is_degraded(),
        "the survived answer is marked degraded (bounded-staleness)"
    );
    assert!(
        s6.fail_static_signals().last_staleness_secs <= s6.static_max(),
        "the staleness age never exceeds the budget (≤ the revocation SLA)"
    );
}

#[test]
fn cdc_4_11_strong_read_bypasses_cache_and_fails_closed_on_hiccup() {
    let acme = scope("acme");
    let svc = provider(&acme);
    let s6 = svc
        .failstatic_cache_with_clock(
            revocation_sla_secs(),
            &fail_static_threshold(),
            myelin_substrate::TestClock::at(1_000),
        )
        .expect("S6 constructs");
    let alice = subject("p:alice");
    let obj = ArtifactRef("repo:core".into());
    let client = ResilientClient::default();
    let target = Target("identity.check".into());

    let _ = consumer_check(
        &client,
        &target,
        &svc,
        &s6,
        &acme,
        &alice,
        &Permission("view".into()),
        &obj,
        &bounded_stale(),
        &ts("2026-06-19T00:00:01Z"),
        true,
    );
    s6.clock().advance(31);

    let strong_hiccup = consumer_check(
        &client,
        &target,
        &svc,
        &s6,
        &acme,
        &alice,
        &Permission("view".into()),
        &obj,
        &strong(),
        &ts("2026-06-19T00:00:02Z"),
        false,
    );
    assert!(
        matches!(strong_hiccup.served, Served::BypassClosed),
        "strong read bypassed S6"
    );
    assert!(
        strong_hiccup.is_deny(),
        "a strong read fails CLOSED on a hiccup (never serves stale)"
    );
}

#[allow(clippy::too_many_arguments)]
fn consumer_check<C: myelin_substrate::Clock>(
    client: &ResilientClient,
    target: &Target,
    svc: &StoreBackedCheck,
    s6: &myelin_identity_service::FailStaticCache<C>,
    scope: &TenantScope,
    subject: &Principal,
    permission: &Permission,
    object: &ArtifactRef,
    at: &Consistency,
    now: &Timestamp,
    identity_up: bool,
) -> myelin_identity_service::CachedDecision {
    let outcome: Result<(), CallError> = client.call_op(target, Idempotency::Idempotent, || {
        if identity_up {
            Ok(())
        } else {
            Err(CallError::Downstream {
                message: "identity check downstream hiccup".into(),
                retry_after_ms: None,
            })
        }
    });
    let source_ok = outcome.is_ok();
    svc.check_failstatic(
        s6, scope, subject, permission, object, at, None, now, source_ok,
    )
}
