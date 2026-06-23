//! # The Issues CDC pair + e2e for contract 11.7 — reserve/settle on every spend-bearing run (ISS-P24 / P-391)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 11.7
//! (*Reserve/settle cost gate* — reserve at dispatch, no balance → no start; settle on completion,
//! never interrupt in-flight; integer minor-units; wholesale ≠ markup; fronts **every agent run and
//! every CI run** + every `SCHEDULE_AND_RUN_JOB`; the wallet is the SAME for CI + agent runs) and row
//! 9.5 (*the workflow↔agent mapping — reserve/settle = the bookends*). Owning architecture:
//! `planning/04-subsystem-architectures/issue-tracker/architecture/03-events-contracts-and-glue.md`
//! §9 (*reserve/settle — spend-bearing agent work; Issues does NOT own the wallet, it consumes the
//! gate*).
//!
//! ## The contract this pair pins (reserve == settle for a completed Issues run; no balance → no start)
//! Row 11.7 (Issues slice) is the seam between the side that **PRODUCES** the wallet balance + the
//! reserve/settle ledger (the **PROVIDER** — the Storage-owned `AgentRunGate` over the durable
//! `CostLedger`, the SAME wallet CI draws from) and the side that **CONSUMES** the gate to front a
//! spend-bearing Issues agent run (the **CONSUMER** — `myelin_issues::IssueSpendGate` /
//! [`spend_bearing_run`], the §9 bookends). The frozen behaviour both sides agree on:
//!
//! - the PROVIDER (the ledger) reserves at dispatch (no balance → no row, no run), holds the run
//!   in-flight (never interrupts it), and settles on completion (one cost event per metered unit,
//!   wholesale ≠ markup, refunding the over-reservation; the reserve is the billing cap);
//! - the CONSUMER (the Issues run lifecycle) reserves the run's estimate against the wallet BEFORE the
//!   agent brain runs (no balance → the brain NEVER starts), runs the brain to completion behind the
//!   in-flight handle, and settles with the brain's actual metered cost — emitting the balanced-wallet
//!   green artifact (`reserved == billed + refunded`, the wallet nets to 0 over the completed run).
//!
//! This is the dedicated 11.7 Issues provider+consumer pair + the chained-mutation e2e the ISS-P24
//! TESTS field names (dispatch a spend-bearing run → reserve → complete → settle → assert balanced).
//! The ledger's own gap/never-interrupt/one-event-per-unit invariants are pinned in
//! `myelin-storage::reserve_settle` + `agent_run_gate` (the shared mechanism, P-103/P-146); HERE we
//! pin the Issues-side CONSUMPTION of the SAME gate.

use myelin_issues::{spend_bearing_run, IssueRunKind, IssueSpendGate, SpendError};
use myelin_storage::reserve_settle::{
    CostLedger, MeteredUnit, MinorUnits, ReservationState, RunId,
};
use myelin_tenancy::TenantId;

fn tenant() -> TenantId {
    TenantId::from_token("01J0ACME")
}

fn run(n: u32) -> RunId {
    RunId::new(format!("01J0ISSUE_RUN_{n}"))
}

/// The actual metered cost the agent brain reports on completion (the SAME `agent.effect` dimension
/// CI's compute + the agent-fabric effect meter bill — the wallet is shared, 11.7).
fn brain_cost(wholesale: u64, markup: u64) -> Vec<MeteredUnit> {
    vec![MeteredUnit {
        unit: "agent.effect",
        wholesale: MinorUnits(wholesale),
        markup: MinorUnits(markup),
    }]
}

/// **PROVIDER side of 11.7 (Issues) — the wallet balance the Commercial control-plane reports.** The
/// provider's promise: this is the SAME wallet CI runs draw from (a single shared balance, not an
/// Issues-private one). The consumer reserves against exactly this number.
fn provider_wallet_balance() -> MinorUnits {
    MinorUnits(5_000)
}

// ───────────────────────── the chained-mutation e2e (the ISS-P24 headline) ───────────────────────

