use std::collections::BTreeMap;

use crate::kms::{DekId, KmsEngine, KmsError, WrappedDek};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StoreTier {
    Oltp,
    Object,
    Log,
    Olap,
    Kms,
    Cache,
    DerivedIndex,
}

impl StoreTier {
    pub fn is_backed_up(self) -> bool {
        match self {
            StoreTier::Oltp | StoreTier::Object | StoreTier::Log | StoreTier::Kms => true,
            StoreTier::Olap | StoreTier::Cache | StoreTier::DerivedIndex => false,
        }
    }

    pub fn is_rebuilt_from_source(self) -> bool {
        !self.is_backed_up()
    }

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

pub type WalOffset = u64;

pub type EpochSecs = u64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalSegment {
    pub end_offset: WalOffset,
    pub committed_at: EpochSecs,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaseBackup {
    pub at_offset: WalOffset,
    pub taken_at: EpochSecs,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackupError {
    WalArchivedOutOfOrder {
        last: WalOffset,
        attempted: WalOffset,
    },
    DerivedTierNotBacked {
        tier: StoreTier,
    },
    PitrTargetUnreachable {
        target: WalOffset,
        earliest_base: Option<WalOffset>,
        latest_archived: Option<WalOffset>,
    },
    Kms(KmsError),
}

impl core::fmt::Display for BackupError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            BackupError::WalArchivedOutOfOrder { last, attempted } => write!(
                f,
                "WAL archived out of order: last archived offset {last}, attempted {attempted} \
                 (WAL archiving is strictly forward - PITR requires it)"
            ),
            BackupError::DerivedTierNotBacked { tier } => write!(
                f,
                "tier {} is DERIVED and is rebuilt from source - there is no backup-restore path \
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
                latest archived {latest_archived:?}) - restore cannot land, NOT a silent partial"
            ),
            BackupError::Kms(error) => write!(f, "KMS backup snapshot failed: {error}"),
        }
    }
}

impl std::error::Error for BackupError {}

#[derive(Clone, Debug, Default)]
pub struct ContinuousArchiver {
    archived: Vec<WalSegment>,
    base_backups: Vec<BaseBackup>,
    committed_offset: WalOffset,
    committed_at: EpochSecs,
}

impl ContinuousArchiver {
    pub fn new() -> ContinuousArchiver {
        ContinuousArchiver::default()
    }

    pub fn record_commit(&mut self, offset: WalOffset, at: EpochSecs) {
        if offset >= self.committed_offset {
            self.committed_offset = offset;
            self.committed_at = at;
        }
    }

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

    pub fn take_base_backup(&mut self, taken_at: EpochSecs) {
        let at_offset = self.latest_archived_offset().unwrap_or(0);
        self.base_backups.push(BaseBackup {
            at_offset,
            taken_at,
        });
    }

    pub fn latest_archived_offset(&self) -> Option<WalOffset> {
        self.archived.last().map(|s| s.end_offset)
    }

    pub fn latest_archived_at(&self) -> Option<EpochSecs> {
        self.archived.last().map(|s| s.committed_at)
    }

    pub fn earliest_base_offset(&self) -> Option<WalOffset> {
        self.base_backups.iter().map(|b| b.at_offset).min()
    }

    pub fn measure_rpo(&self) -> EpochSecs {
        if self.latest_archived_offset().unwrap_or(0) >= self.committed_offset {
            return 0;
        }
        match self.latest_archived_at() {
            Some(archived_at) => self.committed_at.saturating_sub(archived_at),
            None => self.committed_at,
        }
    }

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

    pub fn archived_segment_count(&self) -> usize {
        self.archived.len()
    }

    pub fn base_backup_count(&self) -> usize {
        self.base_backups.len()
    }
}

