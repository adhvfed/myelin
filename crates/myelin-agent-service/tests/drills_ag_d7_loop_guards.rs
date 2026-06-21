//! # AG-D7 drill — the adversarial agent→agent self-trigger loop is HALTED (AG-P12 → P-224)
//!
//! The headline drill the AG-P12 GATE requires (testing-strategy AG-D7 + agent-fabric §5.5): an
//! **adversarial agent→agent self-trigger loop** — the five structural loop guards STOP it. The green
//! artifact (testing-strategy AG-D7, dated CI): the **loop halts `<=` the depth ceiling (12)**, the
//! **shared-root tripwire trips the per-tenant breaker**, the **bounded dispatch pool drops over-cap**,
//! there are **0 raw-text re-triggers** and **0 unbounded forks**. A red drill is information — never
//! weaken it to pass; **the depth ceiling is never raised to make a loop "pass"** (EI-01 §3, the prompt
//! DEFINITION OF DONE).
//!
//! **What the adversarial loop models (§5.5):** an agent that, on each hop, emits an event that would
//! re-trigger ITSELF. The five guards catch it, in cheapest-first order:
//! - the **self-guard** drops the agent's OWN emission re-arriving (`actor.principal == this agent`);
//! - the **reference gate** drops a raw-typed-text re-trigger (only a structured `artifact_ref` node,
//!   the frozen 13.1 inline node, re-triggers — 0 raw-text re-triggers);
//! - the **causal-depth ceiling** (default 12) stops a DEEP self-feeding chain AT the ceiling;
//! - the **shared-root tripwire** trips the per-tenant breaker on a WIDE same-root loop;
//! - the **bounded dispatch pool** sheds/parks the over-cap fan-out (never forks).
//!
//! Every refusal is a [`GuardVerdict::Drop`]/[`GuardVerdict::Park`] — there is NO fork. The 0-fork
//! counter ([`FlowTelemetry::fork_count`]) is the structural proof.
//!
//! **Rides the M0 failure-injection harness:** the [`DependencyBreaker`] (`Dependency::Broker`,
//! tenant-scoped — the SAME seam BUS-D4 / FLOW-D5/D6/D7 use) models the adversarial condition (the loop
//! is "broken open" — the agent keeps re-feeding itself). The drill asserts the survival signals via the
//! M0 assertion library ([`SignalSource`] / [`Predicate`]): the causal-depth max (`<=` ceiling), the
//! fork count (`== 0`), the tripwire firings (`>= 1`) — a typed green/red that is never a swallowed
//! pass (EI-01 §3).

use myelin_agent_service::{AgentLoopGuards, GuardRefusal, GuardVerdict};
use myelin_content::InlineNode;
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
    Timestamp, Visibility,
};
use myelin_flow::FlowTelemetry;
use myelin_harness::{Dependency, DependencyBreaker, Predicate, Scope, SignalName, SignalSource};
use myelin_identity::{Principal, PrincipalId, PrincipalKind, RuntimeRef};
use myelin_tenancy::{Region, TenantId};

const AGENT: &str = "agent-alice";

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn agent_principal(id: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Agent {
            runtime_ref: RuntimeRef("rt".into()),
            on_behalf_of: None,
        },
        tenant(),
    )
}

fn human_principal(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}

/// A structured `artifact_ref` inline node — the ONLY thing the reference gate admits as a re-trigger.
fn ref_node() -> InlineNode {
    InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()))
}

/// An inbound dispatch envelope the guards read — the actor + correlation + depth are what the
/// self-guard / tripwire / depth-ceiling key on (the 3.6 dispatch-tier shape).
fn inbound(actor: Principal, correlation: &str, depth: u32) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(format!("evt-{correlation}-{depth}")),
        type_: EventType("issues.comment.created".into()),
        schema_ver: 1,
        tenant: tenant(),
        region: Region("fr-par".into()),
        actor: Actor(actor),
        subject: ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
        aggregate: AggregateKey("agg-1".into()),
        causation_id: None,
        correlation_id: CorrelationId(correlation.into()),
        caused_by: None,
        depth,
        contains_personal_data: false,
        data_role: DataRole::Processor,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:00Z".into()),
        payload: serde_json::json!({}),
    }
}

