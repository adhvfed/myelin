//! # CDC 1.11 (agent slice) — agent-load caps + the causal-loop guard (P-S20 → P-036)
//!
//! **Contract-index:** row 1.11 (the **agent slice** of the protected-human-lane / loop-guard
//! machinery — AG-6: the bounded dispatch pool drops over-cap (never forks), the causal-depth ceiling,
//! the shared-causal-root tripwire, the bounded predicate-evaluation guard). Consumes row 1.8 (the
//! `causal_depth` histogram + `tripwire_fired` + `dispatch_pool_drops` producer signals).
//!
//! This consumer-driven contract test exercises the 1.11 agent-load shape from OUTSIDE the crate (the
//! Bus reactive/dispatch tier, EB-23/P-143, is the consumer of the [`AgentLoadGuard`]): it builds the
//! envelopes the dispatch tier would deliver and asserts each cap's frozen behaviour + the combined
//! consult. The provider side is [`myelin_substrate::agent_load`]; the unit half is `agent_load::tests`;
//! the SUB-D8 adversarial-loop drill is `tests/drill_sub_d8_causal_loop.rs`. Together they are the
//! dated green artifact.

use myelin_events::{
    derive_envelope, Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EmitContext,
    EventDraft, EventEnvelope, EventId, EventType, Region, TenantId, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_substrate::{
    AgentLoadGuard, BudgetBreach, DepthCeiling, DepthVerdict, DispatchAdmission, DispatchPool,
    GuardOutcome, PredicateGuard, PredicateVerdict, SharedRootTripwire, TripwireVerdict,
};

/// Build a reaction envelope at a chosen causal depth + root — the shape a dispatched reaction
/// carries. Built through `derive_envelope` (the same path `OutboxTx::emit` uses) then the loop-hop
/// `depth`/`correlation_id` are set directly to drive the boundary; in production the outbox stamps
/// them correct-by-construction (§5.3).
fn reaction(depth: u32, root: &str) -> EventEnvelope {
    let draft = EventDraft {
        type_: EventType("agent.run.reacted".into()),
        subject: ArtifactRef(format!("myelin://acme/agent/run/{depth}-{root}")),
        aggregate: AggregateKey(format!("run-{depth}-{root}")),
        payload: serde_json::json!({ "hop": depth }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    };
    let ctx = EmitContext {
        event_id: EventId(format!("evt-{depth}-{root}")),
        tenant: TenantId("acme".into()),
        region: Region("eu-west".into()),
        actor: Actor(Principal::stub(
            PrincipalId("agent".into()),
            PrincipalKind::Service,
            TenantId("acme".into()),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-19T00:00:01Z".into()),
        caused_by: None,
    };
    let mut env = derive_envelope(draft, ctx, None);
    env.depth = depth;
    env.correlation_id = CorrelationId(root.into());
    env
}

/// **CDC 1.11 agent (a) — the bounded dispatch pool drops over-cap, never forks.** The consumer (the
/// dispatch tier) gets a typed `Dropped` it counts, never an unbounded fork into a new worker.
#[test]
fn cdc_1_11_agent_dispatch_pool_drops_over_cap_never_forks() {
    let mut pool = DispatchPool::new(2);
    assert_eq!(pool.try_dispatch(), DispatchAdmission::Admitted);
    assert_eq!(pool.try_dispatch(), DispatchAdmission::Admitted);
    assert_eq!(
        pool.try_dispatch(),
        DispatchAdmission::Dropped,
        "over-cap is DROPPED (never forked) — the §7.4 structural concurrency cap"
    );
    assert_eq!(pool.in_flight(), 2, "in-flight never exceeds the bound");
    assert_eq!(pool.dispatch_pool_drops(), 1, "the drop is exported (contract-1.8)");
}

/// **CDC 1.11 agent (b) — the causal-depth ceiling reads `EventEnvelope.depth` and halts at the hard
/// ceiling (the §5.3 structural guarantee — it reads the envelope field, not a convention).**
#[test]
fn cdc_1_11_agent_depth_ceiling_reads_envelope_depth_and_halts() {
    let mut ceiling = DepthCeiling::v1_floor();
    assert_eq!((ceiling.soft(), ceiling.hard()), (12, 16), "the v1 floor (P-038)");
    assert_eq!(ceiling.evaluate(&reaction(5, "r")), DepthVerdict::Admit);
    assert_eq!(ceiling.evaluate(&reaction(12, "r")), DepthVerdict::AdmitFlagged);
    assert_eq!(
        ceiling.evaluate(&reaction(16, "r")),
        DepthVerdict::Halt,
        "a reaction at the hard depth is halted — the loop is stopped"
    );
    assert_eq!(ceiling.halts(), 1);
}

/// **CDC 1.11 agent (c) — the shared-causal-root tripwire reads `EventEnvelope.correlation_id` and
/// fires on a wide fan-out off one root (the guard a per-chain depth ceiling would miss).**
#[test]
fn cdc_1_11_agent_shared_root_tripwire_reads_correlation_id_and_fires() {
    let mut tw = SharedRootTripwire::new(8, 4);
    let root = CorrelationId("hot".into());
    for _ in 0..3 {
        assert_eq!(tw.record(&reaction(2, "hot")), TripwireVerdict::Admit);
    }
    assert_eq!(
        tw.record(&reaction(2, "hot")),
        TripwireVerdict::Fired,
        "too many reactions off ONE root within the window → fire + quarantine"
    );
    assert!(tw.is_quarantined(&root));
    assert_eq!(tw.tripwire_fired(), 1, "tripwire_fired is exported (contract-1.8)");
    // diverse roots are normal traffic — never fire.
    let mut tw2 = SharedRootTripwire::new(8, 4);
    for i in 0..8 {
        assert_eq!(tw2.record(&reaction(1, &format!("r{i}"))), TripwireVerdict::Admit);
    }
    assert_eq!(tw2.tripwire_fired(), 0);
}

/// **CDC 1.11 agent (d) — the bounded predicate-evaluation guard rejects a crafted over-cost matcher
/// before it can DoS the trigger engine (§7.5).**
#[test]
fn cdc_1_11_agent_predicate_guard_rejects_over_cost_matcher() {
    let mut guard = PredicateGuard::v1_floor();
    assert_eq!((guard.max_steps(), guard.max_eval_micros()), (256, 2_000), "the v1 floor");
    assert_eq!(guard.admit_static(20), PredicateVerdict::WithinBudget);
    assert_eq!(
        guard.admit_static(10_000),
        PredicateVerdict::OverBudget(BudgetBreach::Steps),
        "a crafted matcher is rejected BEFORE evaluation"
    );
    // the runtime backstop catches a predicate that passed the static check but ran long.
    assert_eq!(
        guard.check_runtime(9_999),
        PredicateVerdict::OverBudget(BudgetBreach::Time)
    );
    assert_eq!(guard.rejections(), 2);
}

/// **CDC 1.11 agent — the composed `AgentLoadGuard`: the four caps are complementary, so an
/// adversarial loop is stopped by whichever cap trips first, and a halt takes no permit.** This is the
/// single consult the dispatch tier makes per reaction.
#[test]
fn cdc_1_11_agent_composed_guard_stops_the_loop_whichever_way_it_evades() {
    // a deep chain → halted by depth.
    let mut g = AgentLoadGuard::v1_floor(64);
    assert_eq!(g.admit(&reaction(16, "deep")), GuardOutcome::HaltedByDepth);
    assert_eq!(g.pool.in_flight(), 0, "a halted reaction leaks no permit");
    assert_eq!(g.signals().causal_depth_firings(), 1);

    // a wide fan-out → halted by the tripwire.
    let mut g2 = AgentLoadGuard {
        pool: DispatchPool::new(1000),
        depth: DepthCeiling::new(12, 16),
        tripwire: SharedRootTripwire::new(8, 4),
        predicate: PredicateGuard::v1_floor(),
    };
    for _ in 0..3 {
        assert_eq!(g2.admit(&reaction(2, "fan")), GuardOutcome::Dispatch);
    }
    assert_eq!(g2.admit(&reaction(2, "fan")), GuardOutcome::HaltedByTripwire);
    assert_eq!(g2.signals().tripwire_fired, 1);

    // a concurrency surge → dropped by the pool (never forked).
    let mut g3 = AgentLoadGuard {
        pool: DispatchPool::new(2),
        depth: DepthCeiling::new(12, 16),
        tripwire: SharedRootTripwire::new(64, 16),
        predicate: PredicateGuard::v1_floor(),
    };
    assert_eq!(g3.admit(&reaction(1, "a")), GuardOutcome::Dispatch);
    assert_eq!(g3.admit(&reaction(1, "b")), GuardOutcome::Dispatch);
    assert_eq!(g3.admit(&reaction(1, "c")), GuardOutcome::HaltedByPool);
    assert_eq!(g3.signals().dispatch_pool_drops, 1);
}
