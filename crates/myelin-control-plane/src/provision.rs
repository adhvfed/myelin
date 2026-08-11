use myelin_storage::{GateInputs, GateVerdict, GreenArtifact, KekId, KmsEngine, RestoreVerifyGate};
use myelin_substrate::{DependencyHealth, MetricsHealthSurface, ReadinessReport};
use myelin_tenancy::{CellId, Region, TenantId};

use crate::registry::Registry;
use crate::schema::{CellProvisioning, CellStatus, PlacementStatus, ProvisioningOutcome};

pub const STEP_RESTORE_VERIFY: &str = "restore_verify";
pub const STEP_READINESS: &str = "readiness_probe";
pub const STEP_ACTIVATE: &str = "activate";

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a provisioning verdict must be checked - a dropped RED would silently leave a cell \
              `Provisioning`; the place path never routes to it, but the verdict names WHY (CP-D6)"]
pub enum ProvisionVerdict {
    Activated {
        cell: CellId,
        restore_verify: GreenArtifact,
    },
    StayedProvisioning {
        cell: CellId,
        failure: ProvisionFailure,
    },
}

impl ProvisionVerdict {
    pub fn is_active(&self) -> bool {
        matches!(self, ProvisionVerdict::Activated { .. })
    }

    pub fn green_artifact(&self) -> Option<&GreenArtifact> {
        match self {
            ProvisionVerdict::Activated { restore_verify, .. } => Some(restore_verify),
            ProvisionVerdict::StayedProvisioning { .. } => None,
        }
    }

    pub fn failure(&self) -> Option<&ProvisionFailure> {
        match self {
            ProvisionVerdict::StayedProvisioning { failure, .. } => Some(failure),
            ProvisionVerdict::Activated { .. } => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProvisionFailure {
    RestoreVerifyFailed {
        detail: String,
    },
    NotReady {
        down_dependencies: Vec<String>,
    },
    UnknownCell {
        cell: CellId,
    },
}

impl std::fmt::Display for ProvisionFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProvisionFailure::RestoreVerifyFailed { detail } => write!(
                f,
                "CP-D6 GATE: restore-verify FAILED - the cell's backup is NOT whole (Storage 11.5, \
                 the permanent STOR-D1 gate is RED). The cell stays `Provisioning`; the \
                 place-real-data capability does NOT come online over a red restore-verify (master §1 \
                 Tier 1). Detail: {detail}"
            ),
            ProvisionFailure::NotReady { down_dependencies } => write!(
                f,
                "CP-D6 GATE: readiness FAILED - the cell cannot serve correct traffic (critical \
                 dependencies down: {down_dependencies:?}). The cell stays `Provisioning` (no \
                 traffic until it is ready)."
            ),
            ProvisionFailure::UnknownCell { cell } => write!(
                f,
                "CP-D6 GATE: cell `{}` is not registered - provisioning is refused fail-closed (no \
                 inventory row to gate).",
                cell.as_str()
            ),
        }
    }
}

impl std::error::Error for ProvisionFailure {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ProvisioningSignals {
    pub cells_activated: u64,
    pub cells_held_provisioning: u64,
    pub tenants_decommissioned: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ProvisioningGate {
    gate: RestoreVerifyGate,
}

impl ProvisioningGate {
    pub fn new() -> ProvisioningGate {
        ProvisioningGate {
            gate: RestoreVerifyGate::new(),
        }
    }

