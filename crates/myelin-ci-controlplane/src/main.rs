//! # `ci-controlplane` — the CI Control Plane service binary (CI-P6 → P-349, M4)
//!
//! The "every service `main.rs`" the contract-index row 1.1 names: it composes the DURABLE
//! composition root (MR-009b W3b.6, the W3b.4 pattern) and hands the CI Control Plane
//! [`AppSpec`](myelin_ci_controlplane::controlplane_app_spec) to the harness's one call,
//! [`run_controlplane`](myelin_ci_controlplane::run_controlplane) (a thin wrapper over `serve`).
//! The harness owns the whole lifecycle (boot → migrate → outbox relay → consumers → three ports
//! → graceful drain, with liveness ≠ readiness); this `main` composes and hands off — no
//! hand-rolled lifecycle logic.
//!
//! On boot the CI Control Plane shell runs the complete forward-only data-model migrations (every CI
//! Control-Plane table — `ci_run` … `cost_event` — `(tenant, region)`-first + RLS-on) and
//! auto-registers its OLTP store as a `PersonalDataHolder`. A failed boot / incomplete drain returns
//! non-zero (§3.1) — loud, never a silent success.
//!
//! **DURABLE-BY-DEFAULT (MR-009b W3b.6 / SI-007):** the outbox the relay drains is the PG-backed
//! `outbox` table (`OutboxStore::durable(PgOutboxBacking)`) over the MR-022 `SubstrateProvider`
//! runtime pool, after a privileged migration pool applies the complete schema and is destroyed —
//! committed events survive a process restart. **FAIL LOUD on missing durable config** (the W3b.4
//! service-main pattern): missing/distinct `DATABASE_URL` and `DATABASE_MIGRATION_URL`, an
//! unreachable pool, or a failed migration each exit non-zero — NEVER a silent in-memory fallback
//! (the in-memory
//! `OutboxStore::new()` is `test-support`-gated and does not even compile here).
//!
//! The runtime is the multi-thread `#[tokio::main]` flavor (required): the sync
//! `DurableOutboxBacking` verbs bridge to async sqlx via `block_in_place` + `block_on`, which
//! panics on a current-thread runtime.
//!
//! **Floor:** the substrate AppSpec config still uses its validated default, while every production
//! endpoint and both PostgreSQL roles are explicit through `Mode::RequireEnv`. The per-table
//! behaviour (the scheduler claim, the check emitter, the log index, the metering) is the
//! CI-P12..CI-P24 surface. This shell runs no job: requesting the incomplete production runner
//! fails before database bootstrap until its durable billing and run-token authorities are wired.

use myelin_ci_controlplane::run_controlplane;
use myelin_config::Mode;
use myelin_events::OutboxStore;
use myelin_storage::{all_durable_migrations, HotTables, PgBootstrap, PgOutboxBacking};
use myelin_substrate::Config;
use std::fmt;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartupRefusal {
    IncompleteProductionRunner,
}

impl fmt::Display for StartupRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IncompleteProductionRunner => write!(
                f,
                "MYELIN_CI_RUNNER=1 requires a real durable CostLedger reserve/settle authority \
                 and live per-run-token verification; production runner activation is refused"
            ),
        }
    }
}

impl std::error::Error for StartupRefusal {}

