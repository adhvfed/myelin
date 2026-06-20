//! # CDC 10.1 / 1.4 — the Agent-Fabric side of `PersonalDataHolder{locate, export, rectify,
//! restrict, erase}` + the holder registration seam (AG-P3 → P-132)
//!
//! **Contract:** index rows **10.1** (`PersonalDataHolder` — the five DSR operations) + **1.4**
//! (`PersonalDataHolder` auto-registration on every store the harness opens). The SIGNATURE was
//! frozen at P-GA-01 (`myelin-gdpr`); the GDPR-owned holder bodies landed at P-GA-05. THIS file
//! ships the **Agent-Fabric side** of 10.1/1.4 — the Fabric stores as holders **H11
//! (`AgentMemory`)** (the run/tool_def/proposed_effect/hitl_gate OLTP schema) + **H17
//! (`AgentTrace`)** (the execution trace + conversation history). It is the REGISTRATION SEAM
//! (AG-P3): the holders are registered + classified + callable; the real DSR fan-out bodies land in
//! AG-P23 (→ P-479) and the trace holder body in AG-P19 (→ P-268). It is the provider+consumer CDC
//! pair the contract-coverage scanner (P-S21) reads for the Agent-Fabric holder seam.
//!
//! - **PROVIDER** = the Agent-Fabric holders ([`AgentOltpHolder`] H11 / [`AgentTraceHolder`] H17)
//!   IMPLEMENTING the five-operation 10.1 contract. At AG-P3 they respond with **empty-but-correct**
//!   receipts (the registration seam) — a real, callable stub, never a panic. They register their
//!   stores through the substrate holder registry (contract 1.4) and classify to their H-holders
//!   (H11 OLTP, H17 trace) — 0 orphans.
//! - **CONSUMER** = a minimal DSR-orchestrator stand-in that holds the Fabric holders behind
//!   `dyn PersonalDataHolder`, fans `locate` + `erase` out to them via the contract, and NEVER
//!   reaches into a store (the no-cross-store-read law, gdpr §3.1). This is the shape the real
//!   orchestrator (P-GA-11/P-GA-12) takes when it fans a DSR out to the Agent-Fabric holders in
//!   AG-P23 (the AG-D10 "erasure reaches the trace + memory" fan-out).
//!
//! The dated green artifact: the consumer fans `locate(subject)` + `erase(subject)` out to the
//! Fabric holders; each returns a content-addressed receipt over its (registration-seam) surface;
//! the holders classify to H11/H17 with 0 orphan stores; an unregistered Fabric store fails the
//! holder-registered architecture test (contract 1.4 — the enforcement). If 10.1's body shape
//! drifts, this stops compiling/passing — that is the contract. The REAL erase body (crypto-shred
//! the per-subject DEK + pseudonym-shred the attribution edges + tombstone, AG-D10) lands in AG-P23;
//! this prompt records the surface as registered-no-op-with-named-follow-on, honestly.

use myelin_agent_service::{
    agent_store_classifier, register_agent_holders, AgentOltpHolder, AgentTraceHolder,
    AGENT_OLTP_STORE, AGENT_TRACE_STORE,
};
use myelin_gdpr::{EraseScope, LocateReport, PersonalDataHolder, SubjectRef, TenantId};
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

/// **The CONSUMER side (10.1): a DSR-orchestrator shape that fans out to the Fabric holders via the
/// contract.** It holds the holders behind `dyn PersonalDataHolder` (a heterogeneous set) and calls
/// the contract — it never reaches into a store. This is the shape the real orchestrator
/// (P-GA-11/P-GA-12, and the AG-P23 fan-out) takes; the property pinned here is "the orchestrator
/// touches a Fabric store ONLY through the holder contract".
struct DsrOrchestratorConsumer<'a> {
    holders: Vec<&'a dyn PersonalDataHolder>,
}

impl<'a> DsrOrchestratorConsumer<'a> {
    fn new(holders: Vec<&'a dyn PersonalDataHolder>) -> Self {
        DsrOrchestratorConsumer { holders }
    }

    /// Fan a `locate` out to every Fabric holder via the contract; collect the reports.
    fn fan_out_locate(&self, subject: &SubjectRef, tenant: TenantId) -> Vec<LocateReport> {
        self.holders
            .iter()
            .map(|h| {
                h.locate(subject, tenant.clone())
                    .expect("an Agent-Fabric holder locate succeeds (seam)")
            })
            .collect()
    }

    /// Fan an `erase` out to every Fabric holder via the contract; assert each succeeds (no-op seam).
    fn fan_out_erase(&self, scope: EraseScope) -> usize {
        for h in &self.holders {
            h.erase(scope.clone())
                .expect("an Agent-Fabric holder erase succeeds (no-op seam)");
        }
        self.holders.len()
    }
}

