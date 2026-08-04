use myelin_events::{
    consume, Actor, AggregateKey, ArtifactRef, Backoff, BusTransport, CausedBy, Consumer,
    ConsumerName, ConsumerSpec, CorrelationId, DataRole, DedupLedger, Delivered, EmitContextBase,
    EventDraft, EventEnvelope, EventHandler, EventId, EventType, HandleOutcome, IdMinter,
    InProcessBus, Message, MonotonicMinter, OutboxStore, OutboxTx, PerTenantInflight,
    PrefetchBound, Relay, SubjectPattern, Timestamp, Visibility,
};
use myelin_harness::{DrillRegistry, DrillScenario, Label, Predicate, SignalName, SignalSource};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

const WHITELIST_PREFIX: &str = "myelin://acme/issues/";

fn principal() -> Principal {
    Principal::stub(
        PrincipalId("p".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

fn envelope(id: &str, subject: &str, tenant: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType("issues.issue.created".into()),
        schema_ver: 1,
        tenant: TenantId(tenant.into()),
        region: Region("eu-west".into()),
        actor: Actor(principal()),
        subject: ArtifactRef(subject.into()),
        aggregate: AggregateKey("issue:PROJ-1".into()),
        causation_id: None,
        correlation_id: CorrelationId(id.into()),
        caused_by: Some(CausedBy("session:abc".into())),
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

fn msg(id: &str, subject: &str) -> Message {
    Message {
        subject: subject.into(),
        envelope: envelope(id, subject, "acme"),
    }
}

struct CountingHandler {
    runs: AtomicU32,
}
static SUBJECTS: &[SubjectPattern] = &[];
impl EventHandler for CountingHandler {
    fn subjects(&self) -> &'static [SubjectPattern] {
        SUBJECTS
    }
    fn handle(&self, _ev: &EventEnvelope, _tx: &mut myelin_events::HandlerTx<'_>) -> HandleOutcome {
        self.runs.fetch_add(1, Ordering::SeqCst);
        HandleOutcome::Done
    }
}

struct NaiveStarConsumer {
    pending: std::cell::RefCell<u64>,
    processed: std::cell::RefCell<u64>,
}
impl NaiveStarConsumer {
    fn new() -> Self {
        NaiveStarConsumer {
            pending: std::cell::RefCell::new(0),
            processed: std::cell::RefCell::new(0),
        }
    }
    fn deliver(&self, subject: &str) {
        if subject.starts_with(WHITELIST_PREFIX) {
            *self.processed.borrow_mut() += 1;
        } else {
            *self.pending.borrow_mut() += 1;
        }
    }
    fn lag(&self) -> u64 {
        *self.pending.borrow()
    }
    fn processed(&self) -> u64 {
        *self.processed.borrow()
    }
}

fn template_consumer() -> Consumer<CountingHandler> {
    consume(
        ConsumerSpec::new(ConsumerName("indexer".into()), &[WHITELIST_PREFIX]),
        CountingHandler {
            runs: AtomicU32::new(0),
        },
        DedupLedger::new(),
    )
    .expect("a concrete whitelist is admitted")
}

#[test]
fn drill_bus_d2_whitelist_consumer_does_not_stall_naive_star_does() {
    let template = template_consumer();
    let naive = NaiveStarConsumer::new();

    const JUNK: u64 = 1000;
    let mut flood: Vec<Message> = Vec::new();
    for i in 0..5 {
        flood.push(msg(
            &format!("01J-real-{i}"),
            &format!("{WHITELIST_PREFIX}issue/PROJ-{i}"),
        ));
    }
    let junk_subjects = [
        "myelin://acme/chat/m",
        "myelin://acme/ci/job",
        "myelin://acme/refs/edge",
    ];
    for i in 0..JUNK {
        let s = junk_subjects[(i as usize) % junk_subjects.len()];
        flood.push(msg(&format!("01J-junk-{i}"), &format!("{s}/{i}")));
    }

    for m in &flood {
        naive.deliver(&m.subject);
        let out = template.deliver(m);
        if m.subject.starts_with(WHITELIST_PREFIX) {
            assert_eq!(out, Delivered::Acked, "whitelisted real events are handled");
        } else {
            assert!(
                matches!(out, Delivered::DeadLettered(_)),
                "off-whitelist junk is rejected, not queued"
            );
        }
    }

    assert_eq!(
        template.handler().runs.load(Ordering::SeqCst),
        5,
        "the template handled every real event"
    );
    assert_eq!(
        template.lag(),
        0,
        "the template consumer's lag is bounded (0) - no head-of-line stall"
    );
    assert_eq!(
        template.dead_letters().len() as u64,
        JUNK,
        "the junk was rejected at the boundary, surfaced"
    );

    assert_eq!(
        naive.processed(),
        5,
        "the naive consumer processed only the recognised handful"
    );
    assert_eq!(
        naive.lag(),
        JUNK,
        "the naive `*` consumer's lag grew to the whole junk flood (HoL stall)"
    );

    const LAG_ALARM_BOUND: i64 = 100;
    let mut signals = SignalSource::new();
    signals.set_labelled(
        SignalName::ConsumerLag,
        vec![Label::new("consumer", "naive-star")],
        naive.lag() as i64,
    );
    signals.set_labelled(
        SignalName::ConsumerLag,
        vec![Label::new("consumer", "indexer")],
        template.lag() as i64,
    );

    signals
        .assert_labelled(
            SignalName::ConsumerLag,
            vec![Label::new("consumer", "naive-star")],
            Predicate::Gt(LAG_ALARM_BOUND),
        )
        .expect_green();
    signals
        .assert_labelled(
            SignalName::ConsumerLag,
            vec![Label::new("consumer", "indexer")],
            Predicate::Lte(LAG_ALARM_BOUND),
        )
        .expect_green();

    println!(
        "[2026-06-19] PASS  drill=BUS-D2  template-lag=0 (no HoL stall)  naive-star-lag={JUNK} (unbounded, alarm fires>{LAG_ALARM_BOUND})"
    );
}

#[test]
fn drill_bus_d2_star_subscription_is_unconstructable_through_consume() {
    for bad in ["*", ">", "issues.*", "issues.>", ""] {
        let spec = ConsumerSpec::new(ConsumerName("indexer".into()), &[bad]);
        let r = consume(
            spec,
            CountingHandler {
                runs: AtomicU32::new(0),
            },
            DedupLedger::new(),
        );
        assert!(
            r.is_err(),
            "the over-broad subject `{bad}` is rejected - a `*` consumer cannot be built"
        );
    }
}

#[test]
fn drill_eb05_per_tenant_surge_is_bounded_other_tenant_not_starved() {
    struct SurgeHandler;
    impl EventHandler for SurgeHandler {
        fn subjects(&self) -> &'static [SubjectPattern] {
            SUBJECTS
        }
        fn handle(&self, ev: &EventEnvelope, _tx: &mut myelin_events::HandlerTx<'_>) -> HandleOutcome {
            if ev.tenant.0 == "surge" {
                HandleOutcome::Retry(Backoff { seconds: 5 })
            } else {
                HandleOutcome::Done
            }
        }
    }
    const CAP: u32 = 4;
    let spec = ConsumerSpec {
        durable: ConsumerName("indexer".into()),
        subjects: vec!["myelin://".into()],
        max_ack_pending: PrefetchBound::DEFAULT,
        per_tenant_inflight: PerTenantInflight::new(CAP).unwrap(),
    };
    let c = consume(spec, SurgeHandler, DedupLedger::new()).unwrap();

    let mut throttled = 0u32;
    for i in 0..20 {
        let m = Message {
            subject: "myelin://surge/issues/x".into(),
            envelope: envelope(
                &format!("01J-surge-{i}"),
                "myelin://surge/issues/x",
                "surge",
            ),
        };
        match c.deliver(&m) {
            Delivered::Retried(_) => {}
            Delivered::Throttled(t) => {
                assert_eq!(t, TenantId("surge".into()));
                throttled += 1;
            }
            other => panic!("unexpected surge outcome: {other:?}"),
        }
    }
    assert_eq!(
        c.tenant_inflight(&TenantId("surge".into())),
        CAP,
        "the surge tenant is bounded to its cap"
    );
    assert_eq!(
        throttled,
        20 - CAP,
        "every surge event over the cap was throttled (deferred, not dropped)"
    );

    for i in 0..10 {
        let m = Message {
            subject: "myelin://other/issues/y".into(),
            envelope: envelope(
                &format!("01J-other-{i}"),
                "myelin://other/issues/y",
                "other",
            ),
        };
        assert_eq!(
            c.deliver(&m),
            Delivered::Acked,
            "the other tenant flows under the surge"
        );
    }
    assert_eq!(
        c.tenant_inflight(&TenantId("other".into())),
        0,
        "the other tenant drained (not starved)"
    );

    let mut signals = SignalSource::new();
    signals.set_labelled(
        SignalName::ConsumerLag,
        vec![
            Label::new("consumer", "indexer"),
            Label::new("tenant", "surge"),
        ],
        c.tenant_inflight(&TenantId("surge".into())) as i64,
    );
    signals.set_labelled(
        SignalName::ConsumerLag,
        vec![
            Label::new("consumer", "indexer"),
            Label::new("tenant", "other"),
        ],
        c.tenant_inflight(&TenantId("other".into())) as i64,
    );
    signals
        .assert_labelled(
            SignalName::ConsumerLag,
            vec![
                Label::new("consumer", "indexer"),
                Label::new("tenant", "surge"),
            ],
            Predicate::Lte(CAP as i64),
        )
        .expect_green();
    signals
        .assert_labelled(
            SignalName::ConsumerLag,
            vec![
                Label::new("consumer", "indexer"),
                Label::new("tenant", "other"),
            ],
            Predicate::Eq(0),
        )
        .expect_green();

    println!(
        "[2026-06-19] PASS  drill=EB-05-fairness  surge-inflight={CAP} (bounded to cap)  other-inflight=0 (not starved)"
    );
}

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: TenantId("acme".into()),
        region: Region("eu-west".into()),
        actor: Actor(principal()),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-19T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:abc".into())),
    }
}

