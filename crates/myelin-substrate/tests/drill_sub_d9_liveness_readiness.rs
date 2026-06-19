//! # SUB-D9 — the liveness ≠ readiness drill (P-S14 → global P-031)
//!
//! **Drill catalogue:** `planning/05-refined-shared-systems-architecture/testing-strategy/
//! 01-whole-system-e2e-and-drill-catalogue.md` §4.2 row **SUB-D9**: *"Kill a critical dependency
//! → instance not-ready + sheds; no restart-storm."* Threshold: `readiness flips`; `no liveness
//! churn`. Surface: CI.
//!
//! This is the **dated green artifact** the P-S14 GATE/DRILLS names. It is the EI-01 §3 drill
//! shape: *inject a fault (P-S03 `break_dependency`), drive one unit of load (P-S02 generator),
//! read one telemetry assertion that reads green.* Here:
//!   - **inject** — `break_dependency(Identity, Global)` hard-downs a CRITICAL dependency
//!     (Identity/authz) — a sustained outage (§8.3). The harness dependency-break injector IS the
//!     SUB-D9 probe: the metrics-health surface's `DependencyHealth` reads the injector's
//!     `is_broken` truth, so a really-severed dependency reads down.
//!   - **load** — a burst of readiness + liveness probe scrapes (what the orchestrator does every
//!     few seconds), interleaved across the outage and its healing.
//!   - **assert** — `readiness` gauge flips `1 → 0` (not-ready + sheds) while `liveness_restart_count
//!     == 0` (liveness does NOT check the dependency → no restart-storm). Both read green.
//!
//! The composition with fail-static (§8.3) is named: a *transient* hiccup is absorbed by
//! `FailStatic` (P-S18) before it reads "down" here; this drill is the *sustained*-outage lane
//! (readiness sheds). A fail-static-buys-the-transient drill is SUB-D4 (P-S25).

use myelin_harness::{
    Dependency, DependencyBreaker, DrillResult, DrillScenario, Predicate, Scope, SignalName,
    SignalSource,
};
use myelin_substrate::{
    CriticalDependencies, CriticalDependency, DependencyHealth, MetricsHealthSurface,
};

/// A [`DependencyHealth`] probe backed by the harness dependency-break injector: a critical
/// dependency reads DOWN iff the injector has it broken (for the drill's scope). This is the
/// SUB-D9 wiring the architecture names — the metrics-health readiness probe reads the SAME
/// severed-dependency truth the injector exposes (a really-severed dependency, not a fake flag).
#[derive(Clone)]
struct InjectorHealth {
    breaker: DependencyBreaker,
}

impl DependencyHealth for InjectorHealth {
    fn is_up(&self, dep: &CriticalDependency) -> bool {
        // Map the critical-dependency name to the injector's `Dependency` and consult the GLOBAL
        // scope (a hard-down is rig-wide). `identity` is the canonical critical dep; any other
        // name maps to a `Named` dependency so the wiring is general.
        let dependency = match dep.0.as_str() {
            "identity" => Dependency::Identity,
            other => Dependency::Named(other.to_string()),
        };
        !self.breaker.is_broken(&dependency, &Scope::Global)
    }
}

/// The SUB-D9 drill scenario: hard-down a critical dependency via the injector, scrape both probes
/// across the outage, and assert readiness flips to not-ready (gauge `0`) and sheds while liveness
/// does not restart-storm (`liveness_restart_count == 0`).
fn sub_d9_liveness_readiness_scenario() -> DrillScenario {
    DrillScenario::new("sub-d9-liveness-not-readiness", |ctx| {
        // The metrics-health surface declaring `identity` critical, with its readiness probe wired
        // to the harness injector. Booted (startup gate complete) so readiness is governed purely
        // by dependency health.
        let health = InjectorHealth { breaker: ctx.breaker.clone() };
        let mh = MetricsHealthSurface::new(CriticalDependencies::new(["identity"]), health);
        mh.mark_started();

        // healthy baseline: ready (gauge 1), liveness up, no churn.
        assert_eq!(mh.readiness().verdict.gauge(), 1, "healthy baseline is ready");
        assert!(!mh.liveness().should_restart(), "healthy baseline is live");

        // (inject) hard-down the CRITICAL dependency (a sustained outage, §8.3).
        ctx.breaker.break_dependency(Dependency::Identity, Scope::Global);

        // (load) the orchestrator scrapes both probes repeatedly across the outage. Track the
        // readiness gauge + count any liveness restart that fired (must stay 0).
        let mut readiness_gauge_during_outage = 1i64;
        let mut shed_observed = false;
        for _ in 0..30 {
            let r = mh.readiness();
            readiness_gauge_during_outage = r.verdict.gauge();
            shed_observed |= r.sheds();
            // liveness must NOT restart on a dependency outage — if it ever did, the surface would
            // record a restart. It does not (liveness never checks the dependency).
            if mh.liveness().should_restart() {
                mh.record_liveness_restart();
            }
        }
        // during the outage: not-ready + shedding.
        assert_eq!(readiness_gauge_during_outage, 0, "readiness flipped to not-ready during the outage");
        assert!(shed_observed, "the not-ready instance shed new traffic");
        let liveness_restarts = mh.liveness_restart_count() as i64;

        // (heal) restore the dependency; readiness recovers (proves the flip tracks live truth).
        ctx.breaker.restore_dependency(Dependency::Identity, Scope::Global);
        assert_eq!(mh.readiness().verdict.gauge(), 1, "readiness recovers when the dependency heals");

        // (assert) the two SUB-D9 survival signals, read off the harness telemetry-assertion
        // library — typed, never-swallowed verdicts.
        let mut src = SignalSource::new();
        src.set_scalar(SignalName::Readiness, readiness_gauge_during_outage); // 0 during outage
        src.set_scalar(SignalName::LivenessRestartCount, liveness_restarts); // 0 — no churn

        // readiness flipped to not-ready (gauge == 0) during the outage.
        src.assert_signal(SignalName::Readiness, Predicate::Eq(0)).expect_green();
        // no restart-storm: liveness churn stayed 0.
        src.assert_signal(SignalName::LivenessRestartCount, Predicate::Eq(0))
    })
}

