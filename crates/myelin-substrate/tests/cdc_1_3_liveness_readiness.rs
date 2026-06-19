//! # CDC 1.3 — liveness ≠ readiness on the lifecycle-opened metrics-health surface (P-S14 → P-031)
//!
//! **Contract-index:** row 1.3 (`Liveness ≠ readiness`). This consumer-driven contract test
//! exercises the 1.3 shape THROUGH the `serve` lifecycle (not just the unit module): a service
//! boots from `serve`'s `boot`, the three surfaces open, and the lifecycle-opened metrics-health
//! surface exposes the two INDEPENDENT probes (architecture §4.3):
//!   - **readiness** — a booted instance with every critical dependency up is `Ready`; a severed
//!     critical dependency flips it to `NotReady` + sheds; a still-booting instance is
//!     not-ready-not-killed.
//!   - **liveness** — "not wedged"; a dead critical dependency leaves it `Up` (no restart-storm).
//!
//! The provider side is [`myelin_substrate::serve::boot`] + the [`MetricsHealthSurface`] it opens;
//! this is the consumer (a service's `main` reads the probes the orchestrator scrapes). It is the
//! dated green artifact's CDC half (the runtime SUB-D9 half is `drill_sub_d9_liveness_readiness.rs`).

use myelin_events::relay::InProcessBus;
use myelin_substrate::serve::{boot, AppSpec, OutboxSpec, Surface};
use myelin_substrate::{
    Config, CriticalDependencies, InternalRpc, Liveness, Migrations, PublicRoutes, Readiness,
};

/// **CDC 1.3 — a booted instance is Ready, a severed critical dependency flips readiness (not
/// liveness).** Boots a service declaring `identity` critical; asserts ready; severs `identity`
/// via the lifecycle's health probe; asserts readiness flips to NotReady + sheds while liveness
/// stays Up (no restart).
#[test]
fn cdc_1_3_lifecycle_metrics_health_is_liveness_ne_readiness() {
    let spec = AppSpec {
        name: "svc",
        config: Config::default(),
        migrations: Migrations::default(),
        public: PublicRoutes::default(),
        internal: InternalRpc::default(),
        consumers: vec![],
        holders: AppSpec::auto(),
        outbox: OutboxSpec::new(myelin_events::OutboxStore::new(), InProcessBus::new()),
        // the service declares `identity` a critical dependency (it cannot serve correct traffic
        // without authz); the OLTP store is implicitly critical too (added by the lifecycle).
        critical: CriticalDependencies::new(["identity"]),
    };

    // (boot) the lifecycle opens the three surfaces; the metrics-health surface's startup gate is
    // flipped Complete at the end of a successful boot (a half-booted instance never reads ready).
    let handle = boot(spec).expect("the service boots from serve(AppSpec)");
    assert!(
        handle.surfaces().contains(&Surface::MetricsHealth),
        "the metrics-health surface opened in the lifecycle (§4)"
    );

    let mh = handle.metrics_health();

    // booted + all critical deps up → Ready, liveness Up.
    let r = mh.readiness();
    assert_eq!(r.verdict, Readiness::Ready, "a fully-booted instance with healthy deps is ready");
    assert_eq!(r.verdict.gauge(), 1, "the readiness gauge reads 1");
    assert_eq!(mh.liveness(), Liveness::Up, "a healthy instance is live");
    assert_eq!(mh.liveness_restart_count(), 0, "no restart churn");

    // sever a CRITICAL dependency through the lifecycle's shared health probe (a sustained outage).
    handle.health_probe().mark_down("identity");

    // readiness FLIPS to not-ready + sheds, naming the down dependency; liveness is UNTOUCHED.
    let r = mh.readiness();
    assert_eq!(r.verdict, Readiness::NotReady, "a dead critical dep reports not-ready (§4.3)");
    assert!(r.sheds(), "not-ready → shed new traffic (never serve healthy-but-failing)");
    assert!(
        r.down_critical.iter().any(|d| d.0 == "identity"),
        "the report names the down critical dependency"
    );
    assert_eq!(r.verdict.gauge(), 0, "the readiness gauge reads 0 when not-ready");
    // the load-bearing SUB-D9 property: liveness does NOT check the dependency → no restart-storm.
    assert_eq!(mh.liveness(), Liveness::Up, "liveness stays Up across a dependency outage (§4.3)");
    assert!(!mh.liveness().should_restart(), "a dead dependency must NOT trigger a restart");
    assert_eq!(mh.liveness_restart_count(), 0, "no restart-storm: liveness churn stays 0");

    // the OLTP store is implicitly critical too: severing it also flips readiness.
    handle.health_probe().mark_up("identity");
    assert_eq!(mh.readiness().verdict, Readiness::Ready, "readiness recovers when identity heals");
    handle.health_probe().mark_down("oltp");
    assert_eq!(
        mh.readiness().verdict,
        Readiness::NotReady,
        "the OLTP store is implicitly critical — a dead DB flips readiness"
    );
}

/// **CDC 1.3 — startup is not-ready-not-killed.** A service is `Booting` until boot completes:
/// a fresh metrics-health surface (before `mark_started`) reads NotReady (startup gate) but Up
/// (liveness — a slow boot is not a wedge). Exercised at the unit level here through the public
/// surface type so the contract shape is asserted from outside the crate too.
#[test]
fn cdc_1_3_startup_is_not_ready_not_killed() {
    use myelin_substrate::{HealthTable, MetricsHealthSurface, Startup};
    let s = MetricsHealthSurface::new(CriticalDependencies::new(["identity"]), HealthTable::new());
    // before mark_started: booting.
    assert_eq!(s.startup(), Startup::Booting);
    assert_eq!(s.readiness().verdict, Readiness::NotReady, "an unbooted instance is not-ready");
    assert!(s.readiness().startup_incomplete, "the reason is the startup gate");
    assert_eq!(s.liveness(), Liveness::Up, "startup is NOT killed: liveness stays Up");
    // after a successful boot, the startup gate no longer holds readiness down.
    s.mark_started();
    assert_eq!(s.startup(), Startup::Complete);
    assert_eq!(s.readiness().verdict, Readiness::Ready, "booted + deps up → ready");
}
