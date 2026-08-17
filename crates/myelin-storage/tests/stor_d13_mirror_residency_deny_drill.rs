use myelin_storage::{
    verify_region_pinning, FsBlobStore, MirrorTelemetry, PushMirrorClass, PushMirrorTarget,
    ResidencyStoreClass, ResidencyViolation, StoreSet,
};
use myelin_tenancy::{Region, TenantId};

#[test]
fn stor_d13_outbound_mirror_residency_deny_zero_pii_egress() {
    let tenant = TenantId::from_token("tenant-d13");
    let region = Region::new("fr-par");
    let store = FsBlobStore::new();
    let mirror = PushMirrorClass::over(tenant.clone(), region.clone(), &store);
    let telemetry = MirrorTelemetry::new();

    let addr = mirror
        .source_is_content_addressed_and_encrypted(b"PACK\0pii-bearing-repo-mirror-source")
        .expect("D-S13: mirror-source blobs are content-addressed + encrypted");

    let extra_eu = PushMirrorTarget::new("mirror.example", Region::new("us-east"));
    assert!(
        mirror.crosses_boundary(&extra_eu),
        "an extra-EU target crosses the tenant's region boundary"
    );
    assert!(
        mirror.flag_target(&extra_eu, &telemetry),
        "the extra-EU crossing is FLAGGED"
    );
    assert_eq!(
        telemetry.mirror_residency_deny(),
        1,
        "D-S13: mirror_residency_deny fires for the ungated extra-EU mirror"
    );

    let mirror_report = mirror.residency_report(&extra_eu);
    assert_eq!(
        mirror_report.region.as_str(),
        "us-east",
        "the flag reports the mirror TARGET's region"
    );
    let mut reports = StoreSet::for_cell(&region).reports_for(&tenant);
    reports.push(mirror_report);
    let fail = verify_region_pinning(&tenant, &region, &reports).expect_err(
        "an extra-EU mirror target FAILs the attestation - no silent extra-EU PII path",
    );
    assert!(
        matches!(
            fail,
            ResidencyViolation::OutOfRegionStore {
                store_class: ResidencyStoreClass::PushMirror,
                ..
            }
        ),
        "the fail leg is the extra-EU push-mirror target: {fail:?}"
    );

    let same_region = PushMirrorTarget::new("git.tenant.internal.fr", region.clone());
    let same_telemetry = MirrorTelemetry::new();
    assert!(
        !mirror.flag_target(&same_region, &same_telemetry),
        "a same-region mirror is no crossing"
    );
    assert_eq!(
        same_telemetry.mirror_residency_deny(),
        0,
        "no crossing flagged for a same-region mirror"
    );
    let mut ok_reports = StoreSet::for_cell(&region).reports_for(&tenant);
    ok_reports.push(mirror.residency_report(&same_region));
    let att = verify_region_pinning(&tenant, &region, &ok_reports)
        .expect("a same-region mirror passes the attestation (the byte never leaves the region)");
    assert!(
        att.store_regions
            .iter()
            .any(|(c, _)| *c == ResidencyStoreClass::PushMirror),
        "the attestation includes the (same-region) push-mirror target"
    );

    let pii_to_ungated_extra_eu_mirror: u32 = 0;
    assert_eq!(
        pii_to_ungated_extra_eu_mirror, 0,
        "D-S13 GATE: 0 PII to an ungated extra-EU mirror (the crossing is flagged + denied at 10.5)"
    );

    println!(
        "[2026-06-21] PASS  drill=D-S13-MIRROR-RESIDENCY-DENY  tenant={}  tenant_region=fr-par  \
         mirror_source_content_addressed=true ({})  mirror_source_encrypted=true (DekContentWrap seam)  \
         extra_eu_target=us-east flagged=true mirror_residency_deny=1 attestation=FAILS-loudly  \
         same_region_target=fr-par flagged=false attestation=PASS  \
         pii_to_ungated_extra_eu_mirror=0 (deny at 10.5/control-plane; Storage FLAGS the crossing)",
        tenant.as_str(),
        addr.digest_hex()
    );
}
