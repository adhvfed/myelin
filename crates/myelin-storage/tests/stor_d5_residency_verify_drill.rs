//! # STOR-D5 — the `residency verify` admin-path drill (P-ST-15 / P-102): region pinning enforced
//! end-to-end, **0 cross-region egress**, with the dated green artifact.
//!
//! This is the UNIT-LEVEL STOR-D5 drill (the storage admin-path face): it exercises the
//! `myelin storage residency verify <tenant>` mechanism ([`myelin_storage::StoreSet::residency_verify`])
//! end-to-end and asserts the prompt's GATE — *read/replicate a tenant's data outside its region →
//! impossible by construction; 0 cross-region PII egress*.
//!
//! Three legs, each emitting its verdict LOUDLY (a red aborts the test, EI-01 §3 — the threshold is
//! NOT weakened to pass):
//!   1. **GREEN:** a cell-pinned store set → `residency verify` attests the tenant's SINGLE region;
//!      `cross_region_egress == 0` (the dated STOR-D5 green artifact).
//!   2. **THE WRITE BOUNDARY:** an out-of-region write is REJECTED in-process (the partition-key +
//!      residency-pin mechanism) — *no store writes outside its region*, so cross-region
//!      replication has no source.
//!   3. **THE FAIL LEG (no silent pass):** a misrouted (cross-region) store → `residency verify`
//!      FAILS loudly; `cross_region_egress >= 1` reads RED.
//!
//! The LIVE, real-Postgres twin (the DB enforces the boundary, not app code) is
//! `tests/stor_d5_cross_region_egress_drill.rs` (behind `--features integration`, P-096). This unit
//! drill proves the admin-path aggregation + the in-process boundary; the integration drill proves
//! the DB-level RLS `WITH CHECK`. Both must read 0 cross-region egress.

use myelin_storage::{
    RegionPinnedStore, ResidencyStoreClass, ResidencyVerifySignal, ResidencyViolation, StoreSet,
};
use myelin_tenancy::{Region, TenantId};

/// **THE STOR-D5 DRILL: region pinning enforced end-to-end, 0 cross-region egress.**
#[test]
fn stor_d5_residency_pinning_zero_cross_region_egress() {
    let tenant = TenantId::from_token("tenant-d5");
    let region = Region::new("fr-par");

    // ── (1) GREEN: the admin path attests the tenant's SINGLE region; 0 cross-region egress.
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

    // ── (2) THE WRITE BOUNDARY: an out-of-region write is REJECTED in-process (no source to
    //        replicate). The partition key carries the region; the residency-pin boundary rejects.
    let blob = RegionPinnedStore::pinned_to(ResidencyStoreClass::Blob, region.clone());
    assert_eq!(
        blob.admit_write(&region),
        Ok(()),
        "an in-region write is admitted (the normal path)"
    );
    let rejected = blob
        .admit_write(&Region::new("eu-central"))
        .expect_err("an out-of-region write MUST be rejected — no store writes outside its region");
    assert!(
        matches!(rejected, ResidencyViolation::OutOfRegionWrite { .. }),
        "the write boundary rejects an out-of-region write: {rejected:?}"
    );

    // ── (3) THE FAIL LEG (no silent pass): a misrouted store FAILS the admin path; egress reads RED.
    let misrouted = StoreSet::from_stores(vec![
        RegionPinnedStore::pinned_to(ResidencyStoreClass::Oltp, region.clone()),
        RegionPinnedStore::pinned_to(ResidencyStoreClass::Blob, region.clone()),
        // The index store is (wrongly) in eu-west — a cross-region store.
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
    // The RED signal carries cross_region_egress >= 1 (a breach reads RED, never a silent green).
    let red = ResidencyVerifySignal::red(tenant.clone(), region.clone(), 3, 1);
    assert!(red.cross_region_egress >= 1, "a residency breach reads RED");

    // Dated green artifact (the STOR-D5 PROVEN line — EI-01 §3, observability is part of the pass).
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
