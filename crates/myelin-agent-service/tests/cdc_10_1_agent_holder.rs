use myelin_agent_service::{
    agent_store_classifier, register_agent_holders, AgentOltpHolder, AgentTraceHolder,
    AGENT_OLTP_STORE, AGENT_TRACE_STORE,
};
use myelin_gdpr::{EraseScope, LocateReport, PersonalDataHolder, SubjectRef, TenantId};
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
                    .expect("an Agent-Fabric holder locate succeeds (seam)")
            })
            .collect()
    }

    fn fan_out_erase(&self, scope: EraseScope) -> usize {
        for h in &self.holders {
            h.erase(scope.clone())
                .expect("an Agent-Fabric holder erase succeeds (no-op seam)");
        }
        self.holders.len()
    }
}

#[test]
fn dsr_orchestrator_fans_locate_and_erase_out_to_the_agent_holders_via_the_contract() {
    let oltp = AgentOltpHolder;
    let trace = AgentTraceHolder;
    let consumer = DsrOrchestratorConsumer::new(vec![&oltp, &trace]);
    let subj = subject("psn:agent-cdc");

    let reports = consumer.fan_out_locate(&subj, tenant());
    assert_eq!(
        reports.len(),
        2,
        "both Agent-Fabric holders responded to locate via the contract"
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
    assert_eq!(
        erased, 2,
        "both Agent-Fabric holders honoured the erase contract"
    );
}

#[test]
fn agent_holder_stores_register_and_classify_with_zero_orphans() {
    let registry = register_agent_holders();
    let classifier = agent_store_classifier();
    assert_eq!(
        classify_store(StoreKind::Oltp, AGENT_OLTP_STORE, &classifier),
        Some(Holder::H11AgentMemory),
        "the Fabric OLTP schema is holder H11"
    );
    assert_eq!(
        classify_store(StoreKind::Oltp, AGENT_TRACE_STORE, &classifier),
        Some(Holder::H17AgentTrace),
        "the execution-trace store is holder H17"
    );
    assert_eq!(
        assert_holder_completeness(registry.registrations(), &classifier),
        Ok(()),
        "every Agent-Fabric store is in the exhaustive H1–H18 list - 0 orphan stores"
    );
}

#[test]
fn an_unregistered_agent_store_fails_the_holder_registered_architecture_test() {
    let manifest = StoreManifest::of([
        DeclaredStore::new(StoreKind::Oltp, AGENT_OLTP_STORE),
        DeclaredStore::new(StoreKind::Oltp, AGENT_TRACE_STORE),
    ]);
    assert_eq!(
        assert_all_holders_registered(&manifest, &register_agent_holders()),
        Ok(()),
        "both Fabric stores opened through the harness → the architecture test passes"
    );
    let mut rogue = HolderRegistry::new();
    rogue.open(StoreKind::Oltp, AGENT_OLTP_STORE);
    let err = assert_all_holders_registered(&manifest, &rogue)
        .expect_err("a Fabric store opened outside the harness must FAIL the architecture test");
    assert_eq!(
        err.len(),
        1,
        "exactly the unregistered trace store is the violation"
    );
    assert!(
        err[0].message().contains(AGENT_TRACE_STORE),
        "the failure names the escaped Fabric store: {}",
        err[0].message()
    );
}

#[test]
fn agent_holder_export_is_empty_but_correct() {
    let trace = AgentTraceHolder;
    let bundle = trace
        .export(&subject("psn:agent-1"), tenant())
        .expect("export over the registration seam succeeds");
    assert_eq!(bundle.receipt.operation, "export");
    assert!(bundle.receipt.content_hash.starts_with("blake3:"));
}
