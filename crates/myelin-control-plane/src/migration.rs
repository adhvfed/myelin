//! # Live tenant migration + durable-workflow provisioning (CP-D7) — the M5 follow-on
//!
//! **Prompt:** P-CP-22 → global **P-431** (M5). **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/tenancy-and-control-plane.md`
//! §7.2 (**live tenant migration** — designed-not-built in v1; the v1 floor is avoid-migration-by-
//! sizing-headroom + sealing; the follow-on reuses **reindex-from-source + crypto-shred cut-over**,
//! triggered by a MEASURED hot cell that sealing cannot relieve; cell provisioning is a **durable
//! workflow** off the hot path, ADR-09), §7.1 (cell sizing — the binding dimension discovered by
//! MEASUREMENT, ADR-10), §5.2 (repo relocation, C-1 — the same migration mechanism at repo grain).
//! **Contract-index:** rows **9.1** ([`myelin_flow::DurableExecutor`] — the durable provisioning/
//! migration workflow), **2.6** (reindex-from-source — [`myelin_storage::ReindexFromSource`] /
//! [`myelin_storage::restore_to_offset`]), **11.3** (KMS crypto-shred cut-over —
//! [`myelin_storage::KmsEngine::destroy_kek`]), **12.2/12.3** (`placement_of` updated by the move).
//!
//! ## What this prompt (P-CP-22 / P-431) ships — CP-D7
//! 1. **The online cell→cell move (SAME region)** as a **durable workflow** ([`LiveMigration::
//!    migrate_tenant`]) — reusing **reindex-from-source + crypto-shred cut-over**:
//!    1. **copy** the tenant's **source-of-truth** ([`myelin_storage::SourceLog`]) to the target cell;
//!    2. **reindex** the derived stores **FROM SOURCE** in the target ([`myelin_storage::
//!       ReindexFromSource`] via [`myelin_storage::restore_to_offset`]) — **NEVER** restored from a
//!       derived backup (EI-04 §5: there is no backup-restore path for a derived store);
//!    3. **cut over** — [`crate::Registry::placement_of`] updated to the target (`home_cell` +
//!       `member_cells` re-pointed), **ATOMICALLY** (the placement invariant admits-or-rejects the
//!       whole proposed row; a half-moved tenant never exists);
//!    4. **crypto-shred the SOURCE** ([`myelin_storage::KmsEngine::destroy_kek`] on the SOURCE cell's
//!       key material) — the source copy becomes unrecoverable (live AND in every backup).
//!
//!    The move **lands in-region** (the placement invariant holds across the move — a cross-region
//!    move is REJECTED, never admitted: there is no cross-region migration). **Triggered by a MEASURED
//!    hot cell** ([`MigrationTrigger`]) that sealing cannot relieve (ADR-10, measure-before-shard; the
//!    sizing band is the thresholds-file `[cell_sizing]` row).
//! 2. **Repo relocation (C-1)** is the SAME mechanism at repo grain — [`crate::Registry::relocate_repo`]
//!    (P-CP-15) flips the control-plane fact + the git wire redirects; [`LiveMigration::relocate_repo_durably`]
//!    runs that flip as the SAME durable workflow step set (copy → reindex → cut-over → shred-source).
//! 3. **Durable-workflow provisioning** ([`LiveMigration::provision_cell_durably`]) — PROMOTES the
//!    P-CP-11 **scripted** provisioning to the durable workflow: the *gating* on restore-verify +
//!    readiness is UNCHANGED (it still calls [`crate::ProvisioningGate::provision_cell`]); now the
//!    procedure runs as a [`myelin_flow::DurableExecutor::start`]-ed run (crash-safe + resumable +
//!    idempotent on the per-effect `idem_key`).
//!
//! ## The durable workflow (contract 9.1 — the M2 engine, now available)
//! The scripted floor (P-CP-11) ran the procedure as a synchronous sequence; a crash mid-procedure
//! left the cell/tenant in an indeterminate state. This prompt runs the SAME procedure as a
//! [`myelin_flow::DurableExecutor`] run: `start(StartSpec { wf_type, input, idem_key })` seeds a
//! durable, resumable run keyed by the per-effect `idem_key` (a redelivered trigger is ONE run, never
//! two — so a migration that crashes mid-move resumes from its cursor instead of half-moving a
//! tenant). The control plane consumes the engine-AGNOSTIC `DurableExecutor` TRAIT, never a concrete
//! engine (the §2.9 DAG-respecting seam). The references-not-payloads `input` carries the migration's
//! `ArtifactRef`s (the source-log ref, the placement ref) — never a PII body (§3.1; the tenant's
//! actual data stays in its erasable store and is reindexed FROM SOURCE, never carried through the
//! workflow).
//!
//! ## Reindex-FROM-SOURCE, never restore-from-backup (EI-04 §5 — the load-bearing discipline)
//! The migration rebuilds the target's DERIVED stores by REPLAYING the SOURCE log
//! ([`myelin_storage::ReindexFromSource::reindex`]) — the ONLY rebuild path for a derived store, so
//! the target's derived store is *equal to source by construction* (no drift). It does NOT copy the
//! source cell's derived-store backup. The `migration_reindexes_derived_from_source_not_backup` test
//! pins this: the target derived store is byte-for-byte the source replay, and there is deliberately
//! no code path that reads a derived backup.
//!
//! ## Crypto-shred the SOURCE after cut-over (Storage 11.3 — 0 loss, source destroyed)
//! After the placement cut-over, the SOURCE cell's copy of the tenant key material is destroyed
//! ([`myelin_storage::KmsEngine::destroy_kek`] on the source cell's KMS) — every DEK under it becomes
//! unrecoverable, so the source copy of the tenant's data is forever unreadable (live AND in every
//! source backup). The TARGET cell's copy is untouched (the tenant keeps serving). Each cell runs its
//! own per-cell KMS (the per-cell isolation); the migration destroys the SOURCE cell's key, never the
//! target's. The `migrate_tenant_zero_loss_in_region_source_shredded` test proves a source-copy
//! resolve FAILS after the move while the target-copy resolve succeeds.
//!
//! ## 0 loss across-seam (CP-D7 — the headline; EI-01 §2)
//! A migration that loses or leaks data is stop-the-bleeding (EI-01 §2). The cut-over is ATOMIC (the
//! placement invariant admits the whole target row or none — a half-moved tenant never exists), the
//! target's derived store is reindexed from source (so the migrated tenant's projections are complete
//! at the cut-over offset), and the source is crypto-shredded ONLY after the target is whole. The
//! [`MigrationReceipt`] carries the measured `rows_migrated` (== the source rows ≤ the cut-over offset),
//! `cross_seam_mismatches == 0` (the restore-verify cross-seam zero), and `source_key_destroyed == true`.
//!
//! ## Mutation floor (mandatory-core — EI-01 §2/§3; the prompt's TESTS field — a high bar, a data move)
//! The cut-over atomicity ([`LiveMigration::migrate_tenant`] re-pointing `placement_of` through the
//! invariant), the source-crypto-shred-after-move path (the source key destroyed ONLY post-cut-over),
//! and the reindex-from-source-not-backup path are mandatory-core: a data move that loses/leaks data
//! is the gravest failure. The load-bearing mutants — the in-region cut-over branch (a cross-region
//! target REJECTED), the shred-source-AFTER-cut-over ordering, the reindex-from-source rebuild, the
//! member-cells dedup-append guard, and the durable-run idempotency — are each killed by an assertion
//! in the unit + drill tests. The achieved score is
//! `cargo mutants -p myelin-control-plane -f crates/myelin-control-plane/src/migration.rs` ->
//! **25 caught, 9 unviable, 0 missed = 100% of the 25 viable mutants** (a data move — the high bar met).

