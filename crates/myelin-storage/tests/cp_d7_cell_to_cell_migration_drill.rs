//! P-ST-32 (global P-443) GATE / DRILL — **the STORAGE half of cell→cell migration (CP-D7, FLOOR)**
//! — dated green artifact.
//!
//! **CP-D7 (FLOOR — testing-strategy §4.2 / storage.md §2 "S-M5" + §7.3):** migrate a tenant cell→cell
//! (SAME region) → **0 loss across-seam**, **lands in-region**, **source crypto-shredded**. Telemetry:
//! the migration receipt, 0 loss, the source key destroyed.
//!
//! This is the STORAGE-HALF drill (the data-plane primitive [`migrate_cell_to_cell`] the control plane
//! drives): restore the source's §7.3 cross-seam consistency point INTO the target cell with 0 loss,
//! then crypto-shred the SOURCE cell's key. The CONTROL-PLANE-half drill (the durable-workflow
//! orchestration + the atomic placement cut-over + the cross-region rejection) is
//! `myelin-control-plane/tests/cp_d7_live_migration_drill.rs` (P-431); this proves the storage step the
//! orchestration calls is itself 0-loss + source-shredded, on the SAME `SignalSource`
//! [`SignalName::RestoreCrossSeamMismatch`] zero STOR-D1/STOR-D2 use (so STOR-D1/D2 hold across the
//! cell boundary by construction — observability is part of the pass, EI-01 §3).
//!
//! The drill proves the gate can go RED (an unwhole target ABORTS the move BEFORE any source shred — a
//! drill that cannot go red is not a gate, EI-01 §3) AND green (a same-region move completes, 0 loss,
//! source shredded). No threshold weakened.
//!
//! ## The PROMOTION (recorded, VISION §3)
//! The single-cell floor (M1) is PROMOTED to multi-cell — the cell→cell migration storage step is
//! LIVE. The SIBLING (multi-cell DSR erase fan-out, iterate `member_cells ∪ home_cell`) is
//! **P-ST-33 (global P-445)**, named here.

use myelin_harness::{Predicate, SignalName, SignalSource};
use myelin_storage::{
    migrate_cell_to_cell, BlobPresence, CellMigrationError, CellMigrationRequest, CellTenantTiers,
    ContentHash, ContinuousArchiver, KekId, KeyClass, KmsEngine, SourceLog, WalRow, WalSegment,
};
use myelin_tenancy::{CellId, Region, TenantId};

fn region() -> Region {
    Region::new("fr-par")
}
fn h(s: &str) -> ContentHash {
    ContentHash::blake3(s.as_bytes())
}

/// Backups covering offsets `0..=tail` (a base at 0 + the WAL tail archived to `tail`).
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

/// A cell copy of ACME: a source log projecting two rows (each referencing a present blob), a reachable
/// archiver, and a per-cell KMS with a live tenant KEK + DEK.
fn acme_tiers() -> CellTenantTiers {
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
    ];
    let mut blobs = BlobPresence::new();
    blobs.insert(h("blob-90"));
    blobs.insert(h("blob-100"));
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(TenantId::from_token("acme"), region()));
    kms.ensure_dek(&TenantId::from_token("acme"), &region(), KeyClass::Tenant)
        .unwrap();
    CellTenantTiers {
        source,
        rows,
        blobs,
        archiver: reachable_archiver(300),
        kms,
    }
}

fn request() -> CellMigrationRequest {
    CellMigrationRequest {
        tenant: TenantId::from_token("acme"),
        source_cell: CellId::from_token("cell-fr-1"),
        target_cell: CellId::from_token("cell-fr-2"),
        region: region(),
        cut_over_offset: 100,
    }
}

