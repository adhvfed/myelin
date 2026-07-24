//! # `ci-controlplane` — the CI Control Plane service binary (CI-P6 → P-349, M4)
//!
//! The "every service `main.rs`" the contract-index row 1.1 names: it composes the DURABLE
//! composition root (MR-009b W3b.6, the W3b.4 pattern) and hands the CI Control Plane
//! [`AppSpec`](myelin_ci_controlplane::controlplane_app_spec) to the harness's one call,
//! [`run_controlplane_until_shutdown`](myelin_ci_controlplane::run_controlplane_until_shutdown) (a
//! thin wrapper over `serve_until_shutdown`).
//! The harness owns the whole lifecycle (boot → migrate → outbox relay → consumers → three ports
//! → signal-driven graceful drain, with liveness ≠ readiness); this `main` composes durable workers
//! around that harness-owned lifecycle and joins them during shutdown.
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
//! service-main pattern): missing/distinct `DATABASE_URL`, `DATABASE_MIGRATION_URL`, and
//! `MYELIN_CI_SCHEDULER_DATABASE_URL`, an unreachable pool, or a failed migration each exit
//! non-zero — NEVER a silent in-memory fallback
//! (the in-memory
//! `OutboxStore::new()` is `test-support`-gated and does not even compile here).
//!
//! The runtime is the multi-thread `#[tokio::main]` flavor (required): the sync
//! `DurableOutboxBacking` verbs bridge to async sqlx via `block_in_place` + `block_on`, which
//! panics on a current-thread runtime.
//!
//! The substrate AppSpec config still uses its validated default, while every production endpoint
//! and all three PostgreSQL roles are explicit through `Mode::RequireEnv`. The per-table behaviour
//! (the scheduler claim, check emitter, log index, and metering) is the CI-P12..CI-P24 surface.
//! Runner execution is an exact opt-in (`MYELIN_CI_RUNNER=1`) and preflights the real gVisor
//! executor before opening intake; unset / `0` keeps every runner-host lane dormant.

use myelin_ci_controlplane::run_controlplane_until_shutdown;
use myelin_config::{Mode, MyelinConfig};
use myelin_events::OutboxStore;
use myelin_storage::{
    all_durable_migrations, HotTables, PgBootstrap, PgOutboxBacking, DEFAULT_MAX_CONNECTIONS,
};
use myelin_substrate::Config;
use std::ffi::OsString;
use std::fmt;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
enum StartupRefusal {
    InvalidRunnerSetting(String),
    NonUnicodeRunnerSetting(OsString),
    RunnerHostPreflight(String),
    /// **The rolling-upgrade floor (CT-004d.2 claim-bound completion).** The runner lane refuses to
    /// activate while any non-terminal job's dispatched stage is NULL (a pre-rewire historical dispatch
    /// the reporter must refuse without consuming). A checked invariant, not an assumption — a
    /// healthy deploy (CI has never been production-activated) has zero such rows.
    NonTerminalNullStageBacklog {
        count: i64,
    },
}

impl fmt::Display for StartupRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRunnerSetting(value) => write!(
                f,
                "invalid MYELIN_CI_RUNNER value {value:?}; allowed values are `0`, `1`, or unset"
            ),
            Self::NonUnicodeRunnerSetting(value) => write!(
                f,
                "invalid MYELIN_CI_RUNNER value {value:?} contains non-UTF-8 bytes; allowed values \
                 are `0`, `1`, or unset"
            ),
            Self::RunnerHostPreflight(error) => {
                write!(f, "runner-host executor preflight refused: {error}")
            }
            Self::NonTerminalNullStageBacklog { count } => write!(
                f,
                "runner-lane activation refused: {count} non-terminal job(s) have a NULL dispatched \
                 stage (a pre-rewire rolling-upgrade backlog completion cannot safely attribute); the \
                 activation guard requires zero such rows"
            ),
        }
    }
}

impl std::error::Error for StartupRefusal {}

