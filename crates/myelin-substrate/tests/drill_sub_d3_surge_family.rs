//! # SUB-D3 (F6) — the 30× surge family: human lane holds (within its latency budget), agent/CI
//! sheds, cross-tenant impact 0.
//!
//! **Prompt:** P-S32 → global **P-433** (M5). **Drill catalogue:**
//! `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` §4.2 row **SUB-D3** (*30× agent
//! surge one tenant → human lane holds, agent sheds, others unaffected*) — signals `shed-counts/lane`,
//! per-tenant RED. **Architecture:** `00-platform-substrate.md` §7.2/§7.3 (the shed order +
//! the protected human lane), §7.6 (the per-surface shed budgets), §11 row D-3. **Contract-index:**
//! row **1.11** (the shed order — OWNED, proven at scale here) + row **1.8** (the surge survival
//! signals — `shed-counts/lane`, per-tenant RED — CONSUMED). **Doctrine:**
//! `external-insights/01 §3` (the 1×/10×/30× load generator + the human-lane-within-budget pass) +
//! `§2` (per-tenant blast radius — one tenant's surge unaffects another).
//!
//! ## What this drill is — and how it differs from its F6 siblings (coherence, EI-01 §7)
//! SUB-D3 is the **substrate's own slice** of the F6 surge family (ADR-16), proven against the
//! substrate's OWN public surfaces — the generic [`Surface::HttpIntake`] every public surface has
//! and [`Surface::CiDispatch`] (the CI-surge profile). It is a SIBLING of, not a duplicate of:
//!   - **BUS-D7** (P-420, `drills_bus_d7_agent_surge.rs`) — the Bus reactive/dispatch tier on the
//!     `AgentMention` surface (the agent-publish storm).
//!
//!   - **ID-D9** (P-424, identity-service) — the authz hot-path `check` surge on the authz surface.
//!
//! Each owner proves the SAME three F6 properties on the surface it owns; together they are the
//! master M5→M6 boundary's F6 family. This file proves the substrate's [`ShedLane`] primitive
//! (P-S19/P-S20, the thing the others build on) DIRECTLY under the 30× surge — the shed order +
//! the protected human lane at world scale, on the substrate's own intake.
//!
//! ## The three properties (all EXACT, never weakened — EI-01 §3)
//! 1. **The protected human lane HOLDS, within its latency budget.** Across the full 30× agent/CI
//!    surge on one tenant, the human lane is shed ZERO times (a human is never queued behind a
//!    machine lane — the reserved-human-lane slots, §7.6), AND the human-lane p99 stays within
//!    `surge.human_lane_p99_budget_us` (read from the FROZEN thresholds file, never hardcoded): an
//!    admitted human is served at its normal latency; a shed human (a `429`) would blow the budget.
//! 2. **The agent + CI lanes SHED with `429 + Retry-After`.** The machine lanes cross their
//!    non-reserved ceiling and shed; every shed carries the surface's `Retry-After` (the resilient
//!    client honours it, P-S17 — no retry-storm amplification, §6.2).
//! 3. **Another tenant is UNAFFECTED (the per-tenant bulkhead).** A second tenant trickling baseline
//!    traffic during the surge is shed ZERO times — one tenant's surge fills only that tenant's
//!    budget (EI-02 §1 / EI-01 §2 blast-radius).
//!
//! The surge magnitude (30×) and the human-lane latency budget are both read from the FROZEN
//! `thresholds.toml` — never a hardcoded literal (EI-01 §3). The verdict is bridged into the §10.2
//! harness assertion library ([`SignalSource`] — `ShedCount` labelled per lane + `RequestDuration`
//! per `{kind,tenant}` + `CrossTenantCount`) so the green is LOUD, never swallowed.
//!
//! ## Floors named
//! - **The per-surface shed-budget NUMBERS** the surge measured against are the §7.6 v1 floor; the
//!   M5 budget-tuning follow-on **P-S33** tunes them to the measured values and re-runs SUB-D3
//!   against the tuned numbers. The floor DISCIPLINE (bounded + reserved human lane + shed order) is
//!   the unchanged contract proven here; only the numbers tune.
//! - **SCHED + the 10× CI smoke variant.** SUB-D3 runs at SCHED frequency at full 30× (this file's
//!   headline test); the cheaper 10× variant is the CI smoke (`sub_d3_smoke_10x_*`) — same three
//!   properties, a lighter multiplier so it rides every commit.

