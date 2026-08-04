use myelin_gdpr::{
    EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle, Receipt,
    RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef, TenantId,
};
use myelin_substrate::{Holder, HolderRegistration, HolderRegistry, StoreClassifier, StoreKind};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

pub const CI_OLTP_STORE: &str = "ci_oltp";

pub const ERASED_OUTCOME_NONE_REMAIN: &str =
    "CI crypto-shred complete: per-subject/per-tenant DEK destroyed + identity pseudonymised + \
     ci.*.erased tombstones; 0 recoverable PII incl. backups; structure survives (CI-D3)";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CiStoreClass {
    RunState,
    Logs,
    Artifacts,
    Caches,
    Deployments,
}

impl CiStoreClass {
    pub fn label(self) -> &'static str {
        match self {
            CiStoreClass::RunState => "run-state",
            CiStoreClass::Logs => "logs",
            CiStoreClass::Artifacts => "artifacts",
            CiStoreClass::Caches => "caches",
            CiStoreClass::Deployments => "deployments",
        }
    }

    pub fn store_kind(self) -> StoreKind {
        match self {
            CiStoreClass::RunState | CiStoreClass::Deployments => StoreKind::Oltp,
            CiStoreClass::Logs | CiStoreClass::Artifacts => StoreKind::Blob,
            CiStoreClass::Caches => StoreKind::Cache,
        }
    }

    pub const ALL: [CiStoreClass; 5] = [
        CiStoreClass::RunState,
        CiStoreClass::Logs,
        CiStoreClass::Artifacts,
        CiStoreClass::Caches,
        CiStoreClass::Deployments,
    ];
}

pub const CI_RESIDUAL_POSTURE_REF: &str =
    "contract 10.9 / 00 §X-7 (the ONE platform free-text/immutable-content erasure posture); \
     CI: per-subject DEK crypto-shred (11.4, log_segment.pii_key_ref) + pseudonym shred (4.8, \
     triggered_by/approved_by) + restrict suppression; per-tenant DEK fallback where PII is not \
     isolable; the lawful-basis residual = the ONE [OPEN - LEGAL] posture (parallel/Legal, never a \
     CI-local restatement)";

pub type CiHolderRegistration = HolderRegistration;

pub fn ci_store_classifier() -> StoreClassifier {
    StoreClassifier::of([
        myelin_substrate::StoreHolder::new(StoreKind::Oltp, CI_OLTP_STORE, Holder::H2Ci),
    ])
}

