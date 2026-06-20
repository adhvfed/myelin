//! # CDC 10.4 — the DSR orchestrator API + the state machine + the posture gate (P-GA-11 → P-111)
//!
//! **Contract:** index row 10.4 (`dsr_submit(kind, subject, scope, posture) → dsr_id`;
//! `dsr_status → {state, deadline, checklist}`; `dsr_certificate → MerkleProvenBundle` — the DSR
//! state machine; the 1-month deadline; Art. 28 operable by/for tenants). This is the
//! consumer-driven contract test the coverage scanner (P-S21) reads both halves of:
//!
//! - **provider** = the DSR orchestrator ([`DsrOrchestrator`]) — it accepts a `dsr_submit`,
//!   validates the controller/processor posture (§1), runs the total + ordered state machine
//!   (§4.1 — `received → validated → fanned-out → {awaiting-holders} → verified → completed`,
//!   `awaiting-holders` unskippable), sets the coarse `now + 1 month` deadline, resolves the
//!   read-only checklist FROM the data map, and exposes `dsr_status` / `dsr_certificate`.
//! - **consumer** = (a) a **DSR-submitting caller** (ops / a tenant admin) that submits a request
//!   and polls `dsr_status` to see the state + deadline + the resolved checklist; (b) an
//!   **auditor** consuming `dsr_certificate → MerkleProvenBundle` (the verifiable completion
//!   bundle — the Merkle inclusion proof seal is the named P-GA-20 follow-on).
//!
//! The dated green artifact: a controller-posture access request runs the full happy path and the
//! caller reads the state machine + deadline off `dsr_status`; the auditor reads the
//! content-addressed certificate; AND the posture gate REFUSES a Myelin-initiated erase of tenant
//! content (a captured-expected denial — the §1 controller/processor boundary). If 10.4's API
//! shape or the state-machine ordering drifts, this stops compiling/passing — that is the contract.

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
    SubjectRef::new(Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant()))
}

fn subject_scope(s: &str) -> EraseScope {
    EraseScope::Subject { subject: subject(s), tenant: tenant() }
}

/// A real-shaped data-map inventory with one tagged identity field + one zero-PII derived holder
/// (the orchestrator resolves the checklist FROM this — the map, not a hand-written list).
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

/// PROVIDER + CONSUMER — the happy path: the caller submits a controller-posture access request,
/// polls `dsr_status` across the total + ordered state machine, and the auditor reads the
/// `dsr_certificate` bundle. The state machine never skips `awaiting-holders`.
#[test]
fn cdc_10_4_caller_submits_polls_status_and_auditor_reads_the_certificate() {
    // provider: an orchestrator on a deterministic clock (the deadline base).
    let t0 = 1_700_000_000;
    let orchestrator = DsrOrchestrator::new(TestClock::at(t0));

    // ── consumer (a): the DSR-submitting caller (ops / tenant admin) ─────────────────────────────
    let dsr_id = orchestrator.dsr_submit(
        DsrKind::Access,
        tenant(),
        subject("p1"),
        subject_scope("p1"),
        Posture::Controller,
        Initiator::Myelin,
    );

    // the caller polls dsr_status → {state, deadline, checklist}.
    let s0 = orchestrator.dsr_status(&dsr_id).unwrap();
    assert_eq!(s0.state, DsrState::Received);
    // §4.1 — the deadline is now + 1 month, set on submit.
    assert_eq!(s0.deadline_secs, t0 + DSR_DEADLINE_SECS);

    // drive the total + ordered state machine (the caller / the durable workflow P-GA-12 drives it).
    assert!(orchestrator.validate(&dsr_id).unwrap(), "controller access is admitted");
    assert_eq!(orchestrator.dsr_status(&dsr_id).unwrap().state, DsrState::Validated);

    // §4.1 step 2 — the checklist is resolved FROM the data map (the map drives the scope).
    let checklist = orchestrator.fan_out(&dsr_id, &inventory()).unwrap();
    assert_eq!(orchestrator.dsr_status(&dsr_id).unwrap().state, DsrState::AwaitingHolders);
    // every holder in the map (incl. the zero-PII index) is a checklist line — 0 holders missed.
    let ids: Vec<&str> = checklist.iter().map(|c| c.holder_id.as_str()).collect();
    assert!(ids.contains(&"oltp:identity_oltp") && ids.contains(&"search_index:search_index"));

    // the fan-out (P-GA-12) returns the verified receipts; here the caller records them.
    orchestrator.verify(&dsr_id, vec!["receipt-identity".into()]).unwrap();
    assert_eq!(orchestrator.dsr_status(&dsr_id).unwrap().state, DsrState::Verified);
    orchestrator.complete(&dsr_id).unwrap();
    assert_eq!(orchestrator.dsr_status(&dsr_id).unwrap().state, DsrState::Completed);

    // ── consumer (b): the auditor reads dsr_certificate → MerkleProvenBundle ─────────────────────
    let cert = orchestrator.dsr_certificate(&dsr_id).unwrap();
    assert_eq!(cert.dsr_id, dsr_id);
    assert_eq!(cert.receipts, vec!["receipt-identity".to_string()]);
    assert!(cert.bundle_digest.starts_with("blake3:"), "content-addressed bundle");
    // the Merkle inclusion proof is the named P-GA-20 seal (None on this floor).
    assert!(cert.merkle_inclusion.is_none(), "the Merkle seal is P-GA-20 → P-119");
}

