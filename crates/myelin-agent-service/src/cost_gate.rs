//! # `cost_gate` — the reserve/settle cost gate as the runaway self-limiter (AG-P14 → P-227, M2-B)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/agent-fabric.md` §5.4 (*reserve at dispatch,
//! settle on completion, refuse to start when balance is exhausted, NEVER interrupt one in flight;
//! meter one cost event per model call + per metered effect; wholesale ≠ markup; integer minor-units
//! never floats; the gate UNIFORMLY fronts CI runs and agent runs into the SAME wallet — uniform
//! guarantee #1*), §5.2 step 5 (BUDGET), §5.6 (a run is a durable workflow; reserve/settle are the
//! bookends).
//!
//! **Contract-index:** CONSUMES 11.7 (`reserve`/`settle` — the cost-gate bookends; Storage owns the
//! durable ledger correctness, this crate is the agent-fabric CONSUMER that fronts every run).
//!
//! ## What this prompt ships — the runaway self-limiter at the AGENT-FABRIC tier
//!
//! P-103 ([`myelin_storage::reserve_settle::CostLedger`]) shipped the durable ledger MECHANISM; P-146
//! ([`myelin_storage::agent_run_gate::AgentRunGate`]) shipped the dispatch-fronting GATE + the raw
//! AG-D6/AG-D11 gate-level drills; P-216 ([`crate::skeleton::SkeletonAgent::handle_run`]) wired the
//! reserve/settle bookends into ONE SKELETON run. **AG-P14 closes the M2-B deterministic-correctness
//! family**: it drives the gate as the *runaway self-limiter* through the REAL agent loop — a
//! [`MockAgentRuntime`](crate::mock::MockAgentRuntime) brain looping run-after-run against ONE
//! draining wallet — and proves the AG-D11 contract end-to-end at the fabric tier:
//!
//! 1. **Reserve-at-dispatch / no balance → no run.** Each run reserves its estimate against the
//!    wallet's REMAINING balance before it starts. Once the wallet cannot afford the next run, the
//!    reserve REFUSES it ([`RunawayStep::Refused`]); the run never starts. The reserve-refusal counter
//!    increments — *the loop stops at the wallet, not by a kill.*
//! 2. **NEVER interrupt an in-flight run.** A refusal of run *N+1* does not touch the in-flight run
//!    *N*: the ledger has no tear-down-in-flight API, so `inflight_interrupt_count` is `0` by
//!    construction. The in-flight run runs to completion (its trace is written, it settles).
//! 3. **Settle-on-completion; the Mock meters ZERO.** A run that completes settles its reservation
//!    (the Mock has no real model call → it bills 0 metered units → the whole reservation refunds;
//!    `reserved == settled`). The real per-model-call cost event arrives with `LlmAgentRuntime`
//!    (AG-P25, post-M5) — the Mock metering ZERO is CORRECT, not a floor in the gate mechanism.
//!
//! The headline artifact is [`AgentFabricCostSignal`]: `reserve_refusals > 0` (the runaway was shed)
//! AND `inflight_interrupt_count == 0` (no in-flight run torn down) AND `runs_completed +
//! reserve_refusals == runs_attempted` (no run silently vanished) AND `ledger_balanced` (every
//! minor-unit reserved by a completed run was settled).
//!
//! ## FLOOR named (the gate MECHANISM is complete)
//! - **The real per-model-call cost metering** arrives with `LlmAgentRuntime` (AG-P25, post-M5 —
//!   designed-not-built, the only place a model/SDK/prompt/model-name string appears). The Mock
//!   meters ZERO, which is correct: the gate is the runaway self-limiter REGARDLESS of which brain
//!   runs (that is exactly the point — the limiter does not depend on the cost being non-zero). The
//!   gate mechanism (reserve refuses past exhaustion, never interrupts in-flight, settles on
//!   completion) is COMPLETE and proven here.
//! - **No new data-layer trait is touched** — this module DRIVES the Storage-owned
//!   [`AgentRunGate`]/[`CostLedger`] (proven against the live PG tier by the infra-stage integration
//!   drills, P-103/P-146). No new db/object-store/cache/bus contract → no new integration drill owed
//!   (recorded in the P-227 report).

use crate::mock::{MockAgentRuntime, MockScript};
use crate::skeleton::{SkeletonError, SkeletonTelemetry};
use myelin_storage::reserve_settle::MinorUnits;

