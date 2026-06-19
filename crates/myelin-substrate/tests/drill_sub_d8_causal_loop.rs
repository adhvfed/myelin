//! # SUB-D8 — the adversarial causal-loop drill (P-S20 → global P-036)
//!
//! **Drill catalogue:** `planning/05-refined-shared-systems-architecture/testing-strategy/
//! 01-whole-system-e2e-and-drill-catalogue.md` §4.2 row **SUB-D8**: *"Adversarial agent→agent loop →
//! depth ceiling + tripwire + bounded pool halt it."* Green artifact: `causal-depth histogram;
//! tripwire`. Surface: CI.
//!
//! This is the **dated green artifact** the P-S20 GATE/DRILLS names. It is the EI-01 §3 drill shape:
//! *inject a fault (P-S03 `break_dependency`), drive one unit of load (the adversarial loop), read one
//! telemetry assertion that reads green (P-S04).* Here:
//!   - **inject** — `break_dependency(Named("dispatch-tier"), …)` models the reactive/dispatch tier
//!     under stress (the realistic condition a runaway loop is most dangerous under); the structural
//!     caps must hold REGARDLESS of a healthy downstream.
//!   - **load** — an adversarial agent→agent loop: a DEEP chain (each hop a reaction caused by the
//!     prior, climbing `EventEnvelope.depth`), a WIDE fan-out (many reactions sharing ONE causal
//!     `correlation_id` root within the window), and a CONCURRENCY surge (more in-flight reactions
//!     than the dispatch pool's capacity).
//!   - **assert** — the three contract-1.8 survival signals read green: the `causal_depth` histogram
//!     is **bounded** (the chain never climbs past the hard ceiling), `tripwire_fired >= 1` (the wide
//!     fan-out tripped the shared-root tripwire), and `dispatch_pool_drops >= 1` (the surge was
//!     dropped, never forked). The loop is structurally halted.
//!
//! The full agent-loop proof re-runs in M2 through the real reactive/dispatch tier (AG-P12 / P-224,
//! AG-D7; P-FLOW-18 / P-214); this drill proves the *substrate* machinery against raw envelopes.

