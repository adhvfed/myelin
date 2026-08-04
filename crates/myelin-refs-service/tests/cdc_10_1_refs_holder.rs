use myelin_gdpr::{EraseScope, LocateReport, PersonalDataHolder, SubjectRef, TenantId};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs_service::{
    refs_store_classifier, register_refs_holders, RefsCacheHolder, RefsEdgeHolder,
    REFS_CACHE_STORE, REFS_EDGE_STORE,
};
use myelin_substrate::{assert_holder_completeness, classify_store, Holder, StoreKind};

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
                    .expect("a Refs holder locate succeeds (stub)")
            })
            .collect()
    }

    fn fan_out_erase(&self, scope: EraseScope) -> usize {
        for h in &self.holders {
            h.erase(scope.clone())
                .expect("a Refs holder erase succeeds (no-op stub)");
        }
        self.holders.len()
    }
}

#[test]
fn dsr_orchestrator_fans_locate_and_erase_out_to_the_refs_holders_via_the_contract() {
    let edge = RefsEdgeHolder::default();
    let cache = RefsCacheHolder::default();
    let consumer = DsrOrchestratorConsumer::new(vec![&edge, &cache]);
    let subj = subject("u-cdc");

    let reports = consumer.fan_out_locate(&subj, tenant());
    assert_eq!(
        reports.len(),
        2,
        "both Refs holders responded to locate via the contract"
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
    assert_eq!(erased, 2, "both Refs holders honoured the erase contract");
}

#[test]
fn refs_holder_stores_register_and_classify_with_zero_orphans() {
    let registry = register_refs_holders();
    let classifier = refs_store_classifier();
    assert_eq!(
        classify_store(StoreKind::Oltp, REFS_EDGE_STORE, &classifier),
        Some(Holder::H12ReferenceGraph),
        "the edge inverse-index is holder H12"
    );
    assert_eq!(
        classify_store(StoreKind::Cache, REFS_CACHE_STORE, &classifier),
        Some(Holder::H9Caches),
        "the R2 projection cache classifies structurally to H9"
    );
    assert_eq!(
        assert_holder_completeness(registry.registrations(), &classifier),
        Ok(()),
        "every Refs store is in the exhaustive H1–H18 list - 0 orphan stores"
    );
}

#[test]
fn refs_holder_export_is_empty_but_correct() {
    let edge = RefsEdgeHolder::default();
    let bundle = edge
        .export(&subject("u-1"), tenant())
        .expect("export of an empty bundle succeeds (no edges yet)");
    assert_eq!(bundle.receipt.operation, "export");
    assert!(bundle.receipt.content_hash.starts_with("blake3:"));
}
