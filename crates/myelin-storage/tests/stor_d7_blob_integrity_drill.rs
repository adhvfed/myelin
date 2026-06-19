//! P-ST-03 (global P-047) GATE / DRILL — STOR-D7 (the blob-integrity floor), dated green
//! artifact.
//!
//! **STOR-D7 (storage.md §10 D-S8 / testing-strategy §4.2):** corrupt an fs-BlobStore object →
//! re-hash-on-read detects the content-address mismatch; **0 silent serve**. Telemetry:
//! `blob_integrity_fail` increments on the corrupt read, and the serve is REFUSED (not a
//! silent wrong-bytes return).
//!
//! `blob_integrity_fail` is a storage-DOMAIN telemetry counter (storage.md §9 telemetry),
//! distinct from the frozen 18-signal contract-1.8 survival set in `myelin-harness` — this
//! prompt does NOT extend that frozen set. The drill asserts on the BlobStore's own
//! [`myelin_storage::BlobTelemetry`] counter, loudly (a wrong-bytes serve `panic!`s with the
//! served bytes; the threshold — exactly-one detection, zero silent serves — is NOT weakened
//! to pass, EI-01 §3).

use myelin_storage::{BlobError, BlobStore, FsBlobStore};
use myelin_tenancy::TenantId;

/// **THE STOR-D7 drill.** Across a batch of stored objects, corrupt each and prove the read
/// is refused (0 silent serve) with `blob_integrity_fail` accounting for exactly the corrupt
/// reads. A single silent wrong-bytes serve fails the drill loudly.
#[test]
fn stor_d7_corrupt_blob_is_detected_zero_silent_serve() {
    let store = FsBlobStore::new();
    let tenant = TenantId("acme".into());

    // Store a batch of distinct objects (the 1x load unit) and verify clean reads first.
    const BATCH: usize = 32;
    let mut handles = Vec::with_capacity(BATCH);
    for i in 0..BATCH {
        let bytes = format!("object-#{i}-with-trustworthy-content").into_bytes();
        let h = store.put(&tenant, &bytes).expect("put");
        assert_eq!(store.get(&tenant, &h).expect("clean read"), bytes);
        handles.push((h, bytes));
    }
    // No false positives on clean reads.
    assert_eq!(
        store.telemetry().blob_integrity_fail(),
        0,
        "clean reads must not signal blob_integrity_fail"
    );

    // Corrupt every object and assert detection + 0 silent serve. A silent wrong-bytes serve
    // (the `Ok` arm) `panic!`s immediately — so reaching the assertions below IS the "0 silent
    // serves" proof (silent_serves stays 0 by construction).
    for (h, _original) in &handles {
        assert!(store.corrupt_for_drill(&tenant, h), "object present to corrupt");
        match store.get(&tenant, h) {
            Err(BlobError::IntegrityFail { requested, actual }) => {
                assert_eq!(&requested, h);
                assert_ne!(actual, *h, "corrupt bytes hash to a different address");
            }
            Ok(served) => panic!(
                "STOR-D7 FLOOR BREACHED: corrupt object {} served {} bytes silently",
                h.to_multihash_string(),
                served.len()
            ),
            Err(other) => panic!("expected IntegrityFail, got {other}"),
        }
    }

    // THE green artifact: exactly BATCH detections, 0 silent serves (proven above by the
    // panic-on-Ok arm — no corrupt read returned bytes).
    let detections = store.telemetry().blob_integrity_fail();
    assert_eq!(
        detections, BATCH as u64,
        "every corrupt read must increment blob_integrity_fail exactly once"
    );

    println!(
        "[P-047 DRILL GREEN 2026-06-19] STOR-D7 blob-integrity: batch={BATCH} objects corrupted \
         -> blob_integrity_fail={detections}, silent_serves=0, every corrupt read REFUSED \
         (re-hash-on-read; 0 silent serve - storage.md section 10 D-S8)"
    );
}
