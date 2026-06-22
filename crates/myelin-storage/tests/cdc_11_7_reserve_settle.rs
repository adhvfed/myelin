//! Contract 11.7 CDC pair — the reserve/settle cost gate + the durable per-tenant ledger.
//!
//! The prompt requires "the provider+consumer pair for 11.7 (an agent/CI run-dispatcher)".
//! This is the consumer-driven contract test: the PROVIDER is `myelin-storage` (the durable
//! [`CostLedger`] this prompt ships — Storage owns the ledger correctness); the CONSUMER is
//! a run-dispatcher (modelled here as a tiny `RunDispatcher`, the shape Agent's `AgentRuntime`
//! and `SCHEDULE_AND_RUN_JOB` (P-ST-19) and CI's job-dispatch (M4) take) that fronts every run
//! with reserve-at-dispatch / settle-on-completion and NEVER interrupts an in-flight run.
//!
//! The test pins the frozen call shape every dispatcher relies on — if 11.7's surface drifts
//! (reserve refuses on no-balance; settle records one cost event per metered unit with the
//! wholesale/markup split; an in-flight run is never torn down), this stops compiling/passing.

use myelin_storage::{
    AgentRunGate, CostLedger, DispatchError, MeteredUnit, MinorUnits, ReservationState,
    ReserveError, RunId, RunKind, SettleError,
};
use myelin_tenancy::TenantId;

/// A consumer of 11.7: a run-dispatcher that fronts every dispatch with the cost gate. This
/// is the shape Agent (P-ST-19) and CI (M4) take — it does not re-implement the ledger; it
/// drives the Storage-owned [`CostLedger`].
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

    /// **Reserve-at-dispatch.** The dispatcher reserves an upper-bound cost against the
    /// wallet balance BEFORE the run starts; no balance → no run (the run is not dispatched).
    fn dispatch(
        &mut self,
        run: RunId,
        estimate: MinorUnits,
        wallet_balance: MinorUnits,
    ) -> Result<(), ReserveError> {
        self.ledger
            .reserve(self.tenant.clone(), run.clone(), estimate, wallet_balance)?;
        // The run begins executing — from here it is NEVER interrupted.
        self.ledger
            .begin(&self.tenant, &run)
            .expect("a freshly-reserved run begins");
        Ok(())
    }

    /// **Settle-on-completion.** When the run finishes, the dispatcher settles to the actual
    /// metered units (one cost event each).
    fn complete(
        &mut self,
        run: &RunId,
        units: &[MeteredUnit],
    ) -> Result<myelin_storage::SettleOutcome, SettleError> {
        self.ledger.settle(&self.tenant, run, units)
    }
}

/// The provider+consumer happy path: dispatch reserves, the run completes, settle records
/// one cost event per metered unit with the wholesale/markup split, the over-reservation
/// refunds.
#[test]
fn dispatcher_reserves_then_settles_one_event_per_unit() {
    let tenant = TenantId::from_token("01J0ACME");
    let mut dispatcher = RunDispatcher::boot(tenant.clone());
    let run = RunId::new("01J0RUN_AGENT");

    dispatcher
        .dispatch(run.clone(), MinorUnits(1_000), MinorUnits(5_000))
        .expect("a funded dispatch reserves");

    let units = vec![
        MeteredUnit {
            unit: "llm.tokens",
            wholesale: MinorUnits(120),
            markup: MinorUnits(30),
        },
        MeteredUnit {
            unit: "ci.minute",
            wholesale: MinorUnits(200),
            markup: MinorUnits(50),
        },
    ];
    let outcome = dispatcher.complete(&run, &units).expect("the run settles");

    // One cost event per metered unit; wholesale ≠ markup recorded distinctly.
    assert_eq!(outcome.cost_events.len(), 2);
    assert_eq!(outcome.cost_events[0].wholesale, MinorUnits(120));
    assert_eq!(outcome.cost_events[0].markup, MinorUnits(30));
    assert_eq!(outcome.billed_total, MinorUnits(400));
    assert_eq!(outcome.refunded, MinorUnits(600));
}

