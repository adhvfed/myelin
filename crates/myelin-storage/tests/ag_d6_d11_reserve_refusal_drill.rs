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

#[test]
fn ag_d11_runaway_loop_stops_at_the_wallet() {
    let mut gate = AgentRunGate::new();
    let mut ledger = CostLedger::new();

    let wallet = MicroUsd(500);
    let per_run = MicroUsd(100);
    let attempts = 50u64;
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
            Err(DispatchError::NoBalance { .. }) => {}
            Err(other) => panic!("unexpected dispatch error: {other}"),
        }
    }

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
    assert_eq!(ledger.inflight_interrupt_count(), 0);
    for h in &live {
        assert_eq!(
            ledger.state_of(&tenant(), h.run()),
            Ok(Some(ReservationState::InFlight)),
            "every funded run is still running - none torn down by the refusals"
        );
    }

    let signal = AgentRunGateSignal {
        tenant: tenant(),
        dispatches_attempted: attempts,
        runs_dispatched: gate.runs_dispatched(),
        reserve_refusals: gate.reserve_refusals(),
        inflight_interrupt_count: ledger.inflight_interrupt_count(),
    };
    assert!(signal.is_green(), "AG-D11 must be GREEN: {signal:?}");
    assert!(signal.reserve_refusals > 0, "reserve refusals must fire");
    assert_eq!(signal.inflight_interrupt_count, 0, "the headline zero");
    eprintln!("AG-D11 GREEN [2026-06-20]: {signal:?}");
}

#[test]
fn ag_d6_surge_sheds_over_budget_runs_others_unaffected() {
    let mut gate = AgentRunGate::new();
    let mut ledger = CostLedger::new();

    let attempts = 30u64;
    let per_run = MicroUsd(100);
    let wallet = MicroUsd(1_000);
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

    let err = gate
        .schedule_and_run_job(&mut ledger, tenant(), run(2), MicroUsd(9_000), MicroUsd(50))
        .expect_err("an over-budget scheduled job is refused");
    assert!(matches!(err, DispatchError::NoBalance { .. }));
    assert!(
        ledger.state_of(&tenant(), &run(2)).unwrap().is_none(),
        "the job was never scheduled"
    );

    assert!(ledger.cancel_unstarted(&tenant(), &run(1)).is_err());
    assert_eq!(ledger.inflight_interrupt_count(), 0);
    handle.settle(&mut ledger, &[]).unwrap();
    assert_eq!(
        ledger.state_of(&tenant(), &run(1)),
        Ok(Some(ReservationState::Settled))
    );
}
