//! # Drill — SRCH-D6: the 30× agent/CI query surge + the protected-human-lane shed order
//! (SRCH-P25 → global P-460, M5)
//!
//! **Drill source:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` SRCH-D6
//! (30× agent/CI query surge → human search lane holds, agent sheds, others unaffected; signals:
//! shed-counts; search p99).
//! **Architecture:** `search-and-indexing.md` §6.3 (the query path runs under the principal-aware
//! shed lane — a human's interactive search holds the protected lane; agent/CI search sheds with
//! `429 + Retry-After`; per-tenant in-flight caps; "Search's query surface is one of [the OQ-K
//! surfaces]") + §6.1/§6.2 (per-tenant, in-cell; measure before you shard). **Contract:** row 1.11
//! (the shed order + per-surface budgets OQ-K) + 1.8 (the per-lane shed-count telemetry).
//! **Doctrine:** `external-insights/01-process-and-quality-doctrine.md` §3 (the 1×/10×/30× load
//! generator; the multiplier read from the FROZEN thresholds file, never hardcoded; observability is
//! part of the pass), §2 (the protected human lane; per-tenant blast-radius).
//!
//! ## What this drill proves (the dated green artifact, 2026-06-25)
//! The harness 1×/10×/30× load generator ([`myelin_harness::load_generator`]) drives a surge of mixed
//! human/agent/CI search queries (the agent-skewed mix) at the LIVE Search shed gate
//! ([`myelin_search::SearchShedGate`]) over the [`Surface::SearchQuery`] surface, whose budget is read
//! from the FROZEN thresholds file. The drill then asserts the three SRCH-D6 properties:
//!   1. **the human search lane HOLDS** — under the full 30× agent/CI surge, every human query the
//!      generator issued on the surging tenant was ADMITTED (0 human sheds); the protected lane is
//!      shed last and held within budget;
//!   2. **the agent + CI machine lanes SHED** — the agent fan-out + the CI run-log query storm were
//!      absorbed by shedding (`429 + Retry-After`, shed-count > 0), never queued unboundedly;
//!   3. **other tenants are UNAFFECTED** — a quiet co-tenant's human search is admitted within its
//!      independent per-tenant budget; the surging tenant's storm spent 0 of the quiet tenant's slots
//!      (the per-tenant bulkhead, cross-tenant impact 0).
//!
//! ## Honest recording (the TESTS line)
//! The surge is the P-S02 generator at the FULL 30× multiplier (read from the thresholds file's
//! `[surge] multiplier`, asserted == 30, NEVER hardcoded), with the agent-skewed mixed-principal mix.
//! The shed-budget NUMBERS (`per_tenant_in_flight_cap` / `human_lane_reservation` / `retry_after_secs`
//! for `SearchQuery`) are read from the thresholds file and were MEASURED here: the cap of 160 sheds
//! the machine lanes under the 30× agent/CI storm while the 40-slot reserved human lane (25% of cap,
//! above the 20% measured human-lane floor) held every human query the surge carried. They are written
//! into the thresholds file as the measured defaults-to-beat (validated against the human-lane floor
//! by `Thresholds::validate_shed_budgets()` — a future edit that starves the human lane is a LOUD
//! error, never a quiet regression).
//!
//! ## Floors named
//! - The **world-scale 30× run on real fleet hardware** (a read-node-scaled multi-node cluster) is the
//!   ONE remaining floor — the shared testing-strategy §4.1 30× fleet drill, not a per-slice floor.
//!   The shed-order LOGIC + the per-tenant fairness + the dated artifact ship now and re-run as a
//!   `cargo test` gate on every shed-path-touching change.
//! - The tuned filtered-ANN strategy + the HNSW↔IVF-PQ promotion (the vector hot-path memory-pressure
//!   upgrade, SRCH-D8 recall@k) is **SRCH-P26** — this drill is the surge/shed-order half ONLY.

use myelin_harness::load_generator::{
    LoadGenerator, LoadPrincipalKind, Multiplier, PrincipalMix, RecordingSink, Sink, StormProfile,
};
use myelin_search::{run_search_surge, SearchShedGate, SearchSurgeReport, SEARCH_SURGE_MULTIPLIER};
use myelin_substrate::shed::{RunClass, Surface};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

fn surging() -> TenantId {
    TenantId("acme-surging".into())
}
fn quiet() -> TenantId {
    TenantId("quiet-co-tenant".into())
}

