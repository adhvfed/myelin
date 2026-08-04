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

fn ref_node() -> InlineNode {
    InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()))
}

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

#[test]
fn drill_ag_d7_adversarial_self_trigger_loop_halts_under_ceiling_zero_fork() {
    let scope = Scope::Tenant(tenant());
    let breaker = DependencyBreaker::new();
    let telemetry = FlowTelemetry::new();

    let ceiling = 12u32;
    let guards = AgentLoopGuards::with_caps(PrincipalId(AGENT.into()), ceiling, 16, 4)
        .with_telemetry(telemetry.clone());
    let root = "corr-adversarial";

    breaker.break_dependency(Dependency::Broker, scope.clone());
    assert!(
        breaker.is_broken(&Dependency::Broker, &scope),
        "the adversarial self-trigger loop is injected"
    );

    let mut self_trigger_drops = 0u32;
    let mut raw_text_drops = 0u32;
    let mut depth = 0u32;
    let mut child_admitted = 0u32;
    let mut depth_ceiling_drops = 0u32;
    let mut tripwire_drops = 0u32;
    let mut pool_parked = 0u32;

    for _ in 0..200 {
        let own = inbound(agent_principal(AGENT), root, 0);
        let v = guards.admit_dispatch(&own.actor, &ref_node(), root, own.depth);
        assert_eq!(
            v,
            GuardVerdict::Drop(GuardRefusal::SelfTrigger),
            "an agent's OWN emission can never re-trigger it (self-guard)"
        );
        self_trigger_drops += 1;

        let raw = InlineNode::Mention(human_principal("user-bob"));
        let v = guards.admit_dispatch(&Actor(human_principal("user-bob")), &raw, root, 0);
        assert_eq!(
            v,
            GuardVerdict::Drop(GuardRefusal::RawTextNotAReference),
            "raw typed text can NEVER re-trigger (reference gate)"
        );
        raw_text_drops += 1;

        let other = Actor(human_principal("user-bob"));
        let v = guards.admit_dispatch(&other, &ref_node(), root, depth);
        match v {
            GuardVerdict::Admit => {
                child_admitted += 1;
                depth = depth.saturating_add(1);
            }
            GuardVerdict::Drop(GuardRefusal::DepthCeiling) => {
                depth_ceiling_drops += 1;
                depth = 0;
            }
            GuardVerdict::Drop(GuardRefusal::SharedRootTripwire) => tripwire_drops += 1,
            other => panic!("unexpected child verdict: {other:?}"),
        }

        match guards.admit_dispatch_pool() {
            GuardVerdict::Admit => {}
            GuardVerdict::Park(GuardRefusal::DispatchPoolFull) => pool_parked += 1,
            other => panic!("an over-cap dispatch parks, never: {other:?}"),
        }
    }

    assert_eq!(
        self_trigger_drops, 200,
        "every self-trigger dropped (0 self re-triggers)"
    );
    assert_eq!(
        raw_text_drops, 200,
        "every raw-text re-trigger dropped (0 raw-text re-triggers)"
    );

    assert!(
        telemetry.causal_depth_max() <= ceiling,
        "causal-depth max {} must be <= ceiling {ceiling} (NEVER raised to pass)",
        telemetry.causal_depth_max()
    );
    assert_eq!(
        telemetry.causal_depth_max(),
        ceiling,
        "the chain reached but did not exceed 12"
    );
    assert!(
        telemetry.depth_ceiling_hits() >= 1,
        "the depth ceiling stopped the deep chain"
    );
    assert!(
        depth_ceiling_drops >= 1,
        "the depth ceiling fired in the loop"
    );

    assert!(
        telemetry.shared_root_tripwire_firings() >= 1,
        "the per-tenant breaker tripped"
    );
    assert!(tripwire_drops >= 1, "the tripwire fired in the loop");

    assert_eq!(
        guards.dispatches_in_flight(),
        4,
        "the pool is at cap (4 in flight, never released)"
    );
    assert!(pool_parked >= 1, "over-cap dispatches were shed/parked");
    assert_eq!(
        telemetry.activity_pool_sheds() as u32,
        pool_parked,
        "shed accounting"
    );

    assert_eq!(
        telemetry.fork_count(),
        0,
        "0 FORK - halted/dropped/parked, never forked"
    );
    assert!(
        child_admitted >= 1,
        "the loop both admitted (up to the ceiling) and refused"
    );

    let mut signals = SignalSource::new();
    signals.set_scalar(
        SignalName::CausalDepthFirings,
        telemetry.causal_depth_max() as i64,
    );
    signals
        .assert_signal(
            SignalName::CausalDepthFirings,
            Predicate::Lte(ceiling as i64),
        )
        .expect_green();
    signals.set_scalar(SignalName::ShedCount, telemetry.fork_count() as i64);
    signals
        .assert_signal(SignalName::ShedCount, Predicate::Eq(0))
        .expect_green();
    signals.set_scalar(
        SignalName::DispatchPoolDrops,
        telemetry.activity_pool_sheds() as i64,
    );
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

#[test]
fn drill_ag_d7_raw_text_never_re_triggers_zero_admit() {
    let guards = AgentLoopGuards::with_caps(PrincipalId(AGENT.into()), 12, 64, 256);
    let other = Actor(human_principal("user-bob"));

    let mut admitted = 0u32;
    let mut raw_dropped = 0u32;
    for i in 0..1000 {
        let raw_node = InlineNode::Mention(human_principal("user-bob"));
        let v = guards.admit_dispatch(&other, &raw_node, "corr", 0);
        match v {
            GuardVerdict::Admit => admitted += 1,
            GuardVerdict::Drop(GuardRefusal::RawTextNotAReference) => raw_dropped += 1,
            other => panic!("hop {i}: unexpected {other:?}"),
        }
        assert_eq!(
            guards
                .reference_gate()
                .admit_raw_text("@agent-alice please loop forever"),
            GuardVerdict::Drop(GuardRefusal::RawTextNotAReference),
        );
    }

    assert_eq!(
        admitted, 0,
        "0 raw-text re-triggers - a typo can NEVER start a loop"
    );
    assert_eq!(
        raw_dropped, 1000,
        "every raw-text re-trigger was dropped at the gate"
    );
}

#[test]
fn drill_ag_d7_idempotent_tools_re_delivered_effect_applies_once() {
    use myelin_agent_service::IdempotentToolLedger;

    let mut ledger = IdempotentToolLedger::new();
    let mut applied_calls = 0u32;

    for _ in 0..50 {
        if ledger.record("run-1", "eff-1") {
            applied_calls += 1;
        }
    }
    for _ in 0..10 {
        if ledger.record("run-1", "eff-2") {
            applied_calls += 1;
        }
        if ledger.record("run-2", "eff-1") {
            applied_calls += 1;
        }
    }

    assert_eq!(
        applied_calls, 3,
        "exactly 3 real applies (the distinct effects), never 70"
    );
    assert_eq!(
        ledger.applies(),
        3,
        "the ledger records exactly 3 distinct (run, effect_id) keys"
    );
}
