use myelin_storage::{
    verify_region_pinning, BlobStore, CdnCloneClass, CdnEdgePop, ContentHash, FsBlobStore,
    ResidencyStoreClass, StoreSet,
};
use myelin_tenancy::{Region, TenantId};

struct CloneStormAccelerator<'a> {
    cdn: CdnCloneClass<'a>,
}

impl<'a> CloneStormAccelerator<'a> {
    fn boot_eu(tenant: &str, region: &str, store: &'a dyn BlobStore) -> CloneStormAccelerator<'a> {
        CloneStormAccelerator {
            cdn: CdnCloneClass::over(
                TenantId::from_token(tenant),
                Region::new(region),
                true,
                store,
            ),
        }
    }

    fn publish(&self, bundle: &[u8]) -> ContentHash {
        self.cdn
            .publish_bundle(bundle)
            .expect("publish clone bundle")
    }

    fn eligible<'p>(&self, candidates: &'p [CdnEdgePop]) -> Vec<&'p CdnEdgePop> {
        self.cdn.eligible_edges(candidates)
    }

    fn serve(&self, address: &ContentHash) -> Vec<u8> {
        self.cdn
            .bundle(address)
            .expect("serve bundle by content-address")
    }
}

fn candidate_pops() -> Vec<CdnEdgePop> {
    vec![
        CdnEdgePop::new("par-1", Region::new("fr-par"), true),
        CdnEdgePop::new("ams-1", Region::new("nl-ams"), true),
        CdnEdgePop::new("iad-1", Region::new("us-east"), false),
    ]
}

#[test]
fn cdc_11_2_c3_clone_storm_publishes_and_serves_by_content_address_from_a_within_eu_edge() {
    let store = FsBlobStore::new();
    let accel = CloneStormAccelerator::boot_eu("acme", "fr-par", &store);

    let bundle = b"PACK\0clone-bundle-of-a-hot-repo\0...";
    let address = accel.publish(bundle);
    assert_eq!(address, ContentHash::blake3(bundle));

    let candidates = candidate_pops();
    let edges = accel.eligible(&candidates);
    assert_eq!(
        edges.len(),
        2,
        "an EU tenant's eligible edge set excludes the extra-EU POP"
    );
    assert!(
        edges.iter().all(|p| p.within_eu),
        "every eligible edge is within-EU"
    );

    assert_eq!(
        accel.serve(&address),
        bundle,
        "11.2-C3: serve by content-address round-trips the bundle"
    );
}

#[test]
fn cdc_11_2_c3_consumer_residency_attestation_covers_the_cdn_edge_set() {
    let store = FsBlobStore::new();
    let region = Region::new("fr-par");
    let accel = CloneStormAccelerator::boot_eu("acme", "fr-par", &store);
    let tenant = TenantId::from_token("acme");

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
