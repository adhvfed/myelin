use crate::migrations::migrations as flow_migrations;
use myelin_events::OutboxStore;
use myelin_substrate::{
    boot, serve, AppSpec, Config, CriticalDependencies, HotTables, InternalRpc, Migrations,
    OutboxSpec, PublicRoutes, ServeError, ServeHandle, StoreManifest,
};

pub const SERVICE_NAME: &str = "myelin-flow";

fn flow_service_migrations() -> Migrations {
    flow_migrations()
}

pub fn flow_app_spec(config: Config, outbox: OutboxStore) -> AppSpec {
    AppSpec {
        name: SERVICE_NAME,
        config,
        migrations: flow_service_migrations(),
        hot_tables: HotTables::declare(["workflow_run"]),
        public: PublicRoutes::default(),
        internal: InternalRpc::default(),
        consumers: Vec::new(),
        holders: AppSpec::auto(),
        stores: StoreManifest::new(),
        outbox: OutboxSpec::external_relay(outbox),
        critical: CriticalDependencies::default(),
    }
}

pub fn boot_flow(config: Config, outbox: OutboxStore) -> Result<ServeHandle, ServeError> {
    boot(flow_app_spec(config, outbox))
}

pub fn run_flow(config: Config, outbox: OutboxStore) -> Result<(), ServeError> {
    serve(flow_app_spec(config, outbox))
}

pub fn flow_app_spec_with_engine(
    config: Config,
    outbox: OutboxStore,
    minter: std::sync::Arc<dyn myelin_events::IdMinter>,
    ctx_base: myelin_events::EmitContextBase,
    partition: i16,
    worker: impl Into<String>,
    lease_ttl_secs: i64,
) -> (
    AppSpec,
    crate::engine::FlowDispatcher,
    crate::timer::TimerWheel,
) {
    let runs = crate::engine::RunStore::new();
    let journal = crate::wfctx::WfJournal::new();
    let telemetry = crate::engine::FlowTelemetry::new();
    let timers = crate::timer::TimerStore::new();
    let dispatcher = crate::engine::FlowDispatcher::new(
        runs.clone(),
        outbox.clone(),
        journal.clone(),
        telemetry.clone(),
        minter,
        ctx_base,
        partition,
        worker,
        lease_ttl_secs,
    )
    .with_timers(timers.clone());
    let wheel = crate::timer::TimerWheel::new(
        timers, journal, runs, telemetry, partition,  4_096,
    );
    let spec = flow_app_spec(config, outbox);
    (spec, dispatcher, wheel)
}

