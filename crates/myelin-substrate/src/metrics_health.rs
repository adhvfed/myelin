//! # Liveness ≠ readiness on the metrics-health surface (P-S14 → global P-031, SUB-D9)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/00-platform-substrate.md`
//! §4.3 (the metrics-health surface — **liveness ≠ readiness**):
//!   - **Liveness = "not wedged"** → restart on fail; it **must NOT check dependencies**. A
//!     dead downstream is not a reason to restart this process (restarting does not heal a
//!     downstream; it only adds a restart-storm to the outage).
//!   - **Readiness = "can serve correct traffic now"** → a dead **critical** dependency reports
//!     *not-ready* and stops taking traffic (sheds), **never** reports healthy-but-failing.
//!   - **Startup = boot/migration incomplete** → not-ready, **not-killed** (a slow boot must not
//!     be mistaken for a wedged process and killed before it is ready).
//!
//! And §8.3 (the composition with fail-static): **readiness** handles a *sustained* / hard-down
//! outage (not-ready + shed); **fail-static** ([`crate::FailStatic`], P-S18) buys the
//! seconds-to-minutes of a *transient* hiccup. The two compose: a transient blip is absorbed by
//! fail-static without ever flipping readiness; only a sustained outage flips readiness and sheds.
//!
//! **Contract-index:** row 1.3 (`Liveness ≠ readiness`) — OWNED here.
//!
//! **GATE / DRILLS (this prompt):** **SUB-D9** — kill a critical dependency → the instance
//! reports *not-ready* + sheds; liveness does **not** restart-storm. The survival signals are
//! `readiness` (flips `1 → 0`) + `liveness_restart_count` (stays `0` — no churn). The drill
//! scenario lives in `tests/drill_sub_d9_liveness_readiness.rs`.
//!
//! ## Why the split is load-bearing (not a probe-config detail)
//! Conflating the two probes is a classic outage amplifier: if liveness checked dependencies,
//! a downstream blip would make every replica fail its liveness probe and get **killed** — the
//! orchestrator then restart-storms a fleet that was perfectly healthy, turning one dependency's
//! hiccup into a self-inflicted whole-service outage. The structural rule, enforced here:
//!   - [`MetricsHealthSurface::liveness`] reads ONLY the process's own wedged-ness
//!     ([`LivenessState`]); it is given NO handle to the dependency set, so it *cannot* check a
//!     dependency even by accident. A severed dependency leaves liveness `Up`.
//!   - [`MetricsHealthSurface::readiness`] reads the critical-dependency set + the startup gate;
//!     a single dead critical dependency → [`Readiness::NotReady`] and the surface sheds.
//!
//! ## What "critical" means here
//! A dependency is **critical** iff the service cannot serve *correct* traffic without it (a dead
//! critical dependency must shed, never serve wrong answers). A non-critical dependency being down
//! degrades a feature but does not flip readiness (the service still serves correct traffic for
//! everything else). The service declares its critical set at boot via
//! [`CriticalDependencies`]; the live up/down truth is read through a [`DependencyHealth`] probe
//! (in the drill, the harness dependency-break injector is the probe — a real severed dependency).
//!
//! ## Floors named (deferred bodies → filling prompt)
//! - **The real Kubernetes-style `/livez` + `/readyz` HTTP handlers + the OTLP gauge export** on
//!   the real metrics-health listener land with the real transport/listener wiring (the §3.5
//!   producer is already exported in-process by `serve`'s [`crate::serve::Telemetry`]; here the
//!   readiness/liveness *gauges* are exported by the same in-process meter shape). Named, not
//!   silently skipped (EI-01 §4). The semantics (liveness ignores deps; readiness sheds on a dead
//!   critical dep; startup is not-ready-not-killed) are COMPLETE now.
//! - **Fail-static composition** ([`crate::FailStatic`], 1.10) → **P-S18** (the mechanism) /
//!   **P-S25** (proven vs a real Identity hiccup, SUB-D4). The composition is NAMED + asserted
//!   structurally here ([`ReadinessReport::sheds`] + the §8.3 doc above): a *transient* hiccup is
//!   the fail-static lane, a *sustained* outage is the readiness lane. The transient-vs-sustained
//!   classification policy (the staleness window `W`) is the fail-static prompt's; this surface
//!   reads the *current* up/down truth a probe reports.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The liveness verdict (architecture §4.3) — "is this process wedged?". The ONLY input is the
/// process's own health; it is **never** a function of a dependency (that is the whole point of
/// the split). A wedged process should be restarted; a healthy process whose downstream is down
/// is NOT wedged (restarting it does not heal the downstream).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Liveness {
    /// The process is responsive (not wedged) — do NOT restart.
    Up,
    /// The process is wedged (deadlocked / event loop stalled / unrecoverable) — restart it. The
    /// orchestrator restarts on this; it is reached ONLY by a real wedge, never by a dead
    /// dependency.
    Wedged,
}