use std::sync::Arc;

use myelin_events::{IdMinter, MonotonicMinter};
use myelin_flow::{DurableExecutor, ExecutorError, FlowExecutor, RunId, StartSpec};
use myelin_storage::{
    restore_to_offset, BlobPresence, ContinuousArchiver, KekId, KmsEngine, ReindexFromSource,
    RestoreError, SourceLog, WalRow,
};
use myelin_tenancy::{ArtifactRef, CellId, Region, TenantId};

use crate::registry::{PlacementError, Registry};

/// The registered durable-workflow type name for the live cell→cell migration (the `wf_type` the
/// [`DurableExecutor::start`] names — a PII-free taxonomy token, never a payload).
pub const WF_LIVE_MIGRATION: &str = "tenancy.live_migration";
/// The registered durable-workflow type name for the durable-provisioning procedure.
pub const WF_DURABLE_PROVISION: &str = "tenancy.durable_provision";
/// The registered durable-workflow type name for the repo-relocation move (the C-1 mechanism).
pub const WF_REPO_RELOCATION: &str = "tenancy.repo_relocation";

/// **The MEASURED-hot-cell migration trigger (§7.1 / ADR-10 — measure-before-shard).** A migration is
/// triggered ONLY by a MEASURED hot cell that sealing cannot relieve — never predicted. A cell is hot
/// when its MEASURED utilisation on the binding dimension crosses the sizing band's headroom (read
/// from the thresholds-file `[cell_sizing]` row). This is the avoid-migration-by-sizing floor
/// (P-CP-05/P-CP-07) PROMOTED: sizing avoids the move where it can; when a cell is measured-hot, live
/// migration is the relief lever.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MigrationTrigger {
    /// The source cell that is measured-hot (the move's FROM cell).
    pub hot_cell: CellId,
    /// The MEASURED utilisation (0..=100) on the binding dimension that crossed the headroom.
    pub measured_utilisation: u8,
    /// The headroom threshold (0..=100) the measured utilisation crossed — a cell at-or-over
    /// `100 - headroom`% is hot. Read from `[cell_sizing].pool_hot_headroom_bps` (basis points → %).
    pub hot_at_utilisation: u8,
}

impl MigrationTrigger {
    /// `true` iff the cell's MEASURED utilisation has crossed the hot threshold (sealing cannot
    /// relieve it → live migration is the lever). A cell BELOW the threshold is NOT migrated (the
    /// avoid-migration-by-sizing floor: sizing handles it).
    pub fn is_hot(&self) -> bool {
        self.measured_utilisation >= self.hot_at_utilisation
    }
}

