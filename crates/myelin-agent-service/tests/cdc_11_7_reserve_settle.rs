//! # The CDC pair for contract 11.7 — reserve/settle, the AGENT-FABRIC consumer half (AG-P14 → P-227)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 11.7
//! (`reserve`/`settle` — the cost gate: *reserve at dispatch → no balance → no run; settle on
//! completion; NEVER interrupt in-flight; integer minor-units; wholesale ≠ markup; fronts every agent
//! run + every CI run + every `SCHEDULE_AND_RUN_JOB`*). Owning architecture: `agent-fabric.md` §5.4.
//!
//! The PROVIDER is `myelin-storage` (the durable [`CostLedger`] + the dispatch-fronting
//! [`AgentRunGate`] — Storage owns the ledger correctness; P-103/P-146). The CONSUMER is **the agent
//! fabric's run loop** — the concrete shape AG-P14 ships: the SKELETON loop fronts EVERY run through
//! reserve-at-dispatch, settles on completion, and has NO path that interrupts an in-flight run. The
//! storage-side CDC (`myelin-storage/tests/cdc_11_7_reserve_settle.rs`) models the consumer
//! ABSTRACTLY; THIS pair pins the surface the agent fabric ACTUALLY depends on (it stops compiling/
//! passing if 11.7's reserve/settle surface drifts under the agent fabric's feet). No duplication —
//! a distinct consumer at a distinct tier.
//!
//! The frozen 11.7 surface this consumer relies on:
//! 1. `gate.dispatch(ledger, tenant, run, estimate, available)` → `InFlightRun` on a funded reserve;
//!    `Err(NoBalance)` on an exhausted wallet (no balance → no run; no handle minted).
//! 2. `in_flight.settle(ledger, &units)` → one cost event per metered unit (wholesale ≠ markup);
//!    the over-reservation refunds; the billed total never exceeds the reservation.
//! 3. There is NO API on the gate or the handle that tears down an in-flight run
//!    (`inflight_interrupt_count` is `0` by construction).

use myelin_storage::agent_run_gate::{AgentRunGate, DispatchError, RunKind};
use myelin_storage::reserve_settle::{CostLedger, MeteredUnit, MinorUnits, ReservationState, RunId};
use myelin_tenancy::TenantId;

fn tenant() -> TenantId {
    TenantId::from_token("01J0ACME")
}

/// **CONSUMER side of 11.7 — the agent fabric's reserve-at-dispatch.** The fabric fronts every run
/// through the gate: a funded reserve mints an in-flight handle (the run starts); an exhausted wallet
/// is refused (no balance → no run; the run NEVER starts). The fabric relies on this exact surface —
/// it never reaches around the gate to start a run, so the runaway self-limiter is
/// correct-by-construction.
#[test]
fn consumer_reserve_at_dispatch_no_balance_no_run() {
    let mut gate = AgentRunGate::new();
    let mut ledger = CostLedger::new();

    // Funded → fronted, in-flight, labelled AgentRun (the fabric's run kind).
    let handle = gate
        .dispatch(&mut ledger, tenant(), RunId::new("run-funded"), MinorUnits(100), MinorUnits(1_000))
        .expect("a funded run is fronted");
    assert_eq!(handle.kind(), RunKind::AgentRun);
    assert_eq!(
        ledger.state_of(&tenant(), &RunId::new("run-funded")),
        Some(ReservationState::InFlight)
    );

    // Exhausted wallet → refused; NO handle; NO reservation row (the run never started).
    let err = gate
        .dispatch(&mut ledger, tenant(), RunId::new("run-broke"), MinorUnits(9_000), MinorUnits(10))
        .expect_err("an exhausted wallet refuses the run");
    assert!(matches!(err, DispatchError::NoBalance { .. }), "no balance → no run: {err}");
    assert!(
        ledger.state_of(&tenant(), &RunId::new("run-broke")).is_none(),
        "a refused run leaves NO reservation — it never started"
    );
    assert_eq!(gate.reserve_refusals(), 1, "the gate counted the refusal (the AG-D11 telemetry)");
}

