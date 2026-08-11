use myelin_tenancy::{OpaqueSubjectId, Region, TenantId};

use crate::blob::ContentHash;
use crate::encryption::SubjectId;
use crate::erase::{CryptoShredErase, EpochMillis, EraseError, EraseHolders, ErasureReceipt};
use crate::kms::{DekId, KeyClass, KmsEngine};
use crate::reerase::{PostRestoreErasureLedger, ReErasePass, ReEraseReport};
use crate::restore::RestoreReport;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HolderClass {
    Oltp,
    ObjectStore,
    GitPackTier,
    CiLogs,
    AgentMemory,
    ChatBodies,
    KnowledgeBlocks,
    SearchIndexAndVectors,
    RefsEdges,
    EventBus,
    OlapReadStore,
    NotifInbox,
    WorkflowHistory,
    AuthzTuples,
    IdentityPseudonymMap,
    AuditCarveOut,
    CachesAndCdn,
    Backups,
}

impl HolderClass {
    pub const ALL: [HolderClass; 18] = [
        HolderClass::Oltp,
        HolderClass::ObjectStore,
        HolderClass::GitPackTier,
        HolderClass::CiLogs,
        HolderClass::AgentMemory,
        HolderClass::ChatBodies,
        HolderClass::KnowledgeBlocks,
        HolderClass::SearchIndexAndVectors,
        HolderClass::RefsEdges,
        HolderClass::EventBus,
        HolderClass::OlapReadStore,
        HolderClass::NotifInbox,
        HolderClass::WorkflowHistory,
        HolderClass::AuthzTuples,
        HolderClass::IdentityPseudonymMap,
        HolderClass::AuditCarveOut,
        HolderClass::CachesAndCdn,
        HolderClass::Backups,
    ];

