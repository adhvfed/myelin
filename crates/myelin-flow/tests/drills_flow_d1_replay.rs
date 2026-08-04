use myelin_events::{
    Actor, AggregateKey, ArtifactRef as EvArtifactRef, CausedBy, DataRole, EmitContextBase,
    EventDraft, EventType, IdMinter, MonotonicMinter, OutboxStore, Timestamp, Visibility,
};
use myelin_flow::engine::{drive, run_state, DriveOutcome, FlowTelemetry, RunRow, RunStore};
use myelin_flow::{RetryPolicy, WfCtx, WfJournal, WorkflowBody};
use myelin_harness::{Dependency, DependencyBreaker, Predicate, Scope, SignalName, SignalSource};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};
use std::sync::{Arc, Mutex};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            tenant(),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:abc".into())),
    }
}
fn minter() -> Arc<dyn IdMinter> {
    Arc::new(MonotonicMinter::new())
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

fn ten_activity_body(executed: Arc<Mutex<Vec<usize>>>) -> Box<WorkflowBody> {
    Box::new(move |ctx: &mut WfCtx| {
        for k in 0..10usize {
            let ex = executed.clone();
            ctx.activity(RetryPolicy::default_policy(), move |_idem, _attempt| {
                ex.lock().unwrap().push(k);
                Ok(vec![ArtifactRef(format!(
                    "myelin://acme/agent/effect/e{k}"
                ))])
            })
            .map_err(|e| format!("{e:?}"))?;
        }
        ctx.emit(draft(), None).map_err(|e| format!("{e:?}"))?;
        Ok(vec![ArtifactRef("myelin://acme/agent/run/R1/done".into())])
    })
}

fn crash_after_journaling(
    outbox: &OutboxStore,
    journal: &WfJournal,
    runs: &RunStore,
    up_to: usize,
) -> Vec<usize> {
    let executed = Arc::new(Mutex::new(Vec::new()));
    let ex = executed.clone();
    let mut ctx = WfCtx::begin(
        outbox,
        minter(),
        journal.clone(),
        ctx_base(),
        "R1",
        "agent.run",
        "2026-06-21T00:00:00Z",
        7,
    );
    for k in 0..up_to {
        let ex2 = ex.clone();
        ctx.activity(RetryPolicy::default_policy(), move |_idem, _attempt| {
            ex2.lock().unwrap().push(k);
            Ok(vec![ArtifactRef(format!(
                "myelin://acme/agent/effect/e{k}"
            ))])
        })
        .expect("the activity runs");
    }
    ctx.commit()
        .expect("the first steps co-commit (durable before the crash)");
    let mut r = runs.get(&tenant(), "R1").expect("run");
    r.cursor = up_to as i64;
    runs.put(r);
    let ran = executed.lock().unwrap().clone();
    ran
}

#[test]
fn drill_flow_d1_replay_resume_at_6_zero_double_effect() {
    let scope = Scope::Tenant(tenant());
    let breaker = DependencyBreaker::new();

    let runs = RunStore::new();
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let tele = FlowTelemetry::new();
    runs.put(RunRow::new_runnable(
        tenant(),
        region(),
        "R1",
        "agent.run",
        0,
    ));

    breaker.break_dependency(Dependency::Broker, scope.clone());
    assert!(
        breaker.is_broken(&Dependency::Broker, &scope),
        "the worker-crash fault is injected"
    );
    let ran_before_crash = crash_after_journaling(&outbox, &journal, &runs, 5);
    assert_eq!(
        ran_before_crash,
        vec![0, 1, 2, 3, 4],
        "the crashed worker ran activities 0..=4"
    );
    assert_eq!(
        journal.history_for(&tenant(), "R1").len(),
        5,
        "5 journaled at the crash point"
    );
    assert_eq!(
        runs.get(&tenant(), "R1").unwrap().state,
        run_state::RUNNING,
        "the run survives runnable"
    );

    breaker.restore_dependency(Dependency::Broker, scope.clone());
    assert!(
        !breaker.is_broken(&Dependency::Broker, &scope),
        "the fault is restored"
    );
    let leased = runs
        .lease_runnable(0, "worker-2", 1000, 30)
        .expect("worker-2 re-leases the runnable run");
    assert_eq!(leased.cursor, 5, "the re-leased run resumes from cursor 5");
    assert_eq!(
        leased.lease_owner.as_deref(),
        Some("worker-2"),
        "a fresh worker holds the lease"
    );

    let executed = Arc::new(Mutex::new(Vec::new()));
    let body = ten_activity_body(executed.clone());
    let outcome = drive(
        &runs,
        &outbox,
        &journal,
        &tele,
        minter(),
        ctx_base(),
        &leased,
        "2026-06-21T00:00:00Z",
        7,
        body.as_ref(),
    );

    let ran_after_resume = executed.lock().unwrap().clone();
    assert_eq!(
        ran_after_resume,
        vec![5, 6, 7, 8, 9],
        "resumed at step 6 - only activities 5..=9 ran; 0..=4 replayed (0 re-execution)"
    );
    assert!(
        matches!(outcome, DriveOutcome::Completed(_)),
        "the run completed after recovery"
    );
    assert_eq!(
        journal.history_for(&tenant(), "R1").len(),
        10,
        "10 journaled, 0 lost progress"
    );
    assert_eq!(
        runs.get(&tenant(), "R1").unwrap().state,
        run_state::COMPLETED,
        "settled completed"
    );

    let mut signals = SignalSource::new();
    signals.set_scalar(SignalName::OutboxDepth, tele.double_effect_count() as i64);
    signals
        .assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
        .expect_green();
    assert_eq!(
        tele.double_effect_count(),
        0,
        "0 re-executed side effects (exactly-once-in-effect)"
    );

    assert_eq!(
        tele.commands_replayed(),
        5,
        "the 5 journaled commands replayed (short-circuited)"
    );
    assert_eq!(
        tele.replay_rate_bps(),
        5000,
        "the replay-rate signal is emitted (the green artifact)"
    );

    assert_eq!(breaker.broken_count(), 0, "no leaked dependency break");
    println!(
        "[2026-06-21] PASS  drill=FLOW-D1  kill@5/10 resume@6  re_executed=0 lost=0  replay_rate=5000bps double_effect=0  exactly-once-in-effect  (inject \u{2192} re-lease \u{2192} replay \u{2192} assert green)"
    );
}

#[test]
fn drill_flow_d1_full_replay_re_executes_zero() {
    let runs = RunStore::new();
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let tele = FlowTelemetry::new();
    runs.put(RunRow::new_runnable(
        tenant(),
        region(),
        "R1",
        "agent.run",
        0,
    ));

    let body = ten_activity_body(Arc::new(Mutex::new(Vec::new())));
    let run = runs.get(&tenant(), "R1").unwrap();
    drive(
        &runs,
        &outbox,
        &journal,
        &tele,
        minter(),
        ctx_base(),
        &run,
        "2026-06-21T00:00:00Z",
        7,
        body.as_ref(),
    );
    assert_eq!(journal.history_for(&tenant(), "R1").len(), 10);

    let executed = Arc::new(Mutex::new(Vec::new()));
    let body2 = ten_activity_body(executed.clone());
    let again = runs.get(&tenant(), "R1").unwrap();
    drive(
        &runs,
        &outbox,
        &journal,
        &tele,
        minter(),
        ctx_base(),
        &again,
        "2026-06-21T00:00:00Z",
        7,
        body2.as_ref(),
    );
    assert_eq!(
        executed.lock().unwrap().len(),
        0,
        "a full replay re-executes 0 side effects"
    );
    assert_eq!(
        tele.double_effect_count(),
        0,
        "0 double-effect under redelivery (exactly-once)"
    );
    assert_eq!(
        journal.history_for(&tenant(), "R1").len(),
        10,
        "no duplicate journal rows"
    );
    println!("[2026-06-21] PASS  drill=FLOW-D1  full_replay  re_executed=0 double_effect=0");
}

#[test]
fn flow_d1_registers_into_the_permanent_drill_suite() {
    use myelin_harness::{DrillRegistry, DrillScenario};

    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new("FLOW-D1-replay-resume", |ctx| {
        let scope = Scope::Tenant(tenant());
        let runs = RunStore::new();
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let tele = FlowTelemetry::new();
        runs.put(RunRow::new_runnable(
            tenant(),
            region(),
            "R1",
            "agent.run",
            0,
        ));

        ctx.breaker
            .break_dependency(Dependency::Broker, scope.clone());
        crash_after_journaling(&outbox, &journal, &runs, 5);
        ctx.breaker.restore_dependency(Dependency::Broker, scope);
        let leased = runs
            .lease_runnable(0, "worker-2", 1000, 30)
            .expect("re-lease");
        let executed = Arc::new(Mutex::new(Vec::new()));
        let body = ten_activity_body(executed.clone());
        drive(
            &runs,
            &outbox,
            &journal,
            &tele,
            minter(),
            ctx_base(),
            &leased,
            "2026-06-21T00:00:00Z",
            7,
            body.as_ref(),
        );
        assert_eq!(
            executed.lock().unwrap().clone(),
            vec![5, 6, 7, 8, 9],
            "resumed at 6"
        );

        ctx.signals
            .set_scalar(SignalName::OutboxDepth, tele.double_effect_count() as i64);
        ctx.signals
            .assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
    }));

    let results = registry.run_all();
    assert!(
        results[0].is_pass(),
        "FLOW-D1 drill must read green: {:?}",
        results[0]
    );
    assert!(
        registry.all_green(),
        "the permanent suite re-runs FLOW-D1 green forever"
    );
    println!("{}", results[0].artifact_row("2026-06-21"));
}
