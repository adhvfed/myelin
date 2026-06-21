//! # CDC pair for contract 10.5 / 12.4 (the C6 outbound-mirror flag) — Storage FLAGS the crossing
//! (provider); the control plane's `mirror_allowed` GATES it (consumer). P-ST-25 / global P-255.
//!
//! **DATED GREEN ARTIFACT (2026-06-21).** The C6 outbound push-mirror residency gate has TWO halves
//! across two crates, and this CDC proves they AGREE on a residency-boundary crossing:
//!   - **PROVIDER (Storage, P-ST-25):** [`myelin_storage::PushMirrorClass`] FLAGS the crossing — it
//!     reports the mirror TARGET's region into `residency_verify` (the
//!     [`myelin_storage::ResidencyStoreClass::PushMirror`] report) + counts
//!     `mirror_residency_deny{tenant}`. Storage authors NO allow/deny.
//!   - **CONSUMER / GATE (control plane, P-CP-16 / P-251):** [`myelin_control_plane::MirrorGate::mirror_allowed`]
//!     makes the deny-by-default decision (an extra-region crossing without a recorded
//!     `transfer_allowed` lawful basis is DENIED).
//!
//! The contract this pins: **Storage's flag (the target's region) and the control plane's gate decide
//! the SAME crossing.** For a mirror target whose region ≠ the tenant's region, Storage's
//! `residency_report` reports the foreign region (so the crossing is attestable) AND the control plane
//! `mirror_allowed` denies it by default (so 0 PII reaches the ungated extra-EU mirror). If EITHER
//! half drifts (Storage starts reporting the tenant's region — hiding the crossing — or the gate stops
//! denying a crossing without a `transfer_allowed` entry), this test fails — the point of the glue CDC.
//!
//! **The split is structural (storage.md §6 / EI-01 §7):** Storage answers "does this target cross the
//! residency boundary, and into what region?"; the control plane answers "is this crossing allowed?"
//! (consulting GDPR's `transfer_allowed`). The control plane↔GDPR half is the SUBJECT of P-251's own
//! CDC (`myelin-control-plane/tests/cdc_10_5_mirror_gate.rs`, over the REAL `TransferGate`); HERE the
//! load-bearing seam is **Storage's flag reaching the gate** — so the GDPR `transfer_allowed` half is a
//! faithful in-test [`TransferPolicyDouble`] (mirroring the frozen deny-extra-EU-by-default behaviour),
//! NOT the service crate (which is not a storage dev-dep — keeping this CDC focused on the C6 flag/gate
//! seam P-255 owns, documented deviation EI-01 §1).
//!
//! `myelin-control-plane` is a DEV-dependency of this test ONLY (the same dev-only edge the
//! `cdc_12_4_storage_residency_report` pair uses — the consumer reaching DOWN to its provider).

use myelin_control_plane::{
    Capacity, Cell, CellStatus, IsolationKind, MirrorDecision, MirrorGate,
    MirrorTarget as CpMirrorTarget, PlacementStatus, Registry, TenantPlacement, TransferPolicy,
};
use myelin_storage::{
    verify_region_pinning, FsBlobStore, MirrorTelemetry, PushMirrorClass, PushMirrorTarget,
    ResidencyStoreClass, StoreSet,
};
use myelin_tenancy::{CellId, Region, TenantId};

/// A faithful stand-in for GDPR's `transfer_allowed` (the lawful-transfer half): within-EU/EEA regions
/// are allowed structurally; extra-EU regions are DENIED unless a transfer mechanism is recorded
/// (mirroring the REAL `TransferGate`'s frozen deny-extra-EU-by-default behaviour). The REAL gate is
/// driven in P-251's own CDC; here it stands in so this CDC stays focused on the Storage-flag → gate seam.
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
        capacity: Capacity { tenants_max: 1000, write_qps_max: 5000, storage_bytes_max: 1 << 40 },
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

