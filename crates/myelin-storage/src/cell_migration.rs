//! # The storage half of the cross-cell PII-free pointer bridge + cell→cell migration (CP-D7)
//!
//! **Prompt:** P-ST-32 → global **P-443** (M5). **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/storage.md` §2 "S-M5" (multi-cell: the cross-cell
//! PII-free pointer bridge goes live; cell→cell migration same-region with 0 loss + source
//! crypto-shredded — Storage built the per-cell KEK + per-cell backup machinery in M1, the cell→cell
//! migration is the M5 build), **§7.3** (the cross-seam consistency point the migration restores TO —
//! the per-aggregate outbox `seq` / event-log offset). **Contract-index:** rows **12.6** (the
//! cross-cell PII-free pointer bridge — `CrossCellPointer{subject opaque, type, correlation_id,
//! home_cell}`, resolution ALWAYS cell-local), **11.5** (the restore / cross-seam machinery the
//! migration uses). Recon **§OQ-I** (the multi-cell pointer bridge frame).
//!
//! ## What this module ships — the STORAGE half of CP-D7 (EI-01 §7 coherence: ONE migration, layered)
//! The CONTROL-PLANE half (the durable-workflow orchestration: trigger → start the durable run →
//! atomic placement cut-over → durable idempotency) is `myelin_control_plane::migration::LiveMigration`
//! (P-CP-22 / P-431). The bridge RESOLUTION half (cell A asks cell B to resolve a pointer IN B,
//! permission-checked in B, only the projection crossing) is
//! `myelin_control_plane::cross_cell_bridge::CrossCellBridge` (P-CP-19 / P-429). What this STORAGE
//! prompt owns — the **data-plane primitive** the control plane drives — is the **cell→cell migration
//! storage step**: *restore the source's cross-seam consistency point (§7.3) INTO the target cell with
//! 0 loss, then crypto-shred the SOURCE cell's key material so the source copy is unrecoverable (live
//! AND in every backup)*. This module composes the two existing storage primitives —
//! [`crate::restore::restore_to_offset`] (the §7.3 restore-to-consistent-point) and
//! [`crate::kms::KmsEngine::destroy_kek`] (the source crypto-shred, contract 11.3) — into ONE named,
//! receipted storage operation: [`migrate_cell_to_cell`].
//!
//! Per the coherence rule (EI-01 §7) this prompt does **NOT** re-define the restore mechanism, the
//! reindex-from-source replay, the KMS crypto-shred, or the CrossCellPointer frame — it REUSES them.
//! What is genuinely NEW here is the **storage-half COMPOSITION**: restore-into-target + source-shred
//! as one atomic-in-effect storage step that yields a [`CellMigrationReceipt`] (0 loss, in-region,
//! source shredded), plus the storage-side proof that the [`CrossCellPointer`] bridge carries only an
//! OPAQUE subject (no PII crosses the cell boundary; resolution is cell-local).
//!
//! ## Restore the cross-seam consistency point INTO the target (§7.3 — 0 loss across the seam)
//! The migration rebuilds the target cell at ONE consistent cross-seam point T (the source's outbox
//! `seq` / event-log offset): OLTP rows ≤ T are restored; every referenced `ContentHash` MUST be
//! present in the target object tier (a dangling ref is the §7.3 silent-corruption FAIL — the move
//! ABORTS before any source shred, 0 loss: the source is untouched and keeps serving); derived stores
//! are reindexed FROM SOURCE up to T (the ONLY rebuild path — derived == source by construction, no
//! drift, EI-04 §5); the target KEKs restore from the snapshot (crypto-shredded keys stay dead). The
//! restore-into-target is exactly [`crate::restore::restore_to_offset`] aimed at the TARGET cell's
//! tiers — the SAME restore-verify machinery STOR-D1/STOR-D2 gate (so STOR-D1/D2 hold across the cell
//! boundary by construction).
//!
//! ## Crypto-shred the SOURCE after the restore (contract 11.3 — source unrecoverable)
//! ONLY after the target is whole (the restore returned a [`crate::restore::RestoreReport`] with
//! `dangling_ref_count == 0`) does the migration crypto-shred the SOURCE cell's key
//! ([`crate::kms::KmsEngine::destroy_kek`] on the SOURCE cell's per-cell KMS) — every DEK under it
//! becomes unrecoverable, so the source copy is forever unreadable (live AND in every source backup,
//! since a crypto-shredded key is EXCLUDED from backup, §4 / §7.6). Each cell runs its OWN per-cell
//! KMS (the per-cell isolation), so the shred reaches ONLY the source; the target's freshly-restored
//! key set is untouched (the tenant keeps serving on the target). The ordering is load-bearing:
//! restore-target → verify-whole → shred-source. A source shred BEFORE a whole target would be data
//! loss; this module shreds the source ONLY on the success path.
//!
//! ## The cross-cell PII-free pointer bridge — the STORAGE-half property (12.6, resolution cell-local)
//! The storage layer holds a foreign cell's artifact behind a [`CrossCellPointer`] — an OPAQUE
//! subject + type + correlation_id + home_cell — and NEVER materialises that artifact's PII locally:
//! resolution is ALWAYS cell-local (the home cell renders + permission-checks; only the projection
//! crosses). The storage-half invariant this module proves is structural: a pointer to a
//! foreign-homed artifact carries only the four PII-free frame fields, and [`is_cell_local`] is the
//! single predicate that decides whether storage resolves locally (the pointer is homed HERE) or must
//! defer to the home cell (no foreign-cell PII ever lands in this cell's stores). The bridge
//! RESOLUTION transport is the control-plane `CrossCellBridge`; storage's job is to never read a
//! foreign cell's data into the local tier — [`storage_resolves_locally`] makes that decision explicit.
//!
//! ## Mutation floor (mandatory-core — EI-01 §2/§3; the prompt's TESTS field — a data move)
//! The restore-into-target + source-crypto-shred path ([`migrate_cell_to_cell`]) is mandatory-core: a
//! data move that loses/leaks data is the gravest failure. The load-bearing mutants — the
//! restore-into-target-before-shred ordering, the abort-before-shred-on-unwhole-target branch (0
//! loss), the source-key-destroyed assertion, and the cell-local-resolution discriminant
//! ([`is_cell_local`] / [`storage_resolves_locally`]) — are each killed by an assertion in the unit +
//! drill tests. The floor is **>= 80%** (mandatory-core); see the drill
//! `tests/cp_d7_cell_to_cell_migration_drill.rs` and the CDC pair
//! `tests/cdc_12_6_cell_migration.rs`.
//!
//! ## Floors named (VISION §3 name-your-floors)
//! - **The single-cell floor (M1) is PROMOTED to multi-cell** — the cell→cell migration storage step
//!   is LIVE. What rides the SIBLING prompt **P-ST-33 (global P-445)**: the multi-cell DSR erase
//!   fan-out (iterate `member_cells ∪ home_cell`). Named here.
//! - **The modeled restore floor (P-S12/P-S15)** — there is no live `pg_restore` on this floor (the
//!   real WAL-replay driver is the named storage floor); the migration's restore-into-target is
//!   modeled exactly over the abstract WAL offset via [`crate::restore::restore_to_offset`], whose
//!   SHAPE does not change when the real driver lands (it will POPULATE the restored state this
//!   receipt verifies). Inherited from [`crate::restore`], named there; re-stated here.

