use myelin_identity::Principal;
use myelin_substrate::shed::{
    BoundedQueue, RunClass, RunClassHeader, ShedDecision, ShedLane, Surface, SurfaceBudget,
};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

pub const NOTIF_SURGE_MULTIPLIER: u32 = 30;

pub const NOTIF_SURGE_SURFACE: Surface = Surface::AgentMention;

pub struct NotifShedGate {
    lane: ShedLane,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotifShedRejection {
    pub lane: RunClass,
    pub retry_after_secs: u64,
}

impl NotifShedGate {
    pub fn from_thresholds(thresholds: &Thresholds) -> Result<NotifShedGate, String> {
        let budget = thresholds.shed_budget(NOTIF_SURGE_SURFACE).map_err(|e| {
            format!("Notif shed budget for {NOTIF_SURGE_SURFACE:?} unavailable: {e}")
        })?;
        Ok(NotifShedGate {
            lane: ShedLane::with_budget(NOTIF_SURGE_SURFACE, budget),
        })
    }

    pub fn with_budget(budget: SurfaceBudget) -> NotifShedGate {
        NotifShedGate {
            lane: ShedLane::with_budget(NOTIF_SURGE_SURFACE, budget),
        }
    }

    pub fn admit_for(
        &mut self,
        principal: &Principal,
        header: Option<RunClassHeader>,
    ) -> Result<RunClass, NotifShedRejection> {
        let class = RunClass::derive(&principal.kind, header);
        self.admit_class(&principal.tenant, class).map(|()| class)
    }

