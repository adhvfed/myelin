//! # CDC pair for contract 12.4 — Storage PROVIDES the per-store region reports; the control
//! plane's `residency_verify` CONSUMES + signs them (P-ST-15 / P-102).
//!
//! Contract 12.4 (`residency_verify`) has TWO halves across two crates:
//!   - **PROVIDER (Storage, P-ST-15):** every region-pinned store reports `(store_class, region)`
//!     for a tenant; [`myelin_storage::StoreSet::residency_verify`] aggregates them + proves region
//!     pinning ([`myelin_storage::RegionPinningAttestation`]). Storage OWNS the report-producing
//!     side (it is upstream of the control plane in the crate DAG).
//!   - **CONSUMER (control plane, P-CP-09 / P-085):** `residency_verify` takes the per-store reports
//!     plus the signing key and signs the no-global-pool [`myelin_control_plane::SignedAttestation`]
//!     the auditor verifies.
//!
//! This CDC PROVES the two halves agree WITHOUT a shared report type (the DAG forbids a
//! `myelin-storage -> myelin-control-plane` edge, so neither imports the other's report struct).
//! The bridge is the field-for-field mapping below: Storage's [`myelin_storage::StoreResidencyReport`]
//! maps onto the control plane's `StoreRegionReport`, and the M1 store set + region codes match
//! byte-for-byte. **If either shape drifts (a renamed field, a changed store-class set, a different
//! region accessor), this test stops compiling** — the point of a glue CDC (the storage report is
//! the wire between the two crates; a drift is caught at compile time, not in prod).
//!
//! `myelin-control-plane` is a DEV-dependency of this test ONLY (it never enters the storage crate's
//! production DAG — that edge would be a cycle). The control plane already depends on
//! `myelin-storage`, so the dev-dep here is the consumer reaching DOWN to its provider for the CDC,
//! the normal CDC direction.

use myelin_control_plane::residency_verify::{
    residency_verify as cp_residency_verify, ResidencyMismatch, ResidencySigningKey,
    ResidencyStoreClass as CpStoreClass, StoreRegionReport,
};
use myelin_storage::{ResidencyStoreClass, StoreResidencyReport, StoreSet};
use myelin_tenancy::{Region, TenantId};

/// Map a Storage store-class onto the control-plane store-class (the field-for-field bridge — a
/// drift in EITHER enum is a compile error here). The M1 set is OLTP/blob/index/KMS in both.
fn to_cp_class(class: ResidencyStoreClass) -> CpStoreClass {
    match class {
        ResidencyStoreClass::Oltp => CpStoreClass::Oltp,
        ResidencyStoreClass::Blob => CpStoreClass::Blob,
        ResidencyStoreClass::IndexSearch => CpStoreClass::IndexSearch,
        ResidencyStoreClass::Kms => CpStoreClass::Kms,
        // The T3 firehose archive (P-ST-20 / P-147) is a Storage follow-on store class whose sealed
        // segments physically rest as content-addressed T2 BLOBS (storage.md §3.3: "sealed segments
        // flush to the object tier (T2) as content-addressed blobs"). At the control-plane
        // no-global-pool WIRE it therefore reports under the T2 blob tier — it is not a distinct M1
        // attestation class (the control-plane M1 set is OLTP/blob/index/KMS). Its residency is
        // verified Storage-side by `verify_region_pinning` (which checks ANY reported class's region,
        // so a wrong-region archive FAILs there without a wire change).
        ResidencyStoreClass::T3FirehoseArchive => CpStoreClass::Blob,
    }
}

/// Map a Storage per-store report onto the control-plane report shape (the 12.4 wire).
fn to_cp_report(report: &StoreResidencyReport) -> StoreRegionReport {
    StoreRegionReport::new(to_cp_class(report.store_class), report.region.clone())
}

/// **CDC 12.4 — PROVIDER (Storage) → CONSUMER (control plane): a region-pinned store set produces
/// reports the control plane signs into the no-global-pool attestation.**
#[test]
fn cdc_12_4_storage_reports_feed_control_plane_residency_verify() {
    let tenant = TenantId::from_token("01J0ACME");
    let region = Region::new("fr-par");

    // PROVIDER: Storage's region-pinned store set proves region pinning + emits the per-store reports.
    let set = StoreSet::for_cell(&region);
    let attestation = set
        .residency_verify(&tenant, &region)
        .expect("Storage: every store in-region → a region-pinning attestation");
    let storage_reports = attestation.reports();
    assert_eq!(
        storage_reports.len(),
        ResidencyStoreClass::M1_SET.len(),
        "Storage reports one region per M1 store class"
    );

    // BRIDGE: map Storage's reports onto the control-plane wire shape.
    let cp_reports: Vec<StoreRegionReport> = storage_reports.iter().map(to_cp_report).collect();

    // CONSUMER: the control plane signs them into the auditor's no-global-pool attestation.
    let key = ResidencySigningKey::from_bytes([7u8; 32]);
    let signed = cp_residency_verify(&tenant, &region, &cp_reports, &key)
        .expect("control plane: the store reports sign into a no-global-pool attestation");
    assert_eq!(signed.tenant_id, tenant);
    assert_eq!(signed.region.as_str(), "fr-par");
    assert_eq!(signed.store_regions.len(), ResidencyStoreClass::M1_SET.len());
    assert!(signed.verify(&key), "an auditor verifies the signed no-global-pool attestation");
}

/// **CDC 12.4 fail leg — a cross-region store in Storage's report set makes the control plane's
/// `residency_verify` FAIL identically (the two halves agree on the breach, not just the green).**
/// Storage's admin path catches the cross-region store first; if a raw wrong-region report reached
/// the control plane, IT fails too — the no-global-pool property holds on both sides.
#[test]
fn cdc_12_4_a_cross_region_store_fails_both_halves() {
    let tenant = TenantId::from_token("01J0ACME");
    let region = Region::new("fr-par");

    // Storage's admin path REJECTS a misrouted (eu-north) blob store — the provider catches it.
    let misrouted = StoreSet::from_stores(vec![
        myelin_storage::RegionPinnedStore::pinned_to(ResidencyStoreClass::Oltp, region.clone()),
        myelin_storage::RegionPinnedStore::pinned_to(
            ResidencyStoreClass::Blob,
            Region::new("eu-north"),
        ),
        myelin_storage::RegionPinnedStore::pinned_to(ResidencyStoreClass::IndexSearch, region.clone()),
        myelin_storage::RegionPinnedStore::pinned_to(ResidencyStoreClass::Kms, region.clone()),
    ]);
    assert!(
        misrouted.residency_verify(&tenant, &region).is_err(),
        "Storage (provider) FAILS on a cross-region store"
    );

    // If a raw wrong-region report reached the control plane (consumer), it FAILS too — same breach.
    let raw_reports = vec![
        StoreRegionReport::new(CpStoreClass::Oltp, region.clone()),
        StoreRegionReport::new(CpStoreClass::Blob, Region::new("eu-north")),
        StoreRegionReport::new(CpStoreClass::IndexSearch, region.clone()),
        StoreRegionReport::new(CpStoreClass::Kms, region.clone()),
    ];
    let key = ResidencySigningKey::from_bytes([7u8; 32]);
    let err = cp_residency_verify(&tenant, &region, &raw_reports, &key)
        .expect_err("control plane (consumer) FAILS on the same cross-region store");
    assert!(
        matches!(err, ResidencyMismatch::WrongRegion { .. }),
        "both halves agree the breach is a wrong-region store: {err:?}"
    );
}
