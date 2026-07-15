//! # `notif` — the Notifications service binary (NOTIF-P1 → P-127)
//!
//! The "every service `main.rs`" the contract-index row 1.1 names: it composes the DURABLE
//! composition root (MR-009b W3b.4) and hands the Notif [`AppSpec`](myelin_notif::notif_app_spec)
//! to the harness's one call, [`run_notif`](myelin_notif::run_notif) (a thin wrapper over
//! `serve`). The harness owns the whole lifecycle (boot → migrate → outbox relay → consumers →
//! three ports → graceful drain, with liveness ≠ readiness); this `main` composes and hands off —
//! no hand-rolled lifecycle logic.
//!
//! **DURABLE-BY-DEFAULT (MR-009b W3b.4 / SI-007):** the outbox the relay drains is the PG-backed
//! `outbox` table (`OutboxStore::durable(PgOutboxBacking)`) over the MR-022 `SubstrateProvider`
//! pool, with the substrate foundation migrations (`outbox` + `consumer_dedup`) applied at boot —
//! committed events survive a process restart. **FAIL LOUD on missing durable config** (the W5
//! edge-main pattern): a missing `DATABASE_URL`, an unreachable pool, or a failed foundation
//! migration each exit non-zero — NEVER a silent in-memory fallback. The remaining endpoints keep
//! their dev-stack defaults (`Mode::DevDefaults`) until their own durable waves land; the durable
//! config THIS root depends on is the PG DSN, and that one is required explicitly.
//!
//! The runtime is the multi-thread `#[tokio::main]` flavor (required): the sync
//! `DurableOutboxBacking` verbs bridge to async sqlx via `block_in_place` + `block_on`, which
//! panics on a current-thread runtime.
//!
//! A failed boot / incomplete drain returns non-zero (§3.1) — loud, never a silent success.

use myelin_config::{Mode, MyelinConfig};
use myelin_events::OutboxStore;
use myelin_notif::run_notif;
use myelin_storage::{all_durable_migrations, HotTables, PgOutboxBacking, SubstrateProvider};
use myelin_substrate::Config;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    // FAIL LOUD on missing durable config (W3b.4): the durable outbox requires the PG DSN. No
    // DATABASE_URL → refuse to boot (exit non-zero) — never a silent in-memory fallback.
    if std::env::var("DATABASE_URL")
        .map(|v| v.trim().is_empty())
        .unwrap_or(true)
    {
        eprintln!(
            "notif: DATABASE_URL is required (durable-by-default outbox, MR-009b W3b.4): \
             refusing to boot without durable config — there is no in-memory fallback"
        );
        std::process::exit(1);
    }
    let config = MyelinConfig::from_env(Mode::DevDefaults).unwrap_or_else(|e| {
        eprintln!("notif: invalid config: {e}");
        std::process::exit(1);
    });
    let provider = match SubstrateProvider::connect(config, 8).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!(
                "notif: cannot reach the durable OLTP pool (durable-by-default requires PG): {e}"
            );
            std::process::exit(1);
        }
    };
    // The substrate foundation tables (the frozen `outbox` + `consumer_dedup` DDL) must exist
    // before the durable store binds — applied through the MR-022 migrator (idempotent,
    // forward-only, advisory-locked). Only the foundation set is applied here: the tables THIS
    // root's durable path needs, never a silently-widened migration surface.
    if let Err(e) = provider.migrate_foundation().await {
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
    if let Err(e) = provider
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
    {
        eprintln!("notif: cannot apply the durable migration aggregate (identity/pseudonym/placement/kms/cost/erasure): {e}");
        std::process::exit(1);
    }
    // The DURABLE outbox (SI-007): committed events live in Postgres, not a per-process mutex.
    let outbox = OutboxStore::durable(Arc::new(PgOutboxBacking::new(
        provider.db_pool().clone(),
        tokio::runtime::Handle::current(),
    )));
    // The env-first `Config::from_env()` parse for the substrate AppSpec config is P-S15; the
    // shell boots over the validated default today (the durable config is the provider's above).
    match run_notif(Config::default(), outbox) {
        Ok(()) => {}
        Err(e) => {
            // A failed boot / incomplete drain returns non-zero (§3.1) — loud, never swallowed.
            eprintln!("notif service failed: {e}");
            std::process::exit(1);
        }
    }
}
