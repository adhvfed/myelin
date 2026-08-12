use super::*;
use crate::hardening::HardeningProfile;
use crate::runner::RetryableAttemptCause;
use crate::user_namespace::{UserNamespaceAllocator, UserNamespaceAllocatorError};
use crate::workspace_manager::{WorkspaceManager, WorkspaceManagerError, WorkspaceStorageMode};
use crate::{
    CompletionSettlementOwner, HookError, JobSpec, LaunchPermit, ReserveHandle, ResourceUsage,
    RunnerHooks, SandboxBackend, SandboxCancellation, SandboxCycleOutcome, SandboxHandle,
    SandboxLaunch, SandboxLaunchError, SandboxOutputSink, SandboxResult,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Debug)]
pub enum GvisorError {
    Hook(crate::HookError),
    Hardening(String),
    Runtime(String),
    Image(String),
}

impl std::fmt::Display for GvisorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GvisorError::Hook(e) => write!(f, "gvisor backend: guarantee hook failed: {e}"),
            GvisorError::Hardening(s) => write!(f, "gvisor backend: hardening not enforced: {s}"),
            GvisorError::Runtime(s) => write!(f, "gvisor backend: runsc error: {s}"),
            GvisorError::Image(s) => write!(f, "gvisor backend: image resolution refused: {s}"),
        }
    }
}

impl std::error::Error for GvisorError {}

impl From<crate::HookError> for GvisorError {
    fn from(e: crate::HookError) -> Self {
        GvisorError::Hook(e)
    }
}

impl From<crate::asset_registry::AssetRegistryError> for GvisorError {
    fn from(e: crate::asset_registry::AssetRegistryError) -> Self {
        GvisorError::Image(e.to_string())
    }
}

pub trait RunscChild {
    fn kill(&mut self) -> Result<(), String>;
    fn wait(&mut self) -> Result<i32, String>;
}

pub struct GvisorBackend {
    pub(super) live: Mutex<std::collections::HashMap<String, RunscProc>>,
    pub(super) registry: Option<Arc<crate::asset_registry::GvisorAssetRegistry>>,
    pub(super) workspace_integration: WorkspaceIntegration,
    pub(super) checkout: GvisorCheckoutConfig,
    pub(super) rootfs_overlay: Option<Arc<crate::rootfs_overlay::RootfsOverlayManager>>,
}

#[derive(Debug)]
pub enum GvisorWorkspaceConfig {
    Disabled,
    Enabled {
        base_dir: PathBuf,
        host_capacity_bytes: u64,
        leases_dir: PathBuf,
        min_pool_size: u32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GvisorCheckoutConfig(CheckoutConfigState);

#[derive(Clone, Debug, PartialEq, Eq)]
enum CheckoutConfigState {
    Disabled,
    Enabled { repo_root: PathBuf },
}

#[derive(Debug, PartialEq, Eq)]
pub enum GvisorCheckoutConfigError {
    NotAbsolute(PathBuf),
    NotADirectory {
        path: PathBuf,
        detail: String,
    },
    NotCanonical {
        configured: PathBuf,
        canonical: PathBuf,
    },
}

impl std::fmt::Display for GvisorCheckoutConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GvisorCheckoutConfigError::NotAbsolute(path) => write!(
                f,
                "checkout repository root {path:?} is not absolute - a checkout root must be an \
                 absolute path so no working-directory context can redirect it"
            ),
            GvisorCheckoutConfigError::NotADirectory { path, detail } => write!(
                f,
                "checkout repository root {path:?} is not an existing directory: {detail}"
            ),
            GvisorCheckoutConfigError::NotCanonical {
                configured,
                canonical,
            } => write!(
                f,
                "checkout repository root {configured:?} is not canonical (it resolves to \
                 {canonical:?}) - refusing a symlinked or `..`-bearing root so the durable root is \
                 exactly the audited path"
            ),
        }
    }
}

impl std::error::Error for GvisorCheckoutConfigError {}

impl GvisorCheckoutConfig {
    pub fn disabled() -> Self {
        GvisorCheckoutConfig(CheckoutConfigState::Disabled)
    }

    pub fn enabled(repo_root: impl Into<PathBuf>) -> Result<Self, GvisorCheckoutConfigError> {
        let repo_root = repo_root.into();
        if !repo_root.is_absolute() {
            return Err(GvisorCheckoutConfigError::NotAbsolute(repo_root));
        }
        let metadata = std::fs::metadata(&repo_root).map_err(|error| {
            GvisorCheckoutConfigError::NotADirectory {
                path: repo_root.clone(),
                detail: error.to_string(),
            }
        })?;
        if !metadata.is_dir() {
            return Err(GvisorCheckoutConfigError::NotADirectory {
                path: repo_root.clone(),
                detail: "path exists but is not a directory".to_string(),
            });
        }
        let canonical = std::fs::canonicalize(&repo_root).map_err(|error| {
            GvisorCheckoutConfigError::NotADirectory {
                path: repo_root.clone(),
                detail: format!("canonicalization failed: {error}"),
            }
        })?;
        if canonical != repo_root {
            return Err(GvisorCheckoutConfigError::NotCanonical {
                configured: repo_root,
                canonical,
            });
        }
        Ok(GvisorCheckoutConfig(CheckoutConfigState::Enabled {
            repo_root,
        }))
    }

    pub(crate) fn repo_root(&self) -> Option<&Path> {
        match &self.0 {
            CheckoutConfigState::Disabled => None,
            CheckoutConfigState::Enabled { repo_root } => Some(repo_root),
        }
    }
}

#[derive(Debug)]
pub enum GvisorBackendInitError {
    Workspace(WorkspaceManagerError),
    UserNamespace(UserNamespaceAllocatorError),
}

impl std::fmt::Display for GvisorBackendInitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GvisorBackendInitError::Workspace(e) => {
                write!(f, "workspace manager initialization failed: {e}")
            }
            GvisorBackendInitError::UserNamespace(e) => {
                write!(f, "user-namespace allocator initialization failed: {e}")
            }
        }
    }
}

impl std::error::Error for GvisorBackendInitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            GvisorBackendInitError::Workspace(e) => Some(e),
            GvisorBackendInitError::UserNamespace(e) => Some(e),
        }
    }
}

pub(super) struct RunscProc {
    pub(super) child: Box<dyn RunscChild + Send>,
    pub(super) bundle_dir: PathBuf,
}

pub struct ContainerRun {
    pub child: Box<dyn RunscChild + Send>,
    pub bundle_dir: PathBuf,
    pub result: SandboxResult,
    pub run_error: Option<String>,
}

pub(super) struct JobGuestRoot {
    path: PathBuf,
    _overlay: Option<crate::rootfs_overlay::RootfsOverlay>,
}

