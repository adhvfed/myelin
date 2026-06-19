//! # `restore(to_offset T)` to the cross-seam consistency point + reindex-from-source rebuild
//!
//! **Prompt:** P-ST-12 → global **P-060** (M1). **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/storage.md` §7.3 (the cross-seam consistency
//! point = the per-aggregate outbox `seq` / event-log offset — *restore-to-consistent-point(T):
//! PITR-restore OLTP to the WAL position whose outbox rows have `seq ≤ T`; verify every
//! `ContentHash` referenced by restored rows is present (a referenced-but-missing hash = FAIL, the
//! silent-corruption case); reindex derived stores from source up to offset T (never restore them
//! from their own backups → derived == source by construction); consumers resume at T; restore
//! tenant KEKs EXCEPT any crypto-shredded since the backup*), §7.1 (the tiers restored).
//! **Contract-index:** row **11.5** (the restore + cross-seam half; the CI-wired restore-verify
//! GATE is the sibling P-ST-13 → P-061; the post-restore re-erasure is P-ST-14 → P-100). Consumed:
//! 2.3 (the outbox `seq` cursor — [`crate::coloc`]), 2.6 (reindex-from-source for the derived
//! rebuild), 11.3 (the KMS, for the KEK restore-except-crypto-shredded rule — [`crate::kms`]).
//!
//! ## The doctrine this module sequences around (EI-01 §2 / §3, EI-04 §5)
//! Silent data loss is THE Tier-1 floor (EI-01 §2 — it outranks every feature). The two ways a
//! restore loses data SILENTLY are (1) a restored row that references a blob the restore did not
//! bring back (the `ContentHash`-missing / silent-corruption case), and (2) a derived store
//! restored from its OWN backup, which drifts from the source and resurrects/loses rows that the
//! source does not have. This module makes BOTH loud:
//! - a referenced-but-missing `ContentHash` is a hard [`RestoreError::DanglingBlobRef`] — the
//!   restore FAILs, it does **not** silently pass (the §7.3 silent-corruption case);
//! - derived stores are **reindexed FROM SOURCE** (the durable event log [`SourceLog`]) up to the
//!   restored offset T, never read from a derived-store backup — *derived == source by
//!   construction, no drift* (EI-04 §5: "never restore a derived store from its own backup").
//!
//! ## The cross-seam consistency point (§7.3 — the load-bearing idea)
//! The cross-seam linearisation cursor is the per-aggregate outbox `seq` / event-log offset
//! [`crate::coloc::ColocatedTx`] establishes (the outbox row commits in the SAME OLTP tx as the
//! state change, so **OLTP commit order == event order == WAL order**). [`restore_to_offset`]
//! lands ALL the tiers at ONE such offset T:
//! 1. **OLTP (T1)** — PITR to the WAL position whose outbox rows have `seq ≤ T`: every restored row
//!    was written at-or-before T (a row past T is dropped — the restore does not hold data beyond
//!    the consistency point).
//! 2. **Object/blob (T2)** — every `ContentHash` a restored row references MUST be present
//!    ([`BlobPresence`]); a missing one is the [`RestoreError::DanglingBlobRef`] FAIL.
//! 3. **Derived stores (T4/Search/Refs/…)** — reindexed FROM SOURCE to offset T through the live
//!    consumer path (the [`ReindexFromSource`] replay), never restored from their own backups.
//!    Consumers resume at T.
//! 4. **KMS (T5)** — tenant KEKs restored from the backup snapshot, EXCEPT any crypto-shredded
//!    since the backup (reusing [`crate::kms::KmsEngine::backup_snapshot`], which already excludes
//!    a destroyed key — a shredded key STAYS DEAD across a restore, §7.5).
//!
//! ## What this module OWNS (new) vs what it REUSES (coherence, EI-01 §7)
//! The backup machinery ([`crate::backup::ContinuousArchiver`] — PITR reachability + base backups +
//! the archived WAL tail), the per-aggregate `seq` cross-seam cursor ([`crate::coloc`], P-016), the
//! KMS crypto-shred backup exclusion ([`crate::kms::KmsEngine::backup_snapshot`], P-058), the
//! [`crate::blob::ContentHash`] address (P-047), and the harness cross-seam ASSERTION
//! (`myelin_harness::restore::RestoredSnapshot::verify_cross_seam`, P-056) all already exist. Per
//! the coherence rule this prompt does **NOT** re-define any of them — it REUSES them: the restorer
//! drives the [`ContinuousArchiver`](crate::backup::ContinuousArchiver) to pick the PITR point, the
//! KMS snapshot to restore KEKs, and the [`ContentHash`](crate::blob::ContentHash) to verify blob
//! presence; and the drill (`tests/stor_d1_restore_consistent_point_drill.rs`) feeds this module's
//! output into the harness `verify_cross_seam` assertion (the SAME one SUB-D6 uses). What is
//! genuinely NEW here is the **`restore(to_offset T)` orchestration itself**: the PITR-point
//! selection over the cursor, the referenced-hash-presence verification, the reindex-from-source
//! derived rebuild, and the KEK restore-except-shredded — plus a typed [`RestoreReport`].
//!
//! ## DEVIATION / FLOOR — modeled restore, not a live `pg_restore` (EI-01 §1, write it down)
//! There is **no live Postgres on this floor** (the concrete `serve(AppSpec)` pool body + the real
//! `pg_basebackup`/`pg_restore`/WAL replay are the deferred floors P-S12/P-S15; see [`crate::oltp`]
//! / [`crate::backup`]). So the *mechanism* this prompt owns — *restore OLTP to the cursor point,
//! verify referenced blobs are present, reindex derived from source, restore KEKs except shredded*
//! — is modeled exactly over the abstract WAL offset (the per-aggregate `seq` cursor [`crate::coloc`]
//! establishes, §7.3). The restorer's SHAPE (pick the highest offset ≤ T that base+tail can reach;
//! drop rows past T; FAIL on a dangling blob; replay source→derived; exclude shredded KEKs) does
//! **not** change when the real `pg_restore` + WAL replay lands: that driver will *populate* the
//! restored OLTP/blob/KMS state this module's [`RestoreReport`] verifies; the cross-seam +
//! referenced-hash + reindex-from-source invariants read identically.
//!
//! ## FLOORS NAMED (the prompt's DEFINITION OF DONE)
//! - **The CI-wired restore-verify GATE (STOR-D1, the permanent gate)** that DRIVES this restore on
//!   every store-touching change is the sibling **P-ST-13 (global P-061)** — it spins a clean
//!   target, calls [`restore_to_offset`], and asserts no-loss + cross-seam + cold==live, failing CI
//!   on red. Named here; not built by this prompt.
//! - **Post-restore RE-ERASURE (STOR-D3 — the key stays destroyed across a restore)** is the
//!   sibling **P-ST-14 (global P-100)**: every restore runs a mandatory re-erasure pass against the
//!   GDPR erasure ledger (10.8) so an erasure completed AFTER the backup's PIT is re-applied. This
//!   module already restores KEKs *except crypto-shredded ones* (so a tenant-granularity shred stays
//!   dead); the per-SUBJECT re-erasure against the ledger is P-ST-14. Named, not silent.
//! - **The STOR-D8 forward-dependency:** this restore produces the production-scale RESTORED copy
//!   that online migrations rehearse lock-time against — **P-ST-21 (global P-126)** (online-migration
//!   safety on the restored prod-scale copy). Named per the DoD.
//! - **The real `pg_restore` + WAL-replay driver** is the P-S12/P-S15 floor; the restorer mechanism
//!   ships now and does not change shape when it lands.

