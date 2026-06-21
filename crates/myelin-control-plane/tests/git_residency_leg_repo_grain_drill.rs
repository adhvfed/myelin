//! P-CP-15 (global P-250) GATE / DRILL — **the GIT residency leg of CP-D2/CP-D3 (rides GIT-D8) at
//! REPO grain** — dated green artifact.
//!
//! **The GATE (tenancy-and-control-plane.md §5.2 / §7.3; P-CP-15 GATE):** cross-tenant repo access via
//! a repo-grain misroute → **0 cross-tenant read**; a relocated repo's clone/push is corrected to the
//! current cell-endpoint (the **misroute redirect**). Telemetry: `misroute_count` increments, the
//! `CrossTenantCount` projection == 0.
//!
//! **The most load-bearing zero (EI-01 §2):** a cross-tenant repo read / a repo leaving its region is
//! stop-the-bleeding. The structural defence here is `placement_of(repo)` (the repo-grain layer-4
//! routing answer): the git-wire gateway asks the control-plane authoritative answer which cell homes
//! the repo and rejects+redirects if that is not THIS cell — BEFORE any repo content is touched, so the
//! 0 is by construction. The repo's `region` is read from its TENANT placement (the residency pin), so
//! a repo NEVER reports / relocates to a region different from its tenant's.
//!
//! **This drill proves the gate can go RED** (a relocated/cross-tenant repo request IS rejected +
//! redirected + audited) **AND green** (the home cell serves its own repo; a relocation is corrected to
//! the new cell), and emits the result on the SAME [`SignalSource`] every drill uses (observability is
//! part of the pass, EI-01 §3).
//!
//! **FLOOR (named, VISION §3):** the relocation MECHANISM (the actual byte move) is the M5 build
//! (P-CP-22) — this drill exercises the live routing ANSWER + the redirect-on-relocation PROPERTY (the
//! stored-fact `cell_id` flip + the gateway redirect). Multi-cell repo homing is M5 (P-CP-19/P-CP-20).

use myelin_control_plane::{
    Capacity, Cell, CellGateway, CellStatus, GatewayReject, IsolationKind, Misroute,
    MisrouteAuditRecord, PlacementStatus, Registry, StorageGroup, TenantPlacement,
};
use myelin_harness::{Predicate, SignalName, SignalSource};
use myelin_tenancy::{ArtifactRef, CellId, Region, TenantId};

fn cell(id: &str, region: &str) -> Cell {
    Cell {
        cell_id: CellId::from_token(id),
        region: Region::new(region),
        status: CellStatus::Active,
        isolation_kind: IsolationKind::Pool,
        capacity: Capacity { tenants_max: 1000, write_qps_max: 5000, storage_bytes_max: 1 << 40 },
        utilisation: 10,
        version: 1,
        endpoint: format!("cell.{region}.{id}.myelin.eu"),
    }
}

fn place(reg: &mut Registry, tenant: &str, region: &str, home: &str, slug: &str) {
    reg.place_tenant(TenantPlacement {
        tenant_id: TenantId::from_token(tenant),
        region: Region::new(region),
        home_cell: CellId::from_token(home),
        isolation_tier: IsolationKind::Pool,
        slug: slug.into(),
        status: PlacementStatus::Active,
        member_cells: vec![CellId::from_token(home)],
    })
    .expect("a single-region placement is admitted");
}

fn repo(tenant: &str, id: &str) -> ArtifactRef {
    ArtifactRef(format!("myelin://{tenant}/git/repo/{id}"))
}