impl JobGuestRoot {
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl GvisorBackend {
    pub fn new(registry: Arc<crate::asset_registry::GvisorAssetRegistry>) -> GvisorBackend {
        GvisorBackend {
            live: Mutex::new(std::collections::HashMap::new()),
            registry: Some(registry),
            workspace_integration: WorkspaceIntegration::Disabled,
            checkout: GvisorCheckoutConfig::disabled(),
            rootfs_overlay: None,
        }
    }

    pub fn git_wire_only() -> GvisorBackend {
        GvisorBackend {
            live: Mutex::new(std::collections::HashMap::new()),
            registry: None,
            workspace_integration: WorkspaceIntegration::Disabled,
            checkout: GvisorCheckoutConfig::disabled(),
            rootfs_overlay: None,
        }
    }

    pub fn try_new(
        registry: Arc<crate::asset_registry::GvisorAssetRegistry>,
        workspace_config: GvisorWorkspaceConfig,
        incident_sink: crate::workspace_manager::IncidentSink,
    ) -> Result<GvisorBackend, GvisorBackendInitError> {
        Self::try_new_with_builders(
            registry,
            workspace_config,
            incident_sink,
            UserNamespaceAllocator::try_new,
            WorkspaceManager::try_new,
        )
    }

    fn try_new_with_builders<U, W>(
        registry: Arc<crate::asset_registry::GvisorAssetRegistry>,
        workspace_config: GvisorWorkspaceConfig,
        incident_sink: crate::workspace_manager::IncidentSink,
        build_userns: U,
        build_workspace: W,
    ) -> Result<GvisorBackend, GvisorBackendInitError>
    where
        U: FnOnce(
            PathBuf,
            u32,
            crate::workspace_manager::IncidentSink,
        ) -> Result<UserNamespaceAllocator, UserNamespaceAllocatorError>,
        W: FnOnce(
            WorkspaceStorageMode,
            crate::workspace_manager::IncidentSink,
        ) -> Result<WorkspaceManager, WorkspaceManagerError>,
    {
        let workspace_integration = match workspace_config {
            GvisorWorkspaceConfig::Disabled => WorkspaceIntegration::Disabled,
            GvisorWorkspaceConfig::Enabled {
                base_dir,
                host_capacity_bytes,
                leases_dir,
                min_pool_size,
            } => {
                let userns_allocator =
                    build_userns(leases_dir, min_pool_size, incident_sink.clone())
                        .map_err(GvisorBackendInitError::UserNamespace)?;
                let workspace_manager = build_workspace(
                    WorkspaceStorageMode::EphemeralDisk {
                        base_dir,
                        host_capacity_bytes,
                    },
                    incident_sink,
                )
                .map_err(GvisorBackendInitError::Workspace)?;
                WorkspaceIntegration::Enabled {
                    workspace_manager,
                    userns_allocator,
                }
            }
        };
        Ok(GvisorBackend {
            live: Mutex::new(std::collections::HashMap::new()),
            registry: Some(registry),
            workspace_integration,
            checkout: GvisorCheckoutConfig::disabled(),
            rootfs_overlay: None,
        })
    }

    pub fn with_checkout_config(mut self, checkout: GvisorCheckoutConfig) -> GvisorBackend {
        self.checkout = checkout;
        self
    }

    pub fn with_rootfs_overlay_manager(
        mut self,
        manager: Arc<crate::rootfs_overlay::RootfsOverlayManager>,
    ) -> GvisorBackend {
        self.rootfs_overlay = Some(manager);
        self
    }

    pub(super) fn materialize_job_guest_root(
        &self,
        verified_rootfs: &crate::asset_registry::VerifiedRootfs,
        job_key: &str,
    ) -> Result<JobGuestRoot, String> {
        match &self.rootfs_overlay {
            None => Ok(JobGuestRoot {
                path: verified_rootfs.path().to_path_buf(),
                _overlay: None,
            }),
            Some(manager) => {
                let workload_root = crate::rootfs_overlay::WorkloadRootPermissions::new(
                    unsafe { libc::geteuid() },
                    unsafe { libc::getegid() },
                    0o755,
                )
                .map_err(|error| format!("derive per-job overlay root permissions: {error}"))?;
                let overlay = manager
                    .create_overlay(verified_rootfs, job_key, workload_root)
                    .map_err(|error| format!("create per-job rootfs overlay: {error}"))?;
                Ok(JobGuestRoot {
                    path: overlay.path().to_path_buf(),
                    _overlay: Some(overlay),
                })
            }
        }
    }

    pub fn oci_config(spec: &JobSpec) -> Result<OciConfig, GvisorError> {
        spec.validate_secret_coverage()
            .map_err(|error| GvisorError::Runtime(format!("secret injection refused: {error}")))?;
        let profile = HardeningProfile::derive(spec);
        profile.assert_enforced().map_err(GvisorError::Hardening)?;
        Ok(OciConfig::from_spec(spec, &profile))
    }

    fn launch_with<F>(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
        run: F,
    ) -> Result<SandboxLaunch, SandboxLaunchError<GvisorError>>
    where
        F: FnOnce(
            &JobSpec,
            &OciConfig,
            LaunchPermit,
            &Path,
            &str,
            RuntimePreparation<'_>,
        )
            -> Result<RuntimeFinalization<Result<ContainerRun, RunFailure>>, RunFailure>,
    {
        self.launch_compute_with(spec, hooks, run)
    }

    fn launch_compute_with<F>(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
        run: F,
    ) -> Result<SandboxLaunch, SandboxLaunchError<GvisorError>>
    where
        F: FnOnce(
            &JobSpec,
            &OciConfig,
            LaunchPermit,
            &Path,
            &str,
            RuntimePreparation<'_>,
        )
            -> Result<RuntimeFinalization<Result<ContainerRun, RunFailure>>, RunFailure>,
    {
        let (profile, verified_rootfs, cargo_vendor) =
            self.compute_launch_preflight(spec, hooks)?;
        let reserve = hooks
            .reserve(spec)
            .map_err(|e| SandboxLaunchError::Failed(e.into()))?;
        let container_id = format!("myelin-prod-{}-{}", std::process::id(), unique_suffix());
        self.launch_compute_common_body(
            spec,
            hooks,
            run,
            profile,
            verified_rootfs,
            cargo_vendor,
            reserve,
            container_id,
        )
    }

    fn compute_launch_preflight(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
    ) -> Result<
        (
            HardeningProfile,
            &crate::asset_registry::VerifiedRootfs,
            Option<crate::asset_registry::VerifiedCargoVendor>,
        ),
        SandboxLaunchError<GvisorError>,
    > {
        hooks
            .enforce_isolation_floor(spec)
            .map_err(|e| SandboxLaunchError::Failed(e.into()))?;
        spec.validate_secret_coverage().map_err(|error| {
            SandboxLaunchError::Failed(GvisorError::Runtime(format!(
                "secret injection refused: {error}"
            )))
        })?;
        let cargo_vendor_reference = validated_cargo_vendor_reference(spec)
            .map_err(|error| SandboxLaunchError::Failed(GvisorError::Runtime(error)))?;
        let profile = HardeningProfile::derive(spec);
        profile
            .assert_enforced()
            .map_err(|e| SandboxLaunchError::Failed(GvisorError::Hardening(e)))?;

        let registry = self.registry.as_ref().ok_or_else(|| {
            SandboxLaunchError::Failed(GvisorError::Image(
                "this GvisorBackend was constructed via GvisorBackend::git_wire_only() (no asset \
                 registry) and cannot launch an ordinary image-bearing job - construct it via \
                 GvisorBackend::new(registry) for CI/agent job launch"
                    .to_string(),
            ))
        })?;
        let verified_rootfs = registry
            .resolve(&spec.image)
            .map_err(|e| SandboxLaunchError::Failed(e.into()))?;
        let cargo_vendor = cargo_vendor_reference
            .as_ref()
            .map(|reference| registry.resolve_cargo_vendor(reference).cloned())
            .transpose()
            .map_err(|error| SandboxLaunchError::Failed(GvisorError::Runtime(error.to_string())))?;
        if cargo_vendor.is_some()
            && matches!(&self.workspace_integration, WorkspaceIntegration::Disabled)
        {
            return Err(SandboxLaunchError::Failed(GvisorError::Runtime(
                "a structured Cargo vendor build requires the Enabled workspace integration; \
                 refusing the compute route rather than launching without its vendor mounts"
                    .to_string(),
            )));
        }

        if let WorkspaceIntegration::Enabled {
            workspace_manager,
            userns_allocator,
        } = &self.workspace_integration
        {
            workspace_manager.check_health().map_err(|e| {
                SandboxLaunchError::Failed(GvisorError::Runtime(format!(
                    "workspace manager health check failed: {e}"
                )))
            })?;
            userns_allocator.check_identity().map_err(|e| {
                SandboxLaunchError::Failed(GvisorError::Runtime(format!(
                    "userns allocator identity check failed: {e}"
                )))
            })?;
        }

        Ok((profile, verified_rootfs, cargo_vendor))
    }

    #[allow(clippy::too_many_arguments)]
    fn launch_compute_common_body<F>(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
        run: F,
        profile: HardeningProfile,
        verified_rootfs: &crate::asset_registry::VerifiedRootfs,
        cargo_vendor: Option<crate::asset_registry::VerifiedCargoVendor>,
        reserve: ReserveHandle,
        container_id: String,
    ) -> Result<SandboxLaunch, SandboxLaunchError<GvisorError>>
    where
        F: FnOnce(
            &JobSpec,
            &OciConfig,
            LaunchPermit,
            &Path,
            &str,
            RuntimePreparation<'_>,
        )
            -> Result<RuntimeFinalization<Result<ContainerRun, RunFailure>>, RunFailure>,
    {
        let job_guest_root = match self.materialize_job_guest_root(verified_rootfs, &container_id) {
            Ok(root) => root,
            Err(message) => {
                return Err(self.dispose_run_failure(
                    spec,
                    hooks,
                    &reserve,
                    RunFailure::uncommitted(format!("per-job rootfs overlay: {message}")),
                ));
            }
        };

        let mut enabled_context: Option<EnabledLaunchContext> = None;
        let cfg = match &self.workspace_integration {
            WorkspaceIntegration::Disabled => OciConfig::from_spec(spec, &profile),
            WorkspaceIntegration::Enabled {
                workspace_manager,
                userns_allocator,
            } => match acquire_enabled_workspace(
                spec,
                &profile,
                &container_id,
                job_guest_root.path().to_path_buf(),
                workspace_manager,
                userns_allocator,
                cargo_vendor,
            ) {
                Ok((cfg, context)) => {
                    enabled_context = Some(context);
                    cfg
                }
                Err(failure) => {
                    return Err(self.dispose_run_failure(
                        spec,
                        hooks,
                        &reserve,
                        RunFailure::uncommitted(failure.message),
                    ));
                }
            },
        };

        let prep_result: Result<RuntimePreparation<'_>, String> = match &mut enabled_context {
            None => RuntimePreparation::new(&cfg, RuntimeBinding::Rootless),
            Some(context) => match revalidated_explicit_userns_root_identity() {
                Ok(expected_root_identity) => RuntimePreparation::new(
                    &cfg,
                    RuntimeBinding::Enabled {
                        expected_root_identity,
                        context,
                    },
                ),
                Err(reason) => Err(format!("runsc-root identity revalidation failed: {reason}")),
            },
        };
        let prep = match prep_result {
            Ok(prep) => prep,
            Err(message) => {
                let workspace_manager = match &self.workspace_integration {
                    WorkspaceIntegration::Enabled {
                        workspace_manager, ..
                    } => Some(workspace_manager),
                    WorkspaceIntegration::Disabled => None,
                };
                let mut message = match (enabled_context, workspace_manager) {
                    (Some(context), Some(workspace_manager)) => {
                        let diagnostics = cleanup_pre_bind_failure(context, workspace_manager);
                        join_diagnostics(message, &diagnostics)
                    }
                    _ => message,
                };
                if let Err(release_error) = hooks.release_unused(spec, &reserve) {
                    message = format!(
                        "{message} AND releasing the unused reservation also failed: \
                         {release_error}"
                    );
                }
                return Err(SandboxLaunchError::Failed(GvisorError::Runtime(message)));
            }
        };

        let launch_permit = match hooks.acquire_launch_permit(spec) {
            Ok(permit) => permit,
            Err(attribute_error) => {
                let workspace_manager = match &self.workspace_integration {
                    WorkspaceIntegration::Enabled {
                        workspace_manager, ..
                    } => Some(workspace_manager),
                    WorkspaceIntegration::Disabled => None,
                };
                let cleanup_diagnostics = match (enabled_context, workspace_manager) {
                    (Some(context), Some(workspace_manager)) => {
                        cleanup_pre_bind_failure(context, workspace_manager)
                    }
                    _ => Vec::new(),
                };
                let release_result = hooks.release_unused(spec, &reserve);
                if cleanup_diagnostics.is_empty() && release_result.is_ok() {
                    return Err(SandboxLaunchError::Failed(attribute_error.into()));
                }
                let mut message = join_diagnostics(
                    GvisorError::from(attribute_error).to_string(),
                    &cleanup_diagnostics,
                );
                if let Err(release_error) = release_result {
                    message = format!(
                        "{message} AND releasing the unused reservation also failed: \
                         {release_error}"
                    );
                }
                return Err(SandboxLaunchError::Failed(GvisorError::Runtime(message)));
            }
        };
        let outer_result = run(
            spec,
            &cfg,
            launch_permit,
            job_guest_root.path(),
            &container_id,
            prep,
        );

        let settled_result = match outer_result {
            Err(run_failure) => {
                let workspace_manager = match &self.workspace_integration {
                    WorkspaceIntegration::Enabled {
                        workspace_manager, ..
                    } => Some(workspace_manager),
                    WorkspaceIntegration::Disabled => None,
                };
                let cleanup_diagnostics = match (enabled_context, workspace_manager) {
                    (Some(context), Some(workspace_manager)) => {
                        cleanup_pre_bind_failure(context, workspace_manager)
                    }
                    _ => Vec::new(),
                };
                let run_failure = cleanup_diagnostics
                    .into_iter()
                    .fold(run_failure, augment_run_failure_message);
                return Err(self.dispose_run_failure(spec, hooks, &reserve, run_failure));
            }
            Ok(finalization) => {
                let workspace_manager = match &self.workspace_integration {
                    WorkspaceIntegration::Enabled {
                        workspace_manager, ..
                    } => Some(workspace_manager),
                    WorkspaceIntegration::Disabled => None,
                };
                settle_enabled_finalization(finalization, enabled_context, workspace_manager)
            }
        };

        let ContainerRun {
            child,
            bundle_dir,
            result,
            run_error,
        } = match settled_result {
            Ok(container_run) => container_run,
            Err(run_failure) => {
                return Err(self.dispose_run_failure(spec, hooks, &reserve, run_failure));
            }
        };

        let guest_id = format!("runsc-{}", spec.idem_token.0);
        self.live
            .lock()
            .unwrap()
            .insert(guest_id.clone(), RunscProc { child, bundle_dir });

        if let Err(error) = hooks.settle_completed(spec, &reserve, result.usage) {
            let _ = self.kill(&SandboxHandle {
                guest_id: guest_id.clone(),
            });
            return Err(SandboxLaunchError::Failed(error.into()));
        }

        Ok(SandboxLaunch {
            handle: SandboxHandle { guest_id },
            result,
            output_complete: run_error.is_none(),
        })
    }