impl Liveness {
    /// `true` iff the orchestrator should restart the process on this verdict (only [`Self::Wedged`]).
    pub fn should_restart(self) -> bool {
        matches!(self, Liveness::Wedged)
    }
}

/// The readiness verdict (architecture §4.3) — "can this instance serve *correct* traffic now?".
/// A dead critical dependency or an incomplete boot reports [`Self::NotReady`] and the surface
/// sheds new traffic (never reports healthy-but-failing). A ready instance serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Readiness {
    /// The instance can serve correct traffic — accept traffic.
    Ready,
    /// The instance cannot serve correct traffic right now — **shed** new traffic (do NOT serve
    /// wrong answers). Reached by a dead critical dependency (a sustained outage, §8.3) or an
    /// incomplete boot/migration (startup), never by a healthy instance.
    NotReady,
}

impl Readiness {
    /// `true` iff the instance is ready to accept traffic.
    pub fn is_ready(self) -> bool {
        matches!(self, Readiness::Ready)
    }

    /// `true` iff the instance must SHED new traffic (the not-ready → shed rule, §4.3).
    pub fn sheds(self) -> bool {
        matches!(self, Readiness::NotReady)
    }

    /// The numeric gauge value exported on the metrics-health surface (`1` = ready, `0` =
    /// not-ready) — the SUB-D9 `readiness` survival signal the harness asserts.
    pub fn gauge(self) -> i64 {
        match self {
            Readiness::Ready => 1,
            Readiness::NotReady => 0,
        }
    }
}

/// The process's own wedged-ness, the SOLE input to [`MetricsHealthSurface::liveness`]
/// (architecture §4.3 — liveness must not check dependencies). A typed flag the runtime flips
/// only when the process is genuinely wedged (a watchdog the real event loop pings); a dependency
/// outage NEVER touches it. Held behind a shared atomic so the (future) watchdog and the probe
/// observe one truth.
#[derive(Clone, Default)]
pub struct LivenessState {
    wedged: Arc<std::sync::atomic::AtomicBool>,
}

impl LivenessState {
    /// A fresh, healthy (not-wedged) process.
    pub fn new() -> LivenessState {
        LivenessState::default()
    }

    /// Mark the process wedged (the watchdog detected an unrecoverable stall). The ONLY way
    /// liveness flips — a dependency outage cannot reach this.
    pub fn mark_wedged(&self) {
        self.wedged.store(true, Ordering::SeqCst);
    }

    /// The liveness verdict — purely the process's own wedged-ness (no dependency input).
    pub fn liveness(&self) -> Liveness {
        if self.wedged.load(Ordering::SeqCst) {
            Liveness::Wedged
        } else {
            Liveness::Up
        }
    }
}

/// The startup gate (architecture §4.3 — "startup = boot/migration incomplete → not-ready,
/// not-killed"). While a service is still booting (opening the pool, running migrations, warming
/// caches) it is **not-ready** (must not take traffic — it cannot serve correct traffic yet) but
/// **not-killed** (a slow boot is not a wedge; liveness stays `Up` so the orchestrator does not
/// restart it before it can finish booting). `serve` flips this to complete once boot succeeds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Startup {
    /// Boot/migration is still in progress — not-ready, not-killed.
    #[default]
    Booting,
    /// Boot/migration completed — the startup gate no longer holds readiness down.
    Complete,
}

/// One named critical dependency (architecture §4.3 / §8.3). PII-free opaque name (the same
/// `control-plane-pii-free` discipline as the dependency-break injector labels). A dependency is
/// **critical** iff the service cannot serve correct traffic without it — a dead one flips
/// readiness. The name matches the probe key the [`DependencyHealth`] reports up/down under.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CriticalDependency(pub String);