fn verify_startup_activation(runner_setting: Option<&str>) -> Result<(), StartupRefusal> {
    if runner_setting == Some("1") {
        Err(StartupRefusal::IncompleteProductionRunner)
    } else {
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    // The former runner composition reached sandbox execution with accept-all billing/attribution
    // hooks, placeholder tenancy, and an unresolved stage-spec builder. Keep the flag reserved,
    // but refuse it before PostgreSQL bootstrap until all launch authorities are real and durable.
    let runner_setting = std::env::var("MYELIN_CI_RUNNER").ok();
    if let Err(e) = verify_startup_activation(runner_setting.as_deref()) {
        eprintln!("ci-controlplane: startup refused: {e}");
        std::process::exit(1);
    }

    // Production is strict: validate every endpoint plus distinct migration/runtime PostgreSQL
    // roles before any DDL, durable store, reaper, or listener can be created. `PgBootstrap`
    // alone owns the privileged pool.
    let bootstrap = match PgBootstrap::from_env(Mode::RequireEnv).await {
        Ok(bootstrap) => bootstrap,
        Err(e) => {
            eprintln!("ci-controlplane: database bootstrap refused to start: {e}");
            std::process::exit(1);
        }
    };
    // The substrate foundation tables (the frozen `outbox` + `consumer_dedup` DDL) must exist
    // before the durable store binds — applied through the MR-022 migrator (idempotent,
    // forward-only, advisory-locked). Only the foundation set is applied here: the tables THIS
    // root's durable path needs, never a silently-widened migration surface.
    if let Err(e) = bootstrap.migrate_foundation().await {
        eprintln!(
            "ci-controlplane: cannot apply the substrate foundation migrations \
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
    if let Err(e) = bootstrap
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
    {
        eprintln!("ci-controlplane: cannot apply the durable migration aggregate (identity/pseudonym/placement/kms/cost/erasure): {e}");
        std::process::exit(1);
    }
    // Apply the COMPLETE Controlplane-owned schema through the privileged pool. The AppSpec still
    // declares this exact set for lifecycle/model checks, but production cannot defer real SQL until
    // after cost stores or the reaper have already been constructed.
    if let Err(e) = bootstrap
        .migrate(
            &myelin_ci_controlplane::ci_controlplane_migrations(),
            &myelin_ci_controlplane::ci_controlplane_hot_tables(),
        )
        .await
    {
        eprintln!("ci-controlplane: cannot apply the complete Controlplane migrations: {e}");
        std::process::exit(1);
    }
    // Re-probe the constrained runtime role, close the privileged pool, and erase its DSN before
    // any runtime query/store/reaper/listener is created.
    let provider = match bootstrap.into_runtime().await {
        Ok(provider) => provider,
        Err(e) => {
            eprintln!("ci-controlplane: database runtime handoff refused to start: {e}");
            std::process::exit(1);
        }
    };
    // #11 — BOOT-TIME SHAPE ASSERTION on the money table. `CREATE TABLE IF NOT EXISTS` above no-ops on
    // a pre-existing (possibly pre-CT-004m mis-shaped) `ci_cost_event`, so assert the columns/types are
    // the CI metering-projection shape before the settle path can write money data. FAIL LOUD.
    if let Err(e) = myelin_ci_controlplane::verify_ci_cost_event_shape(provider.db_pool()).await {
        eprintln!("ci-controlplane: ci_cost_event shape assertion failed: {e}");
        std::process::exit(1);
    }
    // The DURABLE outbox (SI-007): committed events live in Postgres, not a per-process mutex.
    let outbox = OutboxStore::durable(Arc::new(PgOutboxBacking::new(
        provider.db_pool().clone(),
        tokio::runtime::Handle::current(),
    )));
    // CT-004a: construct the REAL durable CI `ci_cost_event` projection store from the provider pool —
    // proving the metering path is a production-callable store (not model-only). DORMANT at the shell
    // (no consumer drives it yet); CT-004d attaches it to the `SCHEDULE_AND_RUN_JOB` dispatch settle
    // bookend. CT-004m resolved the former `cost_event` table-name collision (CI's table is now
    // `ci_cost_event`, created by the shared `ci_durable_migrations` applied above). Building it only
    // wraps the pool — no query runs at boot.
    let _ci_cost_events = myelin_ci_controlplane::ci_cost_event_store(provider.db_pool().clone());
    // CT-004c.1: construct the REAL durable `job_queue` store + spawn the dead-runner reaper loop onto
    // the serve runtime (minimal-impact wiring — a bounded background task hung off the existing
    // lifecycle, NOT a new AppSpec schema field). The `job_queue` table the reaper sweeps is created
    // by the full `ci_controlplane_migrations` the `serve(AppSpec)` below applies at boot; the reaper
    // delays its first sweep one interval so that boot-migrate has completed. The reaper is SAFE — it
    // only re-queues expired leases (`leased`→`queued`); it launches NO untrusted code (binding the
    // runner + starting the pipeline body on the sandbox executor is CT-004c.2). The claim path is
    // dormant at the shell (production runner activation is refused above).
    let ci_job_queue = myelin_ci_controlplane::ci_job_queue_store(provider.db_pool().clone());
    let reaper = myelin_ci_controlplane::JobQueueReaper::new(
        ci_job_queue,
        provider.config().region.clone(),
        std::time::Duration::from_secs(15),
    );
    tokio::spawn(reaper.run());
    // The env-first `Config::from_env()` parse for the substrate AppSpec config is P-S15; the
    // shell boots over the validated default today (the durable config is the provider's above).
    match run_controlplane(Config::default(), outbox) {
        Ok(()) => {}
        Err(e) => {
            // A failed boot / incomplete drain returns non-zero (§3.1) — loud, never swallowed.
            eprintln!("ci-controlplane service failed: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{verify_startup_activation, StartupRefusal};

    #[test]
    fn production_runner_flag_has_a_typed_fail_closed_verdict() {
        assert_eq!(
            verify_startup_activation(Some("1")),
            Err(StartupRefusal::IncompleteProductionRunner)
        );
        assert_eq!(verify_startup_activation(None), Ok(()));
        assert_eq!(verify_startup_activation(Some("true")), Ok(()));
    }
}
