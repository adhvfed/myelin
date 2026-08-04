use std::collections::HashMap;
use std::time::Instant;

use myelin_events::{OutboxStore, Timestamp};
use myelin_harness::load_generator::{
    LoadGenerator, LoadPrincipalKind, Multiplier, PrincipalMix, Request, Sink, StormProfile,
};
use myelin_harness::telemetry::{Label, Predicate, SignalName, SignalSource};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, IdentityService, ObjectId, Permission, Principal,
    PrincipalId, PrincipalKind, RelName, RelationTuple, TupleDelta, Zookie,
};
use myelin_identity_service::{StoreBackedCheck, TupleStore};
use myelin_storage::TenantScope;
use myelin_substrate::shed::{RunClass, RunClassHeader, ShedDecision, ShedLane, Surface};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::{ArtifactRef, Region, TenantId};

fn principal(tenant: &str, id: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    p.region = Region("eu-west".into());
    p
}

fn scope_of(p: &Principal) -> TenantScope {
    TenantScope::from_verified_token(p, p.region.clone())
}

fn add(object: &str, relation: &str, subject: &str) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    })
}

fn at_latest() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::Strong,
    }
}

fn seeded_authz_service(tenant: &str) -> StoreBackedCheck {
    let scope = scope_of(&principal(tenant, "p-admin"));
    let store = TupleStore::new(OutboxStore::new());
    store
        .write_tuples(
            &scope,
            &principal(tenant, "p-admin"),
            &[
                add("project:web", "parent_team", "team:eng#view"),
                add("team:eng", "member", "p:surge-human"),
            ],
            None,
            None,
            Timestamp("2026-06-24T00:00:00Z".into()),
        )
        .expect("seed the tenant grant");
    StoreBackedCheck::new(store)
}

fn run_class_of(req: &Request) -> RunClass {
    let header = match req.load_kind {
        LoadPrincipalKind::Ci | LoadPrincipalKind::Service | LoadPrincipalKind::ExternalMcp => {
            Some(RunClassHeader::BatchCi)
        }
        LoadPrincipalKind::Human | LoadPrincipalKind::Agent => None,
    };
    RunClass::derive(&req.principal_kind, header)
}