impl CriticalDependency {
    /// A critical dependency from its opaque name.
    pub fn new(name: impl Into<String>) -> CriticalDependency {
        CriticalDependency(name.into())
    }
}

/// The set of critical dependencies a service declares at boot (architecture §4.3). Readiness is
/// `NotReady` iff ANY declared critical dependency is currently down (a single dead critical
/// dependency means the service cannot serve correct traffic → shed). A non-critical dependency
/// is deliberately NOT in this set — its being down degrades a feature but does not flip readiness.
#[derive(Clone, Debug, Default)]
pub struct CriticalDependencies {
    deps: Vec<CriticalDependency>,
}

impl CriticalDependencies {
    /// Declare the critical set from a list of names.
    pub fn new(names: impl IntoIterator<Item = impl Into<String>>) -> CriticalDependencies {
        CriticalDependencies {
            deps: names.into_iter().map(|n| CriticalDependency::new(n)).collect(),
        }
    }

    /// The declared critical dependencies.
    pub fn deps(&self) -> &[CriticalDependency] {
        &self.deps
    }
}

/// The live up/down truth of a dependency (architecture §4.3 — readiness reads the *current*
/// truth a probe reports). The metrics-health surface consults this for each critical dependency;
/// in the SUB-D9 drill the harness dependency-break injector IS the probe (a really-severed
/// dependency reads down), and in production the resilient client's breaker state (§6) feeds it.
pub trait DependencyHealth: Send + Sync {
    /// Is `dep` currently up (reachable + serving)? `true` = up, `false` = down. Readiness
    /// flips to not-ready iff any critical dependency reads `false`.
    fn is_up(&self, dep: &CriticalDependency) -> bool;
}

/// A simple in-process [`DependencyHealth`] probe — a name→up/down table the test/drill drives.
/// Every dependency defaults to **up** (absence of an explicit down means healthy); a drill marks
/// one down to model a severed critical dependency. (The harness `DependencyBreaker` is the real
/// SUB-D9 probe; this is the unit-test fixture with the same up/down shape.)
#[derive(Clone, Default)]
pub struct HealthTable {
    /// Names explicitly marked DOWN. Absence = up.
    down: Arc<Mutex<BTreeMap<String, ()>>>,
}

impl HealthTable {
    /// A fresh table with everything up.
    pub fn new() -> HealthTable {
        HealthTable::default()
    }

    /// Mark a dependency down (model a severed critical dependency).
    pub fn mark_down(&self, name: &str) {
        self.down
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(name.to_string(), ());
    }

    /// Restore a dependency to up (model the outage healing).
    pub fn mark_up(&self, name: &str) {
        self.down.lock().unwrap_or_else(|e| e.into_inner()).remove(name);
    }
}

impl DependencyHealth for HealthTable {
    fn is_up(&self, dep: &CriticalDependency) -> bool {
        !self.down.lock().unwrap_or_else(|e| e.into_inner()).contains_key(&dep.0)
    }
}

/// The full readiness report (architecture §4.3) — the verdict plus *why*, so the metrics-health
/// surface emits an observable, debuggable answer (observability is part of the pass, EI-01 §3):
/// a not-ready answer names whether it is the startup gate or which critical dependencies are
/// down, never a bare boolean.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadinessReport {
    /// The readiness verdict.
    pub verdict: Readiness,
    /// `true` iff readiness is held down because boot/migration is incomplete (startup gate). A
    /// startup not-ready is *not-killed* (liveness stays up) — a slow boot is not a wedge.
    pub startup_incomplete: bool,
    /// The critical dependencies currently down (the sustained-outage cause, §8.3). Empty when
    /// ready or when not-ready purely for the startup reason.
    pub down_critical: Vec<CriticalDependency>,
}

impl ReadinessReport {
    /// `true` iff the instance must shed new traffic (not-ready → shed, §4.3).
    pub fn sheds(&self) -> bool {
        self.verdict.sheds()
    }

    /// `true` iff the instance is ready.
    pub fn is_ready(&self) -> bool {
        self.verdict.is_ready()
    }
}

