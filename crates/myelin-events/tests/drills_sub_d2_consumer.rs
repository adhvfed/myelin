//! # SUB-D2 drill + the SUB-D1-through-a-consumer re-confirm (P-S08 → P-009)
//!
//! The consumer half of the **silent-data-loss floor**. These are the failure-injection-harness
//! drills the P-S08 TESTS field requires — they CHAIN the whole emit path end-to-end (EI-01 §4:
//! real sessions chain; that is where bugs live):
//!
//!   emit (P-S06) → outbox co-commit (P-S07) → relay publish (P-S07) → broker → **consumer
//!   runtime + dedup ledger (P-S08)**.
//!
//! and inject the **P-S03 scoped-reversible dependency-break injector** (`Dependency::Broker`) to
//! DROP the broker mid-stream, then reconnect (re-bind by name) and re-consume — asserting **0
//! lost + 0 dup** over the sequence and that the **P-S04** `consumer_lag` signal recovers, with
//! no head-of-line stall.
//!
//! Thresholds are **0 lost / 0 dup** and **lag → 0**. A red drill is information: it is NOT
//! weakened — it becomes a dated "claimed, not proven" thresholds-file row (P-S22). **This is a
//! PERMANENT gate (re-run on every emit-path change).**

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, BusTransport, CausedBy, Consumer, ConsumerName, DataRole,
    DedupLedger, Delivered, EmitContextBase, EventDraft, EventEnvelope, EventHandler, EventType,
    HandleOutcome, IdMinter, InProcessBus, Message, MonotonicMinter, OutboxStore, OutboxTx,
    PrefetchBound, Relay, SubjectPattern, Subscription, Timestamp, Visibility,
};
use myelin_harness::{Dependency, DependencyBreaker, Predicate, Scope, SignalName, SignalSource};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

const SUBJECT_PREFIX: &str = "myelin://acme/issues/";

fn clock() -> Timestamp {
    Timestamp("2026-06-19T00:00:02Z".into())
}

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: TenantId("acme".into()),
        region: Region("eu-west".into()),
        actor: Actor(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-19T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:abc".into())),
    }
}