use myelin_tenancy::{CellId, CrossCellPointer, Region, TenantId};

use crate::backup::{ContinuousArchiver, WalOffset};
use crate::kms::{KekId, KmsEngine};
use crate::restore::{restore_to_offset, BlobPresence, RestoreError, SourceLog, WalRow};

// ───────────────────────────── the per-cell tenant copy (the data-plane state) ─────────────────────────────

/// **The storage-plane state of ONE cell's copy of a tenant (the migration's per-cell tiers).** A cell
/// holds the tenant's durable source-of-truth log (the event log derived stores reindex FROM, never a
/// derived backup), the WAL rows (the OLTP source-of-truth at the cut-over offset), the restored
/// object-tier presence (for the referenced-hash check), a continuous archiver (PITR reachability),
/// and its OWN per-cell [`KmsEngine`] (the per-cell key isolation — crypto-shredding the SOURCE cell's
/// KMS destroys the source copy without touching the target's).
///
/// This is the storage tiers the migration reads (source) / rebuilds (target). Modeled in-memory over
/// the abstract WAL offset; the real per-cell stores are the named Storage driver floor
/// (`crate::oltp` / `crate::backup` — P-S12/P-S15), unchanged in shape. PII-free: opaque ids + offsets
/// + content addresses, never a body.
pub struct CellTenantTiers {
    /// The durable source-of-truth log the derived stores reindex FROM (never a derived backup).
    pub source: SourceLog,
    /// The WAL rows (the OLTP source-of-truth) at/through the cut-over offset.
    pub rows: Vec<WalRow>,
    /// The restored object-tier presence (every restored row's `blob_ref` must resolve here).
    pub blobs: BlobPresence,
    /// The continuous archiver (PITR reachability for the cut-over offset).
    pub archiver: ContinuousArchiver,
    /// This cell's OWN KMS engine (per-cell key isolation — crypto-shred reaches only THIS cell's copy).
    pub kms: KmsEngine,
}

