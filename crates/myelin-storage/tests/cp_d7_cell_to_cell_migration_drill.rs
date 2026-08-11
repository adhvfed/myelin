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
    kms.ensure_kek(&KekId::new(TenantId::from_token("acme"), region()))
        .expect("seed the in-memory KEK");
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

#[test]
fn cp_d7_storage_cell_to_cell_zero_loss_in_region_source_shredded() {
    let tenant = TenantId::from_token("acme");
    let source = acme_tiers();
    let mut target = acme_tiers();

    let src_dek = source
        .kms
        .ensure_dek(&tenant, &region(), KeyClass::Tenant)
        .unwrap();
    assert!(source.kms.resolve_dek(&src_dek, &region()).is_ok());

    let receipt = migrate_cell_to_cell(&request(), &source, &mut target)
        .expect("a same-region cell→cell move completes");

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
         cell-fr-1 → cell-fr-2 (same region fr-par) - {} rows restored into the target at the §7.3 \
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

#[test]
fn cp_d7_catches_an_unwhole_move_source_untouched() {
    let tenant = TenantId::from_token("acme");
    let source = acme_tiers();
    let mut target = acme_tiers();
    target.blobs = BlobPresence::new();
    let src_dek = source
        .kms
        .ensure_dek(&tenant, &region(), KeyClass::Tenant)
        .unwrap();

    let err = migrate_cell_to_cell(&request(), &source, &mut target)
        .expect_err("an unwhole target aborts the move BEFORE the source shred");
    assert!(matches!(err, CellMigrationError::TargetRestoreNotWhole(_)));

    assert!(
        source.kms.resolve_dek(&src_dek, &region()).is_ok(),
        "an aborted move leaves the source untouched (0 loss) - the gate caught the regression"
    );

    let mut signals = SignalSource::new();
    signals.set_scalar(SignalName::RestoreCrossSeamMismatch, 1);
    let verdict = signals.assert_signal(SignalName::RestoreCrossSeamMismatch, Predicate::Eq(0));
    assert!(
        !verdict.is_green(),
        "a non-zero cross-seam mismatch reads RED - the gate is not vacuous"
    );

    println!(
        "[P-443 CP-D7 DRILL RED-PROOF 2026-06-24] an unwhole target ABORTED the move BEFORE any \
         source crypto-shred (0 loss: the source is untouched, the tenant keeps serving) - the gate \
         is real (it goes red on a move that would lose/leak data)."
    );
}