    pub(super) fn launch_compute_orchestrated_with<F>(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
        run: F,
    ) -> Result<SandboxCycleOutcome, SandboxLaunchError<GvisorError>>
    where
        F: FnOnce(
            &JobSpec,
            &OciConfig,
            LaunchPermit,
            &Path,
            &str,
            RuntimePreparation<'_>,
        )
            -> Result<RuntimeFinalization<Result<ContainerRun, RunFailure>>, RunFailure>,
    {
        use crate::checkout_orchestration::ParentAttemptAdmission;
        let (profile, verified_rootfs, cargo_vendor) =
            self.compute_launch_preflight(spec, hooks)?;
        let container_id = format!("myelin-prod-{}-{}", std::process::id(), unique_suffix());
        match hooks
            .reserve_parent_attempt(spec)
            .map_err(|e| SandboxLaunchError::Failed(e.into()))?
        {
            ParentAttemptAdmission::Admitted {
                claim: _,
                reserve,
                attempt_authority: _,
            } => self
                .launch_compute_common_body(
                    spec,
                    hooks,
                    run,
                    profile,
                    verified_rootfs,
                    cargo_vendor,
                    reserve,
                    container_id,
                )
                .map(SandboxCycleOutcome::WorkloadLaunched),
            ParentAttemptAdmission::AttemptsExhausted { claim, reserve: _ } => {
                Ok(SandboxCycleOutcome::PreparationTerminal {
                    claim,
                    disposition: crate::runner::PreparationTerminalDisposition::AttemptsExhausted,
                    diagnostic: None,
                })
            }
        }
    }

    fn dispose_run_failure(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
        reserve: &ReserveHandle,
        run_failure: RunFailure,
    ) -> SandboxLaunchError<GvisorError> {
        let message = run_failure.to_string();
        match run_failure {
            RunFailure::Uncommitted { .. } => {
                if let Err(settle_error) = hooks.release_unused(spec, reserve) {
                    return SandboxLaunchError::Failed(GvisorError::Runtime(format!(
                        "run() failed (uncommitted: {message}) AND release_unused also failed \
                         ({settle_error}) - reservation may be leaked"
                    )));
                }
                SandboxLaunchError::Failed(GvisorError::Runtime(message))
            }
            RunFailure::CommitOutcomeUnknown { .. } => {
                SandboxLaunchError::DurableOutcomeUnknown(GvisorError::Runtime(message))
            }
            RunFailure::CommittedButNotExecuted { .. } => {
                let zero = ResourceUsage {
                    cpu_seconds: 0,
                    mem_byte_seconds: 0,
                };
                match hooks.completion_settlement_owner() {
                    CompletionSettlementOwner::TerminalReporter => {
                        SandboxLaunchError::RetryableAttempt {
                            source: GvisorError::Runtime(message),
                            cause: RetryableAttemptCause::SandboxInfrastructure,
                            usage: zero,
                        }
                    }
                    CompletionSettlementOwner::Hook => {
                        if let Err(settle_error) = hooks.settle_completed(spec, reserve, zero) {
                            return SandboxLaunchError::Failed(GvisorError::Runtime(format!(
                                "run() failed (committed but not executed: {message}) AND its \
                                 zero-usage settlement also failed ({settle_error}) - \
                                 reservation may be leaked"
                            )));
                        }
                        SandboxLaunchError::Failed(GvisorError::Runtime(message))
                    }
                }
            }
            RunFailure::Executed { usage, .. } => match hooks.completion_settlement_owner() {
                CompletionSettlementOwner::TerminalReporter => {
                    SandboxLaunchError::RetryableAttempt {
                        source: GvisorError::Runtime(message),
                        cause: RetryableAttemptCause::SandboxInfrastructure,
                        usage,
                    }
                }
                CompletionSettlementOwner::Hook => {
                    if let Err(settle_error) = hooks.settle_completed(spec, reserve, usage) {
                        return SandboxLaunchError::Failed(GvisorError::Runtime(format!(
                            "run() failed (executed: {message}) AND its conservative-usage \
                             settlement also failed ({settle_error}) - reservation may be leaked"
                        )));
                    }
                    SandboxLaunchError::Failed(GvisorError::Runtime(message))
                }
            },
        }
    }
}

impl SandboxBackend for GvisorBackend {
    type Error = GvisorError;

    fn launch(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
    ) -> Result<SandboxLaunch, SandboxLaunchError<Self::Error>> {
        if !matches!(
            crate::derive_checkout_authorization_scope(spec.kind, &spec.workspace),
            Ok(None)
        ) {
            return Err(SandboxLaunchError::Failed(GvisorError::Runtime(
                "checkout-bearing or malformed workspace specs require run_cycle".into(),
            )));
        }
        self.launch_with(
            spec,
            hooks,
            |spec, cfg, permit, rootfs, container_id, prep| {
                run_production_container(spec, cfg, permit, rootfs, container_id, prep)
            },
        )
    }

    fn launch_streaming(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
        output: Arc<dyn SandboxOutputSink>,
        cancellation: SandboxCancellation,
    ) -> Result<SandboxLaunch, SandboxLaunchError<Self::Error>> {
        if !matches!(
            crate::derive_checkout_authorization_scope(spec.kind, &spec.workspace),
            Ok(None)
        ) {
            return Err(SandboxLaunchError::Failed(GvisorError::Runtime(
                "checkout-bearing or malformed workspace specs require run_cycle".into(),
            )));
        }
        let output = cap_total_job_output(output);
        self.launch_with(
            spec,
            hooks,
            move |spec, cfg, permit, rootfs, container_id, prep| {
                run_production_container_streaming(
                    spec,
                    cfg,
                    permit,
                    rootfs,
                    container_id,
                    Some(output),
                    cancellation,
                    prep,
                )
            },
        )
    }