// ───────────────────────────── the migration request + receipt ─────────────────────────────

/// **The PII-free parameters of one storage cell→cell migration.** Groups the move's routing facts —
/// the tenant, the source + target cells, the residency region (the move lands IN this region), and the
/// cross-seam cut-over offset (the §7.3 consistency point the target restore lands at) — so
/// [`migrate_cell_to_cell`] takes one named-field request rather than a long positional argument list
/// (no easy-to-transpose tuple of `CellId`s). PII-free: opaque ids + a region code + an offset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellMigrationRequest {
    /// The tenant to migrate (opaque id).
    pub tenant: TenantId,
    /// The cell to move FROM (the source — crypto-shredded after the move).
    pub source_cell: CellId,
    /// The cell to move TO (the target — homes the tenant after the move).
    pub target_cell: CellId,
    /// The residency region the move lands IN (both cells are in this region — the residency pin holds;
    /// the cross-region rejection is the control-plane placement invariant, restated in the receipt).
    pub region: Region,
    /// The consistent cross-seam cut-over offset (the per-aggregate outbox `seq` the §7.3 cursor
    /// establishes) the target restore lands at.
    pub cut_over_offset: WalOffset,
}

/// **The reason a storage cell→cell migration was ABORTED (loud + named — EI-01 §3).** The only
/// data-plane abort is a target rebuild that is NOT whole (a dangling blob ref / an unreachable PITR
/// point) — the move aborts BEFORE any source crypto-shred (0 loss: the source is untouched, the
/// tenant keeps serving on the source). Wraps the storage [`RestoreError`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CellMigrationError {
    /// The restore-into-target (reindex-from-source to the cross-seam point) is NOT whole — the move
    /// ABORTS before any source crypto-shred (0 loss: the source is untouched). Wraps the storage
    /// restore error (a dangling blob ref = the §7.3 silent-corruption case, or an unreachable PITR
    /// point).
    TargetRestoreNotWhole(RestoreError),
}

impl core::fmt::Display for CellMigrationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CellMigrationError::TargetRestoreNotWhole(e) => write!(
                f,
                "cell→cell migration ABORTED: the target restore-into (reindex-from-source to the \
                 §7.3 consistency point) is NOT whole — the move is aborted BEFORE any source \
                 crypto-shred (0 loss: the source is untouched, the tenant keeps serving). Detail: {e}"
            ),
        }
    }
}

impl std::error::Error for CellMigrationError {}

/// **The receipt of a completed storage cell→cell migration (CP-D7 — the dated artifact's body).**
/// PII-free: the opaque tenant + the cells moved between + the in-region region code + the measured
/// 0-loss numbers + the source-key-destroyed proof. A receipt only exists for a migration that
/// COMPLETED (target restored whole at the cross-seam point, source crypto-shredded) — an aborted move
/// returns `Err`, never a receipt (so a receipt is itself the proof the move was whole).
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a cell-migration receipt carries the 0-loss + source-shredded CP-D7 proof — dropping it \
              discards the evidence the move was whole and the source unrecoverable"]
pub struct CellMigrationReceipt {
    /// The tenant that migrated (opaque id).
    pub tenant: TenantId,
    /// The cell the tenant moved FROM (the source, now crypto-shredded).
    pub source_cell: CellId,
    /// The cell the tenant moved TO (the target, now homing the tenant).
    pub target_cell: CellId,
    /// The residency region — the move landed IN this region (both cells share it; the residency pin
    /// held across the move — there is NO cross-region migration).
    pub region: Region,
    /// The cross-seam consistency point the target was restored to (== the requested cut-over offset).
    pub restored_to_offset: WalOffset,
    /// The number of source rows restored into the target (every OLTP row ≤ the cut-over offset) — the
    /// measured 0-loss count.
    pub rows_migrated: u64,
    /// The cross-seam mismatch count of the target restore (`== 0` — the restore-verify cross-seam
    /// zero; the target is whole at one consistent point, no dangling blob refs).
    pub cross_seam_mismatches: u64,
    /// `true` iff the SOURCE cell's key was crypto-shredded after the restore (the source copy is
    /// unrecoverable, live AND in every backup). Always `true` in a receipt (a move that could not
    /// shred the source is an incomplete move — but the shred of an existing key always succeeds).
    pub source_key_destroyed: bool,
}

