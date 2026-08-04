use myelin_harness::{Predicate, RestoredSnapshot, SignalName, SignalSource};
use myelin_storage::{
    restore_to_offset, BlobPresence, ContentHash, ContinuousArchiver, KekId, KeyClass, KmsEngine,
    RestoreError, RestoreReport, SourceLog, WalRow, WalSegment,
};
use myelin_tenancy::{Region, TenantId};

fn region() -> Region {
    Region("eu-west".into())
}
fn h(s: &str) -> ContentHash {
    ContentHash::blake3(s.as_bytes())
}

fn reachable_archiver(tail: u64) -> ContinuousArchiver {
    let mut arch = ContinuousArchiver::new();
    arch.archive_segment(WalSegment {
        end_offset: 0,
        committed_at: 0,
    })
    .unwrap();
    arch.take_base_backup(1);
    arch.archive_segment(WalSegment {
        end_offset: tail,
        committed_at: 10,
    })
    .unwrap();
    arch
}

fn to_harness_snapshot(report: &RestoreReport, present_blobs: &[ContentHash]) -> RestoredSnapshot {
    let mut b = RestoredSnapshot::builder(report.restored_to_offset);
    for blob in present_blobs {
        b = b.blob(blob.to_multihash_string());
    }
    for row in &report.oltp_rows {
        b = b.row(
            row.id.clone(),
            row.written_at,
            row.blob_ref.as_ref().map(|h| h.to_multihash_string()),
        );
    }
    for doc in report.derived.docs() {
        b = b.index_doc(doc.clone());
    }
    b.build()
}

#[test]
fn stor_d1_restore_lands_one_consistent_point() {
    let kms = KmsEngine::new();
    let t = TenantId("acme".into());
    kms.ensure_kek(&KekId::new(t.clone(), region()));
    kms.ensure_dek(&t, &region(), KeyClass::Tenant).unwrap();

    let arch = reachable_archiver(300);
    let present = vec![h("blob-90"), h("blob-100")];
    let mut blobs = BlobPresence::new();
    for blob in &present {
        blobs.insert(blob.clone());
    }
    let mut source = SourceLog::new();
    source.append(90, "r90").append(100, "r100");
    let rows = vec![
        WalRow {
            id: "r90".into(),
            written_at: 90,
            blob_ref: Some(h("blob-90")),
        },
        WalRow {
            id: "r100".into(),
            written_at: 100,
            blob_ref: Some(h("blob-100")),
        },
        WalRow {
            id: "r-future".into(),
            written_at: 250,
            blob_ref: None,
        },
    ];

    let target = 100;
    let report = restore_to_offset(&arch, target, &rows, &blobs, &source, &kms)
        .expect("a consistent restore must pass");

    assert_eq!(
        report.restored_to_offset, target,
        "restore_consistent_point_offset == T"
    );
    assert_eq!(report.dangling_ref_count, 0, "dangling_ref_count == 0");
    assert!(report.oltp_rows.iter().all(|r| r.written_at <= target));
    assert_eq!(report.oltp_rows.len(), 2, "the future row was dropped");
    assert_eq!(report.derived.resumed_at(), target, "consumers resume at T");

    let snapshot = to_harness_snapshot(&report, &present);
    let cross_seam = snapshot.verify_cross_seam();
    assert!(
        cross_seam.is_consistent(),
        "the restore must land at ONE consistent cross-seam point, got {:?}",
        cross_seam.mismatches
    );

    let mut signals = SignalSource::new();
    signals.set_scalar(
        SignalName::RestoreCrossSeamMismatch,
        cross_seam.mismatch_count(),
    );
    signals
        .assert_signal(SignalName::RestoreCrossSeamMismatch, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-060 DRILL GREEN 2026-06-19] restore-to-consistent-point: restore(to_offset T={target}) \
         landed OLTP↔blob↔index↔offset at ONE consistent point - {} OLTP rows (all seq≤T), \
         dangling_ref_count={} (0), derived reindexed-from-source ({} docs, consumers resume at T), \
         {} cross-seam mismatch(es) on the harness SUB-D6 assertion. Future row (offset 250) dropped. \
         CI-wired restore-verify GATE (STOR-D1) -> P-ST-13 (P-061); post-restore re-erasure -> \
         P-ST-14 (P-100); prod-scale restored copy for STOR-D8 -> P-ST-21 (P-126).",
        report.oltp_rows.len(),
        report.dangling_ref_count,
        report.derived.doc_count(),
        cross_seam.mismatch_count(),
    );
}

#[test]
fn stor_d1_catches_a_row_pointing_at_a_missing_blob() {
    let kms = KmsEngine::new();
    let arch = reachable_archiver(300);
    let mut blobs = BlobPresence::new();
    blobs.insert(h("present"));
    let source = SourceLog::new();
    let rows = vec![
        WalRow {
            id: "ok".into(),
            written_at: 50,
            blob_ref: Some(h("present")),
        },
        WalRow {
            id: "corrupt".into(),
            written_at: 90,
            blob_ref: Some(h("missing")),
        },
    ];

    let err = restore_to_offset(&arch, 100, &rows, &blobs, &source, &kms)
        .expect_err("a row → missing blob MUST make the restore FAIL (the gate fails CI)");
    assert!(
        matches!(err, RestoreError::DanglingBlobRef { ref row_id, .. } if row_id == "corrupt"),
        "the silent-corruption case must name the offending row: {err}"
    );
}

#[test]
fn stor_d1_cross_seam_assertion_reads_red_on_an_inconsistent_restore() {
    let snapshot = RestoredSnapshot::builder(100)
        .row("r1", 95, Some(h("not-restored").to_multihash_string()))
        .build();
    let report = snapshot.verify_cross_seam();
    assert!(
        !report.is_consistent(),
        "a row → missing blob is a cross-seam mismatch"
    );

    let mut signals = SignalSource::new();
    signals.set_scalar(
        SignalName::RestoreCrossSeamMismatch,
        report.mismatch_count(),
    );
    let verdict = signals.assert_signal(SignalName::RestoreCrossSeamMismatch, Predicate::Eq(0));
    assert!(
        !verdict.is_green(),
        "an inconsistent restore MUST read RED on the cross-seam (dangling_ref) assertion"
    );
}
