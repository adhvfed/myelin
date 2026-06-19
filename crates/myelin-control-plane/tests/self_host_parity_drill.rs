//! P-CP-13 (global P-097) GATE / DRILL — **Self-host parity: the degenerate one-cell control plane
//! runs the identical code path** — dated green artifact.
//!
//! **The GATE (testing-strategy self-host-parity leg / tenancy-and-control-plane.md §10):** the
//! degenerate one-cell control plane runs the IDENTICAL `discover`/`place`/`placement_of`/
//! `residency_verify` code path over a one-row registry; the `residency-pin` lint holds; **CP-D3
//! (residency-pin rejects an out-of-region write) runs green on the degenerate cell**. Telemetry: the
//! `residency-attestation` green on the one-cell install.
//!
//! **The load-bearing property (architecture §10, ADR-11.1):** a self-hosted Myelin install is EXACTLY
//! one cell of identical artifacts (same monorepo build). The control plane is *degenerate*
//! (discovery/placement trivially return "this cell", the registry is a one-row local table) but the
//! SAME code path runs — there is NO self-host fork. The customer's data stays in the customer's region
//! by the SAME write-boundary check (`residency-pin`, layer 3). Managed-fleet-only features (cross-cell
//! tenants, fleet deploy waves) are N/A for self-host **by definition** — not a gap, the model.
//!
//! **This drill proves** the degenerate cell runs the shared API end-to-end (place → discover →
//! placement_of → gateway route → residency_verify all resolve to "this cell"), that CP-D3 holds on the
//! degenerate cell (an out-of-region write is REJECTED at the boundary; an in-region write is admitted),
//! and that `residency_verify` is green on the install's own data (`region_mismatches == 0`). It emits
//! the green artifact on the SAME [`SignalSource`] every drill uses (observability is part of the pass,
//! EI-01 §3).
//!
//! **NO floor here (P-CP-13).** Managed-fleet-only is N/A by definition (named, not a gap); CP-D2
//! (misroute) + CP-D4 (blast-radius) are re-confirmed in the dogfood band (P-CP-23). This drill is
//! DB-free (the degenerate registry is in-process, exactly like the fleet's CP-D2/CP-D3/four-layer
//! drills) — `cargo build --workspace` stays DB-free; the live store-layer residency twin is proven in
//! the storage `stor_d5_cross_region_egress` integration drill (P-CP-12).

use myelin_control_plane::{
    CounterMinter, DegenerateControlPlane, IsolationKind, PlacementService, ResidencySigningKey,
    ResidencyStoreClass,
};
use myelin_harness::{Predicate, SignalName, SignalSource};
use myelin_tenancy::{CellId, Region, TenantId};

