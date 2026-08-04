use myelin_tenancy::TenantId;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Dependency {
    Identity,
    Broker,
    Kms,
    DbReplica,
    Firehose,
    Downstream(String),
    Named(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Scope {
    Global,
    Tenant(TenantId),
    Cell(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BreakOutcome {
    Changed,
    NoChange,
}

impl BreakOutcome {
    pub fn changed(self) -> bool {
        matches!(self, BreakOutcome::Changed)
    }
}

#[derive(Clone, Default)]
pub struct DependencyBreaker {
    broken: Arc<Mutex<HashSet<(Dependency, Scope)>>>,
}

impl DependencyBreaker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn break_dependency(&self, dependency: Dependency, scope: Scope) -> BreakOutcome {
        let inserted = self.lock().insert((dependency, scope));
        if inserted {
            BreakOutcome::Changed
        } else {
            BreakOutcome::NoChange
        }
    }

    pub fn restore_dependency(&self, dependency: Dependency, scope: Scope) -> BreakOutcome {
        let removed = self.lock().remove(&(dependency, scope));
        if removed {
            BreakOutcome::Changed
        } else {
            BreakOutcome::NoChange
        }
    }

    pub fn is_broken(&self, dependency: &Dependency, scope: &Scope) -> bool {
        let broken = self.lock();
        if broken.contains(&(dependency.clone(), scope.clone())) {
            return true;
        }
        match scope {
            Scope::Global => false,
            Scope::Tenant(_) | Scope::Cell(_) => {
                broken.contains(&(dependency.clone(), Scope::Global))
            }
        }
    }

    pub fn broken_count(&self) -> usize {
        self.lock().len()
    }

    pub fn restore_all(&self) {
        self.lock().clear();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashSet<(Dependency, Scope)>> {
        self.broken.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant(s: &str) -> Scope {
        Scope::Tenant(TenantId(s.to_string()))
    }

    #[test]
    fn break_then_restore_is_fully_reversible() {
        let breaker = DependencyBreaker::new();
        let scope = tenant("acme");

        assert!(!breaker.is_broken(&Dependency::Identity, &scope));
        assert_eq!(breaker.broken_count(), 0);

        assert_eq!(
            breaker.break_dependency(Dependency::Identity, scope.clone()),
            BreakOutcome::Changed
        );
        assert!(breaker.is_broken(&Dependency::Identity, &scope));
        assert_eq!(breaker.broken_count(), 1);

        assert_eq!(
            breaker.restore_dependency(Dependency::Identity, scope.clone()),
            BreakOutcome::Changed
        );
        assert!(!breaker.is_broken(&Dependency::Identity, &scope));
        assert_eq!(breaker.broken_count(), 0);
    }

    #[test]
    fn break_is_scoped_to_its_named_dependency() {
        let breaker = DependencyBreaker::new();
        let scope = tenant("acme");

        breaker.break_dependency(Dependency::Identity, scope.clone());

        assert!(breaker.is_broken(&Dependency::Identity, &scope));
        assert!(!breaker.is_broken(&Dependency::Broker, &scope));
        assert!(!breaker.is_broken(&Dependency::Kms, &scope));
    }

    #[test]
    fn break_is_scoped_to_its_named_scope() {
        let breaker = DependencyBreaker::new();
        let acme = tenant("acme");
        let globex = tenant("globex");

        breaker.break_dependency(Dependency::Identity, acme.clone());

        assert!(breaker.is_broken(&Dependency::Identity, &acme));
        assert!(!breaker.is_broken(&Dependency::Identity, &globex));
    }

    #[test]
    fn double_break_and_double_restore_are_noops() {
        let breaker = DependencyBreaker::new();
        let scope = tenant("acme");

        assert_eq!(
            breaker.break_dependency(Dependency::Broker, scope.clone()),
            BreakOutcome::Changed
        );
        assert_eq!(
            breaker.break_dependency(Dependency::Broker, scope.clone()),
            BreakOutcome::NoChange
        );
        assert_eq!(breaker.broken_count(), 1);
        assert!(breaker.is_broken(&Dependency::Broker, &scope));

        assert_eq!(
            breaker.restore_dependency(Dependency::Broker, scope.clone()),
            BreakOutcome::Changed
        );
        assert_eq!(
            breaker.restore_dependency(Dependency::Broker, scope.clone()),
            BreakOutcome::NoChange
        );
        assert_eq!(breaker.broken_count(), 0);
        assert!(!breaker.is_broken(&Dependency::Broker, &scope));
    }

    #[test]
    fn restore_of_never_broken_is_a_noop() {
        let breaker = DependencyBreaker::new();
        assert_eq!(
            breaker.restore_dependency(Dependency::Kms, Scope::Global),
            BreakOutcome::NoChange
        );
        assert_eq!(breaker.broken_count(), 0);
    }

    #[test]
    fn global_break_is_seen_by_narrower_consults_but_not_vice_versa() {
        let breaker = DependencyBreaker::new();

        breaker.break_dependency(Dependency::Identity, Scope::Global);
        assert!(breaker.is_broken(&Dependency::Identity, &Scope::Global));
        assert!(breaker.is_broken(&Dependency::Identity, &tenant("acme")));
        assert!(breaker.is_broken(&Dependency::Identity, &Scope::Cell("eu-1".to_string())));
        assert!(!breaker.is_broken(&Dependency::Broker, &tenant("acme")));

        breaker.restore_dependency(Dependency::Identity, Scope::Global);
        assert!(!breaker.is_broken(&Dependency::Identity, &tenant("acme")));

        breaker.break_dependency(Dependency::Identity, tenant("acme"));
        assert!(breaker.is_broken(&Dependency::Identity, &tenant("acme")));
        assert!(!breaker.is_broken(&Dependency::Identity, &Scope::Global));
        assert!(!breaker.is_broken(&Dependency::Identity, &tenant("globex")));
    }

    #[test]
    fn distinct_named_downstreams_are_independent() {
        let breaker = DependencyBreaker::new();
        let scope = tenant("acme");
        let a = Dependency::Downstream("billing".to_string());
        let b = Dependency::Downstream("search".to_string());

        breaker.break_dependency(a.clone(), scope.clone());
        assert!(breaker.is_broken(&a, &scope));
        assert!(!breaker.is_broken(&b, &scope));
    }

    #[test]
    fn handle_clone_shares_state_and_restore_all_drains() {
        let driver = DependencyBreaker::new();
        let fault_point = driver.clone();

        driver.break_dependency(Dependency::Broker, tenant("acme"));
        driver.break_dependency(Dependency::Identity, Scope::Global);

        assert!(fault_point.is_broken(&Dependency::Broker, &tenant("acme")));
        assert!(fault_point.is_broken(&Dependency::Identity, &tenant("acme")));
        assert_eq!(fault_point.broken_count(), 2);

        driver.restore_all();
        assert_eq!(fault_point.broken_count(), 0);
        assert!(!fault_point.is_broken(&Dependency::Broker, &tenant("acme")));
        assert!(!fault_point.is_broken(&Dependency::Identity, &tenant("acme")));
    }
}