#[derive(Clone, Debug)]
pub struct ObjectTierBackup {
    versions: BTreeMap<String, Vec<ObjectVersion>>,
    replica_factor: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectVersion {
    pub version: u64,
    pub stored_len: usize,
    pub replicas: u8,
}

impl ObjectTierBackup {
    pub fn new(replica_factor: u8) -> Result<ObjectTierBackup, BackupError> {
        if replica_factor < 2 {
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

    pub fn version_history(&self, address: &str) -> &[ObjectVersion] {
        self.versions
            .get(address)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    pub fn is_durably_replicated(&self) -> bool {
        self.versions
            .values()
            .flatten()
            .all(|v| v.replicas >= self.replica_factor)
    }

    pub fn replica_factor(&self) -> u8 {
        self.replica_factor
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogTierSeal {
    pub segment_blob: String,
    pub range_index_key: String,
    pub byte_len: usize,
}

impl LogTierSeal {
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

#[derive(Clone, Debug)]
pub struct BackupSet {
    pub at_offset: WalOffset,
    backed_tiers: Vec<StoreTier>,
    kms_keys: Vec<(DekId, WrappedDek)>,
}

impl BackupSet {
    pub fn new(at_offset: WalOffset, kms: &KmsEngine) -> Result<BackupSet, BackupError> {
        Ok(BackupSet {
            at_offset,
            backed_tiers: Vec::new(),
            kms_keys: kms.backup_snapshot().map_err(BackupError::Kms)?,
        })
    }

    pub fn snapshot_tier(&mut self, tier: StoreTier) -> Result<(), BackupError> {
        if !tier.is_backed_up() {
            return Err(BackupError::DerivedTierNotBacked { tier });
        }
        if !self.backed_tiers.contains(&tier) {
            self.backed_tiers.push(tier);
        }
        Ok(())
    }

    pub fn backed_tiers(&self) -> &[StoreTier] {
        &self.backed_tiers
    }

    pub fn kms_keys(&self) -> &[(DekId, WrappedDek)] {
        &self.kms_keys
    }

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

    #[test]
    fn tier_classification_backs_records_and_rebuilds_derived() {
        for t in [
            StoreTier::Oltp,
            StoreTier::Object,
            StoreTier::Log,
            StoreTier::Kms,
        ] {
            assert!(
                t.is_backed_up(),
                "{} is a system of record and must be backed up",
                t.label()
            );
            assert!(!t.is_rebuilt_from_source());
        }
        for t in [StoreTier::Olap, StoreTier::Cache, StoreTier::DerivedIndex] {
            assert!(
                !t.is_backed_up(),
                "{} is derived and must NOT be backed up",
                t.label()
            );
            assert!(t.is_rebuilt_from_source());
        }
    }

    #[test]
    fn a_derived_store_has_no_backup_restore_path() {
        let kms = KmsEngine::new();
        let mut set = BackupSet::new(0, &kms).unwrap();
        for derived in [StoreTier::Olap, StoreTier::Cache, StoreTier::DerivedIndex] {
            let err = set
                .snapshot_tier(derived)
                .expect_err("a derived tier must be rejected");
            assert_eq!(err, BackupError::DerivedTierNotBacked { tier: derived });
        }
        set.snapshot_tier(StoreTier::Oltp).unwrap();
        set.snapshot_tier(StoreTier::Object).unwrap();
        assert!(set.backed_tiers().contains(&StoreTier::Oltp));
        assert!(
            !set.backed_tiers().contains(&StoreTier::Olap),
            "no derived tier in the set"
        );
    }

    #[test]
    fn continuous_archiving_holds_rpo_within_the_window() {
        let mut arch = ContinuousArchiver::new();
        arch.record_commit(100, 600);
        arch.archive_segment(WalSegment {
            end_offset: 100,
            committed_at: 590,
        })
        .unwrap();
        let rpo = arch.measure_rpo();
        assert_eq!(
            rpo, 0,
            "the archived tail reached the committed offset → 0 data at risk"
        );

        arch.record_commit(130, 700);
        let rpo = arch.measure_rpo();
        assert_eq!(
            rpo, 110,
            "RPO is the freshness gap between commit and the archived tail"
        );
        assert!(rpo <= 300, "RPO {rpo}s must be within the 5-min bound");

        arch.archive_segment(WalSegment {
            end_offset: 130,
            committed_at: 698,
        })
        .unwrap();
        assert_eq!(
            arch.measure_rpo(),
            0,
            "once caught up the RPO is 0 - no data at risk"
        );
    }

    #[test]
    fn rpo_is_the_full_window_when_nothing_is_archived() {
        let mut arch = ContinuousArchiver::new();
        arch.record_commit(50, 300);
        assert_eq!(
            arch.measure_rpo(),
            300,
            "un-archived committed data is fully at risk"
        );
    }

    #[test]
    fn wal_archiving_rejects_an_out_of_order_segment() {
        let mut arch = ContinuousArchiver::new();
        arch.archive_segment(WalSegment {
            end_offset: 100,
            committed_at: 10,
        })
        .unwrap();
        let err = arch
            .archive_segment(WalSegment {
                end_offset: 80,
                committed_at: 20,
            })
            .expect_err("a rewound WAL segment must be rejected");
        assert_eq!(
            err,
            BackupError::WalArchivedOutOfOrder {
                last: 100,
                attempted: 80
            }
        );
        arch.archive_segment(WalSegment {
            end_offset: 120,
            committed_at: 30,
        })
        .unwrap();
        assert_eq!(arch.archived_segment_count(), 2);
    }

    #[test]
    fn pitr_is_reachable_only_within_base_plus_wal_tail() {
        let mut arch = ContinuousArchiver::new();
        arch.archive_segment(WalSegment {
            end_offset: 100,
            committed_at: 10,
        })
        .unwrap();
        arch.take_base_backup(11);
        arch.archive_segment(WalSegment {
            end_offset: 200,
            committed_at: 20,
        })
        .unwrap();

        arch.pitr_reachable(150)
            .expect("a target within base+tail is reachable");
        assert!(matches!(
            arch.pitr_reachable(250),
            Err(BackupError::PitrTargetUnreachable { .. })
        ));
        assert!(matches!(
            arch.pitr_reachable(50),
            Err(BackupError::PitrTargetUnreachable { .. })
        ));
    }

    #[test]
    fn object_tier_is_versioned_and_replicated() {
        let mut obj = ObjectTierBackup::new(3).unwrap();
        let v0 = obj.put_version("blake3:aaaa", 1024);
        let v1 = obj.put_version("blake3:aaaa", 2048);
        assert_eq!(v0.version, 0);
        assert_eq!(v1.version, 1);
        assert_eq!(obj.version_history("blake3:aaaa").len(), 2);
        assert_eq!(obj.version_history("blake3:aaaa")[0].stored_len, 1024);
        assert!(obj.is_durably_replicated());
        assert_eq!(v1.replicas, 3);
    }

    #[test]
    fn object_tier_rejects_a_single_copy() {
        assert!(
            ObjectTierBackup::new(1).is_err(),
            "a single-copy object tier is not a backup"
        );
        assert!(ObjectTierBackup::new(2).is_ok());
    }

    #[test]
    fn log_segments_seal_into_the_object_tier() {
        let seal = LogTierSeal::seal("blake3:logseg", "job:7/step:3", 4096);
        assert_eq!(seal.segment_blob, "blake3:logseg");
        assert_eq!(seal.range_index_key, "job:7/step:3");
        assert_eq!(seal.byte_len, 4096);
        assert!(StoreTier::Log.is_backed_up());
    }

    #[test]
    fn a_crypto_shredded_key_is_excluded_from_backup() {
        let kms = KmsEngine::new();
        let live = tenant("live");
        let shredded = tenant("shredded");
        let live_kek = KekId::new(live.clone(), region());
        let shredded_kek = KekId::new(shredded.clone(), region());
        kms.ensure_kek(&live_kek).expect("seed the in-memory KEK");
        kms.ensure_kek(&shredded_kek)
            .expect("seed the in-memory KEK");
        kms.ensure_dek(&live, &region(), KeyClass::Tenant).unwrap();
        kms.ensure_dek(&shredded, &region(), KeyClass::Tenant)
            .unwrap();

        let before = BackupSet::new(100, &kms).unwrap();
        assert!(before.contains_key_for_tenant(&live));
        assert!(before.contains_key_for_tenant(&shredded));

        assert!(kms.destroy_kek(&shredded_kek).unwrap());

        let after = BackupSet::new(200, &kms).unwrap();
        assert!(
            after.contains_key_for_tenant(&live),
            "the live tenant's key is still backed up"
        );
        assert!(
            !after.contains_key_for_tenant(&shredded),
            "a CRYPTO-SHREDDED tenant's key MUST be excluded from backup (§7.5) - it must stay dead"
        );
    }

    #[test]
    fn backup_error_display_is_loud() {
        let e = BackupError::DerivedTierNotBacked {
            tier: StoreTier::Olap,
        };
        let m = e.to_string();
        assert!(
            m.contains("DERIVED"),
            "must name the derived-tier rule: {m}"
        );
        assert!(m.contains("t4-olap"), "must name the offending tier: {m}");
        assert!(!BackupError::WalArchivedOutOfOrder {
            last: 5,
            attempted: 3
        }
        .to_string()
        .is_empty());
    }
}
