//! # STOR-D1/D2 (Bus leg) — post-restore re-erasure: the key stays destroyed across a restore
//! (EB-16 / P-093)
//!
//! This is the Bus's leg of the STOR-D1/D2 restore-verify cross-seam the EB-16 GATE / TESTS field
//! names (the event-log offset is the cross-seam cursor; `post_restore_reerase`). It proves that a
//! restored backup does NOT resurrect an erased subject's inline-PII (external-insights/04 §1: the key
//! stays destroyed even after a backup is restored):
//!
//! 1. **erase + record** a subject in the live cell — crypto-shred the per-subject DEK + record it in
//!    the PII-free, non-shred-erasable erasure ledger ([`myelin_events::BusErasureLedger`], 10.8);
//! 2. **restore an OLDER backup** (one taken BEFORE the erase) — the DEK is resurrected (live again)
//!    and the log row is back WITHOUT its tombstone (exactly what a pre-erase restore does);
//! 3. **re_erase_after_restore**: the Bus's holder REPLAYS the ledger — re-runs the IDENTICAL
//!    crypto-shred over every ledger-listed subject (cold == live), re-destroying the resurrected DEK
//!    + re-emitting `*.erased` tombstones for the restored rows;
//! 4. **READ the re-erasure receipt** (the SCHED artifact) and assert the threshold: **0 resurrected
//!    inline-PII keys after a restore** — and bridge the Bus's survival signals into the harness's
//!    frozen §10.2 assertion library so the verdict is a loud, never-swallowed green (EI-01 §3): after
//!    the re-erase + re-tombstone + drain, `outbox_depth == 0` and `dead_letter_count == 0` (nothing
//!    was lost re-erasing across the restore).
//!
//! The DEVIATION (`myelin-events` cannot depend on the harness in production — the §2.9 DAG; and the
//! cross-seam restore TRIGGER is owned by Storage/GDPR, the floors P-ST-14/P-GA-06) is bridged HERE,
//! in the test build where the harness IS a dev-dependency, exactly as the BUS-D8 drill does it. This
//! drill proves the Bus MECHANISM the downstream restore-verify cross-seam wires.

use myelin_events::{Actor, AggregateKey, EmitContext, EventId};
use myelin_events::{
    ArtifactRef, BusErasureLedger, BusEventLog, BusHolder, BusObservations, BusSignals, DataRole,
    EventDraft, EventType, IdMinter, InMemoryShredder, InProcessBus, InlinePiiShredder,
    MonotonicMinter, OutboxStore, PiiKeyRef, Relay, Timestamp, Visibility,
};
use myelin_harness::{Predicate, SignalName, SignalSource};
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
    Timestamp("2026-06-19T00:00:04Z".into())
}
fn clock() -> Timestamp {
    Timestamp("2026-06-19T00:00:05Z".into())
}
fn actor_for(id: &str) -> Actor {
    Actor(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        tenant(),
    ))
}

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
        occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-19T00:00:00Z".into()),
        caused_by: None,
    };
    myelin_events::derive_envelope(draft, ctx, None)
}

fn minter() -> Arc<dyn IdMinter> {
    Arc::new(MonotonicMinter::new())
}

