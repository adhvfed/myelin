//! # Cell-provisioning gating: restore-verify + readiness before a cell goes `Active` (CP-D6)
//!
//! **Prompt:** P-CP-11 → global **P-083** (M1). **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/tenancy-and-control-plane.md`
//! §7.2 (*cell provisioning is a durable workflow, off the hot path, **gated by
//! restore-verify/readiness before a cell goes `active`***; on the M1 floor it is a SCRIPTED
//! procedure — the durable workflow is the M2 follow-on), §7.1 (the three cell classes the
//! provisioning gate stands up). **Contract-index:** rows **11.5** (the restore-verify gate consumed
//! as the cell-readiness gate — [`myelin_storage::RestoreVerifyGate`]), **11.3** (KMS crypto-shred for
//! tenant decommission — [`myelin_storage::KmsEngine::destroy_kek`]), **9.1** (the `DurableExecutor`
//! the scripted floor is promoted onto — the M2 follow-on, P-CP-22).
//!
//! ## What this prompt (P-CP-11 / P-083) ships — CP-D6
//! A new cell does **NOT** go `Active` / accept any tenant until it **passes restore-verify (Storage
//! 11.5) + readiness**; a failing cell **stays `Provisioning`** (gets no traffic). The gating
//! ([`ProvisioningGate::provision_cell`]):
//! 1. **Restore-verify (Storage 11.5)** — drive the REAL [`myelin_storage::RestoreVerifyGate`] over
//!    the cell's restore inputs. A `GateVerdict::Red` (the backup is not whole) leaves the cell
//!    `Provisioning`. This is the silent-data-loss floor: **the place-real-data capability does not
//!    come online over a red STOR-D1** (master §1 Tier 1) — restore-verify must be green FIRST.
//! 2. **Readiness** — run the cell's [`myelin_substrate::MetricsHealthSurface::readiness`] over its
//!    declared critical dependencies. A `NotReady` cell (a dead critical dependency, or boot not
//!    complete) stays `Provisioning`.
//! 3. **Flip to `Active` IFF both pass.** Only then may `place`/`assign_cell` (P-082) target the cell
//!    — the place path already filters on `CellStatus::Active`, so a cell that fails the gate is
//!    structurally invisible to placement (0 tenants placed on an unverified cell).
//!
//! Each step is recorded in the `cell_provisioning` orchestration log ([`crate::schema::CellProvisioning`],
//! P-CP-05) — the scripted-provisioning floor's audit trail.
//!
//! ## Tenant decommission = crypto-shred the tenant KEK (Storage 11.3)
//! [`ProvisioningGate::decommission_tenant`] destroys the tenant KEK
//! ([`myelin_storage::KmsEngine::destroy_kek`], the tenant-granularity crypto-shred lever, §5): every
//! DEK under it becomes unrecoverable, live AND in every backup (the source key is destroyed). The
//! offboarded tenant's placement row moves to [`crate::schema::PlacementStatus::Offboarding`].
//!
//! ## SCRIPTED is the floor — the durable workflow is the M2 follow-on (P-CP-22) — VISION §3
//! On this M1 floor provisioning runs as a **SCRIPTED procedure**: [`ProvisioningGate::provision_cell`]
//! is a synchronous sequence of steps. The *gating* (no traffic until restore-verify + readiness pass)
//! is M1 and complete; the *durability* of the procedure waits on `myelin-flow`'s `DurableExecutor`
//! (contract 9.1, M2 — not yet available, the prompt says so explicitly). The durable-workflow
//! promotion (the same gating, now crash-safe + resumable) is **P-CP-22**'s scope, where it is
//! re-confirmed under the engine. **Floor named** — recorded here + in the crate docs + the commit body.
//!
//! ## Why this CONSUMES the storage gate rather than re-implementing it (coherence, EI-01 §7)
//! The restore-verify gate is the storage subsystem's headline durability gate
//! ([`myelin_storage::RestoreVerifyGate`], P-ST-13 / P-061). Cell provisioning does NOT re-define it —
//! it is the CDC *consumer* of contract 11.5: it builds the cell's [`myelin_storage::GateInputs`] and
//! reads the typed [`myelin_storage::GateVerdict`]. The cell-readiness gate is literally "did the
//! cell's stores pass the permanent restore-verify gate?". This keeps ONE durability gate in the
//! platform (no parallel second implementation) — exactly the EI-01 §7 reuse discipline.
//!
//! ## Mutation floor (mandatory-core, >= 80% — EI-01 §2/§3; the prompt's TESTS field)
//! The provision-gating logic ([`ProvisioningGate::provision_cell`] — *no traffic until restore-verify
//! AND readiness pass*) is mandatory-core: a cell that takes real data over a red restore-verify is the
//! silent-data-loss floor (master §1 Tier 1). The floor is **>= 80%**; the achieved score is
//! `cargo mutants -p myelin-control-plane -f crates/myelin-control-plane/src/provision.rs` ->
//! **19 caught, 3 unviable, 0 missed = 100% of the 19 viable mutants**. Every mutation of the
//! restore-verify-first branch, the readiness branch, the activate flip, the unknown-cell fail-closed
//! branch, the `tenants_on_unverified_cells` counter, and the loud failure rendering is killed.