fn draft(i: usize, aggregate: &str) -> EventDraft {
    EventDraft {
        type_: EventType(format!("issues.issue.e{i}")),
        subject: ArtifactRef(format!("{SUBJECT_PREFIX}issue/{aggregate}")),
        aggregate: AggregateKey(aggregate.into()),
        payload: serde_json::json!({ "ref": aggregate }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

/// Commit `n` events for one aggregate (the "state-commit" half), returning their ids.
fn commit_n(
    store: &OutboxStore,
    minter: Arc<dyn IdMinter>,
    n: usize,
    aggregate: &str,
) -> Vec<myelin_events::EventId> {
    let mut tx = store.begin(minter, ctx_base());
    tx.stage_state_change("issue created");
    let mut ids = Vec::new();
    for i in 0..n {
        ids.push(tx.emit(draft(i, aggregate), None).unwrap());
    }
    tx.commit().unwrap();
    ids
}

/// A handler that records the DISTINCT event_ids it processed and counts total runs (so a
/// duplicate process — a bug — is observable as `runs > distinct`).
#[derive(Default)]
struct RecordingHandler {
    runs: AtomicU32,
    processed: std::sync::Mutex<Vec<String>>,
}
static SUBJECTS: &[SubjectPattern] = &[];
impl EventHandler for RecordingHandler {
    fn subjects(&self) -> &'static [SubjectPattern] {
        SUBJECTS
    }
    fn handle(&self, ev: &EventEnvelope) -> HandleOutcome {
        self.runs.fetch_add(1, Ordering::SeqCst);
        self.processed.lock().unwrap().push(ev.event_id.0.clone());
        HandleOutcome::Done
    }
}

fn sub() -> Subscription {
    Subscription::bind(
        ConsumerName("indexer".into()),
        &[SUBJECT_PREFIX],
        PrefetchBound::DEFAULT,
    )
    .unwrap()
}

/// Pull every envelope the broker has delivered (the consumer's read of the durable stream) and
/// hand each to the runtime — modeling the broker pushing the bound consumer its messages.
fn pump(consumer: &Consumer<RecordingHandler>, bus: &InProcessBus) -> Vec<Delivered> {
    bus.consume(SUBJECT_PREFIX)
        .into_iter()
        .map(|envelope| {
            let subject = envelope.subject.0.clone();
            consumer.deliver(&Message { subject, envelope })
        })
        .collect()
}

/// **SUB-D2 — drop broker mid-stream → 0 lost across reconnect (bind-by-name + dedup); a slow
/// subject does not head-of-line-block others.** And **SUB-D1 re-confirmed through a consumer:**
/// the dedup ledger absorbs the relay's at-least-once redelivery → 0 dup.
///
/// Chains emit → relay → broker → consumer, drops the broker mid-stream via the P-S03 injector,
/// reconnects, and asserts 0 lost + 0 dup + `consumer_lag → 0`.
#[test]
fn drill_sub_d2_drop_broker_mid_stream_zero_lost_zero_dup() {
    let tenant = TenantId("acme".into());
    let scope = Scope::Tenant(tenant.clone());
    let breaker = DependencyBreaker::new();

    let store = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    // STATE-COMMIT: 6 events committed durably (the producer side).
    let ids = commit_n(&store, minter, 6, "issue:PROJ-1");
    let committed: std::collections::HashSet<_> = ids.iter().cloned().collect();

    let bus = InProcessBus::new();
    let relay = Relay::new(store.clone(), bus.clone(), clock);

    // The durable dedup ledger — survives the reconnect (rule 4: bind-by-name re-uses it).
    let ledger = DedupLedger::new();

    // === Connection 1: relay publishes, consumer processes SOME, then the broker DROPS. ===
    relay.drain_to_empty(); // every committed event is now on the broker.
    assert_eq!(
        bus.delivered_count(),
        6,
        "relay published all 6 (at-least-once available)"
    );

    // The first consumer processes the first 3 of the 6 delivered, then the broker drops.
    let processed_before;
    {
        let c1 = Consumer::new(RecordingHandler::default(), sub(), ledger.clone());
        let delivered = bus.consume(SUBJECT_PREFIX);
        for envelope in delivered.into_iter().take(3) {
            let subject = envelope.subject.0.clone();
            assert_eq!(c1.deliver(&Message { subject, envelope }), Delivered::Acked);
        }
        processed_before = c1.dedup().len();
        assert_eq!(
            processed_before, 3,
            "consumer 1 durably handled 3 before the drop"
        );

        // (1) INJECT: drop the broker mid-stream (the SUB-D2 fault). The remaining 3 were never
        //     delivered to a consumer.
        breaker.break_dependency(Dependency::Broker, scope.clone());
        if breaker.is_broken(&Dependency::Broker, &scope) {
            bus.sever();
        }
        // c1 is dropped here — the connection is gone.
    }

    // === Reconnect (Connection 2): SAME consumer name + SAME ledger (rule 4). ===
    breaker.restore_dependency(Dependency::Broker, scope.clone());
    if !breaker.is_broken(&Dependency::Broker, &scope) {
        bus.heal();
    }

    let c2 = Consumer::new(RecordingHandler::default(), sub(), ledger.clone());
    // The broker REDELIVERS the whole stream (at-least-once) — all 6, incl. the 3 already handled.
    let outcomes = pump(&c2, &bus);

    // 0 DUP: the 3 already-handled events are DEDUPLICATED (the ledger absorbed the redelivery);
    // the 3 not-yet-handled are ACKED (0 lost).
    let deduped = outcomes
        .iter()
        .filter(|o| **o == Delivered::Deduplicated)
        .count();
    let acked = outcomes.iter().filter(|o| **o == Delivered::Acked).count();
    assert_eq!(
        deduped, 3,
        "the 3 already-handled events were deduped (0 dup)"
    );
    assert_eq!(
        acked, 3,
        "the 3 surviving events were handled after reconnect (0 lost)"
    );

    // The handler on c2 ran EXACTLY 3 times (only the not-yet-handled events) — no double-process.
    assert_eq!(
        c2.handler().runs.load(Ordering::SeqCst),
        3,
        "no event processed twice (0 dup)"
    );

    // 0 LOST: across the WHOLE sequence, every committed event was handled exactly once. The
    // ledger now holds exactly the committed set.
    assert_eq!(
        ledger.len(),
        6,
        "all 6 committed events are durably handled (0 lost)"
    );
    let handled: std::collections::HashSet<_> = ids
        .iter()
        .filter(|id| ledger.is_handled(c2.name(), id))
        .cloned()
        .collect();
    assert_eq!(
        handled, committed,
        "the handled set == the committed set (0 lost, 0 dup)"
    );

    // consumer_lag recovers to 0 (the P-S04 survival signal; no HoL stall — all subjects drained).
    let mut signals = SignalSource::new();
    signals.set_scalar(SignalName::ConsumerLag, c2.lag() as i64);
    signals
        .assert_signal(SignalName::ConsumerLag, Predicate::Eq(0))
        .expect_green();

    assert_eq!(breaker.broken_count(), 0, "no leaked break (teardown)");
    println!(
        "[2026-06-19] PASS  drill=SUB-D2  lost=0 dup=0 consumer_lag→0  (emit→relay→drop broker→reconnect→re-consume)"
    );
}

/// **A slow subject does NOT head-of-line-block a fast one (the SUB-D2 no-HoL leg).** One
/// consumer subscribed to TWO subjects; the slow subject's messages RETRY (stay pending, lag
/// rises on that subject) while the fast subject's messages ACK and clear — the fast subject is
/// not stalled behind the slow one.
#[test]
fn drill_sub_d2_slow_subject_does_not_block_fast_subject() {
    // The handler RETRIES anything on the "slow" subject, and is Done on the "fast" one.
    struct LaneHandler;
    impl EventHandler for LaneHandler {
        fn subjects(&self) -> &'static [SubjectPattern] {
            SUBJECTS
        }
        fn handle(&self, ev: &EventEnvelope) -> HandleOutcome {
            if ev.subject.0.contains("/slow/") {
                HandleOutcome::Retry(myelin_events::Backoff { seconds: 30 })
            } else {
                HandleOutcome::Done
            }
        }
    }
    let s = Subscription::bind(
        ConsumerName("indexer".into()),
        &["myelin://acme/slow/", "myelin://acme/fast/"],
        PrefetchBound::DEFAULT,
    )
    .unwrap();
    let c = Consumer::new(LaneHandler, s, DedupLedger::new());

    let slow = Message {
        subject: "myelin://acme/slow/x".into(),
        envelope: envelope("01J-slow", "myelin://acme/slow/x"),
    };
    let fast = Message {
        subject: "myelin://acme/fast/y".into(),
        envelope: envelope("01J-fast", "myelin://acme/fast/y"),
    };

    // The slow subject retries (stays pending, lag on it rises); the fast subject ACKs regardless.
    assert_eq!(c.deliver(&slow), Delivered::Retried(30));
    assert_eq!(
        c.deliver(&fast),
        Delivered::Acked,
        "fast subject not blocked by the slow one"
    );
    assert_eq!(
        c.lag_on("myelin://acme/slow/x"),
        1,
        "the slow subject carries the lag"
    );
    assert_eq!(
        c.lag_on("myelin://acme/fast/y"),
        0,
        "the fast subject drained (no HoL stall)"
    );

    // The P-S04 signal reads the slow subject's lag is non-zero while the fast lane is clear — a
    // drill asserts the fast lane held (its lag == 0).
    let mut signals = SignalSource::new();
    signals.set_labelled(
        SignalName::ConsumerLag,
        vec![myelin_harness::Label::new("consumer", "indexer:fast")],
        c.lag_on("myelin://acme/fast/y") as i64,
    );
    signals
        .assert_labelled(
            SignalName::ConsumerLag,
            vec![myelin_harness::Label::new("consumer", "indexer:fast")],
            Predicate::Eq(0),
        )
        .expect_green();
    println!(
        "[2026-06-19] PASS  drill=SUB-D2  no-HoL-stall: fast lane lag==0 while slow lane retries"
    );
}

fn envelope(id: &str, subject: &str) -> EventEnvelope {
    use myelin_events::{CorrelationId, EventId};
    EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType("issues.issue.created".into()),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: Region("eu-west".into()),
        actor: Actor(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )),
        subject: ArtifactRef(subject.into()),
        aggregate: AggregateKey("a:1".into()),
        causation_id: None,
        correlation_id: CorrelationId(id.into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-19T00:00:01Z".into()),
        payload: serde_json::json!({ "ref": "x" }),
    }
}

