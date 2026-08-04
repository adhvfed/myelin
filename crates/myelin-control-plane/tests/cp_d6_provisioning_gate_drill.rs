use myelin_control_plane::place::CounterMinter;
use myelin_control_plane::{
    Capacity, Cell, CellStatus, IsolationKind, PlacementService, ProvisionFailure,
    ProvisioningGate, ProvisioningSignals, Registry,
};
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
        status: CellStatus::Provisioning,
        isolation_kind: IsolationKind::Pool,
        capacity: Capacity {
            tenants_max: 1000,
            write_qps_max: 5000,
            storage_bytes_max: 1 << 40,
        },
        utilisation: 0,
        version: 1,
        endpoint: format!("cell.eu-west.{id}.myelin.eu"),
    }
}

fn reachable_archiver(tail: u64) -> ContinuousArchiver {
    let mut arch = ContinuousArchiver::new();
    arch.archive_segment(WalSegment {
        end_offset: 0,
        committed_at: 0,
    })
    .unwrap();
    arch.take_base_backup(1);
    arch.archive_segment(WalSegment {
        end_offset: tail,
        committed_at: 10,
    })
    .unwrap();
    arch
}

fn live_kms() -> KmsEngine {
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(TenantId::from_token("acme"), region()));
    kms.ensure_dek(&TenantId::from_token("acme"), &region(), KeyClass::Tenant)
        .unwrap();
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

#[test]
fn cp_d6_no_traffic_to_an_unverified_cell() {
    let gate = ProvisioningGate::new();
    let cell_id = CellId::from_token("cell-eu-west-1");

    let mut reg = Registry::new();
    reg.insert_cell(fresh_cell("cell-eu-west-1"));
    let mut signals = ProvisioningSignals::default();

    let missing = ContentHash::blake3(b"never-restored");
    let corrupt_rows = vec![WalRow {
        id: "corrupt".into(),
        written_at: 90,
        blob_ref: Some(missing),
    }];
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

    let red = gate.provision_cell(
        &mut reg,
        &cell_id,
        &red_inputs,
        &ready_surface(),
        &mut signals,
    );
    assert!(
        !red.is_active(),
        "a cell with an unwhole backup must NOT go Active (the gate is RED)"
    );
    assert!(
        matches!(
            red.failure(),
            Some(ProvisionFailure::RestoreVerifyFailed { .. })
        ),
        "the failure names restore-verify (the silent-data-loss floor): {red:?}"
    );
    assert_eq!(reg.cell(&cell_id).unwrap().status, CellStatus::Provisioning);
    let placer = PlacementService::new(CounterMinter::new());
    assert!(
        placer
            .place(&mut reg, &region(), IsolationKind::Pool, "acme")
            .is_err(),
        "place refuses an unverified (Provisioning) cell - no traffic"
    );
    assert_eq!(
        reg.placement_count(),
        0,
        "0 tenants placed on the unverified cell"
    );
    assert_eq!(ProvisioningGate::tenants_on_unverified_cells(&reg), 0);

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

    let green = gate.provision_cell(
        &mut reg,
        &cell_id,
        &green_inputs,
        &ready_surface(),
        &mut signals,
    );
    assert!(
        green.is_active(),
        "a whole + ready cell ACTIVATES (the gate is GREEN)"
    );
    let artifact = green
        .green_artifact()
        .expect("the restore-verify green artifact is carried");
    assert_eq!(artifact.restored_to_offset, 100);
    assert_eq!(artifact.checksum_mismatches, 0);
    assert_eq!(artifact.cross_seam_mismatches, 0);
    assert_eq!(artifact.resurrected_subjects, 0);
    assert_eq!(reg.cell(&cell_id).unwrap().status, CellStatus::Active);

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

    let mut sig = SignalSource::new();
    sig.set_scalar(
        SignalName::RestoreCrossSeamMismatch,
        artifact.cross_seam_mismatches as i64,
    );
    sig.assert_signal(SignalName::RestoreCrossSeamMismatch, Predicate::Eq(0))
        .expect_green();
    sig.set_scalar(
        SignalName::Readiness,
        ready_surface().readiness().verdict.gauge(),
    );
    sig.assert_signal(SignalName::Readiness, Predicate::Eq(1))
        .expect_green();

    println!(
        "[P-083 CP-D6 GREEN 2026-06-19] cell-provisioning gating: a fresh cell stays `provisioning` \
         until it passes restore-verify (Storage 11.5, the permanent STOR-D1 gate) + readiness - a \
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
    let key_ref = kms
        .ensure_dek(&tenant, &region(), KeyClass::Tenant)
        .unwrap();
    assert!(
        kms.resolve_dek(&key_ref, &region()).is_ok(),
        "the DEK resolves while the tenant is live"
    );

    let mut signals = ProvisioningSignals::default();
    assert!(
        gate.decommission_tenant(&mut reg, &kms, &tenant, &region(), &mut signals),
        "a live tenant's KEK is present to crypto-shred"
    );
    assert!(
        kms.resolve_dek(&key_ref, &region()).is_err(),
        "after decommission the source key is DESTROYED - the DEK is unrecoverable (crypto-shred)"
    );
    assert_eq!(signals.tenants_decommissioned, 1);

    println!(
        "[P-083 CP-D6 GREEN 2026-06-19] tenant decommission crypto-shreds the tenant KEK (Storage \
         11.3) - the source key is destroyed; the DEK no longer resolves. {} tenant(s) decommissioned.",
        signals.tenants_decommissioned,
    );
}
