use std::sync::Arc;

use myelin_events::{IdMinter, MonotonicMinter};
use myelin_flow::{DurableExecutor, ExecutorError, FlowExecutor, RunId, StartSpec};
use myelin_storage::{
    restore_to_offset, BlobPresence, ContinuousArchiver, KekId, KmsEngine, KmsError,
    ReindexFromSource, RestoreError, SourceLog, WalRow,
};
use myelin_tenancy::{ArtifactRef, CellId, Region, TenantId};

use crate::registry::{PlacementError, Registry};

pub const WF_LIVE_MIGRATION: &str = "tenancy.live_migration";
pub const WF_DURABLE_PROVISION: &str = "tenancy.durable_provision";
pub const WF_REPO_RELOCATION: &str = "tenancy.repo_relocation";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MigrationTrigger {
    pub hot_cell: CellId,
    pub measured_utilisation: u8,
    pub hot_at_utilisation: u8,
}

impl MigrationTrigger {
    pub fn is_hot(&self) -> bool {
        self.measured_utilisation >= self.hot_at_utilisation
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MigrationError {
    TenantNotPlaced {
        tenant: TenantId,
    },
    CutOverRejected(PlacementError),
    SourceNotHome {
        tenant: TenantId,
        claimed_source: CellId,
        actual_home: CellId,
    },
    TargetRebuildFailed(RestoreError),
    SourceKeyShredFailed(KmsError),
    Executor(ExecutorError),
}

impl std::fmt::Display for MigrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MigrationError::TenantNotPlaced { tenant } => write!(
                f,
                "live migration REJECTED: tenant `{}` is not placed - there is nothing to migrate \
                 (fail-closed).",
                tenant.as_str()
            ),
            MigrationError::CutOverRejected(e) => write!(
                f,
                "live migration REJECTED at cut-over (the residency pin holds across the move - a \
                 migration lands IN-region; there is NO cross-region migration): {e}"
            ),
            MigrationError::SourceNotHome {
                tenant,
                claimed_source,
                actual_home,
            } => write!(
                f,
                "live migration REJECTED: tenant `{}` is homed on `{}`, not the claimed source `{}` \
                 - a migration moves a tenant off its CURRENT home.",
                tenant.as_str(),
                actual_home.as_str(),
                claimed_source.as_str()
            ),
            MigrationError::TargetRebuildFailed(e) => write!(
                f,
                "live migration ABORTED: the target rebuild (reindex-from-source) is NOT whole - the \
                 move is aborted BEFORE any cut-over or source crypto-shred (0 loss: the source is \
                 untouched). Detail: {e}"
            ),
            MigrationError::SourceKeyShredFailed(e) => write!(
                f,
                "live migration INCOMPLETE: cut-over succeeded, but the source key could not be \
                 crypto-shredded. The source must remain quarantined until erasure succeeds. \
                 Detail: {e}"
            ),
            MigrationError::Executor(e) => {
                write!(f, "live migration REJECTED by the durable executor: {e}")
            }
        }
    }
}

impl std::error::Error for MigrationError {}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a migration receipt carries the 0-loss + source-shredded proof - dropping it discards \
              the CP-D7 evidence the move was whole"]
pub struct MigrationReceipt {
    pub tenant: TenantId,
    pub source_cell: CellId,
    pub target_cell: CellId,
    pub region: Region,
    pub run_id: RunId,
    pub rows_migrated: u64,
    pub cross_seam_mismatches: u64,
    pub source_key_destroyed: bool,
}

pub struct CellTenantCopy {
    pub source: SourceLog,
    pub rows: Vec<WalRow>,
    pub blobs: BlobPresence,
    pub archiver: ContinuousArchiver,
    pub kms: KmsEngine,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MigrationPlan {
    pub tenant: TenantId,
    pub source_cell: CellId,
    pub target_cell: CellId,
    pub cut_over_offset: u64,
    pub idem_key: String,
}

pub struct LiveMigration<E: DurableExecutor> {
    executor: E,
    gate: crate::provision::ProvisioningGate,
}

impl LiveMigration<FlowExecutor> {
    pub fn with_flow_executor(
        operator_tenant: TenantId,
        region: Region,
    ) -> LiveMigration<FlowExecutor> {
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let executor = FlowExecutor::new(minter, operator_tenant, region);
        executor.register_definition(WF_LIVE_MIGRATION);
        executor.register_definition(WF_DURABLE_PROVISION);
        executor.register_definition(WF_REPO_RELOCATION);
        LiveMigration {
            executor,
            gate: crate::provision::ProvisioningGate::new(),
        }
    }
}

impl<E: DurableExecutor> LiveMigration<E> {
    pub fn new(executor: E) -> LiveMigration<E> {
        LiveMigration {
            executor,
            gate: crate::provision::ProvisioningGate::new(),
        }
    }

