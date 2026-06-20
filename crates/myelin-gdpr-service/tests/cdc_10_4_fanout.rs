//! # CDC 10.4 — the data-map-driven resumable fan-out + receipts + the legal-hold gate (P-GA-12 → P-112)
//!
//! **Contract:** index row 10.4 — the **fan-out leg**: the orchestrator resolves the per-holder
//! checklist FROM the data map (`data_map()`, 10.3), applies the legal-hold gate (§4.1 step 3),
//! fans the erase out through the holder contract (10.1) in the canonical erase order
//! (idempotent + resumable — the durable checklist IS the state), collects + verifies the
//! receipts, and seals a **verifiable content-addressed DSR completion receipt** (§4.2). This is
//! the consumer-driven contract test the coverage scanner (P-S21) reads both halves of:
//!
//! - **provider** = the [`FanOutDriver`] over the [`DsrOrchestrator`] (P-GA-11 spine) +
//!   the [`UpstreamHolderOrchestrator`] (P-GA-06 canonical-order resumable fan-out) + the
//!   [`LegalHoldRegistry`] (the G4 gate). It accepts a validated DSR id + a data-map inventory +
//!   the registered holders + the durable checklist, drives the §4.1 algorithm, and returns the
//!   [`FanOutOutcome`] (Erased / DeferredUnderHold / ReadRightServed) carrying the verifiable
//!   completion receipt.
//! - **consumer** = (a) the **DSR orchestration caller** (ops / a tenant admin / the eventual
//!   durable workflow) that submits + validates a DSR then asks the driver to fan it out, reads
//!   the DSR reach `Completed`, and the verifiable receipt back; (b) an **auditor / supervisory
//!   authority** consuming the per-holder receipts (each recording its destroyed key epoch) +
//!   the content-addressed completion receipt — "we erased it" independently checkable (§4.2).
//!
//! The dated green artifact: a controller-posture subject erase resolves its checklist FROM the
//! map, fans out over every existing holder in canonical order (Identity first), seals a
//! content-addressed completion receipt, and the DSR reaches `Completed` with 100% fan-out
//! coverage; AND a subject under an active legal hold has its erase DEFERRED (no holder driven, no
//! double-erase) while a read right is never suspended. If 10.4's fan-out shape, the legal-hold
//! gate, or the §4.2 receipt drifts, this stops compiling/passing — that is the contract.

use std::collections::BTreeSet;

use myelin_gdpr::{EraseScope, PersonalDataHolder, SubjectRef, TenantId};
use myelin_gdpr_service::datamap::{Inventory, InventoryEntry};
use myelin_gdpr_service::holders::{InMemoryShredKms, ShredKeyClass, ShredKeyHandle};
use myelin_gdpr_service::orchestration::{holder_ids, SeamHolder};
use myelin_gdpr_service::{
    DsrKind, DsrOrchestrator, DsrState, EraseChecklist, FanOutDriver, FanOutOutcome, HoldScope,
    Initiator, LegalHoldRegistry, Posture, UpstreamHolderOrchestrator,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_substrate::TestClock;

fn tenant() -> TenantId {
    TenantId::from_token("acme")
}

fn subject(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant()))
}

fn subject_scope(s: &str) -> EraseScope {
    EraseScope::Subject { subject: subject(s), tenant: tenant() }
}

fn kms_with_all_holder_keys(t: &TenantId) -> InMemoryShredKms {
    let kms = InMemoryShredKms::new();
    for (i, id) in [
        holder_ids::IDENTITY,
        holder_ids::BLOB,
        holder_ids::AUTHZ_TUPLES,
        holder_ids::BUS,
        holder_ids::CACHE,
        holder_ids::BACKUP,
    ]
    .iter()
    .enumerate()
    {
        kms.provision(
            ShredKeyHandle { tenant: t.clone(), class: ShredKeyClass::Subject((*id).to_string()) },
            100 + i as u64,
        );
    }
    kms
}

fn seam_holders(kms: &InMemoryShredKms) -> Vec<(&'static str, SeamHolder<'_>)> {
    [
        holder_ids::IDENTITY,
        holder_ids::BLOB,
        holder_ids::AUTHZ_TUPLES,
        holder_ids::BUS,
        holder_ids::CACHE,
        holder_ids::BACKUP,
    ]
    .into_iter()
    .map(|id| (id, SeamHolder::new(id, ShredKeyClass::Subject(id.to_string()), kms)))
    .collect()
}

