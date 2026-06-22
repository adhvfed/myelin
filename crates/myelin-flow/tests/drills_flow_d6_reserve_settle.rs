//! # FLOW-D6 drill — the reserve/settle bookend on a runaway loop vs a depleting wallet (P-FLOW-16 → P-212)
//!
//! The headline drill the P-FLOW-16 GATE requires (testing-strategy FLOW-D6 + the F-6 extended
//! assertion, durable-workflow §8): a **runaway agent loop** against a **depleting wallet** — a new
//! spend-bearing activity (INCLUDING a `SCHEDULE_AND_RUN_JOB` dispatch) is **REFUSED at reserve when
//! the wallet is exhausted** (the job never starts), and an **already-dispatched / in-flight job is
//! NEVER interrupted** (it settles on completion). The green artifact (testing-strategy FLOW-D6):
//! **reserve-refusal count > 0** AND the **in-flight-interrupt counter == 0**, dated. A red drill is
//! information — never weaken it to pass (EI-01 §3).
//!
//! **What the runaway loop models:** an adversarial workflow body that keeps dispatching spend-bearing
//! jobs in a loop (the §6.2 loop the budget self-limits). The wallet starts with budget for a FEW
//! dispatches; the loop tries MANY. The reserve/settle bookend ([`myelin_flow::BudgetGate`]) admits the
//! funded ones, marks them in-flight, and REFUSES the rest the moment the wallet is exhausted — the
//! self-limiter (FLOW-D6 / AG-D11). The in-flight ones (already begun) settle normally; nothing is ever
//! interrupted (the never-interrupt-in-flight invariant is structural — there is NO teardown path for
//! an in-flight reservation, inherited from the Storage ledger, contract 11.7).
//!
//! **Rides the M0 failure-injection harness:** the [`myelin_harness::DependencyBreaker`]
//! (`Dependency::Broker`, tenant-scoped — the SAME seam BUS-D4 / FLOW-D5 use) models the runaway
//! condition (the loop is "broken open" — it keeps feeding itself). The drill asserts the survival
//! signals via the M0 assertion library ([`myelin_harness::SignalSource`] / [`Predicate`]): the
//! reserve-reject-rate (`> 0`) and the in-flight-interrupt count (`== 0`) — a typed green/red that is
//! never a swallowed pass (EI-01 §3).

use myelin_events::{Actor, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp};
use myelin_flow::{
    BudgetGate, FlowTelemetry, JobKind, JobOutcome, JobRunner, JobSpec, RetryPolicy, SignalStore,
    Wallet, WfCtx, WfError, WfJournal,
};
use myelin_harness::{Dependency, DependencyBreaker, Predicate, Scope, SignalSource};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs::ArtifactRef;
use myelin_storage::reserve_settle::{MeteredUnit, MinorUnits};
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
        wholesale: MinorUnits(wholesale),
        markup: MinorUnits(markup),
    }
}

/// A runner that ACCEPTS every dispatch (and counts them) — so the drill can prove the runner is
/// called ONLY for the funded dispatches (the refused ones never reach it).
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

/// **FLOW-D6 — a runaway loop of spend-bearing ACTIVITIES against a depleting wallet: the exhausted
/// wallet REFUSES new dispatches (refusal count > 0), the in-flight ones are NEVER interrupted (== 0).**
///
/// The loop tries 10 metered activities, each costing 100; the wallet holds 300 (room for 3). Activities
/// 1-3 reserve + run + settle (drawing the wallet down); activities 4-10 are REFUSED at reserve (the
/// activity closure never runs). The in-flight-interrupt counter stays 0 — no running activity was torn
/// down. The green artifact: refusals = 7 (> 0), interrupts = 0.
#[test]
fn drill_flow_d6_runaway_activity_loop_refused_when_exhausted_inflight_never_interrupted() {
    let scope = Scope::Tenant(tenant());
    let breaker = DependencyBreaker::new();

    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let telemetry = FlowTelemetry::new();
    // The depleting wallet: 300 minor-units = room for exactly THREE 100-unit dispatches.
    let gate = BudgetGate::new(Wallet::new(MinorUnits(300))).with_telemetry(telemetry.clone());

    // (1) INJECT the runaway condition: the loop is "broken open" (it keeps feeding itself). The SAME
    //     tenant-scoped Broker seam BUS-D4 / FLOW-D5 use.
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

    // (2) DRIVE the runaway loop: 10 spend-bearing activities, each costing 100. The wallet funds 3.
    for _ in 0..10 {
        let ran_c = ran.clone();
        let res = ctx.metered_activity(
            RetryPolicy::default_policy(),
            MinorUnits(100),
            vec![unit(70, 30)], // bill exactly 100 — full draw, no refund
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

    // (3) ASSERT the green artifact via the M0 assertion library.
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
    assert_eq!(gate.balance(), MinorUnits::ZERO, "the wallet is exhausted");

    let mut signals = SignalSource::new();
    // reserve-reject count > 0 (the depleting wallet refused new dispatches) — the headline green.
    signals.set_scalar(
        myelin_harness::SignalName::ShedCount,
        telemetry.reserve_rejected() as i64,
    );
    signals
        .assert_signal(myelin_harness::SignalName::ShedCount, Predicate::Gte(1))
        .expect_green();
    // the in-flight-interrupt counter == 0 (the headline zero — no running activity torn down).
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

/// **FLOW-D6 (F-6 extended) — a runaway loop of `SCHEDULE_AND_RUN_JOB` DISPATCHES against a depleting
/// wallet: the exhausted wallet REFUSES new dispatches (the runner is never called), the in-flight job
/// is NEVER interrupted.**
///
/// The §8 assertion: reserve-at-dispatch fronts the long-park too. The loop tries 6 job dispatches,
/// each reserving 200; the wallet holds 400 (room for 2). Dispatch 1-2 reserve + hand to the runner +
/// settle on the buffered job.done; dispatch 3-6 are REFUSED at reserve (the runner is NEVER called).
/// The in-flight-interrupt counter stays 0.
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
    // The depleting wallet: 400 = room for exactly TWO 200-unit dispatches.
    let gate = BudgetGate::new(Wallet::new(MinorUnits(400))).with_telemetry(telemetry.clone());

    let mut refused = 0u32;
    let mut completed = 0u32;

    // We drive each dispatch on its OWN WfCtx (a fresh body-call) so each dispatch is the first command
    // (command_id agent.run:0) — and pre-buffer its job.done so the funded ones COMPLETE (a fast job).
    for i in 0..6u32 {
        let run_id = format!("R{i}");
        // Pre-buffer the job.done under the deterministic dispatch token for this run (a fast runner).
        let token = myelin_flow::job_idem_token(&run_id, "agent.run:0");
        signals_store.deliver(myelin_flow::SignalRow {
            tenant: tenant(),
            region: region(),
            run_id: run_id.clone(),
            signal_name: myelin_flow::JOB_DONE_SIGNAL.into(),
            idem_key: token,
            payload: vec![ArtifactRef("myelin://acme/ci/green".into())],
            payload_key_ref: None,
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
            MinorUnits(200),
            vec![unit(120, 80)], // bill exactly 200 — full draw
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
        MinorUnits::ZERO,
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