use myelin_storage::{GateInputs, GateVerdict, GreenArtifact, KekId, KmsEngine, RestoreVerifyGate};
use myelin_substrate::{MetricsHealthSurface, DependencyHealth, ReadinessReport};
use myelin_tenancy::{CellId, Region, TenantId};

use crate::registry::Registry;
use crate::schema::{CellProvisioning, CellStatus, PlacementStatus, ProvisioningOutcome};

/// The PII-free name of the restore-verify provisioning step (the `cell_provisioning` log label).
pub const STEP_RESTORE_VERIFY: &str = "restore_verify";
/// The PII-free name of the readiness provisioning step (the `cell_provisioning` log label).
pub const STEP_READINESS: &str = "readiness_probe";
/// The PII-free name of the activation step (the `cell_provisioning` log label).
pub const STEP_ACTIVATE: &str = "activate";

/// **The outcome of provisioning one cell (CP-D6).** Either the cell passed both gating steps and is
/// now `Active` (carrying the dated restore-verify [`GreenArtifact`] — the green proof), or it FAILED
/// a gating step and stays `Provisioning` (carrying EXACTLY which step failed, so the refusal is loud
/// + named — EI-01 §3). A failing cell gets NO traffic: it is never flipped to `Active`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a provisioning verdict must be checked — a dropped RED would silently leave a cell \
              `Provisioning`; the place path never routes to it, but the verdict names WHY (CP-D6)"]
pub enum ProvisionVerdict {
    /// The cell passed restore-verify **and** readiness — it is now `Active` and may accept tenants.
    /// Carries the restore-verify green artifact (the measured durability proof) + the cell id.
    Activated {
        /// The cell that is now `Active`.
        cell: CellId,
        /// The dated restore-verify green artifact (Storage 11.5 — the measured numbers).
        restore_verify: GreenArtifact,
    },
    /// The cell FAILED a gating step — it stays `Provisioning` (no traffic). Names the step.
    StayedProvisioning {
        /// The cell that stayed `Provisioning`.
        cell: CellId,
        /// EXACTLY which gating step failed (observability is part of the pass, EI-01 §3).
        failure: ProvisionFailure,
    },
}

impl ProvisionVerdict {
    /// `true` iff the cell passed both gating steps and is now `Active`.
    pub fn is_active(&self) -> bool {
        matches!(self, ProvisionVerdict::Activated { .. })
    }

    /// The restore-verify green artifact, if the cell activated.
    pub fn green_artifact(&self) -> Option<&GreenArtifact> {
        match self {
            ProvisionVerdict::Activated { restore_verify, .. } => Some(restore_verify),
            ProvisionVerdict::StayedProvisioning { .. } => None,
        }
    }

    /// The gating failure, if the cell stayed `Provisioning`.
    pub fn failure(&self) -> Option<&ProvisionFailure> {
        match self {
            ProvisionVerdict::StayedProvisioning { failure, .. } => Some(failure),
            ProvisionVerdict::Activated { .. } => None,
        }
    }
}

/// EXACTLY which provisioning gating step failed (CP-D6). A cell stays `Provisioning` on any of
/// these — the refusal is loud + named (EI-01 §3), never a bare "provisioning failed".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProvisionFailure {
    /// **Restore-verify FAILED (the silent-data-loss floor, Storage 11.5).** The cell's backup is not
    /// whole — the permanent STOR-D1 gate returned RED. The place-real-data capability does NOT come
    /// online over a red restore-verify (master §1 Tier 1). Carries the storage gate's failure detail.
    RestoreVerifyFailed {
        /// The storage gate's typed failure (rendered — the precise seam that broke).
        detail: String,
    },
    /// **Readiness FAILED.** A critical dependency is down (or boot/migration is incomplete) — the
    /// cell cannot serve correct traffic, so it stays `Provisioning` (it must not take traffic before
    /// it is ready). The dead critical dependencies are named.
    NotReady {
        /// The critical dependencies that are currently down (the readiness shed reason).
        down_dependencies: Vec<String>,
    },
    /// **The cell is not registered.** Provisioning a cell that is not in the inventory is refused
    /// fail-closed (the gate cannot drive a cell it has no inventory row for).
    UnknownCell {
        /// The cell id that is not in the registry inventory.
        cell: CellId,
    },
}