fn verify_startup_activation(
    runner_setting: Result<String, std::env::VarError>,
) -> Result<(), StartupRefusal> {
    match runner_setting {
        Err(std::env::VarError::NotPresent) => Ok(()),
        Ok(value) if value == "0" => Ok(()),
        Ok(value) if value == "1" => Ok(()),
        Ok(value) => Err(StartupRefusal::InvalidRunnerSetting(value)),
        Err(std::env::VarError::NotUnicode(value)) => {
            Err(StartupRefusal::NonUnicodeRunnerSetting(value))
        }
    }
}

fn preflight_runner_host() -> Result<(), StartupRefusal> {
    let required_path = |name: &'static str| match std::env::var(name) {
        Ok(value) if !value.is_empty() => Ok(std::path::PathBuf::from(value)),
        Ok(_) | Err(std::env::VarError::NotPresent) => Err(StartupRefusal::RunnerHostPreflight(
            format!("{name} is required when MYELIN_CI_RUNNER=1"),
        )),
        Err(std::env::VarError::NotUnicode(_)) => Err(StartupRefusal::RunnerHostPreflight(
            format!("{name} must be valid Unicode"),
        )),
    };
    let runsc = required_path(myelin_ci_sandbox::gvisor::ENV_RUNSC_BIN)?;
    let rootfs = required_path(myelin_ci_sandbox::gvisor::ENV_GVISOR_ROOTFS)?;
    myelin_ci_sandbox::gvisor::preflight_gvisor_runner_host(&runsc, &rootfs)
        .map_err(StartupRefusal::RunnerHostPreflight)
}