    pub fn provision_cell<H: DependencyHealth>(
        &self,
        registry: &mut Registry,
        cell: &CellId,
        restore_inputs: &GateInputs<'_>,
        readiness: &MetricsHealthSurface<H>,
        signals: &mut ProvisioningSignals,
    ) -> ProvisionVerdict {
        if registry.cell(cell).is_none() {
            return ProvisionVerdict::StayedProvisioning {
                cell: cell.clone(),
                failure: ProvisionFailure::UnknownCell { cell: cell.clone() },
            };
        }

        let restore_verify = match self.gate.run(restore_inputs) {
            GateVerdict::Green(artifact) => {
                registry.log_provisioning(CellProvisioning {
                    cell_id: cell.clone(),
                    step: STEP_RESTORE_VERIFY.into(),
                    outcome: ProvisioningOutcome::Passed,
                });
                artifact
            }
            GateVerdict::Red(failure) => {
                registry.log_provisioning(CellProvisioning {
                    cell_id: cell.clone(),
                    step: STEP_RESTORE_VERIFY.into(),
                    outcome: ProvisioningOutcome::Failed,
                });
                signals.cells_held_provisioning += 1;
                return ProvisionVerdict::StayedProvisioning {
                    cell: cell.clone(),
                    failure: ProvisionFailure::RestoreVerifyFailed {
                        detail: failure.to_string(),
                    },
                };
            }
        };

        let report: ReadinessReport = readiness.readiness();
        if !report.is_ready() {
            registry.log_provisioning(CellProvisioning {
                cell_id: cell.clone(),
                step: STEP_READINESS.into(),
                outcome: ProvisioningOutcome::Failed,
            });
            signals.cells_held_provisioning += 1;
            return ProvisionVerdict::StayedProvisioning {
                cell: cell.clone(),
                failure: ProvisionFailure::NotReady {
                    down_dependencies: report.down_critical.iter().map(|d| d.0.clone()).collect(),
                },
            };
        }
        registry.log_provisioning(CellProvisioning {
            cell_id: cell.clone(),
            step: STEP_READINESS.into(),
            outcome: ProvisioningOutcome::Passed,
        });

        registry.activate_cell(cell);
        registry.log_provisioning(CellProvisioning {
            cell_id: cell.clone(),
            step: STEP_ACTIVATE.into(),
            outcome: ProvisioningOutcome::Passed,
        });
        signals.cells_activated += 1;

        ProvisionVerdict::Activated {
            cell: cell.clone(),
            restore_verify,
        }
    }

    pub fn decommission_tenant(
        &self,
        registry: &mut Registry,
        kms: &KmsEngine,
        tenant: &TenantId,
        region: &Region,
        signals: &mut ProvisioningSignals,
    ) -> bool {
        let shredded = kms.destroy_kek(&KekId::new(tenant.clone(), region.clone()));
        if shredded {
            signals.tenants_decommissioned += 1;
            registry.set_placement_status(tenant, PlacementStatus::Offboarding);
        }
        shredded
    }