/// **AG-D7 — the adversarial agent→agent self-trigger loop is HALTED by the five structural guards
/// (drops/parks, NEVER forks). Green artifact: loop halts `<=` ceiling (12), tripwire trips, pool sheds,
/// 0 raw-text re-triggers, 0 fork — dated.**
///
/// The loop runs for many hops. On EACH hop the adversary tries to re-trigger the agent four ways at
/// once: (1) replay the agent's OWN emission (self-guard), (2) feed a raw-typed-text re-trigger
/// (reference gate), (3) start a deeper child at `depth + 1` carrying a structured ref from another
/// actor (the self-feeding causal chain → depth ceiling), and (4) fan out a dispatch into the bounded
/// pool. All four are stopped; nothing forks.
#[test]
fn drill_ag_d7_adversarial_self_trigger_loop_halts_under_ceiling_zero_fork() {
    let scope = Scope::Tenant(tenant());
    let breaker = DependencyBreaker::new();
    let telemetry = FlowTelemetry::new();

    // The agent's five guards. Small TRIPWIRE + POOL caps so the wide/fan legs hit fast; the depth
    // ceiling stays at the REAL agent-lane default of 12 (NEVER weakened to pass — the AG-D7 halt bound).
    let ceiling = 12u32;
    let guards = AgentLoopGuards::with_caps(PrincipalId(AGENT.into()), ceiling, 16, 4)
        .with_telemetry(telemetry.clone());
    let root = "corr-adversarial";

    // (1) INJECT the adversarial condition: the loop is "broken open" (the agent keeps re-feeding
    //     itself). The SAME tenant-scoped Broker seam FLOW-D5/D6/D7 use.
    breaker.break_dependency(Dependency::Broker, scope.clone());
    assert!(
        breaker.is_broken(&Dependency::Broker, &scope),
        "the adversarial self-trigger loop is injected"
    );

    // (2) DRIVE the adversarial loop.
    let mut self_trigger_drops = 0u32; // guard 1: the agent's own emission re-arriving.
    let mut raw_text_drops = 0u32; //      guard 2: a raw-typed-text re-trigger.
    let mut depth = 0u32; //               the self-feeding causal chain.
    let mut child_admitted = 0u32;
    let mut depth_ceiling_drops = 0u32; // guard 3.
    let mut tripwire_drops = 0u32; //      guard 4: the per-tenant breaker.
    let mut pool_parked = 0u32; //         guard 5: the bounded dispatch pool.

    for _ in 0..200 {
        // (a) guard 1 — the agent replays its OWN emission. It MUST be dropped before anything else.
        let own = inbound(agent_principal(AGENT), root, 0);
        let v = guards.admit_dispatch(&own.actor, &ref_node(), root, own.depth);
        assert_eq!(
            v,
            GuardVerdict::Drop(GuardRefusal::SelfTrigger),
            "an agent's OWN emission can never re-trigger it (self-guard)"
        );
        self_trigger_drops += 1;

        // (b) guard 2 — a RAW-TYPED-TEXT re-trigger (a human pasting `@agent-alice please loop` as
        //     text). It is NOT a structured artifact_ref node → dropped (0 raw-text re-triggers).
        let raw = InlineNode::Mention(human_principal("user-bob")); // a non-ref node stands for raw text.
        let v = guards.admit_dispatch(&Actor(human_principal("user-bob")), &raw, root, 0);
        assert_eq!(
            v,
            GuardVerdict::Drop(GuardRefusal::RawTextNotAReference),
            "raw typed text can NEVER re-trigger (reference gate)"
        );
        raw_text_drops += 1;

        // (c) guards 3+4 — a LEGITIMATE-looking re-trigger (another actor, a STRUCTURED ref) that
        //     self-feeds the causal chain AND re-enters the same root. The depth ceiling + tripwire
        //     stop it; never forked.
        let other = Actor(human_principal("user-bob"));
        let v = guards.admit_dispatch(&other, &ref_node(), root, depth);
        match v {
            GuardVerdict::Admit => {
                child_admitted += 1;
                depth = depth.saturating_add(1); // self-feed: the child becomes the next parent.
            }
            GuardVerdict::Drop(GuardRefusal::DepthCeiling) => {
                depth_ceiling_drops += 1;
                depth = 0; // re-root at 0 so the SAME-root tripwire now takes over (the wide loop).
            }
            GuardVerdict::Drop(GuardRefusal::SharedRootTripwire) => tripwire_drops += 1,
            other => panic!("unexpected child verdict: {other:?}"),
        }

        // (d) guard 5 — the unbounded-fan-out attempt: the bounded dispatch pool sheds/parks over-cap.
        match guards.admit_dispatch_pool() {
            GuardVerdict::Admit => {} // never released → the pool fills + stays full.
            GuardVerdict::Park(GuardRefusal::DispatchPoolFull) => pool_parked += 1,
            other => panic!("an over-cap dispatch parks, never: {other:?}"),
        }
    }

    // (3) ASSERT the green artifact.
    // guards 1+2: every self-trigger and every raw-text re-trigger was dropped — 0 admitted.
    assert_eq!(self_trigger_drops, 200, "every self-trigger dropped (0 self re-triggers)");
    assert_eq!(raw_text_drops, 200, "every raw-text re-trigger dropped (0 raw-text re-triggers)");

    // guard 3: the causal-depth NEVER exceeded the ceiling — the self-feeding chain was stopped AT it.
    assert!(
        telemetry.causal_depth_max() <= ceiling,
        "causal-depth max {} must be <= ceiling {ceiling} (NEVER raised to pass)",
        telemetry.causal_depth_max()
    );
    assert_eq!(telemetry.causal_depth_max(), ceiling, "the chain reached but did not exceed 12");
    assert!(telemetry.depth_ceiling_hits() >= 1, "the depth ceiling stopped the deep chain");
    assert!(depth_ceiling_drops >= 1, "the depth ceiling fired in the loop");

    // guard 4: the shared-root tripwire tripped the per-tenant breaker on the wide same-root loop.
    assert!(telemetry.shared_root_tripwire_firings() >= 1, "the per-tenant breaker tripped");
    assert!(tripwire_drops >= 1, "the tripwire fired in the loop");

    // guard 5: the bounded dispatch pool capped concurrency at 4 and shed the rest.
    assert_eq!(guards.dispatches_in_flight(), 4, "the pool is at cap (4 in flight, never released)");
    assert!(pool_parked >= 1, "over-cap dispatches were shed/parked");
    assert_eq!(telemetry.activity_pool_sheds() as u32, pool_parked, "shed accounting");

    // THE HEADLINE: 0 fork — nothing was ever multiplied; the loop was stopped, not forked.
    assert_eq!(telemetry.fork_count(), 0, "0 FORK — halted/dropped/parked, never forked");
    assert!(child_admitted >= 1, "the loop both admitted (up to the ceiling) and refused");

    // (4) ASSERT via the M0 assertion library (typed green/red, never a swallowed pass).
    let mut signals = SignalSource::new();
    // the causal-depth signal stays UNDER the ceiling — the AG-D7 halt bound (max <= ceiling 12).
    signals.set_scalar(SignalName::CausalDepthFirings, telemetry.causal_depth_max() as i64);
    signals
        .assert_signal(SignalName::CausalDepthFirings, Predicate::Lte(ceiling as i64))
        .expect_green();
    // the 0-fork counter — the structural proof the gate never forks.
    signals.set_scalar(SignalName::ShedCount, telemetry.fork_count() as i64);
    signals
        .assert_signal(SignalName::ShedCount, Predicate::Eq(0))
        .expect_green();
    // the bounded dispatch-pool drops-over-cap leg fired (>= 1).
    signals.set_scalar(SignalName::DispatchPoolDrops, telemetry.activity_pool_sheds() as i64);
    signals
        .assert_signal(SignalName::DispatchPoolDrops, Predicate::Gte(1))
        .expect_green();

    breaker.restore_dependency(Dependency::Broker, scope);
    assert_eq!(breaker.broken_count(), 0, "no leaked break");

    println!(
        "[2026-06-21] PASS  drill=AG-D7  surface=loop-guards  \
         self_trigger_drops={self_trigger_drops}  raw_text_drops={raw_text_drops} (0 raw-text re-triggers)  \
         causal_depth_max={} (<= ceiling {ceiling})  fork_count=0  depth_ceiling_hits={}  \
         tripwire_firings={}  dispatch_pool_sheds={}  \
         (adversarial agent->agent self-trigger loop halted <= ceiling, never forked)",
        telemetry.causal_depth_max(),
        telemetry.depth_ceiling_hits(),
        telemetry.shared_root_tripwire_firings(),
        telemetry.activity_pool_sheds(),
    );
}