/// **THE GIT RESIDENCY LEG (dated green artifact): repo-grain misroute → 0 cross-tenant + relocation
/// redirect.** cell-w-1 (eu-west) homes ACME's repo `web`; cell-w-2 (eu-west) homes BETA's repo `api`;
/// cell-n-1 is in eu-north (a cross-region cell). The drill proves: (1) the home cell serves its own
/// repo; (2) a cross-tenant repo request to the wrong cell is rejected, 0 cross-tenant read; (3) a
/// relocation (cell-w-1 → cell-w-2, same region) corrects an old-cell clone to the new cell-endpoint;
/// (4) a cross-region relocation is REJECTED (the residency pin at repo grain).
#[test]
fn git_residency_leg_repo_grain() {
    let mut reg = Registry::new();
    reg.insert_cell(cell("cell-w-1", "eu-west"));
    reg.insert_cell(cell("cell-w-2", "eu-west"));
    reg.insert_cell(cell("cell-n-1", "eu-north")); // a cross-region cell (the residency-pin foil).
    place(&mut reg, "01J0ACME", "eu-west", "cell-w-1", "acme");
    place(&mut reg, "01J0BETA", "eu-west", "cell-w-2", "beta");
    reg.register_repo(&repo("01J0ACME", "web"), StorageGroup::from_token("pack-0"))
        .expect("ACME's repo on its home cell");
    reg.register_repo(&repo("01J0BETA", "api"), StorageGroup::from_token("pack-0"))
        .expect("BETA's repo on its home cell");

    // ── GREEN leg: the home cell (cell-w-1) serves ACME's OWN repo — no misroute, region == eu-west. ──
    let home = CellGateway::new(CellId::from_token("cell-w-1"));
    let served = home
        .route_repo(&reg, &repo("01J0ACME", "web"))
        .expect("the home cell serves its own repo (the gate is GREEN)");
    assert_eq!(served.cell_id.as_str(), "cell-w-1");
    assert_eq!(served.region.as_str(), "eu-west", "the repo's region is its TENANT's region (the pin)");
    assert_eq!(served.group.as_str(), "pack-0");
    assert_eq!(home.misroute_count(), 0);
    assert_eq!(home.cross_tenant_reads(), 0);

    // ── RED leg 1 — CROSS-TENANT: cell-w-2 (BETA's home) receives a request for ACME's repo → a
    //    repo-grain misroute. Rejected (NOT proxied) + redirected to cell-w-1 + audited, 0 cross-cell. ──
    let wrong = CellGateway::new(CellId::from_token("cell-w-2"));
    let reject = wrong
        .route_repo(&reg, &repo("01J0ACME", "web"))
        .expect_err("cell-w-2 does not home ACME's repo → REJECTED (the gate is RED for the spoof)");
    assert_eq!(
        reject,
        GatewayReject::Misroute(Misroute {
            tenant_id: TenantId::from_token("01J0ACME"),
            correct_cell: CellId::from_token("cell-w-1"),
            correct_cell_endpoint: "cell.eu-west.cell-w-1.myelin.eu".into(),
        }),
        "the repo-grain misroute redirects to the HOME cell-endpoint (not proxied)"
    );
    assert_eq!(wrong.audit().count(), 1, "the cross-tenant repo misroute is audited (PII-free)");
    assert_eq!(
        wrong.audit().records()[0],
        MisrouteAuditRecord {
            tenant_id: TenantId::from_token("01J0ACME"),
            received_by_cell: CellId::from_token("cell-w-2"),
            home_cell: Some(CellId::from_token("cell-w-1")),
        }
    );
    let cross_tenant_reads_spoof = wrong.cross_tenant_reads();
    assert_eq!(wrong.misroute_count(), 1, "misroute_count increments on a repo-grain misroute");
    assert_eq!(cross_tenant_reads_spoof, 0, "0 cross-tenant/cross-cell repo rows read (the GIT zero)");

    // ── RED leg 2 — RELOCATION: relocate ACME's repo cell-w-1 → cell-w-2 (SAME region). A git-wire
    //    clone still pointing at the OLD cell (cell-w-1) is corrected to the new cell-endpoint. ──
    reg.relocate_repo(&repo("01J0ACME", "web"), CellId::from_token("cell-w-2"), StorageGroup::from_token("pack-9"))
        .expect("a same-region relocation is admitted (a stored-fact flip, NOT a hash recompute)");
    let stale = CellGateway::new(CellId::from_token("cell-w-1")); // the OLD home, now stale.
    let redirect_err = stale
        .route_repo(&reg, &repo("01J0ACME", "web"))
        .expect_err("the OLD cell no longer homes the relocated repo → REJECTED + REDIRECTED");
    let GatewayReject::Misroute(redirect) = redirect_err else { panic!("expected a relocation redirect") };
    assert_eq!(redirect.correct_cell.as_str(), "cell-w-2", "redirect → the CURRENT (relocated) cell");
    assert_eq!(redirect.correct_cell_endpoint, "cell.eu-west.cell-w-2.myelin.eu");
    assert_eq!(stale.cross_tenant_reads(), 0);
    // The redirect is SERVED by the current cell (a redirect, never a proxy) — region still eu-west.
    let current = CellGateway::new(redirect.correct_cell.clone());
    let re_served = current
        .route_repo(&reg, &repo("01J0ACME", "web"))
        .expect("the current cell serves the redirected clone");
    assert_eq!(re_served.cell_id.as_str(), "cell-w-2");
    assert_eq!(re_served.group.as_str(), "pack-9", "the group moved to the target cell");
    assert_eq!(re_served.region.as_str(), "eu-west", "region UNCHANGED — same-region move (the pin)");
    assert_eq!(current.misroute_count(), 0, "the current cell does not misroute its own repo");

    // ── RED leg 3 — RESIDENCY PIN: a CROSS-REGION relocation target (cell-n-1, eu-north) is REJECTED.
    //    A repo cannot leave its tenant's region — 0 repos cross the residency boundary. ──
    let cross_region = reg
        .relocate_repo(&repo("01J0ACME", "web"), CellId::from_token("cell-n-1"), StorageGroup::from_token("g"))
        .expect_err("a cross-region relocation is rejected (the residency pin at repo grain)");
    assert!(
        cross_region.to_string().contains("residency pin"),
        "loud residency reason: {cross_region}"
    );
    // The repo did NOT move — still on cell-w-2, still in eu-west.
    let after_reject = reg.placement_of_repo(&repo("01J0ACME", "web")).expect("still placed");
    assert_eq!(after_reject.cell_id.as_str(), "cell-w-2");
    assert_eq!(after_reject.region.as_str(), "eu-west", "0 repos cross the residency boundary");

    // ── Emit the GIT-residency gate result on the SAME SignalSource every drill uses (observability is
    //    part of the pass, EI-01 §3): the CrossTenantCount projection == 0. ──
    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, cross_tenant_reads_spoof as i64);
    sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0)).expect_green();

    println!(
        "[P-250 GIT-residency (repo-grain) GREEN 2026-06-21] placement_of(repo) LIVE: the home cell \
         served its OWN repo (region=eu-west, the TENANT pin); a CROSS-TENANT repo request to cell-w-2 \
         was REJECTED (not proxied) + REDIRECTED to cell.eu-west.cell-w-1.myelin.eu + AUDITED, \
         cross_tenant/cross_cell repo reads SERVED={} (the GIT zero); a RELOCATION (cell-w-1 → \
         cell-w-2, same region, a stored-fact flip — NO hash recompute) corrected a stale clone to \
         cell.eu-west.cell-w-2.myelin.eu; a CROSS-REGION relocation was REJECTED (the residency pin at \
         repo grain — 0 repos cross the boundary). FLOOR: the relocation byte-move mechanism is M5 \
         (P-CP-22); multi-cell repo homing is M5 (P-CP-19/P-CP-20).",
        cross_tenant_reads_spoof,
    );
}

/// **The gate is NOT vacuous: a cross-tenant repo read SERVED would read RED.** Proves the GIT
/// residency zero is a real tripwire — if a (hypothetical) code path served a foreign repo,
/// `CrossTenantCount > 0` would fail the predicate. (The structural defence pins the real value to 0;
/// this asserts the assertion itself is load-bearing — EI-01 §3, a gate that cannot go red is not a
/// gate. Mirrors `cp_d2_gate_is_not_vacuous`.)
#[test]
fn git_repo_grain_gate_is_not_vacuous() {
    let mut sig = SignalSource::new();
    // A hypothetical regression that SERVED one cross-tenant repo read.
    sig.set_scalar(SignalName::CrossTenantCount, 1);
    assert!(
        !sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0)).is_green(),
        "a served cross-tenant repo read MUST read RED — the GIT residency zero is a real tripwire"
    );
}
