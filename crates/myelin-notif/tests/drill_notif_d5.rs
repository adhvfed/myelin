//! # Drill — NOTIF-D5: the 30× agent-generated notification surge + the protected-human-lane shed
//! order + the delivery-adapter bulkhead (NOTIF-P25 → global P-467, M5)
//!
//! **Drill source:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` NOTIF-D5
//! (30× agent-generated notification surge → human inbox-read lane holds, agent sheds,
//! delivery-adapter bulkhead bounds provider load; signals: shed-counts; delivery-success).
//! **Architecture:** `notifications.md` §5.2 (the fan-out scale axis + the agent-mention-storm shed
//! budget, C5/OQ-K — the protected-human-lane shed order concretised for Notif's storm profile: a
//! per-tenant agent-run in-flight cap, humans never queue behind agent runs, the agent-generated
//! notification lane sheds first with `429 + Retry-After`, a human's interactive inbox read is
//! last-to-shed, and a delivery-adapter bulkhead per provider bounds provider load) + the D-N5 row
//! ("30× agent surge on one tenant; human inbox-read latency in budget; agent lane sheds; cross-tenant
//! unaffected; bulkhead bounds provider load — asserted against the §5.2 shed budget"). **Contract:**
//! row 1.11 (the shed order + per-surface budgets OQ-K) + 1.8 (the per-lane shed-count + delivery
//! telemetry). **Doctrine:** `external-insights/01-process-and-quality-doctrine.md` §3 (the
//! 1×/10×/30× load generator; the multiplier read from the FROZEN thresholds file, never hardcoded;
//! observability — shed-counts + delivery-success — is part of the pass), §2 (the protected human
//! lane; per-tenant blast-radius).
//!
//! ## What this drill proves (the dated green artifact, 2026-06-25)
//! The harness 1×/10×/30× load generator ([`myelin_harness::load_generator`]) drives a surge of mixed
//! human/agent/CI notification ops (the agent-skewed mix, the **agent-mention-storm** profile) at the
//! LIVE Notif shed gate ([`myelin_notif::NotifShedGate`]) over the [`Surface::AgentMention`] surface,
//! whose budget is read from the FROZEN thresholds file, with every admitted notification driven
//! through the per-provider delivery bulkhead ([`myelin_notif::ProviderBulkhead`]). The drill then
//! asserts the four NOTIF-D5 properties:
//!   1. **the human inbox-read lane HOLDS** — under the full 30× agent surge, every human inbox-read
//!      the generator issued on the surging tenant was ADMITTED (0 human sheds); the protected lane is
//!      shed last and held within budget (humans never queue behind agent runs);
//!   2. **the agent-generated notification lane SHEDS** — the agent fan-out + the CI notification storm
//!      were absorbed by shedding (`429 + Retry-After`, shed-count > 0), never queued unboundedly;
//!   3. **other tenants are UNAFFECTED** — a quiet co-tenant's human inbox read is admitted within its
//!      independent per-tenant budget; the surging tenant's storm spent 0 of the quiet tenant's slots
//!      (the per-tenant bulkhead, cross-tenant impact 0);
//!   4. **the delivery-adapter bulkhead BOUNDS provider load** — the concurrent off-cell provider sends
//!      never exceeded the per-provider concurrency bound, and the over-bound sends were shed (the
//!      bulkhead fast-failed rather than buffering an unbounded queue at the provider).
//!
//! ## Honest recording (the TESTS line)
//! The surge is the P-S02 generator at the FULL 30× multiplier (read from the thresholds file's
//! `[surge] multiplier`, asserted == 30, NEVER hardcoded), with the agent-skewed mixed-principal mix
//! over the agent-mention-storm profile. The shed-budget NUMBERS (`per_tenant_in_flight_cap` /
//! `human_lane_reservation` / `retry_after_secs` for `AgentMention`) are read from the thresholds file
//! and were MEASURED there: the cap of 96 sheds the machine lanes under the 30× agent-mention storm
//! while the 24-slot reserved human lane (25% of cap, above the 20% measured human-lane floor) held
//! every human inbox read the surge carried. They are written into the thresholds file as the measured
//! defaults-to-beat (validated against the human-lane floor by `Thresholds::validate_shed_budgets()` —
//! a future edit that starves the human lane is a LOUD error, never a quiet regression).
//!
//! ## Floors named
//! - The **world-scale 30× run on real fleet hardware** is the ONE remaining floor (the shared
//!   testing-strategy §4.1 30× fleet drill, not a per-slice floor). The shed-order LOGIC + the
//!   per-tenant fairness + the bulkhead + the dated artifact ship now and re-run as a `cargo test`
//!   gate on every shed-path-touching change.
//! - The real **EU-sovereign delivery provider** (swapped into the `DeliveryAdapter` trait, [OPEN —
//!   LEGAL]) is **NOTIF-P26**; the off-cell-payload erasure residual is **NOTIF-P27** — this drill is
//!   the surge/shed-order half ONLY. The bulkhead bounds load at whatever adapter is wired.

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

