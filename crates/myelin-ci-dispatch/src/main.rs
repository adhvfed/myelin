//! # `ci-dispatch` — the CI Trigger & Dispatch service binary (CI-P6 → P-349, M4)
//!
//! The "every service `main.rs`" the contract-index row 1.1 names: it composes the DURABLE
//! composition root (MR-009b W3b.6, the W3b.4 pattern) and hands the CI Trigger & Dispatch
//! [`AppSpec`](myelin_ci_dispatch::dispatch_app_spec) to the harness's one call,
//! [`run_dispatch`](myelin_ci_dispatch::run_dispatch) (a thin wrapper over `serve`). The harness
//! owns the whole lifecycle (boot → migrate → outbox relay → consumers → three ports → graceful
//! drain, with liveness ≠ readiness); this `main` composes and hands off.
//!
//! On boot the Trigger & Dispatch shell runs the forward-only migration that creates the
//! `consumer_dedup` ledger (the exactly-once-effect anchor) and auto-registers its OLTP store as a
//! `PersonalDataHolder`. A failed boot / incomplete drain returns non-zero (§3.1) — loud.
//!
//! **DURABLE-BY-DEFAULT (MR-009b W3b.6 / SI-007):** the outbox the relay drains is the PG-backed
//! `outbox` table (`OutboxStore::durable(PgOutboxBacking)`) over the MR-022 `SubstrateProvider`
//! pool, with the substrate foundation migrations (`outbox` + `consumer_dedup`) applied at boot —
//! committed events survive a process restart. **FAIL LOUD on missing durable config** (the W3b.4
//! service-main pattern): a missing `DATABASE_URL`, an unreachable pool, or a failed foundation
//! migration each exit non-zero — NEVER a silent in-memory fallback (the in-memory
//! `OutboxStore::new()` is `test-support`-gated and does not even compile here).
//!
//! The runtime is the multi-thread `#[tokio::main]` flavor (required): the sync
//! `DurableOutboxBacking` verbs bridge to async sqlx via `block_in_place` + `block_on`, which
//! panics on a current-thread runtime.
//!
//! **Floor:** the env-first `Config::from_env()` parse for the substrate AppSpec config lands with
//! the driver (P-S15); the shell boots over the validated default (the durable config THIS root
//! depends on is the PG DSN, required explicitly above). The dispatch behaviour (the
//! `EventMatcher`, the trust-tier stamp, the definition resolution → CAS snapshot, the
//! reserve/start handoff) is CI-P10/CI-P11 — this shell matches no event and starts no workflow
//! yet.

use myelin_ci_dispatch::run_dispatch;
use myelin_config::{Mode, MyelinConfig};
use myelin_events::OutboxStore;
use myelin_storage::{all_durable_migrations, HotTables, PgOutboxBacking, SubstrateProvider};
use myelin_substrate::Config;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    // FAIL LOUD on missing durable config (the W3b.4 pattern): the durable outbox requires the PG
    // DSN. No DATABASE_URL → refuse to boot (exit non-zero) — never a silent in-memory fallback.
    if std::env::var("DATABASE_URL")
        .map(|v| v.trim().is_empty())
        .unwrap_or(true)
    {
        eprintln!(
            "ci-dispatch: DATABASE_URL is required (durable-by-default outbox, MR-009b W3b.6): \
             refusing to boot without durable config — there is no in-memory fallback"
        );
        std::process::exit(1);
    }
    let config = MyelinConfig::from_env(Mode::DevDefaults).unwrap_or_else(|e| {
        eprintln!("ci-dispatch: invalid config: {e}");
        std::process::exit(1);
    });
    let provider = match SubstrateProvider::connect(config, 8).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!(
                "ci-dispatch: cannot reach the durable OLTP pool (durable-by-default requires \
                 PG): {e}"
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
            "ci-dispatch: cannot apply the substrate foundation migrations \
             (outbox/consumer_dedup): {e}"
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
        eprintln!("ci-dispatch: cannot apply the durable migration aggregate (identity/pseudonym/placement/kms/cost/erasure): {e}");
        std::process::exit(1);
    }
    // The DURABLE outbox (SI-007): committed events live in Postgres, not a per-process mutex.
    let outbox = OutboxStore::durable(Arc::new(PgOutboxBacking::new(
        provider.db_pool().clone(),
        tokio::runtime::Handle::current(),
    )));
    // The env-first `Config::from_env()` parse for the substrate AppSpec config is P-S15; the
    // shell boots over the validated default today (the durable config is the provider's above).
    match run_dispatch(Config::default(), outbox) {
        Ok(()) => {}
        Err(e) => {
            // A failed boot / incomplete drain returns non-zero (§3.1) — loud, never swallowed.
            eprintln!("ci-dispatch service failed: {e}");
            std::process::exit(1);
        }
    }
}
