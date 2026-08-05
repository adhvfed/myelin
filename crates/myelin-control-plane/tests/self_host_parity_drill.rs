use myelin_control_plane::{
    CounterMinter, DegenerateControlPlane, IsolationKind, PlacementService, ResidencySigningKey,
    ResidencyStoreClass,
};
use myelin_harness::{Predicate, SignalName, SignalSource};
use myelin_tenancy::{CellId, Region, TenantId};

#[test]
fn self_host_parity_degenerate_one_cell_identical_code_path() {
    let region = Region::new("fr-par");
    let mut sh = DegenerateControlPlane::bootstrap(CellId::from_token("cell-self"), region.clone());

    assert_eq!(
        sh.registry().cell_count(),
        1,
        "a self-host install is EXACTLY one cell"
    );
    assert_eq!(
        sh.cell().region.as_str(),
        "fr-par",
        "pinned to the install's region"
    );

    let service = PlacementService::new(CounterMinter::new());
    let answer = sh
        .place(&service, IsolationKind::Pool, "self-host-tenant")
        .expect("the one Active cell is eligible → placed via the SHARED PlacementService::place");
    let tenant: TenantId = answer.tenant_id.clone();
    assert_eq!(
        answer.home_cell.as_str(),
        "cell-self",
        "placed on the install's own cell"
    );

    let discovered = sh
        .discover_cell(&tenant)
        .expect("a placed tenant discovers");
    assert_eq!(
        discovered.as_str(),
        "cell-self",
        "discover returns 'this cell'"
    );
    let placement = sh
        .placement_of(&tenant)
        .expect("a placed tenant has a placement_of answer");
    assert_eq!(
        placement.home_cell.as_str(),
        "cell-self",
        "placement_of returns 'this cell'"
    );
    assert_eq!(
        placement.member_cells.len(),
        1,
        "member_cells is single-element - multi-cell is N/A for self-host by definition (the model)"
    );

    let gw = sh.gateway();
    let served = gw
        .route(sh.registry(), &tenant)
        .expect("the one cell homes (and serves) every tenant");
    assert_eq!(served.home_cell.as_str(), "cell-self");
    assert_eq!(gw.misroute_count(), 0, "no misroute on a one-cell install");
    let cross_tenant_reads = gw.cross_tenant_reads();
    assert_eq!(
        cross_tenant_reads, 0,
        "0 cross-tenant reads (the CP-D2 zero) on the degenerate cell"
    );

    sh.cp_d3_residency_pin_holds(&Region::new("eu-north"))
        .expect("the residency-pin holds on the degenerate cell (out-of-region write REJECTED)");
    sh.assert_no_cross_region_query_path(&tenant, &Region::new("us-east"))
        .expect(
            "the one cell serves its tenant and that data stays in fr-par (no cross-region path)",
        );
    let out_of_region_writes_admitted = sh
        .four_layer()
        .write_boundary()
        .out_of_region_writes_admitted();
    assert_eq!(
        out_of_region_writes_admitted, 0,
        "0 out-of-region writes admitted (CP-D3 on the degenerate cell)"
    );

    let key = ResidencySigningKey::from_bytes([0x5eu8; 32]);
    let attestation = sh
        .residency_verify_own_data(&tenant, &key)
        .expect("residency_verify is GREEN on the self-host cell's own data");
    assert_eq!(
        attestation.region.as_str(),
        "fr-par",
        "every M1 store reported the install's region"
    );
    assert_eq!(
        attestation.store_regions.len(),
        ResidencyStoreClass::M1_SET.len(),
        "every M1 store attested"
    );
    assert!(
        attestation.verify(&key),
        "the green residency-attestation verifies (0 mismatches)"
    );

    let mut sig = SignalSource::new();
    sig.set_scalar(
        SignalName::CrossTenantCount,
        (out_of_region_writes_admitted + cross_tenant_reads) as i64,
    );
    sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-097 CP-D13/self-host-parity GREEN 2026-06-19] the degenerate one-cell control plane \
         (cell-self, fr-par; registry cell_count={}) ran the IDENTICAL code path: place (shared \
         PlacementService) → home_cell=cell-self; discover/placement_of returned 'this cell' \
         (member_cells single-element); the one cell's gateway ACCEPTED every tenant (misroute_count=0, \
         cross_tenant_reads=0); CP-D3 held - an out-of-region write was REJECTED, an in-region write \
         admitted (out_of_region_writes_admitted={}); residency_verify GREEN on the install's own data \
         ({} M1 stores attested, region_mismatches=0, signature={}…, verifies). NO self-host fork - the \
         answers came from the SHARED Registry/PlacementService/CellGateway/residency_verify. \
         Managed-fleet-only (cross-cell tenants, fleet deploy waves) is N/A by definition, NOT a gap. \
         CP-D2/CP-D4 re-confirmed in the self_tenant band P-CP-23.",
        sh.registry().cell_count(),
        out_of_region_writes_admitted,
        attestation.store_regions.len(),
        &attestation.signature[..attestation.signature.len().min(22)],
    );
}

#[test]
fn self_host_parity_gate_is_not_vacuous() {
    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, 1);
    assert!(
        !sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0)).is_green(),
        "a residency/cross-tenant breach on the degenerate cell MUST read RED - the self-host-parity \
         zero is a real tripwire"
    );
}
