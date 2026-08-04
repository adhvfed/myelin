use myelin_storage::{BlobError, BlobStore, ContentHash, FsBlobStore, ReplicatedBlobStore};
use myelin_tenancy::TenantId;

#[test]
fn stor_d7_object_store_corrupt_primary_recovers_from_replica_zero_silent_serve() {
    let tenant = TenantId("acme".into());
    const BATCH: usize = 32;

    let primary = FsBlobStore::new();
    let replica_a = FsBlobStore::new();
    let replica_b = FsBlobStore::new();

    let mut handles: Vec<(ContentHash, Vec<u8>)> = Vec::with_capacity(BATCH);
    for i in 0..BATCH {
        let bytes = format!("object-#{i}-replicated-trustworthy-content").into_bytes();
        let h = primary.put(&tenant, &bytes).expect("primary put");
        replica_a.put(&tenant, &bytes).expect("replica A put");
        replica_b.put(&tenant, &bytes).expect("replica B put");
        assert_eq!(primary.get(&tenant, &h).expect("clean primary read"), bytes);
        handles.push((h, bytes));
    }

    for (h, _) in &handles {
        assert!(
            primary.corrupt_for_drill(&tenant, h),
            "primary object present to corrupt"
        );
    }

    let store = ReplicatedBlobStore::new(primary, vec![replica_a, replica_b]);
    assert_eq!(store.replica_count(), 2, "two replicas back the primary");

    for (h, original) in &handles {
        match store.get(&tenant, h) {
            Ok(served) => assert_eq!(
                &served, original,
                "recovered bytes MUST be the correct content (re-verified replica)"
            ),
            Err(BlobError::IntegrityFail { .. }) => panic!(
                "STOR-D7 FLOOR BREACHED: object {} had a verifying replica but was NOT recovered",
                h.to_multihash_string()
            ),
            Err(other) => panic!("expected a recovered serve, got {other}"),
        }
    }

    let recovered = store.telemetry().blob_recovered_from_replica();
    assert_eq!(
        recovered, BATCH as u64,
        "every corrupt-primary read must recover from a replica exactly once"
    );
    assert_eq!(
        store.telemetry().blob_unrecoverable(),
        0,
        "no read was unrecoverable (every object had a verifying replica)"
    );

    for (h, original) in &handles {
        assert_eq!(&store.get(&tenant, h).expect("healed read"), original);
    }
    assert_eq!(
        store.telemetry().blob_recovered_from_replica(),
        BATCH as u64,
        "healed primaries serve without a second recovery (the heal is durable)"
    );

    println!(
        "[P-441 DRILL GREEN 2026-06-24] STOR-D7 object-store replica recovery: batch={BATCH} \
         primary objects corrupted -> blob_recovered_from_replica={recovered}, unrecoverable=0, \
         silent_serves=0, every corrupt primary RECOVERED from a verifying replica + primary \
         healed (re-hash-on-read survives the fs->object backing swap; 0 silent serve - \
         storage.md section 10 D-S8 / testing-strategy STOR-D7)"
    );
}

#[test]
fn stor_d7_object_store_all_copies_corrupt_refuses_to_serve() {
    let tenant = TenantId("acme".into());
    let primary = FsBlobStore::new();
    let replica = FsBlobStore::new();
    let bytes = b"doomed-on-every-copy".to_vec();
    let h = primary.put(&tenant, &bytes).expect("primary put");
    replica.put(&tenant, &bytes).expect("replica put");
    assert!(primary.corrupt_for_drill(&tenant, &h));
    assert!(replica.corrupt_for_drill(&tenant, &h));

    let store = ReplicatedBlobStore::new(primary, vec![replica]);
    match store.get(&tenant, &h) {
        Err(BlobError::IntegrityFail { requested, .. }) => assert_eq!(requested, h),
        Ok(served) => panic!(
            "STOR-D7 FLOOR BREACHED: all copies corrupt but served {} bytes silently",
            served.len()
        ),
        Err(other) => panic!("expected IntegrityFail (all copies corrupt), got {other}"),
    }
    assert_eq!(store.telemetry().blob_recovered_from_replica(), 0);
    assert_eq!(store.telemetry().blob_unrecoverable(), 1);
}
