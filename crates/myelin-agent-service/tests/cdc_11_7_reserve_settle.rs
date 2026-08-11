use myelin_storage::agent_run_gate::{AgentRunGate, DispatchError, RunKind};
use myelin_storage::reserve_settle::{CostLedger, MeteredUnit, MicroUsd, ReservationState, RunId};
use myelin_tenancy::TenantId;

fn tenant() -> TenantId {
    TenantId::from_token("01J0ACME")
}

#[test]
fn consumer_reserve_at_dispatch_no_balance_no_run() {
    let mut gate = AgentRunGate::new();
    let mut ledger = CostLedger::new();

    let handle = gate
        .dispatch(
            &mut ledger,
            tenant(),
            RunId::new("run-funded"),
            MicroUsd(100),
            MicroUsd(1_000),
        )
        .expect("a funded run is fronted");
    assert_eq!(handle.kind(), RunKind::AgentRun);
    assert_eq!(
        ledger.state_of(&tenant(), &RunId::new("run-funded")),
        Ok(Some(ReservationState::InFlight))
    );

    let err = gate
        .dispatch(
            &mut ledger,
            tenant(),
            RunId::new("run-broke"),
            MicroUsd(9_000),
            MicroUsd(10),
        )
        .expect_err("an exhausted wallet refuses the run");
    assert!(
        matches!(err, DispatchError::NoBalance { .. }),
        "no balance → no run: {err}"
    );
    assert!(
        ledger
            .state_of(&tenant(), &RunId::new("run-broke"))
            .unwrap()
            .is_none(),
        "a refused run leaves NO reservation - it never started"
    );
    assert_eq!(
        gate.reserve_refusals(),
        1,
        "the gate counted the refusal (the AG-D11 telemetry)"
    );
}

#[test]
fn consumer_settle_on_completion_one_event_per_unit_with_split() {
    let mut gate = AgentRunGate::new();
    let mut ledger = CostLedger::new();
    let handle = gate
        .dispatch(
            &mut ledger,
            tenant(),
            RunId::new("run-1"),
            MicroUsd(1_000),
            MicroUsd(5_000),
        )
        .unwrap();

    let units = vec![
        MeteredUnit {
            unit: "llm.tokens",
            wholesale: MicroUsd(120),
            markup: MicroUsd(30),
        },
        MeteredUnit {
            unit: "ci.minute",
            wholesale: MicroUsd(200),
            markup: MicroUsd(50),
        },
    ];
    let outcome = handle.settle(&mut ledger, &units).expect("the run settles");
    assert_eq!(
        outcome.cost_events.len(),
        2,
        "one cost event per metered unit"
    );
    assert_ne!(
        outcome.cost_events[0].wholesale, outcome.cost_events[0].markup,
        "wholesale ≠ markup recorded distinctly (never conflated)"
    );
    assert_eq!(outcome.billed_total, MicroUsd(400));
    assert_eq!(
        outcome.refunded,
        MicroUsd(600),
        "the over-reservation refunds"
    );
    assert_eq!(
        ledger.state_of(&tenant(), &RunId::new("run-1")),
        Ok(Some(ReservationState::Settled))
    );

    let zero = gate
        .dispatch(
            &mut ledger,
            tenant(),
            RunId::new("run-mock"),
            MicroUsd(10),
            MicroUsd(5_000),
        )
        .unwrap();
    let mock_outcome = zero
        .settle(&mut ledger, &[])
        .expect("a zero-cost run settles");
    assert_eq!(
        mock_outcome.cost_events.len(),
        0,
        "a Mock meters zero units"
    );
    assert_eq!(mock_outcome.billed_total, MicroUsd(0), "a Mock bills 0");
    assert_eq!(
        mock_outcome.refunded,
        MicroUsd(10),
        "the whole reservation refunds"
    );
}

#[test]
fn consumer_relies_on_no_interrupt_path_for_in_flight_runs() {
    let mut gate = AgentRunGate::new();
    let mut ledger = CostLedger::new();
    let handle = gate
        .dispatch(
            &mut ledger,
            tenant(),
            RunId::new("live"),
            MicroUsd(500),
            MicroUsd(1_000),
        )
        .unwrap();

    assert!(
        ledger
            .cancel_unstarted(&tenant(), &RunId::new("live"))
            .is_err(),
        "an in-flight run is NEVER torn down"
    );
    assert_eq!(
        ledger.state_of(&tenant(), &RunId::new("live")),
        Ok(Some(ReservationState::InFlight)),
        "the run is untouched - still in-flight"
    );
    assert_eq!(
        ledger.inflight_interrupt_count(),
        0,
        "0 in-flight interrupts (by construction)"
    );

    handle.settle(&mut ledger, &[]).unwrap();
    assert_eq!(
        ledger.state_of(&tenant(), &RunId::new("live")),
        Ok(Some(ReservationState::Settled))
    );
}

#[test]
fn provider_surface_is_idempotent_on_settle() {
    let mut gate = AgentRunGate::new();
    let mut ledger = CostLedger::new();
    let handle = gate
        .dispatch(
            &mut ledger,
            tenant(),
            RunId::new("run-1"),
            MicroUsd(1_000),
            MicroUsd(5_000),
        )
        .unwrap();
    let units = vec![MeteredUnit {
        unit: "llm.tokens",
        wholesale: MicroUsd(120),
        markup: MicroUsd(30),
    }];
    let first = handle.settle(&mut ledger, &units).unwrap();
    let second = handle.settle(&mut ledger, &units).unwrap();
    assert_eq!(
        first, second,
        "a re-settle returns the SAME outcome (no double-charge)"
    );
}
