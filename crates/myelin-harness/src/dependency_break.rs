//! The scoped-reversible dependency-break injector (T-3).
//!
//! See the crate-level docs for the doctrine / architecture / testing-strategy anchors.
//! This module is the **failure-injection half** of the unit-of-proof: the seam every
//! later drill rides to force ONE dependency to fail for ONE scope without taking the rig
//! down (doctrine EI-01 §3; architecture §11 intro + §11 drill table; testing-strategy
//! §3.2). The load generator (P-S02) drives traffic; THIS injector severs a dependency;
//! the telemetry-assertion library (P-S04) reads that the system survived. Together they
//! are the "inject one fault → drive one unit of load → read one green assertion"
//! self-test the SUB-M0 exit owes.
//!
//! ## The three guarantees this prompt's gate proves
//! - **Reversible.** [`DependencyBreaker::break_dependency`] severs a dependency;
//!   [`DependencyBreaker::restore_dependency`] returns it to a fully working state. A
//!   restored dependency is indistinguishable from one that was never broken — there is no
//!   residual "half-broken" state (testing-strategy §3.2: "the break is lifted and the
//!   system observed recovering").
//! - **Scoped.** A break names BOTH a [`Dependency`] AND a [`Scope`]
//!   (blast-radius-limited to the drill's tenant/cell — testing-strategy §3.2). Breaking
//!   Identity for tenant `acme` leaves the broker up, and leaves Identity up for tenant
//!   `globex`. A drill consults [`DependencyBreaker::is_broken`] with the *exact* dependency
//!   + scope it is exercising, so an unrelated dependency or scope is never collateral.
//! - **Idempotent.** A double-break or double-restore is a no-op: the second call observes
//!   the state is already what it asks for and returns [`BreakOutcome::NoChange`] (vs
//!   [`BreakOutcome::Changed`] for the transition). Idempotence is observable so a test can
//!   assert it (rather than being a silent swallow — EI-01 §3).
//!
//! ## Why a *consult* seam and not a real socket-killer
//! A drill at this layer cannot literally `kill -9` a process: the rig is in-memory and
//! `serve` does not exist yet (it lands at P-S12). The injector instead models the break as
//! shared, queryable state: the fault-point in a future `serve`/client (the relay's publish
//! step, the `AuthzClient::check` call, a downstream RPC) consults `is_broken(dep, scope)`
//! and, when broken, fails exactly as the real severance would (the relay can't publish, the
//! check times out, the RPC errors). This is the **T-3 seam**: one place every drill points
//! its fault at, reversible and scoped by construction. When `serve` and the real clients
//! land (P-S07 relay, P-S13 surfaces, P-S16 `ResilientClient`), each wires its fault-point
//! to this consult; the seam shape does not change.
//!
//! **Floor named (deferred + filling prompt).** This prompt ships the injector machinery
//! (break/restore/consult) and proves its reversibility + scoping + idempotence in
//! isolation. It does NOT yet drive a survival drill — there is no telemetry to assert
//! against until the **telemetry-assertion library (P-S04)**, and no real fault-point to
//! sever until **the outbox relay (P-S07)**, **the three-port `serve` topology
//! (P-S12/P-S13)**, and **the `ResilientClient` (P-S16)** land. The named drills this seam
//! will drive — SUB-D2 (sever the broker mid-stream), SUB-D4 (hard-down Identity), SUB-D5
//! (trip a downstream) — are listed on each [`Dependency`] variant and are wired at those
//! later prompts (testing-strategy §4.2).

use myelin_tenancy::TenantId;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