use std::collections::{BTreeMap, BTreeSet};

use myelin_tenancy::TenantId;

use crate::backup::{ContinuousArchiver, WalOffset};
use crate::blob::ContentHash;
use crate::kms::{DekId, KmsEngine, WrappedDek};

// ───────────────────────────── the restored OLTP row + the source event ─────────────────────────────

/// One OLTP row as it exists in the WAL stream being restored. It carries the cross-seam cursor
/// offset it was last written at (the per-aggregate outbox `seq` co-committed with it, §7.3) and the
/// `ContentHash` it references in the object tier, if any. The restore keeps a row iff its
/// `written_at ≤ T` (the consistency point), and a kept row's `blob_ref` MUST resolve in the restored
/// object tier (else a [`RestoreError::DanglingBlobRef`] — the silent-corruption FAIL).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalRow {
    /// The aggregate/row id (PII-free in tests — a synthetic key; the real id is the aggregate key).
    pub id: String,
    /// The cross-seam cursor offset this row was last written at (the co-committed outbox `seq`).
    /// A row whose offset exceeds the restore target T is DROPPED (not held past the consistency
    /// point).
    pub written_at: WalOffset,
    /// The object-tier blob this row references by content address, if any. `Some(hash)` MUST be
    /// present in the restored object tier — a referenced-but-missing hash is the §7.3
    /// silent-corruption FAIL ([`RestoreError::DanglingBlobRef`]).
    pub blob_ref: Option<ContentHash>,
}

