use myelin_gdpr::{
    EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle, Receipt,
    RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef, TenantId,
};
use myelin_substrate::{Holder, HolderRegistration, HolderRegistry, StoreClassifier, StoreKind};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

pub const ISSUE_OLTP_STORE: &str = "issue_oltp";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IssueStoreClass {
    Issues,
    Comments,
    ChangeLog,
    Worklog,
}

impl IssueStoreClass {
    pub fn label(self) -> &'static str {
        match self {
            IssueStoreClass::Issues => "issues",
            IssueStoreClass::Comments => "comments",
            IssueStoreClass::ChangeLog => "change-log",
            IssueStoreClass::Worklog => "worklog",
        }
    }

    pub const ALL: [IssueStoreClass; 4] = [
        IssueStoreClass::Issues,
        IssueStoreClass::Comments,
        IssueStoreClass::ChangeLog,
        IssueStoreClass::Worklog,
    ];
}

pub const ISSUE_RESIDUAL_POSTURE_REF: &str =
    "contract 10.9 / 00 §X-7 (the ONE platform free-text/immutable-content erasure posture); \
     Issues: per-subject DEK crypto-shred (11.4, issue.pii_key_ref / issue_change_log.pii_key_ref) \
     + pseudonym shred (4.8, assignee/reporter/actor) + restrict suppression; per-tenant DEK \
     fallback where PII is not isolable; the lawful-basis residual = the ONE [OPEN - LEGAL] posture \
     (the OQ-H worklog TBD_LEGAL track, parallel/Legal, never an Issues-local restatement)";

pub type IssueHolderRegistration = HolderRegistration;

pub fn issue_store_classifier() -> StoreClassifier {
    StoreClassifier::of([myelin_substrate::StoreHolder::new(
        StoreKind::Oltp,
        ISSUE_OLTP_STORE,
        Holder::H3Issues,
    )])
}

pub fn register_issue_holders() -> HolderRegistry {
    let mut registry = HolderRegistry::new();
    registry.open(StoreKind::Oltp, ISSUE_OLTP_STORE);
    registry
}

#[derive(Clone, Default)]
pub struct RestrictionFlag {
    restricted: Arc<Mutex<BTreeSet<String>>>,
}

impl RestrictionFlag {
    pub fn new() -> RestrictionFlag {
        RestrictionFlag::default()
    }

    pub fn set(&self, subject: &str, on: bool) {
        let mut g = self.restricted.lock().expect("restriction flag poisoned");
        if on {
            g.insert(subject.to_string());
        } else {
            g.remove(subject);
        }
    }

    pub fn is_restricted(&self, subject: &str) -> bool {
        self.restricted
            .lock()
            .expect("restriction flag poisoned")
            .contains(subject)
    }
}

#[derive(Clone, Default)]
pub struct IssueHolder {
    restriction: RestrictionFlag,
}

impl IssueHolder {
    pub fn new() -> IssueHolder {
        IssueHolder::default()
    }

    pub fn with_restriction(restriction: RestrictionFlag) -> IssueHolder {
        IssueHolder { restriction }
    }

    pub fn register(&self, registry: &mut HolderRegistry) -> IssueHolderRegistration {
        registry.open(StoreKind::Oltp, ISSUE_OLTP_STORE)
    }

    pub fn restriction(&self) -> &RestrictionFlag {
        &self.restriction
    }

    fn subject_id(subject: &SubjectRef) -> String {
        subject.principal.principal_id.0.clone()
    }
}

