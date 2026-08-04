use myelin_agent_service::{
    subject_dek_ref, AgentFabricHolder, AgentFabricStore, FabricErasureLedger,
};
use myelin_events::{InMemoryShredder, InlinePiiShredder};
use myelin_gdpr::{PersonalDataHolder, SubjectRef, TenantId};
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

fn seed(subject: &str) -> AgentFabricHolder<InMemoryShredder> {
    let mut store = AgentFabricStore::new();
    store.write_free_text(7, "proposed_effect.input_payload", subject);
    store.write_free_text(7, "hitl_gate.risk_summary", subject);
    store.write_free_text(7, "trace.trace_body", subject);
    store.write_attribution(7, subject);
    let shredder = InMemoryShredder::new();
    shredder.seal(&subject_dek_ref("acme", subject));
    AgentFabricHolder::new(tenant(), store, shredder)
}

#[test]
fn ag_d10_erasure_reaches_the_trace_and_memory_zero_recoverable() {
    let subject = "psn:alice";
    let dek = subject_dek_ref("acme", subject);
    let holder = seed(subject);

    assert!(
        holder.shredder().is_live(&dek),
        "PRE-erase the per-subject DEK is live - the free-text is recoverable"
    );
    let pre = holder.locate_fabric(&subject_ref(subject));
    assert_eq!(
        pre.free_text_rows.len(),
        3,
        "3 free-text PII rows (proposed_effect / hitl_gate / trace) located pre-erase"
    );
    assert!(
        pre.memory_seam.is_none(),
        "the memory leg is the NAMED SEAM - v1 has no embedding store (AG-P25)"
    );

    let receipt = holder
        .erase_fabric(&subject_ref(subject))
        .expect("the Fabric erase succeeds (KMS reachable)");

    assert_eq!(
        receipt.recoverable, 0,
        "0 recoverable PII post-erase - THE GATE"
    );
    assert!(
        !holder.shredder().is_live(&dek),
        "the per-subject DEK does NOT resolve after the crypto-shred - the trace body, the \
         proposed_effect input, and the hitl_gate risk summary are all unrecoverable"
    );
    assert!(receipt.dek_destroyed, "the per-subject DEK was destroyed");
    assert_eq!(
        receipt.free_text_shredded, 3,
        "all 3 free-text rows crypto-shred-tombstoned (the trace reached too)"
    );
    assert_eq!(
        receipt.attribution_pseudonymised, 1,
        "the attribution edge falls back to the opaque pseudonym (4.8)"
    );
    assert_eq!(
        receipt.memory_embeddings_purged, 0,
        "0 embeddings purged - the named memory seam (AG-P25 post-M5)"
    );
    assert!(receipt.is_green(), "the AG-D10 live-store leg is GREEN");

    let body = holder
        .erase(myelin_gdpr::EraseScope::Subject {
            subject: subject_ref(subject),
            tenant: tenant(),
        })
        .expect("the holder erase body succeeds (idempotent re-erase)");
    assert_eq!(
        body.receipt.key_epoch_destroyed,
        Some(0),
        "the erase receipt records the destroyed per-subject DEK epoch"
    );

    let ledger = FabricErasureLedger::new();
    ledger.record(subject, receipt.dek.clone());
    assert!(
        ledger.is_erased(subject),
        "the PII-free ledger remembers the erase"
    );

    holder.shredder().seal(&dek);
    assert!(
        holder.shredder().is_live(&dek),
        "the restore resurrected the per-subject DEK (the older backup brought it back)"
    );

    let re = ledger
        .re_erase_after_restore(holder.shredder())
        .expect("the re-erasure pass runs (cold == live)");
    assert_eq!(
        re.keys_resurrected_by_restore, 1,
        "the restore brought back 1 per-subject DEK (the honest signal)"
    );
    assert_eq!(
        re.resurrected, 0,
        "0 resurrected per-subject DEKs post re-erasure - THE BACKUP GATE"
    );
    assert!(re.is_green(), "the AG-D10 backup leg is GREEN");
    assert!(
        !holder.shredder().is_live(&dek),
        "the per-subject DEK is destroyed AGAIN after the re-erasure pass - 0 recoverable in backups"
    );
}

#[test]
fn ag_d10_erase_is_loud_on_an_incomplete_crypto_shred() {
    let subject = "psn:bob";
    let mut store = AgentFabricStore::new();
    store.write_free_text(1, "trace.trace_body", subject);
    let shredder = InMemoryShredder::new();
    let dek = subject_dek_ref("acme", subject);
    shredder.seal(&dek);
    shredder.make_unreachable(&dek);
    let holder = AgentFabricHolder::new(tenant(), store, shredder);

    let err = holder
        .erase(myelin_gdpr::EraseScope::Subject {
            subject: subject_ref(subject),
            tenant: tenant(),
        })
        .expect_err("an unreachable KMS makes the erase LOUD, never a silent green");
    assert!(
        err.0.contains("INCOMPLETE"),
        "the erase names itself INCOMPLETE: {}",
        err.0
    );
}