/// One durable source event in the event log (the system of record derived stores reindex FROM).
/// Carries the cross-seam offset it was committed at and the id of the row/aggregate it projects (the
/// join key a derived doc is keyed by). The [`ReindexFromSource`] replay rebuilds every derived doc
/// by replaying these up to offset T — *derived == source by construction* (EI-04 §5), never from a
/// derived-store backup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceEvent {
    /// The cross-seam offset this event was committed at (events replay in offset order).
    pub offset: WalOffset,
    /// The id of the OLTP row/aggregate this event projects into the derived store (the join key).
    pub projects_row_id: String,
}

/// The durable event log — the system-of-record stream derived stores are rebuilt FROM (the only
/// rebuild path, EI-04 §5). On the real floor this is the Bus's `*.snapshot` re-emit + the live
/// event stream (contract 2.6); modeled here as the ordered events the restore replays.
#[derive(Clone, Debug, Default)]
pub struct SourceLog {
    events: Vec<SourceEvent>,
}

impl SourceLog {
    /// An empty source log.
    pub fn new() -> SourceLog {
        SourceLog::default()
    }

    /// Append a source event committed at `offset` that projects `row_id` into the derived store.
    /// Events must be appended in non-decreasing offset order (the durable log is forward-only).
    pub fn append(&mut self, offset: WalOffset, row_id: impl Into<String>) -> &mut Self {
        self.events.push(SourceEvent {
            offset,
            projects_row_id: row_id.into(),
        });
        self
    }

    /// The source events up to and including offset `t` (the slice the derived reindex replays).
    pub fn events_through(&self, t: WalOffset) -> impl Iterator<Item = &SourceEvent> {
        self.events.iter().filter(move |e| e.offset <= t)
    }
}

// ───────────────────────────── the restored derived store (reindex-from-source) ─────────────────────────────

/// A derived store rebuilt by [`ReindexFromSource::reindex`] — it holds the docs replayed from the
/// [`SourceLog`] up to offset T, each keyed by the source row it projects. Because it is built ONLY
/// from the source (never from a derived backup), it is *equal to source by construction*: a doc
/// exists iff a source event ≤ T projects it. This is the structural assertion EI-04 §5 requires —
/// there is no backup-restore code path for a derived store (a [`ReindexFromSource`] is the ONLY way
/// to obtain one).
#[derive(Clone, Debug, Default)]
pub struct ReindexFromSource {
    /// The reindexed derived docs, keyed by the source row id they project (a set — replaying the
    /// same projection twice is idempotent, the dedup the live consumer template gives).
    docs: BTreeSet<String>,
    /// The offset the reindex resumed consumers AT (== T — consumers resume at the restored point).
    resumed_at: WalOffset,
}

impl ReindexFromSource {
    /// **Reindex a derived store FROM SOURCE up to offset `t`** (the §7.3 / EI-04 §5 rebuild — the
    /// ONLY rebuild path for a derived store). Replays the [`SourceLog`] events `≤ t` through the
    /// live consumer projection (here: project `row_id` → a derived doc) and resumes consumers at
    /// `t`. Idempotent: replaying twice yields the same doc set (the consumer-template dedup).
    ///
    /// There is deliberately NO constructor that reads a derived-store backup — a derived store can
    /// ONLY be obtained this way, so it is *equal to source by construction* (no drift, EI-04 §5).
    pub fn reindex(source: &SourceLog, t: WalOffset) -> ReindexFromSource {
        let docs = source
            .events_through(t)
            .map(|e| e.projects_row_id.clone())
            .collect();
        ReindexFromSource {
            docs,
            resumed_at: t,
        }
    }

    /// The reindexed derived docs (each keyed by the source row it projects).
    pub fn docs(&self) -> &BTreeSet<String> {
        &self.docs
    }

    /// `true` iff a derived doc projecting `row_id` was reindexed.
    pub fn has_doc(&self, row_id: &str) -> bool {
        self.docs.contains(row_id)
    }

    /// The offset consumers resumed at (== the restored consistency point T).
    pub fn resumed_at(&self) -> WalOffset {
        self.resumed_at
    }

    /// The number of reindexed derived docs.
    pub fn doc_count(&self) -> usize {
        self.docs.len()
    }
}

// ───────────────────────────── the object-tier presence oracle ─────────────────────────────

