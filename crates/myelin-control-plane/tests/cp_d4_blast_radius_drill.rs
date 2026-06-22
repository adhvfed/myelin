//! P-CP-14 (global P-098) GATE / DRILL — **the CP-outage blast-radius win (CP-D4): already-placed
//! tenants keep serving, only signup degrades** — dated green artifact.
//!
//! **The GATE (testing-strategy CP-D4 (§4.2) / tenancy-and-control-plane.md §8 / the F7 fail-static
//! family §4.1):** hard-down the control plane → already-placed tenants keep serving entirely within
//! their cells; ONLY signup/provisioning degrades. Telemetry: `serving-uptime`, degrade scope =
//! signup/provisioning only. SCHED (F7, fail-static). Never weaken a threshold to pass.
//!
//! **The load-bearing property (architecture §8, VISION §3 — degrade not cascade):** the control
//! plane is small, slow-changing, PII-free, and OFF the per-request hot path. A placed tenant's
//! request is served by its cell's own stores + its cell's own fail-closed `authenticate`/`check`
//! (ADR-03) — the control plane is consulted ONLY to discover the route, and that answer is
//! client-cached + **fail-static for routing** (contract 1.10, `FailStatic`). So a control-plane
//! outage cannot take the data plane down: the worst it does is degrade *signup* + *provisioning*
//! until the CP is back. This drill forces that outage with the **scoped-reversible dependency-break
//! injector** (the T-3 seam, P-S03), drives already-placed-tenant traffic + a signup attempt while
//! the CP is hard-down, and reads that serving survived (`serving-uptime == 100`, degrade scope =
//! signup/provisioning ONLY).
//!
//! **This drill proves the gate can go RED** (the report's `DataPlaneCascaded` scope if a placed
//! request ever failed) **AND green** (the placed tenant kept serving fail-static while signup
//! degraded), and emits the CP-D4 result on the SAME [`SignalSource`] every drill uses (observability
//! is part of the pass, EI-01 §3).
//!
//! **NO floor here (P-CP-14).** This is a property assertion over the already-built discovery cache
//! (P-CP-06 / P-081, `DiscoveryCache` — the fail-static degrade path). The drill is DB-free (the
//! degenerate registry + discovery cache are in-process, exactly like the CP-D2/CP-D3/four-layer
//! drills) — `cargo build --workspace` stays DB-free. The real CP transport + a live multi-cell
//! outage is the same named gateway/transport follow-on the routing surfaces carry; the
//! availability property (placed tenants keep serving, only signup degrades) is complete + drilled
//! now.

use myelin_control_plane::{
    cp_outage_bound, Capacity, Cell, CellGateway, CellStatus, ControlPlane, CounterMinter,
    CpOutageReport, DataPlane, DegradeScope, DiscoveryCache, IsolationKind, PlacementService,
    PlacementStatus, Registry, SignupPlane, TenantPlacement,
};
use myelin_harness::{Dependency, DependencyBreaker, Predicate, Scope, SignalName, SignalSource};
use myelin_substrate::TestClock;
use myelin_tenancy::{CellId, Region, TenantId};

/// The dependency name the CP-D4 drill severs through the T-3 injector (architecture §8): the control
/// plane, hard-down rig-wide. A `Named` dep needs no new enum variant (the every-incident-adds-a-drill
/// loop, EI-01 §5).
fn control_plane_dep() -> Dependency {
    Dependency::Named("control-plane".to_string())
}

fn cell(id: &str, region: &str) -> Cell {
    Cell {
        cell_id: CellId::from_token(id),
        region: Region::new(region),
        status: CellStatus::Active,
        isolation_kind: IsolationKind::Pool,
        capacity: Capacity {
            tenants_max: 1000,
            write_qps_max: 5000,
            storage_bytes_max: 1 << 40,
        },
        utilisation: 10,
        version: 1,
        endpoint: format!("cell.{region}.{id}.myelin.eu"),
    }
}

