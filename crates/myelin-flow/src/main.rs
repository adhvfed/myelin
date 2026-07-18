//! # `myelin-flow` — the durable-workflow service binary (P-FLOW-02 → P-198, M2)
//!
//! The "every service `main.rs`" the contract-index row 1.1 names: it composes the DURABLE
//! composition root (MR-009b W3b.4), boots the harness-owned surfaces, and keeps the scoped
//! PostgreSQL workflow worker alive until an OS drain signal. The harness still owns boot, ports,
//! relay metadata, and final graceful drain; the worker owns only its bounded polling lifecycle.
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
    let bootstrap = match PgBootstrap::from_env(Mode::RequireEnv).await {
        Ok(bootstrap) => bootstrap,
        Err(e) => {
            eprintln!("myelin-flow: database bootstrap refused to start: {e}");
            std::process::exit(1);
        }
    };
    // The substrate foundation tables (the frozen `outbox` + `consumer_dedup` DDL) must exist
    // before the durable store binds — applied through the MR-022 migrator (idempotent,
    // forward-only, advisory-locked). Foundation is deliberately first.
    if let Err(e) = bootstrap.migrate_foundation().await {
        eprintln!(
            "myelin-flow: cannot apply the substrate foundation migrations (outbox/consumer_dedup): {e}"
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
        eprintln!("myelin-flow: cannot apply the durable migration aggregate (identity/pseudonym/placement/kms/cost/erasure): {e}");
        std::process::exit(1);
    }
    // The AppSpec declaration is lifecycle metadata; production executes the six-table flow schema.
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
    // A worker is always pinned to one explicitly configured tenant, residency region, and
    // partition. Missing scope is a boot failure; this binary never discovers/scans all tenants.
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
        // This compiled body is deliberately an OPERATIONAL PROBE, not a product workflow stub.
        // Unsupported configured product definitions fail above. ci.pipeline/merge/maintenance
        // must be composed with their owning subsystem adapters before this binary can claim them.
        if wf_type == OPERATIONAL_PROBE_WF_TYPE && version == 1 {
            let code_hash = blake3::hash(b"myelin.flow.operational-probe@1:returns-empty").to_hex();
            if let Err(e) = worker.register_definition(
                OPERATIONAL_PROBE_WF_TYPE,
                1,
                code_hash.as_str(),
                |_ctx| Ok(Vec::new()),
            ) {
                eprintln!("myelin-flow: cannot register operational probe definition: {e}");
                std::process::exit(1);
            }
        }
    }
    // The DURABLE outbox (SI-007): committed events live in Postgres, not a per-process mutex.
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
    let early_worker_result = loop {
        tokio::select! {
            signal = tokio::signal::ctrl_c() => {
                if let Err(e) = signal {
                    eprintln!("myelin-flow: cannot install shutdown signal handler: {e}");
                }
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
}
