//! P-CP-11 (global P-083) GATE / DRILL — **Cell-provisioning gating: restore-verify + readiness
//! before a cell goes `active` (CP-D6)** — dated green artifact.
//!
//! **The GATE (testing-strategy CP-D6 / tenancy-and-control-plane.md §7.2):** provision a fresh cell
//! → it passes restore-verify (Storage 11.5) **+** readiness BEFORE accepting any tenant; a failing
//! cell stays `provisioning` (gets no traffic). Telemetry: the restore-verify + readiness gate result;
//! **0 tenants placed on an unverified cell**.
//!
//! **Gate-invariant note (master §1 Tier 1):** this prompt's "place real data" capability rests on a
//! GREEN STOR-D1 — restore-verify (Storage P-ST-06/P-061) must be green FIRST. The cell-readiness gate
//! literally CONSUMES the storage [`RestoreVerifyGate`] (contract 11.5); the durability gate is not
//! re-implemented (one durability gate in the platform, coherence EI-01 §7).
//!
//! **FLOOR (named, VISION §3):** provisioning runs as a **SCRIPTED procedure** on this M1 floor (the
//! `DurableExecutor`, `myelin-flow` 9.1, is NOT yet available). The durable-workflow promotion — the
//! SAME gating, now crash-safe + resumable under the engine — is **P-CP-22**'s re-confirmation. The
//! *gating* is M1 and complete; the *durability* of the procedure is the M2 follow-on.
//!
//! This drill proves the gate can go RED (a cell with an unwhole backup stays `provisioning`; EI-01 §3
//! — a drill that cannot go red is not a gate) AND green (a whole + ready cell activates), and emits
//! the gate result on the SAME [`SignalSource`] every drill uses (observability is part of the pass).

use myelin_control_plane::{
    CellStatus, Capacity, Cell, IsolationKind, PlacementService, ProvisionFailure, ProvisioningGate,
    ProvisioningSignals, Registry,
};
use myelin_control_plane::place::CounterMinter;
use myelin_harness::{Predicate, SignalName, SignalSource};
use myelin_storage::{
    ContentHash, ContinuousArchiver, ErasureLedger, GateInputs, KekId, KeyClass, KmsEngine,
    RestoredObject, SourceLog, WalRow, WalSegment,
};
use myelin_substrate::{CriticalDependencies, HealthTable, MetricsHealthSurface};
use myelin_tenancy::{CellId, Region, TenantId};

fn region() -> Region {
    Region::new("eu-west")
}

fn fresh_cell(id: &str) -> Cell {
    Cell {
        cell_id: CellId::from_token(id),
        region: region(),
        status: CellStatus::Provisioning, // a FRESH cell starts Provisioning.
        isolation_kind: IsolationKind::Pool,
        capacity: Capacity { tenants_max: 1000, write_qps_max: 5000, storage_bytes_max: 1 << 40 },
        utilisation: 0,
        version: 1,
        endpoint: format!("cell.eu-west.{id}.myelin.eu"),
    }
}

fn reachable_archiver(tail: u64) -> ContinuousArchiver {
    let mut arch = ContinuousArchiver::new();
    arch.archive_segment(WalSegment { end_offset: 0, committed_at: 0 }).unwrap();
    arch.take_base_backup(1);
    arch.archive_segment(WalSegment { end_offset: tail, committed_at: 10 }).unwrap();
    arch
}

fn live_kms() -> KmsEngine {
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(TenantId::from_token("acme"), region()));
    kms.ensure_dek(&TenantId::from_token("acme"), &region(), KeyClass::Tenant).unwrap();
    kms
}

fn ready_surface() -> MetricsHealthSurface<HealthTable> {
    let surface = MetricsHealthSurface::new(
        CriticalDependencies::new(["oltp", "blob", "kms"]),
        HealthTable::new(),
    );
    surface.mark_started();
    surface
}

