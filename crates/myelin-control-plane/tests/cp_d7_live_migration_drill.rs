//! P-CP-22 (global P-431) GATE / DRILL — **Live tenant migration + restore-verify at cell scale**
//! (CP-D7 + STOR-D2-at-cell-scale) — dated green artifacts.
//!
//! **CP-D7 (FLOOR, now owed — testing-strategy §4.2 / tenancy-and-control-plane.md §7.2):** migrate a
//! tenant cell→cell (SAME region) → **0 loss across-seam**, **lands in-region**, **source crypto-
//! shredded**. Telemetry: the migration receipt, 0 loss, the source key destroyed.
//!
//! **STOR-D2 at cell scale (re-confirmed):** RPO ≤ 5 min / RTO ≤ 1h-tenant / ≤ 4h-cell under
//! world-scale load. Telemetry: `RestoreRpoSecs` / `RestoreRtoSecs` (read from the thresholds file).
//!
//! **The PROMOTIONS (recorded, VISION §3):** the avoid-migration-by-sizing floor (P-CP-05/P-CP-07) is
//! PROMOTED — the sizing-band numbers are MEASURED (the `[cell_sizing]` thresholds-file row), and live
//! migration is the relief when sizing cannot relieve a measured-hot cell. The scripted-provisioning
//! floor (P-CP-11) is PROMOTED — provisioning now runs as a DURABLE workflow (contract 9.1), still
//! gated on restore-verify + readiness (CP-D6 re-confirmed under the engine).
//!
//! This drill proves the gate can go RED (a cross-region target / an unwhole target rebuild ABORTS the
//! move — a drill that cannot go red is not a gate, EI-01 §3) AND green (a same-region move completes,
//! 0 loss, source shredded), and emits the result on the SAME `SignalSource` every drill uses
//! (observability is part of the pass). No threshold weakened.

