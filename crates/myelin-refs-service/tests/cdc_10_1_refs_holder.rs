//! # CDC 10.1 — the Refs side of `PersonalDataHolder{locate, export, rectify, restrict, erase}`
//! (REF-P3 → P-120)
//!
//! **Contract:** index row 10.1 (`PersonalDataHolder` — the five DSR operations). The SIGNATURE was
//! frozen at P-GA-01 (`myelin-gdpr`); the GDPR-owned holder bodies landed at P-GA-05. THIS file
//! ships the **Refs side** of 10.1 — Refs as holder **H12 (`ReferenceGraph`)**. The REAL §4.6
//! erasure body landed at REF-P15 / P-164 (`with_backing` / `with_cache` over the live edge
//! projection + R2 cache); this CDC pair exercises the contract over the **registration-only
//! (unbacked) form** (the `serve`-before-the-store-is-wired posture), which is **empty-but-correct**
//! by construction. The unbacked surface is the provider+consumer CDC pair the contract-coverage
//! scanner (P-S21) reads for the Refs holder seam; the REAL backed body is drilled in the
//! `holder::tests` unit module (REF-D5 CI variant: 0 recoverable cache PII).
//!
//! - **PROVIDER** = the Refs holders ([`RefsEdgeHolder`] H12 / [`RefsCacheHolder`] §3.6) IMPLEMENTING
//!   the five-operation 10.1 contract. At M1 they respond with **empty-but-correct** receipts (a
//!   tenant with no edges has no located data) — a real, callable stub, never a panic. They register
//!   their stores through the substrate holder registry (contract 1.4) and classify to their
//!   H-holders (H12 edge index, H9 cache) — 0 orphans.
//! - **CONSUMER** = a minimal DSR-orchestrator stand-in that holds the Refs holders behind
//!   `dyn PersonalDataHolder`, fans `locate` + `erase` out to them via the contract, and NEVER
//!   reaches into a store (the no-cross-store-read law, gdpr §3.1). This is the shape the real
//!   orchestrator (P-GA-11/P-GA-12) takes when it fans a DSR out to the Refs holder.
//!
//! The dated green artifact: the consumer fans `locate(subject)` + `erase(subject)` out to the Refs
//! holders; each returns a content-addressed receipt over its (unbacked, empty-but-correct) surface;
//! the holders classify to H12/H9 with 0 orphan stores. If 10.1's body shape drifts, this stops
//! compiling/passing — that is the contract. The REAL erase body (purge R2-cache PII + reliance on
//! Identity's pseudonym shred for `origin_actor` + `*.erased` tombstoning, §4.6) landed at REF-P15;
//! the full backup-level 0-recoverable shred drill (REF-D5 at scale) is REF-P25 (named).

use myelin_gdpr::{EraseScope, LocateReport, PersonalDataHolder, SubjectRef, TenantId};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs_service::{
    refs_store_classifier, register_refs_holders, RefsCacheHolder, RefsEdgeHolder, REFS_CACHE_STORE,
    REFS_EDGE_STORE,
};
use myelin_substrate::{
    assert_holder_completeness, classify_store, Holder, StoreKind,
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

/// **The CONSUMER side (10.1): a DSR-orchestrator shape that fans out to the Refs holders via the
/// contract.** It holds the holders behind `dyn PersonalDataHolder` (a heterogeneous set) and calls
/// the contract — it never reaches into a store. This is the shape the real orchestrator
/// (P-GA-11/P-GA-12) takes; the property pinned here is "the orchestrator touches a Refs store ONLY
/// through the holder contract".
struct DsrOrchestratorConsumer<'a> {
    holders: Vec<&'a dyn PersonalDataHolder>,
}

impl<'a> DsrOrchestratorConsumer<'a> {
    fn new(holders: Vec<&'a dyn PersonalDataHolder>) -> Self {
        DsrOrchestratorConsumer { holders }
    }

