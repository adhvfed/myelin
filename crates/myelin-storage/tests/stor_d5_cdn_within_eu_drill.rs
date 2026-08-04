use myelin_storage::{
    verify_region_pinning, BlobError, CdnCloneClass, CdnEdgePop, ContentHash, FsBlobStore,
    ResidencyStoreClass, ResidencyVerifySignal, StoreResidencyReport, StoreSet,
};
use myelin_tenancy::{Region, TenantId};

#[test]
fn stor_d5_cdn_within_eu_zero_cross_region_egress() {
    let tenant = TenantId::from_token("tenant-d5-cdn");
    let region = Region::new("fr-par");
    let store = FsBlobStore::new();
    let cdn = CdnCloneClass::over(
        tenant.clone(),
        region.clone(),
         true,
        &store,
    );

    let bundle = b"PACK\0clone-bundle\0hot-repo-objects";
    let address = cdn.publish_bundle(bundle).expect("publish a clone bundle");
    assert_eq!(
        address,
        ContentHash::blake3(bundle),
        "the cache key IS the content-address"
    );
    assert_eq!(
        cdn.bundle(&address).expect("serve"),
        bundle,
        "serve by content-address is exact"
    );
    assert!(
        store.corrupt_for_drill(&tenant, &address),
        "bundle present to tamper"
    );
    assert!(
        matches!(cdn.bundle(&address), Err(BlobError::IntegrityFail { .. })),
        "a tampered bundle is REFUSED - the content-address is the cache validity check (no staleness)"
    );

    let candidates = vec![
        CdnEdgePop::new("par-1", Region::new("fr-par"), true),
        CdnEdgePop::new("ams-1", Region::new("nl-ams"), true),
        CdnEdgePop::new("iad-1", Region::new("us-east"), false),
    ];
    let eligible = cdn.eligible_edges(&candidates);
    assert_eq!(
        eligible.len(),
        2,
        "the extra-EU POP is excluded from an EU tenant's eligible edge set"
    );
    assert!(
        eligible.iter().all(|p| p.within_eu),
        "STOR-D5 GATE: every eligible CDN edge is within-EU (no PII-bearing bundle reaches an extra-EU edge)"
    );

    let mut reports = StoreSet::for_cell(&region).reports_for(&tenant);
    reports.push(cdn.residency_report());
    let att = verify_region_pinning(&tenant, &region, &reports)
        .expect("the CDN edge set reports the tenant's region → the attestation covers it");
    assert!(
        att.store_regions
            .iter()
            .any(|(c, _)| *c == ResidencyStoreClass::CdnEdgeSet),
        "the residency attestation INCLUDES the CDN edge set (12.4)"
    );
    let green = ResidencyVerifySignal::green(&att);
    assert_eq!(
        green.cross_region_egress, 0,
        "STOR-D5 GATE: 0 cross-region PII egress via the CDN (the headline zero)"
    );

    let bad_cdn = StoreResidencyReport {
        tenant: tenant.clone(),
        store_class: ResidencyStoreClass::CdnEdgeSet,
        region: Region::new("us-east"),
    };
    let mut bad_reports = StoreSet::for_cell(&region).reports_for(&tenant);
    bad_reports.push(bad_cdn);
    let fail = verify_region_pinning(&tenant, &region, &bad_reports)
        .expect_err("a cross-region CDN edge FAILs the attestation (not a silent pass)");
    assert!(
        fail.to_string().contains("no-global-pool"),
        "the cross-region CDN breach is caught by the SAME aggregation: {fail}"
    );

    println!(
        "[2026-06-21] PASS  drill=STOR-D5-CDN-WITHIN-EU  tenant={}  region=fr-par  \
         eligible_edges={}/{} within-EU (extra-EU excluded)  \
         cross_region_egress_via_cdn=0 (0 PII egress)  \
         attestation_includes_cdn_edge_set=true  \
         cross_region_cdn_edge=FAILS-loudly (no silent pass)  \
         class=blob-class-tag-over-unchanged-BlobStore (not a new store)",
        tenant.as_str(),
        eligible.len(),
        candidates.len()
    );
}