/// The CDC pair for 2.4/2.5: the consumer (the runtime) reads the SAME wire envelope the relay
/// (the provider) published, dedups on the `(consumer, event_id)` PK, and the handler sees the
/// frozen envelope shape. Provider = relay/broker; consumer = [`Consumer`]. This is the consumer
/// half the P-S05/P-S06 provider CDC named as landing in P-S08.
#[test]
fn cdc_2_4_2_5_consumer_reads_relayed_envelope_and_dedups() {
    let store = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let ids = commit_n(&store, minter, 2, "issue:PROJ-1");

    let bus = InProcessBus::new();
    Relay::new(store.clone(), bus.clone(), clock).drain_to_empty();

    let ledger = DedupLedger::new();
    let c = Consumer::new(RecordingHandler::default(), sub(), ledger.clone());

    // The consumer reads exactly the relayed envelopes (provider→consumer 2.4 pair) and processes
    // each once.
    let first = pump(&c, &bus);
    assert!(
        first.iter().all(|o| *o == Delivered::Acked),
        "every relayed event processed once"
    );
    assert_eq!(
        c.handler().processed.lock().unwrap().clone(),
        ids.iter().map(|i| i.0.clone()).collect::<Vec<_>>(),
        "the consumer saw the provider's wire event_ids in (aggregate, seq) order"
    );

    // 2.5: a redelivery of the same stream is fully deduped (the `(consumer, event_id)` PK).
    let again = pump(&c, &bus);
    assert!(
        again.iter().all(|o| *o == Delivered::Deduplicated),
        "redelivery is deduped (2.5)"
    );
    assert_eq!(
        c.handler().runs.load(Ordering::SeqCst),
        2,
        "the handler still ran only twice"
    );
}

