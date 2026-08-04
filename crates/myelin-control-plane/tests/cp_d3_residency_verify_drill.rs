use myelin_control_plane::{
    residency_verify, ResidencyAttestationSignal, ResidencyMismatch, ResidencySigningKey,
    ResidencyStoreClass, StoreRegionReport,
};
use myelin_harness::{Predicate, SignalName, SignalSource};
use myelin_tenancy::{Region, TenantId};

fn all_in_region(region: &str) -> Vec<StoreRegionReport> {
    ResidencyStoreClass::M1_SET
        .iter()
        .map(|c| StoreRegionReport::new(*c, Region::new(region)))
        .collect()
}

#[test]
fn cp_d3_residency_verify_m1_store_set() {
    let tenant = TenantId::from_token("01J0ACME");
    let region = Region::new("fr-par");
    let key = ResidencySigningKey::from_bytes([0x5eu8; 32]);

    let attestation = residency_verify(&tenant, &region, &all_in_region("fr-par"), &key)
        .expect("every M1 store in-region → a signed attestation (the gate is GREEN)");
    assert_eq!(attestation.region.as_str(), "fr-par");
    assert_eq!(
        attestation.store_regions.len(),
        ResidencyStoreClass::M1_SET.len(),
        "the attestation aggregates ALL M1 stores (OLTP/blob/index/KMS) - none silently absent"
    );
    for (class, r) in &attestation.store_regions {
        assert_eq!(
            r.as_str(),
            "fr-par",
            "store `{}` reported the tenant's region",
            class.label()
        );
    }
    assert!(
        attestation.signature.starts_with("blake3-mac:"),
        "the attestation is SIGNED"
    );
    assert!(
        attestation.verify(&key),
        "an auditor verifies the no-global-pool attestation"
    );
    let green = ResidencyAttestationSignal::green(&attestation);
    assert_eq!(
        green.region_mismatches, 0,
        "the green artifact is 0 region mismatches"
    );

    let mut wrong = all_in_region("fr-par");
    wrong[1] = StoreRegionReport::new(ResidencyStoreClass::Blob, Region::new("eu-north"));
    let breach = residency_verify(&tenant, &region, &wrong, &key)
        .expect_err("a wrong-region store FAILS the attestation (the gate is RED for the breach)");
    assert_eq!(
        breach,
        ResidencyMismatch::WrongRegion {
            tenant: tenant.clone(),
            tenant_region: Region::new("fr-par"),
            store_class: ResidencyStoreClass::Blob,
            store_region: Region::new("eu-north"),
        }
    );
    assert!(
        breach.to_string().contains("not a silent pass"),
        "loud: {breach}"
    );

    let missing: Vec<StoreRegionReport> = all_in_region("fr-par")
        .into_iter()
        .filter(|r| r.store_class != ResidencyStoreClass::Kms)
        .collect();
    let gap = residency_verify(&tenant, &region, &missing, &key)
        .expect_err("a missing M1 store report FAILS fail-closed (the gate is RED for the gap)");
    assert_eq!(
        gap,
        ResidencyMismatch::MissingStoreReport {
            tenant: tenant.clone(),
            store_class: ResidencyStoreClass::Kms,
        }
    );

    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, green.region_mismatches as i64);
    sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-085 CP-D3 GREEN 2026-06-19] residency_verify over the M1 store set: tenant 01J0ACME \
         (fr-par) - every M1 store (OLTP/blob/index-search/KMS) reported fr-par → a SIGNED, PII-free \
         attestation ({} stores attested, region_mismatches={}, signature={}…); it VERIFIES under the \
         control-plane key. RED legs proven: a blob tier in eu-north FAILED the attestation (not a \
         silent pass); a missing KMS report FAILED fail-closed. FLOOR (NAMED PARTIAL): the store set \
         is the M1 stores only - the CI runner pool + log/artifact/cache coverage is the M4 follow-on \
         P-CP-17 (it extends this SAME residency_verify); the CP-D3 write-boundary + STOR-D5 \
         cross-region-egress runtime drills ride the four-layer enforcement P-CP-12 (live stack).",
        green.stores_attested,
        green.region_mismatches,
        &attestation.signature[..attestation.signature.len().min(22)],
    );
}

#[test]
fn cp_d3_gate_is_not_vacuous() {
    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, 1);
    assert!(
        !sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
            .is_green(),
        "a region mismatch MUST read RED - the residency-attestation zero is a real tripwire"
    );
}
