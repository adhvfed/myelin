//! P-CP-17 (global P-324) CDC — **contract 12.4 with CI-store coverage (C-2): provider + consumer.**
//!
//! The PROVIDER is the control plane minting a [`SignedAttestation`] over the FULL M1 + CI store set
//! ([`residency_verify_ci`]) from the registry's region of record + the M1 store reports + the CI
//! surfaces' region reports (runner pool / log tier / artifact store / cache namespaces). The CONSUMER
//! stands in for an AUDITOR (the `myelin tenant residency verify` CLI, now covering the CI stores): it
//! takes the attestation and — load-bearing — can ONLY (a) read the PII-free fields incl. the
//! self-describing `coverage` (M1+CI), and (b) VERIFY the signature; it cannot read any personal data
//! (there is none) nor forge an attestation (it has no key), nor pass off an M1-only attestation as an
//! M1+CI one (the coverage is bound into the MAC). If the attestation shape drifts — or the CI
//! coverage stops being signed — the consumer stops compiling / stops verifying.
//!
//! This is the CI-coverage twin of the in-crate `cdc_12_4_residency_verify_provider_consumer` (P-085).

use myelin_control_plane::{
    residency_verify_ci, RequiredStoreSet, ResidencySigningKey, ResidencyStoreClass,
    SignedAttestation, StoreRegionReport,
};
use myelin_tenancy::{Region, TenantId};

/// A stand-in AUDITOR consumer for the CI-coverage attestation: it verifies an attestation, reads its
/// PII-free verdict, and — the CI-coverage point — checks the attestation actually covered the CI
/// surfaces (an auditor demanding the no-global-CI-pool property will not accept an M1-only
/// attestation). It holds only the PUBLIC verification surface (the key to verify, never to mint).
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

    // Every M1 store + every CI surface reports the tenant's region.
    let reports: Vec<StoreRegionReport> = ResidencyStoreClass::M1_AND_CI_SET
        .iter()
        .map(|c| StoreRegionReport::new(*c, Region::new("fr-par")))
        .collect();

    // PROVIDER: the control plane mints the CI-coverage attestation.
    let att = residency_verify_ci(&tenant, &region, &reports, &signing_key)
        .expect("a signed M1+CI attestation");

    // CONSUMER: the auditor verifies it + reads the PII-free verdict, demanding CI coverage.
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

    // The auditor REJECTS an M1-only attestation when it demands CI coverage (the coverage is
    // load-bearing — an M1-only attestation does not prove the no-global-CI-pool property).
    let m1_only = myelin_control_plane::residency_verify(&tenant, &region, &reports, &signing_key)
        .expect("a signed M1-only attestation");
    let m1_verdict = AuditorVerdict::from_attestation(&m1_only, &signing_key);
    assert!(
        m1_verdict.verified,
        "the M1-only attestation itself verifies"
    );
    assert!(
        !m1_verdict.covers_ci,
        "an M1-only attestation does NOT claim CI coverage — the auditor can tell the difference"
    );
}
