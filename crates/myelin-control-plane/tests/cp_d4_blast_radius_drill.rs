use myelin_control_plane::{
    cp_outage_bound, Capacity, Cell, CellGateway, CellStatus, ControlPlane, CounterMinter,
    CpOutageReport, DataPlane, DegradeScope, DiscoveryCache, IsolationKind, PlacementService,
    PlacementStatus, Registry, SignupPlane, TenantPlacement,
};
use myelin_harness::{Dependency, DependencyBreaker, Predicate, Scope, SignalName, SignalSource};
use myelin_substrate::TestClock;
use myelin_tenancy::{CellId, Region, TenantId};

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

#[test]
fn cp_d4_cp_outage_blast_radius_placed_tenants_keep_serving() {
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

    let cp = ControlPlane::up();
    let gw = CellGateway::new(CellId::from_token("cell-w-1"));
    let cache = DiscoveryCache::try_new_with_clock(30, 300, cp_outage_bound(), TestClock::at(0))
        .expect("valid §8.2 bound");
    let dp = DataPlane::new(gw, cache, 30);
    let signup = SignupPlane::new(PlacementService::new(CounterMinter::new()));

    let breaker = DependencyBreaker::new();

    let s0 = dp
        .serve(&cp, &reg, &acme)
        .expect("CP up: the placed tenant serves");
    assert!(!s0.via_fail_static, "with the CP up the route is fresh");

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

    let served = dp.placed_requests_served();
    let failed = dp.placed_requests_failed();
    assert_eq!(
        served, 6,
        "all placed-tenant requests served (1 fresh + 5 fail-static)"
    );
    assert_eq!(
        failed, 0,
        "0 placed-tenant requests failed (the CP-D4 zero)"
    );

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

    let report = CpOutageReport::compute(served, failed, signup.signups_degraded());
    assert!(report.is_cp_d4_win(), "the CP-D4 win: {report:?}");
    assert_eq!(
        report.serving_uptime_pct, 100,
        "serving-uptime is 100% (0 placed-request failures)"
    );
    assert_eq!(
        report.degrade_scope,
        DegradeScope::SignupAndProvisioningOnly,
        "the degrade scope is signup/provisioning ONLY - the data plane was unaffected"
    );

    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::RequestRate, report.serving_uptime_pct as i64);
    sig.assert_signal(SignalName::RequestRate, Predicate::Eq(100))
        .expect_green();
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
         its cell - {served} requests served ({} fresh + {} fail-static), {failed} failed (the CP-D4 \
         zero). ONLY signup degraded: {} signup(s) degraded, placement_count=0 while the CP was down. \
         serving-uptime={}%, degrade scope = {:?}. On restore the system recovered (signup placed a \
         new tenant PII-free; 0 leaked breaks - fully reversible). DEGRADE, NOT CASCADE (VISION §3). \
         NO floor - a property assertion over the already-built fail-static discovery cache (P-CP-06).",
        1,
        served - 1,
        signup.signups_degraded(),
        report.serving_uptime_pct,
        report.degrade_scope,
    );
}

#[test]
fn cp_d4_gate_is_not_vacuous() {
    let cascaded = CpOutageReport::compute(5, 1, 2);
    assert_eq!(
        cascaded.degrade_scope,
        DegradeScope::DataPlaneCascaded,
        "a failed placed request cascades"
    );
    assert!(!cascaded.is_cp_d4_win(), "a cascade is NOT the CP-D4 win");

    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::RequestRate, cascaded.serving_uptime_pct as i64);
    assert!(
        !sig.assert_signal(SignalName::RequestRate, Predicate::Eq(100))
            .is_green(),
        "serving-uptime < 100 MUST read RED - the CP-D4 serving-uptime is a real tripwire"
    );
    sig.set_scalar(
        SignalName::CrossTenantCount,
        cascaded.placed_requests_failed as i64,
    );
    assert!(
        !sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
            .is_green(),
        "a failed placed-tenant request MUST read RED - the CP-D4 zero is a real tripwire"
    );
}
