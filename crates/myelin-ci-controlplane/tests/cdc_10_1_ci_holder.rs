use myelin_ci_controlplane::{
    ci_store_classifier, register_ci_holders, CiHolder, RestrictionFlag, CI_OLTP_STORE,
};
use myelin_gdpr::{
    EraseScope, LocateReport, PersonalDataHolder, RestrictReceipt, SubjectRef, TenantId,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_substrate::{
    assert_all_holders_registered, assert_holder_completeness, classify_store, DeclaredStore,
    Holder, HolderRegistry, StoreKind, StoreManifest,
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
                    .expect("the CI holder locate succeeds (typed seam)")
            })
            .collect()
    }

    fn fan_out_restrict(&self, subject: &SubjectRef, on: bool) -> Vec<RestrictReceipt> {
        self.holders
            .iter()
            .map(|h| {
                h.restrict(subject, on)
                    .expect("the CI holder restrict succeeds")
            })
            .collect()
    }

    fn fan_out_erase(&self, scope: EraseScope) -> usize {
        for h in &self.holders {
            h.erase(scope.clone())
                .expect("the CI holder erase succeeds (CI-P9 stub)");
        }
        self.holders.len()
    }
}

#[test]
fn dsr_orchestrator_fans_the_dsr_out_to_the_ci_holder_via_the_contract() {
    let flag = RestrictionFlag::new();
    let ci = CiHolder::with_restriction(flag.clone());
    let consumer = DsrOrchestratorConsumer::new(vec![&ci]);
    let subj = subject("psn:ci-cdc");

    let reports = consumer.fan_out_locate(&subj, tenant());
    assert_eq!(
        reports.len(),
        1,
        "the CI holder responded to locate via the contract"
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

    let restricts = consumer.fan_out_restrict(&subj, true);
    assert_eq!(
        restricts.len(),
        1,
        "the CI holder honoured restrict via the contract"
    );
    assert!(
        flag.is_restricted("psn:ci-cdc"),
        "the restriction flag the CI index/agent/analytics/notif seams read is SET"
    );

    let erased = consumer.fan_out_erase(EraseScope::Subject {
        subject: subj.clone(),
        tenant: tenant(),
    });
    assert_eq!(erased, 1, "the CI holder honoured the erase contract");
}

#[test]
fn ci_holder_store_registers_and_classifies_with_zero_orphans() {
    let registry = register_ci_holders();
    let classifier = ci_store_classifier();
    assert_eq!(
        classify_store(StoreKind::Oltp, CI_OLTP_STORE, &classifier),
        Some(Holder::H2Ci),
        "the CI OLTP schema is holder H2"
    );
    assert_eq!(
        assert_holder_completeness(registry.registrations(), &classifier),
        Ok(()),
        "every CI store is in the exhaustive H1–H18 list - 0 orphan stores"
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
fn ci_holder_export_is_typed_and_empty_but_correct() {
    let ci = CiHolder::new();
    let bundle = ci
        .export(&subject("psn:ci-1"), tenant())
        .expect("export over the CI holder seam succeeds");
    assert_eq!(bundle.receipt.operation, "export");
    assert!(bundle.receipt.content_hash.starts_with("blake3:"));
}
