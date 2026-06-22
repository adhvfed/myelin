//! P-CP-12 (global P-096) GATE / DRILL — **the four-layer region-pinning enforced end-to-end
//! (CP-D3 write-boundary runtime + CP-D2 e2e), no cross-region query path** — dated green artifact.
//!
//! This is the **M1→M2 go/no-go** artifact Tenancy owns. It exercises the four layers of §5.3
//! defence-in-depth wired together over one cell ([`FourLayerEnforcement`]):
//!
//! - **Layers 1+2** — region immutable + the placement invariant (a cross-region member cell is
//!   rejected at placement).
//! - **Layer 3** — the *runtime* `residency-pin` write boundary ([`ResidencyWriteBoundary`]): a
//!   `row.region ≠ cell.region` write is REJECTED at the boundary (the runtime CP-D3 mechanism; its
//!   live-DB twin is the Postgres RLS `WITH CHECK` proven in the storage `stor_d5_cross_region_egress`
//!   integration drill).
//! - **Layer 4** — the gateway rejects (does not proxy) a misrouted `tenant_id` (CP-D2, re-confirmed
//!   end-to-end here).
//!
//! And the headline property: **there is no cross-region query path for personal data** — a request
//! the cell serves can ONLY write in the cell's region.
//!
//! **The most load-bearing zeros (EI-01 §2):** 0 out-of-region writes admitted (layer 3 / STOR-D5),
//! 0 cross-tenant/cross-cell reads (layer 4 / CP-D2). A gate that cannot go RED is not a gate
//! (EI-01 §3) — each leg proves the RED (a breach is rejected) AND the GREEN (the legitimate path).
//!
//! **No engineering floor (P-CP-12):** the residency mechanism is fully built in M1. The
//! `[OPEN — LEGAL]` residuals — region-change-as-DSR (the legal classification; the engineering
//! discipline IS built, layer 1) and slug-PII screening (a data-governance review) — ship regardless
//! and are NOT engineering gates. The live cross-region-egress drill against the dev stack is the
//! storage `stor_d5_cross_region_egress` integration test (`--features integration`).

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

/// **THE P-CP-12 E2E DRILL (dated green artifact): the four layers, wired end-to-end, enforce
/// region-pinning with no cross-region query path for personal data.**
#[test]
fn four_layer_region_pinning_end_to_end() {
    // ── Layers 1+2: build the registry. Two cells in fr-par; ACME homed on cell-fr-1. A
    //    cross-region member cell is REJECTED at placement (the invariant fires). ──
    let mut reg = Registry::new();
    reg.insert_cell(cell("cell-fr-1", "fr-par"));
    reg.insert_cell(cell("cell-fr-2", "fr-par"));
    reg.insert_cell(cell("cell-de-1", "eu-central")); // a DIFFERENT region.

    // RED (layers 1+2): a placement with a cross-region member cell is rejected.
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

    // GREEN (layers 1+2): a single-region placement is admitted.
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

    // ── Wire the four layers over cell-fr-1 (the cell that homes ACME), pinned to fr-par. ──
    let gateway = CellGateway::new(CellId::from_token("cell-fr-1"));
    let enforcement = FourLayerEnforcement::new(&reg, gateway, Region::new("fr-par"));

    // ── Layer 3 (runtime CP-D3): a write in the cell's region is ADMITTED; an out-of-region write
    //    is REJECTED at the boundary (it never reaches the store). ──
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

    // ── Layer 4 (CP-D2 e2e): cell-fr-1 serves its OWN tenant (ACME); cell-fr-2 REJECTS a request
    //    for ACME (a misroute) — redirected, audited, 0 cross-cell read. ──
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

    // ── THE HEADLINE PROPERTY: no cross-region query path for personal data. The cell serves ACME
    //    (layer 4) AND ACME's data can ONLY be written in fr-par (layer 3) — a write in any other
    //    region is rejected. ──
    enforcement
        .assert_no_cross_region_query_path(
            &TenantId::from_token("01J0ACME"),
            &Region::new("eu-central"),
        )
        .expect("NO cross-region query path: ACME is served here and its data stays in fr-par");

    // The two go/no-go zeros hold end-to-end.
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

    // ── residency_verify attestation PASSES for the in-region tenant (the green CP-D3 artifact). ──
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

    // ── Emit the gate result on the SAME SignalSource every drill uses (observability is part of
    //    the pass, EI-01 §3): the CrossTenantCount projection carries the headline zero — here the
    //    SUM of the two go/no-go zeros (0 out-of-region writes + 0 cross-cell reads == 0). ──
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
         cross_tenant_reads={cross_tenant_reads}. NO engineering floor; [OPEN — LEGAL] \
         region-change-as-DSR + slug-PII-screening ship regardless. The live cross-region-egress \
         drill is storage stor_d5_cross_region_egress (--features integration, dev stack)."
    );
}

/// **The gate is NOT vacuous: an out-of-region write that WERE admitted would read RED.** Proves the
/// layer-3 zero is a real tripwire — the no-cross-region-query-path assertion catches an admit.
#[test]
fn cp_d3_runtime_gate_is_not_vacuous() {
    // A boundary whose pin is the cell's region; a write in a foreign region MUST be rejected.
    let boundary = ResidencyWriteBoundary::for_cell(Region::new("fr-par"));
    assert!(
        boundary.check_write(&Region::new("eu-central")).is_err(),
        "an out-of-region write MUST be rejected — a gate that cannot go red is not a gate"
    );

    // And at the signal level: a hypothetical admitted out-of-region write reads RED.
    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, 1); // 1 out-of-region write admitted (the breach).
    assert!(
        !sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
            .is_green(),
        "an admitted out-of-region write MUST read RED — the STOR-D5 zero is a real tripwire"
    );
}

/// **The no-cross-region-query-path assertion catches a cell holding a tenant it does not home
/// (layer 4)** — the assertion is sound at the routing boundary too, not only the write boundary.
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

    // cell-fr-2 does not home ACME → the assertion fails (it must not hold ACME's data).
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
