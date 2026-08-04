use myelin_storage::{
    RegionPinnedStore, ResidencyStoreClass, ResidencyVerifySignal, ResidencyViolation, StoreSet,
};
use myelin_tenancy::{Region, TenantId};

#[test]
fn stor_d5_residency_pinning_zero_cross_region_egress() {
    let tenant = TenantId::from_token("tenant-d5");
    let region = Region::new("fr-par");

    let set = StoreSet::for_cell(&region);
    let att = set
        .residency_verify(&tenant, &region)
        .expect("residency verify attests the tenant's single region");
    assert_eq!(
        att.region.as_str(),
        "fr-par",
        "the attestation pins the tenant's single region"
    );
    assert_eq!(
        att.store_regions.len(),
        ResidencyStoreClass::M1_SET.len(),
        "every M1 store (OLTP/blob/index/KMS) reported its region"
    );
    for (class, r) in &att.store_regions {
        assert_eq!(
            r.as_str(),
            "fr-par",
            "store `{}` is region-pinned to the tenant's region",
            class.label()
        );
    }
    let green = ResidencyVerifySignal::green(&att);
    assert_eq!(
        green.cross_region_egress, 0,
        "STOR-D5 GATE: 0 cross-region PII egress (the headline zero)"
    );
    assert_eq!(
        green.stores_attested,
        ResidencyStoreClass::M1_SET.len() as u32
    );

    let blob = RegionPinnedStore::pinned_to(ResidencyStoreClass::Blob, region.clone());
    assert_eq!(
        blob.admit_write(&region),
        Ok(()),
        "an in-region write is admitted (the normal path)"
    );
    let rejected = blob
        .admit_write(&Region::new("eu-central"))
        .expect_err("an out-of-region write MUST be rejected - no store writes outside its region");
    assert!(
        matches!(rejected, ResidencyViolation::OutOfRegionWrite { .. }),
        "the write boundary rejects an out-of-region write: {rejected:?}"
    );

    let misrouted = StoreSet::from_stores(vec![
        RegionPinnedStore::pinned_to(ResidencyStoreClass::Oltp, region.clone()),
        RegionPinnedStore::pinned_to(ResidencyStoreClass::Blob, region.clone()),
        RegionPinnedStore::pinned_to(ResidencyStoreClass::IndexSearch, Region::new("eu-west")),
        RegionPinnedStore::pinned_to(ResidencyStoreClass::Kms, region.clone()),
    ]);
    let fail = misrouted
        .residency_verify(&tenant, &region)
        .expect_err("a cross-region store FAILS residency verify (not a silent pass)");
    assert!(
        matches!(fail, ResidencyViolation::OutOfRegionStore { .. }),
        "the fail leg is a cross-region store: {fail:?}"
    );
    let red = ResidencyVerifySignal::red(tenant.clone(), region.clone(), 3, 1);
    assert!(red.cross_region_egress >= 1, "a residency breach reads RED");

    println!(
        "[2026-06-20] PASS  drill=STOR-D5-RESIDENCY-VERIFY  tenant={}  region=fr-par  \
         stores_attested={}  cross_region_egress=0 (0 PII egress)  \
         out_of_region_write=REJECTED-in-process (residency-pin boundary)  \
         cross_region_store=FAILS-loudly (no silent pass)  \
         mechanism=region-pinned store set + (tenant,region) partition key",
        tenant.as_str(),
        green.stores_attested
    );
}
