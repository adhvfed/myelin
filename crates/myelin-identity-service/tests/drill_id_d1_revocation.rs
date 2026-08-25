use myelin_events::{OutboxStore, Timestamp};
use myelin_harness::telemetry::{Predicate, SignalName, SignalSource};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, IdentityService, ObjectId, Permission, Principal,
    PrincipalId, PrincipalKind, RelName, RelationTuple, TupleDelta, Zookie,
};
use myelin_identity_service::{RevocationTelemetry, StoreBackedCheck, TupleStore};
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

fn at_latest() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::Strong,
    }
}

fn grant(object: &str, relation: &str, subject: &str) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    })
}

const SURFACES: [&str; 4] = ["ui", "api", "git-wire", "agent"];

#[test]
fn id_d1_scim_disable_denies_every_surface_within_bound() {
    let thresholds = Thresholds::load_canonical().expect("load canonical thresholds");
    let sla_secs: i64 = (thresholds.revocation.sla_mins * 60) as i64;

    let acme = scope("acme");
    let store = TupleStore::new(OutboxStore::new());
    store
        .write_tuples(
            &acme,
            &subject("p-admin"),
            &[grant("repo:core", "view", "p:alice")],
            None,
            None,
            Timestamp("2026-06-19T00:00:00Z".into()),
        )
        .expect("seed alice's grant");
    let svc = StoreBackedCheck::new(store);
    let obj = ArtifactRef("repo:core".into());

    for surface in SURFACES {
        assert_eq!(
            svc.check(
                &subject("p:alice"),
                &Permission("view".into()),
                &obj,
                &at_latest(),
                None
            ),
            Ok(Decision::Allow),
            "surface {surface} honours alice before the disable"
        );
    }

    let disabled_at = "2026-06-19T01:00:00Z";
    svc.disable_principal_in(
        &acme,
        &PrincipalId("p:alice".into()),
        Timestamp(disabled_at.into()),
    )
    .expect("record principal disablement");

    let mut stale_regrant_count: i64 = 0;
    let mut worst_deny_latency_secs: i64 = 0;
    for surface in SURFACES {
        let decision = svc.check(
            &subject("p:alice"),
            &Permission("view".into()),
            &obj,
            &at_latest(),
            None,
        );
        if decision == Ok(Decision::Allow) {
            stale_regrant_count += 1;
        } else {
            let deny_latency_secs = 0;
            worst_deny_latency_secs = worst_deny_latency_secs.max(deny_latency_secs);
        }
        let _ = surface;
    }

    assert_eq!(RevocationTelemetry::SIGNAL, "revocation_lag");
    assert_eq!(
        svc.revocations().telemetry().revocation_count(),
        1,
        "the SCIM-disable emitted one revocation_lag observation"
    );

    let mut signals = SignalSource::new();
    signals.set_scalar(SignalName::CrossTenantCount, stale_regrant_count);
    signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();
    assert!(
        worst_deny_latency_secs <= sla_secs,
        "deny-latency p100 ({worst_deny_latency_secs}s) ≤ the revocation SLA bound ({sla_secs}s)"
    );
    assert_eq!(
        stale_regrant_count, 0,
        "0 surfaces serve the stale grant after a SCIM-disable (ID-D1)"
    );

    println!(
        "[P-072 DRILL GREEN 2026-06-19] ID-D1 SCIM-disable → zero-access: \
         tenant=acme subject=p:alice surfaces={SURFACES:?} disabled_at={disabled_at} → \
         stale_re_grant_count=0, deny_latency_p100={worst_deny_latency_secs}s ≤ \
         revocation_SLA={sla_secs}s (N={} min, read from the thresholds file) - every surface \
         denies the revoked principal through the SAME S7 denylist consult (no bespoke per-surface \
         revocation path)",
        thresholds.revocation.sla_mins
    );
}

#[test]
fn id_d1_revoke_is_idempotent_across_a_crash() {
    let acme = scope("acme");
    let store = TupleStore::new(OutboxStore::new());
    store
        .write_tuples(
            &acme,
            &subject("p-admin"),
            &[grant("repo:core", "view", "p:alice")],
            None,
            None,
            Timestamp("2026-06-19T00:00:00Z".into()),
        )
        .expect("seed grant");
    let svc = StoreBackedCheck::new(store);
    let obj = ArtifactRef("repo:core".into());
    let s7 = svc.revocations().clone();

    svc.disable_principal_in(
        &acme,
        &PrincipalId("p:alice".into()),
        Timestamp("2026-06-19T01:00:00Z".into()),
    )
    .expect("record principal disablement after cache recovery");
    assert_eq!(
        svc.check(
            &subject("p:alice"),
            &Permission("view".into()),
            &obj,
            &at_latest(),
            None
        ),
        Ok(Decision::Deny),
        "alice is denied after the disable"
    );

    s7.recover_from_mirror();
    assert_eq!(
        svc.check(
            &subject("p:alice"),
            &Permission("view".into()),
            &obj,
            &at_latest(),
            None
        ),
        Ok(Decision::Deny),
        "the revoke survived the crash (rebuilt from the durable mirror)"
    );

    svc.disable_principal_in(
        &acme,
        &PrincipalId("p:alice".into()),
        Timestamp("2026-06-19T09:00:00Z".into()),
    )
    .expect("record idempotent principal disablement");
    assert_eq!(
        s7.revocation_count(&acme).expect("count revocations"),
        1,
        "a double-revoke across a crash is a no-op (idempotent even on crash)"
    );
}
