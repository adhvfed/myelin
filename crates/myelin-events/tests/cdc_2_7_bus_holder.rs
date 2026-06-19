//! # CDC 2.7 — the Bus's crypto-shred / tombstone-on-the-log contract pair (EB-15 / P-092)
//!
//! Contract-index row **2.7** ("crypto-shred / tombstone on the log") is **OWNED** by the Bus. This
//! is its consumer-driven-contract (CDC) pair — the two sides of the §5.7 `PersonalDataHolder` seam
//! the Bus instantiates (Bus §4.8, the references-not-payloads + crypto-shred + tombstone triad):
//!
//! - **PROVIDER side** (the Bus): `erase(subject)` crypto-shreds the per-subject inline-PII DEK
//!   through the [`myelin_events::InlinePiiShredder`] KMS seam + emits `*.erased` tombstones through
//!   the outbox, and returns a receipt proving **0 recoverable** inline-PII in the live log.
//! - **CONSUMER side** (the GDPR DSR orchestrator role, contract 10.1): drives the holder via
//!   `locate` → `erase`, reads the receipt's `recoverable_remaining == 0` + `tombstones_emitted`,
//!   and relies on a tombstoned event exporting only the `[erased]` marker (never the unrecoverable
//!   payload). This is the SHAPE the downstream `impl gdpr::PersonalDataHolder` adapter (P-GA-06,
//!   the named floor) wraps — proven here against the mechanism it wraps.
//!
//! Both markers ("provider" / "consumer") are present so the contract-coverage scanner (P-S21)
//! reconciles row 2.7 `covered`. (Rows 10.1/11.3/11.4 are CONSUMED — the Bus calls them; their
//! OWNED CDC pairs live in `myelin-gdpr` / `myelin-storage`.)

use myelin_events::{
    ArtifactRef, BusEventLog, BusHolder, DataRole, EraseReceipt, EventDraft, EventType, IdMinter,
    InMemoryShredder, InlinePiiShredder, MonotonicMinter, OutboxStore, PiiKeyRef, Timestamp,
    Visibility,
};
use myelin_events::{Actor, AggregateKey, EmitContext, EventId};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}

fn inline_pii_event(event_id: &str, subject: &str) -> myelin_events::EventEnvelope {
    let key = PiiKeyRef(format!("kms://acme/0/subject:{subject}"));
    let draft = EventDraft {
        type_: EventType("chat.message.created".into()),
        subject: ArtifactRef(format!("myelin://acme/chat/message/{event_id}")),
        aggregate: AggregateKey(format!("chat.message:{event_id}")),
        payload: serde_json::json!({ "ref": format!("myelin://acme/chat/message/{event_id}") }),
        data_role: DataRole::Processor,
        visibility: Visibility::Internal,
        contains_personal_data: true,
        pii_key_ref: Some(key),
    };
    let ctx = EmitContext {
        event_id: EventId(event_id.into()),
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(PrincipalId(subject.into()), PrincipalKind::Human, tenant())),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-19T00:00:00Z".into()),
        caused_by: None,
    };
    myelin_events::derive_envelope(draft, ctx, None)
}

/// **PROVIDER side of 2.7** — the Bus's owned crypto-shred/tombstone promise: `erase(subject)`
/// destroys the per-subject DEK, emits a `*.erased` tombstone through the outbox, and the receipt
/// reports 0 recoverable inline-PII in the live log.
fn provider_erase(subject: &str) -> (EraseReceipt, InMemoryShredder, PiiKeyRef) {
    let mut log = BusEventLog::new();
    let shredder = InMemoryShredder::new();
    let ev = inline_pii_event("01J-1", subject);
    let key = ev.pii_key_ref.clone().expect("inline-PII event has a key");
    shredder.seal(&key);
    log.append(ev);

    let holder = BusHolder::new(tenant(), region(), shredder.clone());
    let mut outbox = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let receipt = holder.erase(subject, &mut log, &mut outbox, minter).expect("provider erase");
    assert_eq!(outbox.committed_count(), 1, "provider emitted the tombstone via the outbox");
    (receipt, shredder, key)
}

/// **The 2.7 CDC pair: provider erase ⇄ consumer (DSR orchestrator) reads the receipt.** The
/// CONSUMER (the GDPR DSR role, contract 10.1) drives the provider and asserts the contract: the
/// inline-PII DEK is destroyed (unrecoverable in the live log) and the tombstone is present.
#[test]
fn cdc_2_7_provider_crypto_shred_consumer_reads_zero_recoverable_receipt() {
    // PROVIDER: the Bus crypto-shreds + tombstones.
    let (receipt, shredder, key) = provider_erase("u42");

    // CONSUMER (the DSR orchestrator, 10.1): reads the receipt + verifies the crypto-shred is real.
    assert_eq!(receipt.subject, "u42");
    assert_eq!(receipt.recoverable_remaining, 0, "consumer requires 0 recoverable inline-PII");
    assert_eq!(receipt.keys_shredded, 1);
    assert!(receipt.tombstones_emitted >= 1, "consumer requires the tombstone present");
    // The crypto-shred is REAL: the consumer confirms the DEK no longer resolves.
    assert!(!shredder.is_live(&key), "consumer confirms the per-subject DEK is crypto-shredded");
}