/// **AG-D7 (sub-assertion) — the reference gate is the structural reason a human/agent cannot typo into
/// a loop (§5.5, EI-02 §6).** A chained drill where EVERY hop is a raw-typed-text "re-trigger": NONE of
/// them admit (0 raw-text re-triggers), so the loop never even STARTS — it is stopped at the gate, not
/// at the ceiling.
#[test]
fn drill_ag_d7_raw_text_never_re_triggers_zero_admit() {
    let guards = AgentLoopGuards::with_caps(PrincipalId(AGENT.into()), 12, 64, 256);
    let other = Actor(human_principal("user-bob"));

    let mut admitted = 0u32;
    let mut raw_dropped = 0u32;
    // 1000 raw-text re-trigger attempts — a "typo storm". The reference gate drops every one.
    for i in 0..1000 {
        // raw text is modelled as a non-`ArtifactRefNode` node (a mention) AND via admit_raw_text.
        let raw_node = InlineNode::Mention(human_principal("user-bob"));
        let v = guards.admit_dispatch(&other, &raw_node, "corr", 0);
        match v {
            GuardVerdict::Admit => admitted += 1,
            GuardVerdict::Drop(GuardRefusal::RawTextNotAReference) => raw_dropped += 1,
            other => panic!("hop {i}: unexpected {other:?}"),
        }
        // the dedicated raw-text path (a plain string) is ALSO always dropped.
        assert_eq!(
            guards.reference_gate().admit_raw_text("@agent-alice please loop forever"),
            GuardVerdict::Drop(GuardRefusal::RawTextNotAReference),
        );
    }

    assert_eq!(admitted, 0, "0 raw-text re-triggers — a typo can NEVER start a loop");
    assert_eq!(raw_dropped, 1000, "every raw-text re-trigger was dropped at the gate");
}

