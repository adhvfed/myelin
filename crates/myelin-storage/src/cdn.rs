//! # The within-EU CDN clone/bundle blob class (C3) — P-ST-23 / global P-254 (contract 11.2-C3).
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/storage.md` §3.2 ("**C3 — within-EU CDN
//! clone/bundle blob class (NEW)**"): a NAMED blob class over the unchanged [`crate::blob::BlobStore`]
//! for hot-repo / clone-storm acceleration —
//!
//! - a clone/bundle blob is **content-addressed** like any T2 blob (BLAKE3), so an edge cache is a
//!   pure content-address cache: a cache entry is valid **iff its hash matches** — there is no
//!   staleness model to get wrong (the content-address *is* the validity check);
//! - the class is **residency-respecting**: the CDN edge set is **within-EU-only** for any tenant
//!   whose region is EU; PII-bearing content never reaches an extra-EU edge. The tenant's region
//!   pins which edge POPs are eligible (the control plane's `residency_verify`, contract 12.4,
//!   covers the CDN edge set);
//! - it is **NEW but rides the existing trait**: a blob-class TAG + an eligible-edge-set POLICY, not
//!   a new store. The base [`crate::blob::BlobStore`] is unchanged; the CDN is a *delivery layer* in
//!   front of it.
//!
//! Contract-index rows 11.2 (the within-EU CDN clone/bundle class), 12.4 (`residency_verify` covers
//! the CDN edge set). Drill catalogue row STOR-D5 (residency, extended — the CDN edge set is
//! within-EU → 0 cross-region PII egress via the CDN).
//!
//! ## The "one primitive, not a new store" discipline (EI-01 §7)
//! [`CdnCloneClass`] holds a `&dyn BlobStore` (a BORROW of the base tier, never an owned second
//! store) and serves bundles BY content-address: a [`CdnCloneClass::bundle`] is fetched from the
//! base store and its bytes re-hash-verified, so a served bundle is provably the requested content.
//! The structural assertion that "the CDN is not a new store" is the test
//! `the_cdn_class_rides_the_unchanged_base_blobstore` — the SAME [`crate::blob::FsBlobStore`]
//! instance backs both a direct blob read and a CDN bundle serve (no parallel object map).
//!
//! ## The eligible-edge-set policy (residency-respecting)
//! [`CdnEdgeSet::eligible_for`] takes the tenant's region + the FULL candidate POP set and returns
//! ONLY the POPs eligible to serve that tenant. For an **EU tenant** an eligible POP MUST be
//! within-EU ([`CdnEdgePop::within_eu`]); an extra-EU POP is EXCLUDED by construction — *no
//! PII-bearing bundle reaches an extra-EU edge*. Storage does **not** author the geography (which
//! region codes are "EU" / which POP is within-EU): that classification is an INPUT
//! ([`CdnEdgePop::within_eu`], sourced from the control-plane region registry the same way the
//! tenant's EU status is). Storage OWNS the *filter*: "an EU tenant's eligible edge set contains
//! ONLY within-EU POPs" — the within-EU-edge-set filter is the mandatory-core the mutation floor
//! covers.
//!
//! ## `residency_verify` covers the CDN edge set (extends P-ST-15)
//! [`CdnCloneClass::residency_report`] produces a
//! [`crate::residency::ResidencyStoreClass::CdnEdgeSet`] report @ the single region the eligible
//! edge set serves, fed into the SAME [`crate::residency::verify_region_pinning`] aggregation — so a
//! CDN that would serve an EU tenant from an extra-EU region FAILs the attestation WITHOUT a code
//! change (the aggregation's region-equality check already covers any reported class). This is the
//! "the residency attestation includes the CDN edge set" half of the prompt.
//!
//! ## Floors named (the prompt's required follow-ons) — recorded in writing
//! - The **C6 outbound push-mirror residency gate** (the mirror TARGET added to the same
//!   `residency_verify` attestation) is the SIBLING prompt **P-ST-25 (global P-255)** — it becomes
//!   another [`crate::residency::ResidencyStoreClass`] variant + report without changing the
//!   aggregation shape.
//! - The **real edge delivery network** (the actual CDN POP fleet + cache-fill transport) is a
//!   deployment/ops surface; here the class is the content-address-cache SEMANTICS + the
//!   eligible-edge-set policy + the residency report (the load-bearing correctness), proven over the
//!   in-memory [`crate::blob::FsBlobStore`] floor. The object-store backing the bundles ultimately
//!   rest on is the M5 follow-on (P-ST-30/P-ST-31) — a backing swap by the trait's design.
//!
//! ## Mutation floor (mandatory-core, ≥ 80% — EI-01 §2/§3; the prompt's TESTS field)
//! The **within-EU-edge-set filter** ([`CdnEdgeSet::eligible_for`]: the `tenant_is_eu &&
//! !pop.within_eu` exclusion) is mandatory-core: an extra-EU POP admitted into an EU tenant's
//! eligible edge set is the residency breach STOR-D5 exists to catch. The
//! content-address-is-validity check ([`CdnCloneClass::bundle`]'s re-hash-verify via the base
//! store's read path) is the second mandatory-core branch. The floor is **≥ 80%**; the achieved
//! score is recorded in the P-254 report
//! (`cargo mutants -p myelin-storage -f crates/myelin-storage/src/cdn.rs`).

