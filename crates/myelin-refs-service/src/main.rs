use std::sync::Arc;

use myelin_config::Mode;
use myelin_events::nats::{JetStreamConsumerConfig, NatsJetStreamBus};
use myelin_events::{DedupLedger, OutboxStore};
use myelin_refs_service::{
    build_pg_cell_edge_consumer, edge_table_migrations, refs_intake_filter,
    run_refs_ingestion_until_shutdown, PgEdgeStore, EVENT_DURABLE_CONSUMER, EVENT_STREAM_NAME,
    EVENT_SUBJECT_ROOT,
};
use myelin_storage::{
    all_durable_migrations, DurablePlacementBacking, HotTables, PgBootstrap, PgOutboxBacking,
};
use myelin_substrate::{Config, ConsumerReg, Thresholds};

#[tokio::main]
async fn main() {
    myelin_events::install_payload_free_panic_hook("refs");
    let bootstrap = PgBootstrap::from_env(Mode::RequireEnv)
        .await
        .unwrap_or_else(|error| refuse_start("database bootstrap", error));
    bootstrap
        .migrate_foundation()
        .await
        .unwrap_or_else(|error| refuse_start("substrate foundation migration", error));
    bootstrap
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
        .unwrap_or_else(|error| refuse_start("durable migration aggregate", error));
    bootstrap
        .migrate(&edge_table_migrations(), &HotTables::none())
        .await
        .unwrap_or_else(|error| refuse_start("Refs edge migration", error));
    let provider = bootstrap
        .into_runtime()
        .await
        .unwrap_or_else(|error| refuse_start("database runtime handoff", error));

    let cell_id = required_cell_id().unwrap_or_else(|error| refuse_start("cell binding", error));
    let cell = DurablePlacementBacking::new(provider.db_pool().clone())
        .get_cell(&cell_id)
        .await
        .unwrap_or_else(|error| refuse_start("cell directory", error))
        .unwrap_or_else(|| {
            refuse_start(
                "cell binding",
                format!("cell `{cell_id}` is absent from the placement directory"),
            )
        });
    if cell.status != "Active" {
        refuse_start(
            "cell binding",
            format!("cell `{cell_id}` is `{}` instead of Active", cell.status),
        );
    }
    if cell.region != provider.config().region {
        refuse_start(
            "cell binding",
            format!(
                "cell `{cell_id}` belongs to region `{}` instead of `{}`",
                cell.region,
                provider.config().region
            ),
        );
    }

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
    let store = PgEdgeStore::new(provider.db_pool().clone());
    let region = myelin_tenancy::Region(provider.config().region.clone());
    let admission = Thresholds::load_canonical()
        .and_then(|thresholds| thresholds.worker_admission("refs-edge-builder"))
        .unwrap_or_else(|error| refuse_start("worker admission", error));
    let consumers = vec![build_pg_cell_edge_consumer(
        &cell_id,
        &region,
        store,
        dedup,
        dead_letters,
        runtime.clone(),
        admission,
    )
    .map(ConsumerReg::new)
    .unwrap_or_else(|error| refuse_start("cell-bound edge consumer", format!("{error:?}")))];
    let intake = NatsJetStreamBus::connect_consumer(
        JetStreamConsumerConfig::bounded(
            &provider.config().nats_url,
            EVENT_STREAM_NAME,
            EVENT_SUBJECT_ROOT,
            refs_intake_filter(),
            EVENT_DURABLE_CONSUMER,
        )
        .with_admission(admission),
        runtime.clone(),
    )
    .unwrap_or_else(|error| refuse_start("durable event intake", format!("{error:?}")));
    let quarantine: Arc<dyn myelin_events::DurableDeliveryQuarantine> = Arc::new(
        myelin_storage::events_durable::DurableDeliveryQuarantineBacking::new(
            provider.db_pool().clone(),
            runtime,
        ),
    );

    run_refs_ingestion_until_shutdown(
        Config::default(),
        outbox,
        consumers,
        Box::new(intake),
        quarantine,
        shutdown_signal(),
    )
    .await
    .unwrap_or_else(|error| refuse_start("service loop", error));
}

fn required_cell_id() -> Result<String, &'static str> {
    let value = std::env::var("MYELIN_CELL_ID").map_err(|_| "MYELIN_CELL_ID is required")?;
    if value.is_empty()
        || value.len() > 128
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err("MYELIN_CELL_ID must be a trimmed opaque token of at most 128 bytes");
    }
    Ok(value)
}

fn refuse_start(context: &str, error: impl std::fmt::Display) -> ! {
    eprintln!("refs: {context} refused to start: {error}");
    std::process::exit(1)
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .unwrap_or_else(|error| refuse_start("SIGTERM handler", error));
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                if let Err(error) = result {
                    refuse_start("SIGINT handler", error);
                }
            }
            signal = terminate.recv() => {
                if signal.is_none() {
                    refuse_start("SIGTERM stream", "closed unexpectedly");
                }
            }
        }
    }
    #[cfg(not(unix))]
    if let Err(error) = tokio::signal::ctrl_c().await {
        refuse_start("shutdown handler", error);
    }
}