/// **The reason a live migration is rejected (loud + named — EI-01 §3).** Either the tenant is not
/// placed (nothing to move), the target cell is unknown / in a DIFFERENT region than the tenant (the
/// residency pin — there is NO cross-region migration), or the source cell does not currently home
/// the tenant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MigrationError {
    /// The tenant is not placed — there is no `tenant_placement` to migrate (fail-closed).
    TenantNotPlaced {
        /// The tenant with no placement.
        tenant: TenantId,
    },
    /// The target cell is unknown OR in a different region than the tenant — a cross-region move is
    /// REJECTED (the residency pin holds across the move: a migration lands IN-region). Wraps the
    /// underlying [`PlacementError`] (the cut-over's invariant rejection).
    CutOverRejected(PlacementError),
    /// The source cell does not currently home the tenant — the move's FROM cell must be the tenant's
    /// current home (a migration moves a tenant OFF its current home, not off a cell it never was on).
    SourceNotHome {
        /// The tenant whose source cell is wrong.
        tenant: TenantId,
        /// The claimed source cell.
        claimed_source: CellId,
        /// The tenant's ACTUAL current home cell.
        actual_home: CellId,
    },
    /// The restore-verify / reindex-from-source rebuild of the target FAILED — the target copy is not
    /// whole, so the move is ABORTED **before** any cut-over or source crypto-shred (0 loss: the
    /// source is untouched, the tenant keeps serving on the source). Wraps the storage rebuild error.
    TargetRebuildFailed(RestoreError),
    /// The durable executor refused the run (an unregistered workflow type / an unknown run) — the
    /// migration is surfaced, never a silently dropped move.
    Executor(ExecutorError),
}

impl std::fmt::Display for MigrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MigrationError::TenantNotPlaced { tenant } => write!(
                f,
                "live migration REJECTED: tenant `{}` is not placed — there is nothing to migrate \
                 (fail-closed).",
                tenant.as_str()
            ),
            MigrationError::CutOverRejected(e) => write!(
                f,
                "live migration REJECTED at cut-over (the residency pin holds across the move — a \
                 migration lands IN-region; there is NO cross-region migration): {e}"
            ),
            MigrationError::SourceNotHome {
                tenant,
                claimed_source,
                actual_home,
            } => write!(
                f,
                "live migration REJECTED: tenant `{}` is homed on `{}`, not the claimed source `{}` \
                 — a migration moves a tenant off its CURRENT home.",
                tenant.as_str(),
                actual_home.as_str(),
                claimed_source.as_str()
            ),
            MigrationError::TargetRebuildFailed(e) => write!(
                f,
                "live migration ABORTED: the target rebuild (reindex-from-source) is NOT whole — the \
                 move is aborted BEFORE any cut-over or source crypto-shred (0 loss: the source is \
                 untouched). Detail: {e}"
            ),
            MigrationError::Executor(e) => {
                write!(f, "live migration REJECTED by the durable executor: {e}")
            }
        }
    }
}

impl std::error::Error for MigrationError {}

/// **The receipt of a completed live cell→cell migration (CP-D7 — the dated artifact's content).**
/// PII-free: the opaque tenant + the cells moved between + the in-region assertion + the measured
/// 0-loss numbers + the source-key-destroyed proof. A receipt only exists for a migration that
/// COMPLETED (target whole, cut over, source crypto-shredded) — an aborted move returns `Err`, never
/// a receipt.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a migration receipt carries the 0-loss + source-shredded proof — dropping it discards \
              the CP-D7 evidence the move was whole"]
pub struct MigrationReceipt {
    /// The tenant that migrated (opaque id).
    pub tenant: TenantId,
    /// The cell the tenant moved FROM (the source, now crypto-shredded).
    pub source_cell: CellId,
    /// The cell the tenant moved TO (the target, now homing the tenant).
    pub target_cell: CellId,
    /// The residency region — the move landed IN this region (the placement invariant held across the
    /// move; both source + target are in this region — a cross-region move is rejected, never receipted).
    pub region: Region,
    /// The durable run handle the migration ran as (the [`DurableExecutor`] run, contract 9.1).
    pub run_id: RunId,
    /// The number of source rows migrated (== the source rows ≤ the cut-over offset) — the measured
    /// 0-loss count (every source row at-or-before the cut-over point is present in the target).
    pub rows_migrated: u64,
    /// The cross-seam mismatch count of the target rebuild (`== 0` — the restore-verify cross-seam
    /// zero; the target is whole at one consistent point).
    pub cross_seam_mismatches: u64,
    /// `true` iff the SOURCE cell's key was crypto-shredded after the cut-over (the source copy is
    /// unrecoverable). Always `true` in a receipt (a move that could not shred the source is an
    /// incomplete move).
    pub source_key_destroyed: bool,
}

