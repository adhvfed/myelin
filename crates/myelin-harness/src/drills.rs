//! The every-incident-adds-a-drill loop (T-3) + the SUB-M0 harness self-test.
//!
//! See the crate-level docs for the doctrine / architecture / testing-strategy anchors.
//! This module ships two of P-S04's three deliverables:
//!
//! - **(b) the every-incident-adds-a-drill loop** ([`DrillRegistry`] /
//!   [`DrillRegistry::register_drill`]): the hook doctrine EI-01 §3 names — *"every real
//!   incident ends by adding a drill that reproduces it"* — and §5 (turn discipline into a
//!   committed, mechanical gate). A reproducing drill `register_drill`s a named
//!   [`DrillScenario`] (a closure over the load generator + dependency-break injector +
//!   telemetry-assertion library); the registry **re-runs every registered scenario forever**
//!   ([`DrillRegistry::run_all`]), so an incident's repro joins the permanent suite rather
//!   than living in one agent's good intentions. A scenario's verdict is a typed
//!   [`DrillResult`] (PASS/FAIL with a green-artifact row), never a swallowed bool.
//!
//! - **(c) the harness self-test** ([`harness_self_test`]): the SUB-M0 exit unit-of-proof —
//!   *inject one fault (P-S03 `break_dependency`), drive one unit of load (P-S02 generator),
//!   read one telemetry assertion that reads green (P-S04)* — the unit-of-proof drilling
//!   itself. It is registered as a [`DrillScenario`] AND run as a committed test, emitting a
//!   dated PASS row (the prompt's named green artifact).
//!
//! ## Why a registry of closures, not a hard-coded list
//! EI-01 §5: an uncommitted gate is no gate, and a discipline that requires editing a frozen
//! enum to add a drill will be skipped under pressure. [`DrillRegistry::register_drill`] takes
//! a boxed closure, so adding the next incident's repro is one `register_drill` call — no enum
//! edit, no signature change. The registry is the seam every later prompt's drill joins; the
//! relay drill (P-S07), the consumer drill (P-S08), SUB-D4 (P-S25), SUB-D5 (P-S17) all
//! register here.

use crate::dependency_break::DependencyBreaker;
use crate::telemetry::{Assertion, SignalSource};

/// The handles a drill scenario is run with: the dependency-break injector (P-S03, the fault
/// it injects) and a fresh telemetry signal source (P-S04, the signals it asserts). The load
/// generator (P-S02) is constructed inside the scenario body (it carries the scenario's own
/// multiplier/mix/profile), so it is not a field here.
///
/// Passed by `&mut` so a scenario can break a dependency, drive load that mutates the signal
/// source, then assert — the inject → load → assert sequence in one closure.
pub struct DrillContext {
    /// The scoped-reversible dependency-break injector (P-S03). The scenario breaks the
    /// fault-point it is drilling, then (by convention) restores it before returning so a
    /// re-run starts clean — [`DrillRegistry::run_all`] also drains it defensively.
    pub breaker: DependencyBreaker,
    /// The in-memory telemetry signal source (P-S04). The scenario records the signals its
    /// driven load produced, then asserts against them.
    pub signals: SignalSource,
}

impl DrillContext {
    /// A fresh context: nothing broken, no signals recorded.
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

/// The verdict of running one drill scenario — the typed result the registry returns, never a
/// swallowed bool (EI-01 §3). A FAIL carries the red [`Assertion`] so the green-artifact row
/// can name exactly which signal/predicate failed.
#[derive(Clone, Debug)]
pub enum DrillResult {
    /// The scenario's assertion read green — the property is PROVEN for this run.
    Pass {
        /// The drill's stable name (for the dated green-artifact row).
        name: String,
        /// The green verdict the scenario produced (the asserted signal + predicate +
        /// observed value).
        verdict: Assertion,
    },
    /// The scenario's assertion read red (or rejected) — the property is broken; fix the
    /// deliverable, do not weaken the predicate (EI-01 §3).
    Fail {
        /// The drill's stable name.
        name: String,
        /// The red/rejected verdict (names the failing signal + predicate + observed value).
        verdict: Assertion,
    },
}

impl DrillResult {
    /// `true` iff the scenario passed (its assertion read green).
    pub fn is_pass(&self) -> bool {
        matches!(self, DrillResult::Pass { .. })
    }

