//! # `knowledge` — the Knowledge service binary (KN-P04 → P-294, M3)
//!
//! The "every service `main.rs`" the contract-index row 1.1 names: it composes the DURABLE
//! composition root (MR-009b W3b.4) and hands the Knowledge
//! [`AppSpec`](myelin_knowledge::knowledge_app_spec) to the harness's one call,
//! [`serve`](myelin_substrate::serve). The harness owns the whole lifecycle (boot → migrate →
//! relay → consumers → three ports → graceful drain, with liveness ≠ readiness); this `main`
//! composes and hands off — no hand-rolled lifecycle logic (architecture 00 §3.1 / 03 §4).
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
use myelin_knowledge::{knowledge_app_spec, knowledge_service_migrations, HOT_TABLES};
use myelin_storage::{all_durable_migrations, HotTables, PgBootstrap, PgOutboxBacking};
use myelin_substrate::{serve, Config};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let bootstrap = match PgBootstrap::from_env(Mode::RequireEnv).await {
        Ok(bootstrap) => bootstrap,
        Err(e) => {
            eprintln!("knowledge: database bootstrap refused to start: {e}");
            std::process::exit(1);
        }
    };
    // The substrate foundation tables (the frozen `outbox` + `consumer_dedup` DDL) must exist
    // before the durable store binds — applied through the MR-022 migrator (idempotent,
    // forward-only, advisory-locked). Foundation is deliberately first.
    if let Err(e) = bootstrap.migrate_foundation().await {
        eprintln!(
            "knowledge: cannot apply the substrate foundation migrations (outbox/consumer_dedup): {e}"
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
        eprintln!("knowledge: cannot apply the durable migration aggregate (identity/pseudonym/placement/kms/cost/erasure): {e}");
        std::process::exit(1);
    }
    // Apply the marker and complete Knowledge OLTP schema with the AppSpec's exact hot-table guard.
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
    // The DURABLE outbox (SI-007): committed events live in Postgres, not a per-process mutex.
    let outbox = OutboxStore::durable(Arc::new(PgOutboxBacking::new(
        provider.db_pool().clone(),
        tokio::runtime::Handle::current(),
    )));
    // The env-first `Config::from_env()` parse for the substrate AppSpec config is P-S15; the
    // shell boots over the validated default today (the durable config is the provider's above).
    match serve(knowledge_app_spec(Config::default(), outbox)) {
        Ok(()) => {}
        Err(e) => {
            // A failed boot / incomplete drain returns non-zero (§3.1) — loud, never swallowed.
            eprintln!("knowledge service failed: {e}");
            std::process::exit(1);
        }
    }
}