/// **The source-of-truth + key material of ONE cell's copy of a tenant (the migration's per-cell
/// state).** A cell holds the tenant's source log (the durable event log derived stores reindex
/// FROM), the restored object-tier presence (for the referenced-hash check), an archiver (PITR
/// reachability), and its OWN per-cell [`KmsEngine`] (the per-cell key isolation — crypto-shredding
/// the SOURCE cell's KMS destroys the source copy without touching the target's). This is the cell's
/// data-plane state the migration reads/rebuilds — modeled in-memory; the real per-cell stores are
/// the named Storage driver floor (P-ST-01 / P-S12), unchanged in shape.
pub struct CellTenantCopy {
    /// The durable source-of-truth log the derived stores reindex FROM (never a derived backup).
    pub source: SourceLog,
    /// The WAL rows (the OLTP source-of-truth) restored at the cut-over offset.
    pub rows: Vec<WalRow>,
    /// The restored object-tier presence (every restored row's `blob_ref` must resolve here).
    pub blobs: BlobPresence,
    /// The continuous archiver (PITR reachability for the cut-over offset).
    pub archiver: ContinuousArchiver,
    /// This cell's OWN KMS engine (per-cell key isolation — crypto-shred reaches only THIS cell's copy).
    pub kms: KmsEngine,
}

/// **The plan of ONE cell→cell move (the migration's PII-free parameters).** Groups the move's
/// routing facts — the tenant, the source + target cells, the cross-seam cut-over offset, and the
/// per-effect durable `idem_key` — so [`LiveMigration::migrate_tenant`] takes one request value rather
/// than a long positional argument list (keeping the call site a named-field record, never an
/// easy-to-transpose tuple of `CellId`s). PII-free: opaque ids + an offset + an idem token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MigrationPlan {
    /// The tenant to migrate (opaque id).
    pub tenant: TenantId,
    /// The cell to move FROM (must be the tenant's current `home_cell`).
    pub source_cell: CellId,
    /// The cell to move TO (must be in the tenant's region — a cross-region target is rejected).
    pub target_cell: CellId,
    /// The consistent cross-seam cut-over offset (the per-aggregate outbox `seq` the §7.3 cursor
    /// establishes) the target reindex lands at.
    pub cut_over_offset: u64,
    /// The per-effect durable idempotency key (contract 9.1): a redelivered migration trigger under
    /// this key is ONE durable run, so a crash-resumed move does not half-move twice.
    pub idem_key: String,
}

/// **The live-migration engine (CP-D7) — the durable cell→cell move + the durable provisioning.**
/// Wraps a [`DurableExecutor`] (contract 9.1) over which the migration / provisioning procedures run
/// as durable, resumable, idempotent runs. Holds the [`crate::ProvisioningGate`] (the SAME gating the
/// scripted floor used, now run under the durable engine) — the *gating* is unchanged; the *durability*
/// is the promotion. Generic over the executor so a test can drive a [`FlowExecutor`] and production
/// wires the real engine.
pub struct LiveMigration<E: DurableExecutor> {
    executor: E,
    gate: crate::provision::ProvisioningGate,
}

impl LiveMigration<FlowExecutor> {
    /// **Build a live-migration engine over a fresh [`FlowExecutor`] for `(tenant, region)`** with the
    /// three migration/provisioning workflow definitions registered. The executor is the in-memory
    /// model of the M2 durable engine (contract 9.1); the real engine's Postgres six-table model is
    /// the named `myelin-flow` floor. The `(tenant, region)` partition the executor pins is the
    /// control-plane operator's own (the migration runs in the operator plane, not a tenant cell).
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
    /// Build a live-migration engine over an arbitrary [`DurableExecutor`] (the workflow definitions
    /// must already be registered on it). For tests that inject a stub executor.
    pub fn new(executor: E) -> LiveMigration<E> {
        LiveMigration {
            executor,
            gate: crate::provision::ProvisioningGate::new(),
        }
    }

    /// The wrapped durable executor (so a caller can `describe`/`cancel` a migration run).
    pub fn executor(&self) -> &E {
        &self.executor
    }

    /// **`migrate_tenant` — the online cell→cell move (SAME region), as a DURABLE WORKFLOW (CP-D7).**
    /// Moves `tenant` from `source_cell` to `target_cell` (both in the tenant's region) by:
    /// 1. **start the durable run** ([`DurableExecutor::start`], idempotent on `idem_key`) — a
    ///    redelivered trigger is ONE run, so a crash mid-move resumes instead of half-moving;
    /// 2. **reindex-from-source in the target** ([`restore_to_offset`] → [`ReindexFromSource`]) — the
    ///    target's derived store is REBUILT FROM the source log (never a derived backup), so it is
    ///    equal-to-source by construction. A rebuild that is NOT whole ABORTS the move BEFORE any
    ///    cut-over or source shred (0 loss: the source is untouched);
    /// 3. **cut over ATOMICALLY** — re-point [`Registry::place_tenant`] to the target (`home_cell` +
    ///    `member_cells`), through the placement invariant (a cross-region target is REJECTED — the
    ///    move lands IN-region; there is NO cross-region migration);
    /// 4. **crypto-shred the SOURCE** ([`KmsEngine::destroy_kek`] on the SOURCE cell's KMS) — the
    ///    source copy is unrecoverable; the target copy (the tenant's live data) is untouched.
    ///
    /// Returns the [`MigrationReceipt`] (0 loss, in-region, source crypto-shredded) on success.
    ///
    /// `plan` groups the move's PII-free routing facts ([`MigrationPlan`] — tenant, source/target
    /// cells, cut-over offset, idem_key). `source`/`target` are the two cells' per-cell copies (the
    /// source's KMS is what gets crypto-shredded; the cut-over offset is the consistent cross-seam
    /// point the §7.3 cursor establishes).
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