    fn run_cycle(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
        output: Arc<dyn SandboxOutputSink>,
        cancellation: SandboxCancellation,
    ) -> Result<SandboxCycleOutcome, SandboxLaunchError<Self::Error>> {
        let output = cap_total_job_output(output);
        match crate::derive_checkout_authorization_scope(spec.kind, &spec.workspace) {
            Ok(None) => self.launch_compute_orchestrated_with(
                spec,
                hooks,
                move |spec, cfg, permit, rootfs, container_id, prep| {
                    run_production_container_streaming(
                        spec,
                        cfg,
                        permit,
                        rootfs,
                        container_id,
                        Some(output),
                        cancellation,
                        prep,
                    )
                },
            ),
            Ok(Some(_)) => {
                let repo_root = self.checkout.repo_root().ok_or_else(|| {
                    SandboxLaunchError::Failed(GvisorError::Hook(HookError(
                        "a checkout-bearing job requires an enabled checkout repository root, but \
                         this backend's checkout config is disabled - refusing before reserve/spawn"
                            .to_string(),
                    )))
                })?;
                self.launch_checkout_orchestrated_with(
                    spec,
                    hooks,
                    repo_root,
                    &cancellation,
                    Some(output),
                )
                .map(SandboxCycleOutcome::from)
                .map_err(|error| {
                    SandboxLaunchError::Failed(match error {
                        crate::checkout_orchestration::CheckoutOrchestrationError::Hook(h) => {
                            GvisorError::Hook(h)
                        }
                        other => GvisorError::Runtime(other.to_string()),
                    })
                })
            }
            Err(reason) => Err(SandboxLaunchError::Failed(GvisorError::Hook(HookError(
                format!(
                "run_cycle refused a malformed workspace spec (neither a clean compute nor a valid \
                 checkout job): {reason}"
            ),
            )))),
        }
    }

    fn kill(&self, h: &SandboxHandle) -> Result<(), Self::Error> {
        let proc = self.live.lock().unwrap().remove(&h.guest_id);
        if let Some(mut proc) = proc {
            let r = proc.child.kill();
            if let Err(error) = std::fs::remove_dir_all(&proc.bundle_dir) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    return Err(GvisorError::Runtime(format!(
                        "bundle dir {:?} removal failed: {error}",
                        proc.bundle_dir
                    )));
                }
            }
            r.map_err(GvisorError::Runtime)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::runner::RetryableAttemptCause;

    use std::path::PathBuf;

