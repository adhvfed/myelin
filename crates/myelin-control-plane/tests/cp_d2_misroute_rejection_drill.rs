use myelin_control_plane::{
    Capacity, Cell, CellGateway, CellStatus, GatewayReject, IsolationKind, Misroute,
    MisrouteAuditRecord, PlacementStatus, Registry, TenantPlacement,
};
use myelin_harness::{Predicate, SignalName, SignalSource};
use myelin_tenancy::{CellId, Region, TenantId};

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

fn place(reg: &mut Registry, tenant: &str, home: &str, slug: &str) {
    reg.place_tenant(TenantPlacement {
        tenant_id: TenantId::from_token(tenant),
        region: Region::new("eu-west"),
        home_cell: CellId::from_token(home),
        isolation_tier: IsolationKind::Pool,
        slug: slug.into(),
        status: PlacementStatus::Active,
        member_cells: vec![CellId::from_token(home)],
    })
    .expect("a single-region placement is admitted");
}

#[test]
fn cp_d2_misroute_rejection_tenant_grain() {
    let mut reg = Registry::new();
    reg.insert_cell(cell("cell-w-1", "eu-west"));
    reg.insert_cell(cell("cell-w-2", "eu-west"));
    place(&mut reg, "01J0ACME", "cell-w-1", "acme");
    place(&mut reg, "01J0BETA", "cell-w-2", "beta");

    let wrong = CellGateway::new(CellId::from_token("cell-w-2"));
    let reject = wrong
        .route(&reg, &TenantId::from_token("01J0ACME"))
        .expect_err("cell-w-2 does not host ACME → REJECTED (the gate is RED for the spoof)");
    assert_eq!(
        reject,
        GatewayReject::Misroute(Misroute {
            tenant_id: TenantId::from_token("01J0ACME"),
            correct_cell: CellId::from_token("cell-w-1"),
            correct_cell_endpoint: "cell.eu-west.cell-w-1.myelin.eu".into(),
        }),
        "the misroute redirects to the HOME cell-endpoint (not proxied)"
    );
    assert_eq!(wrong.audit().count(), 1, "the misroute is audited");
    assert_eq!(
        wrong.audit().records()[0],
        MisrouteAuditRecord {
            tenant_id: TenantId::from_token("01J0ACME"),
            received_by_cell: CellId::from_token("cell-w-2"),
            home_cell: Some(CellId::from_token("cell-w-1")),
        }
    );
    let misroute_count = wrong.misroute_count();
    let cross_tenant_reads = wrong.cross_tenant_reads();
    assert_eq!(
        misroute_count, 1,
        "misroute_count increments on a rejected misroute"
    );
    assert_eq!(
        cross_tenant_reads, 0,
        "0 cross-tenant/cross-cell rows read (the CP-D2 zero)"
    );

    let unknown = wrong
        .route(&reg, &TenantId::from_token("01J0GHOST"))
        .expect_err("an unknown tenant is rejected (no route)");
    assert_eq!(
        unknown,
        GatewayReject::NoSuchTenant {
            tenant_id: TenantId::from_token("01J0GHOST")
        }
    );
    assert_eq!(
        wrong.misroute_count(),
        2,
        "the unknown-tenant rejection is also counted"
    );
    assert_eq!(wrong.cross_tenant_reads(), 0, "still 0 cross-tenant reads");

    let home = CellGateway::new(CellId::from_token("cell-w-1"));
    let served = home
        .route(&reg, &TenantId::from_token("01J0ACME"))
        .expect("the home cell serves its own tenant (the gate is GREEN)");
    assert_eq!(served.home_cell.as_str(), "cell-w-1");
    assert_eq!(served.region.as_str(), "eu-west");
    assert_eq!(
        served.member_cells.len(),
        1,
        "v1 member_cells single-element (the floor)"
    );
    assert_eq!(
        home.misroute_count(),
        0,
        "the home cell does not misroute its own tenant"
    );
    assert_eq!(home.audit().count(), 0, "nothing to audit on an accept");
    assert_eq!(home.cross_tenant_reads(), 0);

    let GatewayReject::Misroute(redirect) = reject else {
        panic!("expected a misroute")
    };
    let re_routed = CellGateway::new(redirect.correct_cell.clone())
        .route(&reg, &TenantId::from_token("01J0ACME"))
        .expect("the redirected request is served by the home cell");
    assert_eq!(re_routed.home_cell, redirect.correct_cell);

    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, cross_tenant_reads as i64);
    sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-084 CP-D2 GREEN 2026-06-19] placement_of + gateway misroute-rejection (tenant grain): a \
         request to cell-w-2 for a tenant homed on cell-w-1 was REJECTED (not proxied) + REDIRECTED \
         to cell.eu-west.cell-w-1.myelin.eu + AUDITED ({} audit entr{}); an unknown tenant rejected \
         too; misroute_count={}, cross_tenant/cross_cell reads SERVED={} (the CP-D2 zero). The home \
         cell served its OWN tenant (0 misroute). FLOOR: member_cells single-element / same-cell \
         resolution in v1 (the CrossCellPointer multi-cell path is P-CP-19/P-CP-20); the misroute \
         audit's durable tamper-evident chain is GDPR P-GA-19/P-062 (same PII-free shape).",
        wrong.audit().count(),
        if wrong.audit().count() == 1 { "y" } else { "ies" },
        wrong.misroute_count(),
        cross_tenant_reads,
    );
}

#[test]
fn cp_d2_gate_is_not_vacuous() {
    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, 1);
    assert!(
        !sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
            .is_green(),
        "a served cross-tenant read MUST read RED - the CP-D2 zero is a real tripwire"
    );
}