struct AuthzShedSink {
    lane: ShedLane,
    services: HashMap<String, StoreBackedCheck>,
    shed: HashMap<(String, &'static str), u64>,
    admit: HashMap<(String, &'static str), u64>,
    latencies_us: HashMap<(String, &'static str), Vec<u64>>,
    last_agent_retry_after: Option<u64>,
    cross_tenant_reads: i64,
}

impl AuthzShedSink {
    fn new(surface: Surface, budget: myelin_substrate::shed::SurfaceBudget) -> AuthzShedSink {
        AuthzShedSink {
            lane: ShedLane::with_budget(surface, budget),
            services: HashMap::new(),
            shed: HashMap::new(),
            admit: HashMap::new(),
            latencies_us: HashMap::new(),
            last_agent_retry_after: None,
            cross_tenant_reads: 0,
        }
    }

    fn with_service(mut self, tenant: &str, svc: StoreBackedCheck) -> AuthzShedSink {
        self.services.insert(tenant.to_string(), svc);
        self
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

    fn p99_us(&self, tenant: &str, lane: &'static str) -> Option<u64> {
        let mut v = self
            .latencies_us
            .get(&(tenant.to_string(), lane))
            .cloned()
            .unwrap_or_default();
        if v.is_empty() {
            return None;
        }
        v.sort_unstable();
        let rank = (((v.len() as f64) * 0.99).ceil() as usize).max(1) - 1;
        Some(v[rank.min(v.len() - 1)])
    }
}

impl Sink for AuthzShedSink {
    fn handle(&mut self, request: &Request) {
        let class = run_class_of(request);
        let tenant = request.tenant.as_str().to_string();
        match self.lane.admit(&request.tenant, class) {
            ShedDecision::Admit => {
                *self
                    .admit
                    .entry((tenant.clone(), class.lane()))
                    .or_insert(0) += 1;
                if let Some(svc) = self.services.get(&tenant) {
                    let subject = {
                        let mut p = principal(&tenant, "p:surge-human");
                        p.kind = request.principal_kind.clone();
                        p
                    };
                    let object = ArtifactRef("project:web".into());
                    let start = Instant::now();
                    let decision = svc.check(
                        &subject,
                        &Permission("view".into()),
                        &object,
                        &at_latest(),
                        None,
                    );
                    let elapsed_us = start.elapsed().as_micros() as u64;
                    self.latencies_us
                        .entry((tenant.clone(), class.lane()))
                        .or_default()
                        .push(elapsed_us);
                    let _ = decision;

                    let spoof_subject = {
                        let mut m = subject.clone();
                        m.tenant = TenantId("evil-corp".into());
                        m.kind = PrincipalKind::Human;
                        m
                    };
                    if let Some(victim_svc) = self.services.get(&tenant) {
                        if victim_svc.check(
                            &spoof_subject,
                            &Permission("view".into()),
                            &object,
                            &at_latest(),
                            None,
                        ) == Ok(Decision::Allow)
                        {
                            self.cross_tenant_reads += 1;
                        }
                    }
                }
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
fn id_d9_authz_surge_human_lane_holds_agent_sheds_cross_tenant_zero() {
    let thresholds = Thresholds::load_canonical().expect("thresholds.toml loads");
    assert_eq!(
        thresholds.surge.multiplier, 30,
        "the surge default-to-beat is 30×"
    );
    let multiplier =
        Multiplier::custom(thresholds.surge.multiplier).expect("a positive surge multiplier");
    let human_lane_p99_budget_us = thresholds.authz_surge.human_lane_p99_budget_us;

    let budget = thresholds
        .shed_budget(Surface::HttpIntake)
        .expect("the HttpIntake shed budget is in the file");

    let surge_tenant = TenantId("acme".into());
    let other_tenant = TenantId("globex".into());

    let mut sink = AuthzShedSink::new(Surface::HttpIntake, budget)
        .with_service(
            surge_tenant.as_str(),
            seeded_authz_service(surge_tenant.as_str()),
        )
        .with_service(
            other_tenant.as_str(),
            seeded_authz_service(other_tenant.as_str()),
        );

    let surge = LoadGenerator::new(
        64,
        multiplier,
        PrincipalMix::agent_skewed(),
        StormProfile::agent_mention_storm(),
        vec![surge_tenant.clone()],
    )
    .expect("a non-empty tenant list");
    surge.drive(&mut sink);

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
        "ID-D9 RED: the protected human lane was shed during the 30× authz surge \
         (a human must NEVER queue behind agent runs) - threshold 0, NOT weakened"
    );
    let human_admits = sink.admit_of(surge_tenant.as_str(), "human");
    assert!(
        human_admits > 0,
        "the surge actually carried human authz traffic (the agent-skewed mix still has humans), \
         so the 0-human-sheds result is earned, not vacuous"
    );
    let human_p99_us = sink
        .p99_us(surge_tenant.as_str(), "human")
        .expect("the human lane resolved real checks → a p99 exists");
    assert!(
        human_p99_us <= human_lane_p99_budget_us,
        "ID-D9 RED: human-lane authz p99 ({human_p99_us} µs) exceeded the budget \
         ({human_lane_p99_budget_us} µs) under the 30× surge - the budget is NOT weakened to pass"
    );

    let agent_sheds = sink.shed_of(surge_tenant.as_str(), "agent");
    assert!(
        agent_sheds > 0,
        "ID-D9 RED: the agent lane did NOT shed under a 30× surge (the surge must exceed the authz \
         front-door budget) - the shed is the whole point"
    );
    assert_eq!(
        sink.last_agent_retry_after,
        Some(budget.retry_after_secs),
        "every agent shed carries the surface's Retry-After (429 + Retry-After; our clients honour \
         it - no retry-storm amplification)"
    );

    let other_total_sheds: u64 = ["human", "agent", "batch_ci", "speculative"]
        .iter()
        .map(|lane| sink.shed_of(other_tenant.as_str(), lane))
        .sum();
    assert_eq!(
        other_total_sheds, 0,
        "ID-D9 RED: a surge on `acme` shed `globex`'s authz traffic - the per-tenant bulkhead failed \
         (one tenant's surge must NEVER shed another's) - threshold 0, NOT weakened"
    );
    assert!(
        sink.admit_of(other_tenant.as_str(), "human") > 0
            || sink.admit_of(other_tenant.as_str(), "agent") > 0
            || sink.admit_of(other_tenant.as_str(), "batch_ci") > 0,
        "the other tenant's baseline authz traffic was actually admitted (its budget is its own)"
    );
    assert_eq!(
        sink.cross_tenant_reads, 0,
        "ID-D9 RED: a spoofed cross-tenant authz read resolved to Allow UNDER the surge - the \
         identity §6 tenant-predicate floor failed under load - threshold 0, NOT weakened"
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
        SignalName::RequestDuration,
        vec![
            Label::new("kind", "human"),
            Label::new("tenant", surge_tenant.as_str()),
        ],
        human_p99_us as i64,
    );
    src.set_scalar(
        SignalName::CrossTenantCount,
        (other_total_sheds as i64) + sink.cross_tenant_reads,
    );

    let human_held = src.assert_labelled(
        SignalName::ShedCount,
        vec![
            Label::new("lane", "human"),
            Label::new("tenant", surge_tenant.as_str()),
        ],
        Predicate::Eq(0),
    );
    let human_p99 = src.assert_labelled(
        SignalName::RequestDuration,
        vec![
            Label::new("kind", "human"),
            Label::new("tenant", surge_tenant.as_str()),
        ],
        Predicate::Lte(human_lane_p99_budget_us as i64),
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
    human_held.expect_green();
    human_p99.expect_green();
    agent_shed.expect_green();
    cross_tenant_zero.expect_green();

    println!(
        "[P-424 DRILL GREEN 2026-06-24] ID-D9 authz surge: surge_tenant=acme other=globex \
         multiplier=30× issued≈1920 → human lane HELD (0 sheds, {human_admits} admits, \
         authz p99 {human_p99_us} µs ≤ {human_lane_p99_budget_us} µs budget); agent lane SHED \
         {agent_sheds}× (429 + Retry-After {}s); cross-tenant impact 0 (other-tenant sheds 0, \
         spoofed cross-tenant reads 0)",
        budget.retry_after_secs
    );
}

#[test]
fn id_d9_agent_lane_sheds_before_human_lane() {
    let thresholds = Thresholds::load_canonical().expect("load");
    let budget = thresholds.shed_budget(Surface::HttpIntake).expect("budget");
    let tenant = TenantId("acme".into());
    let mut sink = AuthzShedSink::new(Surface::HttpIntake, budget)
        .with_service(tenant.as_str(), seeded_authz_service(tenant.as_str()));

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
    assert_eq!(
        human, 0,
        "the human lane is shed LAST (0 under this surge) - the protected-human-lane invariant"
    );
    assert!(
        agent > human,
        "the agent lane sheds BEFORE the human lane (the §7.2 shed order on the authz surface)"
    );
}