    pub fn executor(&self) -> &E {
        &self.executor
    }

    pub fn migrate_tenant(
        &self,
        registry: &mut Registry,
        plan: &MigrationPlan,
        source: &CellTenantCopy,
        target: &mut CellTenantCopy,
    ) -> Result<MigrationReceipt, MigrationError> {
        let MigrationPlan {
            tenant,
            source_cell,
            target_cell,
            cut_over_offset,
            idem_key,
        } = plan;
        let cut_over_offset = *cut_over_offset;

        let placement =
            registry
                .placement(tenant)
                .ok_or_else(|| MigrationError::TenantNotPlaced {
                    tenant: tenant.clone(),
                })?;
        if &placement.home_cell != source_cell {
            return Err(MigrationError::SourceNotHome {
                tenant: tenant.clone(),
                claimed_source: source_cell.clone(),
                actual_home: placement.home_cell.clone(),
            });
        }

        let run_id = self
            .executor
            .start(StartSpec {
                wf_type: WF_LIVE_MIGRATION.into(),
                input: vec![migration_input_ref(tenant, source_cell, target_cell)],
                budget: None,
                idem_key: idem_key.clone(),
            })
            .map_err(MigrationError::Executor)?;

        let report = restore_to_offset(
            &target.archiver,
            cut_over_offset,
            &source.rows,
            &target.blobs,
            &source.source,
            &target.kms,
        )
        .map_err(MigrationError::TargetRebuildFailed)?;
        target.source = source.source.clone();
        target.rows = report.oltp_rows.clone();
        let derived: ReindexFromSource =
            ReindexFromSource::reindex(&source.source, cut_over_offset);
        let rows_migrated = derived.doc_count() as u64;

        let mut moved = placement.clone();
        moved.home_cell = target_cell.clone();
        moved.member_cells = moved
            .member_cells
            .iter()
            .map(|c| {
                if c == source_cell {
                    target_cell.clone()
                } else {
                    c.clone()
                }
            })
            .collect();
        if !moved.member_cells.contains(target_cell) {
            moved.member_cells.push(target_cell.clone());
        }
        registry
            .place_tenant(moved.clone())
            .map_err(MigrationError::CutOverRejected)?;

        let source_key_destroyed = source
            .kms
            .destroy_kek(&KekId::new(tenant.clone(), placement.region.clone()))
            .map_err(MigrationError::SourceKeyShredFailed)?;

        Ok(MigrationReceipt {
            tenant: tenant.clone(),
            source_cell: source_cell.clone(),
            target_cell: target_cell.clone(),
            region: placement.region.clone(),
            run_id,
            rows_migrated,
            cross_seam_mismatches: report.dangling_ref_count,
            source_key_destroyed,
        })
    }

    pub fn provision_cell_durably<H: myelin_substrate::DependencyHealth>(
        &self,
        registry: &mut Registry,
        cell: &CellId,
        restore_inputs: &myelin_storage::GateInputs<'_>,
        readiness: &myelin_substrate::MetricsHealthSurface<H>,
        signals: &mut crate::provision::ProvisioningSignals,
        idem_key: &str,
    ) -> Result<(RunId, crate::provision::ProvisionVerdict), MigrationError> {
        let run_id = self
            .executor
            .start(StartSpec {
                wf_type: WF_DURABLE_PROVISION.into(),
                input: vec![ArtifactRef(format!(
                    "myelin://control-plane/provision/{}",
                    cell.as_str()
                ))],
                budget: None,
                idem_key: idem_key.into(),
            })
            .map_err(MigrationError::Executor)?;
        let verdict = self
            .gate
            .provision_cell(registry, cell, restore_inputs, readiness, signals);
        Ok((run_id, verdict))
    }

