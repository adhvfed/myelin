//! # CDC 10.8 (Bus consumer-side) — the erasure-ledger post-restore re-erasure pair (EB-16 / P-093)
//!
//! Contract-index row **10.8** ("Erasure ledger — PII-free, non-shred-erasable; drives post-restore
//! re-erasure GD-14") is **OWNED** by GDPR/Audit. The Bus **CONSUMES** it: after Storage restores an
//! older backup (contract 11.5, the cross-seam restore), the Bus PARTICIPATES in the re-erasure
//! fan-out so the key STAYS destroyed across the restore. This is the Bus's consumer-side leg of that
//! seam — the two sides of the post-restore re-erasure contract:
//!
//! - **PROVIDER side** (the erasure-ledger + restore orchestration role — GDPR DSR 10.8 + Storage
//!   11.5 `post_restore_reerase`): records each erasure in the PII-free, non-shred-erasable ledger
//!   ([`myelin_events::BusErasureLedger`]); after a restore resurrects a pre-erase backup, drives the
//!   Bus's re-erasure pass over the ledger.
//! - **CONSUMER side** (the Bus's holder): replays the ledger via
//!   [`myelin_events::BusHolder::re_erase_after_restore`] — re-runs the IDENTICAL crypto-shred over
//!   every ledger-listed subject, re-destroying any DEK the restore resurrected + re-emitting
//!   `*.erased` tombstones — and returns a [`myelin_events::ReErasureReceipt`] proving **0
//!   resurrected** inline-PII keys post-restore. This is the SHAPE the downstream Storage
//!   `post_restore_reerase` cross-seam call (P-ST-14 / P-100) + the GDPR ledger adapter (P-GA-06 /
//!   P-106) wrap — proven here against the mechanism they wrap.
//!
//! Both markers ("provider" / "consumer") are present so the contract-coverage scanner (P-S21) sees
//! the Bus's consumer-side participation. Row 10.8 STAYS `deferred` in the manifest (landing P-115):
//! the full provider+consumer pair (GDPR mints + owns the GLOBAL ledger) lands at GA-15; this file is
//! the Bus's leg, not the GDPR provider side.

use myelin_events::{
    ArtifactRef, BusErasureLedger, BusEventLog, BusHolder, DataRole, EventDraft, EventType, IdMinter,
    InMemoryShredder, InlinePiiShredder, MonotonicMinter, OutboxStore, PiiKeyRef, ReErasureReceipt,
    Timestamp, Visibility,
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
fn now() -> Timestamp {
    Timestamp("2026-06-19T00:00:00Z".into())
}
fn actor_for(id: &str) -> Actor {
    Actor(Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant()))
}

/// A retained inline-PII envelope sealed under `subject`'s per-subject DEK.
fn inline_pii(event_id: &str, subject: &str) -> myelin_events::EventEnvelope {
    let draft = EventDraft {
        type_: EventType("chat.message.created".into()),
        subject: ArtifactRef(format!("myelin://acme/chat/message/{event_id}")),
        aggregate: AggregateKey(format!("chat.message:{event_id}")),
        payload: serde_json::json!({ "ref": format!("myelin://acme/chat/message/{event_id}") }),
        data_role: DataRole::Processor,
        visibility: Visibility::Internal,
        contains_personal_data: true,
        pii_key_ref: Some(PiiKeyRef(format!("kms://acme/0/subject:{subject}"))),
    };
    let ctx = EmitContext {
        event_id: EventId(event_id.into()),
        tenant: tenant(),
        region: region(),
        actor: actor_for(subject),
        schema_ver: 1,
        occurred_at: now(),
        recorded_at: now(),
        caused_by: None,
    };
    myelin_events::derive_envelope(draft, ctx, None)
}

fn minter() -> Arc<dyn IdMinter> {
    Arc::new(MonotonicMinter::new())
}

/// **The 10.8 CDC pair (Bus consumer-side): provider records the erasure + restores an older backup
/// ⇄ the Bus re-erases over the ledger and proves 0 resurrected.** The CONSUMER (the Bus holder)
/// drives `re_erase_after_restore`; the PROVIDER (the erasure-ledger + restore role) supplies the
/// ledger + the restored (resurrected) state.
#[test]
fn cdc_10_8_provider_restores_consumer_re_erases_zero_resurrected() {
    // PROVIDER: a live erase + a PII-free ledger record (10.8). The Bus is the holder; the DSR role
    // calls erase_and_record so the erasure is remembered across a restore.
    let mut live_log = BusEventLog::new();
    let shredder = InMemoryShredder::new();
    let ev = inline_pii("01J-1", "u42");
    if let Some(k) = &ev.pii_key_ref {
        shredder.seal(k);
    }
    live_log.append(ev);
    let holder = BusHolder::new(tenant(), region(), shredder.clone());
    let ledger = BusErasureLedger::new(tenant(), region());
    let mut outbox = OutboxStore::new();
    holder
        .erase_and_record("u42", &mut live_log, &mut outbox, minter(), &ledger, now())
        .expect("provider: live erase + ledger record");

    let key = PiiKeyRef("kms://acme/0/subject:u42".into());
    // PROVIDER: Storage restores an OLDER backup (11.5 cross-seam) — the DEK is resurrected, the log
    // row is back without its tombstone.
    let mut restored_log = BusEventLog::new();
    let ev2 = inline_pii("01J-1", "u42");
    restored_log.append(ev2);
    shredder.seal(&key); // the restore brought the key back

    // CONSUMER: the Bus re-erases over the ledger (post_restore_reerase). The consumer requires
    // 0 resurrected inline-PII keys post-restore (GD-14).
    let mut reerase_outbox = OutboxStore::new();
    let receipt: ReErasureReceipt = holder
        .re_erase_after_restore(&ledger, &mut restored_log, &mut reerase_outbox, minter(), now())
        .expect("consumer: re-erase after restore");

    // The consumer's contract: 0 resurrected; the key is dead again; the ledger drove it.
    assert!(receipt.is_green(), "consumer requires 0 resurrected inline-PII keys post-restore");
    assert_eq!(receipt.resurrected, 0);
    assert_eq!(receipt.keys_resurrected_by_restore, 1, "the restore resurrected one key");
    assert_eq!(receipt.re_erased_subjects, 1);
    assert!(!shredder.is_live(&key), "the key STAYS destroyed across the restore");
    // The ledger is non-shred-erasable: it still remembers u42 was erased after the whole cycle.
    assert!(ledger.is_erased("u42"));
}
