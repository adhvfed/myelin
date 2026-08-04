use myelin_harness::{
    Dependency, DependencyBreaker, DrillResult, DrillScenario, Predicate, Scope, SignalName,
    SignalSource,
};
use myelin_substrate::{
    CriticalDependencies, CriticalDependency, DependencyHealth, MetricsHealthSurface,
};

#[derive(Clone)]
struct InjectorHealth {
    breaker: DependencyBreaker,
}

impl DependencyHealth for InjectorHealth {
    fn is_up(&self, dep: &CriticalDependency) -> bool {
        let dependency = match dep.0.as_str() {
            "identity" => Dependency::Identity,
            other => Dependency::Named(other.to_string()),
        };
        !self.breaker.is_broken(&dependency, &Scope::Global)
    }
}

fn sub_d9_liveness_readiness_scenario() -> DrillScenario {
    DrillScenario::new("sub-d9-liveness-not-readiness", |ctx| {
        let health = InjectorHealth {
            breaker: ctx.breaker.clone(),
        };
        let mh = MetricsHealthSurface::new(CriticalDependencies::new(["identity"]), health);
        mh.mark_started();

        assert_eq!(
            mh.readiness().verdict.gauge(),
            1,
            "healthy baseline is ready"
        );
        assert!(!mh.liveness().should_restart(), "healthy baseline is live");

        ctx.breaker
            .break_dependency(Dependency::Identity, Scope::Global);

        let mut readiness_gauge_during_outage = 1i64;
        let mut shed_observed = false;
        for _ in 0..30 {
            let r = mh.readiness();
            readiness_gauge_during_outage = r.verdict.gauge();
            shed_observed |= r.sheds();
            if mh.liveness().should_restart() {
                mh.record_liveness_restart();
            }
        }
        assert_eq!(
            readiness_gauge_during_outage, 0,
            "readiness flipped to not-ready during the outage"
        );
        assert!(shed_observed, "the not-ready instance shed new traffic");
        let liveness_restarts = mh.liveness_restart_count() as i64;

        ctx.breaker
            .restore_dependency(Dependency::Identity, Scope::Global);
        assert_eq!(
            mh.readiness().verdict.gauge(),
            1,
            "readiness recovers when the dependency heals"
        );

        let mut src = SignalSource::new();
        src.set_scalar(SignalName::Readiness, readiness_gauge_during_outage);
        src.set_scalar(SignalName::LivenessRestartCount, liveness_restarts);

        src.assert_signal(SignalName::Readiness, Predicate::Eq(0))
            .expect_green();
        src.assert_signal(SignalName::LivenessRestartCount, Predicate::Eq(0))
    })
}

#[test]
fn sub_d9_liveness_readiness_drill_is_green() {
    let drill = sub_d9_liveness_readiness_scenario();
    let result = drill.run_once();
    assert!(
        result.is_pass(),
        "SUB-D9 must be green (readiness flips, no liveness churn): {}",
        result.artifact_row("2026-06-19")
    );
    let row = result.artifact_row("2026-06-19");
    assert!(
        row.contains("PASS"),
        "the artifact row records a PASS: {row}"
    );
    assert!(
        row.contains("sub-d9-liveness-not-readiness"),
        "names the drill: {row}"
    );
    println!("{row}");
}

#[test]
fn sub_d9_drill_reruns_green() {
    let drill = sub_d9_liveness_readiness_scenario();
    for _ in 0..3 {
        assert!(
            matches!(drill.run_once(), DrillResult::Pass { .. }),
            "SUB-D9 re-runs green"
        );
    }
}

#[test]
fn sub_d9_gate_is_not_vacuous_a_regression_reads_red() {
    let mut src = SignalSource::new();
    src.set_scalar(SignalName::Readiness, 1);
    let verdict = src.assert_signal(SignalName::Readiness, Predicate::Eq(0));
    assert!(
        !verdict.is_green(),
        "a readiness that did NOT flip on a dead critical dep MUST read RED - the gate is real"
    );

    let mut src = SignalSource::new();
    src.set_scalar(SignalName::LivenessRestartCount, 5);
    let verdict = src.assert_signal(SignalName::LivenessRestartCount, Predicate::Eq(0));
    assert!(
        !verdict.is_green(),
        "a liveness restart-storm on a dependency outage MUST read RED - the gate is real"
    );
}

#[test]
fn readiness_reads_the_real_injector_severance() {
    let breaker = DependencyBreaker::new();
    let health = InjectorHealth {
        breaker: breaker.clone(),
    };
    let mh = MetricsHealthSurface::new(CriticalDependencies::new(["identity"]), health);
    mh.mark_started();
    assert_eq!(
        mh.readiness().verdict.gauge(),
        1,
        "ready while the injector has nothing broken"
    );
    breaker.break_dependency(Dependency::Identity, Scope::Global);
    assert_eq!(
        mh.readiness().verdict.gauge(),
        0,
        "not-ready once the injector severs the dep"
    );
    breaker.restore_dependency(Dependency::Identity, Scope::Global);
    assert_eq!(
        mh.readiness().verdict.gauge(),
        1,
        "ready again once restored"
    );
}
