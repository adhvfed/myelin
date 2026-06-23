//! # CDC 10.1 / 1.4 — the CI side of `PersonalDataHolder{locate, export, rectify, restrict, erase}` +
//! the holder registration seam (CI-P9 → P-352, M4)
//!
//! **Contract:** index rows **10.1** (`PersonalDataHolder` — the five DSR operations) + **1.4**
//! (`PersonalDataHolder` auto-registration on every store the harness opens). The SIGNATURE was frozen
//! at P-GA-01 (`myelin-gdpr`); the GDPR-owned bodies landed at P-GA-05. THIS file ships the **CI
//! Control Plane side** of 10.1/1.4 — the CI stores as holder **H2 (`H2Ci`)** over run-state, logs,
//! artifacts, caches, and deployments (architecture 03 §6). It is the CI-P9 holder SUBSTRATE:
//! locate/export are TYPED (empty-but-correct content-addressed receipts), `restrict` flips a REAL
//! per-subject flag the CI seams read, and `erase` is STUBBED to crypto-shred — a well-defined no-op
//! that NAMES its CI-P32 / CI-D3 fan-out follow-on. It is the provider+consumer CDC pair the
//! contract-coverage scanner (P-S21) reads for the CI holder seam.
//!
//! - **PROVIDER** = the CI holder ([`CiHolder`], H2) IMPLEMENTING the five-operation 10.1 contract. At
//!   CI-P9 it responds with empty-but-correct receipts (locate/export), flips a REAL restriction flag
//!   (restrict), and names its CI-P32 follow-on (erase) — a real, callable holder, never a panic. It
//!   registers the CI OLTP store through the substrate registry (contract 1.4) and classifies to H2 —
//!   0 orphans.
//! - **CONSUMER** = a minimal DSR-orchestrator stand-in that holds the CI holder behind
//!   `dyn PersonalDataHolder`, fans `locate` / `restrict` / `erase` out via the contract, and NEVER
//!   reaches into a store (the no-cross-store-read law, gdpr §3.1). This is the shape the real
//!   orchestrator (P-GA-11/P-GA-12, and the CI-P32 fan-out) takes when it fans a DSR out to CI.
//!
//! The dated green artifact: the consumer fans the DSR out to the CI holder; each op returns a
//! content-addressed receipt; the restriction flag is honoured (the seam reads it); the holder
//! classifies to H2 with 0 orphan stores; an unregistered CI store fails the holder-registered
//! architecture test (contract 1.4 — the enforcement). If 10.1's body shape drifts, this stops
//! compiling/passing — that is the contract. The REAL erase body (the per-subject/per-tenant DEK
//! crypto-shred + pseudonym shred + ci.*.erased tombstone fan-out, CI-D3) lands in CI-P32; this prompt
//! records the surface as registered-typed-with-named-follow-on, honestly.

use myelin_ci_controlplane::{
    ci_store_classifier, register_ci_holders, CiHolder, RestrictionFlag, CI_OLTP_STORE,
};
use myelin_gdpr::{
    EraseScope, LocateReport, PersonalDataHolder, RestrictReceipt, SubjectRef, TenantId,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_substrate::{
    assert_all_holders_registered, assert_holder_completeness, classify_store, DeclaredStore,
    Holder, HolderRegistry, StoreKind, StoreManifest,
};

fn subject(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId::from_token("acme"),
    ))
}

fn tenant() -> TenantId {
    TenantId::from_token("acme")
}

/// **The CONSUMER side (10.1): a DSR-orchestrator shape that fans out to the CI holder via the
/// contract.** It holds the holder behind `dyn PersonalDataHolder` and calls the contract — it never
/// reaches into a store. This is the shape the real orchestrator (P-GA-11/P-GA-12, and the CI-P32
/// fan-out) takes; the property pinned here is "the orchestrator touches a CI store ONLY through the
/// holder contract".
struct DsrOrchestratorConsumer<'a> {
    holders: Vec<&'a dyn PersonalDataHolder>,
}

impl<'a> DsrOrchestratorConsumer<'a> {
    fn new(holders: Vec<&'a dyn PersonalDataHolder>) -> Self {
        DsrOrchestratorConsumer { holders }
    }

    /// Fan a `locate` out to the CI holder via the contract; collect the reports.
    fn fan_out_locate(&self, subject: &SubjectRef, tenant: TenantId) -> Vec<LocateReport> {
        self.holders
            .iter()
            .map(|h| {
                h.locate(subject, tenant.clone())
                    .expect("the CI holder locate succeeds (typed seam)")
            })
            .collect()
    }

    /// Fan a `restrict` out to the CI holder via the contract; collect the receipts.
    fn fan_out_restrict(&self, subject: &SubjectRef, on: bool) -> Vec<RestrictReceipt> {
        self.holders
            .iter()
            .map(|h| {
                h.restrict(subject, on)
                    .expect("the CI holder restrict succeeds")
            })
            .collect()
    }

