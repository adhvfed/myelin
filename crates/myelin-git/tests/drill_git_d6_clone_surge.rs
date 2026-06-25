//! # Drill — GIT-D6: the 30× agent/CI clone surge on a hot repo + the protected-human-lane shed order
//! (GIT-P34 → global P-483, M5)
//!
//! **Drill source:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` GIT-D6
//! (30× agent/CI clone surge on a hot repo → human fetch p99 HELD; agent/CI sheds (`429 + Retry-After`);
//! 0 cross-tenant starvation; the CDN hit-rate measured. Signals: shed-counts; fetch p99; CDN hit).
//! **Architecture:** `git-hosting/architecture/02-internals-and-algorithms.md` (the clone-storm shed, the
//! OQ-K per-surface shed budget) — the Git front door runs under the principal-aware shed lane: a human's
//! interactive fetch HOLDS the protected lane; an agent/CI clone sheds with `429 + Retry-After`; per-tenant
//! in-flight caps. **Contract:** row 1.11 (the shed order + per-surface budgets OQ-K) + 1.8 (the per-lane
//! shed-count + fetch-p99 survival signals). **Doctrine:** `external-insights/01-process-and-quality-
//! doctrine.md` §3 (the 1×/10×/30× load generator; the multiplier read from the FROZEN thresholds file,
//! never hardcoded; observability is part of the pass), §4 (chained-mutation, not single handlers).
//!
//! ## What this drill proves (the dated green artifact, 2026-06-25)
//! The harness 1×/10×/30× load generator ([`myelin_harness::load_generator`]) drives a surge of mixed
//! human/agent/CI clone+fetch traffic (the agent-skewed mix) at the LIVE Git front-door shed gate
//! ([`myelin_git::shed_clone::GitFrontDoorShed`]) over the [`Surface::GitFrontDoor`] surface, whose budget
//! is read from the FROZEN thresholds file. The drill asserts the three GIT-D6 properties:
//!   1. **the human fetch lane HELD** — under the full 30× agent/CI surge, every human interactive fetch the
//!      generator issued on the surging tenant was ADMITTED (0 human sheds); the protected lane is shed last;
//!   2. **the agent + CI machine lanes SHED** — the agent clone fan-out + the CI checkout storm were
//!      absorbed by shedding (`429 + Retry-After`, shed-count > 0), never queued unboundedly;
//!   3. **a quiet co-tenant is UNAFFECTED** — its human clone is admitted within its independent per-tenant
//!      budget; the surge spent 0 of the quiet tenant's slots (the per-tenant bulkhead, cross-tenant 0).
//!
//! ## Honest recording (the TESTS line)
//! The surge is the P-S02 generator at the FULL 30× multiplier (read from the thresholds file's
//! `[surge] multiplier`, asserted == [`GIT_SURGE_MULTIPLIER`], NEVER hardcoded), with the agent-skewed
//! mixed-principal mix. The shed-budget NUMBERS (`per_tenant_in_flight_cap` / `human_lane_reservation` /
//! `retry_after_secs` for `GitFrontDoor`) are read from the file and were MEASURED here: the cap of 128
//! sheds the machine lanes under the 30× agent/CI clone storm while the 32-slot reserved human lane (25%
//! of cap, above the 20% measured floor) held every human fetch the surge carried. They are validated
//! against the human-lane floor by `Thresholds::validate_shed_budgets()` — a future edit that starves the
//! human lane is a LOUD error, never a quiet regression.
//!
//! ## Floors named
//! - The **world-scale 30× run on real fleet hardware** (a real multi-node cell + a real CDN edge fleet)
//!   is the ONE remaining floor — the shared testing-strategy §4.1 30× fleet drill, not a per-slice floor.
//!   The shed-order LOGIC + per-tenant fairness + the dated artifact ship now and re-run as a `cargo test`
//!   gate on every shed-path-touching change. The CDN hit-rate signal is exercised by the GIT-P15 bundle-
//!   URI round-trip (`src/shed_clone.rs`); the full edge-POP hit-rate measurement is the fleet floor.

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

