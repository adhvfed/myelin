use std::collections::BTreeMap;

use myelin_tenancy::{Region, TenantId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ResidencyStoreClass {
    Oltp,
    Blob,
    IndexSearch,
    Kms,
    CiRunnerPool,
    CiLogTier,
    CiArtifactStore,
    CiCacheNamespaces,
}

impl ResidencyStoreClass {
    pub fn label(self) -> &'static str {
        match self {
            ResidencyStoreClass::Oltp => "oltp",
            ResidencyStoreClass::Blob => "blob",
            ResidencyStoreClass::IndexSearch => "index_search",
            ResidencyStoreClass::Kms => "kms",
            ResidencyStoreClass::CiRunnerPool => "ci_runner_pool",
            ResidencyStoreClass::CiLogTier => "ci_log_tier",
            ResidencyStoreClass::CiArtifactStore => "ci_artifact_store",
            ResidencyStoreClass::CiCacheNamespaces => "ci_cache_namespaces",
        }
    }

    pub const M1_SET: [ResidencyStoreClass; 4] = [
        ResidencyStoreClass::Oltp,
        ResidencyStoreClass::Blob,
        ResidencyStoreClass::IndexSearch,
        ResidencyStoreClass::Kms,
    ];

    pub const CI_SET: [ResidencyStoreClass; 4] = [
        ResidencyStoreClass::CiRunnerPool,
        ResidencyStoreClass::CiLogTier,
        ResidencyStoreClass::CiArtifactStore,
        ResidencyStoreClass::CiCacheNamespaces,
    ];

    pub const M1_AND_CI_SET: [ResidencyStoreClass; 8] = [
        ResidencyStoreClass::Oltp,
        ResidencyStoreClass::Blob,
        ResidencyStoreClass::IndexSearch,
        ResidencyStoreClass::Kms,
        ResidencyStoreClass::CiRunnerPool,
        ResidencyStoreClass::CiLogTier,
        ResidencyStoreClass::CiArtifactStore,
        ResidencyStoreClass::CiCacheNamespaces,
    ];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RequiredStoreSet {
    M1Only,
    M1AndCi,
}

impl RequiredStoreSet {
    pub fn required_classes(self) -> &'static [ResidencyStoreClass] {
        match self {
            RequiredStoreSet::M1Only => &ResidencyStoreClass::M1_SET,
            RequiredStoreSet::M1AndCi => &ResidencyStoreClass::M1_AND_CI_SET,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            RequiredStoreSet::M1Only => "m1",
            RequiredStoreSet::M1AndCi => "m1+ci",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreRegionReport {
    pub store_class: ResidencyStoreClass,
    pub region: Region,
}

impl StoreRegionReport {
    pub fn new(store_class: ResidencyStoreClass, region: Region) -> StoreRegionReport {
        StoreRegionReport {
            store_class,
            region,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResidencyMismatch {
    WrongRegion {
        tenant: TenantId,
        tenant_region: Region,
        store_class: ResidencyStoreClass,
        store_region: Region,
    },
    MissingStoreReport {
        tenant: TenantId,
        store_class: ResidencyStoreClass,
    },
}

impl std::fmt::Display for ResidencyMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResidencyMismatch::WrongRegion {
                tenant,
                tenant_region,
                store_class,
                store_region,
            } => write!(
                f,
                "residency_verify FAILED for tenant `{}`: the `{}` store served data in region `{}` \
                 but the tenant is pinned to region `{}` - every store must report the tenant's \
                 region (no-global-pool, architecture §5.4). The attestation FAILS (not a silent \
                 pass, EI-01 §3).",
                tenant.as_str(),
                store_class.label(),
                store_region.as_str(),
                tenant_region.as_str()
            ),
            ResidencyMismatch::MissingStoreReport { tenant, store_class } => write!(
                f,
                "residency_verify FAILED for tenant `{}`: the M1 store class `{}` never reported its \
                 region - a silently-absent store is the global-pool the no-global-pool attestation \
                 must catch (fail-closed, architecture §5.4).",
                tenant.as_str(),
                store_class.label()
            ),
        }
    }
}

impl std::error::Error for ResidencyMismatch {}

#[derive(Clone)]
pub struct ResidencySigningKey {
    key: [u8; 32],
}

impl ResidencySigningKey {
    pub fn from_bytes(key: [u8; 32]) -> ResidencySigningKey {
        ResidencySigningKey { key }
    }

    fn mac(&self, body: &[u8]) -> String {
        let digest = blake3::keyed_hash(&self.key, body);
        format!("blake3-mac:{}", hex::encode(digest.as_bytes()))
    }
}

impl std::fmt::Debug for ResidencySigningKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResidencySigningKey")
            .field("key", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedAttestation {
    pub tenant_id: TenantId,
    pub region: Region,
    pub coverage: RequiredStoreSet,
    pub store_regions: Vec<(ResidencyStoreClass, Region)>,
    pub signature: String,
}

impl SignedAttestation {
    fn canonical_body(
        tenant_id: &TenantId,
        region: &Region,
        coverage: RequiredStoreSet,
        store_regions: &[(ResidencyStoreClass, Region)],
    ) -> Vec<u8> {
        let mut body = String::new();
        body.push_str("residency-attestation\x1f");
        body.push_str(tenant_id.as_str());
        body.push('\x1f');
        body.push_str(region.as_str());
        body.push('\x1f');
        body.push_str("coverage=");
        body.push_str(coverage.label());
        for (class, r) in store_regions {
            body.push('\x1f');
            body.push_str(class.label());
            body.push('=');
            body.push_str(r.as_str());
        }
        body.into_bytes()
    }

    pub fn verify(&self, key: &ResidencySigningKey) -> bool {
        let body = Self::canonical_body(
            &self.tenant_id,
            &self.region,
            self.coverage,
            &self.store_regions,
        );
        key.mac(&body) == self.signature
    }
}

pub fn residency_verify(
    tenant_id: &TenantId,
    region: &Region,
    reports: &[StoreRegionReport],
    key: &ResidencySigningKey,
) -> Result<SignedAttestation, ResidencyMismatch> {
    residency_verify_over(tenant_id, region, RequiredStoreSet::M1Only, reports, key)
}

pub fn residency_verify_ci(
    tenant_id: &TenantId,
    region: &Region,
    reports: &[StoreRegionReport],
    key: &ResidencySigningKey,
) -> Result<SignedAttestation, ResidencyMismatch> {
    residency_verify_over(tenant_id, region, RequiredStoreSet::M1AndCi, reports, key)
}

pub fn residency_verify_over(
    tenant_id: &TenantId,
    region: &Region,
    coverage: RequiredStoreSet,
    reports: &[StoreRegionReport],
    key: &ResidencySigningKey,
) -> Result<SignedAttestation, ResidencyMismatch> {
    let mut by_class: BTreeMap<ResidencyStoreClass, Region> = BTreeMap::new();
    for report in reports {
        if report.region != *region {
            return Err(ResidencyMismatch::WrongRegion {
                tenant: tenant_id.clone(),
                tenant_region: region.clone(),
                store_class: report.store_class,
                store_region: report.region.clone(),
            });
        }
        by_class.insert(report.store_class, report.region.clone());
    }

    for &class in coverage.required_classes() {
        if !by_class.contains_key(&class) {
            return Err(ResidencyMismatch::MissingStoreReport {
                tenant: tenant_id.clone(),
                store_class: class,
            });
        }
    }

    let store_regions: Vec<(ResidencyStoreClass, Region)> = by_class.into_iter().collect();
    let body = SignedAttestation::canonical_body(tenant_id, region, coverage, &store_regions);
    let signature = key.mac(&body);
    Ok(SignedAttestation {
        tenant_id: tenant_id.clone(),
        region: region.clone(),
        coverage,
        store_regions,
        signature,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResidencyAttestationSignal {
    pub tenant_id: TenantId,
    pub region: Region,
    pub stores_attested: u32,
    pub region_mismatches: u32,
}

impl ResidencyAttestationSignal {
    pub fn green(attestation: &SignedAttestation) -> ResidencyAttestationSignal {
        ResidencyAttestationSignal {
            tenant_id: attestation.tenant_id.clone(),
            region: attestation.region.clone(),
            stores_attested: attestation.store_regions.len() as u32,
            region_mismatches: 0,
        }
    }

    pub fn red(
        tenant_id: TenantId,
        region: Region,
        region_mismatches: u32,
    ) -> ResidencyAttestationSignal {
        ResidencyAttestationSignal {
            tenant_id,
            region,
            stores_attested: 0,
            region_mismatches,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> ResidencySigningKey {
        ResidencySigningKey::from_bytes([7u8; 32])
    }

    fn all_in_region(region: &str) -> Vec<StoreRegionReport> {
        ResidencyStoreClass::M1_SET
            .iter()
            .map(|c| StoreRegionReport::new(*c, Region::new(region)))
            .collect()
    }

    fn all_in_region_with_ci(region: &str) -> Vec<StoreRegionReport> {
        ResidencyStoreClass::M1_AND_CI_SET
            .iter()
            .map(|c| StoreRegionReport::new(*c, Region::new(region)))
            .collect()
    }

    #[test]
    fn residency_verify_aggregates_every_store_region() {
        let tenant = TenantId::from_token("01J0ACME");
        let region = Region::new("fr-par");
        let att = residency_verify(&tenant, &region, &all_in_region("fr-par"), &key())
            .expect("every store in-region → a signed attestation");
        assert_eq!(att.tenant_id, tenant);
        assert_eq!(att.region.as_str(), "fr-par");
        assert_eq!(att.store_regions.len(), ResidencyStoreClass::M1_SET.len());
        for (class, r) in &att.store_regions {
            assert_eq!(
                r.as_str(),
                "fr-par",
                "store `{}` reported the tenant's region",
                class.label()
            );
        }
        assert!(
            att.signature.starts_with("blake3-mac:"),
            "the attestation is signed: {}",
            att.signature
        );
        assert!(
            att.verify(&key()),
            "the attestation verifies under the signing key"
        );
    }

    #[test]
    fn residency_verify_fails_on_a_wrong_region_store() {
        let tenant = TenantId::from_token("01J0ACME");
        let region = Region::new("fr-par");
        let mut reports = all_in_region("fr-par");
        reports[1] = StoreRegionReport::new(ResidencyStoreClass::Blob, Region::new("eu-north"));
        let err = residency_verify(&tenant, &region, &reports, &key())
            .expect_err("a wrong-region store FAILS the attestation (not a silent pass)");
        assert_eq!(
            err,
            ResidencyMismatch::WrongRegion {
                tenant: tenant.clone(),
                tenant_region: Region::new("fr-par"),
                store_class: ResidencyStoreClass::Blob,
                store_region: Region::new("eu-north"),
            }
        );
        assert!(
            err.to_string().contains("no-global-pool"),
            "loud reason: {err}"
        );
        assert!(
            err.to_string().contains("not a silent pass"),
            "loud reason: {err}"
        );
    }

    #[test]
    fn residency_verify_fails_on_a_missing_store_report() {
        let tenant = TenantId::from_token("01J0ACME");
        let region = Region::new("fr-par");
        let reports: Vec<StoreRegionReport> = all_in_region("fr-par")
            .into_iter()
            .filter(|r| r.store_class != ResidencyStoreClass::Kms)
            .collect();
        let err = residency_verify(&tenant, &region, &reports, &key())
            .expect_err("a missing M1 store report FAILS fail-closed");
        assert_eq!(
            err,
            ResidencyMismatch::MissingStoreReport {
                tenant: tenant.clone(),
                store_class: ResidencyStoreClass::Kms,
            }
        );
        assert!(
            err.to_string().contains("fail-closed"),
            "loud reason: {err}"
        );
    }

    #[test]
    fn a_tampered_attestation_fails_verification() {
        let tenant = TenantId::from_token("01J0ACME");
        let region = Region::new("fr-par");
        let mut att = residency_verify(&tenant, &region, &all_in_region("fr-par"), &key())
            .expect("a signed attestation");
        assert!(att.verify(&key()), "the genuine attestation verifies");
        att.region = Region::new("eu-north");
        assert!(
            !att.verify(&key()),
            "a tampered region MUST fail verification (the MAC binds it)"
        );
        let mut forged = residency_verify(&tenant, &region, &all_in_region("fr-par"), &key())
            .expect("a signed attestation");
        forged.signature = "blake3-mac:deadbeef".into();
        assert!(!forged.verify(&key()), "a forged signature does not verify");
        let other = ResidencySigningKey::from_bytes([9u8; 32]);
        let genuine = residency_verify(&tenant, &region, &all_in_region("fr-par"), &key())
            .expect("a signed attestation");
        assert!(
            !genuine.verify(&other),
            "a different key does not verify the attestation"
        );
    }

    #[test]
    fn the_attestation_is_pii_free() {
        let tenant = TenantId::from_token("01J0ACME");
        let region = Region::new("fr-par");
        let att = residency_verify(&tenant, &region, &all_in_region("fr-par"), &key())
            .expect("a signed attestation");
        for (class, r) in &att.store_regions {
            assert!(
                matches!(
                    class,
                    ResidencyStoreClass::Oltp
                        | ResidencyStoreClass::Blob
                        | ResidencyStoreClass::IndexSearch
                        | ResidencyStoreClass::Kms
                ),
                "every store-class is an M1 class"
            );
            assert_eq!(r.as_str(), "fr-par");
        }
        let dbg = format!("{:?}", key());
        assert!(
            dbg.contains("<redacted>"),
            "the signing key Debug redacts the secret: {dbg}"
        );
        assert!(!dbg.contains("7"), "the key bytes are not logged: {dbg}");
    }

    #[test]
    fn residency_attestation_signal_green_and_red() {
        let tenant = TenantId::from_token("01J0ACME");
        let region = Region::new("fr-par");
        let att = residency_verify(&tenant, &region, &all_in_region("fr-par"), &key())
            .expect("a signed attestation");
        let green = ResidencyAttestationSignal::green(&att);
        assert_eq!(
            green.stores_attested,
            ResidencyStoreClass::M1_SET.len() as u32
        );
        assert_eq!(
            green.region_mismatches, 0,
            "the green artifact is 0 mismatches"
        );
        assert_eq!(green.region.as_str(), "fr-par");

        let red = ResidencyAttestationSignal::red(tenant, region, 1);
        assert_eq!(red.region_mismatches, 1, "a residency breach reads RED");
    }

    #[test]
    fn the_m1_store_set_is_oltp_blob_index_kms() {
        assert_eq!(
            ResidencyStoreClass::M1_SET.len(),
            4,
            "the M1 set is OLTP/blob/index/KMS"
        );
        let labels: Vec<&str> = ResidencyStoreClass::M1_SET
            .iter()
            .map(|c| c.label())
            .collect();
        assert_eq!(labels, vec!["oltp", "blob", "index_search", "kms"]);
        for ci in ResidencyStoreClass::CI_SET {
            assert!(
                !ResidencyStoreClass::M1_SET.contains(&ci),
                "the CI surface `{}` is in CI_SET, not M1_SET",
                ci.label()
            );
        }
    }

    #[test]
    fn the_ci_store_set_is_runner_log_artifact_cache() {
        let ci_labels: Vec<&str> = ResidencyStoreClass::CI_SET
            .iter()
            .map(|c| c.label())
            .collect();
        assert_eq!(
            ci_labels,
            vec![
                "ci_runner_pool",
                "ci_log_tier",
                "ci_artifact_store",
                "ci_cache_namespaces"
            ]
        );
        assert_eq!(ResidencyStoreClass::M1_AND_CI_SET.len(), 8);
        for c in ResidencyStoreClass::M1_SET {
            assert!(ResidencyStoreClass::M1_AND_CI_SET.contains(&c));
        }
        for c in ResidencyStoreClass::CI_SET {
            assert!(ResidencyStoreClass::M1_AND_CI_SET.contains(&c));
        }
        assert_eq!(
            RequiredStoreSet::M1AndCi.required_classes(),
            &ResidencyStoreClass::M1_AND_CI_SET
        );
        assert_eq!(
            RequiredStoreSet::M1Only.required_classes(),
            &ResidencyStoreClass::M1_SET
        );
    }

    #[test]
    fn residency_verify_ci_aggregates_m1_and_ci() {
        let tenant = TenantId::from_token("01J0ACME");
        let region = Region::new("fr-par");
        let att = residency_verify_ci(&tenant, &region, &all_in_region_with_ci("fr-par"), &key())
            .expect("every M1 + CI store in-region → a signed CI-coverage attestation");
        assert_eq!(att.coverage, RequiredStoreSet::M1AndCi);
        assert_eq!(
            att.store_regions.len(),
            ResidencyStoreClass::M1_AND_CI_SET.len(),
            "the attestation aggregates ALL 8 (M1 + CI) stores - none silently absent"
        );
        for ci in ResidencyStoreClass::CI_SET {
            assert!(
                att.store_regions.iter().any(|(c, _)| *c == ci),
                "CI surface `{}` is attested",
                ci.label()
            );
        }
        assert!(att.verify(&key()), "the CI-coverage attestation verifies");
    }

    #[test]
    fn residency_verify_ci_fails_on_a_wrong_region_ci_runner() {
        let tenant = TenantId::from_token("01J0ACME");
        let region = Region::new("fr-par");
        let mut reports = all_in_region_with_ci("fr-par");
        let idx = reports
            .iter()
            .position(|r| r.store_class == ResidencyStoreClass::CiRunnerPool)
            .unwrap();
        reports[idx] =
            StoreRegionReport::new(ResidencyStoreClass::CiRunnerPool, Region::new("eu-north"));
        let err = residency_verify_ci(&tenant, &region, &reports, &key())
            .expect_err("a wrong-region CI runner FAILS the attestation (not a silent pass)");
        assert_eq!(
            err,
            ResidencyMismatch::WrongRegion {
                tenant: tenant.clone(),
                tenant_region: Region::new("fr-par"),
                store_class: ResidencyStoreClass::CiRunnerPool,
                store_region: Region::new("eu-north"),
            }
        );
        assert!(err.to_string().contains("not a silent pass"), "loud: {err}");
    }

    #[test]
    fn residency_verify_ci_fails_on_a_missing_ci_store() {
        let tenant = TenantId::from_token("01J0ACME");
        let region = Region::new("fr-par");
        let reports: Vec<StoreRegionReport> = all_in_region_with_ci("fr-par")
            .into_iter()
            .filter(|r| r.store_class != ResidencyStoreClass::CiArtifactStore)
            .collect();
        let err = residency_verify_ci(&tenant, &region, &reports, &key())
            .expect_err("a missing CI artifact-store report FAILS fail-closed");
        assert_eq!(
            err,
            ResidencyMismatch::MissingStoreReport {
                tenant: tenant.clone(),
                store_class: ResidencyStoreClass::CiArtifactStore,
            }
        );
        assert!(err.to_string().contains("fail-closed"), "loud: {err}");
    }

    #[test]
    fn coverage_is_bound_into_the_signature() {
        let tenant = TenantId::from_token("01J0ACME");
        let region = Region::new("fr-par");
        let mut m1 = residency_verify(&tenant, &region, &all_in_region("fr-par"), &key())
            .expect("an M1 attestation");
        assert_eq!(m1.coverage, RequiredStoreSet::M1Only);
        assert!(m1.verify(&key()));
        m1.coverage = RequiredStoreSet::M1AndCi;
        assert!(
            !m1.verify(&key()),
            "an M1-only attestation cannot be passed off as an M1+CI one - the coverage is signed"
        );

        let m1_over_ci =
            residency_verify(&tenant, &region, &all_in_region_with_ci("fr-par"), &key())
                .expect("M1-only verify over a superset of reports still succeeds");
        assert_eq!(m1_over_ci.coverage, RequiredStoreSet::M1Only);
        let mut wrong_ci = all_in_region_with_ci("fr-par");
        let idx = wrong_ci
            .iter()
            .position(|r| r.store_class == ResidencyStoreClass::CiLogTier)
            .unwrap();
        wrong_ci[idx] =
            StoreRegionReport::new(ResidencyStoreClass::CiLogTier, Region::new("eu-north"));
        assert!(
            residency_verify(&tenant, &region, &wrong_ci, &key()).is_err(),
            "a wrong-region CI log tier is caught even by the M1-only entry point"
        );
    }

    #[test]
    fn cdc_12_4_residency_verify_provider_consumer() {
        let tenant = TenantId::from_token("01J0ACME");
        let region = Region::new("fr-par");
        let signing_key = key();

        struct AuditorVerdict {
            tenant: String,
            region: String,
            stores_attested: usize,
            verified: bool,
        }
        impl AuditorVerdict {
            fn from_attestation(
                att: &SignedAttestation,
                key: &ResidencySigningKey,
            ) -> AuditorVerdict {
                AuditorVerdict {
                    tenant: att.tenant_id.as_str().to_string(),
                    region: att.region.as_str().to_string(),
                    stores_attested: att.store_regions.len(),
                    verified: att.verify(key),
                }
            }
        }

        let att = residency_verify(&tenant, &region, &all_in_region("fr-par"), &signing_key)
            .expect("a signed attestation");

        let verdict = AuditorVerdict::from_attestation(&att, &signing_key);
        assert_eq!(verdict.tenant, "01J0ACME");
        assert_eq!(verdict.region, "fr-par");
        assert_eq!(verdict.stores_attested, ResidencyStoreClass::M1_SET.len());
        assert!(
            verdict.verified,
            "the auditor verifies the no-global-pool attestation"
        );
    }
}
