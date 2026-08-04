use myelin_storage::{BlobError, BlobStore, FsBlobStore};
use myelin_tenancy::TenantId;

#[test]
fn stor_d7_corrupt_blob_is_detected_zero_silent_serve() {
    let store = FsBlobStore::new();
    let tenant = TenantId("acme".into());

    const BATCH: usize = 32;
    let mut handles = Vec::with_capacity(BATCH);
    for i in 0..BATCH {
        let bytes = format!("object-#{i}-with-trustworthy-content").into_bytes();
        let h = store.put(&tenant, &bytes).expect("put");
        assert_eq!(store.get(&tenant, &h).expect("clean read"), bytes);
        handles.push((h, bytes));
    }
    assert_eq!(
        store.telemetry().blob_integrity_fail(),
        0,
        "clean reads must not signal blob_integrity_fail"
    );

    for (h, _original) in &handles {
        assert!(
            store.corrupt_for_drill(&tenant, h),
            "object present to corrupt"
        );
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