/// The set of `ContentHash`es present in the restored object tier (T2) — the oracle the restore uses
/// to verify every restored row's `blob_ref` resolves. On the real floor this is the restored T2
/// content store (the versioned, in-region-replicated object tier, [`crate::backup::ObjectTierBackup`]);
/// modeled here as the set of present addresses so the referenced-hash-presence check is exact.
#[derive(Clone, Debug, Default)]
pub struct BlobPresence {
    present: BTreeSet<ContentHash>,
}

impl BlobPresence {
    /// An empty object tier (nothing restored yet).
    pub fn new() -> BlobPresence {
        BlobPresence::default()
    }

    /// Record that the blob addressed by `hash` is present in the restored object tier.
    pub fn insert(&mut self, hash: ContentHash) -> &mut Self {
        self.present.insert(hash);
        self
    }

    /// `true` iff `hash` is present in the restored object tier.
    pub fn contains(&self, hash: &ContentHash) -> bool {
        self.present.contains(hash)
    }

    /// The number of present blobs.
    pub fn len(&self) -> usize {
        self.present.len()
    }

    /// `true` iff the object tier holds no blobs.
    pub fn is_empty(&self) -> bool {
        self.present.is_empty()
    }
}

// ───────────────────────────── the restore error ─────────────────────────────

/// An error from a `restore(to_offset T)`. Each names EXACTLY what is wrong (observability is part
/// of the pass condition, EI-01 §3) so a failed restore points at the precise dangling ref / target,
/// never a bare "restore failed". A restore returning `Err` is a HARD FAIL — never a silent partial.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RestoreError {
    /// The PITR target offset T is unreachable from the backups (no covering base backup + archived
    /// WAL tail reaches it). The restore CANNOT land at a consistent point — a loud failure, never a
    /// silent partial restore. Carries the [`crate::backup::BackupError`] reachability detail.
    PitrUnreachable(crate::backup::BackupError),
    /// **A restored OLTP row references a `ContentHash` ABSENT from the restored object tier** — the
    /// §7.3 silent-corruption case. The restore FAILs loudly (it does NOT silently pass): a row
    /// pointing at a missing blob is exactly the data-loss shape a sloppy restore produces. This is
    /// the mandatory-core branch (the silent-data-loss floor — the highest bar).
    DanglingBlobRef {
        /// The row that references the missing blob.
        row_id: String,
        /// The content address it references, which is absent from the restored object tier.
        missing: ContentHash,
    },
}

impl core::fmt::Display for RestoreError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            RestoreError::PitrUnreachable(e) => {
                write!(f, "restore target unreachable from backups: {e}")
            }
            RestoreError::DanglingBlobRef { row_id, missing } => write!(
                f,
                "DANGLING BLOB REF: restored row {row_id} references content {} which is ABSENT \
                 from the restored object tier — the restore FAILS (the §7.3 silent-corruption \
                 case), it does not silently pass",
                missing.to_multihash_string()
            ),
        }
    }
}

impl std::error::Error for RestoreError {}

// ───────────────────────────── the restore report ─────────────────────────────

/// The typed outcome of a successful `restore(to_offset T)` — every tier landed at ONE consistent
/// cross-seam point T. Never a bare `bool`: it carries the restored offset (asserted `== T`), the
/// restored OLTP rows (all `≤ T`), the reindexed-from-source derived store, the restored KEK set
/// (crypto-shredded EXCLUDED), and the dangling-ref count (`== 0`, since a dangling ref is a hard
/// FAIL — present here as `0` so the drill telemetry asserts it). A [`RestoreReport`] only exists for
/// a restore that PASSED the referenced-hash-presence check (a dangling ref is `Err`, never a report).
#[derive(Clone, Debug)]
pub struct RestoreReport {
    /// The consistency point every tier was restored to (== the requested T). Telemetry:
    /// `restore_consistent_point_offset == T`.
    pub restored_to_offset: WalOffset,
    /// The restored OLTP rows — every one written at-or-before T (a row past T was dropped; not held
    /// past the consistency point).
    pub oltp_rows: Vec<WalRow>,
    /// The derived store rebuilt FROM SOURCE up to T (never from its own backup → derived == source).
    pub derived: ReindexFromSource,
    /// The restored tenant KEKs (wrapped DEKs), crypto-shredded keys EXCLUDED (a shredded key stays
    /// dead across the restore, §7.5).
    pub restored_keys: Vec<(DekId, WrappedDek)>,
    /// The number of dangling blob refs found. Always `0` in a successful report (a dangling ref is
    /// a hard [`RestoreError::DanglingBlobRef`]); emitted as the `dangling_ref_count == 0` signal.
    pub dangling_ref_count: u64,
}

