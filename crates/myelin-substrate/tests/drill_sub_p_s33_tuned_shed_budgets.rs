use std::collections::HashMap;

use myelin_harness::load_generator::{
    LoadGenerator, LoadPrincipalKind, Multiplier, PrincipalMix, Request, Sink, StormProfile,
};
use myelin_substrate::shed::{
    RunClass, RunClassHeader, ShedBudgetError, ShedDecision, ShedLane, Surface, SurfaceBudget,
};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

fn run_class_of(req: &Request) -> RunClass {
    let header = match req.load_kind {
        LoadPrincipalKind::Ci | LoadPrincipalKind::Service | LoadPrincipalKind::ExternalMcp => {
            Some(RunClassHeader::BatchCi)
        }
        LoadPrincipalKind::Human | LoadPrincipalKind::Agent => None,
    };
    RunClass::derive(&req.principal_kind, header)
}

struct TunedShedSink {
    lane: ShedLane,
    shed: HashMap<(String, &'static str), u64>,
    admit: HashMap<(String, &'static str), u64>,
    last_machine_retry_after: Option<u64>,
}

impl TunedShedSink {
    fn new(surface: Surface, budget: SurfaceBudget) -> TunedShedSink {
        TunedShedSink {
            lane: ShedLane::with_budget(surface, budget),
            shed: HashMap::new(),
            admit: HashMap::new(),
            last_machine_retry_after: None,
        }
    }

    fn shed_of(&self, tenant: &str, lane: &'static str) -> u64 {
        self.shed
            .get(&(tenant.to_string(), lane))
            .copied()
            .unwrap_or(0)
    }

    fn admit_of(&self, tenant: &str, lane: &'static str) -> u64 {
        self.admit
            .get(&(tenant.to_string(), lane))
            .copied()
            .unwrap_or(0)
    }
}

impl Sink for TunedShedSink {
    fn handle(&mut self, request: &Request) {
        let class = run_class_of(request);
        let tenant = request.tenant.as_str().to_string();
        match self.lane.admit(&request.tenant, class) {
            ShedDecision::Admit => {
                *self
                    .admit
                    .entry((tenant.clone(), class.lane()))
                    .or_insert(0) += 1;
                if class != RunClass::Agent {
                    self.lane.release(&request.tenant, class);
                }
            }
            ShedDecision::Shed { retry_after_secs } => {
                *self.shed.entry((tenant.clone(), class.lane())).or_insert(0) += 1;
                if class != RunClass::Human {
                    self.last_machine_retry_after = Some(retry_after_secs);
                }
            }
        }
    }
}

fn re_run_sub_d3_against_tuned_budget(surface: Surface, budget: SurfaceBudget, multiplier: u32) {
    let mut sink = TunedShedSink::new(surface, budget);

    let surge_tenant = TenantId("acme".into());
    let surge = LoadGenerator::new(
        64,
        Multiplier::custom(multiplier).expect("positive multiplier"),
        PrincipalMix::agent_skewed(),
        StormProfile::ci_surge(),
        vec![surge_tenant.clone()],
    )
    .expect("non-empty tenants");
    surge.drive(&mut sink);

    let other_tenant = TenantId("globex".into());
    let baseline = LoadGenerator::new(
        4,
        Multiplier::BASELINE,
        PrincipalMix::balanced(),
        StormProfile::ci_surge(),
        vec![other_tenant.clone()],
    )
    .expect("non-empty tenants");
    baseline.drive(&mut sink);

    assert_eq!(
        sink.shed_of(surge_tenant.as_str(), "human"),
        0,
        "P-S33 RED: the TUNED budget for {surface:?} shed the protected human lane under the {multiplier}× surge \
         - the tuned numbers must STILL hold the human lane (threshold 0, NOT weakened, EI-01 §3)"
    );
    assert!(
        sink.admit_of(surge_tenant.as_str(), "human") > 0,
        "the surge carried human traffic against {surface:?} - the 0-shed result is earned"
    );

    let machine_sheds = sink.shed_of(surge_tenant.as_str(), "agent")
        + sink.shed_of(surge_tenant.as_str(), "batch_ci");
    assert!(
        machine_sheds > 0,
        "P-S33 RED: the TUNED budget for {surface:?} did not shed the machine lanes under a {multiplier}× surge"
    );
    assert_eq!(
        sink.last_machine_retry_after,
        Some(budget.retry_after_secs),
        "every machine-lane shed carries the surface's TUNED Retry-After (429 + Retry-After)"
    );

    let other_sheds: u64 = ["human", "agent", "batch_ci", "speculative"]
        .iter()
        .map(|l| sink.shed_of(other_tenant.as_str(), l))
        .sum();
    assert_eq!(
        other_sheds, 0,
        "P-S33 RED: a surge on `acme` against {surface:?}'s tuned budget shed `globex` - per-tenant bulkhead failed"
    );
    assert!(
        sink.admit_of(other_tenant.as_str(), "human") > 0
            || sink.admit_of(other_tenant.as_str(), "agent") > 0
            || sink.admit_of(other_tenant.as_str(), "batch_ci") > 0,
        "the other tenant's baseline was admitted (its budget is its own)"
    );
}

#[test]
fn the_tuned_shed_budgets_in_the_file_validate() {
    let t = Thresholds::load_canonical().expect("thresholds.toml loads");
    t.validate_shed_budgets()
        .expect("the TUNED shed budgets in the file must hold the §7.6 human-lane floor (P-S33)");

    for surface in [
        Surface::CollabOpStream,
        Surface::ConnectionTier,
        Surface::AgentMention,
        Surface::GitFrontDoor,
        Surface::RefsBacklinkRead,
        Surface::RefsRefCreate,
        Surface::SearchQuery,
        Surface::WorkflowAgentLane,
        Surface::HttpIntake,
    ] {
        let b = t.shed_budget(surface).expect("present");
        let floor = SurfaceBudget::human_lane_floor(b.per_tenant_in_flight_cap);
        assert!(
            b.human_lane_reservation >= floor,
            "{surface:?} tuned reservation {} is at-or-above the measured human-lane floor {} (cap {})",
            b.human_lane_reservation,
            floor,
            b.per_tenant_in_flight_cap,
        );
    }
    let ci = t.shed_budget(Surface::CiDispatch).expect("present");
    assert_eq!(
        ci.human_lane_reservation, 0,
        "CI is the batch lane (§7.6 n/a)"
    );
    assert!(
        ci.per_tenant_in_flight_cap > 0,
        "CI dispatch is still bounded"
    );
}

#[test]
fn a_thresholds_file_that_starves_a_human_lane_fails_the_gate() {
    let starved_toml = r#"
        version = 1
        as_of = "2026-06-24"
        [revocation]
        sla_mins = 5
        [surge]
        multiplier = 30
        [fail_static]
        status = "OPEN - LEGAL"
        owner = "DPO / Legal"
        static_max_default_secs = 300
        agent_token_ttl_secs = 60
        constraint = "x"
        [rpo_rto]
        rpo_max_mins = 5
        rto_tenant_max_mins = 60
        rto_cell_max_mins = 240
        [depth_ceilings]
        soft = 12
        hard = 16
        [[shed_budgets]]
        surface = "ConnectionTier"
        per_tenant_in_flight_cap = 256
        human_lane_reservation = 4
        retry_after_secs = 3
    "#;
    let t = Thresholds::from_toml(starved_toml).expect("parses (the shape is valid)");
    let err = t
        .validate_shed_budgets()
        .expect_err("a starved human lane must FAIL the gate");
    match err {
        ShedBudgetError::HumanLaneStarved {
            surface,
            reservation,
            floor,
            ..
        } => {
            assert_eq!(surface, Surface::ConnectionTier);
            assert_eq!(reservation, 4);
            assert!(
                reservation < floor,
                "the gate caught a reservation under the measured floor - you cannot tune the human lane into starvation"
            );
        }
        other => panic!("expected HumanLaneStarved, got {other:?}"),
    }
}

#[test]
fn re_running_sub_d3_against_the_tuned_numbers_still_holds_the_human_lane() {
    let t = Thresholds::load_canonical().expect("load");
    let multiplier = t.surge.multiplier;

    let table = t
        .shed_budget_table_validated()
        .expect("the tuned table validates");

    for surface in [
        Surface::HttpIntake,
        Surface::ConnectionTier,
        Surface::AgentMention,
        Surface::CollabOpStream,
        Surface::GitFrontDoor,
        Surface::RefsBacklinkRead,
        Surface::RefsRefCreate,
        Surface::SearchQuery,
        Surface::WorkflowAgentLane,
    ] {
        let budget = table.budget(surface);
        re_run_sub_d3_against_tuned_budget(surface, budget, multiplier);
    }
}

#[test]
fn the_tuned_thresholds_file_round_trips() {
    let t = Thresholds::load_canonical().expect("load");
    let serialized = t.to_toml().expect("serialize");
    let reparsed = Thresholds::from_toml(&serialized).expect("re-parse");
    assert_eq!(t, reparsed, "the tuned file round-trips (no lossy edit)");

    for surface in [
        Surface::CiDispatch,
        Surface::CollabOpStream,
        Surface::ConnectionTier,
        Surface::AgentMention,
        Surface::GitFrontDoor,
        Surface::RefsBacklinkRead,
        Surface::RefsRefCreate,
        Surface::SearchQuery,
        Surface::WorkflowAgentLane,
        Surface::HttpIntake,
    ] {
        assert_eq!(
            t.shed_budget(surface).expect("present"),
            reparsed.shed_budget(surface).expect("present"),
            "the tuned budget for {surface:?} survives the round-trip"
        );
    }
    reparsed
        .validate_shed_budgets()
        .expect("the round-tripped tuned file still holds the human-lane floor");
}

#[test]
fn the_tuned_file_and_the_in_code_table_agree() {
    let t = Thresholds::load_canonical().expect("load");
    let table = myelin_substrate::shed::ShedBudgetTable::v1_floor();
    table
        .validate()
        .expect("the in-code tuned table holds the human-lane floor");
    for surface in [
        Surface::CiDispatch,
        Surface::CollabOpStream,
        Surface::ConnectionTier,
        Surface::AgentMention,
        Surface::GitFrontDoor,
        Surface::RefsBacklinkRead,
        Surface::RefsRefCreate,
        Surface::SearchQuery,
        Surface::WorkflowAgentLane,
        Surface::HttpIntake,
    ] {
        assert_eq!(
            t.shed_budget(surface).expect("present"),
            table.budget(surface),
            "the tuned file and the in-code table must agree for {surface:?} (one source of truth)"
        );
    }
}
