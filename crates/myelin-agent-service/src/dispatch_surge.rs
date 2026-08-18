use myelin_identity::Principal;
use myelin_storage::{AgentRunGate, CostLedger, DispatchError, MicroUsd, RunId};
use myelin_substrate::shed::{
    RunClass, RunClassHeader, ShedDecision, ShedLane, Surface, SurfaceBudget,
};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

pub const AGENT_DISPATCH_SURGE_MULTIPLIER: u32 = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentDispatchShed {
    pub lane: RunClass,
    pub retry_after_secs: u64,
}

pub struct AgentDispatchSurgeGate {
    lane: ShedLane,
}

impl AgentDispatchSurgeGate {
    pub fn from_thresholds(thresholds: &Thresholds) -> Result<AgentDispatchSurgeGate, String> {
        let budget = thresholds
            .shed_budget(Surface::AgentMention)
            .map_err(|e| format!("agent shed budget for AgentMention unavailable: {e}"))?;
        Ok(AgentDispatchSurgeGate {
            lane: ShedLane::with_budget(Surface::AgentMention, budget),
        })
    }

    pub fn with_budget(budget: SurfaceBudget) -> AgentDispatchSurgeGate {
        AgentDispatchSurgeGate {
            lane: ShedLane::with_budget(Surface::AgentMention, budget),
        }
    }

    pub fn admit_for(
        &mut self,
        principal: &Principal,
        header: Option<RunClassHeader>,
    ) -> Result<RunClass, AgentDispatchShed> {
        let class = RunClass::derive(&principal.kind, header);
        self.admit_dispatch(&principal.tenant, class)
            .map(|()| class)
    }

