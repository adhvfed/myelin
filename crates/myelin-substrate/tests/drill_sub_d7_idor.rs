//! # SUB-D7 — the cross-tenant IDOR drill (P-S13 → global P-030)
//!
//! **Drill catalogue:** `planning/05-refined-shared-systems-architecture/testing-strategy/
//! 01-whole-system-e2e-and-drill-catalogue.md` §4.2 row **SUB-D7**: *"Cross-tenant read via
//! path≠token tenant → 0; lint catches a tenant-less query at compile."* Threshold:
//! `misroute-count 0`; lint green. Surface: CI.
//!
//! This is the **dated green artifact** the P-S13 GATE/DRILLS names. It is the EI-01 §3 drill
//! shape: *inject a fault (P-S03 `break_dependency`), drive one unit of load (P-S02 generator),
//! read one telemetry assertion that reads green.* Here:
//!   - **inject** — `break_dependency(Identity, …)` models the gateway's identity dependency under
//!     stress (the realistic condition an IDOR is attempted under); the tenant-from-token mechanism
//!     must hold REGARDLESS — the structural defence does not depend on a healthy downstream.
//!   - **load** — a burst of public requests whose URL path names a DIFFERENT tenant than the
//!     verified token (the spoof) interleaved with honest same-tenant requests.
//!   - **assert** — `CrossTenantCount == 0` (the `misroute_count` projection): ZERO cross-tenant
//!     reads were served. Every spoof was rejected + audited; no honest request was lost.
//!
//! The lint half of the row (the `tenant-predicate` lint catching a tenant-less query at compile
//! time) is shipped + CI-wired by P-S10/P-017 (`myelin-lints`); this drill is the runtime
//! `misroute-count 0` half.

use myelin_harness::{
    Dependency, DrillResult, DrillScenario, Predicate, Scope, SignalName, SignalSource,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_substrate::{InjectedIdentity, PublicReject, PublicSurface};
use myelin_tenancy::TenantId;

/// The SUB-D7 drill scenario: under an injected identity-dependency hiccup, fire a mixed burst of
/// honest same-tenant requests and adversarial path≠token spoof requests at the tenant-from-token
/// public surface, and assert ZERO cross-tenant reads were served (`misroute_count == 0`).
fn sub_d7_idor_scenario() -> DrillScenario {
    DrillScenario::new("sub-d7-cross-tenant-idor", |ctx| {
        // (inject) the gateway's identity dependency is under stress for `globex` — the realistic
        // condition an attacker probes under. The structural defence must hold regardless.
        ctx.breaker
            .break_dependency(Dependency::Identity, Scope::Tenant(TenantId("globex".into())));

        // The public surface the lifecycle opens (tenant-from-token + IDOR reject/audit).
        let surface = PublicSurface::default();

        // (load) a burst: honest acme requests (path == token) + adversarial spoofs (an acme token
        // naming globex in the path, and a globex token naming acme). The 1× unit of load.
        let acme = InjectedIdentity::new(stub("acme-user", "acme"));
        let globex = InjectedIdentity::new(stub("globex-user", "globex"));
        let mut served_cross_tenant = 0i64;
        for _ in 0..30 {
            // honest acme request → served against acme (the token's tenant).
            let r = surface.resolve_tenant(&acme, &TenantId("acme".into()));
            assert_eq!(r, Ok(TenantId("acme".into())), "honest same-tenant request is served");

            // adversarial: acme token tries to read globex via the path → rejected + audited.
            match surface.resolve_tenant(&acme, &TenantId("globex".into())) {
                Err(PublicReject::CrossTenantIdor { .. }) => {}
                Ok(t) => {
                    // a SERVED cross-tenant read — the bug SUB-D7 forbids. Count it (would fail).
                    if t != *acme.token_tenant() {
                        served_cross_tenant += 1;
                    }
                }
            }
            // adversarial: globex token tries to read acme via the path → rejected + audited.
            match surface.resolve_tenant(&globex, &TenantId("acme".into())) {
                Err(PublicReject::CrossTenantIdor { .. }) => {}
                Ok(t) => {
                    if t != *globex.token_tenant() {
                        served_cross_tenant += 1;
                    }
                }
            }
        }

        // restore the injected fault (a re-run starts clean).
        ctx.breaker
            .restore_dependency(Dependency::Identity, Scope::Tenant(TenantId("globex".into())));

        // (assert) the SUB-D7 zero, read off the live surface's misroute_count AND the locally
        // observed served-cross-tenant tally (belt-and-braces: both must be 0).
        assert_eq!(served_cross_tenant, 0, "no spoof was served (local tally)");
        let misroute = surface.misroute_count() as i64;
        // 60 spoof attempts (2 per iteration × 30) were all rejected + audited.
        assert_eq!(surface.audit().count(), 60, "every spoof attempt was audited (PII-free)");

        // Populate the harness telemetry-assertion library with the CrossTenantCount projection
        // and assert it is green (== 0). This is the typed, never-swallowed verdict.
        let mut src = SignalSource::new();
        src.set_scalar(SignalName::CrossTenantCount, misroute);
        src.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
    })
}

fn stub(id: &str, tenant: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, TenantId(tenant.into()))
}

/// **THE SUB-D7 drill — the dated green artifact.** Runs the scenario once and asserts it PASSES:
/// `CrossTenantCount == 0` under an injected identity hiccup, with every spoof rejected + audited.
#[test]
fn sub_d7_cross_tenant_idor_drill_is_green() {
    let drill = sub_d7_idor_scenario();
    let result = drill.run_once();
    assert!(
        result.is_pass(),
        "SUB-D7 must be green (misroute_count == 0): {}",
        result.artifact_row("2026-06-19")
    );
    // The dated green-artifact row (EI-01 §3: a passing drill emits a visible, dated row).
    let row = result.artifact_row("2026-06-19");
    assert!(row.contains("PASS"), "the artifact row records a PASS: {row}");
    assert!(row.contains("sub-d7-cross-tenant-idor"), "names the drill: {row}");
    println!("{row}");
}

/// The drill is re-runnable forever (the every-incident-adds-a-drill loop, T-3): a second run from
/// a fresh context reads green again — the property is proven each time, not a stale leftover.
#[test]
fn sub_d7_drill_reruns_green() {
    let drill = sub_d7_idor_scenario();
    for _ in 0..3 {
        assert!(matches!(drill.run_once(), DrillResult::Pass { .. }), "SUB-D7 re-runs green");
    }
}

/// A control that PROVES the drill would catch a regression: if the surface ever served a
/// cross-tenant read (a non-zero misroute), the `CrossTenantCount == 0` assertion reads RED. We
/// model the regression by asserting a deliberately non-zero count is NOT green — confirming the
/// gate is real (it is not a vacuous always-green). The real surface keeps the count at 0.
#[test]
fn sub_d7_gate_is_not_vacuous_a_nonzero_misroute_reads_red() {
    let surface = PublicSurface::default();
    // drive one honest + one spoof; the real surface stays at 0.
    let id = InjectedIdentity::new(stub("p", "acme"));
    let _ = surface.resolve_tenant(&id, &TenantId("globex".into()));
    assert_eq!(surface.misroute_count(), 0, "the real surface never serves a cross-tenant read");

    // model a regression: a non-zero misroute count must read RED against the == 0 predicate.
    let mut src = SignalSource::new();
    src.set_scalar(SignalName::CrossTenantCount, 1);
    let verdict = src.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0));
    assert!(!verdict.is_green(), "a served cross-tenant read (misroute > 0) MUST read RED — the gate is real");
}
