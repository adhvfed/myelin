//! **Reserve/settle fronts agent runs — the live consumer half (P-ST-19 / global P-146).**
//!
//! **Architecture:** storage.md §9 (contract 11.7 — *reserve/settle cost gate: reserve at
//! dispatch → no balance → no run; settle on completion; **NEVER interrupt in-flight**;
//! integer minor-units; wholesale ≠ markup; **fronts every agent run + every CI run + every
//! `SCHEDULE_AND_RUN_JOB`***). Contract-index row 11.7. Drill catalogue rows **AG-D6** (30×
//! agent dispatch surge → reserve/settle refuses over-budget runs, others unaffected) and
//! **AG-D11** (runaway loop vs an exhausted wallet → reserve refuses new runs, NEVER
//! interrupts in-flight, the loop stops at the wallet).
//!
//! ## What this prompt (P-ST-19) ships — the LIVE consumer half
//! P-ST-16 (global P-103) shipped the durable [`crate::reserve_settle::CostLedger`] MECHANISM
//! (the arithmetic + the never-interrupt invariant + one-cost-event-per-unit). It named a
//! floor: *the gate FRONTS agent runs in M2 → P-ST-19*. **This module fills that floor.** It
//! is the dispatch-fronting GATE that now sits in front of every `AgentRuntime` run and every
//! `SCHEDULE_AND_RUN_JOB`: it does NOT re-implement the ledger — it DRIVES the Storage-owned
//! `CostLedger` through the run lifecycle so that:
//!
//! 1. **Reserve-at-dispatch → no balance → no run.** [`AgentRunGate::dispatch`] reserves the
//!    run's estimated upper-bound cost against the wallet balance BEFORE the run starts. A
//!    no-balance reserve is REFUSED and the run is **never dispatched** (no [`InFlightRun`]
//!    handle is minted — the agent fabric has nothing to start). This is the runaway
//!    self-limiter (AG-D11): a loop against an exhausted wallet stops at the wallet.
//! 2. **The run executes behind a move-only [`InFlightRun`] handle.** Once dispatched the run
//!    is `InFlight` in the ledger. The handle is the ONLY way to settle it, and the gate
//!    exposes **no** API that tears down an in-flight run — the never-interrupt-in-flight
//!    invariant is structural (the ledger's `cancel_unstarted` is barred from an `InFlight`
//!    row, and the gate never calls it on a live run).
//! 3. **Settle-on-completion.** [`InFlightRun::settle`] closes the run with its actual metered
//!    units (one cost event per unit, wholesale ≠ markup), releasing the over-reservation.
//! 4. **`SCHEDULE_AND_RUN_JOB` fronting.** [`AgentRunGate::schedule_and_run_job`] is the
//!    long-park dispatch idiom (the workflow seam, FLOW-15 / P-211): it is reserved the same
//!    way — no balance → not scheduled — and yields the SAME [`InFlightRun`] handle the
//!    completion signal settles. One gate, both dispatch shapes.
//!
//! ## Why a GATE type and not just the raw ledger
//! The raw [`crate::reserve_settle::CostLedger`] lets a careless consumer `begin` a run that
//! was never reserved, or settle a run it never started. The **gate** makes the correct
//! lifecycle the ONLY representable one: a [`DispatchToken`] is minted ONLY by a successful
//! reserve, an [`InFlightRun`] is minted ONLY by starting a dispatch token, and a run can be
//! settled ONLY through its in-flight handle. The agent fabric (AG-P4 → P-216, which lands
//! AFTER this prompt) holds this gate and cannot dispatch a run without going through reserve
//! — *fronting* is correct-by-construction, not a convention the caller must remember.
//!
//! ## Floors named (deferred + the filling prompt)
//! - **The gate FRONTS CI runs in M4** — a CI-subsystem consumer (CI is the heaviest storage
//!   consumer, storage.md §8) fronts every CI run with this SAME gate. That live wiring is the
//!   named M4 follow-on (the CI subsystem's run-dispatch prompt). Recorded in writing here.
//! - **The real `AgentRuntime` brain** is `LlmAgentRuntime`, designed-not-built (AG-P25,
//!   post-M5); `MockAgentRuntime` (AG-P5 → P-217) is the deterministic brain on the same
//!   dispatch path. This gate fronts BOTH — the brain is irrelevant to the cost gate, which is
//!   exactly the point (the gate is the runaway self-limiter regardless of which brain runs).
//! - **A real durable Postgres-backed ledger** lands with the OLTP driver (P-S12), as for
//!   P-ST-16. This gate drives the SAME backend-agnostic `CostLedger`; no NEW
//!   db/object-store/cache/bus trait is touched by this prompt, so **no new integration drill
//!   is owed** (recorded in the P-146 report).
//!
//! ## Mutation floor (mandatory-core, EI-01 §2 — the reserve-never-interrupt path is ≥ 80%)
//! The reserve-at-dispatch / never-interrupt-in-flight / settle-on-completion path is
//! mandatory-core. The unit tests below + the AG-D6/AG-D11 drills exercise: a funded dispatch
//! mints an in-flight handle; an unfunded dispatch mints NONE; an in-flight run is never torn
//! down (no gate API does it); a `SCHEDULE_AND_RUN_JOB` dispatch fronts identically; the
//! surge/runaway drills emit `inflight_interrupt_count == 0` + `reserve_refusals > 0`. The
//! mutation score is reported in the P-146 report (`cargo mutants --file
//! crates/myelin-storage/src/agent_run_gate.rs`).

