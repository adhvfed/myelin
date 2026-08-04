use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, DataRole, EmitContextBase, EventDraft, EventType,
    IdMinter, InProcessBus, MonotonicMinter, OutboxStore, OutboxTx, Relay, Timestamp, Visibility,
};
use myelin_harness::{Dependency, DependencyBreaker, Predicate, Scope, SignalName, SignalSource};
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
        recorded_at: Timestamp("2026-06-19T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:abc".into())),
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

fn commit_n(
    store: &OutboxStore,
    minter: Arc<dyn IdMinter>,
    n: usize,
    aggregate: &str,
) -> Vec<myelin_events::EventId> {
    let mut tx = store.begin(minter, ctx_base("acme"));
    tx.stage_state_change("issue created");
    let mut ids = Vec::new();
    for i in 0..n {
        ids.push(tx.emit(draft(i, aggregate), None).unwrap());
    }
    tx.commit().unwrap();
    ids
}

#[test]
fn drill_sub_d1_kill_between_commit_and_publish() {
    let tenant = TenantId("acme".into());
    let scope = Scope::Tenant(tenant.clone());

    let breaker = DependencyBreaker::new();

    let store = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let ids = commit_n(&store, minter, 6, "issue:PROJ-1");

    let bus = InProcessBus::new();
    let relay = Relay::new(store.clone(), bus.clone(), clock);

    breaker.break_dependency(Dependency::Broker, scope.clone());
    if breaker.is_broken(&Dependency::Broker, &scope) {
        bus.sever();
    }
    relay.drain_to_empty();

    let mut signals = SignalSource::new();
    signals.set_scalar(SignalName::OutboxDepth, store.outbox_depth() as i64);
    signals.set_scalar(
        SignalName::DeadLetterCount,
        store.dead_letter_count() as i64,
    );
    signals
        .assert_signal(SignalName::OutboxDepth, Predicate::Eq(6))
        .expect_green();
    signals
        .assert_signal(SignalName::DeadLetterCount, Predicate::Eq(0))
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
    relay.drain_to_empty();

    let mut after = SignalSource::new();
    after.set_scalar(SignalName::OutboxDepth, store.outbox_depth() as i64);
    after.set_scalar(
        SignalName::DeadLetterCount,
        store.dead_letter_count() as i64,
    );
    after
        .assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
        .expect_green();
    after
        .assert_signal(SignalName::DeadLetterCount, Predicate::Eq(0))
        .expect_green();
    assert_eq!(
        bus.delivered_count(),
        6,
        "exactly-once: 6 committed → 6 delivered"
    );
    assert_eq!(
        bus.delivered_ids(),
        ids.into_iter().collect(),
        "the delivered set == the committed set (0 ghost, 0 lost)"
    );

    assert_eq!(breaker.broken_count(), 0);
    println!("[2026-06-19] PASS  drill=SUB-D1  outbox_depth→0, ghost=0, lost=0  (inject → load → assert green)");
}

#[test]
fn drill_bus_d4_emit_iff_committed() {
    let store = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());

    let committed_ids = commit_n(&store, minter.clone(), 3, "issue:PROJ-1");

    {
        let mut tx = store.begin(minter, ctx_base("acme"));
        tx.stage_state_change("issue PROJ-2 (will crash before commit)");
        tx.emit(draft(0, "issue:PROJ-2"), None).unwrap();
        tx.emit(draft(1, "issue:PROJ-2"), None).unwrap();
    }

    let mut signals = SignalSource::new();
    signals.set_scalar(SignalName::OutboxDepth, store.outbox_depth() as i64);
    signals
        .assert_signal(SignalName::OutboxDepth, Predicate::Eq(3))
        .expect_green();
    assert_eq!(
        store.committed_count(),
        3,
        "emit-iff-committed: the crashed (un-committed) transaction wrote NO event"
    );

    let bus = InProcessBus::new();
    let relay = Relay::new(store.clone(), bus.clone(), clock);
    relay.drain_to_empty();

    let mut after = SignalSource::new();
    after.set_scalar(SignalName::OutboxDepth, store.outbox_depth() as i64);
    after
        .assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
        .expect_green();
    assert_eq!(
        bus.delivered_count(),
        3,
        "only the committed events are delivered"
    );
    assert_eq!(
        bus.delivered_ids(),
        committed_ids.into_iter().collect(),
        "emit-iff-committed: delivered set == committed set, the crashed events never appear"
    );
    println!(
        "[2026-06-19] PASS  drill=BUS-D4  emit_iff_committed=true  (inject → load → assert green)"
    );
}

#[test]
fn sub_d1_registers_into_the_permanent_drill_suite() {
    use myelin_harness::{DrillRegistry, DrillScenario};

    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new("SUB-D1-outbox-relay", |ctx| {
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
        relay.drain_to_empty();

        ctx.signals
            .set_scalar(SignalName::OutboxDepth, store.outbox_depth() as i64);
        assert_eq!(bus.delivered_count(), 4);
        ctx.signals
            .assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
    }));

    let results = registry.run_all();
    assert!(
        results[0].is_pass(),
        "SUB-D1 drill must read green: {:?}",
        results[0]
    );
    assert!(registry.all_green());
    println!("{}", results[0].artifact_row("2026-06-19"));
}