/// **THE DRILL (dated green artifact): provision a fresh cell → it stays `provisioning` on a failed
/// restore-verify (0 tenants placed), then ACTIVATES on a whole backup + readiness — 0 tenants on an
/// unverified cell throughout.**
#[test]
fn cp_d6_no_traffic_to_an_unverified_cell() {
    let gate = ProvisioningGate::new();
    let cell_id = CellId::from_token("cell-eu-west-1");

    // ── RED leg: a fresh cell whose backup is NOT whole stays `provisioning` (no traffic). ──
    let mut reg = Registry::new();
    reg.insert_cell(fresh_cell("cell-eu-west-1"));
    let mut signals = ProvisioningSignals::default();

    // A CORRUPT restore: a row references a blob the restore did not bring back (a dangling ref).
    let missing = ContentHash::blake3(b"never-restored");
    let corrupt_rows = vec![WalRow { id: "corrupt".into(), written_at: 90, blob_ref: Some(missing) }];
    let no_objects: Vec<RestoredObject> = vec![];
    let source = SourceLog::new();
    let kms = live_kms();
    let ledger = ErasureLedger::new();
    let arch = reachable_archiver(300);
    let red_inputs = GateInputs {
        archiver: &arch,
        target: 100,
        rows: &corrupt_rows,
        objects: &no_objects,
        source: &source,
        kms: &kms,
        erasure_ledger: &ledger,
    };

    let red = gate.provision_cell(&mut reg, &cell_id, &red_inputs, &ready_surface(), &mut signals);
    assert!(!red.is_active(), "a cell with an unwhole backup must NOT go Active (the gate is RED)");
    assert!(
        matches!(red.failure(), Some(ProvisionFailure::RestoreVerifyFailed { .. })),
        "the failure names restore-verify (the silent-data-loss floor): {red:?}"
    );
    // The cell stayed `provisioning` — and `place` cannot route to it.
    assert_eq!(reg.cell(&cell_id).unwrap().status, CellStatus::Provisioning);
    let placer = PlacementService::new(CounterMinter::new());
    assert!(
        placer.place(&mut reg, &region(), IsolationKind::Pool, "acme").is_err(),
        "place refuses an unverified (Provisioning) cell — no traffic"
    );
    // The headline CP-D6 zero: 0 tenants placed on the unverified cell.
    assert_eq!(reg.placement_count(), 0, "0 tenants placed on the unverified cell");
    assert_eq!(ProvisioningGate::tenants_on_unverified_cells(&reg), 0);

    // ── GREEN leg: the same cell, now with a WHOLE backup + readiness, ACTIVATES. ──
    let objects = vec![RestoredObject::integral(b"cell-blob".to_vec())];
    let mut whole_source = SourceLog::new();
    whole_source.append(100, "r100");
    let whole_rows = vec![WalRow {
        id: "r100".into(),
        written_at: 100,
        blob_ref: Some(objects[0].content_address.clone()),
    }];
    let green_inputs = GateInputs {
        archiver: &arch,
        target: 100,
        rows: &whole_rows,
        objects: &objects,
        source: &whole_source,
        kms: &kms,
        erasure_ledger: &ledger,
    };

    let green = gate.provision_cell(&mut reg, &cell_id, &green_inputs, &ready_surface(), &mut signals);
    assert!(green.is_active(), "a whole + ready cell ACTIVATES (the gate is GREEN)");
    let artifact = green.green_artifact().expect("the restore-verify green artifact is carried");
    assert_eq!(artifact.restored_to_offset, 100);
    assert_eq!(artifact.checksum_mismatches, 0);
    assert_eq!(artifact.cross_seam_mismatches, 0);
    assert_eq!(artifact.resurrected_subjects, 0);
    assert_eq!(reg.cell(&cell_id).unwrap().status, CellStatus::Active);

    // Now a tenant places onto the VERIFIED cell — and is NOT on an unverified cell.
    placer
        .place(&mut reg, &region(), IsolationKind::Pool, "acme")
        .expect("the activated cell accepts the placement");
    assert_eq!(reg.placement_count(), 1);
    assert_eq!(
        ProvisioningGate::tenants_on_unverified_cells(&reg),
        0,
        "the placed tenant is on a verified (Active) cell"
    );

    assert_eq!(signals.cells_activated, 1);
    assert_eq!(signals.cells_held_provisioning, 1);

    // ── Emit the CP-D6 gate result on the SAME SignalSource every drill uses (observability is part
    // of the pass, EI-01 §3). The restore-verify cross-seam mismatch is 0 (the green leg) and the
    // cell's readiness gauge is 1 (it passed the readiness probe). ──
    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::RestoreCrossSeamMismatch, artifact.cross_seam_mismatches as i64);
    sig.assert_signal(SignalName::RestoreCrossSeamMismatch, Predicate::Eq(0)).expect_green();
    sig.set_scalar(SignalName::Readiness, ready_surface().readiness().verdict.gauge());
    sig.assert_signal(SignalName::Readiness, Predicate::Eq(1)).expect_green();

    println!(
        "[P-083 CP-D6 GREEN 2026-06-19] cell-provisioning gating: a fresh cell stays `provisioning` \
         until it passes restore-verify (Storage 11.5, the permanent STOR-D1 gate) + readiness — a \
         cell with an unwhole backup got 0 tenants ({} held provisioning); a whole + ready cell \
         ACTIVATED ({} activated) and {} restore_verify cross-seam mismatch(es), readiness=1. 0 \
         tenants on an unverified cell. FLOOR: scripted provisioning (the durable-workflow promotion \
         under myelin-flow's DurableExecutor, 9.1, is P-CP-22). Place-real-data rests on a green \
         STOR-D1.",
        signals.cells_held_provisioning,
        signals.cells_activated,
        artifact.cross_seam_mismatches,
    );
}

/// **Tenant decommission crypto-shreds the tenant KEK (Storage 11.3).** A decommissioned tenant's KEK
/// is destroyed — the source key is gone (the tenant-offboard lever). The drill proves the DEK no
/// longer resolves after decommission (the gravest-failure inverse: the key is dead).
#[test]
fn cp_d6_decommission_crypto_shreds_the_kek() {
    let gate = ProvisioningGate::new();
    let mut reg = Registry::new();
    let mut active = fresh_cell("cell-eu-west-1");
    active.status = CellStatus::Active;
    reg.insert_cell(active);

    let tenant = TenantId::from_token("acme");
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant.clone(), region()));
    let key_ref = kms.ensure_dek(&tenant, &region(), KeyClass::Tenant).unwrap();
    assert!(kms.resolve_dek(&key_ref, &region()).is_ok(), "the DEK resolves while the tenant is live");

    let mut signals = ProvisioningSignals::default();
    assert!(
        gate.decommission_tenant(&mut reg, &kms, &tenant, &region(), &mut signals),
        "a live tenant's KEK is present to crypto-shred"
    );
    assert!(
        kms.resolve_dek(&key_ref, &region()).is_err(),
        "after decommission the source key is DESTROYED — the DEK is unrecoverable (crypto-shred)"
    );
    assert_eq!(signals.tenants_decommissioned, 1);

    println!(
        "[P-083 CP-D6 GREEN 2026-06-19] tenant decommission crypto-shreds the tenant KEK (Storage \
         11.3) — the source key is destroyed; the DEK no longer resolves. {} tenant(s) decommissioned.",
        signals.tenants_decommissioned,
    );
}
