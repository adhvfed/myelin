//! # CDC 10.1 — the Issues (H3) + Chat (H5) consumer-holder fan-out (P-GA-30 → P-333)
//!
//! **Contract:** index row 10.1 — the `PersonalDataHolder` fan-out. This prompt owns the
//! ORCHESTRATION leg over the M4 CONSUMER subsystems (the impls are Issues'/Chat's; GDPR REGISTERS
//! them + CALLS them in the canonical erase order with their per-derivative cascades). This is the
//! consumer-driven contract test the coverage scanner (P-S21) reads both halves of:
//!
//! - **provider** = the consumer holders AS `PersonalDataHolder`s (`myelin_gdpr_service::
//!   IssuesStoreHolder` / `ChatStoreHolder`) — each responds to the five-op contract for a subject;
//!   `erase` crypto-shreds the per-subject free-text / message-body DEK (Chat reaching hot + cold).
//! - **consumer** = the DSR ORCHESTRATOR (the fan-out, P-GA-12 / P-GA-06) — it registers H3/H5 through
//!   `IssuesChatCascadeDriver::register_issues_chat` at their canonical phases and fans the erase out,
//!   collecting each holder's receipt. It never reaches into a consumer store (the no-cross-store-read
//!   law — it holds only `&dyn PersonalDataHolder`).
//!
//! The dated green artifact (2026-06-22): the orchestrator fans to the two consumer holders + those
//! holders respond with content-addressed receipts; the fan-out reaches every one (100% coverage). If
//! 10.1's holder-contract shape drifts, this stops compiling/passing — that is the contract.

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
    // PROVIDER: the two consumer holders over a faithful crypto-shred KMS + models.
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

    // CONSUMER: the DSR orchestrator registers H3/H5 at their canonical phases + fans out.
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

    // The provider+consumer pair: the orchestrator fanned to both; each responded.
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
    // The consumer registers H3/H5 at the free-text crypto-shred phase (both shred a per-subject DEK).
    assert_eq!(
        issues_chat_phase_of(ISSUES_DB),
        Some(CanonicalErasePhase::CryptoShredDek)
    );
    assert_eq!(
        issues_chat_phase_of(CHAT_DB),
        Some(CanonicalErasePhase::CryptoShredDek)
    );
    // An unknown holder declares no phase.
    assert_eq!(issues_chat_phase_of("not_a_consumer_store"), None);
}
