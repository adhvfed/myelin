//! # CDC 10.5 (the consent / sub-processor / transfer legs) — the registries + the `transfer_allowed`
//! gate (P-GA-23 → P-150)
//!
//! **Contract:** index row 10.5 (the consent / sub-processor / transfer-gate legs — *"`consent_record/
//! withdraw`; `subprocessors`/`transfer_allowed` (deny extra-EU by default)"*, gdpr §5.2 / §5.3). This
//! is the consumer-driven contract test the coverage scanner (P-S21) reads both halves of, for the
//! consent / sub-processor / transfer legs of 10.5 (the retention leg is P-GA-22 → P-149,
//! `cdc_10_5_retention_engine.rs`):
//!
//! - **provider** = (a) the **`transfer_allowed` gate** ([`TransferGate`]) — deny extra-EU by default,
//!   admit within-EU/EEA, admit an extra-EU target only with a recorded transfer mechanism; (b) the
//!   **consent registry** ([`ConsentRegistry`]) — versioned + withdrawable; a withdrawal propagates
//!   (stops the path, may trigger deletion); (c) the **sub-processor registry**
//!   ([`SubProcessorRegistry`]) — versioned + region + DPA ref + the objection workflow.
//! - **consumer** = (a) a **transfer caller** (the §5.3 outbound push-mirror / sub-processor adapter
//!   seam, or the future real-LLM adapter) that asks `transfer_allowed(target_region)` BEFORE moving
//!   PII and HONOURS the deny; (b) a **consent caller** (the controller-posture activity that records
//!   + withdraws consent and DRIVES the withdrawal-triggered erase over the returned scope).
//!
//! The dated green artifact: an extra-EU transfer is DENIED by default (0 default extra-EU transfers
//! slip through); a within-EU transfer is ADMITTED; a consent withdrawal stops the path and (for a
//! controller-posture consent-only activity) returns the erase scope the caller drives. If 10.5's
//! consent / transfer legs drift (the gate stops denying extra-EU by default; a withdrawal stops
//! propagating), this stops compiling/passing — that is the contract.

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
    SubjectRef::new(Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant()))
}

/// **The `transfer_allowed` gate (the provider) ⇄ a transfer caller (the consumer).** The §5.3
/// outbound replication / sub-processor adapter seam asks the gate BEFORE moving PII: an extra-EU
/// target is DENIED by default; a within-EU target is ADMITTED; an extra-EU target with a recorded
/// transfer mechanism is ADMITTED. The caller HONOURS the deny (it does not move PII on a `Denied`).
#[test]
fn cdc_10_5_transfer_allowed_denies_extra_eu_by_default() {
    let gate = TransferGate::new();

    // consumer (the §5.3 push-mirror seam): a within-EU CDN clone target is ALLOWED.
    assert_eq!(
        gate.transfer_allowed(&Region::new("fr-par")),
        TransferVerdict::Allowed,
        "within-EU acceleration is permitted (§5.3)"
    );

    // consumer (the future real-LLM adapter): an extra-EU target is DENIED by default.
    let verdict = gate.transfer_allowed(&Region::new("us-east"));
    assert_eq!(verdict, TransferVerdict::Denied, "deny extra-EU by default (§5.2)");
    // the caller HONOURS the deny — it must not move PII when the gate says Denied.
    let pii_moved = if verdict.is_allowed() { 1 } else { 0 };
    assert_eq!(pii_moved, 0, "0 default extra-EU PII transfers slip through");

    // record a valid transfer mechanism — now the extra-EU target is ADMITTED (the adapter path).
    gate.record_transfer_mechanism(Region::new("us-east"));
    assert_eq!(
        gate.transfer_allowed(&Region::new("us-east")),
        TransferVerdict::Allowed,
        "an extra-EU target with a recorded mechanism is admitted"
    );

    // the green artifact: the by-default extra-EU deny was counted (the one above, before recording).
    assert_eq!(gate.extra_eu_denial_count(), 1, "the deny-by-default is observable");
}

/// **The consent registry (the provider) ⇄ a consent caller (the consumer).** A controller-posture
/// consent-only activity records consent, then WITHDRAWS it; the registry returns the
/// withdrawal effect carrying the erase scope the caller DRIVES (the withdrawal propagates — stops
/// the path, triggers deletion). The caller consumes the `EraseScope` to drive the EXISTING fan-out.
#[test]
fn cdc_10_5_consent_withdrawal_propagates_with_erase_scope() {
    let reg = ConsentRegistry::new();
    let s = subject("u-cdc");
    let t = tenant();

    // consumer (the controller-posture activity): record consent.
    let v = reg.record(&s, &t, "marketing-emails", 1000);
    assert_eq!(v, 1, "first consent is version 1");
    assert!(reg.in_force(&s, &t, "marketing-emails"), "consent in force");

    // consumer: the subject withdraws consent — the registry returns the propagation effect.
    let effect = reg.withdraw(
        &s,
        &t,
        "marketing-emails",
        WithdrawalBasis::ControllerConsentOnly,
        2000,
    );
    // the path is STOPPED.
    assert!(!reg.in_force(&s, &t, "marketing-emails"), "the consent-path is stopped");
    // the withdrawal TRIGGERS DELETION — the caller consumes the erase scope.
    let scope = match effect {
        WithdrawalEffect::StoppedAndTriggersDeletion(scope) => scope,
        other => panic!("expected a deletion-triggering withdrawal, got {other:?}"),
    };
    // the consumer drives the EXISTING erase fan-out over this scope (the seam, here asserted shape).
    match scope {
        EraseScope::Subject { subject, tenant } => {
            assert_eq!(subject.principal.principal_id.0, "u-cdc", "the erase scope is the subject");
            assert_eq!(tenant.0, "acme");
        }
        other => panic!("expected a Subject erase scope, got {other:?}"),
    }
}

/// **The sub-processor registry (the provider) ⇄ a sub-processor caller (the consumer).** The §5.2
/// versioned list carries region + DPA ref; a tenant OBJECTS and the objection is surfaced. A caller
/// reading the list (the change-notification surface) consumes the version + region + objection.
#[test]
fn cdc_10_5_subprocessor_registry_versioned_region_dpa_objection() {
    let reg = SubProcessorRegistry::new();

    // provider: register a sub-processor with region + DPA ref.
    let v = reg.register("eu-llm-adapter", Region::new("fr-par"), "DPA-2026-001");
    assert_eq!(v, 1);

    // consumer (the objection workflow): a tenant objects.
    assert!(reg.object(&tenant(), "eu-llm-adapter"), "the objection is recorded");

    // consumer (the change-notification surface): read the list — version + region + DPA + objection.
    let entry = reg.get("eu-llm-adapter").expect("registered");
    assert_eq!(entry.region, Region::new("fr-par"), "region surfaced");
    assert_eq!(entry.dpa_ref, "DPA-2026-001", "DPA ref surfaced");
    assert_eq!(entry.version, 1, "version surfaced (the change-notification delta)");
    assert_eq!(entry.objections, vec!["acme".to_string()], "the objection is surfaced");
}
