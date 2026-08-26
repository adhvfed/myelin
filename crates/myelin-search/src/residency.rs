use myelin_substrate::StoreKind;
use myelin_tenancy::{Region, ResidencyTag, TenantId};

use crate::store::SEARCH_INDEX_STORE;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchStoreDescriptor {
    pub kind: StoreKind,
    pub name: &'static str,
    pub tenant: TenantId,
    pub region: Region,
    pub residency: ResidencyTag,
    pub no_cross_region_read_on_personal_data: bool,
}

impl SearchStoreDescriptor {
    pub fn pinned(kind: StoreKind, name: &'static str, tenant: TenantId, region: Region) -> Self {
        SearchStoreDescriptor {
            residency: ResidencyTag::pinned_to(region.clone()),
            kind,
            name,
            tenant,
            region,
            no_cross_region_read_on_personal_data: true,
        }
    }
}

pub fn search_store_descriptors(tenant: TenantId, region: Region) -> Vec<SearchStoreDescriptor> {
    vec![SearchStoreDescriptor::pinned(
        StoreKind::SearchIndex,
        SEARCH_INDEX_STORE,
        tenant,
        region,
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fr_par() -> Region {
        Region("fr-par".into())
    }

    #[test]
    fn search_index_is_residency_pinned_and_has_no_cross_region_read() {
        let descriptors = search_store_descriptors(TenantId::from_token("acme"), fr_par());
        assert_eq!(
            descriptors.len(),
            1,
            "the one per-tenant search index store"
        );
        let d = &descriptors[0];
        assert_eq!(
            d.residency.region(),
            &d.region,
            "the index is pinned to its home region"
        );
        assert_eq!(d.tenant, TenantId::from_token("acme"));
        assert!(
            d.no_cross_region_read_on_personal_data,
            "the Search index has no cross-region read path on personal data (§1/§3.4/§6.4)"
        );
        assert_eq!(
            d.kind,
            StoreKind::SearchIndex,
            "the store is the per-tenant search index"
        );
    }

    #[test]
    fn residency_tag_pins_exactly_to_the_home_region() {
        let d = SearchStoreDescriptor::pinned(
            StoreKind::SearchIndex,
            SEARCH_INDEX_STORE,
            TenantId::from_token("acme"),
            fr_par(),
        );
        assert_eq!(d.residency, ResidencyTag::pinned_to(fr_par()));
        assert_eq!(d.region, fr_par());
    }

    #[test]
    fn descriptor_name_matches_the_registered_holder_store() {
        let descriptors = search_store_descriptors(TenantId::from_token("acme"), fr_par());
        let names: Vec<&str> = descriptors.iter().map(|d| d.name).collect();
        assert!(
            names.contains(&SEARCH_INDEX_STORE),
            "the index is described + registered"
        );
    }
}
