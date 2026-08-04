use myelin_gdpr::{EraseScope, PersonalDataHolder, SubjectRef, TenantId};
use myelin_gdpr_service::{
    issues_chat_phase_of, CanonicalErasePhase, ChatStoreHolder, ChatStoreModel, EraseChecklist,
    InMemoryShredKms, IssuesChatCascadeDriver, IssuesStoreHolder, IssuesStoreModel, ShredKeyClass,
    ShredKeyHandle, UpstreamHolderOrchestrator, CHAT_DB, ISSUES_DB,
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
fn provider_consumer_holders_respond_to_the_orchestrator_fan_out() {
    let kms = InMemoryShredKms::new();
    kms.provision(
        ShredKeyHandle {
            tenant: tenant(),
            class: ShredKeyClass::Subject("u-cdc".into()),
        },
        100,
    );
    let issues = IssuesStoreModel::new();
    issues.index_topology_from_source("u-cdc");
    let chat = ChatStoreModel::new();
    chat.index_from_source("u-cdc");

    let issues_h = IssuesStoreHolder::new(&issues, &kms);
    let chat_h = ChatStoreHolder::new(&chat, &kms);

    let consumers = IssuesChatCascadeDriver::register_issues_chat(vec![
        (ISSUES_DB, &issues_h as &dyn PersonalDataHolder),
        (CHAT_DB, &chat_h),
    ]);
    let orch = UpstreamHolderOrchestrator::new(consumers);

    let checklist = EraseChecklist::new();
    let scope = EraseScope::Subject {
        subject: subject("u-cdc"),
        tenant: tenant(),
    };
    let receipts = orch.fan_out_erase(&scope, &checklist).unwrap();

    assert_eq!(
        receipts.len(),
        2,
        "the orchestrator fanned to both consumer holders"
    );
    assert_eq!(
        orch.fanout_coverage(&checklist),
        1.0,
        "every consumer holder responded (100%)"
    );
    for r in &receipts {
        assert_eq!(
            r.receipt.receipt.operation, "erase",
            "each consumer holder responded with an erase receipt"
        );
        assert!(
            r.receipt.receipt.content_hash.starts_with("blake3:"),
            "content-addressed"
        );
    }
}

#[test]
fn the_consumer_phases_are_the_frozen_canonical_coordinates() {
    assert_eq!(
        issues_chat_phase_of(ISSUES_DB),
        Some(CanonicalErasePhase::CryptoShredDek)
    );
    assert_eq!(
        issues_chat_phase_of(CHAT_DB),
        Some(CanonicalErasePhase::CryptoShredDek)
    );
    assert_eq!(issues_chat_phase_of("not_a_consumer_store"), None);
}
