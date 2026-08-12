use myelin_tenancy::{CellId, CrossCellPointer, Region, TenantId};

use crate::backup::{ContinuousArchiver, WalOffset};
use crate::kms::{KekId, KmsEngine, KmsError};
use crate::restore::{restore_to_offset, BlobPresence, RestoreError, SourceLog, WalRow};

pub struct CellTenantTiers {
    pub source: SourceLog,
    pub rows: Vec<WalRow>,
    pub blobs: BlobPresence,
    pub archiver: ContinuousArchiver,
    pub kms: KmsEngine,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellMigrationRequest {
    pub tenant: TenantId,
    pub source_cell: CellId,
    pub target_cell: CellId,
    pub region: Region,
    pub cut_over_offset: WalOffset,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CellMigrationError {
    TargetRestoreNotWhole(RestoreError),
    SourceKeyShred(KmsError),
}

impl core::fmt::Display for CellMigrationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CellMigrationError::TargetRestoreNotWhole(e) => write!(
                f,
                "cell→cell migration ABORTED: the target restore-into (reindex-from-source to the \
                 §7.3 consistency point) is NOT whole - the move is aborted BEFORE any source \
                crypto-shred (0 loss: the source is untouched, the tenant keeps serving). Detail: {e}"
            ),
            CellMigrationError::SourceKeyShred(error) => write!(
                f,
                "cell→cell migration copied the target but could not shred the source key: {error}"
            ),
        }
    }
}

impl std::error::Error for CellMigrationError {}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a cell-migration receipt carries the 0-loss + source-shredded CP-D7 proof - dropping it \
              discards the evidence the move was whole and the source unrecoverable"]
pub struct CellMigrationReceipt {
    pub tenant: TenantId,
    pub source_cell: CellId,
    pub target_cell: CellId,
    pub region: Region,
    pub restored_to_offset: WalOffset,
    pub rows_migrated: u64,
    pub cross_seam_mismatches: u64,
    pub source_key_destroyed: bool,
}

pub fn migrate_cell_to_cell(
    request: &CellMigrationRequest,
    source: &CellTenantTiers,
    target: &mut CellTenantTiers,
) -> Result<CellMigrationReceipt, CellMigrationError> {
    let CellMigrationRequest {
        tenant,
        source_cell,
        target_cell,
        region,
        cut_over_offset,
    } = request;
    let cut_over_offset = *cut_over_offset;

    let report = restore_to_offset(
        &target.archiver,
        cut_over_offset,
        &source.rows,
        &target.blobs,
        &source.source,
        &target.kms,
    )
    .map_err(CellMigrationError::TargetRestoreNotWhole)?;

    target.source = source.source.clone();
    target.rows = report.oltp_rows.clone();
    let rows_migrated = report.oltp_rows.len() as u64;

    let source_key_destroyed = source
        .kms
        .destroy_kek(&KekId::new(tenant.clone(), region.clone()))
        .map_err(CellMigrationError::SourceKeyShred)?;

    Ok(CellMigrationReceipt {
        tenant: tenant.clone(),
        source_cell: source_cell.clone(),
        target_cell: target_cell.clone(),
        region: region.clone(),
        restored_to_offset: report.restored_to_offset,
        rows_migrated,
        cross_seam_mismatches: report.dangling_ref_count,
        source_key_destroyed,
    })
}

#[inline]
pub fn is_cell_local(pointer: &CrossCellPointer, this_cell: &CellId) -> bool {
    pointer.home_cell() == this_cell
}