#[tokio::main]
async fn main() {
    myelin_events::install_payload_free_panic_hook("ci-controlplane");
    // Runner execution is explicit opt-in. Unset / `0` keeps the runner host dormant; `1` composes
    // the proven production claim, Identity, reservation, fenced launch, recovery, and reporter root.
    // Every other value is refused before PostgreSQL bootstrap.
    let runner_setting = std::env::var("MYELIN_CI_RUNNER");
    // Read by borrow before validation consumes the setting so the complete host stays behind the
    // same single activation decision.
    let runner_host_requested = matches!(&runner_setting, Ok(value) if value == "1");
    if let Err(e) = verify_startup_activation(runner_setting) {
        eprintln!("ci-controlplane: startup refused: {e}");
        std::process::exit(1);
    }
    // Install OS signal handlers before database bootstrap or runner-host construction. A deploy
    // signal that lands after intake starts must always enter the coordinated drain path.
    let shutdown_signals = match ShutdownSignals::install() {
        Ok(signals) => signals,
        Err(error) => {
            eprintln!("ci-controlplane: failed to install shutdown signal handlers: {error}");
            std::process::exit(1);
        }
    };
    // Consume signals immediately, not only after bootstrap. Every intake owner subscribes to this
    // same latch, so a signal received during preflight or migrations is already true at the first
    // starter/workflow/runner instruction.
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    let signal_shutdown = shutdown_tx.clone();
    let signal_task = tokio::spawn(async move {
        shutdown_signals.wait().await;
        let _ = signal_shutdown.send(true);
    });
    eprintln!("ci-controlplane: shutdown handlers armed; startup termination is intake-gated");
    if runner_host_requested {
        if let Err(error) = preflight_runner_host() {
            eprintln!("ci-controlplane: startup refused: {error}");
            std::process::exit(1);
        }
    }

    // Production is strict: validate every endpoint plus three distinct PostgreSQL credentials
    // before any DDL, durable store, reaper, or listener can be created. The scheduler credential
    // is parsed here but is not connected until privileged migrations are complete.
    let platform_config = match MyelinConfig::from_env(Mode::RequireEnv) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("ci-controlplane: platform configuration refused to start: {e}");
            std::process::exit(1);
        }
    };
    let scheduler_config =
        match myelin_ci_controlplane::CiSchedulerDbConfig::from_env(&platform_config) {
            Ok(config) => config,
            Err(e) => {
                eprintln!("ci-controlplane: scheduler configuration refused to start: {e}");
                std::process::exit(1);
            }
        };
    // `PgBootstrap` alone owns the privileged pool.
    let bootstrap = match PgBootstrap::connect(platform_config, DEFAULT_MAX_CONNECTIONS).await {
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
    // The CI runner owns durable `ci.pipeline` executions in Flow. Apply Flow's exact shared
    // migration set before the CI follow-ons that add the active-run recovery index and constrained
    // scheduler discovery policy. This is idempotent with a separately booted Flow service and
    // removes any boot-order dependency on that service.
    if let Err(e) = bootstrap
        .migrate(
            &myelin_flow::migrations::migrations(),
            &HotTables::declare(["workflow_run"]),
        )
        .await
    {
        eprintln!("ci-controlplane: cannot apply the durable Flow prerequisite migrations: {e}");
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
    // The scheduler credential is opened only after privileged migrations and is validated against
    // the authenticated runtime role and the server-owned region map. Its provider exposes no raw
    // pool and can construct only the region claim/reap capability.
    let scheduler_provider = match myelin_ci_controlplane::CiSchedulerDbProvider::connect(
        scheduler_config,
        provider.db_pool(),
    )
    .await
    {
        Ok(provider) => provider,
        Err(e) => {
            eprintln!("ci-controlplane: scheduler database handoff refused to start: {e}");
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
    let _ci_cost_events = myelin_ci_controlplane::ci_cost_event_store(
        provider.db_pool().clone(),
        myelin_tenancy::Region(provider.config().region.clone()),
    );
    let _ci_job_accounting = myelin_ci_controlplane::ci_job_accounting_store(
        provider.db_pool().clone(),
        myelin_tenancy::Region(provider.config().region.clone()),
    );
    // CT-004c.1: construct the REAL durable `job_queue` store + spawn the dead-runner reaper loop onto
    // the serve runtime (minimal-impact wiring — a bounded background task coordinated with the
    // signal-driven lifecycle, NOT a new AppSpec schema field). The `job_queue` table it sweeps is created
    // by the full `ci_controlplane_migrations` the `serve(AppSpec)` below applies at boot; the reaper
    // delays its first sweep one interval so that boot-migrate has completed. It re-queues only work
    // whose Flow/CI owner remains active; an expired launched job under a cancelled owner is instead
    // terminalized through durable operational accounting. It launches NO untrusted code. The claim
    // path is dormant unless the explicit runner-host activation below is requested.
    let region_queue_store = scheduler_provider.region_queue_store();
    let operational_ledger = myelin_storage::DurableCostLedger::new(provider.clone());
    let reaper = myelin_ci_controlplane::JobQueueReaper::new(
        region_queue_store.clone(),
        provider.config().region.clone(),
        std::time::Duration::from_secs(15),
    )
    .with_cancelled_accounting(provider.db_pool().clone(), operational_ledger);
    let reaper_task = tokio::spawn(reaper.run_until_shutdown(shutdown_tx.subscribe()));
    // CT-004 — compose the per-tenant `ci_run`-poll STARTER lane behind the SAME `MYELIN_CI_RUNNER`
    // activation seam the runner uses. `ci_run_starter_factory` builds the REAL, production-callable
    // `PgCiRunStarterFactory` from the runtime pool + cell region + blob CAS: it mints an exact-cell
    // `PgCiPipelineStarter` for a queued run's AUTHORITATIVE tenant (read from `ci_run.tenant_id`), so a
    // region-wide poller can never stamp one tenant's authority onto another's run. Constructing it wraps
    // the pool + blob client only — NO query runs at boot, and NO fixed service identity is ever bound.
    //
    // Explicit opt-in only: `runner_host_requested` is true exactly for `MYELIN_CI_RUNNER=1`. The
    // starter factory below carries the real fixed linux-small-v1 policy plus the PostgreSQL Tier-P
    // operational reservation source. Its outstanding-reservation bound admits one largest valid
    // run when the tenant has no prior live reservations; the scheduler separately limits
    // leased/running work. Initial checks and the manifest-native DAG body are wired through this
    // same coordinated host.
    // The retained host below now owns coordinated start, failure propagation, and bounded shutdown
    // for the starter, workflow recovery, and sandbox runner.
    let runner_host = if runner_host_requested {
        // ROLLING-UPGRADE FLOOR (CT-004d.2): refuse activation while any non-terminal NULL-stage
        // dispatch is still live — completion refuses such a job without consuming its claim, so the
        // lane must not start until the backlog is repaired. A CHECKED invariant (a healthy never-activated
        // deploy has zero).
        match region_queue_store
            .count_non_terminal_null_stage_jobs(&provider.config().region)
            .await
        {
            Ok(0) => {}
            Ok(count) => {
                eprintln!(
                    "ci-controlplane: startup refused: {}",
                    StartupRefusal::NonTerminalNullStageBacklog { count }
                );
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("ci-controlplane: null-stage activation guard query failed: {e}");
                std::process::exit(1);
            }
        }
        let starter_factory = match myelin_ci_controlplane::ci_run_starter_factory(
            provider.db_pool().clone(),
            myelin_tenancy::Region(provider.config().region.clone()),
            Arc::new(myelin_storage::s3blob::S3BlobStore::connect(
                &provider.config().s3,
                tokio::runtime::Handle::current(),
            )),
            tokio::runtime::Handle::current(),
            myelin_storage::DurableCostLedger::with_runtime(
                provider.clone(),
                tokio::runtime::Handle::current(),
            ),
        ) {
            Ok(factory) => factory,
            Err(error) => {
                eprintln!(
                    "ci-controlplane: Tier-P operational reservation composition refused: {error}"
                );
                std::process::exit(1);
            }
        };
        let runner_runtime = match myelin_ci_controlplane::ci_production_runtime_factory(
            provider.clone(),
            tokio::runtime::Handle::current(),
        ) {
            Ok(runtime) => runtime,
            Err(error) => {
                eprintln!("ci-controlplane: exact-tenant runtime composition refused: {error}");
                std::process::exit(1);
            }
        };
        let starter_poller = myelin_ci_controlplane::PgCiRunStarterPoller::new(
            scheduler_provider.region_run_discovery(),
            starter_factory,
            runner_runtime.definition().clone(),
        );
        let workflow_poller = match runner_runtime
            .workflow_poller(scheduler_provider.region_run_discovery(), "ci-flow")
        {
            Ok(poller) => poller,
            Err(error) => {
                eprintln!("ci-controlplane: workflow fan-out composition refused: {error}");
                std::process::exit(1);
            }
        };
        let runner_reporter = match runner_runtime.reporter_router() {
            Ok(reporter) => reporter,
            Err(error) => {
                eprintln!("ci-controlplane: terminal reporter composition refused: {error}");
                std::process::exit(1);
            }
        };
        let runner_cell_id = match std::env::var("MYELIN_CELL_ID") {
            Ok(cell_id) => cell_id,
            Err(_) => {
                eprintln!(
                    "ci-controlplane: runner Identity composition refused: MYELIN_CELL_ID is \
                     required"
                );
                std::process::exit(1);
            }
        };
        let runner_seal_key = match myelin_storage::seal_key_from_env() {
            Ok(seal_key) => seal_key,
            Err(error) => {
                eprintln!(
                    "ci-controlplane: runner Identity composition refused: durable seal key is \
                     unavailable: {error}"
                );
                std::process::exit(1);
            }
        };
        let runner_identity = match myelin_ci_controlplane::ci_runner_identity_authorities(
            provider.clone(),
            runner_cell_id,
            &runner_seal_key,
            tokio::runtime::Handle::current(),
        )
        .await
        {
            Ok(identity) => identity,
            Err(error) => {
                eprintln!("ci-controlplane: runner Identity composition refused: {error}");
                std::process::exit(1);
            }
        };
        let runner_resolver = myelin_ci_controlplane::durable_spec_resolver(
            myelin_ci_controlplane::ci_job_spec_store(provider.db_pool().clone()),
            provider.config().region.clone(),
            tokio::runtime::Handle::current(),
            runner_identity.token_issuer().clone(),
        );
        let runner_hooks = myelin_ci_controlplane::ci_runner_hooks(
            provider.clone(),
            runner_identity.launch_authorizer(),
            tokio::runtime::Handle::current(),
        );
        let runner = myelin_ci_controlplane::CiRunnerLoop::new(
            format!("ci-runner-{}", std::process::id()),
            myelin_ci_controlplane::LINUX_SMALL_V1_RUNNER_LABELS
                .iter()
                .map(|label| (*label).to_owned())
                .collect(),
            vec![myelin_ci_sandbox::TrustTier::Trusted],
            provider.config().region.clone(),
            myelin_ci_controlplane::CI_RUNNER_LEASE_TTL_SECS,
            region_queue_store.clone(),
            myelin_ci_controlplane::ci_job_queue_store(provider.db_pool().clone()),
            tokio::runtime::Handle::current(),
            runner_resolver,
            runner_reporter,
            runner_hooks,
            provider.db_pool().clone(),
            provider.config().s3.clone(),
        );
        match myelin_ci_controlplane::CiRunnerHost::new(starter_poller, workflow_poller, runner)
            .start_with_shutdown(
                myelin_ci_controlplane::CiRunnerHostConfig::production(),
                shutdown_tx.clone(),
            ) {
            Ok(host) => Some(host),
            Err(error) => {
                eprintln!("ci-controlplane: runner-host lifecycle refused: {error}");
                std::process::exit(1);
            }
        }
    } else {
        None
    };
    // The env-first `Config::from_env()` parse for the substrate AppSpec config is P-S15; the
    // shell boots over the validated default today (the durable config is the provider's above).
    let runner_host_failures = runner_host
        .as_ref()
        .map(myelin_ci_controlplane::CiRunnerHostHandle::failure_receiver);
    // This second observer deliberately outlives the service-shutdown future. The supervisor keeps
    // owning every join after the deadline; if a lane never returns, only the process boundary can
    // end it without detaching work inside a still-serving process.
    let runner_host_deadline_task = runner_host.as_ref().map(|host| {
        let failures = host.failure_receiver();
        tokio::spawn(async move {
            if myelin_ci_controlplane::wait_for_ci_runner_host_drain_timeout(failures).await {
                eprintln!("ci-controlplane: runner host exceeded its process-fatal drain deadline");
                std::process::exit(1);
            }
        })
    });
    let service_shutdown = shutdown_tx.clone();
    let service_result = run_controlplane_until_shutdown(Config::default(), outbox, async move {
        let shutdown = async move {
            if *shutdown_rx.borrow_and_update() {
                return;
            }
            let _ = shutdown_rx.changed().await;
        };
        tokio::pin!(shutdown);
        if let Some(failures) = runner_host_failures {
            tokio::select! {
                _ = shutdown.as_mut() => {}
                failure = myelin_ci_controlplane::wait_for_ci_runner_host_failure(failures) => {
                    eprintln!("ci-controlplane: runner host failed: {failure}");
                }
            }
        } else {
            shutdown.as_mut().await;
        }
        let _ = service_shutdown.send(true);
    })
    .await;
    let _ = shutdown_tx.send(true);
    signal_task.abort();
    let _ = signal_task.await;
    let reaper_result = tokio::time::timeout(std::time::Duration::from_secs(10), reaper_task).await;
    let runner_host_result = match runner_host {
        Some(host) => host.shutdown().await,
        None => Ok(()),
    };
    let runner_host_deadline_result = match runner_host_deadline_task {
        Some(task) => task.await,
        None => Ok(()),
    };
    if !matches!(reaper_result, Ok(Ok(()))) {
        eprintln!("ci-controlplane: reaper did not stop cleanly during shutdown");
        std::process::exit(1);
    }
    if let Err(error) = runner_host_result {
        eprintln!("ci-controlplane: runner host did not stop cleanly: {error}");
        std::process::exit(1);
    }
    if runner_host_deadline_result.is_err() {
        eprintln!("ci-controlplane: runner-host deadline watchdog failed");
        std::process::exit(1);
    }
    match service_result {
        Ok(()) => {}
        Err(e) => {
            // A failed boot / incomplete drain returns non-zero (§3.1) — loud, never swallowed.
            eprintln!("ci-controlplane service failed: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(unix)]
struct ShutdownSignals {
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl ShutdownSignals {
    fn install() -> std::io::Result<Self> {
        Ok(Self {
            interrupt: tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?,
            terminate: tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?,
        })
    }

    async fn wait(mut self) {
        tokio::select! {
            signal = self.interrupt.recv() => {
                if signal.is_none() {
                    eprintln!("ci-controlplane: SIGINT stream closed unexpectedly");
                    std::process::exit(1);
                }
            }
            signal = self.terminate.recv() => {
                if signal.is_none() {
                    eprintln!("ci-controlplane: SIGTERM stream closed unexpectedly");
                    std::process::exit(1);
                }
            }
        }
    }
}

#[cfg(not(unix))]
struct ShutdownSignals;

#[cfg(not(unix))]
impl ShutdownSignals {
    fn install() -> std::io::Result<Self> {
        Ok(Self)
    }

    async fn wait(self) {
        if let Err(error) = tokio::signal::ctrl_c().await {
            eprintln!("ci-controlplane: failed while waiting for shutdown signal: {error}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{verify_startup_activation, StartupRefusal};
    use std::env::VarError;

    #[test]
    fn production_runner_flag_is_explicit_opt_in_and_malformed_values_fail_closed() {
        assert_eq!(verify_startup_activation(Ok("1".to_owned())), Ok(()));
        assert_eq!(verify_startup_activation(Err(VarError::NotPresent)), Ok(()));
        assert_eq!(verify_startup_activation(Ok("0".to_owned())), Ok(()));
        assert_eq!(
            verify_startup_activation(Ok("true".to_owned())),
            Err(StartupRefusal::InvalidRunnerSetting("true".to_owned()))
        );
    }

    /// The `MYELIN_CI_RUNNER=1` request gates the complete host on the same validated setting.
    /// Unset / `0` leave it dormant, while invalid values neither request activation nor boot.
    #[test]
    fn runner_host_request_activates_only_for_the_exact_valid_setting() {
        let requested = |setting: Result<String, VarError>| {
            let host_requested = matches!(&setting, Ok(value) if value == "1");
            (host_requested, verify_startup_activation(setting))
        };
        assert_eq!(
            requested(Ok("1".to_owned())),
            (true, Ok(())),
            "MYELIN_CI_RUNNER=1 explicitly requests the complete runner host"
        );
        assert_eq!(requested(Err(VarError::NotPresent)), (false, Ok(())));
        assert_eq!(requested(Ok("0".to_owned())), (false, Ok(())));
        assert_eq!(
            requested(Ok("true".to_owned())),
            (
                false,
                Err(StartupRefusal::InvalidRunnerSetting("true".to_owned()))
            ),
            "an invalid setting neither requests the runner host nor boots"
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_runner_flag_has_a_typed_fail_closed_verdict() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let invalid = OsString::from_vec(vec![b'1', 0xff]);
        let refusal = verify_startup_activation(Err(VarError::NotUnicode(invalid.clone())))
            .expect_err("non-UTF-8 runner setting must be refused");

        assert_eq!(refusal, StartupRefusal::NonUnicodeRunnerSetting(invalid));
        assert!(refusal.to_string().contains("contains non-UTF-8 bytes"));
        assert!(refusal
            .to_string()
            .contains("allowed values are `0`, `1`, or unset"));
    }
}
