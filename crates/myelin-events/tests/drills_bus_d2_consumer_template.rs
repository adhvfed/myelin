//! # BUS-D2 drill + the EB-05 per-tenant-fairness drill + the contract-2.4 CDC pair (EB-05 / P-043)
//!
//! EB-05 ships the **idempotent-consumer template** — the one `EventHandler` runtime every
//! consumer in the platform is built from (contract 2.4; Bus §4.2; abstract-at-the-first-copy,
//! EI-01 §7). The runtime + the seven encoded rules landed in P-S08; EB-05 EXTENDS that base with
//! the two pieces the EB-05 DELIVERABLE names and that the base did not yet have:
//!
//!   1. the single sanctioned entry-point `consume(ConsumerSpec{ durable, subjects /* whitelist,
//!      NEVER `*` */, max_ack_pending, per_tenant_inflight }, handler)`, and
//!   2. the **per-tenant in-flight fairness cap** (rule 6, the agent-surge guard).
//!
//! This file is the EB-05 GATE/DRILLS proof:
//!
//! - **BUS-D2** — flood a (wrongly) `*`-subscribed consumer with unhandled types: the
//!   whitelist-template consumer does NOT stall while the naive `*` one accumulates an unbounded
//!   backlog; the consumer-lag alarm fires; reads `num_pending` (`ConsumerLag`). In our model the
//!   `*` subscription is **unconstructable** ([`Subscription::bind`] / [`consume`] REJECT it
//!   loudly), so the drill proves the *defence itself*: the naive over-broad consumer is the one
//!   modelled by hand (a raw queue with no whitelist) and head-of-line-stalls; the template
//!   consumer, given the SAME flood, drains only what it whitelisted and its lag stays bounded.
//! - **EB-05 per-tenant fairness** — one tenant's surge is bounded to its `per_tenant_inflight`
//!   cap (throttled, deferred, never dropped) while a second tenant keeps flowing; reads
//!   `bus.per_tenant_inflight` via the labelled `ConsumerLag` survival signal.
//! - **CDC 2.4** — the provider (relay/broker) + consumer (the [`Consumer`] runtime) pair: the
//!   consumer reads exactly the wire envelopes the relay published, at the frozen 2.4 shape.
//!
//! Thresholds (EB-05 GATE): the whitelist consumer's lag stays bounded (== its real backlog, 0
//! here) while the naive consumer's lag is unbounded (>= the flood size); the lag alarm fires; one
//! tenant's in-flight never exceeds its cap and the other tenant is not starved (0 lost). A red
//! drill is information — it is NOT weakened (EI-01 §3).

use myelin_events::{
    consume, Actor, AggregateKey, ArtifactRef, Backoff, BusTransport, CausedBy, Consumer,
    ConsumerName, ConsumerSpec, CorrelationId, DataRole, DedupLedger, Delivered, EmitContextBase,
    EventDraft, EventEnvelope, EventHandler, EventId, EventType, HandleOutcome, IdMinter,
    InProcessBus, Message, MonotonicMinter, OutboxStore, OutboxTx, PerTenantInflight, PrefetchBound,
    Relay, SubjectPattern, Timestamp, Visibility,
};
use myelin_harness::{
    DrillRegistry, DrillScenario, Label, Predicate, SignalName, SignalSource,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

const WHITELIST_PREFIX: &str = "myelin://acme/issues/";

fn principal() -> Principal {
    Principal::stub(PrincipalId("p".into()), PrincipalKind::Human, TenantId("acme".into()))
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
    Message { subject: subject.into(), envelope: envelope(id, subject, "acme") }
}

/// The whitelist-template consumer: it only handles `issues.*` (one whitelisted prefix). A handler
/// that COUNTS its runs so a stall (it never runs) or an over-process (it ran on junk) is visible.
struct CountingHandler {
    runs: AtomicU32,
}
static SUBJECTS: &[SubjectPattern] = &[];
impl EventHandler for CountingHandler {
    fn subjects(&self) -> &'static [SubjectPattern] {
        SUBJECTS
    }
    fn handle(&self, _ev: &EventEnvelope) -> HandleOutcome {
        self.runs.fetch_add(1, Ordering::SeqCst);
        HandleOutcome::Done
    }
}

