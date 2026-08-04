use myelin_gdpr::{EraseScope, LocateReport, PersonalDataHolder, SubjectRef, TenantId};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_search::{
    register_search_holder, search_index_holder, SearchIndexHolder, SEARCH_INDEX_STORE,
};
use myelin_substrate::{assert_holder_completeness, Holder, StoreKind};

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

struct DsrOrchestratorConsumer<'a> {
    holders: Vec<&'a dyn PersonalDataHolder>,
}

impl<'a> DsrOrchestratorConsumer<'a> {
    fn new(holders: Vec<&'a dyn PersonalDataHolder>) -> Self {
        DsrOrchestratorConsumer { holders }
    }

    fn fan_out_locate(&self, subject: &SubjectRef, tenant: TenantId) -> Vec<LocateReport> {
        self.holders
            .iter()
            .map(|h| {
                h.locate(subject, tenant.clone())
                    .expect("a Search holder locate succeeds (stub)")
            })
            .collect()
    }

    fn fan_out_erase(&self, scope: EraseScope) -> usize {
        for h in &self.holders {
            h.erase(scope.clone())
                .expect("a Search holder erase succeeds (no-op stub)");
        }
        self.holders.len()
    }
}

#[test]
fn dsr_orchestrator_fans_locate_and_erase_out_to_the_search_holder_via_the_contract() {
    let index = SearchIndexHolder;
    let consumer = DsrOrchestratorConsumer::new(vec![&index]);
    let subj = subject("u-cdc");

    let reports = consumer.fan_out_locate(&subj, tenant());
    assert_eq!(
        reports.len(),
        1,
        "the Search holder responded to locate via the contract"
    );
    for r in &reports {
        assert_eq!(r.receipt.operation, "locate");
        assert!(
            r.receipt.content_hash.starts_with("blake3:"),
            "content-addressed receipt"
        );
        assert!(
            r.receipt.key_epoch_destroyed.is_none(),
            "locate shreds no key"
        );
    }

    let erased = consumer.fan_out_erase(EraseScope::Subject {
        subject: subj.clone(),
        tenant: tenant(),
    });
    assert_eq!(erased, 1, "the Search holder honoured the erase contract");
}

#[test]
fn search_holder_store_registers_and_classifies_with_zero_orphans() {
    let registry = register_search_holder();
    assert!(registry.is_registered(StoreKind::SearchIndex, SEARCH_INDEX_STORE));
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
fn search_holder_export_is_empty_but_correct() {
    let index = SearchIndexHolder;
    let bundle = index
        .export(&subject("u-1"), tenant())
        .expect("export of an empty bundle succeeds (no index yet)");
    assert_eq!(bundle.receipt.operation, "export");
    assert!(bundle.receipt.content_hash.starts_with("blake3:"));
}
