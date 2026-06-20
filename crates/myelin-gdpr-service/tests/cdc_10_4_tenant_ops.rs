//! # CDC 10.4 — DSR tenant-operability: Art. 28 tenant-facing DSR + offboarding +
//! restrict/rectify/portability (P-GA-13 → P-113)
//!
//! **Contract:** index row 10.4 — the **tenant-operability leg**: the orchestrator is operable
//! by/for **tenants** for *their own* data subjects (Art. 28 assistance); a tenant offboarding is a
//! tenant-granularity `erase(EraseScope::Tenant)` over the holder list (the tenant KEK destroyed ⇒
//! the whole tenant unrecoverable) sealing an offboarding certificate; the non-erasure rights
//! (restriction / rectification / portability) route through the orchestrator. This is the
//! consumer-driven contract test the coverage scanner (P-S21) reads both halves of:
//!
//! - **provider** = the [`TenantDsrSurface`] (the tenant-facing façade over the P-GA-11 DSR spine +
//!   the P-GA-12 fan-out driver + the legal-hold gate). It accepts a calling tenant + a DSR kind +
//!   a subject (or a tenant for offboarding) and an inventory + the registered holders + a durable
//!   checklist; it applies the **Art-28 scoping guard**, encodes the tenant-instructed processor
//!   posture, drives the §4.1 fan-out, and returns the outcome / the [`OffboardingCertificate`].
//! - **consumer** = (a) a **tenant admin** (an Art. 28 caller) submitting a DSR for ITS OWN subject
//!   and reading it complete with a verifiable receipt — and being REFUSED a cross-tenant subject
//!   (the IDOR face); (b) an **offboarding operator** running `erase(EraseScope::Tenant)` and
//!   reading the sealed offboarding certificate (100% holder coverage, Identity first); (c) an
//!   **auditor** consuming the verifiable completion receipts.
//!
//! The dated green artifact: a tenant's Art. 28 erase of its own subject completes (100% fan-out);
//! a cross-tenant Art. 28 request is REFUSED before any DSR is minted; a tenant offboarding fans
//! `EraseScope::Tenant` over every holder + seals a content-addressed certificate; and the
//! restrict/rectify/portability rights route through the orchestrator. If 10.4's tenant-operability
//! shape, the Art-28 scoping, or the offboarding fan-out drifts, this stops compiling/passing — that
//! is the contract.

use std::collections::BTreeSet;

use myelin_gdpr::{PersonalDataHolder, SubjectRef, TenantId};
use myelin_gdpr_service::datamap::{Inventory, InventoryEntry};
use myelin_gdpr_service::holders::{InMemoryShredKms, ShredKeyClass, ShredKeyHandle};
use myelin_gdpr_service::orchestration::{holder_ids, SeamHolder};
use myelin_gdpr_service::{
    DsrKind, DsrOrchestrator, DsrState, EraseChecklist, FanOutOutcome, LegalHoldRegistry,
    TenantDsrError, TenantDsrSurface, UpstreamHolderOrchestrator,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_substrate::TestClock;

fn t(s: &str) -> TenantId {
    TenantId::from_token(s)
}

fn subject_in(tenant: &TenantId, id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant.clone()))
}

fn kms_with_all_holder_keys(tenant: &TenantId) -> InMemoryShredKms {
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
            ShredKeyHandle {
                tenant: tenant.clone(),
                class: ShredKeyClass::Subject((*id).to_string()),
            },
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

/// PROVIDER + CONSUMER (a) — Art. 28: a tenant admin submits a DSR for ITS OWN data subject; it is
/// admitted (tenant-instructed processor posture) + driven to completion with a verifiable receipt.
/// A cross-tenant subject is REFUSED before a DSR is minted (the cross-tenant-IDOR face).
#[test]
fn cdc_10_4_art28_tenant_dsr_over_own_subject_completes_cross_tenant_refused() {
    let tenant = t("acme");
    let kms = kms_with_all_holder_keys(&tenant);
    let holders = seam_holders(&kms);
    let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
        holders.iter().map(|(id, h)| (*id, h as &dyn PersonalDataHolder)).collect(),
    );
    let dsr = DsrOrchestrator::new(TestClock::at(1_700_000_000));
    let holds = LegalHoldRegistry::new();
    let surface = TenantDsrSurface::new(&dsr, &holds);

    // consumer (a): the tenant admin submits an Art. 28 erase of its OWN subject — admitted.
    let id = surface
        .submit_for_my_subject(&tenant, DsrKind::Erasure, subject_in(&tenant, "u1"))
        .expect("a tenant may action its own subject (Art. 28)");
    let checklist = EraseChecklist::new();
    let outcome = surface
        .drive_tenant_subject_dsr(&id, &inventory(), &upstream, &checklist)
        .unwrap();
    assert!(matches!(outcome, FanOutOutcome::Erased(_)), "tenant-instructed erase admitted + driven");
    assert_eq!(dsr.state_of(&id).unwrap(), DsrState::Completed);
    assert_eq!(upstream.fanout_coverage(&checklist), 1.0, "100% fan-out over the holder list");

    // the cross-tenant Art. 28 request is REFUSED before any DSR is minted (the IDOR face).
    let err = surface
        .submit_for_my_subject(&t("evil"), DsrKind::Erasure, subject_in(&tenant, "u1"))
        .unwrap_err();
    assert!(matches!(err, TenantDsrError::CrossTenantSubject { .. }), "cross-tenant refused");
}