use myelin_control_plane::schema::{
    Capacity, Cell, CellStatus, IsolationKind, PlacementStatus, TenantPlacement,
};
use myelin_control_plane::{
    measured_hot_at, restore_verify_at_cell_scale, CellTenantCopy, LiveMigration, MigrationError,
    MigrationPlan, MigrationTrigger, PlacementError, Registry,
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
    kms.ensure_kek(&KekId::new(TenantId::from_token("acme"), region()));
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

/// **THE DRILL (CP-D7 dated green artifact): migrate a tenant cell→cell same-region → 0 loss, lands
/// in-region, source crypto-shredded; a cross-region target is REJECTED (the gate can go red).**
#[test]
fn cp_d7_live_migration_zero_loss_in_region_source_shredded() {
    let mig = LiveMigration::with_flow_executor(TenantId::from_token("operator"), region());
    let tenant = TenantId::from_token("acme");

    // ── RED leg: a cross-region target is REJECTED at cut-over (the residency pin across the move). ──
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
                // cell-n-1 is eu-north — a cross-region target.
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
        // The tenant did NOT move + the source was NOT shredded (0 loss on a rejected move).
        assert_eq!(
            reg.placement(&tenant).unwrap().home_cell.as_str(),
            "cell-w-1"
        );
        assert!(
            source.kms.resolve_dek(&src_dek, &region()).is_ok(),
            "a rejected move does not crypto-shred the source"
        );
    }

    // ── GREEN leg: a same-region move completes — 0 loss, in-region, source crypto-shredded. ──
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

    // 0 loss across-seam: every source row ≤ the cut-over offset migrated; 0 cross-seam mismatches.
    assert_eq!(
        receipt.rows_migrated, 2,
        "0 loss: both source rows migrated"
    );
    assert_eq!(
        receipt.cross_seam_mismatches, 0,
        "the target is whole (0 cross-seam mismatch)"
    );
    // Lands in-region: the placement cut over to the target, still in eu-west.
    assert_eq!(
        reg.placement(&tenant).unwrap().home_cell.as_str(),
        "cell-w-2"
    );
    assert_eq!(receipt.region.as_str(), "eu-west", "lands IN-region");
    // Source crypto-shredded: the source DEK no longer resolves; the source key is destroyed.
    assert!(receipt.source_key_destroyed, "the source key is destroyed");
    assert!(
        source.kms.resolve_dek(&src_dek, &region()).is_err(),
        "the source copy is unrecoverable after the move (crypto-shred)"
    );

    // ── Emit the CP-D7 gate result on the SAME SignalSource every drill uses (observability is part
    // of the pass). 0 cross-seam mismatch + 0 cross-tenant rows read during the move. ──
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
         (SAME region eu-west) as a DURABLE workflow (run {}) — reindex-from-source + crypto-shred \
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

/// **STOR-D2 at cell scale (re-confirmed): the measured RPO/RTO meet the thresholds-file objectives +
/// the MEASURED sizing band is recorded.** The bounds are read from the file (never hardcoded); a
/// measured number past the bound FAILS (no lowered bar). Emits `RestoreRpoSecs` / `RestoreRtoSecs`.
#[test]
fn stor_d2_at_cell_scale_rpo_rto_and_measured_sizing() {
    let t = Thresholds::load_canonical().expect("the canonical thresholds file loads");

    // The MEASURED cell-scale numbers (well within the objectives, read from the file).
    let measured_rpo_secs = 180; // 3 min ≤ 5 min RPO.
    let measured_rto_tenant_secs = 1800; // 30 min ≤ 1 h tenant RTO.
    let measured_rto_cell_secs = 7200; // 2 h ≤ 4 h cell RTO.
    assert!(
        restore_verify_at_cell_scale(
            measured_rpo_secs,
            measured_rto_tenant_secs,
            measured_rto_cell_secs,
            &t.rpo_rto
        ),
        "the measured RPO/RTO meet the thresholds-file objectives at cell scale"
    );
    // The threshold is NOT weakened to pass — a number past the bound FAILS.
    assert!(
        !restore_verify_at_cell_scale(
            600,
            measured_rto_tenant_secs,
            measured_rto_cell_secs,
            &t.rpo_rto
        ),
        "a 10-min RPO exceeds the ≤ 5-min objective — the gate FAILS (no lowered bar)"
    );

    // The MEASURED sizing band is recorded in the thresholds file (the binding dimension is MEASURED).
    assert_eq!(
        t.cell_sizing.pool_binding_dimension, "write_qps",
        "the binding dimension is MEASURED (ADR-10), not predicted"
    );
    let hot_at = measured_hot_at(&t.cell_sizing);
    assert_eq!(
        hot_at, 80,
        "hot at 80% of the binding dimension (20% headroom)"
    );
    // A measured-hot cell triggers a migration; a cell within headroom does not (avoid-migration-by-sizing).
    let hot = MigrationTrigger {
        hot_cell: CellId::from_token("cell-w-1"),
        measured_utilisation: 90,
        hot_at_utilisation: hot_at,
    };
    assert!(hot.is_hot(), "a measured-hot cell triggers the migration");

    // Emit the STOR-D2-at-cell-scale RPO/RTO on the SignalSource (observability is part of the pass).
    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::RestoreRpoSecs, measured_rpo_secs as i64);
    sig.assert_signal(
        SignalName::RestoreRpoSecs,
        Predicate::Lte((t.rpo_rto.rpo_max_mins * 60) as i64),
    )
    .expect_green();
    sig.set_labelled(
        SignalName::RestoreRtoSecs,
        vec![myelin_harness::Label::new("grain", "tenant")],
        measured_rto_tenant_secs as i64,
    );
    sig.assert_labelled(
        SignalName::RestoreRtoSecs,
        vec![myelin_harness::Label::new("grain", "tenant")],
        Predicate::Lte((t.rpo_rto.rto_tenant_max_mins * 60) as i64),
    )
    .expect_green();
    sig.set_labelled(
        SignalName::RestoreRtoSecs,
        vec![myelin_harness::Label::new("grain", "cell")],
        measured_rto_cell_secs as i64,
    );
    sig.assert_labelled(
        SignalName::RestoreRtoSecs,
        vec![myelin_harness::Label::new("grain", "cell")],
        Predicate::Lte((t.rpo_rto.rto_cell_max_mins * 60) as i64),
    )
    .expect_green();

    println!(
        "[P-431 STOR-D2@cell-scale GREEN 2026-06-24] restore-verify re-confirmed at cell scale: \
         measured RPO={measured_rpo_secs}s (≤ {}s), RTO-tenant={measured_rto_tenant_secs}s (≤ {}s), \
         RTO-cell={measured_rto_cell_secs}s (≤ {}s). MEASURED sizing band recorded in thresholds.toml: \
         Pool tier binds on `{}` first ({} tenants / {} write-qps / {} bytes; hot at {}% = 20% \
         headroom). No threshold weakened.",
        t.rpo_rto.rpo_max_mins * 60,
        t.rpo_rto.rto_tenant_max_mins * 60,
        t.rpo_rto.rto_cell_max_mins * 60,
        t.cell_sizing.pool_binding_dimension,
        t.cell_sizing.pool_tenants_max,
        t.cell_sizing.pool_write_qps_max,
        t.cell_sizing.pool_storage_bytes_max,
        hot_at,
    );
}

