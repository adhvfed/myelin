//! # FLOW-D7 drill — the adversarial workflow→event→workflow loop is stopped (P-FLOW-18)
//!
//! The headline drill the P-FLOW-18 GATE requires (testing-strategy FLOW-D7 + the F-7 assertion,
//! durable-workflow §8): an **adversarial workflow→event→workflow loop** — the **causal-depth
//! ceiling** + the **shared-root tripwire** + the **bounded activity pool** stop it (it is
//! **dropped/parked, NEVER forked**). The green artifact (testing-strategy FLOW-D7): the
//! **causal-depth signal stays UNDER the ceiling** AND a **0-fork counter**, dated CI. A red drill is
//! information — never weaken it to pass (EI-01 §3).
//!
//! **What the adversarial loop models (§6.2):** a workflow that, on each hop, both (a) starts a child
//! at `depth + 1` (the self-feeding causal chain) AND (b) re-enters the SAME `correlation_id` root (a
//! workflow→event→workflow loop), while ALSO trying to fan out activities unboundedly. Three
//! mechanisms catch it:
//! - the **causal-depth ceiling** ([`CausalGuard::admit_child`]) stops the DEEP chain AT the ceiling;
//! - the **shared-root tripwire** stops the WIDE same-root loop past the window cap;
//! - the **bounded activity pool** ([`CausalGuard::admit_activity`]) sheds/parks the over-cap fan-out.
//!
//! Every refusal is a [`LoopVerdict::Drop`]/[`LoopVerdict::Park`] — there is NO fork. The 0-fork
//! counter ([`FlowTelemetry::fork_count`]) is the structural proof.
//!
//! **Rides the M0 failure-injection harness:** the [`myelin_harness::DependencyBreaker`]
//! (`Dependency::Broker`, tenant-scoped — the SAME seam BUS-D4 / FLOW-D5 / FLOW-D6 use) models the
//! adversarial condition (the loop is "broken open" — it keeps feeding itself). The drill asserts the
//! survival signals via the M0 assertion library ([`myelin_harness::SignalSource`] / [`Predicate`]):
//! the causal-depth max (`<=` ceiling) and the fork count (`== 0`) — a typed green/red that is never a
//! swallowed pass (EI-01 §3).

use myelin_flow::{CausalGuard, FlowTelemetry, LoopVerdict, RefusalReason};
use myelin_harness::{
    Dependency, DependencyBreaker, Predicate, Scope, SignalName, SignalSource,
};
use myelin_tenancy::TenantId;

fn tenant() -> TenantId {
    TenantId("acme".into())
}