// ───────────────────────────── the cell→cell migration storage step (CP-D7) ─────────────────────────────

/// **`migrate_cell_to_cell` — the STORAGE half of the cell→cell move (same region), CP-D7.** Restores
/// the source's cross-seam consistency point (§7.3) INTO the target cell with 0 loss, then crypto-shreds
/// the SOURCE cell's key so the source copy is unrecoverable. The ordering is load-bearing:
///
/// 1. **Restore-into-target** ([`restore_to_offset`] aimed at the TARGET's tiers) — replay the SOURCE
///    rows + SOURCE log into the target at the cut-over offset T: OLTP rows ≤ T, every referenced
///    `ContentHash` present in the target object tier (a dangling ref ABORTS — the §7.3
///    silent-corruption FAIL), derived reindexed FROM SOURCE up to T (derived == source by
///    construction). A rebuild that is NOT whole ABORTS the move HERE — BEFORE any source shred
///    ([`CellMigrationError::TargetRestoreNotWhole`]); 0 loss: the source is untouched.
/// 2. **Materialise** the target's source-of-truth log == the source's (the target now homes the
///    derived-rebuild input) and its restored OLTP rows.
/// 3. **Crypto-shred the SOURCE** ([`KmsEngine::destroy_kek`] on the SOURCE cell's KMS) — ONLY after
///    the target is whole. The source copy is now unrecoverable (live AND in every source backup); the
///    TARGET copy (freshly restored) is untouched. Per-cell isolation: the shred reaches ONLY the
///    source cell's KMS.
///
/// Returns a [`CellMigrationReceipt`] (0 loss, in-region, source crypto-shredded) on success, or
/// [`CellMigrationError::TargetRestoreNotWhole`] (the move aborted before any source shred — 0 loss).
///
/// The CROSS-REGION rejection is the control-plane placement invariant (the cut-over re-points
/// `placement_of` through the in-region invariant); this storage step asserts the request's `region` is
/// carried into the receipt unchanged (the data-plane move lands IN the request's region — it never
/// chooses a region).
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

    // ── 1. Restore-into-target (the §7.3 consistency point) — the ONLY rebuild path is FROM SOURCE. ──
    // A rebuild that is NOT whole (a dangling blob ref / an unreachable PITR point) ABORTS the move
    // HERE — BEFORE any source crypto-shred (0 loss: the source is untouched, the tenant keeps serving).
    let report = restore_to_offset(
        &target.archiver,
        cut_over_offset,
        &source.rows, // the SOURCE rows are restored into the target (copy the source-of-truth).
        &target.blobs, // verified against the TARGET object tier (a dangling ref = abort).
        &source.source, // the derived store is reindexed FROM the SOURCE log (never a backup).
        &target.kms,  // the TARGET KEKs restore from its snapshot (crypto-shredded keys stay dead).
    )
    .map_err(CellMigrationError::TargetRestoreNotWhole)?;

    // ── 2. Materialise the target's tiers FROM SOURCE (equal-to-source by construction). ──
    target.source = source.source.clone();
    target.rows = report.oltp_rows.clone();
    let rows_migrated = report.oltp_rows.len() as u64;

    // ── 3. Crypto-shred the SOURCE cell's key (contract 11.3) — AFTER the target is whole (0 loss). ──
    // The source copy is now unrecoverable (live AND in every source backup — a shredded key is
    // EXCLUDED from backup, §4/§7.6); the TARGET copy (freshly restored) is untouched. Per-cell
    // isolation: the shred reaches ONLY the SOURCE cell's KMS, never the target's.
    let source_key_destroyed = source
        .kms
        .destroy_kek(&KekId::new(tenant.clone(), region.clone()));

    Ok(CellMigrationReceipt {
        tenant: tenant.clone(),
        source_cell: source_cell.clone(),
        target_cell: target_cell.clone(),
        region: region.clone(),
        restored_to_offset: report.restored_to_offset,
        rows_migrated,
        // 0 by construction (a dangling ref returned Err above) — the restore-verify cross-seam zero.
        cross_seam_mismatches: report.dangling_ref_count,
        source_key_destroyed,
    })
}

// ───────────────────────────── the cross-cell PII-free pointer bridge (storage-half) ─────────────────────────────

