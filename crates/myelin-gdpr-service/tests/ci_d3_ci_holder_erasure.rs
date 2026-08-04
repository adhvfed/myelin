use myelin_gdpr::{EraseScope, PersonalDataHolder, Receipt, SubjectRef, TenantId};
use myelin_gdpr_service::{
    ci_holder_schemas, ci_registrations, data_map, CiHolderRegistration, CiLogHolder, CiLogModel,
    CryptoShredKms, EraseChecklist, InMemoryShredKms, ShredKeyClass, ShredKeyHandle,
    UpstreamHolderOrchestrator, CI_DB,
};
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

fn subject_scope(id: &str) -> EraseScope {
    EraseScope::Subject {
        subject: subject(id),
        tenant: tenant(),
    }
}

fn subject_dek(id: &str) -> ShredKeyHandle {
    ShredKeyHandle {
        tenant: tenant(),
        class: ShredKeyClass::Subject(id.into()),
    }
}

fn tenant_dek() -> ShredKeyHandle {
    ShredKeyHandle {
        tenant: tenant(),
        class: ShredKeyClass::Tenant,
    }
}

#[test]
fn ci_d3_erase_fans_to_ci_per_subject_dek_zero_dangling_leak_structure_survives() {
    let kms = InMemoryShredKms::new();
    kms.provision(subject_dek("u-erase"), 200);
    kms.provision(subject_dek("u-keep"), 201);
    kms.provision(tenant_dek(), 202);

    let model = CiLogModel::new();
    model.index_run_graph_from_source("u-erase");
    model.index_run_graph_from_source("u-keep");

    let ci_h = CiLogHolder::new(&model, &kms);

    let inv = data_map(&ci_holder_schemas(Region("fr-par".into())));
    assert!(
        inv.holders.contains("oltp:ci_oltp"),
        "H2 CI is in the data map"
    );
    assert!(
        inv.coverage_gaps(&ci_registrations()).is_empty(),
        "the registered CI holder is in the map - 0 holders missed"
    );

    let ci = CiHolderRegistration::register_ci(vec![(CI_DB, &ci_h as &dyn PersonalDataHolder)]);
    let orch = UpstreamHolderOrchestrator::new(ci);
    let checklist = EraseChecklist::new();
    let receipts = orch
        .fan_out_erase(&subject_scope("u-erase"), &checklist)
        .unwrap();
    assert_eq!(receipts.len(), 1, "the fan-out reached the CI holder");
    assert_eq!(orch.fanout_coverage(&checklist), 1.0, "100% CI coverage");

    assert!(
        !kms.is_present(&subject_dek("u-erase")),
        "the erased subject's per-subject CI-log DEK is destroyed"
    );
    assert_eq!(
        kms.recoverable_in_backup(&subject_dek("u-erase")),
        0,
        "0 recoverable in backups (crypto-shred reaches backups - CI-D3)"
    );

    assert!(
        kms.is_present(&subject_dek("u-keep")),
        "a different subject's CI log survives (the per-subject reach, not a blunt per-tenant erase)"
    );
    assert!(
        kms.is_present(&tenant_dek()),
        "the per-tenant fallback key survives a single-subject erase"
    );

    assert!(
        model.run_graph_present("u-erase"),
        "the erased subject's run-graph structure survives (PII shredded, structure remains)"
    );
    assert!(
        model.run_graph_present("u-keep"),
        "the other subject's structure is untouched"
    );

    let r = &receipts[0].receipt.receipt;
    assert_eq!(r.operation, "erase");
    assert!(
        r.content_hash.starts_with("blake3:"),
        "content-addressed DSR receipt"
    );
    assert!(
        r.key_epoch_destroyed.is_some(),
        "the per-subject-DEK key-shred is recorded (the CI-D3 telemetry green artifact)"
    );
    let expected = Receipt::content_addressed(
        "erase",
        CI_DB,
        "u-erase",
        "acme",
        "crypto_shred:per_subject_ci_log_dek:isolable_segments;structure_survives",
        r.key_epoch_destroyed,
        0,
    );
    assert_eq!(
        r.content_hash, expected.content_hash,
        "the receipt names the per-subject CI-log DEK reach (the C1/P5 extension)"
    );
}

#[test]
fn ci_d3_tenant_offboarding_destroys_the_per_tenant_fallback() {
    let kms = InMemoryShredKms::new();
    kms.provision(subject_dek("u-iso"), 300);
    kms.provision(tenant_dek(), 301);
    let model = CiLogModel::new();
    let ci_h = CiLogHolder::new(&model, &kms);

    let receipt = ci_h.erase(EraseScope::Tenant(tenant())).unwrap();

    assert!(
        !kms.is_present(&tenant_dek()),
        "a tenant offboarding destroys the per-tenant CI-log DEK fallback"
    );
    assert_eq!(
        kms.recoverable_in_backup(&tenant_dek()),
        0,
        "0 recoverable in backups"
    );
    let expected = Receipt::content_addressed(
        "erase",
        CI_DB,
        "*tenant*",
        "acme",
        "crypto_shred:per_tenant_ci_log_dek_fallback:tenant_offboard;structure_survives",
        receipt.receipt.key_epoch_destroyed,
        0,
    );
    assert_eq!(
        receipt.receipt.content_hash, expected.content_hash,
        "the tenant-scope erase names the per-tenant fallback (the non-isolable interleaved PII)"
    );
}
