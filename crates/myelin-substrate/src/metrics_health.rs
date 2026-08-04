use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Liveness {
    Up,
    Wedged,
}

impl Liveness {
    pub fn should_restart(self) -> bool {
        matches!(self, Liveness::Wedged)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Readiness {
    Ready,
    NotReady,
}

impl Readiness {
    pub fn is_ready(self) -> bool {
        matches!(self, Readiness::Ready)
    }

    pub fn sheds(self) -> bool {
        matches!(self, Readiness::NotReady)
    }

    pub fn gauge(self) -> i64 {
        match self {
            Readiness::Ready => 1,
            Readiness::NotReady => 0,
        }
    }
}

#[derive(Clone, Default)]
pub struct LivenessState {
    wedged: Arc<std::sync::atomic::AtomicBool>,
}

impl LivenessState {
    pub fn new() -> LivenessState {
        LivenessState::default()
    }

    pub fn mark_wedged(&self) {
        self.wedged.store(true, Ordering::SeqCst);
    }

    pub fn liveness(&self) -> Liveness {
        if self.wedged.load(Ordering::SeqCst) {
            Liveness::Wedged
        } else {
            Liveness::Up
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Startup {
    #[default]
    Booting,
    Complete,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CriticalDependency(pub String);

impl CriticalDependency {
    pub fn new(name: impl Into<String>) -> CriticalDependency {
        CriticalDependency(name.into())
    }
}

#[derive(Clone, Debug, Default)]
pub struct CriticalDependencies {
    deps: Vec<CriticalDependency>,
}

impl CriticalDependencies {
    pub fn new(names: impl IntoIterator<Item = impl Into<String>>) -> CriticalDependencies {
        CriticalDependencies {
            deps: names
                .into_iter()
                .map(|n| CriticalDependency::new(n))
                .collect(),
        }
    }

    pub fn deps(&self) -> &[CriticalDependency] {
        &self.deps
    }
}

pub trait DependencyHealth: Send + Sync {
    fn is_up(&self, dep: &CriticalDependency) -> bool;
}

#[derive(Clone, Default)]
pub struct HealthTable {
    down: Arc<Mutex<BTreeMap<String, ()>>>,
}

impl HealthTable {
    pub fn new() -> HealthTable {
        HealthTable::default()
    }

    pub fn mark_down(&self, name: &str) {
        self.down
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(name.to_string(), ());
    }

    pub fn mark_up(&self, name: &str) {
        self.down
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(name);
    }
}

impl DependencyHealth for HealthTable {
    fn is_up(&self, dep: &CriticalDependency) -> bool {
        !self
            .down
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&dep.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadinessReport {
    pub verdict: Readiness,
    pub startup_incomplete: bool,
    pub down_critical: Vec<CriticalDependency>,
}

impl ReadinessReport {
    pub fn sheds(&self) -> bool {
        self.verdict.sheds()
    }

    pub fn is_ready(&self) -> bool {
        self.verdict.is_ready()
    }
}

pub struct MetricsHealthSurface<H: DependencyHealth> {
    liveness: LivenessState,
    critical: CriticalDependencies,
    health: H,
    startup: Arc<Mutex<Startup>>,
    liveness_restarts: Arc<AtomicU64>,
}

impl<H: DependencyHealth> MetricsHealthSurface<H> {
    pub fn new(critical: CriticalDependencies, health: H) -> MetricsHealthSurface<H> {
        MetricsHealthSurface {
            liveness: LivenessState::new(),
            critical,
            health,
            startup: Arc::new(Mutex::new(Startup::Booting)),
            liveness_restarts: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn liveness_state(&self) -> &LivenessState {
        &self.liveness
    }

    pub fn mark_started(&self) {
        *self.startup.lock().unwrap_or_else(|e| e.into_inner()) = Startup::Complete;
    }

    pub fn startup(&self) -> Startup {
        *self.startup.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn liveness(&self) -> Liveness {
        self.liveness.liveness()
    }

    pub fn record_liveness_restart(&self) -> u64 {
        self.liveness_restarts.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn liveness_restart_count(&self) -> u64 {
        self.liveness_restarts.load(Ordering::SeqCst)
    }

    pub fn readiness(&self) -> ReadinessReport {
        let startup_incomplete = matches!(self.startup(), Startup::Booting);

        let down_critical: Vec<CriticalDependency> = self
            .critical
            .deps()
            .iter()
            .filter(|d| !self.health.is_up(d))
            .cloned()
            .collect();

        let verdict = if startup_incomplete || !down_critical.is_empty() {
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

    #[test]
    fn startup_is_not_ready_but_not_killed() {
        let (s, _health) = surface(&["identity"]);
        assert_eq!(s.startup(), Startup::Booting);
        let r = s.readiness();
        assert_eq!(
            r.verdict,
            Readiness::NotReady,
            "an incompletely-booted instance is not-ready"
        );
        assert!(
            r.startup_incomplete,
            "the not-ready reason names the startup gate"
        );
        assert!(r.sheds(), "a not-ready instance sheds new traffic");
        assert_eq!(
            s.liveness(),
            Liveness::Up,
            "startup is not-killed: liveness stays Up"
        );
        assert!(
            !s.liveness().should_restart(),
            "the orchestrator must not restart a booting instance"
        );
    }

    #[test]
    fn ready_when_booted_and_all_critical_deps_up() {
        let (s, _health) = surface(&["identity", "storage"]);
        s.mark_started();
        let r = s.readiness();
        assert_eq!(
            r.verdict,
            Readiness::Ready,
            "booted + all critical deps up → ready"
        );
        assert!(r.is_ready());
        assert!(r.down_critical.is_empty(), "nothing down");
        assert_eq!(
            r.verdict.gauge(),
            1,
            "the readiness gauge reads 1 when ready"
        );
    }

    #[test]
    fn severed_critical_dep_flips_readiness_but_not_liveness() {
        let (s, health) = surface(&["identity"]);
        s.mark_started();
        assert_eq!(s.readiness().verdict, Readiness::Ready);
        assert_eq!(s.liveness(), Liveness::Up);

        health.mark_down("identity");

        let r = s.readiness();
        assert_eq!(
            r.verdict,
            Readiness::NotReady,
            "a dead critical dependency reports not-ready"
        );
        assert!(
            r.sheds(),
            "not-ready → shed new traffic (never serve healthy-but-failing)"
        );
        assert_eq!(
            r.down_critical,
            vec![CriticalDependency::new("identity")],
            "the report names the down critical dependency (observability is part of the pass)"
        );
        assert_eq!(
            r.verdict.gauge(),
            0,
            "the readiness gauge reads 0 when not-ready"
        );

        assert_eq!(
            s.liveness(),
            Liveness::Up,
            "liveness must NOT check the dependency (§4.3)"
        );
        assert!(
            !s.liveness().should_restart(),
            "a dead dependency must NOT trigger a restart"
        );
        assert_eq!(
            s.liveness_restart_count(),
            0,
            "no restart-storm: liveness churn stays 0"
        );
    }

    #[test]
    fn readiness_recovers_when_dependency_heals() {
        let (s, health) = surface(&["identity"]);
        s.mark_started();
        health.mark_down("identity");
        assert_eq!(s.readiness().verdict, Readiness::NotReady);
        health.mark_up("identity");
        assert_eq!(
            s.readiness().verdict,
            Readiness::Ready,
            "readiness recovers when the dep heals"
        );
    }

    #[test]
    fn readiness_and_liveness_are_independent() {
        let (s, health) = surface(&["identity", "storage"]);
        s.mark_started();

        health.mark_down("identity");
        health.mark_down("storage");
        assert_eq!(s.readiness().verdict, Readiness::NotReady);
        assert_eq!(
            s.liveness(),
            Liveness::Up,
            "two dead deps still leave liveness Up"
        );

        s.liveness_state().mark_wedged();
        assert_eq!(
            s.liveness(),
            Liveness::Wedged,
            "a real wedge flips liveness"
        );
        assert!(
            s.liveness().should_restart(),
            "a wedged process IS restarted"
        );

        health.mark_up("identity");
        health.mark_up("storage");
        assert_eq!(
            s.liveness(),
            Liveness::Wedged,
            "liveness is independent of dependency health"
        );
    }

    #[test]
    fn non_critical_dependency_down_does_not_flip_readiness() {
        let (s, health) = surface(&["identity"]);
        s.mark_started();
        health.mark_down("search");
        assert_eq!(
            s.readiness().verdict,
            Readiness::Ready,
            "a down non-critical dependency does not flip readiness"
        );
    }

    #[test]
    fn liveness_restart_count_ticks_only_on_a_real_restart() {
        let (s, health) = surface(&["identity"]);
        s.mark_started();
        health.mark_down("identity");
        let _ = s.readiness();
        assert_eq!(
            s.liveness_restart_count(),
            0,
            "a dependency outage causes no restart"
        );
        s.liveness_state().mark_wedged();
        assert_eq!(s.record_liveness_restart(), 1);
        assert_eq!(
            s.liveness_restart_count(),
            1,
            "a real wedge-restart is counted"
        );
    }
}
