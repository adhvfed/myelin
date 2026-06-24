//! # BUS-D7 (F6) — the 30× agent publish surge: human lane holds, agent sheds, others unaffected.
//!
//! **Drill catalogue:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` row **BUS-D7**
//! (*30× agent publish surge one tenant → human/control lane holds, agent sheds, others unaffected*).
//! **Architecture:** `event-bus.md` §4.7 (the reactive/dispatch tier + the OQ-K per-surface shed
//! budgets — the agent-mention storm sheds the agent lane with `429 + Retry-After`; humans never
//! queue behind agent runs) + `00-platform-substrate.md` §7.2/§7.6 (the shed order + the per-tenant
//! bulkhead). Thresholds: **the human lane is NEVER shed; the agent lane SHEDS (429 + Retry-After);
//! another tenant is UNAFFECTED (cross-tenant shed == 0)** — all EXACT, never weakened.
//!
//! ## What this drill proves (the EB-29 M5 follow-on — the OQ-K shed budgets tuned + proven)
//! EB-23 (P-143) ships the dispatch tier; the substrate ships the [`ShedLane`] primitive
//! (P-S19/P-S20) with the §7.6 v1-floor budgets. BUS-D7 proves them under the REAL 30× surge driven
//! by the harness [`LoadGenerator`] (the 1×/10×/30× generator, agent-skewed mix), against the
//! agent-mention surface — the surface the agent-publish storm hits:
//!
//! 1. **The protected human lane HOLDS.** Across the whole 30× agent-skewed surge on one tenant, the
//!    human lane is shed ZERO times — a human is never shed while a machine lane occupies the surface
//!    (the reserved-human-lane slots, §7.6).
//! 2. **The agent lane SHEDS with `429 + Retry-After`.** The agent lane crosses its non-reserved
//!    ceiling and sheds; every shed carries the surface's `Retry-After` (the runtime honours it,
//!    ADR-16.3 — no retry-storm amplification).
//! 3. **Another tenant is UNAFFECTED (the per-tenant bulkhead).** A second tenant trickling baseline
//!    traffic during the surge is shed ZERO times — one tenant's surge fills only that tenant's
//!    budget (EI-02 §1 blast-radius).
//!
//! The surge magnitude is read from the FROZEN thresholds file (`surge.multiplier` == 30×) — never a
//! hardcoded literal (EI-01 §3). The verdict is bridged into the §10.2 harness assertion library
//! (`ShedCount` labelled per lane + `CrossTenantCount`) so the green is LOUD, never swallowed.

use std::collections::HashMap;

use myelin_harness::load_generator::{
    LoadGenerator, LoadPrincipalKind, Multiplier, PrincipalMix, Request, Sink, StormProfile,
};
use myelin_harness::{Label, Predicate, SignalName, SignalSource};
use myelin_substrate::shed::{RunClass, RunClassHeader, ShedDecision, ShedLane, Surface};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

/// Map a load-generator request onto the substrate run-class the shed lane keys on. The load kind
/// projects onto the frozen `PrincipalKind`; CI / service / external-MCP down-class themselves to
/// the batch/CI lane via the injected header (a machine client that backs off), agents stay on the
/// agent lane, humans on the protected lane. This is exactly `RunClass::derive`'s input — the same
/// derivation the real gateway makes (no parallel classifier).
fn run_class_of(req: &Request) -> RunClass {
    let header = match req.load_kind {
        LoadPrincipalKind::Ci | LoadPrincipalKind::Service | LoadPrincipalKind::ExternalMcp => {
            Some(RunClassHeader::BatchCi)
        }
        LoadPrincipalKind::Human | LoadPrincipalKind::Agent => None,
    };
    RunClass::derive(&req.principal_kind, header)
}

