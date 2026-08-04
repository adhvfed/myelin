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

struct ShedSink {
    lane: ShedLane,
    shed: HashMap<(String, &'static str), u64>,
    admit: HashMap<(String, &'static str), u64>,
    last_agent_retry_after: Option<u64>,
}

impl ShedSink {
    fn new(surface: Surface, budget: myelin_substrate::shed::SurfaceBudget) -> ShedSink {
        ShedSink {
            lane: ShedLane::with_budget(surface, budget),
            shed: HashMap::new(),
            admit: HashMap::new(),
            last_agent_retry_after: None,
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

impl Sink for ShedSink {
    fn handle(&mut self, request: &Request) {
        let class = run_class_of(request);
        let tenant = request.tenant.as_str().to_string();
        let decision = self.lane.admit(&request.tenant, class);
        match decision {
            ShedDecision::Admit => {
                *self.admit.entry((tenant, class.lane())).or_insert(0) += 1;
                if class != RunClass::Agent {
                    self.lane.release(&request.tenant, class);
                }
            }
            ShedDecision::Shed { retry_after_secs } => {
                *self.shed.entry((tenant, class.lane())).or_insert(0) += 1;
                if class == RunClass::Agent {
                    self.last_agent_retry_after = Some(retry_after_secs);
                }
            }
        }
    }
}

#[test]
fn bus_d7_agent_surge_human_lane_holds_agent_sheds_others_unaffected() {
    let thresholds = Thresholds::load_canonical().expect("thresholds.toml loads");
    assert_eq!(
        thresholds.surge.multiplier, 30,
        "the surge default-to-beat is 30×"
    );
    let multiplier =
        Multiplier::custom(thresholds.surge.multiplier).expect("a positive surge multiplier");

    let budget = thresholds
        .shed_budget(Surface::AgentMention)
        .expect("the AgentMention shed budget is in the file");
    let mut sink = ShedSink::new(Surface::AgentMention, budget);

    let surge_tenant = TenantId("acme".into());
    let surge = LoadGenerator::new(
        64,
        multiplier,
        PrincipalMix::agent_skewed(),
        StormProfile::agent_mention_storm(),
        vec![surge_tenant.clone()],
    )
    .expect("a non-empty tenant list");
    surge.drive(&mut sink);

    let other_tenant = TenantId("globex".into());
    let baseline = LoadGenerator::new(
        4,
        Multiplier::BASELINE,
        PrincipalMix::balanced(),
        StormProfile::agent_mention_storm(),
        vec![other_tenant.clone()],
    )
    .expect("a non-empty tenant list");
    baseline.drive(&mut sink);

    let human_sheds = sink.shed_of(surge_tenant.as_str(), "human");
    assert_eq!(
        human_sheds, 0,
        "BUS-D7 RED: the protected human lane was shed during the agent surge \
         (a human must NEVER queue behind agent runs) - threshold 0, NOT weakened"
    );
    assert!(
        sink.admit_of(surge_tenant.as_str(), "human") > 0,
        "the surge actually carried human traffic (the agent-skewed mix still has humans), \
         so the 0-human-sheds result is earned, not vacuous"
    );

    let agent_sheds = sink.shed_of(surge_tenant.as_str(), "agent");
    assert!(
        agent_sheds > 0,
        "BUS-D7 RED: the agent lane did NOT shed under a 30× surge (the surge must exceed the \
         agent-mention budget) - the shed is the whole point"
    );
    assert_eq!(
        sink.last_agent_retry_after,
        Some(budget.retry_after_secs),
        "every agent shed carries the surface's Retry-After (429 + Retry-After; the runtime \
         honours it - no retry-storm amplification)"
    );

    let other_total_sheds: u64 = ["human", "agent", "batch_ci", "speculative"]
        .iter()
        .map(|lane| sink.shed_of(other_tenant.as_str(), lane))
        .sum();
    assert_eq!(
        other_total_sheds, 0,
        "BUS-D7 RED: a surge on `acme` shed `globex`'s traffic - the per-tenant bulkhead failed \
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
    src.set_scalar(SignalName::CrossTenantCount, other_total_sheds as i64);

    let human_held = src.assert_labelled(
        SignalName::ShedCount,
        vec![
            Label::new("lane", "human"),
            Label::new("tenant", surge_tenant.as_str()),
        ],
        Predicate::Eq(0),
    );
    let agent_shed = src.assert_labelled(
        SignalName::ShedCount,
        vec![
            Label::new("lane", "agent"),
            Label::new("tenant", surge_tenant.as_str()),
        ],
        Predicate::Gte(1),
    );
    let cross_tenant_zero = src.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0));
    assert!(
        human_held.is_green() && agent_shed.is_green() && cross_tenant_zero.is_green(),
        "BUS-D7 GREEN: human lane held ({human_held:?}), agent lane shed ({agent_shed:?}), \
         cross-tenant 0 ({cross_tenant_zero:?})"
    );
}

#[test]
fn bus_d7_agent_lane_sheds_before_human_lane() {
    let thresholds = Thresholds::load_canonical().expect("load");
    let budget = thresholds
        .shed_budget(Surface::AgentMention)
        .expect("budget");
    let mut sink = ShedSink::new(Surface::AgentMention, budget);
    let tenant = TenantId("acme".into());

    let gen = LoadGenerator::new(
        64,
        Multiplier::SURGE,
        PrincipalMix::from_weights([3, 7, 0, 0, 0]).expect("30% human / 70% agent"),
        StormProfile::agent_mention_storm(),
        vec![tenant.clone()],
    )
    .expect("non-empty tenants");
    gen.drive(&mut sink);

    let human = sink.shed_of(tenant.as_str(), "human");
    let agent = sink.shed_of(tenant.as_str(), "agent");
    assert_eq!(human, 0, "the human lane is shed last (0 under this surge)");
    assert!(
        agent > human,
        "the agent lane sheds BEFORE the human lane (the §7.2 shed order)"
    );
}