    /// Fan a `locate` out to every Refs holder via the contract; collect the reports.
    fn fan_out_locate(&self, subject: &SubjectRef, tenant: TenantId) -> Vec<LocateReport> {
        self.holders
            .iter()
            .map(|h| h.locate(subject, tenant.clone()).expect("a Refs holder locate succeeds (stub)"))
            .collect()
    }

    /// Fan an `erase` out to every Refs holder via the contract; assert each succeeds (no-op stub).
    fn fan_out_erase(&self, scope: EraseScope) -> usize {
        for h in &self.holders {
            h.erase(scope.clone())
                .expect("a Refs holder erase succeeds (no-op stub)");
        }
        self.holders.len()
    }
}

/// **provider + consumer wired together (the 10.1 Refs CDC pair).** The orchestrator (consumer)
/// fans `locate` then `erase` out to the H12 edge holder + the §3.6 cache holder (providers); each
/// returns a content-addressed receipt over its empty surface — the contract is honoured. This is
/// the dated green artifact for the Refs side of 10.1.
#[test]
fn dsr_orchestrator_fans_locate_and_erase_out_to_the_refs_holders_via_the_contract() {
    let edge = RefsEdgeHolder::default();
    let cache = RefsCacheHolder::default();
    let consumer = DsrOrchestratorConsumer::new(vec![&edge, &cache]);
    let subj = subject("u-cdc");

    // locate: each holder responds with a content-addressed receipt over its (empty) surface.
    let reports = consumer.fan_out_locate(&subj, tenant());
    assert_eq!(reports.len(), 2, "both Refs holders responded to locate via the contract");
    for r in &reports {
        assert_eq!(r.receipt.operation, "locate");
        assert!(r.receipt.content_hash.starts_with("blake3:"), "content-addressed receipt");
        assert!(r.receipt.key_epoch_destroyed.is_none(), "locate shreds no key");
    }

    // erase: each holder is a well-defined no-op now (nothing to shred) — never a panic.
    let erased = consumer.fan_out_erase(EraseScope::Subject {
        subject: subj.clone(),
        tenant: tenant(),
    });
    assert_eq!(erased, 2, "both Refs holders honoured the erase contract");
}

/// **The provider registers + classifies (contract 1.4 + gdpr §3.2): 0 orphan Refs stores.** The
/// edge OLTP store classifies to H12 (`ReferenceGraph`), the R2 cache to H9 (`Caches`) — every Refs
/// store is in the exhaustive H1–H18 list, so the M5 DSAR fan-out cannot silently miss Refs.
#[test]
fn refs_holder_stores_register_and_classify_with_zero_orphans() {
    let registry = register_refs_holders();
    let classifier = refs_store_classifier();
    assert_eq!(
        classify_store(StoreKind::Oltp, REFS_EDGE_STORE, &classifier),
        Some(Holder::H12ReferenceGraph),
        "the edge inverse-index is holder H12"
    );
    assert_eq!(
        classify_store(StoreKind::Cache, REFS_CACHE_STORE, &classifier),
        Some(Holder::H9Caches),
        "the R2 projection cache classifies structurally to H9"
    );
    assert_eq!(
        assert_holder_completeness(registry.registrations(), &classifier),
        Ok(()),
        "every Refs store is in the exhaustive H1–H18 list — 0 orphan stores"
    );
}

/// **The stub is empty-but-correct (the REF-P3 surface), not an error.** `export` over a tenant with
/// no edges returns an empty bundle with a content-addressed receipt — a real, callable holder, not
/// a `todo!()`/`Err`. The real located/exported data lands with the edge index (REF-P15).
#[test]
fn refs_holder_export_is_empty_but_correct() {
    let edge = RefsEdgeHolder::default();
    let bundle = edge
        .export(&subject("u-1"), tenant())
        .expect("export of an empty bundle succeeds (no edges yet)");
    assert_eq!(bundle.receipt.operation, "export");
    assert!(bundle.receipt.content_hash.starts_with("blake3:"));
}
