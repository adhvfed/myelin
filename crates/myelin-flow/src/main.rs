use myelin_config::Mode;
use myelin_events::{OutboxStore, UlidMinter};
use myelin_flow::{
    boot_flow, configured_production_definitions, migrations::migrations as flow_migrations,
    PgFlowWorker, PgWorkerScope, OPERATIONAL_PROBE_WF_TYPE,
};
use myelin_storage::{all_durable_migrations, HotTables, PgBootstrap, PgOutboxBacking};
use myelin_substrate::Config;
use std::sync::Arc;
use std::time::Duration;

#[tokio::main]
async fn main() {
    myelin_events::install_payload_free_panic_hook("flow");
    let bootstrap = match PgBootstrap::from_env(Mode::RequireEnv).await {
        Ok(bootstrap) => bootstrap,
        Err(e) => {
            eprintln!("myelin-flow: database bootstrap refused to start: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = bootstrap.migrate_foundation().await {
        eprintln!(
            "myelin-flow: cannot apply the substrate foundation migrations (outbox/consumer_dedup): {e}"
        );
        std::process::exit(1);
    }
    if let Err(e) = bootstrap
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
    {
        eprintln!("myelin-flow: cannot apply the durable migration aggregate (identity/pseudonym/placement/kms/cost/erasure): {e}");
        std::process::exit(1);
    }
    if let Err(e) = bootstrap
        .migrate(&flow_migrations(), &HotTables::declare(["workflow_run"]))
        .await
    {
        eprintln!("myelin-flow: cannot apply the service-owned migrations: {e}");
        std::process::exit(1);
    }
    let provider = match bootstrap.into_runtime().await {
        Ok(provider) => provider,
        Err(e) => {
            eprintln!("myelin-flow: database runtime handoff refused to start: {e}");
            std::process::exit(1);
        }
    };
    let worker_scope = match PgWorkerScope::from_env() {
        Ok(scope) => scope,
        Err(e) => {
            eprintln!("myelin-flow: PostgreSQL worker scope refused to start: {e}");
            std::process::exit(1);
        }
    };
    let mut worker = PgFlowWorker::new(
        provider.db_pool().clone(),
        tokio::runtime::Handle::current(),
        Arc::new(UlidMinter::new()),
        worker_scope,
    );
    let definitions = match configured_production_definitions() {
        Ok(definitions) => definitions,
        Err(e) => {
            eprintln!("myelin-flow: configured workflow definitions refused to start: {e}");
            std::process::exit(1);
        }
    };
    for (wf_type, version) in definitions {
        if wf_type == OPERATIONAL_PROBE_WF_TYPE && version == 1 {
            let code_hash = blake3::hash(b"myelin.flow.operational-probe@1:returns-empty").to_hex();
            if let Err(e) = worker.register_definition(
                OPERATIONAL_PROBE_WF_TYPE,
                1,
                code_hash.as_str(),
                |_input, _ctx| Ok(Vec::new()),
            ) {
                eprintln!("myelin-flow: cannot register operational probe definition: {e}");
                std::process::exit(1);
            }
        }
    }
    let outbox = OutboxStore::durable(Arc::new(PgOutboxBacking::new(
        provider.db_pool().clone(),
        tokio::runtime::Handle::current(),
    )));
    let service = match boot_flow(Config::default(), outbox) {
        Ok(service) => service,
        Err(e) => {
            eprintln!("myelin-flow service boot failed: {e}");
            std::process::exit(1);
        }
    };
    let worker = Arc::new(worker);
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let worker_task = {
        let worker = Arc::clone(&worker);
        tokio::spawn(async move {
            worker
                .run_until_shutdown(shutdown_rx, Duration::from_millis(250), 32)
                .await
        })
    };
    let mut worker_task = worker_task;
    let mut service_tick = tokio::time::interval(Duration::from_millis(250));
    let mut shutdown_error = None;
    let early_worker_result = loop {
        tokio::select! {
            signal = shutdown_signal() => {
                shutdown_error = signal.err();
                break None;
            }
            result = &mut worker_task => break Some(result),
            _ = service_tick.tick() => { service.tick(); }
        }
    };
    let _ = shutdown_tx.send(true);
    let worker_result = match early_worker_result {
        Some(result) => Ok(result),
        None => tokio::time::timeout(Duration::from_secs(10), worker_task).await,
    };
    service.signal_drain();
    let _final_telemetry = service.drain();
    match worker_result {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(e))) => {
            eprintln!("myelin-flow PostgreSQL worker failed: {e}");
            std::process::exit(1);
        }
        Ok(Err(e)) => {
            eprintln!("myelin-flow PostgreSQL worker task failed: {e}");
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!("myelin-flow PostgreSQL worker did not drain within 10 seconds");
            std::process::exit(1);
        }
    }
    if let Some(error) = shutdown_error {
        eprintln!("myelin-flow shutdown signal failed: {error}");
        std::process::exit(1);
    }
}

async fn shutdown_signal() -> Result<(), String> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .map_err(|error| format!("failed to install SIGTERM handler: {error}"))?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                result.map_err(|error| format!("failed while waiting for SIGINT: {error}"))
            }
            signal = terminate.recv() => {
                signal
                    .map(|_| ())
                    .ok_or_else(|| "SIGTERM stream closed unexpectedly".to_string())
            }
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .map_err(|error| format!("failed while waiting for shutdown signal: {error}"))
    }
}
