use crate::reserve_settle::{
    CostLedger, MeteredUnit, MicroUsd, ReservationState, ReserveError, RunId, SettleError,
    SettleOutcome,
};
use myelin_tenancy::TenantId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunKind {
    AgentRun,
    ScheduleAndRunJob,
    CiRun,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DispatchError {
    NoBalance {
        requested: MicroUsd,
        available: MicroUsd,
    },
    AlreadyDispatched,
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
                 - the run was NEVER started (storage §9, AG-D11)",
                requested.0, available.0
            ),
            DispatchError::AlreadyDispatched => write!(
                f,
                "dispatch refused: this run is already in flight - a dispatch is fronted exactly once"
            ),
            DispatchError::AmountOverflow => write!(
                f,
                "dispatch refused: integer minor-units arithmetic overflowed u64 (loud, never a silent wrap)"
            ),
        }
    }
}

impl std::error::Error for DispatchError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InFlightRun {
    tenant: TenantId,
    run: RunId,
    kind: RunKind,
    reserved: MicroUsd,
}

impl InFlightRun {
    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }
    pub fn run(&self) -> &RunId {
        &self.run
    }
    pub fn kind(&self) -> RunKind {
        self.kind
    }
    pub fn reserved(&self) -> MicroUsd {
        self.reserved
    }

    pub fn settle(
        &self,
        ledger: &mut CostLedger,
        units: &[MeteredUnit],
    ) -> Result<SettleOutcome, SettleError> {
        ledger.settle(&self.tenant, &self.run, units)
    }
}

#[derive(Debug, Default)]
pub struct AgentRunGate {
    reserve_refusals: u64,
    runs_dispatched: u64,
}

#[derive(Clone, Copy)]
enum DispatchAdmission {
    Fresh(RunKind),
    WorkflowReplay(RunKind),
}

impl DispatchAdmission {
    fn kind(self) -> RunKind {
        match self {
            Self::Fresh(kind) | Self::WorkflowReplay(kind) => kind,
        }
    }

    fn may_resume(self) -> bool {
        matches!(self, Self::WorkflowReplay(_))
    }
}

impl AgentRunGate {
    pub fn new() -> AgentRunGate {
        AgentRunGate::default()
    }

    pub fn dispatch(
        &mut self,
        ledger: &mut CostLedger,
        tenant: TenantId,
        run: RunId,
        estimate: MicroUsd,
        available: MicroUsd,
    ) -> Result<InFlightRun, DispatchError> {
        self.dispatch_kind(
            ledger,
            tenant,
            run,
            estimate,
            available,
            DispatchAdmission::Fresh(RunKind::AgentRun),
        )
    }

    pub fn dispatch_or_resume_workflow(
        &mut self,
        ledger: &mut CostLedger,
        tenant: TenantId,
        run: RunId,
        estimate: MicroUsd,
        available: MicroUsd,
    ) -> Result<InFlightRun, DispatchError> {
        self.dispatch_kind(
            ledger,
            tenant,
            run,
            estimate,
            available,
            DispatchAdmission::WorkflowReplay(RunKind::AgentRun),
        )
    }

    pub fn schedule_and_run_job(
        &mut self,
        ledger: &mut CostLedger,
        tenant: TenantId,
        run: RunId,
        estimate: MicroUsd,
        available: MicroUsd,
    ) -> Result<InFlightRun, DispatchError> {
        self.dispatch_kind(
            ledger,
            tenant,
            run,
            estimate,
            available,
            DispatchAdmission::Fresh(RunKind::ScheduleAndRunJob),
        )
    }

    fn dispatch_kind(
        &mut self,
        ledger: &mut CostLedger,
        tenant: TenantId,
        run: RunId,
        estimate: MicroUsd,
        available: MicroUsd,
        admission: DispatchAdmission,
    ) -> Result<InFlightRun, DispatchError> {
        let newly_reserved = match ledger.reserve(tenant.clone(), run.clone(), estimate, available) {
            Ok(_reservation) => true,
            Err(ReserveError::DuplicateReservation) if admission.may_resume() => {
                let Some(existing) = ledger.reservation_of(&tenant, &run) else {
                    return Err(DispatchError::AlreadyDispatched);
                };
                if existing.reserved != estimate
                    || !matches!(existing.state, ReservationState::Reserved | ReservationState::InFlight)
                {
                    return Err(DispatchError::AlreadyDispatched);
                }
                false
            }
            Err(ReserveError::DuplicateReservation) => {
                return Err(DispatchError::AlreadyDispatched);
            }
            Err(e) => {
                if matches!(e, ReserveError::InsufficientBalance { .. }) {
                    self.reserve_refusals += 1;
                }
                return Err(e.into());
            }
        };

        match ledger.begin(&tenant, &run) {
            Ok(()) => {}
            Err(_) => {
                if newly_reserved {
                    let _ = ledger.cancel_unstarted(&tenant, &run);
                }
                return Err(DispatchError::AlreadyDispatched);
            }
        }

        if newly_reserved {
            self.runs_dispatched += 1;
        }
        Ok(InFlightRun {
            tenant,
            run,
            kind: admission.kind(),
            reserved: estimate,
        })
    }