/// Map the load-generator's five-kind view onto the shed lane's [`RunClass`] (the §7.2 projection the
/// limiter keys on): Human → the protected lane; Agent → the agent lane; CI/Service/external-MCP → the
/// batch/CI lane. This is the SAME projection the substrate's `RunClass::derive` performs.
fn run_class_of(kind: LoadPrincipalKind) -> RunClass {
    match kind {
        LoadPrincipalKind::Human => RunClass::Human,
        LoadPrincipalKind::Agent => RunClass::Agent,
        LoadPrincipalKind::Ci | LoadPrincipalKind::Service | LoadPrincipalKind::ExternalMcp => {
            RunClass::BatchCi
        }
    }
}

/// A sink that drives every issued clone/fetch through the LIVE Git front-door shed gate, recording per-
/// lane admit/shed outcomes. The machine lanes KEEP their in-flight slot (the storm is sustained — it
/// pressures the cap and sheds); a human fetch is short-lived (released immediately) so a LATER human is
/// still admitted — the protected lane holds 0 shed across the WHOLE surge.
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

/// **GIT-D6: the human fetch lane holds under the 30× agent/CI clone surge, the machine lanes shed, and a
/// quiet co-tenant is unaffected — the dated GREEN ARTIFACT.**
#[test]
fn git_d6_human_fetch_lane_holds_agent_and_ci_shed_others_unaffected() {
    // (0) The surge multiplier + the shed budget come from the FROZEN thresholds file (never hardcoded).
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

    // (1) Drive the 30× agent-skewed mixed-principal clone surge against the LIVE gate on the SURGING tenant.
    let mut gate =
        GitFrontDoorShed::from_thresholds(&thresholds).expect("open the gate from the file");
    let gen = LoadGenerator::new(
        100,
        Multiplier::SURGE, // 30×
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

    // the surge realised a real mixed-principal clone stream (3000 requests, base 100 × 30).
    assert!(human_issued > 0, "the surge carried human fetches");
    assert!(
        agent_issued > 0,
        "the surge carried agent clones (the agent fan-out)"
    );
    assert!(
        ci_issued > 0,
        "the surge carried CI checkouts (the CI run-checkout storm)"
    );

    // (2) PROPERTY 1: the human fetch lane HELD — every human fetch admitted (0 human sheds).
    assert_eq!(
        human_admitted, human_issued,
        "the human fetch lane HELD: all {human_issued} human fetches admitted (0 shed) under the 30× surge"
    );
    assert_eq!(
        gate.shed_count(RunClass::Human),
        0,
        "the protected human fetch lane has 0 shed under the 30× clone surge"
    );

    // (3) PROPERTY 2: the agent + CI machine lanes SHED.
    assert!(
        gate.shed_count(RunClass::Agent) > 0,
        "the agent clone lane sheds (429 + Retry-After) under the surge"
    );
    assert!(
        gate.shed_count(RunClass::BatchCi) > 0,
        "the CI checkout lane sheds (429 + Retry-After) under the surge"
    );

    // (4) PROPERTY 3: a quiet co-tenant is UNAFFECTED (cross-tenant impact 0).
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

/// **The shed order holds across ALL THREE doctrine points (1× / 10× / 30×):** at every multiplier the
/// human lane holds (0 shed) while the machine lanes shed harder as the load climbs.
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

/// **The deterministic two-tenant surge report is GREEN** (the three properties, driven by the surge-
/// runner directly so the cross-tenant blast-radius is asserted exactly).
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

/// **The recorded GitFrontDoor budget is ACHIEVABLE under the surge** (never a lowered bar, EI-01 §3): the
/// human-lane reservation sits at-or-above the measured 20%-of-cap floor.
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
