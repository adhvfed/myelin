use myelin_events::{
    derive_envelope, Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EmitContext,
    EventDraft, EventEnvelope, EventId, EventType, Region, TenantId, Timestamp, Visibility,
};
use myelin_harness::{
    Dependency, DrillContext, DrillRegistry, DrillResult, DrillScenario, Predicate, SignalName,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_substrate::{
    AgentLoadGuard, DepthCeiling, DispatchPool, GuardOutcome, SharedRootTripwire,
};

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

fn sub_d8_causal_loop_scenario() -> DrillScenario {
    DrillScenario::new(
        "sub-d8-adversarial-causal-loop",
        |ctx: &mut DrillContext| {
            ctx.breaker.break_dependency(
                Dependency::Named("dispatch-tier".into()),
                myelin_harness::Scope::Global,
            );

            let mut guard = AgentLoadGuard {
                pool: DispatchPool::new(4),
                depth: DepthCeiling::new(12, 16),
                tripwire: SharedRootTripwire::new(32, 8),
                predicate: myelin_substrate::PredicateGuard::v1_floor(),
            };

            let mut depth_ceiling = DepthCeiling::new(12, 16);
            let mut halted_by_depth = 0u64;
            for hop in 0..40u32 {
                if depth_ceiling
                    .evaluate(&reaction(hop, "chain-root"))
                    .is_halted()
                {
                    halted_by_depth += 1;
                }
            }
            let max_depth = depth_ceiling.max_observed_depth();

            let mut tripwire_fired = 0u64;
            for _ in 0..20 {
                if guard
                    .tripwire
                    .record(&reaction(2, "fanout-root"))
                    .is_fired()
                {
                    tripwire_fired += 1;
                }
            }

            let mut pool_drops = 0u64;
            for i in 0..12 {
                if let GuardOutcome::HaltedByPool = guard_admit_distinct(&mut guard, i) {
                    pool_drops += 1;
                }
            }

            ctx.signals.set_scalar(
                SignalName::CausalDepthFirings,
                (halted_by_depth + tripwire_fired) as i64,
            );
            ctx.signals
                .set_scalar(SignalName::DispatchPoolDrops, pool_drops as i64);

            ctx.breaker.restore_dependency(
                Dependency::Named("dispatch-tier".into()),
                myelin_harness::Scope::Global,
            );

            assert!(
                max_depth < DepthCeiling::HIST_BUCKETS as u32,
                "the causal-depth histogram must be BOUNDED (no unbounded climb): max={max_depth}"
            );
            assert!(
                halted_by_depth >= 1,
                "the deep chain must be halted by the depth ceiling"
            );
            assert!(
                tripwire_fired >= 1,
                "the wide fan-out must fire the shared-root tripwire"
            );
            assert!(
                pool_drops >= 1,
                "the concurrency surge must be dropped (never forked)"
            );

            ctx.signals
                .assert_signal(SignalName::CausalDepthFirings, Predicate::Gte(1))
        },
    )
}

fn guard_admit_distinct(guard: &mut AgentLoadGuard, i: usize) -> GuardOutcome {
    guard.admit(&reaction(1, &format!("surge-{i}")))
}

#[test]
fn sub_d8_adversarial_loop_is_halted_green_artifact() {
    let mut registry = DrillRegistry::new();
    registry.register_drill(sub_d8_causal_loop_scenario());

    let results = registry.run_all();
    assert_eq!(results.len(), 1);
    let result = &results[0];

    assert!(
        result.is_pass(),
        "SUB-D8: the adversarial causal loop must be HALTED by the depth ceiling + tripwire + bounded \
         pool (the loop-guard read green): {result:?}"
    );

    let row = result.artifact_row("2026-06-19");
    assert_eq!(
        row,
        "[2026-06-19] PASS  drill=sub-d8-adversarial-causal-loop  (inject → load → assert green)"
    );
    println!("{row}");
}

#[test]
fn sub_d8_both_survival_signals_read_green() {
    let scenario = sub_d8_causal_loop_scenario();
    let result = scenario.run_once();
    match result {
        DrillResult::Pass { .. } => {
            let mut ctx = DrillContext::new();
            let mut guard = AgentLoadGuard {
                pool: DispatchPool::new(4),
                depth: DepthCeiling::new(12, 16),
                tripwire: SharedRootTripwire::new(32, 8),
                predicate: myelin_substrate::PredicateGuard::v1_floor(),
            };
            for i in 0..12 {
                guard_admit_distinct(&mut guard, i);
            }
            ctx.signals.set_scalar(
                SignalName::DispatchPoolDrops,
                guard.signals().dispatch_pool_drops as i64,
            );
            ctx.signals
                .assert_signal(SignalName::DispatchPoolDrops, Predicate::Gte(1))
                .expect_green();
        }
        DrillResult::Fail { verdict, .. } => panic!("SUB-D8 must pass, got {verdict:?}"),
    }
}
