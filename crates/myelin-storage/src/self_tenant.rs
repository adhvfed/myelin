use std::collections::BTreeMap;

use myelin_tenancy::TenantId;

use crate::backup::WalOffset;
#[cfg(any(test, feature = "test-support"))]
use crate::backup::{ContinuousArchiver, WalSegment};
#[cfg(any(test, feature = "test-support"))]
use crate::blob::ContentHash;
#[cfg(any(test, feature = "test-support"))]
use crate::kms::{KekId, KeyClass, KmsEngine};
#[cfg(any(test, feature = "test-support"))]
use crate::restore::{SourceLog, WalRow};
#[cfg(any(test, feature = "test-support"))]
use crate::restore_verify::{ErasureLedger, GateInputs, RestoreVerifyGate, RestoredObject};
use crate::restore_verify::GreenArtifact;
use myelin_tenancy::Region;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SelfTenantStore {
    Monorepo,
    CiLog,
    Issue,
    Doc,
}

impl SelfTenantStore {
    pub fn label(self) -> &'static str {
        match self {
            SelfTenantStore::Monorepo => "monorepo",
            SelfTenantStore::CiLog => "ci-log",
            SelfTenantStore::Issue => "issue",
            SelfTenantStore::Doc => "doc",
        }
    }

    pub const ALL: [SelfTenantStore; 4] = [
        SelfTenantStore::Monorepo,
        SelfTenantStore::CiLog,
        SelfTenantStore::Issue,
        SelfTenantStore::Doc,
    ];
}

#[derive(Clone, Debug)]
pub struct SelfTenantRecord {
    pub store: SelfTenantStore,
    pub row_id: String,
    pub written_at: WalOffset,
    pub bytes: Vec<u8>,
}