/// SUB-D2 registers into the P-S04 every-incident-adds-a-drill registry so it re-runs forever
/// (EI-01 §3/§5) — a regression on the consumer path re-reds it loudly.
#[test]
fn sub_d2_registers_into_the_permanent_drill_suite() {
    use myelin_harness::{DrillRegistry, DrillScenario};

    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new("SUB-D2-consumer-dedup", |ctx| {
        let tenant = TenantId("acme".into());
        let scope = Scope::Tenant(tenant.clone());

        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let ids = commit_n(&store, minter, 4, "issue:PROJ-1");

        let bus = InProcessBus::new();
        let relay = Relay::new(store.clone(), bus.clone(), clock);
        let ledger = DedupLedger::new();

        relay.drain_to_empty();
        // process 2, then drop the broker.
        {
            let c1 = Consumer::new(RecordingHandler::default(), sub(), ledger.clone());
            for envelope in bus.consume(SUBJECT_PREFIX).into_iter().take(2) {
                let subject = envelope.subject.0.clone();
                c1.deliver(&Message { subject, envelope });
            }
            ctx.breaker
                .break_dependency(Dependency::Broker, scope.clone());
            if ctx.breaker.is_broken(&Dependency::Broker, &scope) {
                bus.sever();
            }
        }
        // reconnect: same name + ledger; redeliver everything → 0 lost, 0 dup.
        ctx.breaker.restore_dependency(Dependency::Broker, scope);
        bus.heal();
        let c2 = Consumer::new(RecordingHandler::default(), sub(), ledger.clone());
        let outcomes = pump(&c2, &bus);

        let acked = outcomes.iter().filter(|o| **o == Delivered::Acked).count();
        let deduped = outcomes
            .iter()
            .filter(|o| **o == Delivered::Deduplicated)
            .count();
        assert_eq!(acked, 2, "the 2 survivors handled after reconnect (0 lost)");
        assert_eq!(deduped, 2, "the 2 already-handled deduped (0 dup)");
        assert_eq!(ledger.len(), 4, "all 4 committed handled exactly once");
        assert_eq!(
            c2.handler().runs.load(Ordering::SeqCst),
            2,
            "no double-process"
        );
        let _ = ids;

        // the asserted survival signal: consumer_lag recovered to 0.
        ctx.signals
            .set_scalar(SignalName::ConsumerLag, c2.lag() as i64);
        ctx.signals
            .assert_signal(SignalName::ConsumerLag, Predicate::Eq(0))
    }));

    let results = registry.run_all();
    assert!(
        results[0].is_pass(),
        "SUB-D2 drill must read green: {:?}",
        results[0]
    );
    assert!(
        registry.all_green(),
        "re-runs forever (a regression re-reds it)"
    );
    println!("{}", results[0].artifact_row("2026-06-19"));
}
