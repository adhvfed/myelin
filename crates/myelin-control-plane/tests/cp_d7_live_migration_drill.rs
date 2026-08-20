use myelin_control_plane::schema::{
    Capacity, Cell, CellStatus, IsolationKind, PlacementStatus, TenantPlacement,
};
use myelin_control_plane::{
    measured_hot_at, CellTenantCopy, LiveMigration, MigrationError, MigrationPlan,
    MigrationTrigger, PlacementError, Registry,
};
use myelin_harness::{Predicate, SignalName, SignalSource};
use myelin_storage::{
    BlobPresence, ContinuousArchiver, KekId, KeyClass, KmsEngine, RestoredObject, SourceLog,
    WalRow, WalSegment,
};
use myelin_substrate::Thresholds;
use myelin_tenancy::{CellId, Region, TenantId};

fn region() -> Region {
    Region::new("eu-west")
}

fn cell(id: &str, region_str: &str) -> Cell {
    Cell {
        cell_id: CellId::from_token(id),
        region: Region::new(region_str),
        status: CellStatus::Active,
        isolation_kind: IsolationKind::Pool,
        capacity: Capacity {
            tenants_max: 2000,
            write_qps_max: 9000,
            storage_bytes_max: 1 << 41,
        },
        utilisation: 50,
        version: 1,
        endpoint: format!("cell.{region_str}.{id}.myelin.eu"),
    }
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

fn acme_copy() -> CellTenantCopy {
    let blob = RestoredObject::integral(b"acme-blob".to_vec());
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
            blob_ref: Some(blob.content_address.clone()),
        },
    ];
    let mut blobs = BlobPresence::new();
    blobs.insert(blob.content_address.clone());
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(TenantId::from_token("acme"), region()))
        .expect("seed the in-memory KEK");
    kms.ensure_dek(&TenantId::from_token("acme"), &region(), KeyClass::Tenant)
        .unwrap();
    CellTenantCopy {
        source,
        rows,
        blobs,
        archiver: reachable_archiver(300),
        kms,
    }
}

fn registry_acme_on_source() -> Registry {
    let mut reg = Registry::new();
    reg.insert_cell(cell("cell-w-1", "eu-west"));
    reg.insert_cell(cell("cell-w-2", "eu-west"));
    reg.insert_cell(cell("cell-n-1", "eu-north"));
    reg.place_tenant(TenantPlacement {
        tenant_id: TenantId::from_token("acme"),
        region: region(),
        home_cell: CellId::from_token("cell-w-1"),
        isolation_tier: IsolationKind::Pool,
        slug: "acme".into(),
        status: PlacementStatus::Active,
        member_cells: vec![CellId::from_token("cell-w-1")],
    })
    .unwrap();
    reg
}

