use myelin_gdpr::{EraseScope, SubjectRef, TenantId};
use myelin_gdpr_service::{
    DsrError, DsrKind, DsrOrchestrator, DsrState, Initiator, Posture, DSR_DEADLINE_SECS,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_substrate::TestClock;

use myelin_gdpr_service::datamap::{Inventory, InventoryEntry};
use std::collections::BTreeSet;

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

fn inventory() -> Inventory {
    let mut holders = BTreeSet::new();
    holders.insert("oltp:identity_oltp".to_string());
    holders.insert("search_index:search_index".to_string());
    Inventory {
        entries: vec![InventoryEntry {
            field_path: "PrincipalRow.email".into(),
            holder_id: "oltp:identity_oltp".into(),
            holder: "H15".into(),
            region: "fr-par".into(),
            category: "ContactInfo".into(),
            role: "PlatformOperational".into(),
            basis: "Contract".into(),
            retention: "UntilContractEnd".into(),
            erasure: "CryptoShred(subject_dek)".into(),
            subject_locator: "principal_id".into(),
        }],
        holders,
        dpia_markers: BTreeSet::new(),
    }
}

#[test]
fn cdc_10_4_caller_submits_polls_status_and_auditor_reads_the_certificate() {
    let t0 = 1_700_000_000;
    let orchestrator = DsrOrchestrator::new(TestClock::at(t0));

    let dsr_id = orchestrator.dsr_submit(
        DsrKind::Access,
        tenant(),
        subject("p1"),
        subject_scope("p1"),
        Posture::Controller,
        Initiator::Myelin,
    );

    let s0 = orchestrator.dsr_status(&dsr_id).unwrap();
    assert_eq!(s0.state, DsrState::Received);
    assert_eq!(s0.deadline_secs, t0 + DSR_DEADLINE_SECS);

    assert!(
        orchestrator.validate(&dsr_id).unwrap(),
        "controller access is admitted"
    );
    assert_eq!(
        orchestrator.dsr_status(&dsr_id).unwrap().state,
        DsrState::Validated
    );

    let checklist = orchestrator.fan_out(&dsr_id, &inventory()).unwrap();
    assert_eq!(
        orchestrator.dsr_status(&dsr_id).unwrap().state,
        DsrState::AwaitingHolders
    );
    let ids: Vec<&str> = checklist.iter().map(|c| c.holder_id.as_str()).collect();
    assert!(ids.contains(&"oltp:identity_oltp") && ids.contains(&"search_index:search_index"));

    orchestrator
        .verify(&dsr_id, vec!["receipt-identity".into()])
        .unwrap();
    assert_eq!(
        orchestrator.dsr_status(&dsr_id).unwrap().state,
        DsrState::Verified
    );
    orchestrator.complete(&dsr_id).unwrap();
    assert_eq!(
        orchestrator.dsr_status(&dsr_id).unwrap().state,
        DsrState::Completed
    );

    let cert = orchestrator.dsr_certificate(&dsr_id).unwrap();
    assert_eq!(cert.dsr_id, dsr_id);
    assert_eq!(cert.receipts, vec!["receipt-identity".to_string()]);
    assert!(
        cert.bundle_digest.starts_with("blake3:"),
        "content-addressed bundle"
    );
    assert!(
        cert.merkle_inclusion.is_none(),
        "the Merkle seal is P-GA-20 → P-119"
    );
}

#[test]
fn cdc_10_4_posture_gate_refuses_myelin_initiated_tenant_content_erase() {
    let orchestrator = DsrOrchestrator::new(TestClock::at(0));

    let refused = orchestrator.dsr_submit(
        DsrKind::Erasure,
        tenant(),
        subject("p1"),
        subject_scope("p1"),
        Posture::Processor,
        Initiator::Myelin,
    );
    assert!(
        !orchestrator.validate(&refused).unwrap(),
        "the posture gate refuses it"
    );
    assert_eq!(
        orchestrator.dsr_status(&refused).unwrap().state,
        DsrState::Refused
    );
    assert_eq!(
        orchestrator.dsr_certificate(&refused).unwrap_err(),
        DsrError::CertificateNotReady(DsrState::Refused)
    );

    let admitted = orchestrator.dsr_submit(
        DsrKind::Erasure,
        tenant(),
        subject("p1"),
        subject_scope("p1"),
        Posture::Processor,
        Initiator::TenantInstructed,
    );
    assert!(
        orchestrator.validate(&admitted).unwrap(),
        "a tenant-instructed erase is admitted"
    );
    assert_eq!(
        orchestrator.dsr_status(&admitted).unwrap().state,
        DsrState::Validated
    );
}

#[test]
fn cdc_10_4_state_machine_is_total_and_awaiting_holders_is_unskippable() {
    let orchestrator = DsrOrchestrator::new(TestClock::at(0));
    let id = orchestrator.dsr_submit(
        DsrKind::Access,
        tenant(),
        subject("p1"),
        subject_scope("p1"),
        Posture::Controller,
        Initiator::Myelin,
    );
    orchestrator.validate(&id).unwrap();
    let err = orchestrator.verify(&id, vec![]).unwrap_err();
    assert_eq!(
        err,
        DsrError::IllegalTransition {
            from: DsrState::Validated,
            to: DsrState::Verified
        }
    );
    assert_eq!(
        orchestrator.dsr_status(&id).unwrap().state,
        DsrState::Validated
    );
}