/// A real-shaped data-map inventory: a tagged identity field + a zero-PII derived holder (the
/// checklist is resolved FROM this — the map, not a hand-written list, drives the scope).
fn inventory() -> Inventory {
    let mut holders = BTreeSet::new();
    holders.insert("identity".to_string());
    holders.insert("search_index:search_index".to_string());
    Inventory {
        entries: vec![InventoryEntry {
            field_path: "PrincipalRow.email".into(),
            holder_id: "identity".into(),
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

/// PROVIDER + CONSUMER — the data-map-driven fan-out: the caller submits + validates a
/// controller-posture erase, the driver resolves the checklist FROM the map and fans the erase out
/// over every existing holder in the canonical order, and the auditor reads the verifiable
/// content-addressed completion receipt (each per-holder receipt records its destroyed key epoch).
#[test]
fn cdc_10_4_driver_fans_out_data_map_driven_and_seals_a_verifiable_receipt() {
    let t = tenant();
    let kms = kms_with_all_holder_keys(&t);
    let holders = seam_holders(&kms);
    // provider: the upstream holder orchestrator + the DSR spine + the legal-hold gate.
    let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
        holders.iter().map(|(id, h)| (*id, h as &dyn PersonalDataHolder)).collect(),
    );
    let dsr = DsrOrchestrator::new(TestClock::at(1_700_000_000));
    let holds = LegalHoldRegistry::new();
    let driver = FanOutDriver::new(&dsr, &holds);

    // ── consumer (a): the DSR orchestration caller submits + validates the erase ────────────────
    let id = dsr.dsr_submit(
        DsrKind::Erasure,
        t.clone(),
        subject("p1"),
        subject_scope("p1"),
        Posture::Controller,
        Initiator::Myelin,
    );
    assert!(dsr.validate(&id).unwrap(), "controller erase admitted by the posture gate");

    // the driver fans it out (resolve from map → gate → fan-out → verify → complete).
    let checklist = EraseChecklist::new();
    let outcome = driver.drive(&id, &inventory(), &upstream, &checklist).unwrap();

    // the DSR reached Completed via the total + ordered state machine.
    assert_eq!(dsr.state_of(&id).unwrap(), DsrState::Completed);
    // the checklist was resolved FROM the map (surfaced in dsr_status) — 0 holders missed.
    let cl = dsr.dsr_status(&id).unwrap().checklist;
    let ids: Vec<&str> = cl.iter().map(|c| c.holder_id.as_str()).collect();
    assert!(ids.contains(&"identity") && ids.contains(&"search_index:search_index"));
    // 100% fan-out coverage over the existing holder set (the §4.1 GATE).
    assert_eq!(upstream.fanout_coverage(&checklist), 1.0);

    // ── consumer (b): the auditor reads the verifiable completion receipt ─────────────────────────
    let receipt = match &outcome {
        FanOutOutcome::Erased(r) => r,
        other => panic!("expected Erased, got {other:?}"),
    };
    assert_eq!(receipt.outcome, "erased");
    assert_eq!(receipt.holder_receipts.len(), 6, "all six upstream holders receipted in order");
    assert_eq!(receipt.holder_receipts[0].holder_id, holder_ids::IDENTITY, "Identity FIRST (§4.1)");
    assert!(receipt.content_hash.starts_with("blake3:"), "content-addressed (§4.2)");
    for hr in &receipt.holder_receipts {
        assert!(
            hr.receipt.receipt.key_epoch_destroyed.is_some(),
            "holder {} records its destroyed key epoch (§4.2 independent-check trail)",
            hr.holder_id
        );
    }
    // the DSR certificate seals the same per-holder receipts (the Merkle inclusion is P-GA-20).
    let cert = dsr.dsr_certificate(&id).unwrap();
    assert_eq!(cert.receipts.len(), 6);
    assert!(cert.merkle_inclusion.is_none(), "the Merkle seal is P-GA-20 → P-119");
}

/// PROVIDER + CONSUMER — the legal-hold gate (§4.1 step 3): a subject under an active hold has its
/// erase DEFERRED (no holder driven — 0 double-erase, recorded *partially deferred*), while a read
/// right is NEVER suspended by a hold. Clearing the hold + re-driving resumes to completion.
#[test]
fn cdc_10_4_legal_hold_defers_an_erase_but_a_read_right_proceeds() {
    let t = tenant();
    let kms = kms_with_all_holder_keys(&t);
    let holders = seam_holders(&kms);
    let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
        holders.iter().map(|(id, h)| (*id, h as &dyn PersonalDataHolder)).collect(),
    );
    let dsr = DsrOrchestrator::new(TestClock::at(0));
    let holds = LegalHoldRegistry::new();
    holds.set(HoldScope::Subject { tenant: "acme".into(), subject: "held".into() }, true);
    let driver = FanOutDriver::new(&dsr, &holds);

    // an ERASE under the hold is DEFERRED — no holder is driven.
    let erase = dsr.dsr_submit(
        DsrKind::Erasure,
        t.clone(),
        subject("held"),
        subject_scope("held"),
        Posture::Controller,
        Initiator::Myelin,
    );
    dsr.validate(&erase).unwrap();
    let checklist = EraseChecklist::new();
    let outcome = driver.drive(&erase, &inventory(), &upstream, &checklist).unwrap();
    assert!(matches!(outcome, FanOutOutcome::DeferredUnderHold(_)), "erase deferred under hold");
    assert_eq!(outcome.receipt().outcome, "deferred:legal_hold");
    assert_eq!(dsr.state_of(&erase).unwrap(), DsrState::AwaitingHolders, "parked, not completed");
    assert_eq!(upstream.fanout_coverage(&checklist), 0.0, "0 holders driven under hold");

    // a READ RIGHT for the held subject is NEVER suspended — it completes.
    let access = dsr.dsr_submit(
        DsrKind::Access,
        t.clone(),
        subject("held"),
        subject_scope("held"),
        Posture::Controller,
        Initiator::Myelin,
    );
    dsr.validate(&access).unwrap();
    let read_outcome = driver.drive(&access, &inventory(), &upstream, &EraseChecklist::new()).unwrap();
    assert!(matches!(read_outcome, FanOutOutcome::ReadRightServed(_)), "access proceeds under hold");
    assert_eq!(dsr.state_of(&access).unwrap(), DsrState::Completed);

    // clear the hold and RE-DRIVE the erase — it resumes to completion (resumable checklist).
    holds.set(HoldScope::Subject { tenant: "acme".into(), subject: "held".into() }, false);
    let resumed = driver.drive(&erase, &inventory(), &upstream, &checklist).unwrap();
    assert!(matches!(resumed, FanOutOutcome::Erased(_)));
    assert_eq!(dsr.state_of(&erase).unwrap(), DsrState::Completed);
    assert_eq!(upstream.fanout_coverage(&checklist), 1.0);
}