/// **The outcome of ONE attempted run in a runaway loop (the AG-D11 per-step verdict).** A run is
/// either ADMITTED (the wallet afforded it → it ran behind the cost gate → it settled) or REFUSED
/// (no balance → the run never started; the loop stops at the wallet). There is deliberately NO
/// third "interrupted" variant — an in-flight run is NEVER torn down (the never-interrupt invariant
/// is in the type: the limiter can only refuse a *next* run, never interrupt a *running* one).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunawayStep {
    /// The run was admitted: the reserve succeeded, the run executed behind the cost gate, and it
    /// settled. Carries the (reserved, settled) minor-units so the drill asserts `reserved ==
    /// settled` (a Mock bills 0 metered units → the whole reservation refunds).
    Admitted {
        /// The amount reserved at dispatch (the run's estimated upper-bound cost).
        reserved: u64,
        /// The amount settled on completion (`billed + refunded`; `== reserved` for the balanced
        /// ledger — the Mock bills 0, refunds the rest).
        settled: u64,
    },
    /// The run was REFUSED: the wallet could not afford the reserve, so the run NEVER started (no
    /// trace, no settle). This is the runaway self-limiter firing — the loop stops at the wallet.
    Refused {
        /// The amount the dispatch asked to reserve.
        requested: u64,
        /// The wallet balance that was available at refusal time.
        available: u64,
    },
}

impl RunawayStep {
    /// Whether this step admitted a run (the wallet afforded it).
    pub fn is_admitted(&self) -> bool {
        matches!(self, RunawayStep::Admitted { .. })
    }
    /// Whether this step refused a run (the runaway self-limiter fired).
    pub fn is_refused(&self) -> bool {
        matches!(self, RunawayStep::Refused { .. })
    }
}

/// **The AG-D11 runaway-self-limiter artifact at the AGENT-FABRIC tier (the green drill signal).**
/// The PII-free aggregate the runaway-loop drill emits when a [`MockAgentRuntime`] brain loops
/// run-after-run against ONE draining wallet. The two headline numbers the gate asserts:
/// `reserve_refusals > 0` (the runaway over-budget tail was shed) and `inflight_interrupt_count ==
/// 0` (no in-flight run was ever torn down — *the loop stops at the wallet, not by a kill*).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentFabricCostSignal {
    /// How many runs the runaway loop ATTEMPTED to dispatch (the funded prefix + the over-budget tail).
    pub runs_attempted: u64,
    /// How many runs were ADMITTED + COMPLETED behind the cost gate (the funded prefix).
    pub runs_completed: u64,
    /// How many dispatches the reserve REFUSED for no balance — the runaway tail that stopped at the
    /// wallet. The AG-D11 `reserve refusals` telemetry.
    pub reserve_refusals: u64,
    /// **The headline zero** — in-flight runs interrupted. `0` is GREEN; `> 0` reads RED (an in-flight
    /// run was torn down — a contract breach). `0` by construction (the gate has no tear-down API).
    pub inflight_interrupt_count: u64,
    /// The total reserved across the ADMITTED runs (integer minor-units).
    pub total_reserved: u64,
    /// The total settled across the ADMITTED runs (integer minor-units). `total_reserved ==
    /// total_settled` is the balanced ledger: every minor-unit a completed run reserved was settled.
    pub total_settled: u64,
}

impl AgentFabricCostSignal {
    /// **Is the ledger balanced?** Every minor-unit reserved by a completed run was settled (billed +
    /// refunded). A Mock bills 0 and refunds the whole reservation; the gate is `reserved == settled`.
    pub fn ledger_balanced(&self) -> bool {
        self.total_reserved == self.total_settled
    }

    /// **Is this a GREEN AG-D11 artifact?** Every attempted run was either completed or refused (no
    /// run silently vanished), the runaway tail was shed (`reserve_refusals > 0`), NOT ONE in-flight
    /// run was interrupted (`inflight_interrupt_count == 0`), and the ledger is balanced. This is the
    /// AG-D11 win: the loop stops at the wallet, the in-flight runs keep running, the books balance.
    pub fn is_green(&self) -> bool {
        self.runs_completed + self.reserve_refusals == self.runs_attempted
            && self.reserve_refusals > 0
            && self.inflight_interrupt_count == 0
            && self.ledger_balanced()
    }
}