/// **THE SUB-D9 drill — the dated green artifact.** Runs the scenario once and asserts it PASSES:
/// a killed critical dependency → readiness not-ready + sheds, with ZERO liveness restart-storm.
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
    assert!(row.contains("PASS"), "the artifact row records a PASS: {row}");
    assert!(row.contains("sub-d9-liveness-not-readiness"), "names the drill: {row}");
    println!("{row}");
}

/// The drill re-runs green forever (the every-incident-adds-a-drill loop, T-3): each run from a
/// fresh context reads green again — the property is proven each time, not a stale leftover.
#[test]
fn sub_d9_drill_reruns_green() {
    let drill = sub_d9_liveness_readiness_scenario();
    for _ in 0..3 {
        assert!(matches!(drill.run_once(), DrillResult::Pass { .. }), "SUB-D9 re-runs green");
    }
}

/// **The gate is NOT vacuous** — a regression reads RED. Two failure modes the gate must catch:
///   1. readiness that did NOT flip on a dead critical dependency (the "healthy-but-failing" bug)
///      → `Readiness == 0` reads RED against the observed `1`.
///   2. a liveness restart-storm on a dependency outage (the conflated-probe bug) →
///      `LivenessRestartCount == 0` reads RED against a non-zero observed value.
///
/// Confirms both SUB-D9 assertions are real, not always-green.
#[test]
fn sub_d9_gate_is_not_vacuous_a_regression_reads_red() {
    // (1) readiness that failed to flip (still reads ready=1 while a critical dep is down).
    let mut src = SignalSource::new();
    src.set_scalar(SignalName::Readiness, 1); // the bug: stayed "ready" during the outage
    let verdict = src.assert_signal(SignalName::Readiness, Predicate::Eq(0));
    assert!(
        !verdict.is_green(),
        "a readiness that did NOT flip on a dead critical dep MUST read RED — the gate is real"
    );

    // (2) a liveness restart-storm caused by a dependency outage (the conflated-probe regression).
    let mut src = SignalSource::new();
    src.set_scalar(SignalName::LivenessRestartCount, 5); // the bug: 5 restarts on a dep outage
    let verdict = src.assert_signal(SignalName::LivenessRestartCount, Predicate::Eq(0));
    assert!(
        !verdict.is_green(),
        "a liveness restart-storm on a dependency outage MUST read RED — the gate is real"
    );
}

/// The SUB-D9 wiring is faithful: the metrics-health readiness probe reads the REAL severed-
/// dependency truth off the injector (not a decoupled fake). Breaking the dependency through the
/// injector flips readiness; restoring it heals — proving the drill exercises the real mechanism.
#[test]
fn readiness_reads_the_real_injector_severance() {
    let breaker = DependencyBreaker::new();
    let health = InjectorHealth { breaker: breaker.clone() };
    let mh = MetricsHealthSurface::new(CriticalDependencies::new(["identity"]), health);
    mh.mark_started();
    assert_eq!(mh.readiness().verdict.gauge(), 1, "ready while the injector has nothing broken");
    breaker.break_dependency(Dependency::Identity, Scope::Global);
    assert_eq!(mh.readiness().verdict.gauge(), 0, "not-ready once the injector severs the dep");
    breaker.restore_dependency(Dependency::Identity, Scope::Global);
    assert_eq!(mh.readiness().verdict.gauge(), 1, "ready again once restored");
}