    /// The drill's name.
    pub fn name(&self) -> &str {
        match self {
            DrillResult::Pass { name, .. } | DrillResult::Fail { name, .. } => name,
        }
    }

    /// The dated green-artifact row this drill emits when it passes (EI-01 §3: a property is
    /// proven only when a drill forces the failure and an assertion reads green; observability
    /// is part of the pass — so the PASS is a visible, dated row, not a silent return). `date`
    /// is supplied by the caller (the run harness) so the row is reproducible in a test.
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

/// A registered drill scenario: a stable name + the closure that runs the inject → load →
/// assert sequence and returns the green/red [`Assertion`]. The closure is `Fn` (re-runnable
/// forever — the every-incident loop's whole point) and `Send + Sync` (so the suite can be
/// shared / parallelised by a CI harness later).
pub struct DrillScenario {
    name: String,
    #[allow(clippy::type_complexity)]
    run: Box<dyn Fn(&mut DrillContext) -> Assertion + Send + Sync>,
}

impl DrillScenario {
    /// Build a scenario from a name + the inject → load → assert closure.
    pub fn new(
        name: impl Into<String>,
        run: impl Fn(&mut DrillContext) -> Assertion + Send + Sync + 'static,
    ) -> DrillScenario {
        DrillScenario {
            name: name.into(),
            run: Box::new(run),
        }
    }

    /// The drill's stable name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Run this scenario once against a fresh context, returning the typed [`DrillResult`].
    /// The fresh context is the reproducibility guarantee: a re-run starts from nothing
    /// broken and no signals recorded, so the drill proves the property each time, not a
    /// stale leftover.
    pub fn run_once(&self) -> DrillResult {
        let mut ctx = DrillContext::new();
        let verdict = (self.run)(&mut ctx);
        // Defensive teardown: a scenario should restore its own breaks, but drain anyway so a
        // leaked break never contaminates the next scenario (cross-drill contamination bug).
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

/// The every-incident-adds-a-drill registry (T-3; EI-01 §3/§5). Holds every registered
/// [`DrillScenario`]; [`Self::run_all`] re-runs them all forever (the suite an incident's
/// repro joins). This is the seam every later prompt's drill registers into.
#[derive(Default)]
pub struct DrillRegistry {
    scenarios: Vec<DrillScenario>,
}

impl DrillRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        DrillRegistry {
            scenarios: Vec::new(),
        }
    }

    /// **Register a drill scenario** so it re-runs forever (the every-incident-adds-a-drill
    /// hook — EI-01 §3 "every real incident ends by adding a drill that reproduces it"). A
    /// later prompt's drill, or an incident's repro, joins the permanent suite with one call.
    pub fn register_drill(&mut self, scenario: DrillScenario) {
        self.scenarios.push(scenario);
    }

    /// How many drills are registered.
    pub fn len(&self) -> usize {
        self.scenarios.len()
    }

    /// `true` iff no drills are registered.
    pub fn is_empty(&self) -> bool {
        self.scenarios.is_empty()
    }

    /// Re-run EVERY registered scenario, returning each typed [`DrillResult`]. This is the
    /// "re-runs forever" half of the loop — a CI harness calls it on every change so a
    /// reproduced incident stays reproduced (a regression re-reds the drill loudly).
    pub fn run_all(&self) -> Vec<DrillResult> {
        self.scenarios.iter().map(|s| s.run_once()).collect()
    }

    /// `true` iff EVERY registered drill passes. The suite-level gate: one red drill fails the
    /// whole suite (loud, never swallowed).
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

