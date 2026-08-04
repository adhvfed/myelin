use myelin_control_plane::residency_verify::{
    residency_verify as cp_residency_verify, ResidencyMismatch, ResidencySigningKey,
    ResidencyStoreClass as CpStoreClass, StoreRegionReport,
};
use myelin_storage::{ResidencyStoreClass, StoreResidencyReport, StoreSet};
use myelin_tenancy::{Region, TenantId};

fn to_cp_class(class: ResidencyStoreClass) -> CpStoreClass {
    match class {
        ResidencyStoreClass::Oltp => CpStoreClass::Oltp,
        ResidencyStoreClass::Blob => CpStoreClass::Blob,
        ResidencyStoreClass::IndexSearch => CpStoreClass::IndexSearch,
        ResidencyStoreClass::Kms => CpStoreClass::Kms,
        ResidencyStoreClass::T3FirehoseArchive => CpStoreClass::Blob,
        ResidencyStoreClass::CdnEdgeSet => CpStoreClass::Blob,
        ResidencyStoreClass::PushMirror => CpStoreClass::Blob,
    }
}

fn to_cp_report(report: &StoreResidencyReport) -> StoreRegionReport {
    StoreRegionReport::new(to_cp_class(report.store_class), report.region.clone())
}

#[test]
fn cdc_12_4_storage_reports_feed_control_plane_residency_verify() {
    let tenant = TenantId::from_token("01J0ACME");
    let region = Region::new("fr-par");

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

    let cp_reports: Vec<StoreRegionReport> = storage_reports.iter().map(to_cp_report).collect();

    let key = ResidencySigningKey::from_bytes([7u8; 32]);
    let signed = cp_residency_verify(&tenant, &region, &cp_reports, &key)
        .expect("control plane: the store reports sign into a no-global-pool attestation");
    assert_eq!(signed.tenant_id, tenant);
    assert_eq!(signed.region.as_str(), "fr-par");
    assert_eq!(
        signed.store_regions.len(),
        ResidencyStoreClass::M1_SET.len()
    );
    assert!(
        signed.verify(&key),
        "an auditor verifies the signed no-global-pool attestation"
    );
}

#[test]
fn cdc_12_4_a_cross_region_store_fails_both_halves() {
    let tenant = TenantId::from_token("01J0ACME");
    let region = Region::new("fr-par");

    let misrouted = StoreSet::from_stores(vec![
        myelin_storage::RegionPinnedStore::pinned_to(ResidencyStoreClass::Oltp, region.clone()),
        myelin_storage::RegionPinnedStore::pinned_to(
            ResidencyStoreClass::Blob,
            Region::new("eu-north"),
        ),
        myelin_storage::RegionPinnedStore::pinned_to(
            ResidencyStoreClass::IndexSearch,
            region.clone(),
        ),
        myelin_storage::RegionPinnedStore::pinned_to(ResidencyStoreClass::Kms, region.clone()),
    ]);
    assert!(
        misrouted.residency_verify(&tenant, &region).is_err(),
        "Storage (provider) FAILS on a cross-region store"
    );

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
