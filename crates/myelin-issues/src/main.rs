//! # `issues` — the Issue Tracker service binary (ISS-P05 → P-371, M4)
//!
//! The "every service `main.rs`" the contract-index row 1.1 names: it composes the DURABLE
//! composition root (MR-009b W3b.4) and hands the Issue Tracker
//! [`AppSpec`](myelin_issues::issues_app_spec) to the harness's one call,
//! [`run_issues_until_shutdown`](myelin_issues::run_issues_until_shutdown) (a thin wrapper over
//! `serve_until_shutdown`). The harness owns the
//! whole lifecycle (boot → migrate → outbox relay → consumers → three ports → graceful drain, with
//! liveness ≠ readiness); this `main` composes and hands off — no hand-rolled lifecycle logic.
//!
//! **DURABLE-BY-DEFAULT (MR-009b W3b.4 / SI-007):** the outbox the relay drains is the PG-backed
//! `outbox` table (`OutboxStore::durable(PgOutboxBacking)`) over the MR-022 `SubstrateProvider`
//! pool, with the substrate foundation migrations (`outbox` + `consumer_dedup`) applied at boot —
//! committed events survive a process restart. Production boot requires the complete endpoint
//! contract and distinct `DATABASE_MIGRATION_URL` / `DATABASE_URL` credentials. Only the bootstrap
//! owns the privileged pool; it closes that pool before returning the constrained runtime provider.
//!
//! The runtime is the multi-thread `#[tokio::main]` flavor (required): the sync
//! `DurableOutboxBacking` verbs bridge to async sqlx via `block_in_place` + `block_on`, which
//! panics on a current-thread runtime.
//!
//! A failed boot / incomplete drain returns non-zero (§3.1) — loud, never a silent success.

use myelin_config::Mode;
use myelin_events::OutboxStore;
use myelin_issues::{issues_hot_tables, issues_migrations, run_issues_until_shutdown};
use myelin_storage::{all_durable_migrations, HotTables, PgBootstrap, PgOutboxBacking};
use myelin_substrate::Config;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    // Production is strict: validate every endpoint plus the split runtime/migration role pair
    // before any DDL or listener can be created. `PgBootstrap` alone owns the privileged pool.
    let bootstrap = match PgBootstrap::from_env(Mode::RequireEnv).await {
        Ok(bootstrap) => bootstrap,
        Err(e) => {
            eprintln!("issues: database bootstrap refused to start: {e}");
            std::process::exit(1);
        }
    };
    // The substrate foundation tables (the frozen `outbox` + `consumer_dedup` DDL) must exist
    // before the durable store binds — applied through the MR-022 migrator (idempotent,
    // forward-only, advisory-locked). Foundation is deliberately first; the durable aggregate and
    // Issues-owned migration set follow in that validated order.
    if let Err(e) = bootstrap.migrate_foundation().await {
        eprintln!(
            "issues: cannot apply the substrate foundation migrations (outbox/consumer_dedup): {e}"
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
        eprintln!("issues: cannot apply the durable migration aggregate (identity/pseudonym/placement/kms/cost/erasure): {e}");
        std::process::exit(1);
    }
    // Apply the Issues-owned spine through the REAL provider migrator. The AppSpec migration list
    // is also the DB-free lifecycle declaration, but it does not execute SQL; production boot must
    // validate + execute this set before any issue store is opened. Fail loud on any schema fault.
    if let Err(e) = bootstrap
        .migrate(&issues_migrations(), &issues_hot_tables())
        .await
    {
        eprintln!("issues: cannot apply the issue-spine migrations: {e}");
        std::process::exit(1);
    }
    // The handoff reconnects and re-validates the constrained role, closes the migration pool, and
    // erases its DSN from the retained config. No serving store is constructed before this point.
    let provider = match bootstrap.into_runtime().await {
        Ok(provider) => provider,
        Err(e) => {
            eprintln!("issues: database runtime handoff refused to start: {e}");
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
    match run_issues_until_shutdown(Config::default(), outbox, shutdown_signal()).await {
        Ok(()) => {}
        Err(e) => {
            // A failed boot / incomplete drain returns non-zero (§3.1) — loud, never swallowed.
            eprintln!("issues service failed: {e}");
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
                    eprintln!("issues: failed to install SIGTERM handler: {error}");
                    std::process::exit(1);
                });
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                if let Err(error) = result {
                    eprintln!("issues: failed while waiting for SIGINT: {error}");
                    std::process::exit(1);
                }
            }
            signal = terminate.recv() => {
                if signal.is_none() {
                    eprintln!("issues: SIGTERM stream closed unexpectedly");
                    std::process::exit(1);
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        if let Err(error) = tokio::signal::ctrl_c().await {
            eprintln!("issues: failed while waiting for shutdown signal: {error}");
            std::process::exit(1);
        }
    }
}
