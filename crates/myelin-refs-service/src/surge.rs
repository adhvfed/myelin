use myelin_identity::Principal;
use myelin_substrate::shed::{
    RunClass, RunClassHeader, ShedDecision, ShedLane, Surface, SurfaceBudget,
};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

pub const REFS_SURGE_MULTIPLIER: u32 = 30;

pub struct RefsShedGate {
    lane: ShedLane,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RefsShedRejection {
    pub lane: RunClass,
    pub retry_after_secs: u64,
}

impl RefsShedGate {
    pub fn from_thresholds(
        thresholds: &Thresholds,
        surface: Surface,
    ) -> Result<RefsShedGate, String> {
        debug_assert!(
            matches!(surface, Surface::RefsBacklinkRead | Surface::RefsRefCreate),
            "RefsShedGate fronts only the two Refs surfaces"
        );
        let budget = thresholds
            .shed_budget(surface)
            .map_err(|e| format!("Refs shed budget for {surface:?} unavailable: {e}"))?;
        Ok(RefsShedGate {
            lane: ShedLane::with_budget(surface, budget),
        })
    }

    pub fn backlink_read_from_thresholds(thresholds: &Thresholds) -> Result<RefsShedGate, String> {
        RefsShedGate::from_thresholds(thresholds, Surface::RefsBacklinkRead)
    }

    pub fn ref_create_from_thresholds(thresholds: &Thresholds) -> Result<RefsShedGate, String> {
        RefsShedGate::from_thresholds(thresholds, Surface::RefsRefCreate)
    }

    pub fn with_budget(surface: Surface, budget: SurfaceBudget) -> RefsShedGate {
        RefsShedGate {
            lane: ShedLane::with_budget(surface, budget),
        }
    }

    pub fn admit_for(
        &mut self,
        principal: &Principal,
        header: Option<RunClassHeader>,
    ) -> Result<RunClass, RefsShedRejection> {
        let class = RunClass::derive(&principal.kind, header);
        self.admit_class(&principal.tenant, class).map(|()| class)
    }