/// Map a load-generator request's five-kind view onto the shed lane's [`RunClass`] (the §7.2
/// projection the limiter keys on): Human → the protected lane; Agent → the agent lane; CI/Service/
/// external-MCP → the batch/CI lane (machine clients that back off). This is the SAME projection the
/// substrate's `RunClass::derive` performs from `Principal.kind`; here we map the generator's richer
/// five-kind view directly so the CI run-log query lane is exercised as the batch lane.
fn run_class_of(kind: LoadPrincipalKind) -> RunClass {
    match kind {
        LoadPrincipalKind::Human => RunClass::Human,
        LoadPrincipalKind::Agent => RunClass::Agent,
        LoadPrincipalKind::Ci | LoadPrincipalKind::Service | LoadPrincipalKind::ExternalMcp => {
            RunClass::BatchCi
        }
    }
}

/// A sink that drives every issued request through the LIVE Search shed gate, recording per-lane admit/
/// shed outcomes and whether each human query was admitted. This is the SRCH-D6 surge harness: the
/// generator issues the mixed-principal surge; the gate decides admit/shed per the shed order.
struct ShedGateSink<'a> {
    gate: &'a mut SearchShedGate,
    /// requests realised, by lane.
    issued: std::collections::HashMap<RunClass, u64>,
    /// admitted, by lane.
    admitted: std::collections::HashMap<RunClass, u64>,
}