/// A named dependency the injector can sever (testing-strategy §3.2: "kill a service
/// between commit and publish, sever the broker mid-stream, fail-over a DB replica
/// mid-merge, hard-down Identity, hard-down KMS, drop a firehose connection, trip a
/// downstream breaker, corrupt one blob object").
///
/// The canonical deps the substrate's own drills name get a dedicated variant so a drill
/// reads as the property it proves; an arbitrary out-of-tree dependency a subsystem drill
/// needs (a specific downstream service, a custom seam) is expressible via
/// [`Dependency::Named`] without a new variant — the catalogue grows without re-freezing
/// this enum (the every-incident-adds-a-drill loop, EI-01 §5, must not require an enum edit
/// to add a drill).
///
/// Held as cloneable, hashable value data so it is a map key in the breaker's set and a
/// telemetry/trace label without carrying PII (`control-plane-pii-free` discipline).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Dependency {
    /// Identity / authz (`AuthzClient`). Severing it drives the fail-static / Id-hiccup
    /// family — **SUB-D4** (already-authenticated survives on the bounded-staleness cache;
    /// a revoked actor is denied once the window closes). Wired at P-S25 over the real
    /// fail-static cache.
    Identity,
    /// The event broker (the bus transport the outbox relay publishes to). Severing it
    /// mid-stream drives **SUB-D2 / BUS-D1** (0 lost + 0 duplicate across a reconnect;
    /// bind-by-name + dedup). Wired at P-S07 (relay) / P-S08 (consumer).
    Broker,
    /// A KMS / key-management dependency (hard-down KMS — the envelope-encryption /
    /// fail-static-availability posture). Wired at Storage's M1 KMS prompts.
    Kms,
    /// A read replica (fail-over a DB replica mid-merge — the read-replica-awareness leg of
    /// the bounded pool, architecture §3.3). Wired at P-S12 (`serve` pool).
    DbReplica,
    /// A firehose subscription (drop a firehose connection — the **D-11** resume-cursor
    /// reconnect-loses-zero-ops drill). Wired at P-S28/P-S29 + the firehose protocol.
    Firehose,
    /// A generic downstream service reached over the `ResilientClient` (trip a downstream
    /// breaker under load — **SUB-D5**, callers fail fast + honour `Retry-After`, no
    /// amplification). Wired at P-S16/P-S17. The `String` names the specific downstream so
    /// two downstreams are independently breakable.
    Downstream(String),
    /// Any other named dependency a drill needs (the escape hatch — see the type docs).
    /// The `String` is the dependency's stable name; two distinct names are independent.
    Named(String),
}

/// The blast-radius scope of a break (testing-strategy §3.2: "scoped =
/// blast-radius-limited to the drill's tenant/cell").
///
/// A break is keyed by `(Dependency, Scope)`: breaking `Identity` for `Tenant(acme)` does
/// not touch `Identity` for `Tenant(globex)` or for `Cell(eu-1)`. [`Scope::Global`] is the
/// rig-wide scope (hard-down the dependency everywhere) for the drills that genuinely take a
/// shared dependency offline; it is deliberately a *distinct* key from any per-tenant scope,
/// so a global break does not silently subsume — or get subsumed by — a tenant break
/// (each scope is severed and restored on its own; see [`DependencyBreaker::is_broken`] for
/// how a consult resolves the two).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Scope {
    /// Rig-wide: the dependency is hard-down everywhere (the "hard-down Identity" /
    /// "hard-down KMS" shared-dependency drills).
    Global,
    /// Limited to one tenant's blast radius (the common case — a drill severs a dependency
    /// for ONE tenant and asserts other tenants are unaffected, the multi-tenant-isolation
    /// half of every surge/hiccup drill).
    Tenant(TenantId),
    /// Limited to one named cell (the cell is the coarser blast-radius unit; the
    /// cell-kill / cell-provisioning drills, STOR-D2 / CP-D6).
    Cell(String),
}

/// The result of a break/restore call — whether it changed state, used to PROVE idempotence
/// (a double-break / double-restore is a no-op).
///
/// Idempotence is made *observable* rather than silent (EI-01 §3: a property is proven by a
/// test that can read the outcome, never a swallowed pass): the first break of a fresh
/// `(dep, scope)` is [`BreakOutcome::Changed`]; an immediate second break of the same pair
/// is [`BreakOutcome::NoChange`]. The same holds for restore.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BreakOutcome {
    /// The call transitioned state (a fresh break severed a working dep; a restore healed a
    /// broken one).
    Changed,
    /// The call was a no-op — the `(dep, scope)` was already in the requested state (the
    /// double-break / double-restore idempotence guarantee).
    NoChange,
}

impl BreakOutcome {
    /// `true` iff this call actually transitioned state. Convenience for asserting the
    /// reversibility/idempotence guarantees in a drill.
    pub fn changed(self) -> bool {
        matches!(self, BreakOutcome::Changed)
    }
}

/// The scoped-reversible dependency-break injector — the T-3 seam.
///
/// A cloneable handle over a shared set of currently-broken `(Dependency, Scope)` pairs. It
/// is `Clone` + `Send` + `Sync` (an `Arc<Mutex<…>>` inside) so the SAME injector can be
/// handed to the rig's fault-points (a future `serve`'s relay, an `AuthzClient`, a
/// downstream client) AND to the drill driver that breaks/restores — both observe one shared
/// truth. Cloning the handle shares the state; it does not copy it.
///
/// The set holds only the pairs that are *currently broken*; a working dependency is the
/// absence of its pair (so "everything works" is the empty set — the natural default and the
/// clean fully-restored state).
#[derive(Clone, Default)]
pub struct DependencyBreaker {
    broken: Arc<Mutex<HashSet<(Dependency, Scope)>>>,
}

impl DependencyBreaker {
    /// A fresh injector with every dependency working (nothing broken).
    pub fn new() -> Self {
        Self::default()
    }

