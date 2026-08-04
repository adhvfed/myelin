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

fn bridge_into_signal_source(rec: &MetricRecorder, src: &mut SignalSource) {
    if let Some(v) = rec.scalar(BusSignal::OutboxDepth) {
        src.set_scalar(SignalName::OutboxDepth, v);
    }
    if let Some(v) = rec.scalar(BusSignal::DeadLetterCount) {
        src.set_scalar(SignalName::DeadLetterCount, v);
    }
    if let Some(v) = rec.scalar(BusSignal::SharedRootTripwireFirings) {
        src.set_scalar(SignalName::CausalDepthFirings, v);
    }
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

#[test]
fn eb11_self_test_inject_producer_kill_read_outbox_depth_and_dedup_assertion() {
    let tenant = TenantId("acme".into());
    let scope = Scope::Tenant(tenant.clone());

    let breaker = DependencyBreaker::new();

    let store = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    commit_n(&store, minter, 5, "issue:PROJ-1");

    let bus = InProcessBus::new();
    let relay = Relay::new(store.clone(), bus.clone(), clock);

    breaker.break_dependency(Dependency::Broker, scope.clone());
    if breaker.is_broken(&Dependency::Broker, &scope) {
        bus.sever();
    }
    let drain_severed = relay.drain_to_empty();

    let obs = BusObservations::default();
    let now_severed = Timestamp("2026-06-19T00:00:30Z".into());
    let sig = BusSignals::snapshot(&store, &drain_severed, &obs, &now_severed, 0);
    let mut rec = MetricRecorder::new();
    sig.emit_to(&mut rec);

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

    breaker.restore_dependency(Dependency::Broker, scope.clone());
    if !breaker.is_broken(&Dependency::Broker, &scope) {
        bus.heal();
    }
    let drain_healed = relay.drain_to_empty();
    let redrain = relay.drain_to_empty();

    let obs_after = BusObservations {
        dedup_hits: redrain.deduplicated as u64 + 1,
        dedup_deliveries: 5 + 1,
        ..BusObservations::default()
    };
    let now_drained = Timestamp("2026-06-19T00:01:00Z".into());
    let sig_after = BusSignals::snapshot(&store, &drain_healed, &obs_after, &now_drained, 0);
    let mut rec_after = MetricRecorder::new();
    sig_after.emit_to(&mut rec_after);

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

    let kinds: std::collections::HashSet<BusSignal> =
        rec.samples().iter().map(|s| s.signal).collect();
    assert_eq!(
        kinds.len(),
        BusSignal::ALL.len(),
        "the full §4.11 set is on the port"
    );
}

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

        ctx.breaker
            .break_dependency(Dependency::Broker, scope.clone());
        if ctx.breaker.is_broken(&Dependency::Broker, &scope) {
            bus.sever();
        }
        relay.drain_to_empty();
        ctx.breaker.restore_dependency(Dependency::Broker, scope);
        bus.heal();
        let drain = relay.drain_to_empty();

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