    pub fn relocate_repo_durably(
        &self,
        registry: &mut Registry,
        repo: &ArtifactRef,
        target_cell: CellId,
        target_group: crate::placement_of_repo::StorageGroup,
        idem_key: &str,
    ) -> Result<RunId, MigrationError> {
        let run_id = self
            .executor
            .start(StartSpec {
                wf_type: WF_REPO_RELOCATION.into(),
                input: vec![repo.clone()],
                budget: None,
                idem_key: idem_key.into(),
            })
            .map_err(MigrationError::Executor)?;
        registry
            .relocate_repo(repo, target_cell, target_group)
            .map_err(|e| match e {
                crate::placement_of_repo::RepoPlacementError::Invariant(pe) => {
                    MigrationError::CutOverRejected(pe)
                }
                _ => MigrationError::TenantNotPlaced {
                    tenant: TenantId::from_token(repo.0.clone()),
                },
            })?;
        Ok(run_id)
    }
}

fn migration_input_ref(tenant: &TenantId, source: &CellId, target: &CellId) -> ArtifactRef {
    ArtifactRef(format!(
        "myelin://{}/control-plane/migration/{}→{}",
        tenant.as_str(),
        source.as_str(),
        target.as_str()
    ))
}

pub fn restore_verify_at_cell_scale(
    measured_rpo_secs: u64,
    measured_rto_tenant_secs: u64,
    measured_rto_cell_secs: u64,
    objectives: &myelin_substrate::RpoRto,
) -> bool {
    measured_rpo_secs <= objectives.rpo_max_mins * 60
        && measured_rto_tenant_secs <= objectives.rto_tenant_max_mins * 60
        && measured_rto_cell_secs <= objectives.rto_cell_max_mins * 60
}

pub fn measured_hot_at(sizing: &myelin_substrate::CellSizing) -> u8 {
    let headroom_pct = (sizing.pool_hot_headroom_bps / 100) as u8;
    100u8.saturating_sub(headroom_pct)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{
        Capacity, Cell, CellStatus, IsolationKind, PlacementStatus, TenantPlacement,
    };
    use myelin_storage::{KeyClass, RestoredObject, WalSegment};

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

    fn engine() -> LiveMigration<FlowExecutor> {
        LiveMigration::with_flow_executor(TenantId::from_token("operator"), region())
    }

    fn plan(tenant: &str, source: &str, target: &str, offset: u64, idem: &str) -> MigrationPlan {
        MigrationPlan {
            tenant: TenantId::from_token(tenant),
            source_cell: CellId::from_token(source),
            target_cell: CellId::from_token(target),
            cut_over_offset: offset,
            idem_key: idem.into(),
        }
    }

    #[test]
    fn migrate_tenant_zero_loss_in_region_source_shredded() {
        let mig = engine();
        let mut reg = registry_acme_on_source();
        let tenant = TenantId::from_token("acme");
        let source = acme_copy();
        let mut target = acme_copy();

        let src_dek = source
            .kms
            .ensure_dek(&tenant, &region(), KeyClass::Tenant)
            .unwrap();
        assert!(source.kms.resolve_dek(&src_dek, &region()).is_ok());

        let receipt = mig
            .migrate_tenant(
                &mut reg,
                &plan("acme", "cell-w-1", "cell-w-2", 100, "mig-acme-1"),
                &source,
                &mut target,
            )
            .expect("a same-region cell→cell move completes");

        let placed = reg.placement(&tenant).unwrap();
        assert_eq!(placed.home_cell.as_str(), "cell-w-2", "cut over to target");
        assert_eq!(placed.region.as_str(), "eu-west", "lands IN-region");
        assert!(
            placed
                .member_cells
                .contains(&CellId::from_token("cell-w-2")),
            "member cells re-pointed to the target"
        );
        assert!(
            !placed
                .member_cells
                .contains(&CellId::from_token("cell-w-1")),
            "the source is no longer a member cell"
        );

        assert_eq!(receipt.rows_migrated, 2, "both source rows ≤ 100 migrated");
        assert_eq!(receipt.cross_seam_mismatches, 0, "the target is whole");
        assert_eq!(receipt.region.as_str(), "eu-west");

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
    fn migrate_tenant_appends_target_to_member_cells_when_absent() {
        let mig = engine();
        let mut reg = Registry::new();
        reg.insert_cell(cell("cell-w-1", "eu-west"));
        reg.insert_cell(cell("cell-w-2", "eu-west"));
        reg.insert_cell(cell("cell-w-3", "eu-west"));
        let tenant = TenantId::from_token("acme");
        reg.place_tenant(TenantPlacement {
            tenant_id: tenant.clone(),
            region: region(),
            home_cell: CellId::from_token("cell-w-1"),
            isolation_tier: IsolationKind::Pool,
            slug: "acme".into(),
            status: PlacementStatus::Active,
            member_cells: vec![CellId::from_token("cell-w-3")],
        })
        .unwrap();
        let source = acme_copy();
        let mut target = acme_copy();

        let _receipt = mig
            .migrate_tenant(
                &mut reg,
                &plan("acme", "cell-w-1", "cell-w-2", 100, "mig-append"),
                &source,
                &mut target,
            )
            .expect("the move completes");

        let placed = reg.placement(&tenant).unwrap();
        assert_eq!(
            placed.home_cell.as_str(),
            "cell-w-2",
            "home cut over to the target"
        );
        assert!(
            placed
                .member_cells
                .contains(&CellId::from_token("cell-w-2")),
            "the target is appended to member_cells (the home cell is a member): {:?}",
            placed.member_cells
        );
        assert!(
            placed
                .member_cells
                .contains(&CellId::from_token("cell-w-3")),
            "the pre-existing member cell is preserved"
        );
    }

    #[test]
    fn migrate_tenant_cross_region_target_is_rejected() {
        let mig = engine();
        let mut reg = registry_acme_on_source();
        let tenant = TenantId::from_token("acme");
        let source = acme_copy();
        let mut target = acme_copy();
        let src_dek = source
            .kms
            .ensure_dek(&tenant, &region(), KeyClass::Tenant)
            .unwrap();

        let err = mig
            .migrate_tenant(
                &mut reg,
                &plan("acme", "cell-w-1", "cell-n-1", 100, "mig-acme-xr"),
                &source,
                &mut target,
            )
            .expect_err(
                "a cross-region migration target is rejected (there is NO cross-region move)",
            );
        assert!(
            matches!(
                err,
                MigrationError::CutOverRejected(PlacementError::CrossRegionMemberCell { .. })
            ),
            "the cut-over invariant rejects the cross-region target: {err}"
        );
        assert!(
            err.to_string().contains("IN-region"),
            "loud residency reason: {err}"
        );

        let placed = reg.placement(&tenant).unwrap();
        assert_eq!(placed.home_cell.as_str(), "cell-w-1", "no move on reject");
        assert!(
            source.kms.resolve_dek(&src_dek, &region()).is_ok(),
            "a rejected move does NOT crypto-shred the source (0 loss)"
        );
    }

    #[test]
    fn migrate_tenant_aborts_before_cutover_on_an_unwhole_target() {
        let mig = engine();
        let mut reg = registry_acme_on_source();
        let tenant = TenantId::from_token("acme");
        let source = acme_copy();
        let mut target = acme_copy();
        target.blobs = BlobPresence::new();
        let src_dek = source
            .kms
            .ensure_dek(&tenant, &region(), KeyClass::Tenant)
            .unwrap();

        let err = mig
            .migrate_tenant(
                &mut reg,
                &plan("acme", "cell-w-1", "cell-w-2", 100, "mig-acme-unwhole"),
                &source,
                &mut target,
            )
            .expect_err("an unwhole target aborts the move");
        assert!(
            matches!(err, MigrationError::TargetRebuildFailed(_)),
            "the move aborts on the unwhole target rebuild: {err}"
        );
        assert_eq!(
            reg.placement(&tenant).unwrap().home_cell.as_str(),
            "cell-w-1"
        );
        assert!(
            source.kms.resolve_dek(&src_dek, &region()).is_ok(),
            "an aborted move leaves the source untouched (0 loss)"
        );
    }

    #[test]
    fn migrate_tenant_wrong_source_is_rejected() {
        let mig = engine();
        let mut reg = registry_acme_on_source();
        let source = acme_copy();
        let mut target = acme_copy();
        let err = mig
            .migrate_tenant(
                &mut reg,
                &plan("acme", "cell-w-2", "cell-n-1", 100, "mig-acme-wrongsrc"),
                &source,
                &mut target,
            )
            .expect_err("the claimed source is not the tenant's home");
        assert!(matches!(err, MigrationError::SourceNotHome { .. }), "{err}");
    }

    #[test]
    fn migrate_unplaced_tenant_is_rejected() {
        let mig = engine();
        let mut reg = Registry::new();
        reg.insert_cell(cell("cell-w-1", "eu-west"));
        reg.insert_cell(cell("cell-w-2", "eu-west"));
        let source = acme_copy();
        let mut target = acme_copy();
        let err = mig
            .migrate_tenant(
                &mut reg,
                &plan("ghost", "cell-w-1", "cell-w-2", 100, "mig-ghost"),
                &source,
                &mut target,
            )
            .expect_err("an unplaced tenant has nothing to migrate");
        assert!(
            matches!(err, MigrationError::TenantNotPlaced { .. }),
            "{err}"
        );
    }

    #[test]
    fn migration_reindexes_derived_from_source_not_backup() {
        let mig = engine();
        let mut reg = registry_acme_on_source();
        let source = acme_copy();
        let mut target = acme_copy();
        target.source = SourceLog::new();

        let _receipt = mig
            .migrate_tenant(
                &mut reg,
                &plan("acme", "cell-w-1", "cell-w-2", 100, "mig-reindex"),
                &source,
                &mut target,
            )
            .expect("the move completes");

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
    fn migration_run_is_idempotent_on_idem_key() {
        let mig = engine();
        let mut reg = registry_acme_on_source();
        let tenant = TenantId::from_token("acme");
        let source = acme_copy();
        let mut target = acme_copy();

        let r1 = mig
            .migrate_tenant(
                &mut reg,
                &plan("acme", "cell-w-1", "cell-w-2", 100, "mig-idem"),
                &source,
                &mut target,
            )
            .expect("first move");

        let r2 = mig
            .executor()
            .start(StartSpec {
                wf_type: WF_LIVE_MIGRATION.into(),
                input: vec![migration_input_ref(
                    &tenant,
                    &CellId::from_token("cell-w-1"),
                    &CellId::from_token("cell-w-2"),
                )],
                budget: None,
                idem_key: "mig-idem".into(),
            })
            .expect("re-start under the same idem_key");
        assert_eq!(
            r1.run_id, r2,
            "a redelivered migration trigger is ONE durable run (effectively-once)"
        );
    }

    #[test]
    fn durable_provisioning_remains_gated_on_restore_verify_and_readiness() {
        use myelin_storage::{ErasureLedger, GateInputs};
        use myelin_substrate::{CriticalDependencies, HealthTable, MetricsHealthSurface};

        let mig = engine();
        let mut reg = Registry::new();
        let mut provisioning = cell("cell-w-1", "eu-west");
        provisioning.status = CellStatus::Provisioning;
        reg.insert_cell(provisioning);
        let cell_id = CellId::from_token("cell-w-1");

        let blob = RestoredObject::integral(b"cell-blob".to_vec());
        let objects = vec![blob.clone()];
        let mut whole_source = SourceLog::new();
        whole_source.append(100, "r100");
        let whole_rows = vec![WalRow {
            id: "r100".into(),
            written_at: 100,
            blob_ref: Some(blob.content_address.clone()),
        }];
        let kms = KmsEngine::new();
        kms.ensure_kek(&KekId::new(TenantId::from_token("acme"), region()))
            .expect("seed the in-memory KEK");
        kms.ensure_dek(&TenantId::from_token("acme"), &region(), KeyClass::Tenant)
            .unwrap();
        let ledger = ErasureLedger::new();
        let inputs = GateInputs {
            archiver: &reachable_archiver(300),
            target: 100,
            rows: &whole_rows,
            objects: &objects,
            source: &whole_source,
            kms: &kms,
            erasure_ledger: &ledger,
        };
        let ready = {
            let s = MetricsHealthSurface::new(
                CriticalDependencies::new(["oltp", "blob", "kms"]),
                HealthTable::new(),
            );
            s.mark_started();
            s
        };
        let mut signals = crate::provision::ProvisioningSignals::default();
        let (run_id, verdict) = mig
            .provision_cell_durably(
                &mut reg,
                &cell_id,
                &inputs,
                &ready,
                &mut signals,
                "prov-w-1",
            )
            .expect("the durable provisioning run starts");
        assert!(
            verdict.is_active(),
            "a whole + ready cell ACTIVATES (gate green)"
        );
        assert_eq!(reg.cell(&cell_id).unwrap().status, CellStatus::Active);
        assert!(!run_id.0.is_empty(), "a durable run handle is returned");

        let mut reg2 = Registry::new();
        let mut p2 = cell("cell-w-9", "eu-west");
        p2.status = CellStatus::Provisioning;
        reg2.insert_cell(p2);
        let not_ready = {
            let h = HealthTable::new();
            h.mark_down("kms");
            let s =
                MetricsHealthSurface::new(CriticalDependencies::new(["oltp", "blob", "kms"]), h);
            s.mark_started();
            s
        };
        let mut signals2 = crate::provision::ProvisioningSignals::default();
        let (_run, verdict2) = mig
            .provision_cell_durably(
                &mut reg2,
                &CellId::from_token("cell-w-9"),
                &inputs,
                &not_ready,
                &mut signals2,
                "prov-w-9",
            )
            .expect("the run starts even though the gate will hold the cell");
        assert!(
            !verdict2.is_active(),
            "a not-ready cell stays Provisioning (gate red)"
        );
        assert_eq!(
            reg2.cell(&CellId::from_token("cell-w-9")).unwrap().status,
            CellStatus::Provisioning
        );
    }

    #[test]
    fn durable_repo_relocation_updates_placement_and_redirects() {
        use crate::placement_of_repo::StorageGroup;
        let mig = engine();
        let mut reg = registry_acme_on_source();
        let repo = ArtifactRef("myelin://acme/git/repo/web".into());
        reg.register_repo(&repo, StorageGroup::from_token("pack-0"))
            .expect("repo registered on the home cell");
        assert_eq!(
            reg.placement_of_repo(&repo).unwrap().cell_id.as_str(),
            "cell-w-1"
        );

        let run = mig
            .relocate_repo_durably(
                &mut reg,
                &repo,
                CellId::from_token("cell-w-2"),
                StorageGroup::from_token("pack-7"),
                "reloc-web-1",
            )
            .expect("a same-region durable relocation");
        assert!(!run.0.is_empty());
        assert_eq!(
            reg.placement_of_repo(&repo).unwrap().cell_id.as_str(),
            "cell-w-2",
            "placement_of(repo) flipped to the target (the git wire redirects)"
        );

        let err = mig
            .relocate_repo_durably(
                &mut reg,
                &repo,
                CellId::from_token("cell-n-1"),
                StorageGroup::from_token("g"),
                "reloc-web-xr",
            )
            .expect_err("a cross-region repo relocation is rejected");
        assert!(
            matches!(
                err,
                MigrationError::CutOverRejected(PlacementError::CrossRegionMemberCell { .. })
            ),
            "{err}"
        );
    }

    #[test]
    fn measured_sizing_band_is_read_from_the_thresholds_file() {
        let t = myelin_substrate::Thresholds::load_canonical().expect("thresholds load");
        assert_eq!(
            t.cell_sizing.pool_binding_dimension, "write_qps",
            "MEASURED binding dimension"
        );
        assert!(
            t.cell_sizing.pool_write_qps_max >= 9000,
            "the measured write-QPS ceiling is the binding dimension"
        );
        assert_eq!(
            measured_hot_at(&t.cell_sizing),
            80,
            "hot at 80% (20% headroom)"
        );

        let cold = MigrationTrigger {
            hot_cell: CellId::from_token("cell-w-1"),
            measured_utilisation: 70,
            hot_at_utilisation: measured_hot_at(&t.cell_sizing),
        };
        assert!(
            !cold.is_hot(),
            "a cell below the headroom is NOT migrated (sizing handles it)"
        );
        let hot = MigrationTrigger {
            hot_cell: CellId::from_token("cell-w-1"),
            measured_utilisation: 85,
            hot_at_utilisation: measured_hot_at(&t.cell_sizing),
        };
        assert!(hot.is_hot(), "a measured-hot cell triggers the migration");
    }

    #[test]
    fn restore_verify_at_cell_scale_meets_rpo_rto() {
        let t = myelin_substrate::Thresholds::load_canonical().expect("thresholds load");
        assert!(
            restore_verify_at_cell_scale(180, 1800, 7200, &t.rpo_rto),
            "RPO 3 min / RTO 30 min-tenant / 2h-cell meet the objectives"
        );
        assert!(
            !restore_verify_at_cell_scale(600, 1800, 7200, &t.rpo_rto),
            "a 10-min RPO exceeds the ≤ 5-min objective - FAILS (no lowered bar)"
        );
    }
}