/// **THE CP-D4 DRILL (dated green artifact): hard-down the control plane (via the scoped-reversible
/// dependency-break injector) → already-placed tenants KEEP SERVING (fail-static routing) entirely
/// within their cells; ONLY signup/provisioning degrades; the system recovers when the CP is
/// restored.**
#[test]
fn cp_d4_cp_outage_blast_radius_placed_tenants_keep_serving() {
    // ── Already-placed data plane: one Active cell in eu-west, one placed tenant (ACME). ──
    let mut reg = Registry::new();
    reg.insert_cell(cell("cell-w-1", "eu-west"));
    let acme = TenantId::from_token("01J0ACME");
    reg.place_tenant(TenantPlacement {
        tenant_id: acme.clone(),
        region: Region::new("eu-west"),
        home_cell: CellId::from_token("cell-w-1"),
        isolation_tier: IsolationKind::Pool,
        slug: "acme".into(),
        status: PlacementStatus::Active,
        member_cells: vec![CellId::from_token("cell-w-1")],
    })
    .expect("the single-region placement is admitted");

    // The control plane + the data plane (cell gateway + fail-static discovery cache) + the signup
    // plane (the ONLY thing that should degrade). The discovery cache uses the production-shaped
    // §8.2 bound (static_max ≤ revocation SLA).
    let cp = ControlPlane::up();
    let gw = CellGateway::new(CellId::from_token("cell-w-1"));
    let cache = DiscoveryCache::try_new_with_clock(30, 300, cp_outage_bound(), TestClock::at(0))
        .expect("valid §8.2 bound");
    let dp = DataPlane::new(gw, cache, 30);
    let signup = SignupPlane::new(PlacementService::new(CounterMinter::new()));

    // ── The scoped-reversible dependency-break injector (the T-3 seam). The data/signup planes
    //    consult it to learn whether the control plane is severed — the SAME seam every later drill
    //    rides (testing-strategy §3.2: reversible + scoped + idempotent). ──
    let breaker = DependencyBreaker::new();

    // ── CP UP: the placed tenant serves (fresh route, primes the discovery cache). ──
    let s0 = dp
        .serve(&cp, &reg, &acme)
        .expect("CP up: the placed tenant serves");
    assert!(!s0.via_fail_static, "with the CP up the route is fresh");

    // ── HARD-DOWN the control plane rig-wide via the injector (the CP-D4 outage). The break is
    //    reversible + scoped: it severs ONLY the control plane, Global scope, and is lifted at the
    //    end. The ControlPlane handle is driven from the injector's consult — the fault-point. ──
    assert!(
        breaker
            .break_dependency(control_plane_dep(), Scope::Global)
            .changed(),
        "the CP is severed"
    );
    if breaker.is_broken(&control_plane_dep(), &Scope::Global) {
        cp.hard_down();
    }
    assert!(
        cp.is_down(),
        "the control plane is hard-down (the CP-D4 outage)"
    );

    // Drive already-placed-tenant traffic during the outage: the cache is past fresh_ttl (age 100 >
    // 30) but inside static_max (300) → every request is served FAIL-STATIC (the last-known-good
    // route) — the placed tenant KEEPS SERVING entirely within its cell.
    dp.cache().clock().advance(100);
    for _ in 0..5 {
        let s = dp
            .serve(&cp, &reg, &acme)
            .expect("CP down: the placed tenant KEEPS SERVING");
        assert!(
            s.via_fail_static,
            "the route is served fail-static while the CP is down"
        );
        assert_eq!(
            s.placement.home_cell.as_str(),
            "cell-w-1",
            "served within its cell"
        );
    }

    // SIGNUP degrades (and ONLY signup) — a new tenant cannot be placed while the CP is down.
    let degraded = signup
        .signup(
            &cp,
            &mut reg,
            &Region::new("eu-west"),
            IsolationKind::Pool,
            "newco",
        )
        .expect_err("CP down: signup DEGRADES (the contained blast radius)");
    assert!(
        degraded.to_string().contains("control plane hard-down"),
        "loud degrade: {degraded}"
    );
    assert_eq!(signup.signups_degraded(), 1, "exactly one signup degraded");
    assert_eq!(
        signup.service().signals().placement_count,
        0,
        "0 tenants placed while the CP was down"
    );

    // The data plane served EVERY placed-tenant request (the CP-D4 zero: 0 failures).
    let served = dp.placed_requests_served(); // 1 (CP up) + 5 (CP down) = 6
    let failed = dp.placed_requests_failed();
    assert_eq!(
        served, 6,
        "all placed-tenant requests served (1 fresh + 5 fail-static)"
    );
    assert_eq!(
        failed, 0,
        "0 placed-tenant requests failed (the CP-D4 zero)"
    );

    // ── RESTORE the control plane (the outage is lifted; the system is observed recovering). The
    //    break is reversible — a restored dependency is indistinguishable from one never broken. ──
    assert!(
        breaker
            .restore_dependency(control_plane_dep(), Scope::Global)
            .changed(),
        "the CP is restored"
    );
    if !breaker.is_broken(&control_plane_dep(), &Scope::Global) {
        cp.restore();
    }
    assert!(!cp.is_down(), "the control plane recovered");
    assert_eq!(
        breaker.broken_count(),
        0,
        "no leaked break (the injector is fully reversible)"
    );
    // Signup works again — a new tenant is placed PII-free (the system recovered).
    let placed = signup
        .signup(
            &cp,
            &mut reg,
            &Region::new("eu-west"),
            IsolationKind::Pool,
            "newco",
        )
        .expect("CP restored: signup works again");
    assert!(
        placed.tenant_id.as_str().starts_with("01J0CP-"),
        "a new tenant placed PII-free post-recovery"
    );

    // ── The measured CP-D4 report: serving-uptime 100%, degrade scope signup/provisioning ONLY. ──
    let report = CpOutageReport::compute(served, failed, signup.signups_degraded());
    assert!(report.is_cp_d4_win(), "the CP-D4 win: {report:?}");
    assert_eq!(
        report.serving_uptime_pct, 100,
        "serving-uptime is 100% (0 placed-request failures)"
    );
    assert_eq!(
        report.degrade_scope,
        DegradeScope::SignupAndProvisioningOnly,
        "the degrade scope is signup/provisioning ONLY — the data plane was unaffected"
    );

    // ── Emit the CP-D4 gate result on the SAME SignalSource every drill uses (observability is part
    //    of the pass, EI-01 §3): serving-uptime == 100 (RequestRate as the served-percentage gauge)
    //    + the CP-D4 zero (placed-request failures == 0, the CrossTenantCount-style tripwire). ──
    let mut sig = SignalSource::new();
    // serving-uptime: the percentage of placed-tenant requests still served during the outage.
    sig.set_scalar(SignalName::RequestRate, report.serving_uptime_pct as i64);
    sig.assert_signal(SignalName::RequestRate, Predicate::Eq(100))
        .expect_green();
    // the CP-D4 zero: 0 placed-tenant requests failed (the data plane did not cascade).
    sig.set_scalar(
        SignalName::CrossTenantCount,
        report.placed_requests_failed as i64,
    );
    sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-098 CP-D4 GREEN 2026-06-19] CP-outage blast-radius win: the control plane was hard-down \
         rig-wide (via the scoped-reversible dependency-break injector, Named(\"control-plane\") / \
         Global); the already-placed tenant (ACME on cell-w-1, eu-west) KEPT SERVING entirely within \
         its cell — {served} requests served ({} fresh + {} fail-static), {failed} failed (the CP-D4 \
         zero). ONLY signup degraded: {} signup(s) degraded, placement_count=0 while the CP was down. \
         serving-uptime={}%, degrade scope = {:?}. On restore the system recovered (signup placed a \
         new tenant PII-free; 0 leaked breaks — fully reversible). DEGRADE, NOT CASCADE (VISION §3). \
         NO floor — a property assertion over the already-built fail-static discovery cache (P-CP-06).",
        1,
        served - 1,
        signup.signups_degraded(),
        report.serving_uptime_pct,
        report.degrade_scope,
    );
}

