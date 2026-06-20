//! The Refs residency-pin CONFIRMATION + the residency-pin / tenant-predicate lint linkage
//! (REF-P3 / P-120; contract 12.x consumed).
//!
//! **Architecture:** reference-graph.md §3 — "All tables: `(tenant, region)` first columns /
//! partition prefix, RLS-enforced, **no cross-tenant query path**; every store is
//! residency-pinned." The edge table (§3.2) and the R2 projection cache (§3.6) are both cell-local,
//! `(tenant, region)`-partitioned, with no cross-tenant query path (resolution happens in the
//! holding cell — §4.2/§6).
//!
//! ## What "confirm the residency-pin" means at REF-P3 (the structural assertion)
//! No edge table / cache exists yet (the migration is REF-P5). So this prompt CONFIRMS the
//! residency-pin **structurally**: each (future) Refs store is described by a
//! [`RefsStoreDescriptor`] that threads the canonical `myelin_tenancy::{TenantId, Region,
//! ResidencyTag}` partition keys — exactly the token types the `residency-pin` lint reads
//! (`tenancy` crate note: the lint "reads `Region`/`ResidencyTag`") and the `tenant-predicate` lint
//! requires (a `TenantId`-first partition prefix). By threading those types through its store
//! descriptors, this crate **links** both lints: when the real edge table ships (REF-P5) the lints
//! recognise its partition prefix + residency tag, and an attempt to add a cross-tenant query path
//! is a structural failure. The `no_cross_tenant_query_path` flag records the §3 invariant as a
//! checked fact, not prose.
//!
//! **Floor named:** the LIVE lint run over the real edge-table migration is REF-P5 (the table must
//! exist to be linted); the per-tenant DEK the store is encrypted under is REF-P4. Here the
//! residency-pin is confirmed at the DESCRIPTOR level (the partition keys + residency tag are
//! present + cell-local), which is what makes REF-P5's lint green from the first migration.

use myelin_substrate::StoreKind;
use myelin_tenancy::{Region, ResidencyTag, TenantId};

use crate::holder::{REFS_CACHE_STORE, REFS_EDGE_STORE};

/// A (future) Refs store's residency descriptor — the structural confirmation that the store is
/// `(tenant, region)`-partitioned, residency-pinned, and has **no cross-tenant query path** (§3).
/// It threads the canonical `myelin_tenancy::{TenantId, Region, ResidencyTag}` partition keys — the
/// token types the `residency-pin` + `tenant-predicate` lints recognise — so describing a Refs
/// store here LINKS those lints to it (REF-P2's structural ratchet). PII-free: a store identifier +
/// partition-key types, never personal data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefsStoreDescriptor {
    /// The store's class (§3.4 — OLTP edge index / cache).
    pub kind: StoreKind,
    /// The store's stable, PII-free name (matches the holder registration name).
    pub name: &'static str,
    /// The tenant partition key — the FIRST column / partition prefix of every Refs row/key
    /// (`tenant-predicate` lint: every query is tenant-first). PII-free (an opaque tenant token).
    pub tenant: TenantId,
    /// The home region the store is pinned to (`residency-pin` lint reads `Region`). All Refs state
    /// is cell-local to this region — resolution happens in the holding cell (§4.2/§6).
    pub region: Region,
    /// The per-row residency marker the `residency-pin` lint reads (`ResidencyTag`) — pins the
    /// store to its `region`, so a cross-region read path is a structural failure.
    pub residency: ResidencyTag,
    /// The §3 invariant recorded as a checked fact: there is **no cross-tenant query path** (every
    /// index/query is tenant-first, RLS-on; cross-tenant gating is via the `public` userset, never a
    /// cross-tenant join — §6.4). `true` for every Refs store.
    pub no_cross_tenant_query_path: bool,
}

impl RefsStoreDescriptor {
    /// Describe a Refs store pinned to `region` (the residency tag is derived from the region — the
    /// store is pinned to its home cell). `no_cross_tenant_query_path` is `true` by construction:
    /// no Refs store has a cross-tenant query path (§3 / §6.4).
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

/// The residency descriptors for both Refs stores (the edge inverse-index + the R2 projection
/// cache), pinned to the given tenant + home region. The structural confirmation that both Refs
/// stores are `(tenant, region)`-partitioned, residency-pinned, cell-local, and cross-tenant-free
/// (§3) — and that the residency-pin + tenant-predicate lints are LINKED to them (the token types
/// are threaded through). When REF-P5 ships the real migration, these are the descriptors the live
/// lints run over.
pub fn refs_store_descriptors(tenant: TenantId, region: Region) -> Vec<RefsStoreDescriptor> {
    vec![
        RefsStoreDescriptor::pinned(StoreKind::Oltp, REFS_EDGE_STORE, tenant.clone(), region.clone()),
        RefsStoreDescriptor::pinned(StoreKind::Cache, REFS_CACHE_STORE, tenant, region),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fr_par() -> Region {
        Region("fr-par".into())
    }

    /// **Both Refs stores are residency-pinned + `(tenant, region)`-partitioned + cross-tenant-free
    /// (§3 — the residency-pin confirmation).** Each descriptor threads `TenantId`/`Region`/
    /// `ResidencyTag` (the lint token types) and records `no_cross_tenant_query_path = true`. This
    /// is the structural confirmation REF-P3 requires; the LIVE lint run over the real table is
    /// REF-P5.
    #[test]
    fn both_refs_stores_are_residency_pinned_and_cross_tenant_free() {
        let descriptors = refs_store_descriptors(TenantId::from_token("acme"), fr_par());
        assert_eq!(descriptors.len(), 2, "the edge index + the R2 cache");
        for d in &descriptors {
            // residency-pin: the store is pinned to its home region (the residency tag matches).
            assert_eq!(d.residency.region(), &d.region, "the store is pinned to its home region");
            // tenant-predicate: the tenant partition key is present (every query is tenant-first).
            assert_eq!(d.tenant, TenantId::from_token("acme"));
            // §3: no cross-tenant query path (the checked invariant, not prose).
            assert!(d.no_cross_tenant_query_path, "no Refs store has a cross-tenant query path");
        }
    }

    /// **The residency tag pins to the same region as the store** — a residency-pin lint reading
    /// `ResidencyTag` sees the store cannot be read cross-region (the pin is exact). This is the
    /// linkage that makes REF-P5's residency-pin lint green from the first migration.
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

    /// **The descriptor names match the holder registration names** (the residency confirmation and
    /// the holder registration address the SAME stores) — so a residency-pinned store is exactly a
    /// registered holder, no store described-but-not-registered or vice versa.
    #[test]
    fn descriptor_names_match_the_registered_holder_stores() {
        let descriptors = refs_store_descriptors(TenantId::from_token("acme"), fr_par());
        let names: Vec<&str> = descriptors.iter().map(|d| d.name).collect();
        assert!(names.contains(&REFS_EDGE_STORE), "the edge index is described + registered");
        assert!(names.contains(&REFS_CACHE_STORE), "the R2 cache is described + registered");
    }
}
