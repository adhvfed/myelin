use crate::mock::{MockAgentRuntime, MockScript};
use crate::skeleton::{SkeletonError, SkeletonTelemetry};
use myelin_storage::reserve_settle::MicroUsd;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunawayStep {
    Admitted {
        reserved: u64,
        settled: u64,
    },
    Refused {
        requested: u64,
        available: u64,
    },
}

impl RunawayStep {
    pub fn is_admitted(&self) -> bool {
        matches!(self, RunawayStep::Admitted { .. })
    }
    pub fn is_refused(&self) -> bool {
        matches!(self, RunawayStep::Refused { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentFabricCostSignal {
    pub runs_attempted: u64,
    pub runs_completed: u64,
    pub reserve_refusals: u64,
    pub inflight_interrupt_count: u64,
    pub total_reserved: u64,
    pub total_settled: u64,
}

impl AgentFabricCostSignal {
    pub fn ledger_balanced(&self) -> bool {
        self.total_reserved == self.total_settled
    }

    pub fn is_green(&self) -> bool {
        self.runs_completed + self.reserve_refusals == self.runs_attempted
            && self.reserve_refusals > 0
            && self.inflight_interrupt_count == 0
            && self.ledger_balanced()
    }
}

pub struct RunawaySelfLimiter {
    wallet: MicroUsd,
    per_run_estimate: MicroUsd,
}

impl RunawaySelfLimiter {
    pub fn new(wallet: MicroUsd, per_run_estimate: MicroUsd) -> RunawaySelfLimiter {
        RunawaySelfLimiter {
            wallet,
            per_run_estimate,
        }
    }

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
            MicroUsd,
            MicroUsd,
            &mut SkeletonTelemetry,
        ) -> Result<u64, SkeletonError>,
    {
        let _ = brain;
        let mut steps = Vec::with_capacity(attempts as usize);
        let mut spent = MicroUsd::ZERO;
        for i in 0..attempts {
            let remaining = self.wallet.checked_sub(spent).unwrap_or(MicroUsd::ZERO);
            let run_id = format!("runaway-{i}");
            match drive_one(run_id, remaining, self.per_run_estimate, telemetry) {
                Ok(settled) => {
                    spent = spent
                        .checked_add(self.per_run_estimate)
                        .expect("wallet arithmetic does not overflow within a drill");
                    steps.push(RunawayStep::Admitted {
                        reserved: self.per_run_estimate.0,
                        settled,
                    });
                }
                Err(SkeletonError::DispatchRefused(_)) => {
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

pub fn runaway_brain() -> MockAgentRuntime {
    MockAgentRuntime::new(MockScript::submit_only(
        "runaway: the same task, over and over - the WALLET stops it, not the brain",
        "runaway step",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let interrupted = AgentFabricCostSignal {
            inflight_interrupt_count: 1,
            ..base.clone()
        };
        assert!(!interrupted.is_green(), "an interrupt reads RED");

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

        let vanished = AgentFabricCostSignal {
            runs_attempted: 6,
            ..base.clone()
        };
        assert!(!vanished.is_green(), "a vanished run reads RED");

        let unbalanced = AgentFabricCostSignal {
            total_settled: 10,
            ..base.clone()
        };
        assert!(!unbalanced.is_green(), "an unbalanced ledger reads RED");
    }

    #[test]
    fn runaway_loop_admits_funded_prefix_and_sheds_the_tail() {
        let limiter = RunawaySelfLimiter::new(MicroUsd(50), MicroUsd(10));
        let brain = runaway_brain();
        let mut tele = SkeletonTelemetry::new();

        let steps = limiter.run_loop(&brain, 12, &mut tele, |_run, available, estimate, _t| {
            if available.0 < estimate.0 {
                Err(SkeletonError::DispatchRefused(format!(
                    "no balance, no run (requested {}, {} available)",
                    estimate.0, available.0
                )))
            } else {
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

    #[test]
    fn an_empty_wallet_refuses_every_run() {
        let limiter = RunawaySelfLimiter::new(MicroUsd(0), MicroUsd(10));
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

    #[test]
    fn runaway_brain_is_well_formed() {
        let brain = runaway_brain();
        assert!(
            brain.script().is_well_formed(),
            "the runaway brain terminates each run"
        );
    }
}