/// **provider + consumer wired together (the 10.1 Agent-Fabric CDC pair).** The orchestrator
/// (consumer) fans `locate` then `erase` out to the H11 OLTP holder + the H17 trace holder
/// (providers); each returns a content-addressed receipt over its registration-seam surface — the
/// contract is honoured. This is the dated green artifact for the Agent-Fabric side of 10.1.
#[test]
fn dsr_orchestrator_fans_locate_and_erase_out_to_the_agent_holders_via_the_contract() {
    let oltp = AgentOltpHolder;
    let trace = AgentTraceHolder;
    let consumer = DsrOrchestratorConsumer::new(vec![&oltp, &trace]);
    let subj = subject("psn:agent-cdc");

    // locate: each holder responds with a content-addressed receipt over its (seam) surface.
    let reports = consumer.fan_out_locate(&subj, tenant());
    assert_eq!(reports.len(), 2, "both Agent-Fabric holders responded to locate via the contract");
    for r in &reports {
        assert_eq!(r.receipt.operation, "locate");
        assert!(r.receipt.content_hash.starts_with("blake3:"), "content-addressed receipt");
        assert!(r.receipt.key_epoch_destroyed.is_none(), "locate shreds no key");
    }

    // erase: each holder is a well-defined no-op now (the seam) — never a panic.
    let erased = consumer.fan_out_erase(EraseScope::Subject {
        subject: subj.clone(),
        tenant: tenant(),
    });
    assert_eq!(erased, 2, "both Agent-Fabric holders honoured the erase contract");
}

/// **The provider registers + classifies (contract 1.4 + gdpr §3.2): 0 orphan Fabric stores.** The
/// OLTP schema classifies to H11 (`AgentMemory`), the trace store to H17 (`AgentTrace`) — every
/// Fabric store is in the exhaustive H1–H18 list, so the M5 DSAR fan-out cannot silently miss the
/// Agent Fabric.
#[test]
fn agent_holder_stores_register_and_classify_with_zero_orphans() {
    let registry = register_agent_holders();
    let classifier = agent_store_classifier();
    assert_eq!(
        classify_store(StoreKind::Oltp, AGENT_OLTP_STORE, &classifier),
        Some(Holder::H11AgentMemory),
        "the Fabric OLTP schema is holder H11"
    );
    assert_eq!(
        classify_store(StoreKind::Oltp, AGENT_TRACE_STORE, &classifier),
        Some(Holder::H17AgentTrace),
        "the execution-trace store is holder H17"
    );
    assert_eq!(
        assert_holder_completeness(registry.registrations(), &classifier),
        Ok(()),
        "every Agent-Fabric store is in the exhaustive H1–H18 list — 0 orphan stores"
    );
}

/// **The 1.4 enforcement (the AG-P3 GATE): a Fabric store opened OUTSIDE the harness FAILS the
/// holder-registered architecture test.** The conforming registry (both opened through the one door)
/// passes; a registry missing the trace store (opened outside the harness) is a loud violation
/// naming exactly the escaped store — an unregistered PII store cannot quietly miss the DSR fan-out.
#[test]
fn an_unregistered_agent_store_fails_the_holder_registered_architecture_test() {
    let manifest = StoreManifest::of([
        DeclaredStore::new(StoreKind::Oltp, AGENT_OLTP_STORE),
        DeclaredStore::new(StoreKind::Oltp, AGENT_TRACE_STORE),
    ]);
    // CONFORMING: both Fabric stores opened through the harness one door.
    assert_eq!(
        assert_all_holders_registered(&manifest, &register_agent_holders()),
        Ok(()),
        "both Fabric stores opened through the harness → the architecture test passes"
    );
    // VIOLATING: the trace store never went through the door.
    let mut rogue = HolderRegistry::new();
    rogue.open(StoreKind::Oltp, AGENT_OLTP_STORE);
    let err = assert_all_holders_registered(&manifest, &rogue)
        .expect_err("a Fabric store opened outside the harness must FAIL the architecture test");
    assert_eq!(err.len(), 1, "exactly the unregistered trace store is the violation");
    assert!(
        err[0].message().contains(AGENT_TRACE_STORE),
        "the failure names the escaped Fabric store: {}",
        err[0].message()
    );
}

/// **The seam is empty-but-correct (the AG-P3 surface), not an error.** `export` over the
/// registration seam returns an empty bundle with a content-addressed receipt — a real, callable
/// holder, not a `todo!()`/`Err`. The real located/exported data lands with the DSR fan-out
/// (AG-P23).
#[test]
fn agent_holder_export_is_empty_but_correct() {
    let trace = AgentTraceHolder;
    let bundle = trace
        .export(&subject("psn:agent-1"), tenant())
        .expect("export over the registration seam succeeds");
    assert_eq!(bundle.receipt.operation, "export");
    assert!(bundle.receipt.content_hash.starts_with("blake3:"));
}
