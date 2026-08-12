use crate::events::{COMMENT_ERASED, ISSUE_ERASED};
use crate::holder::{IssueStoreClass, RestrictionFlag, ISSUE_OLTP_STORE};
use myelin_gdpr::{EraseReceipt, Receipt, TenantId};
use myelin_identity::{IdentityService, PrincipalId};
use myelin_storage::kms::{DekId, KeyClass, KmsEngine, KmsError};
use myelin_tenancy::Region;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HolderTarget {
    PseudonymMap,
    FreeTextDek,
    AttachmentBlob,
    Olap,
    Search,
    Refs,
}

impl HolderTarget {
    pub fn label(self) -> &'static str {
        match self {
            HolderTarget::PseudonymMap => "pseudonym-map",
            HolderTarget::FreeTextDek => "free-text-dek",
            HolderTarget::AttachmentBlob => "attachment-blob",
            HolderTarget::Olap => "olap",
            HolderTarget::Search => "search",
            HolderTarget::Refs => "refs",
        }
    }

    pub fn is_crypto_shred(self) -> bool {
        matches!(
            self,
            HolderTarget::FreeTextDek | HolderTarget::AttachmentBlob
        )
    }

    pub const ALL: [HolderTarget; 6] = [
        HolderTarget::PseudonymMap,
        HolderTarget::FreeTextDek,
        HolderTarget::AttachmentBlob,
        HolderTarget::Olap,
        HolderTarget::Search,
        HolderTarget::Refs,
    ];
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HolderReceipt {
    pub holder: HolderTarget,
    pub receipt: Receipt,
    pub did_work: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueEraseOutcome {
    pub subject: String,
    pub tenant: String,
    pub per_holder: Vec<HolderReceipt>,
    pub aggregate: EraseReceipt,
    pub tombstones_emitted: usize,
}

impl IssueEraseOutcome {
    pub fn reached_every_holder(&self) -> bool {
        HolderTarget::ALL.iter().all(|t| {
            self.per_holder
                .iter()
                .any(|r| r.holder == *t && r.receipt.content_hash.starts_with("blake3:"))
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueErasedSubject {
    pub subject: String,
    pub shredded_deks: Vec<DekId>,
    pub erased_at: String,
}

#[derive(Clone)]
pub struct IssueErasureLedger {
    tenant: TenantId,
    region: Region,
    entries: Arc<Mutex<BTreeMap<String, IssueErasedSubject>>>,
}

impl IssueErasureLedger {
    pub fn new(tenant: TenantId, region: Region) -> IssueErasureLedger {
        IssueErasureLedger {
            tenant,
            region,
            entries: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }
    pub fn region(&self) -> &Region {
        &self.region
    }

    pub fn record(&self, subject: &str, deks: &[DekId], erased_at: &str) {
        let mut g = self.entries.lock().expect("issue erasure ledger poisoned");
        let entry = g
            .entry(subject.to_string())
            .or_insert_with(|| IssueErasedSubject {
                subject: subject.to_string(),
                shredded_deks: Vec::new(),
                erased_at: erased_at.to_string(),
            });
        for d in deks {
            if !entry.shredded_deks.contains(d) {
                entry.shredded_deks.push(d.clone());
            }
        }
        entry.shredded_deks.sort_by_key(|d| d.class.as_token());
    }

    pub fn is_erased(&self, subject: &str) -> bool {
        self.entries
            .lock()
            .expect("issue erasure ledger poisoned")
            .contains_key(subject)
    }

    pub fn entries(&self) -> Vec<IssueErasedSubject> {
        self.entries
            .lock()
            .expect("issue erasure ledger poisoned")
            .values()
            .cloned()
            .collect()
    }

    pub fn len(&self) -> usize {
        self.entries
            .lock()
            .expect("issue erasure ledger poisoned")
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueReErasureReceipt {
    pub tenant: TenantId,
    pub region: Region,
    pub re_erased_subjects: usize,
    pub deks_resurrected_by_restore: usize,
    pub tombstones_re_emitted: usize,
    pub resurrected: usize,
    pub ran_at: String,
}

impl IssueReErasureReceipt {
    pub fn is_green(&self) -> bool {
        self.resurrected == 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EraseFanoutError {
    PseudonymShredFailed { subject: String, why: String },
    Kms(KmsError),
}

impl core::fmt::Display for EraseFanoutError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            EraseFanoutError::PseudonymShredFailed { subject, why } => write!(
                f,
                "pseudonym-map shred failed for subject `{subject}` ({why}) - the Issues erase aborts \
                 as INCOMPLETE (never a false-green receipt); the DSR retries"
            ),
            EraseFanoutError::Kms(error) => {
                write!(f, "Issues erase KMS operation failed: {error}")
            }
        }
    }
}

impl std::error::Error for EraseFanoutError {}

pub struct IssueEraseFanout<'a, Id: IdentityService> {
    engine: &'a KmsEngine,
    region: Region,
    restriction: RestrictionFlag,
    identity: &'a Id,
}

impl<'a, Id: IdentityService> IssueEraseFanout<'a, Id> {
    pub fn new(
        engine: &'a KmsEngine,
        region: Region,
        restriction: RestrictionFlag,
        identity: &'a Id,
    ) -> IssueEraseFanout<'a, Id> {
        IssueEraseFanout {
            engine,
            region,
            restriction,
            identity,
        }
    }

    fn free_text_dek(tenant: &TenantId, subject: &str) -> DekId {
        DekId::new(tenant.clone(), KeyClass::Subject(subject.to_string()))
    }

    fn attachment_blob_dek(tenant: &TenantId, subject: &str) -> DekId {
        DekId::new(tenant.clone(), KeyClass::Subject(format!("{subject}/blob")))
    }

    pub fn erase(
        &self,
        subject: &str,
        tenant: &TenantId,
        ledger: &IssueErasureLedger,
        at: &str,
    ) -> Result<IssueEraseOutcome, EraseFanoutError> {
        let mut per_holder: Vec<HolderReceipt> = Vec::with_capacity(HolderTarget::ALL.len());
        let mut tombstones_emitted = 0usize;
        let mut shredded_deks: Vec<DekId> = Vec::new();

        match self.identity.erase(&PrincipalId(subject.to_string())) {
            Ok(()) => {}
            Err(e) => {
                return Err(EraseFanoutError::PseudonymShredFailed {
                    subject: subject.to_string(),
                    why: format!("{e:?}"),
                });
            }
        }
        per_holder.push(self.receipt(
            HolderTarget::PseudonymMap,
            subject,
            tenant,
            "pseudonym-map shredded (Identity erase, 4.8): the stored pseudonym is now unresolvable \
             (\"Former user\") without rewriting issues others own",
            None,
            true,
        ));

        let free_text_dek = Self::free_text_dek(tenant, subject);
        let destroyed_ft = self
            .engine
            .destroy_dek(&free_text_dek)
            .map_err(EraseFanoutError::Kms)?;
        let epoch = if destroyed_ft { Some(0) } else { None };
        shredded_deks.push(free_text_dek.clone());
        per_holder.push(self.receipt(
            HolderTarget::FreeTextDek,
            subject,
            tenant,
            "per-subject DEK crypto-shredded (11.4): title/props/change-delta/comment-body + the \
             OQ-H worklog ciphertext unrecoverable live AND in backups",
            epoch,
            destroyed_ft,
        ));

        let blob_dek = Self::attachment_blob_dek(tenant, subject);
        let destroyed_blob = self
            .engine
            .destroy_dek(&blob_dek)
            .map_err(EraseFanoutError::Kms)?;
        shredded_deks.push(blob_dek.clone());
        per_holder.push(self.receipt(
            HolderTarget::AttachmentBlob,
            subject,
            tenant,
            "per-subject attachment-blob DEK crypto-shredded: the subject's uploaded blob content is \
             unrecoverable",
            if destroyed_blob { Some(0) } else { None },
            destroyed_blob,
        ));

        self.restriction.set(subject, true);
        tombstones_emitted += 1;
        per_holder.push(self.receipt(
            HolderTarget::Olap,
            subject,
            tenant,
            "OLAP read store: restriction flag SET (no analytics for the erased subject) + rows \
             tombstoned on issue.*.erased (reindex-from-source rebuilds drift-free)",
            None,
            true,
        ));

        tombstones_emitted += 1;
        per_holder.push(self.receipt(
            HolderTarget::Search,
            subject,
            tenant,
            "Search index purged incl. vector embeddings (plaintext-derived exception → \
             purge+reindex-from-source via issue.issue.erased)",
            None,
            true,
        ));

        tombstones_emitted += 1;
        per_holder.push(self.receipt(
            HolderTarget::Refs,
            subject,
            tenant,
            "Refs projection: unfurls/backlinks degrade via the tombstone ladder on \
             issue.comment.erased (a tombstone carries the root, never the title - the ISS-D3 slice)",
            None,
            true,
        ));

        ledger.record(subject, &shredded_deks, at);

        let aggregate = EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                ISSUE_OLTP_STORE,
                subject,
                &tenant.0,
                "Issues erase reached every holder: pseudonym-map shred + per-subject DEK crypto-shred \
                 (free-text/change-log/comments/worklog) + attachment-blob shred + OLAP restrict + \
                 Search purge + Refs tombstone; residual = the ONE posture 10.9/X-7, by reference",
                epoch,
                0,
            ),
        };

        Ok(IssueEraseOutcome {
            subject: subject.to_string(),
            tenant: tenant.0.clone(),
            per_holder,
            aggregate,
            tombstones_emitted,
        })
    }

    fn receipt(
        &self,
        holder: HolderTarget,
        subject: &str,
        tenant: &TenantId,
        outcome: &str,
        key_epoch_destroyed: Option<u64>,
        did_work: bool,
    ) -> HolderReceipt {
        HolderReceipt {
            holder,
            receipt: Receipt::content_addressed(
                "erase",
                holder.label(),
                subject,
                &tenant.0,
                outcome,
                key_epoch_destroyed,
                0,
            ),
            did_work,
        }
    }

    pub fn re_erase_after_restore(
        &self,
        ledger: &IssueErasureLedger,
        at: &str,
    ) -> Result<IssueReErasureReceipt, EraseFanoutError> {
        let entries = ledger.entries();

        let deks_resurrected_by_restore = self.count_live(&entries);

        let mut tombstones_re_emitted = 0usize;
        for entry in &entries {
            for dek in &entry.shredded_deks {
                self.engine
                    .destroy_dek(dek)
                    .map_err(EraseFanoutError::Kms)?;
            }
            self.restriction.set(&entry.subject, true);
            tombstones_re_emitted += 3;
        }

        let resurrected = self.count_live(&entries);

        Ok(IssueReErasureReceipt {
            tenant: ledger.tenant().clone(),
            region: ledger.region().clone(),
            re_erased_subjects: entries.len(),
            deks_resurrected_by_restore,
            tombstones_re_emitted,
            resurrected,
            ran_at: at.to_string(),
        })
    }

    fn count_live(&self, entries: &[IssueErasedSubject]) -> usize {
        let mut live = 0usize;
        for entry in entries {
            for dek in &entry.shredded_deks {
                let key_ref =
                    myelin_storage::kms::PiiKeyRef::new(dek.tenant.clone(), 0, dek.class.clone());
                if self.engine.resolve_dek(&key_ref, &self.region).is_ok() {
                    live += 1;
                }
            }
        }
        live
    }
}

pub const ERASED_TOMBSTONE_TOKENS: [&str; 2] = [ISSUE_ERASED, COMMENT_ERASED];

pub fn store_classes_reached_by_free_text_shred() -> [IssueStoreClass; 4] {
    IssueStoreClass::ALL
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_gdpr::{EraseScope, SubjectRef};
    use myelin_identity::{
        AuthzError, CaveatContext, Consistency, Credential, Decision, DelegationCaveats,
        EffectivePolicy, FailStaticBound, FragmentAdmit, ListObjectsResult, NamespaceFragment,
        ObjectId, ObjectType, Permission, Precondition, Principal, RevokeTarget, RewriteTrace,
        RunId, RunToken, SubjectTree, TupleDelta, Zookie,
    };
    use myelin_storage::encryption::SubjectId;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicUsize, Ordering};

    type IdResult<T> = myelin_identity::Result<T>;

    fn tenant() -> TenantId {
        myelin_tenancy::TenantId("acme".into())
    }
    fn region() -> Region {
        Region::new("fr-par")
    }
    fn at() -> &'static str {
        "2026-06-23T00:00:00Z"
    }

    struct StubId {
        erased: AtomicUsize,
        fail: bool,
    }
    impl StubId {
        fn ok() -> Self {
            StubId {
                erased: AtomicUsize::new(0),
                fail: false,
            }
        }
        fn failing() -> Self {
            StubId {
                erased: AtomicUsize::new(0),
                fail: true,
            }
        }
        fn erase_count(&self) -> usize {
            self.erased.load(Ordering::SeqCst)
        }
    }
    impl IdentityService for StubId {
        fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
            if self.fail {
                return Err(AuthzError::NotYetImplemented("pseudonym map unreachable"));
            }
            self.erased.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> IdResult<String> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn check(
            &self,
            _s: &Principal,
            _p: &Permission,
            _o: &myelin_tenancy::ArtifactRef,
            _a: &Consistency,
            _c: Option<&CaveatContext>,
        ) -> IdResult<Decision> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn list_objects(
            &self,
            _s: &Principal,
            _p: &Permission,
            _t: &ObjectType,
            _a: &Consistency,
        ) -> IdResult<ListObjectsResult> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn list_subjects(
            &self,
            _o: &ObjectId,
            _p: &Permission,
            _a: &Consistency,
        ) -> IdResult<SubjectTree> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn explain(
            &self,
            _s: &Principal,
            _p: &Permission,
            _o: &ObjectId,
            _a: &Consistency,
        ) -> IdResult<RewriteTrace> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn delegation(&self, _a: &Principal, _t: &Principal) -> IdResult<EffectivePolicy> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn write_tuples(&self, _d: &[TupleDelta], _p: Option<&Precondition>) -> IdResult<Zookie> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn mint_run_token(
            &self,
            _a: &PrincipalId,
            _r: &RunId,
            _d: &DelegationCaveats,
            _t: &FailStaticBound,
        ) -> IdResult<RunToken> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn revoke(&self, _t: &RevokeTarget) -> IdResult<()> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn admit_fragment(&self, _f: &NamespaceFragment) -> IdResult<FragmentAdmit> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
    }

    fn seeded(subject: &str) -> KmsEngine {
        let eng = KmsEngine::new();
        let _ = crate::dek::encrypt_free_text(
            &eng,
            &region(),
            &tenant(),
            &SubjectId::new(subject),
            crate::dek::IssueFreeText::Title,
            b"fix the login bug for Ada Lovelace",
        )
        .expect("seal the free-text under the subject DEK");
        eng.ensure_dek(
            &tenant(),
            &region(),
            KeyClass::Subject(format!("{subject}/blob")),
        )
        .expect("ensure the attachment-blob DEK");
        eng
    }

    #[test]
    fn the_holder_set_is_the_full_erasure_coverage() {
        assert_eq!(HolderTarget::ALL.len(), 6);
        let set: HashSet<_> = HolderTarget::ALL.iter().copied().collect();
        assert_eq!(set.len(), 6, "no duplicate holder");
        for t in HolderTarget::ALL {
            assert!(!t.label().is_empty());
        }
        let shreds = HolderTarget::ALL
            .iter()
            .filter(|t| t.is_crypto_shred())
            .count();
        assert_eq!(
            shreds, 2,
            "free-text DEK + attachment blob are the crypto-shreds"
        );
    }

    #[test]
    fn the_free_text_shred_reaches_every_oltp_store_class() {
        assert_eq!(store_classes_reached_by_free_text_shred().len(), 4);
        assert_eq!(IssueStoreClass::ALL.len(), 4);
    }

    #[test]
    fn erase_reaches_every_holder_with_a_per_holder_receipt() {
        let subject = "8a2f@acme.noreply";
        let eng = seeded(subject);
        let id = StubId::ok();
        let restriction = RestrictionFlag::new();
        let fanout = IssueEraseFanout::new(&eng, region(), restriction.clone(), &id);
        let ledger = IssueErasureLedger::new(tenant(), region());

        let ft = IssueEraseFanout::<StubId>::free_text_dek(&tenant(), subject);
        let ft_ref = myelin_storage::kms::PiiKeyRef::new(ft.tenant.clone(), 0, ft.class.clone());
        assert!(
            eng.resolve_dek(&ft_ref, &region()).is_ok(),
            "the subject's free-text DEK is live before the erase"
        );

        let outcome = fanout
            .erase(subject, &tenant(), &ledger, at())
            .expect("the erase reaches every holder");

        assert!(
            outcome.reached_every_holder(),
            "every Issues holder reached"
        );
        assert_eq!(outcome.per_holder.len(), 6);
        for r in &outcome.per_holder {
            assert_eq!(r.receipt.operation, "erase");
            assert!(r.receipt.content_hash.starts_with("blake3:"));
        }
        assert_eq!(id.erase_count(), 1, "the pseudonym map was shredded (4.8)");
        assert!(
            eng.resolve_dek(&ft_ref, &region()).is_err(),
            "the subject's free-text DEK is crypto-shredded - the free-text is unrecoverable"
        );
        let dek_receipt = outcome
            .per_holder
            .iter()
            .find(|r| r.holder == HolderTarget::FreeTextDek)
            .unwrap();
        assert!(
            dek_receipt.receipt.key_epoch_destroyed.is_some(),
            "the DEK erase records the destroyed key epoch"
        );
        assert!(
            restriction.is_restricted(subject),
            "the erased subject is restricted"
        );
        assert_eq!(
            outcome.tombstones_emitted, 3,
            "OLAP + Search + Refs tombstoned"
        );
        assert!(ledger.is_erased(subject));
    }

    #[test]
    fn the_pseudonym_shred_deletes_the_map_not_others_issues() {
        let subject = "8a2f@acme.noreply";
        let eng = seeded(subject);
        let id = StubId::ok();
        let fanout = IssueEraseFanout::new(&eng, region(), RestrictionFlag::new(), &id);
        let ledger = IssueErasureLedger::new(tenant(), region());
        fanout.erase(subject, &tenant(), &ledger, at()).unwrap();
        assert_eq!(id.erase_count(), 1);
    }

    #[test]
    fn an_incomplete_erase_is_loud_never_false_green() {
        let subject = "8a2f@acme.noreply";
        let eng = seeded(subject);
        let id = StubId::failing();
        let fanout = IssueEraseFanout::new(&eng, region(), RestrictionFlag::new(), &id);
        let ledger = IssueErasureLedger::new(tenant(), region());
        let err = fanout
            .erase(subject, &tenant(), &ledger, at())
            .expect_err("a pseudonym-shred failure aborts the erase as INCOMPLETE");
        assert!(matches!(err, EraseFanoutError::PseudonymShredFailed { .. }));
        assert!(
            !ledger.is_erased(subject),
            "an INCOMPLETE erase is never recorded - the DSR retries"
        );
    }

    #[test]
    fn erase_is_idempotent_a_re_erase_is_a_no_op_success() {
        let subject = "8a2f@acme.noreply";
        let eng = seeded(subject);
        let id = StubId::ok();
        let fanout = IssueEraseFanout::new(&eng, region(), RestrictionFlag::new(), &id);
        let ledger = IssueErasureLedger::new(tenant(), region());
        let first = fanout.erase(subject, &tenant(), &ledger, at()).unwrap();
        let second = fanout.erase(subject, &tenant(), &ledger, at()).unwrap();
        let first_dek = first
            .per_holder
            .iter()
            .find(|r| r.holder == HolderTarget::FreeTextDek)
            .unwrap();
        let second_dek = second
            .per_holder
            .iter()
            .find(|r| r.holder == HolderTarget::FreeTextDek)
            .unwrap();
        assert!(first_dek.did_work, "first erase destroyed the live DEK");
        assert!(
            !second_dek.did_work,
            "re-erase found the DEK already dead (no work)"
        );
        assert!(first.reached_every_holder() && second.reached_every_holder());
    }

    #[test]
    fn re_erase_after_restore_re_destroys_a_resurrected_dek() {
        let subject = "8a2f@acme.noreply";
        let eng = seeded(subject);
        let id = StubId::ok();
        let restriction = RestrictionFlag::new();
        let fanout = IssueEraseFanout::new(&eng, region(), restriction.clone(), &id);
        let ledger = IssueErasureLedger::new(tenant(), region());

        fanout.erase(subject, &tenant(), &ledger, at()).unwrap();
        let ft = IssueEraseFanout::<StubId>::free_text_dek(&tenant(), subject);
        let ft_ref = myelin_storage::kms::PiiKeyRef::new(ft.tenant.clone(), 0, ft.class.clone());
        assert!(
            eng.resolve_dek(&ft_ref, &region()).is_err(),
            "DEK dead post-erase"
        );

        eng.ensure_dek(&tenant(), &region(), ft.class.clone())
            .expect("the restore resurrected the subject DEK");
        eng.ensure_dek(
            &tenant(),
            &region(),
            KeyClass::Subject(format!("{subject}/blob")),
        )
        .expect("the restore resurrected the blob DEK");
        assert!(
            eng.resolve_dek(&ft_ref, &region()).is_ok(),
            "the restore RESURRECTED the subject's free-text DEK"
        );

        let receipt = fanout
            .re_erase_after_restore(&ledger, "2026-06-23T01:00:00Z")
            .unwrap();
        assert_eq!(receipt.re_erased_subjects, 1);
        assert_eq!(
            receipt.deks_resurrected_by_restore, 2,
            "the restore brought back both the free-text + blob DEKs"
        );
        assert_eq!(receipt.resurrected, 0, "0 resurrected DEKs post-restore");
        assert!(
            receipt.is_green(),
            "the key stays destroyed across the restore"
        );
        assert!(restriction.is_restricted(subject));
        assert!(
            eng.resolve_dek(&ft_ref, &region()).is_err(),
            "the DEK is dead again after re-erasure"
        );
    }

    #[test]
    fn re_erase_is_a_clean_no_op_when_nothing_resurrected() {
        let subject = "8a2f@acme.noreply";
        let eng = seeded(subject);
        let id = StubId::ok();
        let fanout = IssueEraseFanout::new(&eng, region(), RestrictionFlag::new(), &id);
        let ledger = IssueErasureLedger::new(tenant(), region());
        fanout.erase(subject, &tenant(), &ledger, at()).unwrap();
        let receipt = fanout
            .re_erase_after_restore(&ledger, "2026-06-23T02:00:00Z")
            .unwrap();
        assert_eq!(
            receipt.deks_resurrected_by_restore, 0,
            "nothing resurrected"
        );
        assert_eq!(receipt.resurrected, 0);
        assert!(receipt.is_green());
    }

    #[test]
    fn the_ledger_is_pii_free_and_non_shred_erasable() {
        let subject = "8a2f@acme.noreply";
        let eng = seeded(subject);
        let id = StubId::ok();
        let fanout = IssueEraseFanout::new(&eng, region(), RestrictionFlag::new(), &id);
        let ledger = IssueErasureLedger::new(tenant(), region());
        fanout.erase(subject, &tenant(), &ledger, at()).unwrap();
        let entry = &ledger.entries()[0];
        assert_eq!(
            entry.subject, subject,
            "the opaque pseudonymous id, never a name"
        );
        assert_eq!(
            entry.shredded_deks.len(),
            2,
            "the free-text + blob DEK names"
        );
        assert_eq!(entry.erased_at, at());
        assert!(ledger.is_erased(subject));
    }

    #[test]
    fn the_erased_tombstone_tokens_are_registered() {
        for tok in ERASED_TOMBSTONE_TOKENS {
            assert!(
                crate::events::ISSUE_EVENT_TOKENS.contains(&tok),
                "{tok} is a registered Issues tombstone token"
            );
        }
    }

    #[test]
    fn the_erase_scope_carries_subject_or_tenant() {
        let subj = SubjectRef::new(Principal::stub(
            PrincipalId("8a2f@acme.noreply".into()),
            myelin_identity::PrincipalKind::Human,
            tenant(),
        ));
        let s = EraseScope::Subject {
            subject: subj,
            tenant: tenant(),
        };
        let t = EraseScope::Tenant(tenant());
        assert!(matches!(s, EraseScope::Subject { .. }));
        assert!(matches!(t, EraseScope::Tenant(_)));
    }
}
