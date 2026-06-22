//! # CDC 10.1 — the `PersonalDataHolder` trait bodies + the GDPR-owned holders (P-GA-05 → P-105)
//!
//! **Contract:** index row 10.1 (`PersonalDataHolder{locate, export, rectify, restrict, erase}`).
//! The SIGNATURE was frozen at P-GA-01 (`myelin-gdpr`); the BODIES + the GDPR-owned holder impls
//! (H18 + H16) land here. This is the consumer-driven contract test the coverage scanner (P-S21)
//! reads both halves of:
//!
//! - **provider** = a GDPR-owned holder ([`GdprOwnStoreHolder`] H18 / [`AuditCarveOutHolder`] H16)
//!   IMPLEMENTING the five-operation contract — it responds to `locate`/`export`/`rectify`/
//!   `restrict`/`erase` with a content-addressed receipt; `erase` crypto-shreds its OWN key class
//!   (the GD-4 lever) through the [`CryptoShredKms`] seam, recording the destroyed key epoch.
//! - **consumer** = a minimal DSR-orchestrator stand-in that holds a heterogeneous set of holders
//!   behind `dyn PersonalDataHolder` and fans an erase out to them — it CALLS the holder contract
//!   and NEVER reaches into a store (the no-cross-store-read law, gdpr §3.1). This is the shape the
//!   real orchestrator (P-GA-11/P-GA-12) takes.
//!
//! The dated green artifact: the consumer fans `erase(Subject)` out to the H18 + H16 holders; H18
//! crypto-shreds (0 recoverable after, the destroyed epoch on the receipt); H16 retains the
//! minimised record (never rewrites the chain); both return a content-addressed receipt. If 10.1's
//! body shape drifts, this stops compiling/passing — that is the contract.

use myelin_gdpr::{EraseReceipt, EraseScope, PersonalDataHolder, Receipt, SubjectRef, TenantId};
use myelin_gdpr_service::{
    AuditCarveOutHolder, CryptoShredKms, GdprOwnStoreHolder, InMemoryShredKms, ShredKeyClass,
    ShredKeyHandle,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

fn subject(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId::from_token("acme"),
    ))
}

/// **The CONSUMER side (10.1): the DSR-orchestrator shape that fans out to holders via the
/// contract.** It holds holders behind `dyn PersonalDataHolder` (a heterogeneous set) and calls
/// the contract — it never reaches into a store. This is the shape the real orchestrator
/// (P-GA-11/P-GA-12) takes; the property pinned here is "the orchestrator touches a store ONLY
/// through the holder contract".
struct DsrOrchestratorConsumer<'a> {
    holders: Vec<&'a dyn PersonalDataHolder>,
}

impl<'a> DsrOrchestratorConsumer<'a> {
    fn new(holders: Vec<&'a dyn PersonalDataHolder>) -> Self {
        DsrOrchestratorConsumer { holders }
    }

    /// Fan an erase out to every registered holder via the contract; collect the receipts.
    fn fan_out_erase(&self, scope: EraseScope) -> Vec<EraseReceipt> {
        self.holders
            .iter()
            .map(|h| {
                h.erase(scope.clone())
                    .expect("a GDPR-owned holder erase succeeds")
            })
            .collect()
    }
}

/// **provider + consumer wired together:** the orchestrator (consumer) fans `erase(Subject)` out to
/// the H18 + H16 GDPR-owned holders (providers); the receipts attest the contract was honoured.
#[test]
fn dsr_orchestrator_fans_erase_out_to_the_gdpr_owned_holders_via_the_contract() {
    let tenant = TenantId::from_token("acme");
    let subj = subject("u-cdc");

    // The provider's crypto-shred MECHANISM (the KMS seam) — the subject's consent DEK present.
    let kms = InMemoryShredKms::new();
    kms.provision(
        ShredKeyHandle {
            tenant: tenant.clone(),
            class: ShredKeyClass::Subject(subj.principal.principal_id.0.clone()),
        },
        11,
    );

    // The PROVIDERS: H18 (GDPR own stores) + H16 (audit carve-out).
    let h18 = GdprOwnStoreHolder::new(&kms);
    let h16 = AuditCarveOutHolder::new(&kms);

    // The CONSUMER fans out via `dyn PersonalDataHolder` — never reaching into a store.
    let orchestrator = DsrOrchestratorConsumer::new(vec![&h18, &h16]);
    let receipts = orchestrator.fan_out_erase(EraseScope::Subject {
        subject: subj.clone(),
        tenant: tenant.clone(),
    });

    assert_eq!(
        receipts.len(),
        2,
        "the fan-out reached both GDPR-owned holders"
    );
    // Every receipt is content-addressed (the provider honoured "each op returns a receipt").
    for r in &receipts {
        assert_eq!(r.receipt.operation, "erase");
        assert!(r.receipt.content_hash.starts_with("blake3:"));
    }

    // H18: the consent DEK was crypto-shred — 0 recoverable, the destroyed epoch recorded.
    let handle = ShredKeyHandle {
        tenant: tenant.clone(),
        class: ShredKeyClass::Subject(subj.principal.principal_id.0.clone()),
    };
    assert_eq!(
        kms.recoverable_in_backup(&handle),
        0,
        "H18 crypto-shred: 0 recoverable"
    );
    assert!(
        receipts
            .iter()
            .any(|r| r.receipt.key_epoch_destroyed == Some(11)),
        "the H18 erase receipt records the destroyed key epoch"
    );
}

/// **The consumer relies on the frozen RECEIPT shape** (a `Receipt` content-addressed, recording
/// the destroyed key epoch) — the provider+consumer pin the same `Receipt` struct. If the receipt
/// shape drifts, neither side compiles.
#[test]
fn receipt_shape_is_the_frozen_provider_consumer_contract() {
    let r = Receipt::content_addressed(
        "erase",
        "gdpr_own_store",
        "u",
        "acme",
        "crypto_shred",
        Some(2),
        0,
    );
    assert_eq!(r.operation, "erase");
    assert_eq!(r.key_epoch_destroyed, Some(2));
    // Round-trips (the consumer deserializes the provider's receipt off the audit log).
    let back: Receipt = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
    assert_eq!(back, r);
}
