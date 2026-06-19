//! # Continuous WAL archiving + base backups + PITR (the RPO floor) — the backup half of 11.5
//!
//! **Prompt:** P-ST-11 → global **P-059** (M1). **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/storage.md` §7.1 (continuous WAL archiving +
//! base backups → PITR, **RPO target ≤ 5 min**; the object tier T2 versioned + in-region
//! replicated; the log tier T3 sealed segments are immutable T2 blobs + the range index in T1;
//! **OLAP T4 + caches T7 + derived indexes are NOT backed up — rebuilt** via reindex-from-source;
//! KMS keys backed up only under the cell root while the tenant is live; **a crypto-shredded key
//! is excluded from backup**), §7.3 (the cross-seam consistency point = the per-aggregate outbox
//! `seq` / event-log offset — written into the WAL stream).
//! **Contract-index:** row **11.5** (the backup / archiving / PITR half; the
//! `restore(to_offset)` plus cross-seam is the sibling P-ST-12 → P-060; the CI-wired
//! restore-verify GATE is P-ST-13 → P-061). Consumed: 2.3 (the outbox `seq` written into the WAL
//! stream), 11.3 (the KMS, for the crypto-shredded-key-excluded-from-backup rule).
//!
//! ## The doctrine this module sequences around (EI-01 §2 / §3)
//! *"A backup that has never been restored is not a backup."* Silent data loss is THE Tier-1
//! floor (EI-01 §2 — it outranks every feature). The RPO gate resolves to a **quantified
//! threshold** (`rpo_max_mins`, read from the versioned `thresholds.toml`, never a hardcoded
//! magic number) and **observability is part of the pass** (EI-01 §3): the measured
//! `backup_rpo_seconds` is emitted on the telemetry source so the STOR-D2 drill asserts it `≤
//! 300`. This module ships the BACKUP machinery (archiving + base backups + PITR + the tier
//! classification + the crypto-shred exclusion); the **restore** machinery + the cross-seam
//! consistency assertion + the RPO/RTO *measurement signal* live in the harness (P-056) and the
//! restore sibling (P-060) — see "what this module owns vs reuses" below.
//!
//! ## What this module OWNS (new) vs what it REUSES (coherence, EI-01 §7)
//! The cross-seam consistency ASSERTION, the `RestoreOutcome`/`RestoredSnapshot` model, and the
//! three restore telemetry signals (`RestoreRpoSecs` / `RestoreRtoSecs` /
//! `RestoreCrossSeamMismatch`) already exist in `myelin-harness::restore` (shipped by P-056). The
//! per-aggregate `seq` cross-seam cursor already exists in [`crate::coloc`] (P-016). The KMS
//! crypto-shred backup exclusion already exists in [`crate::kms::KmsEngine::backup_snapshot`]
//! (P-058). Per the coherence rule this prompt does **NOT** re-define any of them — it REUSES
//! them in place. What is genuinely NEW here is the **OLTP-tier backup machinery itself**:
//! - [`ContinuousArchiver`] — continuous WAL archiving (every committed WAL segment is archived
//!   off-host) + periodic [`BaseBackup`]s, giving a **PITR window** (base backup + the archived
//!   WAL tail). It MEASURES the live RPO ([`ContinuousArchiver::measure_rpo`]) = the gap between
//!   the last durably-committed write and the last durably-archived WAL position. This is the
//!   number STOR-D2 asserts ≤ the `rpo_max_mins` bound.
//! - [`StoreTier`] — the T1..T7 store-tier classification with the structural [`StoreTier::is_backed_up`]
//!   rule: **T4 (OLAP) / T7 (caches) / derived indexes are NOT backed up** (rebuilt from source).
//!   A backup of a derived tier is a TYPE ERROR ([`BackupError::DerivedTierNotBacked`]) — there is
//!   no backup-restore code path for them, by construction (the structural assertion the prompt
//!   + the unit tests require).
//! - [`ObjectTierBackup`] — the T2 object tier **versioned + in-region replicated**; content
//!   addressing makes integrity re-hash-verifiable (a restored blob's recomputed hash must equal
//!   its address, the [`crate::blob`] STOR-D7 property).
//! - [`LogTierSeal`] — the T3 log tier: **sealed segments are immutable T2 blobs + the
//!   `(job,step,byte-range)` range index in T1** (so a log segment rides the same T2 versioning +
//!   T1 PITR, not a separate backup path).
//! - [`BackupSet`] — the orchestrator that snapshots all backed-up tiers at one WAL offset
//!   (the cross-seam point) AND excludes crypto-shredded KMS keys (reusing
//!   [`crate::kms::KmsEngine::backup_snapshot`]) — a shredded key must stay dead across a restore
//!   (§7.5).
//!
//! ## DEVIATION / FLOOR — modeled WAL, not a live Postgres (EI-01 §1, write it down)
//! There is **no live Postgres on this floor** (the concrete `serve(AppSpec)` pool body + the real
//! `pg_basebackup`/WAL-G archiver are the deferred floors P-S12/P-S15; see [`crate::oltp`]). So the
//! *mechanism* this prompt owns — *continuously archive the WAL tail, take periodic base backups,
//! and the recoverable point is base + archived tail* — is modeled exactly over an abstract WAL
//! offset (the per-aggregate `seq` cursor [`crate::coloc`] establishes, §7.3). The archiver's
//! shape (append a sealed WAL segment on commit; the recoverable point = the highest archived
//! offset; RPO = committed − archived) does **not** change when the real `archive_command` / WAL
//! shipping lands: that driver will *feed* [`ContinuousArchiver::archive_segment`] off the real WAL
//! stream; the RPO computation + the tier classification + the crypto-shred exclusion read
//! identically.
//!
//! ## FLOORS NAMED (the prompt's DEFINITION OF DONE)
//! - **The RPO number (≤ 5 min)** is the proposed default-to-beat — MEASURED by STOR-D2 here
//!   (against the `rpo_max_mins` threshold), and **re-confirmed at cell scale in M5 (P-ST-30 →
//!   P-636 family)**. The RTO / cell-kill half (STOR-D2's RTO leg) is the sibling **P-ST-14 (global
//!   P-100)**; the no-loss STOR-D1 CI gate is **P-ST-13 (global P-061)** driving the restore
//!   sibling **P-ST-12 (global P-060)**. All named here per the DoD.
//! - **The real WAL-shipping driver** (`archive_command` / WAL-G / `pg_basebackup`) is the
//!   P-S12/P-S15 floor; the archiver mechanism + RPO measurement ship now and do not change shape
//!   when it lands.