/// PROVIDER + CONSUMER (b) — tenant offboarding: `erase(EraseScope::Tenant)` fans over the holder
/// list (the tenant KEK destroyed ⇒ whole tenant unrecoverable), sealing a content-addressed
/// offboarding certificate the operator reads (100% coverage, Identity first, every key epoch
/// recorded).
#[test]
fn cdc_10_4_tenant_offboarding_fans_erase_tenant_and_seals_a_certificate() {
    let tenant = t("acme");
    let kms = kms_with_all_holder_keys(&tenant);
    let holders = seam_holders(&kms);
    let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
        holders.iter().map(|(id, h)| (*id, h as &dyn PersonalDataHolder)).collect(),
    );
    let dsr = DsrOrchestrator::new(TestClock::at(1_700_000_000));
    let holds = LegalHoldRegistry::new();
    let surface = TenantDsrSurface::new(&dsr, &holds);

    let checklist = EraseChecklist::new();
    let cert = surface
        .offboard_tenant(&tenant, &inventory(), &upstream, &checklist)
        .expect("a tenant offboarding is an authorised erase (§4.4)");

    assert_eq!(cert.tenant, tenant);
    assert_eq!(cert.completion.scope_token, "acme", "tenant-granularity (no subject)");
    assert_eq!(cert.completion.outcome, "erased");
    assert_eq!(cert.completion.holder_receipts.len(), 6, "all six holders shredded for offboarding");
    assert_eq!(cert.completion.holder_receipts[0].holder_id, holder_ids::IDENTITY, "Identity FIRST");
    assert!(cert.completion.content_hash.starts_with("blake3:"), "content-addressed (§4.2)");
    assert_eq!(upstream.fanout_coverage(&checklist), 1.0, "100% fan-out (the §4.4 GATE)");
    assert_eq!(dsr.state_of(&cert.dsr_id).unwrap(), DsrState::Completed);
    for hr in &cert.completion.holder_receipts {
        assert!(hr.receipt.receipt.key_epoch_destroyed.is_some(), "tenant-KEK shred recorded");
    }
}

/// PROVIDER + CONSUMER (a) — the non-erasure rights (restriction / rectification / portability)
/// route through the orchestrator as a tenant-instructed DSR; none is suspended by an active hold
/// (they are not erases — §4.1 step 3); each completes with the right's outcome.
#[test]
fn cdc_10_4_restrict_rectify_portability_route_through_the_orchestrator() {
    let tenant = t("acme");
    let kms = kms_with_all_holder_keys(&tenant);
    let holders = seam_holders(&kms);
    let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
        holders.iter().map(|(id, h)| (*id, h as &dyn PersonalDataHolder)).collect(),
    );
    let dsr = DsrOrchestrator::new(TestClock::at(0));
    let holds = LegalHoldRegistry::new();
    use myelin_gdpr_service::HoldScope;
    holds.set(HoldScope::Tenant("acme".into()), true); // a hold is active — non-erasure rights still proceed.
    let surface = TenantDsrSurface::new(&dsr, &holds);

    let r = surface
        .restrict_subject(&tenant, subject_in(&tenant, "u-r"), &inventory(), &upstream, &EraseChecklist::new())
        .unwrap();
    assert_eq!(r.receipt().outcome, "restriction");

    let rec = surface
        .rectify_subject(&tenant, subject_in(&tenant, "u-rec"), &inventory(), &upstream, &EraseChecklist::new())
        .unwrap();
    assert_eq!(rec.receipt().outcome, "rectification");

    let p = surface
        .portability_for_subject(&tenant, subject_in(&tenant, "u-p"), &inventory(), &upstream, &EraseChecklist::new())
        .unwrap();
    assert_eq!(p.receipt().outcome, "portability");
}
