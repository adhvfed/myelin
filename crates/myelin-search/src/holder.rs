use myelin_gdpr::{
    EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle, Receipt,
    RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef, TenantId,
};
use myelin_substrate::{classify_store, Holder, HolderRegistration, HolderRegistry, StoreKind};

pub const SEARCH_INDEX_STORE: &str = "search_index";

pub type SearchHolderRegistration = HolderRegistration;

pub fn register_search_holder() -> HolderRegistry {
    let mut registry = HolderRegistry::new();
    registry.open(StoreKind::SearchIndex, SEARCH_INDEX_STORE);
    registry
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SearchIndexHolder;

impl SearchIndexHolder {
    pub fn register(&self, registry: &mut HolderRegistry) -> SearchHolderRegistration {
        registry.open(StoreKind::SearchIndex, SEARCH_INDEX_STORE)
    }

    fn subject_id(subject: &SubjectRef) -> String {
        subject.principal.principal_id.0.clone()
    }
}

impl PersonalDataHolder for SearchIndexHolder {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                SEARCH_INDEX_STORE,
                &Self::subject_id(subject),
                &tenant.0,
                "no-search-data (SRCH-P02 stub: index lands SRCH-P03; locate body SRCH-P15)",
                None,
                0,
            ),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                SEARCH_INDEX_STORE,
                &Self::subject_id(subject),
                &tenant.0,
                "empty-bundle (SRCH-P02 stub: index derived/reconstructible - never the export source)",
                None,
                0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                SEARCH_INDEX_STORE,
                &Self::subject_id(subject),
                "",
                "no-op (SRCH-P02 stub: index derived; rectify via reindex-from-source SRCH-P15)",
                None,
                0,
            ),
        })
    }

    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed(
                "restrict",
                SEARCH_INDEX_STORE,
                &Self::subject_id(subject),
                "",
                &format!("no-op on={on} (SRCH-P02 stub: no index yet; suppression SRCH-P15 / GA-D7 P-152)"),
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
                SEARCH_INDEX_STORE,
                &subject_id,
                &tenant,
                "no-op (SRCH-P02 stub: no index to purge; PRIMARY purge+reindex SRCH-P15; index DEK reserved here)",
                None,
                0,
            ),
        })
    }
}

pub fn search_index_holder() -> Option<Holder> {
    classify_store(
        StoreKind::SearchIndex,
        SEARCH_INDEX_STORE,
        &Default::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_substrate::assert_holder_completeness;

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
    fn search_registers_its_index_store_as_a_holder() {
        let registry = register_search_holder();
        assert!(registry.is_registered(StoreKind::SearchIndex, SEARCH_INDEX_STORE));
        assert_eq!(
            registry.len(),
            1,
            "exactly the one Search index store registered"
        );
    }

    #[test]
    fn re_registration_is_idempotent() {
        let mut registry = register_search_holder();
        SearchIndexHolder.register(&mut registry);
        assert_eq!(
            registry.len(),
            1,
            "re-opening the same Search store does not double-register"
        );
    }

    #[test]
    fn search_store_classifies_to_h7_no_orphan() {
        let registry = register_search_holder();
        assert_eq!(
            search_index_holder(),
            Some(Holder::H7SearchIndex),
            "the per-tenant index is holder H7"
        );
        assert_eq!(
            assert_holder_completeness(registry.registrations(), &Default::default()),
            Ok(()),
            "the Search index store is in the exhaustive H1–H18 list - 0 orphan stores"
        );
    }

    #[test]
    fn holder_stub_returns_empty_but_correct_locate_and_export() {
        let holder = SearchIndexHolder;
        let subj = subject("u-1");
        let locate = holder
            .locate(&subj, tenant())
            .expect("locate over empty surface succeeds");
        assert_eq!(locate.receipt.operation, "locate");
        assert!(locate.receipt.content_hash.starts_with("blake3:"));
        assert!(
            locate.receipt.key_epoch_destroyed.is_none(),
            "locate shreds no key"
        );

        let export = holder
            .export(&subj, tenant())
            .expect("export of empty bundle succeeds");
        assert_eq!(export.receipt.operation, "export");
        assert!(export.receipt.content_hash.starts_with("blake3:"));
    }

    #[test]
    fn holder_stub_erase_is_a_no_op_receipt_and_idempotent() {
        let holder = SearchIndexHolder;
        let scope = EraseScope::Subject {
            subject: subject("u-1"),
            tenant: tenant(),
        };
        let r1 = holder
            .erase(scope.clone())
            .expect("stub erase succeeds (no-op)");
        let r2 = holder.erase(scope).expect("stub erase is idempotent");
        assert_eq!(
            r1, r2,
            "the same erase scope yields the identical content-addressed receipt"
        );
        assert!(
            r1.receipt.key_epoch_destroyed.is_none(),
            "no key shredded (no index exists)"
        );
    }

    #[test]
    fn holder_stub_restrict_surface() {
        let holder = SearchIndexHolder;
        let subj = subject("u-2");
        assert!(
            holder.restrict(&subj, true).is_ok(),
            "restrict on succeeds (no-op stub)"
        );
        assert!(
            holder.restrict(&subj, false).is_ok(),
            "restrict off succeeds (no-op stub)"
        );
    }

    #[test]
    fn holder_is_object_safe() {
        let holders: Vec<Box<dyn PersonalDataHolder>> = vec![Box::new(SearchIndexHolder)];
        let subj = subject("u-3");
        for h in &holders {
            assert!(
                h.locate(&subj, tenant()).is_ok(),
                "the holder responds to the contract"
            );
        }
    }
}
