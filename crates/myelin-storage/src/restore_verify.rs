use std::collections::{BTreeMap, BTreeSet};

use myelin_tenancy::TenantId;

use crate::backup::{ContinuousArchiver, WalOffset};
use crate::blob::ContentHash;
use crate::kms::KmsEngine;
use crate::restore::{
    restore_to_offset, BlobPresence, RestoreError, RestoreReport, SourceLog, WalRow,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoredObject {
    pub content_address: ContentHash,
    pub bytes: Vec<u8>,
}

impl RestoredObject {
    pub fn integral(bytes: impl Into<Vec<u8>>) -> RestoredObject {
        let bytes = bytes.into();
        RestoredObject {
            content_address: ContentHash::blake3(&bytes),
            bytes,
        }
    }

    pub fn checksum_parity_holds(&self) -> bool {
        ContentHash::blake3(&self.bytes) == self.content_address
    }
}

#[derive(Clone, Debug, Default)]
pub struct RestoreTarget {
    pub oltp_rows: Vec<WalRow>,
    pub objects: BTreeMap<ContentHash, Vec<u8>>,
    pub derived_docs: BTreeSet<String>,
    pub restored_to_offset: WalOffset,
}

#[derive(Clone)]
pub struct ErasureLedger {
    backend: ErasureLedgerBackend,
}

#[derive(Clone)]
enum ErasureLedgerBackend {
    #[cfg(any(test, feature = "test-support"))]
    Memory(std::sync::Arc<std::sync::Mutex<BTreeMap<TenantId, WalOffset>>>),
    Pg(Box<crate::restore_verify_durable::DurableRestoreErasureLedger>),
}

#[cfg(any(test, feature = "test-support"))]
impl Default for ErasureLedger {
    fn default() -> Self {
        Self::new()
    }
}

impl ErasureLedger {
    #[cfg(any(test, feature = "test-support"))]
    pub fn new() -> ErasureLedger {
        ErasureLedger {
            backend: ErasureLedgerBackend::Memory(std::sync::Arc::new(std::sync::Mutex::new(
                BTreeMap::new(),
            ))),
        }
    }

    pub fn with_pg(
        backing: crate::restore_verify_durable::DurableRestoreErasureLedger,
    ) -> ErasureLedger {
        ErasureLedger {
            backend: ErasureLedgerBackend::Pg(Box::new(backing)),
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn record_erased(&self, tenant: TenantId) -> &Self {
        self.record_erased_at(tenant, 0)
    }

    pub fn record_erased_at(&self, tenant: TenantId, completed_at_offset: WalOffset) -> &Self {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            ErasureLedgerBackend::Memory(m) => {
                m.lock()
                    .expect("erasure ledger poisoned")
                    .insert(tenant, completed_at_offset);
            }
            ErasureLedgerBackend::Pg(backing) => {
                backing.record_erased_at(&tenant, completed_at_offset)
            }
        }
        self
    }

    pub fn records(&self) -> Vec<(TenantId, WalOffset)> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            ErasureLedgerBackend::Memory(m) => m
                .lock()
                .expect("erasure ledger poisoned")
                .iter()
                .map(|(t, o)| (t.clone(), *o))
                .collect(),
            ErasureLedgerBackend::Pg(backing) => backing.records(),
        }
    }

    pub fn erased_tenants(&self) -> BTreeSet<TenantId> {
        self.records().into_iter().map(|(t, _)| t).collect()
    }
}

pub struct GateInputs<'a> {
    pub archiver: &'a ContinuousArchiver,
    pub target: WalOffset,
    pub rows: &'a [WalRow],
    pub objects: &'a [RestoredObject],
    pub source: &'a SourceLog,
    pub kms: &'a KmsEngine,
    pub erasure_ledger: &'a ErasureLedger,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GateFailure {
    RestoreFailed(RestoreError),
    ChecksumMismatch {
        row_id: String,
        content_address: ContentHash,
    },
    ReferencedObjectAbsent {
        row_id: String,
        content_address: ContentHash,
    },
    CrossSeamMismatch {
        count: usize,
        detail: String,
    },
    ErasureResurrected {
        tenant: TenantId,
    },
}

