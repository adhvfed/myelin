//! # `myelin-flow` — the durable-workflow service binary (P-FLOW-02 → P-198, M2)
//!
//! The "every service `main.rs`" the contract-index row 1.1 names: it composes the DURABLE
//! composition root (MR-009b W3b.4) and hands the flow [`AppSpec`](myelin_flow::flow_app_spec) to
//! the harness's one call, [`run_flow`](myelin_flow::run_flow) (a thin wrapper over `serve`). The
//! harness owns the whole lifecycle (boot → migrate → outbox relay → consumers → three ports →
//! graceful drain, with liveness ≠ readiness); this `main` composes and hands off — no
//! hand-rolled lifecycle logic (§10: the engine boots from `serve(AppSpec)`, there is no second
//! emit/boot path).
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
use myelin_events::OutboxStore;
use myelin_flow::{migrations::migrations as flow_migrations, run_flow};
use myelin_storage::{all_durable_migrations, HotTables, PgBootstrap, PgOutboxBacking};
use myelin_substrate::Config;
use std::sync::Arc;

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
    // The DURABLE outbox (SI-007): committed events live in Postgres, not a per-process mutex.
    let outbox = OutboxStore::durable(Arc::new(PgOutboxBacking::new(
        provider.db_pool().clone(),
        tokio::runtime::Handle::current(),
    )));
    // The env-first `Config::from_env()` parse for the substrate AppSpec config is P-S15; the
    // shell boots over the validated default today (the durable config is the provider's above).
    match run_flow(Config::default(), outbox) {
        Ok(()) => {}
        Err(e) => {
            // A failed boot / incomplete drain returns non-zero (§3.1) — loud, never swallowed.
            eprintln!("myelin-flow service failed: {e}");
            std::process::exit(1);
        }
    }
}