/// **CDC 10.5/12.4 (C6) — an extra-EU mirror target: Storage FLAGS the crossing into
/// `residency_verify`, and the control-plane gate DENIES it by default. The two halves agree.**
#[test]
fn cdc_c6_storage_flag_and_control_plane_gate_agree_on_an_extra_eu_crossing() {
    let tenant = TenantId::from_token("01J0ACME");
    let region = Region::new("fr-par"); // ACME is EU-resident.
    let store = FsBlobStore::new();

    // ── PROVIDER (Storage): keep mirror-source blobs content-addressed + encrypted, and FLAG the
    //    crossing for an extra-EU target. ──
    let mirror = PushMirrorClass::over(tenant.clone(), region.clone(), &store);
    // The mirror-source bytes are content-addressed + encrypted (storage.md §6(a)).
    mirror
        .source_is_content_addressed_and_encrypted(b"PACK\0mirror-source")
        .expect("mirror-source blobs are content-addressed + encrypted");

    let storage_target = PushMirrorTarget::new("github.com", Region::new("us-east"));
    let telemetry = MirrorTelemetry::new();
    let flagged = mirror.flag_target(&storage_target, &telemetry);
    assert!(flagged, "Storage FLAGS the extra-EU crossing");
    assert_eq!(telemetry.mirror_residency_deny(), 1, "the flagged crossing is counted (the C6 signal)");

    // Storage's flag REPORTS the mirror TARGET's region into residency_verify — the crossing surfaces.
    let mirror_report = mirror.residency_report(&storage_target);
    assert_eq!(mirror_report.store_class, ResidencyStoreClass::PushMirror);
    assert_eq!(mirror_report.region.as_str(), "us-east", "Storage flags the TARGET's region");

    let mut reports = StoreSet::for_cell(&region).reports_for(&tenant);
    reports.push(mirror_report);
    assert!(
        verify_region_pinning(&tenant, &region, &reports).is_err(),
        "an extra-EU mirror target FAILs the attestation — the no-extra-EU-PII property is attestable"
    );

    // ── BRIDGE: the SAME target Storage flagged is mapped to the control-plane gate's shape. ──
    let cp_target = CpMirrorTarget::new(storage_target.host.clone(), storage_target.region.clone());

    // ── CONSUMER / GATE (control plane): the deny-by-default decision on the SAME crossing. ──
    let reg = registry_with("01J0ACME", "fr-par", "cell-w-1");
    let policy = TransferPolicyDouble::new();
    let mut gate = MirrorGate::new();
    let decision = gate.mirror_allowed(&reg, &tenant, &cp_target, &policy);
    assert!(
        matches!(decision, MirrorDecision::Deny { .. }),
        "the control-plane gate DENIES the same crossing by default (0 PII to an ungated extra-EU mirror)"
    );
    assert_eq!(gate.unauthorised_pushes_prevented(), 1, "the gate prevented the unauthorised push");

    println!(
        "[P-255 CDC 10.5/12.4 C6 GREEN 2026-06-21] Storage FLAGS an extra-EU mirror crossing \
         (github.com @ us-east) into residency_verify (PushMirror @ us-east → attestation FAILS) + \
         mirror_residency_deny=1; the control-plane mirror_allowed GATE DENIES the SAME crossing by \
         default → 0 PII to an ungated extra-EU mirror. Storage flags, the control plane gates — the \
         two halves agree on the crossing."
    );
}

/// **CDC 10.5/12.4 (C6) — a SAME-region mirror: Storage flags no crossing (the attestation passes),
/// and the control-plane gate ALLOWS it (no boundary crossed). The two halves agree on the non-crossing.**
#[test]
fn cdc_c6_storage_flag_and_control_plane_gate_agree_on_a_same_region_mirror() {
    let tenant = TenantId::from_token("01J0ACME");
    let region = Region::new("fr-par");
    let store = FsBlobStore::new();

    // PROVIDER: a same-region mirror is no crossing — Storage flags nothing, reports the tenant's region.
    let mirror = PushMirrorClass::over(tenant.clone(), region.clone(), &store);
    let storage_target = PushMirrorTarget::new("git.acme.internal.fr", region.clone());
    let telemetry = MirrorTelemetry::new();
    assert!(!mirror.flag_target(&storage_target, &telemetry), "a same-region mirror is no crossing");
    assert_eq!(telemetry.mirror_residency_deny(), 0, "no crossing flagged");

    let mut reports = StoreSet::for_cell(&region).reports_for(&tenant);
    reports.push(mirror.residency_report(&storage_target));
    assert!(
        verify_region_pinning(&tenant, &region, &reports).is_ok(),
        "a same-region mirror passes the attestation (the byte never leaves the region)"
    );

    // CONSUMER / GATE: the same-region mirror is allowed (no boundary crossed) — the gate never even
    // consults the transfer policy.
    let cp_target = CpMirrorTarget::new(storage_target.host.clone(), storage_target.region.clone());
    let reg = registry_with("01J0ACME", "fr-par", "cell-w-1");
    let policy = TransferPolicyDouble::new();
    let mut gate = MirrorGate::new();
    assert!(
        gate.mirror_allowed(&reg, &tenant, &cp_target, &policy).is_allowed(),
        "the control-plane gate ALLOWS a same-region mirror (no crossing)"
    );
    assert_eq!(gate.unauthorised_pushes_prevented(), 0, "no unauthorised push prevented — no crossing");
}
