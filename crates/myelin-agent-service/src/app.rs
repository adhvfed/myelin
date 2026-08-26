#[cfg(test)]
use myelin_substrate::{boot, serve, ServeHandle};
use myelin_substrate::{
    AppSpec, Config, CriticalDependencies, HotTables, InternalRpc, Migrations, OutboxSpec,
    PublicRoutes, ServeError,
};

pub const SERVICE_NAME: &str = "myelin-agent";

pub const EVENT_STREAM_NAME: &str = "MYELIN_EVENTS";
pub const EVENT_SUBJECT_ROOT: &str = "myelin.events";
pub const EVENT_DURABLE_CONSUMER: &str = "agent-governed-trigger-intake";

pub fn trigger_intake_filter() -> String {
    format!("{EVENT_SUBJECT_ROOT}.evt.>")
}

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
        Migration::phased(id, ddl, MigrationPhase::Plain, table)
    }))
}

fn agent_app_spec(config: Config, outbox: myelin_events::OutboxStore) -> AppSpec {
    AppSpec {
        name: SERVICE_NAME,
        config,
        migrations: agent_service_migrations(),
        hot_tables: HotTables::none(),
        public: PublicRoutes::default(),
        internal: InternalRpc::default(),
        consumers: Vec::new(),
        outbox: OutboxSpec::external_relay(outbox),
        critical: CriticalDependencies::default(),
        intake_scope: None,
    }
}

pub fn governed_trigger_consumer_reg(
    tenant: &myelin_tenancy::TenantId,
    region: &myelin_tenancy::Region,
    store: std::sync::Arc<dyn crate::trigger_consumer::TriggerBindingStore>,
    visibility: std::sync::Arc<dyn crate::trigger_consumer::TriggerOwnerVisibility>,
    approvals: std::sync::Arc<dyn crate::trigger_consumer::TriggerApprovalInbox>,
    dedup: myelin_events::DedupLedger,
    dead_letters: std::sync::Arc<dyn myelin_events::DurableDeadLetter>,
) -> Result<myelin_substrate::ConsumerReg, myelin_events::SubscribeError> {
    let subject = format!("myelin://{}/", tenant.0);
    let handler = crate::trigger_consumer::GovernedTriggerConsumer::new(
        tenant.0.clone(),
        region.0.clone(),
        store,
        visibility,
        approvals,
    );
    let subscription = myelin_events::consumer::Subscription::bind(
        myelin_events::ConsumerName(format!(
            "{}-{}",
            crate::trigger_consumer::TRIGGER_CONSUMER_NAME,
            tenant.0
        )),
        &[subject.as_str()],
        myelin_events::PrefetchBound::DEFAULT,
    )?;
    Ok(myelin_substrate::ConsumerReg::new(
        myelin_events::Consumer::new(handler, subscription, dedup)
            .with_dead_letter_sink(myelin_events::DeadLetterSink::durable(dead_letters)),
    ))
}

/// The intake scope for a cell-bound agent service: exactly the tenants the
/// placement directory hosts here. The durable intake subscribes to the whole
/// event stream, so deliveries for tenants placed on OTHER cells (or not
/// placed at all) are routine and must terminate without quarantine noise;
/// within a hosted tenant, an unmatched delivery still quarantines loudly.
pub fn placed_tenant_intake_scope(
    tenants: &[myelin_tenancy::TenantId],
) -> myelin_substrate::IntakeScope {
    let prefixes: Vec<String> = tenants
        .iter()
        .map(|tenant| format!("myelin://{}/", tenant.0))
        .collect();
    std::sync::Arc::new(move |subject: &str| {
        prefixes.iter().any(|prefix| subject.starts_with(prefix))
    })
}

fn agent_app_spec_with_ingestion(
    config: Config,
    outbox: myelin_events::OutboxStore,
    consumers: Vec<myelin_substrate::ConsumerReg>,
    intake: Box<dyn myelin_events::EventConsumer>,
    delivery_quarantine: std::sync::Arc<dyn myelin_events::DurableDeliveryQuarantine>,
    intake_scope: Option<myelin_substrate::IntakeScope>,
) -> AppSpec {
    let mut spec = agent_app_spec(config, outbox.clone());
    spec.outbox = OutboxSpec::external_relay_with_consumer(outbox, intake, delivery_quarantine);
    spec.consumers = consumers;
    spec.intake_scope = intake_scope;
    spec
}

pub async fn run_agent_ingestion_until_shutdown<F>(
    config: Config,
    outbox: myelin_events::OutboxStore,
    consumers: Vec<myelin_substrate::ConsumerReg>,
    intake: Box<dyn myelin_events::EventConsumer>,
    delivery_quarantine: std::sync::Arc<dyn myelin_events::DurableDeliveryQuarantine>,
    intake_scope: Option<myelin_substrate::IntakeScope>,
    shutdown: F,
) -> Result<(), ServeError>
where
    F: std::future::Future<Output = ()>,
{
    myelin_substrate::serve_until_shutdown(
        agent_app_spec_with_ingestion(
            config,
            outbox,
            consumers,
            intake,
            delivery_quarantine,
            intake_scope,
        ),
        shutdown,
    )
    .await
}

#[cfg(test)]
fn boot_agent(
    config: Config,
    outbox: myelin_events::OutboxStore,
) -> Result<ServeHandle, ServeError> {
    boot(agent_app_spec(config, outbox))
}

#[cfg(test)]
fn run_agent(config: Config, outbox: myelin_events::OutboxStore) -> Result<(), ServeError> {
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
    fn bare_spec_owns_migrations_but_no_implicit_consumers() {
        let spec = agent_app_spec(Config::default(), myelin_events::OutboxStore::new());
        assert_eq!(spec.name, SERVICE_NAME);
        assert!(
            spec.consumers.is_empty(),
            "production must inject the governed per-tenant consumers explicitly"
        );
        assert_eq!(
            spec.migrations.0.len(),
            5,
            "the AppSpec wires the AG-P2 five-table data model"
        );
        let ids: Vec<&str> = spec.migrations.0.iter().map(|m| m.id.as_ref()).collect();
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
    fn placed_tenant_scope_admits_exactly_the_hosted_tenants() {
        let scope = placed_tenant_intake_scope(&[
            myelin_tenancy::TenantId("acme".into()),
            myelin_tenancy::TenantId("globex".into()),
        ]);
        assert!(scope("myelin://acme/chat/channel/01J0C"));
        assert!(scope("myelin://globex/ci/run/r1"));
        assert!(!scope("myelin://initech/ci/run/r1"), "unhosted tenant");
        assert!(
            !scope("myelin://acme-evil/chat/channel/01J0C"),
            "a tenant id prefix must not smuggle another tenant into scope"
        );
        assert!(!scope("myelin://acme"), "no trailing slash, no scope");

        let empty = placed_tenant_intake_scope(&[]);
        assert!(
            !empty("myelin://acme/chat/channel/01J0C"),
            "a cell hosting no tenants is in scope for nothing"
        );
    }
}