/// **STOR-D1/D2 (Bus leg): erase a subject → restore an older backup → re-erase → 0 resurrected
/// inline-PII keys post-restore + nothing lost.** The unit-of-proof the EB-16 GATE requires.
#[test]
fn bus_post_restore_re_erasure_zero_resurrected_keys_nothing_lost() {
    // (1) ERASE + RECORD u42 in the live cell. The PII-free ledger (10.8) durably remembers it.
    let mut live_log = BusEventLog::new();
    let shredder = InMemoryShredder::new();
    let ev = inline_pii("01J-1", "u42");
    if let Some(k) = &ev.pii_key_ref {
        shredder.seal(k);
    }
    live_log.append(ev);

    let holder = BusHolder::new(tenant(), region(), shredder.clone());
    let ledger = BusErasureLedger::new(tenant(), region());
    let mut live_outbox = OutboxStore::new();
    holder
        .erase_and_record(
            "u42",
            &mut live_log,
            &mut live_outbox,
            minter(),
            &ledger,
            now(),
        )
        .expect("live erase + ledger record");

    let key = PiiKeyRef("kms://acme/0/subject:u42".into());
    assert!(
        !shredder.is_live(&key),
        "precondition: the DEK is dead in the live cell"
    );
    assert!(
        ledger.is_erased("u42"),
        "the PII-free ledger remembers the erasure"
    );

    // (2) RESTORE an OLDER backup (taken BEFORE the erase): the DEK is resurrected (re-sealed) and the
    //     log row is back WITHOUT its tombstone — exactly what restoring a pre-erase backup does.
    let mut restored_log = BusEventLog::new();
    restored_log.append(inline_pii("01J-1", "u42"));
    shredder.seal(&key);
    assert!(
        shredder.is_live(&key),
        "the restore RESURRECTED u42's inline-PII DEK"
    );

    // (3) RE-ERASE AFTER RESTORE: replay the ledger — re-destroy the resurrected key + re-tombstone.
    let mut reerase_outbox = OutboxStore::new();
    let receipt = holder
        .re_erase_after_restore(
            &ledger,
            &mut restored_log,
            &mut reerase_outbox,
            minter(),
            now(),
        )
        .expect("re-erase after restore (KMS reachable)");

    // (4) READ the re-erasure receipt (the SCHED artifact) — the STOR-D1/D2 Bus-leg threshold.
    assert_eq!(
        receipt.resurrected, 0,
        "STOR-D1/D2: 0 resurrected inline-PII keys post-restore"
    );
    assert!(receipt.is_green(), "the Bus's restore-verify leg is GREEN");
    assert_eq!(
        receipt.keys_resurrected_by_restore, 1,
        "the restore brought the key back (honest)"
    );
    assert_eq!(receipt.re_erased_subjects, 1);
    assert!(
        receipt.tombstones_re_emitted >= 1,
        "re-tombstoned the restored row"
    );
    // The crypto-shred is REAL across the restore: the key no longer resolves.
    assert!(
        !shredder.is_live(&key),
        "the key STAYS destroyed across the restore"
    );
    // The restored row is tombstoned again (consumers degrade gracefully on it).
    assert!(
        restored_log.is_tombstoned("01J-1"),
        "the restored row carries a tombstone again"
    );

    // (5) RELAY → BROKER: the re-emitted tombstone publishes (the consumer-degrade signal survives
    //     the re-erasure too).
    let bus = InProcessBus::new();
    let relay = Relay::new(reerase_outbox.clone(), bus.clone(), clock);
    let drain = relay.drain_to_empty();
    assert!(
        drain.published >= 1,
        "the relay published the re-emitted *.erased tombstone"
    );

    // (6) BRIDGE into the harness §10.2 assertion library — a LOUD green (never swallowed): after the
    //     re-erase + tombstone emit + drain, nothing was lost (depth 0, no dead-letters).
    let obs = BusObservations::default();
    let sig = BusSignals::snapshot(&reerase_outbox, &drain, &obs, &now(), 0);
    let mut rec = myelin_events::MetricRecorder::new();
    sig.emit_to(&mut rec);

    let mut src = SignalSource::new();
    if let Some(v) = rec.scalar(myelin_events::BusSignal::OutboxDepth) {
        src.set_scalar(SignalName::OutboxDepth, v);
    }
    if let Some(v) = rec.scalar(myelin_events::BusSignal::DeadLetterCount) {
        src.set_scalar(SignalName::DeadLetterCount, v);
    }
    let depth_ok = src.assert_signal(SignalName::OutboxDepth, Predicate::Eq(0));
    let dlq_ok = src.assert_signal(SignalName::DeadLetterCount, Predicate::Eq(0));
    assert!(
        depth_ok.is_green(),
        "outbox drained after re-erasing across the restore: {depth_ok:?}"
    );
    assert!(
        dlq_ok.is_green(),
        "no tombstone dead-lettered re-erasing: {dlq_ok:?}"
    );
}

/// **STOR-D1/D2 (Bus leg) loud-failure:** a KMS failure during the post-restore re-erasure ABORTS the
/// pass loudly (never silently reports green) — the DSR retries. The re-erasure is part of the DSR,
/// never "assume re-erased".
#[test]
fn bus_post_restore_re_erasure_is_loud_on_kms_failure() {
    let mut live_log = BusEventLog::new();
    let shredder = InMemoryShredder::new();
    let ev = inline_pii("01J-1", "u42");
    if let Some(k) = &ev.pii_key_ref {
        shredder.seal(k);
    }
    live_log.append(ev);
    let holder = BusHolder::new(tenant(), region(), shredder.clone());
    let ledger = BusErasureLedger::new(tenant(), region());
    let mut live_outbox = OutboxStore::new();
    holder
        .erase_and_record(
            "u42",
            &mut live_log,
            &mut live_outbox,
            minter(),
            &ledger,
            now(),
        )
        .expect("live erase + record");

    // Restore resurrects the key, but the KMS is unreachable → the re-erase MUST be loud.
    let mut restored = BusEventLog::new();
    restored.append(inline_pii("01J-1", "u42"));
    let key = PiiKeyRef("kms://acme/0/subject:u42".into());
    shredder.seal(&key);
    shredder.make_unreachable(&key);

    let mut ro = OutboxStore::new();
    let err = holder
        .re_erase_after_restore(&ledger, &mut restored, &mut ro, minter(), now())
        .expect_err("re-erase is loud on a KMS failure (never assume re-erased)");
    assert!(matches!(err, myelin_events::ShredError::KmsUnavailable(_)));
}