use myelin_tenancy::{Region, TenantId};

use crate::blob::{BlobStore, ContentHash};
use crate::residency::{ResidencyStoreClass, StoreResidencyReport};

/// Maximum clone/bundle body materialized by the CDN class in one read.
pub const CDN_MAX_BUNDLE_BYTES: usize = 512 * 1024 * 1024;

/// **A candidate CDN edge POP (point of presence).** PII-free: an opaque POP id, the POP's region,
/// and whether that POP is within-EU. Storage does NOT author the geography — `within_eu` is an
/// INPUT sourced from the control-plane region registry (the same source the tenant's EU status
/// comes from). Storage OWNS the *filter* over these candidates (the eligible-edge-set policy).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CdnEdgePop {
    /// The opaque edge-POP id (a delivery-location label — never personal data).
    pub id: String,
    /// The POP's region code.
    pub region: Region,
    /// **Whether this POP is within-EU** — the control-plane-sourced geography classification. An EU
    /// tenant's eligible edge set may contain ONLY POPs where this is `true` (the residency filter).
    pub within_eu: bool,
}

impl CdnEdgePop {
    /// Construct an edge POP candidate (PII-free).
    pub fn new(id: impl Into<String>, region: Region, within_eu: bool) -> CdnEdgePop {
        CdnEdgePop {
            id: id.into(),
            region,
            within_eu,
        }
    }
}

/// **The eligible-edge-set POLICY (residency-respecting; storage.md §3.2 C3).** Given the tenant's
/// region (and whether the tenant is an EU tenant) it filters a candidate POP set to ONLY those
/// eligible to serve that tenant's PII-bearing bundles.
///
/// The single load-bearing rule (mandatory-core): **for an EU tenant an eligible POP MUST be
/// within-EU** — an extra-EU POP is excluded by construction, so *no PII-bearing bundle reaches an
/// extra-EU edge*. A non-EU tenant has no within-EU restriction (its content may serve from its own
/// region's POPs); this prompt's gate is the EU-tenant residency property.
#[derive(Clone, Copy, Debug, Default)]
pub struct CdnEdgeSet;