    pub fn tenants_on_unverified_cells(registry: &Registry) -> usize {
        registry
            .placements_iter()
            .filter(|p| {
                registry
                    .cell(&p.home_cell)
                    .is_none_or(|c| c.status != CellStatus::Active)
            })
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::place::{CounterMinter, PlacementService};
    use crate::schema::{Capacity, Cell, IsolationKind, TenantPlacement};
    use myelin_storage::{
        ContentHash, ContinuousArchiver, ErasureLedger, KeyClass, RestoredObject, SourceLog,
        WalRow, WalSegment,
    };
    use myelin_substrate::{CriticalDependencies, HealthTable, MetricsHealthSurface};

    fn region() -> Region {
        Region::new("eu-west")
    }

    fn provisioning_cell(id: &str) -> Cell {
        Cell {
            cell_id: CellId::from_token(id),
            region: region(),
            status: CellStatus::Provisioning,
            isolation_kind: IsolationKind::Pool,
            capacity: Capacity {
                tenants_max: 1000,
                write_qps_max: 5000,
                storage_bytes_max: 1 << 40,
            },
            utilisation: 0,
            version: 1,
            endpoint: format!("cell.eu-west.{id}.myelin.eu"),
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

    fn live_kms() -> KmsEngine {
        let kms = KmsEngine::new();
        kms.ensure_kek(&KekId::new(TenantId::from_token("acme"), region()))
            .expect("seed the in-memory KEK");
        kms.ensure_dek(&TenantId::from_token("acme"), &region(), KeyClass::Tenant)
            .unwrap();
        kms
    }

    struct WholeRestore {
        arch: ContinuousArchiver,
        rows: Vec<WalRow>,
        objects: Vec<RestoredObject>,
        source: SourceLog,
        kms: KmsEngine,
        ledger: ErasureLedger,
    }

    impl WholeRestore {
        fn new() -> WholeRestore {
            let objects = vec![RestoredObject::integral(b"cell-blob".to_vec())];
            let mut source = SourceLog::new();
            source.append(100, "r100");
            let rows = vec![WalRow {
                id: "r100".into(),
                written_at: 100,
                blob_ref: Some(objects[0].content_address.clone()),
            }];
            WholeRestore {
                arch: reachable_archiver(300),
                rows,
                objects,
                source,
                kms: live_kms(),
                ledger: ErasureLedger::new(),
            }
        }

        fn inputs(&self) -> GateInputs<'_> {
            GateInputs {
                archiver: &self.arch,
                target: 100,
                rows: &self.rows,
                objects: &self.objects,
                source: &self.source,
                kms: &self.kms,
                erasure_ledger: &self.ledger,
            }
        }
    }

    struct CorruptRestore {
        arch: ContinuousArchiver,
        rows: Vec<WalRow>,
        objects: Vec<RestoredObject>,
        source: SourceLog,
        kms: KmsEngine,
        ledger: ErasureLedger,
    }

    impl CorruptRestore {
        fn new() -> CorruptRestore {
            let missing = ContentHash::blake3(b"never-restored");
            CorruptRestore {
                arch: reachable_archiver(300),
                rows: vec![WalRow {
                    id: "corrupt".into(),
                    written_at: 90,
                    blob_ref: Some(missing),
                }],
                objects: vec![],
                source: SourceLog::new(),
                kms: live_kms(),
                ledger: ErasureLedger::new(),
            }
        }

        fn inputs(&self) -> GateInputs<'_> {
            GateInputs {
                archiver: &self.arch,
                target: 100,
                rows: &self.rows,
                objects: &self.objects,
                source: &self.source,
                kms: &self.kms,
                erasure_ledger: &self.ledger,
            }
        }
    }

    fn ready_surface() -> MetricsHealthSurface<HealthTable> {
        let critical = CriticalDependencies::new(["oltp", "blob", "kms"]);
        let health = HealthTable::new();
        let surface = MetricsHealthSurface::new(critical, health);
        surface.mark_started();
        surface
    }

    fn not_ready_surface() -> MetricsHealthSurface<HealthTable> {
        let critical = CriticalDependencies::new(["oltp", "blob", "kms"]);
        let health = HealthTable::new();
        health.mark_down("kms");
        let surface = MetricsHealthSurface::new(critical, health);
        surface.mark_started();
        surface
    }

    fn registry_with_cell(cell: Cell) -> Registry {
        let mut reg = Registry::new();
        reg.insert_cell(cell);
        reg
    }

    #[test]
    fn cell_goes_active_only_when_restore_verify_and_readiness_pass() {
        let mut reg = registry_with_cell(provisioning_cell("cell-w-1"));
        let cell = CellId::from_token("cell-w-1");
        assert_eq!(reg.cell(&cell).unwrap().status, CellStatus::Provisioning);

        let restore = WholeRestore::new();
        let readiness = ready_surface();
        let mut signals = ProvisioningSignals::default();

        let verdict = ProvisioningGate::new().provision_cell(
            &mut reg,
            &cell,
            &restore.inputs(),
            &readiness,
            &mut signals,
        );

        assert!(
            verdict.is_active(),
            "both gating steps green ⇒ Active: {verdict:?}"
        );
        assert!(
            verdict.green_artifact().is_some(),
            "the restore-verify green artifact is carried"
        );
        assert_eq!(reg.cell(&cell).unwrap().status, CellStatus::Active);
        assert_eq!(signals.cells_activated, 1);
        assert_eq!(signals.cells_held_provisioning, 0);
        let steps: Vec<String> = reg
            .provisioning_log()
            .iter()
            .map(|e| e.step.clone())
            .collect();
        assert_eq!(
            steps,
            vec![STEP_RESTORE_VERIFY, STEP_READINESS, STEP_ACTIVATE]
        );
        assert!(reg
            .provisioning_log()
            .iter()
            .all(|e| e.outcome == ProvisioningOutcome::Passed));
    }

    #[test]
    fn failing_restore_verify_keeps_the_cell_provisioning() {
        let mut reg = registry_with_cell(provisioning_cell("cell-w-1"));
        let cell = CellId::from_token("cell-w-1");
        let restore = CorruptRestore::new();
        let readiness = ready_surface();
        let mut signals = ProvisioningSignals::default();

        let verdict = ProvisioningGate::new().provision_cell(
            &mut reg,
            &cell,
            &restore.inputs(),
            &readiness,
            &mut signals,
        );

        assert!(
            !verdict.is_active(),
            "a red restore-verify must NOT activate the cell"
        );
        assert!(
            matches!(
                verdict.failure(),
                Some(ProvisionFailure::RestoreVerifyFailed { .. })
            ),
            "the failure names restore-verify: {verdict:?}"
        );
        let rendered = verdict.failure().unwrap().to_string();
        assert!(
            rendered.contains("restore-verify FAILED"),
            "loud restore-verify reason: {rendered}"
        );
        assert!(
            rendered.contains("master §1 Tier 1"),
            "names the silent-data-loss floor: {rendered}"
        );
        assert_eq!(reg.cell(&cell).unwrap().status, CellStatus::Provisioning);
        assert_eq!(signals.cells_activated, 0);
        assert_eq!(signals.cells_held_provisioning, 1);
        let log = reg.provisioning_log();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].step, STEP_RESTORE_VERIFY);
        assert_eq!(log[0].outcome, ProvisioningOutcome::Failed);
    }