/// **No balance → no run** (the runaway self-limiter, AG-D11): a dispatch whose estimate
/// exceeds the wallet balance is REFUSED — the run never dispatches.
#[test]
fn dispatch_refused_on_no_balance() {
    let tenant = TenantId::from_token("01J0ACME");
    let mut dispatcher = RunDispatcher::boot(tenant.clone());
    let run = RunId::new("01J0RUN_BROKE");
    let err = dispatcher
        .dispatch(run.clone(), MinorUnits(9_000), MinorUnits(100))
        .expect_err("an unfunded dispatch is refused");
    assert!(matches!(err, ReserveError::InsufficientBalance { .. }));
    assert!(
        dispatcher.ledger.state_of(&tenant, &run).is_none(),
        "a refused dispatch leaves NO reservation — the run never started"
    );
}

/// **NEVER interrupt in-flight** (the master invariant): once a run is in-flight, the
/// dispatcher cannot tear it down — `cancel_unstarted` refuses, the run keeps running, the
/// interrupt counter stays 0.
#[test]
fn in_flight_run_is_never_interrupted() {
    let tenant = TenantId::from_token("01J0ACME");
    let mut dispatcher = RunDispatcher::boot(tenant.clone());
    let run = RunId::new("01J0RUN_AGENT");
    dispatcher
        .dispatch(run.clone(), MinorUnits(500), MinorUnits(1_000))
        .unwrap();
    // The run is in-flight — an attempt to cancel it is refused.
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

// ───────────────────────── 11.7 LIVE CONSUMER HALF (P-ST-19 / P-146) ─────────────────────────
//
// P-ST-19 ships the LIVE consumer of 11.7 — the Storage-owned [`AgentRunGate`] that fronts every
// `AgentRuntime` run + every `SCHEDULE_AND_RUN_JOB`. These CDC cases pin the gate's frozen
// dispatch surface (the shape the agent fabric AG-P4/P-216 and CI's M4 dispatcher take): reserve-
// at-dispatch mints a move-only in-flight handle; no balance → no handle; the in-flight handle is
// the only settle path; the gate exposes NO interrupt API. PROVIDER = myelin-storage (the gate +
// ledger). CONSUMER = the run-dispatcher modelled below.

/// The CONSUMER side: the agent fabric / CI dispatcher holds an [`AgentRunGate`] + the durable
/// ledger and fronts every run through the gate. It cannot start a run without reserving first
/// (the gate mints the in-flight handle only on a funded reserve) — fronting is
/// correct-by-construction, not a convention.
#[test]
fn gate_fronts_every_run_reserve_then_settle_through_the_handle() {
    let tenant = TenantId::from_token("01J0ACME");
    let mut gate = AgentRunGate::new();
    let mut ledger = CostLedger::new();
    let run = RunId::new("01J0RUN_AGENT");

    // Reserve-at-dispatch mints the in-flight handle (the ONLY way to start a run).
    let handle = gate
        .dispatch(
            &mut ledger,
            tenant.clone(),
            run.clone(),
            MinorUnits(1_000),
            MinorUnits(5_000),
        )
        .expect("a funded dispatch fronts the run");
    assert_eq!(handle.kind(), RunKind::AgentRun);
    assert_eq!(
        ledger.state_of(&tenant, &run),
        Some(ReservationState::InFlight)
    );

    // Settle-on-completion through the handle: one cost event per metered unit, wholesale ≠ markup.
    let outcome = handle
        .settle(
            &mut ledger,
            &[MeteredUnit {
                unit: "llm.tokens",
                wholesale: MinorUnits(120),
                markup: MinorUnits(30),
            }],
        )
        .expect("the run settles through its handle");
    assert_eq!(outcome.cost_events.len(), 1);
    assert_eq!(outcome.billed_total, MinorUnits(150));
    assert_eq!(outcome.refunded, MinorUnits(850));
}

/// No balance → no run: the gate refuses the dispatch and mints NO handle — the run never starts
/// (the agent fabric has nothing to run). The runaway self-limiter (AG-D11) at the consumer
/// boundary.
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
            MinorUnits(9_000),
            MinorUnits(100),
        )
        .expect_err("an unfunded dispatch is refused");
    assert!(matches!(err, DispatchError::NoBalance { .. }));
    assert!(
        ledger.state_of(&tenant, &run).is_none(),
        "no handle, no reservation — the run never started"
    );
    assert_eq!(gate.reserve_refusals(), 1);
}