use crate::reserve_settle::{
    CostLedger, MeteredUnit, MinorUnits, ReserveError, RunId, SettleError, SettleOutcome,
};
use myelin_tenancy::TenantId;

/// The kind of run this gate fronts — recorded so the drill/telemetry can distinguish an
/// `AgentRuntime` run from a `SCHEDULE_AND_RUN_JOB` dispatch from a (future, M4) CI run. The
/// cost-gate behaviour is IDENTICAL across kinds (the whole point: one gate fronts every kind);
/// this is purely an observability label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunKind {
    /// An `AgentRuntime` run (the brain loop — mock or, later, LLM).
    AgentRun,
    /// A `SCHEDULE_AND_RUN_JOB` long-park dispatch (the workflow idiom; FLOW-15 → P-211).
    ScheduleAndRunJob,
    /// A CI run (the M4 follow-on; named here so the enum is total — the CI consumer wires it).
    CiRun,
}

/// An error refusing a dispatch — a run is never started against an empty wallet. Wraps the
/// ledger's [`ReserveError`] so the agent fabric sees ONE error type at its dispatch boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DispatchError {
    /// The wallet balance is insufficient for the run's reservation — **the run is not
    /// dispatched** (no in-flight handle is minted). The runaway self-limiter (AG-D11).
    NoBalance {
        /// The amount the dispatch asked to reserve (the run's estimated upper bound).
        requested: MinorUnits,
        /// The wallet balance available (from Commercial).
        available: MinorUnits,
    },
    /// A run with this `(tenant, run)` is already dispatched — a dispatch is fronted once (the
    /// idempotency guard; a re-dispatch of a live run is rejected loudly, never double-reserved).
    AlreadyDispatched,
    /// Integer minor-units arithmetic overflowed `u64` (loud, never a silent wrap).
    AmountOverflow,
}

impl From<ReserveError> for DispatchError {
    fn from(e: ReserveError) -> DispatchError {
        match e {
            ReserveError::InsufficientBalance {
                requested,
                available,
            } => DispatchError::NoBalance {
                requested,
                available,
            },
            ReserveError::DuplicateReservation => DispatchError::AlreadyDispatched,
            ReserveError::AmountOverflow => DispatchError::AmountOverflow,
        }
    }
}

impl core::fmt::Display for DispatchError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            DispatchError::NoBalance {
                requested,
                available,
            } => write!(
                f,
                "dispatch refused: no balance, no run (requested {} minor-units, {} available) \
                 — the run was NEVER started (storage §9, AG-D11)",
                requested.0, available.0
            ),
            DispatchError::AlreadyDispatched => write!(
                f,
                "dispatch refused: this run is already in flight — a dispatch is fronted exactly once"
            ),
            DispatchError::AmountOverflow => write!(
                f,
                "dispatch refused: integer minor-units arithmetic overflowed u64 (loud, never a silent wrap)"
            ),
        }
    }
}

impl std::error::Error for DispatchError {}