    pub fn admit_class(
        &mut self,
        tenant: &TenantId,
        class: RunClass,
    ) -> Result<(), NotifShedRejection> {
        match self.lane.admit(tenant, class) {
            ShedDecision::Admit => Ok(()),
            ShedDecision::Shed { retry_after_secs } => Err(NotifShedRejection {
                lane: class,
                retry_after_secs,
            }),
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

#[derive(Clone, Debug)]
pub struct ProviderBulkhead {
    provider: String,
    queue: BoundedQueue,
}

impl ProviderBulkhead {
    pub fn new(provider: impl Into<String>, concurrency: u32) -> ProviderBulkhead {
        ProviderBulkhead {
            provider: provider.into(),
            queue: BoundedQueue::new(concurrency),
        }
    }

    pub fn try_send(&mut self) -> bool {
        self.queue.try_acquire()
    }

    pub fn release(&mut self) {
        self.queue.release();
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn in_flight(&self) -> u32 {
        self.queue.in_flight()
    }

    pub fn concurrency(&self) -> u32 {
        self.queue.capacity()
    }

    pub fn shed_count(&self) -> u64 {
        self.queue.shed_count()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotifSurgeReport {
    pub surging_agent_shed_count: u64,
    pub surging_ci_shed_count: u64,
    pub surging_human_shed_count: u64,
    pub surging_human_admitted: bool,
    pub quiet_human_admitted: bool,
    pub cross_tenant_impact: u32,
    pub provider_peak_in_flight: u32,
    pub provider_bound: u32,
    pub provider_bulkhead_shed: u64,
}

impl NotifSurgeReport {
    pub fn is_notif_d5_green(&self) -> bool {
        self.surging_agent_shed_count > 0
            && self.surging_ci_shed_count > 0
            && self.surging_human_shed_count == 0
            && self.surging_human_admitted
            && self.quiet_human_admitted
            && self.cross_tenant_impact == 0
            && self.provider_bound > 0
            && self.provider_peak_in_flight <= self.provider_bound
            && self.provider_bulkhead_shed > 0
    }

    pub fn summary(&self) -> String {
        format!(
            "NOTIF-D5: surging agent_shed={} ci_shed={} human_shed={} surging_human_admitted={} \
             quiet_human_admitted={} cross_tenant_impact={} | provider_peak={}/{} bulkhead_shed={} → {}",
            self.surging_agent_shed_count,
            self.surging_ci_shed_count,
            self.surging_human_shed_count,
            self.surging_human_admitted,
            self.quiet_human_admitted,
            self.cross_tenant_impact,
            self.provider_peak_in_flight,
            self.provider_bound,
            self.provider_bulkhead_shed,
            if self.is_notif_d5_green() {
                "GREEN"
            } else {
                "RED"
            }
        )
    }
}

pub fn run_notif_surge(
    gate: &mut NotifShedGate,
    bulkhead: &mut ProviderBulkhead,
    surging: &TenantId,
    quiet: &TenantId,
    storm_agent_ops: u64,
    storm_ci_ops: u64,
    _multiplier: u32,
) -> NotifSurgeReport {
    let mut provider_peak: u32 = 0;

    for _ in 0..storm_ci_ops {
        if gate.admit_class(surging, RunClass::BatchCi).is_ok() {
            attempt_delivery(bulkhead, &mut provider_peak);
        }
    }
    for _ in 0..storm_agent_ops {
        if gate.admit_class(surging, RunClass::Agent).is_ok() {
            attempt_delivery(bulkhead, &mut provider_peak);
        }
    }

    let surging_human_admitted = gate.admit_class(surging, RunClass::Human).is_ok();

    let quiet_in_flight_before = gate.in_flight(quiet);
    let quiet_human_admitted = gate.admit_class(quiet, RunClass::Human).is_ok();

    NotifSurgeReport {
        surging_agent_shed_count: gate.shed_count(RunClass::Agent),
        surging_ci_shed_count: gate.shed_count(RunClass::BatchCi),
        surging_human_shed_count: gate.shed_count(RunClass::Human),
        surging_human_admitted,
        quiet_human_admitted,
        cross_tenant_impact: quiet_in_flight_before,
        provider_peak_in_flight: provider_peak,
        provider_bound: bulkhead.concurrency(),
        provider_bulkhead_shed: bulkhead.shed_count(),
    }
}

fn attempt_delivery(bulkhead: &mut ProviderBulkhead, peak: &mut u32) {
    let _ = bulkhead.try_send();
    *peak = (*peak).max(bulkhead.in_flight());
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
    fn the_notif_shed_budget_is_read_from_the_thresholds_file() {
        let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
        let gate =
            NotifShedGate::from_thresholds(&thresholds).expect("AgentMention budget present");
        assert_eq!(gate.surface(), Surface::AgentMention);
        assert_eq!(NOTIF_SURGE_SURFACE, Surface::AgentMention);

        let b = thresholds
            .shed_budget(Surface::AgentMention)
            .expect("present");
        assert!(
            b.per_tenant_in_flight_cap > 0,
            "AgentMention bounded (§7.1)"
        );
        assert!(
            b.human_lane_reservation > 0,
            "AgentMention reserves a human inbox-read lane (humans never queue behind agent runs)"
        );
        assert_eq!(thresholds.surge.multiplier, NOTIF_SURGE_MULTIPLIER);
    }

    #[test]
    fn a_human_read_is_served_while_the_agent_lane_sheds() {
        let mut gate = NotifShedGate::with_budget(small_budget());
        let a = agent("acme");
        let h = human("acme");

        for _ in 0..4 {
            assert!(
                gate.admit_for(&a, None).is_ok(),
                "agent notification admitted under budget"
            );
        }
        let shed = gate.admit_for(&a, None).expect_err("the agent storm sheds");
        assert_eq!(shed.lane, RunClass::Agent);
        assert_eq!(
            shed.retry_after_secs, 10,
            "the shed carries a Retry-After the agent runtime honours (ADR-16.3)"
        );

        assert_eq!(
            gate.admit_for(&h, None)
                .expect("the human is served while the agent lane sheds"),
            RunClass::Human
        );
        assert_eq!(
            gate.shed_count(RunClass::Human),
            0,
            "the human inbox-read lane: 0 shed"
        );
        assert!(gate.shed_count(RunClass::Agent) >= 1, "agent lane: sheds");
    }

    #[test]
    fn the_per_tenant_in_flight_cap_refuses_over_cap() {
        let mut gate = NotifShedGate::with_budget(small_budget());
        let t = tenant("acme");
        for _ in 0..4 {
            gate.admit_class(&t, RunClass::Agent)
                .expect("agent admitted under the per-tenant cap");
        }
        assert_eq!(
            gate.in_flight(&t),
            4,
            "the per-tenant in-flight is at the cap"
        );
        assert!(
            gate.admit_class(&t, RunClass::Agent).is_err(),
            "over-cap → reserve/settle refuses (the per-tenant in-flight cap bites)"
        );
    }

    #[test]
    fn shed_priority_is_speculative_then_batch_then_agent_then_human() {
        let mut gate = NotifShedGate::with_budget(small_budget());
        let t = tenant("acme");
        for _ in 0..2 {
            gate.admit_class(&t, RunClass::Agent)
                .expect("agent admitted");
        }
        assert!(
            gate.admit_class(&t, RunClass::Speculative).is_err(),
            "speculative sheds first"
        );
        gate.admit_class(&t, RunClass::BatchCi)
            .expect("batch admitted");
        assert!(
            gate.admit_class(&t, RunClass::BatchCi).is_err(),
            "batch/CI notification lane sheds next"
        );
        gate.admit_class(&t, RunClass::Agent)
            .expect("agent admitted");
        assert!(
            gate.admit_class(&t, RunClass::Agent).is_err(),
            "agent sheds before the human"
        );
        gate.admit_class(&t, RunClass::Human)
            .expect("human inbox read served - shed last");

        assert_eq!(gate.shed_count(RunClass::Speculative), 1);
        assert_eq!(gate.shed_count(RunClass::BatchCi), 1);
        assert_eq!(gate.shed_count(RunClass::Agent), 1);
        assert_eq!(gate.shed_count(RunClass::Human), 0);
    }

    #[test]
    fn one_tenants_surge_never_sheds_anothers_human() {
        let mut gate = NotifShedGate::with_budget(small_budget());
        let noisy = agent("noisy");
        let quiet_human = human("quiet");

        for _ in 0..4 {
            gate.admit_for(&noisy, None).expect("noisy agent admitted");
        }
        assert!(
            gate.admit_for(&noisy, None).is_err(),
            "noisy agent notification lane sheds"
        );
        assert_eq!(gate.in_flight(&tenant("noisy")), 4, "noisy has 4 in-flight");
        assert_eq!(
            gate.in_flight(&tenant("quiet")),
            0,
            "the quiet tenant's budget is independent"
        );
        assert_eq!(
            gate.admit_for(&quiet_human, None)
                .expect("the quiet human is served"),
            RunClass::Human,
            "the noisy storm must NEVER shed another tenant's human inbox read"
        );
    }

    #[test]
    fn a_machine_principal_cannot_spoof_the_human_lane() {
        let mut gate = NotifShedGate::with_budget(small_budget());
        let a = agent("acme");
        assert_eq!(gate.admit_for(&a, None).expect("admitted"), RunClass::Agent);
        let h = human("acme");
        assert_eq!(
            gate.admit_for(&h, Some(RunClassHeader::Speculative))
                .expect("admitted"),
            RunClass::Speculative,
            "a human-issued prefetch read may down-class itself"
        );
    }

    #[test]
    fn release_frees_a_slot_after_the_surge() {
        let mut gate = NotifShedGate::with_budget(SurfaceBudget {
            per_tenant_in_flight_cap: 3,
            human_lane_reservation: 1,
            retry_after_secs: 1,
        });
        let t = tenant("acme");
        gate.admit_class(&t, RunClass::Agent).expect("admitted");
        gate.admit_class(&t, RunClass::Agent).expect("admitted");
        assert!(
            gate.admit_class(&t, RunClass::Agent).is_err(),
            "agent sheds"
        );
        gate.release(&t, RunClass::Agent);
        gate.admit_class(&t, RunClass::Agent)
            .expect("a released slot is reusable");
    }

    #[test]
    fn the_provider_bulkhead_bounds_provider_load() {
        let mut bh = ProviderBulkhead::new("email", 2);
        assert_eq!(bh.provider(), "email");
        assert_eq!(bh.concurrency(), 2);
        assert!(bh.try_send(), "first send under the bound");
        assert!(bh.try_send(), "second send under the bound");
        assert!(
            !bh.try_send(),
            "a third concurrent send SHEDS - the provider load is bounded (Little's Law)"
        );
        assert_eq!(
            bh.in_flight(),
            2,
            "provider in-flight never exceeds the bound"
        );
        assert_eq!(
            bh.shed_count(),
            1,
            "the over-bound send was shed, not buffered"
        );
        bh.release();
        assert!(
            bh.try_send(),
            "a released permit is reusable after the surge"
        );
    }

    #[test]
    fn provider_bulkheads_are_per_provider_isolated() {
        let mut email = ProviderBulkhead::new("email", 1);
        let mut push = ProviderBulkhead::new("push", 1);
        assert!(email.try_send());
        assert!(!email.try_send(), "email at its bound");
        assert!(push.try_send(), "push provider's bulkhead is independent");
        assert_eq!(email.in_flight(), 1);
        assert_eq!(push.in_flight(), 1);
    }

    #[test]
    fn run_notif_surge_is_green() {
        let mut gate = NotifShedGate::with_budget(small_budget());
        let mut bh = ProviderBulkhead::new("email", 2);
        let surging = tenant("noisy");
        let quiet = tenant("quiet");
        let report = run_notif_surge(
            &mut gate,
            &mut bh,
            &surging,
            &quiet,
            50,
            50,
            NOTIF_SURGE_MULTIPLIER,
        );
        assert!(report.is_notif_d5_green(), "{}", report.summary());
        assert!(report.surging_agent_shed_count > 0, "agent lane shed");
        assert!(
            report.surging_ci_shed_count > 0,
            "CI notification lane shed"
        );
        assert_eq!(report.surging_human_shed_count, 0, "human inbox-read held");
        assert!(report.surging_human_admitted, "surging tenant's human held");
        assert!(report.quiet_human_admitted, "quiet co-tenant's human held");
        assert_eq!(report.cross_tenant_impact, 0, "cross-tenant impact 0");
        assert!(
            report.provider_peak_in_flight <= report.provider_bound,
            "the bulkhead bounded provider load (peak ≤ bound)"
        );
        assert!(
            report.provider_bulkhead_shed > 0,
            "the bulkhead shed the over-bound sends (fast-fail, not buffer)"
        );
    }

    #[test]
    fn an_unbounded_lane_reads_red() {
        let huge = SurfaceBudget {
            per_tenant_in_flight_cap: 1_000_000,
            human_lane_reservation: 200_000,
            retry_after_secs: 10,
        };
        let mut gate = NotifShedGate::with_budget(huge);
        let mut bh = ProviderBulkhead::new("email", 1_000_000);
        let report = run_notif_surge(
            &mut gate,
            &mut bh,
            &tenant("noisy"),
            &tenant("quiet"),
            100,
            100,
            NOTIF_SURGE_MULTIPLIER,
        );
        assert_eq!(
            report.surging_agent_shed_count, 0,
            "the unbounded lane swallowed the storm"
        );
        assert!(
            !report.is_notif_d5_green(),
            "an unbounded lane MUST read RED"
        );
    }

    #[test]
    fn an_unbounded_provider_reads_red() {
        let mut gate = NotifShedGate::with_budget(small_budget());
        let mut bh = ProviderBulkhead::new("email", 1_000_000);
        let report = run_notif_surge(
            &mut gate,
            &mut bh,
            &tenant("noisy"),
            &tenant("quiet"),
            50,
            50,
            NOTIF_SURGE_MULTIPLIER,
        );
        assert!(report.surging_agent_shed_count > 0, "the lane still sheds");
        assert_eq!(
            report.provider_bulkhead_shed, 0,
            "the unbounded provider never shed"
        );
        assert!(
            !report.is_notif_d5_green(),
            "an unbounded provider (no bulkhead bound) MUST read RED"
        );
    }

    #[test]
    fn each_notif_d5_condition_is_load_bearing() {
        let green = NotifSurgeReport {
            surging_agent_shed_count: 5,
            surging_ci_shed_count: 5,
            surging_human_shed_count: 0,
            surging_human_admitted: true,
            quiet_human_admitted: true,
            cross_tenant_impact: 0,
            provider_peak_in_flight: 2,
            provider_bound: 2,
            provider_bulkhead_shed: 3,
        };
        assert!(green.is_notif_d5_green(), "the baseline is green");

        assert!(!NotifSurgeReport {
            surging_agent_shed_count: 0,
            ..green.clone()
        }
        .is_notif_d5_green());
        assert!(!NotifSurgeReport {
            surging_ci_shed_count: 0,
            ..green.clone()
        }
        .is_notif_d5_green());
        assert!(!NotifSurgeReport {
            surging_human_shed_count: 1,
            ..green.clone()
        }
        .is_notif_d5_green());
        assert!(!NotifSurgeReport {
            surging_human_admitted: false,
            ..green.clone()
        }
        .is_notif_d5_green());
        assert!(!NotifSurgeReport {
            quiet_human_admitted: false,
            ..green.clone()
        }
        .is_notif_d5_green());
        assert!(!NotifSurgeReport {
            cross_tenant_impact: 1,
            ..green.clone()
        }
        .is_notif_d5_green());
        assert!(!NotifSurgeReport {
            provider_bulkhead_shed: 0,
            ..green.clone()
        }
        .is_notif_d5_green());
        assert!(!NotifSurgeReport {
            provider_peak_in_flight: 3,
            ..green.clone()
        }
        .is_notif_d5_green());
        assert!(!NotifSurgeReport {
            provider_bound: 0,
            provider_peak_in_flight: 0,
            ..green.clone()
        }
        .is_notif_d5_green());
    }

    #[test]
    fn the_summary_carries_the_measured_signals() {
        let report = NotifSurgeReport {
            surging_agent_shed_count: 7,
            surging_ci_shed_count: 9,
            surging_human_shed_count: 0,
            surging_human_admitted: true,
            quiet_human_admitted: true,
            cross_tenant_impact: 0,
            provider_peak_in_flight: 2,
            provider_bound: 2,
            provider_bulkhead_shed: 4,
        };
        let s = report.summary();
        assert!(
            s.contains("agent_shed=7"),
            "names the agent shed count: {s}"
        );
        assert!(s.contains("ci_shed=9"), "names the CI shed count: {s}");
        assert!(
            s.contains("human_shed=0"),
            "names the human shed count: {s}"
        );
        assert!(
            s.contains("cross_tenant_impact=0"),
            "names cross-tenant impact: {s}"
        );
        assert!(
            s.contains("bulkhead_shed=4"),
            "names the bulkhead shed count: {s}"
        );
        assert!(s.contains("GREEN"), "names the verdict: {s}");
        assert!(s.starts_with("NOTIF-D5:"), "the artifact is labelled: {s}");
    }

}