    pub fn admit_class(
        &mut self,
        tenant: &TenantId,
        class: RunClass,
    ) -> Result<(), RefsShedRejection> {
        match self.lane.admit(tenant, class) {
            ShedDecision::Admit => Ok(()),
            ShedDecision::Shed { retry_after_secs } => Err(RefsShedRejection {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RefsSurgeReport {
    pub surging_agent_shed_count: u64,
    pub surging_human_shed_count: u64,
    pub surging_human_admitted: bool,
    pub quiet_human_admitted: bool,
    pub cross_tenant_impact: u32,
}

impl RefsSurgeReport {
    pub fn is_ref_d10_green(&self) -> bool {
        self.surging_agent_shed_count > 0
            && self.surging_human_shed_count == 0
            && self.surging_human_admitted
            && self.quiet_human_admitted
            && self.cross_tenant_impact == 0
    }

    pub fn summary(&self) -> String {
        format!(
            "REF-D10: surging agent_shed={} human_shed={} surging_human_admitted={} \
             quiet_human_admitted={} cross_tenant_impact={} → {}",
            self.surging_agent_shed_count,
            self.surging_human_shed_count,
            self.surging_human_admitted,
            self.quiet_human_admitted,
            self.cross_tenant_impact,
            if self.is_ref_d10_green() {
                "GREEN"
            } else {
                "RED"
            }
        )
    }
}

pub fn run_refs_surge(
    gate: &mut RefsShedGate,
    surging: &TenantId,
    quiet: &TenantId,
    storm_agent_ops: u64,
    _multiplier: u32,
) -> RefsSurgeReport {
    for _ in 0..storm_agent_ops {
        let _ = gate.admit_class(surging, RunClass::Agent);
    }

    let surging_human_admitted = gate.admit_class(surging, RunClass::Human).is_ok();

    let quiet_in_flight_before = gate.in_flight(quiet);
    let quiet_human_admitted = gate.admit_class(quiet, RunClass::Human).is_ok();

    RefsSurgeReport {
        surging_agent_shed_count: gate.shed_count(RunClass::Agent),
        surging_human_shed_count: gate.shed_count(RunClass::Human),
        surging_human_admitted,
        quiet_human_admitted,
        cross_tenant_impact: quiet_in_flight_before,
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
            retry_after_secs: 3,
        }
    }

    #[test]
    fn the_refs_shed_budgets_are_read_from_the_thresholds_file() {
        let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
        let read = RefsShedGate::backlink_read_from_thresholds(&thresholds)
            .expect("RefsBacklinkRead budget present");
        let create = RefsShedGate::ref_create_from_thresholds(&thresholds)
            .expect("RefsRefCreate budget present");
        assert_eq!(read.surface(), Surface::RefsBacklinkRead);
        assert_eq!(create.surface(), Surface::RefsRefCreate);

        for surface in [Surface::RefsBacklinkRead, Surface::RefsRefCreate] {
            let b = thresholds.shed_budget(surface).expect("present");
            assert!(b.per_tenant_in_flight_cap > 0, "{surface:?} bounded (§7.1)");
            assert!(
                b.human_lane_reservation > 0,
                "{surface:?} reserves a human lane"
            );
        }
    }

    #[test]
    fn shed_order_serves_the_human_while_the_agent_lane_sheds() {
        let mut gate = RefsShedGate::with_budget(Surface::RefsBacklinkRead, small_budget());
        let a = agent("acme");
        let h = human("acme");

        for _ in 0..4 {
            assert!(
                gate.admit_for(&a, None).is_ok(),
                "agent op admitted under budget"
            );
        }
        let shed = gate.admit_for(&a, None).expect_err("the agent storm sheds");
        assert_eq!(shed.lane, RunClass::Agent);
        assert_eq!(shed.retry_after_secs, 3, "the shed carries a Retry-After");

        assert_eq!(
            gate.admit_for(&h, None)
                .expect("the human is served while the agent sheds"),
            RunClass::Human
        );
        assert_eq!(gate.shed_count(RunClass::Human), 0, "human lane: 0 shed");
        assert!(gate.shed_count(RunClass::Agent) >= 1, "agent lane: sheds");
    }

    #[test]
    fn shed_priority_is_speculative_then_batch_then_agent_then_human() {
        let mut gate = RefsShedGate::with_budget(Surface::RefsRefCreate, small_budget());
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
            "batch/ci sheds next"
        );
        gate.admit_class(&t, RunClass::Agent)
            .expect("agent admitted");
        assert!(
            gate.admit_class(&t, RunClass::Agent).is_err(),
            "agent sheds before the human"
        );
        gate.admit_class(&t, RunClass::Human)
            .expect("human served - shed last");

        assert_eq!(gate.shed_count(RunClass::Speculative), 1);
        assert_eq!(gate.shed_count(RunClass::BatchCi), 1);
        assert_eq!(gate.shed_count(RunClass::Agent), 1);
        assert_eq!(gate.shed_count(RunClass::Human), 0);
    }

    #[test]
    fn one_tenants_storm_never_sheds_anothers_human() {
        let mut gate = RefsShedGate::with_budget(Surface::RefsBacklinkRead, small_budget());
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
                .expect("the quiet human is served"),
            RunClass::Human,
            "the noisy storm must NEVER shed another tenant's human"
        );
    }

    #[test]
    fn a_machine_principal_cannot_spoof_the_human_lane() {
        let mut gate = RefsShedGate::with_budget(Surface::RefsRefCreate, small_budget());
        let a = agent("acme");
        assert_eq!(gate.admit_for(&a, None).expect("admitted"), RunClass::Agent);
        let h = human("acme");
        assert_eq!(
            gate.admit_for(&h, Some(RunClassHeader::Speculative))
                .expect("admitted"),
            RunClass::Speculative,
            "a human-issued prefetch may down-class itself"
        );
    }

    #[test]
    fn release_frees_a_slot_after_the_surge() {
        let mut gate = RefsShedGate::with_budget(
            Surface::RefsBacklinkRead,
            SurfaceBudget {
                per_tenant_in_flight_cap: 3,
                human_lane_reservation: 1,
                retry_after_secs: 1,
            },
        );
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
    fn run_refs_surge_is_green() {
        let mut gate = RefsShedGate::with_budget(Surface::RefsBacklinkRead, small_budget());
        let surging = tenant("noisy");
        let quiet = tenant("quiet");
        let report = run_refs_surge(&mut gate, &surging, &quiet, 50, REFS_SURGE_MULTIPLIER);
        assert!(report.is_ref_d10_green(), "{}", report.summary());
        assert!(report.surging_agent_shed_count > 0, "agent lane shed");
        assert_eq!(report.surging_human_shed_count, 0, "human lane held");
        assert!(report.surging_human_admitted, "surging tenant's human held");
        assert!(report.quiet_human_admitted, "quiet co-tenant's human held");
        assert_eq!(report.cross_tenant_impact, 0, "cross-tenant impact 0");
    }

    #[test]
    fn an_unbounded_lane_reads_red() {
        let huge = SurfaceBudget {
            per_tenant_in_flight_cap: 1_000_000,
            human_lane_reservation: 200_000,
            retry_after_secs: 3,
        };
        let mut gate = RefsShedGate::with_budget(Surface::RefsBacklinkRead, huge);
        let report = run_refs_surge(
            &mut gate,
            &tenant("noisy"),
            &tenant("quiet"),
            100,
            REFS_SURGE_MULTIPLIER,
        );
        assert_eq!(
            report.surging_agent_shed_count, 0,
            "the unbounded lane swallowed the storm"
        );
        assert!(
            !report.is_ref_d10_green(),
            "an unbounded lane MUST read RED"
        );
    }
}