/// **AG-D7 (sub-assertion) — the idempotent-tool ledger makes a re-delivered effect 0-mutation
/// (§5.5).** A loop that re-delivers the SAME `(run, effect_id)` N times applies it EXACTLY ONCE; the
/// apply count equals the number of DISTINCT effects, never the number of delivery attempts.
#[test]
fn drill_ag_d7_idempotent_tools_re_delivered_effect_applies_once() {
    use myelin_agent_service::IdempotentToolLedger;

    let mut ledger = IdempotentToolLedger::new();
    let mut applied_calls = 0u32; // how many times a "real apply" would have fired.

    // a loop re-delivers run-1's effect eff-1 fifty times (a retried dispatch / double-clicked resume).
    for _ in 0..50 {
        if ledger.record("run-1", "eff-1") {
            applied_calls += 1; // the FIRST one applies; the rest are no-ops.
        }
    }
    // plus two genuinely distinct effects, each re-delivered.
    for _ in 0..10 {
        if ledger.record("run-1", "eff-2") {
            applied_calls += 1;
        }
        if ledger.record("run-2", "eff-1") {
            applied_calls += 1;
        }
    }

    assert_eq!(applied_calls, 3, "exactly 3 real applies (the distinct effects), never 70");
    assert_eq!(ledger.applies(), 3, "the ledger records exactly 3 distinct (run, effect_id) keys");
    // a re-delivered effect double-mutates 0 times — the structural exactly-once.
}