impl core::fmt::Display for GateFailure {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            GateFailure::RestoreFailed(e) => {
                write!(f, "RESTORE-VERIFY FAIL - the restore itself failed: {e}")
            }
            GateFailure::ChecksumMismatch { row_id, content_address } => write!(
                f,
                "RESTORE-VERIFY FAIL - CHECKSUM MISMATCH: restored object {} (referenced by row \
                 {row_id}) does not re-hash to its content-address - silent corruption, the restore \
                 is NOT whole",
                content_address.to_multihash_string()
            ),
            GateFailure::ReferencedObjectAbsent { row_id, content_address } => write!(
                f,
                "RESTORE-VERIFY FAIL - REFERENCED OBJECT ABSENT: row {row_id} references {} which is \
                 not in the restored object tier",
                content_address.to_multihash_string()
            ),
            GateFailure::CrossSeamMismatch { count, detail } => write!(
                f,
                "RESTORE-VERIFY FAIL - CROSS-SEAM: {count} mismatch(es) across OLTP↔blob↔index↔offset \
                 - the restore did NOT land at one consistent point: {detail}"
            ),
            GateFailure::ErasureResurrected { tenant } => write!(
                f,
                "RESTORE-VERIFY FAIL - ERASURE RESURRECTED: tenant {} was crypto-shredded before the \
                 backup but has a restored key - a shred that did NOT stay dead across the restore \
                 (§7.5). THE GRAVEST FAILURE: it un-erases a person",
                tenant.0
            ),
        }
    }
}

