//! # P-ID-31 (global P-424) GATE / DRILL — ID-D9, the 30× authz-surge protected-human-lane drill.
//!
//! **Drill catalogue row ID-D9 (§4.2, F6):** *30× agent surge on the authz hot path → human lane
//! holds, agent sheds.* Survival signals: **shed-counts; authz p99.** Cadence: SCHED. This is the
//! M5 hardening of Identity (Id is *correct* at M1 — the eight M1 drills; *hardened* at M5 — this
//! surge proof). The M1→M2 exit-gate scorecard (P-ID-21) named this drill as the M5-hardening floor;
//! P-ID-31 ships it.
//!
//! **Architecture.** identity §13 (authz is the **highest-QPS shared system**; S8/S5 the scaling
//! moves) + §10 (the fail-static surface; correctness fail-closed, availability fail-static) +
//! 00-platform-substrate §7.2/§7.6 (the protected-human-lane shed order
//! `speculative → batch/CI → agent → human-last`, `429 + Retry-After`, per-tenant) + contract-index
//! rows 4.11 (the shed order on the authz surface — OWNED/finalised by this prompt), 1.8
//! (`auth_decision_latency` / shed-counts), 1.11 (the protected-human-lane shed order).
//!
//! ## What this drill proves (the F6 property, on the AUTHZ hot path)
//! EB-29 / BUS-D7 proved F6 on the Bus dispatch tier; this drill proves the SAME property on the
//! **authz `check` hot path** — the highest-QPS surface — driven by the harness [`LoadGenerator`]
//! (the 1×/10×/30× generator, agent-skewed mix) against the authz front-door shed lane:
//!
//! 1. **The protected human lane HOLDS within budget.** Across the whole 30× agent-skewed surge on
//!    one tenant, the human lane is shed ZERO times AND every admitted human `check` is resolved
//!    within the human-lane authz p99 budget (read from the FROZEN thresholds file —
//!    `authz_surge.human_lane_p99_budget_us`, never a hardcoded literal).
//! 2. **The agent lane SHEDS with `429 + Retry-After`.** The agent lane crosses its non-reserved
//!    ceiling and sheds; every shed carries the surface's `Retry-After` (our clients honour it,
//!    ADR-16.3 — no retry-storm amplification).
//! 3. **Cross-tenant impact is 0.** A second tenant trickling baseline `check` traffic during the
//!    surge is shed ZERO times (the per-tenant bulkhead) AND the surge's spoofed cross-tenant reads
//!    resolve to 0 (the identity §6 tenant-predicate floor still holds under load).
//!
//! The verdict is bridged into the §10.2 harness assertion library (`ShedCount` per lane,
//! `RequestDuration` = the authz-decision latency per kind, `CrossTenantCount`) so the green is
//! LOUD, never swallowed (EI-01 §3). The surge magnitude is read from the FROZEN thresholds file.

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

// ── fixtures (shared with the other ID drills' shape) ─────────────────────────────────────────

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

/// Build a `StoreBackedCheck` seeded so that any principal in `tenant` inherits `view` on
/// `project:web` (the org→team→project core hierarchy the engine resolves). The surge then drives a
/// real `check(view, project:web)` per request on the authz hot path.
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

/// Map a load-generator request onto the substrate run-class the shed lane keys on (the SAME
/// `RunClass::derive` the real gateway makes — no parallel classifier). CI / service / external-MCP
/// down-class themselves to the batch/CI lane via the injected header; agents stay on the agent lane;
/// humans on the protected lane.
fn run_class_of(req: &Request) -> RunClass {
    let header = match req.load_kind {
        LoadPrincipalKind::Ci | LoadPrincipalKind::Service | LoadPrincipalKind::ExternalMcp => {
            Some(RunClassHeader::BatchCi)
        }
        LoadPrincipalKind::Human | LoadPrincipalKind::Agent => None,
    };
    RunClass::derive(&req.principal_kind, header)
}

