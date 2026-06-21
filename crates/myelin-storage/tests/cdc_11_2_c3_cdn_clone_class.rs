//! Contract 11.2-C3 CDC pair — the within-EU **CDN clone/bundle blob class** (P-ST-23 / global
//! P-254).
//!
//! The prompt requires "the provider+consumer pair for 11.2-C3 (the CDN-class consumer)". This is
//! the consumer-driven contract test: the PROVIDER is `myelin-storage` (the [`CdnCloneClass`] over
//! the unchanged content-addressed `BlobStore` trait this prompt ships); the CONSUMER is the **Git
//! clone-storm acceleration path** (modelled here as a tiny `CloneStormAccelerator`) that publishes
//! a clone bundle, picks an ELIGIBLE within-EU edge, and serves the bundle BY CONTENT-ADDRESS. The
//! test pins the frozen call shape the Git subsystem relies on — if `publish_bundle` / `bundle` /
//! `eligible_edges` / `residency_report` drift, it stops compiling/passing.
//!
//! **The load-bearing contract this pins:**
//! 1. a clone bundle is **content-addressed** — the consumer caches it BY its content-address and a
//!    cache entry is valid iff its hash matches (no staleness model);
//! 2. an **EU tenant's eligible edge set is within-EU-only** — the consumer never serves a
//!    PII-bearing bundle from an extra-EU edge (the residency property);
//! 3. the CDN reports into `residency_verify` — the consumer's residency attestation INCLUDES the
//!    CDN edge set (a cross-region CDN edge FAILs the same aggregation).

use myelin_storage::{
    verify_region_pinning, BlobStore, CdnCloneClass, CdnEdgePop, ContentHash, FsBlobStore,
    ResidencyStoreClass, StoreSet,
};
use myelin_tenancy::{Region, TenantId};

/// A consumer of the 11.2-C3 CDN class: the Git clone-storm accelerator. It publishes a clone
/// bundle for a hot repo, selects an eligible (within-EU for an EU tenant) edge POP, and serves the
/// bundle to a cloning client BY CONTENT-ADDRESS — exactly how clone-storm acceleration off-loads a
/// hot repo's bundle to the edge (the content-address-as-cache-key pattern).
struct CloneStormAccelerator<'a> {
    cdn: CdnCloneClass<'a>,
}

impl<'a> CloneStormAccelerator<'a> {
    /// Boot the accelerator over the (borrowed) base blob tier for an EU tenant pinned to `region`.
    fn boot_eu(tenant: &str, region: &str, store: &'a dyn BlobStore) -> CloneStormAccelerator<'a> {
        CloneStormAccelerator {
            cdn: CdnCloneClass::over(
                TenantId::from_token(tenant),
                Region::new(region),
                /* tenant_is_eu */ true,
                store,
            ),
        }
    }

    /// Publish a hot repo's clone bundle, returning its content-address (the edge cache key).
    fn publish(&self, bundle: &[u8]) -> ContentHash {
        self.cdn.publish_bundle(bundle).expect("publish clone bundle")
    }

    /// Pick the eligible edges for this tenant from a candidate POP set (within-EU for an EU tenant).
    fn eligible<'p>(&self, candidates: &'p [CdnEdgePop]) -> Vec<&'p CdnEdgePop> {
        self.cdn.eligible_edges(candidates)
    }

    /// Serve the bundle from the edge BY its content-address (re-hash-verified — the validity check).
    fn serve(&self, address: &ContentHash) -> Vec<u8> {
        self.cdn.bundle(address).expect("serve bundle by content-address")
    }
}

fn candidate_pops() -> Vec<CdnEdgePop> {
    vec![
        CdnEdgePop::new("par-1", Region::new("fr-par"), true),
        CdnEdgePop::new("ams-1", Region::new("nl-ams"), true),
        CdnEdgePop::new("iad-1", Region::new("us-east"), false), // extra-EU — ineligible for an EU tenant
    ]
}

/// THE CDC pair: the Git clone-storm consumer publishes a bundle, resolves it by content-address
/// from an eligible within-EU edge, and the served bytes are exactly the published bundle — the
/// provider (`myelin-storage`'s CDN clone class) honours the frozen 11.2-C3 shape.
#[test]
fn cdc_11_2_c3_clone_storm_publishes_and_serves_by_content_address_from_a_within_eu_edge() {
    let store = FsBlobStore::new();
    let accel = CloneStormAccelerator::boot_eu("acme", "fr-par", &store);

    let bundle = b"PACK\0clone-bundle-of-a-hot-repo\0...";
    let address = accel.publish(bundle);
    // The cache key IS the content-address (BLAKE3 of the bundle) — no staleness model.
    assert_eq!(address, ContentHash::blake3(bundle));

    // The consumer picks an eligible edge — within-EU only for an EU tenant.
    let candidates = candidate_pops();
    let edges = accel.eligible(&candidates);
    assert_eq!(edges.len(), 2, "an EU tenant's eligible edge set excludes the extra-EU POP");
    assert!(edges.iter().all(|p| p.within_eu), "every eligible edge is within-EU");

    // Serving by the content-address returns the exact published bundle (re-hash-verified).
    assert_eq!(accel.serve(&address), bundle, "11.2-C3: serve by content-address round-trips the bundle");
}

/// The consumer relies on the residency attestation COVERING the CDN edge set (12.4): the CDN
/// report is aggregated WITH the M1 store reports, and a cross-region CDN edge FAILs the same
/// `verify_region_pinning` — the provider honours "residency_verify covers the CDN edge set".
#[test]
fn cdc_11_2_c3_consumer_residency_attestation_covers_the_cdn_edge_set() {
    let store = FsBlobStore::new();
    let region = Region::new("fr-par");
    let accel = CloneStormAccelerator::boot_eu("acme", "fr-par", &store);
    let tenant = TenantId::from_token("acme");

    // The consumer gathers the M1 store reports + the CDN report into one attestation.
    let mut reports = StoreSet::for_cell(&region).reports_for(&tenant);
    reports.push(accel.cdn.residency_report());
    let att = verify_region_pinning(&tenant, &region, &reports)
        .expect("the CDN edge set reports the tenant's region → the attestation covers it");
    assert!(
        att.store_regions
            .iter()
            .any(|(c, _)| *c == ResidencyStoreClass::CdnEdgeSet),
        "the consumer's residency attestation INCLUDES the CDN edge set (12.4)"
    );

    // A cross-region CDN edge (an EU tenant served from us-east) FAILs the SAME aggregation — the
    // consumer gets 0 cross-region PII egress via the CDN by construction (no wire change needed).
    let bad_cdn = myelin_storage::StoreResidencyReport {
        tenant: tenant.clone(),
        store_class: ResidencyStoreClass::CdnEdgeSet,
        region: Region::new("us-east"),
    };
    let mut bad_reports = StoreSet::for_cell(&region).reports_for(&tenant);
    bad_reports.push(bad_cdn);
    assert!(
        verify_region_pinning(&tenant, &region, &bad_reports).is_err(),
        "a cross-region CDN edge FAILs the residency attestation (0 cross-region PII egress via the CDN)"
    );
}
