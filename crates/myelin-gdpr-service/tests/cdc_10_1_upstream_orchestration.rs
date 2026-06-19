//! # CDC 10.1 — the upstream-store holder ORCHESTRATION + the canonical erase order (P-GA-06 → P-106)
//!
//! **Contract:** index row 10.1 (`PersonalDataHolder{…}` + the M1-shared-layer holder
//! orchestration + the canonical erase order + the resumable receipt). The trait BODIES + the
//! GDPR-owned holders were P-GA-05; the ORCHESTRATION over the upstream stores (H6/H8/H9/H10/
//! H14/H15) is P-GA-06. This is the consumer-driven contract test the coverage scanner (P-S21)
//! reads both halves of:
//!
//! - **provider** = an UPSTREAM holder ([`SeamHolder`], the faithful M1 store double whose
//!   `erase` crypto-shreds its OWN key class through the [`CryptoShredKms`] seam) IMPLEMENTING
//!   the contract — the store owns its `erase`; GDPR calls it.
//! - **consumer** = the [`UpstreamHolderOrchestrator`] (the DSR-orchestrator's holder-fan-out
//!   stage) CALLING the upstream holders **in the canonical erase order** (Identity FIRST) and
//!   recording each receipt into the durable [`EraseChecklist`] — it CALLS the holder contract
//!   and NEVER reaches into a store (the no-cross-store-read law, gdpr §3.1).
//!
//! The dated green artifact: the orchestrator (consumer) fans `erase(Subject)` out to the six M1
//! upstream holders (providers) in the canonical order; Identity (H15) is erased FIRST so every
//! downstream holder sees only the opaque pseudonym; each holder returns a content-addressed,
//! resumable receipt recording the destroyed key epoch; `erasure_fanout_coverage` reads 100%.
//! If 10.1's orchestration shape drifts, this stops compiling/passing — that is the contract.

use myelin_gdpr::{EraseScope, PersonalDataHolder, SubjectRef, TenantId};
use myelin_gdpr_service::{
    holder_ids, CryptoShredKms, EraseChecklist, InMemoryShredKms, SeamHolder, ShredKeyClass,
    ShredKeyHandle, UpstreamHolderOrchestrator,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

fn subject(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId::from_token("acme"),
    ))
}

/// **provider + consumer wired together:** the orchestrator (consumer) fans `erase(Subject)` out
/// to the six M1 upstream holders (providers) in the canonical erase order (Identity first); the
/// receipts attest the contract was honoured, in order, with 100% coverage.
#[test]
fn orchestrator_fans_erase_out_to_the_upstream_holders_in_canonical_order() {
    let tenant = TenantId::from_token("acme");
    let subj = subject("u-cdc-orch");

    // The PROVIDERS' crypto-shred mechanism (one key class per upstream holder).
    let kms = InMemoryShredKms::new();
    let ids = [
        holder_ids::IDENTITY,
        holder_ids::BLOB,
        holder_ids::AUTHZ_TUPLES,
        holder_ids::BUS,
        holder_ids::CACHE,
        holder_ids::BACKUP,
    ];
    for (i, id) in ids.iter().enumerate() {
        kms.provision(
            ShredKeyHandle {
                tenant: tenant.clone(),
                class: ShredKeyClass::Subject((*id).to_string()),
            },
            10 + i as u64,
        );
    }

    // The PROVIDERS: the six upstream-store holder seams.
    let holders: Vec<(&'static str, SeamHolder)> = ids
        .iter()
        .map(|id| (*id, SeamHolder::new(id, ShredKeyClass::Subject(id.to_string()), &kms)))
        .collect();

    // The CONSUMER: the orchestrator fans out via `dyn PersonalDataHolder`, never into a store.
    let orch = UpstreamHolderOrchestrator::register_m1_upstream(
        holders.iter().map(|(id, h)| (*id, h as &dyn PersonalDataHolder)).collect(),
    );
    let checklist = EraseChecklist::new();
    let receipts = orch
        .fan_out_erase(
            &EraseScope::Subject {
                subject: subj.clone(),
                tenant: tenant.clone(),
            },
            &checklist,
        )
        .expect("the canonical fan-out succeeds");

    // The fan-out reached all six holders, Identity FIRST, backups LAST (the canonical order).
    assert_eq!(receipts.len(), 6, "the fan-out reached every M1 upstream holder");
    assert_eq!(receipts[0].holder_id, holder_ids::IDENTITY, "Identity (pseudonym map) erased FIRST");
    assert_eq!(receipts.last().unwrap().holder_id, holder_ids::BACKUP, "backups erased LAST");

    // Every receipt is content-addressed + records the destroyed key epoch (independently checkable).
    for r in &receipts {
        assert_eq!(r.receipt.receipt.operation, "erase");
        assert!(r.receipt.receipt.content_hash.starts_with("blake3:"));
        assert!(r.receipt.receipt.key_epoch_destroyed.is_some());
    }

    // 100% coverage over the existing holder set (the M1-holder orchestration floor).
    assert_eq!(orch.fanout_coverage(&checklist), 1.0);

    // 0 recoverable across every holder after the fan-out (the erasure post-condition).
    for id in ids {
        let handle = ShredKeyHandle {
            tenant: tenant.clone(),
            class: ShredKeyClass::Subject(id.to_string()),
        };
        assert_eq!(kms.recoverable_in_backup(&handle), 0, "{id}: 0 recoverable after erase");
    }
}

/// **The consumer relies on resumability** (the durable checklist is the state): a re-driven
/// fan-out re-affirms the SAME receipts and re-calls nothing (the contract is idempotent).
#[test]
fn re_driving_the_fan_out_is_idempotent_for_the_consumer() {
    let tenant = TenantId::from_token("acme");
    let kms = InMemoryShredKms::new();
    let id = holder_ids::BLOB;
    kms.provision(
        ShredKeyHandle { tenant: tenant.clone(), class: ShredKeyClass::Subject(id.into()) },
        7,
    );
    let h = SeamHolder::new(id, ShredKeyClass::Subject(id.into()), &kms);
    let orch = UpstreamHolderOrchestrator::register_m1_upstream(vec![(id, &h as &dyn PersonalDataHolder)]);
    let checklist = EraseChecklist::new();
    let scope = EraseScope::Subject { subject: subject("u-idem-cdc"), tenant: tenant.clone() };

    let first = orch.fan_out_erase(&scope, &checklist).unwrap();
    let second = orch.fan_out_erase(&scope, &checklist).unwrap();
    assert_eq!(first, second, "an idempotent re-drive returns the SAME receipts");
    assert_eq!(h.erase_call_count(), 1, "the already-receipted holder is NOT re-called");
}