    /// Fan an `erase` out to the CI holder via the contract; assert each succeeds (stub no-op).
    fn fan_out_erase(&self, scope: EraseScope) -> usize {
        for h in &self.holders {
            h.erase(scope.clone())
                .expect("the CI holder erase succeeds (CI-P9 stub)");
        }
        self.holders.len()
    }
}

/// **provider + consumer wired together (the 10.1 CI CDC pair).** The orchestrator (consumer) fans
/// `locate` → `restrict` → `erase` out to the CI holder (provider); each returns a content-addressed
/// receipt over its (CI-P9 substrate) surface — the contract is honoured. This is the dated green
/// artifact for the CI side of 10.1.
#[test]
fn dsr_orchestrator_fans_the_dsr_out_to_the_ci_holder_via_the_contract() {
    let flag = RestrictionFlag::new();
    let ci = CiHolder::with_restriction(flag.clone());
    let consumer = DsrOrchestratorConsumer::new(vec![&ci]);
    let subj = subject("psn:ci-cdc");

    // locate: the holder responds with a content-addressed receipt over its (typed seam) surface.
    let reports = consumer.fan_out_locate(&subj, tenant());
    assert_eq!(
        reports.len(),
        1,
        "the CI holder responded to locate via the contract"
    );
    for r in &reports {
        assert_eq!(r.receipt.operation, "locate");
        assert!(
            r.receipt.content_hash.starts_with("blake3:"),
            "content-addressed receipt"
        );
        assert!(
            r.receipt.key_epoch_destroyed.is_none(),
            "locate shreds no key"
        );
    }

    // restrict: the holder flips a REAL flag the CI seams read (honoured-at-the-seam, not a no-op).
    let restricts = consumer.fan_out_restrict(&subj, true);
    assert_eq!(
        restricts.len(),
        1,
        "the CI holder honoured restrict via the contract"
    );
    assert!(
        flag.is_restricted("psn:ci-cdc"),
        "the restriction flag the CI index/agent/analytics/notif seams read is SET"
    );

    // erase: the holder is a well-defined no-op now (the CI-P9 substrate) — never a panic.
    let erased = consumer.fan_out_erase(EraseScope::Subject {
        subject: subj.clone(),
        tenant: tenant(),
    });
    assert_eq!(erased, 1, "the CI holder honoured the erase contract");
}

/// **The provider registers + classifies (contract 1.4 + gdpr §3.2): 0 orphan CI stores.** The CI
/// OLTP schema classifies to H2 (`H2Ci`) — every CI store is in the exhaustive H1–H18 list, so the M5
/// DSAR fan-out cannot silently miss CI.
#[test]
fn ci_holder_store_registers_and_classifies_with_zero_orphans() {
    let registry = register_ci_holders();
    let classifier = ci_store_classifier();
    assert_eq!(
        classify_store(StoreKind::Oltp, CI_OLTP_STORE, &classifier),
        Some(Holder::H2Ci),
        "the CI OLTP schema is holder H2"
    );
    assert_eq!(
        assert_holder_completeness(registry.registrations(), &classifier),
        Ok(()),
        "every CI store is in the exhaustive H1–H18 list — 0 orphan stores"
    );
}

/// **The 1.4 enforcement (the CI-P9 GATE): a CI store opened OUTSIDE the harness FAILS the
/// holder-registered architecture test.** The conforming registry (the CI OLTP store opened through
/// the one door) passes; a registry missing it (opened outside the harness) is a loud violation naming
/// exactly the escaped store — an unregistered PII store cannot quietly miss the DSR fan-out.
#[test]
fn an_unregistered_ci_store_fails_the_holder_registered_architecture_test() {
    let manifest = StoreManifest::of([DeclaredStore::new(StoreKind::Oltp, CI_OLTP_STORE)]);
    // CONFORMING: the CI OLTP store opened through the harness one door.
    assert_eq!(
        assert_all_holders_registered(&manifest, &register_ci_holders()),
        Ok(()),
        "the CI store opened through the harness → the architecture test passes"
    );
    // VIOLATING: the CI store never went through the door.
    let rogue = HolderRegistry::new();
    let err = assert_all_holders_registered(&manifest, &rogue)
        .expect_err("a CI store opened outside the harness must FAIL the architecture test");
    assert_eq!(
        err.len(),
        1,
        "exactly the unregistered CI store is the violation"
    );
    assert!(
        err[0].message().contains(CI_OLTP_STORE),
        "the failure names the escaped CI store: {}",
        err[0].message()
    );
}

/// **The seam is typed + empty-but-correct (the CI-P9 surface), not an error.** `export` over the CI
/// holder returns an empty bundle with a content-addressed receipt — a real, callable holder, not a
/// `todo!()`/`Err`. The real exported data lands with the DSR fan-out (CI-P32) + the log/artifact
/// bands (CI-P20/P22).
#[test]
fn ci_holder_export_is_typed_and_empty_but_correct() {
    let ci = CiHolder::new();
    let bundle = ci
        .export(&subject("psn:ci-1"), tenant())
        .expect("export over the CI holder seam succeeds");
    assert_eq!(bundle.receipt.operation, "export");
    assert!(bundle.receipt.content_hash.starts_with("blake3:"));
}
