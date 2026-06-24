//! P-ST-30 (global P-441) GATE / DRILL — **STOR-D7 on the object-store BlobStore**, dated green
//! artifact.
//!
//! **STOR-D7 (storage.md §10 D-S8 / testing-strategy §4.2 row STOR-D7):** *"Corrupt an object →
//! re-hash-on-read detects it (content-address mismatch); **recover from replica/backup.** 0
//! silent serve."* The fs-floor half (re-hash-on-read detection) is greened by
//! `stor_d7_blob_integrity_drill.rs` (P-047); THIS drill greens the OBJECT-TIER half the
//! P-ST-30 follow-on adds — the **recover-from-a-replica** property — proving the integrity
//! property SURVIVES the fs→object backing swap and gains replica recovery.
//!
//! The recovery logic ([`myelin_storage::ReplicatedBlobStore`]) is backing-agnostic over the
//! UNCHANGED [`BlobStore`] trait, so this CI drill exercises it over the deterministic, DB-free
//! [`FsBlobStore`] floor (the same trait the live `S3BlobStore` implements). The LIVE proof on
//! the real RustFS object store — primary+replica buckets — rides the `integration` feature
//! (`tests/integration_backends.rs::replicated_object_store_recovers_corrupt_primary_from_replica`),
//! per the dev-real binding policy. The threshold (0 silent serve, every corrupt primary
//! recovered) is NOT weakened to pass (EI-01 §3).

use myelin_storage::{BlobError, BlobStore, ContentHash, FsBlobStore, ReplicatedBlobStore};
use myelin_tenancy::TenantId;

/// **THE STOR-D7-on-object-store drill.** Over a batch of replicated objects, corrupt the
/// PRIMARY copy of each and prove: the read RECOVERS the correct bytes from a verifying replica
/// (0 silent serve), the primary is healed, and `blob_recovered_from_replica` accounts for
/// exactly the corrupt-primary reads. A silent wrong-bytes serve `panic!`s loudly.
#[test]
fn stor_d7_object_store_corrupt_primary_recovers_from_replica_zero_silent_serve() {
    let tenant = TenantId("acme".into());
    const BATCH: usize = 32;

    // Build primary + two replica backings (the object-tier redundancy degree). We hold the
    // primary separately to corrupt it directly, then move all three into the replicated store.
    let primary = FsBlobStore::new();
    let replica_a = FsBlobStore::new();
    let replica_b = FsBlobStore::new();

    let mut handles: Vec<(ContentHash, Vec<u8>)> = Vec::with_capacity(BATCH);
    for i in 0..BATCH {
        let bytes = format!("object-#{i}-replicated-trustworthy-content").into_bytes();
        let h = primary.put(&tenant, &bytes).expect("primary put");
        replica_a.put(&tenant, &bytes).expect("replica A put");
        replica_b.put(&tenant, &bytes).expect("replica B put");
        // Clean read from the primary first (no false-positive recovery).
        assert_eq!(primary.get(&tenant, &h).expect("clean primary read"), bytes);
        handles.push((h, bytes));
    }

    // Corrupt the PRIMARY copy of every object (bit-rot on the primary node).
    for (h, _) in &handles {
        assert!(
            primary.corrupt_for_drill(&tenant, h),
            "primary object present to corrupt"
        );
    }

    let store = ReplicatedBlobStore::new(primary, vec![replica_a, replica_b]);
    assert_eq!(store.replica_count(), 2, "two replicas back the primary");

    // Every corrupt-primary read RECOVERS from a replica — 0 silent serve. A silent wrong-bytes
    // serve `panic!`s immediately, so reaching the post-loop assertions IS the 0-silent-serve
    // proof.
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

    // THE green artifact: exactly BATCH recoveries, 0 unrecoverable, 0 silent serves (proven by
    // the byte-equality assertion above on every read).
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

    // The primaries are HEALED: a second read of each serves cleanly with NO further recovery.
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

/// **0 silent serve when EVERY copy is corrupt** (the negative half of the gate): if the primary
/// AND every replica are corrupt, the read is REFUSED (IntegrityFail surfaced) — never a silent
/// wrong-bytes serve — and `blob_unrecoverable` accounts for it.
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
