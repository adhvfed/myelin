use crate::cost_bounder::{plan_board_query, CostBudget, FacetCatalog, PlanOutcome};
use myelin_identity::{Literal, ObjectId, Principal, PrincipalId, PrincipalKind, SetExpr, Zookie};
use myelin_query::{CmpOp, Expr, Predicate, QueryAst};
use myelin_substrate::shed::{RunClass, ShedDecision, ShedLane, Surface, SurfaceBudget};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::{Region, TenantId};

pub const ISSUES_SURGE_MULTIPLIER: u32 = 30;

pub struct IssuesOwnerShed {
    lane: ShedLane,
}

impl IssuesOwnerShed {
    pub fn new() -> IssuesOwnerShed {
        IssuesOwnerShed {
            lane: ShedLane::new(Surface::HttpIntake),
        }
    }

    pub fn with_budget(budget: SurfaceBudget) -> IssuesOwnerShed {
        IssuesOwnerShed {
            lane: ShedLane::with_budget(Surface::HttpIntake, budget),
        }
    }

    pub fn from_thresholds(thresholds: &Thresholds) -> Result<IssuesOwnerShed, String> {
        thresholds.validate_shed_budgets().map_err(|e| {
            format!("the HttpIntake shed budget must hold the human-lane floor: {e}")
        })?;
        let budget = thresholds
            .shed_budget(Surface::HttpIntake)
            .map_err(|e| format!("the HttpIntake shed budget must be present: {e}"))?;
        Ok(IssuesOwnerShed::with_budget(budget))
    }

