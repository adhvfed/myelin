use myelin_config::Mode;
use myelin_events::nats::{JetStreamConsumerConfig, NatsJetStreamBus};
use myelin_events::{DedupLedger, OutboxStore};
use myelin_git::check_status_store::{
    build_durable_check_consumer, check_status_hot_tables, check_status_migrations,
};
use myelin_storage::{all_durable_migrations, HotTables, PgBootstrap, PgOutboxBacking};
use myelin_substrate::{
    AppSpec, Config, ConsumerReg, CriticalDependencies, InternalRpc, OutboxSpec, PublicRoutes,
    StoreManifest,
};
use std::sync::Arc;

const SERVICE_NAME: &str = "git-check-projection";
const EVENT_STREAM_NAME: &str = "MYELIN_EVENTS";
const EVENT_SUBJECT_ROOT: &str = "myelin.events";
const EVENT_DURABLE_CONSUMER: &str = "git-check-status";

fn check_intake_filter() -> String {
    format!("{EVENT_SUBJECT_ROOT}.evt.*.ci.check.*.updated")
}

#[tokio::main]
async fn main() {
    myelin_events::install_payload_free_panic_hook(SERVICE_NAME);
    let bootstrap = PgBootstrap::from_env(Mode::RequireEnv)
        .await
        .unwrap_or_else(|error| {
            eprintln!("{SERVICE_NAME}: database bootstrap refused to start: {error}");
            std::process::exit(1);
        });
    if let Err(error) = bootstrap.migrate_foundation().await {
        eprintln!("{SERVICE_NAME}: cannot apply foundation migrations: {error}");
        std::process::exit(1);
    }
    if let Err(error) = bootstrap
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
    {
        eprintln!("{SERVICE_NAME}: cannot apply durable migrations: {error}");
        std::process::exit(1);
    }
    if let Err(error) = bootstrap
        .migrate(&check_status_migrations(), &check_status_hot_tables())
        .await
    {
        eprintln!("{SERVICE_NAME}: cannot apply Git check migrations: {error}");
        std::process::exit(1);
    }
    let provider = bootstrap.into_runtime().await.unwrap_or_else(|error| {
        eprintln!("{SERVICE_NAME}: database runtime handoff refused: {error}");
        std::process::exit(1);
    });
    let runtime = tokio::runtime::Handle::current();
    let outbox = OutboxStore::durable(Arc::new(PgOutboxBacking::new(
        provider.db_pool().clone(),
        runtime.clone(),
    )));
    let dedup = DedupLedger::durable(Arc::new(
        myelin_storage::events_durable::DurableDedupBacking::new(
            provider.db_pool().clone(),
            runtime.clone(),
        ),
    ));
    let dead_letters: Arc<dyn myelin_events::DurableDeadLetter> = Arc::new(
        myelin_storage::events_durable::DurableDeadLetterBacking::new(
            provider.db_pool().clone(),
            runtime.clone(),
        ),
    );
    let consumer = build_durable_check_consumer(
        runtime.clone(),
        provider.config().region.clone(),
        dedup,
        dead_letters,
    )
    .unwrap_or_else(|error| {
        eprintln!("{SERVICE_NAME}: cannot register check consumer: {error:?}");
        std::process::exit(1);
    });
    let intake = NatsJetStreamBus::connect_consumer(
        JetStreamConsumerConfig::bounded(
            &provider.config().nats_url,
            EVENT_STREAM_NAME,
            EVENT_SUBJECT_ROOT,
            check_intake_filter(),
            EVENT_DURABLE_CONSUMER,
        ),
        runtime.clone(),
    )
    .unwrap_or_else(|_| {
        eprintln!("{SERVICE_NAME}: cannot bind durable check intake");
        std::process::exit(1);
    });
    let quarantine: Arc<dyn myelin_events::DurableDeliveryQuarantine> = Arc::new(
        myelin_storage::events_durable::DurableDeliveryQuarantineBacking::new(
            provider.db_pool().clone(),
            runtime,
        ),
    );
    let spec = AppSpec {
        name: SERVICE_NAME,
        config: Config::default(),
        migrations: check_status_migrations(),
        hot_tables: check_status_hot_tables(),
        public: PublicRoutes::default(),
        internal: InternalRpc::default(),
        consumers: vec![ConsumerReg::new(consumer)],
        holders: AppSpec::auto(),
        stores: StoreManifest::new(),
        outbox: OutboxSpec::external_relay_with_consumer(outbox, Box::new(intake), quarantine),
        critical: CriticalDependencies::default(),
    };
    if let Err(error) = myelin_substrate::serve_until_shutdown(spec, shutdown_signal()).await {
        eprintln!("{SERVICE_NAME}: service failed: {error}");
        std::process::exit(1);
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .unwrap_or_else(|error| {
                    eprintln!("{SERVICE_NAME}: cannot install SIGTERM handler: {error}");
                    std::process::exit(1);
                });
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                if let Err(error) = result {
                    eprintln!("{SERVICE_NAME}: SIGINT listener failed: {error}");
                    std::process::exit(1);
                }
            }
            signal = terminate.recv() => {
                if signal.is_none() {
                    eprintln!("{SERVICE_NAME}: SIGTERM stream closed");
                    std::process::exit(1);
                }
            }
        }
    }
    #[cfg(not(unix))]
    if let Err(error) = tokio::signal::ctrl_c().await {
        eprintln!("{SERVICE_NAME}: shutdown listener failed: {error}");
        std::process::exit(1);
    }
}