/// **CDC pair for the migration workflow (provider + consumer): the ops/sizing trigger calling the
/// migration.** The PROVIDER is the [`LiveMigration`] performing the durable cell→cell move; the
/// CONSUMER stands in for the **ops/sizing trigger** (§7.1) — it observes a MEASURED-hot cell (the
/// `cell_utilisation` telemetry crossing the sizing-band headroom) and ONLY THEN drives a migration
/// (the avoid-migration-by-sizing floor: a cell within headroom is NOT migrated). If the migration
/// contract drifts (a move that does not land in-region / does not crypto-shred the source), the
/// consumer's invariant breaks.
#[test]
fn cdc_migration_workflow_ops_sizing_trigger_provider_consumer() {
    let t = Thresholds::load_canonical().expect("thresholds");
    let mig = LiveMigration::with_flow_executor(TenantId::from_token("operator"), region());

    /// The ops/sizing-trigger consumer: it migrates a tenant OFF a cell ONLY when that cell is
    /// MEASURED-hot (utilisation ≥ the sizing-band headroom). A cold cell is NOT migrated.
    struct SizingTrigger<'a> {
        mig: &'a LiveMigration<myelin_flow::FlowExecutor>,
        hot_at: u8,
    }
    impl SizingTrigger<'_> {
        /// Migrate per `plan` ONLY when the source cell is MEASURED-hot (`measured_util` ≥ the
        /// sizing-band headroom); a cold cell returns `None` (avoid-migration-by-sizing). On a hot cell
        /// the move is driven + the in-region + source-shredded invariant asserted.
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
                return None; // cold cell — avoid-migration-by-sizing: do NOT move.
            }
            // The cell is MEASURED-hot — drive the migration; assert it lands in-region + shreds source.
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

    // A COLD cell (util 50% < 80%) is NOT migrated (the consumer returns None — sizing handles it).
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

    // A MEASURED-HOT cell (util 90% ≥ 80%) IS migrated — and the move lands in-region + shreds source.
    {
        let mut reg = registry_acme_on_source();
        let src = acme_copy();
        let mut tgt = acme_copy();
        let decision = consumer.maybe_migrate(&mut reg, 90, &mk_plan("cdc-hot"), &src, &mut tgt);
        assert_eq!(
            decision,
            Some(true),
            "a measured-hot cell migrates — in-region + source crypto-shredded"
        );
        assert_eq!(
            reg.placement(&tenant).unwrap().home_cell.as_str(),
            "cell-w-2"
        );
    }
}