use std::collections::HashMap;

use myelin_harness::load_generator::{
    LoadGenerator, LoadPrincipalKind, Multiplier, PrincipalMix, Request, Sink, StormProfile,
};
use myelin_harness::{Label, Predicate, SignalName, SignalSource};
use myelin_substrate::shed::{RunClass, RunClassHeader, ShedDecision, ShedLane, Surface};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

/// Map a load-generator request onto the substrate run-class the shed lane keys on (the SAME
/// derivation the real gateway makes via [`RunClass::derive`] — no parallel classifier). CI /
/// service / external-MCP down-class themselves to the batch/CI lane via the injected header (a
/// machine client that backs off); agents stay on the agent lane; humans on the protected lane.
fn run_class_of(req: &Request) -> RunClass {
    let header = match req.load_kind {
        LoadPrincipalKind::Ci | LoadPrincipalKind::Service | LoadPrincipalKind::ExternalMcp => {
            Some(RunClassHeader::BatchCi)
        }
        LoadPrincipalKind::Human | LoadPrincipalKind::Agent => None,
    };
    RunClass::derive(&req.principal_kind, header)
}

/// A modelled per-request service latency in MICROSECONDS, for the human-lane-within-budget
/// assertion (property 1). The substrate's reserved human lane means an admitted human never queues
/// behind a machine lane, so it is served at its normal latency; a SHED human (a `429`) is NOT
/// "within budget". We model:
///   - an ADMITTED human → a small fixed service latency (well under the budget) — the reserved
///     lane held, the human was served immediately;
///   - a SHED human → a sentinel ABOVE any budget (a `429` blows the latency budget by definition).
///
/// This is the substrate-level analogue of ID-D9's measured `check` p99: the property that matters
/// is "the human lane completes within budget", and the lane holding (0 sheds) is what keeps it so.
const HUMAN_ADMIT_SERVICE_LATENCY_US: u64 = 800;
const SHED_LATENCY_SENTINEL_US: u64 = u64::MAX;