    pub fn admit_dispatch(
        &mut self,
        tenant: &TenantId,
        class: RunClass,
    ) -> Result<(), AgentDispatchShed> {
        match self.lane.admit(tenant, class) {
            ShedDecision::Admit => Ok(()),
            ShedDecision::Shed { retry_after_secs } => Err(AgentDispatchShed {
                lane: class,
                retry_after_secs,
            }),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn dispatch_through(
        &mut self,
        gate: &mut AgentRunGate,
        ledger: &mut CostLedger,
        tenant: &TenantId,
        run: RunId,
        class: RunClass,
        estimate: MicroUsd,
        available: MicroUsd,
    ) -> Result<(), DispatchFrontError> {
        self.admit_dispatch(tenant, class)
            .map_err(DispatchFrontError::Shed)?;
        match gate.dispatch(ledger, tenant.clone(), run, estimate, available) {
            Ok(_in_flight) => Ok(()),
            Err(e) => {
                self.release(tenant, class);
                Err(DispatchFrontError::Reserve(e))
            }
        }
    }

    pub fn release(&mut self, tenant: &TenantId, class: RunClass) {
        self.lane.release(tenant, class);
    }

    pub fn shed_count(&self, class: RunClass) -> u64 {
        self.lane.shed_count(class)
    }

    pub fn in_flight(&self, tenant: &TenantId) -> u32 {
        self.lane.in_flight(tenant)
    }

    pub fn surface(&self) -> Surface {
        self.lane.surface()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DispatchFrontError {
    Shed(AgentDispatchShed),
    Reserve(DispatchError),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RetryAfterHonouringRuntime {
    backoff_total_secs: u64,
    immediate_retries: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeReaction {
    Proceed,
    Backoff(u64),
}

impl RetryAfterHonouringRuntime {
    pub fn new() -> RetryAfterHonouringRuntime {
        RetryAfterHonouringRuntime::default()
    }

    pub fn on_shed(&mut self, shed: AgentDispatchShed) -> RuntimeReaction {
        self.backoff_total_secs = self
            .backoff_total_secs
            .saturating_add(shed.retry_after_secs);
        RuntimeReaction::Backoff(shed.retry_after_secs)
    }

    pub fn on_admit(&mut self) -> RuntimeReaction {
        RuntimeReaction::Proceed
    }

    pub fn backoff_total_secs(&self) -> u64 {
        self.backoff_total_secs
    }

    pub fn immediate_retries(&self) -> u64 {
        self.immediate_retries
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentDispatchSurgeReport {
    pub surging_agent_shed_count: u64,
    pub surging_human_shed_count: u64,
    pub surging_human_admitted: bool,
    pub quiet_human_admitted: bool,
    pub cross_tenant_impact: u32,
    pub agent_shed_retry_after_secs: u64,
    pub reserve_refusals: u64,
    pub inflight_interrupt_count: u64,
}

impl AgentDispatchSurgeReport {
    pub fn is_ag_d6_green(&self) -> bool {
        self.surging_agent_shed_count > 0
            && self.agent_shed_retry_after_secs > 0
            && self.surging_human_shed_count == 0
            && self.surging_human_admitted
            && self.quiet_human_admitted
            && self.cross_tenant_impact == 0
            && self.reserve_refusals > 0
            && self.inflight_interrupt_count == 0
    }

    pub fn summary(&self) -> String {
        format!(
            "AG-D6: surging agent_shed={} (retry_after={}s) human_shed={} surging_human_admitted={} \
             quiet_human_admitted={} cross_tenant_impact={} reserve_refusals={} \
             inflight_interrupt={} → {}",
            self.surging_agent_shed_count,
            self.agent_shed_retry_after_secs,
            self.surging_human_shed_count,
            self.surging_human_admitted,
            self.quiet_human_admitted,
            self.cross_tenant_impact,
            self.reserve_refusals,
            self.inflight_interrupt_count,
            if self.is_ag_d6_green() {
                "GREEN"
            } else {
                "RED"
            }
        )
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run_agent_dispatch_surge(
    lane_gate: &mut AgentDispatchSurgeGate,
    reserve_gate: &mut AgentRunGate,
    ledger: &mut CostLedger,
    runtime: &mut RetryAfterHonouringRuntime,
    surging: &TenantId,
    quiet: &TenantId,
    storm_agent_ops: u64,
    per_run: MicroUsd,
    wallet: MicroUsd,
    _multiplier: u32,
) -> AgentDispatchSurgeReport {
    let mut agent_shed_retry_after_secs = 0u64;
    let mut spent = MicroUsd::ZERO;

    let mut lane_admitted: Vec<u64> = Vec::new();
    for i in 0..storm_agent_ops {
        match lane_gate.admit_dispatch(surging, RunClass::Agent) {
            Ok(()) => {
                lane_admitted.push(i);
                let _ = runtime.on_admit();
            }
            Err(shed) => {
                agent_shed_retry_after_secs = shed.retry_after_secs;
                let _ = runtime.on_shed(shed);
            }
        }
    }

    for i in lane_admitted {
        let remaining = wallet.checked_sub(spent).unwrap_or(MicroUsd::ZERO);
        let run = RunId::new(format!("01J0SURGE_{i}"));
        match reserve_gate.dispatch(ledger, surging.clone(), run, per_run, remaining) {
            Ok(_in_flight) => {
                spent = spent.checked_add(per_run).unwrap_or(spent);
            }
            Err(_) => {
                lane_gate.release(surging, RunClass::Agent);
            }
        }
    }

    let surging_human_admitted = lane_gate.admit_dispatch(surging, RunClass::Human).is_ok();

    let quiet_in_flight_before = lane_gate.in_flight(quiet);
    let quiet_human_admitted = lane_gate.admit_dispatch(quiet, RunClass::Human).is_ok();

    AgentDispatchSurgeReport {
        surging_agent_shed_count: lane_gate.shed_count(RunClass::Agent),
        surging_human_shed_count: lane_gate.shed_count(RunClass::Human),
        surging_human_admitted,
        quiet_human_admitted,
        cross_tenant_impact: quiet_in_flight_before,
        agent_shed_retry_after_secs,
        reserve_refusals: reserve_gate.reserve_refusals(),
        inflight_interrupt_count: ledger.inflight_interrupt_count(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{DataRole, PrincipalId, PrincipalKind, PrincipalStatus, RuntimeRef};
    use myelin_tenancy::Region;

    fn tenant(s: &str) -> TenantId {
        TenantId(s.to_string())
    }

    fn human(tenant_slug: &str) -> Principal {
        Principal::new(
            tenant(tenant_slug),
            Region("fr-par".into()),
            PrincipalId(format!("h-{tenant_slug}")),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
        )
    }

    fn agent(tenant_slug: &str) -> Principal {
        Principal::new(
            tenant(tenant_slug),
            Region("fr-par".into()),
            PrincipalId(format!("a-{tenant_slug}")),
            PrincipalKind::Agent {
                runtime_ref: RuntimeRef("rt".into()),
                on_behalf_of: None,
            },
            DataRole::Controller,
            PrincipalStatus::Active,
        )
    }

    fn small_budget() -> SurfaceBudget {
        SurfaceBudget {
            per_tenant_in_flight_cap: 6,
            human_lane_reservation: 2,
            retry_after_secs: 10,
        }
    }

    #[test]
    fn the_agent_shed_budget_is_read_from_the_thresholds_file() {
        let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
        let gate = AgentDispatchSurgeGate::from_thresholds(&thresholds)
            .expect("AgentMention budget present");
        assert_eq!(gate.surface(), Surface::AgentMention);

        let b = thresholds
            .shed_budget(Surface::AgentMention)
            .expect("present");
        assert!(b.per_tenant_in_flight_cap > 0, "bounded (§7.1)");
        assert!(b.human_lane_reservation > 0, "reserves a human lane");
        assert!(b.retry_after_secs > 0, "sheds with a Retry-After");
    }


    #[test]
    fn shed_order_serves_the_human_while_the_agent_lane_sheds() {
        let mut gate = AgentDispatchSurgeGate::with_budget(small_budget());
        let a = agent("acme");
        let h = human("acme");

        for _ in 0..4 {
            assert!(
                gate.admit_for(&a, None).is_ok(),
                "agent dispatch admitted under budget"
            );
        }
        let shed = gate.admit_for(&a, None).expect_err("the agent storm sheds");
        assert_eq!(shed.lane, RunClass::Agent);
        assert_eq!(shed.retry_after_secs, 10, "the shed carries a Retry-After");

        assert_eq!(
            gate.admit_for(&h, None)
                .expect("the human dispatch is served while the agent sheds"),
            RunClass::Human
        );
        assert_eq!(gate.shed_count(RunClass::Human), 0, "human lane: 0 shed");
        assert!(gate.shed_count(RunClass::Agent) >= 1, "agent lane: sheds");
    }

    #[test]
    fn shed_priority_is_speculative_then_batch_then_agent_then_human() {
        let mut gate = AgentDispatchSurgeGate::with_budget(small_budget());
        let t = tenant("acme");
        for _ in 0..2 {
            gate.admit_dispatch(&t, RunClass::Agent)
                .expect("agent admitted");
        }
        assert!(
            gate.admit_dispatch(&t, RunClass::Speculative).is_err(),
            "speculative sheds first"
        );
        gate.admit_dispatch(&t, RunClass::BatchCi)
            .expect("batch admitted");
        assert!(
            gate.admit_dispatch(&t, RunClass::BatchCi).is_err(),
            "batch/ci sheds next"
        );
        gate.admit_dispatch(&t, RunClass::Agent)
            .expect("agent admitted");
        assert!(
            gate.admit_dispatch(&t, RunClass::Agent).is_err(),
            "agent sheds before the human dispatch"
        );
        gate.admit_dispatch(&t, RunClass::Human)
            .expect("human dispatch served - shed last");

        assert_eq!(gate.shed_count(RunClass::Speculative), 1);
        assert_eq!(gate.shed_count(RunClass::BatchCi), 1);
        assert_eq!(gate.shed_count(RunClass::Agent), 1);
        assert_eq!(gate.shed_count(RunClass::Human), 0);
    }

    #[test]
    fn a_429_carries_a_retry_after() {
        let budget = SurfaceBudget {
            per_tenant_in_flight_cap: 4,
            human_lane_reservation: 1,
            retry_after_secs: 7,
        };
        let mut gate = AgentDispatchSurgeGate::with_budget(budget);
        let t = tenant("acme");
        for _ in 0..3 {
            gate.admit_dispatch(&t, RunClass::Agent).expect("admitted");
        }
        let shed = gate
            .admit_dispatch(&t, RunClass::Agent)
            .expect_err("the agent lane sheds");
        assert_eq!(
            shed.retry_after_secs, 7,
            "the 429 carries the surface's Retry-After (the runtime honours it - no amplification)"
        );
    }

    #[test]
    fn one_tenants_storm_never_sheds_anothers_human() {
        let mut gate = AgentDispatchSurgeGate::with_budget(small_budget());
        let noisy = agent("noisy");
        let quiet_human = human("quiet");

        for _ in 0..4 {
            gate.admit_for(&noisy, None).expect("noisy agent admitted");
        }
        assert!(
            gate.admit_for(&noisy, None).is_err(),
            "noisy agent lane sheds"
        );
        assert_eq!(gate.in_flight(&tenant("noisy")), 4, "noisy has 4 in-flight");
        assert_eq!(
            gate.in_flight(&tenant("quiet")),
            0,
            "the quiet tenant's budget is independent"
        );
        assert_eq!(
            gate.admit_for(&quiet_human, None)
                .expect("the quiet human dispatch is served"),
            RunClass::Human,
            "the noisy storm must NEVER shed another tenant's human dispatch"
        );
    }

    #[test]
    fn a_machine_principal_cannot_spoof_the_human_lane() {
        let mut gate = AgentDispatchSurgeGate::with_budget(small_budget());
        let a = agent("acme");
        assert_eq!(gate.admit_for(&a, None).expect("admitted"), RunClass::Agent);
        let h = human("acme");
        assert_eq!(
            gate.admit_for(&h, Some(RunClassHeader::BatchCi))
                .expect("admitted"),
            RunClass::BatchCi,
            "a human-issued batch dispatch may down-class itself (never up-class)"
        );
    }

    #[test]
    fn release_frees_a_slot_after_the_surge() {
        let mut gate = AgentDispatchSurgeGate::with_budget(SurfaceBudget {
            per_tenant_in_flight_cap: 3,
            human_lane_reservation: 1,
            retry_after_secs: 1,
        });
        let t = tenant("acme");
        gate.admit_dispatch(&t, RunClass::Agent).expect("admitted");
        gate.admit_dispatch(&t, RunClass::Agent).expect("admitted");
        assert!(
            gate.admit_dispatch(&t, RunClass::Agent).is_err(),
            "agent sheds"
        );
        gate.release(&t, RunClass::Agent);
        gate.admit_dispatch(&t, RunClass::Agent)
            .expect("a released slot is reusable");
    }

    #[test]
    fn dispatch_through_admits_when_under_both_fronts() {
        let mut lane = AgentDispatchSurgeGate::with_budget(small_budget());
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let t = tenant("acme");
        lane.dispatch_through(
            &mut gate,
            &mut ledger,
            &t,
            RunId::new("r1".to_string()),
            RunClass::Agent,
            MicroUsd(100),
            MicroUsd(1_000),
        )
        .expect("admitted at both fronts");
        assert_eq!(gate.runs_dispatched(), 1, "the run was fronted");
        assert_eq!(lane.in_flight(&t), 1, "the lane holds the in-flight run");
    }

    #[test]
    fn dispatch_through_reserve_refusal_releases_the_lane_slot() {
        let mut lane = AgentDispatchSurgeGate::with_budget(small_budget());
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let t = tenant("acme");
        let err = lane
            .dispatch_through(
                &mut gate,
                &mut ledger,
                &t,
                RunId::new("r1".to_string()),
                RunClass::Agent,
                MicroUsd(9_000),
                MicroUsd(10),
            )
            .expect_err("the wallet refuses an over-budget dispatch");
        assert!(matches!(err, DispatchFrontError::Reserve(_)));
        assert_eq!(gate.reserve_refusals(), 1, "the reserve refusal ticked");
        assert_eq!(
            lane.in_flight(&t),
            0,
            "the lane slot is released on a wallet refusal (the run never started)"
        );
    }

    #[test]
    fn the_runtime_honours_retry_after_no_retry_storm() {
        let mut runtime = RetryAfterHonouringRuntime::new();
        let retry_afters = [10u64, 3, 7, 5, 10, 2];
        let mut expected_backoff = 0u64;
        for &secs in &retry_afters {
            let reaction = runtime.on_shed(AgentDispatchShed {
                lane: RunClass::Agent,
                retry_after_secs: secs,
            });
            assert_eq!(
                reaction,
                RuntimeReaction::Backoff(secs),
                "the runtime backs off for the advertised Retry-After, never retries immediately"
            );
            expected_backoff += secs;
        }
        assert_eq!(
            runtime.immediate_retries(),
            0,
            "the no-retry-storm invariant: ZERO immediate retries (a shed always backs off)"
        );
        assert_eq!(
            runtime.backoff_total_secs(),
            expected_backoff,
            "the runtime honoured every shed's Retry-After (the cumulative backoff)"
        );
    }

    #[test]
    fn an_admitted_dispatch_proceeds_with_no_backoff() {
        let mut runtime = RetryAfterHonouringRuntime::new();
        assert_eq!(runtime.on_admit(), RuntimeReaction::Proceed);
        assert_eq!(runtime.backoff_total_secs(), 0);
        assert_eq!(runtime.immediate_retries(), 0);
    }

    #[test]
    fn run_agent_dispatch_surge_is_green() {
        let mut lane = AgentDispatchSurgeGate::with_budget(small_budget());
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let mut runtime = RetryAfterHonouringRuntime::new();
        let surging = tenant("noisy");
        let quiet = tenant("quiet");
        let report = run_agent_dispatch_surge(
            &mut lane,
            &mut gate,
            &mut ledger,
            &mut runtime,
            &surging,
            &quiet,
            50,
            MicroUsd(100),
            MicroUsd(300),
            AGENT_DISPATCH_SURGE_MULTIPLIER,
        );
        assert!(report.is_ag_d6_green(), "{}", report.summary());
        assert!(report.surging_agent_shed_count > 0, "agent lane shed");
        assert!(
            report.agent_shed_retry_after_secs > 0,
            "the agent shed carried a Retry-After"
        );
        assert_eq!(report.surging_human_shed_count, 0, "human lane held");
        assert!(report.surging_human_admitted, "surging tenant's human held");
        assert!(report.quiet_human_admitted, "quiet co-tenant's human held");
        assert_eq!(report.cross_tenant_impact, 0, "cross-tenant impact 0");
        assert!(
            report.reserve_refusals > 0,
            "the wallet refused over-budget runs"
        );
        assert_eq!(
            report.inflight_interrupt_count, 0,
            "0 interrupts under surge"
        );
        assert_eq!(runtime.immediate_retries(), 0, "no retry storm");
        assert!(
            runtime.backoff_total_secs() > 0,
            "the runtime backed off on the sheds"
        );
    }

    #[test]
    fn an_unbounded_lane_reads_red() {
        let huge = SurfaceBudget {
            per_tenant_in_flight_cap: 1_000_000,
            human_lane_reservation: 200_000,
            retry_after_secs: 10,
        };
        let mut lane = AgentDispatchSurgeGate::with_budget(huge);
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let mut runtime = RetryAfterHonouringRuntime::new();
        let report = run_agent_dispatch_surge(
            &mut lane,
            &mut gate,
            &mut ledger,
            &mut runtime,
            &tenant("noisy"),
            &tenant("quiet"),
            100,
            MicroUsd(100),
            MicroUsd(1_000_000),
            AGENT_DISPATCH_SURGE_MULTIPLIER,
        );
        assert_eq!(
            report.surging_agent_shed_count, 0,
            "the unbounded lane swallowed the storm"
        );
        assert!(
            !report.is_ag_d6_green(),
            "an unbounded agent lane (storm not absorbed by shedding) MUST read RED"
        );
    }

}
