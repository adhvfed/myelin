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

    let ci = CiHolderRegistration::register_ci(vec![(CI_DB, &ci_h as &dyn PersonalDataHolder)]);
    let orch = UpstreamHolderOrchestrator::new(ci);

    let checklist = EraseChecklist::new();
    let scope = EraseScope::Subject {
        subject: subject("u-cdc"),
        tenant: tenant(),
    };
    let receipts = orch.fan_out_erase(&scope, &checklist).unwrap();

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
    assert_eq!(
        ci_phase_of(CI_DB),
        Some(CanonicalErasePhase::CryptoShredDek),
        "the CI holder shreds the per-subject CI-log DEK at the free-text crypto-shred phase"
    );
}