pub fn register_ci_holders() -> HolderRegistry {
    let mut registry = HolderRegistry::new();
    registry.open(StoreKind::Oltp, CI_OLTP_STORE);
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
pub struct CiHolder {
    restriction: RestrictionFlag,
}

impl CiHolder {
    pub fn new() -> CiHolder {
        CiHolder::default()
    }

    pub fn with_restriction(restriction: RestrictionFlag) -> CiHolder {
        CiHolder { restriction }
    }

    pub fn register(&self, registry: &mut HolderRegistry) -> CiHolderRegistration {
        registry.open(StoreKind::Oltp, CI_OLTP_STORE)
    }

    pub fn restriction(&self) -> &RestrictionFlag {
        &self.restriction
    }

    #[allow(clippy::too_many_arguments)]
    pub fn erase_with_fanout(
        &self,
        subject: &SubjectRef,
        tenant: TenantId,
        footprint: &crate::crypto_shred_erase::CiSubjectFootprint,
        kms: &myelin_storage::kms::KmsEngine,
        region: myelin_tenancy::Region,
        store: &mut crate::surfacing::ArtifactStore,
    ) -> DsrResult<EraseReceipt> {
        let fanout = crate::crypto_shred_erase::CiEraseFanOut::new(kms, region);
        let scope = EraseScope::Subject {
            subject: subject.clone(),
            tenant: tenant.clone(),
        };
        let (ci, _tombstones) = fanout
            .erase_subject(&Self::subject_id(subject), &tenant, footprint, store)
            .map_err(|e| myelin_gdpr::DsrError(e.to_string()))?;
        Ok(crate::crypto_shred_erase::CiEraseFanOut::holder_receipt(
            &scope, &ci,
        ))
    }

    fn subject_id(subject: &SubjectRef) -> String {
        subject.principal.principal_id.0.clone()
    }
}

impl PersonalDataHolder for CiHolder {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                CI_OLTP_STORE,
                &Self::subject_id(subject),
                &tenant.0,
                "CI locate over run-state/logs/artifacts/caches/deployments (CI-P9 typed seam; \
                 the full subject-walk = CI-P20/P22 + the DSR fan-out CI-P32)",
                None,
                0,
            ),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                CI_OLTP_STORE,
                &Self::subject_id(subject),
                &tenant.0,
                "CI export: the subject's CI footprint (triggered runs + approvals) as references + \
                 log excerpts (CI-P9 typed seam; the full bundle = CI-P20/P22 + CI-P32)",
                None,
                0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                CI_OLTP_STORE,
                &Self::subject_id(subject),
                "",
                "no-op (CI run-state/logs are machine-emitted; rectify-by-reindex = GDPR P-GA-24)",
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
                CI_OLTP_STORE,
                &sid,
                "",
                if on {
                    "CI restrict ON: no indexing / no agent-use / no analytics / no notification (§6)"
                } else {
                    "CI restrict OFF: the per-subject restriction flag is cleared (§6)"
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
                CI_OLTP_STORE,
                &subject_id,
                &tenant,
                "seam: the destructive crypto-shred fan-out runs via CiHolder::erase_with_fanout \
                 (CI-P32 / CI-D3 - per-subject/per-tenant DEK crypto-shred + pseudonym shred + \
                 ci.*.erased tombstones over run-state/logs/artifacts/caches/deployments, 0 \
                 recoverable incl. backups; residual = the ONE posture 10.9/X-7, by reference)",
                None,
                0,
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn the_ci_store_class_set_is_the_holder_coverage() {
        assert_eq!(CiStoreClass::ALL.len(), 5);
        for c in [
            CiStoreClass::RunState,
            CiStoreClass::Logs,
            CiStoreClass::Artifacts,
            CiStoreClass::Caches,
            CiStoreClass::Deployments,
        ] {
            assert!(
                CiStoreClass::ALL.contains(&c),
                "{} must be in the holder coverage",
                c.label()
            );
        }
        assert_eq!(CiStoreClass::RunState.store_kind(), StoreKind::Oltp);
        assert_eq!(CiStoreClass::Deployments.store_kind(), StoreKind::Oltp);
        assert_eq!(CiStoreClass::Logs.store_kind(), StoreKind::Blob);
        assert_eq!(CiStoreClass::Artifacts.store_kind(), StoreKind::Blob);
        assert_eq!(CiStoreClass::Caches.store_kind(), StoreKind::Cache);
        assert_eq!(CiStoreClass::Logs.label(), "logs");
    }

    #[test]
    fn ci_store_registers_and_classifies_to_h2_no_orphan() {
        let registry = register_ci_holders();
        assert!(registry.is_registered(StoreKind::Oltp, CI_OLTP_STORE));
        assert_eq!(registry.len(), 1, "exactly the CI OLTP store registered");
        let classifier = ci_store_classifier();
        assert_eq!(
            classify_store(StoreKind::Oltp, CI_OLTP_STORE, &classifier),
            Some(Holder::H2Ci),
            "the CI OLTP schema is holder H2 (CI subsystem DB + log segments)"
        );
        assert_eq!(
            assert_holder_completeness(registry.registrations(), &classifier),
            Ok(()),
            "every CI store is in the exhaustive H1–H18 list - 0 orphan stores"
        );
    }

    #[test]
    fn ci_blob_and_cache_stores_classify_structurally_no_forgotten_table() {
        let classifier = ci_store_classifier();
        assert_eq!(
            classify_store(StoreKind::Blob, "ci_logs", &classifier),
            Some(Holder::H6BlobStore),
            "CI log/artifact blob tier classifies structurally to H6"
        );
        assert_eq!(
            classify_store(StoreKind::Cache, "ci_cache", &classifier),
            Some(Holder::H9Caches),
            "CI cache tier classifies structurally to H9 (no forgotten cache table)"
        );
    }

    #[test]
    fn an_unregistered_ci_store_fails_the_holder_registered_architecture_test() {
        let manifest = StoreManifest::of([DeclaredStore::new(StoreKind::Oltp, CI_OLTP_STORE)]);
        assert_eq!(
            assert_all_holders_registered(&manifest, &register_ci_holders()),
            Ok(()),
            "the CI store opened through the harness → the architecture test passes"
        );
        let rogue = HolderRegistry::new();
        let err = assert_all_holders_registered(&manifest, &rogue)
            .expect_err("a CI store opened outside the harness must FAIL the architecture test");
        assert_eq!(
            err.len(),
            1,
            "exactly the unregistered CI store is the violation"
        );
        assert!(
            err[0].message().contains(CI_OLTP_STORE),
            "the failure names the escaped CI store: {}",
            err[0].message()
        );
    }

    #[test]
    fn locate_and_export_are_typed_and_empty_but_correct() {
        let holder = CiHolder::new();
        let subj = subject("psn:ci-7");
        let locate = holder
            .locate(&subj, tenant())
            .expect("locate over the CI surface succeeds");
        assert_eq!(locate.receipt.operation, "locate");
        assert!(locate.receipt.content_hash.starts_with("blake3:"));
        assert!(
            locate.receipt.key_epoch_destroyed.is_none(),
            "locate shreds no key"
        );
        let export = holder
            .export(&subj, tenant())
            .expect("export over the CI surface succeeds");
        assert_eq!(export.receipt.operation, "export");
        assert!(export.receipt.content_hash.starts_with("blake3:"));
    }

    #[test]
    fn restrict_flips_a_real_flag_the_seams_read() {
        let flag = RestrictionFlag::new();
        let holder = CiHolder::with_restriction(flag.clone());
        let subj = subject("psn:ci-restricted");
        let sid = "psn:ci-restricted";

        assert!(!flag.is_restricted(sid));

        let r = holder.restrict(&subj, true).expect("restrict ON");
        assert_eq!(r.receipt.operation, "restrict");
        assert!(
            flag.is_restricted(sid),
            "the restriction flag the CI index/agent/analytics/notif seams read is SET"
        );

        holder.restrict(&subj, false).expect("restrict OFF");
        assert!(!flag.is_restricted(sid), "the restriction flag is cleared");
    }

    #[test]
    fn erase_is_a_stubbed_crypto_shred_no_op_that_names_ci_p32() {
        let holder = CiHolder::new();
        let scope = EraseScope::Subject {
            subject: subject("psn:ci-7"),
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
            "no DEK shredded (the crypto-shred body is CI-P32)"
        );
        assert_eq!(r1.receipt.operation, "erase");
        assert!(r1.receipt.content_hash.starts_with("blake3:"));
    }

    #[test]
    fn the_residual_is_by_reference_to_the_one_platform_posture() {
        assert!(
            CI_RESIDUAL_POSTURE_REF.contains("10.9") && CI_RESIDUAL_POSTURE_REF.contains("X-7"),
            "the residual cites the ONE platform posture (10.9 / X-7), by reference"
        );
        assert!(
            CI_RESIDUAL_POSTURE_REF.contains("never a CI-local restatement"),
            "the residual is by reference, never restated CI-local"
        );
    }

    #[test]
    fn ci_holder_is_object_safe() {
        let holders: Vec<Box<dyn PersonalDataHolder>> = vec![Box::new(CiHolder::new())];
        let subj = subject("psn:ci-9");
        for h in &holders {
            assert!(
                h.locate(&subj, tenant()).is_ok(),
                "the CI holder responds to the contract"
            );
        }
    }
}