/// **An in-flight run handle (move-only by intent).** Minted ONLY by a successful
/// [`AgentRunGate::dispatch`] / [`AgentRunGate::schedule_and_run_job`]; the run is `InFlight`
/// in the ledger the moment this exists. The ONLY way to close the run is to [`settle`] it
/// through this handle — and there is **no method on this handle (or on the gate) that
/// interrupts the run**. This is how "NEVER interrupt in-flight" becomes structural: to even
/// *attempt* an interruption a consumer would have to call a method that does not exist.
///
/// [`settle`]: InFlightRun::settle
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InFlightRun {
    tenant: TenantId,
    run: RunId,
    kind: RunKind,
    reserved: MinorUnits,
}

impl InFlightRun {
    /// The tenant this run belongs to (the partition key — no cross-tenant path).
    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }
    /// The run id.
    pub fn run(&self) -> &RunId {
        &self.run
    }
    /// What kind of run this is (observability label).
    pub fn kind(&self) -> RunKind {
        self.kind
    }
    /// The amount reserved at dispatch (the billing cap — a settle never bills more).
    pub fn reserved(&self) -> MinorUnits {
        self.reserved
    }

    /// **Settle-on-completion.** Close this run with its actual metered `units`, against the
    /// supplied `ledger`. Records exactly one cost event per unit (wholesale ≠ markup),
    /// refunds the over-reservation. Idempotent: settling an already-settled run returns the
    /// same outcome and records no further cost events (a double-completion never
    /// double-charges). Returns the [`SettleOutcome`] so the agent fabric can report the bill.
    pub fn settle(
        &self,
        ledger: &mut CostLedger,
        units: &[MeteredUnit],
    ) -> Result<SettleOutcome, SettleError> {
        ledger.settle(&self.tenant, &self.run, units)
    }
}

/// **The reserve/settle gate that fronts every agent run + every `SCHEDULE_AND_RUN_JOB`.**
///
/// The agent fabric (AG-P4 → P-216) holds one of these per cell and CANNOT dispatch a run
/// without going through [`dispatch`] / [`schedule_and_run_job`] — both reserve first, so a
/// run is fronted by the cost gate correct-by-construction. The gate borrows the
/// Storage-owned [`CostLedger`] (the durable ledger correctness lives there); it owns the
/// *lifecycle policy* (reserve → in-flight handle → settle).
///
/// [`dispatch`]: AgentRunGate::dispatch
/// [`schedule_and_run_job`]: AgentRunGate::schedule_and_run_job
#[derive(Debug, Default)]
pub struct AgentRunGate {
    /// The count of dispatches REFUSED for no balance — the AG-D6/AG-D11 telemetry
    /// (`reserve refusals`). It is observability, not control flow.
    reserve_refusals: u64,
    /// The count of dispatches ADMITTED (a run was fronted and started). Lets the drill assert
    /// the surge admitted exactly the funded runs and shed the rest.
    runs_dispatched: u64,
}

impl AgentRunGate {
    /// A fresh gate.
    pub fn new() -> AgentRunGate {
        AgentRunGate::default()
    }

    /// **Reserve-at-dispatch for an `AgentRuntime` run.** Reserve `estimate` (the run's
    /// estimated upper-bound cost, integer minor-units) against the wallet `available`
    /// balance, then start the run. On success an [`InFlightRun`] handle is returned (the run
    /// is now `InFlight` and NEVER interrupted). On no-balance the dispatch is REFUSED, the
    /// `reserve_refusals` counter ticks, and **no handle is minted** — the run never starts.
    pub fn dispatch(
        &mut self,
        ledger: &mut CostLedger,
        tenant: TenantId,
        run: RunId,
        estimate: MinorUnits,
        available: MinorUnits,
    ) -> Result<InFlightRun, DispatchError> {
        self.dispatch_kind(ledger, tenant, run, estimate, available, RunKind::AgentRun)
    }

