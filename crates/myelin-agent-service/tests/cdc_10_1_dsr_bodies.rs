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

struct DsrFanout<'a> {
    holders: Vec<&'a dyn PersonalDataHolder>,
}

impl<'a> DsrFanout<'a> {
    fn new(holders: Vec<&'a dyn PersonalDataHolder>) -> Self {
        DsrFanout { holders }
    }

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
    assert_eq!(
        r.key_epoch_destroyed,
        Some(0),
        "the erase receipt records the destroyed per-subject DEK epoch (11.4 / GD-4)"
    );
    assert!(
        !holder.shredder().is_live(&dek),
        "the per-subject DEK is destroyed (10.9 crypto-shred, never hide)"
    );
}

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
