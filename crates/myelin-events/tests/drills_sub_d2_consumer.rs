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
    fn handle(&self, ev: &EventEnvelope, _tx: &mut myelin_events::HandlerTx<'_>) -> HandleOutcome {
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

fn pump(consumer: &Consumer<RecordingHandler>, bus: &InProcessBus) -> Vec<Delivered> {
    bus.consume(SUBJECT_PREFIX)
        .into_iter()
        .map(|envelope| {
            let subject = envelope.subject.0.clone();
            consumer.deliver(&Message { subject, envelope })
        })
        .collect()
}

#[test]
fn drill_sub_d2_drop_broker_mid_stream_zero_lost_zero_dup() {
    let tenant = TenantId("acme".into());
    let scope = Scope::Tenant(tenant.clone());
    let breaker = DependencyBreaker::new();

    let store = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let ids = commit_n(&store, minter, 6, "issue:PROJ-1");
    let committed: std::collections::HashSet<_> = ids.iter().cloned().collect();

    let bus = InProcessBus::new();
    let relay = Relay::new(store.clone(), bus.clone(), clock);

    let ledger = DedupLedger::new();

    relay.drain_to_empty();
    assert_eq!(
        bus.delivered_count(),
        6,
        "relay published all 6 (at-least-once available)"
    );

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

        breaker.break_dependency(Dependency::Broker, scope.clone());
        if breaker.is_broken(&Dependency::Broker, &scope) {
            bus.sever();
        }
    }

    breaker.restore_dependency(Dependency::Broker, scope.clone());
    if !breaker.is_broken(&Dependency::Broker, &scope) {
        bus.heal();
    }

    let c2 = Consumer::new(RecordingHandler::default(), sub(), ledger.clone());
    let outcomes = pump(&c2, &bus);

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

    assert_eq!(
        c2.handler().runs.load(Ordering::SeqCst),
        3,
        "no event processed twice (0 dup)"
    );

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

#[test]
fn drill_sub_d2_slow_subject_does_not_block_fast_subject() {
    struct LaneHandler;
    impl EventHandler for LaneHandler {
        fn subjects(&self) -> &'static [SubjectPattern] {
            SUBJECTS
        }
        fn handle(&self, ev: &EventEnvelope, _tx: &mut myelin_events::HandlerTx<'_>) -> HandleOutcome {
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

#[test]
fn cdc_2_4_2_5_consumer_reads_relayed_envelope_and_dedups() {
    let store = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let ids = commit_n(&store, minter, 2, "issue:PROJ-1");

    let bus = InProcessBus::new();
    Relay::new(store.clone(), bus.clone(), clock).drain_to_empty();

    let ledger = DedupLedger::new();
    let c = Consumer::new(RecordingHandler::default(), sub(), ledger.clone());

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