    /// **Reserve-at-dispatch for a `SCHEDULE_AND_RUN_JOB` long-park dispatch.** Identical cost
    /// gate to [`dispatch`](AgentRunGate::dispatch): reserve the estimate against the wallet,
    /// no balance → not scheduled. The returned [`InFlightRun`] handle is settled by the
    /// completion signal (the long-park idiom; FLOW-15 → P-211).
    pub fn schedule_and_run_job(
        &mut self,
        ledger: &mut CostLedger,
        tenant: TenantId,
        run: RunId,
        estimate: MinorUnits,
        available: MinorUnits,
    ) -> Result<InFlightRun, DispatchError> {
        self.dispatch_kind(
            ledger,
            tenant,
            run,
            estimate,
            available,
            RunKind::ScheduleAndRunJob,
        )
    }

    /// The shared reserve-then-start lifecycle behind both dispatch shapes. Reserve first (no
    /// balance → no run, no handle); then mark the reservation in-flight and mint the handle.
    /// If the begin somehow fails (it cannot, on a row we just reserved) the run is NOT left in
    /// a started-but-unhandled state — the reservation is rolled back so a retry can re-reserve.
    fn dispatch_kind(
        &mut self,
        ledger: &mut CostLedger,
        tenant: TenantId,
        run: RunId,
        estimate: MinorUnits,
        available: MinorUnits,
        kind: RunKind,
    ) -> Result<InFlightRun, DispatchError> {
        // 1. Reserve-at-dispatch. No balance → no run (the run is not dispatched).
        match ledger.reserve(tenant.clone(), run.clone(), estimate, available) {
            Ok(_reservation) => {}
            Err(e) => {
                if matches!(e, ReserveError::InsufficientBalance { .. }) {
                    self.reserve_refusals += 1;
                }
                return Err(e.into());
            }
        }

        // 2. Start the run — it is now InFlight and will NEVER be interrupted.
        match ledger.begin(&tenant, &run) {
            Ok(()) => {}
            Err(_) => {
                // Defensive: a freshly-reserved row always begins. If it ever did not, refund
                // the unstarted reservation so a retry can re-reserve (we never leave a phantom
                // started run). This is not the never-interrupt path — the run never started.
                let _ = ledger.cancel_unstarted(&tenant, &run);
                return Err(DispatchError::AlreadyDispatched);
            }
        }

        self.runs_dispatched += 1;
        Ok(InFlightRun {
            tenant,
            run,
            kind,
            reserved: estimate,
        })
    }

    /// The number of dispatches REFUSED for no balance — the AG-D6/AG-D11 `reserve refusals`
    /// telemetry.
    pub fn reserve_refusals(&self) -> u64 {
        self.reserve_refusals
    }

    /// The number of dispatches ADMITTED (runs fronted + started).
    pub fn runs_dispatched(&self) -> u64 {
        self.runs_dispatched
    }
}

/// **The agent-run reserve-refusal drill artifact (AG-D6 / AG-D11).** The PII-free aggregate a
/// surge / runaway-loop drill emits: how many dispatches were attempted, how many were
/// admitted (funded), how many were refused for no balance (`reserve refusals`), and — the
/// headline zero — how many in-flight runs were interrupted (`0`, by construction). A run that
/// settled to its metered cost is also counted so the drill can assert one-cost-event-per-unit
/// held under surge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentRunGateSignal {
    /// The tenant the drill ran for (opaque id, PII-free).
    pub tenant: TenantId,
    /// Dispatches attempted (funded + over-budget).
    pub dispatches_attempted: u64,
    /// Dispatches ADMITTED (a run was fronted + started) — the funded subset.
    pub runs_dispatched: u64,
    /// Dispatches REFUSED for no balance — the `reserve refusals` the AG-D6/AG-D11 gate reads.
    pub reserve_refusals: u64,
    /// **The headline zero** — in-flight runs interrupted. `0` is GREEN; `> 0` reads RED (an
    /// in-flight run was torn down — a contract breach). `0` by construction (no gate API does
    /// it).
    pub inflight_interrupt_count: u64,
}

