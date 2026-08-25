use myelin_events::{OutboxStore, Timestamp};
use myelin_harness::telemetry::{Predicate, SignalName, SignalSource};
use myelin_identity::{
    Consistency, ConsistencyMode, ObjectId, Permission, Principal, PrincipalId, PrincipalKind,
    RelName, RelationTuple, TupleDelta, Zookie,
};
use myelin_identity_service::{Served, StoreBackedCheck, TupleStore};
use myelin_storage::TenantScope;
use myelin_substrate::Thresholds;
use myelin_tenancy::{ArtifactRef, Region, TenantId};

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
        at_least: Zookie("z-strong".into()),
        mode: ConsistencyMode::Strong,
    }
}

fn ts(s: &str) -> Timestamp {
    Timestamp(s.into())
}

#[test]
fn id_d2_fail_static_survives_hiccup_and_denies_revoked() {
    let thresholds = Thresholds::load_canonical().expect("load canonical thresholds");
    let sla_secs: i64 = (thresholds.revocation.sla_mins * 60) as i64;
    let fs_threshold = thresholds.fail_static.clone();

    let acme = scope("acme");
    let store = TupleStore::new(OutboxStore::new());
    store
        .write_tuples(
            &acme,
            &subject("p-admin"),
            &[grant("repo:core", "view", "p:alice")],
            None,
            None,
            ts("2026-06-19T00:00:00Z"),
        )
        .expect("seed alice's grant");
    let svc = StoreBackedCheck::new(store);
    let alice = subject("p:alice");
    let obj = ArtifactRef("repo:core".into());

    let s6 = svc
        .failstatic_cache_with_clock(
            sla_secs as u64,
            &fs_threshold,
            myelin_substrate::TestClock::at(1_000),
        )
        .expect("S6 constructs against the thresholds-file bound");
    assert!(
        (s6.static_max() as i64) <= sla_secs,
        "the fail-static window sits under the revocation SLA (4.11): W={}s ≤ N={}s",
        s6.static_max(),
        sla_secs
    );

    let healthy = svc.check_failstatic(
        &s6,
        &acme,
        &alice,
        &Permission("view".into()),
        &obj,
        &bounded_stale(),
        None,
        &ts("2026-06-19T00:00:01Z"),
        true,
    );
    assert!(
        matches!(healthy.served, Served::Fresh),
        "healthy default-consistency read is fresh"
    );
    assert!(
        healthy.is_allow(),
        "alice's grant is allowed and the coarse answer is cached"
    );

    s6.clock().advance(31);
    let survived = svc.check_failstatic(
        &s6,
        &acme,
        &alice,
        &Permission("view".into()),
        &obj,
        &bounded_stale(),
        None,
        &ts("2026-06-19T00:00:02Z"),
        false,
    );
    assert!(
        matches!(survived.served, Served::Static),
        "during the Id hiccup the default-consistency read survives on the coarse fail-static cache"
    );
    assert!(
        survived.is_allow(),
        "authenticated traffic SURVIVES the hiccup (still Allow)"
    );

    let strong_during_hiccup = svc.check_failstatic(
        &s6,
        &acme,
        &alice,
        &Permission("view".into()),
        &obj,
        &strong(),
        None,
        &ts("2026-06-19T00:00:03Z"),
        false,
    );
    assert!(
        matches!(strong_during_hiccup.served, Served::BypassClosed),
        "a zookie-stamped read BYPASSES S6 (the new-enemy guard)"
    );
    assert!(
        strong_during_hiccup.is_deny(),
        "a strong read fails CLOSED during the hiccup (never served stale)"
    );

    svc.disable_principal_in(
        &acme,
        &PrincipalId("p:alice".into()),
        ts("2026-06-19T00:04:00Z"),
    )
    .expect("record principal disablement");

    let mut successful_authz_after_cache_for_revoked: i64 = 0;
    for i in 0..8 {
        let d = svc.check_failstatic(
            &s6,
            &acme,
            &alice,
            &Permission("view".into()),
            &obj,
            &bounded_stale(),
            None,
            &ts(&format!("2026-06-19T00:05:{i:02}Z")),
            false,
        );
        if d.is_allow() {
            successful_authz_after_cache_for_revoked += 1;
        } else {
            assert!(
                matches!(d.served, Served::Revoked),
                "the revoke is enforced through the cache"
            );
        }
    }

    let mut signals = SignalSource::new();
    signals.set_scalar(
        SignalName::CrossTenantCount,
        successful_authz_after_cache_for_revoked,
    );
    signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

    let fs = s6.fail_static_signals();
    assert!(
        fs.stale >= 1,
        "authenticated traffic survived on the static (degraded) fail-static rung"
    );
    let mut fs_signals = SignalSource::new();
    fs_signals.set_scalar(
        SignalName::FailStaticStalenessSecs,
        fs.last_staleness_secs as i64,
    );
    fs_signals
        .assert_signal(
            SignalName::FailStaticStalenessSecs,
            Predicate::Lte(s6.static_max() as i64),
        )
        .expect_green();
    assert!(
        (fs.last_staleness_secs as i64) <= sla_secs,
        "staleness age ({}s) ≤ the revocation SLA ({sla_secs}s)",
        fs.last_staleness_secs
    );

    assert_eq!(
        myelin_identity_service::CacheTelemetry::CACHE_HIT_RATIO,
        "cache_hit_ratio"
    );
    assert_eq!(
        myelin_identity_service::CacheTelemetry::STALENESS_AGE,
        "staleness_age"
    );
    assert!(
        s6.telemetry().cache_hit_ratio_pct().is_some(),
        "the cache_hit_ratio is observable (cache-consulting reads happened)"
    );

    assert_eq!(
        successful_authz_after_cache_for_revoked, 0,
        "0 successful authz after the cache for a revoked subject during a hiccup (ID-D2 / F7)"
    );

    println!(
        "[P-073 DRILL GREEN 2026-06-19] ID-D2 Id-hiccup / fail-static: tenant=acme subject=p:alice \
         object=repo:core → authenticated traffic SURVIVED on the coarse fail-static cache \
         (fresh={}, stale={}, closed={}, staleness_age={}s ≤ static_max={}s ≤ revocation_SLA={}s); \
         a Strong/zookie read BYPASSED S6 and failed closed (new-enemy guard); a SCIM-disabled \
         subject got successful_authz_after_cache=0 across an 8-read batch (the just-revoked grant \
         is denied THROUGH the stale cache) - thresholds read from the canonical file, never \
         hardcoded; W is [OPEN - LEGAL] (L-1), the static_max ≤ SLA ≥ token-TTL constraint enforced \
         by the S6 constructor regardless",
        fs.fresh, fs.stale, fs.closed, fs.last_staleness_secs, s6.static_max(), sla_secs
    );
}