#[inline]
pub fn storage_resolves_locally(pointer: &CrossCellPointer, this_cell: &CellId) -> bool {
    is_cell_local(pointer, this_cell)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob::ContentHash;
    use crate::kms::KeyClass;
    use myelin_tenancy::{ArtifactRef, ArtifactType, CorrelationId, OpaqueSubjectId};

    fn region() -> Region {
        Region::new("fr-par")
    }

    fn content(bytes: &[u8]) -> ContentHash {
        ContentHash::blake3(bytes)
    }

    fn reachable_archiver(tail: WalOffset) -> ContinuousArchiver {
        use crate::backup::WalSegment;
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
        let blob = content(b"acme-blob");
        let mut source = SourceLog::new();
        source.append(50, "r50");
        source.append(100, "r100");
        let rows = vec![
            WalRow {
                id: "r50".into(),
                written_at: 50,
                blob_ref: None,
            },
            WalRow {
                id: "r100".into(),
                written_at: 100,
                blob_ref: Some(blob.clone()),
            },
        ];
        let mut blobs = BlobPresence::new();
        blobs.insert(blob);
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

    fn request(offset: WalOffset) -> CellMigrationRequest {
        CellMigrationRequest {
            tenant: TenantId::from_token("acme"),
            source_cell: CellId::from_token("cell-fr-1"),
            target_cell: CellId::from_token("cell-fr-2"),
            region: region(),
            cut_over_offset: offset,
        }
    }

    #[test]
    fn migrate_cell_to_cell_zero_loss_in_region_source_shredded() {
        let tenant = TenantId::from_token("acme");
        let source = acme_tiers();
        let mut target = acme_tiers();

        let src_dek = source
            .kms
            .ensure_dek(&tenant, &region(), KeyClass::Tenant)
            .unwrap();
        assert!(source.kms.resolve_dek(&src_dek, &region()).is_ok());

        let receipt = migrate_cell_to_cell(&request(100), &source, &mut target)
            .expect("a same-region cell→cell move completes");

        assert_eq!(receipt.rows_migrated, 2, "both source rows ≤ 100 migrated");
        assert_eq!(
            receipt.restored_to_offset, 100,
            "restored to the cut-over point"
        );
        assert_eq!(receipt.cross_seam_mismatches, 0, "the target is whole");
        assert_eq!(receipt.region.as_str(), "fr-par", "lands IN-region");
        assert_eq!(receipt.source_cell.as_str(), "cell-fr-1");
        assert_eq!(receipt.target_cell.as_str(), "cell-fr-2");

        assert!(receipt.source_key_destroyed, "the source key was destroyed");
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
    }

    #[test]
    fn migration_reindexes_target_from_source_not_backup() {
        use crate::restore::ReindexFromSource;
        let source = acme_tiers();
        let mut target = acme_tiers();
        target.source = SourceLog::new();

        let _receipt =
            migrate_cell_to_cell(&request(100), &source, &mut target).expect("the move completes");

        let from_source = ReindexFromSource::reindex(&source.source, 100);
        let target_derived = ReindexFromSource::reindex(&target.source, 100);
        assert_eq!(
            target_derived.docs(),
            from_source.docs(),
            "the target derived store is the SOURCE replay (reindex-from-source, never a backup)"
        );
        assert!(
            from_source.has_doc("r50") && from_source.has_doc("r100"),
            "the source rows are projected into the target"
        );
    }

    #[test]
    fn migrate_cell_to_cell_aborts_before_shred_on_unwhole_target() {
        let tenant = TenantId::from_token("acme");
        let source = acme_tiers();
        let mut target = acme_tiers();
        target.blobs = BlobPresence::new();
        let src_dek = source
            .kms
            .ensure_dek(&tenant, &region(), KeyClass::Tenant)
            .unwrap();

        let err = migrate_cell_to_cell(&request(100), &source, &mut target)
            .expect_err("an unwhole target aborts the move");
        assert!(
            matches!(err, CellMigrationError::TargetRestoreNotWhole(_)),
            "the move aborts on the unwhole target restore: {err}"
        );
        assert!(
            err.to_string().contains("BEFORE any source crypto-shred"),
            "loud abort reason: {err}"
        );

        assert!(
            source.kms.resolve_dek(&src_dek, &region()).is_ok(),
            "an aborted move leaves the source untouched (0 loss)"
        );
    }

    #[test]
    fn migrate_cell_to_cell_aborts_on_unreachable_cut_over() {
        let source = acme_tiers();
        let mut target = acme_tiers();
        target.archiver = reachable_archiver(50);

        let err = migrate_cell_to_cell(&request(100), &source, &mut target)
            .expect_err("an unreachable cut-over aborts the move");
        assert!(matches!(
            err,
            CellMigrationError::TargetRestoreNotWhole(RestoreError::PitrUnreachable(_))
        ));
    }

    #[test]
    fn receipt_lands_in_the_requested_region() {
        let source = acme_tiers();
        let mut target = acme_tiers();
        let receipt = migrate_cell_to_cell(&request(100), &source, &mut target).unwrap();
        assert_eq!(
            receipt.region,
            region(),
            "the move lands IN the request's region"
        );
    }

    fn pointer(subject: &str, home: &str) -> CrossCellPointer {
        CrossCellPointer::new(
            OpaqueSubjectId::from_ref(ArtifactRef(subject.into())),
            ArtifactType::Issue,
            CorrelationId("01J0CORR".into()),
            CellId::from_token(home),
        )
    }

    #[test]
    fn cross_cell_pointer_subject_is_opaque_no_pii() {
        let p = pointer("myelin://01J0ACME/issues/issue/7", "cell-fr-2");
        assert_eq!(
            p.subject().artifact_ref().0,
            "myelin://01J0ACME/issues/issue/7"
        );
        assert_eq!(p.home_cell().as_str(), "cell-fr-2");
        assert_eq!(p.artifact_type(), &ArtifactType::Issue);
        assert_eq!(p.correlation_id(), &CorrelationId("01J0CORR".into()));
    }

    #[test]
    fn storage_resolves_a_pointer_locally_iff_homed_here() {
        let here = CellId::from_token("cell-fr-1");
        let local = pointer("myelin://01J0ACME/issues/issue/7", "cell-fr-1");
        let foreign = pointer("myelin://01J0BETA/issues/issue/9", "cell-fr-2");

        assert!(is_cell_local(&local, &here));
        assert!(storage_resolves_locally(&local, &here));

        assert!(!is_cell_local(&foreign, &here));
        assert!(
            !storage_resolves_locally(&foreign, &here),
            "a foreign-homed pointer is never read into the local tier (resolution is cell-local)"
        );
    }
}
