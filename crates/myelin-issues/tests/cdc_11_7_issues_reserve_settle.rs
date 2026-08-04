use myelin_issues::{spend_bearing_run, IssueRunKind, IssueSpendGate, SpendError};
use myelin_storage::reserve_settle::{
    CostLedger, MeteredUnit, MicroUsd, ReservationState, RunId,
};
use myelin_tenancy::TenantId;

fn tenant() -> TenantId {
    TenantId::from_token("01J0ACME")
}

fn run(n: u32) -> RunId {
    RunId::new(format!("01J0ISSUE_RUN_{n}"))
}

fn brain_cost(wholesale: u64, markup: u64) -> Vec<MeteredUnit> {
    vec![MeteredUnit {
        unit: "agent.effect",
        wholesale: MicroUsd(wholesale),
        markup: MicroUsd(markup),
    }]
}

fn provider_wallet_balance() -> MicroUsd {
    MicroUsd(5_000)
}

#[test]
fn e2e_dispatch_reserve_complete_settle_balances_the_wallet() {
    let mut gate = IssueSpendGate::new();
    let mut ledger = CostLedger::new();

    let signal = spend_bearing_run(
        &mut gate,
        &mut ledger,
        tenant(),
        run(1),
        IssueRunKind::Triage,
        MicroUsd(1_000),
        provider_wallet_balance(),
        || brain_cost(250, 150),
    )
    .expect("a funded spend-bearing run completes end to end");

    assert_eq!(signal.reserved, MicroUsd(1_000));
    assert_eq!(signal.billed, MicroUsd(400), "wholesale 250 + markup 150");
    assert_eq!(
        signal.refunded,
        MicroUsd(600),
        "the over-reservation refunds"
    );
    assert_eq!(signal.cost_events, 1, "one cost event per metered unit");
    assert_eq!(signal.inflight_interrupt_count, 0, "the headline zero");
    assert!(
        signal.is_green(),
        "reserve == settle for the completed run: {signal:?}"
    );

    assert_eq!(
        ledger.state_of(&tenant(), &run(1)),
        Some(ReservationState::Settled),
        "a completed run settles"
    );
    let events = ledger.cost_events_for(&tenant(), &run(1));
    assert_eq!(events.len(), 1, "exactly one cost event recorded");
    assert_eq!(events[0].wholesale, MicroUsd(250));
    assert_eq!(events[0].markup, MicroUsd(150));
    assert_ne!(events[0].wholesale, events[0].markup, "wholesale ≠ markup");
}

#[test]
fn no_balance_means_the_issues_run_never_starts() {
    let mut gate = IssueSpendGate::new();
    let mut ledger = CostLedger::new();
    let mut brain_ran = false;

    let err = spend_bearing_run(
        &mut gate,
        &mut ledger,
        tenant(),
        run(1),
        IssueRunKind::Forecast,
        MicroUsd(9_000),
        MicroUsd(100),
        || {
            brain_ran = true;
            brain_cost(10, 0)
        },
    )
    .expect_err("an over-budget Issues run is refused");

    assert_eq!(
        err,
        SpendError::NoBalance {
            requested: MicroUsd(9_000),
            available: MicroUsd(100),
        }
    );
    assert!(
        !brain_ran,
        "no balance → no start: the agent brain NEVER ran"
    );
    assert!(
        ledger.state_of(&tenant(), &run(1)).is_none(),
        "a refused run leaves NO reservation in the SHARED ledger"
    );
    assert_eq!(
        gate.reserve_refusals(),
        1,
        "the refusal is counted (AG-D11)"
    );
    assert_eq!(gate.runs_dispatched(), 0);
}

#[test]
fn the_shared_wallet_drains_across_successive_issues_runs() {
    let mut gate = IssueSpendGate::new();
    let mut ledger = CostLedger::new();
    let wallet = MicroUsd(250);

    let s1 = spend_bearing_run(
        &mut gate,
        &mut ledger,
        tenant(),
        run(1),
        IssueRunKind::Triage,
        MicroUsd(100),
        wallet,
        || brain_cost(80, 20),
    )
    .expect("run 1 funded");
    assert!(s1.is_green());

    let s2 = spend_bearing_run(
        &mut gate,
        &mut ledger,
        tenant(),
        run(2),
        IssueRunKind::Forecast,
        MicroUsd(100),
        MicroUsd(150),
        || brain_cost(80, 20),
    )
    .expect("run 2 funded");
    assert!(s2.is_green());

    let err = spend_bearing_run(
        &mut gate,
        &mut ledger,
        tenant(),
        run(3),
        IssueRunKind::SlaDraft,
        MicroUsd(100),
        MicroUsd(50),
        || brain_cost(80, 20),
    )
    .expect_err("run 3 over the drained wallet is refused");
    assert!(matches!(err, SpendError::NoBalance { .. }));

    assert_eq!(gate.runs_dispatched(), 2, "exactly the funded runs ran");
    assert_eq!(
        gate.reserve_refusals(),
        1,
        "the over-budget run was refused"
    );
    assert_eq!(ledger.inflight_interrupt_count(), 0);
}