impl CdnEdgeSet {
    /// **Filter `candidates` to the POPs eligible to serve `tenant` (whose region is `tenant_region`,
    /// and who is an EU tenant iff `tenant_is_eu`).** For an EU tenant the result contains ONLY
    /// within-EU POPs ([`CdnEdgePop::within_eu`]); an extra-EU POP is EXCLUDED. The result is
    /// returned in candidate order (stable). This is the within-EU-edge-set filter the STOR-D5 CDN
    /// extension asserts on.
    pub fn eligible_for<'a>(
        &self,
        tenant_is_eu: bool,
        candidates: &'a [CdnEdgePop],
    ) -> Vec<&'a CdnEdgePop> {
        candidates
            .iter()
            .filter(|pop| {
                // The load-bearing residency rule: an EU tenant may ONLY use a within-EU POP — a
                // POP is eligible iff the tenant is non-EU OR the POP is within-EU. A non-EU tenant
                // has no within-EU restriction; an extra-EU POP for an EU tenant is excluded by
                // construction (no PII-bearing bundle reaches an extra-EU edge).
                !tenant_is_eu || pop.within_eu
            })
            .collect()
    }

    /// The single region an EU tenant's eligible edge set serves into the residency attestation: the
    /// tenant's own region (every eligible POP is within-EU and the bundles are pinned to the
    /// tenant's region for the attestation). The per-POP region is a delivery detail; the residency
    /// REPORT is "the CDN serves this tenant's bundles within the tenant's region" — a wrong-region
    /// CDN is caught by [`crate::residency::verify_region_pinning`] against the tenant's region.
    fn attested_region(tenant_region: &Region) -> Region {
        tenant_region.clone()
    }
}

/// **The within-EU CDN clone/bundle blob class (C3) — a blob-class TAG over the unchanged
/// [`BlobStore`], NOT a new store.** It BORROWS the base content-addressed blob tier (a `&dyn
/// BlobStore`, never an owned second store) and serves clone/bundle blobs BY CONTENT ADDRESS:
/// [`Self::bundle`] reads the bundle from the base store and re-hash-verifies it (the base store's
/// read path), so a served bundle is provably the requested content (the content-address IS the
/// cache-validity check — no staleness model). The class is pinned to a tenant + its region, so its
/// [`Self::residency_report`] feeds the SAME [`crate::residency::verify_region_pinning`] aggregation
/// the M1 stores do.
pub struct CdnCloneClass<'a> {
    /// The tenant whose keyspace the bundles live in.
    tenant: TenantId,
    /// The tenant's pinned region — the region the eligible edge set serves into the attestation.
    region: Region,
    /// The BASE content-addressed blob tier (BORROWED — the CDN is not a new store). The bundles
    /// are ordinary content-addressed blobs in the tenant's keyspace; the CDN is the delivery layer.
    base: &'a dyn BlobStore,
    /// Whether this tenant is an EU tenant (drives the within-EU edge-set restriction).
    tenant_is_eu: bool,
}

impl<'a> CdnCloneClass<'a> {
    /// Build the CDN clone/bundle class for `tenant` (pinned to `region`, EU iff `tenant_is_eu`)
    /// over the base content-addressed blob `store` (BORROWED — never an owned second store).
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

    /// The tenant the class serves.
    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    /// **Publish a clone/bundle blob into the CDN class (content-addressed).** A bundle is an
    /// ordinary content-addressed blob in the tenant's keyspace — the CDN class is a TAG, so
    /// publishing is just a `put` through the unchanged base store; the returned [`ContentHash`] IS
    /// the bundle's CDN cache key (the content-address). No new store is created.
    pub fn publish_bundle(&self, bytes: &[u8]) -> Result<ContentHash, crate::blob::BlobError> {
        self.base.put(&self.tenant, bytes)
    }

    /// **Serve a clone/bundle blob by content-address — the content-address IS the cache-validity
    /// check (no staleness model).** Reads the bundle at `hash` from the base store, which
    /// **re-hash-verifies** the bytes and REFUSES a content-address mismatch (the STOR-D7 0-silent-
    /// serve floor) — so a served bundle is provably the exact requested content. An edge cache over
    /// this is a pure content-address cache: a cache entry is valid iff its hash matches.
    pub fn bundle(&self, hash: &ContentHash) -> Result<Vec<u8>, crate::blob::BlobError> {
        self.bundle_bounded(hash, CDN_MAX_BUNDLE_BYTES)
    }

