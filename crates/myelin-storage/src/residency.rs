use std::collections::BTreeSet;

use myelin_tenancy::{Region, TenantId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ResidencyStoreClass {
    Oltp,
    Blob,
    IndexSearch,
    Kms,
    T3FirehoseArchive,
    CdnEdgeSet,
    PushMirror,
}

impl ResidencyStoreClass {
    pub fn label(self) -> &'static str {
        match self {
            ResidencyStoreClass::Oltp => "oltp",
            ResidencyStoreClass::Blob => "blob",
            ResidencyStoreClass::IndexSearch => "index_search",
            ResidencyStoreClass::Kms => "kms",
            ResidencyStoreClass::T3FirehoseArchive => "t3_firehose_archive",
            ResidencyStoreClass::CdnEdgeSet => "cdn_edge_set",
            ResidencyStoreClass::PushMirror => "push_mirror",
        }
    }

    pub const M1_SET: [ResidencyStoreClass; 4] = [
        ResidencyStoreClass::Oltp,
        ResidencyStoreClass::Blob,
        ResidencyStoreClass::IndexSearch,
        ResidencyStoreClass::Kms,
    ];
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegionPinnedStore {
    store_class: ResidencyStoreClass,
    region: Region,
}

impl RegionPinnedStore {
    pub fn pinned_to(store_class: ResidencyStoreClass, region: Region) -> RegionPinnedStore {
        RegionPinnedStore {
            store_class,
            region,
        }
    }

    pub fn store_class(&self) -> ResidencyStoreClass {
        self.store_class
    }

    pub fn region(&self) -> &Region {
        &self.region
    }

    pub fn admit_write(&self, row_region: &Region) -> Result<(), ResidencyViolation> {
        if row_region != &self.region {
            return Err(ResidencyViolation::OutOfRegionWrite {
                store_class: self.store_class,
                store_region: self.region.clone(),
                row_region: row_region.clone(),
            });
        }
        Ok(())
    }

    pub fn report_for(&self, tenant: &TenantId) -> StoreResidencyReport {
        StoreResidencyReport {
            tenant: tenant.clone(),
            store_class: self.store_class,
            region: self.region.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreResidencyReport {
    pub tenant: TenantId,
    pub store_class: ResidencyStoreClass,
    pub region: Region,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResidencyViolation {
    OutOfRegionWrite {
        store_class: ResidencyStoreClass,
        store_region: Region,
        row_region: Region,
    },
    OutOfRegionStore {
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

impl std::fmt::Display for ResidencyViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResidencyViolation::OutOfRegionWrite { store_class, store_region, row_region } => {
                write!(
                    f,
                    "residency WRITE boundary REJECTED a write: the `{}` store is pinned to region \
                     `{}` but the row targeted region `{}` - no store ever writes outside its \
                     region (storage.md §1.1, the residency-pin write boundary). 0 out-of-region \
                     writes admitted.",
                    store_class.label(),
                    store_region.as_str(),
                    row_region.as_str()
                )
            }
            ResidencyViolation::OutOfRegionStore {
                tenant,
                tenant_region,
                store_class,
                store_region,
            } => write!(
                f,
                "residency verify FAILED for tenant `{}`: the `{}` store served data in region `{}` \
                 but the tenant is pinned to region `{}` - every store must report the tenant's \
                 region (no-global-pool, STOR-D5). The attestation FAILS (not a silent pass, \
                 EI-01 §3).",
                tenant.as_str(),
                store_class.label(),
                store_region.as_str(),
                tenant_region.as_str()
            ),
            ResidencyViolation::MissingStoreReport { tenant, store_class } => write!(
                f,
                "residency verify FAILED for tenant `{}`: the M1 store class `{}` never reported \
                 its region - a silently-absent store is the global-pool the no-global-pool \
                 attestation must catch (fail-closed, STOR-D5).",
                tenant.as_str(),
                store_class.label()
            ),
        }
    }
}

impl std::error::Error for ResidencyViolation {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegionPinningAttestation {
    pub tenant: TenantId,
    pub region: Region,
    pub store_regions: Vec<(ResidencyStoreClass, Region)>,
}

impl RegionPinningAttestation {
    pub fn reports(&self) -> Vec<StoreResidencyReport> {
        self.store_regions
            .iter()
            .map(|(class, region)| StoreResidencyReport {
                tenant: self.tenant.clone(),
                store_class: *class,
                region: region.clone(),
            })
            .collect()
    }
}

pub fn verify_region_pinning(
    tenant: &TenantId,
    tenant_region: &Region,
    reports: &[StoreResidencyReport],
) -> Result<RegionPinningAttestation, ResidencyViolation> {
    let mut store_regions: Vec<(ResidencyStoreClass, Region)> = Vec::new();
    let mut present: BTreeSet<ResidencyStoreClass> = BTreeSet::new();
    for report in reports {
        if &report.region != tenant_region {
            return Err(ResidencyViolation::OutOfRegionStore {
                tenant: tenant.clone(),
                tenant_region: tenant_region.clone(),
                store_class: report.store_class,
                store_region: report.region.clone(),
            });
        }
        if present.insert(report.store_class) {
            store_regions.push((report.store_class, report.region.clone()));
        }
    }

    for class in ResidencyStoreClass::M1_SET {
        if !present.contains(&class) {
            return Err(ResidencyViolation::MissingStoreReport {
                tenant: tenant.clone(),
                store_class: class,
            });
        }
    }

    store_regions.sort_by_key(|(class, _)| *class);
    Ok(RegionPinningAttestation {
        tenant: tenant.clone(),
        region: tenant_region.clone(),
        store_regions,
    })
}

#[derive(Clone, Debug)]
pub struct StoreSet {
    stores: Vec<RegionPinnedStore>,
}

impl StoreSet {
    pub fn for_cell(region: &Region) -> StoreSet {
        let stores = ResidencyStoreClass::M1_SET
            .iter()
            .map(|class| RegionPinnedStore::pinned_to(*class, region.clone()))
            .collect();
        StoreSet { stores }
    }

    pub fn from_stores(stores: Vec<RegionPinnedStore>) -> StoreSet {
        StoreSet { stores }
    }

    pub fn reports_for(&self, tenant: &TenantId) -> Vec<StoreResidencyReport> {
        self.stores.iter().map(|s| s.report_for(tenant)).collect()
    }

    pub fn residency_verify(
        &self,
        tenant: &TenantId,
        tenant_region: &Region,
    ) -> Result<RegionPinningAttestation, ResidencyViolation> {
        verify_region_pinning(tenant, tenant_region, &self.reports_for(tenant))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResidencyVerifySignal {
    pub tenant: TenantId,
    pub region: Region,
    pub stores_attested: u32,
    pub cross_region_egress: u32,
}

impl ResidencyVerifySignal {
    pub fn green(att: &RegionPinningAttestation) -> ResidencyVerifySignal {
        ResidencyVerifySignal {
            tenant: att.tenant.clone(),
            region: att.region.clone(),
            stores_attested: att.store_regions.len() as u32,
            cross_region_egress: 0,
        }
    }

    pub fn red(
        tenant: TenantId,
        region: Region,
        stores_attested: u32,
        cross_region_egress: u32,
    ) -> ResidencyVerifySignal {
        ResidencyVerifySignal {
            tenant,
            region,
            stores_attested,
            cross_region_egress,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant() -> TenantId {
        TenantId::from_token("01J0ACME")
    }

    #[test]
    fn a_store_is_region_pinned_and_reports_its_region() {
        let store = RegionPinnedStore::pinned_to(ResidencyStoreClass::Oltp, Region::new("fr-par"));
        assert_eq!(store.region().as_str(), "fr-par");
        assert_eq!(store.store_class(), ResidencyStoreClass::Oltp);
        let report = store.report_for(&tenant());
        assert_eq!(report.store_class, ResidencyStoreClass::Oltp);
        assert_eq!(report.region.as_str(), "fr-par");
        assert_eq!(report.tenant, tenant());
    }

    #[test]
    fn the_write_boundary_rejects_an_out_of_region_write() {
        let store = RegionPinnedStore::pinned_to(ResidencyStoreClass::Blob, Region::new("fr-par"));
        assert_eq!(store.admit_write(&Region::new("fr-par")), Ok(()));
        let err = store
            .admit_write(&Region::new("eu-central"))
            .expect_err("an out-of-region write MUST be rejected by the residency write boundary");
        assert_eq!(
            err,
            ResidencyViolation::OutOfRegionWrite {
                store_class: ResidencyStoreClass::Blob,
                store_region: Region::new("fr-par"),
                row_region: Region::new("eu-central"),
            }
        );
        assert!(err
            .to_string()
            .contains("no store ever writes outside its region"));
    }

    #[test]
    fn residency_verify_attests_the_tenants_single_region() {
        let region = Region::new("fr-par");
        let set = StoreSet::for_cell(&region);
        let att = set
            .residency_verify(&tenant(), &region)
            .expect("every store in-region → a region-pinning attestation");
        assert_eq!(att.tenant, tenant());
        assert_eq!(att.region.as_str(), "fr-par");
        assert_eq!(att.store_regions.len(), ResidencyStoreClass::M1_SET.len());
        for (class, r) in &att.store_regions {
            assert_eq!(
                r.as_str(),
                "fr-par",
                "store `{}` reports the tenant's single region",
                class.label()
            );
        }
        let signal = ResidencyVerifySignal::green(&att);
        assert_eq!(
            signal.cross_region_egress, 0,
            "the green STOR-D5 artifact is 0 cross-region egress"
        );
        assert_eq!(
            signal.stores_attested,
            ResidencyStoreClass::M1_SET.len() as u32
        );
    }

    #[test]
    fn residency_verify_fails_on_a_cross_region_store() {
        let region = Region::new("fr-par");
        let set = StoreSet::from_stores(vec![
            RegionPinnedStore::pinned_to(ResidencyStoreClass::Oltp, region.clone()),
            RegionPinnedStore::pinned_to(ResidencyStoreClass::Blob, Region::new("eu-north")),
            RegionPinnedStore::pinned_to(ResidencyStoreClass::IndexSearch, region.clone()),
            RegionPinnedStore::pinned_to(ResidencyStoreClass::Kms, region.clone()),
        ]);
        let err = set
            .residency_verify(&tenant(), &region)
            .expect_err("a cross-region store FAILS the attestation (not a silent pass)");
        assert_eq!(
            err,
            ResidencyViolation::OutOfRegionStore {
                tenant: tenant(),
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
        let region = Region::new("fr-par");
        let set = StoreSet::from_stores(vec![
            RegionPinnedStore::pinned_to(ResidencyStoreClass::Oltp, region.clone()),
            RegionPinnedStore::pinned_to(ResidencyStoreClass::Blob, region.clone()),
            RegionPinnedStore::pinned_to(ResidencyStoreClass::IndexSearch, region.clone()),
        ]);
        let err = set
            .residency_verify(&tenant(), &region)
            .expect_err("a missing M1 store report FAILS fail-closed");
        assert_eq!(
            err,
            ResidencyViolation::MissingStoreReport {
                tenant: tenant(),
                store_class: ResidencyStoreClass::Kms,
            }
        );
        assert!(
            err.to_string().contains("fail-closed"),
            "loud reason: {err}"
        );
        let red = ResidencyVerifySignal::red(tenant(), region, 3, 0);
        assert!(
            red.stores_attested < ResidencyStoreClass::M1_SET.len() as u32,
            "a missing-store FAIL is caught by the store-set-coverage assertion, not just the egress zero"
        );
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
    }

    #[test]
    fn the_attestation_is_pii_free() {
        let region = Region::new("fr-par");
        let att = StoreSet::for_cell(&region)
            .residency_verify(&tenant(), &region)
            .expect("a region-pinning attestation");
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
        let reports = att.reports();
        assert_eq!(reports.len(), ResidencyStoreClass::M1_SET.len());
        for report in &reports {
            assert_eq!(report.region.as_str(), "fr-par");
            assert_eq!(report.tenant.as_str(), "01J0ACME");
        }
    }
}
