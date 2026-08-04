use myelin_gdpr::{EraseScope, PersonalDataHolder, SubjectRef, TenantId};
use myelin_gdpr_service::data_map;
use myelin_gdpr_service::{
    producer_holder_ids, producer_holder_schemas, producer_registrations, AgentTraceModel,
    EraseChecklist, GitDbHolder, KnowledgeAgentTraceHolder, KnowledgeStoreHolder,
    KnowledgeStoreModel, ProducerHolderRegistration, ShredKeyClass, ShredKeyHandle,
    UpstreamHolderOrchestrator, AUDIT_CARVE_OUT_STORE,
};
use myelin_gdpr_service::{CryptoShredKms, InMemoryShredKms};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::Region;

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

fn subject_scope(s: &str) -> EraseScope {
    EraseScope::Subject {
        subject: subject(s),
        tenant: tenant(),
    }
}

fn subject_dek(subject_token: &str) -> ShredKeyHandle {
    ShredKeyHandle {
        tenant: tenant(),
        class: ShredKeyClass::Subject(subject_token.to_string()),
    }
}

#[test]
fn producer_holder_erasure_gate_kn_d4_and_kn_d12_are_green() {
    let inv = data_map(&producer_holder_schemas(Region("fr-par".into())));
    assert!(
        inv.holders.contains("oltp:git_oltp"),
        "H1 Git is in the data map"
    );
    assert!(
        inv.holders.contains("oltp:knowledge_oltp"),
        "H4 Knowledge is in the data map"
    );
    assert!(
        inv.holders.contains("oltp:agent_fabric_trace"),
        "H17 agent-trace is in the data map"
    );
    assert!(
        inv.coverage_gaps(&producer_registrations()).is_empty(),
        "0 holders missed - every registered producer holder is in the map"
    );

    let kms = InMemoryShredKms::new();
    kms.provision(subject_dek("u-subject"), 1000);

    let knowledge = KnowledgeStoreModel::new();
    knowledge.index_embedding_from_source("u-subject");
    let trace = AgentTraceModel::new();
    trace.write_trace_from_source("u-subject", "blake3:run-trace-cafef00d");

    assert!(
        kms.is_present(&subject_dek("u-subject")),
        "the per-subject DEK is live before erase"
    );
    assert_eq!(
        knowledge.reidentify_hits("u-subject"),
        1,
        "the embedding re-identifies before erase"
    );
    assert!(
        trace.has_trace("u-subject"),
        "the run trace is present before erase"
    );

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

    assert_eq!(
        orch.holder_ids_in_order().last(),
        Some(&producer_holder_ids::AGENT_TRACE),
        "the agent trace shreds last (a trailing derived copy)"
    );

    let checklist = EraseChecklist::new();
    let receipts = orch
        .fan_out_erase(&subject_scope("u-subject"), &checklist)
        .unwrap();
    assert_eq!(receipts.len(), 3, "all three producer holders were reached");
    assert_eq!(
        orch.fanout_coverage(&checklist),
        1.0,
        "100% coverage of the producer holders"
    );
    for r in &receipts {
        assert_eq!(r.receipt.receipt.operation, "erase");
        assert!(
            r.receipt.receipt.content_hash.starts_with("blake3:"),
            "each receipt is content-addressed"
        );
    }

    assert!(
        !kms.is_present(&subject_dek("u-subject")),
        "the per-subject DEK is destroyed (free-text unrecoverable)"
    );
    assert_eq!(
        kms.recoverable_in_backup(&subject_dek("u-subject")),
        0,
        "KN-D4: 0 recoverable in backups (crypto-shred reaches backups)"
    );
    assert_eq!(
        knowledge.reidentify_hits("u-subject"),
        0,
        "KN-D4: 0 embedding re-identification - the vectors were PURGED, not hidden"
    );

    assert!(
        !trace.has_trace("u-subject"),
        "KN-D12: the agent trace is crypto-shredded (0 recoverable)"
    );
    assert_ne!(
        producer_holder_ids::AGENT_TRACE,
        AUDIT_CARVE_OUT_STORE,
        "KN-D12: the H17 trace holder is DISTINCT from the H16 audit carve-out (§6.5)"
    );
}

#[test]
fn producer_fan_out_is_resumable_no_double_shred() {
    let kms = InMemoryShredKms::new();
    kms.provision(subject_dek("u-resume"), 2000);
    let knowledge = KnowledgeStoreModel::new();
    knowledge.index_embedding_from_source("u-resume");
    let trace = AgentTraceModel::new();
    trace.write_trace_from_source("u-resume", "blake3:trace");

    let git_h = GitDbHolder::new(&kms);
    let kn_h = KnowledgeStoreHolder::new(&knowledge, &kms);
    let trace_h = KnowledgeAgentTraceHolder::new(&trace, &kms);
    let orch =
        UpstreamHolderOrchestrator::new(ProducerHolderRegistration::register_producers(vec![
            (
                producer_holder_ids::GIT_DB,
                &git_h as &dyn PersonalDataHolder,
            ),
            (producer_holder_ids::KNOWLEDGE_DB, &kn_h),
            (producer_holder_ids::AGENT_TRACE, &trace_h),
        ]));

    let checklist = EraseChecklist::new();
    let first = orch
        .fan_out_erase(&subject_scope("u-resume"), &checklist)
        .unwrap();
    let second = orch
        .fan_out_erase(&subject_scope("u-resume"), &checklist)
        .unwrap();
    assert_eq!(
        first, second,
        "an idempotent re-drive returns the SAME receipts (0 double-erase)"
    );
    assert_eq!(
        trace.erase_call_count(),
        1,
        "the trace was shredded exactly once across the re-drive"
    );
    assert_eq!(
        knowledge.erase_call_count(),
        1,
        "the knowledge embedding was purged exactly once"
    );
}
