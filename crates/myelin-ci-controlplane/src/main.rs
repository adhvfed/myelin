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
    /// CT-007 slice 4: `MYELIN_CI_WORKSPACE_MODE` names neither `enabled`, `disabled`, nor is unset —
    /// a typo such as `enable` must refuse loudly rather than silently downgrade to `Disabled`.
    InvalidWorkspaceMode(String),
    NonUnicodeWorkspaceMode(OsString),
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
            Self::InvalidWorkspaceMode(value) => write!(
                f,
                "invalid MYELIN_CI_WORKSPACE_MODE value {value:?}; allowed values are `enabled`, \
                 `disabled`, or unset"
            ),
            Self::NonUnicodeWorkspaceMode(value) => write!(
                f,
                "invalid MYELIN_CI_WORKSPACE_MODE value {value:?} contains non-UTF-8 bytes; allowed \
                 values are `enabled`, `disabled`, or unset"
            ),
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

/// CT-007 slice 4: the `runsc --root=`/helper-dir pair a caller must feed
/// [`myelin_ci_sandbox::gvisor::preflight_explicit_userns_policy`] once `MYELIN_CI_WORKSPACE_MODE`
/// requests the `Enabled` activation level.
#[derive(Debug)]
struct ExplicitUsernsPolicyPaths {
    helper_dir: std::path::PathBuf,
    runsc_root: std::path::PathBuf,
}

/// The owned outcome of parsing the CT-007 slice 4 workspace-activation env contract: the exact
/// [`myelin_ci_sandbox::gvisor::GvisorWorkspaceConfig`] to construct `GvisorBackend` with, plus (only
/// for `Enabled`) the paths the explicit-userns preflight needs. Parsed ONCE in `main` and carried
/// across the entire startup sequence into [`myelin_ci_controlplane::CiRunnerLoop::new`] — never
/// re-read from the environment later, so a preflighted snapshot can never diverge from what the
/// runner thread actually constructs against.
#[derive(Debug)]
struct ParsedWorkspaceActivation {
    workspace_config: myelin_ci_sandbox::gvisor::GvisorWorkspaceConfig,
    explicit_policy: Option<ExplicitUsernsPolicyPaths>,
}