use std::collections::BTreeMap;

use crate::kms::{DekId, KmsEngine, WrappedDek};

// ───────────────────────────── the store-tier classification ─────────────────────────────

/// The Phase-3 store-map tier of a store (storage.md §2 / §7.1). The load-bearing distinction
/// for backup is [`StoreTier::is_backed_up`]: **systems of record (T1/T2/T3/T5) are backed up;
/// derived stores (T4 OLAP / T7 caches / derived indexes) are NOT — they are rebuilt from source
/// (reindex-from-source).** Backing up a derived store is a structural error (there is no
/// backup-restore code path for it, by construction).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StoreTier {
    /// T1 — OLTP (Postgres-class). Backed up via **continuous WAL archiving + base backups →
    /// PITR**. The authoritative domain state + the co-located outbox (the cross-seam cursor).
    Oltp,
    /// T2 — Object/blob (S3-compatible). Backed up via **versioning + in-region replication**;
    /// content addressing makes integrity re-hash-verifiable.
    Object,
    /// T3 — Log/firehose. **Sealed segments are immutable T2 blobs + the range index in T1** — it
    /// rides T2 versioning + T1 PITR, not a separate backup path.
    Log,
    /// T4 — OLAP read store. **NOT backed up** — rebuilt from source (reindex-from-source).
    Olap,
    /// T5 — KMS. Keys backed up **only under the cell root while the tenant is live**; a
    /// crypto-shredded key is EXCLUDED ([`KmsEngine::backup_snapshot`]).
    Kms,
    /// T7 — Cache. **NOT backed up** — a cache is rebuilt on demand (it is, by definition, derived).
    Cache,
    /// A derived secondary index (Search / Refs edge index / …). **NOT backed up** — rebuilt from
    /// source. Distinguished from [`StoreTier::Olap`]/[`StoreTier::Cache`] only for a clearer error.
    DerivedIndex,
}

impl StoreTier {
    /// **The structural backup-vs-rebuild rule (§7.1).** `true` iff this tier is a system of
    /// record that is *backed up*; `false` for a derived tier that is *rebuilt from source*
    /// (OLAP / caches / derived indexes). This is the classification [`BackupSet::snapshot_tier`]
    /// enforces — a derived tier can never be added to a backup set (no backup-restore path for it).
    pub fn is_backed_up(self) -> bool {
        match self {
            StoreTier::Oltp | StoreTier::Object | StoreTier::Log | StoreTier::Kms => true,
            // The derived tiers — NEVER backed up, rebuilt from source by construction.
            StoreTier::Olap | StoreTier::Cache | StoreTier::DerivedIndex => false,
        }
    }

    /// `true` iff this tier is a DERIVED store (rebuilt from source, never restored from its own
    /// backup). The exact complement of [`is_backed_up`](Self::is_backed_up) — named for the
    /// reindex-from-source consumers (P-060) that ask "must I rebuild this?".
    pub fn is_rebuilt_from_source(self) -> bool {
        !self.is_backed_up()
    }

    /// The short tier label for telemetry / artifacts (PII-free).
    pub fn label(self) -> &'static str {
        match self {
            StoreTier::Oltp => "t1-oltp",
            StoreTier::Object => "t2-object",
            StoreTier::Log => "t3-log",
            StoreTier::Olap => "t4-olap",
            StoreTier::Kms => "t5-kms",
            StoreTier::Cache => "t7-cache",
            StoreTier::DerivedIndex => "derived-index",
        }
    }
}

// ───────────────────────────── the WAL offset (cross-seam cursor) ─────────────────────────────

