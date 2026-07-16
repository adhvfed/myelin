//! # BUS-D8 (live-store leg) — the Bus crypto-shred + tombstone erasure drill (EB-15 / P-092)
//!
//! This is the **BUS-D8 live-store-leg** drill the EB-15 GATE / TESTS field names (the erasure drill
//! greened in the live-store leg; the *reaches-backups* leg is the M5 follow-on EB-29). It proves the
//! Bus's instantiation of the ONE platform erasure posture (Bus §4.8 / X-7, by reference):
//!
//! 1. **seed** an in-cell event log with the subject's RARE inline-PII events (sealed under a
//!    per-subject DEK) + the common references-not-payloads events;
//! 2. **erase(subject)** through the holder mechanism ([`myelin_events::BusHolder`]) — crypto-shred
//!    the per-subject DEK through the [`myelin_events::InlinePiiShredder`] KMS seam + emit `*.erased`
//!    tombstones through the **outbox** (the only sanctioned emit path; BUS-2);
//! 3. **relay → broker → consumer**: the relay publishes the tombstone, a live consumer sees it and
//!    **degrades gracefully** ([`myelin_events::degrade_on_tombstone`] → `Done`, never blocks, never
//!    reads the now-unrecoverable payload);
//! 4. **READ the erase-receipt + tombstone-count** (the SCHED artifact) and assert the BUS-D8
//!    threshold: **0 recoverable inline-PII in the live log; tombstones present** — and bridge the
//!    Bus's survival signals into the harness's frozen §10.2 assertion library so the verdict is a
//!    loud, never-swallowed green (EI-01 §3): after the erase + tombstone emit + drain, `outbox_depth
//!    == 0` and `dead_letter_count == 0` (nothing was lost erasing the subject).
//!
//! The DEVIATION (`myelin-events` cannot depend on the harness in production — the §2.9 DAG) is
//! bridged HERE, in the test build where the harness IS a dev-dependency, exactly as the EB-11
//! self-test does it. The `impl gdpr::PersonalDataHolder` adapter + the live `KmsEngine` binding are
//! the named floor P-GA-06; this drill proves the MECHANISM that adapter wraps.

use myelin_events::{
    degrade_on_tombstone, ArtifactRef, BusEventLog, BusHolder, BusSignals, ConsumerName, DataRole,
    DedupLedger, EventDraft, EventType, IdMinter, InMemoryShredder, InProcessBus, MonotonicMinter,
    OutboxStore, PiiKeyRef, Relay, Subscription, Timestamp, Visibility, BUS_ERASED_TYPE,
};
use myelin_events::{Actor, AggregateKey, BusObservations, EmitContext, EventId};
use myelin_events::{BusTransport, InlinePiiShredder};
use myelin_events::{Consumer, HandleOutcome, Message, PrefetchBound, SubjectPattern};
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
fn clock() -> Timestamp {
    Timestamp("2026-06-19T00:00:02Z".into())
}

fn actor_for(id: &str) -> Actor {
    Actor(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        tenant(),
    ))
}

/// Build one retained envelope. `pii_subject = Some(s)` = an inline-PII event sealed under `s`'s
/// per-subject DEK; `None` = references-not-payloads (no inline PII).
fn retained(
    event_id: &str,
    type_: &str,
    actor_id: &str,
    pii_subject: Option<&str>,
) -> myelin_events::EventEnvelope {
    let (contains, key) = match pii_subject {
        Some(s) => (true, Some(PiiKeyRef(format!("kms://acme/0/subject:{s}")))),
        None => (false, None),
    };
    let draft = EventDraft {
        type_: EventType(type_.into()),
        subject: ArtifactRef(format!("myelin://acme/chat/message/{event_id}")),
        aggregate: AggregateKey(format!("chat.message:{event_id}")),
        payload: serde_json::json!({ "ref": format!("myelin://acme/chat/message/{event_id}") }),
        data_role: DataRole::Processor,
        visibility: Visibility::Internal,
        contains_personal_data: contains,
        pii_key_ref: key,
    };
    let ctx = EmitContext {
        event_id: EventId(event_id.into()),
        tenant: tenant(),
        region: region(),
        actor: actor_for(actor_id),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-19T00:00:00Z".into()),
        caused_by: None,
    };
    myelin_events::derive_envelope(draft, ctx, None)
}

/// A consumer that degrades gracefully on the `*.erased` tombstone (the BUS-D8 "consumers degrade
/// gracefully" leg). It records each tombstone it saw; it NEVER reads the shredded payload and NEVER
/// returns `Retry`/`NonRetryable` for a tombstone.
struct DegradingConsumer {
    seen_tombstones: std::sync::Mutex<usize>,
}

static SUBJECTS: &[SubjectPattern] = &[];

impl myelin_events::EventHandler for DegradingConsumer {
    fn subjects(&self) -> &'static [SubjectPattern] {
        SUBJECTS
    }
    fn handle(&self, ev: &myelin_events::EventEnvelope, _tx: &mut myelin_events::HandlerTx<'_>) -> HandleOutcome {
        if ev.type_.0 == BUS_ERASED_TYPE {
            *self.seen_tombstones.lock().unwrap() += 1;
            return degrade_on_tombstone(ev);
        }
        HandleOutcome::Done
    }
}

