use myelin_ci_controlplane::run_controlplane_until_shutdown;
use myelin_config::{Mode, MyelinConfig};
use myelin_events::OutboxStore;
use myelin_storage::{all_durable_migrations, HotTables, PgBootstrap, PgOutboxBacking};
use myelin_substrate::Config;
use std::ffi::OsString;
use std::fmt;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
enum StartupRefusal {
    InvalidRunnerSetting(String),
    NonUnicodeRunnerSetting(OsString),
    RunnerHostPreflight(String),
    InvalidWorkspaceMode(String),
    NonUnicodeWorkspaceMode(OsString),
    NonTerminalNullStageBacklog { count: i64 },
    UnknownRunnerExecutionProfile(String),
    NonUnicodeRunnerExecutionProfiles(OsString),
    EmptyRunnerExecutionProfiles,
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
            Self::UnknownRunnerExecutionProfile(value) => write!(
                f,
                "invalid {ENV_CI_RUNNER_EXECUTION_PROFILES} entry {value:?}; allowed profiles are \
                 `linux-small-v1` and `linux-build-v1`"
            ),
            Self::NonUnicodeRunnerExecutionProfiles(value) => write!(
                f,
                "invalid {ENV_CI_RUNNER_EXECUTION_PROFILES} value {value:?} contains non-UTF-8 bytes"
            ),
            Self::EmptyRunnerExecutionProfiles => write!(
                f,
                "{ENV_CI_RUNNER_EXECUTION_PROFILES} names no profiles; omit it for the default \
                 `linux-small-v1`"
            ),
        }
    }
}

const ENV_CI_RUNNER_EXECUTION_PROFILES: &str = "MYELIN_CI_RUNNER_EXECUTION_PROFILES";

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

fn resolve_runner_execution_profiles(
    setting: Result<String, std::env::VarError>,
) -> Result<Vec<myelin_ci_controlplane::CiExecutionProfileV1>, StartupRefusal> {
    let value = match setting {
        Err(std::env::VarError::NotPresent) => {
            return Ok(vec![
                myelin_ci_controlplane::CiExecutionProfileV1::LinuxSmallV1,
            ]);
        }
        Err(std::env::VarError::NotUnicode(value)) => {
            return Err(StartupRefusal::NonUnicodeRunnerExecutionProfiles(value));
        }
        Ok(value) => value,
    };
    let mut profiles = Vec::new();
    for token in value
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        match myelin_ci_controlplane::CiExecutionProfileV1::from_label(token) {
            Some(profile) if !profiles.contains(&profile) => profiles.push(profile),
            Some(_) => {}
            None => {
                return Err(StartupRefusal::UnknownRunnerExecutionProfile(
                    token.to_owned(),
                ));
            }
        }
    }
    if profiles.is_empty() {
        return Err(StartupRefusal::EmptyRunnerExecutionProfiles);
    }
    Ok(profiles)
}

fn checkout_workspace_capability_requested(
    runner_setting: &Result<String, std::env::VarError>,
    workspace_mode: &Result<String, std::env::VarError>,
) -> bool {
    matches!(
        (runner_setting, workspace_mode),
        (Ok(runner), Ok(workspace)) if runner == "1" && workspace == "enabled"
    )
}

#[derive(Debug)]
struct ExplicitUsernsPolicyPaths {
    helper_dir: std::path::PathBuf,
    runsc_root: std::path::PathBuf,
}

#[derive(Debug)]
struct ParsedWorkspaceActivation {
    workspace_config: myelin_ci_sandbox::gvisor::GvisorWorkspaceConfig,
    explicit_policy: Option<ExplicitUsernsPolicyPaths>,
}

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
    let required_absolute_path =
        |name: &'static str, value: Result<String, std::env::VarError>| match value {
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

fn main() {
    myelin_events::install_payload_free_panic_hook("ci-controlplane");
    let runner_setting = std::env::var("MYELIN_CI_RUNNER");
    let workspace_mode = std::env::var("MYELIN_CI_WORKSPACE_MODE");
    let checkout_workspace_runner_requested =
        checkout_workspace_capability_requested(&runner_setting, &workspace_mode);
    if let Err(error) = myelin_ci_sandbox::gvisor::prepare_checkout_host_verification_capability(
        checkout_workspace_runner_requested,
    ) {
        eprintln!(
            "ci-controlplane: startup refused: checkout host-verification privilege \
             normalization failed: {error}"
        );
        std::process::exit(1);
    }
    let runner_host_requested = matches!(&runner_setting, Ok(value) if value == "1");
    if let Err(e) = verify_startup_activation(runner_setting) {
        eprintln!("ci-controlplane: startup refused: {e}");
        std::process::exit(1);
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("ci-controlplane: failed to construct the async runtime: {error}");
            std::process::exit(1);
        }
    };
    runtime.block_on(run(runner_host_requested));
}