/// **The gate is NOT vacuous: a placed-tenant request that FAILED during the outage WOULD read RED.**
/// Proves the CP-D4 serving-uptime is a real tripwire — if the data plane cascaded (a placed request
/// failed because routing did NOT fail-static), serving-uptime drops below 100 and the failure count
/// ticks above 0, both of which fail the predicates. A gate that cannot go red is not a gate (EI-01
/// §3).
#[test]
fn cp_d4_gate_is_not_vacuous() {
    // A hypothetical cascade: 1 of 6 placed-tenant requests failed during the outage.
    let cascaded = CpOutageReport::compute(5, 1, 2);
    assert_eq!(
        cascaded.degrade_scope,
        DegradeScope::DataPlaneCascaded,
        "a failed placed request cascades"
    );
    assert!(!cascaded.is_cp_d4_win(), "a cascade is NOT the CP-D4 win");

    let mut sig = SignalSource::new();
    // serving-uptime dropped below 100 → the predicate fails.
    sig.set_scalar(SignalName::RequestRate, cascaded.serving_uptime_pct as i64);
    assert!(
        !sig.assert_signal(SignalName::RequestRate, Predicate::Eq(100))
            .is_green(),
        "serving-uptime < 100 MUST read RED — the CP-D4 serving-uptime is a real tripwire"
    );
    // and the failure count ticked above 0 → the zero tripwire fails too.
    sig.set_scalar(
        SignalName::CrossTenantCount,
        cascaded.placed_requests_failed as i64,
    );
    assert!(
        !sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
            .is_green(),
        "a failed placed-tenant request MUST read RED — the CP-D4 zero is a real tripwire"
    );
}