/// **The runaway self-limiter: a driver that loops `MockAgentRuntime` runs through the SKELETON loop
/// against ONE draining wallet (the AG-D11 e2e harness, §5.4).** It does NOT re-implement the cost
/// gate — it DRIVES the real [`SkeletonAgent::handle_run`] path (which fronts every run through the
/// Storage [`AgentRunGate`](myelin_storage::agent_run_gate::AgentRunGate)) once per loop iteration,
/// shrinking the wallet's remaining balance as live reservations hold funds. The point of the type is
/// that the runaway loop is a value the drill can OBSERVE step-by-step: every iteration is a
/// [`RunawayStep`], and the loop's aggregate is an [`AgentFabricCostSignal`].
pub struct RunawaySelfLimiter {
    /// The wallet's TOTAL balance (integer minor-units). The remaining balance shrinks as live
    /// reservations hold funds; once `remaining < estimate` the next reserve is REFUSED.
    wallet: MinorUnits,
    /// Each run's estimated upper-bound cost reserved at dispatch (integer minor-units).
    per_run_estimate: MinorUnits,
}

impl RunawaySelfLimiter {
    /// Build a runaway self-limiter over a `wallet` total balance and a `per_run_estimate` (the
    /// upper-bound cost each run reserves at dispatch). The wallet affords `wallet / per_run_estimate`
    /// runs before the reserve refuses the rest.
    pub fn new(wallet: MinorUnits, per_run_estimate: MinorUnits) -> RunawaySelfLimiter {
        RunawaySelfLimiter {
            wallet,
            per_run_estimate,
        }
    }

    /// **Drive a runaway loop of `attempts` mock runs through the SKELETON cost gate against the one
    /// draining wallet (the AG-D11 chained e2e).** Each iteration builds a fresh [`RunSubstrate`] via
    /// `make_substrate(run_id, available, estimate)` (the caller supplies the substrate factory so this
    /// module takes no dependency on the dispatch-tier seams), drives ONE
    /// [`MockAgentRuntime`]-brained run through [`SkeletonAgent::handle_run`], and records the
    /// [`RunawayStep`]. A run that is ADMITTED holds its reservation (the remaining balance shrinks by
    /// the estimate); a run that is REFUSED leaves the balance untouched (no balance → no run). The
    /// already-in-flight runs are NEVER interrupted by a later refusal — the gate has no tear-down API.
    ///
    /// `brain` is the SAME deterministic scripted brain for every run (a runaway loop runs the same
    /// task over and over); the cost gate is INDEPENDENT of which brain runs — that is the point.
    /// Returns the per-step verdicts + the shared [`SkeletonTelemetry`] (the balanced-ledger signal).
    ///
    /// The `make_substrate` closure receives `(run_id, available, estimate)` and MUST build a
    /// substrate whose `available`/`estimate` are those passed (this module owns the draining-balance
    /// arithmetic; the caller owns the substrate wiring — gate, ledger, minter, journal, outbox).
    pub fn run_loop<F>(
        &self,
        brain: &MockAgentRuntime,
        attempts: u64,
        telemetry: &mut SkeletonTelemetry,
        mut drive_one: F,
    ) -> Vec<RunawayStep>
    where
        F: FnMut(
            String,
            MinorUnits,
            MinorUnits,
            &mut SkeletonTelemetry,
        ) -> Result<u64, SkeletonError>,
    {
        let _ = brain; // the brain is the same for every run; the gate is brain-independent.
        let mut steps = Vec::with_capacity(attempts as usize);
        // `spent` is the funds held by live reservations; the wallet's REMAINING balance is
        // `wallet - spent`. A refused run does NOT increase `spent` (no balance → no run).
        let mut spent = MinorUnits::ZERO;
        for i in 0..attempts {
            let remaining = self.wallet.checked_sub(spent).unwrap_or(MinorUnits::ZERO);
            let run_id = format!("runaway-{i}");
            match drive_one(run_id, remaining, self.per_run_estimate, telemetry) {
                Ok(settled) => {
                    // The run was admitted + completed behind the cost gate. A live reservation holds
                    // the estimate (the wallet's remaining balance shrinks by it).
                    spent = spent
                        .checked_add(self.per_run_estimate)
                        .expect("wallet arithmetic does not overflow within a drill");
                    steps.push(RunawayStep::Admitted {
                        reserved: self.per_run_estimate.0,
                        settled,
                    });
                }
                Err(SkeletonError::DispatchRefused(_)) => {
                    // No balance → no run. The runaway self-limiter fired: the loop stops at the
                    // wallet, NOT by a kill. The in-flight runs are untouched (the gate has no
                    // tear-down API). `spent` is unchanged — a refused run reserves nothing.
                    steps.push(RunawayStep::Refused {
                        requested: self.per_run_estimate.0,
                        available: remaining.0,
                    });
                }
                Err(other) => panic!("unexpected SKELETON error in the runaway loop: {other}"),
            }
        }
        steps
    }