/// Sol's round-1 design review, tightened in round 2: every `Enabled`-only variable is received as
/// a caller-supplied closure, so `Disabled`/unset mode structurally never READS them -- not merely
/// "reads but ignores" (the original `Result`-argument shape evaluated `std::env::var` for all six
/// variables in the caller before this function was even entered, which the poisoned-`Result` test
/// could not distinguish from genuine non-observation). A test can now prove non-observation with a
/// panicking closure: if `Disabled` mode ever called it, the test would panic instead of merely
/// asserting on a value.
#[allow(clippy::too_many_arguments)]
fn parse_workspace_activation_given(
    mode: Result<String, std::env::VarError>,
    explicit_userns_runsc_root: impl FnOnce() -> Result<String, std::env::VarError>,
    userns_leases_dir: impl FnOnce() -> Result<String, std::env::VarError>,
    ci_workspaces_dir: impl FnOnce() -> Result<String, std::env::VarError>,
    capacity_bytes: impl FnOnce() -> Result<String, std::env::VarError>,
    explicit_userns_helper_dir: impl FnOnce() -> Result<String, std::env::VarError>,
) -> Result<ParsedWorkspaceActivation, StartupRefusal> {
    let enabled = match mode {
        Err(std::env::VarError::NotPresent) => false,
        Ok(value) if value == "disabled" => false,
        Ok(value) if value == "enabled" => true,
        Ok(value) => return Err(StartupRefusal::InvalidWorkspaceMode(value)),
        Err(std::env::VarError::NotUnicode(value)) => {
            return Err(StartupRefusal::NonUnicodeWorkspaceMode(value));
        }
    };
    if !enabled {
        return Ok(ParsedWorkspaceActivation {
            workspace_config: myelin_ci_sandbox::gvisor::GvisorWorkspaceConfig::Disabled,
            explicit_policy: None,
        });
    }
    let required_absolute_path = |name: &'static str, value: Result<String, std::env::VarError>| {
        match value {
            Ok(v) if !v.is_empty() => {
                let path = std::path::PathBuf::from(&v);
                if path.is_absolute() {
                    Ok(path)
                } else {
                    Err(StartupRefusal::RunnerHostPreflight(format!(
                        "{name} must be an absolute path, got {v:?}"
                    )))
                }
            }
            Ok(_) | Err(std::env::VarError::NotPresent) => {
                Err(StartupRefusal::RunnerHostPreflight(format!(
                    "{name} is required when MYELIN_CI_WORKSPACE_MODE=enabled"
                )))
            }
            Err(std::env::VarError::NotUnicode(_)) => Err(StartupRefusal::RunnerHostPreflight(
                format!("{name} must be valid Unicode"),
            )),
        }
    };
    let runsc_root = required_absolute_path(
        myelin_ci_sandbox::gvisor::ENV_EXPLICIT_USERNS_RUNSC_ROOT,
        explicit_userns_runsc_root(),
    )?;
    let leases_dir = required_absolute_path("MYELIN_USERNS_LEASES_DIR", userns_leases_dir())?;
    let base_dir = required_absolute_path("MYELIN_CI_WORKSPACES_DIR", ci_workspaces_dir())?;
    let host_capacity_bytes = match capacity_bytes() {
        Ok(v) => v.parse::<u64>().ok().filter(|n| *n > 0).ok_or_else(|| {
            StartupRefusal::RunnerHostPreflight(format!(
                "MYELIN_CI_WORKSPACE_CAPACITY_BYTES must be a positive integer byte count, got {v:?}"
            ))
        })?,
        Err(std::env::VarError::NotPresent) => {
            return Err(StartupRefusal::RunnerHostPreflight(
                "MYELIN_CI_WORKSPACE_CAPACITY_BYTES is required when \
                 MYELIN_CI_WORKSPACE_MODE=enabled -- there is no default; the aggregate disk \
                 admission ceiling is an operator/storage-layout decision"
                    .to_string(),
            ));
        }
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(StartupRefusal::RunnerHostPreflight(
                "MYELIN_CI_WORKSPACE_CAPACITY_BYTES must be valid Unicode".to_string(),
            ));
        }
    };
    // Only `NotPresent` defaults to `/usr/bin` -- an explicitly empty value is treated as likely
    // operator error (Sol's round-2 review), not silently coerced to the default.
    let helper_dir = match explicit_userns_helper_dir() {
        Err(std::env::VarError::NotPresent) => std::path::PathBuf::from("/usr/bin"),
        other => required_absolute_path(
            myelin_ci_sandbox::gvisor::ENV_EXPLICIT_USERNS_HELPER_DIR,
            other,
        )?,
    };
    Ok(ParsedWorkspaceActivation {
        workspace_config: myelin_ci_sandbox::gvisor::GvisorWorkspaceConfig::Enabled {
            base_dir,
            host_capacity_bytes,
            leases_dir,
            // Fixed at 1 (Sol's design review): `RunnerAgent::run_one` performs exactly one
            // synchronous launch at a time -- a launch fully binds and releases (or quarantines)
            // its lease before the next claim starts, so the actual simultaneous-lease requirement
            // is exactly one. A larger pool would be arbitrary slack, not real concurrency; if
            // runner concurrency is ever introduced, derive this from that setting in the same
            // change rather than picking a number here.
            min_pool_size: 1,
        },
        explicit_policy: Some(ExplicitUsernsPolicyPaths {
            helper_dir,
            runsc_root,
        }),
    })
}

fn parse_workspace_activation() -> Result<ParsedWorkspaceActivation, StartupRefusal> {
    parse_workspace_activation_given(
        std::env::var("MYELIN_CI_WORKSPACE_MODE"),
        || std::env::var(myelin_ci_sandbox::gvisor::ENV_EXPLICIT_USERNS_RUNSC_ROOT),
        || std::env::var("MYELIN_USERNS_LEASES_DIR"),
        || std::env::var("MYELIN_CI_WORKSPACES_DIR"),
        || std::env::var("MYELIN_CI_WORKSPACE_CAPACITY_BYTES"),
        || std::env::var(myelin_ci_sandbox::gvisor::ENV_EXPLICIT_USERNS_HELPER_DIR),
    )
}

