//! # CDC 10.1 — the Search side of `PersonalDataHolder{locate, export, rectify, restrict, erase}`
//! (SRCH-P02 → P-122)
//!
//! **Contract:** index row 10.1 (`PersonalDataHolder` — the five DSR operations). The SIGNATURE was
//! frozen at P-GA-01 (`myelin-gdpr`); the GDPR-owned holder bodies landed at P-GA-05. THIS file
//! ships the **Search side** of 10.1 — Search as holder **H7 (`SearchIndex`)**, a STUB surface at
//! M1 (no index exists yet; the real PURGE + REINDEX erase is SRCH-P15). It is the provider+consumer
//! CDC pair the contract-coverage scanner (P-S21) reads for the Search holder seam.
//!
//! - **PROVIDER** = the Search holder ([`SearchIndexHolder`] H7) IMPLEMENTING the five-operation
//!   10.1 contract. At M1 it responds with **empty-but-correct** receipts (a tenant with no index
//!   has no located docs/vectors) — a real, callable stub, never a panic. It registers its store
//!   through the substrate holder registry (contract 1.4) and classifies to H7 (structurally, by the
//!   SearchIndex kind) — 0 orphans.
//! - **CONSUMER** = a minimal DSR-orchestrator stand-in that holds the Search holder behind
//!   `dyn PersonalDataHolder`, fans `locate` + `erase` out to it via the contract, and NEVER reaches
//!   into the store (the no-cross-store-read law, gdpr §3.1). This is the shape the real orchestrator
//!   (P-GA-11/P-GA-12) takes when it fans a DSR out to the Search holder.
//!
//! The dated green artifact: the consumer fans `locate(subject)` + `erase(subject)` out to the
//! Search holder; it returns a content-addressed receipt over its (empty) surface; it classifies to
//! H7 with 0 orphan stores. If 10.1's body shape drifts, this stops compiling/passing — that is the
//! contract. The REAL erase body (purge + reindex, vectors compacted, restrict suppression, §4.8)
//! lands in SRCH-P15; this prompt records the surface as untested-at-runtime-but-named (no engine to
//! drill at M1), honestly.

use myelin_gdpr::{EraseScope, LocateReport, PersonalDataHolder, SubjectRef, TenantId};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_search::{
    register_search_holder, search_index_holder, SearchIndexHolder, SEARCH_INDEX_STORE,
};
use myelin_substrate::{assert_holder_completeness, Holder, StoreKind};

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

/// **The CONSUMER side (10.1): a DSR-orchestrator shape that fans out to the Search holder via the
/// contract.** It holds the holder behind `dyn PersonalDataHolder` and calls the contract — it never
/// reaches into the store. This is the shape the real orchestrator (P-GA-11/P-GA-12) takes; the
/// property pinned here is "the orchestrator touches the Search store ONLY through the holder
/// contract".
struct DsrOrchestratorConsumer<'a> {
    holders: Vec<&'a dyn PersonalDataHolder>,
}

impl<'a> DsrOrchestratorConsumer<'a> {
    fn new(holders: Vec<&'a dyn PersonalDataHolder>) -> Self {
        DsrOrchestratorConsumer { holders }
    }

    /// Fan a `locate` out to every Search holder via the contract; collect the reports.
    fn fan_out_locate(&self, subject: &SubjectRef, tenant: TenantId) -> Vec<LocateReport> {
        self.holders
            .iter()
            .map(|h| {
                h.locate(subject, tenant.clone())
                    .expect("a Search holder locate succeeds (stub)")
            })
            .collect()
    }

    /// Fan an `erase` out to every Search holder via the contract; assert each succeeds (no-op stub).
    fn fan_out_erase(&self, scope: EraseScope) -> usize {
        for h in &self.holders {
            h.erase(scope.clone())
                .expect("a Search holder erase succeeds (no-op stub)");
        }
        self.holders.len()
    }
}

/// **provider + consumer wired together (the 10.1 Search CDC pair).** The orchestrator (consumer)
/// fans `locate` then `erase` out to the H7 index holder (provider); it returns a content-addressed
/// receipt over its empty surface — the contract is honoured. This is the dated green artifact for
/// the Search side of 10.1.
#[test]
fn dsr_orchestrator_fans_locate_and_erase_out_to_the_search_holder_via_the_contract() {
    let index = SearchIndexHolder;
    let consumer = DsrOrchestratorConsumer::new(vec![&index]);
    let subj = subject("u-cdc");

    // locate: the holder responds with a content-addressed receipt over its (empty) surface.
    let reports = consumer.fan_out_locate(&subj, tenant());
    assert_eq!(
        reports.len(),
        1,
        "the Search holder responded to locate via the contract"
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

    // erase: the holder is a well-defined no-op now (nothing to purge) — never a panic.
    let erased = consumer.fan_out_erase(EraseScope::Subject {
        subject: subj.clone(),
        tenant: tenant(),
    });
    assert_eq!(erased, 1, "the Search holder honoured the erase contract");
}

/// **The provider registers + classifies (contract 1.4 + gdpr §3.2): 0 orphan Search stores.** The
/// index store classifies structurally to H7 (`SearchIndex`) — the Search store is in the exhaustive
/// H1–H18 list, so the M5 DSAR fan-out cannot silently miss Search.
#[test]
fn search_holder_store_registers_and_classifies_with_zero_orphans() {
    let registry = register_search_holder();
    assert!(registry.is_registered(StoreKind::SearchIndex, SEARCH_INDEX_STORE));
    assert_eq!(
        search_index_holder(),
        Some(Holder::H7SearchIndex),
        "the per-tenant index is holder H7"
    );
    // The search index classifies structurally (kind=SearchIndex → H7), so an empty classifier
    // suffices; the completeness assertion is GREEN — 0 orphan stores.
    assert_eq!(
        assert_holder_completeness(registry.registrations(), &Default::default()),
        Ok(()),
        "the Search index store is in the exhaustive H1–H18 list — 0 orphan stores"
    );
}

/// **The stub is empty-but-correct (the SRCH-P02 surface), not an error.** `export` over a tenant
/// with no index returns an empty bundle with a content-addressed receipt — a real, callable holder,
/// not a `todo!()`/`Err`. The index is derived/reconstructible — never the export source of truth;
/// the real located/exported data lands with the index (SRCH-P15).
#[test]
fn search_holder_export_is_empty_but_correct() {
    let index = SearchIndexHolder;
    let bundle = index
        .export(&subject("u-1"), tenant())
        .expect("export of an empty bundle succeeds (no index yet)");
    assert_eq!(bundle.receipt.operation, "export");
    assert!(bundle.receipt.content_hash.starts_with("blake3:"));
}
