use myelin_git::shed_clone::GitFrontDoorShed;
use myelin_git::surge::{run_git_clone_surge, GIT_SURGE_MULTIPLIER};
use myelin_harness::load_generator::{
    LoadGenerator, LoadPrincipalKind, Multiplier, PrincipalMix, Request, Sink, StormProfile,
};
use myelin_substrate::shed::{RunClass, Surface, SurfaceBudget};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

fn surging() -> TenantId {
    TenantId("acme-surging".into())
}
fn quiet() -> TenantId {
    TenantId("quiet-co-tenant".into())
}

fn run_class_of(kind: LoadPrincipalKind) -> RunClass {
    match kind {
        LoadPrincipalKind::Human => RunClass::Human,
        LoadPrincipalKind::Agent => RunClass::Agent,
        LoadPrincipalKind::Ci | LoadPrincipalKind::Service | LoadPrincipalKind::ExternalMcp => {
            RunClass::BatchCi
        }
    }
}

struct ShedGateSink<'a> {
    gate: &'a mut GitFrontDoorShed,
    issued: std::collections::HashMap<RunClass, u64>,
    admitted: std::collections::HashMap<RunClass, u64>,
}

impl<'a> ShedGateSink<'a> {
    fn new(gate: &'a mut GitFrontDoorShed) -> Self {
        ShedGateSink {
            gate,
            issued: std::collections::HashMap::new(),
            admitted: std::collections::HashMap::new(),
        }
    }
    fn issued(&self, c: RunClass) -> u64 {
        self.issued.get(&c).copied().unwrap_or(0)
    }
    fn admitted(&self, c: RunClass) -> u64 {
        self.admitted.get(&c).copied().unwrap_or(0)
    }
}

impl Sink for ShedGateSink<'_> {
    fn handle(&mut self, request: &Request) {
        let class = run_class_of(request.load_kind);
        *self.issued.entry(class).or_insert(0) += 1;
        if self.gate.admit_class(&request.tenant, class).is_ok() {
            *self.admitted.entry(class).or_insert(0) += 1;
            if class == RunClass::Human {
                self.gate.release(&request.tenant, class);
            }
        }
    }
}

#[test]
fn git_d6_human_fetch_lane_holds_agent_and_ci_shed_others_unaffected() {
    let thresholds = Thresholds::load_canonical().expect("the canonical thresholds file loads");
    assert_eq!(
        thresholds.surge.multiplier, GIT_SURGE_MULTIPLIER,
        "the surge multiplier is read from the file (30×), never hardcoded"
    );
    thresholds
        .validate_shed_budgets()
        .expect("the tuned GitFrontDoor shed budget holds the human-lane floor");
    let budget = thresholds
        .shed_budget(Surface::GitFrontDoor)
        .expect("GitFrontDoor budget present in the file");

    let mut gate =
        GitFrontDoorShed::from_thresholds(&thresholds).expect("open the gate from the file");
    let gen = LoadGenerator::new(
        100,
        Multiplier::SURGE,
        PrincipalMix::agent_skewed(),
        StormProfile::ci_surge(),
        vec![surging()],
    )
    .expect("a non-empty surge");
    let (human_issued, human_admitted, agent_issued, ci_issued) = {
        let mut sink = ShedGateSink::new(&mut gate);
        gen.drive(&mut sink);
        (
            sink.issued(RunClass::Human),
            sink.admitted(RunClass::Human),
            sink.issued(RunClass::Agent),
            sink.issued(RunClass::BatchCi),
        )
    };

    assert!(human_issued > 0, "the surge carried human fetches");
    assert!(
        agent_issued > 0,
        "the surge carried agent clones (the agent fan-out)"
    );
    assert!(
        ci_issued > 0,
        "the surge carried CI checkouts (the CI run-checkout storm)"
    );

    assert_eq!(
        human_admitted, human_issued,
        "the human fetch lane HELD: all {human_issued} human fetches admitted (0 shed) under the 30× surge"
    );
    assert_eq!(
        gate.shed_count(RunClass::Human),
        0,
        "the protected human fetch lane has 0 shed under the 30× clone surge"
    );

    assert!(
        gate.shed_count(RunClass::Agent) > 0,
        "the agent clone lane sheds (429 + Retry-After) under the surge"
    );
    assert!(
        gate.shed_count(RunClass::BatchCi) > 0,
        "the CI checkout lane sheds (429 + Retry-After) under the surge"
    );

    assert_eq!(
        gate.in_flight(&quiet()),
        0,
        "the surging tenant's storm spent 0 of the quiet co-tenant's budget (per-tenant bulkhead)"
    );
    assert!(
        gate.admit_class(&quiet(), RunClass::Human).is_ok(),
        "the quiet co-tenant's human fetch is admitted (the surge never sheds another tenant's human)"
    );

    println!(
        "[P-483 GIT-D6 GATE GREEN 2026-06-25] cap={} reserved={} retry_after_secs={} | \
         human issued={human_issued} admitted={human_admitted} shed=0 | agent shed={} | ci shed={} | \
         cross_tenant_impact=0",
        budget.per_tenant_in_flight_cap,
        budget.human_lane_reservation,
        budget.retry_after_secs,
        gate.shed_count(RunClass::Agent),
        gate.shed_count(RunClass::BatchCi),
    );
}