/// The deterministic ordering seam behind [`prepare_runner_host`]: explicit-userns preflight (if
/// `Enabled`) always runs BEFORE the rootless preflight (Sol's design review — the explicit-userns
/// preflight pins/hardens the SAME `runsc` binary the rootless preflight goes on to execute, so it
/// must run first), and a failure in the explicit-userns step must prevent the rootless step from
/// ever running. Neither the real `parse_workspace_activation` nor either real preflight function is
/// called from here directly, so a test can prove the ordering/short-circuit with fake closures that
/// record call order, without touching the real environment or filesystem.
fn prepare_runner_host_given(
    parsed: Result<ParsedWorkspaceActivation, StartupRefusal>,
    run_explicit_userns_preflight: impl FnOnce(&ExplicitUsernsPolicyPaths) -> Result<(), String>,
    run_rootless_preflight: impl FnOnce() -> Result<(), StartupRefusal>,
) -> Result<myelin_ci_sandbox::gvisor::GvisorWorkspaceConfig, StartupRefusal> {
    let parsed = parsed?;
    if let Some(policy) = &parsed.explicit_policy {
        run_explicit_userns_preflight(policy).map_err(StartupRefusal::RunnerHostPreflight)?;
    }
    run_rootless_preflight()?;
    Ok(parsed.workspace_config)
}

/// CT-007 slice 4: parse the complete workspace-activation contract, preflight it (explicit-userns
/// first when `Enabled`, then the always-required rootless base), and return the OWNED
/// [`myelin_ci_sandbox::gvisor::GvisorWorkspaceConfig`] `main` carries into
/// [`myelin_ci_controlplane::CiRunnerLoop::new`] — never re-derived later from a second environment
/// read, so preflight can never validate one snapshot while construction uses another.
fn prepare_runner_host() -> Result<myelin_ci_sandbox::gvisor::GvisorWorkspaceConfig, StartupRefusal>
{
    prepare_runner_host_given(
        parse_workspace_activation(),
        |policy| {
            myelin_ci_sandbox::gvisor::preflight_explicit_userns_policy(
                &policy.helper_dir,
                &policy.runsc_root,
            )
        },
        || {
            let required_path = |name: &'static str| match std::env::var(name) {
                Ok(value) if !value.is_empty() => Ok(std::path::PathBuf::from(value)),
                Ok(_) | Err(std::env::VarError::NotPresent) => {
                    Err(StartupRefusal::RunnerHostPreflight(format!(
                        "{name} is required when MYELIN_CI_RUNNER=1"
                    )))
                }
                Err(std::env::VarError::NotUnicode(_)) => Err(StartupRefusal::RunnerHostPreflight(
                    format!("{name} must be valid Unicode"),
                )),
            };
            let runsc = required_path(myelin_ci_sandbox::gvisor::ENV_RUNSC_BIN)?;
            let rootfs = required_path(myelin_ci_sandbox::gvisor::ENV_GVISOR_ROOTFS)?;
            myelin_ci_sandbox::gvisor::preflight_gvisor_runner_host(&runsc, &rootfs)
                .map_err(StartupRefusal::RunnerHostPreflight)
        },
    )
}

/// Stage-B Hop-A repository-root contract. There is deliberately no fallback: production runner
/// activation requires an explicit absolute, existing, canonical directory.
const ENV_CI_CHECKOUT_REPO_ROOT: &str = "MYELIN_CI_CHECKOUT_REPO_ROOT";