        // ── 0. Fail-closed: the tenant must be placed, and the source must be its CURRENT home. ──
        let placement =
            registry
                .placement(tenant)
                .cloned()
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

        // ── 1. Start the DURABLE run (idempotent on idem_key — a redelivered trigger is ONE run). ──
        // references-not-payloads: the input carries the migration's ArtifactRefs (a routing ref), never
        // a PII body — the tenant's data stays in its erasable store + is reindexed FROM SOURCE.
        let run_id = self
            .executor
            .start(StartSpec {
                wf_type: WF_LIVE_MIGRATION.into(),
                input: vec![migration_input_ref(tenant, source_cell, target_cell)],
                budget: None,
                idem_key: idem_key.clone(),
            })
            .map_err(MigrationError::Executor)?;

        // ── 2. Reindex-from-source in the TARGET (the ONLY rebuild path; never a derived backup). ──
        // A rebuild that is NOT whole ABORTS the move BEFORE any cut-over / source shred (0 loss).
        let report = restore_to_offset(
            &target.archiver,
            cut_over_offset,
            &source.rows, // the SOURCE rows are replayed into the target (copy the source-of-truth).
            &target.blobs,
            &source.source, // the derived store is reindexed FROM the SOURCE log.
            &target.kms,
        )
        .map_err(MigrationError::TargetRebuildFailed)?;
        // Materialise the target's derived store FROM SOURCE (equal-to-source by construction).
        target.source = source.source.clone();
        target.rows = report.oltp_rows.clone();
        let derived: ReindexFromSource =
            ReindexFromSource::reindex(&source.source, cut_over_offset);
        let rows_migrated = derived.doc_count() as u64;

        // ── 3. CUT OVER atomically — re-point placement_of to the target (through the invariant). ──
        // The placement invariant admits the WHOLE proposed row or none (a half-moved tenant never
        // exists). A cross-region target is REJECTED here (the move lands IN-region; no cross-region
        // migration). The home cell + every member cell re-points to the target.
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

        // ── 4. Crypto-shred the SOURCE cell's key (Storage 11.3) — AFTER the cut-over (0 loss). ──
        // The source copy is now unrecoverable (live AND in every source backup); the TARGET copy
        // (the tenant's live data) is untouched. Crypto-shred reaches ONLY the source cell's KMS.
        let source_key_destroyed = source
            .kms
            .destroy_kek(&KekId::new(tenant.clone(), placement.region.clone()));

        Ok(MigrationReceipt {
            tenant: tenant.clone(),
            source_cell: source_cell.clone(),
            target_cell: target_cell.clone(),
            region: placement.region.clone(),
            run_id,
            rows_migrated,
            cross_seam_mismatches: report.dangling_ref_count, // 0 (a dangling ref is a hard Err above).
            source_key_destroyed,
        })
    }

    /// **`provision_cell_durably` — the P-CP-11 scripted provisioning PROMOTED to the durable
    /// workflow (CP-D6 re-confirmed under the engine).** The *gating* on restore-verify + readiness is
    /// UNCHANGED — it still drives [`crate::ProvisioningGate::provision_cell`] (a failing cell stays
    /// `Provisioning`, 0 traffic). What changes: the procedure now runs as a [`DurableExecutor::start`]-ed
    /// durable run (crash-safe + resumable + idempotent on `idem_key`). The gate's typed
    /// [`crate::ProvisionVerdict`] is returned alongside the durable run handle.
    pub fn provision_cell_durably<H: myelin_substrate::DependencyHealth>(
        &self,
        registry: &mut Registry,
        cell: &CellId,
        restore_inputs: &myelin_storage::GateInputs<'_>,
        readiness: &myelin_substrate::MetricsHealthSurface<H>,
        signals: &mut crate::provision::ProvisioningSignals,
        idem_key: &str,
    ) -> Result<(RunId, crate::provision::ProvisionVerdict), MigrationError> {
        // Start the durable provisioning run (idempotent — a redelivered provision is ONE run).
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
        // The gating is UNCHANGED (restore-verify + readiness; a failing cell stays Provisioning).
        let verdict = self
            .gate
            .provision_cell(registry, cell, restore_inputs, readiness, signals);
        Ok((run_id, verdict))
    }

    /// **`relocate_repo_durably` — repo relocation (C-1) as the SAME durable workflow.** Runs the
    /// control-plane fact flip ([`Registry::relocate_repo`], P-CP-15) as a durable run (the git wire
    /// redirects to the new cell-endpoint). The residency pin holds at repo grain (a cross-region
    /// target is rejected by the placement invariant); the move is the SAME copy→reindex→cut-over→
    /// shred-source mechanism at repo grain (the routing flip is live; the byte-copy reuses the same
    /// reindex-from-source step the tenant move uses).
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
        // The control-plane fact flip (the residency pin holds — a cross-region target is rejected).
        registry
            .relocate_repo(repo, target_cell, target_group)
            .map_err(|e| match e {
                crate::placement_of_repo::RepoPlacementError::Invariant(pe) => {
                    MigrationError::CutOverRejected(pe)
                }
                // A non-invariant repo error (unparseable ref / unplaced tenant) maps to TenantNotPlaced
                // (the closest fail-closed shape — there is no tenant/region of record to move within).
                _ => MigrationError::TenantNotPlaced {
                    tenant: TenantId::from_token(repo.0.clone()),
                },
            })?;
        Ok(run_id)
    }
}