/// The metrics-health surface (architecture §4.3) — the third of the three surfaces `serve`
/// opens (alongside the public + internal surfaces in [`crate::topology`]). It exposes the two
/// **independent** probes:
///   - [`Self::liveness`] — "not wedged"; reads ONLY [`LivenessState`]; **never** a dependency.
///   - [`Self::readiness`] — "can serve correct traffic now"; reads the critical-dependency set +
///     the startup gate; a dead critical dependency → not-ready + shed.
///
/// It exports both as the SUB-D9 survival signals (`readiness` gauge + `liveness_restart_count`).
///
/// The structural guarantee that liveness cannot check a dependency is enforced by construction:
/// the liveness path is given the [`LivenessState`] and is NOT given the [`DependencyHealth`] /
/// [`CriticalDependencies`] handles — they live on the readiness path only.
pub struct MetricsHealthSurface<H: DependencyHealth> {
    /// The process's own wedged-ness (the SOLE liveness input).
    liveness: LivenessState,
    /// The declared critical-dependency set (readiness input).
    critical: CriticalDependencies,
    /// The live dependency up/down probe (readiness input).
    health: H,
    /// The startup gate (readiness input; not-ready-not-killed while booting).
    startup: Arc<Mutex<Startup>>,
    /// The count of liveness-triggered restarts the surface has reported (the SUB-D9 "no
    /// restart-storm" signal). A dependency outage NEVER increments this (readiness handles the
    /// outage); it ticks only on a real wedge. Pinned to 0 across an outage by construction —
    /// exposed as a live counter so a regression (a future path that restarted on a dep outage)
    /// would show up as non-zero.
    liveness_restarts: Arc<AtomicU64>,
}

