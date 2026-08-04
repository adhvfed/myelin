use myelin_events::{
    Actor, AggregateKey, ArtifactRef as EvArtifactRef, CausedBy, DataRole, EmitContextBase,
    EventDraft, EventType, IdMinter, InProcessBus, MonotonicMinter, OutboxStore, Relay, Timestamp,
    Visibility,
};
use myelin_flow::{ActivityError, RetryPolicy, WfCtx, WfJournal};
use myelin_harness::{Dependency, DependencyBreaker, Predicate, Scope, SignalName, SignalSource};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

fn clock() -> Timestamp {
    Timestamp("2026-06-21T00:00:02Z".into())
}

fn ctx_base(tenant: &str) -> EmitContextBase {
    EmitContextBase {
        tenant: TenantId(tenant.into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:abc".into())),
    }
}

fn draft() -> EventDraft {
    EventDraft {
        type_: EventType("agent.run.step".into()),
        subject: EvArtifactRef("myelin://acme/agent/run/R1".into()),
        aggregate: AggregateKey("run:R1".into()),
        payload: serde_json::json!({ "ref": "R1" }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

fn minter() -> Arc<dyn IdMinter> {
    Arc::new(MonotonicMinter::new())
}

fn run_step(outbox: &OutboxStore, journal: &WfJournal, crash_before_commit: bool) {
    let mut ctx = WfCtx::begin(
        outbox,
        minter(),
        journal.clone(),
        ctx_base("acme"),
        "R1",
        "agent.run",
        "2026-06-21T00:00:00Z",
        7,
    );
    ctx.activity(RetryPolicy::default_policy(), |_idem, _attempt| {
        Ok(vec![ArtifactRef("myelin://acme/agent/effect/e1".into())])
    })
    .expect("the activity runs");
    ctx.emit(draft(), None)
        .expect("emit buffers into the co-commit txn");
    if crash_before_commit {
        drop(ctx);
    } else {
        ctx.commit().expect("the journal + outbox co-commit");
    }
}

#[test]
fn drill_flow_d5_journal_outbox_co_commit() {
    let tenant = TenantId("acme".into());
    let scope = Scope::Tenant(tenant.clone());
    let breaker = DependencyBreaker::new();

    let outbox = OutboxStore::new();
    let journal = WfJournal::new();

    breaker.break_dependency(Dependency::Broker, scope.clone());
    let crashing = breaker.is_broken(&Dependency::Broker, &scope);
    assert!(crashing, "the fault is injected");
    run_step(&outbox, &journal, crashing);

    assert_eq!(
        journal.history_len(),
        0,
        "0 lost: the crashed step journaled nothing"
    );
    assert_eq!(
        journal.attempt_len(),
        0,
        "0 lost: the attempt ledger is unwritten too"
    );
    let mut signals = SignalSource::new();
    signals.set_scalar(SignalName::OutboxDepth, outbox.outbox_depth() as i64);
    signals.set_scalar(
        SignalName::DeadLetterCount,
        outbox.dead_letter_count() as i64,
    );
    signals
        .assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
        .expect_green();
    signals
        .assert_signal(SignalName::DeadLetterCount, Predicate::Eq(0))
        .expect_green();

    breaker.restore_dependency(Dependency::Broker, scope.clone());
    let healthy = !breaker.is_broken(&Dependency::Broker, &scope);
    assert!(healthy, "the fault is restored");
    run_step(&outbox, &journal, !healthy);

    assert_eq!(
        journal.history_len(),
        1,
        "co-commit: exactly one journal row"
    );
    assert_eq!(
        journal.attempt_len(),
        1,
        "co-commit: exactly one attempt ledger row"
    );
    let mut after = SignalSource::new();
    after.set_scalar(SignalName::OutboxDepth, outbox.outbox_depth() as i64);
    after
        .assert_signal(SignalName::OutboxDepth, Predicate::Eq(1))
        .expect_green();

    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), clock);
    relay.drain_to_empty();
    assert_eq!(
        bus.delivered_count(),
        1,
        "exactly-once: the co-committed event is delivered once"
    );
    let mut drained = SignalSource::new();
    drained.set_scalar(SignalName::OutboxDepth, outbox.outbox_depth() as i64);
    drained
        .assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
        .expect_green();

    assert_eq!(breaker.broken_count(), 0);
    println!(
        "[2026-06-21] PASS  drill=FLOW-D5  co_commit=atomic  ghost=0 lost=0  journal_rows=1 outbox_depth→0  (inject → drive → assert green)"
    );
}

#[test]
fn drill_flow_d5_emit_failure_writes_neither() {
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    {
        let mut ctx = WfCtx::begin(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base("acme"),
            "R2",
            "agent.run",
            "2026-06-21T00:00:00Z",
            1,
        );
        ctx.activity(RetryPolicy::default_policy(), |_i, _a| {
            Ok(vec![ArtifactRef("myelin://acme/agent/effect/e1".into())])
        })
        .expect("activity");
        ctx.emit(draft(), None).expect("emit staged");
        drop(ctx);
    }
    assert_eq!(journal.history_len(), 0, "no journal row on the crash path");
    assert_eq!(
        outbox.committed_count(),
        0,
        "no committed outbox row on the crash path"
    );
    assert_eq!(outbox.outbox_depth(), 0, "0 ghost");
    println!("[2026-06-21] PASS  drill=FLOW-D5  emit_path_abort=neither  (0 ghost, 0 lost)");
}

#[test]
fn flow_d5_registers_into_the_permanent_drill_suite() {
    use myelin_harness::{DrillRegistry, DrillScenario};

    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new("FLOW-D5-wfctx-co-commit", |ctx| {
        let tenant = TenantId("acme".into());
        let scope = Scope::Tenant(tenant.clone());

        let outbox = OutboxStore::new();
        let journal = WfJournal::new();

        ctx.breaker
            .break_dependency(Dependency::Broker, scope.clone());
        let crashing = ctx.breaker.is_broken(&Dependency::Broker, &scope);
        run_step(&outbox, &journal, crashing);
        assert_eq!(journal.history_len(), 0, "crashed step journals nothing");
        assert_eq!(outbox.committed_count(), 0, "crashed step emits nothing");

        ctx.breaker.restore_dependency(Dependency::Broker, scope);
        run_step(&outbox, &journal, false);
        assert_eq!(
            journal.history_len(),
            1,
            "co-commit journals exactly one row"
        );

        ctx.signals
            .set_scalar(SignalName::OutboxDepth, outbox.outbox_depth() as i64);
        ctx.signals
            .assert_signal(SignalName::OutboxDepth, Predicate::Eq(1))
    }));

    let results = registry.run_all();
    assert!(
        results[0].is_pass(),
        "FLOW-D5 drill must read green: {:?}",
        results[0]
    );
    assert!(
        registry.all_green(),
        "the permanent suite re-runs FLOW-D5 green forever"
    );
    println!("{}", results[0].artifact_row("2026-06-21"));
}

#[test]
fn drill_flow_d5_failed_activity_no_ghost_emit() {
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let mut ctx = WfCtx::begin(
        &outbox,
        minter(),
        journal.clone(),
        ctx_base("acme"),
        "R3",
        "agent.run",
        "2026-06-21T00:00:00Z",
        9,
    );
    let err = ctx
        .activity(RetryPolicy { max_attempts: 2 }, |_i, attempt| {
            Err(ActivityError(format!("hard failure {attempt}")))
        })
        .expect_err("the activity exhausts its retries");
    assert!(matches!(err, myelin_flow::WfError::ActivityExhausted(_)));
    ctx.commit().expect("the failure co-commits");
    assert_eq!(
        outbox.outbox_depth(),
        0,
        "0 ghost: a failed activity emitted nothing"
    );
    assert_eq!(
        journal.history_len(),
        1,
        "the activity_failed row IS journaled (0 lost)"
    );
}