/// A sink that admits each issued request against a per-tenant [`ShedLane`] on the substrate's own
/// surface, recording the admit/shed verdict per `(tenant, lane)` and the human-lane p99 latency.
/// The substrate's public-surface admission point modelled as the §7.2 shed lane (the real gateway
/// issues the literal `429 + Retry-After`).
struct ShedSink {
    lane: ShedLane,
    /// `(tenant, lane) → shed count`.
    shed: HashMap<(String, &'static str), u64>,
    /// `(tenant, lane) → admit count`.
    admit: HashMap<(String, &'static str), u64>,
    /// Per-tenant human-lane request latencies (µs), for the within-budget p99 assertion.
    human_latencies: HashMap<String, Vec<u64>>,
    /// The `Retry-After` carried on the most recent machine-lane shed (asserted present + matching).
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

    /// The human-lane p99 latency (µs) for a tenant — the value property 1 asserts within budget. A
    /// nearest-rank p99 over the recorded human latencies; a single shed human (sentinel) pushes the
    /// p99 to the sentinel (so the budget assertion fails LOUDLY, never silently passes).
    fn human_p99_us(&self, tenant: &str) -> Option<u64> {
        let mut v = self.human_latencies.get(tenant)?.clone();
        if v.is_empty() {
            return None;
        }
        v.sort_unstable();
        // nearest-rank p99: ceil(0.99 * n) - 1 (0-indexed), clamped into range.
        let n = v.len();
        let rank = ((99 * n).div_ceil(100)).max(1) - 1;
        Some(v[rank.min(n - 1)])
    }
}

impl Sink for ShedSink {
    fn handle(&mut self, request: &Request) {
        let class = run_class_of(request);
        let tenant = request.tenant.as_str().to_string();
        // The realistic surge model (§7.4): a sustained MACHINE run (agent / batch-CI) HOLDS its
        // permit for the duration of the run (the surge keeps the machine lanes saturated — that IS
        // the storm), while an interactive HUMAN admits-then-COMPLETES quickly (release). So the
        // machine lanes fill their non-reserved budget and shed, while the reserved slots stay free
        // for the humans who admit + complete. This is precisely "humans never queue behind machine
        // runs": a long-lived machine run cannot occupy a reserved-for-human slot, so a human always
        // finds one. A model that released the machine permits would never saturate (no storm); a
        // model that held the human permits would falsely shed humans the interactive lane never holds.
        let decision = self.lane.admit(&request.tenant, class);
        match decision {
            ShedDecision::Admit => {
                *self
                    .admit
                    .entry((tenant.clone(), class.lane()))
                    .or_insert(0) += 1;
                if class == RunClass::Human {
                    // an admitted human is served at its normal latency (within budget) and releases.
                    self.human_latencies
                        .entry(tenant)
                        .or_default()
                        .push(HUMAN_ADMIT_SERVICE_LATENCY_US);
                    self.lane.release(&request.tenant, class);
                } else if class != RunClass::Agent {
                    // short batch/CI completes immediately too (interactive-adjacent). The AGENT lane
                    // HOLDS (the sustained storm pressure the human lane must survive).
                    self.lane.release(&request.tenant, class);
                }
            }
            ShedDecision::Shed { retry_after_secs } => {
                *self.shed.entry((tenant.clone(), class.lane())).or_insert(0) += 1;
                if class == RunClass::Human {
                    // a shed human is a 429 — that BLOWS the latency budget (not "within budget").
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

/// Drive a surge of a given multiplier on the surge tenant + a baseline trickle on a second tenant,
/// against the substrate's surface, and assert the three SUB-D3 properties + the human-lane latency
/// budget. Shared by the 30× headline drill and the 10× CI smoke variant (same properties, lighter
/// load) so there is ONE assertion path (no drift between the smoke and the full drill).
fn drive_and_assert_sub_d3(surface: Surface, multiplier: Multiplier, base_requests: u64) {
    // Both the surge magnitude and the human-lane latency budget come from the FROZEN file.
    let thresholds = Thresholds::load_canonical().expect("thresholds.toml loads");
    let budget = thresholds
        .shed_budget(surface)
        .expect("the surface's shed budget is in the file");
    let human_lane_p99_budget_us = thresholds.surge.human_lane_p99_budget_us;
    let mut sink = ShedSink::new(surface, budget);

    // The SURGE tenant (acme): an agent/CI-skewed mix at the configured multiplier. The base is
    // chosen so even the 10× smoke well exceeds the surface budget (the human lane must survive even
    // when the machine lanes are hammered).
    let surge_tenant = TenantId("acme".into());
    let surge = LoadGenerator::new(
        base_requests,
        multiplier,
        PrincipalMix::agent_skewed(), // mostly agent + CI machine traffic, a thin human lane.
        StormProfile::ci_surge(),
        vec![surge_tenant.clone()],
    )
    .expect("a non-empty tenant list");
    surge.drive(&mut sink);

    // A SECOND tenant (globex) trickling baseline traffic DURING the surge — its budget is its own.
    let other_tenant = TenantId("globex".into());
    let baseline = LoadGenerator::new(
        4, // a small baseline trickle (4 requests at 1×).
        Multiplier::BASELINE,
        PrincipalMix::balanced(),
        StormProfile::ci_surge(),
        vec![other_tenant.clone()],
    )
    .expect("a non-empty tenant list");
    baseline.drive(&mut sink);

    // ── (1a) THE HUMAN LANE HELD: 0 human sheds on the surge tenant. ──
    let human_sheds = sink.shed_of(surge_tenant.as_str(), "human");
    assert_eq!(
        human_sheds, 0,
        "SUB-D3 RED: the protected human lane was shed during the {multiplier:?} surge on \
         {surface:?} (a human must NEVER queue behind a machine lane) — threshold 0, NOT weakened"
    );
    let human_admits = sink.admit_of(surge_tenant.as_str(), "human");
    assert!(
        human_admits > 0,
        "the surge actually carried human traffic (the agent-skewed mix still has a human lane), \
         so the 0-human-sheds result is earned, not vacuous"
    );

    // ── (1b) THE HUMAN LANE HELD WITHIN ITS LATENCY BUDGET (read from the file, not hardcoded). ──
    let human_p99 = sink
        .human_p99_us(surge_tenant.as_str())
        .expect("the surge carried human traffic, so a human-lane p99 exists");
    assert!(
        human_p99 <= human_lane_p99_budget_us,
        "SUB-D3 RED: the human-lane p99 ({human_p99} µs) blew the budget \
         ({human_lane_p99_budget_us} µs) under the {multiplier:?} surge — the human lane did not \
         hold within budget; fix the deliverable, do NOT weaken the budget (EI-01 §3)"
    );

    // ── (2) THE MACHINE LANES SHED with 429 + Retry-After. ──
    let agent_sheds = sink.shed_of(surge_tenant.as_str(), "agent");
    let batch_ci_sheds = sink.shed_of(surge_tenant.as_str(), "batch_ci");
    let machine_sheds = agent_sheds + batch_ci_sheds;
    assert!(
        machine_sheds > 0,
        "SUB-D3 RED: the machine lanes did NOT shed under a {multiplier:?} surge (the surge must \
         exceed the surface budget) — the shed is the whole point"
    );
    assert_eq!(
        sink.last_machine_retry_after,
        Some(budget.retry_after_secs),
        "every machine-lane shed carries the surface's Retry-After (429 + Retry-After; the \
         resilient client honours it — no retry-storm amplification, §6.2)"
    );

    // ── (3) THE OTHER TENANT WAS UNAFFECTED: 0 sheds for globex (per-tenant bulkhead). ──
    let other_total_sheds: u64 = ["human", "agent", "batch_ci", "speculative"]
        .iter()
        .map(|lane| sink.shed_of(other_tenant.as_str(), lane))
        .sum();
    assert_eq!(
        other_total_sheds, 0,
        "SUB-D3 RED: a surge on `acme` shed `globex`'s traffic — the per-tenant bulkhead failed \
         (one tenant's surge must NEVER shed another's) — threshold 0, NOT weakened"
    );
    assert!(
        sink.admit_of(other_tenant.as_str(), "human") > 0
            || sink.admit_of(other_tenant.as_str(), "agent") > 0
            || sink.admit_of(other_tenant.as_str(), "batch_ci") > 0,
        "the other tenant's baseline traffic was actually admitted (its budget is its own)"
    );

    // ── BRIDGE into the §10.2 harness assertion library — LOUD greens, never swallowed. ──
    let mut src = SignalSource::new();
    // shed-count per lane (the §10.2 row-7 / contract-1.8 signal): human lane == 0, machine lanes >= 1.
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
    // RED per principal-kind per tenant: the human-lane p99 duration (contract 1.8, the
    // 30×-surge-human-lane-holds signal). Asserted within the file's budget.
    src.set_labelled(
        SignalName::RequestDuration,
        vec![
            Label::new("kind", "human"),
            Label::new("tenant", surge_tenant.as_str()),
        ],
        human_p99 as i64,
    );
    // the cross-tenant shed projection (the per-tenant-bulkhead zero).
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
    // the machine lanes shed: agent OR batch/CI crossed its ceiling (at least one >= 1).
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

/// **SUB-D3 (the headline, SCHED frequency): the full 30× surge family on the substrate's HTTP
/// intake — the human lane holds within its latency budget, the agent/CI lanes shed with `429 +
/// Retry-After`, and a second tenant is unaffected.**
///
/// The surge multiplier is read from the FROZEN thresholds file (`surge.multiplier` == 30×), never a
/// hardcoded literal. This is the dated green artifact the DoD names (the passing test IS the
/// artifact, re-run on every change).
#[test]
fn sub_d3_30x_surge_family_human_lane_holds_machine_sheds_others_unaffected() {
    let thresholds = Thresholds::load_canonical().expect("thresholds.toml loads");
    assert_eq!(
        thresholds.surge.multiplier, 30,
        "the surge default-to-beat is 30×"
    );
    let multiplier =
        Multiplier::custom(thresholds.surge.multiplier).expect("a positive surge multiplier");
    // base 64; 64 * 30 = 1920 issued on the surge tenant, far over the HttpIntake cap (200).
    drive_and_assert_sub_d3(Surface::HttpIntake, multiplier, 64);
}

/// **SUB-D3 on the CI-dispatch surface (the CI-surge profile, §7.6 row 1): the same three
/// properties on the CI dispatch surface where CI + agent share the wallet.** CI is the batch lane
/// (no human reservation on CiDispatch itself), but the surge still proves the machine lanes shed
/// and a second tenant is unaffected; the human-lane-within-budget property rides the HttpIntake
/// surface (a human's interactive request hits the generic intake, not the CI dispatch queue). Here
/// we assert the machine-shed + cross-tenant-0 properties on the CI surface directly.
#[test]
fn sub_d3_30x_surge_ci_dispatch_machine_sheds_others_unaffected() {
    let thresholds = Thresholds::load_canonical().expect("load");
    let multiplier =
        Multiplier::custom(thresholds.surge.multiplier).expect("a positive surge multiplier");
    let budget = thresholds.shed_budget(Surface::CiDispatch).expect("budget");
    let mut sink = ShedSink::new(Surface::CiDispatch, budget);

    let surge_tenant = TenantId("acme".into());
    // a CI/agent-heavy mix (no human reservation on CiDispatch — CI is the batch lane).
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

    // the machine lanes shed under the surge.
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
    // the second tenant is unaffected (per-tenant bulkhead).
    let other_sheds: u64 = ["agent", "batch_ci"]
        .iter()
        .map(|l| sink.shed_of(other_tenant.as_str(), l))
        .sum();
    assert_eq!(
        other_sheds, 0,
        "SUB-D3 RED: a CI surge on `acme` shed `globex`'s CI traffic — the per-tenant bulkhead failed"
    );
}

/// **The 10× CI smoke variant (rides every commit): the same three SUB-D3 properties at a lighter
/// 10× multiplier.** SUB-D3 runs at SCHED frequency at full 30×; this cheaper 10× variant is the CI
/// smoke that re-greens the property on every change (testing-strategy §4.2 "a cheaper 10× CI smoke
/// variant"). Same assertion path as the headline — no drift between smoke and full drill.
#[test]
fn sub_d3_smoke_10x_human_lane_holds_machine_sheds_others_unaffected() {
    // base 64; 64 * 10 = 640 issued, still well over the HttpIntake cap (200).
    drive_and_assert_sub_d3(Surface::HttpIntake, Multiplier::STRESS, 64);
}

/// **The shed order among lanes (§7.2): the machine lanes shed BEFORE the human lane.** A focused
/// unit over the same surface: drive a mixed human+agent load past saturation and assert the agent
/// lane's shed count strictly exceeds the human lane's (which is 0) — the graded-ceiling shed order,
/// the property the surge drill leans on (the substrate's own restatement of the §7.2 order under
/// the surge generator).
#[test]
fn sub_d3_machine_lane_sheds_before_human_lane() {
    let thresholds = Thresholds::load_canonical().expect("load");
    let budget = thresholds.shed_budget(Surface::HttpIntake).expect("budget");
    let mut sink = ShedSink::new(Surface::HttpIntake, budget);
    let tenant = TenantId("acme".into());

    // a 30× surge with a mix that guarantees BOTH lanes carry traffic.
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