fn clock() -> Timestamp {
    Timestamp("2026-06-19T00:00:02Z".into())
}

#[test]
fn cdc_2_4_provider_relay_to_consumer_template_pair() {
    let store = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let mut tx = store.begin(minter, ctx_base());
    tx.stage_state_change("issues created");
    let mut ids = Vec::new();
    for i in 0..3 {
        let draft = EventDraft {
            type_: EventType(format!("issues.issue.e{i}")),
            subject: ArtifactRef(format!("{WHITELIST_PREFIX}issue/PROJ-{i}")),
            aggregate: AggregateKey("issue:PROJ-1".into()),
            payload: serde_json::json!({ "ref": i }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        };
        ids.push(tx.emit(draft, None).unwrap());
    }
    tx.commit().unwrap();

    let bus = InProcessBus::new();
    Relay::new(store.clone(), bus.clone(), clock).drain_to_empty();
    assert_eq!(
        bus.delivered_count(),
        3,
        "the provider relayed all 3 onto the broker"
    );

    let c = consume(
        ConsumerSpec::new(ConsumerName("indexer".into()), &[WHITELIST_PREFIX]),
        CountingHandler {
            runs: AtomicU32::new(0),
        },
        DedupLedger::new(),
    )
    .unwrap();

    let outcomes: Vec<Delivered> = bus
        .consume(WHITELIST_PREFIX)
        .into_iter()
        .map(|envelope| {
            let subject = envelope.subject.0.clone();
            c.deliver(&Message { subject, envelope })
        })
        .collect();

    assert_eq!(
        outcomes.len(),
        3,
        "the consumer saw exactly the 3 relayed events (provider↔consumer pair)"
    );
    assert!(
        outcomes.iter().all(|o| *o == Delivered::Acked),
        "each relayed event processed once"
    );
    assert_eq!(
        c.handler().runs.load(Ordering::SeqCst),
        3,
        "the handler ran exactly 3 times"
    );
    assert_eq!(
        c.dedup().len(),
        3,
        "3 (consumer, event_id) pairs recorded (2.5, the dedup half)"
    );

    let again: Vec<Delivered> = bus
        .consume(WHITELIST_PREFIX)
        .into_iter()
        .map(|envelope| {
            let subject = envelope.subject.0.clone();
            c.deliver(&Message { subject, envelope })
        })
        .collect();
    assert!(
        again.iter().all(|o| *o == Delivered::Deduplicated),
        "redelivery is deduped under 2.4"
    );
    assert_eq!(
        c.handler().runs.load(Ordering::SeqCst),
        3,
        "the handler still ran only 3 times (0 dup)"
    );
    let _ = ids;
}

#[test]
fn bus_d2_and_fairness_register_into_the_permanent_drill_suite() {
    let mut registry = DrillRegistry::new();

    registry.register_drill(DrillScenario::new("BUS-D2-whitelist-no-stall", |ctx| {
        let template = template_consumer();
        let naive = NaiveStarConsumer::new();
        const JUNK: u64 = 500;
        for i in 0..3 {
            let m = msg(
                &format!("01J-real-{i}"),
                &format!("{WHITELIST_PREFIX}issue/PROJ-{i}"),
            );
            naive.deliver(&m.subject);
            template.deliver(&m);
        }
        for i in 0..JUNK {
            let s = format!("myelin://acme/chat/m/{i}");
            naive.deliver(&s);
            template.deliver(&msg(&format!("01J-junk-{i}"), &s));
        }
        assert_eq!(template.lag(), 0, "template no-stall");
        assert_eq!(naive.lag(), JUNK, "naive `*` stalled");
        ctx.signals.set_labelled(
            SignalName::ConsumerLag,
            vec![Label::new("consumer", "naive-star")],
            naive.lag() as i64,
        );
        ctx.signals.assert_labelled(
            SignalName::ConsumerLag,
            vec![Label::new("consumer", "naive-star")],
            Predicate::Gt(100),
        )
    }));

    registry.register_drill(DrillScenario::new("EB-05-per-tenant-fairness", |ctx| {
        struct SurgeHandler;
        impl EventHandler for SurgeHandler {
            fn subjects(&self) -> &'static [SubjectPattern] {
                SUBJECTS
            }
            fn handle(&self, ev: &EventEnvelope, _tx: &mut myelin_events::HandlerTx<'_>) -> HandleOutcome {
                if ev.tenant.0 == "surge" {
                    HandleOutcome::Retry(Backoff { seconds: 5 })
                } else {
                    HandleOutcome::Done
                }
            }
        }
        const CAP: u32 = 3;
        let c = consume(
            ConsumerSpec {
                durable: ConsumerName("indexer".into()),
                subjects: vec!["myelin://".into()],
                max_ack_pending: PrefetchBound::DEFAULT,
                per_tenant_inflight: PerTenantInflight::new(CAP).unwrap(),
            },
            SurgeHandler,
            DedupLedger::new(),
        )
        .unwrap();
        for i in 0..10 {
            let m = Message {
                subject: "myelin://surge/x".into(),
                envelope: envelope(&format!("01J-s-{i}"), "myelin://surge/x", "surge"),
            };
            c.deliver(&m);
        }
        let other = Message {
            subject: "myelin://other/y".into(),
            envelope: envelope("01J-o", "myelin://other/y", "other"),
        };
        assert_eq!(
            c.deliver(&other),
            Delivered::Acked,
            "other tenant not starved"
        );
        assert_eq!(
            c.tenant_inflight(&TenantId("surge".into())),
            CAP,
            "surge bounded to cap"
        );
        ctx.signals.set_labelled(
            SignalName::ConsumerLag,
            vec![
                Label::new("consumer", "indexer"),
                Label::new("tenant", "surge"),
            ],
            c.tenant_inflight(&TenantId("surge".into())) as i64,
        );
        ctx.signals.assert_labelled(
            SignalName::ConsumerLag,
            vec![
                Label::new("consumer", "indexer"),
                Label::new("tenant", "surge"),
            ],
            Predicate::Lte(CAP as i64),
        )
    }));

    let results = registry.run_all();
    assert!(
        results.iter().all(|r| r.is_pass()),
        "BUS-D2 + fairness must read green: {results:?}"
    );
    assert!(
        registry.all_green(),
        "they re-run forever (a regression re-reds them)"
    );
    for r in &results {
        println!("{}", r.artifact_row("2026-06-19"));
    }
}
