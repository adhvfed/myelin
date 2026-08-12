use myelin_gdpr::{EraseReceipt, EraseScope, Receipt, Result as DsrResult, SubjectRef, TenantId};
use myelin_storage::encryption::SubjectId;
use myelin_storage::{
    CryptoShredErase, EpochMillis, EraseError, EraseHolders, ErasureLedgerSink, KmsEngine,
    PseudonymShred, RefsTombstone, SearchPurge,
};
use std::sync::Mutex;

use crate::refs_glue::PageStore;
use myelin_events::ArtifactRef;
use myelin_search::engine::IndexBackend;

use super::HOLDER_ID;

pub struct KnowledgeEmbeddingPurge<'a, B: IndexBackend> {
    index: &'a Mutex<B>,
    doc_ids: Vec<String>,
    purged: std::sync::atomic::AtomicUsize,
}

impl<'a, B: IndexBackend> KnowledgeEmbeddingPurge<'a, B> {
    pub fn new(index: &'a Mutex<B>, doc_ids: Vec<String>) -> KnowledgeEmbeddingPurge<'a, B> {
        KnowledgeEmbeddingPurge {
            index,
            doc_ids,
            purged: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    pub fn purged_count(&self) -> usize {
        self.purged.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl<B: IndexBackend> SearchPurge for KnowledgeEmbeddingPurge<'_, B> {
    fn purge_and_reindex(
        &self,
        _subject: &SubjectId,
        _tenant: &TenantId,
    ) -> Result<(), EraseError> {
        let mut index = self
            .index
            .lock()
            .map_err(|_| EraseError::SearchPurge("knowledge search index lock poisoned".into()))?;
        for id in &self.doc_ids {
            index.delete(id).map_err(|e| {
                EraseError::SearchPurge(format!("kn embedding purge of `{id}`: {e}"))
            })?;
            self.purged
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(())
    }
}

pub struct KnowledgeBacklinkTombstone<'a> {
    store: &'a Mutex<PageStore>,
    refs: Vec<ArtifactRef>,
    tombstoned: std::sync::atomic::AtomicUsize,
}

impl<'a> KnowledgeBacklinkTombstone<'a> {
    pub fn new(
        store: &'a Mutex<PageStore>,
        refs: Vec<ArtifactRef>,
    ) -> KnowledgeBacklinkTombstone<'a> {
        KnowledgeBacklinkTombstone {
            store,
            refs,
            tombstoned: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    pub fn tombstoned_count(&self) -> usize {
        self.tombstoned.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl RefsTombstone for KnowledgeBacklinkTombstone<'_> {
    fn tombstone(&self, _subject: &SubjectId, _tenant: &TenantId) -> Result<(), EraseError> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| EraseError::RefsTombstone("knowledge page store lock poisoned".into()))?;
        for r in &self.refs {
            store.mark_erased(r);
            self.tombstoned
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnowledgeEraseReceipt {
    pub receipt: Receipt,
    pub recoverable_in_backup: usize,
    pub dek_destroyed_now: bool,
    pub key_shred_count: usize,
    pub embeddings_purged: usize,
    pub backlinks_tombstoned: usize,
    pub crypto_shred_lag_ms: EpochMillis,
    pub re_run: bool,
}

impl KnowledgeEraseReceipt {
    pub fn is_green(&self) -> bool {
        self.recoverable_in_backup == 0
    }
}

pub struct KnowledgeErase<'a> {
    storage: CryptoShredErase<'a>,
}

impl<'a> KnowledgeErase<'a> {
    pub fn new(engine: &'a KmsEngine, region: myelin_tenancy::Region) -> KnowledgeErase<'a> {
        KnowledgeErase {
            storage: CryptoShredErase::new(engine, region),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn erase_subject<B: IndexBackend>(
        &self,
        subject: &SubjectRef,
        tenant: &TenantId,
        pseudonym: &dyn PseudonymShred,
        embeddings: &KnowledgeEmbeddingPurge<'_, B>,
        backlinks: &KnowledgeBacklinkTombstone<'_>,
        bus: &dyn BusEraseSeam,
        ledger: &dyn ErasureLedgerSink,
        now: EpochMillis,
    ) -> Result<KnowledgeEraseReceipt, EraseError> {
        let subject_id = SubjectId::new(subject.principal.principal_id.0.clone());

        let holders = EraseHolders {
            pseudonym,
            search: embeddings,
            refs: backlinks,
            bus: bus.as_storage_seam(),
            ledger,
            git_reach: None,
        };

        let storage_receipt = self.storage.erase(&subject_id, tenant, &holders, now)?;

        let receipt = Receipt::content_addressed(
            "erase",
            HOLDER_ID,
            &subject.principal.principal_id.0,
            tenant.as_str(),
            "kn erase (KN-D4 structural floor): per-subject DEK crypto-shred (free-text unrecoverable in \
             op-log/snapshots/backups, 11.4) + pseudonym-map shred (attribution, 4.8) + embeddings purged \
             in lockstep + backlinks tombstoned (*.erased, 2.7); residual = the ONE platform posture (10.9 \
             by reference, [OPEN - LEGAL] KQ-8)",
            None,
            0,
        );

        Ok(KnowledgeEraseReceipt {
            receipt,
            recoverable_in_backup: storage_receipt.recoverable_in_backup,
            dek_destroyed_now: storage_receipt.dek_destroyed_now,
            key_shred_count: usize::from(storage_receipt.dek_destroyed_now),
            embeddings_purged: embeddings.purged_count(),
            backlinks_tombstoned: backlinks.tombstoned_count(),
            crypto_shred_lag_ms: storage_receipt.crypto_shred_lag_ms,
            re_run: storage_receipt.re_run,
        })
    }
}

pub trait BusEraseSeam {
    fn as_storage_seam(&self) -> &dyn myelin_storage::BusErase;
}

impl<T: myelin_storage::BusErase> BusEraseSeam for T {
    fn as_storage_seam(&self) -> &dyn myelin_storage::BusErase {
        self
    }
}

pub fn holder_erase_receipt(scope: &EraseScope) -> DsrResult<EraseReceipt> {
    let (operation_note, subject_label, tenant_label) = match scope {
        EraseScope::Subject { subject, tenant } => (
            "kn erase(subject): the KN-P26 structural floor - per-subject DEK crypto-shred (11.4) + \
             pseudonym-map shred (4.8) + embeddings purged in lockstep + backlinks tombstoned (2.7); \
             residual = the ONE platform posture (10.9 by reference). The rich seam-wired body is \
             KnowledgeErase::erase_subject.",
            subject.principal.principal_id.0.clone(),
            tenant.as_str().to_string(),
        ),
        EraseScope::Tenant(tenant) => (
            "kn erase(tenant offboarding): the lever is the per-tenant KEK destroy (11.4) - the storage \
             tenant-offboarding path (P-ST-10) owns the KEK destroy; the Knowledge holder records the \
             receipt and defers to it.",
            "<tenant-offboarding>".to_string(),
            tenant.as_str().to_string(),
        ),
    };
    let receipt = Receipt::content_addressed(
        "erase",
        HOLDER_ID,
        &subject_label,
        &tenant_label,
        operation_note,
        None,
        0,
    );
    Ok(EraseReceipt { receipt })
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_query::FieldValue;
    use myelin_search::engine::{AclFilter, Hit, IndexDocument, IndexError};
    use myelin_search::vector::{Embedding, VectorHit};
    use myelin_storage::encryption::{ColumnCryptor, EncryptedColumn};
    use myelin_storage::kms::{DekId, KekId, KeyClass};
    use myelin_tenancy::Region;
    use std::cell::RefCell;
    use std::collections::{BTreeMap, BTreeSet};

    #[derive(Default)]
    struct MemIndex {
        docs: BTreeMap<String, ()>,
    }
    impl IndexBackend for MemIndex {
        fn upsert(&mut self, doc: &IndexDocument) -> Result<(), IndexError> {
            self.docs.insert(doc.doc_id.clone(), ());
            Ok(())
        }
        fn delete(&mut self, doc_id: &str) -> Result<(), IndexError> {
            self.docs.remove(doc_id);
            Ok(())
        }
        fn search(&self, _: &AclFilter, _: &str, _: usize) -> Result<Vec<Hit>, IndexError> {
            Ok(vec![])
        }
        fn search_structured(
            &self,
            _: &AclFilter,
            _: &str,
            _: &FieldValue,
            _: usize,
        ) -> Result<Vec<Hit>, IndexError> {
            Ok(vec![])
        }
        fn semantic(
            &self,
            _: &AclFilter,
            _: &Embedding,
            _: usize,
        ) -> Result<Vec<VectorHit>, IndexError> {
            Ok(vec![])
        }
        fn merge(&mut self) -> Result<(), IndexError> {
            Ok(())
        }
        fn snapshot(&mut self) -> Result<u64, IndexError> {
            Ok(self.docs.len() as u64)
        }
        fn indexed_zookie_of(&self, doc_id: &str) -> Option<String> {
            self.docs.get(doc_id).map(|_| "z0".to_string())
        }
    }

    fn tenant() -> TenantId {
        myelin_tenancy::TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn subject_ref(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            tenant(),
        ))
    }

    #[derive(Default)]
    struct RecPseudonym {
        shredded: RefCell<BTreeSet<String>>,
    }
    impl PseudonymShred for RecPseudonym {
        fn shred_pseudonym(&self, s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            self.shredded.borrow_mut().insert(s.0.clone());
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecBus {
        erased: RefCell<BTreeSet<String>>,
    }
    impl myelin_storage::BusErase for RecBus {
        fn erase_inline_pii(&self, s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            self.erased.borrow_mut().insert(s.0.clone());
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecLedger {
        erased: RefCell<BTreeSet<String>>,
    }
    impl ErasureLedgerSink for RecLedger {
        fn record_erasure(&self, s: &SubjectId, _t: &TenantId, _at: EpochMillis) {
            self.erased.borrow_mut().insert(s.0.clone());
        }
        fn is_erased(&self, s: &SubjectId, _t: &TenantId) -> bool {
            self.erased.borrow().contains(&s.0)
        }
    }

    fn engine_with_subject_freetext(
        subject: &SubjectRef,
        plaintext: &[u8],
    ) -> (KmsEngine, EncryptedColumn) {
        let kms = KmsEngine::new();
        kms.ensure_kek(&KekId::new(tenant(), region()))
            .expect("seed the in-memory KEK");
        let cryptor = ColumnCryptor::new(&kms, region());
        let sid = SubjectId::new(subject.principal.principal_id.0.clone());
        let col = cryptor
            .encrypt(
                &tenant(),
                Some(&sid),
                &myelin_gdpr::ErasureMethod::CryptoShred("subject_dek".into()),
                plaintext,
            )
            .expect("seal a per-subject free-text column under the subject DEK");
        (kms, col)
    }

    fn index_with_subject_docs(doc_ids: &[&str]) -> Mutex<MemIndex> {
        let mut idx = MemIndex::default();
        for id in doc_ids {
            idx.upsert(&IndexDocument::new(*id, "alice's bio + nearest vector"))
                .expect("seed a subject index doc");
        }
        Mutex::new(idx)
    }

    fn store_with_subject_refs() -> Mutex<PageStore> {
        Mutex::new(PageStore::new())
    }

    #[test]
    fn kn_d4_erase_subject_zero_recoverable_pii_including_vectors() {
        let subject = subject_ref("p-alice");
        let (kms, sealed) = engine_with_subject_freetext(&subject, b"alice's home address");
        let cryptor = ColumnCryptor::new(&kms, region());

        assert!(
            cryptor.decrypt(&sealed).is_ok(),
            "the subject's free-text decrypts BEFORE the erase"
        );
        let subject_dek = DekId::new(
            tenant(),
            KeyClass::Subject(subject.principal.principal_id.0.clone()),
        );
        assert!(
            kms.backup_snapshot()
                .unwrap()
                .iter()
                .any(|(d, _)| *d == subject_dek),
            "the subject's per-subject DEK is in the backup BEFORE erase"
        );

        let doc_ids = ["kn:page:home", "kn:block:b1", "kn:vector:v1"];
        let index = index_with_subject_docs(&doc_ids);
        let store = store_with_subject_refs();
        let backlink_refs: Vec<ArtifactRef> = [
            "myelin://acme/knowledge/page/home",
            "myelin://acme/knowledge/page/other#block-b9",
        ]
        .iter()
        .map(|s| ArtifactRef((*s).into()))
        .collect();

        let eraser = KnowledgeErase::new(&kms, region());
        let pseudonym = RecPseudonym::default();
        let bus = RecBus::default();
        let ledger = RecLedger::default();
        let embeddings =
            KnowledgeEmbeddingPurge::new(&index, doc_ids.iter().map(|s| s.to_string()).collect());
        let backlinks = KnowledgeBacklinkTombstone::new(&store, backlink_refs.clone());

        let receipt = eraser
            .erase_subject(
                &subject,
                &tenant(),
                &pseudonym,
                &embeddings,
                &backlinks,
                &bus,
                &ledger,
                1_000,
            )
            .expect("the KN-D4 erase succeeds (every step green)");

        assert!(
            receipt.dek_destroyed_now,
            "the per-subject DEK was destroyed"
        );
        assert_eq!(
            receipt.key_shred_count, 1,
            "ONE key per subject (CR-I), not O(blocks)"
        );
        assert_eq!(
            receipt.recoverable_in_backup, 0,
            "0 of the subject's DEKs recoverable from the backup (the crypto-shred reached backups)"
        );
        assert!(
            cryptor.decrypt(&sealed).is_err(),
            "the subject's free-text is UNRECOVERABLE live after the crypto-shred"
        );
        assert!(
            !kms.backup_snapshot()
                .unwrap()
                .iter()
                .any(|(d, _)| *d == subject_dek),
            "the subject's DEK is ABSENT from the backup after erase (0 recoverable, §7.5)"
        );

        assert_eq!(
            receipt.embeddings_purged, 3,
            "every subject index doc (page + block + vector) was purged in lockstep"
        );
        {
            let idx = index.lock().unwrap();
            for id in doc_ids {
                assert!(
                    idx.indexed_zookie_of(id).is_none(),
                    "the subject's index doc `{id}` is purged (0 vector survives)"
                );
            }
        }

        assert_eq!(receipt.backlinks_tombstoned, 2, "every backlink tombstoned");
        {
            let st = store.lock().unwrap();
            for r in &backlink_refs {
                assert!(
                    st.is_erased(r),
                    "the backlink `{}` is marked ERASED (tombstone, not the content)",
                    r.0
                );
            }
        }

        assert!(
            pseudonym
                .shredded
                .borrow()
                .contains(&subject.principal.principal_id.0),
            "the pseudonym map was shredded (4.8)"
        );
        assert!(
            bus.erased
                .borrow()
                .contains(&subject.principal.principal_id.0),
            "the Bus inline-PII keys shredded + *.erased emitted (2.7)"
        );
        assert!(
            ledger.is_erased(
                &SubjectId::new(subject.principal.principal_id.0.clone()),
                &tenant()
            ),
            "the erasure receipt was recorded into the ledger (10.8)"
        );

        assert!(
            receipt.is_green(),
            "KN-D4 green: 0 recoverable structured PII incl. vectors"
        );
        assert_eq!(receipt.receipt.operation, "erase");
        assert!(receipt.receipt.content_hash.starts_with("blake3:"));
        assert!(!receipt.re_run, "the first erase is not a re-run");
    }

    #[test]
    fn per_subject_dek_destroy_makes_ciphertext_unrecoverable_live_and_in_backup() {
        let subject = subject_ref("p-bob");
        let (kms, sealed) = engine_with_subject_freetext(&subject, b"bob's medical note");
        let cryptor = ColumnCryptor::new(&kms, region());
        assert!(cryptor.decrypt(&sealed).is_ok(), "decrypts before");

        let eraser = KnowledgeErase::new(&kms, region());
        let index = index_with_subject_docs(&[]);
        let store = store_with_subject_refs();
        let embeddings = KnowledgeEmbeddingPurge::new(&index, vec![]);
        let backlinks = KnowledgeBacklinkTombstone::new(&store, vec![]);
        let r = eraser
            .erase_subject(
                &subject,
                &tenant(),
                &RecPseudonym::default(),
                &embeddings,
                &backlinks,
                &RecBus::default(),
                &RecLedger::default(),
                5,
            )
            .unwrap();
        assert!(r.dek_destroyed_now);
        assert_eq!(r.recoverable_in_backup, 0);
        assert!(
            cryptor.decrypt(&sealed).is_err(),
            "unrecoverable after the destroy"
        );
    }

    #[test]
    fn re_erasing_an_already_erased_subject_is_a_noop_success() {
        let subject = subject_ref("p-twice");
        let (kms, _sealed) = engine_with_subject_freetext(&subject, b"bio");
        let eraser = KnowledgeErase::new(&kms, region());
        let index = index_with_subject_docs(&["kn:page:p"]);
        let store = store_with_subject_refs();
        let pseudonym = RecPseudonym::default();
        let bus = RecBus::default();
        let ledger = RecLedger::default();

        let e1 = KnowledgeEmbeddingPurge::new(&index, vec!["kn:page:p".into()]);
        let b1 = KnowledgeBacklinkTombstone::new(&store, vec![]);
        let r1 = eraser
            .erase_subject(&subject, &tenant(), &pseudonym, &e1, &b1, &bus, &ledger, 1)
            .expect("first erase");
        assert!(r1.dek_destroyed_now);
        assert_eq!(r1.key_shred_count, 1);
        assert!(!r1.re_run);

        let e2 = KnowledgeEmbeddingPurge::new(&index, vec!["kn:page:p".into()]);
        let b2 = KnowledgeBacklinkTombstone::new(&store, vec![]);
        let r2 = eraser
            .erase_subject(&subject, &tenant(), &pseudonym, &e2, &b2, &bus, &ledger, 2)
            .expect("re-erase is a no-op SUCCESS, never an error");
        assert!(!r2.dek_destroyed_now, "the DEK was already destroyed");
        assert_eq!(r2.key_shred_count, 0, "a re-run shreds 0 keys this call");
        assert!(r2.re_run, "the second erase is flagged a re-run");
        assert_eq!(r2.recoverable_in_backup, 0, "still 0 recoverable");
        assert!(r2.is_green());
    }

    struct FailingIndex;
    impl IndexBackend for FailingIndex {
        fn upsert(&mut self, _doc: &IndexDocument) -> Result<(), IndexError> {
            Ok(())
        }
        fn delete(&mut self, _doc_id: &str) -> Result<(), IndexError> {
            Err(IndexError::Engine("vector index unavailable".into()))
        }
        fn search(&self, _: &AclFilter, _: &str, _: usize) -> Result<Vec<Hit>, IndexError> {
            Ok(vec![])
        }
        fn search_structured(
            &self,
            _: &AclFilter,
            _: &str,
            _: &FieldValue,
            _: usize,
        ) -> Result<Vec<Hit>, IndexError> {
            Ok(vec![])
        }
        fn semantic(
            &self,
            _: &AclFilter,
            _: &Embedding,
            _: usize,
        ) -> Result<Vec<VectorHit>, IndexError> {
            Ok(vec![])
        }
        fn merge(&mut self) -> Result<(), IndexError> {
            Ok(())
        }
        fn snapshot(&mut self) -> Result<u64, IndexError> {
            Ok(0)
        }
        fn indexed_zookie_of(&self, _doc_id: &str) -> Option<String> {
            None
        }
    }

    #[test]
    fn embedding_purge_failure_aborts_loudly_and_never_records_the_erasure() {
        let subject = subject_ref("p-fail");
        let (kms, _sealed) = engine_with_subject_freetext(&subject, b"bio");
        let eraser = KnowledgeErase::new(&kms, region());
        let index = Mutex::new(FailingIndex);
        let store = store_with_subject_refs();
        let ledger = RecLedger::default();
        let embeddings = KnowledgeEmbeddingPurge::new(&index, vec!["kn:vector:v1".into()]);
        let backlinks = KnowledgeBacklinkTombstone::new(&store, vec![]);

        let err = eraser
            .erase_subject(
                &subject,
                &tenant(),
                &RecPseudonym::default(),
                &embeddings,
                &backlinks,
                &RecBus::default(),
                &ledger,
                1,
            )
            .expect_err(
                "a failed embedding purge is a LOUD error (the index is plaintext-derived PII)",
            );
        assert!(
            matches!(err, EraseError::SearchPurge(_)),
            "the loud error names the Search/embedding purge step"
        );
        assert!(
            !ledger.is_erased(&SubjectId::new("p-fail"), &tenant()),
            "an incomplete erase is NOT recorded as erased"
        );
    }

    #[test]
    fn backlinks_and_embeddings_tombstone_in_lockstep() {
        let subject = subject_ref("p-lock");
        let (kms, _sealed) = engine_with_subject_freetext(&subject, b"bio");
        let eraser = KnowledgeErase::new(&kms, region());
        let doc_ids = ["kn:page:p", "kn:vector:v"];
        let index = index_with_subject_docs(&doc_ids);
        let store = store_with_subject_refs();
        let refs: Vec<ArtifactRef> = ["myelin://acme/knowledge/page/p#block-b1"]
            .iter()
            .map(|s| ArtifactRef((*s).into()))
            .collect();
        let embeddings =
            KnowledgeEmbeddingPurge::new(&index, doc_ids.iter().map(|s| s.to_string()).collect());
        let backlinks = KnowledgeBacklinkTombstone::new(&store, refs.clone());
        let r = eraser
            .erase_subject(
                &subject,
                &tenant(),
                &RecPseudonym::default(),
                &embeddings,
                &backlinks,
                &RecBus::default(),
                &RecLedger::default(),
                1,
            )
            .unwrap();
        assert_eq!(r.embeddings_purged, 2, "both index docs purged");
        assert_eq!(r.backlinks_tombstoned, 1, "the backlink tombstoned");
        let st = store.lock().unwrap();
        assert!(st.is_erased(&refs[0]), "the backlink is ERASED (tombstone)");
    }

    #[test]
    fn cdc_10_1_holder_erase_returns_a_content_addressed_receipt() {
        let subject = subject_ref("p-dsr");
        let sub_receipt = holder_erase_receipt(&EraseScope::Subject {
            subject: subject.clone(),
            tenant: tenant(),
        })
        .expect("subject erase returns a receipt");
        assert_eq!(sub_receipt.receipt.operation, "erase");
        assert!(sub_receipt.receipt.content_hash.starts_with("blake3:"));

        let tenant_receipt = holder_erase_receipt(&EraseScope::Tenant(tenant()))
            .expect("tenant offboarding erase returns a receipt");
        assert_eq!(tenant_receipt.receipt.operation, "erase");
        assert_ne!(
            sub_receipt.receipt.content_hash, tenant_receipt.receipt.content_hash,
            "subject and tenant-offboarding erase receipts are distinct"
        );
    }

    #[test]
    fn receipt_is_green_only_when_zero_recoverable() {
        let red = KnowledgeEraseReceipt {
            receipt: Receipt::content_addressed("erase", HOLDER_ID, "u", "acme", "n", None, 0),
            recoverable_in_backup: 1,
            dek_destroyed_now: true,
            key_shred_count: 1,
            embeddings_purged: 0,
            backlinks_tombstoned: 0,
            crypto_shred_lag_ms: 0,
            re_run: false,
        };
        assert!(!red.is_green(), "non-zero recoverable is RED");
        let green = KnowledgeEraseReceipt {
            recoverable_in_backup: 0,
            ..red
        };
        assert!(green.is_green(), "0 recoverable is GREEN");
    }
}
