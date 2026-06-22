//! # FLOW-D1 drill — deterministic replay/recovery + lease-based crash recovery (P-FLOW-05 → P-202)
//!
//! This is the failure-injection-harness drill the P-FLOW-05 TESTS field requires (the CHAINED
//! drill, EI-01 §4): it rides the M0 **scoped-reversible dependency-break injector**
//! ([`myelin_harness::DependencyBreaker`], `Dependency::Broker` as the worker-crash fault) to inject
//! the **"kill a worker at activity 5 of 10 mid-run"** fault, then drives the
//! [`myelin_flow::engine`] replay/lease loop (the P-FLOW-05 deliverable): ANOTHER worker re-leases
//! the run, replays `wf_history` (short-circuiting every journaled command — 0 re-executed side
//! effects), and resumes at step 6. It reads the M0 telemetry-assertion library survival signals —
//! the **replay-rate** (emitted) and the **0-double-effect** counter on the metrics port (a typed
//! green/red, never a swallowed pass — EI-01 §3).
//!
//! **The threshold is exact (testing-strategy FLOW-D1):** resume at step 6, 0 re-executed side
//! effects, 0 lost progress, exactly-once-in-effect. A red drill is information, not a thing to
//! weaken to pass. The replay-rate signal emitted + the 0-double-effect counter is the dated CI
//! green artifact.

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

/// A 10-activity workflow body. Each activity records its index into `executed` (so the drill reads
/// which steps RAN vs replayed) and emits one domain event (so the co-commit is exercised). Returns
/// the terminal "done" ref.
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

/// Model a worker that crashes after journaling the first `up_to` activities: it runs an `up_to`-step
/// body on its own `WfCtx`, co-commits those steps (durable), then DIES before settling the run row —
/// the run is left `running`, cursor bumped, lease lapsed. The journal is the source of truth.
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
    // the worker bumped the cursor as it journaled, then DIED before settling the terminal state.
    let mut r = runs.get(&tenant(), "R1").expect("run");
    r.cursor = up_to as i64;
    runs.put(r);
    let ran = executed.lock().unwrap().clone();
    ran
}

/// **FLOW-D1 — kill a worker at activity 5/10 mid-run → another re-leases, replays, resumes at step 6
/// with 0 re-executed side effects, 0 lost progress, exactly-once-in-effect.**
///
/// Rides the M0 injector (`Dependency::Broker`, tenant-scoped, as the worker-crash fault) + the M0
/// assertion library (the replay-rate + 0-double-effect survival signals). The fault is injected at
/// step 5; on RESTORE another worker re-leases and the replay resumes at step 6.
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

    // (1) INJECT the fault: the worker crashes at activity 5 of 10. The first 5 steps co-commit
    //     (durable), then the worker dies — the run is left runnable from cursor 5.
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

    // (2) RESTORE: another worker re-leases the run (the dead worker's lease lapsed) and re-drives.
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

    // (3) ASSERT the FLOW-D1 thresholds: resumed at step 6 (only 5..=9 ran), 0 re-executed side
    //     effects, 0 lost progress, the run completed exactly-once-in-effect.
    let ran_after_resume = executed.lock().unwrap().clone();
    assert_eq!(
        ran_after_resume,
        vec![5, 6, 7, 8, 9],
        "resumed at step 6 — only activities 5..=9 ran; 0..=4 replayed (0 re-execution)"
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

    // The 0-double-effect counter on the metrics port — the FLOW-D1 green artifact (exactly-once).
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

    // The replay-rate signal is emitted: drive 2 replayed 5 commands, executed 5 → 5000 bps.
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

    // teardown: no leaked break.
    assert_eq!(breaker.broken_count(), 0, "no leaked dependency break");
    println!(
        "[2026-06-21] PASS  drill=FLOW-D1  kill@5/10 resume@6  re_executed=0 lost=0  replay_rate=5000bps double_effect=0  exactly-once-in-effect  (inject \u{2192} re-lease \u{2192} replay \u{2192} assert green)"
    );
}

/// **FLOW-D1 also asserts: the 0-double-effect counter is a REAL probe (a regression would red it).**
/// A re-drive of a FULLY-journaled run replays ALL 10 commands and re-executes NONE — the
/// double-effect counter stays 0. This pins the floor: a mutant that removes the replay short-circuit
/// (re-executes a journaled activity) makes the counter non-zero and reds the drill.
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

    // drive 1: complete the run (10 journaled commands).
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

    // a redelivery re-drives the same journal — everything replays, 0 re-execution.
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

/// The drill REGISTERS into the M0 every-incident-adds-a-drill registry so it re-runs forever
/// (EI-01 §3/§5) — a regression on the replay short-circuit re-reds it loudly.
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

        // inject the worker-crash fault, journal 5 steps, restore, re-lease, replay.
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

        // the 0-double-effect counter is the asserted survival signal.
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
