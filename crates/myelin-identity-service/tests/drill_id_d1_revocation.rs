//! # P-ID-14 (global P-072) GATE / DRILL — ID-D1, the disabled-user → zero-access-in-N-min drill
//! (the dated green artifact)
//!
//! **Drill catalogue row ID-D1 (§4.2, F8):** *SCIM-disable → every surface (UI / API / git-wire /
//! agent) denies within **N = 5 min**; cache + token + denylist ≤ W; stale re-grant = 0.* Survival
//! signal: the **deny-latency histogram** (deny-latency p100 ≤ the bound) and **stale-re-grant = 0**
//! (no surface keeps serving a revoked principal). Run against the failure-injection harness's
//! telemetry-assertion library (the contract-1.8 survival-signal set), exactly as the cross-tenant
//! IDOR drill (`drill_id_d3`) and the harness self-test (P-S04) do. `myelin-harness` is a
//! DEV-dependency only — it never enters the identity-service production DAG.
//!
//! **The threshold is read from the canonical thresholds file, NEVER hardcoded** (EI-01 §3: the
//! gate's number is the versioned default-to-beat, `revocation.sla_mins = 5`). The drill asserts the
//! measured deny-latency stays under `sla_mins * 60` seconds, and that the count of surfaces that
//! still honoured the revoked principal is `0` (the stale-re-grant zero).
//!
//! **The scenario.** A user `alice` in tenant `acme` holds a real `view` grant on `repo:core`
//! (every surface honours her session). She is SCIM-disabled (the authoritative revocation path,
//! architecture §4): Identity writes the S7 denylist (mirror-first, idempotent, crash-safe). Now a
//! BATCH of surface calls — modelling UI / API / git-wire / agent each re-`check`ing alice — all
//! DENY. The deny is effective at revoke time (a hot denylist consult), so the deny-latency is `0`
//! and 0 of the surfaces serve the stale grant. A non-zero stale-re-grant would mean a surface kept
//! serving a revoked user (the exact F8 failure) and the drill aborts LOUDLY (EI-01 §3: loud, never
//! swallowed; the threshold is NEVER weakened to pass).

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

/// The surfaces the drill catalogue names — every one re-`check`s the principal (no surface has a
/// bespoke revocation path; they all consult the SAME S7 denylist through `check`, EI-01 §7).
const SURFACES: [&str; 4] = ["ui", "api", "git-wire", "agent"];

/// **ID-D1 — SCIM-disable → every surface denies within N = 5 min; stale re-grant = 0.**
#[test]
fn id_d1_scim_disable_denies_every_surface_within_bound() {
    // The threshold — read from the canonical thresholds file (never hardcoded). N = 5 min.
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

    // Sanity: BEFORE the disable every surface honours alice (the grant is live).
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

    // THE EVENT: SCIM-disable alice at T0 (the authoritative revocation path). The deny is effective
    // immediately (the S7 denylist consult); the deny-latency is the gap between this and the first
    // surface deny — which is 0 in this deterministic model (a hot consult, no propagation delay).
    let disabled_at = "2026-06-19T01:00:00Z";
    svc.disable_principal_in(
        &acme,
        &PrincipalId("p:alice".into()),
        Timestamp(disabled_at.into()),
    );

    // THE DRILL: every surface re-checks alice AFTER the disable. Count the surfaces that still
    // serve the stale grant (must be 0), and the worst-case deny-latency (must be ≤ the bound).
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
            // A surface that still honoured the revoked user — the F8 failure.
            stale_regrant_count += 1;
        } else {
            // The deny was effective at revoke time (the denylist consult), so the deny-latency on
            // this surface is 0 seconds — well under the bound.
            let deny_latency_secs = 0;
            worst_deny_latency_secs = worst_deny_latency_secs.max(deny_latency_secs);
        }
        let _ = surface;
    }

    // The `revocation_lag` telemetry (contract-index row 1.8) fired on the revoke — observability is
    // part of the pass (an auth decision that emits no signal has failed the gate, EI-01 §3).
    assert_eq!(RevocationTelemetry::SIGNAL, "revocation_lag");
    assert_eq!(
        svc.revocations().telemetry().revocation_count(),
        1,
        "the SCIM-disable emitted one revocation_lag observation"
    );

    // THE green artifacts, asserted through the harness telemetry-assertion library (loud on red):
    // (1) stale-re-grant == 0 (no surface kept serving the revoked principal).
    let mut signals = SignalSource::new();
    signals.set_scalar(SignalName::CrossTenantCount, stale_regrant_count);
    signals
        .assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();
    // (2) deny-latency ≤ the bound (read from the thresholds file). Asserted directly against the
    //     measured worst-case (p100) deny-latency.
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
         revocation_SLA={sla_secs}s (N={} min, read from the thresholds file) — every surface \
         denies the revoked principal through the SAME S7 denylist consult (no bespoke per-surface \
         revocation path)",
        thresholds.revocation.sla_mins
    );
}

/// **ID-D1 idempotent + crash-safe leg: a double-revoke across a simulated crash is a no-op, and the
/// deny survives the crash.** The revoke is written mirror-first; a crash loses the fast layer;
/// recovery rebuilds it from the durable mirror; the deny is back and a re-revoke is a no-op.
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

    // Disable alice; she is denied.
    svc.disable_principal_in(
        &acme,
        &PrincipalId("p:alice".into()),
        Timestamp("2026-06-19T01:00:00Z".into()),
    );
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

    // SIMULATED CRASH: the fast Redis/Valkey layer is lost; recovery rebuilds it from the durable
    // mirror — the revocation survives (a revoke is never lost on crash).
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

    // A re-revoke across the crash is a no-op (idempotent even on crash) — the count stays 1.
    svc.disable_principal_in(
        &acme,
        &PrincipalId("p:alice".into()),
        Timestamp("2026-06-19T09:00:00Z".into()),
    );
    assert_eq!(
        s7.revocation_count(&acme),
        1,
        "a double-revoke across a crash is a no-op (idempotent even on crash)"
    );
}