/// **E2E (the ISS-P24 chained mutation): dispatch a spend-bearing Issues run → RESERVE → complete →
/// SETTLE → assert the wallet BALANCED.** The whole §9 / 9.5 bookend round-trip on the SHARED wallet:
/// reserve the run's estimate (no balance → no start), run the brain to completion behind the
/// in-flight handle, settle with the brain's actual metered cost. The green artifact is the balanced
/// wallet (`reserved == billed + refunded`, the wallet nets to 0 over the completed run).
#[test]
fn e2e_dispatch_reserve_complete_settle_balances_the_wallet() {
    let mut gate = IssueSpendGate::new();
    let mut ledger = CostLedger::new();

    // dispatch a triage agent run: reserve an upper bound of 1000 against the SHARED wallet (5000).
    // the brain then runs to completion and reports its ACTUAL cost (wholesale 250 + markup 150 = 400).
    let signal = spend_bearing_run(
        &mut gate,
        &mut ledger,
        tenant(),
        run(1),
        IssueRunKind::Triage,
        MinorUnits(1_000),
        provider_wallet_balance(),
        || brain_cost(250, 150),
    )
    .expect("a funded spend-bearing run completes end to end");

    // the wallet BALANCED: 1000 reserved = 400 billed + 600 refunded (the reserve is fully accounted —
    // none leaked; the wallet nets to 0 over the completed run).
    assert_eq!(signal.reserved, MinorUnits(1_000));
    assert_eq!(signal.billed, MinorUnits(400), "wholesale 250 + markup 150");
    assert_eq!(
        signal.refunded,
        MinorUnits(600),
        "the over-reservation refunds"
    );
    assert_eq!(signal.cost_events, 1, "one cost event per metered unit");
    assert_eq!(signal.inflight_interrupt_count, 0, "the headline zero");
    assert!(
        signal.is_green(),
        "reserve == settle for the completed run: {signal:?}"
    );

    // the run is Settled in the SHARED ledger (it ran to completion) + recorded its cost event.
    assert_eq!(
        ledger.state_of(&tenant(), &run(1)),
        Some(ReservationState::Settled),
        "a completed run settles"
    );
    let events = ledger.cost_events_for(&tenant(), &run(1));
    assert_eq!(events.len(), 1, "exactly one cost event recorded");
    // wholesale ≠ markup kept distinct (C-1 — never conflated into one number).
    assert_eq!(events[0].wholesale, MinorUnits(250));
    assert_eq!(events[0].markup, MinorUnits(150));
    assert_ne!(events[0].wholesale, events[0].markup, "wholesale ≠ markup");
}

// ───────────────────────── PROVIDER + CONSUMER: no balance → no start ─────────────────────────────

/// **PROVIDER + CONSUMER agree on the §9 floor: no balance → no start (the agent brain NEVER runs).**
/// The provider's wallet cannot afford the run; the consumer's reserve is refused; the brain closure
/// never fires; no reservation row is left behind. The runaway self-limiter (AG-D11) on Issues runs.
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
        MinorUnits(9_000),
        MinorUnits(100), // the provider's wallet cannot afford the run
        || {
            brain_ran = true; // MUST NOT execute — no balance → no start
            brain_cost(10, 0)
        },
    )
    .expect_err("an over-budget Issues run is refused");

    assert_eq!(
        err,
        SpendError::NoBalance {
            requested: MinorUnits(9_000),
            available: MinorUnits(100),
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

// ───────────────────────── the shared wallet: CI + agent runs net the same balance ───────────────

/// **The wallet is SHARED (11.7): two Issues agent runs draw down the SAME balance the gate fronts.**
/// A surge of Issues runs against a draining wallet admits exactly the funded runs and refuses the
/// rest — the SAME shared-wallet behaviour a CI surge sees (the gate fronts every kind identically).
#[test]
fn the_shared_wallet_drains_across_successive_issues_runs() {
    let mut gate = IssueSpendGate::new();
    let mut ledger = CostLedger::new();
    // the shared wallet affords exactly 2 runs of 100 each (balance 250 with refunds released on
    // settle); a third run at 100 against the remaining 50 is refused.
    let wallet = MinorUnits(250);

    // run 1: reserve 100, settle 100 (no refund) → 150 remaining.
    let s1 = spend_bearing_run(
        &mut gate,
        &mut ledger,
        tenant(),
        run(1),
        IssueRunKind::Triage,
        MinorUnits(100),
        wallet,
        || brain_cost(80, 20),
    )
    .expect("run 1 funded");
    assert!(s1.is_green());

    // run 2: reserve 100 against the remaining 150 → funded; settle 100 → 50 remaining.
    let s2 = spend_bearing_run(
        &mut gate,
        &mut ledger,
        tenant(),
        run(2),
        IssueRunKind::Forecast,
        MinorUnits(100),
        MinorUnits(150),
        || brain_cost(80, 20),
    )
    .expect("run 2 funded");
    assert!(s2.is_green());

    // run 3: reserve 100 against the remaining 50 → refused (no balance → no start).
    let err = spend_bearing_run(
        &mut gate,
        &mut ledger,
        tenant(),
        run(3),
        IssueRunKind::SlaDraft,
        MinorUnits(100),
        MinorUnits(50),
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
    // no in-flight run was ever interrupted by the refusal (the never-interrupt invariant).
    assert_eq!(ledger.inflight_interrupt_count(), 0);
}