/// **CONSUMER side of 11.7 — settle-on-completion through the in-flight handle.** The fabric settles a
/// completed run with its actual metered units: one cost event per unit, wholesale ≠ markup recorded
/// distinctly, the over-reservation refunded, the billed total capped at the reservation. The fabric
/// depends on this surface to bill correctly (and on the SKELETON/Mock path it bills ZERO units → the
/// whole reservation refunds, the gate's balanced-ledger property).
#[test]
fn consumer_settle_on_completion_one_event_per_unit_with_split() {
    let mut gate = AgentRunGate::new();
    let mut ledger = CostLedger::new();
    let handle = gate
        .dispatch(&mut ledger, tenant(), RunId::new("run-1"), MinorUnits(1_000), MinorUnits(5_000))
        .unwrap();

    // A run that metered two units (the LlmAgentRuntime shape, AG-P25): one cost event per unit.
    let units = vec![
        MeteredUnit { unit: "llm.tokens", wholesale: MinorUnits(120), markup: MinorUnits(30) },
        MeteredUnit { unit: "ci.minute", wholesale: MinorUnits(200), markup: MinorUnits(50) },
    ];
    let outcome = handle.settle(&mut ledger, &units).expect("the run settles");
    assert_eq!(outcome.cost_events.len(), 2, "one cost event per metered unit");
    assert_ne!(
        outcome.cost_events[0].wholesale, outcome.cost_events[0].markup,
        "wholesale ≠ markup recorded distinctly (never conflated)"
    );
    assert_eq!(outcome.billed_total, MinorUnits(400));
    assert_eq!(outcome.refunded, MinorUnits(600), "the over-reservation refunds");
    assert_eq!(
        ledger.state_of(&tenant(), &RunId::new("run-1")),
        Some(ReservationState::Settled)
    );

    // The SKELETON/Mock shape: a run that meters ZERO units settles the whole reservation as refund
    // (reserved == settled; billed 0). This is the property the agent-fabric balanced-ledger gate
    // reads — the Mock metering ZERO is CORRECT, not a floor.
    let zero = gate
        .dispatch(&mut ledger, tenant(), RunId::new("run-mock"), MinorUnits(10), MinorUnits(5_000))
        .unwrap();
    let mock_outcome = zero.settle(&mut ledger, &[]).expect("a zero-cost run settles");
    assert_eq!(mock_outcome.cost_events.len(), 0, "a Mock meters zero units");
    assert_eq!(mock_outcome.billed_total, MinorUnits(0), "a Mock bills 0");
    assert_eq!(mock_outcome.refunded, MinorUnits(10), "the whole reservation refunds");
}

/// **CONSUMER side of 11.7 — there is NO interrupt path (the never-interrupt-in-flight invariant the
/// fabric relies on).** The agent fabric NEVER interrupts an in-flight run because the provider gives
/// it no way to: the gate exposes no tear-down method, and the ledger's only teardown
/// (`cancel_unstarted`) is structurally barred from an in-flight row. The fabric's runaway
/// self-limiter (AG-D11) depends on EXACTLY this — a refusal of a NEXT run can never reach into a
/// RUNNING one.
#[test]
fn consumer_relies_on_no_interrupt_path_for_in_flight_runs() {
    let mut gate = AgentRunGate::new();
    let mut ledger = CostLedger::new();
    let handle = gate
        .dispatch(&mut ledger, tenant(), RunId::new("live"), MinorUnits(500), MinorUnits(1_000))
        .unwrap();

    // The ONLY teardown the ledger has refuses an in-flight row (the fabric cannot reach around it).
    assert!(
        ledger.cancel_unstarted(&tenant(), &RunId::new("live")).is_err(),
        "an in-flight run is NEVER torn down"
    );
    assert_eq!(
        ledger.state_of(&tenant(), &RunId::new("live")),
        Some(ReservationState::InFlight),
        "the run is untouched — still in-flight"
    );
    assert_eq!(ledger.inflight_interrupt_count(), 0, "0 in-flight interrupts (by construction)");

    // The run still settles normally on its OWN completion (it kept running).
    handle.settle(&mut ledger, &[]).unwrap();
    assert_eq!(
        ledger.state_of(&tenant(), &RunId::new("live")),
        Some(ReservationState::Settled)
    );
}

/// **PROVIDER side of 11.7 — the frozen surface the consumer pins above is implemented.** A funded
/// dispatch mints a handle; an unfunded one does not; the settle is idempotent (a double-completion
/// never double-charges — the surface the fabric's at-least-once delivery relies on).
#[test]
fn provider_surface_is_idempotent_on_settle() {
    let mut gate = AgentRunGate::new();
    let mut ledger = CostLedger::new();
    let handle = gate
        .dispatch(&mut ledger, tenant(), RunId::new("run-1"), MinorUnits(1_000), MinorUnits(5_000))
        .unwrap();
    let units = vec![MeteredUnit { unit: "llm.tokens", wholesale: MinorUnits(120), markup: MinorUnits(30) }];
    let first = handle.settle(&mut ledger, &units).unwrap();
    let second = handle.settle(&mut ledger, &units).unwrap();
    assert_eq!(first, second, "a re-settle returns the SAME outcome (no double-charge)");
}