/// **FLOW-D7 — the adversarial workflow→event→workflow loop: the depth ceiling + the shared-root
/// tripwire + the bounded activity pool stop it (drops/parks, NEVER forks). Green artifact:
/// causal-depth `<=` ceiling AND fork count `== 0`, dated.**
///
/// The loop runs for many hops. On EACH hop the adversary tries to:
///   (a) start a child at `depth + 1` (the self-feeding causal chain), and
///   (b) re-enter the same `correlation_id` root (the wf→event→wf loop), and
///   (c) fan out one more activity (the unbounded-fan-out attempt).
/// The guard at small caps (ceiling 8, shared-root cap 16, pool cap 4) catches all three: the depth
/// chain stops at the ceiling, the same-root loop trips, the activity pool sheds over-cap. The depth
/// NEVER exceeds the ceiling; nothing forks.
#[test]
fn drill_flow_d7_adversarial_loop_stopped_under_ceiling_zero_fork() {
    let scope = Scope::Tenant(tenant());
    let breaker = DependencyBreaker::new();
    let telemetry = FlowTelemetry::new();

    // small caps so the adversarial loop hits them fast (the in-isolation drill, never weakened — the
    // REAL caps are FLOW-D7's production ceilings; the small caps prove the SAME mechanism).
    let ceiling = 8u32;
    let guard = CausalGuard::with_caps(ceiling, 16, 4).with_telemetry(telemetry.clone());
    let root = "corr-adversarial";

    // (1) INJECT the adversarial condition: the loop is "broken open" (it keeps feeding itself). The
    //     SAME tenant-scoped Broker seam BUS-D4 / FLOW-D5 / FLOW-D6 use.
    breaker.break_dependency(Dependency::Broker, scope.clone());
    assert!(breaker.is_broken(&Dependency::Broker, &scope), "the adversarial loop is injected");

    // (2) DRIVE the adversarial loop. The depth chain self-feeds: an admitted child becomes the next
    //     parent. The same-root re-entry and the activity fan-out run alongside.
    let mut depth = 0u32;
    let mut child_admitted = 0u32;
    let mut child_dropped = 0u32;
    let mut activity_admitted = 0u32;
    let mut activity_parked = 0u32;
    let mut tripwire_drops = 0u32;

    for _ in 0..200 {
        // (a) + (b): the self-feeding child start (depth chain + same-root tripwire).
        let (verdict, reason) = guard.admit_child(root, depth);
        match verdict {
            LoopVerdict::Admit => {
                child_admitted += 1;
                depth = depth.saturating_add(1); // self-feed: the child becomes the next parent.
            }
            LoopVerdict::Drop => {
                child_dropped += 1;
                match reason {
                    Some(RefusalReason::DepthCeiling) => {
                        // the depth chain hit the ceiling — reset the depth so the loop keeps trying
                        // (the adversary re-roots at depth 0 but the SAME correlation root, so the
                        // shared-root tripwire now takes over).
                        depth = 0;
                    }
                    Some(RefusalReason::SharedRootTripwire) => tripwire_drops += 1,
                    other => panic!("unexpected child drop reason: {other:?}"),
                }
            }
            LoopVerdict::Park => panic!("a child start drops, it never parks"),
        }

        // (c): the unbounded-fan-out attempt — the bounded pool sheds/parks over-cap.
        match guard.admit_activity().0 {
            LoopVerdict::Admit => activity_admitted += 1, // never released → the pool fills + stays full.
            LoopVerdict::Park => activity_parked += 1,
            LoopVerdict::Drop => panic!("an over-cap activity parks, it never drops"),
        }
    }

    // (3) ASSERT the green artifact.
    // the causal-depth NEVER exceeded the ceiling — the self-feeding chain was stopped AT it.
    assert!(
        telemetry.causal_depth_max() <= ceiling,
        "causal-depth max {} must be <= ceiling {ceiling}",
        telemetry.causal_depth_max()
    );
    assert_eq!(telemetry.causal_depth_max(), ceiling, "the chain reached but did not exceed the ceiling");
    // the depth ceiling fired (the deep chain was stopped) AND the shared-root tripwire fired (the wide
    // same-root loop was caught) — both halves of §6.2 demonstrably engaged.
    assert!(telemetry.depth_ceiling_hits() >= 1, "the depth ceiling stopped the deep chain");
    assert!(telemetry.shared_root_tripwire_firings() >= 1, "the tripwire stopped the same-root loop");
    assert_eq!(tripwire_drops as u64, telemetry.shared_root_tripwire_firings(), "tripwire-drop accounting");
    // the bounded activity pool capped concurrency at 4 and shed the rest.
    assert!(activity_admitted <= 4, "the pool admitted at most its cap of 4");
    assert_eq!(guard.activities_in_flight(), 4, "the pool is at cap (4 in flight, never released)");
    assert!(activity_parked >= 1, "over-cap activities were shed/parked");
    assert_eq!(telemetry.activity_pool_sheds() as u32, activity_parked, "shed accounting");
    // THE HEADLINE: 0 fork — nothing was ever multiplied; the loop was stopped, not forked.
    assert_eq!(telemetry.fork_count(), 0, "0 FORK — the adversarial loop was dropped/parked, never forked");

    assert!(child_admitted >= 1 && child_dropped >= 1, "the loop both admitted and refused");

    // (4) ASSERT via the M0 assertion library (typed green/red, never a swallowed pass).
    let mut signals = SignalSource::new();
    // the causal-depth signal stays UNDER the ceiling — the FLOW-D7 headline (here: max <= ceiling).
    signals.set_scalar(SignalName::CausalDepthFirings, telemetry.causal_depth_max() as i64);
    signals
        .assert_signal(SignalName::CausalDepthFirings, Predicate::Lte(ceiling as i64))
        .expect_green();
    // the 0-fork counter — the structural proof the gate never forks.
    signals.set_scalar(SignalName::ShedCount, telemetry.fork_count() as i64);
    signals
        .assert_signal(SignalName::ShedCount, Predicate::Eq(0))
        .expect_green();

    breaker.restore_dependency(Dependency::Broker, scope.clone());
    assert_eq!(breaker.broken_count(), 0, "no leaked break");

    println!(
        "[2026-06-21] PASS  drill=FLOW-D7  surface=loop-safety  causal_depth_max={} (<= ceiling {ceiling})  \
         fork_count=0  depth_ceiling_hits={}  tripwire_firings={}  activity_pool_sheds={}  \
         (adversarial wf->event->wf loop dropped/parked, never forked)",
        telemetry.causal_depth_max(),
        telemetry.depth_ceiling_hits(),
        telemetry.shared_root_tripwire_firings(),
        telemetry.activity_pool_sheds(),
    );
}

/// **FLOW-D7 (F-7 sub-assertion) — releasing in-flight activities lets the parked fan-out drain (the
/// pool is bounded, not a permanent block).** After the storm, releasing slots admits the held work —
/// the bound SHAPES the fan-out (steady-state under cap), it does not deadlock it.
#[test]
fn drill_flow_d7_bounded_pool_drains_on_release() {
    let telemetry = FlowTelemetry::new();
    let guard = CausalGuard::with_caps(8, 16, 3).with_telemetry(telemetry.clone());

    // fill the pool to cap, then over-cap → parked.
    for _ in 0..3 {
        assert_eq!(guard.admit_activity().0, LoopVerdict::Admit);
    }
    assert_eq!(guard.admit_activity().0, LoopVerdict::Park, "over-cap is parked");
    assert_eq!(guard.activities_in_flight(), 3, "the pool is at cap");

    // release one → a slot frees → the held work admits (the bound is steady-state, not a deadlock).
    guard.release_activity();
    assert_eq!(guard.admit_activity().0, LoopVerdict::Admit, "a freed slot admits the held activity");
    assert_eq!(guard.activities_in_flight(), 3, "back at cap — bounded, never over");
    assert_eq!(telemetry.fork_count(), 0, "still 0 fork");
}