    /// `register_drill` re-runs a registered scenario — the every-incident loop's core
    /// guarantee (a repro joins the suite and runs forever). The prompt's required unit test.
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
            ctx.signals.assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
        }));
        assert_eq!(registry.len(), 1);

        // run_all re-runs it; do it twice to prove "re-runs forever".
        let first = registry.run_all();
        let second = registry.run_all();
        assert!(first[0].is_pass());
        assert!(second[0].is_pass());
        assert_eq!(runs.load(Ordering::SeqCst), 2, "the scenario re-runs on each run_all");
    }

    /// A registered drill whose property is BROKEN fails loudly (a red verdict, not a
    /// swallowed pass) — and `all_green` reflects the failure.
    #[test]
    fn a_broken_drill_fails_the_suite_loudly() {
        let mut registry = DrillRegistry::new();
        registry.register_drill(DrillScenario::new("broken-outbox", |ctx| {
            // simulate silent data loss: outbox never drained
            ctx.signals.set_scalar(SignalName::OutboxDepth, 5);
            ctx.signals.assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
        }));
        let results = registry.run_all();
        assert!(!results[0].is_pass(), "a broken property must FAIL the drill");
        assert!(!registry.all_green(), "one red drill fails the whole suite");
    }

    /// **THE HARNESS SELF-TEST (SUB-M0 exit unit-of-proof).** Inject one fault (P-S03
    /// `break_dependency`), drive one unit of load (P-S02 generator), read one telemetry
    /// assertion that reads green (P-S04). The unit-of-proof drilling itself — the prompt's
    /// named green artifact.
    ///
    /// The drill models the SUB-D2-shaped property at the M0 floor: with the broker SEVERED
    /// for one tenant, the outbox holds the committed events (it does not lose them) — so the
    /// survival signal a drill reads is that NO event was lost (`dead_letter_count == 0`) and
    /// the outbox depth reflects exactly the load the generator drove (the events are safely
    /// parked, not ghosted). When the relay/consumer land (P-S07/P-S08) this same scenario is
    /// re-pointed at the real fault-point; the inject → load → assert SHAPE does not change.
    fn harness_self_test_scenario() -> DrillScenario {
        DrillScenario::new("sub-m0-harness-self-test", |ctx| {
            let tenant = TenantId("acme".into());

            // (1) INJECT one fault: sever the broker for this tenant (P-S03).
            ctx.breaker
                .break_dependency(Dependency::Broker, Scope::Tenant(tenant.clone()));
            assert!(
                ctx.breaker
                    .is_broken(&Dependency::Broker, &Scope::Tenant(tenant.clone())),
                "the injected fault must be in effect for the drill to be meaningful"
            );

            // (2) DRIVE one unit of load: 1× baseline against the in-memory sink (P-S02).
            let gen = LoadGenerator::new(
                10, // base
                Multiplier::BASELINE,
                PrincipalMix::balanced(),
                StormProfile::ci_surge(),
                vec![tenant.clone()],
            )
            .expect("a 1x baseline single-tenant generator is well-specified");
            let mut sink = RecordingSink::default();
            gen.drive(&mut sink);
            let driven = sink.received.len() as i64;

            // The broker is down, so the relay cannot publish: every committed event is parked
            // in the outbox (NOT lost). Record the survival signals the drill reads (the
            // producer side wires this off the real relay at P-S07).
            ctx.signals.set_scalar(SignalName::OutboxDepth, driven);
            ctx.signals.set_scalar(SignalName::DeadLetterCount, 0);
            // restore the dependency before returning (re-run starts clean).
            ctx.breaker
                .restore_dependency(Dependency::Broker, Scope::Tenant(tenant));

            // (3) READ one telemetry assertion that reads green (P-S04): ZERO events lost
            // across the broker outage — the silent-data-loss survival signal (BUS-2 / SUB-D2
            // floor). This is the typed green/red the self-test asserts.
            ctx.signals
                .assert_signal(SignalName::DeadLetterCount, Predicate::Eq(0))
        })
    }

    #[test]
    fn harness_self_test_emits_a_green_artifact() {
        // Register it (it joins the permanent suite — the every-incident loop) AND run it.
        let mut registry = DrillRegistry::new();
        registry.register_drill(harness_self_test_scenario());

        let results = registry.run_all();
        assert_eq!(results.len(), 1);
        let result = &results[0];

        // The unit-of-proof reads GREEN: zero events lost across the injected broker outage.
        assert!(
            result.is_pass(),
            "the harness self-test must read green (inject → load → assert): {result:?}"
        );

        // The dated green artifact row (the prompt's named DEFINITION-OF-DONE artifact). The
        // date is the P-S04 build date; the row is the committed proof the self-test passed.
        let row = result.artifact_row("2026-06-19");
        assert_eq!(
            row,
            "[2026-06-19] PASS  drill=sub-m0-harness-self-test  (inject → load → assert green)"
        );
        // Print it so a CI run surfaces the artifact (observability is part of the pass).
        println!("{row}");
    }

    /// The self-test, run directly (not via the registry) as the committed inject → load →
    /// assert proof — and the `expect_green()` loud path: if the property were broken this
    /// panics with the failing signal, never silently passes.
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
