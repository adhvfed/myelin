use myelin_harness::load_generator::{
    LoadGenerator, LoadPrincipalKind, Multiplier, PrincipalMix, RecordingSink, Sink, StormProfile,
};
use myelin_notif::{
    run_notif_surge, NotifShedGate, NotifSurgeReport, ProviderBulkhead, NOTIF_SURGE_MULTIPLIER,
};
use myelin_substrate::shed::{RunClass, Surface};
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
    gate: &'a mut NotifShedGate,
    bulkhead: &'a mut ProviderBulkhead,
    provider_peak: u32,
    issued: std::collections::HashMap<RunClass, u64>,
    admitted: std::collections::HashMap<RunClass, u64>,
}

impl<'a> ShedGateSink<'a> {
    fn new(gate: &'a mut NotifShedGate, bulkhead: &'a mut ProviderBulkhead) -> Self {
        ShedGateSink {
            gate,
            bulkhead,
            provider_peak: 0,
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
    fn handle(&mut self, request: &myelin_harness::load_generator::Request) {
        let class = run_class_of(request.load_kind);
        *self.issued.entry(class).or_insert(0) += 1;
        if self.gate.admit_class(&request.tenant, class).is_ok() {
            *self.admitted.entry(class).or_insert(0) += 1;
            if class == RunClass::Human {
                self.gate.release(&request.tenant, class);
            } else {
                let _ = self.bulkhead.try_send();
                self.provider_peak = self.provider_peak.max(self.bulkhead.in_flight());
            }
        }
    }
}

#[test]
fn notif_d5_human_lane_holds_agent_sheds_others_unaffected_bulkhead_bounds_provider() {
    let thresholds = Thresholds::load_canonical().expect("the canonical thresholds file loads");
    assert_eq!(
        thresholds.surge.multiplier, NOTIF_SURGE_MULTIPLIER,
        "the surge multiplier is read from the file (30×), never hardcoded"
    );
    thresholds
        .validate_shed_budgets()
        .expect("the tuned AgentMention shed budget holds the human-lane floor");
    let budget = thresholds
        .shed_budget(Surface::AgentMention)
        .expect("AgentMention budget present in the file");

    let mut gate =
        NotifShedGate::from_thresholds(&thresholds).expect("open the gate from the file");
    let mut bulkhead = ProviderBulkhead::new("email", budget.human_lane_reservation.max(1));
    let gen = LoadGenerator::new(
        100,
        Multiplier::SURGE,
        PrincipalMix::agent_skewed(),
        StormProfile::agent_mention_storm(),
        vec![surging()],
    )
    .expect("a non-empty surge");
    let (human_issued, human_admitted, agent_issued, ci_issued, provider_peak) = {
        let mut sink = ShedGateSink::new(&mut gate, &mut bulkhead);
        gen.drive(&mut sink);
        (
            sink.issued(RunClass::Human),
            sink.admitted(RunClass::Human),
            sink.issued(RunClass::Agent),
            sink.issued(RunClass::BatchCi),
            sink.provider_peak,
        )
    };

    assert!(human_issued > 0, "the surge carried human inbox reads");
    assert!(
        agent_issued > 0,
        "the surge carried agent notification ops (the agent fan-out)"
    );
    assert!(
        ci_issued > 0,
        "the surge carried CI/service notification ops (the batch notification storm)"
    );

    assert_eq!(
        human_admitted, human_issued,
        "the human inbox-read lane HELD: all {human_issued} reads admitted (0 shed) under the 30× surge"
    );
    assert_eq!(
        gate.shed_count(RunClass::Human),
        0,
        "the protected human inbox-read lane has 0 shed under the 30× surge (§5.2)"
    );

    assert!(
        gate.shed_count(RunClass::Agent) > 0,
        "the agent notification lane sheds (429 + Retry-After) under the surge"
    );
    assert!(
        gate.shed_count(RunClass::BatchCi) > 0,
        "the CI/batch notification lane sheds (429 + Retry-After) under the surge"
    );

    assert_eq!(
        gate.in_flight(&quiet()),
        0,
        "the surging tenant's storm spent 0 of the quiet co-tenant's budget (per-tenant bulkhead)"
    );
    assert!(
        gate.admit_class(&quiet(), RunClass::Human).is_ok(),
        "the quiet co-tenant's human inbox read is admitted (the surge never sheds another tenant's human)"
    );

    assert!(
        provider_peak <= bulkhead.concurrency(),
        "the delivery-adapter bulkhead bounded provider load: peak {provider_peak} ≤ bound {}",
        bulkhead.concurrency()
    );
    assert!(
        bulkhead.shed_count() > 0,
        "the bulkhead shed the over-bound concurrent sends (it bounds provider load, never buffers)"
    );

    println!(
        "[P-467 NOTIF-D5 GATE GREEN 2026-06-25] cap={} reserved={} retry_after_secs={} | \
         human issued={human_issued} admitted={human_admitted} shed=0 | agent shed={} | ci shed={} | \
         cross_tenant_impact=0 | provider_peak={provider_peak}/{} bulkhead_shed={}",
        budget.per_tenant_in_flight_cap,
        budget.human_lane_reservation,
        budget.retry_after_secs,
        gate.shed_count(RunClass::Agent),
        gate.shed_count(RunClass::BatchCi),
        bulkhead.concurrency(),
        bulkhead.shed_count(),
    );
}

#[test]
fn notif_d5_human_lane_holds_across_1x_10x_30x() {
    let thresholds = Thresholds::load_canonical().expect("load");
    for m in [Multiplier::BASELINE, Multiplier::STRESS, Multiplier::SURGE] {
        let mut gate =
            NotifShedGate::from_thresholds(&thresholds).expect("open the gate from the file");
        let mut bulkhead = ProviderBulkhead::new("email", 8);
        let gen = LoadGenerator::new(
            100,
            m,
            PrincipalMix::agent_skewed(),
            StormProfile::agent_mention_storm(),
            vec![surging()],
        )
        .expect("a non-empty surge");
        let (human_issued, human_admitted) = {
            let mut sink = ShedGateSink::new(&mut gate, &mut bulkhead);
            gen.drive(&mut sink);
            (sink.issued(RunClass::Human), sink.admitted(RunClass::Human))
        };
        assert!(human_issued > 0, "the {}x surge carried humans", m.factor());
        assert_eq!(
            human_admitted,
            human_issued,
            "the human inbox-read lane HELD at {}x (all {human_issued} admitted, 0 shed)",
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
            "[P-467 NOTIF-D5 {}x] human admitted={human_admitted}/{human_issued} (0 shed) | machine shed={}",
            m.factor(),
            gate.shed_count(RunClass::Agent) + gate.shed_count(RunClass::BatchCi),
        );
    }
}

#[test]
fn notif_d5_surge_report_is_green_with_a_quiet_co_tenant() {
    let thresholds = Thresholds::load_canonical().expect("load");
    let mut gate = NotifShedGate::from_thresholds(&thresholds).expect("open the gate");
    let b = thresholds
        .shed_budget(Surface::AgentMention)
        .expect("present");
    let mut bulkhead = ProviderBulkhead::new("email", b.human_lane_reservation.max(1));
    let report: NotifSurgeReport = run_notif_surge(
        &mut gate,
        &mut bulkhead,
        &surging(),
        &quiet(),
        500,
        500,
        thresholds.surge.multiplier,
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
        "the bulkhead bounded provider load"
    );
    assert!(
        report.provider_bulkhead_shed > 0,
        "the bulkhead shed the excess"
    );
    println!("[P-467 NOTIF-D5 report] {}", report.summary());
}

#[test]
fn notif_d5_recorded_budget_is_achievable_and_the_surge_is_real() {
    let thresholds = Thresholds::load_canonical().expect("load");
    let b = thresholds
        .shed_budget(Surface::AgentMention)
        .expect("present");
    let floor = myelin_substrate::shed::SurfaceBudget::human_lane_floor(b.per_tenant_in_flight_cap);
    assert!(
        b.human_lane_reservation >= floor,
        "the AgentMention human-lane reservation {} must be at-or-above the measured floor {} \
         (never tuned into starvation)",
        b.human_lane_reservation,
        floor
    );

    let gen = LoadGenerator::new(
        100,
        Multiplier::SURGE,
        PrincipalMix::agent_skewed(),
        StormProfile::agent_mention_storm(),
        vec![surging()],
    )
    .expect("a non-empty surge");
    let mut sink = RecordingSink::default();
    gen.drive(&mut sink);
    assert_eq!(
        sink.received.len(),
        3000,
        "the realised 30× count (100 × 30)"
    );
    let humans = sink
        .received
        .iter()
        .filter(|r| r.load_kind == LoadPrincipalKind::Human)
        .count();
    let machines = sink.received.len() - humans;
    assert!(humans > 0, "the surge carried a thin human lane");
    assert!(
        machines > humans,
        "the agent-skewed surge is machine-heavy ({machines} machine vs {humans} human)"
    );
}