/// A sink that admits each issued request against a per-tenant [`ShedLane`] on the agent-mention
/// surface, recording the admit/shed verdict per `(tenant, lane)`. The reactive/dispatch tier's
/// admission point modelled as the §7.6 shed lane (the real gateway issues the `429 + Retry-After`).
struct ShedSink {
    lane: ShedLane,
    /// `(tenant, lane) → shed count` — the per-tenant-per-lane shed tally the drill asserts.
    shed: HashMap<(String, &'static str), u64>,
    /// `(tenant, lane) → admit count`.
    admit: HashMap<(String, &'static str), u64>,
    /// The `Retry-After` carried on the most recent agent shed (asserted present + matching budget).
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
        // The realistic surge model (§7.4): an AGENT publish-storm run HOLDS its permit for the
        // duration of the run (the surge keeps the agent lane saturated — that is the storm), while
        // an interactive HUMAN (and a short batch/CI request) admits-then-COMPLETES quickly
        // (release). So the agent lane fills its non-reserved budget and sheds, while the reserved
        // slots stay free for the humans who admit + complete. This is precisely "humans never queue
        // behind agent runs": a long-lived agent run cannot occupy a reserved-for-human slot, so a
        // human always finds one. A model that released the agent permits would never saturate
        // (no storm); a model that held the human permits would falsely shed humans the real
        // interactive lane never holds.
        let decision = self.lane.admit(&request.tenant, class);
        match decision {
            ShedDecision::Admit => {
                *self.admit.entry((tenant, class.lane())).or_insert(0) += 1;
                // Non-agent lanes complete immediately (interactive / short batch) → release the
                // permit. Agent runs HOLD (the sustained storm pressure the human lane must survive).
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

/// **BUS-D7 (the headline): a 30× agent-skewed surge on ONE tenant → the human lane holds, the agent
/// lane sheds with `429 + Retry-After`, and a second tenant is unaffected.**
#[test]
fn bus_d7_agent_surge_human_lane_holds_agent_sheds_others_unaffected() {
    // The surge magnitude is read from the FROZEN thresholds file — never a hardcoded literal.
    let thresholds = Thresholds::load_canonical().expect("thresholds.toml loads");
    assert_eq!(
        thresholds.surge.multiplier, 30,
        "the surge default-to-beat is 30×"
    );
    let multiplier =
        Multiplier::custom(thresholds.surge.multiplier).expect("a positive surge multiplier");

    // The agent-mention surface budget — the §7.6 floor (read from the file; the agent-publish storm
    // hits this surface). It has a RESERVED human-lane fraction the surge must not breach.
    let budget = thresholds
        .shed_budget(Surface::AgentMention)
        .expect("the AgentMention shed budget is in the file");
    let mut sink = ShedSink::new(Surface::AgentMention, budget);

    // The SURGE tenant (acme): 30× agent-skewed traffic. A meaningful base so the surge well exceeds
    // the surface budget (the human lane must survive even when the agent lane is hammered).
    let surge_tenant = TenantId("acme".into());
    let surge = LoadGenerator::new(
        64, // base requests; 64 * 30 = 1920 issued, far over the agent-mention cap.
        multiplier,
        PrincipalMix::agent_skewed(), // mostly agents (the F6 surge mix).
        StormProfile::agent_mention_storm(),
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
        StormProfile::agent_mention_storm(),
        vec![other_tenant.clone()],
    )
    .expect("a non-empty tenant list");
    baseline.drive(&mut sink);

    // ── (1) THE HUMAN LANE HELD: 0 human sheds on the surge tenant. ──
    let human_sheds = sink.shed_of(surge_tenant.as_str(), "human");
    assert_eq!(
        human_sheds, 0,
        "BUS-D7 RED: the protected human lane was shed during the agent surge \
         (a human must NEVER queue behind agent runs) — threshold 0, NOT weakened"
    );
    assert!(
        sink.admit_of(surge_tenant.as_str(), "human") > 0,
        "the surge actually carried human traffic (the agent-skewed mix still has humans), \
         so the 0-human-sheds result is earned, not vacuous"
    );

    // ── (2) THE AGENT LANE SHED with 429 + Retry-After. ──
    let agent_sheds = sink.shed_of(surge_tenant.as_str(), "agent");
    assert!(
        agent_sheds > 0,
        "BUS-D7 RED: the agent lane did NOT shed under a 30× surge (the surge must exceed the \
         agent-mention budget) — the shed is the whole point"
    );
    assert_eq!(
        sink.last_agent_retry_after,
        Some(budget.retry_after_secs),
        "every agent shed carries the surface's Retry-After (429 + Retry-After; the runtime \
         honours it — no retry-storm amplification)"
    );

    // ── (3) THE OTHER TENANT WAS UNAFFECTED: 0 sheds for globex (per-tenant bulkhead). ──
    let other_total_sheds: u64 = ["human", "agent", "batch_ci", "speculative"]
        .iter()
        .map(|lane| sink.shed_of(other_tenant.as_str(), lane))
        .sum();
    assert_eq!(
        other_total_sheds, 0,
        "BUS-D7 RED: a surge on `acme` shed `globex`'s traffic — the per-tenant bulkhead failed \
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
    // shed-count per lane (the §10.2 row-7 signal): human lane == 0, agent lane >= 1.
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

/// **The shed order among lanes (§7.2): the agent lane sheds BEFORE the human lane.** A focused unit
/// over the same surface: drive a mixed human+agent load past saturation and assert the agent lane's
/// shed count strictly exceeds the human lane's (which is 0) — the graded-ceiling shed order, the
/// property the surge drill leans on.
#[test]
fn bus_d7_agent_lane_sheds_before_human_lane() {
    let thresholds = Thresholds::load_canonical().expect("load");
    let budget = thresholds
        .shed_budget(Surface::AgentMention)
        .expect("budget");
    let mut sink = ShedSink::new(Surface::AgentMention, budget);
    let tenant = TenantId("acme".into());

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
    assert_eq!(human, 0, "the human lane is shed last (0 under this surge)");
    assert!(
        agent > human,
        "the agent lane sheds BEFORE the human lane (the §7.2 shed order)"
    );
}