/// **BUS-D8 (live-store leg): erase a subject → 0 recoverable inline-PII live + tombstones present +
/// consumers degrade gracefully + nothing lost.** The unit-of-proof the EB-15 GATE requires.
#[test]
fn bus_d8_live_store_leg_crypto_shred_renders_inline_pii_unrecoverable_tombstones_present() {
    // (0) SEED the in-cell log: subject u42 has one rare inline-PII event; u99 one; plus two
    //     references-only events (the common case — erasing tombstones the identity, not the fact).
    let mut log = BusEventLog::new();
    let shredder = InMemoryShredder::new();
    let e1 = retained("01J-1", "chat.message.created", "u42", Some("u42"));
    let e2 = retained("01J-2", "chat.message.created", "u99", Some("u99"));
    let e3 = retained("01J-3", "issue.issue.created", "u42", None);
    let e4 = retained("01J-4", "git.pr.opened", "u7", None);
    for e in [&e1, &e2, &e3, &e4] {
        if let Some(k) = &e.pii_key_ref {
            shredder.seal(k);
        }
    }
    log.append(e1);
    log.append(e2);
    log.append(e3);
    log.append(e4);

    let key_u42 = PiiKeyRef("kms://acme/0/subject:u42".into());
    assert!(
        shredder.is_live(&key_u42),
        "precondition: u42's inline-PII DEK is live"
    );

    // (1) ERASE(u42): crypto-shred the per-subject DEK + emit *.erased tombstones into the outbox.
    let holder = BusHolder::new(tenant(), region(), shredder.clone());
    let mut outbox = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let receipt = holder
        .erase("u42", &mut log, &mut outbox, minter)
        .expect("erase succeeds (KMS reachable)");

    // (2) READ the erase-receipt (the SCHED artifact) — the BUS-D8 threshold.
    assert_eq!(
        receipt.recoverable_remaining, 0,
        "BUS-D8: 0 recoverable inline-PII in the live log"
    );
    assert_eq!(
        receipt.keys_shredded, 1,
        "the per-subject DEK was destroyed"
    );
    assert_eq!(receipt.tombstones_emitted, 1, "tombstones present");
    // The crypto-shred is REAL: u42's DEK no longer resolves (the live-log ciphertext is dead).
    assert!(
        !shredder.is_live(&key_u42),
        "u42's inline-PII DEK is crypto-shredded — unrecoverable"
    );
    // Per-subject granularity (GD-4): u99 is UNTOUCHED.
    assert!(
        shredder.is_live(&PiiKeyRef("kms://acme/0/subject:u99".into())),
        "u99 untouched"
    );
    // The erased event is tombstoned in the live log (the consumer-degrade signal).
    assert!(
        log.is_tombstoned("01J-1"),
        "the erased event carries a tombstone"
    );

    // (3) RELAY → BROKER → CONSUMER: the relay publishes the tombstone; a live consumer degrades.
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), clock);
    let drain = relay.drain_to_empty();
    assert!(
        drain.published >= 1,
        "the relay published the *.erased tombstone"
    );

    // The relay publishes each event under its `subject` ArtifactRef; the Bus's tombstones land
    // under `myelin://<tenant>/bus/event/<id>`, so the consumer binds + reads that prefix.
    const TOMBSTONE_PREFIX: &str = "myelin://acme/bus/event";
    let consumer = Consumer::new(
        DegradingConsumer {
            seen_tombstones: std::sync::Mutex::new(0),
        },
        Subscription::bind(
            ConsumerName("indexer".into()),
            &[TOMBSTONE_PREFIX],
            PrefetchBound::DEFAULT,
        )
        .expect("bind"),
        DedupLedger::new(),
    );
    for envelope in bus.consume(TOMBSTONE_PREFIX) {
        let subject = envelope.subject.0.clone();
        let _ = consumer.deliver(&Message { subject, envelope });
    }
    assert_eq!(
        *consumer.handler().seen_tombstones.lock().unwrap(),
        1,
        "the live consumer saw the tombstone and degraded gracefully (never blocked, never read the payload)"
    );

    // (4) BRIDGE into the harness §10.2 assertion library — a LOUD green (never swallowed): after
    //     the erase + tombstone emit + drain, nothing was lost (depth 0, no dead-letters).
    let obs = BusObservations::default();
    let now = Timestamp("2026-06-19T00:00:03Z".into());
    let sig = BusSignals::snapshot(&outbox, &drain, &obs, &now, 0);
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
        "outbox drained after erasing the subject: {depth_ok:?}"
    );
    assert!(
        dlq_ok.is_green(),
        "no tombstone dead-lettered erasing the subject: {dlq_ok:?}"
    );
}

/// **BUS-D8 loud-failure leg:** a crypto-shred KMS failure ABORTS the erase as INCOMPLETE (never
/// "assume erased"; the DSR retries). The DEK stays as-is, no tombstone is committed.
#[test]
fn bus_d8_crypto_shred_kms_failure_is_loud_never_assumes_erased() {
    let mut log = BusEventLog::new();
    let shredder = InMemoryShredder::new();
    let e1 = retained("01J-1", "chat.message.created", "u42", Some("u42"));
    if let Some(k) = &e1.pii_key_ref {
        shredder.seal(k);
    }
    log.append(e1);
    let key_u42 = PiiKeyRef("kms://acme/0/subject:u42".into());
    shredder.make_unreachable(&key_u42);

    let holder = BusHolder::new(tenant(), region(), shredder.clone());
    let mut outbox = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());

    let err = holder
        .erase("u42", &mut log, &mut outbox, minter)
        .expect_err("loud failure");
    assert!(matches!(err, myelin_events::ShredError::KmsUnavailable(_)));
    assert_eq!(
        outbox.committed_count(),
        0,
        "no tombstone committed on a failed erase"
    );
    assert!(
        !log.is_tombstoned("01J-1"),
        "not tombstoned on a failed erase"
    );
    assert!(
        shredder.is_live(&key_u42),
        "DEK untouched — the DSR retries (never assume erased)"
    );
}
