use std::collections::HashMap;

use myelin_harness::load_generator::{
    LoadGenerator, LoadPrincipalKind, Multiplier, PrincipalMix, Request, Sink, StormProfile,
};
use myelin_harness::{Label, Predicate, SignalName, SignalSource};
use myelin_substrate::shed::{RunClass, RunClassHeader, ShedDecision, ShedLane, Surface};
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

const HUMAN_ADMIT_SERVICE_LATENCY_US: u64 = 800;
const SHED_LATENCY_SENTINEL_US: u64 = u64::MAX;

struct ShedSink {
    lane: ShedLane,
    shed: HashMap<(String, &'static str), u64>,
    admit: HashMap<(String, &'static str), u64>,
    human_latencies: HashMap<String, Vec<u64>>,
    last_machine_retry_after: Option<u64>,
}

impl ShedSink {
    fn new(surface: Surface, budget: myelin_substrate::shed::SurfaceBudget) -> ShedSink {
        ShedSink {
            lane: ShedLane::with_budget(surface, budget),
            shed: HashMap::new(),
            admit: HashMap::new(),
            human_latencies: HashMap::new(),
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

    fn human_p99_us(&self, tenant: &str) -> Option<u64> {
        let mut v = self.human_latencies.get(tenant)?.clone();
        if v.is_empty() {
            return None;
        }
        v.sort_unstable();
        let n = v.len();
        let rank = ((99 * n).div_ceil(100)).max(1) - 1;
        Some(v[rank.min(n - 1)])
    }
}

impl Sink for ShedSink {
    fn handle(&mut self, request: &Request) {
        let class = run_class_of(request);
        let tenant = request.tenant.as_str().to_string();
        let decision = self.lane.admit(&request.tenant, class);
        match decision {
            ShedDecision::Admit => {
                *self
                    .admit
                    .entry((tenant.clone(), class.lane()))
                    .or_insert(0) += 1;
                if class == RunClass::Human {
                    self.human_latencies
                        .entry(tenant)
                        .or_default()
                        .push(HUMAN_ADMIT_SERVICE_LATENCY_US);
                    self.lane.release(&request.tenant, class);
                } else if class != RunClass::Agent {
                    self.lane.release(&request.tenant, class);
                }
            }
            ShedDecision::Shed { retry_after_secs } => {
                *self.shed.entry((tenant.clone(), class.lane())).or_insert(0) += 1;
                if class == RunClass::Human {
                    self.human_latencies
                        .entry(tenant)
                        .or_default()
                        .push(SHED_LATENCY_SENTINEL_US);
                } else {
                    self.last_machine_retry_after = Some(retry_after_secs);
                }
            }
        }
    }
}

fn drive_and_assert_sub_d3(surface: Surface, multiplier: Multiplier, base_requests: u64) {
    let thresholds = Thresholds::load_canonical().expect("thresholds.toml loads");
    let budget = thresholds
        .shed_budget(surface)
        .expect("the surface's shed budget is in the file");
    let human_lane_p99_budget_us = thresholds.surge.human_lane_p99_budget_us;
    let mut sink = ShedSink::new(surface, budget);

    let surge_tenant = TenantId("acme".into());
    let surge = LoadGenerator::new(
        base_requests,
        multiplier,
        PrincipalMix::agent_skewed(),
        StormProfile::ci_surge(),
        vec![surge_tenant.clone()],
    )
    .expect("a non-empty tenant list");
    surge.drive(&mut sink);

    let other_tenant = TenantId("globex".into());
    let baseline = LoadGenerator::new(
        4,
        Multiplier::BASELINE,
        PrincipalMix::balanced(),
        StormProfile::ci_surge(),
        vec![other_tenant.clone()],
    )
    .expect("a non-empty tenant list");
    baseline.drive(&mut sink);

    let human_sheds = sink.shed_of(surge_tenant.as_str(), "human");
    assert_eq!(
        human_sheds, 0,
        "SUB-D3 RED: the protected human lane was shed during the {multiplier:?} surge on \
         {surface:?} (a human must NEVER queue behind a machine lane) - threshold 0, NOT weakened"
    );
    let human_admits = sink.admit_of(surge_tenant.as_str(), "human");
    assert!(
        human_admits > 0,
        "the surge actually carried human traffic (the agent-skewed mix still has a human lane), \
         so the 0-human-sheds result is earned, not vacuous"
    );

    let human_p99 = sink
        .human_p99_us(surge_tenant.as_str())
        .expect("the surge carried human traffic, so a human-lane p99 exists");
    assert!(
        human_p99 <= human_lane_p99_budget_us,
        "SUB-D3 RED: the human-lane p99 ({human_p99} µs) blew the budget \
         ({human_lane_p99_budget_us} µs) under the {multiplier:?} surge - the human lane did not \
         hold within budget; fix the deliverable, do NOT weaken the budget (EI-01 §3)"
    );

    let agent_sheds = sink.shed_of(surge_tenant.as_str(), "agent");
    let batch_ci_sheds = sink.shed_of(surge_tenant.as_str(), "batch_ci");
    let machine_sheds = agent_sheds + batch_ci_sheds;
    assert!(
        machine_sheds > 0,
        "SUB-D3 RED: the machine lanes did NOT shed under a {multiplier:?} surge (the surge must \
         exceed the surface budget) - the shed is the whole point"
    );
    assert_eq!(
        sink.last_machine_retry_after,
        Some(budget.retry_after_secs),
        "every machine-lane shed carries the surface's Retry-After (429 + Retry-After; the \
         resilient client honours it - no retry-storm amplification, §6.2)"
    );

    let other_total_sheds: u64 = ["human", "agent", "batch_ci", "speculative"]
        .iter()
        .map(|lane| sink.shed_of(other_tenant.as_str(), lane))
        .sum();
    assert_eq!(
        other_total_sheds, 0,
        "SUB-D3 RED: a surge on `acme` shed `globex`'s traffic - the per-tenant bulkhead failed \
         (one tenant's surge must NEVER shed another's) - threshold 0, NOT weakened"
    );
    assert!(
        sink.admit_of(other_tenant.as_str(), "human") > 0
            || sink.admit_of(other_tenant.as_str(), "agent") > 0
            || sink.admit_of(other_tenant.as_str(), "batch_ci") > 0,
        "the other tenant's baseline traffic was actually admitted (its budget is its own)"
    );

    let mut src = SignalSource::new();
    src.set_labelled(
        SignalName::ShedCount,
        vec![
            Label::new("lane", "human"),
            Label::new("tenant", surge_tenant.as_str()),
        ],
        human_sheds as i64,
    );
    src.set_labelled(
        SignalName::ShedCount,
        vec![
            Label::new("lane", "agent"),
            Label::new("tenant", surge_tenant.as_str()),
        ],
        agent_sheds as i64,
    );
    src.set_labelled(
        SignalName::ShedCount,
        vec![
            Label::new("lane", "batch_ci"),
            Label::new("tenant", surge_tenant.as_str()),
        ],
        batch_ci_sheds as i64,
    );
    src.set_labelled(
        SignalName::RequestDuration,
        vec![
            Label::new("kind", "human"),
            Label::new("tenant", surge_tenant.as_str()),
        ],
        human_p99 as i64,
    );
    src.set_scalar(SignalName::CrossTenantCount, other_total_sheds as i64);

    let human_held = src.assert_labelled(
        SignalName::ShedCount,
        vec![
            Label::new("lane", "human"),
            Label::new("tenant", surge_tenant.as_str()),
        ],
        Predicate::Eq(0),
    );
    let human_within_budget = src.assert_labelled(
        SignalName::RequestDuration,
        vec![
            Label::new("kind", "human"),
            Label::new("tenant", surge_tenant.as_str()),
        ],
        Predicate::Lte(human_lane_p99_budget_us as i64),
    );
    let machine_shed = if agent_sheds > 0 {
        src.assert_labelled(
            SignalName::ShedCount,
            vec![
                Label::new("lane", "agent"),
                Label::new("tenant", surge_tenant.as_str()),
            ],
            Predicate::Gte(1),
        )
    } else {
        src.assert_labelled(
            SignalName::ShedCount,
            vec![
                Label::new("lane", "batch_ci"),
                Label::new("tenant", surge_tenant.as_str()),
            ],
            Predicate::Gte(1),
        )
    };
    let cross_tenant_zero = src.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0));
    assert!(
        human_held.is_green()
            && human_within_budget.is_green()
            && machine_shed.is_green()
            && cross_tenant_zero.is_green(),
        "SUB-D3 GREEN ({surface:?}, {multiplier:?}): human lane held ({human_held:?}), within \
         budget ({human_within_budget:?}), machine lane shed ({machine_shed:?}), cross-tenant 0 \
         ({cross_tenant_zero:?})"
    );
}

#[test]
fn sub_d3_30x_surge_family_human_lane_holds_machine_sheds_others_unaffected() {
    let thresholds = Thresholds::load_canonical().expect("thresholds.toml loads");
    assert_eq!(
        thresholds.surge.multiplier, 30,
        "the surge default-to-beat is 30×"
    );
    let multiplier =
        Multiplier::custom(thresholds.surge.multiplier).expect("a positive surge multiplier");
    drive_and_assert_sub_d3(Surface::HttpIntake, multiplier, 64);
}

#[test]
fn sub_d3_30x_surge_ci_dispatch_machine_sheds_others_unaffected() {
    let thresholds = Thresholds::load_canonical().expect("load");
    let multiplier =
        Multiplier::custom(thresholds.surge.multiplier).expect("a positive surge multiplier");
    let budget = thresholds.shed_budget(Surface::CiDispatch).expect("budget");
    let mut sink = ShedSink::new(Surface::CiDispatch, budget);

    let surge_tenant = TenantId("acme".into());
    let surge = LoadGenerator::new(
        64,
        multiplier,
        PrincipalMix::from_weights([0, 5, 0, 5, 0]).expect("agent + CI machine mix"),
        StormProfile::ci_surge(),
        vec![surge_tenant.clone()],
    )
    .expect("non-empty tenants");
    surge.drive(&mut sink);

    let other_tenant = TenantId("globex".into());
    let baseline = LoadGenerator::new(
        4,
        Multiplier::BASELINE,
        PrincipalMix::from_weights([0, 1, 0, 1, 0]).expect("baseline machine trickle"),
        StormProfile::ci_surge(),
        vec![other_tenant.clone()],
    )
    .expect("non-empty tenants");
    baseline.drive(&mut sink);

    let machine_sheds = sink.shed_of(surge_tenant.as_str(), "agent")
        + sink.shed_of(surge_tenant.as_str(), "batch_ci");
    assert!(
        machine_sheds > 0,
        "SUB-D3 RED: the CI-dispatch surface did not shed under a 30× CI/agent surge"
    );
    assert_eq!(
        sink.last_machine_retry_after,
        Some(budget.retry_after_secs),
        "every CI-dispatch shed carries the surface's Retry-After"
    );
    let other_sheds: u64 = ["agent", "batch_ci"]
        .iter()
        .map(|l| sink.shed_of(other_tenant.as_str(), l))
        .sum();
    assert_eq!(
        other_sheds, 0,
        "SUB-D3 RED: a CI surge on `acme` shed `globex`'s CI traffic - the per-tenant bulkhead failed"
    );
}

#[test]
fn sub_d3_smoke_10x_human_lane_holds_machine_sheds_others_unaffected() {
    drive_and_assert_sub_d3(Surface::HttpIntake, Multiplier::STRESS, 64);
}

#[test]
fn sub_d3_machine_lane_sheds_before_human_lane() {
    let thresholds = Thresholds::load_canonical().expect("load");
    let budget = thresholds.shed_budget(Surface::HttpIntake).expect("budget");
    let mut sink = ShedSink::new(Surface::HttpIntake, budget);
    let tenant = TenantId("acme".into());

    let gen = LoadGenerator::new(
        64,
        Multiplier::SURGE,
        PrincipalMix::from_weights([3, 7, 0, 0, 0]).expect("30% human / 70% agent"),
        StormProfile::ci_surge(),
        vec![tenant.clone()],
    )
    .expect("non-empty tenants");
    gen.drive(&mut sink);

    let human = sink.shed_of(tenant.as_str(), "human");
    let agent = sink.shed_of(tenant.as_str(), "agent");
    assert_eq!(human, 0, "the human lane is shed last (0 under this surge)");
    assert!(
        agent > human,
        "the machine lane sheds BEFORE the human lane (the §7.2 shed order)"
    );
}
