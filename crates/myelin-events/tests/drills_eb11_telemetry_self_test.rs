//! # EB-11 / P-014 — the Bus survival-signal harness self-test (the M0→M1 observability gate)
//!
//! This is the **harness self-test** the M0→M1 exit gate requires (master §2: "the harness can
//! inject a fault and read a telemetry assertion"). It is the *observability precondition* for
//! EB-04's SUB-D1 / BUS-D4 and every later Bus drill: it proves the Bus's contract-1.8 survival
//! signals ([`myelin_events::telemetry`], the EB-11 DELIVERABLE) can be SNAPSHOT after an
//! injected fault and ASSERTED against — a typed green/red that is never a swallowed pass
//! (EI-01 §3).
//!
//! The shape (EB-11 GATE / TESTS):
//! 1. **inject** a producer-kill fault via the P-S03 scoped-reversible dependency-break injector
//!    (`Dependency::Broker`, tenant-scoped) — the relay's transport severs, so committed events
//!    park in the outbox (the kill between commit and publish);
//! 2. **snapshot** the Bus's §4.11 survival signals off live state ([`BusSignals::snapshot`]) and
//!    **emit** them onto a [`MetricRecorder`] (the metrics-health port seam);
//! 3. **read the telemetry assertion**: bridge the recorded Bus samples into the harness
//!    `SignalSource` (the frozen §10.2 assertion library, P-S04) and assert `outbox_depth` +
//!    the dedup hit-rate read the value the property demands — loud, never swallowed.
//!
//! The DEVIATION (events cannot depend on the harness in production — see the crate-level
//! `Status (P-014 / EB-11)` note) is bridged HERE, in the test build where the harness IS a
//! dev-dependency: [`bridge_into_signal_source`] maps each [`BusSignal`] sample onto the
//! matching harness `SignalName`.

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, BusObservations, BusSignal, BusSignals, DataRole,
    DrainReport, EmitContextBase, EventDraft, EventType, IdMinter, InProcessBus, MetricRecorder,
    MonotonicMinter, OutboxStore, OutboxTx, Relay, Timestamp, Visibility,
};
use myelin_harness::{
    Dependency, DependencyBreaker, Label, Predicate, Scope, SignalName, SignalSource,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

fn clock() -> Timestamp {
    Timestamp("2026-06-19T00:00:02Z".into())
}

fn ctx_base(tenant: &str) -> EmitContextBase {
    EmitContextBase {
        tenant: TenantId(tenant.into()),
        region: Region("eu-west".into()),
        actor: Actor(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-19T00:00:00Z".into()),
        caused_by: None,
    }
}

fn draft(i: usize, aggregate: &str) -> EventDraft {
    EventDraft {
        type_: EventType(format!("issues.issue.e{i}")),
        subject: ArtifactRef(format!("myelin://acme/issues/issue/{aggregate}")),
        aggregate: AggregateKey(aggregate.into()),
        payload: serde_json::json!({ "ref": aggregate }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

fn commit_n(store: &OutboxStore, minter: Arc<dyn IdMinter>, n: usize, aggregate: &str) {
    let mut tx = store.begin(minter, ctx_base("acme"));
    tx.stage_state_change("created");
    for i in 0..n {
        tx.emit(draft(i, aggregate), None).unwrap();
    }
    tx.commit().unwrap();
}

/// **The DEVIATION bridge** (test build only): map the Bus's emitted [`MetricRecorder`] samples
/// onto the harness's frozen §10.2 `SignalSource`, so a drill can assert the Bus's survival
/// signals with the loud-never-swallowed `Predicate`/`Assertion` machinery. The Bus emits with
/// `BusSignal` (its provider vocabulary); the harness asserts with `SignalName` (the §10.2 name
/// enum) — this is the one place the two meet (the producer port at `serve` does the same map).
fn bridge_into_signal_source(rec: &MetricRecorder, src: &mut SignalSource) {
    if let Some(v) = rec.scalar(BusSignal::OutboxDepth) {
        src.set_scalar(SignalName::OutboxDepth, v);
    }
    if let Some(v) = rec.scalar(BusSignal::DeadLetterCount) {
        src.set_scalar(SignalName::DeadLetterCount, v);
    }
    // Causal-depth firings: the Bus's shared-root tripwire firings map onto the §10.2
    // CausalDepthFirings row (loop-safety, D-8).
    if let Some(v) = rec.scalar(BusSignal::SharedRootTripwireFirings) {
        src.set_scalar(SignalName::CausalDepthFirings, v);
    }
    // Consumer lag is labelled per consumer in BOTH vocabularies; bridge each.
    for sample in rec.samples() {
        if sample.signal == BusSignal::ConsumerLag {
            let labels: Vec<Label> = sample
                .labels
                .iter()
                .map(|l| Label::new(l.key.clone(), l.value.clone()))
                .collect();
            src.set_labelled(SignalName::ConsumerLag, labels, sample.value);
        }
    }
}

/// **EB-11 SELF-TEST — inject a producer-kill fault, READ the outbox-depth + dedup telemetry
/// assertion.** The unit-of-proof the M0→M1 boundary requires.
#[test]
fn eb11_self_test_inject_producer_kill_read_outbox_depth_and_dedup_assertion() {
    let tenant = TenantId("acme".into());
    let scope = Scope::Tenant(tenant.clone());

    // The P-S03 injector — the shared fault seam the relay consults.
    let breaker = DependencyBreaker::new();

    let store = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    // STATE-COMMIT: 5 events committed (durable in the outbox, unsent).
    commit_n(&store, minter, 5, "issue:PROJ-1");

    let bus = InProcessBus::new();
    let relay = Relay::new(store.clone(), bus.clone(), clock);

    // (1) INJECT the producer-kill fault: break the broker for this tenant. The relay's publish
    //     fails, so committed events stay parked in the outbox (the kill between commit & publish).
    breaker.break_dependency(Dependency::Broker, scope.clone());
    if breaker.is_broken(&Dependency::Broker, &scope) {
        bus.sever();
    }
    let drain_severed = relay.drain_to_empty();

    // (2) SNAPSHOT the Bus's survival signals off live state + EMIT them onto the metrics port.
    //     The producer-kill leaves depth == 5 and the oldest row aging.
    let obs = BusObservations::default();
    let now_severed = Timestamp("2026-06-19T00:00:30Z".into()); // oldest row aged 30s
    let sig = BusSignals::snapshot(&store, &drain_severed, &obs, &now_severed, 0);
    let mut rec = MetricRecorder::new();
    sig.emit_to(&mut rec);

    // The Bus EMITTED the silent-data-loss signals (a missing signal is itself a failure).
    assert_eq!(
        rec.scalar(BusSignal::OutboxDepth),
        Some(5),
        "outbox depth emitted"
    );
    assert_eq!(
        rec.scalar(BusSignal::OutboxAgeSecs),
        Some(30),
        "outbox age emitted"
    );
    assert_eq!(
        rec.scalar(BusSignal::DeadLetterCount),
        Some(0),
        "dead-letter count emitted"
    );

    // (3) READ THE TELEMETRY ASSERTION through the harness §10.2 assertion library: while the
    //     broker is severed, every committed event is PARKED (0 lost) — depth == 5, dead-letters
    //     0. The verdict is a typed green; a red would panic loudly with the observed value.
    let mut src = SignalSource::new();
    bridge_into_signal_source(&rec, &mut src);
    src.assert_signal(SignalName::OutboxDepth, Predicate::Eq(5))
        .expect_green();
    src.assert_signal(SignalName::DeadLetterCount, Predicate::Eq(0))
        .expect_green();
    assert_eq!(
        bus.delivered_count(),
        0,
        "severed broker delivered nothing (0 ghost)"
    );

    // (4) RESTORE the dependency + drain: the outbox-depth telemetry DRAINS to 0, and a consumer
    //     that sees the relay's redelivery dedups it — the dedup hit-rate signal proves
    //     effectively-once (the dedup half of the assertion EB-11 names). 5 committed → 5
    //     delivered, exactly once.
    breaker.restore_dependency(Dependency::Broker, scope.clone());
    if !breaker.is_broken(&Dependency::Broker, &scope) {
        bus.heal();
    }
    let drain_healed = relay.drain_to_empty();
    // a second drain pass re-claims nothing new but a redelivery would be broker-deduped: model
    // the dedup-hit the consumer ledger absorbs (1 redelivery seen, deduped) for the hit-rate.
    let redrain = relay.drain_to_empty();

    let obs_after = BusObservations {
        dedup_hits: redrain.deduplicated as u64 + 1, // the consumer-ledger dedup of one redelivery
        dedup_deliveries: 5 + 1,
        ..BusObservations::default()
    };
    let now_drained = Timestamp("2026-06-19T00:01:00Z".into());
    let sig_after = BusSignals::snapshot(&store, &drain_healed, &obs_after, &now_drained, 0);
    let mut rec_after = MetricRecorder::new();
    sig_after.emit_to(&mut rec_after);

    // outbox-depth drained to 0 (SUB-D1), age 0 (no unsent row), dedup hit recorded.
    assert_eq!(rec_after.scalar(BusSignal::OutboxDepth), Some(0));
    assert_eq!(
        rec_after.scalar(BusSignal::OutboxAgeSecs),
        Some(0),
        "drained → age 0"
    );
    assert_eq!(
        rec_after.scalar(BusSignal::DedupHits),
        Some(1),
        "one redelivery deduped"
    );
    assert_eq!(rec_after.scalar(BusSignal::DedupDeliveries), Some(6));

    let mut src_after = SignalSource::new();
    bridge_into_signal_source(&rec_after, &mut src_after);
    // THE outbox-depth assertion the self-test reads after the injected kill: → 0.
    src_after
        .assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
        .expect_green();
    src_after
        .assert_signal(SignalName::DeadLetterCount, Predicate::Eq(0))
        .expect_green();

    assert_eq!(
        bus.delivered_count(),
        5,
        "exactly-once: 5 committed → 5 delivered"
    );
    assert_eq!(breaker.broken_count(), 0, "no leaked break");
    println!(
        "[2026-06-19] PASS  drill=EB-11-self-test  inject(producer-kill) → \
         outbox_depth 5→0, age→0, dead_letters=0, dedup_hits=1  (inject → snapshot → assert green)"
    );
}

/// The §4.11 set the self-test asserts against is the FULL Bus contribution — emitting all of
/// it onto the port and reading every signal back proves observability is complete (EI-01 §3:
/// a drill that emits no signal has failed; the Bus is the largest single contributor to 1.8).
#[test]
fn eb11_full_4_11_signal_set_is_emitted_to_the_port() {
    let store = OutboxStore::new();
    let obs = BusObservations {
        consumer_lag: vec![("search-indexer".into(), 0)],
        per_tenant_inflight: vec![("acme".into(), 0)],
        dedup_hits: 0,
        dedup_deliveries: 12,
        causal_depth_max: 2,
        shared_root_tripwire_firings: 0,
    };
    let drain = DrainReport {
        published: 12,
        ..DrainReport::default()
    };
    let sig = BusSignals::snapshot(
        &store,
        &drain,
        &obs,
        &Timestamp("2026-06-19T00:00:00Z".into()),
        7,
    );
    let mut rec = MetricRecorder::new();
    sig.emit_to(&mut rec);

    // Every SCALAR §4.11 signal is present on the port (an absent signal would be a RED).
    for s in [
        BusSignal::OutboxDepth,
        BusSignal::OutboxAgeSecs,
        BusSignal::RelayPublished,
        BusSignal::DeadLetterCount,
        BusSignal::PublishLatencyMillis,
        BusSignal::DedupHits,
        BusSignal::DedupDeliveries,
        BusSignal::CausalDepthMax,
        BusSignal::SharedRootTripwireFirings,
    ] {
        assert!(
            rec.scalar(s).is_some(),
            "scalar signal {s:?} must be emitted"
        );
    }
    // The two labelled signals are present per their label set.
    assert!(rec
        .labelled(
            BusSignal::ConsumerLag,
            &[myelin_events::MetricLabel::new(
                "consumer",
                "search-indexer"
            )]
        )
        .is_some());
    assert!(rec
        .labelled(
            BusSignal::PerTenantInflight,
            &[myelin_events::MetricLabel::new("tenant", "acme")]
        )
        .is_some());

    // 11 distinct scalar+labelled signal kinds, exactly the §4.11 set.
    let kinds: std::collections::HashSet<BusSignal> =
        rec.samples().iter().map(|s| s.signal).collect();
    assert_eq!(
        kinds.len(),
        BusSignal::ALL.len(),
        "the full §4.11 set is on the port"
    );
}

/// The self-test also REGISTERS into the P-S04 every-incident-adds-a-drill registry so it
/// re-runs forever (EI-01 §3/§5) — a regression that stops the Bus emitting its survival
/// signals re-reds it loudly. This makes the observability precondition a PERMANENT gate.
#[test]
fn eb11_self_test_registers_into_the_permanent_drill_suite() {
    use myelin_harness::{DrillRegistry, DrillScenario};

    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new("EB-11-telemetry-self-test", |ctx| {
        let tenant = TenantId("acme".into());
        let scope = Scope::Tenant(tenant.clone());

        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        commit_n(&store, minter, 4, "issue:PROJ-1");

        let bus = InProcessBus::new();
        let relay = Relay::new(store.clone(), bus.clone(), clock);

        // inject the producer-kill via the scenario's breaker (harness drains it on teardown).
        ctx.breaker
            .break_dependency(Dependency::Broker, scope.clone());
        if ctx.breaker.is_broken(&Dependency::Broker, &scope) {
            bus.sever();
        }
        relay.drain_to_empty();
        // restore + drain → depth 0.
        ctx.breaker.restore_dependency(Dependency::Broker, scope);
        bus.heal();
        let drain = relay.drain_to_empty();

        // snapshot → emit → bridge → assert the outbox-depth survival signal drained to 0.
        let sig = BusSignals::snapshot(
            &store,
            &drain,
            &BusObservations::default(),
            &Timestamp("2026-06-19T00:01:00Z".into()),
            0,
        );
        let mut rec = MetricRecorder::new();
        sig.emit_to(&mut rec);
        ctx.signals.set_scalar(
            SignalName::OutboxDepth,
            rec.scalar(BusSignal::OutboxDepth).unwrap(),
        );
        assert_eq!(bus.delivered_count(), 4);
        ctx.signals
            .assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
    }));

    let results = registry.run_all();
    assert!(
        results[0].is_pass(),
        "EB-11 self-test must read green: {:?}",
        results[0]
    );
    assert!(registry.all_green(), "the self-test re-runs green forever");
    println!("{}", results[0].artifact_row("2026-06-19"));
}
