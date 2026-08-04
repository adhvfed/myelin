use myelin_storage::agent_run_gate::{AgentRunGate, DispatchError, InFlightRun, RunKind};
use myelin_storage::reserve_settle::{
    CostLedger, MeteredUnit, MicroUsd, RunId, SettleError, SettleOutcome,
};
use myelin_tenancy::TenantId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueRunKind {
    Triage,
    Forecast,
    SlaDraft,
    Automation,
}

impl IssueRunKind {
    pub fn metered_unit(self) -> &'static str {
        match self {
            IssueRunKind::Triage
            | IssueRunKind::Forecast
            | IssueRunKind::SlaDraft
            | IssueRunKind::Automation => "agent.effect",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            IssueRunKind::Triage => "triage",
            IssueRunKind::Forecast => "forecast",
            IssueRunKind::SlaDraft => "sla_draft",
            IssueRunKind::Automation => "automation",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpendError {
    NoBalance {
        requested: MicroUsd,
        available: MicroUsd,
    },
    AlreadyDispatched,
    AmountOverflow,
    Settle(SettleError),
}

impl From<DispatchError> for SpendError {
    fn from(e: DispatchError) -> SpendError {
        match e {
            DispatchError::NoBalance {
                requested,
                available,
            } => SpendError::NoBalance {
                requested,
                available,
            },
            DispatchError::AlreadyDispatched => SpendError::AlreadyDispatched,
            DispatchError::AmountOverflow => SpendError::AmountOverflow,
        }
    }
}

impl From<SettleError> for SpendError {
    fn from(e: SettleError) -> SpendError {
        SpendError::Settle(e)
    }
}

impl core::fmt::Display for SpendError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SpendError::NoBalance {
                requested,
                available,
            } => write!(
                f,
                "spend-bearing Issues run refused: no balance, no start (requested {} minor-units, \
                 {} available) - the run was NEVER started (arch §9 / 11.7, AG-D11)",
                requested.0, available.0
            ),
            SpendError::AlreadyDispatched => write!(
                f,
                "spend-bearing Issues run refused: this run is already in flight - a run is fronted \
                 exactly once (never double-reserved)"
            ),
            SpendError::AmountOverflow => write!(
                f,
                "spend-bearing Issues run refused: integer minor-units arithmetic overflowed (loud, \
                 never a silent wrap)"
            ),
            SpendError::Settle(e) => write!(f, "spend-bearing Issues run settle failed: {e}"),
        }
    }
}

impl std::error::Error for SpendError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispatchedRun {
    kind: IssueRunKind,
    handle: InFlightRun,
}

impl DispatchedRun {
    pub fn kind(&self) -> IssueRunKind {
        self.kind
    }
    pub fn tenant(&self) -> &TenantId {
        self.handle.tenant()
    }
    pub fn run(&self) -> &RunId {
        self.handle.run()
    }
    pub fn reserved(&self) -> MicroUsd {
        self.handle.reserved()
    }

    pub fn settle(
        &self,
        ledger: &mut CostLedger,
        units: &[MeteredUnit],
    ) -> Result<SettleOutcome, SettleError> {
        self.handle.settle(ledger, units)
    }
}

#[derive(Debug, Default)]
pub struct IssueSpendGate {
    gate: AgentRunGate,
}

impl IssueSpendGate {
    pub fn new() -> IssueSpendGate {
        IssueSpendGate::default()
    }

    pub fn reserve_run(
        &mut self,
        ledger: &mut CostLedger,
        tenant: TenantId,
        run: RunId,
        kind: IssueRunKind,
        estimate: MicroUsd,
        available: MicroUsd,
    ) -> Result<DispatchedRun, SpendError> {
        let handle = self
            .gate
            .dispatch(ledger, tenant, run, estimate, available)
            .map_err(SpendError::from)?;
        debug_assert_eq!(handle.kind(), RunKind::AgentRun);
        Ok(DispatchedRun { kind, handle })
    }

    pub fn reserve_refusals(&self) -> u64 {
        self.gate.reserve_refusals()
    }

    pub fn runs_dispatched(&self) -> u64 {
        self.gate.runs_dispatched()
    }
}