pub fn flow_signal_consumer_reg(
    tenant: &myelin_tenancy::TenantId,
    executor: crate::FlowExecutor,
    dedup: myelin_events::DedupLedger,
) -> Result<myelin_substrate::ConsumerReg, myelin_events::SubscribeError> {
    use myelin_events::{consume, ConsumerName, ConsumerSpec, SubjectPattern};
    let prefix = format!("sig.{}.", tenant.0);
    let subjects: &'static [SubjectPattern] =
        Box::leak(vec![SubjectPattern(prefix.clone())].into_boxed_slice());
    let consumer = crate::FlowSignalConsumer::new(executor, subjects);
    let runtime = consume(
        ConsumerSpec::new(
            ConsumerName(format!("flow-signal-{}", tenant.0)),
            &[prefix.as_str()],
        ),
        consumer,
        dedup,
    )?;
    Ok(myelin_substrate::ConsumerReg::new(runtime))
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_substrate::{
        HealthTable, Liveness, MetricsHealthSurface, Readiness, Startup, Surface,
    };

    #[test]
    fn flow_shell_boots_and_three_ports_bind() {
        let handle =
            boot_flow(Config::default(), OutboxStore::new()).expect("the myelin-flow shell boots");
        assert_eq!(handle.name(), SERVICE_NAME);
        assert_eq!(
            handle.surfaces(),
            &[Surface::Public, Surface::Internal, Surface::MetricsHealth],
            "the three ports (public / internal-RPC / metrics-health) all bound (3/3)"
        );
    }

    #[test]
    fn readiness_is_false_pre_migrate_but_liveness_is_up() {
        let surface =
            MetricsHealthSurface::new(CriticalDependencies::new(["oltp"]), HealthTable::new());
        assert_eq!(surface.startup(), Startup::Booting);
        let r = surface.readiness();
        assert_eq!(
            r.verdict,
            Readiness::NotReady,
            "readiness is FALSE until the migrate-complete gate lifts"
        );
        assert!(
            r.startup_incomplete,
            "the not-ready reason names the startup (pre-migrate) gate"
        );
        assert!(r.sheds(), "a not-ready instance sheds new traffic");
        assert_eq!(
            surface.liveness(),
            Liveness::Up,
            "liveness ≠ readiness: a booting instance is not-killed (liveness stays Up)"
        );
        surface.mark_started();
        assert_eq!(
            surface.readiness().verdict,
            Readiness::Ready,
            "after migrate-complete the readiness gate lifts → ready"
        );
    }

    #[test]
    fn booted_instance_is_ready_after_migrate_complete() {
        let handle = boot_flow(Config::default(), OutboxStore::new()).expect("boot");
        assert_eq!(
            handle.metrics_health().startup(),
            Startup::Complete,
            "boot completed → the migrate gate lifted (the six-table set applied)"
        );
        assert_eq!(
            handle.metrics_health().readiness().verdict,
            Readiness::Ready,
            "a booted flow instance (the six tables migrated, deps up) is ready"
        );
    }

    #[test]
    fn shell_wires_the_six_table_migration_set_and_empty_consumer_seam() {
        let spec = flow_app_spec(Config::default(), OutboxStore::new());
        assert_eq!(spec.name, SERVICE_NAME);
        assert!(
            spec.consumers.is_empty(),
            "the replay engine + signal/timer consumers are the P-FLOW-04..05/09/13 floor (empty seam)"
        );
        assert_eq!(
            spec.migrations.0.len(),
            12,
            "six table creates plus six online workflow control/drive/repair expands (incl. the concurrent-index validation)"
        );
        assert_eq!(
            spec.migrations,
            crate::migrations::migrations(),
            "the migrate phase wires EXACTLY the P-FLOW-01 set (no second schema)"
        );
    }

    #[test]
    fn inbound_signal_consumer_fills_the_consumer_slot() {
        use myelin_events::DedupLedger;
        use myelin_tenancy::{Region, TenantId};
        let minter: std::sync::Arc<dyn myelin_events::IdMinter> =
            std::sync::Arc::new(myelin_events::MonotonicMinter::new());
        let tenant = TenantId("acme".into());
        let ex = crate::FlowExecutor::new(minter, tenant.clone(), Region("fr-par".into()));
        let reg = flow_signal_consumer_reg(&tenant, ex, DedupLedger::new())
            .expect("the sig.acme. whitelist binds through the sanctioned consume (never `*`)");

        let mut spec = flow_app_spec(Config::default(), OutboxStore::new());
        spec.consumers = vec![reg];
        assert_eq!(
            spec.consumers.len(),
            1,
            "the inbound-signal consumer occupies the consumer slot (P-FLOW-09)"
        );
    }

    #[test]
    fn flow_oltp_store_auto_registers_as_a_holder_at_boot() {
        use myelin_substrate::StoreKind;
        let handle = boot_flow(Config::default(), OutboxStore::new()).expect("boot");
        assert!(
            handle
                .holder_registry()
                .is_registered(StoreKind::Oltp, SERVICE_NAME),
            "the flow OLTP store auto-registered as a holder at boot (opening IS registering)"
        );
        assert!(
            handle.holder_registered().is_ok(),
            "no store the service declares escaped registration (the holder-registered architecture test)"
        );
    }

    #[test]
    fn boot_registered_flow_store_classifies_and_completeness_is_green() {
        use crate::holder::{flow_history_holder, flow_store_classifier};
        use myelin_substrate::{assert_holder_completeness, Holder};
        let handle = boot_flow(Config::default(), OutboxStore::new()).expect("boot");
        assert_eq!(flow_history_holder(), Some(Holder::H8EventBus));
        assert_eq!(
            assert_holder_completeness(
                handle.holder_registry().registrations(),
                &flow_store_classifier(),
            ),
            Ok(()),
            "every store the flow harness opens is in the exhaustive H1–H18 list - 0 orphan"
        );
    }

    #[test]
    fn run_flow_boots_serves_and_drains_cleanly() {
        assert_eq!(
            run_flow(Config::default(), OutboxStore::new()),
            Ok(()),
            "the flow shell boots → migrates → relays → drains cleanly (depth 0)"
        );
    }

    #[test]
    fn failed_boot_returns_non_zero() {
        let r = run_flow(Config("BAD_POOL".into()), OutboxStore::new());
        assert!(r.is_err(), "a failed boot must return non-zero (Err)");
        assert!(
            r.unwrap_err().0.contains("fail-fast"),
            "the boot error names the §3.2 fail-fast config validation"
        );
    }

    #[test]
    fn engine_wired_spec_returns_a_driving_dispatcher() {
        use crate::engine::{run_state, DriveOutcome, RunRow};
        use crate::RetryPolicy;
        use myelin_events::{Actor, EmitContextBase, MonotonicMinter, Timestamp};
        use myelin_identity::{Principal, PrincipalId, PrincipalKind};
        use myelin_refs::ArtifactRef;
        use myelin_tenancy::{Region, TenantId};
        use std::sync::Arc;

        let tenant = TenantId("acme".into());
        let region = Region("fr-par".into());
        let ctx_base = EmitContextBase {
            tenant: tenant.clone(),
            region: region.clone(),
            actor: Actor(Principal::stub(
                PrincipalId("p".into()),
                PrincipalKind::Human,
                tenant.clone(),
            )),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
            caused_by: None,
        };
        let (spec, mut dispatcher, _wheel) = flow_app_spec_with_engine(
            Config::default(),
            OutboxStore::new(),
            Arc::new(MonotonicMinter::new()),
            ctx_base,
            0,
            "worker-1",
            30,
        );
        assert_eq!(
            spec.migrations.0.len(),
            12,
            "the engine-wired spec keeps the schema and online control expands"
        );

        dispatcher.register(
            "agent.run",
            Box::new(|ctx: &mut crate::WfCtx| {
                ctx.activity(RetryPolicy::default_policy(), |_i, _a| {
                    Ok(vec![ArtifactRef("myelin://acme/agent/effect/e0".into())])
                })
                .map_err(|e| format!("{e:?}"))?;
                Ok(vec![])
            }),
        );
        dispatcher.runs().put(RunRow::new_runnable(
            tenant.clone(),
            region,
            "R1",
            "agent.run",
            0,
        ));
        let outcome = dispatcher.tick(1000, "2026-06-21T00:00:00Z", 7);
        assert!(
            matches!(outcome, Some(DriveOutcome::Completed(_))),
            "the dispatcher drove the run"
        );
        assert_eq!(
            dispatcher.runs().get(&tenant, "R1").unwrap().state,
            run_state::COMPLETED,
            "the seeded run completed under the engine-wired dispatcher"
        );
        assert_eq!(
            dispatcher.telemetry().double_effect_count(),
            0,
            "0 double-effect"
        );
    }

    #[test]
    fn timer_wheel_wired_into_consumer_seam_parks_fires_and_re_drives() {
        use crate::engine::{run_state, DriveOutcome, RunRow};
        use myelin_events::{Actor, EmitContextBase, MonotonicMinter, Timestamp};
        use myelin_identity::{Principal, PrincipalId, PrincipalKind};
        use myelin_refs::ArtifactRef;
        use myelin_tenancy::{Region, TenantId};
        use std::sync::Arc;

        let tenant = TenantId("acme".into());
        let region = Region("fr-par".into());
        let ctx_base = EmitContextBase {
            tenant: tenant.clone(),
            region: region.clone(),
            actor: Actor(Principal::stub(
                PrincipalId("p".into()),
                PrincipalKind::Human,
                tenant.clone(),
            )),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
            caused_by: None,
        };
        let (_spec, mut dispatcher, wheel) = flow_app_spec_with_engine(
            Config::default(),
            OutboxStore::new(),
            Arc::new(MonotonicMinter::new()),
            ctx_base,
            0,
            "worker-1",
            30,
        );

        dispatcher.register(
            "sla.run",
            Box::new(|ctx: &mut crate::WfCtx| {
                ctx.sleep_for(600).map_err(|e| format!("{e:?}"))?;
                ctx.activity(crate::RetryPolicy::default_policy(), |_i, _a| {
                    Ok(vec![ArtifactRef(
                        "myelin://acme/agent/effect/after-sleep".into(),
                    )])
                })
                .map_err(|e| format!("{e:?}"))?;
                Ok(vec![])
            }),
        );
        dispatcher.runs().put(RunRow::new_runnable(
            tenant.clone(),
            region,
            "R1",
            "sla.run",
            0,
        ));

        let o1 = dispatcher.tick(1000, "2026-06-21T00:00:00Z", 7);
        assert!(
            matches!(o1, Some(DriveOutcome::Waiting)),
            "the sleep parked the run, got {o1:?}"
        );
        assert_eq!(
            dispatcher.runs().get(&tenant, "R1").unwrap().state,
            run_state::WAITING,
            "the run is waiting (no runtime)"
        );
        assert_eq!(
            wheel.timers().unfired_count(),
            1,
            "one durable timer armed on the wheel"
        );

        assert_eq!(
            wheel.tick(1100),
            0,
            "the not-yet-due timer is NOT fired (far-future bucket untouched)"
        );
        assert_eq!(
            dispatcher.runs().get(&tenant, "R1").unwrap().state,
            run_state::WAITING,
            "still waiting"
        );

        assert_eq!(wheel.tick(1600), 1, "the due timer fires at its minute");
        assert_eq!(
            dispatcher.runs().get(&tenant, "R1").unwrap().state,
            run_state::RUNNING,
            "the wheel woke the run"
        );
        assert_eq!(
            wheel.telemetry().timer_wheel_lag(),
            0,
            "the timer-wheel-lag is 0 after the fire (SC-11 health signal)"
        );

        let o2 = dispatcher.tick(1601, "2026-06-21T00:00:01Z", 7);
        assert!(
            matches!(o2, Some(DriveOutcome::Completed(_))),
            "the run completed past the sleep, got {o2:?}"
        );
        assert_eq!(
            dispatcher.runs().get(&tenant, "R1").unwrap().state,
            run_state::COMPLETED,
            "the run is completed"
        );
        assert_eq!(
            dispatcher.telemetry().double_effect_count(),
            0,
            "0 double-effect (the sleep replayed, did not re-arm)"
        );
    }

    #[test]
    fn graceful_drain_leaves_outbox_depth_zero() {
        let handle = boot_flow(Config::default(), OutboxStore::new()).expect("boot");
        handle.signal_drain();
        assert!(handle.is_draining(), "intake is stopped");
        handle.tick();
        let t = handle.telemetry();
        assert_eq!(
            t.outbox_depth(),
            0,
            "the graceful drain leaves outbox_depth == 0"
        );
        assert_eq!(
            t.dead_letter_count(),
            0,
            "nothing dead-lettered on a clean shell drain"
        );
    }
}