impl<'a> ShedGateSink<'a> {
    fn new(gate: &'a mut SearchShedGate) -> Self {
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
    fn handle(&mut self, request: &myelin_harness::load_generator::Request) {
        let class = run_class_of(request.load_kind);
        *self.issued.entry(class).or_insert(0) += 1;
        // Admit against the gate for THIS request's tenant — the per-tenant bulkhead is keyed on it.
        if self.gate.admit_class(&request.tenant, class).is_ok() {
            *self.admitted.entry(class).or_insert(0) += 1;
            // The machine lanes (agent/CI) KEEP their in-flight slot so the storm PRESSURES the cap and
            // sheds (the surge is sustained, not a one-shot exhaustion). A human search is short-lived:
            // release it immediately so a LATER human is still admitted — the protected lane holds 0
            // shed across the WHOLE surge, not just until the reserved slots fill once.
            if class == RunClass::Human {
                self.gate.release(&request.tenant, class);
            }
        }
    }
}

/// **SRCH-D6: the human search lane holds under the 30× agent/CI surge, the machine lanes shed, and a
/// quiet co-tenant is unaffected — the dated GREEN ARTIFACT.**
#[test]
fn srch_d6_human_lane_holds_agent_and_ci_shed_others_unaffected() {
    // (0) The surge multiplier + the shed budget come from the FROZEN thresholds file (never hardcoded).
    let thresholds = Thresholds::load_canonical().expect("the canonical thresholds file loads");
    assert_eq!(
        thresholds.surge.multiplier, SEARCH_SURGE_MULTIPLIER,
        "the surge multiplier is read from the file (30×), never hardcoded"
    );
    // the tuned shed budgets in the file hold the §7.6 human-lane floor (a starved row would fail here).
    thresholds
        .validate_shed_budgets()
        .expect("the tuned SearchQuery shed budget holds the human-lane floor");
    let budget = thresholds
        .shed_budget(Surface::SearchQuery)
        .expect("SearchQuery budget present in the file");

    // (1) Drive the 30× agent-skewed mixed-principal surge against the LIVE gate on the SURGING tenant.
    let mut gate =
        SearchShedGate::from_thresholds(&thresholds).expect("open the gate from the file");
    let gen = LoadGenerator::new(
        100,
        Multiplier::SURGE, // 30×
        PrincipalMix::agent_skewed(),
        StormProfile::agent_mention_storm(),
        vec![surging()],
    )
    .expect("a non-empty surge");
    let realised = {
        let mut sink = ShedGateSink::new(&mut gate);
        gen.drive(&mut sink);
        (
            sink.issued(RunClass::Human),
            sink.admitted(RunClass::Human),
            sink.issued(RunClass::Agent),
            sink.issued(RunClass::BatchCi),
        )
    };
    let (human_issued, human_admitted, agent_issued, ci_issued) = realised;

    // The surge realised a real mixed-principal stream (3000 requests, base 100 × 30).
    assert!(human_issued > 0, "the surge carried human search queries");
    assert!(
        agent_issued > 0,
        "the surge carried agent search queries (the agent fan-out)"
    );
    assert!(
        ci_issued > 0,
        "the surge carried CI/service search queries (the CI run-log query storm)"
    );

    // (2) PROPERTY 1: the human search lane HELD — EVERY human query the surge issued was admitted
    // (0 human sheds). The protected lane is shed last and held within budget under the full surge.
    assert_eq!(
        human_admitted, human_issued,
        "the human search lane HELD: all {human_issued} human queries admitted (0 shed) under the 30× surge"
    );
    assert_eq!(
        gate.shed_count(RunClass::Human),
        0,
        "the protected human lane has 0 shed under the 30× surge (§6.3)"
    );

    // (3) PROPERTY 2: the agent + CI machine lanes SHED (absorbed by shedding, never queued unbounded).
    assert!(
        gate.shed_count(RunClass::Agent) > 0,
        "the agent search lane sheds (429 + Retry-After) under the surge"
    );
    assert!(
        gate.shed_count(RunClass::BatchCi) > 0,
        "the CI run-log query lane sheds (429 + Retry-After) under the surge"
    );

    // (4) PROPERTY 3: a quiet co-tenant is UNAFFECTED — the surge spent 0 of the quiet tenant's budget,
    // and its human search is admitted within its independent per-tenant budget (cross-tenant impact 0).
    assert_eq!(
        gate.in_flight(&quiet()),
        0,
        "the surging tenant's storm spent 0 of the quiet co-tenant's budget (per-tenant bulkhead)"
    );
    assert!(
        gate.admit_class(&quiet(), RunClass::Human).is_ok(),
        "the quiet co-tenant's human search is admitted (the surge never sheds another tenant's human)"
    );

    // The dated green-artifact line (SCHED): observability is part of the pass.
    println!(
        "[P-460 SRCH-D6 GATE GREEN 2026-06-25] cap={} reserved={} retry_after_secs={} | \
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
/// human lane holds (0 shed) while the machine lanes shed harder as the load climbs. The protected
/// lane is held UNDER LOAD, not just at baseline.
#[test]
fn srch_d6_human_lane_holds_across_1x_10x_30x() {
    let thresholds = Thresholds::load_canonical().expect("load");
    for m in [Multiplier::BASELINE, Multiplier::STRESS, Multiplier::SURGE] {
        let mut gate =
            SearchShedGate::from_thresholds(&thresholds).expect("open the gate from the file");
        let gen = LoadGenerator::new(
            100,
            m,
            PrincipalMix::agent_skewed(),
            StormProfile::agent_mention_storm(),
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
        // at 10x and 30x the machine lanes are over budget and shed; at 1x they may fit (no shed needed).
        if m.factor() >= 10 {
            assert!(
                gate.shed_count(RunClass::Agent) + gate.shed_count(RunClass::BatchCi) > 0,
                "the machine lanes shed at {}x",
                m.factor()
            );
        }
        println!(
            "[P-460 SRCH-D6 {}x] human admitted={human_admitted}/{human_issued} (0 shed) | machine shed={}",
            m.factor(),
            gate.shed_count(RunClass::Agent) + gate.shed_count(RunClass::BatchCi),
        );
    }
}

/// **The deterministic two-tenant surge report is GREEN** (the three properties, driven by the
/// surge-runner directly so the cross-tenant blast-radius is asserted exactly).
#[test]
fn srch_d6_surge_report_is_green_with_a_quiet_co_tenant() {
    let thresholds = Thresholds::load_canonical().expect("load");
    let mut gate = SearchShedGate::from_thresholds(&thresholds).expect("open the gate");
    // a storm well past the per-tenant cap so BOTH machine lanes must shed.
    let report: SearchSurgeReport = run_search_surge(
        &mut gate,
        &surging(),
        &quiet(),
        500,
        500,
        thresholds.surge.multiplier,
    );
    assert!(report.is_srch_d6_green(), "{}", report.summary());
    assert!(report.surging_agent_shed_count > 0, "agent lane shed");
    assert!(
        report.surging_ci_shed_count > 0,
        "CI run-log query lane shed"
    );
    assert_eq!(report.surging_human_shed_count, 0, "human lane held");
    assert!(report.surging_human_admitted, "surging tenant's human held");
    assert!(report.quiet_human_admitted, "quiet co-tenant's human held");
    assert_eq!(report.cross_tenant_impact, 0, "cross-tenant impact 0");
    println!("[P-460 SRCH-D6 report] {}", report.summary());
}

/// **The recorded SearchQuery budget is ACHIEVABLE under the surge** (never a lowered bar, EI-01 §3):
/// the human-lane reservation sits at-or-above the measured 20%-of-cap human-lane floor, and the
/// RecordingSink confirms the generator realised a genuine mixed-principal 30× stream (the surge is
/// real, not a vacuous pass).
#[test]
fn srch_d6_recorded_budget_is_achievable_and_the_surge_is_real() {
    let thresholds = Thresholds::load_canonical().expect("load");
    let b = thresholds
        .shed_budget(Surface::SearchQuery)
        .expect("present");
    // the reserved human lane is at-or-above the measured floor (a starved lane would fail validation).
    let floor = myelin_substrate::shed::SurfaceBudget::human_lane_floor(b.per_tenant_in_flight_cap);
    assert!(
        b.human_lane_reservation >= floor,
        "the SearchQuery human-lane reservation {} must be at-or-above the measured floor {} \
         (never tuned into starvation)",
        b.human_lane_reservation,
        floor
    );

    // the generator realises a genuine 30× mixed-principal stream (3000 requests, base 100 × 30).
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