fn prepare_checkout_config_given(
    value: Result<String, std::env::VarError>,
) -> Result<myelin_ci_sandbox::gvisor::GvisorCheckoutConfig, StartupRefusal> {
    let value = match value {
        Ok(value) if !value.is_empty() => value,
        Ok(_) | Err(std::env::VarError::NotPresent) => {
            return Err(StartupRefusal::RunnerHostPreflight(format!(
                "{ENV_CI_CHECKOUT_REPO_ROOT} is required when MYELIN_CI_RUNNER=1; there is no default"
            )))
        }
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(StartupRefusal::RunnerHostPreflight(format!(
                "{ENV_CI_CHECKOUT_REPO_ROOT} must be valid Unicode"
            )))
        }
    };
    myelin_ci_sandbox::gvisor::GvisorCheckoutConfig::enabled(value)
        .map_err(|error| StartupRefusal::RunnerHostPreflight(error.to_string()))
}

fn prepare_checkout_config(
) -> Result<myelin_ci_sandbox::gvisor::GvisorCheckoutConfig, StartupRefusal> {
    prepare_checkout_config_given(std::env::var(ENV_CI_CHECKOUT_REPO_ROOT))
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
    // CT-007 slice 4: parsed and preflighted ONCE here, before PostgreSQL bootstrap -- carried
    // as this exact owned value all the way to `CiRunnerLoop::new` below, never re-derived from a
    // second environment read (which could observe a different snapshot than what was preflighted).
    let gvisor_workspace_config = if runner_host_requested {
        match prepare_runner_host() {
            Ok(config) => Some(config),
            Err(error) => {
                eprintln!("ci-controlplane: startup refused: {error}");
                std::process::exit(1);
            }
        }
    } else {
        None
    };
    let gvisor_checkout_config = if runner_host_requested {
        match prepare_checkout_config() {
            Ok(config) => Some(config),
            Err(error) => {
                eprintln!("ci-controlplane: startup refused: {error}");
                std::process::exit(1);
            }
        }
    } else {
        None
    };

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
        let runner_wiring = match myelin_ci_controlplane::ci_runner_v2_wiring(
            provider.clone(),
            &runner_identity,
            tokio::runtime::Handle::current(),
        ) {
            Ok(wiring) => wiring,
            Err(error) => {
                eprintln!("ci-controlplane: V2 runner composition refused: {error}");
                std::process::exit(1);
            }
        };
        let (runner_resolver, runner_hooks) = runner_wiring.into_parts();
        let runner = myelin_ci_controlplane::CiRunnerLoop::new(
            format!("ci-runner-{}", std::process::id()),
            myelin_ci_controlplane::LINUX_SMALL_V1_RUNNER_LABELS
                .iter()
                .map(|label| (*label).to_owned())
                .collect(),
            vec![myelin_ci_sandbox::TrustTier::Trusted],
            provider.config().region.clone(),
            myelin_ci_controlplane::CI_RUNNER_EXECUTION_LEASE_TTL_SECS,
            region_queue_store.clone(),
            myelin_ci_controlplane::ci_job_queue_store(provider.db_pool().clone()),
            tokio::runtime::Handle::current(),
            runner_resolver,
            runner_reporter,
            runner_hooks,
            provider.db_pool().clone(),
            provider.config().s3.clone(),
            gvisor_workspace_config
                .expect("gvisor_workspace_config is Some whenever runner_host_requested is true"),
            gvisor_checkout_config
                .expect("gvisor_checkout_config is Some whenever runner_host_requested is true"),
        );
        // THE DEFINITION CUTOVER FENCE (CT-007 lease/topology reconciliation) — the LAST boot gate,
        // deliberately after every fallible composition above. It is a single transaction that locks
        // the superseded `wf_definition` row `FOR UPDATE` (the same row a fresh old-binary admission
        // holds `FOR SHARE` until it commits), runs the database-wide backlog probe under that
        // fence, and only then drains the old version and activates this one — atomically. A
        // preflight SELECT could not close that race; see `cutover_definition`'s own doc.
        //
        // Placed here so a refusal cannot leave a half-composed process holding a committed
        // registry transition, and so nothing has spawned that would admit work under the new
        // version before the fence commits.
        if let Err(error) = runner_runtime
            .cutover_definition(&scheduler_provider.region_run_discovery())
            .await
        {
            eprintln!("ci-controlplane: startup refused: {error}");
            std::process::exit(1);
        }
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
    use super::{prepare_checkout_config_given, verify_startup_activation, StartupRefusal};
    use std::env::VarError;
    use std::ffi::OsString;

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

    #[test]
    fn checkout_repository_root_is_required_and_boot_validated_without_a_fallback() {
        let missing = prepare_checkout_config_given(Err(VarError::NotPresent))
            .expect_err("runner activation has no implicit checkout repository root");
        assert!(missing.to_string().contains("there is no default"));

        let root = std::env::temp_dir().join(format!(
            "myelin-checkout-config-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        let _config = prepare_checkout_config_given(Ok(root.to_string_lossy().into_owned()))
            .expect("an explicit canonical directory is accepted");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_runner_flag_has_a_typed_fail_closed_verdict() {
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

    // ───────── CT-007 slice 4: workspace-activation parsing + preflight ordering ─────────

    use super::{
        parse_workspace_activation_given, prepare_runner_host_given, ExplicitUsernsPolicyPaths,
        ParsedWorkspaceActivation,
    };
    use myelin_ci_sandbox::gvisor::GvisorWorkspaceConfig;
    use std::path::PathBuf;

    type EnvVarResult = Result<String, VarError>;

    /// Wraps an already-computed value as the `FnOnce`-closure shape
    /// `parse_workspace_activation_given` takes for every Enabled-only variable, so existing
    /// owned/cloned `EnvVarResult` test fixtures don't need to change shape.
    fn fixed(value: EnvVarResult) -> impl FnOnce() -> EnvVarResult {
        move || value
    }

    /// Panics if the parser ever calls it. Reused across every Disabled/unset-mode test case as
    /// the closure for all five Enabled-only parameters: since a non-capturing fn item is `Copy`,
    /// passing it five times moves nothing. If `parse_workspace_activation_given` ever read an
    /// Enabled-only variable while `enabled == false`, the test would panic here rather than merely
    /// asserting on an already-observed value (Sol's round-2 review: the original `Result`-argument
    /// shape only proved the parser IGNORED already-read values, not that it never read them).
    fn panics_if_called() -> EnvVarResult {
        panic!("must not be read when MYELIN_CI_WORKSPACE_MODE is unset/disabled")
    }

    fn valid_enabled_inputs() -> (EnvVarResult, EnvVarResult, EnvVarResult, EnvVarResult, EnvVarResult)
    {
        (
            Ok("/opt/myelin/gvisor-runsc-root".to_owned()),
            Ok("/var/lib/myelin/userns-leases".to_owned()),
            Ok("/var/lib/myelin/ci-workspaces".to_owned()),
            Ok("1073741824".to_owned()),
            Err(VarError::NotPresent),
        )
    }

    #[test]
    fn unset_or_disabled_workspace_mode_produces_disabled_and_ignores_enabled_only_variables() {
        for mode in [Err(VarError::NotPresent), Ok("disabled".to_owned())] {
            let parsed = parse_workspace_activation_given(
                mode,
                panics_if_called,
                panics_if_called,
                panics_if_called,
                panics_if_called,
                panics_if_called,
            )
            .expect("unset/disabled mode must never fail, and must never read Enabled-only variables");
            assert!(matches!(
                parsed.workspace_config,
                GvisorWorkspaceConfig::Disabled
            ));
            assert!(parsed.explicit_policy.is_none());
        }
    }

    #[test]
    fn invalid_workspace_mode_refuses() {
        let refusal = parse_workspace_activation_given(
            Ok("enable".to_owned()),
            panics_if_called,
            panics_if_called,
            panics_if_called,
            panics_if_called,
            panics_if_called,
        )
        .expect_err("a typo must never silently downgrade to Disabled");
        assert_eq!(refusal, StartupRefusal::InvalidWorkspaceMode("enable".to_owned()));
        assert!(refusal.to_string().contains("allowed values are `enabled`, `disabled`, or unset"));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_workspace_mode_refuses() {
        use std::os::unix::ffi::OsStringExt;

        let invalid = OsString::from_vec(vec![b'e', 0xff]);
        let refusal = parse_workspace_activation_given(
            Err(VarError::NotUnicode(invalid.clone())),
            panics_if_called,
            panics_if_called,
            panics_if_called,
            panics_if_called,
            panics_if_called,
        )
        .expect_err("non-UTF-8 mode must refuse");
        assert_eq!(refusal, StartupRefusal::NonUnicodeWorkspaceMode(invalid));
    }

    #[test]
    fn enabled_mode_requires_every_variable() {
        let (runsc_root, leases_dir, workspaces_dir, capacity, helper_dir) =
            valid_enabled_inputs();
        // Each of the four required variables, omitted one at a time, must refuse -- with the rest
        // valid, isolating exactly which variable's absence caused the refusal.
        let cases: [(&str, EnvVarResult, EnvVarResult, EnvVarResult, EnvVarResult); 4] = [
            (
                myelin_ci_sandbox::gvisor::ENV_EXPLICIT_USERNS_RUNSC_ROOT,
                Err(VarError::NotPresent),
                leases_dir.clone(),
                workspaces_dir.clone(),
                capacity.clone(),
            ),
            (
                "MYELIN_USERNS_LEASES_DIR",
                runsc_root.clone(),
                Err(VarError::NotPresent),
                workspaces_dir.clone(),
                capacity.clone(),
            ),
            (
                "MYELIN_CI_WORKSPACES_DIR",
                runsc_root.clone(),
                leases_dir.clone(),
                Err(VarError::NotPresent),
                capacity.clone(),
            ),
            (
                "MYELIN_CI_WORKSPACE_CAPACITY_BYTES",
                runsc_root.clone(),
                leases_dir.clone(),
                workspaces_dir.clone(),
                Err(VarError::NotPresent),
            ),
        ];
        for (missing_name, runsc_root, leases_dir, workspaces_dir, capacity) in cases {
            let refusal = parse_workspace_activation_given(
                Ok("enabled".to_owned()),
                fixed(runsc_root),
                fixed(leases_dir),
                fixed(workspaces_dir),
                fixed(capacity),
                fixed(helper_dir.clone()),
            )
            .expect_err(&format!("omitting {missing_name} must refuse"));
            assert!(
                refusal.to_string().contains(missing_name),
                "refusal must name the missing variable {missing_name}: {refusal}"
            );
        }
    }

    #[test]
    fn capacity_rejects_missing_zero_malformed_and_overflow() {
        let (runsc_root, leases_dir, workspaces_dir, _capacity, helper_dir) =
            valid_enabled_inputs();
        for bad_capacity in [
            Err(VarError::NotPresent),
            Ok("0".to_owned()),
            Ok("not-a-number".to_owned()),
            Ok("-1".to_owned()),
            Ok("999999999999999999999999999999".to_owned()), // overflows u64
        ] {
            let refusal = parse_workspace_activation_given(
                Ok("enabled".to_owned()),
                fixed(runsc_root.clone()),
                fixed(leases_dir.clone()),
                fixed(workspaces_dir.clone()),
                fixed(bad_capacity.clone()),
                fixed(helper_dir.clone()),
            )
            .expect_err(&format!("{bad_capacity:?} must refuse"));
            assert!(refusal
                .to_string()
                .contains("MYELIN_CI_WORKSPACE_CAPACITY_BYTES"));
        }
        // A genuinely valid positive integer must succeed.
        let ParsedWorkspaceActivation {
            workspace_config, ..
        } = parse_workspace_activation_given(
            Ok("enabled".to_owned()),
            fixed(runsc_root),
            fixed(leases_dir),
            fixed(workspaces_dir),
            fixed(Ok("42".to_owned())),
            fixed(helper_dir),
        )
        .expect("a valid positive capacity must be accepted");
        match workspace_config {
            GvisorWorkspaceConfig::Enabled {
                host_capacity_bytes,
                ..
            } => assert_eq!(host_capacity_bytes, 42),
            GvisorWorkspaceConfig::Disabled => panic!("enabled mode must produce Enabled"),
        }
    }

    #[test]
    fn enabled_parsing_produces_the_exact_config_and_min_pool_size_one() {
        let parsed = parse_workspace_activation_given(
            Ok("enabled".to_owned()),
            || Ok("/opt/myelin/gvisor-runsc-root".to_owned()),
            || Ok("/var/lib/myelin/userns-leases".to_owned()),
            || Ok("/var/lib/myelin/ci-workspaces".to_owned()),
            || Ok("1073741824".to_owned()),
            || Err(VarError::NotPresent), // helper_dir unset -> default
        )
        .expect("a fully-specified Enabled configuration must parse");
        match parsed.workspace_config {
            GvisorWorkspaceConfig::Enabled {
                base_dir,
                host_capacity_bytes,
                leases_dir,
                min_pool_size,
            } => {
                assert_eq!(base_dir, PathBuf::from("/var/lib/myelin/ci-workspaces"));
                assert_eq!(host_capacity_bytes, 1073741824);
                assert_eq!(leases_dir, PathBuf::from("/var/lib/myelin/userns-leases"));
                assert_eq!(min_pool_size, 1, "the userns pool must be fixed at 1");
            }
            GvisorWorkspaceConfig::Disabled => panic!("enabled mode must produce Enabled"),
        }
        let policy = parsed
            .explicit_policy
            .expect("Enabled mode must carry the explicit-userns preflight paths");
        assert_eq!(policy.runsc_root, PathBuf::from("/opt/myelin/gvisor-runsc-root"));
        assert_eq!(policy.helper_dir, PathBuf::from("/usr/bin"), "unset helper dir must default");
    }

    #[test]
    fn explicitly_empty_helper_dir_refuses_rather_than_silently_defaulting() {
        let (runsc_root, leases_dir, workspaces_dir, capacity, _) = valid_enabled_inputs();
        let refusal = parse_workspace_activation_given(
            Ok("enabled".to_owned()),
            fixed(runsc_root),
            fixed(leases_dir),
            fixed(workspaces_dir),
            fixed(capacity),
            || Ok(String::new()),
        )
        .expect_err("an explicitly empty helper dir must refuse, not silently default to /usr/bin");
        assert!(refusal
            .to_string()
            .contains(myelin_ci_sandbox::gvisor::ENV_EXPLICIT_USERNS_HELPER_DIR));
    }

    #[test]
    fn relative_paths_are_refused_for_every_enabled_only_directory() {
        let (runsc_root, leases_dir, workspaces_dir, capacity, _) = valid_enabled_inputs();
        let valid_helper_dir: EnvVarResult = Ok("/usr/bin".to_owned());
        // Each of the four Enabled-only directories, set to a relative path one at a time (the
        // other three valid), must refuse -- proving the absolute-path check applies uniformly,
        // including to the OPTIONAL, explicitly-configured helper directory (Sol's round-2 review:
        // the prior version of this test only ever exercised `runsc_root`).
        let cases: [(&str, EnvVarResult, EnvVarResult, EnvVarResult, EnvVarResult); 4] = [
            (
                "runsc root",
                Ok("relative/path".to_owned()),
                leases_dir.clone(),
                workspaces_dir.clone(),
                valid_helper_dir.clone(),
            ),
            (
                "leases directory",
                runsc_root.clone(),
                Ok("relative/path".to_owned()),
                workspaces_dir.clone(),
                valid_helper_dir.clone(),
            ),
            (
                "workspace directory",
                runsc_root.clone(),
                leases_dir.clone(),
                Ok("relative/path".to_owned()),
                valid_helper_dir,
            ),
            (
                "explicit helper directory",
                runsc_root.clone(),
                leases_dir.clone(),
                workspaces_dir.clone(),
                Ok("relative/path".to_owned()),
            ),
        ];
        for (label, runsc_root, leases_dir, workspaces_dir, helper_dir) in cases {
            let refusal = parse_workspace_activation_given(
                Ok("enabled".to_owned()),
                fixed(runsc_root),
                fixed(leases_dir),
                fixed(workspaces_dir),
                fixed(capacity.clone()),
                fixed(helper_dir),
            )
            .expect_err(&format!("a relative {label} path must be refused"));
            assert!(
                refusal.to_string().contains("must be an absolute path"),
                "{label}: {refusal}"
            );
        }
    }

    #[test]
    fn disabled_preflight_never_calls_the_explicit_userns_preflight() {
        let called = std::cell::Cell::new(false);
        let config = prepare_runner_host_given(
            Ok(ParsedWorkspaceActivation {
                workspace_config: GvisorWorkspaceConfig::Disabled,
                explicit_policy: None,
            }),
            |_policy| {
                called.set(true);
                panic!("explicit-userns preflight must never run for Disabled mode")
            },
            || Ok(()),
        )
        .expect("Disabled mode with a successful rootless preflight must succeed");
        assert!(matches!(config, GvisorWorkspaceConfig::Disabled));
        assert!(!called.get());
    }

    #[test]
    fn enabled_preflight_calls_explicit_userns_first_then_rootless() {
        let order = std::cell::RefCell::new(Vec::new());
        let config = prepare_runner_host_given(
            Ok(ParsedWorkspaceActivation {
                workspace_config: GvisorWorkspaceConfig::Enabled {
                    base_dir: PathBuf::from("/var/lib/myelin/ci-workspaces"),
                    host_capacity_bytes: 1,
                    leases_dir: PathBuf::from("/var/lib/myelin/userns-leases"),
                    min_pool_size: 1,
                },
                explicit_policy: Some(ExplicitUsernsPolicyPaths {
                    helper_dir: PathBuf::from("/usr/bin"),
                    runsc_root: PathBuf::from("/opt/myelin/gvisor-runsc-root"),
                }),
            }),
            |_policy| {
                order.borrow_mut().push("explicit_userns");
                Ok(())
            },
            || {
                order.borrow_mut().push("rootless");
                Ok(())
            },
        )
        .expect("both preflights succeeding must return the Enabled configuration");
        assert!(matches!(config, GvisorWorkspaceConfig::Enabled { .. }));
        assert_eq!(order.into_inner(), vec!["explicit_userns", "rootless"]);
    }

    #[test]
    fn explicit_userns_preflight_failure_prevents_the_rootless_preflight() {
        let rootless_called = std::cell::Cell::new(false);
        let refusal = prepare_runner_host_given(
            Ok(ParsedWorkspaceActivation {
                workspace_config: GvisorWorkspaceConfig::Enabled {
                    base_dir: PathBuf::from("/var/lib/myelin/ci-workspaces"),
                    host_capacity_bytes: 1,
                    leases_dir: PathBuf::from("/var/lib/myelin/userns-leases"),
                    min_pool_size: 1,
                },
                explicit_policy: Some(ExplicitUsernsPolicyPaths {
                    helper_dir: PathBuf::from("/usr/bin"),
                    runsc_root: PathBuf::from("/opt/myelin/gvisor-runsc-root"),
                }),
            }),
            |_policy| Err("synthetic explicit-userns preflight failure".to_owned()),
            || {
                rootless_called.set(true);
                panic!("rootless preflight must never run after an explicit-userns failure")
            },
        )
        .expect_err("an explicit-userns preflight failure must refuse startup");
        assert!(!rootless_called.get());
        assert!(refusal
            .to_string()
            .contains("synthetic explicit-userns preflight failure"));
    }

    #[test]
    fn a_parse_failure_short_circuits_before_either_preflight_runs() {
        let explicit_called = std::cell::Cell::new(false);
        let rootless_called = std::cell::Cell::new(false);
        let refusal = prepare_runner_host_given(
            Err(StartupRefusal::InvalidWorkspaceMode("enable".to_owned())),
            |_policy| {
                explicit_called.set(true);
                Ok(())
            },
            || {
                rootless_called.set(true);
                Ok(())
            },
        )
        .expect_err("a parse failure must propagate directly");
        assert_eq!(refusal, StartupRefusal::InvalidWorkspaceMode("enable".to_owned()));
        assert!(!explicit_called.get());
        assert!(!rootless_called.get());
    }
}
