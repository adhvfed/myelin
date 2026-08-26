use myelin_substrate::StoreKind;
use myelin_tenancy::{Region, ResidencyTag, TenantId};

use crate::store::{REFS_CACHE_STORE, REFS_EDGE_STORE};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefsStoreDescriptor {
    pub kind: StoreKind,
    pub name: &'static str,
    pub tenant: TenantId,
    pub region: Region,
    pub residency: ResidencyTag,
    pub no_cross_tenant_query_path: bool,
}

impl RefsStoreDescriptor {
    pub fn pinned(kind: StoreKind, name: &'static str, tenant: TenantId, region: Region) -> Self {
        RefsStoreDescriptor {
            residency: ResidencyTag::pinned_to(region.clone()),
            kind,
            name,
            tenant,
            region,
            no_cross_tenant_query_path: true,
        }
    }
}

pub fn refs_store_descriptors(tenant: TenantId, region: Region) -> Vec<RefsStoreDescriptor> {
    vec![
        RefsStoreDescriptor::pinned(
            StoreKind::Oltp,
            REFS_EDGE_STORE,
            tenant.clone(),
            region.clone(),
        ),
        RefsStoreDescriptor::pinned(StoreKind::Cache, REFS_CACHE_STORE, tenant, region),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fr_par() -> Region {
        Region("fr-par".into())
    }

    #[test]
    fn both_refs_stores_are_residency_pinned_and_cross_tenant_free() {
        let descriptors = refs_store_descriptors(TenantId::from_token("acme"), fr_par());
        assert_eq!(descriptors.len(), 2, "the edge index + the R2 cache");
        for d in &descriptors {
            assert_eq!(
                d.residency.region(),
                &d.region,
                "the store is pinned to its home region"
            );
            assert_eq!(d.tenant, TenantId::from_token("acme"));
            assert!(
                d.no_cross_tenant_query_path,
                "no Refs store has a cross-tenant query path"
            );
        }
    }

    #[test]
    fn residency_tag_pins_exactly_to_the_home_region() {
        let d = RefsStoreDescriptor::pinned(
            StoreKind::Oltp,
            REFS_EDGE_STORE,
            TenantId::from_token("acme"),
            fr_par(),
        );
        assert_eq!(d.residency, ResidencyTag::pinned_to(fr_par()));
        assert_eq!(d.region, fr_par());
    }

    #[test]
    fn descriptor_names_match_the_registered_holder_stores() {
        let descriptors = refs_store_descriptors(TenantId::from_token("acme"), fr_par());
        let names: Vec<&str> = descriptors.iter().map(|d| d.name).collect();
        assert!(
            names.contains(&REFS_EDGE_STORE),
            "the edge index is described + registered"
        );
        assert!(
            names.contains(&REFS_CACHE_STORE),
            "the R2 cache is described + registered"
        );
    }
}
