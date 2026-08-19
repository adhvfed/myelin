use myelin_events::relay::InProcessBus;
use myelin_substrate::serve::{boot, AppSpec, OutboxSpec, Surface};
use myelin_substrate::{
    Config, CriticalDependencies, HotTables, InternalRpc, Liveness, Migrations, PublicRoutes,
    Readiness, StoreManifest,
};

#[test]
fn cdc_1_3_lifecycle_metrics_health_is_liveness_ne_readiness() {
    let spec = AppSpec {
        name: "svc",
        config: Config::default(),
        migrations: Migrations::default(),
        hot_tables: HotTables::none(),
        public: PublicRoutes::default(),
        internal: InternalRpc::default(),
        consumers: vec![],
        holders: AppSpec::auto(),
        stores: StoreManifest::new(),
        outbox: OutboxSpec::new(myelin_events::OutboxStore::new(), InProcessBus::new()),
        critical: CriticalDependencies::new(["identity"]),
        intake_scope: None,
    };

    let handle = boot(spec).expect("the service boots from serve(AppSpec)");
    assert!(
        handle.surfaces().contains(&Surface::MetricsHealth),
        "the metrics-health surface opened in the lifecycle (§4)"
    );

    let mh = handle.metrics_health();

    let r = mh.readiness();
    assert_eq!(
        r.verdict,
        Readiness::Ready,
        "a fully-booted instance with healthy deps is ready"
    );
    assert_eq!(r.verdict.gauge(), 1, "the readiness gauge reads 1");
    assert_eq!(mh.liveness(), Liveness::Up, "a healthy instance is live");
    assert_eq!(mh.liveness_restart_count(), 0, "no restart churn");

    handle.health_probe().mark_down("identity");

    let r = mh.readiness();
    assert_eq!(
        r.verdict,
        Readiness::NotReady,
        "a dead critical dep reports not-ready (§4.3)"
    );
    assert!(
        r.sheds(),
        "not-ready → shed new traffic (never serve healthy-but-failing)"
    );
    assert!(
        r.down_critical.iter().any(|d| d.0 == "identity"),
        "the report names the down critical dependency"
    );
    assert_eq!(
        r.verdict.gauge(),
        0,
        "the readiness gauge reads 0 when not-ready"
    );
    assert_eq!(
        mh.liveness(),
        Liveness::Up,
        "liveness stays Up across a dependency outage (§4.3)"
    );
    assert!(
        !mh.liveness().should_restart(),
        "a dead dependency must NOT trigger a restart"
    );
    assert_eq!(
        mh.liveness_restart_count(),
        0,
        "no restart-storm: liveness churn stays 0"
    );

    handle.health_probe().mark_up("identity");
    assert_eq!(
        mh.readiness().verdict,
        Readiness::Ready,
        "readiness recovers when identity heals"
    );
    handle.health_probe().mark_down("oltp");
    assert_eq!(
        mh.readiness().verdict,
        Readiness::NotReady,
        "the OLTP store is implicitly critical - a dead DB flips readiness"
    );
}

#[test]
fn cdc_1_3_startup_is_not_ready_not_killed() {
    use myelin_substrate::{HealthTable, MetricsHealthSurface, Startup};
    let s = MetricsHealthSurface::new(CriticalDependencies::new(["identity"]), HealthTable::new());
    assert_eq!(s.startup(), Startup::Booting);
    assert_eq!(
        s.readiness().verdict,
        Readiness::NotReady,
        "an unbooted instance is not-ready"
    );
    assert!(
        s.readiness().startup_incomplete,
        "the reason is the startup gate"
    );
    assert_eq!(
        s.liveness(),
        Liveness::Up,
        "startup is NOT killed: liveness stays Up"
    );
    s.mark_started();
    assert_eq!(s.startup(), Startup::Complete);
    assert_eq!(
        s.readiness().verdict,
        Readiness::Ready,
        "booted + deps up → ready"
    );
}