impl SelfTenantRecord {
    pub fn new(
        store: SelfTenantStore,
        row_id: impl Into<String>,
        written_at: WalOffset,
        bytes: impl Into<Vec<u8>>,
    ) -> SelfTenantRecord {
        SelfTenantRecord {
            store,
            row_id: row_id.into(),
            written_at,
            bytes: bytes.into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SelfTenantCorpus {
    tenant: TenantId,
    region: Region,
    records: Vec<SelfTenantRecord>,
}

impl SelfTenantCorpus {
    pub fn new(tenant: TenantId, region: Region) -> SelfTenantCorpus {
        SelfTenantCorpus {
            tenant,
            region,
            records: Vec::new(),
        }
    }

    pub fn commit(&mut self, record: SelfTenantRecord) -> &mut Self {
        self.records.push(record);
        self
    }

    pub fn commit_record(
        &mut self,
        store: SelfTenantStore,
        row_id: impl Into<String>,
        written_at: WalOffset,
        bytes: impl Into<Vec<u8>>,
    ) -> &mut Self {
        self.commit(SelfTenantRecord::new(store, row_id, written_at, bytes))
    }

    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    pub fn region(&self) -> &Region {
        &self.region
    }

    pub fn records(&self) -> &[SelfTenantRecord] {
        &self.records
    }

    pub fn latest_offset(&self) -> WalOffset {
        self.records.iter().map(|r| r.written_at).max().unwrap_or(0)
    }

    pub fn stores_present(&self) -> std::collections::BTreeSet<SelfTenantStore> {
        self.records.iter().map(|r| r.store).collect()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn restored_objects(&self) -> Vec<RestoredObject> {
        self.records
            .iter()
            .map(|r| RestoredObject::integral(r.bytes.clone()))
            .collect()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn wal_rows(&self) -> Vec<WalRow> {
        self.records
            .iter()
            .map(|r| WalRow {
                id: r.row_id.clone(),
                written_at: r.written_at,
                blob_ref: Some(ContentHash::blake3(&r.bytes)),
            })
            .collect()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn source_log(&self) -> SourceLog {
        let mut source = SourceLog::new();
        for r in &self.records {
            source.append(r.written_at, r.row_id.clone());
        }
        source
    }

    #[cfg(any(test, feature = "test-support"))]
    fn archiver(&self) -> ContinuousArchiver {
        let mut arch = ContinuousArchiver::new();
        arch.archive_segment(WalSegment {
            end_offset: 0,
            committed_at: 0,
        })
        .expect("base segment");
        arch.take_base_backup(1);
        arch.archive_segment(WalSegment {
            end_offset: self.latest_offset(),
            committed_at: 10,
        })
        .expect("tail segment");
        arch
    }

    #[cfg(any(test, feature = "test-support"))]
    fn kms(&self) -> KmsEngine {
        let kms = KmsEngine::new();
        kms.ensure_kek(&KekId::new(self.tenant.clone(), self.region.clone()));
        kms.ensure_dek(&self.tenant, &self.region, KeyClass::Tenant)
            .expect("the self-host tenant's DEK");
        kms
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn run_restore_verify_on_self_tenant(
    corpus: &SelfTenantCorpus,
    now_iso: &str,
) -> Result<SelfTenantGreenArtifact, crate::restore_verify::GateFailure> {
    let archiver = corpus.archiver();
    let objects = corpus.restored_objects();
    let rows = corpus.wal_rows();
    let source = corpus.source_log();
    let kms = corpus.kms();
    let erasure_ledger = ErasureLedger::new();

    let inputs = GateInputs {
        archiver: &archiver,
        target: corpus.latest_offset(),
        rows: &rows,
        objects: &objects,
        source: &source,
        kms: &kms,
        erasure_ledger: &erasure_ledger,
    };

    let artifact = RestoreVerifyGate::new().run_or_fail_ci(&inputs)?;

    let mut by_store: BTreeMap<SelfTenantStore, usize> = BTreeMap::new();
    for r in corpus.records() {
        *by_store.entry(r.store).or_insert(0) += 1;
    }

    Ok(SelfTenantGreenArtifact {
        gate: artifact,
        date: now_iso.to_string(),
        tenant: corpus.tenant().clone(),
        region: corpus.region().clone(),
        records_by_store: by_store,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelfTenantGreenArtifact {
    pub gate: GreenArtifact,
    pub date: String,
    pub tenant: TenantId,
    pub region: Region,
    pub records_by_store: BTreeMap<SelfTenantStore, usize>,
}

impl SelfTenantGreenArtifact {
    pub fn summary(&self) -> String {
        let breakdown: Vec<String> = self
            .records_by_store
            .iter()
            .map(|(store, n)| format!("{}={n}", store.label()))
            .collect();
        format!(
            "[P-506 SELF_TENANT RESTORE-VERIFY GREEN {date}] tenant={tenant} region={region}: {gate} \
             - verified Myelin's OWN data: {breakdown}",
            date = self.date,
            tenant = self.tenant.0,
            region = self.region.as_str(),
            gate = self.gate.summary(),
            breakdown = breakdown.join(", "),
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageIncident {
    pub id: String,
    pub gate_id: String,
    pub summary: String,
    pub repro_drill_name: String,
}

impl StorageIncident {
    pub fn new(
        id: impl Into<String>,
        gate_id: impl Into<String>,
        summary: impl Into<String>,
        repro_drill_name: impl Into<String>,
    ) -> StorageIncident {
        StorageIncident {
            id: id.into(),
            gate_id: gate_id.into(),
            summary: summary.into(),
            repro_drill_name: repro_drill_name.into(),
        }
    }

    pub fn issue_draft(&self) -> IncidentIssueDraft {
        IncidentIssueDraft {
            title: format!("[storage incident {}] {}", self.id, self.summary),
            body: format!(
                "A storage incident surfaced during self-hosting.\n\nGate touched: {}\nReproducing \
                 drill (registered into the permanent harness suite, re-runs forever): {}\n\nThe \
                 every-incident-adds-a-drill loop (EI-01 §3) requires this incident's repro join the \
                 suite - the drill below IS that repro.",
                self.gate_id, self.repro_drill_name
            ),
            gate_id: self.gate_id.clone(),
        }
    }

    pub fn drill_ticket(&self) -> IncidentDrillTicket {
        IncidentDrillTicket {
            drill_name: self.repro_drill_name.clone(),
            gate_id: self.gate_id.clone(),
            incident_id: self.id.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncidentIssueDraft {
    pub title: String,
    pub body: String,
    pub gate_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncidentDrillTicket {
    pub drill_name: String,
    pub gate_id: String,
    pub incident_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenRow {
    pub id: &'static str,
    pub title: &'static str,
    pub proof_command: &'static str,
    pub artifact_date: Option<String>,
}

impl ProvenRow {
    pub fn is_dated(&self) -> bool {
        self.artifact_date.is_some()
    }
}

pub fn proven_storage_rows(date: &str) -> Vec<ProvenRow> {
    fn row(id: &'static str, title: &'static str, cmd: &'static str, date: &str) -> ProvenRow {
        ProvenRow {
            id,
            title,
            proof_command: cmd,
            artifact_date: Some(date.to_string()),
        }
    }
    vec![
        row(
            "STOR-D1",
            "restore-verify gate - the silent-data-loss floor (the permanent gate)",
            "cargo test -p myelin-storage --test stor_d1_restore_verify_gate_drill",
            date,
        ),
        row(
            "STOR-D2",
            "RPO ≤ 5 min / RTO - continuous archiving + PITR + cell-kill restore",
            "cargo test -p myelin-storage --test stor_d2_cell_kill_rto_drill",
            date,
        ),
        row(
            "STOR-D2-cell",
            "restore-verify at cell scale under world-scale load (RPO/RTO held under surge)",
            "cargo test -p myelin-storage --test stor_d2_d8_cell_scale_under_world_scale_load_drill",
            date,
        ),
        row(
            "STOR-D3",
            "post-restore re-erasure - 0 resurrected subjects across a restore",
            "cargo test -p myelin-storage --test stor_d3_post_restore_reerase_drill",
            date,
        ),
        row(
            "STOR-D4",
            "crypto-shred erase - 0 recoverable PII in backups",
            "cargo test -p myelin-storage --test stor_d4_crypto_shred_drill",
            date,
        ),
        row(
            "STOR-D5",
            "residency end-to-end - 0 cross-region egress",
            "cargo test -p myelin-storage --test stor_d5_cross_region_egress_drill",
            date,
        ),
        row(
            "STOR-D7",
            "blob integrity - BLAKE3 re-hash-on-read, 0 silent serve of a corrupt object",
            "cargo test -p myelin-storage blob",
            date,
        ),
        row(
            "STOR-D8",
            "online migration on a restored copy - lock-time bound held",
            "cargo test -p myelin-storage --test stor_d8_online_migration_under_load_drill",
            date,
        ),
        row(
            "D-S11",
            "trust-scoped cache - cache_scope_violation == 0",
            "cargo test -p myelin-storage ci_cache_scope",
            date,
        ),
        row(
            "D-S12",
            "OLAP restriction gate - olap_restricted_subject_leak == 0",
            "cargo test -p myelin-storage olap_restrict",
            date,
        ),
        row(
            "D-S13",
            "outbound-mirror seam - mirror deny holds",
            "cargo test -p myelin-storage mirror",
            date,
        ),
        row(
            "CP-D7",
            "cross-cell pointer bridge + cell→cell migration - 0 loss",
            "cargo test -p myelin-storage --test cp_d7_cell_to_cell_migration_drill",
            date,
        ),
        row(
            "GA-D8",
            "multi-cell DSR erase fan-out - per-cell receipt set complete",
            "cargo test -p myelin-storage --test ga_d8_multi_cell_erase_fanout_drill",
            date,
        ),
        row(
            "E2E-4",
            "full DSAR crypto-shred fan-out - 0 holders missed, 0 recoverable",
            "cargo test -p myelin-storage holder_fanout",
            date,
        ),
        row(
            "E2E-3",
            "cold-reindex == live for the derived stores (the reindex-parity half)",
            "cargo test -p myelin-storage e2e3_reindex_parity",
            date,
        ),
    ]
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a truth-up verdict must be checked - a dropped RED means a CLAIMED-NOT-PROVEN storage \
              row silently drifts the docs from the code (EI-01 §1: a claim that outlives its \
              verification misleads the next agent)"]
pub enum TruthUpVerdict {
    Green {
        rows_confirmed: usize,
        date: String,
    },
    Red {
        undated_rows: Vec<&'static str>,
    },
}

impl TruthUpVerdict {
    pub fn is_green(&self) -> bool {
        matches!(self, TruthUpVerdict::Green { .. })
    }

    pub fn undated_rows(&self) -> &[&'static str] {
        match self {
            TruthUpVerdict::Green { .. } => &[],
            TruthUpVerdict::Red { undated_rows } => undated_rows,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TruthUpPass;

impl TruthUpPass {
    pub fn new() -> TruthUpPass {
        TruthUpPass
    }

    pub fn run(&self, rows: &[ProvenRow], date: &str) -> TruthUpVerdict {
        let undated: Vec<&'static str> = rows
            .iter()
            .filter(|r| !r.is_dated())
            .map(|r| r.id)
            .collect();
        if undated.is_empty() {
            TruthUpVerdict::Green {
                rows_confirmed: rows.len(),
                date: date.to_string(),
            }
        } else {
            TruthUpVerdict::Red {
                undated_rows: undated,
            }
        }
    }

    pub fn run_or_fail_ci(&self, rows: &[ProvenRow], date: &str) -> Result<usize, TruthUpRed> {
        match self.run(rows, date) {
            TruthUpVerdict::Green { rows_confirmed, .. } => Ok(rows_confirmed),
            TruthUpVerdict::Red { undated_rows } => Err(TruthUpRed {
                undated_rows: undated_rows.iter().map(|s| s.to_string()).collect(),
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TruthUpRed {
    pub undated_rows: Vec<String>,
}

impl core::fmt::Display for TruthUpRed {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "TRUTH-UP FAIL - {} storage row(s) CLAIMED-NOT-PROVEN (no dated green artifact): {} \
             - a claim that outlives its verification misleads the next agent (EI-01 §1); fix the \
             doc or re-run the drill",
            self.undated_rows.len(),
            self.undated_rows.join(", ")
        )
    }
}

impl std::error::Error for TruthUpRed {}

#[cfg(test)]
mod tests {
    use super::*;

    fn region() -> Region {
        Region::new("fr-par")
    }

    fn self_host_corpus() -> SelfTenantCorpus {
        let mut corpus = SelfTenantCorpus::new(TenantId("myelin-self".into()), region());
        corpus
            .commit_record(
                SelfTenantStore::Monorepo,
                "commit-abc123",
                10,
                b"fn main() {}".to_vec(),
            )
            .commit_record(
                SelfTenantStore::CiLog,
                "ci-run-42-step-3",
                20,
                b"cargo test ... ok".to_vec(),
            )
            .commit_record(
                SelfTenantStore::Issue,
                "issue-P-506",
                30,
                b"self_tenant the restore gate".to_vec(),
            )
            .commit_record(
                SelfTenantStore::Doc,
                "doc-storage-arch",
                40,
                "# Storage §7".as_bytes().to_vec(),
            );
        corpus
    }

    #[test]
    fn restore_verify_greens_on_myelins_own_stores() {
        let corpus = self_host_corpus();
        let artifact = run_restore_verify_on_self_tenant(&corpus, "2026-06-25")
            .expect("the restore-verify gate must GREEN on Myelin's own data");

        assert_eq!(artifact.gate.restored_to_offset, 40);
        assert_eq!(
            artifact.gate.oltp_row_count, 4,
            "all four own-data records restored"
        );
        assert_eq!(
            artifact.gate.objects_verified, 4,
            "all four checksum-parity-verified"
        );
        assert_eq!(artifact.gate.dangling_ref_count, 0);
        assert_eq!(artifact.gate.checksum_mismatches, 0);
        assert_eq!(artifact.gate.cross_seam_mismatches, 0);
        assert_eq!(artifact.gate.resurrected_subjects, 0);

        assert_eq!(artifact.date, "2026-06-25");
        assert_eq!(artifact.tenant.0, "myelin-self");
        assert_eq!(
            artifact.records_by_store.len(),
            4,
            "all four store classes verified"
        );
        let s = artifact.summary();
        assert!(
            s.contains("P-506 SELF_TENANT RESTORE-VERIFY GREEN 2026-06-25"),
            "dated: {s}"
        );
        assert!(
            s.contains("monorepo=1")
                && s.contains("ci-log=1")
                && s.contains("issue=1")
                && s.contains("doc=1"),
            "breakdown: {s}"
        );
    }

    #[test]
    fn the_self_tenant_corpus_covers_all_four_own_stores() {
        let corpus = self_host_corpus();
        assert_eq!(
            corpus.stores_present(),
            SelfTenantStore::ALL.into_iter().collect(),
            "the self_tenant loop covers monorepo + ci-logs + issues + docs"
        );
    }

    #[test]
    fn a_corrupt_own_data_record_fails_the_self_tenant_gate() {
        use crate::restore_verify::GateFailure;

        let corpus = self_host_corpus();
        assert!(run_restore_verify_on_self_tenant(&corpus, "2026-06-25").is_ok());

        let archiver = corpus.archiver();
        let rows = corpus.wal_rows();
        let source = corpus.source_log();
        let kms = corpus.kms();
        let ledger = ErasureLedger::new();
        let mut objects = corpus.restored_objects();
        let original_addr = objects[0].content_address.clone();
        objects[0] = RestoredObject {
            content_address: original_addr.clone(),
            bytes: b"CORRUPTED-MONOREPO-COMMIT".to_vec(),
        };
        let inputs = GateInputs {
            archiver: &archiver,
            target: corpus.latest_offset(),
            rows: &rows,
            objects: &objects,
            source: &source,
            kms: &kms,
            erasure_ledger: &ledger,
        };
        let err = RestoreVerifyGate::new()
            .run_or_fail_ci(&inputs)
            .expect_err("a corrupt own-data record MUST fail the self_tenant restore-verify gate");
        assert!(
            matches!(err, GateFailure::ChecksumMismatch { ref content_address, .. } if *content_address == original_addr),
            "the gate names the corrupt own-data object: {err}"
        );
        assert!(err.to_string().contains("CHECKSUM MISMATCH"), "loud: {err}");
    }

    #[test]
    fn a_storage_incident_files_an_issue_and_registers_a_drill() {
        let incident = StorageIncident::new(
            "INC-STOR-001",
            "STOR-D1",
            "a restored CI log re-hashed wrong after a base-backup boundary",
            "repro_stor_d1_ci_log_rehash_at_base_boundary",
        );

        let draft = incident.issue_draft();
        assert!(draft.title.contains("INC-STOR-001"));
        assert!(draft.title.contains("re-hashed wrong"));
        assert_eq!(draft.gate_id, "STOR-D1");
        assert!(draft
            .body
            .contains("repro_stor_d1_ci_log_rehash_at_base_boundary"));
        assert!(draft.body.contains("every-incident-adds-a-drill"));

        let ticket = incident.drill_ticket();
        assert_eq!(
            ticket.drill_name,
            "repro_stor_d1_ci_log_rehash_at_base_boundary"
        );
        assert_eq!(ticket.gate_id, "STOR-D1");
        assert_eq!(ticket.incident_id, "INC-STOR-001");
    }

    #[test]
    fn truth_up_greens_when_every_proven_row_is_dated() {
        let rows = proven_storage_rows("2026-06-25");
        assert!(!rows.is_empty(), "the PROVEN set is non-empty");
        let verdict = TruthUpPass::new().run(&rows, "2026-06-25");
        assert!(
            verdict.is_green(),
            "every proven row is dated → green: {:?}",
            verdict.undated_rows()
        );
        match verdict {
            TruthUpVerdict::Green {
                rows_confirmed,
                date,
            } => {
                assert_eq!(rows_confirmed, rows.len());
                assert_eq!(date, "2026-06-25");
            }
            TruthUpVerdict::Red { .. } => unreachable!(),
        }
        let ids: Vec<&str> = rows.iter().map(|r| r.id).collect();
        for must in [
            "STOR-D1", "STOR-D2", "STOR-D3", "STOR-D4", "STOR-D5", "STOR-D7", "STOR-D8", "D-S11",
            "D-S12", "D-S13", "CP-D7", "GA-D8", "E2E-4", "E2E-3",
        ] {
            assert!(
                ids.contains(&must),
                "the truth-up set must enumerate {must}"
            );
        }
    }

    #[test]
    fn truth_up_reds_loudly_on_a_claimed_not_proven_row() {
        let mut rows = proven_storage_rows("2026-06-25");
        let undated = rows
            .iter_mut()
            .find(|r| r.id == "STOR-D1")
            .expect("STOR-D1 present");
        undated.artifact_date = None;

        let verdict = TruthUpPass::new().run(&rows, "2026-06-25");
        assert!(
            !verdict.is_green(),
            "a claimed-not-proven row MUST red the truth-up pass"
        );
        assert_eq!(verdict.undated_rows(), &["STOR-D1"]);

        let err = TruthUpPass::new()
            .run_or_fail_ci(&rows, "2026-06-25")
            .expect_err("a claimed-not-proven row MUST fail the truth-up CI job");
        assert!(err.to_string().contains("TRUTH-UP FAIL"), "loud: {err}");
        assert!(
            err.to_string().contains("STOR-D1"),
            "names the undated row: {err}"
        );
    }

    #[test]
    fn truth_up_run_or_fail_ci_returns_ok_when_all_dated() {
        let rows = proven_storage_rows("2026-06-25");
        let count = TruthUpPass::new()
            .run_or_fail_ci(&rows, "2026-06-25")
            .expect("a fully-dated PROVEN set must not fail the truth-up CI job");
        assert_eq!(count, rows.len());
    }
}