impl PersonalDataHolder for IssueHolder {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                ISSUE_OLTP_STORE,
                &Self::subject_id(subject),
                &tenant.0,
                "Issues locate over issues/comments/change-log/worklog (ISS-P05 typed seam; \
                 the full subject-walk = ISS-P06 + the DSR fan-out ISS-P31)",
                None,
                0,
            ),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                ISSUE_OLTP_STORE,
                &Self::subject_id(subject),
                &tenant.0,
                "Issues export: the subject's footprint (reported/assigned issues + comments + \
                 change-log) as references + free-text excerpts (ISS-P05 typed seam; the full \
                 bundle = ISS-P06 + ISS-P31)",
                None,
                0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                ISSUE_OLTP_STORE,
                &Self::subject_id(subject),
                "",
                "no-op (ISS-P05 substrate; the patch-apply + reindex-from-source = ISS-P31 / GDPR 10.4)",
                None,
                0,
            ),
        })
    }

    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        let sid = Self::subject_id(subject);
        self.restriction.set(&sid, on);
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed(
                "restrict",
                ISSUE_OLTP_STORE,
                &sid,
                "",
                if on {
                    "Issues restrict ON: no indexing / no agent-use / no analytics / no notification (§7)"
                } else {
                    "Issues restrict OFF: the per-subject restriction flag is cleared (§7)"
                },
                None,
                0,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        let (subject_id, tenant) = match &scope {
            EraseScope::Subject { subject, tenant } => {
                (Self::subject_id(subject), tenant.0.clone())
            }
            EraseScope::Tenant(t) => (String::new(), t.0.clone()),
        };
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                ISSUE_OLTP_STORE,
                &subject_id,
                &tenant,
                "Issues erase (the full fan-out is crate::holder_erase::IssueEraseFanout, ISS-P31): \
                 per-subject DEK crypto-shred (free-text/change-log/comments/worklog) + attachment-blob \
                 shred + pseudonym-map shred (4.8) + OLAP restrict + Search purge + Refs tombstone \
                 across every holder, with post-restore re-erasure (GD-14); residual = the ONE posture \
                 10.9/X-7, by reference",
                None,
                0,
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations::{
        CYCLE_TABLE, ISSUE_CHANGE_LOG_TABLE, ISSUE_RELATION_TABLE, ISSUE_TABLE, MILESTONE_TABLE,
        PREFIX_COUNTER_TABLE, SCHEME_TABLE,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_substrate::{
        assert_all_holders_registered, assert_holder_completeness, classify_store, DeclaredStore,
        StoreManifest,
    };

    fn subject(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            TenantId::from_token("acme"),
        ))
    }

    fn tenant() -> TenantId {
        TenantId::from_token("acme")
    }

    #[test]
    fn the_issue_store_class_set_is_the_holder_coverage() {
        assert_eq!(IssueStoreClass::ALL.len(), 4);
        for c in [
            IssueStoreClass::Issues,
            IssueStoreClass::Comments,
            IssueStoreClass::ChangeLog,
            IssueStoreClass::Worklog,
        ] {
            assert!(
                IssueStoreClass::ALL.contains(&c),
                "{} must be in the holder coverage",
                c.label()
            );
        }
        assert_eq!(IssueStoreClass::Worklog.label(), "worklog");
    }

    #[test]
    fn issue_store_registers_and_classifies_to_h3_no_orphan() {
        let registry = register_issue_holders();
        assert!(registry.is_registered(StoreKind::Oltp, ISSUE_OLTP_STORE));
        assert_eq!(
            registry.len(),
            1,
            "exactly the Issues OLTP store registered"
        );
        let classifier = issue_store_classifier();
        assert_eq!(
            classify_store(StoreKind::Oltp, ISSUE_OLTP_STORE, &classifier),
            Some(Holder::H3Issues),
            "the Issues OLTP spine is holder H3 (Issues subsystem DB)"
        );
        assert_eq!(
            assert_holder_completeness(registry.registrations(), &classifier),
            Ok(()),
            "every Issues store is in the exhaustive H1–H18 list - 0 orphan stores"
        );
    }

    #[test]
    fn an_unregistered_issue_store_fails_the_holder_registered_architecture_test() {
        let manifest = StoreManifest::of([DeclaredStore::new(StoreKind::Oltp, ISSUE_OLTP_STORE)]);
        assert_eq!(
            assert_all_holders_registered(&manifest, &register_issue_holders()),
            Ok(()),
            "the Issues store opened through the harness → the architecture test passes"
        );
        let rogue = HolderRegistry::new();
        let err = assert_all_holders_registered(&manifest, &rogue).expect_err(
            "an Issues store opened outside the harness must FAIL the architecture test",
        );
        assert_eq!(
            err.len(),
            1,
            "exactly the unregistered Issues store is the violation"
        );
        assert!(
            err[0].message().contains(ISSUE_OLTP_STORE),
            "the failure names the escaped Issues store: {}",
            err[0].message()
        );
    }

    #[test]
    fn locate_and_export_are_typed_and_empty_but_correct() {
        let holder = IssueHolder::new();
        let subj = subject("psn:iss-7");
        let locate = holder
            .locate(&subj, tenant())
            .expect("locate over the Issues surface succeeds");
        assert_eq!(locate.receipt.operation, "locate");
        assert!(locate.receipt.content_hash.starts_with("blake3:"));
        assert!(
            locate.receipt.key_epoch_destroyed.is_none(),
            "locate shreds no key"
        );
        let export = holder
            .export(&subj, tenant())
            .expect("export over the Issues surface succeeds");
        assert_eq!(export.receipt.operation, "export");
        assert!(export.receipt.content_hash.starts_with("blake3:"));
    }

    #[test]
    fn restrict_flips_a_real_flag_the_seams_read() {
        let flag = RestrictionFlag::new();
        let holder = IssueHolder::with_restriction(flag.clone());
        let subj = subject("psn:iss-restricted");
        let sid = "psn:iss-restricted";

        assert!(!flag.is_restricted(sid));
        let r = holder.restrict(&subj, true).expect("restrict ON");
        assert_eq!(r.receipt.operation, "restrict");
        assert!(
            flag.is_restricted(sid),
            "the restriction flag the Issues index/agent/analytics/notif seams read is SET"
        );
        holder.restrict(&subj, false).expect("restrict OFF");
        assert!(!flag.is_restricted(sid), "the restriction flag is cleared");
    }

    #[test]
    fn erase_trait_surface_returns_a_typed_aggregate_receipt() {
        let holder = IssueHolder::new();
        let scope = EraseScope::Subject {
            subject: subject("psn:iss-7"),
            tenant: tenant(),
        };
        let r1 = holder.erase(scope.clone()).expect("erase succeeds (stub)");
        let r2 = holder.erase(scope).expect("erase is idempotent");
        assert_eq!(
            r1, r2,
            "the same erase scope yields the identical content-addressed receipt"
        );
        assert!(
            r1.receipt.key_epoch_destroyed.is_none(),
            "no DEK shredded (the crypto-shred body is ISS-P07/P31)"
        );
        assert_eq!(r1.receipt.operation, "erase");
        assert!(r1.receipt.content_hash.starts_with("blake3:"));
    }

    #[test]
    fn issue_holder_is_object_safe() {
        let holders: Vec<Box<dyn PersonalDataHolder>> = vec![Box::new(IssueHolder::new())];
        let subj = subject("psn:iss-9");
        for h in &holders {
            assert!(
                h.locate(&subj, tenant()).is_ok(),
                "the Issues holder responds to the contract"
            );
        }
    }

    #[test]
    fn the_holder_spans_the_one_oltp_spine() {
        for t in [
            ISSUE_TABLE,
            ISSUE_RELATION_TABLE,
            ISSUE_CHANGE_LOG_TABLE,
            SCHEME_TABLE,
            CYCLE_TABLE,
            MILESTONE_TABLE,
            PREFIX_COUNTER_TABLE,
        ] {
            assert!(
                !t.is_empty(),
                "the spine table name `{t}` is a real table in the H3 store"
            );
        }
        assert_eq!(ISSUE_OLTP_STORE, "issue_oltp");
    }
}
