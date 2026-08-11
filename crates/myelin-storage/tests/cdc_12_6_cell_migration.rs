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

struct LiveMigrationOrchestrator {
    placement_home: CellId,
}

impl LiveMigrationOrchestrator {
    fn new(home: &str) -> Self {
        LiveMigrationOrchestrator {
            placement_home: CellId::from_token(home),
        }
    }

    fn migrate(
        &mut self,
        request: &CellMigrationRequest,
        source: &CellTenantTiers,
        target: &mut CellTenantTiers,
    ) -> Result<CellMigrationReceipt, CellMigrationError> {
        let receipt = migrate_cell_to_cell(request, source, target)?;
        self.placement_home = request.target_cell.clone();
        Ok(receipt)
    }
}

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

    assert_eq!(receipt.rows_migrated, 2);
    assert_eq!(receipt.cross_seam_mismatches, 0);
    assert_eq!(receipt.region.as_str(), "fr-par");
    assert!(receipt.source_key_destroyed);

    assert_eq!(orch.placement_home.as_str(), "cell-fr-2");
    assert!(source.kms.resolve_dek(&src_dek, &region()).is_err());
}

#[test]
fn cdc_unwhole_target_aborts_and_orchestrator_does_not_cut_over() {
    let tenant = TenantId::from_token("acme");
    let source = acme_tiers();
    let mut target = acme_tiers();
    target.blobs = BlobPresence::new();
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

    assert_eq!(orch.placement_home.as_str(), "cell-fr-1");
    assert!(
        source.kms.resolve_dek(&src_dek, &region()).is_ok(),
        "an aborted move leaves the source untouched (0 loss)"
    );
}