    /// Sever ONE named `dependency` for ONE named `scope`, without affecting any other
    /// dependency or any other scope.
    ///
    /// Returns [`BreakOutcome::Changed`] if this transitioned a working dependency to broken,
    /// or [`BreakOutcome::NoChange`] if it was already broken for this exact pair (the
    /// double-break no-op — the idempotence guarantee, observably).
    pub fn break_dependency(&self, dependency: Dependency, scope: Scope) -> BreakOutcome {
        let inserted = self.lock().insert((dependency, scope));
        if inserted {
            BreakOutcome::Changed
        } else {
            BreakOutcome::NoChange
        }
    }

    /// Restore ONE named `dependency` for ONE named `scope` to a fully working state.
    ///
    /// Returns [`BreakOutcome::Changed`] if this healed a broken dependency, or
    /// [`BreakOutcome::NoChange`] if it was already working for this exact pair (the
    /// double-restore no-op). A restored pair leaves no residual state — the set returns to
    /// exactly the shape it had before the matching break (reversibility).
    pub fn restore_dependency(&self, dependency: Dependency, scope: Scope) -> BreakOutcome {
        let removed = self.lock().remove(&(dependency, scope));
        if removed {
            BreakOutcome::Changed
        } else {
            BreakOutcome::NoChange
        }
    }

    /// The consult a rig fault-point makes: is `dependency` broken for the work happening in
    /// `scope` right now?
    ///
    /// Resolution rule (so scoping is unambiguous at the fault-point): the dependency is
    /// broken for `scope` iff EITHER an exact `(dependency, scope)` break is in effect, OR a
    /// [`Scope::Global`] break of that dependency is in effect. A global break is, by
    /// definition, "the dependency is down everywhere", so a per-tenant/per-cell consult must
    /// see it; a per-tenant break, by contrast, is invisible to a *different* tenant's
    /// consult (that is the whole point of scoping). A `Global` consult sees only `Global`
    /// breaks (a per-tenant break does not take the dependency down globally).
    pub fn is_broken(&self, dependency: &Dependency, scope: &Scope) -> bool {
        let broken = self.lock();
        // exact match (covers the Global-scope consult against a Global break too)
        if broken.contains(&(dependency.clone(), scope.clone())) {
            return true;
        }
        // a narrower (tenant/cell) consult also sees a Global break of the same dependency.
        match scope {
            Scope::Global => false,
            Scope::Tenant(_) | Scope::Cell(_) => {
                broken.contains(&(dependency.clone(), Scope::Global))
            }
        }
    }

    /// The number of `(dependency, scope)` pairs currently severed. `0` is the
    /// fully-working / fully-restored state. Used by tests/telemetry to assert the rig
    /// returned to clean after a drill (no leaked breaks across drills).
    pub fn broken_count(&self) -> usize {
        self.lock().len()
    }