#[allow(clippy::too_many_arguments)]
pub fn spend_bearing_run<F>(
    gate: &mut IssueSpendGate,
    ledger: &mut CostLedger,
    tenant: TenantId,
    run: RunId,
    kind: IssueRunKind,
    estimate: MicroUsd,
    available: MicroUsd,
    work: F,
) -> Result<BalancedRunSignal, SpendError>
where
    F: FnOnce() -> Vec<MeteredUnit>,
{
    let dispatched = gate.reserve_run(
        ledger,
        tenant.clone(),
        run.clone(),
        kind,
        estimate,
        available,
    )?;
    let reserved = dispatched.reserved();

    let units = work();

    let outcome = dispatched.settle(ledger, &units)?;

    Ok(BalancedRunSignal {
        tenant,
        run,
        kind,
        reserved,
        billed: outcome.billed_total,
        refunded: outcome.refunded,
        cost_events: outcome.cost_events.len() as u64,
        metered_units: units.len() as u64,
        inflight_interrupt_count: ledger.inflight_interrupt_count(),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BalancedRunSignal {
    pub tenant: TenantId,
    pub run: RunId,
    pub kind: IssueRunKind,
    pub reserved: MicroUsd,
    pub billed: MicroUsd,
    pub refunded: MicroUsd,
    pub cost_events: u64,
    pub metered_units: u64,
    pub inflight_interrupt_count: u64,
}

impl BalancedRunSignal {
    pub fn is_green(&self) -> bool {
        let accounted = self
            .billed
            .checked_add(self.refunded)
            .map(|a| a == self.reserved)
            .unwrap_or(false);
        accounted && self.cost_events == self.metered_units && self.inflight_interrupt_count == 0
    }
}

pub fn per_effect_idem_key(card_id: &str, effect_idx: usize, total_effects: usize) -> String {
    debug_assert!(total_effects >= 1, "a card has at least one effect");
    debug_assert!(
        effect_idx < total_effects,
        "effect_idx ({effect_idx}) must index into the card's {total_effects} effect(s)"
    );
    if total_effects == 1 {
        card_id.to_string()
    } else {
        format!("{card_id}:{effect_idx}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_storage::reserve_settle::ReservationState;

    fn tenant() -> TenantId {
        TenantId::from_token("01J0ACME")
    }

    fn run(n: u32) -> RunId {
        RunId::new(format!("01J0ISSUE_RUN_{n}"))
    }

    fn units(wholesale: u64, markup: u64) -> Vec<MeteredUnit> {
        vec![MeteredUnit {
            unit: "agent.effect",
            wholesale: MicroUsd(wholesale),
            markup: MicroUsd(markup),
        }]
    }

    #[test]
    fn funded_run_balances_the_wallet_reserve_equals_settle() {
        let mut gate = IssueSpendGate::new();
        let mut ledger = CostLedger::new();
        let signal = spend_bearing_run(
            &mut gate,
            &mut ledger,
            tenant(),
            run(1),
            IssueRunKind::Triage,
            MicroUsd(1_000),
            MicroUsd(5_000),
            || units(300, 100),
        )
        .expect("a funded run completes");

        assert_eq!(signal.reserved, MicroUsd(1_000));
        assert_eq!(
            signal.billed,
            MicroUsd(400),
            "billed wholesale 300 + markup 100"
        );
        assert_eq!(
            signal.refunded,
            MicroUsd(600),
            "the over-reservation refunds"
        );
        assert_eq!(signal.cost_events, 1, "one cost event per metered unit");
        assert_eq!(signal.metered_units, 1);
        assert_eq!(signal.inflight_interrupt_count, 0, "the headline zero");
        assert!(signal.is_green(), "the wallet BALANCED: {signal:?}");

        assert_eq!(
            ledger.state_of(&tenant(), &run(1)),
            Some(ReservationState::Settled),
            "a completed run settles"
        );
        assert_eq!(gate.runs_dispatched(), 1);
        assert_eq!(gate.reserve_refusals(), 0);
    }

    #[test]
    fn no_balance_means_no_start_and_the_work_never_runs() {
        let mut gate = IssueSpendGate::new();
        let mut ledger = CostLedger::new();
        let mut work_ran = false;
        let err = spend_bearing_run(
            &mut gate,
            &mut ledger,
            tenant(),
            run(1),
            IssueRunKind::Forecast,
            MicroUsd(9_000),
            MicroUsd(100),
            || {
                work_ran = true;
                units(10, 0)
            },
        )
        .expect_err("an over-budget run is refused");

        assert_eq!(
            err,
            SpendError::NoBalance {
                requested: MicroUsd(9_000),
                available: MicroUsd(100),
            }
        );
        assert!(
            !work_ran,
            "no balance → no start: the agent brain NEVER ran"
        );
        assert!(
            ledger.state_of(&tenant(), &run(1)).is_none(),
            "a refused run leaves NO reservation - it never started"
        );
        assert_eq!(gate.reserve_refusals(), 1);
        assert_eq!(gate.runs_dispatched(), 0);
    }

    #[test]
    fn settle_never_interrupts_an_in_flight_run() {
        let mut gate = IssueSpendGate::new();
        let mut ledger = CostLedger::new();
        let dispatched = gate
            .reserve_run(
                &mut ledger,
                tenant(),
                run(1),
                IssueRunKind::SlaDraft,
                MicroUsd(500),
                MicroUsd(1_000),
            )
            .expect("a funded run is dispatched");
        assert_eq!(
            ledger.state_of(&tenant(), &run(1)),
            Some(ReservationState::InFlight),
            "the dispatched run is in-flight"
        );
        assert!(
            ledger.cancel_unstarted(&tenant(), &run(1)).is_err(),
            "an in-flight run is NEVER interrupted"
        );
        assert_eq!(ledger.inflight_interrupt_count(), 0, "0 interrupts");
        let outcome = dispatched.settle(&mut ledger, &units(200, 50)).unwrap();
        assert_eq!(outcome.billed_total, MicroUsd(250));
        assert_eq!(
            ledger.state_of(&tenant(), &run(1)),
            Some(ReservationState::Settled)
        );
    }

    #[test]
    fn a_run_billed_at_its_full_reserve_is_balanced_with_zero_refund() {
        let mut gate = IssueSpendGate::new();
        let mut ledger = CostLedger::new();
        let signal = spend_bearing_run(
            &mut gate,
            &mut ledger,
            tenant(),
            run(1),
            IssueRunKind::Automation,
            MicroUsd(400),
            MicroUsd(400),
            || units(300, 100),
        )
        .expect("a run billed at its full reserve");
        assert_eq!(signal.billed, MicroUsd(400));
        assert_eq!(signal.refunded, MicroUsd(0), "nothing to refund");
        assert!(
            signal.is_green(),
            "reserve == billed (refund 0) is balanced"
        );
    }

    #[test]
    fn redispatch_of_a_live_run_is_rejected() {
        let mut gate = IssueSpendGate::new();
        let mut ledger = CostLedger::new();
        gate.reserve_run(
            &mut ledger,
            tenant(),
            run(1),
            IssueRunKind::Triage,
            MicroUsd(100),
            MicroUsd(1_000),
        )
        .unwrap();
        let err = gate
            .reserve_run(
                &mut ledger,
                tenant(),
                run(1),
                IssueRunKind::Triage,
                MicroUsd(100),
                MicroUsd(1_000),
            )
            .expect_err("a second dispatch of a live run is rejected");
        assert_eq!(err, SpendError::AlreadyDispatched);
    }

    #[test]
    fn double_settle_never_double_charges() {
        let mut gate = IssueSpendGate::new();
        let mut ledger = CostLedger::new();
        let dispatched = gate
            .reserve_run(
                &mut ledger,
                tenant(),
                run(1),
                IssueRunKind::Triage,
                MicroUsd(1_000),
                MicroUsd(1_000),
            )
            .unwrap();
        let first = dispatched.settle(&mut ledger, &units(300, 100)).unwrap();
        let second = dispatched.settle(&mut ledger, &units(300, 100)).unwrap();
        assert_eq!(first, second, "a double-settle returns the same outcome");
        assert_eq!(
            ledger.cost_events_for(&tenant(), &run(1)).len(),
            1,
            "no further cost events on the second settle (no double-charge)"
        );
    }

    #[test]
    fn every_run_kind_bills_the_frozen_agent_effect_dimension() {
        for kind in [
            IssueRunKind::Triage,
            IssueRunKind::Forecast,
            IssueRunKind::SlaDraft,
            IssueRunKind::Automation,
        ] {
            assert_eq!(kind.metered_unit(), "agent.effect");
        }
        assert_eq!(IssueRunKind::Triage.as_str(), "triage");
        assert_eq!(IssueRunKind::Forecast.as_str(), "forecast");
        assert_eq!(IssueRunKind::SlaDraft.as_str(), "sla_draft");
        assert_eq!(IssueRunKind::Automation.as_str(), "automation");
    }

    #[test]
    fn an_unbalanced_signal_is_not_green() {
        let leaked = BalancedRunSignal {
            tenant: tenant(),
            run: run(1),
            kind: IssueRunKind::Triage,
            reserved: MicroUsd(1_000),
            billed: MicroUsd(400),
            refunded: MicroUsd(100),
            cost_events: 1,
            metered_units: 1,
            inflight_interrupt_count: 0,
        };
        assert!(!leaked.is_green(), "a leaked reserve must read RED");

        let mismatch = BalancedRunSignal {
            tenant: tenant(),
            run: run(1),
            kind: IssueRunKind::Triage,
            reserved: MicroUsd(1_000),
            billed: MicroUsd(400),
            refunded: MicroUsd(600),
            cost_events: 1,
            metered_units: 2,
            inflight_interrupt_count: 0,
        };
        assert!(!mismatch.is_green(), "a cost-event/unit mismatch reads RED");

        let interrupted = BalancedRunSignal {
            tenant: tenant(),
            run: run(1),
            kind: IssueRunKind::Triage,
            reserved: MicroUsd(1_000),
            billed: MicroUsd(400),
            refunded: MicroUsd(600),
            cost_events: 1,
            metered_units: 1,
            inflight_interrupt_count: 1,
        };
        assert!(!interrupted.is_green(), "an in-flight interrupt reads RED");
    }

    #[test]
    fn per_effect_idem_key_follows_the_frozen_oq_f_rule() {
        assert_eq!(
            per_effect_idem_key("card:R1:triage", 0, 1),
            "card:R1:triage"
        );
        assert_eq!(
            per_effect_idem_key("card:R1:batch", 0, 3),
            "card:R1:batch:0"
        );
        assert_eq!(
            per_effect_idem_key("card:R1:batch", 1, 3),
            "card:R1:batch:1"
        );
        assert_eq!(
            per_effect_idem_key("card:R1:batch", 2, 3),
            "card:R1:batch:2"
        );
        let k0 = per_effect_idem_key("card:R1:batch", 0, 3);
        let k1 = per_effect_idem_key("card:R1:batch", 1, 3);
        let k2 = per_effect_idem_key("card:R1:batch", 2, 3);
        assert_ne!(k0, k1);
        assert_ne!(k1, k2);
        assert_ne!(k0, k2);
    }

    #[test]
    #[should_panic(expected = "must index into")]
    fn per_effect_idem_key_panics_on_out_of_range_idx() {
        let _ = per_effect_idem_key("card", 3, 3);
    }

    #[test]
    fn spend_error_displays_are_loud() {
        let e = SpendError::NoBalance {
            requested: MicroUsd(9_000),
            available: MicroUsd(100),
        }
        .to_string();
        assert!(
            e.contains("no balance, no start"),
            "must cite the floor: {e}"
        );
        assert!(
            e.contains("NEVER started"),
            "must say it never started: {e}"
        );
        assert!(!SpendError::AlreadyDispatched.to_string().is_empty());
        assert!(!SpendError::AmountOverflow.to_string().is_empty());
        assert!(SpendError::Settle(SettleError::NoSuchReservation)
            .to_string()
            .contains("settle failed"));
    }

    #[test]
    fn dispatch_error_maps_to_spend_error() {
        assert_eq!(
            SpendError::from(DispatchError::AlreadyDispatched),
            SpendError::AlreadyDispatched
        );
        assert_eq!(
            SpendError::from(DispatchError::AmountOverflow),
            SpendError::AmountOverflow
        );
        assert_eq!(
            SpendError::from(DispatchError::NoBalance {
                requested: MicroUsd(5),
                available: MicroUsd(1),
            }),
            SpendError::NoBalance {
                requested: MicroUsd(5),
                available: MicroUsd(1),
            }
        );
    }
}