#[test]
fn git_d6_human_lane_holds_across_1x_10x_30x() {
    let thresholds = Thresholds::load_canonical().expect("load");
    for m in [Multiplier::BASELINE, Multiplier::STRESS, Multiplier::SURGE] {
        let mut gate =
            GitFrontDoorShed::from_thresholds(&thresholds).expect("open the gate from the file");
        let gen = LoadGenerator::new(
            100,
            m,
            PrincipalMix::agent_skewed(),
            StormProfile::ci_surge(),
            vec![surging()],
        )
        .expect("a non-empty surge");
        let (human_issued, human_admitted) = {
            let mut sink = ShedGateSink::new(&mut gate);
            gen.drive(&mut sink);
            (sink.issued(RunClass::Human), sink.admitted(RunClass::Human))
        };
        assert!(human_issued > 0, "the {}x surge carried humans", m.factor());
        assert_eq!(
            human_admitted,
            human_issued,
            "the human lane HELD at {}x (all {human_issued} admitted, 0 shed)",
            m.factor()
        );
        assert_eq!(gate.shed_count(RunClass::Human), 0);
        if m.factor() >= 10 {
            assert!(
                gate.shed_count(RunClass::Agent) + gate.shed_count(RunClass::BatchCi) > 0,
                "the machine lanes shed at {}x",
                m.factor()
            );
        }
        println!(
            "[P-483 GIT-D6 {}x] human admitted={human_admitted}/{human_issued} (0 shed) | machine shed={}",
            m.factor(),
            gate.shed_count(RunClass::Agent) + gate.shed_count(RunClass::BatchCi),
        );
    }
}

#[test]
fn git_d6_surge_report_is_green_with_a_quiet_co_tenant() {
    let thresholds = Thresholds::load_canonical().expect("load");
    let mut gate = GitFrontDoorShed::from_thresholds(&thresholds).expect("open the gate");
    let report = run_git_clone_surge(
        &mut gate,
        &surging(),
        &quiet(),
        300,
        300,
        thresholds.surge.multiplier,
    );
    assert!(report.is_git_d6_green(), "{}", report.summary());
    assert!(report.surging_agent_shed_count > 0, "agent clone lane shed");
    assert!(report.surging_ci_shed_count > 0, "CI checkout lane shed");
    assert_eq!(report.surging_human_shed_count, 0, "human fetch lane held");
    assert!(report.surging_human_admitted, "surging tenant's human held");
    assert!(report.quiet_human_admitted, "quiet co-tenant's human held");
    assert_eq!(report.cross_tenant_impact, 0, "cross-tenant impact 0");
    println!("[P-483 GIT-D6 report] {}", report.summary());
}

#[test]
fn git_d6_recorded_budget_is_achievable() {
    let thresholds = Thresholds::load_canonical().expect("load");
    let b = thresholds
        .shed_budget(Surface::GitFrontDoor)
        .expect("present");
    let floor = SurfaceBudget::human_lane_floor(b.per_tenant_in_flight_cap);
    assert!(
        b.human_lane_reservation >= floor,
        "the GitFrontDoor human-lane reservation {} must be at-or-above the measured floor {} \
         (never tuned into starvation)",
        b.human_lane_reservation,
        floor
    );
}
