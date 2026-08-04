use myelin_config::Mode;
use myelin_events::OutboxStore;
use myelin_knowledge::{knowledge_app_spec, knowledge_service_migrations, HOT_TABLES};
use myelin_storage::{all_durable_migrations, HotTables, PgBootstrap, PgOutboxBacking};
use myelin_substrate::{serve_until_shutdown, Config};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    myelin_events::install_payload_free_panic_hook("knowledge");
    let bootstrap = match PgBootstrap::from_env(Mode::RequireEnv).await {
        Ok(bootstrap) => bootstrap,
        Err(e) => {
            eprintln!("knowledge: database bootstrap refused to start: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = bootstrap.migrate_foundation().await {
        eprintln!(
            "knowledge: cannot apply the substrate foundation migrations (outbox/consumer_dedup): {e}"
        );
        std::process::exit(1);
    }
    if let Err(e) = bootstrap
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
    {
        eprintln!("knowledge: cannot apply the durable migration aggregate (identity/pseudonym/placement/kms/cost/erasure): {e}");
        std::process::exit(1);
    }
    if let Err(e) = bootstrap
        .migrate(
            &knowledge_service_migrations(),
            &HotTables::declare(HOT_TABLES),
        )
        .await
    {
        eprintln!("knowledge: cannot apply the service-owned migrations: {e}");
        std::process::exit(1);
    }
    let provider = match bootstrap.into_runtime().await {
        Ok(provider) => provider,
        Err(e) => {
            eprintln!("knowledge: database runtime handoff refused to start: {e}");
            std::process::exit(1);
        }
    };
    let outbox = OutboxStore::durable(Arc::new(PgOutboxBacking::new(
        provider.db_pool().clone(),
        tokio::runtime::Handle::current(),
    )));
    match serve_until_shutdown(
        knowledge_app_spec(Config::default(), outbox),
        shutdown_signal(),
    )
    .await
    {
        Ok(()) => {}
        Err(e) => {
            eprintln!("knowledge service failed: {e}");
            std::process::exit(1);
        }
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .unwrap_or_else(|error| {
                    eprintln!("knowledge: failed to install SIGTERM handler: {error}");
                    std::process::exit(1);
                });
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                if let Err(error) = result {
                    eprintln!("knowledge: failed while waiting for SIGINT: {error}");
                    std::process::exit(1);
                }
            }
            signal = terminate.recv() => {
                if signal.is_none() {
                    eprintln!("knowledge: SIGTERM stream closed unexpectedly");
                    std::process::exit(1);
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        if let Err(error) = tokio::signal::ctrl_c().await {
            eprintln!("knowledge: failed while waiting for shutdown signal: {error}");
            std::process::exit(1);
        }
    }
}