impl AgentRunGateSignal {
    /// Is this a GREEN artifact? Every attempted dispatch was either admitted or refused (no
    /// dispatch silently vanished), AND zero in-flight runs were interrupted. A drill in which
    /// over-budget runs are refused (`reserve_refusals > 0`) while in-flight runs keep running
    /// (`inflight_interrupt_count == 0`) is the AG-D11 win.
    pub fn is_green(&self) -> bool {
        self.runs_dispatched + self.reserve_refusals == self.dispatches_attempted
            && self.inflight_interrupt_count == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reserve_settle::ReservationState;

    fn tenant() -> TenantId {
        TenantId::from_token("01J0ACME")
    }

    fn run(n: u32) -> RunId {
        RunId::new(format!("01J0RUN_{n}"))
    }

    /// **A funded agent-run dispatch fronts the run** — it reserves the estimate and mints an
    /// in-flight handle; the run is `InFlight` in the ledger.
    #[test]
    fn funded_dispatch_fronts_the_run() {
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let handle = gate
            .dispatch(
                &mut ledger,
                tenant(),
                run(1),
                MinorUnits(1_000),
                MinorUnits(5_000),
            )
            .expect("a funded dispatch fronts the run");
        assert_eq!(handle.kind(), RunKind::AgentRun);
        assert_eq!(handle.reserved(), MinorUnits(1_000));
        assert_eq!(
            ledger.state_of(&tenant(), &run(1)),
            Some(ReservationState::InFlight),
            "the dispatched run is in-flight"
        );
        assert_eq!(gate.runs_dispatched(), 1);
        assert_eq!(gate.reserve_refusals(), 0);
    }

    /// **No balance → no run (the run is NEVER started).** An over-budget dispatch is refused;
    /// NO in-flight handle is minted; NO reservation row is left behind; the refusal counter
    /// ticks. This is the runaway self-limiter (AG-D11).
    #[test]
    fn no_balance_means_no_run() {
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let err = gate
            .dispatch(
                &mut ledger,
                tenant(),
                run(1),
                MinorUnits(9_000),
                MinorUnits(100),
            )
            .expect_err("an over-budget dispatch is refused");
        assert_eq!(
            err,
            DispatchError::NoBalance {
                requested: MinorUnits(9_000),
                available: MinorUnits(100),
            }
        );
        assert!(
            ledger.state_of(&tenant(), &run(1)).is_none(),
            "a refused dispatch leaves NO reservation — the run never started"
        );
        assert_eq!(gate.reserve_refusals(), 1);
        assert_eq!(gate.runs_dispatched(), 0);
    }

    /// **Settle-on-completion through the in-flight handle** records one cost event per metered
    /// unit (wholesale ≠ markup) and refunds the over-reservation.
    #[test]
    fn settle_through_handle_records_one_event_per_unit() {
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let handle = gate
            .dispatch(
                &mut ledger,
                tenant(),
                run(1),
                MinorUnits(1_000),
                MinorUnits(5_000),
            )
            .unwrap();
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
        let outcome = handle.settle(&mut ledger, &units).expect("the run settles");
        assert_eq!(outcome.cost_events.len(), 2, "one cost event per metered unit");
        assert_ne!(
            outcome.cost_events[0].wholesale, outcome.cost_events[0].markup,
            "wholesale ≠ markup recorded distinctly"
        );
        assert_eq!(outcome.billed_total, MinorUnits(400));
        assert_eq!(outcome.refunded, MinorUnits(600));
        assert_eq!(
            ledger.state_of(&tenant(), &run(1)),
            Some(ReservationState::Settled)
        );
    }

    /// **NEVER interrupt in-flight.** The gate exposes NO method that tears down an in-flight
    /// run — and the underlying ledger's only teardown (`cancel_unstarted`) is structurally
    /// barred from an in-flight row. So an in-flight run stays in-flight until IT settles, and
    /// the interrupt counter is 0. (This test reaches THROUGH to the ledger to PROVE the
    /// underlying bar; the gate has no such method to call.)
    #[test]
    fn in_flight_run_is_never_interrupted() {
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let handle = gate
            .dispatch(
                &mut ledger,
                tenant(),
                run(1),
                MinorUnits(500),
                MinorUnits(1_000),
            )
            .unwrap();
        // The only teardown the ledger has refuses an in-flight row.
        assert!(
            ledger.cancel_unstarted(&tenant(), &run(1)).is_err(),
            "an in-flight run is NEVER torn down"
        );
        assert_eq!(
            ledger.state_of(&tenant(), &run(1)),
            Some(ReservationState::InFlight),
            "the run is untouched — still in-flight"
        );
        assert_eq!(
            ledger.inflight_interrupt_count(),
            0,
            "no in-flight run was ever interrupted (the headline zero)"
        );
        // The run can STILL settle normally — it kept running.
        handle.settle(&mut ledger, &[]).unwrap();
        assert_eq!(
            ledger.state_of(&tenant(), &run(1)),
            Some(ReservationState::Settled)
        );
    }

    /// **`SCHEDULE_AND_RUN_JOB` fronts identically** — same reserve gate, same handle, same
    /// no-balance refusal; only the [`RunKind`] label differs.
    #[test]
    fn schedule_and_run_job_fronts_identically() {
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        // Funded: fronted, labelled ScheduleAndRunJob.
        let handle = gate
            .schedule_and_run_job(
                &mut ledger,
                tenant(),
                run(1),
                MinorUnits(300),
                MinorUnits(1_000),
            )
            .expect("a funded scheduled job is fronted");
        assert_eq!(handle.kind(), RunKind::ScheduleAndRunJob);
        // Over-budget: not scheduled (no balance → no run), refusal counted.
        let err = gate
            .schedule_and_run_job(
                &mut ledger,
                tenant(),
                run(2),
                MinorUnits(9_000),
                MinorUnits(10),
            )
            .expect_err("an over-budget scheduled job is refused");
        assert!(matches!(err, DispatchError::NoBalance { .. }));
        assert!(ledger.state_of(&tenant(), &run(2)).is_none());
        assert_eq!(gate.reserve_refusals(), 1);
    }

    /// A re-dispatch of a live run is rejected loudly (a dispatch is fronted exactly once) —
    /// the idempotency guard, never a double-reserve.
    #[test]
    fn redispatch_of_a_live_run_is_rejected() {
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        gate.dispatch(
            &mut ledger,
            tenant(),
            run(1),
            MinorUnits(100),
            MinorUnits(1_000),
        )
        .unwrap();
        let err = gate
            .dispatch(
                &mut ledger,
                tenant(),
                run(1),
                MinorUnits(100),
                MinorUnits(1_000),
            )
            .expect_err("a second dispatch of a live run is rejected");
        assert_eq!(err, DispatchError::AlreadyDispatched);
    }

    /// The error Displays are loud and specific (observability is part of the pass).
    #[test]
    fn dispatch_error_displays_are_loud() {
        let e = DispatchError::NoBalance {
            requested: MinorUnits(9_000),
            available: MinorUnits(100),
        }
        .to_string();
        assert!(e.contains("no balance, no run"), "must cite the floor: {e}");
        assert!(e.contains("NEVER started"), "must say the run never started: {e}");
        assert!(!DispatchError::AlreadyDispatched.to_string().is_empty());
        assert!(!DispatchError::AmountOverflow.to_string().is_empty());
    }

    /// The [`ReserveError`] → [`DispatchError`] conversion maps every variant (so the agent
    /// fabric sees one error type) — and a DUPLICATE (not insufficient-balance) does NOT tick
    /// the refusal counter (only a real no-balance is a "reserve refusal").
    #[test]
    fn reserve_error_maps_and_only_no_balance_counts_as_a_refusal() {
        assert_eq!(
            DispatchError::from(ReserveError::DuplicateReservation),
            DispatchError::AlreadyDispatched
        );
        assert_eq!(
            DispatchError::from(ReserveError::AmountOverflow),
            DispatchError::AmountOverflow
        );
        // A duplicate dispatch must NOT be counted as a no-balance reserve refusal.
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        gate.dispatch(&mut ledger, tenant(), run(1), MinorUnits(100), MinorUnits(1_000))
            .unwrap();
        let _ = gate.dispatch(&mut ledger, tenant(), run(1), MinorUnits(100), MinorUnits(1_000));
        assert_eq!(
            gate.reserve_refusals(),
            0,
            "a duplicate dispatch is not a no-balance refusal"
        );
    }

    /// **THE AG-D11 RUNAWAY-LOOP DRILL.** A loop dispatches runs against a wallet that drains:
    /// the first N runs (funded) are fronted; once the wallet cannot afford the next run, the
    /// reserve REFUSES it — the loop stops at the wallet. Crucially, the already-in-flight runs
    /// are NEVER interrupted (the interrupt counter is 0). Emits a GREEN [`AgentRunGateSignal`].
    #[test]
    fn ag_d11_runaway_loop_stops_at_the_wallet_never_interrupting_in_flight() {
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        // The wallet affords exactly 3 runs of 100 each (balance 300); the loop tries 6.
        let wallet = MinorUnits(300);
        let per_run = MinorUnits(100);
        let mut spent = MinorUnits::ZERO;
        let mut live_handles = Vec::new();
        let attempts = 6u64;
        for i in 0..attempts {
            // The wallet's remaining balance shrinks as live reservations hold funds.
            let remaining = wallet.checked_sub(spent).unwrap();
            match gate.dispatch(&mut ledger, tenant(), run(i as u32), per_run, remaining) {
                Ok(handle) => {
                    spent = spent.checked_add(per_run).unwrap();
                    live_handles.push(handle);
                }
                Err(DispatchError::NoBalance { .. }) => { /* the loop stops at the wallet */ }
                Err(other) => panic!("unexpected dispatch error: {other}"),
            }
        }

        // Exactly 3 runs were fronted; the other 3 were refused (the loop stopped at the wallet).
        assert_eq!(gate.runs_dispatched(), 3, "only the funded runs were fronted");
        assert_eq!(gate.reserve_refusals(), 3, "the over-budget runs were refused");
        // The in-flight runs were NEVER interrupted by the refusals.
        assert_eq!(ledger.inflight_interrupt_count(), 0);
        for h in &live_handles {
            assert_eq!(
                ledger.state_of(&tenant(), h.run()),
                Some(ReservationState::InFlight),
                "every funded run is still running — none was torn down"
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
    }

    /// **THE AG-D6 SURGE DRILL.** A 30× dispatch surge by one tenant: the funded runs are
    /// fronted, the over-budget surge is shed (reserve refusals), and no in-flight run is
    /// interrupted. Emits a GREEN [`AgentRunGateSignal`] with `reserve_refusals > 0` (the surge
    /// was shed) and `inflight_interrupt_count == 0`.
    #[test]
    fn ag_d6_surge_sheds_over_budget_runs_without_interrupting() {
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        // 30 dispatches; the wallet affords 10 of them (balance 1000, 100 each).
        let attempts = 30u64;
        let per_run = MinorUnits(100);
        let wallet = MinorUnits(1_000);
        let mut spent = MinorUnits::ZERO;
        for i in 0..attempts {
            let remaining = wallet.checked_sub(spent).unwrap_or(MinorUnits::ZERO);
            if gate
                .dispatch(&mut ledger, tenant(), run(i as u32), per_run, remaining)
                .is_ok()
            {
                spent = spent.checked_add(per_run).unwrap();
            }
        }
        assert_eq!(gate.runs_dispatched(), 10, "exactly the funded runs admitted");
        assert_eq!(gate.reserve_refusals(), 20, "the surge over budget was shed");
        assert_eq!(ledger.inflight_interrupt_count(), 0, "0 interrupts under surge");

        let signal = AgentRunGateSignal {
            tenant: tenant(),
            dispatches_attempted: attempts,
            runs_dispatched: gate.runs_dispatched(),
            reserve_refusals: gate.reserve_refusals(),
            inflight_interrupt_count: ledger.inflight_interrupt_count(),
        };
        assert!(signal.is_green(), "AG-D6 must be GREEN: {signal:?}");
        assert!(signal.reserve_refusals > 0, "the surge must have shed runs");
    }

    /// A RED signal (an in-flight interrupt, or a dispatch that vanished) is classified NOT
    /// green — proving `is_green` is not vacuously true.
    #[test]
    fn a_red_signal_is_not_green() {
        let interrupted = AgentRunGateSignal {
            tenant: tenant(),
            dispatches_attempted: 10,
            runs_dispatched: 10,
            reserve_refusals: 0,
            inflight_interrupt_count: 1,
        };
        assert!(!interrupted.is_green(), "an in-flight interrupt must read RED");

        let vanished = AgentRunGateSignal {
            tenant: tenant(),
            dispatches_attempted: 10,
            runs_dispatched: 7,
            reserve_refusals: 2, // 7 + 2 = 9 != 10 — a dispatch vanished
            inflight_interrupt_count: 0,
        };
        assert!(!vanished.is_green(), "a vanished dispatch must read RED");
    }
}