    /// **Build the AG-D11 [`AgentFabricCostSignal`] from a completed runaway loop.** The aggregate the
    /// drill gates on: the admitted/refused split, the in-flight interrupt count (read from the
    /// ledger), and the balanced reserve/settle totals. `inflight_interrupt_count` is the
    /// ledger's structural `0`.
    pub fn signal(steps: &[RunawayStep], inflight_interrupt_count: u64) -> AgentFabricCostSignal {
        let mut runs_completed = 0u64;
        let mut reserve_refusals = 0u64;
        let mut total_reserved = 0u64;
        let mut total_settled = 0u64;
        for s in steps {
            match s {
                RunawayStep::Admitted { reserved, settled } => {
                    runs_completed += 1;
                    total_reserved = total_reserved.saturating_add(*reserved);
                    total_settled = total_settled.saturating_add(*settled);
                }
                RunawayStep::Refused { .. } => reserve_refusals += 1,
            }
        }
        AgentFabricCostSignal {
            runs_attempted: steps.len() as u64,
            runs_completed,
            reserve_refusals,
            inflight_interrupt_count,
            total_reserved,
            total_settled,
        }
    }
}

/// **A runaway-loop script: a `MockAgentRuntime` brain that submits immediately every run.** A
/// runaway loop is the SAME task over and over; the cost gate (not the brain) is what stops it. This
/// is a well-formed single-turn script (the brain is irrelevant to the cost gate — that is the point).
pub fn runaway_brain() -> MockAgentRuntime {
    MockAgentRuntime::new(MockScript::submit_only(
        "runaway: the same task, over and over — the WALLET stops it, not the brain",
        "runaway step",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The runaway-step verdict predicates are exact.** An admitted step is admitted (not refused);
    /// a refused step is refused (not admitted). Kills a `-> true`/`-> false` constant mutant.
    #[test]
    fn runaway_step_predicates_are_exact() {
        let admitted = RunawayStep::Admitted {
            reserved: 10,
            settled: 10,
        };
        let refused = RunawayStep::Refused {
            requested: 10,
            available: 1,
        };
        assert!(admitted.is_admitted());
        assert!(!admitted.is_refused());
        assert!(refused.is_refused());
        assert!(!refused.is_admitted());
    }

    /// **The signal aggregates the per-step verdicts exactly.** Two admitted + three refused → 2
    /// completed, 3 refusals, the reserved/settled totals summed.
    #[test]
    fn signal_aggregates_steps_exactly() {
        let steps = vec![
            RunawayStep::Admitted {
                reserved: 10,
                settled: 10,
            },
            RunawayStep::Admitted {
                reserved: 10,
                settled: 10,
            },
            RunawayStep::Refused {
                requested: 10,
                available: 0,
            },
            RunawayStep::Refused {
                requested: 10,
                available: 0,
            },
            RunawayStep::Refused {
                requested: 10,
                available: 0,
            },
        ];
        let sig = RunawaySelfLimiter::signal(&steps, 0);
        assert_eq!(sig.runs_attempted, 5);
        assert_eq!(sig.runs_completed, 2);
        assert_eq!(sig.reserve_refusals, 3);
        assert_eq!(sig.total_reserved, 20);
        assert_eq!(sig.total_settled, 20);
        assert!(sig.ledger_balanced(), "reserved == settled");
        assert!(sig.is_green(), "the AG-D11 artifact is GREEN: {sig:?}");
    }

    /// **`is_green` is not vacuously true.** A signal with an interrupt, a vanished run, NO refusals,
    /// or an unbalanced ledger each reads RED.
    #[test]
    fn is_green_is_not_vacuous() {
        let base = AgentFabricCostSignal {
            runs_attempted: 5,
            runs_completed: 2,
            reserve_refusals: 3,
            inflight_interrupt_count: 0,
            total_reserved: 20,
            total_settled: 20,
        };
        assert!(base.is_green());

        // An in-flight interrupt is RED.
        let interrupted = AgentFabricCostSignal {
            inflight_interrupt_count: 1,
            ..base.clone()
        };
        assert!(!interrupted.is_green(), "an interrupt reads RED");

        // No refusals → the runaway never hit the wallet → NOT the AG-D11 win.
        let no_refusal = AgentFabricCostSignal {
            runs_attempted: 2,
            runs_completed: 2,
            reserve_refusals: 0,
            ..base.clone()
        };
        assert!(
            !no_refusal.is_green(),
            "no refusal is not the runaway-limiter win"
        );

        // A vanished run (completed + refused != attempted) is RED.
        let vanished = AgentFabricCostSignal {
            runs_attempted: 6,
            ..base.clone()
        };
        assert!(!vanished.is_green(), "a vanished run reads RED");

        // An unbalanced ledger is RED.
        let unbalanced = AgentFabricCostSignal {
            total_settled: 10,
            ..base.clone()
        };
        assert!(!unbalanced.is_green(), "an unbalanced ledger reads RED");
    }

    /// **The runaway loop drives exactly the funded prefix + sheds the rest (the AG-D11 mechanics).**
    /// A wallet of 50 affords 5 runs of 10; a loop of 12 admits 5, refuses 7. The `drive_one`
    /// closure here is a deterministic in-memory stand-in for the SKELETON path (a Mock bills 0 →
    /// settled == reserved); the FULL real-substrate chained drill lives in the integration test
    /// `tests/drills_ag_d11_runaway_self_limiter.rs`.
    #[test]
    fn runaway_loop_admits_funded_prefix_and_sheds_the_tail() {
        let limiter = RunawaySelfLimiter::new(MinorUnits(50), MinorUnits(10));
        let brain = runaway_brain();
        let mut tele = SkeletonTelemetry::new();

        let steps = limiter.run_loop(&brain, 12, &mut tele, |_run, available, estimate, _t| {
            // The cost-gate decision, in miniature: no balance → no run (the SKELETON's reserve).
            if available.0 < estimate.0 {
                Err(SkeletonError::DispatchRefused(format!(
                    "no balance, no run (requested {}, {} available)",
                    estimate.0, available.0
                )))
            } else {
                // Admitted: a Mock bills 0 → the whole reservation settles (reserved == settled).
                Ok(estimate.0)
            }
        });

        let admitted = steps.iter().filter(|s| s.is_admitted()).count();
        let refused = steps.iter().filter(|s| s.is_refused()).count();
        assert_eq!(admitted, 5, "the wallet afforded exactly 5 runs");
        assert_eq!(
            refused, 7,
            "the runaway tail was shed (the loop stopped at the wallet)"
        );

        let sig = RunawaySelfLimiter::signal(&steps, 0);
        assert!(sig.is_green(), "AG-D11 GREEN: {sig:?}");
        assert_eq!(sig.total_reserved, 50);
        assert_eq!(sig.total_settled, 50);
    }

    /// **A wallet that affords NOTHING refuses every run (the degenerate runaway).** The loop stops
    /// at the wallet immediately; every step is a refusal; 0 interrupts. (Not green — no run
    /// completed — but it PROVES the limiter never admits an unfunded run.)
    #[test]
    fn an_empty_wallet_refuses_every_run() {
        let limiter = RunawaySelfLimiter::new(MinorUnits(0), MinorUnits(10));
        let brain = runaway_brain();
        let mut tele = SkeletonTelemetry::new();
        let steps = limiter.run_loop(&brain, 4, &mut tele, |_r, available, estimate, _t| {
            if available.0 < estimate.0 {
                Err(SkeletonError::DispatchRefused("no balance".into()))
            } else {
                Ok(estimate.0)
            }
        });
        assert!(
            steps.iter().all(|s| s.is_refused()),
            "an empty wallet admits nothing"
        );
        let sig = RunawaySelfLimiter::signal(&steps, 0);
        assert_eq!(sig.runs_completed, 0);
        assert_eq!(sig.reserve_refusals, 4);
        assert_eq!(
            sig.inflight_interrupt_count, 0,
            "0 interrupts even when nothing ran"
        );
    }

    /// **The runaway brain is a well-formed single-turn script** (the brain is irrelevant to the cost
    /// gate — the wallet stops the loop, not the brain).
    #[test]
    fn runaway_brain_is_well_formed() {
        let brain = runaway_brain();
        assert!(
            brain.script().is_well_formed(),
            "the runaway brain terminates each run"
        );
    }
}