impl RestoreReport {
    /// `true` iff a key for `tenant` was restored (used by the crypto-shred-exclusion assertion: a
    /// shredded tenant has NO restored key, so this is `false` — it stays dead across the restore).
    pub fn restored_key_for_tenant(&self, tenant: &TenantId) -> bool {
        self.restored_keys.iter().any(|(id, _)| &id.tenant == tenant)
    }
}

// ───────────────────────────── restore(to_offset T) ─────────────────────────────

/// **`restore(to_offset T)` to the cross-seam consistency point (§7.3 — the headline).** Lands every
/// tier at ONE consistent point T (the per-aggregate outbox `seq` / event-log offset, the cross-seam
/// cursor [`crate::coloc`] establishes):
///
/// 1. **PITR-reachability** — the target T must be reachable from the backups (a covering base backup
///    whose archived WAL tail reaches it); else [`RestoreError::PitrUnreachable`] (a loud failure,
///    never a silent partial). Reuses [`ContinuousArchiver::pitr_reachable`].
/// 2. **OLTP (T1)** — restore the rows whose cursor offset is `≤ T` (PITR to the WAL position whose
///    outbox rows have `seq ≤ T`). A row past T is DROPPED (not held past the consistency point).
/// 3. **Object/blob (T2)** — every restored row's `blob_ref` MUST be present in `blobs`; a missing
///    one is the §7.3 silent-corruption FAIL [`RestoreError::DanglingBlobRef`] (NOT a silent pass).
/// 4. **Derived (T4/Search/Refs/…)** — reindexed FROM SOURCE up to T via [`ReindexFromSource::reindex`]
///    (the ONLY rebuild path; consumers resume at T — never restored from a derived backup).
/// 5. **KMS (T5)** — tenant KEKs restored from `kms.backup_snapshot()`, which already EXCLUDES
///    crypto-shredded keys (§7.5 — a shredded key stays dead across the restore).
///
/// Returns a [`RestoreReport`] (every tier consistent at T, `dangling_ref_count == 0`) on success, or
/// the first hard [`RestoreError`] (the silent-data-loss floor: a dangling ref FAILs the whole
/// restore — it never silently passes).
///
/// `archiver` selects the PITR point (reachability); `rows` are the WAL rows being restored; `blobs`
/// is the restored object-tier presence oracle; `source` is the durable event log derived stores
/// reindex FROM; `kms` is the engine whose backup snapshot (crypto-shredded keys excluded) restores
/// the KEKs.
pub fn restore_to_offset(
    archiver: &ContinuousArchiver,
    target: WalOffset,
    rows: &[WalRow],
    blobs: &BlobPresence,
    source: &SourceLog,
    kms: &KmsEngine,
) -> Result<RestoreReport, RestoreError> {
    // (1) PITR-reachability: the target must be recoverable from base + archived WAL tail. A target
    // outside the recoverable range is a LOUD failure, never a silent partial restore (§7.1).
    archiver
        .pitr_reachable(target)
        .map_err(RestoreError::PitrUnreachable)?;

    // (2) OLTP: restore only the rows whose cross-seam cursor offset is ≤ T. A row written PAST T is
    // dropped — the restore does not hold data beyond the consistency point (no forward-inconsistency).
    let restored_rows: Vec<WalRow> = rows
        .iter()
        .filter(|r| r.written_at <= target)
        .cloned()
        .collect();

    // (3) Object/blob: every restored row's referenced ContentHash MUST be present in the restored
    // object tier. A referenced-but-missing hash is the §7.3 silent-corruption case — the restore
    // FAILS HARD here (the silent-data-loss floor — never a silent pass). We surface the FIRST
    // dangling ref; the drill (and P-061's gate) assert `dangling_ref_count == 0`.
    for row in &restored_rows {
        if let Some(hash) = &row.blob_ref {
            if !blobs.contains(hash) {
                return Err(RestoreError::DanglingBlobRef {
                    row_id: row.id.clone(),
                    missing: hash.clone(),
                });
            }
        }
    }

    // (4) Derived stores: rebuilt FROM SOURCE up to T (the ONLY rebuild path — never from a derived
    // backup → derived == source by construction, EI-04 §5). Consumers resume at T.
    let derived = ReindexFromSource::reindex(source, target);

    // (5) KMS: restore the tenant KEKs from the backup snapshot, which ALREADY excludes crypto-shredded
    // keys (§7.5 — a shredded key must stay dead across a restore). We do NOT re-introduce a shredded
    // key here; the snapshot is the authoritative restore set.
    let restored_keys = kms.backup_snapshot();

    Ok(RestoreReport {
        restored_to_offset: target,
        oltp_rows: restored_rows,
        derived,
        restored_keys,
        // A successful restore has ZERO dangling refs by construction (a dangling ref returned Err
        // above). Emitted as the `dangling_ref_count == 0` telemetry the drill asserts.
        dangling_ref_count: 0,
    })
}

