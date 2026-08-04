//! # AG-D6 / AG-D11 — reserve/settle fronts agent runs (never interrupt in-flight).
//!
//! The P-ST-19 / global **P-146** headline drill. Drill catalogue
//! (`testing-strategy/01 …` §4.1):
//!
//! - **AG-D6** (F6/F9): *30× agent dispatch surge → reserve/settle refuses over-budget runs,
//!   others unaffected.* Telemetry: shed-counts; **reserve refusals**.
//! - **AG-D11** (F9): *runaway loop vs an exhausted wallet → reserve refuses new runs (NEVER
//!   interrupts in-flight); the loop stops at the wallet.* Telemetry: **reserve refusals; 0
//!   interrupt**.
//!
//! Storage owns the durable ledger correctness (contract 11.7); this prompt ships the **live
//! consumer half** — the [`AgentRunGate`] that fronts every `AgentRuntime` run + every
//! `SCHEDULE_AND_RUN_JOB` (the agent fabric AG-P4 / P-216 lands AFTER this prompt and holds
//! this gate; CI runs are fronted by the SAME gate in M4). The drill exercises the REAL gate
//! path (reserve-at-dispatch → in-flight handle → settle / refuse), not a shortcut.
//!
//! The dated GREEN artifact: `reserve_refusals > 0` (the over-budget surge / runaway loop was
//! shed) AND `inflight_interrupt_count == 0` (no in-flight run was ever torn down) AND
//! `runs_dispatched + reserve_refusals == dispatches_attempted` (no dispatch silently
//! vanished). STOR-D1/STOR-D2 remain green (this prompt touches no store backend — see the
//! P-146 report).
//!
//! **CDC pair (11.7, the live-consumer half).** PROVIDER = `myelin-storage` (the Storage-owned
//! [`AgentRunGate`] + [`CostLedger`] this prompt ships — Storage owns the durable ledger
//! correctness). CONSUMER = the run-dispatch loop in each drill below (the shape the agent fabric
//! AG-P4/P-216 and CI's M4 dispatcher take): it fronts every dispatch through the provider's gate
//! and NEVER re-implements the ledger. The drill pins the frozen provider surface the consumer
//! relies on (reserve-at-dispatch mints a handle; no balance → no handle; settle through the
//! handle; no interrupt API).
//!
//! FLOOR (EI-01 §1): the gate fronts CI runs in M4 (a CI-subsystem run-dispatch consumer) —
//! the named M4 follow-on; the real `AgentRuntime` brain (`LlmAgentRuntime`) is designed-not-
//! built (AG-P25). This drill proves the gate FRONTS the run lifecycle correct-by-construction
//! over the reference (agent-run + `SCHEDULE_AND_RUN_JOB`) dispatch shapes.

use myelin_storage::{
    AgentRunGate, AgentRunGateSignal, CostLedger, DispatchError, MeteredUnit, MicroUsd,
    ReservationState, RunId, RunKind,
};
use myelin_tenancy::TenantId;

fn tenant() -> TenantId {
    TenantId::from_token("01J0ACME")
}

fn run(n: u32) -> RunId {
    RunId::new(format!("01J0RUN_{n}"))
}

/// **AG-D11 — the runaway loop stops at the wallet, never interrupting in-flight.**
///
/// A runaway agent loop dispatches runs against a wallet that drains as live reservations hold
/// funds. The funded prefix is fronted; once the wallet cannot afford the next run the reserve
/// REFUSES it and the loop stops. The already-in-flight runs keep running — 0 interrupts.
#[test]
fn ag_d11_runaway_loop_stops_at_the_wallet() {
    let mut gate = AgentRunGate::new();
    let mut ledger = CostLedger::new();

    let wallet = MicroUsd(500); // affords exactly 5 runs of 100
    let per_run = MicroUsd(100);
    let attempts = 50u64; // a runaway loop tries far more than it can afford
    let mut spent = MicroUsd::ZERO;
    let mut live = Vec::new();

    for i in 0..attempts {
        let remaining = wallet.checked_sub(spent).unwrap_or(MicroUsd::ZERO);
        match gate.dispatch(&mut ledger, tenant(), run(i as u32), per_run, remaining) {
            Ok(handle) => {
                assert_eq!(handle.kind(), RunKind::AgentRun);
                spent = spent.checked_add(per_run).unwrap();
                live.push(handle);
            }
            Err(DispatchError::NoBalance { .. }) => { /* the loop stops at the wallet */ }
            Err(other) => panic!("unexpected dispatch error: {other}"),
        }
    }

    // Exactly the funded prefix ran; the rest were refused.
    assert_eq!(
        gate.runs_dispatched(),
        5,
        "only the funded runs were fronted"
    );
    assert_eq!(
        gate.reserve_refusals(),
        45,
        "the runaway over-budget runs were refused"
    );
    // NOT ONE in-flight run was interrupted by the refusals.
    assert_eq!(ledger.inflight_interrupt_count(), 0);
    for h in &live {
        assert_eq!(
            ledger.state_of(&tenant(), h.run()),
            Some(ReservationState::InFlight),
            "every funded run is still running — none torn down by the refusals"
        );
    }

    let signal = AgentRunGateSignal {
        tenant: tenant(),
        dispatches_attempted: attempts,
        runs_dispatched: gate.runs_dispatched(),
        reserve_refusals: gate.reserve_refusals(),
        inflight_interrupt_count: ledger.inflight_interrupt_count(),
    };
    // The dated GREEN artifact.
    assert!(signal.is_green(), "AG-D11 must be GREEN: {signal:?}");
    assert!(signal.reserve_refusals > 0, "reserve refusals must fire");
    assert_eq!(signal.inflight_interrupt_count, 0, "the headline zero");
    eprintln!("AG-D11 GREEN [2026-06-20]: {signal:?}");
}