/// PROVIDER + CONSUMER — the posture gate (§1, the controller/processor boundary): a
/// Myelin-initiated erase of TENANT CONTENT is REFUSED (a captured-expected denial), while a
/// tenant-instructed erase of the same content is ADMITTED. The auditor cannot read a certificate
/// for a refused (un-driven) DSR.
#[test]
fn cdc_10_4_posture_gate_refuses_myelin_initiated_tenant_content_erase() {
    let orchestrator = DsrOrchestrator::new(TestClock::at(0));

    // Myelin-initiated erase of tenant content (processor posture) — REFUSED.
    let refused = orchestrator.dsr_submit(
        DsrKind::Erasure,
        tenant(),
        subject("p1"),
        subject_scope("p1"),
        Posture::Processor,
        Initiator::Myelin,
    );
    assert!(!orchestrator.validate(&refused).unwrap(), "the posture gate refuses it");
    assert_eq!(orchestrator.dsr_status(&refused).unwrap().state, DsrState::Refused);
    // the auditor cannot certify a refused DSR (no fan-out ran).
    assert_eq!(
        orchestrator.dsr_certificate(&refused).unwrap_err(),
        DsrError::CertificateNotReady(DsrState::Refused)
    );

    // the SAME erase, tenant-instructed (Art. 28) — ADMITTED.
    let admitted = orchestrator.dsr_submit(
        DsrKind::Erasure,
        tenant(),
        subject("p1"),
        subject_scope("p1"),
        Posture::Processor,
        Initiator::TenantInstructed,
    );
    assert!(orchestrator.validate(&admitted).unwrap(), "a tenant-instructed erase is admitted");
    assert_eq!(orchestrator.dsr_status(&admitted).unwrap().state, DsrState::Validated);
}

/// PROVIDER — the state machine is total + ordered (§4.1): `awaiting-holders` cannot be skipped,
/// and an illegal transition is a loud typed error, never a silent skip (the consumer relies on
/// "verified means the holders were actually driven").
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
    // try to skip fan-out → awaiting-holders and jump straight to verify: rejected.
    let err = orchestrator.verify(&id, vec![]).unwrap_err();
    assert_eq!(
        err,
        DsrError::IllegalTransition { from: DsrState::Validated, to: DsrState::Verified }
    );
    // the DSR did not silently advance.
    assert_eq!(orchestrator.dsr_status(&id).unwrap().state, DsrState::Validated);
}
