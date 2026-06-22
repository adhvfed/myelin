//! # CDC 10.1 — the CI consumer-holder fan-out (H2 — CI + log segments) (P-GA-29 → P-332)
//!
//! **Contract:** index row 10.1 — the `PersonalDataHolder` fan-out. This prompt owns the
//! ORCHESTRATION leg over the CI CONSUMER subsystem (the `erase` impl is CI's; GDPR REGISTERS H2 +
//! CALLS it in the canonical erase order). This is the consumer-driven contract test the coverage
//! scanner (P-S21) reads both halves of:
//!
//! - **provider** = the CI holder AS a `PersonalDataHolder` (`myelin_gdpr_service::CiLogHolder`) —
//!   responds to the five-op contract for a subject; `erase` crypto-shreds the per-subject CI-log DEK
//!   (the isolable inline log PII reach), per-tenant fallback on a tenant offboarding.
//! - **consumer** = the DSR ORCHESTRATOR (the fan-out, P-GA-12 / P-GA-06) — it registers H2 through
//!   `CiHolderRegistration::register_ci` at its canonical phase and fans the erase out, collecting the
//!   holder's receipt. It never reaches into the CI store (the no-cross-store-read law — it holds only
//!   `&dyn PersonalDataHolder`).
//!
//! The dated green artifact: the orchestrator fans to the CI holder + it responds with a
//! content-addressed receipt; the fan-out reaches it (100% coverage). If 10.1's holder-contract shape
//! drifts, this stops compiling/passing — that is the contract.

use myelin_gdpr::{EraseScope, PersonalDataHolder, Receipt, SubjectRef, TenantId};
use myelin_gdpr_service::{
    ci_phase_of, CanonicalErasePhase, CiHolderRegistration, CiLogHolder, CiLogModel,
    EraseChecklist, InMemoryShredKms, ShredKeyClass, ShredKeyHandle, UpstreamHolderOrchestrator,
    CI_DB,
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
fn provider_ci_holder_responds_to_the_orchestrator_fan_out() {
    // PROVIDER: the CI holder over a faithful crypto-shred KMS + run-graph model.
    let kms = InMemoryShredKms::new();
    kms.provision(
        ShredKeyHandle {
            tenant: tenant(),
            class: ShredKeyClass::Subject("u-cdc".into()),
        },
        100,
    );
    let model = CiLogModel::new();
    model.index_run_graph_from_source("u-cdc");
    let ci_h = CiLogHolder::new(&model, &kms);

    // CONSUMER: the DSR orchestrator registers H2 at its canonical phase + fans out.
    let ci = CiHolderRegistration::register_ci(vec![(CI_DB, &ci_h as &dyn PersonalDataHolder)]);
    let orch = UpstreamHolderOrchestrator::new(ci);

    let checklist = EraseChecklist::new();
    let scope = EraseScope::Subject {
        subject: subject("u-cdc"),
        tenant: tenant(),
    };
    let receipts = orch.fan_out_erase(&scope, &checklist).unwrap();

    // The provider+consumer pair: the orchestrator fanned to the CI holder; it responded.
    assert_eq!(
        receipts.len(),
        1,
        "the orchestrator fanned to the CI holder"
    );
    assert_eq!(
        orch.fanout_coverage(&checklist),
        1.0,
        "the CI holder responded (100%)"
    );
    let r = &receipts[0];
    assert_eq!(
        r.receipt.receipt.operation, "erase",
        "the CI holder responded with an erase receipt"
    );
    assert!(
        r.receipt.receipt.content_hash.starts_with("blake3:"),
        "content-addressed"
    );
    // The CI erase names the per-subject CI-log DEK reach (the outcome is folded into the content
    // hash) — proven via the expected content-address.
    let expected = Receipt::content_addressed(
        "erase",
        CI_DB,
        "u-cdc",
        "acme",
        "crypto_shred:per_subject_ci_log_dek:isolable_segments;structure_survives",
        r.receipt.receipt.key_epoch_destroyed,
        0,
    );
    assert_eq!(
        r.receipt.receipt.content_hash, expected.content_hash,
        "the CI erase names the per-subject CI-log DEK reach (the C1/P5 extension)"
    );
}

#[test]
fn the_ci_phase_is_the_frozen_canonical_coordinate() {
    // The consumer registers H2 at the free-text crypto-shred phase (the CI log DEK is a free-text DEK
    // holder, §4.1).
    assert_eq!(
        ci_phase_of(CI_DB),
        Some(CanonicalErasePhase::CryptoShredDek),
        "the CI holder shreds the per-subject CI-log DEK at the free-text crypto-shred phase"
    );
}
