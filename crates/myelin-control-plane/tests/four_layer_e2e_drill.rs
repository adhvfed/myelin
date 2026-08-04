use myelin_control_plane::{
    Capacity, Cell, CellGateway, CellStatus, CrossRegionPathError, FourLayerEnforcement,
    IsolationKind, PlacementStatus, Registry, ResidencyWriteBoundary, ResidencyWriteRejected,
    TenantPlacement,
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

#[test]
fn four_layer_region_pinning_end_to_end() {
    let mut reg = Registry::new();
    reg.insert_cell(cell("cell-fr-1", "fr-par"));
    reg.insert_cell(cell("cell-fr-2", "fr-par"));
    reg.insert_cell(cell("cell-de-1", "eu-central"));

    let cross_region = FourLayerEnforcement::place(
        &mut reg,
        TenantPlacement {
            tenant_id: TenantId::from_token("01J0CROSS"),
            region: Region::new("fr-par"),
            home_cell: CellId::from_token("cell-fr-1"),
            isolation_tier: IsolationKind::Pool,
            slug: "cross".into(),
            status: PlacementStatus::Active,
            member_cells: vec![
                CellId::from_token("cell-fr-1"),
                CellId::from_token("cell-de-1"),
            ],
        },
    );
    assert!(
        cross_region.is_err(),
        "layers 1+2: a cross-region member cell is rejected at placement"
    );

    FourLayerEnforcement::place(
        &mut reg,
        TenantPlacement {
            tenant_id: TenantId::from_token("01J0ACME"),
            region: Region::new("fr-par"),
            home_cell: CellId::from_token("cell-fr-1"),
            isolation_tier: IsolationKind::Pool,
            slug: "acme".into(),
            status: PlacementStatus::Active,
            member_cells: vec![CellId::from_token("cell-fr-1")],
        },
    )
    .expect("layers 1+2: a single-region placement is admitted");

    let gateway = CellGateway::new(CellId::from_token("cell-fr-1"));
    let enforcement = FourLayerEnforcement::new(&reg, gateway, Region::new("fr-par"));

    enforcement
        .admit_write(&Region::new("fr-par"))
        .expect("layer 3: an in-region write is admitted");
    let rejected = enforcement
        .admit_write(&Region::new("eu-central"))
        .expect_err("layer 3: an out-of-region write is REJECTED at the boundary (runtime CP-D3)");
    assert_eq!(
        rejected,
        ResidencyWriteRejected {
            cell_region: Region::new("fr-par"),
            row_region: Region::new("eu-central"),
        }
    );
    assert!(
        rejected.to_string().contains("no cross-region query path"),
        "loud: {rejected}"
    );

    let served = enforcement
        .route(&TenantId::from_token("01J0ACME"))
        .expect("layer 4: the home cell serves its own tenant");
    assert_eq!(served.region.as_str(), "fr-par");

    let wrong = FourLayerEnforcement::new(
        &reg,
        CellGateway::new(CellId::from_token("cell-fr-2")),
        Region::new("fr-par"),
    );
    let misroute = wrong
        .route(&TenantId::from_token("01J0ACME"))
        .expect_err("layer 4: cell-fr-2 does not host ACME → REJECTED (CP-D2)");
    assert!(
        misroute.to_string().contains("REJECTED"),
        "loud misroute: {misroute}"
    );
    assert_eq!(
        wrong.gateway().misroute_count(),
        1,
        "the misroute is counted"
    );
    assert_eq!(
        wrong.gateway().cross_tenant_reads(),
        0,
        "0 cross-tenant/cross-cell reads (the CP-D2 zero)"
    );

    enforcement
        .assert_no_cross_region_query_path(
            &TenantId::from_token("01J0ACME"),
            &Region::new("eu-central"),
        )
        .expect("NO cross-region query path: ACME is served here and its data stays in fr-par");

    let out_of_region_writes_admitted =
        enforcement.write_boundary().out_of_region_writes_admitted();
    let cross_tenant_reads = enforcement.gateway().cross_tenant_reads();
    assert_eq!(
        out_of_region_writes_admitted, 0,
        "0 out-of-region writes admitted (layer 3 / STOR-D5)"
    );
    assert_eq!(
        cross_tenant_reads, 0,
        "0 cross-tenant/cross-cell reads (layer 4 / CP-D2)"
    );

    use myelin_control_plane::{
        residency_verify, ResidencySigningKey, ResidencyStoreClass, StoreRegionReport,
    };
    let key = ResidencySigningKey::from_bytes([0x12u8; 32]);
    let reports: Vec<StoreRegionReport> = ResidencyStoreClass::M1_SET
        .iter()
        .map(|c| StoreRegionReport::new(*c, Region::new("fr-par")))
        .collect();
    let attestation = residency_verify(
        &TenantId::from_token("01J0ACME"),
        &Region::new("fr-par"),
        &reports,
        &key,
    )
    .expect("residency_verify attestation PASSES for in-region writes (the green CP-D3 artifact)");
    assert!(
        attestation.verify(&key),
        "the attestation is signed + verifies"
    );

    let mut sig = SignalSource::new();
    sig.set_scalar(
        SignalName::CrossTenantCount,
        (out_of_region_writes_admitted + cross_tenant_reads) as i64,
    );
    sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-096 P-CP-12 GREEN 2026-06-19] four-layer region-pinning enforced end-to-end \
         (CP-D3 + CP-D2 e2e): layers 1+2 rejected a cross-region member-cell placement + admitted a \
         single-region one; layer 3 (runtime) REJECTED an out-of-region write (eu-central) at the \
         boundary + admitted the fr-par write; layer 4 served the home tenant + REJECTED a misroute \
         to cell-fr-2 (misroute_count=1, cross_tenant_reads=0); the no-cross-region-query-path \
         assertion held; residency_verify attested fr-par across all 4 M1 stores. GO/NO-GO ZEROS: \
         out_of_region_writes_admitted={out_of_region_writes_admitted}, \
         cross_tenant_reads={cross_tenant_reads}. NO engineering floor; [OPEN - LEGAL] \
         region-change-as-DSR + slug-PII-screening ship regardless. The live cross-region-egress \
         drill is storage stor_d5_cross_region_egress (--features integration, dev stack)."
    );
}

