use crate::dependency_break::DependencyBreaker;
use crate::telemetry::{Assertion, SignalSource};

pub struct DrillContext {
    pub breaker: DependencyBreaker,
    pub signals: SignalSource,
}

impl DrillContext {
    pub fn new() -> Self {
        DrillContext {
            breaker: DependencyBreaker::new(),
            signals: SignalSource::new(),
        }
    }
}

impl Default for DrillContext {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug)]
pub enum DrillResult {
    Pass { name: String, verdict: Assertion },
    Fail { name: String, verdict: Assertion },
}

impl DrillResult {
    pub fn is_pass(&self) -> bool {
        matches!(self, DrillResult::Pass { .. })
    }

    pub fn name(&self) -> &str {
        match self {
            DrillResult::Pass { name, .. } | DrillResult::Fail { name, .. } => name,
        }
    }

    pub fn artifact_row(&self, date: &str) -> String {
        match self {
            DrillResult::Pass { name, .. } => {
                format!("[{date}] PASS  drill={name}  (inject → load → assert green)")
            }
            DrillResult::Fail { name, verdict } => {
                format!("[{date}] FAIL  drill={name}  verdict={verdict:?}")
            }
        }
    }
}

pub struct DrillScenario {
    name: String,
    #[allow(clippy::type_complexity)]
    run: Box<dyn Fn(&mut DrillContext) -> Assertion + Send + Sync>,
}

impl DrillScenario {
    pub fn new(
        name: impl Into<String>,
        run: impl Fn(&mut DrillContext) -> Assertion + Send + Sync + 'static,
    ) -> DrillScenario {
        DrillScenario {
            name: name.into(),
            run: Box::new(run),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn run_once(&self) -> DrillResult {
        let mut ctx = DrillContext::new();
        let verdict = (self.run)(&mut ctx);
        ctx.breaker.restore_all();
        if verdict.is_green() {
            DrillResult::Pass {
                name: self.name.clone(),
                verdict,
            }
        } else {
            DrillResult::Fail {
                name: self.name.clone(),
                verdict,
            }
        }
    }
}

#[derive(Default)]
pub struct DrillRegistry {
    scenarios: Vec<DrillScenario>,
}

impl DrillRegistry {
    pub fn new() -> Self {
        DrillRegistry {
            scenarios: Vec::new(),
        }
    }

    pub fn register_drill(&mut self, scenario: DrillScenario) {
        self.scenarios.push(scenario);
    }

    pub fn len(&self) -> usize {
        self.scenarios.len()
    }

    pub fn is_empty(&self) -> bool {
        self.scenarios.is_empty()
    }

    pub fn run_all(&self) -> Vec<DrillResult> {
        self.scenarios.iter().map(|s| s.run_once()).collect()
    }

    pub fn all_green(&self) -> bool {
        self.run_all().iter().all(|r| r.is_pass())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dependency_break::{Dependency, Scope};
    use crate::load_generator::{
        LoadGenerator, Multiplier, PrincipalMix, RecordingSink, StormProfile,
    };
    use crate::telemetry::{Predicate, SignalName};
    use myelin_tenancy::TenantId;

    #[test]
    fn register_drill_reruns_a_registered_scenario() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let runs = Arc::new(AtomicUsize::new(0));
        let runs_in = runs.clone();
        let mut registry = DrillRegistry::new();
        registry.register_drill(DrillScenario::new("counts-its-runs", move |ctx| {
            runs_in.fetch_add(1, Ordering::SeqCst);
            ctx.signals.set_scalar(SignalName::OutboxDepth, 0);
            ctx.signals
                .assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
        }));
        assert_eq!(registry.len(), 1);

        let first = registry.run_all();
        let second = registry.run_all();
        assert!(first[0].is_pass());
        assert!(second[0].is_pass());
        assert_eq!(
            runs.load(Ordering::SeqCst),
            2,
            "the scenario re-runs on each run_all"
        );
    }

    #[test]
    fn a_broken_drill_fails_the_suite_loudly() {
        let mut registry = DrillRegistry::new();
        registry.register_drill(DrillScenario::new("broken-outbox", |ctx| {
            ctx.signals.set_scalar(SignalName::OutboxDepth, 5);
            ctx.signals
                .assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
        }));
        let results = registry.run_all();
        assert!(
            !results[0].is_pass(),
            "a broken property must FAIL the drill"
        );
        assert!(!registry.all_green(), "one red drill fails the whole suite");
    }

    fn harness_self_test_scenario() -> DrillScenario {
        DrillScenario::new("sub-m0-harness-self-test", |ctx| {
            let tenant = TenantId("acme".into());

            ctx.breaker
                .break_dependency(Dependency::Broker, Scope::Tenant(tenant.clone()));
            assert!(
                ctx.breaker
                    .is_broken(&Dependency::Broker, &Scope::Tenant(tenant.clone())),
                "the injected fault must be in effect for the drill to be meaningful"
            );

            let gen = LoadGenerator::new(
                10,
                Multiplier::BASELINE,
                PrincipalMix::balanced(),
                StormProfile::ci_surge(),
                vec![tenant.clone()],
            )
            .expect("a 1x baseline single-tenant generator is well-specified");
            let mut sink = RecordingSink::default();
            gen.drive(&mut sink);
            let driven = sink.received.len() as i64;

            ctx.signals.set_scalar(SignalName::OutboxDepth, driven);
            ctx.signals.set_scalar(SignalName::DeadLetterCount, 0);
            ctx.breaker
                .restore_dependency(Dependency::Broker, Scope::Tenant(tenant));

            ctx.signals
                .assert_signal(SignalName::DeadLetterCount, Predicate::Eq(0))
        })
    }

    #[test]
    fn harness_self_test_emits_a_green_artifact() {
        let mut registry = DrillRegistry::new();
        registry.register_drill(harness_self_test_scenario());

        let results = registry.run_all();
        assert_eq!(results.len(), 1);
        let result = &results[0];

        assert!(
            result.is_pass(),
            "the harness self-test must read green (inject → load → assert): {result:?}"
        );

        let row = result.artifact_row("2026-06-19");
        assert_eq!(
            row,
            "[2026-06-19] PASS  drill=sub-m0-harness-self-test  (inject → load → assert green)"
        );
        println!("{row}");
    }

    #[test]
    fn harness_self_test_inject_load_assert_is_green() {
        let scenario = harness_self_test_scenario();
        let result = scenario.run_once();
        match result {
            DrillResult::Pass { verdict, .. } => verdict.expect_green(),
            DrillResult::Fail { verdict, .. } => {
                panic!("self-test must pass, got {verdict:?}")
            }
        }
    }
}
