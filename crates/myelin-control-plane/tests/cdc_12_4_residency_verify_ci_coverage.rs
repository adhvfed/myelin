use myelin_control_plane::{
    residency_verify_ci, RequiredStoreSet, ResidencySigningKey, ResidencyStoreClass,
    SignedAttestation, StoreRegionReport,
};
use myelin_tenancy::{Region, TenantId};

struct AuditorVerdict {
    tenant: String,
    region: String,
    stores_attested: usize,
    covers_ci: bool,
    ci_surfaces_in_region: bool,
    verified: bool,
}

impl AuditorVerdict {
    fn from_attestation(att: &SignedAttestation, key: &ResidencySigningKey) -> AuditorVerdict {
        let ci_surfaces_in_region = ResidencyStoreClass::CI_SET.iter().all(|ci| {
            att.store_regions
                .iter()
                .any(|(c, r)| c == ci && r.as_str() == att.region.as_str())
        });
        AuditorVerdict {
            tenant: att.tenant_id.as_str().to_string(),
            region: att.region.as_str().to_string(),
            stores_attested: att.store_regions.len(),
            covers_ci: att.coverage == RequiredStoreSet::M1AndCi,
            ci_surfaces_in_region,
            verified: att.verify(key),
        }
    }
}

#[test]
fn cdc_12_4_residency_verify_ci_provider_consumer() {
    let tenant = TenantId::from_token("01J0EUTENANT");
    let region = Region::new("fr-par");
    let signing_key = ResidencySigningKey::from_bytes([0xc1u8; 32]);

    let reports: Vec<StoreRegionReport> = ResidencyStoreClass::M1_AND_CI_SET
        .iter()
        .map(|c| StoreRegionReport::new(*c, Region::new("fr-par")))
        .collect();

    let att = residency_verify_ci(&tenant, &region, &reports, &signing_key)
        .expect("a signed M1+CI attestation");

    let verdict = AuditorVerdict::from_attestation(&att, &signing_key);
    assert_eq!(verdict.tenant, "01J0EUTENANT");
    assert_eq!(verdict.region, "fr-par");
    assert_eq!(
        verdict.stores_attested,
        ResidencyStoreClass::M1_AND_CI_SET.len()
    );
    assert!(
        verdict.covers_ci,
        "the auditor sees the attestation covered the CI store set (no-global-CI-pool)"
    );
    assert!(
        verdict.ci_surfaces_in_region,
        "every CI surface reported the tenant's region"
    );
    assert!(
        verdict.verified,
        "the auditor verifies the no-global-CI-pool attestation"
    );

    let m1_only = myelin_control_plane::residency_verify(&tenant, &region, &reports, &signing_key)
        .expect("a signed M1-only attestation");
    let m1_verdict = AuditorVerdict::from_attestation(&m1_only, &signing_key);
    assert!(
        m1_verdict.verified,
        "the M1-only attestation itself verifies"
    );
    assert!(
        !m1_verdict.covers_ci,
        "an M1-only attestation does NOT claim CI coverage - the auditor can tell the difference"
    );
}