    pub fn reserve_refusals(&self) -> u64 {
        self.reserve_refusals
    }

    pub fn runs_dispatched(&self) -> u64 {
        self.runs_dispatched
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentRunGateSignal {
    pub tenant: TenantId,
    pub dispatches_attempted: u64,
    pub runs_dispatched: u64,
    pub reserve_refusals: u64,
    pub inflight_interrupt_count: u64,
}

impl AgentRunGateSignal {
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

    #[test]
    fn funded_dispatch_fronts_the_run() {
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let handle = gate
            .dispatch(
                &mut ledger,
                tenant(),
                run(1),
                MicroUsd(1_000),
                MicroUsd(5_000),
            )
            .expect("a funded dispatch fronts the run");
        assert_eq!(handle.kind(), RunKind::AgentRun);
        assert_eq!(handle.reserved(), MicroUsd(1_000));
        assert_eq!(
            ledger.state_of(&tenant(), &run(1)),
            Some(ReservationState::InFlight),
            "the dispatched run is in-flight"
        );
        assert_eq!(gate.runs_dispatched(), 1);
        assert_eq!(gate.reserve_refusals(), 0);
    }

    #[test]
    fn the_same_inflight_dispatch_resumes_without_reserving_twice() {
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let first = gate
            .dispatch_or_resume_workflow(
                &mut ledger,
                tenant(),
                run(1),
                MicroUsd(1_000),
                MicroUsd(5_000),
            )
            .expect("the first drive reserves and begins the run");
        let replay = gate
            .dispatch_or_resume_workflow(
                &mut ledger,
                tenant(),
                run(1),
                MicroUsd(1_000),
                MicroUsd::ZERO,
            )
            .expect("a workflow replay resumes its exact in-flight reservation");

        assert_eq!(replay.reserved(), first.reserved());
        assert_eq!(gate.runs_dispatched(), 1, "replay is not a second dispatch");
        assert_eq!(
            ledger.reservation_of(&tenant(), &run(1)).unwrap().reserved,
            MicroUsd(1_000),
            "the original reservation remains the only reservation",
        );
    }

    #[test]
    fn replay_cannot_change_an_inflight_reservation() {
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        gate.dispatch_or_resume_workflow(
            &mut ledger,
            tenant(),
            run(1),
            MicroUsd(1_000),
            MicroUsd(5_000),
        )
        .unwrap();

        assert_eq!(
            gate.dispatch_or_resume_workflow(
                &mut ledger,
                tenant(),
                run(1),
                MicroUsd(2_000),
                MicroUsd(5_000),
            ),
            Err(DispatchError::AlreadyDispatched),
        );
        assert_eq!(ledger.reservation_of(&tenant(), &run(1)).unwrap().reserved, MicroUsd(1_000));
    }

    #[test]
    fn no_balance_means_no_run() {
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let err = gate
            .dispatch(
                &mut ledger,
                tenant(),
                run(1),
                MicroUsd(9_000),
                MicroUsd(100),
            )
            .expect_err("an over-budget dispatch is refused");
        assert_eq!(
            err,
            DispatchError::NoBalance {
                requested: MicroUsd(9_000),
                available: MicroUsd(100),
            }
        );
        assert!(
            ledger.state_of(&tenant(), &run(1)).is_none(),
            "a refused dispatch leaves NO reservation - the run never started"
        );
        assert_eq!(gate.reserve_refusals(), 1);
        assert_eq!(gate.runs_dispatched(), 0);
    }

    #[test]
    fn settle_through_handle_records_one_event_per_unit() {
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let handle = gate
            .dispatch(
                &mut ledger,
                tenant(),
                run(1),
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
            "wholesale ≠ markup recorded distinctly"
        );
        assert_eq!(outcome.billed_total, MicroUsd(400));
        assert_eq!(outcome.refunded, MicroUsd(600));
        assert_eq!(
            ledger.state_of(&tenant(), &run(1)),
            Some(ReservationState::Settled)
        );
    }

    #[test]
    fn in_flight_run_is_never_interrupted() {
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let handle = gate
            .dispatch(
                &mut ledger,
                tenant(),
                run(1),
                MicroUsd(500),
                MicroUsd(1_000),
            )
            .unwrap();
        assert!(
            ledger.cancel_unstarted(&tenant(), &run(1)).is_err(),
            "an in-flight run is NEVER torn down"
        );
        assert_eq!(
            ledger.state_of(&tenant(), &run(1)),
            Some(ReservationState::InFlight),
            "the run is untouched - still in-flight"
        );
        assert_eq!(
            ledger.inflight_interrupt_count(),
            0,
            "no in-flight run was ever interrupted (the headline zero)"
        );
        handle.settle(&mut ledger, &[]).unwrap();
        assert_eq!(
            ledger.state_of(&tenant(), &run(1)),
            Some(ReservationState::Settled)
        );
    }

    #[test]
    fn schedule_and_run_job_fronts_identically() {
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let handle = gate
            .schedule_and_run_job(
                &mut ledger,
                tenant(),
                run(1),
                MicroUsd(300),
                MicroUsd(1_000),
            )
            .expect("a funded scheduled job is fronted");
        assert_eq!(handle.kind(), RunKind::ScheduleAndRunJob);
        let err = gate
            .schedule_and_run_job(
                &mut ledger,
                tenant(),
                run(2),
                MicroUsd(9_000),
                MicroUsd(10),
            )
            .expect_err("an over-budget scheduled job is refused");
        assert!(matches!(err, DispatchError::NoBalance { .. }));
        assert!(ledger.state_of(&tenant(), &run(2)).is_none());
        assert_eq!(gate.reserve_refusals(), 1);
    }

    #[test]
    fn redispatch_of_a_live_run_is_rejected() {
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        gate.dispatch(
            &mut ledger,
            tenant(),
            run(1),
            MicroUsd(100),
            MicroUsd(1_000),
        )
        .unwrap();
        let err = gate
            .dispatch(
                &mut ledger,
                tenant(),
                run(1),
                MicroUsd(100),
                MicroUsd(1_000),
            )
            .expect_err("a second dispatch of a live run is rejected");
        assert_eq!(err, DispatchError::AlreadyDispatched);
    }

    #[test]
    fn dispatch_error_displays_are_loud() {
        let e = DispatchError::NoBalance {
            requested: MicroUsd(9_000),
            available: MicroUsd(100),
        }
        .to_string();
        assert!(e.contains("no balance, no run"), "must cite the floor: {e}");
        assert!(
            e.contains("NEVER started"),
            "must say the run never started: {e}"
        );
        assert!(!DispatchError::AlreadyDispatched.to_string().is_empty());
        assert!(!DispatchError::AmountOverflow.to_string().is_empty());
    }

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
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        gate.dispatch(
            &mut ledger,
            tenant(),
            run(1),
            MicroUsd(100),
            MicroUsd(1_000),
        )
        .unwrap();
        let _ = gate.dispatch(
            &mut ledger,
            tenant(),
            run(1),
            MicroUsd(100),
            MicroUsd(1_000),
        );
        assert_eq!(
            gate.reserve_refusals(),
            0,
            "a duplicate dispatch is not a no-balance refusal"
        );
    }

    #[test]
    fn ag_d11_runaway_loop_stops_at_the_wallet_never_interrupting_in_flight() {
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let wallet = MicroUsd(300);
        let per_run = MicroUsd(100);
        let mut spent = MicroUsd::ZERO;
        let mut live_handles = Vec::new();
        let attempts = 6u64;
        for i in 0..attempts {
            let remaining = wallet.checked_sub(spent).unwrap();
            match gate.dispatch(&mut ledger, tenant(), run(i as u32), per_run, remaining) {
                Ok(handle) => {
                    spent = spent.checked_add(per_run).unwrap();
                    live_handles.push(handle);
                }
                Err(DispatchError::NoBalance { .. }) => {  }
                Err(other) => panic!("unexpected dispatch error: {other}"),
            }
        }

        assert_eq!(
            gate.runs_dispatched(),
            3,
            "only the funded runs were fronted"
        );
        assert_eq!(
            gate.reserve_refusals(),
            3,
            "the over-budget runs were refused"
        );
        assert_eq!(ledger.inflight_interrupt_count(), 0);
        for h in &live_handles {
            assert_eq!(
                ledger.state_of(&tenant(), h.run()),
                Some(ReservationState::InFlight),
                "every funded run is still running - none was torn down"
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

    #[test]
    fn ag_d6_surge_sheds_over_budget_runs_without_interrupting() {
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let attempts = 30u64;
        let per_run = MicroUsd(100);
        let wallet = MicroUsd(1_000);
        let mut spent = MicroUsd::ZERO;
        for i in 0..attempts {
            let remaining = wallet.checked_sub(spent).unwrap_or(MicroUsd::ZERO);
            if gate
                .dispatch(&mut ledger, tenant(), run(i as u32), per_run, remaining)
                .is_ok()
            {
                spent = spent.checked_add(per_run).unwrap();
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
            "the surge over budget was shed"
        );
        assert_eq!(
            ledger.inflight_interrupt_count(),
            0,
            "0 interrupts under surge"
        );

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

    #[test]
    fn a_red_signal_is_not_green() {
        let interrupted = AgentRunGateSignal {
            tenant: tenant(),
            dispatches_attempted: 10,
            runs_dispatched: 10,
            reserve_refusals: 0,
            inflight_interrupt_count: 1,
        };
        assert!(
            !interrupted.is_green(),
            "an in-flight interrupt must read RED"
        );

        let vanished = AgentRunGateSignal {
            tenant: tenant(),
            dispatches_attempted: 10,
            runs_dispatched: 7,
            reserve_refusals: 2,
            inflight_interrupt_count: 0,
        };
        assert!(!vanished.is_green(), "a vanished dispatch must read RED");
    }
}
