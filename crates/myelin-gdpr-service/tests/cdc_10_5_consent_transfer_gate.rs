use myelin_gdpr::{EraseScope, SubjectRef, TenantId};
use myelin_gdpr_service::{
    ConsentRegistry, SubProcessorRegistry, TransferGate, TransferVerdict, WithdrawalBasis,
    WithdrawalEffect,
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

#[test]
fn cdc_10_5_transfer_allowed_denies_extra_eu_by_default() {
    let gate = TransferGate::new();

    assert_eq!(
        gate.transfer_allowed(&Region::new("fr-par")),
        TransferVerdict::Allowed,
        "within-EU acceleration is permitted (§5.3)"
    );

    let verdict = gate.transfer_allowed(&Region::new("us-east"));
    assert_eq!(
        verdict,
        TransferVerdict::Denied,
        "deny extra-EU by default (§5.2)"
    );
    let pii_moved = if verdict.is_allowed() { 1 } else { 0 };
    assert_eq!(
        pii_moved, 0,
        "0 default extra-EU PII transfers slip through"
    );

    gate.record_transfer_mechanism(Region::new("us-east"));
    assert_eq!(
        gate.transfer_allowed(&Region::new("us-east")),
        TransferVerdict::Allowed,
        "an extra-EU target with a recorded mechanism is admitted"
    );

    assert_eq!(
        gate.extra_eu_denial_count(),
        1,
        "the deny-by-default is observable"
    );
}

#[test]
fn cdc_10_5_consent_withdrawal_propagates_with_erase_scope() {
    let reg = ConsentRegistry::new();
    let s = subject("u-cdc");
    let t = tenant();

    let v = reg.record(&s, &t, "marketing-emails", 1000);
    assert_eq!(v, 1, "first consent is version 1");
    assert!(reg.in_force(&s, &t, "marketing-emails"), "consent in force");

    let effect = reg.withdraw(
        &s,
        &t,
        "marketing-emails",
        WithdrawalBasis::ControllerConsentOnly,
        2000,
    );
    assert!(
        !reg.in_force(&s, &t, "marketing-emails"),
        "the consent-path is stopped"
    );
    let scope = match effect {
        WithdrawalEffect::StoppedAndTriggersDeletion(scope) => scope,
        other => panic!("expected a deletion-triggering withdrawal, got {other:?}"),
    };
    match scope {
        EraseScope::Subject { subject, tenant } => {
            assert_eq!(
                subject.principal.principal_id.0, "u-cdc",
                "the erase scope is the subject"
            );
            assert_eq!(tenant.0, "acme");
        }
        other => panic!("expected a Subject erase scope, got {other:?}"),
    }
}

#[test]
fn cdc_10_5_subprocessor_registry_versioned_region_dpa_objection() {
    let reg = SubProcessorRegistry::new();

    let v = reg.register("eu-llm-adapter", Region::new("fr-par"), "DPA-2026-001");
    assert_eq!(v, 1);

    assert!(
        reg.object(&tenant(), "eu-llm-adapter"),
        "the objection is recorded"
    );

    let entry = reg.get("eu-llm-adapter").expect("registered");
    assert_eq!(entry.region, Region::new("fr-par"), "region surfaced");
    assert_eq!(entry.dpa_ref, "DPA-2026-001", "DPA ref surfaced");
    assert_eq!(
        entry.version, 1,
        "version surfaced (the change-notification delta)"
    );
    assert_eq!(
        entry.objections,
        vec!["acme".to_string()],
        "the objection is surfaced"
    );
}