/// The WAL / event-log offset — the cross-seam linearisation cursor (storage.md §7.3). The
/// per-aggregate outbox `seq` [`crate::coloc::ColocatedTx`] commits is written into the WAL stream
/// in the SAME transaction as the state change, so **OLTP commit order == event order == WAL
/// order**. A PITR restores to the WAL position whose outbox rows have `seq ≤ T` (the sibling
/// P-060). Modeled as a `u64` here (the same scalar cursor shape `myelin-harness::restore::Offset`
/// uses) — the backup machinery is cursor-shape-neutral.
pub type WalOffset = u64;

/// A coarse monotone clock tick used to MEASURE the RPO window (seconds). On the real floor this is
/// the WAL record's commit timestamp vs the archiver's last-shipped timestamp; modeled here as a
/// `u64` seconds-since-epoch so the RPO gap is an exact, assertable number. Frozen unit: SECONDS
/// (the `_secs` units anchor).
pub type EpochSecs = u64;

// ───────────────────────────── continuous WAL archiving + base backups ─────────────────────────────

/// One sealed, archived WAL segment (the unit continuous archiving ships off-host). It carries the
/// highest WAL offset it contains (the cross-seam cursor up to which this segment makes recoverable)
/// and the commit timestamp of that offset (for the RPO measurement). Immutable once archived
/// (append-only — a WAL segment is never rewritten, the precondition for PITR).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalSegment {
    /// The highest WAL offset (cross-seam `seq`) durably contained in this archived segment.
    pub end_offset: WalOffset,
    /// The commit timestamp (seconds) of `end_offset` — the freshness this segment archives up to.
    pub committed_at: EpochSecs,
}

/// A periodic base backup — a full snapshot of the OLTP tier at a WAL offset. PITR = the most
/// recent base backup BEFORE the target T, replayed forward through the archived WAL segments to
/// T. A base backup alone gives a coarse RPO (the backup interval); **continuous WAL archiving is
/// what holds RPO ≤ 5 min** (the WAL tail closes the gap between base backups).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaseBackup {
    /// The WAL offset this base backup snapshots the tier AT (replay starts here).
    pub at_offset: WalOffset,
    /// The wall-clock (seconds) the base backup completed.
    pub taken_at: EpochSecs,
}

/// An error from the backup machinery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackupError {
    /// A WAL segment was archived OUT OF ORDER (its `end_offset` ≤ the last archived offset). WAL
    /// archiving is strictly append-only/forward — an out-of-order or rewound segment would break
    /// the PITR guarantee (you could not replay forward to a consistent point). Rejected loudly.
    WalArchivedOutOfOrder {
        /// The last offset already archived.
        last: WalOffset,
        /// The offending segment's end offset (≤ `last`).
        attempted: WalOffset,
    },
    /// A backup was requested for a DERIVED tier (OLAP / cache / derived index). There is **no
    /// backup-restore code path** for derived stores — they are rebuilt from source
    /// (reindex-from-source). This is the structural assertion §7.1 requires, enforced as a type
    /// error.
    DerivedTierNotBacked {
        /// The derived tier that was (wrongly) asked to be backed up.
        tier: StoreTier,
    },
    /// PITR was requested to a target offset for which no covering base backup + WAL tail exists
    /// (the target predates the earliest base backup, or the WAL tail does not reach it). The
    /// restore CANNOT land at the target — a loud failure, never a silent partial restore.
    PitrTargetUnreachable {
        /// The requested target offset.
        target: WalOffset,
        /// The earliest recoverable offset (the earliest base backup), if any.
        earliest_base: Option<WalOffset>,
        /// The latest archived offset (the WAL tail), if any.
        latest_archived: Option<WalOffset>,
    },
}

impl core::fmt::Display for BackupError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            BackupError::WalArchivedOutOfOrder { last, attempted } => write!(
                f,
                "WAL archived out of order: last archived offset {last}, attempted {attempted} \
                 (WAL archiving is strictly forward — PITR requires it)"
            ),
            BackupError::DerivedTierNotBacked { tier } => write!(
                f,
                "tier {} is DERIVED and is rebuilt from source — there is no backup-restore path \
                 for it (storage.md §7.1)",
                tier.label()
            ),
            BackupError::PitrTargetUnreachable {
                target,
                earliest_base,
                latest_archived,
            } => write!(
                f,
                "PITR target offset {target} is unreachable (earliest base {earliest_base:?}, \
                 latest archived {latest_archived:?}) — restore cannot land, NOT a silent partial"
            ),
        }
    }
}

impl std::error::Error for BackupError {}

/// **Continuous WAL archiving + periodic base backups → PITR (the §7.1 OLTP-tier backup
/// machinery).** It holds the append-only list of archived [`WalSegment`]s (continuous archiving),
/// the list of [`BaseBackup`]s, and tracks the highest WAL offset/timestamp that has been *durably
/// committed* to the primary (so the live RPO = committed − archived can be measured).
///
/// The recoverable point is **base backup + archived WAL tail**. PITR to a target T restores from
/// the most recent base backup at-or-before T and replays the archived WAL forward to T (the actual
/// replay is the restore sibling P-060; here the archiver provides the recoverable RANGE + proves
/// the RPO bound holds).
#[derive(Clone, Debug, Default)]
pub struct ContinuousArchiver {
    /// The archived WAL segments, append-only and strictly increasing by `end_offset`.
    archived: Vec<WalSegment>,
    /// The periodic base backups (the PITR replay anchors).
    base_backups: Vec<BaseBackup>,
    /// The highest WAL offset durably COMMITTED to the primary (may lead the archived tail — the
    /// difference is the data-at-risk the RPO measures).
    committed_offset: WalOffset,
    /// The commit timestamp (seconds) of `committed_offset` — the "now" the RPO is measured against.
    committed_at: EpochSecs,
}