    #[test]
    fn not_ready_keeps_the_cell_provisioning() {
        let mut reg = registry_with_cell(provisioning_cell("cell-w-1"));
        let cell = CellId::from_token("cell-w-1");
        let restore = WholeRestore::new();
        let readiness = not_ready_surface();
        let mut signals = ProvisioningSignals::default();

        let verdict = ProvisioningGate::new().provision_cell(
            &mut reg,
            &cell,
            &restore.inputs(),
            &readiness,
            &mut signals,
        );

        assert!(!verdict.is_active(), "a not-ready cell must NOT activate");
        match verdict.failure() {
            Some(ProvisionFailure::NotReady { down_dependencies }) => {
                assert!(
                    down_dependencies.contains(&"kms".to_string()),
                    "names the down dep"
                );
            }
            other => panic!("expected NotReady, got {other:?}"),
        }
        let rendered = verdict.failure().unwrap().to_string();
        assert!(
            rendered.contains("readiness FAILED"),
            "loud readiness reason: {rendered}"
        );
        assert!(
            rendered.contains("kms"),
            "names the down dependency: {rendered}"
        );
        assert_eq!(reg.cell(&cell).unwrap().status, CellStatus::Provisioning);
        assert_eq!(signals.cells_held_provisioning, 1);
        let log = reg.provisioning_log();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].outcome, ProvisioningOutcome::Passed);
        assert_eq!(log[1].step, STEP_READINESS);
        assert_eq!(log[1].outcome, ProvisioningOutcome::Failed);
    }

    #[test]
    fn zero_tenants_placed_on_an_unverified_cell() {
        let mut reg = registry_with_cell(provisioning_cell("cell-w-1"));
        let cell = CellId::from_token("cell-w-1");
        let mut signals = ProvisioningSignals::default();

        let restore = CorruptRestore::new();
        let readiness = ready_surface();
        let verdict = ProvisioningGate::new().provision_cell(
            &mut reg,
            &cell,
            &restore.inputs(),
            &readiness,
            &mut signals,
        );
        assert!(!verdict.is_active());

        let placer = PlacementService::new(CounterMinter::new());
        let err = placer
            .place(&mut reg, &region(), IsolationKind::Pool, "acme")
            .expect_err("no Active cell ⇒ place refuses; it never routes to an unverified cell");
        assert!(
            err.to_string().contains("no Active cell"),
            "loud refusal: {err}"
        );

        assert_eq!(reg.placement_count(), 0);
        assert_eq!(ProvisioningGate::tenants_on_unverified_cells(&reg), 0);
    }

    #[test]
    fn place_routes_to_a_verified_cell_after_provisioning() {
        let mut reg = registry_with_cell(provisioning_cell("cell-w-1"));
        let cell = CellId::from_token("cell-w-1");
        let mut signals = ProvisioningSignals::default();

        let restore = WholeRestore::new();
        let readiness = ready_surface();
        assert!(ProvisioningGate::new()
            .provision_cell(&mut reg, &cell, &restore.inputs(), &readiness, &mut signals)
            .is_active());

        let placer = PlacementService::new(CounterMinter::new());
        let answer = placer
            .place(&mut reg, &region(), IsolationKind::Pool, "acme")
            .expect("an Active cell accepts the placement");
        assert_eq!(answer.home_cell.as_str(), "cell-w-1");
        assert_eq!(ProvisioningGate::tenants_on_unverified_cells(&reg), 0);
    }

    #[test]
    fn tenants_on_unverified_cells_counts_a_real_violation() {
        let mut reg = registry_with_cell({
            let mut c = provisioning_cell("cell-w-1");
            c.status = CellStatus::Active;
            c
        });
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
        assert_eq!(ProvisioningGate::tenants_on_unverified_cells(&reg), 0);

        assert!(reg.set_cell_status(&CellId::from_token("cell-w-1"), CellStatus::Provisioning));
        assert_eq!(
            ProvisioningGate::tenants_on_unverified_cells(&reg),
            1,
            "a tenant on a non-Active cell is counted - the CP-D6 zero is real, not vacuous"
        );

        assert!(reg.activate_cell(&CellId::from_token("cell-w-1")));
        assert_eq!(ProvisioningGate::tenants_on_unverified_cells(&reg), 0);
    }

    #[test]
    fn unknown_cell_is_refused_fail_closed() {
        let mut reg = Registry::new();
        let cell = CellId::from_token("cell-ghost");
        let restore = WholeRestore::new();
        let readiness = ready_surface();
        let mut signals = ProvisioningSignals::default();

        let verdict = ProvisioningGate::new().provision_cell(
            &mut reg,
            &cell,
            &restore.inputs(),
            &readiness,
            &mut signals,
        );
        assert!(matches!(
            verdict.failure(),
            Some(ProvisionFailure::UnknownCell { .. })
        ));
        assert!(!verdict.is_active());
        let rendered = verdict.failure().unwrap().to_string();
        assert!(
            rendered.contains("not registered"),
            "loud unknown-cell reason: {rendered}"
        );
        assert!(
            rendered.contains("cell-ghost"),
            "names the unknown cell: {rendered}"
        );
    }

    #[test]
    fn decommission_crypto_shreds_the_tenant_kek() {
        let mut reg = registry_with_cell({
            let mut c = provisioning_cell("cell-w-1");
            c.status = CellStatus::Active;
            c
        });
        let tenant = TenantId::from_token("acme");
        reg.place_tenant(TenantPlacement {
            tenant_id: tenant.clone(),
            region: region(),
            home_cell: CellId::from_token("cell-w-1"),
            isolation_tier: IsolationKind::Pool,
            slug: "acme".into(),
            status: PlacementStatus::Active,
            member_cells: vec![CellId::from_token("cell-w-1")],
        })
        .unwrap();

        let kms = KmsEngine::new();
        kms.ensure_kek(&KekId::new(tenant.clone(), region()))
            .expect("seed the in-memory KEK");
        let key_ref = kms
            .ensure_dek(&tenant, &region(), KeyClass::Tenant)
            .unwrap();
        assert!(
            kms.resolve_dek(&key_ref, &region()).is_ok(),
            "the DEK resolves while live"
        );

        let mut signals = ProvisioningSignals::default();
        let gate = ProvisioningGate::new();
        let shredded = gate.decommission_tenant(&mut reg, &kms, &tenant, &region(), &mut signals);
        assert!(shredded, "a KEK was present to destroy");
        assert_eq!(signals.tenants_decommissioned, 1);

        assert!(
            kms.resolve_dek(&key_ref, &region()).is_err(),
            "after decommission the DEK is unrecoverable (the KEK was crypto-shredded)"
        );
        assert_eq!(
            reg.placement(&tenant).unwrap().status,
            PlacementStatus::Offboarding
        );

        let again = gate.decommission_tenant(&mut reg, &kms, &tenant, &region(), &mut signals);
        assert!(
            !again,
            "a second decommission destroys nothing (idempotent)"
        );
        assert_eq!(
            signals.tenants_decommissioned, 1,
            "the count does not double-increment"
        );
    }

    #[test]
    fn cdc_provisioning_gate_provider_consumer() {
        struct SizingCaller<'a> {
            placer: &'a PlacementService,
        }
        impl SizingCaller<'_> {
            fn place_if_verified(&self, reg: &mut Registry, region: &Region) -> Result<(), String> {
                self.placer
                    .place(reg, region, IsolationKind::Pool, "acme")
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            }
        }

        let mut reg = registry_with_cell(provisioning_cell("cell-w-1"));
        let cell = CellId::from_token("cell-w-1");
        let placer = PlacementService::new(CounterMinter::new());
        let caller = SizingCaller { placer: &placer };

        assert!(
            caller.place_if_verified(&mut reg, &region()).is_err(),
            "a Provisioning (un-gated) cell takes no tenants"
        );
        assert_eq!(ProvisioningGate::tenants_on_unverified_cells(&reg), 0);

        let restore = WholeRestore::new();
        let readiness = ready_surface();
        let mut signals = ProvisioningSignals::default();
        assert!(ProvisioningGate::new()
            .provision_cell(&mut reg, &cell, &restore.inputs(), &readiness, &mut signals)
            .is_active());

        caller
            .place_if_verified(&mut reg, &region())
            .expect("the gated cell accepts the tenant");
        assert_eq!(reg.placement_count(), 1);
        assert_eq!(ProvisioningGate::tenants_on_unverified_cells(&reg), 0);
    }
}