    /// Restore EVERY break in one call — the drill teardown convenience so a drill cannot
    /// leak a severed dependency into the next test (an undrained break is a cross-drill
    /// contamination bug; this makes a clean reset one call).
    pub fn restore_all(&self) {
        self.lock().clear();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashSet<(Dependency, Scope)>> {
        // The harness is test-support; a poisoned lock means a test panicked while holding
        // it, which is itself a failure we want to surface loudly (not swallow) — so we
        // recover the guard and let the originating panic stand rather than masking it.
        self.broken.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant(s: &str) -> Scope {
        Scope::Tenant(TenantId(s.to_string()))
    }

    /// GATE (reversibility): break → severed, restore → fully working again, with no
    /// residual state. This is the prompt's named green artifact.
    #[test]
    fn break_then_restore_is_fully_reversible() {
        let breaker = DependencyBreaker::new();
        let scope = tenant("acme");

        // starts working
        assert!(!breaker.is_broken(&Dependency::Identity, &scope));
        assert_eq!(breaker.broken_count(), 0);

        // break → severed
        assert_eq!(
            breaker.break_dependency(Dependency::Identity, scope.clone()),
            BreakOutcome::Changed
        );
        assert!(breaker.is_broken(&Dependency::Identity, &scope));
        assert_eq!(breaker.broken_count(), 1);

        // restore → fully working again, no residual state (count back to 0)
        assert_eq!(
            breaker.restore_dependency(Dependency::Identity, scope.clone()),
            BreakOutcome::Changed
        );
        assert!(!breaker.is_broken(&Dependency::Identity, &scope));
        assert_eq!(breaker.broken_count(), 0);
    }

    /// GATE (scoping — across dependencies): breaking ONE dependency leaves an unrelated
    /// dependency up in the same scope.
    #[test]
    fn break_is_scoped_to_its_named_dependency() {
        let breaker = DependencyBreaker::new();
        let scope = tenant("acme");

        breaker.break_dependency(Dependency::Identity, scope.clone());

        // the broken one is severed...
        assert!(breaker.is_broken(&Dependency::Identity, &scope));
        // ...but the broker (a different dependency, same scope) stays up.
        assert!(!breaker.is_broken(&Dependency::Broker, &scope));
        assert!(!breaker.is_broken(&Dependency::Kms, &scope));
    }

    /// GATE (scoping — across scopes): breaking a dependency for one tenant leaves the SAME
    /// dependency up for a different tenant (the multi-tenant blast-radius guarantee).
    #[test]
    fn break_is_scoped_to_its_named_scope() {
        let breaker = DependencyBreaker::new();
        let acme = tenant("acme");
        let globex = tenant("globex");

        breaker.break_dependency(Dependency::Identity, acme.clone());

        assert!(breaker.is_broken(&Dependency::Identity, &acme));
        assert!(!breaker.is_broken(&Dependency::Identity, &globex));
    }

    /// GATE (idempotence): a double-break and a double-restore are each a no-op, and the
    /// no-op is observable ([`BreakOutcome::NoChange`]).
    #[test]
    fn double_break_and_double_restore_are_noops() {
        let breaker = DependencyBreaker::new();
        let scope = tenant("acme");

        // first break changes state; second is a no-op
        assert_eq!(
            breaker.break_dependency(Dependency::Broker, scope.clone()),
            BreakOutcome::Changed
        );
        assert_eq!(
            breaker.break_dependency(Dependency::Broker, scope.clone()),
            BreakOutcome::NoChange
        );
        // a double-break did not "double-sever": still exactly one broken pair
        assert_eq!(breaker.broken_count(), 1);
        assert!(breaker.is_broken(&Dependency::Broker, &scope));

        // first restore changes state; second is a no-op
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

    /// Restoring a dependency that was never broken is a clean no-op (not an error / not a
    /// panic) — the injector tolerates an over-eager teardown.
    #[test]
    fn restore_of_never_broken_is_a_noop() {
        let breaker = DependencyBreaker::new();
        assert_eq!(
            breaker.restore_dependency(Dependency::Kms, Scope::Global),
            BreakOutcome::NoChange
        );
        assert_eq!(breaker.broken_count(), 0);
    }

    /// A `Global` break takes the dependency down for every narrower consult (tenant/cell),
    /// but a per-tenant break does NOT take it down globally — the asymmetric scoping rule.
    #[test]
    fn global_break_is_seen_by_narrower_consults_but_not_vice_versa() {
        let breaker = DependencyBreaker::new();

        // Global break → every tenant/cell consult sees Identity down.
        breaker.break_dependency(Dependency::Identity, Scope::Global);
        assert!(breaker.is_broken(&Dependency::Identity, &Scope::Global));
        assert!(breaker.is_broken(&Dependency::Identity, &tenant("acme")));
        assert!(breaker.is_broken(&Dependency::Identity, &Scope::Cell("eu-1".to_string())));
        // a different dependency is still up everywhere
        assert!(!breaker.is_broken(&Dependency::Broker, &tenant("acme")));

        breaker.restore_dependency(Dependency::Identity, Scope::Global);
        assert!(!breaker.is_broken(&Dependency::Identity, &tenant("acme")));

        // Per-tenant break → NOT seen by a Global consult (it is not down everywhere).
        breaker.break_dependency(Dependency::Identity, tenant("acme"));
        assert!(breaker.is_broken(&Dependency::Identity, &tenant("acme")));
        assert!(!breaker.is_broken(&Dependency::Identity, &Scope::Global));
        assert!(!breaker.is_broken(&Dependency::Identity, &tenant("globex")));
    }

    /// Two `Downstream(name)` / `Named(name)` dependencies are independent: breaking one
    /// does not sever the other (the catalogue's escape-hatch deps stay isolated).
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

    /// A clone of the handle shares state — the fault-point and the drill driver see the
    /// SAME breaks (this is what makes the injector usable as the shared T-3 seam). And
    /// `restore_all` drains every break for a clean teardown.
    #[test]
    fn handle_clone_shares_state_and_restore_all_drains() {
        let driver = DependencyBreaker::new();
        let fault_point = driver.clone(); // what the rig holds

        driver.break_dependency(Dependency::Broker, tenant("acme"));
        driver.break_dependency(Dependency::Identity, Scope::Global);

        // the rig's clone observes the breaks the driver made
        assert!(fault_point.is_broken(&Dependency::Broker, &tenant("acme")));
        assert!(fault_point.is_broken(&Dependency::Identity, &tenant("acme")));
        assert_eq!(fault_point.broken_count(), 2);

        // teardown drains everything, observed through either handle
        driver.restore_all();
        assert_eq!(fault_point.broken_count(), 0);
        assert!(!fault_point.is_broken(&Dependency::Broker, &tenant("acme")));
        assert!(!fault_point.is_broken(&Dependency::Identity, &tenant("acme")));
    }
}
