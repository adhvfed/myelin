//! # `notif` — the Notifications service binary (NOTIF-P1 → P-127)
//!
//! The "every service `main.rs`" the contract-index row 1.1 names: it composes the DURABLE
//! composition root (MR-009b W3b.4) and hands the durable tenant-bound routers + JetStream intake
//! to [`run_notif_ingestion_until_shutdown`](myelin_notif::run_notif_ingestion_until_shutdown), a
//! thin wrapper over `serve_until_shutdown`. The harness owns the lifecycle (boot → migrate →
//! intake → three ports → graceful drain, with liveness ≠ readiness); this `main` composes and
//! hands off — no hand-rolled lifecycle logic.
//!
//! **DURABLE-BY-DEFAULT (MR-009b W3b.4 / SI-007):** the outbox the relay drains is the PG-backed
//! `outbox` table (`OutboxStore::durable(PgOutboxBacking)`) over the MR-022 `SubstrateProvider`
//! pool, with the substrate foundation migrations (`outbox` + `consumer_dedup`) applied at boot —
//! committed events survive a process restart. Production boot requires the complete endpoint
//! contract and distinct migration/runtime database credentials. The privileged pool applies every
//! migration and is closed before the runtime provider or outbox is constructed.
//!
//! The runtime is the multi-thread `#[tokio::main]` flavor (required): the sync
//! `DurableOutboxBacking` verbs bridge to async sqlx via `block_in_place` + `block_on`, which
//! panics on a current-thread runtime.
//!
//! A failed boot / incomplete drain returns non-zero (§3.1) — loud, never a silent success.

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
use myelin_substrate::Config;
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
    // The substrate foundation tables (the frozen `outbox` + `consumer_dedup` DDL) must exist
    // before the durable store binds — applied through the MR-022 migrator (idempotent,
    // forward-only, advisory-locked). Foundation is deliberately first.
    if let Err(e) = bootstrap.migrate_foundation().await {
        eprintln!(
            "notif: cannot apply the substrate foundation migrations (outbox/consumer_dedup): {e}"
        );
        std::process::exit(1);
    }
    // W7.2 (doc-18 Part 5) — THE BOOT-MIGRATIONS FIX: apply the FULL durable migration aggregate
    // (identity 0010–0019, pseudonym 0020–0022, placement 0030–0039, kms 0040–0042, cost/erasure
    // 0050–0053) after the foundation, so EVERY durable store bound at this main's boot has its
    // tables on a fresh DB (doc-18: a main that migrated only a piecemeal subset left the stores it
    // constructs writing to un-migrated tables). Idempotent + advisory-locked (safe on re-boot);
    // FAIL LOUD, never a silent fallback.
    if let Err(e) = bootstrap
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
    {
        eprintln!("notif: cannot apply the durable migration aggregate (identity/pseudonym/placement/kms/cost/erasure): {e}");
        std::process::exit(1);
    }
    // The AppSpec declaration does not run SQL; create all nine Notif RLS tables now.
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
    // The DURABLE outbox (SI-007): committed events live in Postgres, not a per-process mutex.
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
        ),
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
    // The env-first `Config::from_env()` parse for the substrate AppSpec config is P-S15; the
    // shell boots over the validated default today (the durable config is the provider's above).
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
            // A failed boot / incomplete drain returns non-zero (§3.1) — loud, never swallowed.
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