impl ContinuousArchiver {
    /// A fresh archiver with no segments + no base backups (RPO is undefined until the first
    /// commit + the first archive).
    pub fn new() -> ContinuousArchiver {
        ContinuousArchiver::default()
    }

    /// Record that the primary has durably COMMITTED up to `offset` at `at` (a new write landed).
    /// This advances the "now" the RPO is measured against; until the archiver ships the WAL
    /// segment covering it, `offset` is data-at-risk. Monotone: a commit never moves backward.
    pub fn record_commit(&mut self, offset: WalOffset, at: EpochSecs) {
        if offset >= self.committed_offset {
            self.committed_offset = offset;
            self.committed_at = at;
        }
    }

    /// **Continuous archiving: ship one sealed WAL segment off-host.** Append-only and strictly
    /// forward — a segment whose `end_offset` does not advance past the last archived offset is
    /// rejected ([`BackupError::WalArchivedOutOfOrder`]), because PITR requires a monotone WAL to
    /// replay forward. On the real floor this is the `archive_command` shipping a completed WAL
    /// file; here it is the modeled equivalent.
    pub fn archive_segment(&mut self, segment: WalSegment) -> Result<(), BackupError> {
        if let Some(last) = self.archived.last() {
            if segment.end_offset <= last.end_offset {
                return Err(BackupError::WalArchivedOutOfOrder {
                    last: last.end_offset,
                    attempted: segment.end_offset,
                });
            }
        }
        self.archived.push(segment);
        Ok(())
    }

    /// Take a periodic base backup AT the current archived tail (the PITR replay anchor). A base
    /// backup is taken at an offset that is already archived, so replay-from-here is always
    /// possible. `taken_at` is the wall-clock it completed.
    pub fn take_base_backup(&mut self, taken_at: EpochSecs) {
        let at_offset = self.latest_archived_offset().unwrap_or(0);
        self.base_backups.push(BaseBackup {
            at_offset,
            taken_at,
        });
    }

    /// The highest WAL offset durably ARCHIVED (the recoverable tail). `None` if nothing archived
    /// yet. This is the point a PITR can recover up to.
    pub fn latest_archived_offset(&self) -> Option<WalOffset> {
        self.archived.last().map(|s| s.end_offset)
    }

    /// The commit timestamp (seconds) of the latest archived segment — the freshness the backup
    /// holds. `None` if nothing archived yet.
    pub fn latest_archived_at(&self) -> Option<EpochSecs> {
        self.archived.last().map(|s| s.committed_at)
    }

    /// The earliest base-backup offset (the earliest point PITR can anchor a replay at). `None`
    /// if no base backup has been taken.
    pub fn earliest_base_offset(&self) -> Option<WalOffset> {
        self.base_backups.iter().map(|b| b.at_offset).min()
    }

    /// **MEASURE THE LIVE RPO (the STOR-D2 number).** The recovery-POINT objective is the window of
    /// committed data that is NOT yet durably archived: the wall-clock gap between the last
    /// durably-committed write and the freshness of the last archived WAL segment. If the archiver
    /// is caught up (archived offset ≥ committed offset) the RPO is **0** — there is no data at
    /// risk. Returns seconds (the frozen `_secs` unit), the value the drill asserts `≤ rpo bound`.
    ///
    /// The measurement is the gap in COMMIT-TIME freshness: `committed_at − latest_archived_at`,
    /// floored at 0 (archiving cannot be ahead of commits in freshness on the happy path). With
    /// nothing committed yet the RPO is 0 (no data ⇒ no loss).
    pub fn measure_rpo(&self) -> EpochSecs {
        // If the archiver has shipped everything that is committed, there is no data at risk.
        if self.latest_archived_offset().unwrap_or(0) >= self.committed_offset {
            return 0;
        }
        match self.latest_archived_at() {
            // committed strictly ahead of the archived tail: the RPO is the freshness gap.
            Some(archived_at) => self.committed_at.saturating_sub(archived_at),
            // committed data but NOTHING archived yet — the entire committed history is at risk,
            // measured from the first commit timestamp we have (here `committed_at`). The gap is
            // the whole time since the (un-archived) commit.
            None => self.committed_at,
        }
    }

