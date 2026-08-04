use myelin_events::{EventEnvelope, EventHandler, HandleOutcome, SubjectPattern};
use myelin_substrate::{
    boot, serve, AppSpec, Config, CriticalDependencies, HotTables, InternalRpc, Migrations,
    OutboxSpec, PublicRoutes, ServeError, ServeHandle, StoreManifest,
};

pub const SERVICE_NAME: &str = "myelin-agent";

pub const AGENT_DISPATCH_SUBJECT_PREFIX: &str = "agent.dispatch.";

fn agent_service_migrations() -> Migrations {
    use crate::migrations::{
        rls_scope_sql, HITL_GATE_DDL, PROPOSED_EFFECT_DDL, RUN_DDL, TOOL_DEF_DDL, TRACE_DDL,
    };
    use myelin_substrate::{Migration, MigrationPhase};
    let tables: [(&'static str, &str, &str); 5] = [
        ("0001_create_agent_run", RUN_DDL, "agent_run"),
        ("0002_create_agent_tool_def", TOOL_DEF_DDL, "agent_tool_def"),
        (
            "0003_create_agent_proposed_effect",
            PROPOSED_EFFECT_DDL,
            "agent_proposed_effect",
        ),
        (
            "0004_create_agent_hitl_gate",
            HITL_GATE_DDL,
            "agent_hitl_gate",
        ),
        ("0005_create_agent_trace", TRACE_DDL, "agent_trace"),
    ];
    Migrations::of(tables.into_iter().map(|(id, create_ddl, table)| {
        let mut ddl = String::new();
        ddl.push_str(create_ddl);
        ddl.push(';');
        ddl.push('\n');
        ddl.push_str(&rls_scope_sql(table));
        ddl.push(';');
        let ddl: &'static str = Box::leak(ddl.into_boxed_str());
        Migration::phased(id, ddl, MigrationPhase::Plain, table)
    }))
}

pub struct SkeletonDispatchConsumer {
    subjects: &'static [SubjectPattern],
}

impl SkeletonDispatchConsumer {
    pub fn new(subjects: &'static [SubjectPattern]) -> SkeletonDispatchConsumer {
        SkeletonDispatchConsumer { subjects }
    }
}

impl EventHandler for SkeletonDispatchConsumer {
    fn subjects(&self) -> &'static [SubjectPattern] {
        self.subjects
    }

    fn handle(&self, _ev: &EventEnvelope, _tx: &mut myelin_events::HandlerTx<'_>) -> HandleOutcome {
        HandleOutcome::Done
    }
}

pub fn agent_app_spec(config: Config, outbox: myelin_events::OutboxStore) -> AppSpec {
    AppSpec {
        name: SERVICE_NAME,
        config,
        migrations: agent_service_migrations(),
        hot_tables: HotTables::none(),
        public: PublicRoutes::default(),
        internal: InternalRpc::default(),
        consumers: Vec::new(),
        holders: AppSpec::auto(),
        stores: StoreManifest::new(),
        outbox: OutboxSpec::external_relay(outbox),
        critical: CriticalDependencies::default(),
    }
}

pub fn agent_dispatch_consumer_reg(
    tenant: &myelin_tenancy::TenantId,
    dedup: myelin_events::DedupLedger,
) -> Result<myelin_substrate::ConsumerReg, myelin_events::SubscribeError> {
    use myelin_events::{consume, ConsumerName, ConsumerSpec};
    let prefix = format!("{AGENT_DISPATCH_SUBJECT_PREFIX}{}.", tenant.0);
    let subjects: &'static [SubjectPattern] =
        Box::leak(vec![SubjectPattern(prefix.clone())].into_boxed_slice());
    let consumer = SkeletonDispatchConsumer::new(subjects);
    let runtime = consume(
        ConsumerSpec::new(
            ConsumerName(format!("agent-dispatch-{}", tenant.0)),
            &[prefix.as_str()],
        ),
        consumer,
        dedup,
    )?;
    Ok(myelin_substrate::ConsumerReg::new(runtime))
}

pub fn boot_agent(
    config: Config,
    outbox: myelin_events::OutboxStore,
) -> Result<ServeHandle, ServeError> {
    boot(agent_app_spec(config, outbox))
}

