use myelin_flow::{CausalGuard, FlowTelemetry, LoopVerdict, RefusalReason};
use myelin_harness::{Dependency, DependencyBreaker, Predicate, Scope, SignalName, SignalSource};
use myelin_tenancy::TenantId;

fn tenant() -> TenantId {
    TenantId("acme".into())
}

#[test]
fn drill_flow_d7_adversarial_loop_stopped_under_ceiling_zero_fork() {
    let scope = Scope::Tenant(tenant());
    let breaker = DependencyBreaker::new();
    let telemetry = FlowTelemetry::new();

    let ceiling = 8u32;
    let guard = CausalGuard::with_caps(ceiling, 16, 4).with_telemetry(telemetry.clone());
    let root = "corr-adversarial";

    breaker.break_dependency(Dependency::Broker, scope.clone());
    assert!(
        breaker.is_broken(&Dependency::Broker, &scope),
        "the adversarial loop is injected"
    );

    let mut depth = 0u32;
    let mut child_admitted = 0u32;
    let mut child_dropped = 0u32;
    let mut activity_admitted = 0u32;
    let mut activity_parked = 0u32;
    let mut tripwire_drops = 0u32;

    for _ in 0..200 {
        let (verdict, reason) = guard.admit_child(root, depth);
        match verdict {
            LoopVerdict::Admit => {
                child_admitted += 1;
                depth = depth.saturating_add(1);
            }
            LoopVerdict::Drop => {
                child_dropped += 1;
                match reason {
                    Some(RefusalReason::DepthCeiling) => {
                        depth = 0;
                    }
                    Some(RefusalReason::SharedRootTripwire) => tripwire_drops += 1,
                    other => panic!("unexpected child drop reason: {other:?}"),
                }
            }
            LoopVerdict::Park => panic!("a child start drops, it never parks"),
        }

        match guard.admit_activity().0 {
            LoopVerdict::Admit => activity_admitted += 1,
            LoopVerdict::Park => activity_parked += 1,
            LoopVerdict::Drop => panic!("an over-cap activity parks, it never drops"),
        }
    }

    assert!(
        telemetry.causal_depth_max() <= ceiling,
        "causal-depth max {} must be <= ceiling {ceiling}",
        telemetry.causal_depth_max()
    );
    assert_eq!(
        telemetry.causal_depth_max(),
        ceiling,
        "the chain reached but did not exceed the ceiling"
    );
    assert!(
        telemetry.depth_ceiling_hits() >= 1,
        "the depth ceiling stopped the deep chain"
    );
    assert!(
        telemetry.shared_root_tripwire_firings() >= 1,
        "the tripwire stopped the same-root loop"
    );
    assert_eq!(
        tripwire_drops as u64,
        telemetry.shared_root_tripwire_firings(),
        "tripwire-drop accounting"
    );
    assert!(
        activity_admitted <= 4,
        "the pool admitted at most its cap of 4"
    );
    assert_eq!(
        guard.activities_in_flight(),
        4,
        "the pool is at cap (4 in flight, never released)"
    );
    assert!(activity_parked >= 1, "over-cap activities were shed/parked");
    assert_eq!(
        telemetry.activity_pool_sheds() as u32,
        activity_parked,
        "shed accounting"
    );
    assert_eq!(
        telemetry.fork_count(),
        0,
        "0 FORK - the adversarial loop was dropped/parked, never forked"
    );

    assert!(
        child_admitted >= 1 && child_dropped >= 1,
        "the loop both admitted and refused"
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

#[test]
fn drill_flow_d7_bounded_pool_drains_on_release() {
    let telemetry = FlowTelemetry::new();
    let guard = CausalGuard::with_caps(8, 16, 3).with_telemetry(telemetry.clone());

    for _ in 0..3 {
        assert_eq!(guard.admit_activity().0, LoopVerdict::Admit);
    }
    assert_eq!(
        guard.admit_activity().0,
        LoopVerdict::Park,
        "over-cap is parked"
    );
    assert_eq!(guard.activities_in_flight(), 3, "the pool is at cap");

    guard.release_activity();
    assert_eq!(
        guard.admit_activity().0,
        LoopVerdict::Admit,
        "a freed slot admits the held activity"
    );
    assert_eq!(
        guard.activities_in_flight(),
        3,
        "back at cap - bounded, never over"
    );
    assert_eq!(telemetry.fork_count(), 0, "still 0 fork");
}
