use myelin_tenancy::{Region, TenantId};

use crate::blob::{BlobStore, ContentHash};
use crate::residency::{ResidencyStoreClass, StoreResidencyReport};

pub const CDN_MAX_BUNDLE_BYTES: usize = 512 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CdnEdgePop {
    pub id: String,
    pub region: Region,
    pub within_eu: bool,
}

impl CdnEdgePop {
    pub fn new(id: impl Into<String>, region: Region, within_eu: bool) -> CdnEdgePop {
        CdnEdgePop {
            id: id.into(),
            region,
            within_eu,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CdnEdgeSet;

impl CdnEdgeSet {
    pub fn eligible_for<'a>(
        &self,
        tenant_is_eu: bool,
        candidates: &'a [CdnEdgePop],
    ) -> Vec<&'a CdnEdgePop> {
        candidates
            .iter()
            .filter(|pop| {
                !tenant_is_eu || pop.within_eu
            })
            .collect()
    }

    fn attested_region(tenant_region: &Region) -> Region {
        tenant_region.clone()
    }
}

pub struct CdnCloneClass<'a> {
    tenant: TenantId,
    region: Region,
    base: &'a dyn BlobStore,
    tenant_is_eu: bool,
}

impl<'a> CdnCloneClass<'a> {
    pub fn over(
        tenant: TenantId,
        region: Region,
        tenant_is_eu: bool,
        store: &'a dyn BlobStore,
    ) -> CdnCloneClass<'a> {
        CdnCloneClass {
            tenant,
            region,
            base: store,
            tenant_is_eu,
        }
    }

    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    pub fn publish_bundle(&self, bytes: &[u8]) -> Result<ContentHash, crate::blob::BlobError> {
        self.publish_bundle_bounded(bytes, CDN_MAX_BUNDLE_BYTES)
    }

    pub fn publish_bundle_bounded(
        &self,
        bytes: &[u8],
        maximum_bytes: usize,
    ) -> Result<ContentHash, crate::blob::BlobError> {
        if bytes.len() > maximum_bytes {
            return Err(crate::blob::BlobError::SizeLimitExceeded {
                actual: bytes.len(),
                maximum: maximum_bytes,
            });
        }
        self.base.put(&self.tenant, bytes)
    }

    pub fn bundle(&self, hash: &ContentHash) -> Result<Vec<u8>, crate::blob::BlobError> {
        self.bundle_bounded(hash, CDN_MAX_BUNDLE_BYTES)
    }

    pub fn bundle_bounded(
        &self,
        hash: &ContentHash,
        maximum_bytes: usize,
    ) -> Result<Vec<u8>, crate::blob::BlobError> {
        self.base
            .get_bounded(&self.tenant, hash, maximum_bytes)
    }

    pub fn eligible_edges<'p>(&self, candidates: &'p [CdnEdgePop]) -> Vec<&'p CdnEdgePop> {
        CdnEdgeSet.eligible_for(self.tenant_is_eu, candidates)
    }

    pub fn residency_report(&self) -> StoreResidencyReport {
        StoreResidencyReport {
            tenant: self.tenant.clone(),
            store_class: ResidencyStoreClass::CdnEdgeSet,
            region: CdnEdgeSet::attested_region(&self.region),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob::FsBlobStore;
    use crate::residency::{
        verify_region_pinning, RegionPinnedStore, ResidencyStoreClass, StoreSet,
    };

    fn tenant() -> TenantId {
        TenantId::from_token("01J0ACME")
    }

    fn eu_pops() -> Vec<CdnEdgePop> {
        vec![
            CdnEdgePop::new("par-1", Region::new("fr-par"), true),
            CdnEdgePop::new("ams-1", Region::new("nl-ams"), true),
            CdnEdgePop::new("iad-1", Region::new("us-east"), false),
        ]
    }

    #[test]
    fn a_cdn_clone_bundle_is_content_addressed_and_the_address_is_the_validity_check() {
        let store = FsBlobStore::new();
        let cdn = CdnCloneClass::over(tenant(), Region::new("fr-par"), true, &store);

        let bundle_bytes = b"PACK\0clone-bundle-of-hot-repo";
        let addr = cdn.publish_bundle(bundle_bytes).expect("publish bundle");
        assert_eq!(
            cdn.publish_bundle_bounded(bundle_bytes, bundle_bytes.len())
                .expect("exact publish limit accepted"),
            addr
        );
        assert!(matches!(
            cdn.publish_bundle_bounded(bundle_bytes, bundle_bytes.len() - 1),
            Err(crate::blob::BlobError::SizeLimitExceeded { .. })
        ));
        assert_eq!(addr, ContentHash::blake3(bundle_bytes));

        let served = cdn.bundle(&addr).expect("serve bundle by content-address");
        assert_eq!(
            served, bundle_bytes,
            "the served bundle is the exact requested content"
        );
        assert_eq!(
            cdn.bundle_bounded(&addr, bundle_bytes.len())
                .expect("exact read limit accepted"),
            bundle_bytes
        );
        assert!(matches!(
            cdn.bundle_bounded(&addr, bundle_bytes.len() - 1),
            Err(crate::blob::BlobError::SizeLimitExceeded { .. })
        ));

        assert!(
            store.corrupt_for_drill(&tenant(), &addr),
            "bundle present to corrupt"
        );
        assert!(
            matches!(
                cdn.bundle(&addr),
                Err(crate::blob::BlobError::IntegrityFail { .. })
            ),
            "a tampered bundle MUST be refused - the content-address is the cache-validity check"
        );
    }

    #[test]
    fn the_eu_tenant_eligible_edge_set_is_within_eu_only() {
        let store = FsBlobStore::new();
        let cdn = CdnCloneClass::over(tenant(), Region::new("fr-par"), true, &store);
        let candidates = eu_pops();

        let eligible = cdn.eligible_edges(&candidates);
        assert_eq!(
            eligible.len(),
            2,
            "an EU tenant's eligible edge set excludes extra-EU POPs"
        );
        assert!(
            eligible.iter().all(|pop| pop.within_eu),
            "every eligible POP for an EU tenant is within-EU - no PII-bearing bundle reaches an extra-EU edge"
        );
        assert!(
            !eligible.iter().any(|pop| pop.id == "iad-1"),
            "the extra-EU POP is NOT eligible for an EU tenant"
        );
    }

    #[test]
    fn a_non_eu_tenant_has_no_within_eu_restriction() {
        let candidates = eu_pops();
        let eligible = CdnEdgeSet.eligible_for(false, &candidates);
        assert_eq!(
            eligible.len(),
            candidates.len(),
            "a non-EU tenant has no within-EU restriction"
        );
    }

    #[test]
    fn the_residency_attestation_includes_the_cdn_edge_set() {
        let store = FsBlobStore::new();
        let region = Region::new("fr-par");
        let cdn = CdnCloneClass::over(tenant(), region.clone(), true, &store);

        let cdn_report = cdn.residency_report();
        assert_eq!(cdn_report.store_class, ResidencyStoreClass::CdnEdgeSet);
        assert_eq!(cdn_report.region, region);

        let mut reports = StoreSet::for_cell(&region).reports_for(&tenant());
        reports.push(cdn_report);
        let att = verify_region_pinning(&tenant(), &region, &reports)
            .expect("every store (incl. the CDN edge set) reports the tenant's region");
        assert!(
            att.store_regions
                .iter()
                .any(|(class, _)| *class == ResidencyStoreClass::CdnEdgeSet),
            "the residency attestation includes the CDN edge set (12.4)"
        );
    }

    #[test]
    fn a_cross_region_cdn_edge_fails_the_residency_attestation() {
        let region = Region::new("fr-par");
        let bad_cdn = StoreResidencyReport {
            tenant: tenant(),
            store_class: ResidencyStoreClass::CdnEdgeSet,
            region: Region::new("us-east"),
        };
        let mut reports = StoreSet::for_cell(&region).reports_for(&tenant());
        reports.push(bad_cdn);
        let err = verify_region_pinning(&tenant(), &region, &reports).expect_err(
            "a CDN edge in the wrong region FAILs the attestation (0 cross-region egress)",
        );
        assert!(
            err.to_string().contains("no-global-pool"),
            "the CDN cross-region breach is caught by the SAME aggregation: {err}"
        );
    }

    #[test]
    fn the_cdn_class_rides_the_unchanged_base_blobstore() {
        let store = FsBlobStore::new();
        let cdn = CdnCloneClass::over(tenant(), Region::new("fr-par"), true, &store);
        let bytes = b"shared-backing bundle";

        let addr = cdn.publish_bundle(bytes).expect("publish");

        let via_base = BlobStore::get(&store, &tenant(), &addr).expect("base store has the bundle");
        assert_eq!(via_base, bytes);
        assert_eq!(cdn.bundle(&addr).expect("cdn serve"), bytes);
    }

    #[test]
    fn the_cdn_store_class_label_is_stable() {
        assert_eq!(ResidencyStoreClass::CdnEdgeSet.label(), "cdn_edge_set");
        assert!(!RegionPinnedStore::pinned_to(
            ResidencyStoreClass::CdnEdgeSet,
            Region::new("fr-par")
        )
        .region()
        .as_str()
        .is_empty());
        assert!(!ResidencyStoreClass::M1_SET.contains(&ResidencyStoreClass::CdnEdgeSet));
    }
}