/// Map a load-generator request's five-kind view onto the shed lane's [`RunClass`] (the §7.2/§5.2
/// projection the limiter keys on): Human → the protected inbox-read lane; Agent → the agent
/// notification lane; CI/Service/external-MCP → the batch/CI lane (machine clients that back off). The
/// SAME projection the substrate's `RunClass::derive` performs from `Principal.kind`.
fn run_class_of(kind: LoadPrincipalKind) -> RunClass {
    match kind {
        LoadPrincipalKind::Human => RunClass::Human,
        LoadPrincipalKind::Agent => RunClass::Agent,
        LoadPrincipalKind::Ci | LoadPrincipalKind::Service | LoadPrincipalKind::ExternalMcp => {
            RunClass::BatchCi
        }
    }
}

/// A sink that drives every issued notification op through the LIVE Notif shed gate, recording
/// per-lane admit/shed outcomes and whether each human inbox read was admitted. Every admitted
/// notification also attempts a delivery through the per-provider bulkhead (the off-cell load). This
/// is the NOTIF-D5 surge harness: the generator issues the mixed-principal surge; the gate decides
/// admit/shed per the shed order; the bulkhead bounds the provider load.
struct ShedGateSink<'a> {
    gate: &'a mut NotifShedGate,
    bulkhead: &'a mut ProviderBulkhead,
    /// the peak concurrent provider sends the bulkhead allowed (must stay ≤ the bound).
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
        // Admit against the gate for THIS request's tenant — the per-tenant bulkhead is keyed on it.
        if self.gate.admit_class(&request.tenant, class).is_ok() {
            *self.admitted.entry(class).or_insert(0) += 1;
            // The machine lanes (agent/CI) KEEP their in-flight slot so the storm PRESSURES the cap
            // and sheds (the surge is sustained). A human inbox read is short-lived: release it
            // immediately so a LATER human is still admitted — the protected lane holds 0 shed across
            // the WHOLE surge, not just until the reserved slots fill once. An admitted notification
            // attempts an off-cell delivery through the per-provider bulkhead (held, never released —
            // the concurrent send pressure the bulkhead must bound below the lane budget).
            if class == RunClass::Human {
                self.gate.release(&request.tenant, class);
            } else {
                let _ = self.bulkhead.try_send();
                self.provider_peak = self.provider_peak.max(self.bulkhead.in_flight());
            }
        }
    }
}