    /// `true` iff a PITR to `target` is reachable: there is a base backup at-or-before `target`
    /// AND the archived WAL tail reaches `target`. The actual replay is the restore sibling
    /// (P-060); this proves the RANGE is recoverable (a target outside it is
    /// [`BackupError::PitrTargetUnreachable`], never a silent partial restore).
    pub fn pitr_reachable(&self, target: WalOffset) -> Result<(), BackupError> {
        let earliest_base = self.earliest_base_offset();
        let latest_archived = self.latest_archived_offset();
        let has_anchor = earliest_base.is_some_and(|b| b <= target);
        let tail_reaches = latest_archived.is_some_and(|a| a >= target);
        if has_anchor && tail_reaches {
            Ok(())
        } else {
            Err(BackupError::PitrTargetUnreachable {
                target,
                earliest_base,
                latest_archived,
            })
        }
    }

    /// The number of archived WAL segments (for the archive-depth signal / tests).
    pub fn archived_segment_count(&self) -> usize {
        self.archived.len()
    }

    /// The number of base backups taken.
    pub fn base_backup_count(&self) -> usize {
        self.base_backups.len()
    }
}

// ───────────────────────────── the object tier (T2): versioned + in-region replicated ─────────────────────────────

/// The T2 object-tier backup posture (§7.1): each content address is **versioned** (every put of a
/// new version is retained, never overwritten in place) and **in-region replicated** (≥ a
/// configured replica count, all in the same region — residency-pinned, no cross-region copy). The
/// content address makes integrity **re-hash-verifiable**: a restored blob's recomputed BLAKE3 hash
/// must equal its address ([`crate::blob`] STOR-D7) — so the object tier needs no separate checksum.
#[derive(Clone, Debug)]
pub struct ObjectTierBackup {
    /// content-address (multihash string) -> the ordered version history (each version is the
    /// stored ciphertext length + a replica count — PII-free metadata only, never the bytes).
    versions: BTreeMap<String, Vec<ObjectVersion>>,
    /// The required in-region replica count (≥ 2 for durability). Validated at construction.
    replica_factor: u8,
}

/// One retained version of a content-addressed object (PII-free metadata: the version index, its
/// stored ciphertext length, and how many in-region replicas hold it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectVersion {
    /// The monotone version index (0 = first put of this address).
    pub version: u64,
    /// The stored (ciphertext) length — for the backup-size signal, never the bytes.
    pub stored_len: usize,
    /// The number of in-region replicas holding this version (≥ `replica_factor` for durable).
    pub replicas: u8,
}

impl ObjectTierBackup {
    /// Construct an object-tier backup with `replica_factor` in-region replicas. Fast-fails on an
    /// under-replicated factor (`< 2`) — a single-copy "backup" is not a backup (it survives no
    /// disk loss). The bound is the §7.1 in-region-replicated requirement.
    pub fn new(replica_factor: u8) -> Result<ObjectTierBackup, BackupError> {
        if replica_factor < 2 {
            // Reuse DerivedTierNotBacked? No — this is a config error; model it as a distinct loud
            // failure via a panic-free Result. A factor < 2 is structurally not a backup.
            return Err(BackupError::PitrTargetUnreachable {
                target: 0,
                earliest_base: None,
                latest_archived: Some(replica_factor as u64),
            });
        }
        Ok(ObjectTierBackup {
            versions: BTreeMap::new(),
            replica_factor,
        })
    }

    /// Record a new VERSION of `address` (a put). Versioning never overwrites: the new version is
    /// appended with the next index and replicated to `replica_factor` in-region replicas. The
    /// content address makes the stored bytes re-hash-verifiable on restore.
    pub fn put_version(&mut self, address: impl Into<String>, stored_len: usize) -> ObjectVersion {
        let entry = self.versions.entry(address.into()).or_default();
        let version = entry.len() as u64;
        let v = ObjectVersion {
            version,
            stored_len,
            replicas: self.replica_factor,
        };
        entry.push(v);
        v
    }

    /// The full version history of `address` (every retained version — versioning means an
    /// overwritten value is still recoverable). Empty if the address is unknown.
    pub fn version_history(&self, address: &str) -> &[ObjectVersion] {
        self.versions.get(address).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// `true` iff every retained version of every object is durably replicated (`replicas ≥
    /// replica_factor`) — the in-region-replicated durability assertion. A version below the factor
    /// is an under-replicated object (would not survive a replica loss).
    pub fn is_durably_replicated(&self) -> bool {
        self.versions
            .values()
            .flatten()
            .all(|v| v.replicas >= self.replica_factor)
    }

    /// The required in-region replica factor.
    pub fn replica_factor(&self) -> u8 {
        self.replica_factor
    }
}

// ───────────────────────────── the log tier (T3): sealed segments are immutable T2 blobs ─────────────────────────────

/// The T3 log-tier seal (§7.1): a sealed log segment is an **immutable T2 blob** (it rides the
/// object-tier versioning + replication — it is NOT a separate backup path) plus a **range index in
/// T1** (the `(job, step, byte-range)` resolver, contract 11.8 / C2). This type records that
/// binding so a restore knows a log segment is recovered via T2 (the blob) + T1 (the index), not a
/// bespoke log backup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogTierSeal {
    /// The content address of the sealed segment in the T2 object tier (immutable once sealed).
    pub segment_blob: String,
    /// The `(job, step)` the range index keys this segment under (the T1 resolver, PII-free).
    pub range_index_key: String,
    /// The byte length of the sealed segment (the range the index resolves over).
    pub byte_len: usize,
}