/// The authz-hot-path sink: admit each issued request against a per-tenant [`ShedLane`] on the
/// authz front door (the generic public `HttpIntake` surface every `check` enters through). An
/// ADMITTED request performs a REAL `check` against the seeded [`StoreBackedCheck`] and TIMES it
/// (the authz-decision latency, contract 1.8 `auth_decision_latency`); a SHED request records the
/// shed in its lane.
///
/// The realistic surge model (identity §13 / substrate §7.4): an AGENT authz storm HOLDS its permit
/// for the duration of the run (the surge keeps the agent lane saturated — that is the storm), while
/// an interactive HUMAN admits-then-COMPLETES the `check` quickly (release). So the agent lane fills
/// its non-reserved budget and sheds, while the reserved slots stay free for the humans who admit +
/// complete within budget. This is precisely "humans never queue behind agent runs": a long-lived
/// agent run cannot occupy a reserved-for-human slot, so a human always finds one.
struct AuthzShedSink {
    lane: ShedLane,
    /// One seeded authz service per tenant (each tenant's own partition — the cross-tenant floor).
    services: HashMap<String, StoreBackedCheck>,
    /// `(tenant, lane) → shed count`.
    shed: HashMap<(String, &'static str), u64>,
    /// `(tenant, lane) → admit count`.
    admit: HashMap<(String, &'static str), u64>,
    /// Per-(tenant, lane) admitted-`check` latencies, in microseconds — the authz-decision latency
    /// histogram the p99 is read off (the human lane's p99 is the F6 budget assertion).
    latencies_us: HashMap<(String, &'static str), Vec<u64>>,
    /// The `Retry-After` carried on the most recent agent shed (asserted present + matching budget).
    last_agent_retry_after: Option<u64>,
    /// Cross-tenant reads that resolved to Allow on a SPOOFED reference (must stay 0 under load).
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

    /// Register a tenant's seeded authz service (so an admitted request runs a real `check`).
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

    /// The p99 of admitted-`check` latencies for one (tenant, lane), in microseconds — `None` if no
    /// admitted check was timed there. Nearest-rank p99 (the standard latency quantile).
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
        // nearest-rank: ceil(0.99 * N) − 1 (0-indexed), clamped into range.
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
                // Perform a REAL authz check on the hot path and TIME it (the authz-decision latency).
                if let Some(svc) = self.services.get(&tenant) {
                    // The subject is THIS tenant's principal; the object is its own project:web. A
                    // human in the surge tenant inherits view (the engine resolves within its
                    // partition); an agent likewise checks its own tenant. The latency we time is the
                    // authz hot-path decision cost.
                    let subject = {
                        let mut p = principal(&tenant, "p:surge-human");
                        // tag the kind so the check is exercised across kinds (kind is DATA, not a
                        // branch — the decision path is identical, identity §3).
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
                    let _ = decision; // the latency, not the verdict, is the F6 signal here.

                    // CROSS-TENANT probe under load: the same admitted request also attempts a
                    // SPOOFED cross-tenant read (a surge attacker copies the object ref of ANOTHER
                    // tenant). The engine reads only the verified scope's partition, so this must
                    // resolve to Deny — cross-tenant impact 0 even under the surge.
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
                // Non-agent lanes complete immediately (interactive / short batch) → release. Agent
                // runs HOLD (the sustained storm pressure the human lane must survive).
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

/// **ID-D9 (the headline): a 30× agent-skewed surge on the authz hot path → the human lane holds
/// within the authz p99 budget, the agent lane sheds with `429 + Retry-After`, cross-tenant impact
/// is 0.**
#[test]
fn id_d9_authz_surge_human_lane_holds_agent_sheds_cross_tenant_zero() {
    // The surge magnitude + the human-lane authz p99 budget are read from the FROZEN thresholds file
    // — never hardcoded literals (EI-01 §3).
    let thresholds = Thresholds::load_canonical().expect("thresholds.toml loads");
    assert_eq!(
        thresholds.surge.multiplier, 30,
        "the surge default-to-beat is 30×"
    );
    let multiplier =
        Multiplier::custom(thresholds.surge.multiplier).expect("a positive surge multiplier");
    let human_lane_p99_budget_us = thresholds.authz_surge.human_lane_p99_budget_us;

    // The authz front door is the generic public HttpIntake surface every `check` enters through; its
    // §7.6 floor budget has a RESERVED human-lane fraction the surge must not breach.
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

    // The SURGE tenant (acme): 30× agent-skewed authz traffic, well over the surface budget.
    let surge = LoadGenerator::new(
        64, // base; 64 * 30 = 1920 issued, far over the HttpIntake cap.
        multiplier,
        PrincipalMix::agent_skewed(), // mostly agents (the F6 surge mix), with a thin human lane.
        StormProfile::agent_mention_storm(),
        vec![surge_tenant.clone()],
    )
    .expect("a non-empty tenant list");
    surge.drive(&mut sink);

    // A SECOND tenant (globex) trickling baseline authz traffic DURING the surge — its budget is its
    // own (the per-tenant bulkhead).
    let baseline = LoadGenerator::new(
        4,
        Multiplier::BASELINE,
        PrincipalMix::balanced(),
        StormProfile::agent_mention_storm(),
        vec![other_tenant.clone()],
    )
    .expect("a non-empty tenant list");
    baseline.drive(&mut sink);

    // ── (1) THE HUMAN LANE HELD: 0 human sheds AND human-lane authz p99 within budget. ──
    let human_sheds = sink.shed_of(surge_tenant.as_str(), "human");
    assert_eq!(
        human_sheds, 0,
        "ID-D9 RED: the protected human lane was shed during the 30× authz surge \
         (a human must NEVER queue behind agent runs) — threshold 0, NOT weakened"
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
         ({human_lane_p99_budget_us} µs) under the 30× surge — the budget is NOT weakened to pass"
    );

    // ── (2) THE AGENT LANE SHED with 429 + Retry-After. ──
    let agent_sheds = sink.shed_of(surge_tenant.as_str(), "agent");
    assert!(
        agent_sheds > 0,
        "ID-D9 RED: the agent lane did NOT shed under a 30× surge (the surge must exceed the authz \
         front-door budget) — the shed is the whole point"
    );
    assert_eq!(
        sink.last_agent_retry_after,
        Some(budget.retry_after_secs),
        "every agent shed carries the surface's Retry-After (429 + Retry-After; our clients honour \
         it — no retry-storm amplification)"
    );

    // ── (3) CROSS-TENANT IMPACT 0: the other tenant unaffected AND no spoofed cross-tenant read. ──
    let other_total_sheds: u64 = ["human", "agent", "batch_ci", "speculative"]
        .iter()
        .map(|lane| sink.shed_of(other_tenant.as_str(), lane))
        .sum();
    assert_eq!(
        other_total_sheds, 0,
        "ID-D9 RED: a surge on `acme` shed `globex`'s authz traffic — the per-tenant bulkhead failed \
         (one tenant's surge must NEVER shed another's) — threshold 0, NOT weakened"
    );
    assert!(
        sink.admit_of(other_tenant.as_str(), "human") > 0
            || sink.admit_of(other_tenant.as_str(), "agent") > 0
            || sink.admit_of(other_tenant.as_str(), "batch_ci") > 0,
        "the other tenant's baseline authz traffic was actually admitted (its budget is its own)"
    );
    assert_eq!(
        sink.cross_tenant_reads, 0,
        "ID-D9 RED: a spoofed cross-tenant authz read resolved to Allow UNDER the surge — the \
         identity §6 tenant-predicate floor failed under load — threshold 0, NOT weakened"
    );

    // ── BRIDGE into the §10.2 harness assertion library — LOUD greens, never swallowed. ──
    let mut src = SignalSource::new();
    // shed-counts per lane (§10.2 row-7): human lane == 0, agent lane >= 1.
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
    // authz p99 = the RED request-duration signal per principal-kind (§10.2 row 3;
    // `auth_decision_latency`, contract 1.8): the human lane's authz-decision p99, in µs.
    src.set_labelled(
        SignalName::RequestDuration,
        vec![
            Label::new("kind", "human"),
            Label::new("tenant", surge_tenant.as_str()),
        ],
        human_p99_us as i64,
    );
    // cross-tenant impact: both the bulkhead (other tenant 0 sheds) and the spoofed-read 0.
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

/// **The shed order on the authz surface (§7.2): the agent lane sheds BEFORE the human lane.** A
/// focused unit over the same surface: drive a mixed human+agent authz load past saturation and
/// assert the agent lane's shed count strictly exceeds the human lane's (which is 0) — the
/// graded-ceiling shed order the surge drill leans on. **This is the mutation-floor anchor: a
/// mutation that sheds the human lane (instead of the agent lane) MUST be caught here.**
#[test]
fn id_d9_agent_lane_sheds_before_human_lane() {
    let thresholds = Thresholds::load_canonical().expect("load");
    let budget = thresholds.shed_budget(Surface::HttpIntake).expect("budget");
    let tenant = TenantId("acme".into());
    let mut sink = AuthzShedSink::new(Surface::HttpIntake, budget)
        .with_service(tenant.as_str(), seeded_authz_service(tenant.as_str()));

    // A 30× surge with a mix that guarantees BOTH lanes carry traffic.
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
        "the human lane is shed LAST (0 under this surge) — the protected-human-lane invariant"
    );
    assert!(
        agent > human,
        "the agent lane sheds BEFORE the human lane (the §7.2 shed order on the authz surface)"
    );
}