    use crate::gvisor::test_fixtures::*;
    use crate::user_namespace::{UserNamespaceAllocator, UserNamespaceAllocatorError};
    use crate::workspace_manager::{WorkspaceManager, WorkspaceManagerError, WorkspaceStorageMode};
    use crate::{
        CompletionSettlementOwner, EgressPolicy, IdemToken, ImageRef, JobKind, JobSpec,
        MeterTarget, ReserveHandle, ResourceLimits, ResourceUsage, RunTokenCredential, RunnerHooks,
        SandboxBackend, SandboxCancellation, SandboxLaunchError, SandboxOutputSink, TrustTier,
        WorkspaceSpec,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    #[test]
    fn gvisor_checkout_config_validates_the_repo_root_at_boot() {
        assert!(matches!(
            GvisorCheckoutConfig::enabled("relative/repo"),
            Err(GvisorCheckoutConfigError::NotAbsolute(_))
        ));
        let missing = std::env::temp_dir().join(format!(
            "myelin-checkout-root-missing-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        assert!(matches!(
            GvisorCheckoutConfig::enabled(&missing),
            Err(GvisorCheckoutConfigError::NotADirectory { .. })
        ));
        let file_path = std::env::temp_dir().join(format!(
            "myelin-checkout-root-file-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        std::fs::write(&file_path, b"not a dir").unwrap();
        assert!(matches!(
            GvisorCheckoutConfig::enabled(&file_path),
            Err(GvisorCheckoutConfigError::NotADirectory { .. })
        ));
        let _ = std::fs::remove_file(&file_path);
        let base = std::env::temp_dir()
            .join(format!(
                "myelin-checkout-root-ok-{}-{}",
                std::process::id(),
                unique_suffix()
            ))
            .canonicalize()
            .unwrap_or_else(|_| {
                let p = std::env::temp_dir().join(format!(
                    "myelin-checkout-root-ok-{}-{}",
                    std::process::id(),
                    unique_suffix()
                ));
                std::fs::create_dir_all(&p).unwrap();
                std::fs::canonicalize(&p).unwrap()
            });
        std::fs::create_dir_all(&base).unwrap();
        let base = std::fs::canonicalize(&base).unwrap();
        let accepted =
            GvisorCheckoutConfig::enabled(&base).expect("a canonical directory must be accepted");
        assert_eq!(
            accepted.repo_root(),
            Some(base.as_path()),
            "an enabled config exposes exactly the validated root"
        );
        let non_canonical = base.join("..").join(base.file_name().unwrap());
        assert!(matches!(
            GvisorCheckoutConfig::enabled(&non_canonical),
            Err(GvisorCheckoutConfigError::NotCanonical { .. })
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn structured_cargo_compute_route_refuses_instead_of_skipping_vendor_boundary() {
        let fixture = cargo_boundary_fixture("compute-route");
        let backend = GvisorBackend::new(cargo_compute_registry(&fixture));
        let job = structured_cargo_spec(&fixture.reference);
        let error = backend
            .launch_with(
                &job,
                &ok_hooks(),
                |_spec, _cfg, _permit, _rootfs, _container_id, _prep| {
                    panic!(
                        "a structured compute job without Enabled workspace support must not run"
                    )
                },
            )
            .expect_err("the compute route must refuse rather than omit the vendor mounts");
        assert!(
            error
                .to_string()
                .contains("requires the Enabled workspace integration"),
            "{error}"
        );

        let mut networked_job = structured_cargo_spec(&fixture.reference);
        networked_job.egress.allow = vec!["registry.example:443".into()];
        let error = backend
            .launch_with(
                &networked_job,
                &ok_hooks(),
                |_spec, _cfg, _permit, _rootfs, _container_id, _prep| {
                    panic!("a networked structured compute job must not run")
                },
            )
            .expect_err("the compute route must apply empty-egress validation");
        assert!(
            error.to_string().contains("empty egress (network=none)"),
            "{error}"
        );
    }

    #[test]
    fn new_and_git_wire_only_construct_disabled_workspace_integration() {
        let backend = GvisorBackend::new(test_registry());
        assert!(matches!(
            backend.workspace_integration,
            WorkspaceIntegration::Disabled
        ));
        let git_wire_backend = GvisorBackend::git_wire_only();
        assert!(matches!(
            git_wire_backend.workspace_integration,
            WorkspaceIntegration::Disabled
        ));
    }

    #[test]
    fn try_new_with_disabled_config_never_touches_the_filesystem() {
        let backend = GvisorBackend::try_new(
            test_registry(),
            GvisorWorkspaceConfig::Disabled,
            Arc::new(|_: &str| {}),
        )
        .expect("Disabled construction must never fail");
        assert!(matches!(
            backend.workspace_integration,
            WorkspaceIntegration::Disabled
        ));
    }

    #[test]
    fn try_new_with_enabled_config_refuses_before_touching_workspace_when_userns_is_unsafe() {
        let base = std::env::temp_dir().join(format!(
            "myelin-gvisor-try-new-workspace-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let leases_dir = std::env::temp_dir().join(format!(
            "myelin-gvisor-try-new-leases-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let result = GvisorBackend::try_new(
            test_registry(),
            GvisorWorkspaceConfig::Enabled {
                base_dir: base.clone(),
                host_capacity_bytes: 1 << 30,
                leases_dir,
                min_pool_size: 1,
            },
            Arc::new(|_: &str| {}),
        );
        match result {
            Err(GvisorBackendInitError::UserNamespace(_)) => {}
            Err(other) => panic!("expected a UserNamespace error, got a different error: {other}"),
            Ok(_) => panic!(
                "expected a UserNamespace error - a leases dir under this test's own home/tmp \
                 directory must never be considered safe"
            ),
        }
        assert!(
            !base.exists(),
            "workspace reconciliation must never run when userns construction fails first"
        );
    }

    #[test]
    fn try_new_with_builders_never_calls_workspace_builder_when_userns_fails() {
        let workspace_builder_called = Arc::new(AtomicBool::new(false));
        let flag = workspace_builder_called.clone();
        let result = GvisorBackend::try_new_with_builders(
            test_registry(),
            GvisorWorkspaceConfig::Enabled {
                base_dir: PathBuf::from("/nonexistent-base-for-this-test"),
                host_capacity_bytes: 1 << 30,
                leases_dir: PathBuf::from("/nonexistent-leases-for-this-test"),
                min_pool_size: 1,
            },
            Arc::new(|_: &str| {}),
            |_leases_dir, _min_pool_size, _sink| {
                Err(UserNamespaceAllocatorError::NoSubordinateEntry {
                    path: PathBuf::from("/etc/subuid"),
                    uid: 0,
                })
            },
            move |_mode, _sink| {
                flag.store(true, Ordering::SeqCst);
                Err(WorkspaceManagerError::AlreadyLocked {
                    base_dir: PathBuf::new(),
                })
            },
        );
        match result {
            Err(GvisorBackendInitError::UserNamespace(_)) => {}
            Err(other) => panic!("expected UserNamespace(_), got a different error: {other}"),
            Ok(_) => panic!("expected UserNamespace(_), got Ok"),
        }
        assert!(
            !workspace_builder_called.load(Ordering::SeqCst),
            "the workspace builder must never run once the userns builder has failed"
        );
    }

    #[test]
    fn try_new_with_builders_maps_a_workspace_failure_after_userns_succeeds() {
        let base = std::env::temp_dir().join(format!(
            "myelin-gvisor-builders-workspace-fails-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 8);
        write_subordinate_file(&subgid, 200_000, 8);
        let result = GvisorBackend::try_new_with_builders(
            test_registry(),
            GvisorWorkspaceConfig::Enabled {
                base_dir: PathBuf::from("/nonexistent-base-for-this-test"),
                host_capacity_bytes: 1 << 30,
                leases_dir: leases_dir.clone(),
                min_pool_size: 1,
            },
            Arc::new(|_: &str| {}),
            |leases_dir, min_pool_size, sink| {
                crate::user_namespace::UserNamespaceAllocator::try_new_for_tests(
                    leases_dir,
                    &subuid,
                    &subgid,
                    min_pool_size,
                    sink,
                )
            },
            |mode, _sink| {
                assert!(
                    matches!(mode, WorkspaceStorageMode::EphemeralDisk { .. }),
                    "the correct mode must be forwarded to the workspace builder"
                );
                Err(WorkspaceManagerError::AlreadyLocked {
                    base_dir: PathBuf::from("/nonexistent-base-for-this-test"),
                })
            },
        );
        match result {
            Err(GvisorBackendInitError::Workspace(_)) => {}
            Err(other) => panic!("expected Workspace(_), got a different error: {other}"),
            Ok(_) => panic!("expected Workspace(_), got Ok"),
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn try_new_with_builders_produces_enabled_holding_both_managers_when_both_succeed() {
        let base = std::env::temp_dir().join(format!(
            "myelin-gvisor-builders-both-succeed-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 8);
        write_subordinate_file(&subgid, 200_000, 8);
        let backend = GvisorBackend::try_new_with_builders(
            test_registry(),
            GvisorWorkspaceConfig::Enabled {
                base_dir: PathBuf::from("/nonexistent-base-for-this-test"),
                host_capacity_bytes: 1 << 30,
                leases_dir: leases_dir.clone(),
                min_pool_size: 1,
            },
            Arc::new(|_: &str| {}),
            |leases_dir, min_pool_size, sink| {
                crate::user_namespace::UserNamespaceAllocator::try_new_for_tests(
                    leases_dir,
                    &subuid,
                    &subgid,
                    min_pool_size,
                    sink,
                )
            },
            |_mode, sink| WorkspaceManager::try_new(WorkspaceStorageMode::Disabled, sink),
        )
        .expect("both builders must succeed with a real, fixture-backed subordinate range");
        assert!(matches!(
            backend.workspace_integration,
            WorkspaceIntegration::Enabled { .. }
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn try_new_with_builders_invokes_neither_builder_when_disabled() {
        let userns_called = Arc::new(AtomicBool::new(false));
        let workspace_called = Arc::new(AtomicBool::new(false));
        let u = userns_called.clone();
        let w = workspace_called.clone();
        let backend = GvisorBackend::try_new_with_builders(
            test_registry(),
            GvisorWorkspaceConfig::Disabled,
            Arc::new(|_: &str| {}),
            move |leases_dir, min_pool_size, sink| {
                u.store(true, Ordering::SeqCst);
                UserNamespaceAllocator::try_new(leases_dir, min_pool_size, sink)
            },
            move |mode, sink| {
                w.store(true, Ordering::SeqCst);
                WorkspaceManager::try_new(mode, sink)
            },
        )
        .expect("Disabled must always succeed");
        assert!(matches!(
            backend.workspace_integration,
            WorkspaceIntegration::Disabled
        ));
        assert!(!userns_called.load(Ordering::SeqCst));
        assert!(!workspace_called.load(Ordering::SeqCst));
    }

    #[test]
    fn compute_launch_guest_root_is_a_per_job_overlay_leaving_the_base_byte_pristine() {
        use crate::asset_registry::{GvisorAssetRegistry, RootfsAssetBinding};
        use crate::canonical_tree_sha256_hex;
        use crate::rootfs_overlay::{RootfsOverlayManager, RootfsOverlayMode};

        let root = std::env::temp_dir().join(format!(
            "myelin-overlay-integration-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let base = root.join("pinned-base");
        let overlays = root.join("overlays");
        std::fs::create_dir_all(base.join("etc")).unwrap();
        std::fs::create_dir(base.join("workspace")).unwrap();
        std::fs::create_dir_all(base.join("opt/myelin/cargo-vendor")).unwrap();
        std::fs::write(base.join("etc/keep"), b"keep").unwrap();
        std::fs::write(base.join("delete-me"), b"delete").unwrap();
        let digest = canonical_tree_sha256_hex(&base).unwrap();
        let image = ImageRef::pinned(format!("test.local/overlay-int@sha256:{digest}")).unwrap();

        let registry = Arc::new(
            GvisorAssetRegistry::from_bindings(vec![RootfsAssetBinding {
                image: image.clone(),
                rootfs: base.clone(),
            }])
            .expect("the pinned base verifies"),
        );
        let manager = Arc::new(
            RootfsOverlayManager::initialize(
                RootfsOverlayMode::DeterministicDirectoryForTests {
                    overlays_dir: overlays.clone(),
                },
                Arc::new(|_message: &str| {}),
            )
            .expect("the deterministic overlay manager initializes"),
        );
        let backend = GvisorBackend::new(registry).with_rootfs_overlay_manager(manager);

        let job = JobSpec::new(
            JobKind::Agent,
            image,
            vec!["true".into()],
            vec![],
            vec![],
            EgressPolicy { allow: vec![] },
            ResourceLimits {
                cpu_millis: 1000,
                mem_bytes: 256 << 20,
                disk_bytes: 1 << 30,
                tmpfs_bytes: 1 << 30,
                pids_max: 64,
                timeout_secs: 120,
            },
            WorkspaceSpec::default(),
            TrustTier::UntrustedFork,
            RunTokenCredential::new("test-bearer", "j", 300).unwrap(),
            MeterTarget {
                reserve_id: "r".into(),
            },
            IdemToken("idem-overlay-int-1".into()),
        )
        .unwrap();

        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(|spec: &JobSpec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_spec| Ok(())),
            Box::new(|_spec| Ok(())),
        );

        let base_digest_before = canonical_tree_sha256_hex(&base).unwrap();
        let observed_root = Arc::new(Mutex::new(None::<PathBuf>));
        let seen = observed_root.clone();
        let base_for_closure = base.clone();

        let launch = backend
            .launch_with(
                &job,
                &hooks,
                move |_spec, _cfg, _permit, rootfs, _container_id, _prep| {
                    assert_ne!(
                        rootfs, base_for_closure,
                        "the launch must NOT hand runsc the shared pinned base as its guest root"
                    );
                    assert_eq!(
                        std::fs::read_to_string(rootfs.join("etc/keep")).unwrap(),
                        "keep"
                    );
                    std::fs::create_dir(rootfs.join("workspace/gofer-mount-target")).unwrap();
                    std::fs::write(rootfs.join("workspace/gofer-mount-target/x"), b"job-write")
                        .unwrap();
                    std::fs::remove_file(rootfs.join("delete-me")).unwrap();
                    *seen.lock().unwrap() = Some(rootfs.to_path_buf());
                    Ok(fake_finalization())
                },
            )
            .expect("the compute path launches");
        assert!(launch.output_complete);

        assert_eq!(
            canonical_tree_sha256_hex(&base).unwrap(),
            base_digest_before,
            "the pinned base rootfs digest must be byte-identical after a job that wrote to its root"
        );
        assert!(
            base.join("delete-me").exists(),
            "a base file the job deleted (in the overlay) must still exist in the base"
        );
        assert!(
            !base.join("workspace/gofer-mount-target").exists(),
            "a mount target the job created (in the overlay) must NOT appear in the base"
        );
        let observed = observed_root
            .lock()
            .unwrap()
            .clone()
            .expect("run observed a root");
        assert_ne!(
            observed, base,
            "the guest root was a per-job overlay, not the base"
        );
        assert!(
            observed.starts_with(&overlays),
            "the per-job overlay lives under the manager's overlay root: {observed:?}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn enabled_health_checks_refuse_before_reserve_is_ever_called() {
        let Some((userns_allocator, leases_dir)) =
            real_userns_allocator_for_tests("health-precedes-reserve")
        else {
            return;
        };
        let replacement = leases_dir.with_extension("replacement");
        std::fs::rename(&leases_dir, &replacement).unwrap();
        std::fs::create_dir_all(&leases_dir).unwrap();
        assert!(
            userns_allocator.check_identity().is_err(),
            "the replaced leases dir must make check_identity() fail"
        );

        let workspace_manager =
            WorkspaceManager::try_new(WorkspaceStorageMode::Disabled, Arc::new(|_: &str| {}))
                .unwrap();
        let backend = GvisorBackend {
            live: Mutex::new(std::collections::HashMap::new()),
            registry: Some(test_registry()),
            workspace_integration: WorkspaceIntegration::Enabled {
                workspace_manager,
                userns_allocator,
            },
            checkout: GvisorCheckoutConfig::disabled(),
            rootfs_overlay: None,
        };
        let reserve_called = Arc::new(AtomicBool::new(false));
        let reserve_called_in_hook = reserve_called.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(move |spec: &JobSpec| {
                reserve_called_in_hook.store(true, Ordering::SeqCst);
                Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))
            }),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );
        let result = backend.launch(&spec(vec![]), &hooks);
        assert!(
            result.is_err(),
            "a failed userns identity check must refuse the launch"
        );
        assert!(
            !reserve_called.load(Ordering::SeqCst),
            "hooks.reserve must never be called once an Enabled health check has failed"
        );
        let _ = std::fs::remove_dir_all(&leases_dir);
        let _ = std::fs::remove_dir_all(&replacement);
    }

    #[test]
    #[cfg(feature = "integration")]
    fn explicit_user_namespace_boots_through_the_real_enabled_backend_and_launch() {
        let _leases_dir_guard = USERNS_DRILL_LEASES_DIR_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Err(e) = preflight_explicit_userns_policy(
            resolved_explicit_userns_helper_dir(),
            resolved_explicit_userns_runsc_root(),
        ) {
            eprintln!(
                "[explicit-userns activation drill] SKIP: preflight_explicit_userns_policy failed: {e}"
            );
            return;
        }
        let rootfs = crate::resolved_gvisor_rootfs();
        if !rootfs.exists() {
            eprintln!(
                "[explicit-userns activation drill] SKIP: staged rootfs absent at {rootfs:?}"
            );
            return;
        }
        let leases_dir = match std::env::var(USERNS_DRILL_LEASES_DIR_ENV) {
            Ok(value) if !value.is_empty() => PathBuf::from(value),
            _ => {
                eprintln!(
                    "[explicit-userns activation drill] SKIP: {USERNS_DRILL_LEASES_DIR_ENV} is not \
                     set - this drill needs an operator-provisioned leases directory satisfying the \
                     STRICT production allocator contract (pre-existing, euid-owned, mode 0700 or \
                     stricter, non-writable-by-us ancestor chain); it cannot fabricate one itself"
                );
                return;
            }
        };

        let tag = format!("{}-{}", std::process::id(), unique_suffix());
        let mut workspace_base_dir = std::env::home_dir().expect("HOME must be set for this test");
        workspace_base_dir.push(format!(
            ".local/state/myelin-userns-activation-workspace-{tag}"
        ));
        let incident_sink: crate::workspace_manager::IncidentSink =
            Arc::new(|msg: &str| eprintln!("[explicit-userns activation drill incident] {msg}"));

        let backend = GvisorBackend::try_new(
            real_userns_drill_registry(&rootfs),
            GvisorWorkspaceConfig::Enabled {
                base_dir: workspace_base_dir.clone(),
                host_capacity_bytes: 1 << 30,
                leases_dir,
                min_pool_size: 1,
            },
            incident_sink,
        )
        .expect(
            "GvisorBackend::try_new(Enabled) must succeed once an operator-provisioned leases \
             directory is configured -- reaching this point asserts the host IS correctly \
             provisioned, so a construction failure here is a genuine regression",
        );

        let digest = crate::canonical_tar::canonical_tree_sha256_hex(&rootfs)
            .expect("hash the real staged rootfs");
        let mut command_spec = spec(vec![]);
        command_spec.image =
            ImageRef::pinned(format!("test.local/userns-drill@sha256:{digest}")).unwrap();
        command_spec.command = vec!["/bin/sh".into(), "-c".into(), "id".into()];

        let launch = backend.launch(&command_spec, &ok_hooks()).expect(
            "launch through the real Enabled activation path must succeed on a correctly \
                      provisioned host",
        );
        assert_eq!(
            launch.result.exit_code,
            Some(0),
            "the guest `id` command must exit 0, stderr: {}",
            String::from_utf8_lossy(&launch.result.stderr)
        );
        assert!(!launch.result.timed_out);
        let stdout = String::from_utf8_lossy(&launch.result.stdout);
        assert!(
            stdout.contains("uid=65534") && stdout.contains("gid=65534"),
            "the guest must report uid/gid 65534 (mapped via the OCI uidMappings/gidMappings this \
             slice emits) through the REAL Enabled activation path, got: {stdout:?}"
        );
        backend
            .kill(&launch.handle)
            .expect("kill must succeed to clean up the live-map entry after a completed run");

        let _ = std::fs::remove_dir_all(&workspace_base_dir);
    }

    #[test]
    fn gvisor_launch_drives_four_guarantees_on_the_same_trait() {
        let backend = GvisorBackend::new(test_registry());
        let launch = backend
            .launch_with(
                &spec(vec![]),
                &ok_hooks(),
                |_spec, _cfg, permit, _rootfs, _container_id, _prep| {
                    permit
                        .commit_and_release()
                        .map_err(|error| RunFailure::uncommitted(error.to_string()))?;
                    Ok(fake_finalization())
                },
            )
            .unwrap();
        assert_eq!(launch.handle.guest_id, "runsc-idem-runsc-1");
        assert_eq!(launch.result.exit_code, Some(0));
        assert!(launch.result.passed());
        backend.kill(&launch.handle).unwrap();
    }

    #[test]
    fn launch_with_generates_a_distinct_container_id_the_closure_receives() {
        let backend = GvisorBackend::new(test_registry());
        let seen = Arc::new(Mutex::new(Vec::new()));
        for _ in 0..2 {
            let seen = seen.clone();
            backend
                .launch_with(
                    &spec(vec![]),
                    &ok_hooks(),
                    move |_spec, _cfg, permit, _rootfs, container_id, _prep| {
                        seen.lock().unwrap().push(container_id.to_string());
                        permit
                            .commit_and_release()
                            .map_err(|error| RunFailure::uncommitted(error.to_string()))?;
                        Ok(fake_finalization())
                    },
                )
                .unwrap();
        }
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        for id in seen.iter() {
            assert!(
                id.starts_with(&format!("myelin-prod-{}-", std::process::id())),
                "unexpected container_id shape: {id:?}"
            );
        }
        assert_ne!(
            seen[0], seen[1],
            "two separate launches must never reuse the same container_id"
        );
    }

    #[test]
    fn gvisor_refuses_to_start_on_exhaustion() {
        let backend = GvisorBackend::new(test_registry());
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(|_spec| Err(crate::HookError("exhausted".into()))),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );
        let r = backend.launch_with(
            &spec(vec![]),
            &hooks,
            |_spec, _cfg, _permit, _rootfs, _container_id, _prep| Ok(fake_finalization()),
        );
        assert!(matches!(
            r,
            Err(SandboxLaunchError::Failed(GvisorError::Hook(_)))
        ));
    }

    #[test]
    fn golden_compute_trace_through_launch_with_is_byte_stable() {
        let backend = GvisorBackend::new(test_registry());
        let trace = Arc::new(Mutex::new(Vec::<String>::new()));
        let observed_container_id = Arc::new(Mutex::new(None::<String>));

        let t_iso = trace.clone();
        let t_res = trace.clone();
        let t_settle = trace.clone();
        let t_attr = trace.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(move |spec| {
                t_res.lock().unwrap().push("reserve".into());
                Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))
            }),
            Box::new(move |_spec, _h, usage| {
                t_settle.lock().unwrap().push(format!(
                    "settle:{}:{}",
                    usage.cpu_seconds, usage.mem_byte_seconds
                ));
                Ok(())
            }),
            Box::new(move |_spec| {
                t_attr.lock().unwrap().push("acquire_launch_permit".into());
                Ok(())
            }),
            Box::new(move |_spec| {
                t_iso.lock().unwrap().push("isolation_floor".into());
                Ok(())
            }),
        );

        let t_run = trace.clone();
        let seen_id = observed_container_id.clone();
        let launch = backend
            .launch_with(
                &spec(vec![]),
                &hooks,
                move |_spec, _cfg, _permit, _rootfs, container_id, _prep| {
                    t_run.lock().unwrap().push("run_spawn".into());
                    *seen_id.lock().unwrap() = Some(container_id.to_string());
                    Ok(fake_finalization())
                },
            )
            .expect("the ordinary compute path launches");

        assert_eq!(
            *trace.lock().unwrap(),
            vec![
                "isolation_floor".to_string(),
                "reserve".to_string(),
                "acquire_launch_permit".to_string(),
                "run_spawn".to_string(),
                "settle:1:1".to_string(),
            ],
            "the ordered compute sequence through launch_with -> launch_compute_with is the fence"
        );

        let observed = observed_container_id
            .lock()
            .unwrap()
            .clone()
            .expect("the run closure observed a container id");
        assert!(
            observed.starts_with(&format!("myelin-prod-{}-", std::process::id())),
            "the run closure sees the stable myelin-prod-* workload id, got {observed:?}"
        );
        assert_eq!(
            backend.live.lock().unwrap().len(),
            1,
            "a successful compute launch inserts exactly one live entry"
        );
        assert!(launch.output_complete);
    }

    #[test]
    fn golden_git_wire_only_refuses_at_registry_before_reserve() {
        let backend = GvisorBackend::git_wire_only();
        let trace = Arc::new(Mutex::new(Vec::<String>::new()));
        let t_iso = trace.clone();
        let t_res = trace.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(move |spec| {
                t_res.lock().unwrap().push("reserve".into());
                Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))
            }),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_spec| Ok(())),
            Box::new(move |_spec| {
                t_iso.lock().unwrap().push("isolation_floor".into());
                Ok(())
            }),
        );
        let ran = Arc::new(AtomicBool::new(false));
        let ran_at = ran.clone();
        let result = backend.launch_with(
            &spec(vec![]),
            &hooks,
            move |_spec, _cfg, _permit, _rootfs, _container_id, _prep| {
                ran_at.store(true, Ordering::SeqCst);
                Ok(fake_finalization())
            },
        );
        assert!(
            matches!(result, Err(SandboxLaunchError::Failed(GvisorError::Image(_)))),
            "a git_wire_only backend refuses an image-bearing job at registry resolve, got {result:?}"
        );
        assert_eq!(
            *trace.lock().unwrap(),
            vec!["isolation_floor".to_string()],
            "isolation floor runs, then registry resolve refuses BEFORE reserve is ever called"
        );
        assert!(
            !ran.load(Ordering::SeqCst),
            "the run closure never spawns on a pre-reserve refusal"
        );
    }

    #[test]
    fn golden_reserve_failure_stops_before_launch_permit_and_run() {
        let backend = GvisorBackend::new(test_registry());
        let trace = Arc::new(Mutex::new(Vec::<String>::new()));
        let t_iso = trace.clone();
        let t_res = trace.clone();
        let t_attr = trace.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(move |_spec| {
                t_res.lock().unwrap().push("reserve".into());
                Err(crate::HookError("reserve exhausted".into()))
            }),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(move |_spec| {
                t_attr.lock().unwrap().push("acquire_launch_permit".into());
                Ok(())
            }),
            Box::new(move |_spec| {
                t_iso.lock().unwrap().push("isolation_floor".into());
                Ok(())
            }),
        );
        let ran = Arc::new(AtomicBool::new(false));
        let ran_at = ran.clone();
        let result = backend.launch_with(
            &spec(vec![]),
            &hooks,
            move |_spec, _cfg, _permit, _rootfs, _container_id, _prep| {
                ran_at.store(true, Ordering::SeqCst);
                Ok(fake_finalization())
            },
        );
        assert!(
            matches!(
                result,
                Err(SandboxLaunchError::Failed(GvisorError::Hook(_)))
            ),
            "a reserve refusal surfaces as a Hook failure, got {result:?}"
        );
        assert_eq!(
            *trace.lock().unwrap(),
            vec!["isolation_floor".to_string(), "reserve".to_string()],
            "isolation floor then reserve; the reserve failure stops before the launch permit and run"
        );
        assert!(
            !ran.load(Ordering::SeqCst),
            "the run closure never spawns when reserve refuses"
        );
    }

    #[test]
    fn successful_reporter_owned_gvisor_launch_defers_settlement_to_terminal_reporter() {
        let backend = GvisorBackend::new(test_registry());
        let hook_settled = Arc::new(AtomicBool::new(false));
        let hook_settled_at = hook_settled.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::TerminalReporter,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(move |_spec, _h, _u| {
                hook_settled_at.store(true, Ordering::SeqCst);
                Ok(())
            }),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );

        backend
            .launch_with(
                &spec(vec![]),
                &hooks,
                |_spec, _cfg, permit, _rootfs, _container_id, _prep| {
                    permit
                        .commit_and_release()
                        .map_err(|error| RunFailure::uncommitted(error.to_string()))?;
                    Ok(fake_finalization())
                },
            )
            .expect("the sandbox returns measured usage for the reporter transaction");
        assert!(
            !hook_settled.load(Ordering::SeqCst),
            "reporter-owned completion must not settle through the hook"
        );
    }

    #[test]
    fn settlement_failure_unconditionally_kills_and_forgets_the_container() {
        let backend = GvisorBackend::new(test_registry());
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(|_spec, _handle, _usage| {
                Err(crate::HookError("injected settlement failure".into()))
            }),
            Box::new(|_spec| Ok(())),
            Box::new(|_spec| Ok(())),
        );

        let result = backend.launch_with(
            &spec(vec![]),
            &hooks,
            |_spec, _cfg, permit, _rootfs, _container_id, _prep| {
                permit
                    .commit_and_release()
                    .map_err(|error| RunFailure::uncommitted(error.to_string()))?;
                Ok(fake_finalization())
            },
        );

        assert!(matches!(
            result,
            Err(SandboxLaunchError::Failed(GvisorError::Hook(_)))
        ));
        assert!(
            backend.live.lock().unwrap().is_empty(),
            "an error without a returned handle cannot retain an unreachable live-map entry"
        );
    }

    #[test]
    fn gvisor_releases_the_unused_reserve_when_final_attribution_refuses() {
        let backend = GvisorBackend::new(test_registry());
        let settled = Arc::new(Mutex::new(None));
        let settled_at = settled.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::TerminalReporter,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(move |_spec, _h, usage| {
                *settled_at.lock().unwrap() = Some(usage);
                Ok(())
            }),
            Box::new(|_t| Err(crate::HookError("claim canceled".into()))),
            Box::new(|_s| Ok(())),
        );
        let spawned = Arc::new(AtomicBool::new(false));
        let spawned_at = spawned.clone();
        let result = backend.launch_with(
            &spec(vec![]),
            &hooks,
            move |_spec, _cfg, _permit, _rootfs, _container_id, _prep| {
                spawned_at.store(true, Ordering::SeqCst);
                Ok(fake_finalization())
            },
        );
        assert!(matches!(
            result,
            Err(SandboxLaunchError::Failed(GvisorError::Hook(_)))
        ));
        assert!(!spawned.load(Ordering::SeqCst));
        assert_eq!(
            *settled.lock().unwrap(),
            Some(ResourceUsage {
                cpu_seconds: 0,
                mem_byte_seconds: 0,
            })
        );
    }

    #[test]
    fn launch_permit_refusal_compounds_with_a_failing_reservation_release() {
        let backend = GvisorBackend::new(test_registry());
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(|_spec, _h, _usage| {
                Err(crate::HookError("settle backend unavailable".into()))
            }),
            Box::new(|_t| Err(crate::HookError("claim canceled".into()))),
            Box::new(|_s| Ok(())),
        );
        let result = backend.launch_with(
            &spec(vec![]),
            &hooks,
            |_spec, _cfg, _permit, _rootfs, _container_id, _prep| Ok(fake_finalization()),
        );
        match result {
            Err(SandboxLaunchError::Failed(GvisorError::Runtime(message))) => {
                assert!(
                    message.contains("claim canceled"),
                    "the original attribution refusal must survive: {message}"
                );
                assert!(
                    message.contains("releasing the unused reservation also failed"),
                    "the release failure must be compounded in, not lost: {message}"
                );
                assert!(
                    message.contains("settle backend unavailable"),
                    "the release failure's own text must be present verbatim: {message}"
                );
            }
            other => panic!("expected a compound GvisorError::Runtime, got {other:?}"),
        }
    }

    #[test]
    fn gvisor_run_failure_uncommitted_releases_reserve_via_release_unused() {
        let backend = GvisorBackend::new(test_registry());
        let settled = Arc::new(Mutex::new(None));
        let settled_at = settled.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::TerminalReporter,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(move |_spec, _h, usage| {
                *settled_at.lock().unwrap() = Some(usage);
                Ok(())
            }),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );
        let result = backend.launch_with(
            &spec(vec![]),
            &hooks,
            |_spec, _cfg, _permit, _rootfs, _container_id, _prep| {
                Err(RunFailure::uncommitted("injected uncommitted run failure"))
            },
        );
        assert!(
            matches!(
                result,
                Err(SandboxLaunchError::Failed(GvisorError::Runtime(_)))
            ),
            "an uncommitted run failure must surface as Failed(GvisorError::Runtime): {result:?}"
        );
        assert_eq!(
            *settled.lock().unwrap(),
            Some(ResourceUsage {
                cpu_seconds: 0,
                mem_byte_seconds: 0,
            }),
            "release_unused must settle at zero even under reporter-owned completion - it is \
             owner-independent, unlike settle_completed"
        );
    }

    #[test]
    fn gvisor_run_failure_commit_outcome_unknown_never_releases_or_settles() {
        let backend = GvisorBackend::new(test_registry());
        let settled = Arc::new(AtomicBool::new(false));
        let settled_at = settled.clone();
        let released = Arc::new(AtomicBool::new(false));
        let released_at = released.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(move |_spec, _h, usage| {
                settled_at.store(true, Ordering::SeqCst);
                if usage
                    == (ResourceUsage {
                        cpu_seconds: 0,
                        mem_byte_seconds: 0,
                    })
                {
                    released_at.store(true, Ordering::SeqCst);
                }
                Ok(())
            }),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );
        let result = backend.launch_with(
            &spec(vec![]),
            &hooks,
            |_spec, _cfg, _permit, _rootfs, _container_id, _prep| {
                Err(RunFailure::commit_outcome_unknown(
                    "injected commit-outcome-unknown run failure",
                ))
            },
        );
        assert!(
            matches!(
                result,
                Err(SandboxLaunchError::DurableOutcomeUnknown(GvisorError::Runtime(_)))
            ),
            "a commit-outcome-unknown run failure must surface as DurableOutcomeUnknown: {result:?}"
        );
        assert!(
            !settled.load(Ordering::SeqCst) && !released.load(Ordering::SeqCst),
            "neither settle_completed nor release_unused (which also calls the settle hook) may \
             ever fire for an outcome-unknown attempt"
        );
    }