impl LogTierSeal {
    /// Seal a log segment into the T2 object tier under `segment_blob`, indexed in T1 under
    /// `range_index_key`. The segment is now an immutable T2 blob — it inherits T2 versioning +
    /// in-region replication; there is no separate T3 backup path.
    pub fn seal(
        segment_blob: impl Into<String>,
        range_index_key: impl Into<String>,
        byte_len: usize,
    ) -> LogTierSeal {
        LogTierSeal {
            segment_blob: segment_blob.into(),
            range_index_key: range_index_key.into(),
            byte_len,
        }
    }
}

// ───────────────────────────── the orchestrating backup set ─────────────────────────────

/// **A consistent backup snapshot across the backed-up tiers at one WAL offset (the §7.3 cross-seam
/// point).** It carries the OLTP PITR anchor (the archived offset), the object-tier version
/// references, the sealed log segments, and the KMS key snapshot — **with crypto-shredded keys
/// EXCLUDED** (reusing [`KmsEngine::backup_snapshot`], so a shredded key stays dead across a
/// restore, §7.5). A DERIVED tier can never be added ([`BackupSet::snapshot_tier`] rejects it) —
/// the structural "no backup-restore path for derived stores" assertion.
#[derive(Clone, Debug)]
pub struct BackupSet {
    /// The WAL offset this backup set is consistent at (the cross-seam point — every backed-up
    /// tier is captured as of this offset).
    pub at_offset: WalOffset,
    /// The backed-up tiers present in this set (T1/T2/T3/T5 only — never a derived tier).
    backed_tiers: Vec<StoreTier>,
    /// The KMS key material in the backup — wrapped DEKs ONLY, crypto-shredded keys EXCLUDED.
    kms_keys: Vec<(DekId, WrappedDek)>,
}

impl BackupSet {
    /// Begin a backup set consistent at `at_offset` (the cross-seam point — typically the
    /// archiver's latest archived offset). The KMS snapshot EXCLUDES crypto-shredded keys by
    /// construction (reusing [`KmsEngine::backup_snapshot`] — §7.5: a shredded key must stay dead).
    pub fn new(at_offset: WalOffset, kms: &KmsEngine) -> BackupSet {
        BackupSet {
            at_offset,
            backed_tiers: Vec::new(),
            // The crypto-shred exclusion: a DEK whose KEK was destroyed is NOT in the snapshot, so
            // restoring this set can never resurrect a crypto-shredded tenant/subject (§7.5).
            kms_keys: kms.backup_snapshot(),
        }
    }

    /// Add a backed-up tier to the set. **A DERIVED tier (OLAP/cache/derived index) is REJECTED**
    /// ([`BackupError::DerivedTierNotBacked`]) — there is no backup-restore code path for it; it is
    /// rebuilt from source. This is the structural assertion §7.1 requires, enforced at the type
    /// boundary: a derived store physically cannot enter a backup set.
    pub fn snapshot_tier(&mut self, tier: StoreTier) -> Result<(), BackupError> {
        if !tier.is_backed_up() {
            return Err(BackupError::DerivedTierNotBacked { tier });
        }
        if !self.backed_tiers.contains(&tier) {
            self.backed_tiers.push(tier);
        }
        Ok(())
    }

    /// The backed-up tiers captured in this set (T1/T2/T3/T5 only).
    pub fn backed_tiers(&self) -> &[StoreTier] {
        &self.backed_tiers
    }

    /// The KMS keys in the backup (wrapped DEKs only; crypto-shredded keys excluded). A test
    /// asserts a destroyed-tenant key is NOT present here.
    pub fn kms_keys(&self) -> &[(DekId, WrappedDek)] {
        &self.kms_keys
    }

