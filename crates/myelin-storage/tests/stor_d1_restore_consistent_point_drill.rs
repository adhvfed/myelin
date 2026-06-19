//! P-ST-12 (global P-060) GATE / DRILL — restore-to-consistent-point (0 dangling), dated green
//! artifact.
//!
//! **The GATE (storage.md §7.3):** `restore(to_offset T)` lands OLTP at the WAL position whose outbox
//! `seq ≤ T`; **every referenced `ContentHash` is present** (a missing hash FAILs, not silently
//! passes); **derived == source-replay** (reindex-from-source, never a derived backup). Telemetry:
//! `restore_consistent_point_offset == T`, `dangling_ref_count == 0`.
//!
//! This drill exercises the full `restore(to_offset T)` scenario and feeds its output into the
//! **harness cross-seam ASSERTION** (`myelin_harness::RestoredSnapshot::verify_cross_seam`, P-056 —
//! the SAME assertion SUB-D6 / STOR-D1 drive), so the storage restore and the substrate's cross-seam
//! invariant are proven on ONE consistent point (coherence, EI-01 §7 — never a parallel second
//! assertion). The measured signals are emitted on the SAME [`SignalSource`] every drill uses
//! (observability is part of the pass, EI-01 §3): `RestoreCrossSeamMismatch == 0` is the
//! `dangling_ref_count == 0` telemetry (a dangling blob ref IS a cross-seam mismatch).
//!
//! ## Scope (named, EI-01 §4)
//! This is the M1 single-tenant-scale restore drill against the modeled WAL/PITR machinery (the real
//! `pg_restore` + WAL-replay driver is the P-S12/P-S15 floor). The CI-WIRED restore-verify GATE
//! (STOR-D1, the permanent gate) that re-runs this on EVERY store-touching change is the sibling
//! **P-ST-13 (global P-061)**; the post-restore re-erasure (STOR-D3) is **P-ST-14 (global P-100)**;
//! the prod-scale RESTORED copy this produces for online-migration-under-load is **P-ST-21 (global
//! P-126, STOR-D8)**. All named in the prompt + the crate docs.

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

/// Backups covering offsets `0..=tail` (a base at 0 + the WAL tail archived to `tail`).
fn reachable_archiver(tail: u64) -> ContinuousArchiver {
    let mut arch = ContinuousArchiver::new();
    arch.archive_segment(WalSegment { end_offset: 0, committed_at: 0 }).unwrap();
    arch.take_base_backup(1);
    arch.archive_segment(WalSegment { end_offset: tail, committed_at: 10 }).unwrap();
    arch
}

/// Map a storage [`RestoreReport`] into the harness [`RestoredSnapshot`] so the SAME cross-seam
/// assertion SUB-D6 / STOR-D1 use verifies this restore (coherence, EI-01 §7). The restored OLTP rows
/// carry their offset + blob ref; the restored object tier is the present blob set; the derived docs
/// are the reindexed-from-source projections. A consistent restore → 0 mismatches.
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

