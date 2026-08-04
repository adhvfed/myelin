use myelin_storage::{
    AgentRunGate, CostLedger, DispatchError, MeteredUnit, MicroUsd, ReservationState,
    ReserveError, RunId, RunKind, SettleError,
};
use myelin_tenancy::TenantId;

struct RunDispatcher {
    ledger: CostLedger,
    tenant: TenantId,
}

impl RunDispatcher {
    fn boot(tenant: TenantId) -> RunDispatcher {
        RunDispatcher {
            ledger: CostLedger::new(),
            tenant,
        }
    }

    fn dispatch(
        &mut self,
        run: RunId,
        estimate: MicroUsd,
        wallet_balance: MicroUsd,
    ) -> Result<(), ReserveError> {
        self.ledger
            .reserve(self.tenant.clone(), run.clone(), estimate, wallet_balance)?;
        self.ledger
            .begin(&self.tenant, &run)
            .expect("a freshly-reserved run begins");
        Ok(())
    }

    fn complete(
        &mut self,
        run: &RunId,
        units: &[MeteredUnit],
    ) -> Result<myelin_storage::SettleOutcome, SettleError> {
        self.ledger.settle(&self.tenant, run, units)
    }
}

#[test]
fn dispatcher_reserves_then_settles_one_event_per_unit() {
    let tenant = TenantId::from_token("01J0ACME");
    let mut dispatcher = RunDispatcher::boot(tenant.clone());
    let run = RunId::new("01J0RUN_AGENT");

    dispatcher
        .dispatch(run.clone(), MicroUsd(1_000), MicroUsd(5_000))
        .expect("a funded dispatch reserves");

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
    let outcome = dispatcher.complete(&run, &units).expect("the run settles");

    assert_eq!(outcome.cost_events.len(), 2);
    assert_eq!(outcome.cost_events[0].wholesale, MicroUsd(120));
    assert_eq!(outcome.cost_events[0].markup, MicroUsd(30));
    assert_eq!(outcome.billed_total, MicroUsd(400));
    assert_eq!(outcome.refunded, MicroUsd(600));
}

#[test]
fn dispatch_refused_on_no_balance() {
    let tenant = TenantId::from_token("01J0ACME");
    let mut dispatcher = RunDispatcher::boot(tenant.clone());
    let run = RunId::new("01J0RUN_BROKE");
    let err = dispatcher
        .dispatch(run.clone(), MicroUsd(9_000), MicroUsd(100))
        .expect_err("an unfunded dispatch is refused");
    assert!(matches!(err, ReserveError::InsufficientBalance { .. }));
    assert!(
        dispatcher.ledger.state_of(&tenant, &run).is_none(),
        "a refused dispatch leaves NO reservation - the run never started"
    );
}

#[test]
fn in_flight_run_is_never_interrupted() {
    let tenant = TenantId::from_token("01J0ACME");
    let mut dispatcher = RunDispatcher::boot(tenant.clone());
    let run = RunId::new("01J0RUN_AGENT");
    dispatcher
        .dispatch(run.clone(), MicroUsd(500), MicroUsd(1_000))
        .unwrap();
    let err = dispatcher.ledger.cancel_unstarted(&tenant, &run);
    assert!(err.is_err(), "an in-flight run is never cancelled");
    assert_eq!(
        dispatcher.ledger.state_of(&tenant, &run),
        Some(ReservationState::InFlight)
    );
    assert_eq!(
        dispatcher.ledger.inflight_interrupt_count(),
        0,
        "no in-flight reservation was ever interrupted (the headline zero)"
    );
}

#[test]
fn gate_fronts_every_run_reserve_then_settle_through_the_handle() {
    let tenant = TenantId::from_token("01J0ACME");
    let mut gate = AgentRunGate::new();
    let mut ledger = CostLedger::new();
    let run = RunId::new("01J0RUN_AGENT");

    let handle = gate
        .dispatch(
            &mut ledger,
            tenant.clone(),
            run.clone(),
            MicroUsd(1_000),
            MicroUsd(5_000),
        )
        .expect("a funded dispatch fronts the run");
    assert_eq!(handle.kind(), RunKind::AgentRun);
    assert_eq!(
        ledger.state_of(&tenant, &run),
        Some(ReservationState::InFlight)
    );

    let outcome = handle
        .settle(
            &mut ledger,
            &[MeteredUnit {
                unit: "llm.tokens",
                wholesale: MicroUsd(120),
                markup: MicroUsd(30),
            }],
        )
        .expect("the run settles through its handle");
    assert_eq!(outcome.cost_events.len(), 1);
    assert_eq!(outcome.billed_total, MicroUsd(150));
    assert_eq!(outcome.refunded, MicroUsd(850));
}

#[test]
fn gate_refuses_dispatch_on_no_balance_no_handle_minted() {
    let tenant = TenantId::from_token("01J0ACME");
    let mut gate = AgentRunGate::new();
    let mut ledger = CostLedger::new();
    let run = RunId::new("01J0RUN_BROKE");

    let err = gate
        .dispatch(
            &mut ledger,
            tenant.clone(),
            run.clone(),
            MicroUsd(9_000),
            MicroUsd(100),
        )
        .expect_err("an unfunded dispatch is refused");
    assert!(matches!(err, DispatchError::NoBalance { .. }));
    assert!(
        ledger.state_of(&tenant, &run).is_none(),
        "no handle, no reservation - the run never started"
    );
    assert_eq!(gate.reserve_refusals(), 1);
}
