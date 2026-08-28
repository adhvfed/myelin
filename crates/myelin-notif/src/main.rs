use myelin_config::Mode;
use myelin_events::nats::{JetStreamConsumerConfig, NatsJetStreamBus};
use myelin_events::{DedupLedger, OutboxStore, UlidMinter};
use myelin_notif::pg_inbox::PgInboxStore;
use myelin_notif::{
    build_durable_router, migrations::migrations as notif_migrations,
    run_notif_ingestion_until_shutdown, signal_intake_filter, EVENT_DURABLE_CONSUMER,
    EVENT_STREAM_NAME, EVENT_SUBJECT_ROOT,
};
use myelin_storage::{
    all_durable_migrations, DurablePlacementBacking, HotTables, PgBootstrap, PgOutboxBacking,
};
use myelin_substrate::{Config, Thresholds};
use myelin_tenancy::TenantId;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    myelin_events::install_payload_free_panic_hook("notif");
    let bootstrap = match PgBootstrap::from_env(Mode::RequireEnv).await {
        Ok(bootstrap) => bootstrap,
        Err(e) => {
            eprintln!("notif: database bootstrap refused to start: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = bootstrap.migrate_foundation().await {
        eprintln!(
            "notif: cannot apply the substrate foundation migrations (outbox/consumer_dedup): {e}"
        );
        std::process::exit(1);
    }
    if let Err(e) = bootstrap
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
    {
        eprintln!("notif: cannot apply the durable migration aggregate (identity/pseudonym/placement/kms/cost/erasure): {e}");
        std::process::exit(1);
    }
    if let Err(e) = bootstrap
        .migrate(&notif_migrations(), &HotTables::none())
        .await
    {
        eprintln!("notif: cannot apply the service-owned migrations: {e}");
        std::process::exit(1);
    }
    let provider = match bootstrap.into_runtime().await {
        Ok(provider) => provider,
        Err(e) => {
            eprintln!("notif: database runtime handoff refused to start: {e}");
            std::process::exit(1);
        }
    };
    let outbox = OutboxStore::durable(Arc::new(PgOutboxBacking::new(
        provider.db_pool().clone(),
        tokio::runtime::Handle::current(),
    )));
    let cell_id = match required_cell_id() {
        Ok(cell_id) => cell_id,
        Err(error) => {
            eprintln!("notif: {error}");
            std::process::exit(1);
        }
    };
    let local_tenants = match DurablePlacementBacking::new(provider.db_pool().clone())
        .local_tenants(&cell_id)
        .await
    {
        Ok(rows) => rows
            .into_iter()
            .filter(|row| row.active)
            .map(|row| TenantId(row.tenant_id))
            .collect::<Vec<_>>(),
        Err(error) => {
            eprintln!("notif: cannot load the active local-tenant directory: {error}");
            std::process::exit(1);
        }
    };
    if local_tenants.is_empty() {
        eprintln!(
            "notif: cell `{cell_id}` has no active local tenants; refusing broad signal intake"
        );
        std::process::exit(1);
    }

    let runtime = tokio::runtime::Handle::current();
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
    let minter: Arc<dyn myelin_events::IdMinter> = Arc::new(UlidMinter::new());
    let inbox = PgInboxStore::new(provider.db_pool().clone());
    let admission = Thresholds::load_canonical()
        .and_then(|thresholds| thresholds.worker_admission("notification-signal-router"))
        .unwrap_or_else(|error| {
            eprintln!("notif: worker admission refused to start: {error}");
            std::process::exit(1);
        });
    let mut consumers = Vec::with_capacity(local_tenants.len());
    for tenant in &local_tenants {
        let consumer = build_durable_router(
            tenant,
            provider.config().region.clone(),
            inbox.clone(),
            outbox.clone(),
            dedup.clone(),
            dead_letters.clone(),
            minter.clone(),
            runtime.clone(),
            admission,
        )
        .unwrap_or_else(|error| {
            eprintln!(
                "notif: cannot register the tenant-bound signal router for `{}`: {error:?}",
                tenant.0
            );
            std::process::exit(1);
        });
        consumers.push(myelin_substrate::ConsumerReg::new(consumer));
    }

    let intake = NatsJetStreamBus::connect_consumer(
        JetStreamConsumerConfig::bounded(
            &provider.config().nats_url,
            EVENT_STREAM_NAME,
            EVENT_SUBJECT_ROOT,
            signal_intake_filter(),
            EVENT_DURABLE_CONSUMER,
        )
        .with_admission(admission),
        runtime.clone(),
    )
    .unwrap_or_else(|_error| {
        eprintln!("notif: cannot bind durable signal intake");
        std::process::exit(1);
    });
    let delivery_quarantine: Arc<dyn myelin_events::DurableDeliveryQuarantine> = Arc::new(
        myelin_storage::events_durable::DurableDeliveryQuarantineBacking::new(
            provider.db_pool().clone(),
            runtime,
        ),
    );
    match run_notif_ingestion_until_shutdown(
        Config::default(),
        outbox,
        consumers,
        Box::new(intake),
        delivery_quarantine,
        shutdown_signal(),
    )
    .await
    {
        Ok(()) => {}
        Err(e) => {
            eprintln!("notif service failed: {e}");
            std::process::exit(1);
        }
    }
}

fn required_cell_id() -> Result<String, &'static str> {
    let value = std::env::var("MYELIN_CELL_ID")
        .map_err(|_| "MYELIN_CELL_ID is required for tenant-bound signal intake")?;
    if value.is_empty()
        || value.len() > 128
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(
            "MYELIN_CELL_ID must be a trimmed, non-empty opaque token of at most 128 bytes",
        );
    }
    Ok(value)
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .unwrap_or_else(|error| {
                    eprintln!("notif: failed to install SIGTERM handler: {error}");
                    std::process::exit(1);
                });
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                if let Err(error) = result {
                    eprintln!("notif: failed while waiting for SIGINT: {error}");
                    std::process::exit(1);
                }
            }
            signal = terminate.recv() => {
                if signal.is_none() {
                    eprintln!("notif: SIGTERM stream closed unexpectedly");
                    std::process::exit(1);
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        if let Err(error) = tokio::signal::ctrl_c().await {
            eprintln!("notif: failed while waiting for shutdown signal: {error}");
            std::process::exit(1);
        }
    }
}