/// **THE DRILL (dated green artifact): `restore(to_offset T)` lands at ONE consistent cross-seam
/// point — 0 dangling refs, OLTP at `seq ≤ T`, derived == source-replay.**
///
/// The scenario: a tenant with state at offsets 90/100 (each referencing a present blob), source
/// events that project those rows, a KEK, and a future row at offset 250 that the restore must drop.
/// Restore to T=100, then assert (a) the storage report is consistent at T with 0 dangling, and (b)
/// the harness cross-seam assertion (the SUB-D6 one) reports 0 mismatches on the mapped snapshot.
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
        WalRow { id: "r90".into(), written_at: 90, blob_ref: Some(h("blob-90")) },
        WalRow { id: "r100".into(), written_at: 100, blob_ref: Some(h("blob-100")) },
        WalRow { id: "r-future".into(), written_at: 250, blob_ref: None }, // > T → dropped
    ];

    let target = 100;
    let report = restore_to_offset(&arch, target, &rows, &blobs, &source, &kms)
        .expect("a consistent restore must pass");

    // (a) the storage report: restored to T, OLTP ≤ T, 0 dangling, derived == source-replay.
    assert_eq!(report.restored_to_offset, target, "restore_consistent_point_offset == T");
    assert_eq!(report.dangling_ref_count, 0, "dangling_ref_count == 0");
    assert!(report.oltp_rows.iter().all(|r| r.written_at <= target));
    assert_eq!(report.oltp_rows.len(), 2, "the future row was dropped");
    assert_eq!(report.derived.resumed_at(), target, "consumers resume at T");

    // (b) the harness cross-seam assertion (the SUB-D6 / STOR-D1 one) agrees: 0 mismatches.
    let snapshot = to_harness_snapshot(&report, &present);
    let cross_seam = snapshot.verify_cross_seam();
    assert!(
        cross_seam.is_consistent(),
        "the restore must land at ONE consistent cross-seam point, got {:?}",
        cross_seam.mismatches
    );

    // The green artifact: emit the dangling/cross-seam telemetry observably (the SAME signal surface
    // every drill uses). 0 dangling refs ⇒ 0 cross-seam mismatches.
    let mut signals = SignalSource::new();
    signals.set_scalar(SignalName::RestoreCrossSeamMismatch, cross_seam.mismatch_count());
    signals
        .assert_signal(SignalName::RestoreCrossSeamMismatch, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-060 DRILL GREEN 2026-06-19] restore-to-consistent-point: restore(to_offset T={target}) \
         landed OLTP↔blob↔index↔offset at ONE consistent point — {} OLTP rows (all seq≤T), \
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

/// **The drill CATCHES a sloppy restore (the assertion is real, not vacuous):** a restore that brings
/// back a row referencing a blob it did NOT restore (the §7.3 silent-corruption case) FAILS HARD —
/// the storage restore returns `Err`, so the gate would FAIL CI (never a silent pass). This proves
/// the gate would fail on a regression (EI-01 §3 — a drill that cannot go red is not a gate).
#[test]
fn stor_d1_catches_a_row_pointing_at_a_missing_blob() {
    let kms = KmsEngine::new();
    let arch = reachable_archiver(300);
    let mut blobs = BlobPresence::new();
    blobs.insert(h("present")); // "missing" is NOT restored — the injected silent-corruption case
    let source = SourceLog::new();
    let rows = vec![
        WalRow { id: "ok".into(), written_at: 50, blob_ref: Some(h("present")) },
        WalRow { id: "corrupt".into(), written_at: 90, blob_ref: Some(h("missing")) },
    ];

    let err = restore_to_offset(&arch, 100, &rows, &blobs, &source, &kms)
        .expect_err("a row → missing blob MUST make the restore FAIL (the gate fails CI)");
    assert!(
        matches!(err, RestoreError::DanglingBlobRef { ref row_id, .. } if row_id == "corrupt"),
        "the silent-corruption case must name the offending row: {err}"
    );
}

/// **The harness cross-seam assertion would ALSO read RED** on an inconsistent restore (a row →
/// missing blob mapped into the snapshot), proving the cross-seam telemetry bites — the
/// `RestoreCrossSeamMismatch == 0` signal goes red. (We build the snapshot directly here since the
/// storage `restore_to_offset` would have already failed hard; this exercises the assertion surface
/// the CI gate reads.)
#[test]
fn stor_d1_cross_seam_assertion_reads_red_on_an_inconsistent_restore() {
    // A restored row references a blob the restore did not bring back (the mismatch).
    let snapshot = RestoredSnapshot::builder(100)
        .row("r1", 95, Some(h("not-restored").to_multihash_string()))
        .build();
    let report = snapshot.verify_cross_seam();
    assert!(!report.is_consistent(), "a row → missing blob is a cross-seam mismatch");

    let mut signals = SignalSource::new();
    signals.set_scalar(SignalName::RestoreCrossSeamMismatch, report.mismatch_count());
    let verdict = signals.assert_signal(SignalName::RestoreCrossSeamMismatch, Predicate::Eq(0));
    assert!(
        !verdict.is_green(),
        "an inconsistent restore MUST read RED on the cross-seam (dangling_ref) assertion"
    );
}