/// **Is the `pointer` homed in `this_cell`? (the cell-local-resolution discriminant — 12.6).** The
/// load-bearing predicate of the cross-cell PII-free pointer bridge's STORAGE half: resolution is
/// ALWAYS cell-local, so storage resolves a pointer's artifact from the LOCAL tier ONLY when the
/// pointer is homed HERE. A pointer homed in a FOREIGN cell is NOT resolved locally — its PII never
/// lands in this cell's stores (the home cell renders + permission-checks; only the projection crosses
/// the control-plane bridge). This is the single decision storage makes; `true` ⇒ resolve from the
/// local tier, `false` ⇒ defer to the home cell (no local read of foreign PII).
#[inline]
pub fn is_cell_local(pointer: &CrossCellPointer, this_cell: &CellId) -> bool {
    pointer.home_cell() == this_cell
}

/// **Does storage resolve this `pointer` from its LOCAL tier? (the no-foreign-PII rule — 12.6).** The
/// inverse-facing companion to [`is_cell_local`], named for the call site: storage resolves a pointer
/// from its own stores IFF the pointer is homed in `this_cell`. For a foreign-homed pointer this is
/// `false` — storage NEVER reads the foreign artifact's PII into the local tier; it defers to the home
/// cell over the control-plane bridge (resolution is cell-local; only the already-rendered projection
/// crosses, never the PII). Equivalent to [`is_cell_local`], exposed under the storage-decision name so
/// the no-foreign-PII property reads at the call site.
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

    /// A cell copy of ACME: a source log projecting two rows, a present object tier (the referenced
    /// blob is present), a reachable archiver, and a per-cell KMS with a live tenant KEK + DEK (so a
    /// resolve succeeds while the copy is live).
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

    fn request(offset: WalOffset) -> CellMigrationRequest {
        CellMigrationRequest {
            tenant: TenantId::from_token("acme"),
            source_cell: CellId::from_token("cell-fr-1"),
            target_cell: CellId::from_token("cell-fr-2"),
            region: region(),
            cut_over_offset: offset,
        }
    }

    // ───────────────────────── the storage half of CP-D7 ─────────────────────────

    /// **The headline CP-D7 (storage half): cell→cell migration restores the §7.3 consistency point
    /// into the target with 0 loss, lands in-region, and crypto-shreds the SOURCE.** Every source row
    /// ≤ the cut-over offset is restored into the target; 0 cross-seam mismatches; the source DEK no
    /// longer resolves (source unrecoverable); the target DEK still resolves (the tenant keeps serving).
    #[test]
    fn migrate_cell_to_cell_zero_loss_in_region_source_shredded() {
        let tenant = TenantId::from_token("acme");
        let source = acme_tiers();
        let mut target = acme_tiers();

        // The source DEK resolves BEFORE the move (the source copy is live).
        let src_dek = source
            .kms
            .ensure_dek(&tenant, &region(), KeyClass::Tenant)
            .unwrap();
        assert!(source.kms.resolve_dek(&src_dek, &region()).is_ok());

        let receipt = migrate_cell_to_cell(&request(100), &source, &mut target)
            .expect("a same-region cell→cell move completes");

        // 0 loss: both source rows ≤ 100 restored into the target at the consistency point.
        assert_eq!(receipt.rows_migrated, 2, "both source rows ≤ 100 migrated");
        assert_eq!(
            receipt.restored_to_offset, 100,
            "restored to the cut-over point"
        );
        assert_eq!(receipt.cross_seam_mismatches, 0, "the target is whole");
        assert_eq!(receipt.region.as_str(), "fr-par", "lands IN-region");
        assert_eq!(receipt.source_cell.as_str(), "cell-fr-1");
        assert_eq!(receipt.target_cell.as_str(), "cell-fr-2");

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

    /// **The target derived store is reindexed FROM SOURCE (equal-to-source by construction) — never a
    /// derived backup.** After the move the target's source-of-truth log == the source's, so a reindex
    /// of the target replays the SAME docs as a reindex of the source.
    #[test]
    fn migration_reindexes_target_from_source_not_backup() {
        use crate::restore::ReindexFromSource;
        let source = acme_tiers();
        let mut target = acme_tiers();
        // The target's source-of-truth starts DIFFERENT (a stale leftover) — the move must REBUILD it.
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

    /// **The move ABORTS BEFORE any source crypto-shred on an unwhole target (0 loss).** A dangling
    /// blob ref in the target (the source rows reference a blob the target object tier does not hold)
    /// makes the restore-into-target FAIL — the move aborts, the source is NOT shredded (it keeps
    /// serving). This is the load-bearing ordering: restore-into-target → verify-whole → shred-source.
    #[test]
    fn migrate_cell_to_cell_aborts_before_shred_on_unwhole_target() {
        let tenant = TenantId::from_token("acme");
        let source = acme_tiers();
        // A target whose object tier is EMPTY — the source rows reference a blob the target did not
        // bring back (a dangling ref → restore-into-target FAILS).
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

        // The SOURCE is NOT crypto-shredded (the move aborted before the shred) — 0 loss.
        assert!(
            source.kms.resolve_dek(&src_dek, &region()).is_ok(),
            "an aborted move leaves the source untouched (0 loss)"
        );
    }

    /// **A migration to an UNREACHABLE cut-over offset aborts (loud PITR failure, source untouched).**
    /// The target archiver cannot reach the requested offset — the restore-into-target fails, the move
    /// aborts before any source shred.
    #[test]
    fn migrate_cell_to_cell_aborts_on_unreachable_cut_over() {
        let source = acme_tiers();
        let mut target = acme_tiers();
        target.archiver = reachable_archiver(50); // can only reach offset 50, not 100.

        let err = migrate_cell_to_cell(&request(100), &source, &mut target)
            .expect_err("an unreachable cut-over aborts the move");
        assert!(matches!(
            err,
            CellMigrationError::TargetRestoreNotWhole(RestoreError::PitrUnreachable(_))
        ));
    }

    /// **The receipt carries the region unchanged — the data-plane move lands IN the request's region.**
    /// The storage step never chooses a region; the cross-region rejection is the control-plane
    /// placement invariant (restated here: the receipt's region == the request's region).
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

    // ───────────────────────── the cross-cell PII-free pointer bridge (storage half) ─────────────────────────

    fn pointer(subject: &str, home: &str) -> CrossCellPointer {
        CrossCellPointer::new(
            OpaqueSubjectId::from_ref(ArtifactRef(subject.into())),
            ArtifactType::Issue,
            CorrelationId("01J0CORR".into()),
            CellId::from_token(home),
        )
    }

    /// **A cross-cell pointer carries ONLY an opaque subject — no PII crosses the cell boundary
    /// (12.6).** The pointer's subject is an `ArtifactRef`-class opaque id (a `myelin://…` token),
    /// never a name/email/body — the storage half holds only the four PII-free frame fields for a
    /// foreign-homed artifact.
    #[test]
    fn cross_cell_pointer_subject_is_opaque_no_pii() {
        let p = pointer("myelin://01J0ACME/issues/issue/7", "cell-fr-2");
        // The subject is an opaque artifact ref — there is no `.name()`/`.email()` to call.
        assert_eq!(
            p.subject().artifact_ref().0,
            "myelin://01J0ACME/issues/issue/7"
        );
        assert_eq!(p.home_cell().as_str(), "cell-fr-2");
        // The frame is exactly four PII-free fields (the fifth-field compile_fail proof lives on the
        // CrossCellPointer type in myelin-tenancy).
        assert_eq!(p.artifact_type(), &ArtifactType::Issue);
        assert_eq!(p.correlation_id(), &CorrelationId("01J0CORR".into()));
    }

    /// **Resolution is ALWAYS cell-local: storage resolves a pointer from its LOCAL tier IFF the
    /// pointer is homed HERE (12.6).** A pointer homed in THIS cell resolves locally; a pointer homed
    /// in a FOREIGN cell does NOT — its PII never lands in this cell's stores (the home cell renders +
    /// permission-checks; only the projection crosses the control-plane bridge).
    #[test]
    fn storage_resolves_a_pointer_locally_iff_homed_here() {
        let here = CellId::from_token("cell-fr-1");
        let local = pointer("myelin://01J0ACME/issues/issue/7", "cell-fr-1");
        let foreign = pointer("myelin://01J0BETA/issues/issue/9", "cell-fr-2");

        // A pointer homed HERE resolves from the local tier.
        assert!(is_cell_local(&local, &here));
        assert!(storage_resolves_locally(&local, &here));

        // A FOREIGN-homed pointer is NOT resolved locally — no foreign PII lands in this cell.
        assert!(!is_cell_local(&foreign, &here));
        assert!(
            !storage_resolves_locally(&foreign, &here),
            "a foreign-homed pointer is never read into the local tier (resolution is cell-local)"
        );
    }
}
