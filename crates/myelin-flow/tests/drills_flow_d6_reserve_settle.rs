use myelin_events::{Actor, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp};
use myelin_flow::{
    BudgetGate, FlowTelemetry, JobKind, JobOutcome, JobRunner, JobSpec, RetryPolicy, SignalStore,
    Wallet, WfCtx, WfError, WfJournal,
};
use myelin_harness::{Dependency, DependencyBreaker, Predicate, Scope, SignalSource};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs::ArtifactRef;
use myelin_storage::reserve_settle::{MeteredUnit, MicroUsd};
use myelin_tenancy::{Region, TenantId};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn minter() -> Arc<dyn IdMinter> {
    Arc::new(MonotonicMinter::new())
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
        caused_by: None,
    }
}

fn unit(wholesale: u64, markup: u64) -> MeteredUnit {
    MeteredUnit {
        unit: "agent.step",
        wholesale: MicroUsd(wholesale),
        markup: MicroUsd(markup),
    }
}

#[derive(Default)]
struct CountingRunner {
    calls: AtomicUsize,
}
impl JobRunner for CountingRunner {
    fn dispatch(&self, _spec: &JobSpec) -> Result<(), myelin_flow::ActivityError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[test]
fn drill_flow_d6_runaway_activity_loop_refused_when_exhausted_inflight_never_interrupted() {
    let scope = Scope::Tenant(tenant());
    let breaker = DependencyBreaker::new();

    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let telemetry = FlowTelemetry::new();
    let gate = BudgetGate::new(Wallet::new(MicroUsd(300))).with_telemetry(telemetry.clone());

    breaker.break_dependency(Dependency::Broker, scope.clone());
    assert!(
        breaker.is_broken(&Dependency::Broker, &scope),
        "the runaway loop is injected"
    );

    let ran = Arc::new(AtomicUsize::new(0));
    let mut refused = 0u32;
    let mut admitted = 0u32;

    let mut ctx = WfCtx::begin(
        &outbox,
        minter(),
        journal,
        ctx_base(),
        "R1",
        "agent.run",
        "2026-06-21T00:00:00Z",
        7,
    )
    .with_budget(gate.clone());

    for _ in 0..10 {
        let ran_c = ran.clone();
        let res = ctx.metered_activity(
            RetryPolicy::default_policy(),
            MicroUsd(100),
            vec![unit(70, 30)],
            move |_idem, _att| {
                ran_c.fetch_add(1, Ordering::SeqCst);
                Ok(vec![ArtifactRef("myelin://acme/agent/step".into())])
            },
        );
        match res {
            Ok(_) => admitted += 1,
            Err(WfError::CoCommit(ref m)) if m.contains("wallet exhausted") => refused += 1,
            Err(other) => panic!("unexpected error in the loop: {other:?}"),
        }
    }

    assert_eq!(
        admitted, 3,
        "exactly 3 funded dispatches admitted (300 / 100)"
    );
    assert_eq!(
        refused, 7,
        "the remaining 7 were REFUSED at reserve (no balance → no dispatch)"
    );
    assert_eq!(
        ran.load(Ordering::SeqCst),
        3,
        "ONLY the 3 funded activities ran (the 7 never started)"
    );
    assert_eq!(gate.balance(), MicroUsd::ZERO, "the wallet is exhausted");

    let mut signals = SignalSource::new();
    signals.set_scalar(
        myelin_harness::SignalName::ShedCount,
        telemetry.reserve_rejected() as i64,
    );
    signals
        .assert_signal(myelin_harness::SignalName::ShedCount, Predicate::Gte(1))
        .expect_green();
    signals.set_scalar(
        myelin_harness::SignalName::CausalDepthFirings,
        gate.inflight_interrupt_count() as i64,
    );
    signals
        .assert_signal(
            myelin_harness::SignalName::CausalDepthFirings,
            Predicate::Eq(0),
        )
        .expect_green();

    assert_eq!(telemetry.reserve_attempted(), 10, "10 reserve attempts");
    assert_eq!(telemetry.reserve_rejected(), 7, "7 refused");
    assert_eq!(
        telemetry.reserve_reject_rate_bps(),
        7_000,
        "70% reject rate (7/10)"
    );

    breaker.restore_dependency(Dependency::Broker, scope.clone());
    assert_eq!(breaker.broken_count(), 0, "no leaked break");
    println!(
        "[2026-06-21] PASS  drill=FLOW-D6  surface=activity  reserve_refusals={refused} (>0)  \
         inflight_interrupts={}  reject_rate_bps={}  admitted={admitted}  (depleting wallet vs runaway loop)",
        gate.inflight_interrupt_count(),
        telemetry.reserve_reject_rate_bps(),
    );
}

#[test]
fn drill_flow_d6_runaway_dispatch_loop_refused_when_exhausted_runner_never_called() {
    let scope = Scope::Tenant(tenant());
    let breaker = DependencyBreaker::new();
    breaker.break_dependency(Dependency::Broker, scope.clone());

    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let signals_store = SignalStore::new();
    let telemetry = FlowTelemetry::new();
    let runner = CountingRunner::default();
    let gate = BudgetGate::new(Wallet::new(MicroUsd(400))).with_telemetry(telemetry.clone());

    let mut refused = 0u32;
    let mut completed = 0u32;

    for i in 0..6u32 {
        let run_id = format!("R{i}");
        let token = myelin_flow::job_idem_token(&run_id, "agent.run:0");
        signals_store.deliver(myelin_flow::SignalRow {
            tenant: tenant(),
            region: region(),
            run_id: run_id.clone(),
            signal_name: myelin_flow::JOB_DONE_SIGNAL.into(),
            idem_key: token,
            payload: vec![ArtifactRef("myelin://acme/ci/green".into())],
            payload_key_ref: None,
            received_unix_ms: 0,
            consumed_seq: None,
        });

        let mut ctx = WfCtx::begin(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            run_id,
            "agent.run",
            "2026-06-21T00:00:00Z",
            7,
        )
        .with_signals(signals_store.clone())
        .with_budget(gate.clone());

        let res = ctx.metered_schedule_and_run_job(
            JobSpec::new(JobKind::Agent, "agent://acme/job/loop"),
            &runner,
            None,
            MicroUsd(200),
            vec![unit(120, 80)],
        );
        match res {
            Ok(JobOutcome::Completed { .. }) => completed += 1,
            Err(WfError::CoCommit(ref m)) if m.contains("wallet exhausted") => refused += 1,
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    assert_eq!(
        completed, 2,
        "exactly 2 funded job dispatches completed (400 / 200)"
    );
    assert_eq!(refused, 4, "the remaining 4 were REFUSED at reserve");
    assert_eq!(
        runner.calls.load(Ordering::SeqCst),
        2,
        "the runner was called ONLY for the 2 funded jobs"
    );
    assert_eq!(
        gate.balance(),
        MicroUsd::ZERO,
        "the wallet is exhausted (2 × 200 drawn)"
    );

    let mut sig = SignalSource::new();
    sig.set_scalar(
        myelin_harness::SignalName::ShedCount,
        telemetry.reserve_rejected() as i64,
    );
    sig.assert_signal(myelin_harness::SignalName::ShedCount, Predicate::Gte(1))
        .expect_green();
    sig.set_scalar(
        myelin_harness::SignalName::CausalDepthFirings,
        gate.inflight_interrupt_count() as i64,
    );
    sig.assert_signal(
        myelin_harness::SignalName::CausalDepthFirings,
        Predicate::Eq(0),
    )
    .expect_green();

    breaker.restore_dependency(Dependency::Broker, scope);
    println!(
        "[2026-06-21] PASS  drill=FLOW-D6  surface=schedule_and_run_job  reserve_refusals={refused} (>0)  \
         inflight_interrupts={}  runner_calls={}  completed={completed}  (F-6 extended: reserve fronts the long-park)",
        gate.inflight_interrupt_count(),
        runner.calls.load(Ordering::SeqCst),
    );
}
