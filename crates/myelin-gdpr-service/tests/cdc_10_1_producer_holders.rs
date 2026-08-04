use myelin_gdpr::{EraseScope, PersonalDataHolder, SubjectRef, TenantId};
use myelin_gdpr_service::{
    producer_holder_ids, producer_phase_of, AgentTraceModel, CanonicalErasePhase, EraseChecklist,
    GitDbHolder, InMemoryShredKms, KnowledgeAgentTraceHolder, KnowledgeStoreHolder,
    KnowledgeStoreModel, ProducerHolderRegistration, ShredKeyClass, ShredKeyHandle,
    UpstreamHolderOrchestrator,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

fn tenant() -> TenantId {
    TenantId::from_token("acme")
}

fn subject(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        tenant(),
    ))
}

#[test]
fn provider_producer_holders_respond_to_the_orchestrator_fan_out() {
    let kms = InMemoryShredKms::new();
    kms.provision(
        ShredKeyHandle {
            tenant: tenant(),
            class: ShredKeyClass::Subject("u-cdc".into()),
        },
        100,
    );
    let knowledge = KnowledgeStoreModel::new();
    knowledge.index_embedding_from_source("u-cdc");
    let trace = AgentTraceModel::new();
    trace.write_trace_from_source("u-cdc", "blake3:trace");

    let git_h = GitDbHolder::new(&kms);
    let kn_h = KnowledgeStoreHolder::new(&knowledge, &kms);
    let trace_h = KnowledgeAgentTraceHolder::new(&trace, &kms);

    let producers = ProducerHolderRegistration::register_producers(vec![
        (
            producer_holder_ids::GIT_DB,
            &git_h as &dyn PersonalDataHolder,
        ),
        (producer_holder_ids::KNOWLEDGE_DB, &kn_h),
        (producer_holder_ids::AGENT_TRACE, &trace_h),
    ]);
    let orch = UpstreamHolderOrchestrator::new(producers);

    let checklist = EraseChecklist::new();
    let scope = EraseScope::Subject {
        subject: subject("u-cdc"),
        tenant: tenant(),
    };
    let receipts = orch.fan_out_erase(&scope, &checklist).unwrap();

    assert_eq!(
        receipts.len(),
        3,
        "the orchestrator fanned to all three producer holders"
    );
    assert_eq!(
        orch.fanout_coverage(&checklist),
        1.0,
        "every producer holder responded (100%)"
    );
    for r in &receipts {
        assert_eq!(
            r.receipt.receipt.operation, "erase",
            "each producer holder responded with an erase receipt"
        );
        assert!(
            r.receipt.receipt.content_hash.starts_with("blake3:"),
            "content-addressed"
        );
    }
}

#[test]
fn the_producer_phases_are_the_frozen_canonical_coordinates() {
    assert_eq!(
        producer_phase_of(producer_holder_ids::GIT_DB),
        Some(CanonicalErasePhase::CryptoShredDek)
    );
    assert_eq!(
        producer_phase_of(producer_holder_ids::KNOWLEDGE_DB),
        Some(CanonicalErasePhase::CryptoShredDek)
    );
    assert_eq!(
        producer_phase_of(producer_holder_ids::AGENT_TRACE),
        Some(CanonicalErasePhase::CachesAndDerivedCopies),
        "the agent trace is a trailing derived copy (after the per-subject DEK shred)"
    );
}