impl<H: DependencyHealth> MetricsHealthSurface<H> {
    /// Open the metrics-health surface over a service's declared critical set + a live health
    /// probe, starting in the **Booting** startup state (not-ready, not-killed) until
    /// [`Self::mark_started`] is called at the end of a successful boot.
    pub fn new(critical: CriticalDependencies, health: H) -> MetricsHealthSurface<H> {
        MetricsHealthSurface {
            liveness: LivenessState::new(),
            critical,
            health,
            startup: Arc::new(Mutex::new(Startup::Booting)),
            liveness_restarts: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The live liveness state handle (so the runtime watchdog can mark the process wedged).
    pub fn liveness_state(&self) -> &LivenessState {
        &self.liveness
    }

    /// Flip the startup gate to **Complete** (boot + migrations succeeded). After this, readiness
    /// is governed purely by the critical-dependency health (the startup gate no longer holds it
    /// down). `serve` calls this at the end of a successful boot.
    pub fn mark_started(&self) {
        *self.startup.lock().unwrap_or_else(|e| e.into_inner()) = Startup::Complete;
    }

    /// The current startup state.
    pub fn startup(&self) -> Startup {
        *self.startup.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// **The liveness probe (architecture §4.3) — "is the process wedged?".** Reads ONLY the
    /// process's own [`LivenessState`]; it is structurally incapable of checking a dependency
    /// (it has no handle to the health probe). A dead critical dependency leaves this `Up` — the
    /// instance is not wedged, so it must NOT be restarted (restarting would not heal the
    /// downstream; it would only restart-storm a healthy fleet).
    ///
    /// **Equivalent-mutant note (cargo-mutants, the M6 gate P-S37):** this method deliberately
    /// ignores `self.health` / `self.critical` — a mutant that *added* a dependency check here
    /// would be the §4.3 bug, caught by `readiness_and_liveness_are_independent` (a severed dep
    /// must leave liveness `Up`). The independence is the property, asserted directly.
    pub fn liveness(&self) -> Liveness {
        self.liveness.liveness()
    }

    /// Record that the orchestrator restarted the process on a liveness failure (the watchdog
    /// detected a wedge). Increments the `liveness_restart_count` SUB-D9 signal. Called ONLY on a
    /// genuine wedge — never on a dependency outage (that is readiness's job). Returns the new
    /// count.
    pub fn record_liveness_restart(&self) -> u64 {
        self.liveness_restarts.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// **The SUB-D9 "no restart-storm" survival signal — `liveness_restart_count`.** The number
    /// of liveness-triggered restarts. Stays at its baseline (`0`) across a dependency outage
    /// because [`Self::liveness`] never checks a dependency; a non-zero value would mean a
    /// dependency outage caused a restart-storm — the §4.3 bug. Exposed as a live counter so that
    /// regression is observable.
    pub fn liveness_restart_count(&self) -> u64 {
        self.liveness_restarts.load(Ordering::SeqCst)
    }

    /// **The readiness probe (architecture §4.3 / §8.3) — "can this instance serve *correct*
    /// traffic now?".** Not-ready (→ shed) iff EITHER the startup gate is still `Booting`
    /// (boot/migration incomplete — not-ready, not-killed) OR any declared **critical**
    /// dependency reads down (a sustained outage — shed, never serve wrong answers). Otherwise
    /// ready. Returns a [`ReadinessReport`] naming the reason (observability is part of the pass).
    ///
    /// Composition with fail-static (§8.3): this surface reads the *current* up/down truth a
    /// probe reports. A *transient* hiccup is absorbed by [`crate::FailStatic`] (P-S18) BEFORE it
    /// reaches "the dependency is down" — only a *sustained* outage reads down here and flips
    /// readiness. The two lanes compose; this is the sustained-outage lane.
    pub fn readiness(&self) -> ReadinessReport {
        let startup_incomplete = matches!(self.startup(), Startup::Booting);

        // Every declared critical dependency that is currently down (the sustained-outage cause).
        let down_critical: Vec<CriticalDependency> = self
            .critical
            .deps()
            .iter()
            .filter(|d| !self.health.is_up(d))
            .cloned()
            .collect();

        let verdict = if startup_incomplete || !down_critical.is_empty() {
            // not-ready → shed (a dead critical dependency or an incomplete boot; never serve
            // healthy-but-failing).
            Readiness::NotReady
        } else {
            Readiness::Ready
        };

        ReadinessReport {
            verdict,
            startup_incomplete,
            down_critical,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surface(critical: &[&str]) -> (MetricsHealthSurface<HealthTable>, HealthTable) {
        let health = HealthTable::new();
        let s = MetricsHealthSurface::new(
            CriticalDependencies::new(critical.iter().copied()),
            health.clone(),
        );
        (s, health)
    }

    /// **Startup = not-ready, not-killed (§4.3).** Before boot completes, readiness is NotReady
    /// (and names the startup reason) AND liveness is Up (a slow boot is NOT a wedge → not killed).
    #[test]
    fn startup_is_not_ready_but_not_killed() {
        let (s, _health) = surface(&["identity"]);
        // before mark_started: booting.
        assert_eq!(s.startup(), Startup::Booting);
        let r = s.readiness();
        assert_eq!(r.verdict, Readiness::NotReady, "an incompletely-booted instance is not-ready");
        assert!(r.startup_incomplete, "the not-ready reason names the startup gate");
        assert!(r.sheds(), "a not-ready instance sheds new traffic");
        // NOT killed: liveness is Up (a slow boot must not be mistaken for a wedge).
        assert_eq!(s.liveness(), Liveness::Up, "startup is not-killed: liveness stays Up");
        assert!(!s.liveness().should_restart(), "the orchestrator must not restart a booting instance");
    }

    /// After boot completes with every critical dependency up, the instance is Ready.
    #[test]
    fn ready_when_booted_and_all_critical_deps_up() {
        let (s, _health) = surface(&["identity", "storage"]);
        s.mark_started();
        let r = s.readiness();
        assert_eq!(r.verdict, Readiness::Ready, "booted + all critical deps up → ready");
        assert!(r.is_ready());
        assert!(r.down_critical.is_empty(), "nothing down");
        assert_eq!(r.verdict.gauge(), 1, "the readiness gauge reads 1 when ready");
    }

    /// **THE SUB-D9 core property — a severed critical dependency flips readiness to not-ready +
    /// sheds, while liveness stays Up (no restart).** The single load-bearing §4.3 assertion.
    #[test]
    fn severed_critical_dep_flips_readiness_but_not_liveness() {
        let (s, health) = surface(&["identity"]);
        s.mark_started();
        // healthy: ready, liveness up.
        assert_eq!(s.readiness().verdict, Readiness::Ready);
        assert_eq!(s.liveness(), Liveness::Up);

        // sever the critical dependency (a sustained outage).
        health.mark_down("identity");

        // readiness FLIPS to not-ready + sheds, and names the down dependency.
        let r = s.readiness();
        assert_eq!(r.verdict, Readiness::NotReady, "a dead critical dependency reports not-ready");
        assert!(r.sheds(), "not-ready → shed new traffic (never serve healthy-but-failing)");
        assert_eq!(
            r.down_critical,
            vec![CriticalDependency::new("identity")],
            "the report names the down critical dependency (observability is part of the pass)"
        );
        assert_eq!(r.verdict.gauge(), 0, "the readiness gauge reads 0 when not-ready");

        // liveness does NOT flip — the process is not wedged; restarting would not heal the dep.
        assert_eq!(s.liveness(), Liveness::Up, "liveness must NOT check the dependency (§4.3)");
        assert!(!s.liveness().should_restart(), "a dead dependency must NOT trigger a restart");
        assert_eq!(s.liveness_restart_count(), 0, "no restart-storm: liveness churn stays 0");
    }

    /// Readiness RECOVERS when the severed dependency heals (the outage lifting → ready again),
    /// proving the flip is driven by the live truth, not a sticky latch.
    #[test]
    fn readiness_recovers_when_dependency_heals() {
        let (s, health) = surface(&["identity"]);
        s.mark_started();
        health.mark_down("identity");
        assert_eq!(s.readiness().verdict, Readiness::NotReady);
        // heal the dependency.
        health.mark_up("identity");
        assert_eq!(s.readiness().verdict, Readiness::Ready, "readiness recovers when the dep heals");
    }

    /// **Liveness does NOT check dependencies (§4.3) — the independence property.** Across a dead
    /// critical dependency AND a healed one, liveness is unchanged (`Up`); only a genuine wedge
    /// flips it. (This is what stops the restart-storm: a downstream blip never makes a healthy
    /// replica fail its liveness probe.)
    #[test]
    fn readiness_and_liveness_are_independent() {
        let (s, health) = surface(&["identity", "storage"]);
        s.mark_started();

        // dead critical deps → readiness not-ready, liveness untouched.
        health.mark_down("identity");
        health.mark_down("storage");
        assert_eq!(s.readiness().verdict, Readiness::NotReady);
        assert_eq!(s.liveness(), Liveness::Up, "two dead deps still leave liveness Up");

        // a genuine wedge (the ONLY thing that flips liveness) → liveness Wedged, restart.
        s.liveness_state().mark_wedged();
        assert_eq!(s.liveness(), Liveness::Wedged, "a real wedge flips liveness");
        assert!(s.liveness().should_restart(), "a wedged process IS restarted");

        // healing the deps does not un-wedge the process (the two are independent).
        health.mark_up("identity");
        health.mark_up("storage");
        assert_eq!(s.liveness(), Liveness::Wedged, "liveness is independent of dependency health");
    }

    /// A NON-critical dependency being down does NOT flip readiness (only the declared *critical*
    /// set does). The service still serves correct traffic for everything else.
    #[test]
    fn non_critical_dependency_down_does_not_flip_readiness() {
        let (s, health) = surface(&["identity"]); // only identity is critical
        s.mark_started();
        // a dependency NOT in the critical set goes down.
        health.mark_down("search");
        assert_eq!(
            s.readiness().verdict,
            Readiness::Ready,
            "a down non-critical dependency does not flip readiness"
        );
    }

    /// The `liveness_restart_count` ticks ONLY on a recorded wedge-restart, never on a dependency
    /// outage — the SUB-D9 "no restart-storm" signal is real (it would catch a regression that
    /// restarted on a dep outage).
    #[test]
    fn liveness_restart_count_ticks_only_on_a_real_restart() {
        let (s, health) = surface(&["identity"]);
        s.mark_started();
        // an outage does NOT increment the restart count.
        health.mark_down("identity");
        let _ = s.readiness();
        assert_eq!(s.liveness_restart_count(), 0, "a dependency outage causes no restart");
        // a genuine wedge-restart increments it.
        s.liveness_state().mark_wedged();
        assert_eq!(s.record_liveness_restart(), 1);
        assert_eq!(s.liveness_restart_count(), 1, "a real wedge-restart is counted");
    }
}