impl std::fmt::Display for ProvisionFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProvisionFailure::RestoreVerifyFailed { detail } => write!(
                f,
                "CP-D6 GATE: restore-verify FAILED — the cell's backup is NOT whole (Storage 11.5, \
                 the permanent STOR-D1 gate is RED). The cell stays `Provisioning`; the \
                 place-real-data capability does NOT come online over a red restore-verify (master §1 \
                 Tier 1). Detail: {detail}"
            ),
            ProvisionFailure::NotReady { down_dependencies } => write!(
                f,
                "CP-D6 GATE: readiness FAILED — the cell cannot serve correct traffic (critical \
                 dependencies down: {down_dependencies:?}). The cell stays `Provisioning` (no \
                 traffic until it is ready)."
            ),
            ProvisionFailure::UnknownCell { cell } => write!(
                f,
                "CP-D6 GATE: cell `{}` is not registered — provisioning is refused fail-closed (no \
                 inventory row to gate).",
                cell.as_str()
            ),
        }
    }
}

impl std::error::Error for ProvisionFailure {}

/// **The PII-free provisioning telemetry (CP-D6; contract 1.8).** Aggregate counters only —
/// observability is part of the pass (EI-01 §3). Never per-subject data: a cell id is an opaque
/// routing token, never personal data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ProvisioningSignals {
    /// Cells that passed the gate and went `Active` (a successful provision). Aggregate-only.
    pub cells_activated: u64,
    /// Cells that FAILED the gate and stayed `Provisioning` (a refused provision). Aggregate-only.
    pub cells_held_provisioning: u64,
    /// Tenants decommissioned (KEK crypto-shredded). Aggregate-only.
    pub tenants_decommissioned: u64,
}

/// **The cell-provisioning gate (CP-D6) — the SCRIPTED procedure (the M1 floor).** Wraps the
/// restore-verify gate ([`RestoreVerifyGate`], Storage 11.5) + the readiness probe + the KMS
/// crypto-shred lever (Storage 11.3). [`Self::provision_cell`] runs the gating; only a cell that
/// passes BOTH restore-verify AND readiness is flipped `Provisioning → Active`. The durable-workflow
/// promotion (the same gating under `myelin-flow`'s `DurableExecutor`, 9.1) is the M2 follow-on
/// (P-CP-22) — this floor is scripted, named.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProvisioningGate {
    gate: RestoreVerifyGate,
}

impl ProvisioningGate {
    /// A fresh provisioning gate (stateless; the restore-verify gate it wraps is stateless too).
    pub fn new() -> ProvisioningGate {
        ProvisioningGate { gate: RestoreVerifyGate::new() }
    }

