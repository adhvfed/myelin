use myelin_ci_dispatch::{
    git_intake_filter, run_dispatch_until_shutdown, AuthoritativeGitRoot, RecoveringIntake,
    EVENT_DURABLE_CONSUMER, EVENT_STREAM_NAME, EVENT_SUBJECT_ROOT,
};
use myelin_config::Mode;
use myelin_events::nats::JetStreamConsumerConfig;
use myelin_events::OutboxStore;
use myelin_storage::{all_durable_migrations, HotTables, PgBootstrap, PgOutboxBacking};
use myelin_substrate::Config;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    myelin_events::install_payload_free_panic_hook("ci-dispatch");
    let bootstrap = match PgBootstrap::from_env(Mode::RequireEnv).await {
        Ok(bootstrap) => bootstrap,
        Err(e) => {
            eprintln!("ci-dispatch: database bootstrap refused to start: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = bootstrap.migrate_foundation().await {
        eprintln!(
            "ci-dispatch: cannot apply the substrate foundation migrations \
             (outbox/consumer_dedup): {e}"
        );
        std::process::exit(1);
    }
    if let Err(e) = bootstrap
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
    {
        eprintln!("ci-dispatch: cannot apply the durable migration aggregate (identity/pseudonym/placement/kms/cost/erasure): {e}");
        std::process::exit(1);
    }
    if let Err(e) = bootstrap
        .migrate(
            &myelin_ci_controlplane::ci_durable_migrations(),
            &myelin_ci_controlplane::ci_durable_hot_tables(),
        )
        .await
    {
        eprintln!("ci-dispatch: cannot apply the shared CI durable migrations (ci_run/check_attempt/ci_cost_event): {e}");
        std::process::exit(1);
    }
    if let Err(e) = bootstrap
        .migrate(
            &myelin_ci_dispatch::dispatch_migrations(),
            &HotTables::none(),
        )
        .await
    {
        eprintln!("ci-dispatch: cannot apply the Dispatch service migrations: {e}");
        std::process::exit(1);
    }
    let provider = match bootstrap.into_runtime().await {
        Ok(provider) => provider,
        Err(e) => {
            eprintln!("ci-dispatch: database runtime handoff refused to start: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = myelin_ci_controlplane::verify_ci_cost_event_shape(provider.db_pool()).await {
        eprintln!("ci-dispatch: ci_cost_event shape assertion failed: {e}");
        std::process::exit(1);
    }
    let outbox = OutboxStore::durable(Arc::new(PgOutboxBacking::new(
        provider.db_pool().clone(),
        tokio::runtime::Handle::current(),
    )));
    let git_root = std::env::var("MYELIN_GIT_ROOT")
        .map_err(|_| "MYELIN_GIT_ROOT is required".to_string())
        .and_then(|path| AuthoritativeGitRoot::validate(path).map_err(|error| error.to_string()))
        .unwrap_or_else(|message| {
            eprintln!("ci-dispatch: {message}; refusing broker intake");
            std::process::exit(1);
        });
    eprintln!(
        "ci-dispatch: authoritative Git reads use {} in cell region {}",
        git_root.as_path().display(),
        provider.config().region
    );
    let minter: Arc<dyn myelin_events::IdMinter> = Arc::new(myelin_events::UlidMinter::new());
    let ci_run = myelin_ci_controlplane::ci_run_store_factory(provider.db_pool().clone());
    let dedup = myelin_events::DedupLedger::durable(Arc::new(
        myelin_storage::events_durable::DurableDedupBacking::new(
            provider.db_pool().clone(),
            tokio::runtime::Handle::current(),
        ),
    ) as Arc<dyn myelin_events::DurableDedup>);
    let dead_letters: Arc<dyn myelin_events::DurableDeadLetter> = Arc::new(
        myelin_storage::events_durable::DurableDeadLetterBacking::new(
            provider.db_pool().clone(),
            tokio::runtime::Handle::current(),
        ),
    );
    let blobs = Arc::new(myelin_storage::s3blob::S3BlobStore::connect(
        &provider.config().s3,
        tokio::runtime::Handle::current(),
    ));
    match blobs.preflight() {
        Ok(()) => {}
        Err(
            error @ (myelin_storage::blob::BlobDependencyError::PermanentConfig
            | myelin_storage::blob::BlobDependencyError::PermanentAuth),
        ) => {
            eprintln!("ci-dispatch: {error}; refusing broker intake");
            std::process::exit(1);
        }
        Err(myelin_storage::blob::BlobDependencyError::Transient) => {
            eprintln!("ci-dispatch: object-store dependency is temporarily unavailable; starting not-ready");
        }
    }
    let consumers = myelin_ci_dispatch::build_dispatch_consumers(
        git_root,
        blobs.clone(),
        ci_run,
        dedup,
        dead_letters,
        provider.config().region.clone(),
        minter,
        tokio::runtime::Handle::current(),
    )
    .unwrap_or_else(|e| {
        eprintln!("ci-dispatch: cannot register the ci-dispatch.trigger consumer: {e:?}");
        std::process::exit(1);
    });

    let intake = RecoveringIntake::new(
        JetStreamConsumerConfig::bounded(
            &provider.config().nats_url,
            EVENT_STREAM_NAME,
            EVENT_SUBJECT_ROOT,
            git_intake_filter(),
            EVENT_DURABLE_CONSUMER,
        ),
        blobs,
        tokio::runtime::Handle::current(),
    );
    let delivery_quarantine: Arc<dyn myelin_events::DurableDeliveryQuarantine> = Arc::new(
        myelin_storage::events_durable::DurableDeliveryQuarantineBacking::new(
            provider.db_pool().clone(),
            tokio::runtime::Handle::current(),
        ),
    );

    match run_dispatch_until_shutdown(
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
            eprintln!("ci-dispatch service failed: {e}");
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
                    eprintln!("ci-dispatch: failed to install SIGTERM handler: {error}");
                    std::process::exit(1);
                });
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                if let Err(error) = result {
                    eprintln!("ci-dispatch: failed while waiting for SIGINT: {error}");
                    std::process::exit(1);
                }
            }
            signal = terminate.recv() => {
                if signal.is_none() {
                    eprintln!("ci-dispatch: SIGTERM stream closed unexpectedly");
                    std::process::exit(1);
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        if let Err(error) = tokio::signal::ctrl_c().await {
            eprintln!("ci-dispatch: failed while waiting for shutdown signal: {error}");
            std::process::exit(1);
        }
    }
}