/// **AG-D6 — a 30× agent dispatch surge sheds the over-budget runs, others unaffected.**
///
/// One tenant floods the gate with 30 dispatches; the wallet affords 10. The funded 10 are
/// fronted, the 20-deep surge is shed (reserve refusals), and the in-flight runs are
/// unaffected — they settle normally afterwards (one cost event per metered unit holds under
/// surge).
#[test]
fn ag_d6_surge_sheds_over_budget_runs_others_unaffected() {
    let mut gate = AgentRunGate::new();
    let mut ledger = CostLedger::new();

    let attempts = 30u64;
    let per_run = MicroUsd(100);
    let wallet = MicroUsd(1_000); // affords 10
    let mut spent = MicroUsd::ZERO;
    let mut live = Vec::new();

    for i in 0..attempts {
        let remaining = wallet.checked_sub(spent).unwrap_or(MicroUsd::ZERO);
        match gate.dispatch(&mut ledger, tenant(), run(i as u32), per_run, remaining) {
            Ok(handle) => {
                spent = spent.checked_add(per_run).unwrap();
                live.push(handle);
            }
            Err(DispatchError::NoBalance { .. }) => {}
            Err(other) => panic!("unexpected error: {other}"),
        }
    }

    assert_eq!(
        gate.runs_dispatched(),
        10,
        "exactly the funded runs admitted"
    );
    assert_eq!(
        gate.reserve_refusals(),
        20,
        "the 30× surge over budget was shed"
    );
    assert_eq!(
        ledger.inflight_interrupt_count(),
        0,
        "0 interrupts under surge"
    );

    // The fronted runs are unaffected — each settles to one cost event per metered unit.
    for h in &live {
        let outcome = h
            .settle(
                &mut ledger,
                &[MeteredUnit {
                    unit: "llm.tokens",
                    wholesale: MicroUsd(60),
                    markup: MicroUsd(15),
                }],
            )
            .expect("a fronted run settles");
        assert_eq!(
            outcome.cost_events.len(),
            1,
            "one cost event per metered unit"
        );
        assert_eq!(outcome.billed_total, MicroUsd(75));
        assert_eq!(
            outcome.refunded,
            MicroUsd(25),
            "the over-reservation refunds"
        );
    }

    let signal = AgentRunGateSignal {
        tenant: tenant(),
        dispatches_attempted: attempts,
        runs_dispatched: gate.runs_dispatched(),
        reserve_refusals: gate.reserve_refusals(),
        inflight_interrupt_count: ledger.inflight_interrupt_count(),
    };
    assert!(signal.is_green(), "AG-D6 must be GREEN: {signal:?}");
    assert!(signal.reserve_refusals > 0, "the surge must have shed runs");
    eprintln!("AG-D6 GREEN [2026-06-20]: {signal:?}");
}

/// **`SCHEDULE_AND_RUN_JOB` is fronted by the SAME gate** (the long-park dispatch idiom) — the
/// no-balance refusal and never-interrupt invariant hold identically; only the [`RunKind`]
/// label distinguishes it.
#[test]
fn schedule_and_run_job_fronted_by_the_same_gate() {
    let mut gate = AgentRunGate::new();
    let mut ledger = CostLedger::new();

    let handle = gate
        .schedule_and_run_job(
            &mut ledger,
            tenant(),
            run(1),
            MicroUsd(400),
            MicroUsd(1_000),
        )
        .expect("a funded scheduled job is fronted");
    assert_eq!(handle.kind(), RunKind::ScheduleAndRunJob);

    // Over-budget scheduled job: not scheduled (no balance → no run).
    let err = gate
        .schedule_and_run_job(
            &mut ledger,
            tenant(),
            run(2),
            MicroUsd(9_000),
            MicroUsd(50),
        )
        .expect_err("an over-budget scheduled job is refused");
    assert!(matches!(err, DispatchError::NoBalance { .. }));
    assert!(
        ledger.state_of(&tenant(), &run(2)).is_none(),
        "the job was never scheduled"
    );

    // The fronted job is in-flight and cannot be torn down.
    assert!(ledger.cancel_unstarted(&tenant(), &run(1)).is_err());
    assert_eq!(ledger.inflight_interrupt_count(), 0);
    // It settles normally on its completion signal (the long-park settle).
    handle.settle(&mut ledger, &[]).unwrap();
    assert_eq!(
        ledger.state_of(&tenant(), &run(1)),
        Some(ReservationState::Settled)
    );
}