/// The **naive over-broad consumer** the template defends against (EI-03 §6.1): a raw queue with
/// NO subject whitelist that must accept EVERY event routed to it, most of which it cannot handle
/// — they pile up unhandled (head-of-line stall). This is what the platform must NEVER ship; it is
/// modelled by hand here purely so BUS-D2 can show the template consumer does NOT stall the way it
/// does, given the identical flood.
struct NaiveStarConsumer {
    /// Unhandled messages accumulating behind the types it doesn't recognise — its unbounded lag.
    pending: std::cell::RefCell<u64>,
    /// What it actually managed to process (only the handful of recognised types).
    processed: std::cell::RefCell<u64>,
}
impl NaiveStarConsumer {
    fn new() -> Self {
        NaiveStarConsumer { pending: std::cell::RefCell::new(0), processed: std::cell::RefCell::new(0) }
    }
    /// Deliver to the naive consumer. It "recognises" only `issues.*` subjects; everything else it
    /// cannot handle, so it sits in the backlog forever (head-of-line block) — its `num_pending`
    /// grows without bound.
    fn deliver(&self, subject: &str) {
        if subject.starts_with(WHITELIST_PREFIX) {
            *self.processed.borrow_mut() += 1;
        } else {
            // unhandled type: it cannot term it (no whitelist discipline) and it cannot ack it
            // (it never ran a handler), so it head-of-line-blocks — the backlog only grows.
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
        CountingHandler { runs: AtomicU32::new(0) },
        DedupLedger::new(),
    )
    .expect("a concrete whitelist is admitted")
}

/// **BUS-D2 — the whitelist-template consumer does NOT head-of-line-stall on a flood of unhandled
/// types; the naive `*` consumer does; the lag alarm fires.**
///
/// Flood BOTH consumers with the same mixed stream: a handful of whitelisted `issues.*` events the
/// indexer handles, plus a huge wave of `chat.*`/`ci.*`/etc. events it never whitelisted. The
/// template consumer drains only its whitelisted subjects (lag bounded to its real backlog — 0
/// here, all whitelisted events acked); the off-whitelist flood is rejected at the boundary
/// (dead-lettered, never piled up behind the real work). The naive consumer's backlog grows to the
/// whole flood — an unbounded lag the alarm fires on.
#[test]
fn drill_bus_d2_whitelist_consumer_does_not_stall_naive_star_does() {
    let template = template_consumer();
    let naive = NaiveStarConsumer::new();

    // The flood: 5 whitelisted real events + 1000 unhandled junk events of types the indexer never
    // subscribed to (the "tens of millions of unprocessed messages" case, scaled down for CI).
    const JUNK: u64 = 1000;
    let mut flood: Vec<Message> = Vec::new();
    for i in 0..5 {
        flood.push(msg(&format!("01J-real-{i}"), &format!("{WHITELIST_PREFIX}issue/PROJ-{i}")));
    }
    let junk_subjects = ["myelin://acme/chat/m", "myelin://acme/ci/job", "myelin://acme/refs/edge"];
    for i in 0..JUNK {
        let s = junk_subjects[(i as usize) % junk_subjects.len()];
        flood.push(msg(&format!("01J-junk-{i}"), &format!("{s}/{i}")));
    }

    // Both consumers see the SAME flood.
    for m in &flood {
        // The naive `*` consumer accepts everything routed to it — the junk piles up.
        naive.deliver(&m.subject);
        // The template consumer: whitelisted events are handled+acked; off-whitelist junk is
        // rejected at the boundary (dead-lettered), never queued behind the real work.
        let out = template.deliver(m);
        if m.subject.starts_with(WHITELIST_PREFIX) {
            assert_eq!(out, Delivered::Acked, "whitelisted real events are handled");
        } else {
            assert!(matches!(out, Delivered::DeadLettered(_)), "off-whitelist junk is rejected, not queued");
        }
    }

    // === The template consumer did NOT stall: it handled all 5 real events; its lag is BOUNDED. ===
    assert_eq!(template.handler().runs.load(Ordering::SeqCst), 5, "the template handled every real event");
    assert_eq!(template.lag(), 0, "the template consumer's lag is bounded (0) — no head-of-line stall");
    assert_eq!(template.dead_letters().len() as u64, JUNK, "the junk was rejected at the boundary, surfaced");

    // === The naive `*` consumer STALLED: it processed only the 5 it recognised; the rest is an
    //     UNBOUNDED backlog its lag grows without bound on. ===
    assert_eq!(naive.processed(), 5, "the naive consumer processed only the recognised handful");
    assert_eq!(naive.lag(), JUNK, "the naive `*` consumer's lag grew to the whole junk flood (HoL stall)");

    // === The lag alarm FIRES on the naive consumer (num_pending crossed the bound); it does NOT on
    //     the template consumer. The §4.11 `ConsumerLag` survival signal, labelled per consumer. ===
    const LAG_ALARM_BOUND: i64 = 100;
    let mut signals = SignalSource::new();
    signals.set_labelled(SignalName::ConsumerLag, vec![Label::new("consumer", "naive-star")], naive.lag() as i64);
    signals.set_labelled(SignalName::ConsumerLag, vec![Label::new("consumer", "indexer")], template.lag() as i64);

    // the alarm fires on the naive consumer: its lag EXCEEDS the bound.
    signals
        .assert_labelled(
            SignalName::ConsumerLag,
            vec![Label::new("consumer", "naive-star")],
            Predicate::Gt(LAG_ALARM_BOUND),
        )
        .expect_green();
    // the alarm does NOT fire on the template consumer: its lag is within the bound.
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

/// **A `*` (or `>` / empty) subscription is UNCONSTRUCTABLE — the structural half of BUS-D2.** The
/// platform cannot even build the naive over-broad consumer through the sanctioned path: `consume`
/// REJECTS a wildcard subject loudly at registration. The defence is not a runtime check that can
/// be forgotten; it is a type-level impossibility.
#[test]
fn drill_bus_d2_star_subscription_is_unconstructable_through_consume() {
    for bad in ["*", ">", "issues.*", "issues.>", ""] {
        let spec = ConsumerSpec::new(ConsumerName("indexer".into()), &[bad]);
        let r = consume(spec, CountingHandler { runs: AtomicU32::new(0) }, DedupLedger::new());
        assert!(r.is_err(), "the over-broad subject `{bad}` is rejected — a `*` consumer cannot be built");
    }
}

// === EB-05 per-tenant fairness drill (rule 6, the agent-surge guard) ===

/// **One tenant's surge is bounded to its per-tenant in-flight cap; a second tenant is NOT
/// starved.** With a cap of 4, a surging tenant whose work keeps retrying fills its 4 in-flight
/// slots and is throttled (deferred, never dropped); every event from a DIFFERENT tenant still
/// acks. Reads the `bus.per_tenant_inflight` survival signal (via the labelled `ConsumerLag`
/// signal — the consumer-scoped fairness projection).
#[test]
fn drill_eb05_per_tenant_surge_is_bounded_other_tenant_not_starved() {
    // surge tenant retries forever (work stays outstanding); everyone else is Done.
    struct SurgeHandler;
    impl EventHandler for SurgeHandler {
        fn subjects(&self) -> &'static [SubjectPattern] {
            SUBJECTS
        }
        fn handle(&self, ev: &EventEnvelope) -> HandleOutcome {
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

    // The surge tenant floods 20 distinct events. Only CAP can be in-flight at once; the rest are
    // THROTTLED (deferred), never dropped.
    let mut throttled = 0u32;
    for i in 0..20 {
        let m = Message {
            subject: "myelin://surge/issues/x".into(),
            envelope: envelope(&format!("01J-surge-{i}"), "myelin://surge/issues/x", "surge"),
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
    assert_eq!(c.tenant_inflight(&TenantId("surge".into())), CAP, "the surge tenant is bounded to its cap");
    assert_eq!(throttled, 20 - CAP, "every surge event over the cap was throttled (deferred, not dropped)");

    // A DIFFERENT tenant's events ALL ack — the surge did NOT starve it (fairness held, 0 lost).
    for i in 0..10 {
        let m = Message {
            subject: "myelin://other/issues/y".into(),
            envelope: envelope(&format!("01J-other-{i}"), "myelin://other/issues/y", "other"),
        };
        assert_eq!(c.deliver(&m), Delivered::Acked, "the other tenant flows under the surge");
    }
    assert_eq!(c.tenant_inflight(&TenantId("other".into())), 0, "the other tenant drained (not starved)");

    // The `bus.per_tenant_inflight` survival signal: the surge tenant's in-flight is bounded to the
    // cap; the other tenant's is 0 (it never queued behind the surge).
    let mut signals = SignalSource::new();
    signals.set_labelled(
        SignalName::ConsumerLag,
        vec![Label::new("consumer", "indexer"), Label::new("tenant", "surge")],
        c.tenant_inflight(&TenantId("surge".into())) as i64,
    );
    signals.set_labelled(
        SignalName::ConsumerLag,
        vec![Label::new("consumer", "indexer"), Label::new("tenant", "other")],
        c.tenant_inflight(&TenantId("other".into())) as i64,
    );
    signals
        .assert_labelled(
            SignalName::ConsumerLag,
            vec![Label::new("consumer", "indexer"), Label::new("tenant", "surge")],
            Predicate::Lte(CAP as i64),
        )
        .expect_green();
    signals
        .assert_labelled(
            SignalName::ConsumerLag,
            vec![Label::new("consumer", "indexer"), Label::new("tenant", "other")],
            Predicate::Eq(0),
        )
        .expect_green();

    println!(
        "[2026-06-19] PASS  drill=EB-05-fairness  surge-inflight={CAP} (bounded to cap)  other-inflight=0 (not starved)"
    );
}

// === CDC 2.4: the provider (relay/broker) + consumer (the template runtime) pair ===

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

/// **CDC 2.4 — provider (relay) → consumer (the [`Consumer`] template) pair.** The provider commits
/// events through the outbox + relay onto the broker; the consumer, stood up through the EB-05
/// `consume(ConsumerSpec, …)` entry-point, reads exactly those wire envelopes at the frozen 2.4
/// shape (`subjects()` whitelist, `handle → {Done|…}`, dedup ledger, lag metric) and processes each
/// once. The consumer half of the 2.4 contract the provider's emit-path CDC pairs with.
#[test]
fn cdc_2_4_provider_relay_to_consumer_template_pair() {
    // === Provider: commit 3 events through the sanctioned emit path, relay them onto the broker. ===
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
    assert_eq!(bus.delivered_count(), 3, "the provider relayed all 3 onto the broker");

    // === Consumer: stood up through the EB-05 entry-point, reads exactly the relayed envelopes. ===
    let c = consume(
        ConsumerSpec::new(ConsumerName("indexer".into()), &[WHITELIST_PREFIX]),
        CountingHandler { runs: AtomicU32::new(0) },
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

    assert_eq!(outcomes.len(), 3, "the consumer saw exactly the 3 relayed events (provider↔consumer pair)");
    assert!(outcomes.iter().all(|o| *o == Delivered::Acked), "each relayed event processed once");
    assert_eq!(c.handler().runs.load(Ordering::SeqCst), 3, "the handler ran exactly 3 times");
    assert_eq!(c.dedup().len(), 3, "3 (consumer, event_id) pairs recorded (2.5, the dedup half)");

    // A redelivery of the same stream is fully deduped (the 2.5 effectively-once anchor under 2.4).
    let again: Vec<Delivered> = bus
        .consume(WHITELIST_PREFIX)
        .into_iter()
        .map(|envelope| {
            let subject = envelope.subject.0.clone();
            c.deliver(&Message { subject, envelope })
        })
        .collect();
    assert!(again.iter().all(|o| *o == Delivered::Deduplicated), "redelivery is deduped under 2.4");
    assert_eq!(c.handler().runs.load(Ordering::SeqCst), 3, "the handler still ran only 3 times (0 dup)");
    let _ = ids;
}

/// BUS-D2 + the EB-05 fairness drill register into the permanent every-incident-adds-a-drill suite
/// (EI-01 §3/§5) so they re-run forever — a regression on the consumer template re-reds them.
#[test]
fn bus_d2_and_fairness_register_into_the_permanent_drill_suite() {
    let mut registry = DrillRegistry::new();

    // BUS-D2: the whitelist template's lag stays bounded under a junk flood; the naive `*` does not.
    registry.register_drill(DrillScenario::new("BUS-D2-whitelist-no-stall", |ctx| {
        let template = template_consumer();
        let naive = NaiveStarConsumer::new();
        const JUNK: u64 = 500;
        for i in 0..3 {
            let m = msg(&format!("01J-real-{i}"), &format!("{WHITELIST_PREFIX}issue/PROJ-{i}"));
            naive.deliver(&m.subject);
            template.deliver(&m);
        }
        for i in 0..JUNK {
            let s = format!("myelin://acme/chat/m/{i}");
            naive.deliver(&s);
            template.deliver(&msg(&format!("01J-junk-{i}"), &s));
        }
        // the template's lag is bounded (0); the naive consumer's is the whole flood.
        assert_eq!(template.lag(), 0, "template no-stall");
        assert_eq!(naive.lag(), JUNK, "naive `*` stalled");
        // the alarm fires on the naive consumer's unbounded lag.
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

    // EB-05 fairness: a surge tenant is bounded to its cap; the signal reads in-flight <= cap.
    registry.register_drill(DrillScenario::new("EB-05-per-tenant-fairness", |ctx| {
        struct SurgeHandler;
        impl EventHandler for SurgeHandler {
            fn subjects(&self) -> &'static [SubjectPattern] {
                SUBJECTS
            }
            fn handle(&self, ev: &EventEnvelope) -> HandleOutcome {
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
        // the other tenant still flows.
        let other = Message {
            subject: "myelin://other/y".into(),
            envelope: envelope("01J-o", "myelin://other/y", "other"),
        };
        assert_eq!(c.deliver(&other), Delivered::Acked, "other tenant not starved");
        assert_eq!(c.tenant_inflight(&TenantId("surge".into())), CAP, "surge bounded to cap");
        ctx.signals.set_labelled(
            SignalName::ConsumerLag,
            vec![Label::new("consumer", "indexer"), Label::new("tenant", "surge")],
            c.tenant_inflight(&TenantId("surge".into())) as i64,
        );
        ctx.signals.assert_labelled(
            SignalName::ConsumerLag,
            vec![Label::new("consumer", "indexer"), Label::new("tenant", "surge")],
            Predicate::Lte(CAP as i64),
        )
    }));

    let results = registry.run_all();
    assert!(results.iter().all(|r| r.is_pass()), "BUS-D2 + fairness must read green: {results:?}");
    assert!(registry.all_green(), "they re-run forever (a regression re-reds them)");
    for r in &results {
        println!("{}", r.artifact_row("2026-06-19"));
    }
}