impl std::error::Error for GateFailure {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GreenArtifact {
    pub restored_to_offset: WalOffset,
    pub oltp_row_count: usize,
    pub objects_verified: usize,
    pub derived_doc_count: usize,
    pub dangling_ref_count: u64,
    pub checksum_mismatches: u64,
    pub cross_seam_mismatches: u64,
    pub resurrected_subjects: u64,
}

impl GreenArtifact {
    pub fn summary(&self) -> String {
        format!(
            "restore-verify PASS: restore(to_offset T={}) landed OLTP↔blob↔index↔offset at ONE \
             consistent point - {} OLTP rows (all seq≤T), {} objects checksum-parity-verified, {} \
             derived docs reindexed-from-source; dangling_ref_count={}, checksum_mismatches={}, \
             cross_seam_mismatches={}, resurrected_subjects={} (all 0). cold==live by construction.",
            self.restored_to_offset,
            self.oltp_row_count,
            self.objects_verified,
            self.derived_doc_count,
            self.dangling_ref_count,
            self.checksum_mismatches,
            self.cross_seam_mismatches,
            self.resurrected_subjects,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a restore-verify gate verdict must be checked - a dropped RED is a SWALLOWED \
              silent-data-loss failure (the permanent gate, EI-01 §5: loud-never-swallowed)"]
pub enum GateVerdict {
    Green(GreenArtifact),
    Red(GateFailure),
}

impl GateVerdict {
    pub fn is_green(&self) -> bool {
        matches!(self, GateVerdict::Green(_))
    }

    pub fn green_artifact(&self) -> Option<&GreenArtifact> {
        match self {
            GateVerdict::Green(a) => Some(a),
            GateVerdict::Red(_) => None,
        }
    }

    pub fn failure(&self) -> Option<&GateFailure> {
        match self {
            GateVerdict::Red(f) => Some(f),
            GateVerdict::Green(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RestoreVerifyGate;

impl RestoreVerifyGate {
    pub fn new() -> RestoreVerifyGate {
        RestoreVerifyGate
    }

    pub fn run(&self, inputs: &GateInputs<'_>) -> GateVerdict {
        self.run_inner(inputs)
    }

    fn run_inner(&self, inputs: &GateInputs<'_>) -> GateVerdict {
        let presence = build_presence(inputs.objects);
        let report = match restore_to_offset(
            inputs.archiver,
            inputs.target,
            inputs.rows,
            &presence,
            inputs.source,
            inputs.kms,
        ) {
            Ok(report) => report,
            Err(e) => return GateVerdict::Red(GateFailure::RestoreFailed(e)),
        };

        let object_bytes: BTreeMap<ContentHash, Vec<u8>> = inputs
            .objects
            .iter()
            .map(|o| (o.content_address.clone(), o.bytes.clone()))
            .collect();

        for row in &report.oltp_rows {
            if let Some(content_address) = &row.blob_ref {
                match object_bytes.get(content_address) {
                    None => {
                        return GateVerdict::Red(GateFailure::ReferencedObjectAbsent {
                            row_id: row.id.clone(),
                            content_address: content_address.clone(),
                        });
                    }
                    Some(bytes) => {
                        if &ContentHash::blake3(bytes) != content_address {
                            return GateVerdict::Red(GateFailure::ChecksumMismatch {
                                row_id: row.id.clone(),
                                content_address: content_address.clone(),
                            });
                        }
                    }
                }
            }
        }

        let cross_seam = verify_cross_seam_native(&report, &object_bytes);
        if !cross_seam.is_empty() {
            return GateVerdict::Red(GateFailure::CrossSeamMismatch {
                count: cross_seam.len(),
                detail: cross_seam.join("; "),
            });
        }

        for (tenant, completed_at_offset) in inputs.erasure_ledger.records() {
            if completed_at_offset > report.restored_to_offset {
                return GateVerdict::Red(GateFailure::ErasureResurrected { tenant });
            }
            if report.restored_key_for_tenant(&tenant) {
                return GateVerdict::Red(GateFailure::ErasureResurrected { tenant });
            }
        }

        GateVerdict::Green(GreenArtifact {
            restored_to_offset: report.restored_to_offset,
            oltp_row_count: report.oltp_rows.len(),
            objects_verified: report
                .oltp_rows
                .iter()
                .filter(|r| r.blob_ref.is_some())
                .count(),
            derived_doc_count: report.derived.doc_count(),
            dangling_ref_count: report.dangling_ref_count,
            checksum_mismatches: 0,
            cross_seam_mismatches: 0,
            resurrected_subjects: 0,
        })
    }

    pub fn run_or_fail_ci(&self, inputs: &GateInputs<'_>) -> Result<GreenArtifact, GateFailure> {
        match self.run(inputs) {
            GateVerdict::Green(artifact) => Ok(artifact),
            GateVerdict::Red(failure) => Err(failure),
        }
    }
}

fn build_presence(objects: &[RestoredObject]) -> BlobPresence {
    let mut presence = BlobPresence::new();
    for obj in objects {
        presence.insert(obj.content_address.clone());
    }
    presence
}

fn verify_cross_seam_native(
    report: &RestoreReport,
    object_bytes: &BTreeMap<ContentHash, Vec<u8>>,
) -> Vec<String> {
    let mut mismatches = Vec::new();
    let row_ids: BTreeSet<&str> = report.oltp_rows.iter().map(|r| r.id.as_str()).collect();

    for row in &report.oltp_rows {
        if let Some(addr) = &row.blob_ref {
            if !object_bytes.contains_key(addr) {
                mismatches.push(format!(
                    "row {} → missing blob {}",
                    row.id,
                    addr.to_multihash_string()
                ));
            }
        }
        if row.written_at > report.restored_to_offset {
            mismatches.push(format!(
                "row {} written at offset {} is past the restored point {}",
                row.id, row.written_at, report.restored_to_offset
            ));
        }
    }
    for doc in report.derived.docs() {
        if !row_ids.contains(doc.as_str()) {
            mismatches.push(format!("orphan derived doc projecting absent row {doc}"));
        }
    }
    mismatches
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::WalSegment;
    use crate::kms::{KekId, KeyClass};
    use myelin_tenancy::Region;

    fn region() -> Region {
        Region("eu-west".into())
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

    fn kms_with_tenant(t: &TenantId) -> KmsEngine {
        let kms = KmsEngine::new();
        kms.ensure_kek(&KekId::new(t.clone(), region()))
            .expect("seed the in-memory KEK");
        kms.ensure_dek(t, &region(), KeyClass::Tenant).unwrap();
        kms
    }

    #[test]
    fn the_gate_greens_a_whole_restore_with_measured_numbers() {
        let t = tenant("acme");
        let kms = kms_with_tenant(&t);
        let arch = reachable_archiver(300);
        let objects = vec![
            RestoredObject::integral(b"blob-90".to_vec()),
            RestoredObject::integral(b"blob-100".to_vec()),
        ];
        let mut source = SourceLog::new();
        source.append(90, "r90").append(100, "r100");
        let rows = vec![
            WalRow {
                id: "r90".into(),
                written_at: 90,
                blob_ref: Some(objects[0].content_address.clone()),
            },
            WalRow {
                id: "r100".into(),
                written_at: 100,
                blob_ref: Some(objects[1].content_address.clone()),
            },
            WalRow {
                id: "r-future".into(),
                written_at: 250,
                blob_ref: None,
            },
        ];
        let ledger = ErasureLedger::new();
        let inputs = GateInputs {
            archiver: &arch,
            target: 100,
            rows: &rows,
            objects: &objects,
            source: &source,
            kms: &kms,
            erasure_ledger: &ledger,
        };

        let verdict = RestoreVerifyGate::new().run(&inputs);
        assert!(
            verdict.is_green(),
            "a whole restore must GREEN, got {:?}",
            verdict.failure()
        );
        let artifact = verdict.green_artifact().expect("green artifact present");
        assert_eq!(artifact.restored_to_offset, 100);
        assert_eq!(artifact.oltp_row_count, 2, "the future row was dropped");
        assert_eq!(
            artifact.objects_verified, 2,
            "both referenced objects checksum-parity-verified"
        );
        assert_eq!(
            artifact.derived_doc_count, 2,
            "derived == source-replay to T"
        );
        assert_eq!(artifact.dangling_ref_count, 0);
        assert_eq!(artifact.checksum_mismatches, 0);
        assert_eq!(artifact.cross_seam_mismatches, 0);
        assert_eq!(artifact.resurrected_subjects, 0);
        let s = artifact.summary();
        assert!(s.contains("restore-verify PASS"));
        assert!(s.contains("T=100"));
    }

    #[test]
    fn run_or_fail_ci_returns_ok_on_green() {
        let t = tenant("acme");
        let kms = kms_with_tenant(&t);
        let arch = reachable_archiver(300);
        let objects = vec![RestoredObject::integral(b"x".to_vec())];
        let mut source = SourceLog::new();
        source.append(50, "r1");
        let rows = vec![WalRow {
            id: "r1".into(),
            written_at: 50,
            blob_ref: Some(objects[0].content_address.clone()),
        }];
        let ledger = ErasureLedger::new();
        let inputs = GateInputs {
            archiver: &arch,
            target: 100,
            rows: &rows,
            objects: &objects,
            source: &source,
            kms: &kms,
            erasure_ledger: &ledger,
        };
        let artifact = RestoreVerifyGate::new()
            .run_or_fail_ci(&inputs)
            .expect("a whole restore must not fail CI");
        assert_eq!(artifact.oltp_row_count, 1);
    }

    #[test]
    fn a_corrupt_restored_object_fails_the_gate() {
        let t = tenant("acme");
        let kms = kms_with_tenant(&t);
        let arch = reachable_archiver(300);
        let address = ContentHash::blake3(b"good-bytes");
        let corrupt = RestoredObject {
            content_address: address.clone(),
            bytes: b"CORRUPTED".to_vec(),
        };
        let objects = vec![corrupt];
        let source = SourceLog::new();
        let rows = vec![WalRow {
            id: "r1".into(),
            written_at: 50,
            blob_ref: Some(address.clone()),
        }];
        let ledger = ErasureLedger::new();
        let inputs = GateInputs {
            archiver: &arch,
            target: 100,
            rows: &rows,
            objects: &objects,
            source: &source,
            kms: &kms,
            erasure_ledger: &ledger,
        };

        let verdict = RestoreVerifyGate::new().run(&inputs);
        assert!(
            !verdict.is_green(),
            "a corrupt restored object MUST FAIL the gate, not pass silently"
        );
        assert_eq!(
            verdict.failure(),
            Some(&GateFailure::ChecksumMismatch {
                row_id: "r1".into(),
                content_address: address.clone()
            })
        );
        let err = RestoreVerifyGate::new()
            .run_or_fail_ci(&inputs)
            .expect_err("must fail CI");
        assert!(
            err.to_string().contains("CHECKSUM MISMATCH"),
            "loud + specific: {err}"
        );
    }

    #[test]
    fn a_corrupted_backup_fails_ci() {
        let t = tenant("acme");
        let kms = kms_with_tenant(&t);
        let arch = reachable_archiver(300);
        let present = RestoredObject::integral(b"present".to_vec());
        let missing_addr = ContentHash::blake3(b"missing");
        let objects = vec![present.clone()];
        let source = SourceLog::new();
        let rows = vec![
            WalRow {
                id: "ok".into(),
                written_at: 50,
                blob_ref: Some(present.content_address.clone()),
            },
            WalRow {
                id: "corrupt".into(),
                written_at: 90,
                blob_ref: Some(missing_addr),
            },
        ];
        let ledger = ErasureLedger::new();
        let inputs = GateInputs {
            archiver: &arch,
            target: 100,
            rows: &rows,
            objects: &objects,
            source: &source,
            kms: &kms,
            erasure_ledger: &ledger,
        };

        let err = RestoreVerifyGate::new().run_or_fail_ci(&inputs).expect_err(
            "a corrupted backup (row → missing blob) MUST fail CI, never silently pass",
        );
        assert!(
            matches!(&err, GateFailure::RestoreFailed(RestoreError::DanglingBlobRef { row_id, .. }) if row_id == "corrupt"),
            "the gate surfaces the restore's hard dangling-ref FAIL: {err}"
        );
        assert!(
            err.to_string().contains("RESTORE-VERIFY FAIL"),
            "loud: {err}"
        );
    }

    #[test]
    fn the_gate_fails_on_a_cross_seam_orphan_doc() {
        let t = tenant("acme");
        let kms = kms_with_tenant(&t);
        let arch = reachable_archiver(300);
        let objects: Vec<RestoredObject> = vec![];
        let mut source = SourceLog::new();
        source.append(50, "ghost");
        let rows: Vec<WalRow> = vec![];
        let ledger = ErasureLedger::new();
        let inputs = GateInputs {
            archiver: &arch,
            target: 100,
            rows: &rows,
            objects: &objects,
            source: &source,
            kms: &kms,
            erasure_ledger: &ledger,
        };

        let verdict = RestoreVerifyGate::new().run(&inputs);
        assert!(
            !verdict.is_green(),
            "an orphan derived doc is a cross-seam mismatch"
        );
        match verdict.failure() {
            Some(GateFailure::CrossSeamMismatch { count, .. }) => assert_eq!(*count, 1),
            other => panic!("expected a CrossSeamMismatch, got {other:?}"),
        }
    }

    #[test]
    fn an_erased_tenant_stays_erased_across_the_restore() {
        let live = tenant("live");
        let shredded = tenant("shredded");
        let kms = KmsEngine::new();
        kms.ensure_kek(&KekId::new(live.clone(), region()))
            .expect("seed the in-memory KEK");
        kms.ensure_dek(&live, &region(), KeyClass::Tenant).unwrap();
        kms.ensure_kek(&KekId::new(shredded.clone(), region()))
            .expect("seed the in-memory KEK");
        kms.ensure_dek(&shredded, &region(), KeyClass::Tenant)
            .unwrap();
        assert!(kms
            .destroy_kek(&KekId::new(shredded.clone(), region()))
            .unwrap());

        let arch = reachable_archiver(300);
        let objects: Vec<RestoredObject> = vec![];
        let source = SourceLog::new();
        let rows: Vec<WalRow> = vec![];
        let ledger = ErasureLedger::new();
        ledger.record_erased(shredded.clone());
        let inputs = GateInputs {
            archiver: &arch,
            target: 100,
            rows: &rows,
            objects: &objects,
            source: &source,
            kms: &kms,
            erasure_ledger: &ledger,
        };

        let verdict = RestoreVerifyGate::new().run(&inputs);
        assert!(
            verdict.is_green(),
            "an erased tenant must stay erased → green, got {:?}",
            verdict.failure()
        );
    }

    #[test]
    fn the_gate_fails_on_a_resurrected_erased_subject() {
        let resurrected = tenant("should-be-dead");
        let kms = kms_with_tenant(&resurrected);
        let arch = reachable_archiver(300);
        let objects: Vec<RestoredObject> = vec![];
        let source = SourceLog::new();
        let rows: Vec<WalRow> = vec![];
        let ledger = ErasureLedger::new();
        ledger.record_erased(resurrected.clone());
        let inputs = GateInputs {
            archiver: &arch,
            target: 100,
            rows: &rows,
            objects: &objects,
            source: &source,
            kms: &kms,
            erasure_ledger: &ledger,
        };

        let verdict = RestoreVerifyGate::new().run(&inputs);
        assert!(
            !verdict.is_green(),
            "a resurrected erased subject MUST fail the gate"
        );
        assert_eq!(
            verdict.failure(),
            Some(&GateFailure::ErasureResurrected {
                tenant: resurrected.clone()
            })
        );
        let err = RestoreVerifyGate::new()
            .run_or_fail_ci(&inputs)
            .expect_err("must fail CI");
        assert!(
            err.to_string().contains("ERASURE RESURRECTED"),
            "loud + specific: {err}"
        );
    }

    #[test]
    fn an_erasure_completed_inside_the_backup_window_is_refused_by_the_bare_gate() {
        let windowed = tenant("erased-after-the-backup");
        let kms = KmsEngine::new();
        let arch = reachable_archiver(300);
        let objects: Vec<RestoredObject> = vec![];
        let source = SourceLog::new();
        let rows: Vec<WalRow> = vec![];
        let ledger = ErasureLedger::new();
        ledger.record_erased_at(windowed.clone(), 140);
        let inputs = GateInputs {
            archiver: &arch,
            target: 100,
            rows: &rows,
            objects: &objects,
            source: &source,
            kms: &kms,
            erasure_ledger: &ledger,
        };

        let verdict = RestoreVerifyGate::new().run(&inputs);
        assert!(
            !verdict.is_green(),
            "an erasure completed inside the backup window MUST be caught (the §7.6 residual)"
        );
        assert_eq!(
            verdict.failure(),
            Some(&GateFailure::ErasureResurrected {
                tenant: windowed.clone()
            }),
            "the gate refuses the restore-inside-window resurrection"
        );
    }

    #[test]
    fn an_erasure_completed_at_or_before_the_pit_is_the_before_backup_case() {
        let shredded = tenant("erased-before-the-backup");
        let kms = KmsEngine::new();
        kms.ensure_kek(&KekId::new(shredded.clone(), region()))
            .expect("seed the in-memory KEK");
        kms.ensure_dek(&shredded, &region(), KeyClass::Tenant)
            .unwrap();
        assert!(kms
            .destroy_kek(&KekId::new(shredded.clone(), region()))
            .unwrap());
        let arch = reachable_archiver(300);
        let objects: Vec<RestoredObject> = vec![];
        let source = SourceLog::new();
        let rows: Vec<WalRow> = vec![];
        let ledger = ErasureLedger::new();
        ledger.record_erased_at(shredded.clone(), 100);
        let inputs = GateInputs {
            archiver: &arch,
            target: 100,
            rows: &rows,
            objects: &objects,
            source: &source,
            kms: &kms,
            erasure_ledger: &ledger,
        };
        let verdict = RestoreVerifyGate::new().run(&inputs);
        assert!(
            verdict.is_green(),
            "an erasure completed at-or-before the PIT is the before-backup case → green, got {:?}",
            verdict.failure()
        );
    }

    #[test]
    fn checksum_parity_holds_iff_bytes_rehash_to_address() {
        let ok = RestoredObject::integral(b"hello".to_vec());
        assert!(ok.checksum_parity_holds(), "integral object → parity holds");

        let corrupt = RestoredObject {
            content_address: ContentHash::blake3(b"hello"),
            bytes: b"tampered".to_vec(),
        };
        assert!(
            !corrupt.checksum_parity_holds(),
            "tampered bytes → parity broken"
        );
    }
}
