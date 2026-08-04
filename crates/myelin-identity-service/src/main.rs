use myelin_config::Mode;
use myelin_events::OutboxStore;
use myelin_identity_service::{identity_app_spec, identity_service_migrations};
use myelin_storage::{all_durable_migrations, HotTables, PgBootstrap, PgOutboxBacking};
use myelin_substrate::{serve_until_shutdown, Config};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    myelin_events::install_payload_free_panic_hook("identity");
    let bootstrap = match PgBootstrap::from_env(Mode::RequireEnv).await {
        Ok(bootstrap) => bootstrap,
        Err(e) => {
            eprintln!("identity: database bootstrap refused to start: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = bootstrap.migrate_foundation().await {
        eprintln!("identity: cannot apply the substrate foundation migrations (outbox/consumer_dedup): {e}");
        std::process::exit(1);
    }
    if let Err(e) = bootstrap
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
    {
        eprintln!("identity: cannot apply the durable migration aggregate (identity/pseudonym/placement/kms/cost/erasure): {e}");
        std::process::exit(1);
    }
    if let Err(e) = bootstrap
        .migrate(&identity_service_migrations(), &HotTables::none())
        .await
    {
        eprintln!("identity: cannot apply the service-owned migrations: {e}");
        std::process::exit(1);
    }
    let provider = match bootstrap.into_runtime().await {
        Ok(provider) => provider,
        Err(e) => {
            eprintln!("identity: database runtime handoff refused to start: {e}");
            std::process::exit(1);
        }
    };
    let outbox = OutboxStore::durable(Arc::new(PgOutboxBacking::new(
        provider.db_pool().clone(),
        tokio::runtime::Handle::current(),
    )));
    match serve_until_shutdown(
        identity_app_spec(Config::default(), outbox),
        shutdown_signal(),
    )
    .await
    {
        Ok(()) => {}
        Err(e) => {
            eprintln!("identity service failed: {e}");
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
                    eprintln!("identity: failed to install SIGTERM handler: {error}");
                    std::process::exit(1);
                });
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                if let Err(error) = result {
                    eprintln!("identity: failed while waiting for SIGINT: {error}");
                    std::process::exit(1);
                }
            }
            signal = terminate.recv() => {
                if signal.is_none() {
                    eprintln!("identity: SIGTERM stream closed unexpectedly");
                    std::process::exit(1);
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        if let Err(error) = tokio::signal::ctrl_c().await {
            eprintln!("identity: failed while waiting for shutdown signal: {error}");
            std::process::exit(1);
        }
    }
}