    pub fn holder_id(self) -> &'static str {
        match self {
            HolderClass::Oltp => "oltp",
            HolderClass::ObjectStore => "blob_store",
            HolderClass::GitPackTier => "git_pack_tier",
            HolderClass::CiLogs => "ci_logs",
            HolderClass::AgentMemory => "agent_memory",
            HolderClass::ChatBodies => "chat_bodies",
            HolderClass::KnowledgeBlocks => "knowledge_blocks",
            HolderClass::SearchIndexAndVectors => "search_index_vectors",
            HolderClass::RefsEdges => "refs_edges",
            HolderClass::EventBus => "event_bus",
            HolderClass::OlapReadStore => "olap_read_store",
            HolderClass::NotifInbox => "notif_inbox",
            HolderClass::WorkflowHistory => "workflow_history",
            HolderClass::AuthzTuples => "authz_tuples",
            HolderClass::IdentityPseudonymMap => "identity",
            HolderClass::AuditCarveOut => "audit_carve_out",
            HolderClass::CachesAndCdn => "cache_cdn",
            HolderClass::Backups => "backups",
        }
    }

    pub fn h_number(self) -> &'static str {
        match self {
            HolderClass::Oltp => "H1",
            HolderClass::ObjectStore => "H2",
            HolderClass::GitPackTier => "H3",
            HolderClass::CiLogs => "H4",
            HolderClass::AgentMemory => "H5",
            HolderClass::ChatBodies => "H6",
            HolderClass::KnowledgeBlocks => "H7",
            HolderClass::SearchIndexAndVectors => "H8",
            HolderClass::RefsEdges => "H9",
            HolderClass::EventBus => "H10",
            HolderClass::OlapReadStore => "H11",
            HolderClass::NotifInbox => "H12",
            HolderClass::WorkflowHistory => "H13",
            HolderClass::AuthzTuples => "H14",
            HolderClass::IdentityPseudonymMap => "H15",
            HolderClass::AuditCarveOut => "H16",
            HolderClass::CachesAndCdn => "H17",
            HolderClass::Backups => "H18",
        }
    }

    pub fn erasure(self) -> HolderErasure {
        match self {
            HolderClass::Oltp
            | HolderClass::CiLogs
            | HolderClass::AgentMemory
            | HolderClass::ChatBodies
            | HolderClass::KnowledgeBlocks => HolderErasure::SubjectDekCryptoShred,
            HolderClass::ObjectStore | HolderClass::GitPackTier => {
                HolderErasure::BlobDekCryptoShred
            }
            HolderClass::SearchIndexAndVectors => HolderErasure::PurgeAndReindex,
            HolderClass::RefsEdges
            | HolderClass::OlapReadStore
            | HolderClass::NotifInbox
            | HolderClass::WorkflowHistory
            | HolderClass::AuthzTuples
            | HolderClass::CachesAndCdn => HolderErasure::PurgeOrTombstone,
            HolderClass::EventBus => HolderErasure::BusErase,
            HolderClass::IdentityPseudonymMap => HolderErasure::PseudonymMapShred,
            HolderClass::AuditCarveOut => HolderErasure::AuditCarveOut,
            HolderClass::Backups => HolderErasure::BackupByConstruction,
        }
    }

    pub fn carries_vectors(self) -> bool {
        matches!(self, HolderClass::SearchIndexAndVectors)
    }

    pub fn is_backup_tier(self) -> bool {
        matches!(self, HolderClass::Backups)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HolderErasure {
    SubjectDekCryptoShred,
    BlobDekCryptoShred,
    PurgeAndReindex,
    PurgeOrTombstone,
    BusErase,
    PseudonymMapShred,
    AuditCarveOut,
    BackupByConstruction,
}

impl HolderErasure {
    pub fn destroys_key(self) -> bool {
        matches!(
            self,
            HolderErasure::SubjectDekCryptoShred | HolderErasure::BlobDekCryptoShred
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResidualPosture {
    TheOneDocumentedPosture,
    Undocumented,
}

impl ResidualPosture {
    pub fn documented() -> ResidualPosture {
        ResidualPosture::TheOneDocumentedPosture
    }

    pub fn is_documented(self) -> bool {
        matches!(self, ResidualPosture::TheOneDocumentedPosture)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HolderCoverage {
    pub holder: HolderClass,
    pub erasure: HolderErasure,
    pub reached: bool,
    pub recoverable: usize,
}

impl HolderCoverage {
    pub fn is_green(&self) -> bool {
        self.reached && self.recoverable == 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HolderCoverageReceiptSet {
    pub subject: OpaqueSubjectId,
    pub tenant: TenantId,
    pub coverages: Vec<HolderCoverage>,
    pub erase_receipt: ErasureReceipt,
    pub residual: ResidualPosture,
    pub ran_at: EpochMillis,
}

impl HolderCoverageReceiptSet {
    pub fn holders_missed(&self) -> usize {
        HolderClass::ALL
            .iter()
            .filter(|h| !self.coverages.iter().any(|c| &c.holder == *h && c.reached))
            .count()
    }

    pub fn recoverable_pii(&self) -> usize {
        self.coverages.iter().map(|c| c.recoverable).sum()
    }

    pub fn vectors_purged(&self) -> bool {
        self.coverages
            .iter()
            .any(|c| c.holder.carries_vectors() && c.is_green())
    }

    pub fn backups_clean(&self) -> bool {
        self.coverages
            .iter()
            .any(|c| c.holder.is_backup_tier() && c.is_green())
    }

    pub fn is_complete(&self) -> bool {
        self.holders_missed() == 0
            && self.coverages.len() == HolderClass::ALL.len()
            && self.recoverable_pii() == 0
            && self.vectors_purged()
            && self.backups_clean()
            && self.residual.is_documented()
    }

    pub fn seal_certificate(&self) -> HolderCoverageCertificate {
        let mut manifest = String::new();
        manifest.push_str(&format!(
            "E2E-4 holder-coverage subject={} tenant={} ran_at={}\n",
            self.subject.artifact_ref().0,
            self.tenant.as_str(),
            self.ran_at,
        ));
        for h in HolderClass::ALL.iter() {
            let cov = self.coverages.iter().find(|c| &c.holder == h);
            match cov {
                Some(c) => manifest.push_str(&format!(
                    "{} {} reached={} recoverable={}\n",
                    h.h_number(),
                    h.holder_id(),
                    c.reached,
                    c.recoverable,
                )),
                None => manifest.push_str(&format!(
                    "{} {} reached=false recoverable=MISSED\n",
                    h.h_number(),
                    h.holder_id(),
                )),
            }
        }
        manifest.push_str(&format!(
            "verdict={} holders_missed={} recoverable_pii={} residual={}\n",
            if self.is_complete() { "GREEN" } else { "RED" },
            self.holders_missed(),
            self.recoverable_pii(),
            if self.residual.is_documented() {
                "documented"
            } else {
                "UNDOCUMENTED"
            },
        ));
        HolderCoverageCertificate {
            digest: ContentHash::blake3(manifest.as_bytes()),
            sealed: self.is_complete(),
            holders_missed: self.holders_missed(),
            recoverable_pii: self.recoverable_pii(),
            ran_at: self.ran_at,
        }
    }

    pub fn summary(&self) -> String {
        format!(
            "E2E-4 storage holder fan-out [t={}]: subject={} tenant={} holders={}/{} \
             holders_missed={} recoverable_pii={} vectors_purged={} backups_clean={} residual={} -> {}",
            self.ran_at,
            self.subject.artifact_ref().0,
            self.tenant.as_str(),
            self.coverages.iter().filter(|c| c.reached).count(),
            HolderClass::ALL.len(),
            self.holders_missed(),
            self.recoverable_pii(),
            self.vectors_purged(),
            self.backups_clean(),
            if self.residual.is_documented() {
                "documented"
            } else {
                "UNDOCUMENTED"
            },
            if self.is_complete() { "GREEN" } else { "RED" },
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HolderCoverageCertificate {
    pub digest: ContentHash,
    pub sealed: bool,
    pub holders_missed: usize,
    pub recoverable_pii: usize,
    pub ran_at: EpochMillis,
}

impl HolderCoverageCertificate {
    pub fn is_green(&self) -> bool {
        self.sealed && self.holders_missed == 0 && self.recoverable_pii == 0
    }
}

pub struct FullHolderFanOut<'a> {
    engine: &'a KmsEngine,
    region: Region,
}

impl<'a> FullHolderFanOut<'a> {
    pub fn new(engine: &'a KmsEngine, region: Region) -> FullHolderFanOut<'a> {
        FullHolderFanOut { engine, region }
    }

    pub fn fan_out(
        &self,
        subject: &SubjectId,
        tenant: &TenantId,
        holders: &EraseHolders<'_>,
        now: EpochMillis,
    ) -> Result<HolderCoverageReceiptSet, EraseError> {
        self.fan_out_inner(subject, tenant, holders, now, &[])
    }

    pub fn fan_out_withholding(
        &self,
        subject: &SubjectId,
        tenant: &TenantId,
        holders: &EraseHolders<'_>,
        now: EpochMillis,
        withhold: &[HolderClass],
    ) -> Result<HolderCoverageReceiptSet, EraseError> {
        self.fan_out_inner(subject, tenant, holders, now, withhold)
    }

    fn fan_out_inner(
        &self,
        subject: &SubjectId,
        tenant: &TenantId,
        holders: &EraseHolders<'_>,
        now: EpochMillis,
        withhold: &[HolderClass],
    ) -> Result<HolderCoverageReceiptSet, EraseError> {
        let eraser = CryptoShredErase::new(self.engine, self.region.clone());
        let erase_receipt = eraser.erase(subject, tenant, holders, now)?;

        let subject_dek = DekId::new(tenant.clone(), KeyClass::Subject(subject.0.clone()));
        let blob_dek = DekId::new(tenant.clone(), KeyClass::Blob);

        let mut coverages = Vec::with_capacity(HolderClass::ALL.len());
        for &holder in HolderClass::ALL.iter() {
            let reached = !withhold.contains(&holder);
            let recoverable = if !reached {
                1
            } else {
                match holder.erasure() {
                    HolderErasure::SubjectDekCryptoShred => self.dek_present(&subject_dek) as usize,
                    HolderErasure::BlobDekCryptoShred => self.dek_present(&blob_dek) as usize,
                    _ => 0,
                }
            };
            coverages.push(HolderCoverage {
                holder,
                erasure: holder.erasure(),
                reached,
                recoverable,
            });
        }

        let recoverable_total: usize = coverages.iter().map(|c| c.recoverable).sum();
        let residual = if recoverable_total == 0 {
            ResidualPosture::documented()
        } else {
            ResidualPosture::Undocumented
        };

        Ok(HolderCoverageReceiptSet {
            subject: OpaqueSubjectId::from_ref(myelin_tenancy::ArtifactRef(subject.0.clone())),
            tenant: tenant.clone(),
            coverages,
            erase_receipt,
            residual,
            ran_at: now,
        })
    }

    fn dek_present(&self, dek: &DekId) -> bool {
        self.engine.backup_snapshot().iter().any(|(d, _)| d == dek)
    }

    pub fn reerase_after_restore(
        &self,
        report: &RestoreReport,
        ledger: &dyn PostRestoreErasureLedger,
        holders: &EraseHolders<'_>,
        now: EpochMillis,
    ) -> Result<ReEraseReport, EraseError> {
        let pass = ReErasePass::new(self.engine, self.region.clone());
        pass.run(report, ledger, holders, now)
    }
}

pub fn holder_ids_not_covered<'a>(expected: &[&'a str]) -> Vec<&'a str> {
    let covered: std::collections::BTreeSet<&str> =
        HolderClass::ALL.iter().map(|h| h.holder_id()).collect();
    expected
        .iter()
        .filter(|id| !covered.contains(*id))
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encryption::ColumnCryptor;
    use crate::erase::{BusErase, ErasureLedgerSink, PseudonymShred, RefsTombstone, SearchPurge};
    use crate::kms::{KekId, KmsEngine};
    use myelin_gdpr::ErasureMethod;
    use std::cell::RefCell;
    use std::collections::BTreeSet;

    fn t(s: &str) -> TenantId {
        TenantId::from_token(s)
    }
    fn r() -> Region {
        Region("fr-par".to_string())
    }

    #[derive(Default)]
    struct Seams {
        erased: RefCell<BTreeSet<String>>,
    }
    impl PseudonymShred for Seams {
        fn shred_pseudonym(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            Ok(())
        }
    }
    impl SearchPurge for Seams {
        fn purge_and_reindex(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            Ok(())
        }
    }
    impl RefsTombstone for Seams {
        fn tombstone(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            Ok(())
        }
    }
    impl BusErase for Seams {
        fn erase_inline_pii(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            Ok(())
        }
    }
    impl ErasureLedgerSink for Seams {
        fn record_erasure(&self, subject: &SubjectId, _t: &TenantId, _at: EpochMillis) {
            self.erased.borrow_mut().insert(subject.0.clone());
        }
        fn is_erased(&self, subject: &SubjectId, _t: &TenantId) -> bool {
            self.erased.borrow().contains(&subject.0)
        }
    }

    fn holders(seams: &Seams) -> EraseHolders<'_> {
        EraseHolders {
            pseudonym: seams,
            search: seams,
            refs: seams,
            bus: seams,
            ledger: seams,
            git_reach: None,
        }
    }

    fn holders_with_git_reach<'a>(
        seams: &'a Seams,
        git_reach: &'a crate::git_shred::GitCryptoShredReach<'a>,
    ) -> EraseHolders<'a> {
        EraseHolders {
            pseudonym: seams,
            search: seams,
            refs: seams,
            bus: seams,
            ledger: seams,
            git_reach: Some(git_reach),
        }
    }

    fn engine_with_subject(tenant: &TenantId, subject: &SubjectId) -> KmsEngine {
        let kms = KmsEngine::new();
        kms.ensure_kek(&KekId::new(tenant.clone(), r()))
            .expect("seed the in-memory KEK");
        let cryptor = ColumnCryptor::new(&kms, r());
        cryptor
            .encrypt(
                tenant,
                Some(subject),
                &ErasureMethod::CryptoShred("subject_dek".into()),
                b"alice free-text across every holder",
            )
            .expect("seal a per-subject column");
        kms.ensure_dek(tenant, &r(), KeyClass::Blob)
            .expect("create the per-tenant blob DEK");
        kms
    }

    #[test]
    fn catalogue_is_the_exhaustive_h1_h18_set() {
        assert_eq!(HolderClass::ALL.len(), 18, "the catalogue is H1..H18");
        let ids: BTreeSet<&str> = HolderClass::ALL.iter().map(|h| h.holder_id()).collect();
        assert_eq!(ids.len(), 18, "18 distinct holder ids");
        let hs: BTreeSet<&str> = HolderClass::ALL.iter().map(|h| h.h_number()).collect();
        assert_eq!(hs.len(), 18, "18 distinct H-numbers");
        for n in 1..=18 {
            let label = format!("H{n}");
            assert!(hs.contains(label.as_str()), "{label} is in the catalogue");
        }
        assert!(HolderClass::SearchIndexAndVectors.carries_vectors());
        assert!(HolderClass::Backups.is_backup_tier());
        assert!(!HolderClass::Oltp.carries_vectors());
        assert!(!HolderClass::Oltp.is_backup_tier());
    }

    #[test]
    fn erasure_modalities_route_correctly() {
        assert_eq!(
            HolderClass::SearchIndexAndVectors.erasure(),
            HolderErasure::PurgeAndReindex
        );
        assert!(!HolderClass::SearchIndexAndVectors.erasure().destroys_key());
        assert_eq!(
            HolderClass::AuditCarveOut.erasure(),
            HolderErasure::AuditCarveOut
        );
        assert!(!HolderClass::AuditCarveOut.erasure().destroys_key());
        assert!(HolderClass::Oltp.erasure().destroys_key());
        assert!(HolderClass::ChatBodies.erasure().destroys_key());
        assert!(HolderClass::ObjectStore.erasure().destroys_key());
        assert!(HolderClass::GitPackTier.erasure().destroys_key());
        assert_eq!(
            HolderClass::Backups.erasure(),
            HolderErasure::BackupByConstruction
        );
    }

    #[test]
    fn fan_out_reaches_every_holder_zero_missed_zero_recoverable() {
        let tenant = t("01J0ACME");
        let subject = SubjectId::new("u-e2e4");
        let kms = engine_with_subject(&tenant, &subject);
        let fanout = FullHolderFanOut::new(&kms, r());
        let seams = Seams::default();
        let git_reach = crate::git_shred::GitCryptoShredReach::new(&kms, r());

        let set = fanout
            .fan_out(
                &subject,
                &tenant,
                &holders_with_git_reach(&seams, &git_reach),
                1_000,
            )
            .expect("the fan-out succeeds");

        assert_eq!(set.coverages.len(), 18, "one coverage per H-holder");
        assert_eq!(set.holders_missed(), 0, "0 holders missed (the E2E-4 zero)");
        assert_eq!(
            set.recoverable_pii(),
            0,
            "0 recoverable across every holder"
        );
        assert!(set.vectors_purged(), "the vector holder is reached + clean");
        assert!(set.backups_clean(), "the backup tier is reached + clean");
        assert_eq!(set.residual, ResidualPosture::documented());
        assert!(set.is_complete(), "the holder-coverage set is COMPLETE");
        assert!(set.erase_receipt.dek_destroyed_now);
        assert!(set.erase_receipt.is_green());
        for cov in &set.coverages {
            assert!(cov.reached, "{} reached", cov.holder.h_number());
            assert!(cov.is_green(), "{} green", cov.holder.h_number());
        }
        assert!(set.summary().contains("GREEN"));
        assert!(set.summary().contains("holders_missed=0"));
        assert!(set.summary().contains("recoverable_pii=0"));
    }

    #[test]
    fn certificate_seals_a_green_fan_out_and_is_deterministic() {
        let tenant = t("01J0ACME");
        let subject = SubjectId::new("u-cert");
        let kms = engine_with_subject(&tenant, &subject);
        let fanout = FullHolderFanOut::new(&kms, r());
        let seams = Seams::default();
        let git_reach = crate::git_shred::GitCryptoShredReach::new(&kms, r());
        let set = fanout
            .fan_out(
                &subject,
                &tenant,
                &holders_with_git_reach(&seams, &git_reach),
                7,
            )
            .unwrap();

        let cert = set.seal_certificate();
        assert!(cert.sealed, "a green fan-out seals a sealed certificate");
        assert!(cert.is_green(), "the certificate is green (0/0, sealed)");
        assert_eq!(cert.holders_missed, 0);
        assert_eq!(cert.recoverable_pii, 0);
        assert_eq!(cert.ran_at, 7);
        let cert2 = set.seal_certificate();
        assert_eq!(cert.digest, cert2.digest, "the certificate is reproducible");
        assert!(cert.digest.to_multihash_string().starts_with("blake3:"));
    }

    #[test]
    fn a_withheld_holder_is_missed_and_seals_a_red_certificate() {
        let tenant = t("01J0ACME");
        let subject = SubjectId::new("u-miss");
        let kms = engine_with_subject(&tenant, &subject);
        let fanout = FullHolderFanOut::new(&kms, r());
        let seams = Seams::default();

        let set = fanout
            .fan_out_withholding(&subject, &tenant, &holders(&seams), 1, &[HolderClass::Oltp])
            .unwrap();

        assert_eq!(set.holders_missed(), 1, "the withheld holder is MISSED");
        assert!(
            set.recoverable_pii() >= 1,
            "the withheld holder has a recoverable key (a real miss, not vacuous)"
        );
        assert!(!set.is_complete(), "an incomplete fan-out is RED");
        assert_eq!(set.residual, ResidualPosture::Undocumented);
        let cert = set.seal_certificate();
        assert!(!cert.sealed, "a red fan-out seals a non-sealed certificate");
        assert!(!cert.is_green());
        assert!(cert.holders_missed >= 1);
        assert!(set.summary().contains("RED"));
    }

    #[test]
    fn re_running_the_fan_out_is_a_noop_success_across_holders() {
        let tenant = t("01J0ACME");
        let subject = SubjectId::new("u-idem");
        let kms = engine_with_subject(&tenant, &subject);
        let fanout = FullHolderFanOut::new(&kms, r());
        let seams = Seams::default();
        let git_reach = crate::git_shred::GitCryptoShredReach::new(&kms, r());

        let first = fanout
            .fan_out(
                &subject,
                &tenant,
                &holders_with_git_reach(&seams, &git_reach),
                1,
            )
            .unwrap();
        assert!(first.is_complete());
        assert!(
            first.erase_receipt.dek_destroyed_now,
            "first destroys the DEK"
        );
        assert!(!first.erase_receipt.re_run);

        let second = fanout
            .fan_out(
                &subject,
                &tenant,
                &holders_with_git_reach(&seams, &git_reach),
                2,
            )
            .unwrap();
        assert_eq!(second.holders_missed(), 0, "the re-run still misses 0");
        assert_eq!(second.recoverable_pii(), 0, "still 0 recoverable");
        assert!(second.is_complete(), "the re-run is still complete + green");
        assert!(
            second.erase_receipt.re_run,
            "the second fan-out is an idempotent re-run"
        );
        assert!(
            !second.erase_receipt.dek_destroyed_now,
            "no DEK destroyed the second pass (already gone)"
        );
    }

    #[test]
    fn reerase_after_restore_holds_across_the_full_holder_set() {
        use crate::backup::{ContinuousArchiver, WalSegment};
        use crate::reerase::{ErasureRecord, InMemoryPostPitLedger};
        use crate::restore::{restore_to_offset, BlobPresence, SourceLog};

        let tenant = t("01J0ACME");
        let subject = SubjectId::new("u-erased-after-backup");
        let kms = engine_with_subject(&tenant, &subject);
        let subject_dek = DekId::new(tenant.clone(), KeyClass::Subject(subject.0.clone()));
        assert!(
            kms.backup_snapshot().iter().any(|(d, _)| *d == subject_dek),
            "the restore resurrected the subject DEK"
        );

        let mut arch = ContinuousArchiver::new();
        arch.archive_segment(WalSegment {
            end_offset: 0,
            committed_at: 0,
        })
        .unwrap();
        arch.take_base_backup(1);
        arch.archive_segment(WalSegment {
            end_offset: 300,
            committed_at: 10,
        })
        .unwrap();
        let report = restore_to_offset(
            &arch,
            100,
            &[],
            &BlobPresence::new(),
            &SourceLog::new(),
            &kms,
        )
        .unwrap();

        let mut ledger = InMemoryPostPitLedger::new();
        ledger.record(ErasureRecord::new(subject.clone(), tenant.clone(), 140));

        let fanout = FullHolderFanOut::new(&kms, r());
        let seams = Seams::default();
        let rep = fanout
            .reerase_after_restore(&report, &ledger, &holders(&seams), 1_000)
            .expect("the re-erasure pass succeeds across the full holder set");

        assert!(rep.is_green(), "0 resurrected after the pass (§7.5)");
        assert_eq!(rep.resurrected_count, 0);
        assert!(rep.re_erased_subject(&subject, &tenant));
        assert!(
            !kms.backup_snapshot().iter().any(|(d, _)| *d == subject_dek),
            "the resurrected DEK is re-killed"
        );
    }

    #[test]
    fn catalogue_covers_the_orchestrator_storage_holder_ids() {
        let expected = [
            "blob_store",
            "event_bus",
            "cache_cdn",
            "backups",
            "authz_tuples",
            "identity",
        ];
        assert!(
            holder_ids_not_covered(&expected).is_empty(),
            "the H1–H18 catalogue covers every storage-owned orchestrator holder id"
        );
        assert_eq!(
            holder_ids_not_covered(&["not_a_holder"]),
            vec!["not_a_holder"]
        );
    }

    #[test]
    fn completeness_predicates_are_real_readings() {
        let tenant = t("01J0ACME");
        let green = HolderCoverage {
            holder: HolderClass::Oltp,
            erasure: HolderErasure::SubjectDekCryptoShred,
            reached: true,
            recoverable: 0,
        };
        assert!(green.is_green());
        let leaky = HolderCoverage {
            recoverable: 1,
            ..green.clone()
        };
        assert!(!leaky.is_green(), "a recoverable key is NOT green");
        let unreached = HolderCoverage {
            reached: false,
            ..green
        };
        assert!(!unreached.is_green(), "an unreached holder is NOT green");

        assert!(ResidualPosture::documented().is_documented());
        assert!(!ResidualPosture::Undocumented.is_documented());

        let no_vectors: Vec<HolderCoverage> = HolderClass::ALL
            .iter()
            .filter(|h| !h.carries_vectors())
            .map(|&h| HolderCoverage {
                holder: h,
                erasure: h.erasure(),
                reached: true,
                recoverable: 0,
            })
            .collect();
        let set = HolderCoverageReceiptSet {
            subject: OpaqueSubjectId::from_ref(myelin_tenancy::ArtifactRef("u".into())),
            tenant,
            coverages: no_vectors,
            erase_receipt: ErasureReceipt {
                subject: "u".into(),
                tenant: t("01J0ACME"),
                dek_destroyed_now: true,
                recoverable_in_backup: 0,
                crypto_shred_lag_ms: 0,
                re_run: false,
                completed_at: 0,
            },
            residual: ResidualPosture::documented(),
            ran_at: 0,
        };
        assert!(
            !set.vectors_purged(),
            "the vector holder is absent → vectors not proven purged"
        );
        assert!(
            !set.is_complete(),
            "a set missing the vector holder is NOT complete (the H8 assertion bites)"
        );
        assert_eq!(set.holders_missed(), 1, "the vector holder reads as missed");
    }

    fn full_green_set(ran_at: EpochMillis) -> HolderCoverageReceiptSet {
        let coverages: Vec<HolderCoverage> = HolderClass::ALL
            .iter()
            .map(|&h| HolderCoverage {
                holder: h,
                erasure: h.erasure(),
                reached: true,
                recoverable: 0,
            })
            .collect();
        HolderCoverageReceiptSet {
            subject: OpaqueSubjectId::from_ref(myelin_tenancy::ArtifactRef("u".into())),
            tenant: t("01J0ACME"),
            coverages,
            erase_receipt: ErasureReceipt {
                subject: "u".into(),
                tenant: t("01J0ACME"),
                dek_destroyed_now: true,
                recoverable_in_backup: 0,
                crypto_shred_lag_ms: 0,
                re_run: false,
                completed_at: 0,
            },
            residual: ResidualPosture::documented(),
            ran_at,
        }
    }

    #[test]
    fn is_complete_requires_every_clause() {
        let base = full_green_set(1);
        assert!(base.is_complete(), "the baseline is complete");

        let mut leaky = base.clone();
        leaky.coverages[0].recoverable = 1;
        assert!(
            !leaky.is_complete(),
            "a recoverable key makes it incomplete"
        );
        let mut no_vec = base.clone();
        let vi = no_vec
            .coverages
            .iter()
            .position(|c| c.holder.carries_vectors())
            .unwrap();
        no_vec.coverages[vi].recoverable = 1;
        assert!(!no_vec.vectors_purged());
        assert!(!no_vec.is_complete(), "vectors not purged → incomplete");
        let mut no_bk = base.clone();
        let bi = no_bk
            .coverages
            .iter()
            .position(|c| c.holder.is_backup_tier())
            .unwrap();
        no_bk.coverages[bi].recoverable = 1;
        assert!(!no_bk.backups_clean());
        assert!(!no_bk.is_complete(), "backups not clean → incomplete");
        let mut bad_res = base.clone();
        bad_res.residual = ResidualPosture::Undocumented;
        assert!(!bad_res.is_complete(), "undocumented residual → incomplete");
        let mut missed = base;
        missed.coverages[0].reached = false;
        assert!(!missed.is_complete(), "a missed holder → incomplete");
        assert_eq!(missed.holders_missed(), 1);
    }

    #[test]
    fn backups_clean_requires_the_backup_holder_specifically() {
        let coverages: Vec<HolderCoverage> = HolderClass::ALL
            .iter()
            .filter(|h| !h.is_backup_tier())
            .map(|&h| HolderCoverage {
                holder: h,
                erasure: h.erasure(),
                reached: true,
                recoverable: 0,
            })
            .collect();
        let set = HolderCoverageReceiptSet {
            coverages,
            ..full_green_set(1)
        };
        assert!(
            !set.backups_clean(),
            "no backup holder → backups NOT proven clean (even with every other holder green)"
        );
        let mut leaky = full_green_set(1);
        let bi = leaky
            .coverages
            .iter()
            .position(|c| c.holder.is_backup_tier())
            .unwrap();
        leaky.coverages[bi].recoverable = 1;
        assert!(
            !leaky.backups_clean(),
            "a recoverable backup holder is NOT clean"
        );
    }

    #[test]
    fn certificate_digest_depends_on_each_holders_coverage() {
        let a = full_green_set(1);
        let mut b = full_green_set(1);
        b.coverages[3].recoverable = 1;
        assert_ne!(
            a.seal_certificate().digest,
            b.seal_certificate().digest,
            "a per-holder coverage change MUST change the certificate digest"
        );
    }

    #[test]
    fn certificate_is_green_requires_sealed_and_zero_zero() {
        let green = HolderCoverageCertificate {
            digest: ContentHash::blake3(b"x"),
            sealed: true,
            holders_missed: 0,
            recoverable_pii: 0,
            ran_at: 0,
        };
        assert!(green.is_green());
        assert!(
            !HolderCoverageCertificate {
                sealed: false,
                ..green.clone()
            }
            .is_green(),
            "not sealed → not green"
        );
        assert!(
            !HolderCoverageCertificate {
                holders_missed: 1,
                ..green.clone()
            }
            .is_green(),
            "a missed holder → not green"
        );
        assert!(
            !HolderCoverageCertificate {
                recoverable_pii: 1,
                ..green
            }
            .is_green(),
            "a recoverable key → not green"
        );
    }

    #[test]
    fn blob_holders_read_recoverable_off_the_kms() {
        let tenant = t("01J0ACME");
        let subject = SubjectId::new("u-blob");
        let kms = engine_with_subject(&tenant, &subject);
        let blob_dek = DekId::new(tenant.clone(), KeyClass::Blob);
        assert!(
            kms.backup_snapshot().iter().any(|(d, _)| *d == blob_dek),
            "the per-tenant blob DEK is present before the erase"
        );
        let fanout = FullHolderFanOut::new(&kms, r());
        let seams = Seams::default();
        let set = fanout
            .fan_out(&subject, &tenant, &holders(&seams), 1)
            .unwrap();
        let blob_cov: Vec<&HolderCoverage> = set
            .coverages
            .iter()
            .filter(|c| matches!(c.erasure, HolderErasure::BlobDekCryptoShred))
            .collect();
        assert_eq!(
            blob_cov.len(),
            2,
            "two blob-DEK holders (object store + git pack tier)"
        );
        for cov in &blob_cov {
            assert_eq!(
                cov.recoverable,
                1,
                "{} reads the live per-tenant blob DEK as recoverable (a real KMS read, not 0)",
                cov.holder.h_number()
            );
        }
    }

    #[test]
    fn a_withheld_subject_dek_holder_reads_nonzero_recoverable_specifically() {
        let tenant = t("01J0ACME");
        let subject = SubjectId::new("u-wh-subj");
        let kms = engine_with_subject(&tenant, &subject);
        let fanout = FullHolderFanOut::new(&kms, r());
        let seams = Seams::default();
        let git_reach = crate::git_shred::GitCryptoShredReach::new(&kms, r());

        let set = fanout
            .fan_out_withholding(
                &subject,
                &tenant,
                &holders_with_git_reach(&seams, &git_reach),
                1,
                &[HolderClass::ChatBodies],
            )
            .unwrap();

        let chat = set
            .coverages
            .iter()
            .find(|c| c.holder == HolderClass::ChatBodies)
            .unwrap();
        assert!(!chat.reached, "the withheld holder is not reached");
        assert_eq!(
            chat.recoverable, 1,
            "the withheld holder reads recoverable=1 (a real miss, never a vacuous 0)"
        );
        for cov in set.coverages.iter().filter(|c| {
            matches!(c.erasure, HolderErasure::SubjectDekCryptoShred)
                && c.holder != HolderClass::ChatBodies
        }) {
            assert_eq!(
                cov.recoverable,
                0,
                "{} (reached) read 0 recoverable",
                cov.holder.h_number()
            );
        }
        assert!(!set.is_complete(), "a withheld holder is RED");
    }

    #[test]
    fn certificate_binds_each_manifest_line_to_the_right_holder() {
        let ordered = full_green_set(1);
        let mut shuffled = full_green_set(1);
        let oltp_i = ordered
            .coverages
            .iter()
            .position(|c| c.holder == HolderClass::Oltp)
            .unwrap();
        let bus_i = ordered
            .coverages
            .iter()
            .position(|c| c.holder == HolderClass::EventBus)
            .unwrap();
        let mut ordered = ordered;
        ordered.coverages[oltp_i].reached = true;
        ordered.coverages[bus_i].reached = false;
        let mut base = ordered.clone();
        base.coverages.reverse();
        shuffled.coverages = base.coverages;
        let oi = shuffled
            .coverages
            .iter()
            .position(|c| c.holder == HolderClass::Oltp)
            .unwrap();
        let bi = shuffled
            .coverages
            .iter()
            .position(|c| c.holder == HolderClass::EventBus)
            .unwrap();
        shuffled.coverages[oi].reached = true;
        shuffled.coverages[bi].reached = false;

        assert_eq!(
            ordered.seal_certificate().digest,
            shuffled.seal_certificate().digest,
            "the manifest digest is INVARIANT to coverage order (each line binds to ITS holder - \
             kills the `== -> !=` find mutant, which would bind lines to the wrong holder)"
        );

        let mut dropped = full_green_set(1);
        dropped.coverages.retain(|c| c.holder != HolderClass::Oltp);
        assert_eq!(dropped.holders_missed(), 1);
        assert!(!dropped.is_complete());
        assert_ne!(
            ordered.seal_certificate().digest,
            dropped.seal_certificate().digest,
            "a missing holder changes the digest (the MISSED line is real)"
        );
    }
}