    pub fn admit_class(&mut self, tenant: &TenantId, class: RunClass) -> Result<(), u64> {
        match self.lane.admit(tenant, class) {
            ShedDecision::Admit => Ok(()),
            ShedDecision::Shed { retry_after_secs } => Err(retry_after_secs),
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
}

impl Default for IssuesOwnerShed {
    fn default() -> Self {
        IssuesOwnerShed::new()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssuesOwnerSurgeReport {
    pub surging_human_shed_count: u64,
    pub surging_human_admitted: bool,
    pub surging_agent_shed_count: u64,
    pub surging_batch_shed_count: u64,
    pub quiet_human_admitted: bool,
    pub cross_tenant_impact: u32,
    pub retry_after_secs: u64,
}

impl IssuesOwnerSurgeReport {
    pub fn is_f6_green(&self) -> bool {
        self.surging_human_shed_count == 0
            && self.surging_human_admitted
            && self.surging_agent_shed_count > 0
            && self.surging_batch_shed_count > 0
            && self.retry_after_secs > 0
            && self.quiet_human_admitted
            && self.cross_tenant_impact == 0
    }

    pub fn summary(&self) -> String {
        format!(
            "ISS-F6: human held(admitted={}, shed={}) | agent shed={} | batch shed={} | \
             retry_after={}s | quiet human admitted={} | cross_tenant_impact={}",
            self.surging_human_admitted,
            self.surging_human_shed_count,
            self.surging_agent_shed_count,
            self.surging_batch_shed_count,
            self.retry_after_secs,
            self.quiet_human_admitted,
            self.cross_tenant_impact,
        )
    }
}

pub fn run_issues_owner_surge(
    gate: &mut IssuesOwnerShed,
    surging: &TenantId,
    quiet: &TenantId,
    base_agent: u32,
    base_batch: u32,
    multiplier: u32,
) -> IssuesOwnerSurgeReport {
    let agent_total = base_agent.saturating_mul(multiplier.max(1));
    let batch_total = base_batch.saturating_mul(multiplier.max(1));

    let mut retry_after_secs = 0u64;

    let mut surging_human_admitted = true;
    let bursts = agent_total.max(batch_total).max(1);
    for i in 0..bursts {
        if i < agent_total {
            if let Err(secs) = gate.admit_class(surging, RunClass::Agent) {
                retry_after_secs = secs;
            }
        }
        if i < batch_total {
            if let Err(secs) = gate.admit_class(surging, RunClass::BatchCi) {
                retry_after_secs = secs;
            }
        }
        match gate.admit_class(surging, RunClass::Human) {
            Ok(()) => gate.release(surging, RunClass::Human),
            Err(_) => surging_human_admitted = false,
        }
    }

    let cross_tenant_impact = gate.in_flight(quiet);
    let quiet_human_admitted = gate.admit_class(quiet, RunClass::Human).is_ok();
    if quiet_human_admitted {
        gate.release(quiet, RunClass::Human);
    }

    IssuesOwnerSurgeReport {
        surging_human_shed_count: gate.shed_count(RunClass::Human),
        surging_human_admitted,
        surging_agent_shed_count: gate.shed_count(RunClass::Agent),
        surging_batch_shed_count: gate.shed_count(RunClass::BatchCi),
        quiet_human_admitted,
        cross_tenant_impact,
        retry_after_secs,
    }
}

pub fn open_surge_gate_from_thresholds() -> Result<(IssuesOwnerShed, Thresholds), String> {
    let thresholds = Thresholds::load_canonical().map_err(|e| format!("thresholds load: {e}"))?;
    let gate = IssuesOwnerShed::from_thresholds(&thresholds)?;
    Ok((gate, thresholds))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssD2CellScaleReport {
    pub board_issue_count: u64,
    pub field_count: u32,
    pub plans_evaluated: u64,
    pub served_oltp: u64,
    pub escalated: u64,
    pub refined: u64,
    pub unbounded_scans: u64,
}

impl IssD2CellScaleReport {
    pub fn is_iss_d2_green(&self) -> bool {
        self.unbounded_scans == 0
            && self.board_issue_count >= 1_000_000
            && self.field_count >= 50
            && self.served_oltp > 0
            && self.escalated > 0
    }

    pub fn summary(&self) -> String {
        format!(
            "ISS-D2@cell-scale: board={} issues × {} fields | plans={} (oltp={}, escalate={}, refine={}) | \
             unbounded_scans={}",
            self.board_issue_count,
            self.field_count,
            self.plans_evaluated,
            self.served_oltp,
            self.escalated,
            self.refined,
            self.unbounded_scans,
        )
    }
}

fn cell_scale_viewer() -> Principal {
    Principal::stub(
        PrincipalId("p:eng".into()),
        PrincipalKind::Human,
        TenantId("iss-cell".into()),
    )
}

fn cell_scale_acl() -> SetExpr {
    SetExpr::Ids(vec![ObjectId("ENG-1".into()), ObjectId("ENG-2".into())])
}

fn ast_over(field: &str) -> QueryAst {
    QueryAst::compiled(Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: Expr::Var(field.into()),
        rhs: Expr::Lit(Literal::Str("x".into())),
    })
    .expect("a well-formed single-predicate AST")
}

pub fn run_iss_d2_cell_scale(board_issue_count: u64) -> IssD2CellScaleReport {
    let tenant = TenantId("iss-cell".into());
    let region = Region("fr-par".into());
    let zk = Zookie("zk-0000000010".into());
    let viewer = cell_scale_viewer();
    let acl = cell_scale_acl();

    let mut fields: Vec<String> = vec![
        "state".into(),
        "severity".into(),
        "text".into(),
        "semantic".into(),
    ];
    for i in 0..50 {
        fields.push(format!("custom_field_{i:02}"));
    }
    let field_count = fields.len() as u32;

    let fanouts: [u64; 7] = [
        10,
        1_000,
        50_000,
        board_issue_count / 10,
        board_issue_count,
        board_issue_count.saturating_mul(6),
        board_issue_count.saturating_mul(50),
    ];

    let mut served_oltp = 0u64;
    let mut escalated = 0u64;
    let mut refined = 0u64;
    let mut unbounded_scans = 0u64;
    let mut plans_evaluated = 0u64;

    for promote_severity in [false, true] {
        let mut cat = FacetCatalog::new();
        if promote_severity {
            cat.promote("severity");
        }
        for field in &fields {
            for &fanout in &fanouts {
                let outcome = plan_board_query(
                    &ast_over(field),
                    &acl,
                    &viewer,
                    &tenant,
                    &region,
                    &zk,
                    &cat,
                    &CostBudget::DEFAULT,
                    fanout,
                );
                plans_evaluated += 1;
                if !outcome.assert_no_unbounded_scan() {
                    unbounded_scans += 1;
                }
                match outcome {
                    PlanOutcome::ServeOltp(_) => served_oltp += 1,
                    PlanOutcome::EscalateToSearch(_) => escalated += 1,
                    PlanOutcome::Refine(_) => refined += 1,
                }
            }
        }
    }

    IssD2CellScaleReport {
        board_issue_count,
        field_count,
        plans_evaluated,
        served_oltp,
        escalated,
        refined,
        unbounded_scans,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surging() -> TenantId {
        TenantId("acme-surging".into())
    }
    fn quiet() -> TenantId {
        TenantId("quiet-co-tenant".into())
    }

    #[test]
    fn surge_const_matches_the_frozen_file() {
        let t = Thresholds::load_canonical().expect("load");
        assert_eq!(
            t.surge.multiplier, ISSUES_SURGE_MULTIPLIER,
            "the surge multiplier is read from the file (30×), never hardcoded"
        );
    }

    #[test]
    fn iss_f6_report_is_green_with_a_quiet_co_tenant() {
        let (mut gate, t) = open_surge_gate_from_thresholds().expect("open the gate");
        let report = run_issues_owner_surge(
            &mut gate,
            &surging(),
            &quiet(),
            200,
            200,
            t.surge.multiplier,
        );
        assert!(report.is_f6_green(), "{}", report.summary());
        assert_eq!(report.surging_human_shed_count, 0, "human lane held");
        assert!(report.surging_agent_shed_count > 0, "agent lane shed");
        assert!(report.surging_batch_shed_count > 0, "batch lane shed");
        assert!(report.retry_after_secs > 0, "429 carried a Retry-After");
        assert_eq!(report.cross_tenant_impact, 0, "cross-tenant impact 0");
    }

    #[test]
    fn iss_f6_report_goes_red_when_the_lane_does_not_shed() {
        let mut gate = IssuesOwnerShed::with_budget(SurfaceBudget {
            per_tenant_in_flight_cap: 1_000_000,
            human_lane_reservation: 250_000,
            retry_after_secs: 5,
        });
        let report = run_issues_owner_surge(&mut gate, &surging(), &quiet(), 10, 10, 30);
        assert!(
            !report.is_f6_green(),
            "a never-shedding lane must FAIL F6 (the green is a real property): {}",
            report.summary()
        );
        assert_eq!(
            report.surging_agent_shed_count, 0,
            "nothing shed (unbounded)"
        );
    }

    #[test]
    fn iss_d2_holds_at_cell_scale() {
        let report = run_iss_d2_cell_scale(1_000_000);
        assert!(report.is_iss_d2_green(), "{}", report.summary());
        assert_eq!(
            report.unbounded_scans, 0,
            "the cost-bounder NEVER emits an unbounded JSONB scan at cell scale"
        );
        assert!(report.board_issue_count >= 1_000_000, "a 1M+ board");
        assert!(report.field_count >= 50, "a 50+ custom-field board");
        assert!(report.served_oltp > 0, "some queries serve on OLTP");
        assert!(report.escalated > 0, "some queries escalate to Search");
    }

    #[test]
    fn iss_d2_accounting_is_exhaustive_and_bounded() {
        let report = run_iss_d2_cell_scale(2_000_000);
        assert_eq!(
            report.served_oltp + report.escalated + report.refined,
            report.plans_evaluated,
            "every plan is accounted for (served | escalate | refine), none unbounded"
        );
        assert_eq!(report.unbounded_scans, 0);
    }
}