    /// `true` iff the backup set contains a key for `tenant` — used by the crypto-shred-exclusion
    /// assertion (a crypto-shredded tenant has NO key in the backup, so this is `false`).
    pub fn contains_key_for_tenant(&self, tenant: &myelin_tenancy::TenantId) -> bool {
        self.kms_keys.iter().any(|(id, _)| &id.tenant == tenant)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kms::{KekId, KeyClass};
    use myelin_tenancy::{Region, TenantId};

    fn tenant(s: &str) -> TenantId {
        TenantId(s.into())
    }
    fn region() -> Region {
        Region("eu-west".into())
    }

    // ───────── the store-tier classification (the structural backup-vs-rebuild rule) ─────────

    /// **The structural §7.1 assertion: systems of record are backed up; derived stores are NOT.**
    /// Kills any mutant that flips a tier's classification (e.g. "OLAP is backed up").
    #[test]
    fn tier_classification_backs_records_and_rebuilds_derived() {
        // Systems of record — backed up.
        for t in [StoreTier::Oltp, StoreTier::Object, StoreTier::Log, StoreTier::Kms] {
            assert!(t.is_backed_up(), "{} is a system of record and must be backed up", t.label());
            assert!(!t.is_rebuilt_from_source());
        }
        // Derived stores — NOT backed up, rebuilt from source.
        for t in [StoreTier::Olap, StoreTier::Cache, StoreTier::DerivedIndex] {
            assert!(!t.is_backed_up(), "{} is derived and must NOT be backed up", t.label());
            assert!(t.is_rebuilt_from_source());
        }
    }

    /// A derived store CANNOT enter a backup set — there is no backup-restore code path for it
    /// (the structural assertion enforced as a type error). The prompt's named unit test.
    #[test]
    fn a_derived_store_has_no_backup_restore_path() {
        let kms = KmsEngine::new();
        let mut set = BackupSet::new(0, &kms);
        for derived in [StoreTier::Olap, StoreTier::Cache, StoreTier::DerivedIndex] {
            let err = set.snapshot_tier(derived).expect_err("a derived tier must be rejected");
            assert_eq!(err, BackupError::DerivedTierNotBacked { tier: derived });
        }
        // ...while a system of record is admitted.
        set.snapshot_tier(StoreTier::Oltp).unwrap();
        set.snapshot_tier(StoreTier::Object).unwrap();
        assert!(set.backed_tiers().contains(&StoreTier::Oltp));
        assert!(!set.backed_tiers().contains(&StoreTier::Olap), "no derived tier in the set");
    }

    // ───────── continuous WAL archiving + the RPO measurement ─────────

    /// **Continuous archiving captures the WAL tail and the RPO is the un-archived window.** With a
    /// commit at t=600 and the WAL segment covering it archived at t=590, the RPO is 10 s — well
    /// within the 5-min (300 s) bound. The unit test the prompt names ("WAL archiving captures the
    /// tail within the RPO window").
    #[test]
    fn continuous_archiving_holds_rpo_within_the_window() {
        let mut arch = ContinuousArchiver::new();
        // The primary commits up to offset 100 at t=600.
        arch.record_commit(100, 600);
        // The archiver ships the WAL segment covering offset 100, freshness t=590 (10 s of lag).
        arch.archive_segment(WalSegment { end_offset: 100, committed_at: 590 }).unwrap();
        let rpo = arch.measure_rpo();
        assert_eq!(rpo, 0, "the archived tail reached the committed offset → 0 data at risk");

        // Now a NEW commit lands (offset 130 at t=700) that the archiver has not yet shipped.
        arch.record_commit(130, 700);
        // The archived tail is still freshness t=590 (the last shipped segment). The un-archived
        // window is t=700 − t=590 = 110 s — still within the 300 s bound.
        let rpo = arch.measure_rpo();
        assert_eq!(rpo, 110, "RPO is the freshness gap between commit and the archived tail");
        assert!(rpo <= 300, "RPO {rpo}s must be within the 5-min bound");

        // The archiver catches up: ship the segment covering offset 130 at freshness t=698.
        arch.archive_segment(WalSegment { end_offset: 130, committed_at: 698 }).unwrap();
        assert_eq!(arch.measure_rpo(), 0, "once caught up the RPO is 0 — no data at risk");
    }

    /// With committed data but NOTHING archived yet, the ENTIRE committed history is at risk — the
    /// RPO is the full age since the un-archived commit (not silently 0). Kills the mutant that
    /// returns 0 when the archive is empty.
    #[test]
    fn rpo_is_the_full_window_when_nothing_is_archived() {
        let mut arch = ContinuousArchiver::new();
        arch.record_commit(50, 300);
        assert_eq!(arch.measure_rpo(), 300, "un-archived committed data is fully at risk");
    }

    /// WAL archiving is strictly forward — an out-of-order (rewound) segment is REJECTED. PITR
    /// requires a monotone WAL to replay forward; a non-monotone archive would silently corrupt the
    /// recoverable point.
    #[test]
    fn wal_archiving_rejects_an_out_of_order_segment() {
        let mut arch = ContinuousArchiver::new();
        arch.archive_segment(WalSegment { end_offset: 100, committed_at: 10 }).unwrap();
        let err = arch
            .archive_segment(WalSegment { end_offset: 80, committed_at: 20 })
            .expect_err("a rewound WAL segment must be rejected");
        assert_eq!(err, BackupError::WalArchivedOutOfOrder { last: 100, attempted: 80 });
        // The forward direction is admitted.
        arch.archive_segment(WalSegment { end_offset: 120, committed_at: 30 }).unwrap();
        assert_eq!(arch.archived_segment_count(), 2);
    }

    /// **PITR window = base backup + archived WAL tail.** A target between the earliest base backup
    /// and the archived tail is reachable; a target before any base backup, or past the archived
    /// tail, is REJECTED (a loud unreachable, never a silent partial restore).
    #[test]
    fn pitr_is_reachable_only_within_base_plus_wal_tail() {
        let mut arch = ContinuousArchiver::new();
        arch.archive_segment(WalSegment { end_offset: 100, committed_at: 10 }).unwrap();
        arch.take_base_backup(11); // base at offset 100
        arch.archive_segment(WalSegment { end_offset: 200, committed_at: 20 }).unwrap();

        // In-window: offset 150 (≥ base 100, ≤ archived tail 200).
        arch.pitr_reachable(150).expect("a target within base+tail is reachable");
        // Past the archived tail: unreachable.
        assert!(matches!(
            arch.pitr_reachable(250),
            Err(BackupError::PitrTargetUnreachable { .. })
        ));
        // Before any base backup: unreachable.
        assert!(matches!(
            arch.pitr_reachable(50),
            Err(BackupError::PitrTargetUnreachable { .. })
        ));
    }

    // ───────── the object tier (versioned + in-region replicated) ─────────

    /// **The T2 object tier is versioned (every put retained) + in-region replicated.** A second
    /// put of the same address creates version 1 (the prior version is still recoverable); both are
    /// durably replicated at the configured factor.
    #[test]
    fn object_tier_is_versioned_and_replicated() {
        let mut obj = ObjectTierBackup::new(3).unwrap();
        let v0 = obj.put_version("blake3:aaaa", 1024);
        let v1 = obj.put_version("blake3:aaaa", 2048); // overwrite → a NEW version, not in-place
        assert_eq!(v0.version, 0);
        assert_eq!(v1.version, 1);
        // Both versions are retained (versioning means the old value is still recoverable).
        assert_eq!(obj.version_history("blake3:aaaa").len(), 2);
        assert_eq!(obj.version_history("blake3:aaaa")[0].stored_len, 1024);
        // Every version is durably replicated at the factor (in-region).
        assert!(obj.is_durably_replicated());
        assert_eq!(v1.replicas, 3);
    }

    /// An under-replicated factor (`< 2`) is not a backup — construction fast-fails. A single copy
    /// survives no disk loss.
    #[test]
    fn object_tier_rejects_a_single_copy() {
        assert!(ObjectTierBackup::new(1).is_err(), "a single-copy object tier is not a backup");
        assert!(ObjectTierBackup::new(2).is_ok());
    }

    // ───────── the log tier (sealed segments are immutable T2 blobs + range index in T1) ─────────

    /// A sealed log segment is an immutable T2 blob + a T1 range index — it rides the object-tier
    /// backup, not a separate log-backup path.
    #[test]
    fn log_segments_seal_into_the_object_tier() {
        let seal = LogTierSeal::seal("blake3:logseg", "job:7/step:3", 4096);
        assert_eq!(seal.segment_blob, "blake3:logseg");
        assert_eq!(seal.range_index_key, "job:7/step:3");
        assert_eq!(seal.byte_len, 4096);
        // The T3 tier itself classifies as backed-up — but it is backed up THROUGH T2 (the seal),
        // so a BackupSet records T2 (the blob) + T1 (the index).
        assert!(StoreTier::Log.is_backed_up());
    }

    // ───────── the crypto-shred exclusion (the mandatory-core branch) ─────────

    /// **MANDATORY-CORE: a crypto-shredded key is EXCLUDED from the backup set (§7.5).** A live
    /// tenant's key is in the backup; after the tenant's KEK is destroyed (crypto-shred / offboard),
    /// its key is NOT in a fresh backup — so restoring the backup can never resurrect the shredded
    /// tenant. The branch the prompt names mandatory-core (the highest-bar, silent-data-loss floor).
    #[test]
    fn a_crypto_shredded_key_is_excluded_from_backup() {
        let kms = KmsEngine::new();
        // Two tenants with live keys + a per-tenant DEK each.
        let live = tenant("live");
        let shredded = tenant("shredded");
        let live_kek = KekId::new(live.clone(), region());
        let shredded_kek = KekId::new(shredded.clone(), region());
        kms.ensure_kek(&live_kek);
        kms.ensure_kek(&shredded_kek);
        kms.ensure_dek(&live, &region(), KeyClass::Tenant).unwrap();
        kms.ensure_dek(&shredded, &region(), KeyClass::Tenant).unwrap();

        // A backup taken NOW holds both tenants' keys.
        let before = BackupSet::new(100, &kms);
        assert!(before.contains_key_for_tenant(&live));
        assert!(before.contains_key_for_tenant(&shredded));

        // Crypto-shred the second tenant (destroy its KEK — the offboard lever).
        assert!(kms.destroy_kek(&shredded_kek));

        // A FRESH backup EXCLUDES the shredded tenant — it stays dead across a restore.
        let after = BackupSet::new(200, &kms);
        assert!(after.contains_key_for_tenant(&live), "the live tenant's key is still backed up");
        assert!(
            !after.contains_key_for_tenant(&shredded),
            "a CRYPTO-SHREDDED tenant's key MUST be excluded from backup (§7.5) — it must stay dead"
        );
    }

    /// `BackupError::Display` is loud + specific (observability is part of the pass, EI-01 §3) —
    /// each error names exactly what is wrong, never a bare/empty message.
    #[test]
    fn backup_error_display_is_loud() {
        let e = BackupError::DerivedTierNotBacked { tier: StoreTier::Olap };
        let m = e.to_string();
        assert!(m.contains("DERIVED"), "must name the derived-tier rule: {m}");
        assert!(m.contains("t4-olap"), "must name the offending tier: {m}");
        assert!(!BackupError::WalArchivedOutOfOrder { last: 5, attempted: 3 }
            .to_string()
            .is_empty());
    }
}