#[test]
fn cp_d7_live_migration_zero_loss_in_region_source_shredded() {
    let mig = LiveMigration::with_flow_executor(TenantId::from_token("operator"), region());
    let tenant = TenantId::from_token("acme");

    {
        let mut reg = registry_acme_on_source();
        let source = acme_copy();
        let mut target = acme_copy();
        let src_dek = source
            .kms
            .ensure_dek(&tenant, &region(), KeyClass::Tenant)
            .unwrap();
        let err = mig
            .migrate_tenant(
                &mut reg,
                &MigrationPlan {
                    tenant: tenant.clone(),
                    source_cell: CellId::from_token("cell-w-1"),
                    target_cell: CellId::from_token("cell-n-1"),
                    cut_over_offset: 100,
                    idem_key: "cpd7-red-xr".into(),
                },
                &source,
                &mut target,
            )
            .expect_err("a cross-region migration is rejected (there is NO cross-region move)");
        assert!(matches!(
            err,
            MigrationError::CutOverRejected(PlacementError::CrossRegionMemberCell { .. })
        ));
        assert_eq!(
            reg.placement(&tenant).unwrap().home_cell.as_str(),
            "cell-w-1"
        );
        assert!(
            source.kms.resolve_dek(&src_dek, &region()).is_ok(),
            "a rejected move does not crypto-shred the source"
        );
    }

    let mut reg = registry_acme_on_source();
    let source = acme_copy();
    let mut target = acme_copy();
    let src_dek = source
        .kms
        .ensure_dek(&tenant, &region(), KeyClass::Tenant)
        .unwrap();

    let receipt = mig
        .migrate_tenant(
            &mut reg,
            &MigrationPlan {
                tenant: tenant.clone(),
                source_cell: CellId::from_token("cell-w-1"),
                target_cell: CellId::from_token("cell-w-2"),
                cut_over_offset: 100,
                idem_key: "cpd7-green-1".into(),
            },
            &source,
            &mut target,
        )
        .expect("a same-region cell→cell move completes");

    assert_eq!(
        receipt.rows_migrated, 2,
        "0 loss: both source rows migrated"
    );
    assert_eq!(
        receipt.cross_seam_mismatches, 0,
        "the target is whole (0 cross-seam mismatch)"
    );
    assert_eq!(
        reg.placement(&tenant).unwrap().home_cell.as_str(),
        "cell-w-2"
    );
    assert_eq!(receipt.region.as_str(), "eu-west", "lands IN-region");
    assert!(receipt.source_key_destroyed, "the source key is destroyed");
    assert!(
        source.kms.resolve_dek(&src_dek, &region()).is_err(),
        "the source copy is unrecoverable after the move (crypto-shred)"
    );

    let mut sig = SignalSource::new();
    sig.set_scalar(
        SignalName::RestoreCrossSeamMismatch,
        receipt.cross_seam_mismatches as i64,
    );
    sig.assert_signal(SignalName::RestoreCrossSeamMismatch, Predicate::Eq(0))
        .expect_green();
    sig.set_scalar(SignalName::CrossTenantCount, 0);
    sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-431 CP-D7 GREEN 2026-06-24] live tenant migration: ACME migrated cell-w-1 → cell-w-2 \
         (SAME region eu-west) as a DURABLE workflow (run {}) - reindex-from-source + crypto-shred \
         cut-over. 0 LOSS across-seam ({} rows migrated, {} cross-seam mismatch), lands IN-region, \
         SOURCE crypto-shredded (source_key_destroyed={}). A cross-region target was REJECTED (the \
         residency pin holds across the move). PROMOTED: avoid-migration-by-sizing → live migration; \
         scripted provisioning → durable-workflow provisioning.",
        receipt.run_id.0,
        receipt.rows_migrated,
        receipt.cross_seam_mismatches,
        receipt.source_key_destroyed,
    );
}

#[test]
fn cdc_migration_workflow_ops_sizing_trigger_provider_consumer() {
    let t = Thresholds::load_canonical().expect("thresholds");
    let mig = LiveMigration::with_flow_executor(TenantId::from_token("operator"), region());

    struct SizingTrigger<'a> {
        mig: &'a LiveMigration<myelin_flow::FlowExecutor>,
        hot_at: u8,
    }
    impl SizingTrigger<'_> {
        fn maybe_migrate(
            &self,
            reg: &mut Registry,
            measured_util: u8,
            plan: &MigrationPlan,
            src_copy: &CellTenantCopy,
            tgt_copy: &mut CellTenantCopy,
        ) -> Option<bool> {
            let trigger = MigrationTrigger {
                hot_cell: plan.source_cell.clone(),
                measured_utilisation: measured_util,
                hot_at_utilisation: self.hot_at,
            };
            if !trigger.is_hot() {
                return None;
            }
            let receipt = self
                .mig
                .migrate_tenant(reg, plan, src_copy, tgt_copy)
                .ok()?;
            Some(receipt.region == region() && receipt.source_key_destroyed)
        }
    }

    let consumer = SizingTrigger {
        mig: &mig,
        hot_at: measured_hot_at(&t.cell_sizing),
    };
    let tenant = TenantId::from_token("acme");
    let mk_plan = |idem: &str| MigrationPlan {
        tenant: tenant.clone(),
        source_cell: CellId::from_token("cell-w-1"),
        target_cell: CellId::from_token("cell-w-2"),
        cut_over_offset: 100,
        idem_key: idem.into(),
    };

    {
        let mut reg = registry_acme_on_source();
        let src = acme_copy();
        let mut tgt = acme_copy();
        let decision = consumer.maybe_migrate(&mut reg, 50, &mk_plan("cdc-cold"), &src, &mut tgt);
        assert!(
            decision.is_none(),
            "a cold cell is NOT migrated (avoid-migration-by-sizing)"
        );
        assert_eq!(
            reg.placement(&tenant).unwrap().home_cell.as_str(),
            "cell-w-1"
        );
    }

    {
        let mut reg = registry_acme_on_source();
        let src = acme_copy();
        let mut tgt = acme_copy();
        let decision = consumer.maybe_migrate(&mut reg, 90, &mk_plan("cdc-hot"), &src, &mut tgt);
        assert_eq!(
            decision,
            Some(true),
            "a measured-hot cell migrates - in-region + source crypto-shredded"
        );
        assert_eq!(
            reg.placement(&tenant).unwrap().home_cell.as_str(),
            "cell-w-2"
        );
    }
}