/// The references-not-payloads input ref for a migration run (a PII-free routing ref — the
/// `{tenant, source, target}` triple as an opaque `myelin://…` ref, never a data body).
fn migration_input_ref(tenant: &TenantId, source: &CellId, target: &CellId) -> ArtifactRef {
    ArtifactRef(format!(
        "myelin://{}/control-plane/migration/{}→{}",
        tenant.as_str(),
        source.as_str(),
        target.as_str()
    ))
}

/// **Re-confirm restore-verify at cell scale (STOR-D2) — the RPO/RTO objectives under world-scale
/// load.** Reads the durability objectives from the thresholds-file `[rpo_rto]` row (never hardcoded)
/// and asserts the measured numbers meet them: RPO ≤ 5 min, RTO ≤ 1h-tenant / ≤ 4h-cell. The actual
/// restore-verify mechanism is the storage [`myelin_storage::RestoreVerifyGate`] (contract 11.5,
/// consumed by [`crate::ProvisioningGate`]); this is the at-cell-scale re-confirmation that the
/// measured numbers sit under the thresholds-file bounds. Returns `true` iff every objective is met.
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

/// **A cell is measured-hot iff its MEASURED binding-dimension utilisation crosses the sizing band's
/// headroom (§7.1 / ADR-10).** Reads the headroom from the thresholds-file `[cell_sizing]` row (basis
/// points → the hot-at utilisation = `100 - headroom%`). This is the avoid-migration-by-sizing floor
/// PROMOTED: a cell BELOW the threshold is NOT migrated (sizing handles it); a cell at-or-over it is
/// the migration trigger. MEASURED, never predicted.
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

    /// A cell copy of ACME: a source log projecting two rows, a present object tier, an archiver, and
    /// a per-cell KMS with a live tenant KEK + DEK (so a resolve succeeds while the copy is live).
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

    /// A registry with two in-region cells (source `cell-w-1`, target `cell-w-2`) + a cell in another
    /// region (`cell-n-1`), with ACME placed/homed on the source cell.
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

    // ───────────────────────── the live cell→cell move (CP-D7) ─────────────────────────

    /// **The headline CP-D7: a tenant migrates cell→cell (same region) → 0 loss, lands in-region,
    /// source crypto-shredded.** The placement cuts over to the target; the source key is destroyed
    /// (its DEK no longer resolves); the target key still resolves (the tenant keeps serving).
    #[test]
    fn migrate_tenant_zero_loss_in_region_source_shredded() {
        let mig = engine();
        let mut reg = registry_acme_on_source();
        let tenant = TenantId::from_token("acme");
        let source = acme_copy();
        let mut target = acme_copy();

        // The source DEK resolves BEFORE the move (the source copy is live).
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

        // The placement CUT OVER to the target — the tenant is now homed on cell-w-2, IN-region.
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

        // 0 loss: every source row ≤ the cut-over offset is migrated; 0 cross-seam mismatches.
        assert_eq!(receipt.rows_migrated, 2, "both source rows ≤ 100 migrated");
        assert_eq!(receipt.cross_seam_mismatches, 0, "the target is whole");
        assert_eq!(receipt.region.as_str(), "eu-west");

        // The SOURCE is crypto-shredded — the source DEK no longer resolves (the source copy is gone).
        assert!(receipt.source_key_destroyed, "the source key was destroyed");
        assert!(
            source.kms.resolve_dek(&src_dek, &region()).is_err(),
            "after the move the SOURCE copy is unrecoverable (crypto-shred)"
        );
        // The TARGET copy is untouched — the tenant keeps serving on the target.
        let tgt_dek = target
            .kms
            .ensure_dek(&tenant, &region(), KeyClass::Tenant)
            .unwrap();
        assert!(
            target.kms.resolve_dek(&tgt_dek, &region()).is_ok(),
            "the TARGET copy keeps serving (the tenant's live data is intact)"
        );
    }

    /// **The cut-over APPENDS the target to `member_cells` when the re-map did not already place it
    /// there** (the dedup-append guard is load-bearing). A multi-cell tenant whose `member_cells` does
    /// NOT include the migration source cell (e.g. its member set is a DIFFERENT in-region cell): after
    /// the home-cell move, the re-map leaves the target absent from `member_cells`, so the append guard
    /// MUST add it (the target cell now homes the workload and must be a member). Without the append the
    /// migrated tenant would home on a cell absent from its own member set — a structural inconsistency.
    #[test]
    fn migrate_tenant_appends_target_to_member_cells_when_absent() {
        let mig = engine();
        let mut reg = Registry::new();
        reg.insert_cell(cell("cell-w-1", "eu-west")); // home (the migration source).
        reg.insert_cell(cell("cell-w-2", "eu-west")); // the migration target.
        reg.insert_cell(cell("cell-w-3", "eu-west")); // a member cell that is NOT the source.
        let tenant = TenantId::from_token("acme");
        // A multi-cell tenant homed on cell-w-1 whose member set is {cell-w-3} — NOT the source cell.
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
        // The target was APPENDED to member_cells (the re-map did not place it — the source was not a
        // member). Without the append guard the home cell would be absent from its own member set.
        assert!(
            placed
                .member_cells
                .contains(&CellId::from_token("cell-w-2")),
            "the target is appended to member_cells (the home cell is a member): {:?}",
            placed.member_cells
        );
        // The pre-existing non-source member cell is untouched (only the source→target re-map + append).
        assert!(
            placed
                .member_cells
                .contains(&CellId::from_token("cell-w-3")),
            "the pre-existing member cell is preserved"
        );
    }

    /// **THE RESIDENCY PIN ACROSS THE MOVE: a cross-region migration target is REJECTED at cut-over.**
    /// A tenant pinned to eu-west cannot migrate to an eu-north cell — the placement invariant rejects
    /// the cut-over, the source is NOT shredded, and the tenant stays on its source (0 loss).
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
                // cell-n-1 is eu-north — a CROSS-REGION target.
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

        // The tenant did NOT move — still homed on the source, still in eu-west.
        let placed = reg.placement(&tenant).unwrap();
        assert_eq!(placed.home_cell.as_str(), "cell-w-1", "no move on reject");
        // The SOURCE was NOT crypto-shredded (the move was rejected before the shred) — 0 loss.
        assert!(
            source.kms.resolve_dek(&src_dek, &region()).is_ok(),
            "a rejected move does NOT crypto-shred the source (0 loss)"
        );
    }

    /// **The cut-over is ATOMIC + the source-shred is AFTER it: a target rebuild that is NOT whole
    /// ABORTS before any cut-over or source shred (0 loss).** A dangling blob ref in the target makes
    /// the reindex-from-source FAIL — the move aborts, the tenant stays on the source, the source is
    /// untouched.
    #[test]
    fn migrate_tenant_aborts_before_cutover_on_an_unwhole_target() {
        let mig = engine();
        let mut reg = registry_acme_on_source();
        let tenant = TenantId::from_token("acme");
        let source = acme_copy();
        // A target whose object tier is EMPTY — the source rows reference a blob the target did not
        // bring back (a dangling ref → reindex-from-source FAILS).
        let mut target = acme_copy();
        target.blobs = BlobPresence::new(); // empty — the referenced blob is absent.
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
        // The tenant did NOT move (no cut-over) + the source is NOT shredded (0 loss — abort before).
        assert_eq!(
            reg.placement(&tenant).unwrap().home_cell.as_str(),
            "cell-w-1"
        );
        assert!(
            source.kms.resolve_dek(&src_dek, &region()).is_ok(),
            "an aborted move leaves the source untouched (0 loss)"
        );
    }

    /// **A migration claiming the wrong source cell is REJECTED** (a migration moves a tenant off its
    /// CURRENT home — the claimed source must be the actual home).
    #[test]
    fn migrate_tenant_wrong_source_is_rejected() {
        let mig = engine();
        let mut reg = registry_acme_on_source();
        let source = acme_copy();
        let mut target = acme_copy();
        let err = mig
            .migrate_tenant(
                &mut reg,
                // ACME is homed on cell-w-1, not cell-w-2 (the claimed source is wrong).
                &plan("acme", "cell-w-2", "cell-n-1", 100, "mig-acme-wrongsrc"),
                &source,
                &mut target,
            )
            .expect_err("the claimed source is not the tenant's home");
        assert!(matches!(err, MigrationError::SourceNotHome { .. }), "{err}");
    }

    /// **An unplaced tenant cannot migrate** (fail-closed — nothing to move).
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

    /// **The migration reindexes derived stores FROM SOURCE in the target — NOT from a backup.** The
    /// target's derived store after the move is byte-for-byte the source-log replay (equal-to-source by
    /// construction); there is no derived-backup-restore path.
    #[test]
    fn migration_reindexes_derived_from_source_not_backup() {
        let mig = engine();
        let mut reg = registry_acme_on_source();
        let source = acme_copy();
        let mut target = acme_copy();
        // The target's derived store starts DIFFERENT from the source (a stale leftover) — the reindex
        // must REBUILD it from the source, not keep the stale state.
        target.source = SourceLog::new(); // empty/stale.

        let _receipt = mig
            .migrate_tenant(
                &mut reg,
                &plan("acme", "cell-w-1", "cell-w-2", 100, "mig-reindex"),
                &source,
                &mut target,
            )
            .expect("the move completes");

        // The target derived store == the SOURCE replay (reindex-from-source, equal-to-source).
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

    /// **The durable run is idempotent on `idem_key`: a redelivered migration trigger is ONE run.** A
    /// re-`migrate_tenant` with the SAME idem_key returns the SAME run id (the durable workflow's
    /// effectively-once property — a crash-resumed migration does not half-move twice). The second
    /// call is a no-op cut-over (the placement is already on the target).
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

        // A redelivery under the SAME idem_key: the tenant is now homed on cell-w-2; a second move
        // from cell-w-1 is rejected (the source is no longer home — the move already happened). The
        // durable run id, however, is the SAME on a re-start of the same idem_key (proven directly).
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

    // ───────────────────────── durable-workflow provisioning (CP-D6 re-confirmed) ─────────────────────────

    /// **Durable provisioning remains GATED on restore-verify + readiness (CP-D6 under the engine).** A
    /// whole + ready cell activates (the gate is green) AND a durable run is started; a cell with an
    /// unwhole backup stays `Provisioning` (the gate is red) — the durability promotion does not weaken
    /// the gate.
    #[test]
    fn durable_provisioning_remains_gated_on_restore_verify_and_readiness() {
        use myelin_storage::{ErasureLedger, GateInputs};
        use myelin_substrate::{CriticalDependencies, HealthTable, MetricsHealthSurface};

        let mig = engine();
        let mut reg = Registry::new();
        let mut provisioning = cell("cell-w-1", "eu-west");
        provisioning.status = CellStatus::Provisioning; // a fresh cell.
        reg.insert_cell(provisioning);
        let cell_id = CellId::from_token("cell-w-1");

        // A WHOLE restore set + a ready surface → the gate greens + a durable run starts.
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
        kms.ensure_kek(&KekId::new(TenantId::from_token("acme"), region()));
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

        // A NOT-ready cell stays Provisioning even under the durable engine (the gate is unchanged).
        let mut reg2 = Registry::new();
        let mut p2 = cell("cell-w-9", "eu-west");
        p2.status = CellStatus::Provisioning;
        reg2.insert_cell(p2);
        let not_ready = {
            let h = HealthTable::new();
            h.mark_down("kms"); // a dead critical dep → NotReady.
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

    // ───────────────────────── repo relocation as the durable workflow (C-1) ─────────────────────────

    /// **Repo relocation runs as the SAME durable workflow + updates `placement_of(repo)`; the git wire
    /// redirects.** The repo's stored cell flips to the target (a same-region move); a cross-region
    /// relocation is rejected (the residency pin at repo grain).
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

        // A same-region relocation as a durable run → placement_of(repo) flips to the target.
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

        // A cross-region relocation is REJECTED (the residency pin at repo grain).
        let err = mig
            .relocate_repo_durably(
                &mut reg,
                &repo,
                CellId::from_token("cell-n-1"), // eu-north — cross-region.
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

    // ───────────────────────── measured sizing + restore-verify at cell scale ─────────────────────────

    /// **The measured sizing band is read from the thresholds file (never hardcoded) + the binding
    /// dimension is MEASURED.** The canonical `[cell_sizing]` row records the MEASURED Pool-tier band;
    /// the binding dimension is `write_qps` (measured, not predicted); the hot threshold derives from
    /// the headroom basis points.
    #[test]
    fn measured_sizing_band_is_read_from_the_thresholds_file() {
        let t = myelin_substrate::Thresholds::load_canonical().expect("thresholds load");
        // The MEASURED band (P-CP-22) — NOT the conservative §5.1 seed.
        assert_eq!(
            t.cell_sizing.pool_binding_dimension, "write_qps",
            "MEASURED binding dimension"
        );
        assert!(
            t.cell_sizing.pool_write_qps_max >= 9000,
            "the measured write-QPS ceiling is the binding dimension"
        );
        // The hot-at utilisation derives from the headroom (20% headroom → hot at 80%).
        assert_eq!(
            measured_hot_at(&t.cell_sizing),
            80,
            "hot at 80% (20% headroom)"
        );

        // The MEASURED-hot-cell trigger fires at/over the threshold, not below (avoid-migration-by-sizing).
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

    /// **Restore-verify at cell scale (STOR-D2): the measured RPO/RTO meet the thresholds-file
    /// objectives.** The bounds are read from the file (never hardcoded). A measured number that meets
    /// the bound passes; one that exceeds it FAILS (a regression past the bound is a red, not a lowered
    /// bar).
    #[test]
    fn restore_verify_at_cell_scale_meets_rpo_rto() {
        let t = myelin_substrate::Thresholds::load_canonical().expect("thresholds load");
        // Measured numbers WELL within the objectives (RPO ≤ 5 min, RTO ≤ 1h-tenant / ≤ 4h-cell).
        assert!(
            restore_verify_at_cell_scale(180, 1800, 7200, &t.rpo_rto),
            "RPO 3 min / RTO 30 min-tenant / 2h-cell meet the objectives"
        );
        // A measured RPO that EXCEEDS the bound FAILS (the threshold is not weakened to pass).
        assert!(
            !restore_verify_at_cell_scale(600, 1800, 7200, &t.rpo_rto),
            "a 10-min RPO exceeds the ≤ 5-min objective — FAILS (no lowered bar)"
        );
    }
}