#[test]
fn cp_d3_runtime_gate_is_not_vacuous() {
    let boundary = ResidencyWriteBoundary::for_cell(Region::new("fr-par"));
    assert!(
        boundary.check_write(&Region::new("eu-central")).is_err(),
        "an out-of-region write MUST be rejected - a gate that cannot go red is not a gate"
    );

    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, 1);
    assert!(
        !sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
            .is_green(),
        "an admitted out-of-region write MUST read RED - the STOR-D5 zero is a real tripwire"
    );
}

#[test]
fn assertion_catches_a_cell_that_does_not_home_the_tenant() {
    let mut reg = Registry::new();
    reg.insert_cell(cell("cell-fr-1", "fr-par"));
    reg.insert_cell(cell("cell-fr-2", "fr-par"));
    FourLayerEnforcement::place(
        &mut reg,
        TenantPlacement {
            tenant_id: TenantId::from_token("01J0ACME"),
            region: Region::new("fr-par"),
            home_cell: CellId::from_token("cell-fr-1"),
            isolation_tier: IsolationKind::Pool,
            slug: "acme".into(),
            status: PlacementStatus::Active,
            member_cells: vec![CellId::from_token("cell-fr-1")],
        },
    )
    .expect("placed");

    let enforcement = FourLayerEnforcement::new(
        &reg,
        CellGateway::new(CellId::from_token("cell-fr-2")),
        Region::new("fr-par"),
    );
    let err = enforcement
        .assert_no_cross_region_query_path(
            &TenantId::from_token("01J0ACME"),
            &Region::new("eu-central"),
        )
        .expect_err("cell-fr-2 does not home ACME → the assertion fails");
    assert!(
        matches!(err, CrossRegionPathError::TenantNotServedHere { .. }),
        "loud: {err}"
    );
}