/// A small convenience over [`restore_to_offset`]: the per-tenant count of restored KEKs (for the
/// crypto-shred-exclusion telemetry). Crypto-shredded tenants contribute `0`.
pub fn restored_key_counts(report: &RestoreReport) -> BTreeMap<TenantId, usize> {
    let mut counts: BTreeMap<TenantId, usize> = BTreeMap::new();
    for (id, _) in &report.restored_keys {
        *counts.entry(id.tenant.clone()).or_insert(0) += 1;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::WalSegment;
    use crate::kms::{KekId, KeyClass};
    use myelin_tenancy::Region;

    /// A region helper (the per-`(tenant, region)` KEK grain).
    fn region_eu() -> Region {
        Region("eu-west".into())
    }

    fn h(s: &str) -> ContentHash {
        ContentHash::blake3(s.as_bytes())
    }

    fn tenant(s: &str) -> TenantId {
        TenantId(s.into())
    }

    /// An archiver whose base + WAL tail makes every offset in `0..=tail` reachable (a base at 0,
    /// the tail archived to `tail`).
    fn reachable_archiver(tail: WalOffset) -> ContinuousArchiver {
        let mut arch = ContinuousArchiver::new();
        arch.archive_segment(WalSegment { end_offset: 0, committed_at: 0 }).unwrap();
        arch.take_base_backup(1); // base anchor at offset 0
        arch.archive_segment(WalSegment { end_offset: tail, committed_at: 10 }).unwrap();
        arch
    }

    // ───────── the cross-seam consistency point (the PITR cursor) ─────────

    /// **The restore lands OLTP at the WAL position whose outbox seq ≤ T** (the §7.3 cursor). Rows
    /// at-or-before T are restored; a row PAST T is dropped (not held beyond the consistency point).
    /// Kills the mutant that flips `<=` to `<` or keeps past-offset rows.
    #[test]
    fn restore_lands_oltp_at_the_seq_le_t_cursor() {
        let arch = reachable_archiver(200);
        let blobs = BlobPresence::new();
        let source = SourceLog::new();
        let kms = KmsEngine::new();
        let rows = vec![
            WalRow { id: "r1".into(), written_at: 90, blob_ref: None },
            WalRow { id: "r2".into(), written_at: 100, blob_ref: None }, // == T, kept
            WalRow { id: "r3".into(), written_at: 140, blob_ref: None }, // > T, DROPPED
        ];
        let report = restore_to_offset(&arch, 100, &rows, &blobs, &source, &kms).unwrap();

        assert_eq!(report.restored_to_offset, 100, "restored to the consistency point T");
        let ids: Vec<&str> = report.oltp_rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["r1", "r2"], "rows ≤ T restored; the row past T dropped");
        assert!(
            report.oltp_rows.iter().all(|r| r.written_at <= 100),
            "no restored row may be past the consistency point"
        );
    }

    // ───────── the referenced-hash-presence check (the silent-data-loss floor — mandatory-core) ─────────

    /// **A referenced-but-PRESENT blob restores cleanly (0 dangling refs).** The happy path: every
    /// restored row's `ContentHash` is present in the restored object tier.
    #[test]
    fn a_present_referenced_blob_restores_clean() {
        let arch = reachable_archiver(200);
        let mut blobs = BlobPresence::new();
        blobs.insert(h("blob-a")).insert(h("blob-b"));
        let source = SourceLog::new();
        let kms = KmsEngine::new();
        let rows = vec![
            WalRow { id: "r1".into(), written_at: 90, blob_ref: Some(h("blob-a")) },
            WalRow { id: "r2".into(), written_at: 100, blob_ref: Some(h("blob-b")) },
        ];
        let report = restore_to_offset(&arch, 100, &rows, &blobs, &source, &kms).unwrap();
        assert_eq!(report.dangling_ref_count, 0, "every referenced blob is present → 0 dangling");
    }

    /// **MANDATORY-CORE: a referenced-but-MISSING ContentHash makes the restore FAIL (not silently
    /// pass)** — the §7.3 silent-corruption case, the highest-bar silent-data-loss floor. The unit
    /// test the prompt names. Kills any mutant that drops the presence check or swallows the failure.
    #[test]
    fn a_missing_referenced_hash_makes_restore_fail() {
        let arch = reachable_archiver(200);
        let mut blobs = BlobPresence::new();
        blobs.insert(h("blob-a")); // blob-b is NOT restored — the injected silent-corruption case
        let source = SourceLog::new();
        let kms = KmsEngine::new();
        let rows = vec![
            WalRow { id: "r1".into(), written_at: 90, blob_ref: Some(h("blob-a")) },
            WalRow { id: "r2".into(), written_at: 95, blob_ref: Some(h("blob-b")) }, // MISSING
        ];
        let err = restore_to_offset(&arch, 100, &rows, &blobs, &source, &kms)
            .expect_err("a row → missing blob MUST make the restore FAIL, not pass silently");
        assert_eq!(
            err,
            RestoreError::DanglingBlobRef { row_id: "r2".into(), missing: h("blob-b") }
        );
        // The error is loud + specific (observability is part of the pass).
        let m = err.to_string();
        assert!(m.contains("DANGLING BLOB REF"), "must name the dangling-ref case: {m}");
        assert!(m.contains("r2"), "must name the offending row: {m}");
    }

    /// A blob referenced ONLY by a row PAST the consistency point does NOT cause a failure — that row
    /// was dropped, so its (absent) blob is irrelevant. The presence check applies only to RESTORED
    /// rows (≤ T). Kills the mutant that checks blobs before the offset filter.
    #[test]
    fn a_dropped_rows_missing_blob_does_not_fail_the_restore() {
        let arch = reachable_archiver(200);
        let blobs = BlobPresence::new(); // empty — no blobs restored
        let source = SourceLog::new();
        let kms = KmsEngine::new();
        let rows = vec![
            WalRow { id: "kept".into(), written_at: 90, blob_ref: None },
            // r-future is PAST T=100 and references a missing blob — but it is dropped, so OK.
            WalRow { id: "r-future".into(), written_at: 150, blob_ref: Some(h("gone")) },
        ];
        let report = restore_to_offset(&arch, 100, &rows, &blobs, &source, &kms).unwrap();
        assert_eq!(report.oltp_rows.len(), 1, "only the kept row is restored");
        assert_eq!(report.dangling_ref_count, 0);
    }

    // ───────── reindex-from-source (derived == source by construction, EI-04 §5) ─────────

    /// **A derived store is rebuilt FROM SOURCE up to T — never from its own backup.** The reindex
    /// replays the source log ≤ T and resumes consumers at T; a source event PAST T is not replayed.
    /// There is NO backup-restore path for a derived store (the only constructor is `reindex`).
    #[test]
    fn derived_stores_rebuild_from_source_not_a_backup() {
        let arch = reachable_archiver(200);
        let blobs = BlobPresence::new();
        let mut source = SourceLog::new();
        source
            .append(50, "r1")
            .append(90, "r2")
            .append(100, "r3")
            .append(140, "r-future"); // PAST T=100 — must NOT be reindexed
        let kms = KmsEngine::new();

        let report = restore_to_offset(&arch, 100, &[], &blobs, &source, &kms).unwrap();
        let derived = &report.derived;
        assert!(derived.has_doc("r1") && derived.has_doc("r2") && derived.has_doc("r3"));
        assert!(
            !derived.has_doc("r-future"),
            "a source event past T must NOT be reindexed (consumers resume at T)"
        );
        assert_eq!(derived.resumed_at(), 100, "consumers resume at the restored point T");
        assert_eq!(derived.doc_count(), 3, "derived == source replayed to T, by construction");
    }

    /// Reindex-from-source is IDEMPOTENT — replaying the same source twice yields the same derived
    /// doc set (the live-consumer-template dedup). Kills a mutant that double-counts a replayed doc.
    #[test]
    fn reindex_from_source_is_idempotent() {
        let mut source = SourceLog::new();
        source.append(10, "dup").append(20, "dup").append(30, "other");
        let a = ReindexFromSource::reindex(&source, 100);
        let b = ReindexFromSource::reindex(&source, 100);
        assert_eq!(a.docs(), b.docs(), "reindex is deterministic + idempotent");
        assert_eq!(a.doc_count(), 2, "the duplicated projection collapses to one doc");
    }

    // ───────── the KEK restore-except-crypto-shredded rule (§7.5) ─────────

    /// **A crypto-shredded KEK is NOT restored** (§7.5 — it stays dead across the restore). A live
    /// tenant's key restores; a tenant whose KEK was destroyed since the backup has NO restored key.
    /// The unit test the prompt names. Reuses the KMS backup_snapshot exclusion (P-058).
    #[test]
    fn a_crypto_shredded_kek_is_not_restored() {
        let arch = reachable_archiver(200);
        let blobs = BlobPresence::new();
        let source = SourceLog::new();
        let kms = KmsEngine::new();

        let live = tenant("live");
        let shredded = tenant("shredded");
        let live_kek = KekId::new(live.clone(), region_eu());
        let shredded_kek = KekId::new(shredded.clone(), region_eu());
        kms.ensure_kek(&live_kek);
        kms.ensure_kek(&shredded_kek);
        kms.ensure_dek(&live, &region_eu(), KeyClass::Tenant).unwrap();
        kms.ensure_dek(&shredded, &region_eu(), KeyClass::Tenant).unwrap();

        // Crypto-shred the second tenant (destroy its KEK — the offboard / erasure lever).
        assert!(kms.destroy_kek(&shredded_kek));

        let report = restore_to_offset(&arch, 100, &[], &blobs, &source, &kms).unwrap();
        assert!(
            report.restored_key_for_tenant(&live),
            "a LIVE tenant's KEK must be restored"
        );
        assert!(
            !report.restored_key_for_tenant(&shredded),
            "a CRYPTO-SHREDDED tenant's KEK must NOT be restored — it stays dead across the restore (§7.5)"
        );
        let counts = restored_key_counts(&report);
        assert_eq!(counts.get(&shredded), None, "the shredded tenant contributes 0 restored keys");
        assert_eq!(counts.get(&live).copied(), Some(1));
    }

    // ───────── PITR reachability (a loud failure, never a silent partial) ─────────

    /// **An unreachable PITR target is a LOUD failure, never a silent partial restore.** A target
    /// past the archived WAL tail (or before any base backup) returns [`RestoreError::PitrUnreachable`].
    #[test]
    fn an_unreachable_target_fails_loudly() {
        let arch = reachable_archiver(100); // tail reaches only offset 100
        let blobs = BlobPresence::new();
        let source = SourceLog::new();
        let kms = KmsEngine::new();
        // Target 500 is past the archived tail (100) — unreachable.
        let err = restore_to_offset(&arch, 500, &[], &blobs, &source, &kms)
            .expect_err("a target past the WAL tail must fail loudly, never a silent partial");
        assert!(matches!(err, RestoreError::PitrUnreachable(_)));
        assert!(!err.to_string().is_empty(), "the failure is observable");
    }

    /// The whole restore lands at ONE consistent point: OLTP ≤ T, every referenced blob present,
    /// derived == source-replay to T, KEKs restored-except-shredded, 0 dangling — all simultaneously.
    /// The end-to-end happy path the drill asserts cross-seam-consistent.
    #[test]
    fn the_whole_restore_lands_at_one_consistent_point() {
        let arch = reachable_archiver(300);
        let mut blobs = BlobPresence::new();
        blobs.insert(h("a")).insert(h("b"));
        let mut source = SourceLog::new();
        source.append(90, "r1").append(100, "r2");
        let kms = KmsEngine::new();
        let t = tenant("acme");
        kms.ensure_kek(&KekId::new(t.clone(), region_eu()));
        kms.ensure_dek(&t, &region_eu(), KeyClass::Tenant).unwrap();

        let rows = vec![
            WalRow { id: "r1".into(), written_at: 90, blob_ref: Some(h("a")) },
            WalRow { id: "r2".into(), written_at: 100, blob_ref: Some(h("b")) },
            WalRow { id: "r3".into(), written_at: 250, blob_ref: None }, // past T → dropped
        ];
        let report = restore_to_offset(&arch, 100, &rows, &blobs, &source, &kms).unwrap();

        assert_eq!(report.restored_to_offset, 100);
        assert_eq!(report.oltp_rows.len(), 2, "rows ≤ T");
        assert_eq!(report.dangling_ref_count, 0, "every referenced blob present");
        assert!(report.derived.has_doc("r1") && report.derived.has_doc("r2"));
        assert_eq!(report.derived.resumed_at(), 100);
        assert!(report.restored_key_for_tenant(&t), "the live tenant's KEK is restored");
    }
}
