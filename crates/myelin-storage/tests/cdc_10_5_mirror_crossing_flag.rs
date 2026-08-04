use myelin_control_plane::{
    Capacity, Cell, CellStatus, IsolationKind, MirrorDecision, MirrorGate,
    MirrorTarget as CpMirrorTarget, PlacementStatus, Registry, TenantPlacement, TransferPolicy,
};
use myelin_storage::{
    verify_region_pinning, FsBlobStore, MirrorTelemetry, PushMirrorClass, PushMirrorTarget,
    ResidencyStoreClass, StoreSet,
};
use myelin_tenancy::{CellId, Region, TenantId};

struct TransferPolicyDouble {
    eu: std::collections::BTreeSet<String>,
}

impl TransferPolicyDouble {
    fn new() -> TransferPolicyDouble {
        let mut eu = std::collections::BTreeSet::new();
        for r in ["fr-par", "nl-ams", "de-fra", "no-osl"] {
            eu.insert(r.to_string());
        }
        TransferPolicyDouble { eu }
    }
}

impl TransferPolicy for TransferPolicyDouble {
    fn transfer_allowed(&self, target: &Region) -> bool {
        self.eu.contains(target.as_str())
    }
}

fn registry_with(tenant: &str, region: &str, home: &str) -> Registry {
    let mut reg = Registry::new();
    reg.insert_cell(Cell {
        cell_id: CellId::from_token(home),
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
        endpoint: format!("cell.{region}.{home}.myelin.eu"),
    });
    reg.place_tenant(TenantPlacement {
        tenant_id: TenantId::from_token(tenant),
        region: Region::new(region),
        home_cell: CellId::from_token(home),
        isolation_tier: IsolationKind::Pool,
        slug: tenant.into(),
        status: PlacementStatus::Active,
        member_cells: vec![CellId::from_token(home)],
    })
    .expect("a single-region placement is admitted");
    reg
}

#[test]
fn cdc_c6_storage_flag_and_control_plane_gate_agree_on_an_extra_eu_crossing() {
    let tenant = TenantId::from_token("01J0ACME");
    let region = Region::new("fr-par");
    let store = FsBlobStore::new();

    let mirror = PushMirrorClass::over(tenant.clone(), region.clone(), &store);
    mirror
        .source_is_content_addressed_and_encrypted(b"PACK\0mirror-source")
        .expect("mirror-source blobs are content-addressed + encrypted");

    let storage_target = PushMirrorTarget::new("github.com", Region::new("us-east"));
    let telemetry = MirrorTelemetry::new();
    let flagged = mirror.flag_target(&storage_target, &telemetry);
    assert!(flagged, "Storage FLAGS the extra-EU crossing");
    assert_eq!(
        telemetry.mirror_residency_deny(),
        1,
        "the flagged crossing is counted (the C6 signal)"
    );

    let mirror_report = mirror.residency_report(&storage_target);
    assert_eq!(mirror_report.store_class, ResidencyStoreClass::PushMirror);
    assert_eq!(
        mirror_report.region.as_str(),
        "us-east",
        "Storage flags the TARGET's region"
    );

    let mut reports = StoreSet::for_cell(&region).reports_for(&tenant);
    reports.push(mirror_report);
    assert!(
        verify_region_pinning(&tenant, &region, &reports).is_err(),
        "an extra-EU mirror target FAILs the attestation - the no-extra-EU-PII property is attestable"
    );

    let cp_target = CpMirrorTarget::new(storage_target.host.clone(), storage_target.region.clone());

    let reg = registry_with("01J0ACME", "fr-par", "cell-w-1");
    let policy = TransferPolicyDouble::new();
    let mut gate = MirrorGate::new();
    let decision = gate.mirror_allowed(&reg, &tenant, &cp_target, &policy);
    assert!(
        matches!(decision, MirrorDecision::Deny { .. }),
        "the control-plane gate DENIES the same crossing by default (0 PII to an ungated extra-EU mirror)"
    );
    assert_eq!(
        gate.unauthorised_pushes_prevented(),
        1,
        "the gate prevented the unauthorised push"
    );

    println!(
        "[P-255 CDC 10.5/12.4 C6 GREEN 2026-06-21] Storage FLAGS an extra-EU mirror crossing \
         (github.com @ us-east) into residency_verify (PushMirror @ us-east → attestation FAILS) + \
         mirror_residency_deny=1; the control-plane mirror_allowed GATE DENIES the SAME crossing by \
         default → 0 PII to an ungated extra-EU mirror. Storage flags, the control plane gates - the \
         two halves agree on the crossing."
    );
}

#[test]
fn cdc_c6_storage_flag_and_control_plane_gate_agree_on_a_same_region_mirror() {
    let tenant = TenantId::from_token("01J0ACME");
    let region = Region::new("fr-par");
    let store = FsBlobStore::new();

    let mirror = PushMirrorClass::over(tenant.clone(), region.clone(), &store);
    let storage_target = PushMirrorTarget::new("git.acme.internal.fr", region.clone());
    let telemetry = MirrorTelemetry::new();
    assert!(
        !mirror.flag_target(&storage_target, &telemetry),
        "a same-region mirror is no crossing"
    );
    assert_eq!(telemetry.mirror_residency_deny(), 0, "no crossing flagged");

    let mut reports = StoreSet::for_cell(&region).reports_for(&tenant);
    reports.push(mirror.residency_report(&storage_target));
    assert!(
        verify_region_pinning(&tenant, &region, &reports).is_ok(),
        "a same-region mirror passes the attestation (the byte never leaves the region)"
    );

    let cp_target = CpMirrorTarget::new(storage_target.host.clone(), storage_target.region.clone());
    let reg = registry_with("01J0ACME", "fr-par", "cell-w-1");
    let policy = TransferPolicyDouble::new();
    let mut gate = MirrorGate::new();
    assert!(
        gate.mirror_allowed(&reg, &tenant, &cp_target, &policy)
            .is_allowed(),
        "the control-plane gate ALLOWS a same-region mirror (no crossing)"
    );
    assert_eq!(
        gate.unauthorised_pushes_prevented(),
        0,
        "no unauthorised push prevented - no crossing"
    );
}