/// **NOTIF-D5: the human inbox-read lane holds under the 30× agent-mention surge, the machine lanes
/// shed, a quiet co-tenant is unaffected, and the delivery bulkhead bounds provider load — the dated
/// GREEN ARTIFACT.**
#[test]
fn notif_d5_human_lane_holds_agent_sheds_others_unaffected_bulkhead_bounds_provider() {
    // (0) The surge multiplier + the shed budget come from the FROZEN thresholds file (never hardcoded).
    let thresholds = Thresholds::load_canonical().expect("the canonical thresholds file loads");
    assert_eq!(
        thresholds.surge.multiplier, NOTIF_SURGE_MULTIPLIER,
        "the surge multiplier is read from the file (30×), never hardcoded"
    );
    // the tuned shed budgets in the file hold the §7.6 human-lane floor (a starved row would fail here).
    thresholds
        .validate_shed_budgets()
        .expect("the tuned AgentMention shed budget holds the human-lane floor");
    let budget = thresholds
        .shed_budget(Surface::AgentMention)
        .expect("AgentMention budget present in the file");

    // (1) Drive the 30× agent-skewed agent-mention-storm surge against the LIVE gate on the SURGING
    // tenant. The provider bulkhead bound is TIGHTER than the lane budget so the admitted deliveries
    // overflow it (the off-cell provider load is bounded below the lane budget — the §5.2 bulkhead).
    let mut gate =
        NotifShedGate::from_thresholds(&thresholds).expect("open the gate from the file");
    let mut bulkhead = ProviderBulkhead::new("email", budget.human_lane_reservation.max(1));
    let gen = LoadGenerator::new(
        100,
        Multiplier::SURGE, // 30×
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

    // The surge realised a real mixed-principal stream (3000 ops, base 100 × 30).
    assert!(human_issued > 0, "the surge carried human inbox reads");
    assert!(
        agent_issued > 0,
        "the surge carried agent notification ops (the agent fan-out)"
    );
    assert!(
        ci_issued > 0,
        "the surge carried CI/service notification ops (the batch notification storm)"
    );

    // (2) PROPERTY 1: the human inbox-read lane HELD — EVERY human read the surge issued was admitted
    // (0 human sheds). The protected lane is shed last and held within budget under the full surge.
    assert_eq!(
        human_admitted, human_issued,
        "the human inbox-read lane HELD: all {human_issued} reads admitted (0 shed) under the 30× surge"
    );
    assert_eq!(
        gate.shed_count(RunClass::Human),
        0,
        "the protected human inbox-read lane has 0 shed under the 30× surge (§5.2)"
    );

    // (3) PROPERTY 2: the agent + CI machine lanes SHED (absorbed by shedding, never queued unbounded).
    assert!(
        gate.shed_count(RunClass::Agent) > 0,
        "the agent notification lane sheds (429 + Retry-After) under the surge"
    );
    assert!(
        gate.shed_count(RunClass::BatchCi) > 0,
        "the CI/batch notification lane sheds (429 + Retry-After) under the surge"
    );

    // (4) PROPERTY 3: a quiet co-tenant is UNAFFECTED — the surge spent 0 of the quiet tenant's budget,
    // and its human inbox read is admitted within its independent per-tenant budget (cross-tenant 0).
    assert_eq!(
        gate.in_flight(&quiet()),
        0,
        "the surging tenant's storm spent 0 of the quiet co-tenant's budget (per-tenant bulkhead)"
    );
    assert!(
        gate.admit_class(&quiet(), RunClass::Human).is_ok(),
        "the quiet co-tenant's human inbox read is admitted (the surge never sheds another tenant's human)"
    );

    // (5) PROPERTY 4: the delivery-adapter bulkhead BOUNDED provider load — peak ≤ bound, and the
    // over-bound sends were shed (fast-failed, not buffered at the provider).
    assert!(
        provider_peak <= bulkhead.concurrency(),
        "the delivery-adapter bulkhead bounded provider load: peak {provider_peak} ≤ bound {}",
        bulkhead.concurrency()
    );
    assert!(
        bulkhead.shed_count() > 0,
        "the bulkhead shed the over-bound concurrent sends (it bounds provider load, never buffers)"
    );

    // The dated green-artifact line (SCHED): observability is part of the pass (shed-counts + the
    // delivery-load bound — NOTIF-D5's named signals).
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

/// **The shed order holds across ALL THREE doctrine points (1× / 10× / 30×):** at every multiplier the
/// human inbox-read lane holds (0 shed) while the machine lanes shed harder as the load climbs. The
/// protected lane is held UNDER LOAD, not just at baseline.
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
        // at 10x and 30x the machine lanes are over budget and shed; at 1x they may fit.
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

/// **The deterministic two-tenant surge report is GREEN** (the four properties, driven by the
/// surge-runner directly so the cross-tenant blast-radius + the bulkhead bound are asserted exactly).
#[test]
fn notif_d5_surge_report_is_green_with_a_quiet_co_tenant() {
    let thresholds = Thresholds::load_canonical().expect("load");
    let mut gate = NotifShedGate::from_thresholds(&thresholds).expect("open the gate");
    let b = thresholds
        .shed_budget(Surface::AgentMention)
        .expect("present");
    // a provider bound tighter than the lane budget so the admitted deliveries overflow it.
    let mut bulkhead = ProviderBulkhead::new("email", b.human_lane_reservation.max(1));
    // a storm well past the per-tenant cap so BOTH machine lanes must shed.
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

/// **The recorded AgentMention budget is ACHIEVABLE under the surge** (never a lowered bar, EI-01 §3):
/// the human-lane reservation sits at-or-above the measured 20%-of-cap human-lane floor, and the
/// RecordingSink confirms the generator realised a genuine mixed-principal 30× stream (the surge is
/// real, not a vacuous pass).
#[test]
fn notif_d5_recorded_budget_is_achievable_and_the_surge_is_real() {
    let thresholds = Thresholds::load_canonical().expect("load");
    let b = thresholds
        .shed_budget(Surface::AgentMention)
        .expect("present");
    // the reserved human lane is at-or-above the measured floor (a starved lane would fail validation).
    let floor = myelin_substrate::shed::SurfaceBudget::human_lane_floor(b.per_tenant_in_flight_cap);
    assert!(
        b.human_lane_reservation >= floor,
        "the AgentMention human-lane reservation {} must be at-or-above the measured floor {} \
         (never tuned into starvation)",
        b.human_lane_reservation,
        floor
    );

    // the generator realises a genuine 30× mixed-principal stream (3000 ops, base 100 × 30).
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
