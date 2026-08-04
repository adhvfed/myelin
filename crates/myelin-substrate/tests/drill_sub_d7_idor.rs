use myelin_harness::{
    Dependency, DrillResult, DrillScenario, Predicate, Scope, SignalName, SignalSource,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_substrate::{InjectedIdentity, PublicReject, PublicSurface};
use myelin_tenancy::TenantId;

fn sub_d7_idor_scenario() -> DrillScenario {
    DrillScenario::new("sub-d7-cross-tenant-idor", |ctx| {
        ctx.breaker.break_dependency(
            Dependency::Identity,
            Scope::Tenant(TenantId("globex".into())),
        );

        let surface = PublicSurface::default();

        let acme = InjectedIdentity::new(stub("acme-user", "acme"));
        let globex = InjectedIdentity::new(stub("globex-user", "globex"));
        let mut served_cross_tenant = 0i64;
        for _ in 0..30 {
            let r = surface.resolve_tenant(&acme, &TenantId("acme".into()));
            assert_eq!(
                r,
                Ok(TenantId("acme".into())),
                "honest same-tenant request is served"
            );

            match surface.resolve_tenant(&acme, &TenantId("globex".into())) {
                Err(PublicReject::CrossTenantIdor { .. }) => {}
                Ok(t) => {
                    if t != *acme.token_tenant() {
                        served_cross_tenant += 1;
                    }
                }
            }
            match surface.resolve_tenant(&globex, &TenantId("acme".into())) {
                Err(PublicReject::CrossTenantIdor { .. }) => {}
                Ok(t) => {
                    if t != *globex.token_tenant() {
                        served_cross_tenant += 1;
                    }
                }
            }
        }

        ctx.breaker.restore_dependency(
            Dependency::Identity,
            Scope::Tenant(TenantId("globex".into())),
        );

        assert_eq!(served_cross_tenant, 0, "no spoof was served (local tally)");
        let misroute = surface.misroute_count() as i64;
        assert_eq!(
            surface.audit().count(),
            60,
            "every spoof attempt was audited (PII-free)"
        );

        let mut src = SignalSource::new();
        src.set_scalar(SignalName::CrossTenantCount, misroute);
        src.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
    })
}

fn stub(id: &str, tenant: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    )
}

#[test]
fn sub_d7_cross_tenant_idor_drill_is_green() {
    let drill = sub_d7_idor_scenario();
    let result = drill.run_once();
    assert!(
        result.is_pass(),
        "SUB-D7 must be green (misroute_count == 0): {}",
        result.artifact_row("2026-06-19")
    );
    let row = result.artifact_row("2026-06-19");
    assert!(
        row.contains("PASS"),
        "the artifact row records a PASS: {row}"
    );
    assert!(
        row.contains("sub-d7-cross-tenant-idor"),
        "names the drill: {row}"
    );
    println!("{row}");
}

#[test]
fn sub_d7_drill_reruns_green() {
    let drill = sub_d7_idor_scenario();
    for _ in 0..3 {
        assert!(
            matches!(drill.run_once(), DrillResult::Pass { .. }),
            "SUB-D7 re-runs green"
        );
    }
}

#[test]
fn sub_d7_gate_is_not_vacuous_a_nonzero_misroute_reads_red() {
    let surface = PublicSurface::default();
    let id = InjectedIdentity::new(stub("p", "acme"));
    let _ = surface.resolve_tenant(&id, &TenantId("globex".into()));
    assert_eq!(
        surface.misroute_count(),
        0,
        "the real surface never serves a cross-tenant read"
    );

    let mut src = SignalSource::new();
    src.set_scalar(SignalName::CrossTenantCount, 1);
    let verdict = src.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0));
    assert!(
        !verdict.is_green(),
        "a served cross-tenant read (misroute > 0) MUST read RED - the gate is real"
    );
}