/// **THE SELF-HOST-PARITY DRILL (dated green artifact): the degenerate one-cell control plane runs the
/// IDENTICAL `discover`/`place`/`placement_of`/`residency_verify` code path over a one-row registry;
/// the `residency-pin` lint holds + CP-D3 runs green; `residency_verify` is green on the install's own
/// data.**
#[test]
fn self_host_parity_degenerate_one_cell_identical_code_path() {
    // A self-hosted install in the customer's region (MYELIN_REGION=fr-par in the dev/prod stack) — a
    // degenerate one-cell control plane. The SAME myelin-control-plane code, one Active cell.
    let region = Region::new("fr-par");
    let mut sh = DegenerateControlPlane::bootstrap(CellId::from_token("cell-self"), region.clone());

    // ── ONE-ROW registry (architecture §10): exactly one cell — the install's own. ──
    assert_eq!(sh.registry().cell_count(), 1, "a self-host install is EXACTLY one cell");
    assert_eq!(sh.cell().region.as_str(), "fr-par", "pinned to the install's region");

    // ── `place` runs the IDENTICAL two-phase-signup code path (PlacementService::place — no fork). ──
    let service = PlacementService::new(CounterMinter::new());
    let answer = sh
        .place(&service, IsolationKind::Pool, "self-host-tenant")
        .expect("the one Active cell is eligible → placed via the SHARED PlacementService::place");
    let tenant: TenantId = answer.tenant_id.clone();
    assert_eq!(answer.home_cell.as_str(), "cell-self", "placed on the install's own cell");

    // ── `discover` / `placement_of` return "this cell" (the SHARED Registry methods). ──
    let discovered = sh.discover_cell(&tenant).expect("a placed tenant discovers");
    assert_eq!(discovered.as_str(), "cell-self", "discover returns 'this cell'");
    let placement = sh.placement_of(&tenant).expect("a placed tenant has a placement_of answer");
    assert_eq!(placement.home_cell.as_str(), "cell-self", "placement_of returns 'this cell'");
    assert_eq!(
        placement.member_cells.len(),
        1,
        "member_cells is single-element — multi-cell is N/A for self-host by definition (the model)"
    );

    // ── The one cell's gateway (layer 4) ACCEPTS every tenant it homes; 0 cross-tenant reads. ──
    let gw = sh.gateway();
    let served = gw.route(sh.registry(), &tenant).expect("the one cell homes (and serves) every tenant");
    assert_eq!(served.home_cell.as_str(), "cell-self");
    assert_eq!(gw.misroute_count(), 0, "no misroute on a one-cell install");
    let cross_tenant_reads = gw.cross_tenant_reads();
    assert_eq!(cross_tenant_reads, 0, "0 cross-tenant reads (the CP-D2 zero) on the degenerate cell");

    // ── CP-D3 ON THE DEGENERATE CELL: the residency-pin write boundary REJECTS an out-of-region write
    //    (the SAME layer-3 check a fleet cell runs). An in-region write is admitted. ──
    sh.cp_d3_residency_pin_holds(&Region::new("eu-north"))
        .expect("the residency-pin holds on the degenerate cell (out-of-region write REJECTED)");
    // The no-cross-region-query-path property holds on the degenerate cell (the SAME four-layer check).
    sh.assert_no_cross_region_query_path(&tenant, &Region::new("us-east"))
        .expect("the one cell serves its tenant and that data stays in fr-par (no cross-region path)");
    let out_of_region_writes_admitted = sh.four_layer().write_boundary().out_of_region_writes_admitted();
    assert_eq!(out_of_region_writes_admitted, 0, "0 out-of-region writes admitted (CP-D3 on the degenerate cell)");

    // ── `residency_verify` GREEN on the install's own data (the SHARED free function — no fork). ──
    let key = ResidencySigningKey::from_bytes([0x5eu8; 32]);
    let attestation = sh
        .residency_verify_own_data(&tenant, &key)
        .expect("residency_verify is GREEN on the self-host cell's own data");
    assert_eq!(attestation.region.as_str(), "fr-par", "every M1 store reported the install's region");
    assert_eq!(
        attestation.store_regions.len(),
        ResidencyStoreClass::M1_SET.len(),
        "every M1 store attested"
    );
    assert!(attestation.verify(&key), "the green residency-attestation verifies (0 mismatches)");

    // ── Emit the green artifact on the SAME SignalSource every drill uses (observability is part of the
    //    pass, EI-01 §3). The self-host-parity zero is the sum of cross-region/cross-tenant violations
    //    on the degenerate cell (out-of-region writes admitted + cross-tenant reads) — pinned to 0. ──
    let mut sig = SignalSource::new();
    sig.set_scalar(
        SignalName::CrossTenantCount,
        (out_of_region_writes_admitted + cross_tenant_reads) as i64,
    );
    sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0)).expect_green();

    println!(
        "[P-097 CP-D13/self-host-parity GREEN 2026-06-19] the degenerate one-cell control plane \
         (cell-self, fr-par; registry cell_count={}) ran the IDENTICAL code path: place (shared \
         PlacementService) → home_cell=cell-self; discover/placement_of returned 'this cell' \
         (member_cells single-element); the one cell's gateway ACCEPTED every tenant (misroute_count=0, \
         cross_tenant_reads=0); CP-D3 held — an out-of-region write was REJECTED, an in-region write \
         admitted (out_of_region_writes_admitted={}); residency_verify GREEN on the install's own data \
         ({} M1 stores attested, region_mismatches=0, signature={}…, verifies). NO self-host fork — the \
         answers came from the SHARED Registry/PlacementService/CellGateway/residency_verify. \
         Managed-fleet-only (cross-cell tenants, fleet deploy waves) is N/A by definition, NOT a gap. \
         CP-D2/CP-D4 re-confirmed in the dogfood band P-CP-23.",
        sh.registry().cell_count(),
        out_of_region_writes_admitted,
        attestation.store_regions.len(),
        &attestation.signature[..attestation.signature.len().min(22)],
    );
}

/// **The gate is NOT vacuous: a residency/cross-tenant breach on the degenerate cell WOULD read RED.**
/// Proves the self-host-parity zero is a real tripwire — if the degenerate cell admitted an
/// out-of-region write (or served a foreign tenant), the predicate would fail. A gate that cannot go
/// red is not a gate (EI-01 §3).
#[test]
fn self_host_parity_gate_is_not_vacuous() {
    let mut sig = SignalSource::new();
    // A hypothetical breach on the degenerate cell: one out-of-region write admitted.
    sig.set_scalar(SignalName::CrossTenantCount, 1);
    assert!(
        !sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0)).is_green(),
        "a residency/cross-tenant breach on the degenerate cell MUST read RED — the self-host-parity \
         zero is a real tripwire"
    );
}
