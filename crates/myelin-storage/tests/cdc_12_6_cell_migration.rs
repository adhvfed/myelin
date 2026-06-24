//! Contract 12.6 / 11.5 CDC pair — the **cell→cell migration storage step** (P-ST-32 / global P-443).
//!
//! The prompt requires "CDC: provider+consumer pair for the cell→cell migration". This is the
//! consumer-driven contract test:
//!
//! - the **PROVIDER** is `myelin-storage` — the [`migrate_cell_to_cell`] storage step this prompt ships
//!   (restore the source's §7.3 cross-seam consistency point INTO the target cell with 0 loss, then
//!   crypto-shred the SOURCE cell's key) + the [`CellMigrationRequest`] / [`CellMigrationReceipt`] /
//!   [`CellTenantTiers`] / [`CellMigrationError`] types;
//! - the **CONSUMER** is the **control-plane live-migration orchestrator**
//!   (`myelin_control_plane::migration::LiveMigration`, P-431) modelled here as a tiny
//!   `LiveMigrationOrchestrator`. It drives the storage step exactly as the real orchestrator does:
//!   restore-into-the-target at the cross-seam cut-over offset, read the receipt to decide
//!   completed/aborted, and (in production) re-point the placement ATOMICALLY only AFTER the storage
//!   step returned a whole receipt. This is exactly the call shape the real orchestrator relies on — if
//!   `migrate_cell_to_cell`'s signature / the receipt shape / the abort-before-shred contract drift,
//!   this stops compiling/passing.
//!
//! It also pins the load-bearing contract properties the consumer depends on: a migration to an unwhole
//! target ABORTS BEFORE any source crypto-shred (0 loss — the orchestrator must NOT cut over), and a
//! completed migration crypto-shreds the SOURCE (the source copy is unrecoverable) while leaving the
//! TARGET serving.
//!
//! NOTE on the rows: contract 12.6 is the cross-cell PII-free pointer bridge (resolution cell-local —
//! exercised in the unit tests `cell_migration::tests`); contract 11.5 is the restore/cross-seam
//! machinery the cell→cell migration USES. This CDC pair covers the cell→cell migration storage step
//! (the storage half of CP-D7) that rides BOTH rows; the bridge-resolution CDC is the control-plane
//! `cdc_12_6_bridge_resolution_live` (P-429) and the events-propagation `cdc_12_6_crosscell` (P-438).

use myelin_storage::{
    migrate_cell_to_cell, BlobPresence, CellMigrationError, CellMigrationReceipt,
    CellMigrationRequest, CellTenantTiers, ContentHash, ContinuousArchiver, KekId, KeyClass,
    KmsEngine, SourceLog, WalRow, WalSegment,
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

/// A consumer of the cell→cell migration storage step: the control-plane live-migration orchestrator
/// (the P-431 caller). It drives the provider exactly as the real orchestrator does — restore-into-the
/// target at the cross-seam cut-over offset, then (only on a whole receipt) cut over the placement.
/// Modelled here with a `placement_home` it flips ATOMICALLY only when the storage step succeeds.
struct LiveMigrationOrchestrator {
    /// The tenant's current home cell (the placement fact the orchestrator owns).
    placement_home: CellId,
}

impl LiveMigrationOrchestrator {
    fn new(home: &str) -> Self {
        LiveMigrationOrchestrator {
            placement_home: CellId::from_token(home),
        }
    }

    /// Drive the storage step then cut over IFF it returned a whole receipt (the real orchestrator's
    /// shape: the placement is re-pointed ONLY after the storage step is whole — a half-moved tenant
    /// never exists).
    fn migrate(
        &mut self,
        request: &CellMigrationRequest,
        source: &CellTenantTiers,
        target: &mut CellTenantTiers,
    ) -> Result<CellMigrationReceipt, CellMigrationError> {
        let receipt = migrate_cell_to_cell(request, source, target)?;
        // The storage step is whole → cut over the placement (atomically, in production).
        self.placement_home = request.target_cell.clone();
        Ok(receipt)
    }
}

/// **The provider+consumer contract holds: the orchestrator drives the storage step, gets a whole
/// receipt (0 loss, source shredded), and cuts over the placement.** The exact call shape the real
/// control-plane orchestrator relies on.
#[test]
fn cdc_orchestrator_drives_a_whole_cell_to_cell_move() {
    let tenant = TenantId::from_token("acme");
    let source = acme_tiers();
    let mut target = acme_tiers();
    let src_dek = source
        .kms
        .ensure_dek(&tenant, &region(), KeyClass::Tenant)
        .unwrap();

    let request = CellMigrationRequest {
        tenant: tenant.clone(),
        source_cell: CellId::from_token("cell-fr-1"),
        target_cell: CellId::from_token("cell-fr-2"),
        region: region(),
        cut_over_offset: 100,
    };
    let mut orch = LiveMigrationOrchestrator::new("cell-fr-1");

    let receipt = orch
        .migrate(&request, &source, &mut target)
        .expect("the orchestrator completes the move");

    // The receipt shape the consumer reads: 0 loss, in-region, source shredded.
    assert_eq!(receipt.rows_migrated, 2);
    assert_eq!(receipt.cross_seam_mismatches, 0);
    assert_eq!(receipt.region.as_str(), "fr-par");
    assert!(receipt.source_key_destroyed);

    // The orchestrator cut over the placement to the target (only AFTER the whole receipt).
    assert_eq!(orch.placement_home.as_str(), "cell-fr-2");
    // The source copy is unrecoverable.
    assert!(source.kms.resolve_dek(&src_dek, &region()).is_err());
}

/// **The consumer-depended contract: an unwhole target ABORTS BEFORE any source shred — the
/// orchestrator does NOT cut over (0 loss).** If `migrate_cell_to_cell` ever shredded the source on an
/// unwhole target, this would catch it — the orchestrator's no-cut-over-on-abort invariant rides this
/// contract.
#[test]
fn cdc_unwhole_target_aborts_and_orchestrator_does_not_cut_over() {
    let tenant = TenantId::from_token("acme");
    let source = acme_tiers();
    let mut target = acme_tiers();
    target.blobs = BlobPresence::new(); // the referenced blobs are absent — the restore aborts.
    let src_dek = source
        .kms
        .ensure_dek(&tenant, &region(), KeyClass::Tenant)
        .unwrap();

    let request = CellMigrationRequest {
        tenant: tenant.clone(),
        source_cell: CellId::from_token("cell-fr-1"),
        target_cell: CellId::from_token("cell-fr-2"),
        region: region(),
        cut_over_offset: 100,
    };
    let mut orch = LiveMigrationOrchestrator::new("cell-fr-1");

    let err = orch
        .migrate(&request, &source, &mut target)
        .expect_err("an unwhole target aborts the move");
    assert!(matches!(err, CellMigrationError::TargetRestoreNotWhole(_)));

    // The orchestrator did NOT cut over (the placement stays on the source) — 0 loss.
    assert_eq!(orch.placement_home.as_str(), "cell-fr-1");
    // The source was NOT crypto-shredded (the abort is before the shred).
    assert!(
        source.kms.resolve_dek(&src_dek, &region()).is_ok(),
        "an aborted move leaves the source untouched (0 loss)"
    );
}
