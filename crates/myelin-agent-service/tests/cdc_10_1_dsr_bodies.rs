//! # CDC 10.1 / 10.9 / 10.4 / 11.4 / 4.8 — the Fabric's FULL DSR holder BODIES wired into the
//! DSR fan-out (AG-P23 → P-479)
//!
//! **Contracts:** index rows **10.1** (the Fabric's `PersonalDataHolder` bodies — locate/export/erase
//! for run/trace/memory), **10.9** (the ONE erasure posture — purge / crypto-shred / pseudonymise,
//! never hide — instantiated **by reference**), **10.4** (the DSR fan-out iterates holders — the
//! Fabric holder is wired in), **11.4** (per-subject DEK crypto-shred — the lever), **4.8**
//! (`resolve_pseudonym` — attribution falls back to the opaque pseudonym).
//!
//! This is the provider+consumer CDC pair for the AG-P23 bodies (distinct from `cdc_10_1_agent_holder`,
//! which pins the AG-P3 REGISTRATION SEAM). Here:
//! - **PROVIDER** = [`AgentFabricHolder`] implementing the five-op 10.1 contract with REAL bodies (the
//!   per-subject DEK crypto-shred + the pseudonym fallback).
//! - **CONSUMER** = a minimal DSR-orchestrator stand-in that holds the Fabric holder behind
//!   `dyn PersonalDataHolder`, fans `locate` then `erase` out via the contract (10.4), and asserts the
//!   erase receipt records the destroyed key epoch (11.4) — it NEVER reaches into a store.
//!
//! If 10.1's body shape or the 11.4 receipt grammar drifts, this stops compiling/passing — the contract.

use myelin_agent_service::{subject_dek_ref, AgentFabricHolder, AgentFabricStore};
use myelin_events::{InMemoryShredder, InlinePiiShredder};
use myelin_gdpr::{EraseScope, PersonalDataHolder, Receipt, SubjectRef, TenantId};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::TenantId as TyTenantId;

fn tenant() -> TenantId {
    TyTenantId("acme".into())
}

fn subject_ref(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        tenant(),
    ))
}

fn seeded(subject: &str) -> AgentFabricHolder<InMemoryShredder> {
    let mut store = AgentFabricStore::new();
    store.write_free_text(1, "proposed_effect.input_payload", subject);
    store.write_free_text(1, "trace.trace_body", subject);
    store.write_attribution(1, subject);
    let shredder = InMemoryShredder::new();
    shredder.seal(&subject_dek_ref("acme", subject));
    AgentFabricHolder::new(tenant(), store, shredder)
}

/// **The CONSUMER side (10.4): a DSR-orchestrator shape that fans out to the Fabric holder via the
/// contract.** It holds the holder behind `dyn PersonalDataHolder` and calls the 10.1 surface — it
/// never reaches into a store. This is the shape the real orchestrator (P-GA-12) takes when it fans a
/// DSR out to the Agent-Fabric holder (the AG-D10 fan-out).
struct DsrFanout<'a> {
    holders: Vec<&'a dyn PersonalDataHolder>,
}

impl<'a> DsrFanout<'a> {
    fn new(holders: Vec<&'a dyn PersonalDataHolder>) -> Self {
        DsrFanout { holders }
    }

    /// Fan an `erase` out to every holder via the contract; collect the receipts (10.4).
    fn fan_out_erase(&self, scope: EraseScope) -> Vec<Receipt> {
        self.holders
            .iter()
            .map(|h| {
                h.erase(scope.clone())
                    .expect("the Fabric holder erase succeeds via the contract")
                    .receipt
            })
            .collect()
    }
}

/// **provider + consumer wired together (the 10.1/10.4/11.4 CDC pair).** The orchestrator (consumer)
/// fans `erase` out to the Fabric holder (provider); the holder crypto-shreds the per-subject DEK
/// (11.4) and returns a content-addressed erase receipt recording the destroyed key epoch — the
/// contract is honoured. The ONE erasure posture (10.9) is instantiated, not restated.
#[test]
fn dsr_fan_out_erase_crypto_shreds_via_the_contract_and_records_the_key_epoch() {
    let subject = "psn:cdc";
    let holder = seeded(subject);
    let dek = subject_dek_ref("acme", subject);
    assert!(holder.shredder().is_live(&dek), "the DEK is live pre-erase");

    let fanout = DsrFanout::new(vec![&holder]);
    let receipts = fanout.fan_out_erase(EraseScope::Subject {
        subject: subject_ref(subject),
        tenant: tenant(),
    });

    assert_eq!(receipts.len(), 1, "the fan-out reached the Fabric holder");
    let r = &receipts[0];
    assert_eq!(r.operation, "erase");
    assert!(
        r.content_hash.starts_with("blake3:"),
        "content-addressed (11.4)"
    );
    // 11.4 — the receipt records WHICH per-subject DEK epoch the crypto-shred destroyed.
    assert_eq!(
        r.key_epoch_destroyed,
        Some(0),
        "the erase receipt records the destroyed per-subject DEK epoch (11.4 / GD-4)"
    );
    // the crypto-shred actually destroyed the key (the posture is real, not a claim).
    assert!(
        !holder.shredder().is_live(&dek),
        "the per-subject DEK is destroyed (10.9 crypto-shred, never hide)"
    );
}

/// **4.8 — attribution falls back to the opaque pseudonym on erase.** After the erase the attribution
/// edge is pseudonymised (the run FACT survives; the identity behind it does not).
#[test]
fn erase_pseudonymises_the_attribution_edge() {
    let subject = "psn:dee";
    let holder = seeded(subject);
    let receipt = holder.erase_fabric(&subject_ref(subject)).expect("erase");
    assert_eq!(
        receipt.attribution_pseudonymised, 1,
        "the attribution edge falls back to the opaque pseudonym (4.8)"
    );
    assert_eq!(
        receipt.recoverable, 0,
        "0 recoverable free-text (10.9 crypto-shred)"
    );
}

/// **10.1 — locate / export bodies are real content-addressed receipts that span the run + trace.**
/// The consumer can fan a `locate` out and read which free-text the crypto-shred would reach.
#[test]
fn locate_body_spans_the_run_and_trace_via_the_contract() {
    let subject = "psn:eve";
    let holder = seeded(subject);
    let report = holder
        .locate(&subject_ref(subject), tenant())
        .expect("locate via the contract");
    assert_eq!(report.receipt.operation, "locate");
    assert!(report.receipt.content_hash.starts_with("blake3:"));
    assert!(
        report.receipt.key_epoch_destroyed.is_none(),
        "locate shreds no key"
    );
}