    #[test]
    fn gvisor_run_failure_committed_but_not_executed_hook_owner_settles_zero_then_fails() {
        let backend = GvisorBackend::new(test_registry());
        let settled = Arc::new(Mutex::new(None));
        let settled_at = settled.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(move |_spec, _h, usage| {
                *settled_at.lock().unwrap() = Some(usage);
                Ok(())
            }),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );
        let result = backend.launch_with(
            &spec(vec![]),
            &hooks,
            |_spec, _cfg, _permit, _rootfs, _container_id, _prep| {
                Err(RunFailure::committed_but_not_executed(
                    "injected committed-but-not-executed run failure",
                ))
            },
        );
        assert!(
            matches!(
                result,
                Err(SandboxLaunchError::Failed(GvisorError::Runtime(_)))
            ),
            "a Hook-owned committed-but-not-executed failure must surface as Failed: {result:?}"
        );
        assert_eq!(
            *settled.lock().unwrap(),
            Some(ResourceUsage {
                cpu_seconds: 0,
                mem_byte_seconds: 0,
            }),
            "Hook ownership must settle zero usage synchronously through settle_completed"
        );
    }

    #[test]
    fn gvisor_run_failure_committed_but_not_executed_reporter_owner_yields_retryable_attempt() {
        let backend = GvisorBackend::new(test_registry());
        let settled = Arc::new(AtomicBool::new(false));
        let settled_at = settled.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::TerminalReporter,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(move |_spec, _h, _usage| {
                settled_at.store(true, Ordering::SeqCst);
                Ok(())
            }),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );
        let result = backend.launch_with(
            &spec(vec![]),
            &hooks,
            |_spec, _cfg, _permit, _rootfs, _container_id, _prep| {
                Err(RunFailure::committed_but_not_executed(
                    "injected committed-but-not-executed run failure",
                ))
            },
        );
        match result {
            Err(SandboxLaunchError::RetryableAttempt { cause, usage, .. }) => {
                assert_eq!(cause, RetryableAttemptCause::SandboxInfrastructure);
                assert_eq!(
                    usage,
                    ResourceUsage {
                        cpu_seconds: 0,
                        mem_byte_seconds: 0,
                    }
                );
            }
            other => panic!("expected RetryableAttempt with zero usage, got {other:?}"),
        }
        assert!(
            !settled.load(Ordering::SeqCst),
            "settle_completed must never be called directly here - the runner's retryable-attempt \
             transaction is the sole accounting path under reporter ownership"
        );
    }

    #[test]
    fn gvisor_run_failure_executed_hook_owner_settles_fallback_usage_then_fails() {
        let backend = GvisorBackend::new(test_registry());
        let settled = Arc::new(Mutex::new(None));
        let settled_at = settled.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(move |_spec, _h, usage| {
                *settled_at.lock().unwrap() = Some(usage);
                Ok(())
            }),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );
        let fallback_usage = ResourceUsage {
            cpu_seconds: 7,
            mem_byte_seconds: 700,
        };
        let result = backend.launch_with(
            &spec(vec![]),
            &hooks,
            move |_spec, _cfg, _permit, _rootfs, _container_id, _prep| {
                Err(RunFailure::executed(
                    "injected executed-phase run failure",
                    fallback_usage,
                ))
            },
        );
        assert!(
            matches!(
                result,
                Err(SandboxLaunchError::Failed(GvisorError::Runtime(_)))
            ),
            "a Hook-owned executed-phase failure must surface as Failed: {result:?}"
        );
        assert_eq!(
            *settled.lock().unwrap(),
            Some(fallback_usage),
            "the executed phase must settle its carried conservative fallback usage, never zero"
        );
    }

    #[test]
    fn gvisor_run_failure_executed_reporter_owner_yields_retryable_attempt_with_fallback_usage() {
        let backend = GvisorBackend::new(test_registry());
        let settled = Arc::new(AtomicBool::new(false));
        let settled_at = settled.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::TerminalReporter,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(move |_spec, _h, _usage| {
                settled_at.store(true, Ordering::SeqCst);
                Ok(())
            }),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );
        let fallback_usage = ResourceUsage {
            cpu_seconds: 3,
            mem_byte_seconds: 300,
        };
        let result = backend.launch_with(
            &spec(vec![]),
            &hooks,
            move |_spec, _cfg, _permit, _rootfs, _container_id, _prep| {
                Err(RunFailure::executed(
                    "injected executed-phase run failure",
                    fallback_usage,
                ))
            },
        );
        match result {
            Err(SandboxLaunchError::RetryableAttempt { cause, usage, .. }) => {
                assert_eq!(cause, RetryableAttemptCause::SandboxInfrastructure);
                assert_eq!(usage, fallback_usage);
            }
            other => panic!("expected RetryableAttempt with the fallback usage, got {other:?}"),
        }
        assert!(
            !settled.load(Ordering::SeqCst),
            "settle_completed must never be called directly here - the runner's retryable-attempt \
             transaction is the sole accounting path under reporter ownership"
        );
    }

    #[test]
    fn red_isolation_floor_refuses_before_registry_lookup_reserve_or_spawn() {
        let floor_called = Arc::new(AtomicBool::new(false));
        let floor_called_at = floor_called.clone();
        let reserve_called = Arc::new(AtomicBool::new(false));
        let reserve_called_at = reserve_called.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(move |spec| {
                reserve_called_at.store(true, Ordering::SeqCst);
                Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))
            }),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_t| Ok(())),
            Box::new(move |_spec| {
                floor_called_at.store(true, Ordering::SeqCst);
                Err(crate::HookError(
                    "isolation floor is RED for this test".into(),
                ))
            }),
        );

        let mut unregistered_spec = spec(vec![]);
        unregistered_spec.image = ImageRef::pinned(
            "test.local/genuinely-unregistered@sha256:3333333333333333333333333333333333333333333333333333333333333333",
        )
        .unwrap();
        let spawned = Arc::new(AtomicBool::new(false));
        let spawned_at = spawned.clone();
        let backend = GvisorBackend::new(Arc::new(
            crate::asset_registry::GvisorAssetRegistry::from_bindings(vec![]).unwrap(),
        ));
        let result = backend.launch_with(
            &unregistered_spec,
            &hooks,
            move |_spec, _cfg, _permit, _rootfs, _container_id, _prep| {
                spawned_at.store(true, Ordering::SeqCst);
                Ok(fake_finalization())
            },
        );

        assert!(
            matches!(result, Err(SandboxLaunchError::Failed(GvisorError::Hook(_)))),
            "the isolation floor's own refusal must surface, proving it ran BEFORE the registry \
             lookup (an unregistered image would otherwise short-circuit as `Image` first): {result:?}"
        );
        assert!(
            floor_called.load(Ordering::SeqCst),
            "the isolation floor must be consulted even for an unresolvable image"
        );
        assert!(
            !reserve_called.load(Ordering::SeqCst),
            "no reserve may be attempted"
        );
        assert!(
            !spawned.load(Ordering::SeqCst),
            "the run closure must never be invoked"
        );
    }

    #[test]
    fn unknown_image_after_green_floor_refuses_before_reserve_or_spawn() {
        let floor_called = Arc::new(AtomicBool::new(false));
        let floor_called_at = floor_called.clone();
        let reserve_called = Arc::new(AtomicBool::new(false));
        let reserve_called_at = reserve_called.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(move |spec| {
                reserve_called_at.store(true, Ordering::SeqCst);
                Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))
            }),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_t| Ok(())),
            Box::new(move |_spec| {
                floor_called_at.store(true, Ordering::SeqCst);
                Ok(())
            }),
        );

        let mut unregistered_spec = spec(vec![]);
        unregistered_spec.image = ImageRef::pinned(
            "test.local/genuinely-unregistered@sha256:3333333333333333333333333333333333333333333333333333333333333333",
        )
        .unwrap();
        let spawned = Arc::new(AtomicBool::new(false));
        let spawned_at = spawned.clone();
        let backend = GvisorBackend::new(Arc::new(
            crate::asset_registry::GvisorAssetRegistry::from_bindings(vec![]).unwrap(),
        ));
        let result = backend.launch_with(
            &unregistered_spec,
            &hooks,
            move |_spec, _cfg, _permit, _rootfs, _container_id, _prep| {
                spawned_at.store(true, Ordering::SeqCst);
                Ok(fake_finalization())
            },
        );

        assert!(matches!(
            result,
            Err(SandboxLaunchError::Failed(GvisorError::Image(_)))
        ));
        assert!(
            floor_called.load(Ordering::SeqCst),
            "the isolation floor must have been consulted (and passed) first"
        );
        assert!(
            !reserve_called.load(Ordering::SeqCst),
            "no reserve may be attempted"
        );
        assert!(
            !spawned.load(Ordering::SeqCst),
            "the run closure must never be invoked"
        );
    }

    #[test]
    fn git_wire_only_backend_refuses_ordinary_launch() {
        let backend = GvisorBackend::git_wire_only();
        let hooks = ok_hooks();
        let result = backend.launch(&spec(vec![]), &hooks);
        assert!(
            matches!(
                result,
                Err(SandboxLaunchError::Failed(GvisorError::Image(_)))
            ),
            "a git-wire-only backend has no asset registry and must refuse an ordinary launch as \
             GvisorError::Image, not panic or hang: {result:?}"
        );
    }

    #[test]
    fn git_wire_only_backend_refuses_ordinary_launch_streaming() {
        let backend = GvisorBackend::git_wire_only();
        let hooks = ok_hooks();
        let output: Arc<dyn SandboxOutputSink> = Arc::new(RecordingOutput::default());
        let result =
            backend.launch_streaming(&spec(vec![]), &hooks, output, SandboxCancellation::new());
        assert!(
            matches!(result, Err(SandboxLaunchError::Failed(GvisorError::Image(_)))),
            "a git-wire-only backend must refuse ordinary launch_streaming the same way as launch: \
             {result:?}"
        );
    }
}
