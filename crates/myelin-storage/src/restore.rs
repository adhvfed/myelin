use std::collections::{BTreeMap, BTreeSet};

use myelin_tenancy::TenantId;

use crate::backup::{ContinuousArchiver, WalOffset};
use crate::blob::ContentHash;
use crate::kms::{DekId, KmsEngine, WrappedDek};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalRow {
    pub id: String,
    pub written_at: WalOffset,
    pub blob_ref: Option<ContentHash>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceEvent {
    pub offset: WalOffset,
    pub projects_row_id: String,
}

#[derive(Clone, Debug, Default)]
pub struct SourceLog {
    events: Vec<SourceEvent>,
}

impl SourceLog {
    pub fn new() -> SourceLog {
        SourceLog::default()
    }

    pub fn append(&mut self, offset: WalOffset, row_id: impl Into<String>) -> &mut Self {
        self.events.push(SourceEvent {
            offset,
            projects_row_id: row_id.into(),
        });
        self
    }

    pub fn events_through(&self, t: WalOffset) -> impl Iterator<Item = &SourceEvent> {
        self.events.iter().filter(move |e| e.offset <= t)
    }
}

#[derive(Clone, Debug, Default)]
pub struct ReindexFromSource {
    docs: BTreeSet<String>,
    resumed_at: WalOffset,
}

impl ReindexFromSource {
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

    pub fn docs(&self) -> &BTreeSet<String> {
        &self.docs
    }

    pub fn has_doc(&self, row_id: &str) -> bool {
        self.docs.contains(row_id)
    }

    pub fn resumed_at(&self) -> WalOffset {
        self.resumed_at
    }

    pub fn doc_count(&self) -> usize {
        self.docs.len()
    }
}

#[derive(Clone, Debug, Default)]
pub struct BlobPresence {
    present: BTreeSet<ContentHash>,
}

impl BlobPresence {
    pub fn new() -> BlobPresence {
        BlobPresence::default()
    }

    pub fn insert(&mut self, hash: ContentHash) -> &mut Self {
        self.present.insert(hash);
        self
    }

    pub fn contains(&self, hash: &ContentHash) -> bool {
        self.present.contains(hash)
    }

    pub fn len(&self) -> usize {
        self.present.len()
    }

    pub fn is_empty(&self) -> bool {
        self.present.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RestoreError {
    PitrUnreachable(crate::backup::BackupError),
    DanglingBlobRef {
        row_id: String,
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
                 from the restored object tier - the restore FAILS (the §7.3 silent-corruption \
                 case), it does not silently pass",
                missing.to_multihash_string()
            ),
        }
    }
}

impl std::error::Error for RestoreError {}

#[derive(Clone, Debug)]
pub struct RestoreReport {
    pub restored_to_offset: WalOffset,
    pub oltp_rows: Vec<WalRow>,
    pub derived: ReindexFromSource,
    pub restored_keys: Vec<(DekId, WrappedDek)>,
    pub dangling_ref_count: u64,
}

impl RestoreReport {
    pub fn restored_key_for_tenant(&self, tenant: &TenantId) -> bool {
        self.restored_keys
            .iter()
            .any(|(id, _)| &id.tenant == tenant)
    }
}

pub fn restore_to_offset(
    archiver: &ContinuousArchiver,
    target: WalOffset,
    rows: &[WalRow],
    blobs: &BlobPresence,
    source: &SourceLog,
    kms: &KmsEngine,
) -> Result<RestoreReport, RestoreError> {
    archiver
        .pitr_reachable(target)
        .map_err(RestoreError::PitrUnreachable)?;

    let restored_rows: Vec<WalRow> = rows
        .iter()
        .filter(|r| r.written_at <= target)
        .cloned()
        .collect();

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

    let derived = ReindexFromSource::reindex(source, target);

    let restored_keys = kms.backup_snapshot();

    Ok(RestoreReport {
        restored_to_offset: target,
        oltp_rows: restored_rows,
        derived,
        restored_keys,
        dangling_ref_count: 0,
    })
}

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

    fn region_eu() -> Region {
        Region("eu-west".into())
    }

    fn h(s: &str) -> ContentHash {
        ContentHash::blake3(s.as_bytes())
    }

    fn tenant(s: &str) -> TenantId {
        TenantId(s.into())
    }

    fn reachable_archiver(tail: WalOffset) -> ContinuousArchiver {
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

    #[test]
    fn restore_lands_oltp_at_the_seq_le_t_cursor() {
        let arch = reachable_archiver(200);
        let blobs = BlobPresence::new();
        let source = SourceLog::new();
        let kms = KmsEngine::new();
        let rows = vec![
            WalRow {
                id: "r1".into(),
                written_at: 90,
                blob_ref: None,
            },
            WalRow {
                id: "r2".into(),
                written_at: 100,
                blob_ref: None,
            },
            WalRow {
                id: "r3".into(),
                written_at: 140,
                blob_ref: None,
            },
        ];
        let report = restore_to_offset(&arch, 100, &rows, &blobs, &source, &kms).unwrap();

        assert_eq!(
            report.restored_to_offset, 100,
            "restored to the consistency point T"
        );
        let ids: Vec<&str> = report.oltp_rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["r1", "r2"],
            "rows ≤ T restored; the row past T dropped"
        );
        assert!(
            report.oltp_rows.iter().all(|r| r.written_at <= 100),
            "no restored row may be past the consistency point"
        );
    }

    #[test]
    fn a_present_referenced_blob_restores_clean() {
        let arch = reachable_archiver(200);
        let mut blobs = BlobPresence::new();
        blobs.insert(h("blob-a")).insert(h("blob-b"));
        let source = SourceLog::new();
        let kms = KmsEngine::new();
        let rows = vec![
            WalRow {
                id: "r1".into(),
                written_at: 90,
                blob_ref: Some(h("blob-a")),
            },
            WalRow {
                id: "r2".into(),
                written_at: 100,
                blob_ref: Some(h("blob-b")),
            },
        ];
        let report = restore_to_offset(&arch, 100, &rows, &blobs, &source, &kms).unwrap();
        assert_eq!(
            report.dangling_ref_count, 0,
            "every referenced blob is present → 0 dangling"
        );
    }

    #[test]
    fn a_missing_referenced_hash_makes_restore_fail() {
        let arch = reachable_archiver(200);
        let mut blobs = BlobPresence::new();
        blobs.insert(h("blob-a"));
        let source = SourceLog::new();
        let kms = KmsEngine::new();
        let rows = vec![
            WalRow {
                id: "r1".into(),
                written_at: 90,
                blob_ref: Some(h("blob-a")),
            },
            WalRow {
                id: "r2".into(),
                written_at: 95,
                blob_ref: Some(h("blob-b")),
            },
        ];
        let err = restore_to_offset(&arch, 100, &rows, &blobs, &source, &kms)
            .expect_err("a row → missing blob MUST make the restore FAIL, not pass silently");
        assert_eq!(
            err,
            RestoreError::DanglingBlobRef {
                row_id: "r2".into(),
                missing: h("blob-b")
            }
        );
        let m = err.to_string();
        assert!(
            m.contains("DANGLING BLOB REF"),
            "must name the dangling-ref case: {m}"
        );
        assert!(m.contains("r2"), "must name the offending row: {m}");
    }

    #[test]
    fn a_dropped_rows_missing_blob_does_not_fail_the_restore() {
        let arch = reachable_archiver(200);
        let blobs = BlobPresence::new();
        let source = SourceLog::new();
        let kms = KmsEngine::new();
        let rows = vec![
            WalRow {
                id: "kept".into(),
                written_at: 90,
                blob_ref: None,
            },
            WalRow {
                id: "r-future".into(),
                written_at: 150,
                blob_ref: Some(h("gone")),
            },
        ];
        let report = restore_to_offset(&arch, 100, &rows, &blobs, &source, &kms).unwrap();
        assert_eq!(report.oltp_rows.len(), 1, "only the kept row is restored");
        assert_eq!(report.dangling_ref_count, 0);
    }

    #[test]
    fn derived_stores_rebuild_from_source_not_a_backup() {
        let arch = reachable_archiver(200);
        let blobs = BlobPresence::new();
        let mut source = SourceLog::new();
        source
            .append(50, "r1")
            .append(90, "r2")
            .append(100, "r3")
            .append(140, "r-future");
        let kms = KmsEngine::new();

        let report = restore_to_offset(&arch, 100, &[], &blobs, &source, &kms).unwrap();
        let derived = &report.derived;
        assert!(derived.has_doc("r1") && derived.has_doc("r2") && derived.has_doc("r3"));
        assert!(
            !derived.has_doc("r-future"),
            "a source event past T must NOT be reindexed (consumers resume at T)"
        );
        assert_eq!(
            derived.resumed_at(),
            100,
            "consumers resume at the restored point T"
        );
        assert_eq!(
            derived.doc_count(),
            3,
            "derived == source replayed to T, by construction"
        );
    }

    #[test]
    fn reindex_from_source_is_idempotent() {
        let mut source = SourceLog::new();
        source
            .append(10, "dup")
            .append(20, "dup")
            .append(30, "other");
        let a = ReindexFromSource::reindex(&source, 100);
        let b = ReindexFromSource::reindex(&source, 100);
        assert_eq!(a.docs(), b.docs(), "reindex is deterministic + idempotent");
        assert_eq!(
            a.doc_count(),
            2,
            "the duplicated projection collapses to one doc"
        );
    }

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
        kms.ensure_kek(&live_kek).expect("seed the in-memory KEK");
        kms.ensure_kek(&shredded_kek)
            .expect("seed the in-memory KEK");
        kms.ensure_dek(&live, &region_eu(), KeyClass::Tenant)
            .unwrap();
        kms.ensure_dek(&shredded, &region_eu(), KeyClass::Tenant)
            .unwrap();

        assert!(kms.destroy_kek(&shredded_kek));

        let report = restore_to_offset(&arch, 100, &[], &blobs, &source, &kms).unwrap();
        assert!(
            report.restored_key_for_tenant(&live),
            "a LIVE tenant's KEK must be restored"
        );
        assert!(
            !report.restored_key_for_tenant(&shredded),
            "a CRYPTO-SHREDDED tenant's KEK must NOT be restored - it stays dead across the restore (§7.5)"
        );
        let counts = restored_key_counts(&report);
        assert_eq!(
            counts.get(&shredded),
            None,
            "the shredded tenant contributes 0 restored keys"
        );
        assert_eq!(counts.get(&live).copied(), Some(1));
    }

    #[test]
    fn an_unreachable_target_fails_loudly() {
        let arch = reachable_archiver(100);
        let blobs = BlobPresence::new();
        let source = SourceLog::new();
        let kms = KmsEngine::new();
        let err = restore_to_offset(&arch, 500, &[], &blobs, &source, &kms)
            .expect_err("a target past the WAL tail must fail loudly, never a silent partial");
        assert!(matches!(err, RestoreError::PitrUnreachable(_)));
        assert!(!err.to_string().is_empty(), "the failure is observable");
    }

    #[test]
    fn the_whole_restore_lands_at_one_consistent_point() {
        let arch = reachable_archiver(300);
        let mut blobs = BlobPresence::new();
        blobs.insert(h("a")).insert(h("b"));
        let mut source = SourceLog::new();
        source.append(90, "r1").append(100, "r2");
        let kms = KmsEngine::new();
        let t = tenant("acme");
        kms.ensure_kek(&KekId::new(t.clone(), region_eu()))
            .expect("seed the in-memory KEK");
        kms.ensure_dek(&t, &region_eu(), KeyClass::Tenant).unwrap();

        let rows = vec![
            WalRow {
                id: "r1".into(),
                written_at: 90,
                blob_ref: Some(h("a")),
            },
            WalRow {
                id: "r2".into(),
                written_at: 100,
                blob_ref: Some(h("b")),
            },
            WalRow {
                id: "r3".into(),
                written_at: 250,
                blob_ref: None,
            },
        ];
        let report = restore_to_offset(&arch, 100, &rows, &blobs, &source, &kms).unwrap();

        assert_eq!(report.restored_to_offset, 100);
        assert_eq!(report.oltp_rows.len(), 2, "rows ≤ T");
        assert_eq!(
            report.dangling_ref_count, 0,
            "every referenced blob present"
        );
        assert!(report.derived.has_doc("r1") && report.derived.has_doc("r2"));
        assert_eq!(report.derived.resumed_at(), 100);
        assert!(
            report.restored_key_for_tenant(&t),
            "the live tenant's KEK is restored"
        );
    }
}
