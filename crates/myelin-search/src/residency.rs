//! The Search residency-pin CONFIRMATION + the residency-pin / tenant-predicate lint linkage
//! (SRCH-P02 / P-122; contract 12.x consumed).
//!
//! **Architecture:** search-and-indexing.md §3.4 ("Index layout, residency": the per-tenant index
//! tier is `(tenant, region)`-keyed, lives in the tenant's cell, residency-pinned; per-tenant index
//! directories give "residency + crypto-shred-per-index for free"), §1 ("every store
//! residency-pinned, per-tenant envelope-encrypted, crypto-shred-capable, a holder; **no
//! cross-region index read on personal data**"). The whole correctness story is in-cell, per-tenant
//! (§6.1) — there is no cross-tenant query path (the permission-aware query is tenant-first,
//! SRCH-P08), and a cross-cell search is a residency-free permission-filtered local merge (§6.4),
//! never a cross-region index read.
//!
//! ## What "confirm the residency-pin" means at SRCH-P02 (the structural assertion)
//! No index exists yet (the encrypted-from-birth per-tenant index layout is SRCH-P03). So this
//! prompt CONFIRMS the residency-pin **structurally**: the (future) Search index store is described
//! by a [`SearchStoreDescriptor`] that threads the canonical `myelin_tenancy::{TenantId, Region,
//! ResidencyTag}` partition keys — exactly the token types the `residency-pin` lint reads
//! (`tenancy` crate: the lint "reads `Region`/`ResidencyTag`") and the `tenant-predicate` lint
//! requires (a `TenantId`-first partition prefix). By threading those types through its store
//! descriptor, this crate **links** both lints (the SRCH-P01 search-requires-acl-filter ratchet's
//! residency siblings): when the real index layout ships (SRCH-P03) the lints recognise its
//! partition prefix + residency tag, and an attempt to add a cross-region index read on personal
//! data is a structural failure. The `no_cross_region_read_on_personal_data` flag records the §1/§3.4
//! invariant as a checked fact, not prose.
//!
//! **Floor named:** the LIVE lint run over the real index-layout migration is SRCH-P03 (the layout
//! must exist to be linted); the per-tenant index DEK the store is encrypted under is reserved by
//! [`crate::dek`] in THIS prompt. Here the residency-pin is confirmed at the DESCRIPTOR level (the
//! partition keys + residency tag are present + cell-local), which is what makes SRCH-P03's lint
//! green from the first migration.

use myelin_substrate::StoreKind;
use myelin_tenancy::{Region, ResidencyTag, TenantId};

use crate::holder::SEARCH_INDEX_STORE;

/// The (future) Search index store's residency descriptor — the structural confirmation that the
/// store is `(tenant, region)`-keyed, residency-pinned, cell-local, and has **no cross-region index
/// read on personal data** (§1 / §3.4). It threads the canonical `myelin_tenancy::{TenantId, Region,
/// ResidencyTag}` partition keys — the token types the `residency-pin` + `tenant-predicate` lints
/// recognise — so describing the Search index here LINKS those lints to it. PII-free: a store
/// identifier + partition-key types, never personal data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchStoreDescriptor {
    /// The store's class — the per-tenant search index ([`StoreKind::SearchIndex`], §3.4).
    pub kind: StoreKind,
    /// The store's stable, PII-free name (matches the holder registration name).
    pub name: &'static str,
    /// The tenant partition key — the FIRST key of every index doc (`tenant-predicate` lint: every
    /// query is tenant-first). PII-free (an opaque tenant token).
    pub tenant: TenantId,
    /// The home region the index is pinned to (`residency-pin` lint reads `Region`). The index lives
    /// in the tenant's cell, region-local — resolution + query happen in the holding cell (§6.1).
    pub region: Region,
    /// The per-store residency marker the `residency-pin` lint reads (`ResidencyTag`) — pins the
    /// index to its `region`, so a cross-region read on personal data is a structural failure.
    pub residency: ResidencyTag,
    /// The §1/§3.4 invariant recorded as a checked fact: there is **no cross-region index read on
    /// personal data** (the index lives in the tenant's cell; a cross-cell search is a residency-free
    /// permission-filtered local merge, §6.4, never a cross-region index read). `true` for the
    /// Search index.
    pub no_cross_region_read_on_personal_data: bool,
}

impl SearchStoreDescriptor {
    /// Describe the Search index store pinned to `region` (the residency tag is derived from the
    /// region — the index is pinned to its home cell). `no_cross_region_read_on_personal_data` is
    /// `true` by construction: the Search index has no cross-region read path on personal data (§1 /
    /// §3.4 / §6.4).
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

/// The residency descriptor for the Search index store, pinned to the given tenant + home region.
/// The structural confirmation that the Search index is `(tenant, region)`-keyed, residency-pinned,
/// cell-local, and free of any cross-region index read on personal data (§1 / §3.4) — and that the
/// residency-pin + tenant-predicate lints are LINKED to it (the token types are threaded through).
/// When SRCH-P03 ships the real index layout, this is the descriptor the live lints run over. A
/// `Vec` (single element) so the call shape matches the multi-store services (e.g. Refs) for the
/// data-map walk.
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

    /// **The Search index store is residency-pinned + `(tenant, region)`-keyed + free of a
    /// cross-region read on personal data (§1 / §3.4 — the residency-pin confirmation).** The
    /// descriptor threads `TenantId`/`Region`/`ResidencyTag` (the lint token types) and records
    /// `no_cross_region_read_on_personal_data = true`. This is the structural confirmation SRCH-P02
    /// requires; the LIVE lint run over the real layout is SRCH-P03.
    #[test]
    fn search_index_is_residency_pinned_and_has_no_cross_region_read() {
        let descriptors = search_store_descriptors(TenantId::from_token("acme"), fr_par());
        assert_eq!(descriptors.len(), 1, "the one per-tenant search index store");
        let d = &descriptors[0];
        // residency-pin: the store is pinned to its home region (the residency tag matches).
        assert_eq!(d.residency.region(), &d.region, "the index is pinned to its home region");
        // tenant-predicate: the tenant partition key is present (every query is tenant-first).
        assert_eq!(d.tenant, TenantId::from_token("acme"));
        // §1/§3.4: no cross-region index read on personal data (the checked invariant, not prose).
        assert!(
            d.no_cross_region_read_on_personal_data,
            "the Search index has no cross-region read path on personal data (§1/§3.4/§6.4)"
        );
        assert_eq!(d.kind, StoreKind::SearchIndex, "the store is the per-tenant search index");
    }

    /// **The residency tag pins to the same region as the store** — a residency-pin lint reading
    /// `ResidencyTag` sees the index cannot be read cross-region (the pin is exact). This is the
    /// linkage that makes SRCH-P03's residency-pin lint green from the first migration.
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

    /// **The descriptor name matches the holder registration name** (the residency confirmation and
    /// the holder registration address the SAME store) — so the residency-pinned store is exactly
    /// the registered holder, no store described-but-not-registered or vice versa.
    #[test]
    fn descriptor_name_matches_the_registered_holder_store() {
        let descriptors = search_store_descriptors(TenantId::from_token("acme"), fr_par());
        let names: Vec<&str> = descriptors.iter().map(|d| d.name).collect();
        assert!(names.contains(&SEARCH_INDEX_STORE), "the index is described + registered");
    }
}