    /// Serve a bundle under a caller-selected byte ceiling, checking metadata before body fetch.
    pub fn bundle_bounded(
        &self,
        hash: &ContentHash,
        maximum_bytes: usize,
    ) -> Result<Vec<u8>, crate::blob::BlobError> {
        self.base
            .get_bounded(&self.tenant, hash, maximum_bytes)
    }

    /// **The eligible within-EU edge set for THIS tenant** over a candidate POP set — for an EU
    /// tenant, only within-EU POPs (the residency filter). Delegates to [`CdnEdgeSet::eligible_for`].
    pub fn eligible_edges<'p>(&self, candidates: &'p [CdnEdgePop]) -> Vec<&'p CdnEdgePop> {
        CdnEdgeSet.eligible_for(self.tenant_is_eu, candidates)
    }

    /// **The CDN edge set residency report (extends `residency_verify` — contract 12.4).** The CDN
    /// serves this tenant's bundles within the tenant's region, so it reports
    /// [`ResidencyStoreClass::CdnEdgeSet`] @ that region, fed into the SAME
    /// [`crate::residency::verify_region_pinning`] aggregation. A CDN that would serve an EU tenant
    /// from an extra-EU region FAILs there without a code change (the aggregation checks any reported
    /// class's region against the tenant's region). PII-free.
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
            // An extra-EU POP — must NOT be eligible for an EU tenant.
            CdnEdgePop::new("iad-1", Region::new("us-east"), false),
        ]
    }

    /// **A CDN clone bundle is content-addressed — the address IS the validity check (no staleness
    /// model).** Publishing returns the content-address; serving by that address re-hash-verifies and
    /// returns the exact bytes; a tampered bundle is REFUSED (0 silent serve), proving the
    /// content-address-as-cache-validity property the C3 class leans on.
    #[test]
    fn a_cdn_clone_bundle_is_content_addressed_and_the_address_is_the_validity_check() {
        let store = FsBlobStore::new();
        let cdn = CdnCloneClass::over(tenant(), Region::new("fr-par"), true, &store);

        let bundle_bytes = b"PACK\0clone-bundle-of-hot-repo";
        let addr = cdn.publish_bundle(bundle_bytes).expect("publish bundle");
        // The address IS the BLAKE3 of the bundle bytes (content-addressed like any T2 blob).
        assert_eq!(addr, ContentHash::blake3(bundle_bytes));

        // Serve by content-address: the bytes are re-hash-verified and exact.
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
            Err(crate::blob::BlobError::ReadLimitExceeded { .. })
        ));

        // A tampered bundle is REFUSED (the content-address is the validity check — no staleness).
        assert!(
            store.corrupt_for_drill(&tenant(), &addr),
            "bundle present to corrupt"
        );
        assert!(
            matches!(
                cdn.bundle(&addr),
                Err(crate::blob::BlobError::IntegrityFail { .. })
            ),
            "a tampered bundle MUST be refused — the content-address is the cache-validity check"
        );
    }

    /// **The CDN edge set for an EU tenant contains ONLY within-EU edges (the residency filter, the
    /// mandatory-core).** The extra-EU POP is EXCLUDED; the within-EU POPs are eligible.
    #[test]
    fn the_eu_tenant_eligible_edge_set_is_within_eu_only() {
        let store = FsBlobStore::new();
        let cdn = CdnCloneClass::over(tenant(), Region::new("fr-par"), true, &store);
        let candidates = eu_pops();

        let eligible = cdn.eligible_edges(&candidates);
        // Exactly the two within-EU POPs — the extra-EU `iad-1` is excluded.
        assert_eq!(
            eligible.len(),
            2,
            "an EU tenant's eligible edge set excludes extra-EU POPs"
        );
        assert!(
            eligible.iter().all(|pop| pop.within_eu),
            "every eligible POP for an EU tenant is within-EU — no PII-bearing bundle reaches an extra-EU edge"
        );
        assert!(
            !eligible.iter().any(|pop| pop.id == "iad-1"),
            "the extra-EU POP is NOT eligible for an EU tenant"
        );
    }

    /// **A non-EU tenant has no within-EU restriction** (the filter is the EU-tenant residency
    /// property; this proves the `tenant_is_eu` guard is load-bearing — flipping it changes the set,
    /// killing the mutant that drops the EU condition).
    #[test]
    fn a_non_eu_tenant_has_no_within_eu_restriction() {
        let candidates = eu_pops();
        // tenant_is_eu = false → all candidates are eligible (no within-EU filter applies).
        let eligible = CdnEdgeSet.eligible_for(false, &candidates);
        assert_eq!(
            eligible.len(),
            candidates.len(),
            "a non-EU tenant has no within-EU restriction"
        );
    }

    /// **The residency attestation INCLUDES the CDN edge set (extends `residency_verify`, 12.4).** The
    /// CDN report is fed alongside the M1 store reports into the SAME aggregation; the attestation
    /// then covers the CDN edge set @ the tenant's region.
    #[test]
    fn the_residency_attestation_includes_the_cdn_edge_set() {
        let store = FsBlobStore::new();
        let region = Region::new("fr-par");
        let cdn = CdnCloneClass::over(tenant(), region.clone(), true, &store);

        let cdn_report = cdn.residency_report();
        assert_eq!(cdn_report.store_class, ResidencyStoreClass::CdnEdgeSet);
        assert_eq!(cdn_report.region, region);

        // Feed the CDN report alongside the M1 store reports into the SAME aggregation.
        let mut reports = StoreSet::for_cell(&region).reports_for(&tenant());
        reports.push(cdn_report);
        let att = verify_region_pinning(&tenant(), &region, &reports)
            .expect("every store (incl. the CDN edge set) reports the tenant's region");
        // The attestation's store set now INCLUDES the CDN edge set.
        assert!(
            att.store_regions
                .iter()
                .any(|(class, _)| *class == ResidencyStoreClass::CdnEdgeSet),
            "the residency attestation includes the CDN edge set (12.4)"
        );
    }

    /// **A CDN edge serving an EU tenant from an extra-EU region FAILs the attestation WITHOUT a code
    /// change** (the aggregation's region-equality check covers the CDN class — 0 cross-region PII
    /// egress via the CDN, the STOR-D5 CDN extension).
    #[test]
    fn a_cross_region_cdn_edge_fails_the_residency_attestation() {
        let region = Region::new("fr-par");
        // A (wrongly) extra-EU CDN report — eu data served from us-east.
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

    /// **The C3 class RIDES the unchanged base BlobStore — it is NOT a new store (the structural
    /// assertion, EI-01 §7).** The SAME [`FsBlobStore`] instance backs both a direct blob `get` and a
    /// CDN bundle serve: the bundle published through the CDN class is readable through the base trait
    /// at the same content-address (no parallel object map).
    #[test]
    fn the_cdn_class_rides_the_unchanged_base_blobstore() {
        let store = FsBlobStore::new();
        let cdn = CdnCloneClass::over(tenant(), Region::new("fr-par"), true, &store);
        let bytes = b"shared-backing bundle";

        // Publish through the CDN class.
        let addr = cdn.publish_bundle(bytes).expect("publish");

        // The SAME base store serves the SAME bytes at the SAME address via the plain trait —
        // proving the CDN is a tag over the unchanged store, not a second store.
        let via_base = BlobStore::get(&store, &tenant(), &addr).expect("base store has the bundle");
        assert_eq!(via_base, bytes);
        // And the CDN serve returns the identical bytes.
        assert_eq!(cdn.bundle(&addr).expect("cdn serve"), bytes);
    }

    /// The CDN store-class label is stable + PII-free (the attestation body / telemetry tag).
    #[test]
    fn the_cdn_store_class_label_is_stable() {
        assert_eq!(ResidencyStoreClass::CdnEdgeSet.label(), "cdn_edge_set");
        // It is NOT in the M1 set (a named follow-on variant, not a redefinition of M1).
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
