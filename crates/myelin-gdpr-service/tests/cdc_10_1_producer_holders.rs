//! # CDC 10.1 — the producer-holder fan-out (Git H1 / Knowledge H4 / agent-trace H17) (P-GA-27 → P-256)
//!
//! **Contract:** index row 10.1 — the `PersonalDataHolder` fan-out. This prompt owns the
//! ORCHESTRATION leg over the M3 PRODUCER subsystems (the impls are Git/KN/Agent's; GDPR REGISTERS
//! them + CALLS them in the canonical erase order). This is the consumer-driven contract test the
//! coverage scanner (P-S21) reads both halves of:
//!
//! - **provider** = the producer holders AS `PersonalDataHolder`s (`myelin_gdpr_service::GitDbHolder`
//!   / `KnowledgeStoreHolder` / `KnowledgeAgentTraceHolder`) — each responds to the five-op contract
//!   for a subject; `erase` crypto-shreds (Git inline bodies / Knowledge blocks+db-rows+embeddings /
//!   the agent trace).
//! - **consumer** = the DSR ORCHESTRATOR (the fan-out, P-GA-12 / P-GA-06) — it registers H1/H4/H17
//!   through `ProducerHolderRegistration::register_producers` at their canonical phases and fans the
//!   erase out, collecting each holder's receipt. It never reaches into a producer store (the
//!   no-cross-store-read law — it holds only `&dyn PersonalDataHolder`).
//!
//! The dated green artifact: the orchestrator fans to the three producer holders + those holders
//! respond with content-addressed receipts; the fan-out reaches every one (100% coverage). If 10.1's
//! holder-contract shape drifts, this stops compiling/passing — that is the contract.

use myelin_gdpr::{EraseScope, PersonalDataHolder, SubjectRef, TenantId};
use myelin_gdpr_service::{
    producer_holder_ids, producer_phase_of, AgentTraceModel, CanonicalErasePhase,
    EraseChecklist, GitDbHolder, InMemoryShredKms, KnowledgeAgentTraceHolder, KnowledgeStoreHolder,
    KnowledgeStoreModel, ProducerHolderRegistration, ShredKeyClass, ShredKeyHandle,
    UpstreamHolderOrchestrator,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

fn tenant() -> TenantId {
    TenantId::from_token("acme")
}

fn subject(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant()))
}

#[test]
fn provider_producer_holders_respond_to_the_orchestrator_fan_out() {
    // PROVIDER: the three producer holders over a faithful crypto-shred KMS + models.
    let kms = InMemoryShredKms::new();
    kms.provision(
        ShredKeyHandle { tenant: tenant(), class: ShredKeyClass::Subject("u-cdc".into()) },
        100,
    );
    let knowledge = KnowledgeStoreModel::new();
    knowledge.index_embedding_from_source("u-cdc");
    let trace = AgentTraceModel::new();
    trace.write_trace_from_source("u-cdc", "blake3:trace");

    let git_h = GitDbHolder::new(&kms);
    let kn_h = KnowledgeStoreHolder::new(&knowledge, &kms);
    let trace_h = KnowledgeAgentTraceHolder::new(&trace, &kms);

    // CONSUMER: the DSR orchestrator registers H1/H4/H17 at their canonical phases + fans out.
    let producers = ProducerHolderRegistration::register_producers(vec![
        (producer_holder_ids::GIT_DB, &git_h as &dyn PersonalDataHolder),
        (producer_holder_ids::KNOWLEDGE_DB, &kn_h),
        (producer_holder_ids::AGENT_TRACE, &trace_h),
    ]);
    let orch = UpstreamHolderOrchestrator::new(producers);

    let checklist = EraseChecklist::new();
    let scope = EraseScope::Subject { subject: subject("u-cdc"), tenant: tenant() };
    let receipts = orch.fan_out_erase(&scope, &checklist).unwrap();

    // The provider+consumer pair: the orchestrator fanned to all three; each responded.
    assert_eq!(receipts.len(), 3, "the orchestrator fanned to all three producer holders");
    assert_eq!(orch.fanout_coverage(&checklist), 1.0, "every producer holder responded (100%)");
    for r in &receipts {
        assert_eq!(r.receipt.receipt.operation, "erase", "each producer holder responded with an erase receipt");
        assert!(r.receipt.receipt.content_hash.starts_with("blake3:"), "content-addressed");
    }
}

#[test]
fn the_producer_phases_are_the_frozen_canonical_coordinates() {
    // The consumer registers H1/H4 at the free-text crypto-shred phase; H17 trails as a derived copy.
    assert_eq!(producer_phase_of(producer_holder_ids::GIT_DB), Some(CanonicalErasePhase::CryptoShredDek));
    assert_eq!(producer_phase_of(producer_holder_ids::KNOWLEDGE_DB), Some(CanonicalErasePhase::CryptoShredDek));
    assert_eq!(
        producer_phase_of(producer_holder_ids::AGENT_TRACE),
        Some(CanonicalErasePhase::CachesAndDerivedCopies),
        "the agent trace is a trailing derived copy (after the per-subject DEK shred)"
    );
}