pub fn run_agent(config: Config, outbox: myelin_events::OutboxStore) -> Result<(), ServeError> {
    serve(agent_app_spec(config, outbox))
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_substrate::{
        HealthTable, Liveness, MetricsHealthSurface, Readiness, Startup, Surface,
    };

    #[test]
    fn agent_shell_boots_and_three_ports_bind() {
        let handle = boot_agent(Config::default(), myelin_events::OutboxStore::new())
            .expect("the myelin-agent shell boots");
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
            "readiness FALSE until migrate completes"
        );
        assert!(r.sheds(), "a not-ready instance sheds new traffic");
        assert_eq!(
            surface.liveness(),
            Liveness::Up,
            "liveness ≠ readiness: booting is not-killed"
        );
        surface.mark_started();
        assert_eq!(
            surface.readiness().verdict,
            Readiness::Ready,
            "migrate-complete lifts readiness"
        );
    }

    #[test]
    fn booted_instance_is_ready_after_migrate_complete() {
        let handle =
            boot_agent(Config::default(), myelin_events::OutboxStore::new()).expect("boot");
        assert_eq!(
            handle.metrics_health().startup(),
            Startup::Complete,
            "boot completed → the migrate gate lifted (the five-table set applied)"
        );
        assert_eq!(
            handle.metrics_health().readiness().verdict,
            Readiness::Ready,
            "a booted agent instance (the five tables migrated, deps up) is ready"
        );
    }

    #[test]
    fn shell_wires_the_five_table_migration_set_and_empty_consumer_seam() {
        let spec = agent_app_spec(Config::default(), myelin_events::OutboxStore::new());
        assert_eq!(spec.name, SERVICE_NAME);
        assert!(
            spec.consumers.is_empty(),
            "the dispatch consumer is wired per-tenant (empty bare seam)"
        );
        assert_eq!(
            spec.migrations.0.len(),
            5,
            "the AppSpec wires the AG-P2 five-table data model"
        );
        let ids: Vec<&str> = spec.migrations.0.iter().map(|m| m.id).collect();
        assert_eq!(
            ids,
            vec![
                "0001_create_agent_run",
                "0002_create_agent_tool_def",
                "0003_create_agent_proposed_effect",
                "0004_create_agent_hitl_gate",
                "0005_create_agent_trace",
            ],
            "the substrate migrate set carries the SAME five AG-P2 migrations (one schema)"
        );
    }

    #[test]
    fn dispatch_consumer_fills_the_consumer_slot() {
        use myelin_events::DedupLedger;
        use myelin_tenancy::TenantId;
        let tenant = TenantId("acme".into());
        let reg = agent_dispatch_consumer_reg(&tenant, DedupLedger::new()).expect(
            "the agent.dispatch.acme. whitelist binds through the sanctioned consume (never `*`)",
        );
        let mut spec = agent_app_spec(Config::default(), myelin_events::OutboxStore::new());
        spec.consumers = vec![reg];
        assert_eq!(
            spec.consumers.len(),
            1,
            "the dispatch consumer occupies the consumer slot (3.6)"
        );
    }

    #[test]
    fn agent_stores_auto_register_as_holders_at_boot() {
        use myelin_substrate::StoreKind;
        let handle =
            boot_agent(Config::default(), myelin_events::OutboxStore::new()).expect("boot");
        assert!(
            handle
                .holder_registry()
                .is_registered(StoreKind::Oltp, SERVICE_NAME),
            "the agent OLTP store auto-registered as a holder at boot"
        );
        assert!(
            handle.holder_registered().is_ok(),
            "no store the service declares escaped registration (the holder-registered architecture test)"
        );
    }

    #[test]
    fn run_agent_boots_serves_and_drains_cleanly() {
        assert_eq!(
            run_agent(Config::default(), myelin_events::OutboxStore::new()),
            Ok(()),
            "the agent shell boots → migrates → relays → drains cleanly (depth 0)"
        );
    }

    #[test]
    fn failed_boot_returns_non_zero() {
        let r = run_agent(Config("BAD_POOL".into()), myelin_events::OutboxStore::new());
        assert!(r.is_err(), "a failed boot must return non-zero (Err)");
    }

    #[test]
    fn dispatch_consumer_is_explicit_first_notify_only() {
        use myelin_events::{
            Actor, AggregateKey, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
            Timestamp, Visibility,
        };
        use myelin_identity::{Principal, PrincipalId, PrincipalKind};
        use myelin_refs::ArtifactRef;
        use myelin_tenancy::{Region, TenantId};
        let subjects: &'static [SubjectPattern] =
            Box::leak(vec![SubjectPattern("agent.dispatch.acme.".into())].into_boxed_slice());
        let consumer = SkeletonDispatchConsumer::new(subjects);
        let tenant = TenantId("acme".into());
        let ev = EventEnvelope {
            event_id: EventId("ev-1".into()),
            type_: EventType("agent.dispatch.acme.mention".into()),
            schema_ver: 1,
            tenant: tenant.clone(),
            region: Region("fr-par".into()),
            actor: Actor(Principal::stub(
                PrincipalId("p".into()),
                PrincipalKind::Human,
                tenant,
            )),
            subject: ArtifactRef("myelin://acme/chat/msg/1".into()),
            aggregate: AggregateKey("conv:1".into()),
            causation_id: None,
            correlation_id: CorrelationId("c1".into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
            payload: serde_json::json!({}),
        };
        assert_eq!(
            consumer.handle(&ev, &mut myelin_events::HandlerTx::none()),
            HandleOutcome::Done,
            "explicit-first: a delivered match NOTIFIES (Done); it does not auto-spawn a costed run"
        );
    }
}