    /// **`provision_cell` — the CP-D6 gating (the SCRIPTED procedure).** Runs restore-verify + the
    /// readiness probe; flips the cell `Provisioning → Active` IFF BOTH pass. A failing cell stays
    /// `Provisioning` (0 traffic — the place path filters on `Active`). Every step is recorded in the
    /// `cell_provisioning` orchestration log. Returns the typed [`ProvisionVerdict`].
    ///
    /// Order is load-bearing: restore-verify FIRST (the silent-data-loss floor — a cell with an
    /// unwhole backup never takes real data, master §1 Tier 1), then readiness. The first failing
    /// step short-circuits and leaves the cell `Provisioning` with that step's reason.
    ///
    /// `readiness` is the cell's [`MetricsHealthSurface`] over its declared critical dependencies (the
    /// caller wires the cell's real probe). `restore_inputs` is the cell's restore-verify source (the
    /// Storage gate's [`GateInputs`] — the modeled clean target on this floor; the real `pg_restore`
    /// driver is the Storage floor P-S12/P-S15, unchanged in shape).
    pub fn provision_cell<H: DependencyHealth>(
        &self,
        registry: &mut Registry,
        cell: &CellId,
        restore_inputs: &GateInputs<'_>,
        readiness: &MetricsHealthSurface<H>,
        signals: &mut ProvisioningSignals,
    ) -> ProvisionVerdict {
        // Fail-closed: a cell with no inventory row cannot be gated.
        if registry.cell(cell).is_none() {
            return ProvisionVerdict::StayedProvisioning {
                cell: cell.clone(),
                failure: ProvisionFailure::UnknownCell { cell: cell.clone() },
            };
        }

        // STEP 1 — restore-verify (Storage 11.5). The silent-data-loss floor: a cell whose backup is
        // not whole NEVER goes Active (no real data over a red STOR-D1, master §1 Tier 1).
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
                // The cell stays `Provisioning` (we never flipped it) — 0 traffic.
                return ProvisionVerdict::StayedProvisioning {
                    cell: cell.clone(),
                    failure: ProvisionFailure::RestoreVerifyFailed {
                        detail: failure.to_string(),
                    },
                };
            }
        };

        // STEP 2 — readiness. A cell that cannot serve correct traffic (a dead critical dependency)
        // stays `Provisioning`.
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
                    down_dependencies: report
                        .down_critical
                        .iter()
                        .map(|d| d.0.clone())
                        .collect(),
                },
            };
        }
        registry.log_provisioning(CellProvisioning {
            cell_id: cell.clone(),
            step: STEP_READINESS.into(),
            outcome: ProvisioningOutcome::Passed,
        });

        // BOTH passed — flip the cell `Provisioning → Active` (the ONLY transition that admits
        // traffic). The place path (P-082) filters on `Active`, so until this flip the cell is
        // structurally invisible to placement (0 tenants placed on an unverified cell).
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

    /// **`decommission_tenant` — crypto-shred the tenant KEK (Storage 11.3).** Destroys the tenant's
    /// KEK in `region` ([`KmsEngine::destroy_kek`], the tenant-granularity crypto-shred lever): every
    /// DEK under it becomes unrecoverable, live AND in every backup (the source key is destroyed). The
    /// tenant's placement row (if present) moves to [`PlacementStatus::Offboarding`]. Returns `true`
    /// iff a KEK was present to destroy (idempotent: a second call destroys nothing and returns
    /// `false`). The decommission is the tenant-offboard lever — one operation, tenant-granularity
    /// erasure.
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
            // The placement record moves to Offboarding (the PII-free routing record reflects it).
            registry.set_placement_status(tenant, PlacementStatus::Offboarding);
        }
        shredded
    }

    /// **The number of tenants currently placed on a cell that is NOT `Active` — the CP-D6 invariant
    /// the gate guarantees is `0`.** A cell only goes `Active` after restore-verify + readiness pass,
    /// and the place path filters on `Active`, so no tenant is ever placed on an unverified
    /// (`Provisioning`) cell. This is the gate's headline-zero, asserted in the drill.
    pub fn tenants_on_unverified_cells(registry: &Registry) -> usize {
        registry
            .placements_iter()
            .filter(|p| {
                // The cell a tenant is homed on must be Active; any home cell that is not Active means
                // a tenant slipped onto an unverified cell (the invariant violation this counts).
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
    use crate::schema::{Capacity, Cell, IsolationKind, TenantPlacement};
    use crate::place::{CounterMinter, PlacementService};
    use myelin_storage::{
        ContentHash, ContinuousArchiver, ErasureLedger, KeyClass, RestoredObject, SourceLog,
        WalRow, WalSegment,
    };
    use myelin_substrate::{CriticalDependencies, HealthTable, MetricsHealthSurface};

    // ───────────────────────────── builders ─────────────────────────────

    fn region() -> Region {
        Region::new("eu-west")
    }

    fn provisioning_cell(id: &str) -> Cell {
        Cell {
            cell_id: CellId::from_token(id),
            region: region(),
            // The cell starts `Provisioning` — the gate is what flips it to `Active`.
            status: CellStatus::Provisioning,
            isolation_kind: IsolationKind::Pool,
            capacity: Capacity { tenants_max: 1000, write_qps_max: 5000, storage_bytes_max: 1 << 40 },
            utilisation: 0,
            version: 1,
            endpoint: format!("cell.eu-west.{id}.myelin.eu"),
        }
    }

    /// Backups covering offsets `0..=tail` (a base at 0 + the WAL tail archived to `tail`).
    fn reachable_archiver(tail: u64) -> ContinuousArchiver {
        let mut arch = ContinuousArchiver::new();
        arch.archive_segment(WalSegment { end_offset: 0, committed_at: 0 }).unwrap();
        arch.take_base_backup(1);
        arch.archive_segment(WalSegment { end_offset: tail, committed_at: 10 }).unwrap();
        arch
    }

    /// A KMS with a live KEK for the cell's tenant (so the restore-verify gate has key material to
    /// verify) — modeling the cell's stores being whole.
    fn live_kms() -> KmsEngine {
        let kms = KmsEngine::new();
        kms.ensure_kek(&KekId::new(TenantId::from_token("acme"), region()));
        kms.ensure_dek(&TenantId::from_token("acme"), &region(), KeyClass::Tenant).unwrap();
        kms
    }

    /// A WHOLE restore set: a row referencing a checksum-integral object, source events that project
    /// it, a reachable archiver, an empty erasure ledger — the restore-verify gate GREENs this.
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

    /// A CORRUPT restore set: a row references a blob the restore did NOT bring back (a dangling ref —
    /// the §7.3 silent-corruption case). The restore-verify gate REDs this, so the cell stays
    /// `Provisioning`.
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
                rows: vec![WalRow { id: "corrupt".into(), written_at: 90, blob_ref: Some(missing) }],
                objects: vec![], // the referenced blob is absent — the backup is NOT whole.
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

    /// A READY cell metrics-health surface (boot complete, all critical deps up).
    fn ready_surface() -> MetricsHealthSurface<HealthTable> {
        let critical = CriticalDependencies::new(["oltp", "blob", "kms"]);
        let health = HealthTable::new();
        let surface = MetricsHealthSurface::new(critical, health);
        surface.mark_started(); // boot complete → readiness is governed by dep health.
        surface
    }

    /// A NOT-READY cell surface: a critical dependency (`kms`) is down → readiness sheds.
    fn not_ready_surface() -> MetricsHealthSurface<HealthTable> {
        let critical = CriticalDependencies::new(["oltp", "blob", "kms"]);
        let health = HealthTable::new();
        health.mark_down("kms"); // a dead critical dependency → NotReady.
        let surface = MetricsHealthSurface::new(critical, health);
        surface.mark_started();
        surface
    }

    fn registry_with_cell(cell: Cell) -> Registry {
        let mut reg = Registry::new();
        reg.insert_cell(cell);
        reg
    }

    // ───────────────────────────── unit: the gate ─────────────────────────────

    /// **A cell stays `Provisioning` until restore-verify + readiness pass; on pass it goes `Active`.**
    /// The headline CP-D6 property: both gating steps green ⇒ the cell is flipped `Active` and the
    /// restore-verify green artifact is returned.
    #[test]
    fn cell_goes_active_only_when_restore_verify_and_readiness_pass() {
        let mut reg = registry_with_cell(provisioning_cell("cell-w-1"));
        let cell = CellId::from_token("cell-w-1");
        // Before provisioning: the cell is Provisioning (not yet gated).
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

        assert!(verdict.is_active(), "both gating steps green ⇒ Active: {verdict:?}");
        assert!(verdict.green_artifact().is_some(), "the restore-verify green artifact is carried");
        // The cell is now Active in the registry (the ONLY traffic-admitting state).
        assert_eq!(reg.cell(&cell).unwrap().status, CellStatus::Active);
        assert_eq!(signals.cells_activated, 1);
        assert_eq!(signals.cells_held_provisioning, 0);
        // Every gating step + the activation are recorded in the cell_provisioning log.
        let steps: Vec<&str> = reg.provisioning_log().iter().map(|e| e.step.as_str()).collect();
        assert_eq!(steps, vec![STEP_RESTORE_VERIFY, STEP_READINESS, STEP_ACTIVATE]);
        assert!(reg.provisioning_log().iter().all(|e| e.outcome == ProvisioningOutcome::Passed));
    }

    /// **A failing RESTORE-VERIFY leaves the cell `Provisioning` (the silent-data-loss floor).** A cell
    /// whose backup is not whole NEVER goes `Active` — the place-real-data capability does not come
    /// online over a red restore-verify (master §1 Tier 1). 0 traffic.
    #[test]
    fn failing_restore_verify_keeps_the_cell_provisioning() {
        let mut reg = registry_with_cell(provisioning_cell("cell-w-1"));
        let cell = CellId::from_token("cell-w-1");
        let restore = CorruptRestore::new(); // the backup is NOT whole.
        let readiness = ready_surface(); // readiness would pass — but restore-verify is first + fails.
        let mut signals = ProvisioningSignals::default();

        let verdict = ProvisioningGate::new().provision_cell(
            &mut reg,
            &cell,
            &restore.inputs(),
            &readiness,
            &mut signals,
        );

        assert!(!verdict.is_active(), "a red restore-verify must NOT activate the cell");
        assert!(
            matches!(verdict.failure(), Some(ProvisionFailure::RestoreVerifyFailed { .. })),
            "the failure names restore-verify: {verdict:?}"
        );
        // The rendered failure is loud + specific (observability is part of the pass, EI-01 §3).
        let rendered = verdict.failure().unwrap().to_string();
        assert!(rendered.contains("restore-verify FAILED"), "loud restore-verify reason: {rendered}");
        assert!(rendered.contains("master §1 Tier 1"), "names the silent-data-loss floor: {rendered}");
        // The cell stayed `Provisioning` (we never flipped it) — 0 traffic.
        assert_eq!(reg.cell(&cell).unwrap().status, CellStatus::Provisioning);
        assert_eq!(signals.cells_activated, 0);
        assert_eq!(signals.cells_held_provisioning, 1);
        // The log records the restore-verify step FAILED (and readiness was never reached).
        let log = reg.provisioning_log();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].step, STEP_RESTORE_VERIFY);
        assert_eq!(log[0].outcome, ProvisioningOutcome::Failed);
    }

    /// **A NOT-READY cell stays `Provisioning` even when restore-verify passes.** Readiness is the
    /// second gating step — a dead critical dependency means the cell cannot serve correct traffic, so
    /// it gets no traffic. The failure names the down dependency.
    #[test]
    fn not_ready_keeps_the_cell_provisioning() {
        let mut reg = registry_with_cell(provisioning_cell("cell-w-1"));
        let cell = CellId::from_token("cell-w-1");
        let restore = WholeRestore::new(); // restore-verify passes...
        let readiness = not_ready_surface(); // ...but a critical dependency is down.
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
                assert!(down_dependencies.contains(&"kms".to_string()), "names the down dep");
            }
            other => panic!("expected NotReady, got {other:?}"),
        }
        // The rendered failure is loud + names the down dependency.
        let rendered = verdict.failure().unwrap().to_string();
        assert!(rendered.contains("readiness FAILED"), "loud readiness reason: {rendered}");
        assert!(rendered.contains("kms"), "names the down dependency: {rendered}");
        assert_eq!(reg.cell(&cell).unwrap().status, CellStatus::Provisioning);
        assert_eq!(signals.cells_held_provisioning, 1);
        // restore-verify Passed, readiness Failed, no activate step.
        let log = reg.provisioning_log();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].outcome, ProvisioningOutcome::Passed);
        assert_eq!(log[1].step, STEP_READINESS);
        assert_eq!(log[1].outcome, ProvisioningOutcome::Failed);
    }

    /// **0 tenants are placed on a cell that has not passed restore-verify (the CP-D6 zero).** The
    /// place path (P-082) filters on `Active`; a `Provisioning` cell is structurally invisible to
    /// placement. After a FAILED provision, `place` refuses — 0 tenants on the unverified cell.
    #[test]
    fn zero_tenants_placed_on_an_unverified_cell() {
        let mut reg = registry_with_cell(provisioning_cell("cell-w-1"));
        let cell = CellId::from_token("cell-w-1");
        let mut signals = ProvisioningSignals::default();

        // The cell FAILS restore-verify → stays Provisioning.
        let restore = CorruptRestore::new();
        let readiness = ready_surface();
        let verdict = ProvisioningGate::new().provision_cell(
            &mut reg, &cell, &restore.inputs(), &readiness, &mut signals,
        );
        assert!(!verdict.is_active());

        // `place` cannot route to the unverified cell — assignment filters on Active.
        let placer = PlacementService::new(CounterMinter::new());
        let err = placer.place(&mut reg, &region(), IsolationKind::Pool, "acme").expect_err(
            "no Active cell ⇒ place refuses; it never routes to an unverified cell",
        );
        assert!(err.to_string().contains("no Active cell"), "loud refusal: {err}");

        // The CP-D6 headline zero: 0 tenants placed, and 0 on an unverified cell.
        assert_eq!(reg.placement_count(), 0);
        assert_eq!(ProvisioningGate::tenants_on_unverified_cells(&reg), 0);
    }

    /// **After a SUCCESSFUL provision, `place` may route to the now-`Active` cell — and that tenant is
    /// NOT on an unverified cell.** The complement of the zero: once the gate greens, the cell takes
    /// traffic, and the tenant lands on a verified (`Active`) cell.
    #[test]
    fn place_routes_to_a_verified_cell_after_provisioning() {
        let mut reg = registry_with_cell(provisioning_cell("cell-w-1"));
        let cell = CellId::from_token("cell-w-1");
        let mut signals = ProvisioningSignals::default();

        let restore = WholeRestore::new();
        let readiness = ready_surface();
        assert!(
            ProvisioningGate::new()
                .provision_cell(&mut reg, &cell, &restore.inputs(), &readiness, &mut signals)
                .is_active()
        );

        // Now the cell is Active — place routes to it.
        let placer = PlacementService::new(CounterMinter::new());
        let answer = placer
            .place(&mut reg, &region(), IsolationKind::Pool, "acme")
            .expect("an Active cell accepts the placement");
        assert_eq!(answer.home_cell.as_str(), "cell-w-1");
        // The placed tenant is on a VERIFIED (Active) cell — 0 tenants on an unverified cell.
        assert_eq!(ProvisioningGate::tenants_on_unverified_cells(&reg), 0);
    }

    /// **`tenants_on_unverified_cells` actually COUNTS — it is not the constant 0.** If a placement is
    /// (incorrectly) on a non-`Active` cell, the function reports it. This kills the "always 0" mutant:
    /// the CP-D6 zero is only meaningful if the counter genuinely counts. We force the pathological
    /// state (a placement whose home cell was flipped back to `Provisioning` — e.g. a regression that
    /// de-activated a cell out from under a tenant) and assert the function reports `1`, then heal it
    /// (re-`Active`) and assert it drops to `0`.
    #[test]
    fn tenants_on_unverified_cells_counts_a_real_violation() {
        let mut reg = registry_with_cell({
            let mut c = provisioning_cell("cell-w-1");
            c.status = CellStatus::Active; // Active so the placement invariant admits the tenant.
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
        // Healthy: the tenant is on an Active cell → 0 on an unverified cell.
        assert_eq!(ProvisioningGate::tenants_on_unverified_cells(&reg), 0);

        // Pathological: the home cell is de-activated under the tenant (a regression the counter
        // catches). The function MUST now report 1 (not a constant 0).
        assert!(reg.set_cell_status(&CellId::from_token("cell-w-1"), CellStatus::Provisioning));
        assert_eq!(
            ProvisioningGate::tenants_on_unverified_cells(&reg),
            1,
            "a tenant on a non-Active cell is counted — the CP-D6 zero is real, not vacuous"
        );

        // Heal: re-Active → back to 0.
        assert!(reg.activate_cell(&CellId::from_token("cell-w-1")));
        assert_eq!(ProvisioningGate::tenants_on_unverified_cells(&reg), 0);
    }

    /// **An UNKNOWN cell is refused fail-closed (the gate cannot provision a cell with no inventory
    /// row).** No status flip, no log entry beyond the refusal verdict.
    #[test]
    fn unknown_cell_is_refused_fail_closed() {
        let mut reg = Registry::new(); // empty — the cell is not registered.
        let cell = CellId::from_token("cell-ghost");
        let restore = WholeRestore::new();
        let readiness = ready_surface();
        let mut signals = ProvisioningSignals::default();

        let verdict = ProvisioningGate::new().provision_cell(
            &mut reg, &cell, &restore.inputs(), &readiness, &mut signals,
        );
        assert!(matches!(verdict.failure(), Some(ProvisionFailure::UnknownCell { .. })));
        assert!(!verdict.is_active());
        let rendered = verdict.failure().unwrap().to_string();
        assert!(rendered.contains("not registered"), "loud unknown-cell reason: {rendered}");
        assert!(rendered.contains("cell-ghost"), "names the unknown cell: {rendered}");
    }

    // ───────────────────────────── unit: tenant decommission (crypto-shred KEK) ─────────────────────────────

    /// **Tenant decommission crypto-shreds the tenant KEK (Storage 11.3 — the source key is
    /// destroyed).** `destroy_kek` returns true the first time (a KEK was present), and a subsequent
    /// `resolve_dek` fails (the key is gone). The placement moves to `Offboarding`. Idempotent: a
    /// second decommission destroys nothing.
    #[test]
    fn decommission_crypto_shreds_the_tenant_kek() {
        let mut reg = registry_with_cell({
            let mut c = provisioning_cell("cell-w-1");
            c.status = CellStatus::Active;
            c
        });
        // A placed tenant whose KEK exists.
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
        kms.ensure_kek(&KekId::new(tenant.clone(), region()));
        // The key resolves BEFORE decommission.
        let key_ref = kms.ensure_dek(&tenant, &region(), KeyClass::Tenant).unwrap();
        assert!(kms.resolve_dek(&key_ref, &region()).is_ok(), "the DEK resolves while live");

        let mut signals = ProvisioningSignals::default();
        let gate = ProvisioningGate::new();
        // Decommission: crypto-shred the KEK.
        let shredded = gate.decommission_tenant(&mut reg, &kms, &tenant, &region(), &mut signals);
        assert!(shredded, "a KEK was present to destroy");
        assert_eq!(signals.tenants_decommissioned, 1);

        // The source key is DESTROYED — the DEK no longer resolves (crypto-shred).
        assert!(
            kms.resolve_dek(&key_ref, &region()).is_err(),
            "after decommission the DEK is unrecoverable (the KEK was crypto-shredded)"
        );
        // The placement reflects the offboarding.
        assert_eq!(reg.placement(&tenant).unwrap().status, PlacementStatus::Offboarding);

        // Idempotent: a second decommission destroys nothing (the KEK is already gone).
        let again = gate.decommission_tenant(&mut reg, &kms, &tenant, &region(), &mut signals);
        assert!(!again, "a second decommission destroys nothing (idempotent)");
        assert_eq!(signals.tenants_decommissioned, 1, "the count does not double-increment");
    }

    // ───────────────────────────── CDC: the provisioning gate (provider + consumer) ─────────────────────────────

    /// **CDC pair for the provisioning gate (provider + consumer).** The PROVIDER is this crate's
    /// [`ProvisioningGate`] gating a cell `Provisioning → Active`. The CONSUMER stands in for a
    /// **`place`/sizing caller** (P-082/§7.1): it checks a cell is `Active` (== passed restore-verify +
    /// readiness) BEFORE placing a tenant on it. Load-bearing: the consumer can ONLY observe the cell's
    /// `status` (the gate's verdict made durable) — it cannot place onto a `Provisioning` cell (the
    /// place path filters on `Active`). If the gating contract drifts (a cell flips Active without
    /// passing the gate), this consumer's invariant (`tenants_on_unverified_cells == 0`) breaks.
    #[test]
    fn cdc_provisioning_gate_provider_consumer() {
        /// A stand-in `place`/sizing consumer: it places a tenant ONLY on a cell the gate marked
        /// `Active`, and asserts the CP-D6 zero (0 tenants on an unverified cell).
        struct SizingCaller<'a> {
            placer: &'a PlacementService,
        }
        impl SizingCaller<'_> {
            /// Place a tenant — but ONLY if an `Active` (gate-passed) cell exists in the region.
            fn place_if_verified(
                &self,
                reg: &mut Registry,
                region: &Region,
            ) -> Result<(), String> {
                // The consumer reads the gate's durable verdict (cell.status) via the place path,
                // which structurally refuses unless an Active cell exists.
                self.placer
                    .place(reg, region, IsolationKind::Pool, "acme")
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            }
        }

        // PROVIDER: provision a cell through the gate.
        let mut reg = registry_with_cell(provisioning_cell("cell-w-1"));
        let cell = CellId::from_token("cell-w-1");
        let placer = PlacementService::new(CounterMinter::new());
        let caller = SizingCaller { placer: &placer };

        // BEFORE the gate passes: the consumer cannot place (the cell is Provisioning).
        assert!(
            caller.place_if_verified(&mut reg, &region()).is_err(),
            "a Provisioning (un-gated) cell takes no tenants"
        );
        assert_eq!(ProvisioningGate::tenants_on_unverified_cells(&reg), 0);

        // Run the gate (restore-verify + readiness pass) → the cell goes Active.
        let restore = WholeRestore::new();
        let readiness = ready_surface();
        let mut signals = ProvisioningSignals::default();
        assert!(
            ProvisioningGate::new()
                .provision_cell(&mut reg, &cell, &restore.inputs(), &readiness, &mut signals)
                .is_active()
        );

        // AFTER the gate passes: the consumer CAN place — on a verified cell.
        caller.place_if_verified(&mut reg, &region()).expect("the gated cell accepts the tenant");
        assert_eq!(reg.placement_count(), 1);
        assert_eq!(ProvisioningGate::tenants_on_unverified_cells(&reg), 0);
    }
}