/// **THE DRILL (dated green artifact): cell→cell migration restores the §7.3 consistency point into the
/// target with 0 loss, lands in-region, and crypto-shreds the SOURCE.** The receipt records 0 loss +
/// the source key destroyed; the harness `RestoreCrossSeamMismatch` zero (the SAME STOR-D1/D2 zero)
/// reads `0` across the cell boundary; the source DEK no longer resolves (source unrecoverable) while
/// the target DEK still resolves (the tenant keeps serving).
#[test]
fn cp_d7_storage_cell_to_cell_zero_loss_in_region_source_shredded() {
    let tenant = TenantId::from_token("acme");
    let source = acme_tiers();
    let mut target = acme_tiers();

    // The source DEK resolves BEFORE the move (the source copy is live).
    let src_dek = source
        .kms
        .ensure_dek(&tenant, &region(), KeyClass::Tenant)
        .unwrap();
    assert!(source.kms.resolve_dek(&src_dek, &region()).is_ok());

    let receipt = migrate_cell_to_cell(&request(), &source, &mut target)
        .expect("a same-region cell→cell move completes");

    // 0 loss across-seam + lands in-region + source shredded (the CP-D7 receipt telemetry).
    assert_eq!(
        receipt.rows_migrated, 2,
        "0 loss: both source rows ≤ T migrated"
    );
    assert_eq!(receipt.cross_seam_mismatches, 0, "0 cross-seam mismatches");
    assert_eq!(
        receipt.restored_to_offset, 100,
        "restored to the cut-over point"
    );
    assert_eq!(receipt.region.as_str(), "fr-par", "lands IN-region");
    assert!(receipt.source_key_destroyed, "source crypto-shredded");

    // The SOURCE copy is unrecoverable; the TARGET copy keeps serving.
    assert!(
        source.kms.resolve_dek(&src_dek, &region()).is_err(),
        "after the move the SOURCE copy is unrecoverable (crypto-shred)"
    );
    let tgt_dek = target
        .kms
        .ensure_dek(&tenant, &region(), KeyClass::Tenant)
        .unwrap();
    assert!(
        target.kms.resolve_dek(&tgt_dek, &region()).is_ok(),
        "the TARGET copy keeps serving (the tenant's live data is intact)"
    );

    // The green artifact: emit the 0-loss-across-seam telemetry observably on the SAME signal surface
    // STOR-D1/STOR-D2 use (so STOR-D1/D2 hold across the cell boundary). 0 cross-seam mismatches.
    let mut signals = SignalSource::new();
    signals.set_scalar(
        SignalName::RestoreCrossSeamMismatch,
        receipt.cross_seam_mismatches as i64,
    );
    signals
        .assert_signal(SignalName::RestoreCrossSeamMismatch, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-443 CP-D7 DRILL GREEN 2026-06-24] storage cell→cell migration: tenant `acme` moved \
         cell-fr-1 → cell-fr-2 (same region fr-par) — {} rows restored into the target at the §7.3 \
         consistency point (offset {}), {} cross-seam mismatch(es) (0 loss across-seam), \
         source_key_destroyed={} (the SOURCE copy is unrecoverable, live AND in every backup), the \
         TARGET copy keeps serving. STOR-D1/STOR-D2 hold across the cell boundary (the same \
         RestoreCrossSeamMismatch==0 zero). Single-cell floor PROMOTED to multi-cell; multi-cell DSR \
         erase fan-out -> P-ST-33 (P-445).",
        receipt.rows_migrated,
        receipt.restored_to_offset,
        receipt.cross_seam_mismatches,
        receipt.source_key_destroyed,
    );
}

/// **The drill CATCHES an unwhole move (the gate is real, not vacuous): an unwhole target ABORTS the
/// move BEFORE any source crypto-shred — 0 loss.** A target object tier missing the referenced blob
/// (the §7.3 silent-corruption case) makes the restore-into-target FAIL — the move aborts, the source
/// is NOT shredded (it keeps serving). This proves the gate would fail on a regression (a move that
/// lost/leaked data) — EI-01 §3.
#[test]
fn cp_d7_catches_an_unwhole_move_source_untouched() {
    let tenant = TenantId::from_token("acme");
    let source = acme_tiers();
    let mut target = acme_tiers();
    target.blobs = BlobPresence::new(); // the referenced blobs are ABSENT in the target.
    let src_dek = source
        .kms
        .ensure_dek(&tenant, &region(), KeyClass::Tenant)
        .unwrap();

    let err = migrate_cell_to_cell(&request(), &source, &mut target)
        .expect_err("an unwhole target aborts the move BEFORE the source shred");
    assert!(matches!(err, CellMigrationError::TargetRestoreNotWhole(_)));

    // 0 loss: the source was NOT shredded (the abort is before the shred) — the gate would FAIL CI.
    assert!(
        source.kms.resolve_dek(&src_dek, &region()).is_ok(),
        "an aborted move leaves the source untouched (0 loss) — the gate caught the regression"
    );

    // The RED read: a non-zero cross-seam mismatch would read RED on the SAME signal surface.
    let mut signals = SignalSource::new();
    signals.set_scalar(SignalName::RestoreCrossSeamMismatch, 1);
    let verdict = signals.assert_signal(SignalName::RestoreCrossSeamMismatch, Predicate::Eq(0));
    assert!(
        !verdict.is_green(),
        "a non-zero cross-seam mismatch reads RED — the gate is not vacuous"
    );

    println!(
        "[P-443 CP-D7 DRILL RED-PROOF 2026-06-24] an unwhole target ABORTED the move BEFORE any \
         source crypto-shred (0 loss: the source is untouched, the tenant keeps serving) — the gate \
         is real (it goes red on a move that would lose/leak data)."
    );
}