use myelin_events::{
    derive_envelope, Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EmitContext,
    EventDraft, EventEnvelope, EventId, EventType, Region, TenantId, Timestamp, Visibility,
};
use myelin_harness::{
    Dependency, DrillContext, DrillRegistry, DrillResult, DrillScenario, Predicate, SignalName,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_substrate::{AgentLoadGuard, DepthCeiling, DispatchPool, GuardOutcome, SharedRootTripwire};

/// Build a reaction envelope at a chosen causal depth + root (the same path `OutboxTx::emit` stamps).
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

/// The SUB-D8 drill scenario: under an injected dispatch-tier hiccup, drive an adversarial
/// agent→agent loop (deep chain + wide fan-out + concurrency surge) through the [`AgentLoadGuard`] and
/// assert the three loop-guard survival signals read green.
fn sub_d8_causal_loop_scenario() -> DrillScenario {
    DrillScenario::new("sub-d8-adversarial-causal-loop", |ctx: &mut DrillContext| {
        // (inject) the reactive/dispatch tier is under stress — the structural caps must hold anyway.
        ctx.breaker
            .break_dependency(Dependency::Named("dispatch-tier".into()), myelin_harness::Scope::Global);

        // The guard the dispatch tier consults per reaction. A SMALL pool (4) so the concurrency surge
        // trips the pool; the depth ceiling + tripwire at small bounds so the loop trips them quickly.
        let mut guard = AgentLoadGuard {
            pool: DispatchPool::new(4),
            depth: DepthCeiling::new(12, 16),
            tripwire: SharedRootTripwire::new(32, 8),
            predicate: myelin_substrate::PredicateGuard::v1_floor(),
        };

        // ---- (load 1) a DEEP chain: each hop climbs depth, sharing one chain-root. The depth ceiling
        // halts it at the hard depth (16); the histogram is bounded. -----------------------------------
        let mut depth_ceiling = DepthCeiling::new(12, 16);
        let mut halted_by_depth = 0u64;
        for hop in 0..40u32 {
            if depth_ceiling.evaluate(&reaction(hop, "chain-root")).is_halted() {
                halted_by_depth += 1;
            }
        }
        let max_depth = depth_ceiling.max_observed_depth();

        // ---- (load 2) a WIDE fan-out: 20 reactions off ONE root within the window → the shared-root
        // tripwire fires. (Each is shallow, so a depth ceiling alone would miss it.) ------------------
        let mut tripwire_fired = 0u64;
        for _ in 0..20 {
            if guard.tripwire.record(&reaction(2, "fanout-root")).is_fired() {
                tripwire_fired += 1;
            }
        }

        // ---- (load 3) a CONCURRENCY surge: 12 distinct-root shallow reactions at a pool of 4 → 8 are
        // dropped (never forked). Use the pool directly (distinct roots so only the pool can trip). ----
        let mut pool_drops = 0u64;
        for i in 0..12 {
            if let GuardOutcome::HaltedByPool = guard_admit_distinct(&mut guard, i) {
                pool_drops += 1;
            }
        }

        // Record the three contract-1.8 survival signals (the producer side wires this off the real
        // dispatch-tier meter at EB-23/P-143; here the drill records what the guards observed).
        ctx.signals
            .set_scalar(SignalName::CausalDepthFirings, (halted_by_depth + tripwire_fired) as i64);
        ctx.signals.set_scalar(SignalName::DispatchPoolDrops, pool_drops as i64);

        // restore the injected fault before returning (a re-run starts clean).
        ctx.breaker
            .restore_dependency(Dependency::Named("dispatch-tier".into()), myelin_harness::Scope::Global);

        // (assert) the loop was HALTED by the guards:
        //   - the depth histogram is bounded (never climbed past the bucket ceiling — no runaway).
        assert!(
            max_depth < DepthCeiling::HIST_BUCKETS as u32,
            "the causal-depth histogram must be BOUNDED (no unbounded climb): max={max_depth}"
        );
        assert!(halted_by_depth >= 1, "the deep chain must be halted by the depth ceiling");
        assert!(tripwire_fired >= 1, "the wide fan-out must fire the shared-root tripwire");
        assert!(pool_drops >= 1, "the concurrency surge must be dropped (never forked)");

        // The single telemetry assertion that reads green: the loop-guard FIRED
        // (`causal_depth_firings >= 1`) — the SUB-D8 survival signal. (The dispatch-pool-drops
        // assertion below is the bounded-pool half; both must be green.)
        ctx.signals
            .assert_signal(SignalName::CausalDepthFirings, Predicate::Gte(1))
    })
}

/// Helper: admit a reaction at a DISTINCT root so the only cap that can trip is the dispatch pool
/// (distinct roots never share enough to fire the tripwire; shallow depth never reaches the ceiling).
fn guard_admit_distinct(guard: &mut AgentLoadGuard, i: usize) -> GuardOutcome {
    guard.admit(&reaction(1, &format!("surge-{i}")))
}

/// **THE SUB-D8 DRILL** — the dated green artifact. Register it (it joins the permanent every-incident
/// suite) AND run it; assert the loop-guard survival signals read green and the pool-drops signal is
/// non-zero (the bounded pool dropped, never forked).
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

    // The dated green-artifact row (the prompt's named DEFINITION-OF-DONE artifact).
    let row = result.artifact_row("2026-06-19");
    assert_eq!(
        row,
        "[2026-06-19] PASS  drill=sub-d8-adversarial-causal-loop  (inject → load → assert green)"
    );
    println!("{row}");
}

/// The drill, run directly, asserting BOTH survival signals (the loop-guard fired AND the bounded pool
/// dropped over-cap) — the full SUB-D8 green pair, not just the first assertion.
#[test]
fn sub_d8_both_survival_signals_read_green() {
    let scenario = sub_d8_causal_loop_scenario();
    let result = scenario.run_once();
    match result {
        DrillResult::Pass { .. } => {
            // re-derive the signals to assert the bounded-pool half explicitly (the scenario's own
            // assertion is the causal-depth-firings half).
            let mut ctx = DrillContext::new();
            // drive the surge directly to read dispatch_pool_drops.
            let mut guard = AgentLoadGuard {
                pool: DispatchPool::new(4),
                depth: DepthCeiling::new(12, 16),
                tripwire: SharedRootTripwire::new(32, 8),
                predicate: myelin_substrate::PredicateGuard::v1_floor(),
            };
            for i in 0..12 {
                guard_admit_distinct(&mut guard, i);
            }
            ctx.signals
                .set_scalar(SignalName::DispatchPoolDrops, guard.signals().dispatch_pool_drops as i64);
            ctx.signals
                .assert_signal(SignalName::DispatchPoolDrops, Predicate::Gte(1))
                .expect_green();
        }
        DrillResult::Fail { verdict, .. } => panic!("SUB-D8 must pass, got {verdict:?}"),
    }
}