async fn run(runner_host_requested: bool) {
    let shutdown_signals = match ShutdownSignals::install() {
        Ok(signals) => signals,
        Err(error) => {
            eprintln!("ci-controlplane: failed to install shutdown signal handlers: {error}");
            std::process::exit(1);
        }
    };
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    let signal_shutdown = shutdown_tx.clone();
    let signal_task = tokio::spawn(async move {
        shutdown_signals.wait().await;
        let _ = signal_shutdown.send(true);
    });
    eprintln!("ci-controlplane: shutdown handlers armed; startup termination is intake-gated");
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
    let bootstrap = match PgBootstrap::connect_configured(platform_config).await {
        Ok(bootstrap) => bootstrap,
        Err(e) => {
            eprintln!("ci-controlplane: database bootstrap refused to start: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = bootstrap.migrate_foundation().await {
        eprintln!(
            "ci-controlplane: cannot apply the substrate foundation migrations \
             (outbox/consumer_dedup): {e}"
        );
        std::process::exit(1);
    }
    if let Err(e) = bootstrap
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
    {
        eprintln!("ci-controlplane: cannot apply the durable migration aggregate (identity/pseudonym/placement/kms/cost/erasure): {e}");
        std::process::exit(1);
    }
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
    let provider = match bootstrap.into_runtime().await {
        Ok(provider) => provider,
        Err(e) => {
            eprintln!("ci-controlplane: database runtime handoff refused to start: {e}");
            std::process::exit(1);
        }
    };
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
    if let Err(e) = myelin_ci_controlplane::verify_ci_cost_event_shape(provider.db_pool()).await {
        eprintln!("ci-controlplane: ci_cost_event shape assertion failed: {e}");
        std::process::exit(1);
    }
    let outbox = OutboxStore::durable(Arc::new(PgOutboxBacking::new(
        provider.db_pool().clone(),
        tokio::runtime::Handle::current(),
    )));
    let _ci_cost_events = myelin_ci_controlplane::ci_cost_event_store(
        provider.db_pool().clone(),
        myelin_tenancy::Region(provider.config().region.clone()),
    );
    let _ci_job_accounting = myelin_ci_controlplane::ci_job_accounting_store(
        provider.db_pool().clone(),
        myelin_tenancy::Region(provider.config().region.clone()),
    );
    let region_queue_store = scheduler_provider.region_queue_store();
    let operational_ledger = myelin_storage::DurableCostLedger::new(provider.clone());
    let reaper = myelin_ci_controlplane::JobQueueReaper::new(
        region_queue_store.clone(),
        provider.config().region.clone(),
        std::time::Duration::from_secs(15),
    )
    .with_cancelled_accounting(provider.db_pool().clone(), operational_ledger);
    let reaper_task = tokio::spawn(reaper.run_until_shutdown(shutdown_tx.subscribe()));
    let runner_host = if runner_host_requested {
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
            runner_reporter.clone(),
        ) {
            Ok(wiring) => wiring,
            Err(error) => {
                eprintln!("ci-controlplane: V2 runner composition refused: {error}");
                std::process::exit(1);
            }
        };
        let (runner_resolver, runner_hooks) = runner_wiring.into_parts();
        let runner_execution_profiles = match resolve_runner_execution_profiles(std::env::var(
            ENV_CI_RUNNER_EXECUTION_PROFILES,
        )) {
            Ok(profiles) => profiles,
            Err(refusal) => {
                eprintln!("ci-controlplane: startup refused: {refusal}");
                std::process::exit(1);
            }
        };
        let runner = myelin_ci_controlplane::CiRunnerLoop::new(
            format!("ci-runner-{}", std::process::id()),
            myelin_ci_controlplane::runner_labels_for_profiles(&runner_execution_profiles),
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
    let runner_host_failures = runner_host
        .as_ref()
        .map(myelin_ci_controlplane::CiRunnerHostHandle::failure_receiver);
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
    use super::{
        checkout_workspace_capability_requested, prepare_checkout_config_given,
        resolve_runner_execution_profiles, verify_startup_activation, StartupRefusal,
    };
    use myelin_ci_controlplane::{runner_labels_for_profiles, CiExecutionProfileV1};
    use std::env::VarError;
    use std::ffi::OsString;

    #[test]
    fn unset_runner_profiles_default_to_small_only_and_leave_labels_byte_unchanged() {
        let profiles = resolve_runner_execution_profiles(Err(VarError::NotPresent)).unwrap();
        assert_eq!(profiles, vec![CiExecutionProfileV1::LinuxSmallV1]);
        assert_eq!(
            runner_labels_for_profiles(&profiles),
            vec!["linux".to_owned(), "linux-small-v1".to_owned()]
        );
    }

    #[test]
    fn build_capable_host_advertises_the_sorted_deduped_union() {
        let profiles =
            resolve_runner_execution_profiles(Ok("linux-small-v1,linux-build-v1".to_owned()))
                .unwrap();
        assert_eq!(
            profiles,
            vec![
                CiExecutionProfileV1::LinuxSmallV1,
                CiExecutionProfileV1::LinuxBuildV1
            ]
        );
        assert_eq!(
            runner_labels_for_profiles(&profiles),
            vec![
                "linux".to_owned(),
                "linux-build-v1".to_owned(),
                "linux-small-v1".to_owned()
            ]
        );
    }

    #[test]
    fn runner_profiles_dedupe_and_ignore_blank_entries() {
        let profiles =
            resolve_runner_execution_profiles(Ok(" linux-build-v1 , , linux-build-v1 ".to_owned()))
                .unwrap();
        assert_eq!(profiles, vec![CiExecutionProfileV1::LinuxBuildV1]);
    }

    #[test]
    fn unknown_or_empty_runner_profiles_fail_closed() {
        assert_eq!(
            resolve_runner_execution_profiles(Ok("linux-mega-v1".to_owned())),
            Err(StartupRefusal::UnknownRunnerExecutionProfile(
                "linux-mega-v1".to_owned()
            ))
        );
        assert_eq!(
            resolve_runner_execution_profiles(Ok("  , ".to_owned())),
            Err(StartupRefusal::EmptyRunnerExecutionProfiles)
        );
        assert_eq!(
            resolve_runner_execution_profiles(Err(VarError::NotUnicode(OsString::from("x")))),
            Err(StartupRefusal::NonUnicodeRunnerExecutionProfiles(
                OsString::from("x")
            ))
        );
    }

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
    fn dac_read_search_is_retained_only_for_the_exact_enabled_runner_path() {
        let absent = Err(VarError::NotPresent);
        assert!(checkout_workspace_capability_requested(
            &Ok("1".to_owned()),
            &Ok("enabled".to_owned())
        ));
        for (runner, workspace) in [
            (Ok("0".to_owned()), Ok("enabled".to_owned())),
            (absent, Ok("enabled".to_owned())),
            (Ok("1".to_owned()), Ok("disabled".to_owned())),
            (Ok("1".to_owned()), Ok("invalid".to_owned())),
            (Ok("1".to_owned()), Err(VarError::NotPresent)),
        ] {
            assert!(
                !checkout_workspace_capability_requested(&runner, &workspace),
                "non-enabled configuration must drop CAP_DAC_READ_SEARCH entirely"
            );
        }
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
        assert!(
            !checkout_workspace_capability_requested(
                &Err(VarError::NotUnicode(invalid.clone())),
                &Ok("enabled".to_owned())
            ),
            "a non-Unicode runner configuration must drop CAP_DAC_READ_SEARCH entirely"
        );
        let refusal = verify_startup_activation(Err(VarError::NotUnicode(invalid.clone())))
            .expect_err("non-UTF-8 runner setting must be refused");

        assert_eq!(refusal, StartupRefusal::NonUnicodeRunnerSetting(invalid));
        assert!(refusal.to_string().contains("contains non-UTF-8 bytes"));
        assert!(refusal
            .to_string()
            .contains("allowed values are `0`, `1`, or unset"));
    }

    use super::{
        parse_workspace_activation_given, prepare_runner_host_given, ExplicitUsernsPolicyPaths,
        ParsedWorkspaceActivation,
    };
    use myelin_ci_sandbox::gvisor::GvisorWorkspaceConfig;
    use std::path::PathBuf;

    type EnvVarResult = Result<String, VarError>;

    fn fixed(value: EnvVarResult) -> impl FnOnce() -> EnvVarResult {
        move || value
    }

    fn panics_if_called() -> EnvVarResult {
        panic!("must not be read when MYELIN_CI_WORKSPACE_MODE is unset/disabled")
    }

    fn valid_enabled_inputs() -> (
        EnvVarResult,
        EnvVarResult,
        EnvVarResult,
        EnvVarResult,
        EnvVarResult,
    ) {
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
            .expect(
                "unset/disabled mode must never fail, and must never read Enabled-only variables",
            );
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
        assert_eq!(
            refusal,
            StartupRefusal::InvalidWorkspaceMode("enable".to_owned())
        );
        assert!(refusal
            .to_string()
            .contains("allowed values are `enabled`, `disabled`, or unset"));
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
        let (runsc_root, leases_dir, workspaces_dir, capacity, helper_dir) = valid_enabled_inputs();
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
            Ok("999999999999999999999999999999".to_owned()),
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
            || Err(VarError::NotPresent),
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
        assert_eq!(
            policy.runsc_root,
            PathBuf::from("/opt/myelin/gvisor-runsc-root")
        );
        assert_eq!(
            policy.helper_dir,
            PathBuf::from("/usr/bin"),
            "unset helper dir must default"
        );
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
        assert_eq!(
            refusal,
            StartupRefusal::InvalidWorkspaceMode("enable".to_owned())
        );
        assert!(!explicit_called.get());
        assert!(!rootless_called.get());
    }
}
