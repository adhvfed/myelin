//! The [`GvisorBackend`] itself — its construction/configuration, the live-container bookkeeping,
//! the compute launch path, and the [`SandboxBackend`] trait impl.

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

/// A gVisor backend error.
#[derive(Debug)]
pub enum GvisorError {
    /// A four-guarantee hook failed.
    Hook(crate::HookError),
    /// The mandatory hardening profile could not be asserted in force (fail-closed).
    Hardening(String),
    /// The `runsc` runtime errored.
    Runtime(String),
    /// `spec.image` could not be resolved against the [`GvisorAssetRegistry`]'s already-verified
    /// entries BEFORE any resource was reserved — an unregistered reference (registry construction
    /// itself refuses an unsupported digest algorithm, an invalid rootfs path, or a canonical-tree
    /// digest mismatch, so none of those can surface here). Refused in `launch_with` AFTER
    /// `enforce_isolation_floor`/the hardening assert but BEFORE `reserve`/anything else.
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

/// A live gVisor container handle (the OCI/`runsc` container id, killable on teardown). Its
/// lifecycle is RECONCILED with the Firecracker [`VmmChild`](crate::firecracker::VmmChild): both
/// expose `kill` (whole-guest teardown) AND `wait` (block until the command exits). For gVisor the
/// REAL exit is captured directly from the `runsc` child's exit status during the run (no separate
/// wait), so [`wait`](RunscChild::wait) is retained only for lifecycle-shape parity with `VmmChild`.
pub trait RunscChild {
    fn kill(&mut self) -> Result<(), String>;
    /// Wait for the container's process to exit; returns its exit code (0 == clean). Reconciles with
    /// `VmmChild::wait` so both backends share the same launch→run→wait→result lifecycle shape.
    fn wait(&mut self) -> Result<i32, String>;
}

/// The gVisor (`runsc`) second backend — same trait, same hardening, OCI/`runsc` path.
///
/// **No `Default`, deliberately.** An ordinary (non-git-wire) launch resolves `spec.image` through a
/// [`GvisorAssetRegistry`](crate::asset_registry::GvisorAssetRegistry) — there is no
/// registry-less production backend a caller could construct by accident. [`GvisorBackend::new`]
/// requires one explicitly; [`GvisorBackend::git_wire_only`] is the SEPARATE, loudly-named
/// constructor for the git-wire receive/upload-pack path (which resolves its OWN rootfs via
/// [`resolved_gvisor_git_rootfs`] and never consults the registry) and REFUSES ordinary
/// `launch`/`launch_streaming`.
pub struct GvisorBackend {
    /// guest_id → the live container's teardown state (its `runsc` child + bundle temp dir). Ephemeral;
    /// one job per container, never reused.
    pub(super) live: Mutex<std::collections::HashMap<String, RunscProc>>,
    /// The image→rootfs authority an ordinary launch resolves `spec.image` through. `None` only for
    /// a [`GvisorBackend::git_wire_only`] backend, which refuses ordinary launch outright (so this
    /// is never consulted from that path either).
    pub(super) registry: Option<Arc<crate::asset_registry::GvisorAssetRegistry>>,
    /// CT-007 slice 3, piece 4: intentionally stored but not yet consulted anywhere — `launch`/
    /// `launch_with` don't read this yet (that's the later `OciExecutionLayout`/launch-redesign
    /// piece). `new`/`git_wire_only` always construct `Disabled`; only the not-yet-public
    /// [`GvisorBackend::try_new`] can construct `Enabled`.
    pub(super) workspace_integration: WorkspaceIntegration,
    /// The boot-validated checkout repository root selection. Constructors initialize it disabled;
    /// the activated runner composition replaces it with its boot-validated enabled config.
    pub(super) checkout: GvisorCheckoutConfig,
    /// CT-007 #26/#27: the per-job rootfs copy-on-write overlay manager. When `Some`, EVERY launch's
    /// guest root is a fresh per-job OverlayFS merged view whose read-only lower is the once-verified
    /// base inode — so all host-side mount-target creation and gofer writes land in the per-job upper
    /// and the shared, digest-pinned base tree is NEVER mutated across jobs (the #26 base-immutability
    /// property; the #27 verify-to-use fd-binding + identity re-check lives inside the manager). `None`
    /// (every existing constructor's default, and the git-wire path) uses the verified base directly —
    /// the exact pre-integration behavior, so all existing callers/tests are byte-unchanged. Opt in
    /// via [`Self::with_rootfs_overlay_manager`]; production enables it in the runner composition.
    pub(super) rootfs_overlay: Option<Arc<crate::rootfs_overlay::RootfsOverlayManager>>,
}

/// Caller-facing configuration for [`GvisorBackend::try_new`] — mirrors [`WorkspaceIntegration`]'s
/// shape but as INPUT (paths/params), not already-constructed managers. CT-007 slice 3, piece 7c:
/// `launch_with` now fully consumes `Enabled` (health checks, acquisition, durable bind, checked
/// finalization, evidence-validated release) — this is `pub`, not `pub(crate)`, from this piece
/// onward.
#[derive(Debug)]
pub enum GvisorWorkspaceConfig {
    Disabled,
    Enabled {
        /// Forwarded into `WorkspaceStorageMode::EphemeralDisk { base_dir, host_capacity_bytes }`.
        base_dir: PathBuf,
        host_capacity_bytes: u64,
        /// Forwarded into `UserNamespaceAllocator::try_new`.
        leases_dir: PathBuf,
        min_pool_size: u32,
    },
}

/// **The checkout repository-root selection for [`GvisorBackend`] (CT-007 slice 5b.3-6e.1 —
/// DORMANT).**
///
/// Hop A of a checkout preparation fetches from a BARE repository on the host. Production needs an
/// EXPLICIT, boot-validated absolute path for that root — never a relative, defaulted, or
/// symlink-redirectable one an attacker-influenced working directory could point elsewhere.
///
/// **The invalid state is UNCONSTRUCTABLE (Sol's 6e.1 blocker 1).** The payload is OPAQUE: an enabled
/// selection wraps a PRIVATE [`CheckoutConfigState`], so external code cannot fabricate an
/// `Enabled { repo_root }` with an unvalidated path. The ONLY way to obtain an enabled config is
/// [`GvisorCheckoutConfig::enabled`], which validates at boot (absolute, existing, directory,
/// canonical). [`GvisorBackend::with_checkout_config`] therefore takes an ALREADY-validated value and
/// never sees an unchecked path. Every existing [`GvisorBackend`] constructor uses
/// [`GvisorCheckoutConfig::disabled`]; the activating slice (5b.3-6e.2) is the one that selects an
/// enabled config and routes a checkout-bearing spec through
/// [`GvisorBackend::launch_checkout_orchestrated_with`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GvisorCheckoutConfig(CheckoutConfigState);

/// The PRIVATE state a [`GvisorCheckoutConfig`] wraps. Private so `Enabled` is unconstructable outside
/// this module — the boot-validating [`GvisorCheckoutConfig::enabled`] is the sole path to it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum CheckoutConfigState {
    /// No checkout root selected — the backend does not enter the checkout path.
    Disabled,
    /// A boot-validated bare-repository root for Hop A. Reachable ONLY through the validating
    /// constructor, so `repo_root` is always absolute, existing, a directory, and canonical.
    Enabled {
        /// The absolute, existing, canonical bare-repository root.
        repo_root: PathBuf,
    },
}

/// Why [`GvisorCheckoutConfig::enabled`] refused a proposed checkout repository root at boot.
#[derive(Debug, PartialEq, Eq)]
pub enum GvisorCheckoutConfigError {
    /// The configured path was relative. A checkout root must be absolute so no working-directory
    /// context can reinterpret it.
    NotAbsolute(PathBuf),
    /// The configured path does not resolve to an existing directory (missing, or a non-directory).
    NotADirectory {
        /// The configured path.
        path: PathBuf,
        /// The underlying reason.
        detail: String,
    },
    /// The configured path is not already canonical — it differs from its own canonicalization (a
    /// symlinked or `..`-bearing root). Refused so the durable root is byte-for-byte the audited path.
    NotCanonical {
        /// The configured path.
        configured: PathBuf,
        /// What it canonicalizes to.
        canonical: PathBuf,
    },
}

impl std::fmt::Display for GvisorCheckoutConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GvisorCheckoutConfigError::NotAbsolute(path) => write!(
                f,
                "checkout repository root {path:?} is not absolute — a checkout root must be an \
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
                 {canonical:?}) — refusing a symlinked or `..`-bearing root so the durable root is \
                 exactly the audited path"
            ),
        }
    }
}

impl std::error::Error for GvisorCheckoutConfigError {}

impl GvisorCheckoutConfig {
    /// The checkout-disabled selection for non-runner and legacy backends.
    pub fn disabled() -> Self {
        GvisorCheckoutConfig(CheckoutConfigState::Disabled)
    }

    /// **Validate and select a checkout repository root AT BOOT.** The ONLY constructor of an enabled
    /// config (the wrapped state is private), so an enabled `GvisorCheckoutConfig` can never carry an
    /// unvalidated path. Refuses a relative, nonexistent, non-directory, or non-canonical path. There
    /// is deliberately NO default fallback: an unconfigured or malformed root fails closed here rather
    /// than defaulting to some ambient path.
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

    /// The boot-validated checkout root, if this config is enabled. `pub(crate)` — the sandbox's own
    /// (dormant) checkout selection reads it; external code never sees the raw path. Dormant in 6e.1:
    /// 6e.2 is the first reader.
    #[allow(dead_code)]
    pub(crate) fn repo_root(&self) -> Option<&Path> {
        match &self.0 {
            CheckoutConfigState::Disabled => None,
            CheckoutConfigState::Enabled { repo_root } => Some(repo_root),
        }
    }
}

/// Why [`GvisorBackend::try_new`] failed to construct an `Enabled` [`WorkspaceIntegration`].
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

/// A live container's teardown state: the (already-exited/killed) `runsc` child + the bundle temp dir
/// to remove on teardown. Mirrors the Firecracker `GuestProc` (one job per sandbox).
pub(super) struct RunscProc {
    pub(super) child: Box<dyn RunscChild + Send>,
    pub(super) bundle_dir: PathBuf,
}

/// What a launch's run-closure hands back to [`GvisorBackend::launch_with`]: the spawned `runsc` child
/// (already exited/killed by the time this is returned; carried for idempotent teardown), the bundle
/// temp dir (removed on teardown), and the **already-captured** [`SandboxResult`]. Mirrors the
/// Firecracker `GuestRun` — the CT-001 seam now carries a REAL result, no longer a stub. The real
/// production closure ([`run_production_container`]) runs `runsc run --bundle` and fills this from the
/// container's REAL runtime result; unit tests inject a fake child + a canned result so the
/// four-guarantee control flow is testable without a runtime (the injectable-spawn seam — preserved).
pub struct ContainerRun {
    /// The spawned (and, by the time this is returned, already-exited/killed) `runsc` child.
    pub child: Box<dyn RunscChild + Send>,
    /// The bundle temp dir to remove on teardown.
    pub bundle_dir: PathBuf,
    /// The captured command result (exit / timeout / usage / bounded streams).
    pub result: SandboxResult,
    /// A post-spawn transport/cancellation failure. Usage is still settled before launch refuses.
    pub run_error: Option<String>,
}

/// The guest root materialized for ONE launch (CT-007 #26/#27). `path` is what the launch stages as
/// `root.path` and precreates mount targets under. When `overlay` is `Some`, that path is a per-job
/// OverlayFS merged view and the held [`RootfsOverlay`](crate::rootfs_overlay::RootfsOverlay) guard
/// keeps the mount + its fd-bound lower alive for the whole launch and tears the overlay down on
/// drop (which the launch schedules AFTER `run(...)` returns — i.e. after runsc has exited and the
/// OCI bundle was cleaned). When `overlay` is `None`, `path` is the verified base itself and there is
/// nothing to tear down: the exact pre-integration behavior.
pub(super) struct JobGuestRoot {
    path: PathBuf,
    // A pure RAII teardown guard: never read, held only so the per-job OverlayFS mount + its fd-bound
    // lower stay alive for the whole launch and are unmounted/removed on drop (scheduled AFTER
    // `run(...)` returns). `None` when no overlay manager is installed.
    #[allow(dead_code)]
    overlay: Option<crate::rootfs_overlay::RootfsOverlay>,
}

impl JobGuestRoot {
    /// The path to stage as `root.path` / precreate mount targets under for this launch.
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl GvisorBackend {
    /// A new backend with no live containers, resolving every ordinary (non-git-wire) launch's
    /// `spec.image` through `registry` — the real launch authority (CT-007 gate 2/4). There is no
    /// argument-less constructor: a registry MUST be supplied explicitly (see
    /// [`GvisorBackend::git_wire_only`] for the one legitimate case that needs none).
    pub fn new(registry: Arc<crate::asset_registry::GvisorAssetRegistry>) -> GvisorBackend {
        GvisorBackend {
            live: Mutex::new(std::collections::HashMap::new()),
            registry: Some(registry),
            workspace_integration: WorkspaceIntegration::Disabled,
            checkout: GvisorCheckoutConfig::disabled(),
            rootfs_overlay: None,
        }
    }

    /// A backend for the git-wire receive/upload-pack path ONLY
    /// ([`launch_git_wire`](Self::launch_git_wire) / [`launch_git_receive_pack`](Self::launch_git_receive_pack)
    /// and their `_until_cancelled` variants) — that path resolves its OWN rootfs via
    /// [`resolved_gvisor_git_rootfs`], a separate, pre-existing, deliberately different mechanism
    /// from ordinary job launch, and never consults an image registry. A backend built this way has
    /// NO registry at all, so an ordinary [`SandboxBackend::launch`]/[`SandboxBackend::launch_streaming`]
    /// call REFUSES with [`GvisorError::Image`] — a git-wire-only instance can never accidentally be
    /// used to launch an ordinary, image-bearing job.
    pub fn git_wire_only() -> GvisorBackend {
        GvisorBackend {
            live: Mutex::new(std::collections::HashMap::new()),
            registry: None,
            workspace_integration: WorkspaceIntegration::Disabled,
            checkout: GvisorCheckoutConfig::disabled(),
            rootfs_overlay: None,
        }
    }

    /// CT-007 slice 3: the ONLY way to construct a backend with `Enabled` workspace/user-namespace
    /// integration. Piece 7c: `launch_with` fully consumes this integration now, so this
    /// constructor is `pub`. Delegates to [`Self::try_new_with_builders`] with the REAL
    /// constructors; see that method for the fixed construction-order contract.
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

    /// The actual construction logic, generic over HOW the allocator/manager are built — a test
    /// seam (Sol's round-1 review of piece 4): the real [`UserNamespaceAllocator::try_new`] is
    /// fixed to `/etc/subuid`/`/etc/subgid` with full hardening always enforced, which no ordinary
    /// dev/CI sandbox host can satisfy (ANY leases_dir such a host can create itself sits under a
    /// directory it owns or can write to, which the allocator's own ancestor check always refuses —
    /// the SAME constraint this crate's explicit-userns drill already documents). That made the
    /// public `try_new`'s own success path — and therefore the `Workspace(_)` error-mapping branch,
    /// reachable only once userns has ALREADY succeeded — untestable without either weakening
    /// production strictness or fabricating a fake non-real value of these real manager types. This
    /// method fixes that: injectable builders still must return the REAL `UserNamespaceAllocator`/
    /// `WorkspaceManager` types (tests satisfy them via the existing test-relaxed constructors —
    /// `UserNamespaceAllocator::try_new_for_tests`, `WorkspaceManager::try_new` with
    /// `WorkspaceStorageMode::Disabled` — never a fabricated stand-in), so this seam changes nothing
    /// about what a genuine `Enabled` value actually contains.
    ///
    /// Fixed construction order: `build_userns` (the mandatory identity authority) FIRST, THEN
    /// `build_workspace` — `WorkspaceManager::try_new` is not side-effect-free (its boot
    /// reconciliation deletes orphaned subvolumes), so a misconfigured `/etc/subuid`/`/etc/subgid`
    /// must refuse before that ever runs. If workspace construction then fails, the
    /// already-constructed allocator is simply dropped (safely releasing its lock — no lease was
    /// ever minted from construction alone).
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

    /// Select a validated checkout repository root. The Stage-B runner composition calls this with
    /// [`GvisorCheckoutConfig::Enabled`], routing checkout-bearing specs through
    /// [`Self::launch_checkout_orchestrated_with`].
    #[allow(dead_code)]
    pub fn with_checkout_config(mut self, checkout: GvisorCheckoutConfig) -> GvisorBackend {
        self.checkout = checkout;
        self
    }

    /// CT-007 #26/#27: install the per-job rootfs CoW overlay manager (the runner composition builds
    /// and initializes it ONCE at startup — [`RootfsOverlayManager::initialize`] enters the runner's
    /// private mount namespace under [`RootfsOverlayMode::OverlayFs`] before any worker thread is
    /// spawned). Once installed, EVERY compute/checkout launch derives its guest root as a fresh
    /// per-job overlay ([`Self::materialize_job_guest_root`]) instead of mounting the shared base tree
    /// directly, so the digest-pinned base never drifts across jobs.
    #[allow(dead_code)]
    pub fn with_rootfs_overlay_manager(
        mut self,
        manager: Arc<crate::rootfs_overlay::RootfsOverlayManager>,
    ) -> GvisorBackend {
        self.rootfs_overlay = Some(manager);
        self
    }

    /// Derive the guest root for ONE job. With a rootfs overlay manager installed (#26/#27) this
    /// mints a fresh per-job OverlayFS view whose read-only lower is the once-verified base inode
    /// (the returned [`JobGuestRoot`] owns the teardown guard for the whole launch); without one it
    /// returns the verified base path directly, byte-for-byte the pre-integration behavior. Either
    /// way [`JobGuestRoot::path`] is what the caller stages as `root.path` and precreates mount
    /// targets under — so when the overlay is active every such write lands in the per-job upper, not
    /// the shared base.
    ///
    /// `job_key` must be a safe single path component; the caller passes the freshly minted container
    /// id, which already satisfies that.
    pub(super) fn materialize_job_guest_root(
        &self,
        verified_rootfs: &crate::asset_registry::VerifiedRootfs,
        job_key: &str,
    ) -> Result<JobGuestRoot, String> {
        match &self.rootfs_overlay {
            None => Ok(JobGuestRoot {
                path: verified_rootfs.path().to_path_buf(),
                overlay: None,
            }),
            Some(manager) => {
                // The host-visible merged root is owned by THIS runner process (the euid runsc/the
                // gofer run as), so mount-target precreation and gofer opens can traverse it; chown to
                // self needs no CAP_CHOWN. `root.readonly=true` still makes the guest see `/` as
                // read-only — the writable upper only absorbs HOST-side layout, it never grants the
                // untrusted workload a writable guest root.
                let workload_root = crate::rootfs_overlay::WorkloadRootPermissions::new(
                    // SAFETY: `geteuid`/`getegid` are always-successful, side-effect-free syscalls.
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
                    overlay: Some(overlay),
                })
            }
        }
    }

    /// Build the OCI config a launch WOULD use for `spec` (the hardened profile derived + the OCI
    /// JSON assembled), without running. Asserts the mandatory profile is in force.
    pub fn oci_config(spec: &JobSpec) -> Result<OciConfig, GvisorError> {
        spec.validate_secret_coverage()
            .map_err(|error| GvisorError::Runtime(format!("secret injection refused: {error}")))?;
        let profile = HardeningProfile::derive(spec);
        profile.assert_enforced().map_err(GvisorError::Hardening)?;
        Ok(OciConfig::from_spec(spec, &profile))
    }

    /// CT-007 slice 5b.3-6b: the launch entry point `launch`/`launch_streaming` call. Today it is a
    /// PLAIN DELEGATING WRAPPER around [`Self::launch_compute_with`] — EVERY spec, compute or
    /// checkout-bearing, runs the ordinary compute path byte-for-byte (a checkout-bearing job's
    /// `spec.workspace` is still silently ignored here, exactly as before this slice). That is the
    /// current dormant reality, verified structurally: checkout-bearing specs ALREADY reach this method
    /// in production — the manifest dispatch (`ci_manifest_job_runner::manifest_dispatch_parts`) builds
    /// `(Some, Some)` workspaces and the durable spec resolver (`runner_bind`) admits them once their
    /// checkout claim window is set — and they run as compute. `launch_with` deliberately does NOT
    /// shape-divert on `spec.workspace`: per Sol's design, SELECTING the checkout-aware path is
    /// 5b.3-6e's single activating cutover (which flips reservation/credential/accounting write
    /// versions atomically and only THEN routes valid checkout jobs through
    /// [`checkout_runtime::AcquiredCheckoutRuntime`] + [`Self::launch_checkout_continuation`]).
    /// Introducing a shape branch here in 6b would change production behavior for every manifest CI job
    /// (turning a run-as-compute into a refusal/panic), which this behavior-preserving slice forbids.
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

    /// Drive the four-guarantee seam in the mandated order — **isolation floor → hardening assert →
    /// image resolution (now a cheap, already-verified lookup) → reserve → final attribution/claim
    /// CAS → run → settle** — fail-closed at every step, then hand the captured [`SandboxResult`]
    /// back behind the redrawn CT-001 seam. This mirrors the Firecracker backend's own `launch_with`
    /// ordering exactly, with the image lookup inserted after the hardening assert and before
    /// reserve. `spec.image` is looked up against the registry's ALREADY-VERIFIED entries (the
    /// canonical-tree digest work happened ONCE, at [`GvisorAssetRegistry::from_bindings`]
    /// construction time — see `crate::asset_registry` — never per launch); an unknown image still
    /// refuses before any resource is reserved or any launch permit is granted (CT-007 gate 2/4), but
    /// a RED isolation floor now refuses BEFORE the registry is even consulted, so an exhausted-wallet
    /// caller cannot force a (now-cheap, but still real) lookup with zero chance of ever launching,
    /// and the floor is honoured even for callers naming an image the registry doesn't know about.
    /// The `run` closure does the actual run: it stages an OCI bundle from the built [`OciConfig`] +
    /// the verified rootfs path, runs `runsc run --bundle` (the untrusted `spec.command`), captures
    /// the real exit/streams/usage and enforces `spec.limits.timeout_secs`, and returns a
    /// [`ContainerRun`]. The trait `launch` passes [`run_production_container`] (a REAL `runsc`
    /// container); unit tests pass a closure returning a fake child + a canned result so the control
    /// flow is testable without a runtime (the injectable-spawn seam — preserved). `run` is only
    /// invoked AFTER reserve succeeds, so an exhausted wallet / unmet isolation floor refuses-to-start
    /// and `runsc` never spawns (CT-002b: the result is CONSUMED from the run, never hardcoded —
    /// reconciles with the Firecracker `launch_with`).
    ///
    /// CT-007 slice 5b.3-6b: extracted BYTE-FOR-BYTE out of the old `launch_with` (same
    /// signature/generics/hook-order/workspace-acquisition/run-closure/cleanup). It is the ordinary
    /// COMPUTE path — every production launch, compute or checkout-bearing, currently reaches it via
    /// the [`Self::launch_with`] wrapper. `spec.workspace` is intentionally never read here; making it
    /// checkout-aware is 5b.3-6e's job, not this slice's.
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
        // CT-007 slice 5b.3-6e.1: BYTE/BEHAVIOR-IDENTICAL to the pre-6e.1 compute path — the shared
        // preflight, the LEGACY `reserve`, the same container id, then the shared post-reservation
        // common body. The three operations happen in the exact prior order with the exact prior side
        // effects; only the surrounding boilerplate moved into named helpers so the dormant
        // `launch_compute_orchestrated_with` reuses this identical preflight and body.
        let (profile, verified_rootfs, cargo_vendor) =
            self.compute_launch_preflight(spec, hooks)?;
        let reserve = hooks
            .reserve(spec)
            .map_err(|e| SandboxLaunchError::Failed(e.into()))?;
        // CT-007 slice 3, piece 7a: generated HERE, not deep inside the run closure — the
        // `Enabled`-workspace path needs this same value before it ever calls `run`, to durably
        // bind a `UserNamespaceLease` to it ahead of exec (piece 7c), and (piece 7c) as the
        // workspace's own `job_key` — already a safe, unique path component.
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

    /// **Shared launch PREFLIGHT for both compute entries (CT-007 slice 5b.3-6e.1).** The isolation
    /// floor, the derived+asserted hardening profile, the registry rootfs resolution, and (for
    /// `Enabled`) both managers' independent health checks — everything BEFORE any reservation. Every
    /// refusal here is an ordinary pre-commit `Failed`: nothing durable has been claimed yet. The
    /// `VerifiedRootfs` is returned by borrow so the common body reuses the EXACT same resolution.
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
        // #4 isolation floor FIRST — the hardening profile must hold before any code (including the
        // registry lookup) runs. Mirrors the Firecracker backend's own ordering. Every early refusal
        // here is an ordinary pre-commit `Failed` — nothing durable has been claimed yet.
        hooks
            .enforce_isolation_floor(spec)
            .map_err(|e| SandboxLaunchError::Failed(e.into()))?;
        spec.validate_secret_coverage().map_err(|error| {
            SandboxLaunchError::Failed(GvisorError::Runtime(format!(
                "secret injection refused: {error}"
            )))
        })?;
        // Validate the structured-build selector before generic hardening can reject a networked
        // spec for a different reason. This keeps the compute routes' Cargo-specific empty-egress
        // refusal explicit, while registry resolution remains after the hardening assertion.
        let cargo_vendor_reference = validated_cargo_vendor_reference(spec)
            .map_err(|error| SandboxLaunchError::Failed(GvisorError::Runtime(error)))?;
        let profile = HardeningProfile::derive(spec);
        profile
            .assert_enforced()
            .map_err(|e| SandboxLaunchError::Failed(GvisorError::Hardening(e)))?;

        // CT-007 gate 2/4: resolve `spec.image` against the registry's ALREADY-VERIFIED entries — a
        // cheap O(1) lookup now (verification happened once, at registry construction). Still BEFORE
        // reserve/the launch-permit CAS — an unregistered image never reserves or spawns. A
        // `git_wire_only()` backend has no registry at all and refuses here, so it can never launch
        // an ordinary image-bearing job.
        let registry = self.registry.as_ref().ok_or_else(|| {
            SandboxLaunchError::Failed(GvisorError::Image(
                "this GvisorBackend was constructed via GvisorBackend::git_wire_only() (no asset \
                 registry) and cannot launch an ordinary image-bearing job — construct it via \
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

        // CT-007 slice 3, piece 7c: for `Enabled`, both managers must be independently healthy
        // BEFORE `reserve` — no reservation or other resource exists yet, so either refusal is an
        // ordinary pre-commit `Failed`.
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

    /// **The SHARED, source-identical post-reservation compute body (CT-007 slice 5b.3-6e.1).** Runs
    /// the workspace acquisition, runtime preparation, launch-permit CAS, the ONE legitimate `runsc`
    /// spawn (via `run`), the checked finalization/settlement tail, guest registration, and completion
    /// settlement — exactly the body [`Self::launch_compute_with`] ran inline before 6e.1. BOTH the
    /// legacy compute entry and the dormant [`Self::launch_compute_orchestrated_with`] run this exact
    /// code after their (identical) preflight; the ONLY difference between the two is which reservation
    /// mode produced `reserve`.
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
        // CT-007 #26/#27: derive THIS job's guest root before building the OCI layout. With a rootfs
        // overlay manager installed, `job_guest_root.path()` is a fresh per-job OverlayFS merged view
        // (verified base as the read-only lower); every use of the base path below — the workspace
        // layout's `absolute_rootfs`, the cargo-vendor mount-target precreation, and the staged
        // `root.path` — flows through it, so mount-target creation and gofer writes land in the per-job
        // upper and never mutate the shared pinned base. Without a manager it IS the verified base
        // (unchanged behavior). The guard is held for the whole call and torn down after `run()`.
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

        // CT-007 slice 3, piece 7c: acquire capacity + a userns lease + a real workspace, and
        // build the explicit-userns OCI layout from them — `Disabled` keeps the plain Rootless
        // `cfg` unchanged. `enabled_context` is `Some` for the REST of this call's lifetime
        // whenever `Enabled`, regardless of what happens later — it is only ever consumed (moved
        // out of this `Option`) by the cleanup paths below, never dropped bare.
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
                    // Sol's round-2 review: route through `dispose_run_failure` so a
                    // `release_unused` failure COMPOUNDS with the original acquisition failure
                    // (never silently replaces it via a bare `?`) — this acquisition failure
                    // never spawned anything, so `Uncommitted` is the correct phase. (Compute does not
                    // consume the 6c `reconciliation_required` signal — the reservation-owned
                    // reconciliation reaper is the compute path's existing owner; behaviour unchanged.)
                    return Err(self.dispose_run_failure(
                        spec,
                        hooks,
                        &reserve,
                        RunFailure::uncommitted(failure.message),
                    ));
                }
            },
        };

        // CT-007 slice 3, piece 7c: build the single validated `RuntimePreparation` — for
        // `Enabled`, the runsc-root identity is revalidated HERE (live, immediately before use,
        // never cached on `GvisorBackend`) to minimize the gap before the actual `bind` call
        // (which re-revalidates it again, live, one more time right at that exact boundary).
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
                // CT-007 slice 3, piece 7c (Sol's round-1 review, blocker 1): a `release_unused`
                // failure must AUGMENT this message, never silently replace it via a bare `?`.
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
                // CT-007 slice 3, piece 7c (Sol's round-1 review, blocker 1 — the fix, applied
                // without regressing the ORIGINAL `GvisorError` variant callers match on, e.g.
                // `GvisorError::Hook`): only fall back to a compound `GvisorError::Runtime(..)`
                // message when there is actually something to augment with; the common
                // Disabled/nothing-else-failed case preserves `attribute_error`'s own variant
                // exactly as before.
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
        // Run the container + capture the REAL result (the ONE legitimate `runsc`-spawn site — the
        // sandbox seam's mechanism; the `no-host-exec` named exclusion). `run` cleans up its own
        // bundle/container on error.
        //
        // `run`'s failure carries the phase the launch reached before it failed (Sol's design, fixing
        // a genuine pre-existing leak: the OLD code just propagated the error here and returned early,
        // never calling `release_unused` NOR `settle_completed` — leaking the reservation forever on
        // ANY run() failure, including ones where a real sandbox process actually executed). The
        // correct disposition ALSO depends on `hooks.completion_settlement_owner()`: under
        // `TerminalReporter` ownership, `settle_completed` is a documented no-op (deferred to the
        // real reporter), so a post-commit failure can only be honestly recorded by routing it
        // through `SandboxLaunchError::RetryableAttempt` — the runner then calls the reporter's own
        // `report_retryable_attempt` transaction, which durably accounts usage AND requeues the exact
        // claim without emitting `job.done`. Returning a bare `Failed` here for a post-commit failure
        // under reporter ownership would silently discard the accounting the reporter exists to do.
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
                // A pre-cgroup OR pre-bind failure (rootfs missing, mode mismatch, bundle staging,
                // cgroup creation, identity drift, bind failure) — no runtime was ever prepared
                // (or bind never durably committed), so there is nothing to finalize. `Bound` is
                // structurally unreachable here (see `cleanup_pre_bind_failure`'s own doc).
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
                // CT-007 slice 3, piece 7c (Sol's round-1 review, blocker 1): every cleanup
                // diagnostic is folded in — never `let _ = ...`-discarded — so a workspace-delete
                // or lease-release failure here is never silently lost behind the original
                // acquisition/preparation/bind failure.
                let run_failure = cleanup_diagnostics
                    .into_iter()
                    .fold(run_failure, augment_run_failure_message);
                return Err(self.dispose_run_failure(spec, hooks, &reserve, run_failure));
            }
            Ok(finalization) => {
                // CT-007 slice 5b.3-6c: the finalization→settle tail is now the shared
                // `settle_enabled_finalization` (BYTE-IDENTICAL logic to the pre-6c inline body) so
                // the checkout workload path can settle the capsule's OWN enabled context through the
                // exact same audited tail. Compute passes its own `Some`/`None` context + manager.
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

        // Settle against the result's REAL measured usage (CT-002b) — never interrupt in-flight.
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

    /// **The compute-V2 orchestrated entry selected by the typed cycle's compute arm.**
    ///
    /// A compute job under V2 cannot merely use the legacy [`ReserveHook`](crate::ReserveHook): it
    /// must create a durable PARENT-ATTEMPT row (so exhaustion is expressible) before any workload
    /// launches. This entry runs the IDENTICAL [`compute_launch_preflight`](Self::compute_launch_preflight)
    /// as the legacy [`launch_compute_with`](Self::launch_compute_with), then reserves through
    /// [`RunnerHooks::reserve_parent_attempt`] instead of `reserve`:
    ///
    /// - [`ParentAttemptAdmission::Admitted`](crate::checkout_orchestration::ParentAttemptAdmission::Admitted):
    ///   discard ONLY the unused [`AttemptAuthority`](crate::checkout_orchestration::AttemptAuthority)
    ///   (a compute workload never drives per-phase preparation credentials), RETAIN the durable parent
    ///   row (already inserted by the admission) and the `reserve`, and run the SAME source-identical
    ///   [`launch_compute_common_body`](Self::launch_compute_common_body) — its `SandboxLaunch` becomes
    ///   [`SandboxCycleOutcome::WorkloadLaunched`](crate::SandboxCycleOutcome::WorkloadLaunched).
    /// - [`ParentAttemptAdmission::AttemptsExhausted`](crate::checkout_orchestration::ParentAttemptAdmission::AttemptsExhausted):
    ///   the durable budget is spent, so NOTHING spawns — return a typed
    ///   [`SandboxCycleOutcome::PreparationTerminal`](crate::SandboxCycleOutcome::PreparationTerminal)
    ///   carrying the reporting `claim` and [`AttemptsExhausted`](crate::runner::PreparationTerminalDisposition::AttemptsExhausted).
    ///   The `reserve` is settled later by the preparation reporter against the claim's durable
    ///   identity (the in-hand [`ReserveHandle`] is only an id string, not an RAII guard), so dropping
    ///   it here leaks nothing.
    ///
    /// **Zero production callers** — the occurrence pin holds it at 0 — so `launch_compute_with`
    /// remains the sole LIVE compute path until the activating slice (6e.2) routes production
    /// `RunnerAgent` through the typed cycle seam.
    #[allow(dead_code)]
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
        // Same generated container id as the legacy path — a safe, unique path component.
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

    /// Dispose of a post-`reserve` [`RunFailure`] into the correct [`SandboxLaunchError`] variant,
    /// per phase AND per `hooks.completion_settlement_owner()` (Sol's disposition table):
    ///
    /// | Phase                     | `Hook` owner                        | `TerminalReporter` owner             |
    /// |----------------------------|--------------------------------------|----------------------------------------|
    /// | `Uncommitted`              | `release_unused`, then `Failed`       | `release_unused`, then `Failed`         |
    /// | `CommitOutcomeUnknown`     | `DurableOutcomeUnknown`               | `DurableOutcomeUnknown`                 |
    /// | `CommittedButNotExecuted`  | settle zero, then `Failed`            | `RetryableAttempt(SandboxInfrastructure, zero)` |
    /// | `Executed`                 | settle carried usage, then `Failed`   | `RetryableAttempt(SandboxInfrastructure, usage)` |
    ///
    /// `Uncommitted` and `CommitOutcomeUnknown` are owner-independent: an uncommitted attempt has no
    /// terminal report to defer to regardless of who owns completion, and an outcome-unknown attempt
    /// must never be guessed at either way. Only the two post-commit phases branch on ownership,
    /// because only they have a real (if zero) measured cost a `TerminalReporter` must account.
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
                         ({settle_error}) — reservation may be leaked"
                    )));
                }
                SandboxLaunchError::Failed(GvisorError::Runtime(message))
            }
            RunFailure::CommitOutcomeUnknown { .. } => {
                // Neither release nor settle — the durable store may or may not have actually
                // committed. Guessing either way misaccounts a real reservation; durable
                // reconciliation (the existing lease/claim reaper) is the only honest owner here.
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
                                 zero-usage settlement also failed ({settle_error}) — \
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
                             settlement also failed ({settle_error}) — reservation may be leaked"
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

    /// Run a digest-pinned [`JobSpec`] inside a REAL `runsc` (gVisor) sandbox. Blocks until the
    /// container has run and the four guarantees have fired. The REAL `runsc` container is spawned
    /// here — the one legitimate runtime-spawn site (the `no-host-exec` named exclusion; this seam IS
    /// the unified sandbox, not a bypass of it).
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

    /// **CT-007 slice 5b.3-6e.2: the typed sandbox CYCLE — the shape selector (behind the cycle
    /// method).** Overrides the trait default so a gVisor cycle routes on the job's workspace
    /// shape:
    ///
    /// - `(None, None)` — an ordinary compute job — runs the V2 parent-attempt-admitted orchestrated
    ///   entry, streaming exactly as [`Self::launch_streaming`] does.
    /// - `(Some, Some)` — a checkout-bearing job — runs the full Hop A → Hop B → workload orchestration
    ///   against the boot-validated checkout repository root; a checkout spec on a backend whose
    ///   checkout config is `disabled()` FAILS CLOSED (it never silently runs as compute).
    /// - a partial/malformed workspace — refused before any reserve or spawn.
    ///
    /// Stage B selects this from production [`RunnerAgent`](crate::runner::RunnerAgent), together
    /// with V2 hooks and an enabled checkout root.
    fn run_cycle(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
        output: Arc<dyn SandboxOutputSink>,
        cancellation: SandboxCancellation,
    ) -> Result<SandboxCycleOutcome, SandboxLaunchError<Self::Error>> {
        let output = cap_total_job_output(output);
        match crate::derive_checkout_authorization_scope(spec.kind, &spec.workspace) {
            // (None, None): compute — the V2 orchestrated entry (parent-attempt admission), streaming.
            //
            // CT-007 5b.3-6e.2 Stage A (Sol ruling): this compute arm is a FUTURE / non-manifest path,
            // DEAD-in-CI today. Every CI manifest job is checkout-bearing — `CiDriveManifestV1`'s
            // `workspace` mandates a `repo_ref`/`commit_oid`, and the durable authority reconstruction
            // (`runtime_authorities_from_durable_claim` in the control plane) therefore ALWAYS yields a
            // `(Some, Some)` checkout scope (see the `None`-for-compute note at
            // `myelin-ci-controlplane/src/ci_launch_authority.rs:68`, and the enforcing invariant test
            // `every_ci_manifest_authority_is_checkout_bearing` in
            // `integration_ci_6e2_active_path.rs`). The arm is kept intentionally: it lets a FUTURE
            // compute authority (workload-as-first-generation) activate WITHOUT reshaping the selector.
            // A live-PG compute-through-V2 proof is deferred to Stage B / whatever change first makes a
            // compute CI authority representable — the invariant test is the tripwire that forces it.
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
            // (Some, Some): checkout-bearing — the full Hop A → Hop B → workload orchestration against
            // the boot-validated repository root. No enabled root ⇒ fail closed (never run as compute).
            Ok(Some(_)) => {
                let repo_root = self.checkout.repo_root().ok_or_else(|| {
                    SandboxLaunchError::Failed(GvisorError::Hook(HookError(
                        "a checkout-bearing job requires an enabled checkout repository root, but \
                         this backend's checkout config is disabled — refusing before reserve/spawn"
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
            // A malformed workspace (mixed Some/None, unparseable ref/commit) — refuse before reserve
            // or spawn; it is dispatched as neither compute nor checkout.
            Err(reason) => Err(SandboxLaunchError::Failed(GvisorError::Hook(HookError(
                format!(
                "run_cycle refused a malformed workspace spec (neither a clean compute nor a valid \
                 checkout job): {reason}"
            ),
            )))),
        }
    }

    /// Whole-container kill on teardown: best-effort destroy the container + remove its bundle temp
    /// dir. The container is ephemeral, never reused. Idempotent — the run path has already deleted
    /// the container + bundle on completion, so killing an already-gone container is a no-op success.
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

    /// CT-007 slice 5b.3-6e.1: the checkout repository-root config validates at boot — no default
    /// fallback, and a relative / nonexistent / non-directory / non-canonical root fails closed.
    #[test]
    fn gvisor_checkout_config_validates_the_repo_root_at_boot() {
        // A relative path is refused.
        assert!(matches!(
            GvisorCheckoutConfig::enabled("relative/repo"),
            Err(GvisorCheckoutConfigError::NotAbsolute(_))
        ));
        // A nonexistent absolute path is refused.
        let missing = std::env::temp_dir().join(format!(
            "myelin-checkout-root-missing-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        assert!(matches!(
            GvisorCheckoutConfig::enabled(&missing),
            Err(GvisorCheckoutConfigError::NotADirectory { .. })
        ));
        // An absolute path to a FILE (not a directory) is refused.
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
        // A real, canonical directory is ACCEPTED and retains the exact root.
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
        // A non-canonical path (a `..`-bearing route to the same real dir) is refused, even though it
        // resolves to an existing directory.
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

    // ───────── CT-007 slice 3, piece 4: WorkspaceIntegration / GvisorWorkspaceConfig ─────────

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
        // Any leases_dir this unprivileged test process can create itself sits under a directory
        // it owns or can write to (its own home dir, or /tmp) — the strict production allocator's
        // ancestor-not-writable-by-us check refuses EVERY such path, deliberately: a genuinely
        // hardened leases dir requires a root-provisioned deployment layout, out of scope for an
        // ordinary unit test (this crate's own explicit-userns drill documents the identical
        // constraint). This makes the refusal fully deterministic here, which is exactly what this
        // test wants: proof that workspace reconciliation is never even attempted once userns
        // construction fails.
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
                "expected a UserNamespace error — a leases dir under this test's own home/tmp \
                 directory must never be considered safe"
            ),
        }
        assert!(
            !base.exists(),
            "workspace reconciliation must never run when userns construction fails first"
        );
    }

    /// Sol's round-1 review of piece 4: the public `try_new`'s own success path (and therefore the
    /// `Workspace(_)` error-mapping branch, reachable only once userns has ALREADY succeeded) is
    /// untestable end-to-end on an ordinary dev/CI host — the strict production allocator's
    /// ancestor check always refuses any leases_dir this unprivileged test process can create
    /// itself, AND `base_dir` here is never a real quota-enforcing Btrfs mount either, so even a
    /// hypothetical userns success would just trade one failure for another. These tests instead
    /// exercise `try_new_with_builders` directly, injecting builders that still return the REAL
    /// `UserNamespaceAllocator`/`WorkspaceManager` types (via their own existing test-relaxed
    /// constructors) — never a fabricated stand-in — so what `Enabled` actually holds is unchanged.
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
            // A `WorkspaceManager::Disabled` instance stands in as a genuine, real value of the
            // right type (mode-forwarding itself is already asserted in the sibling test above) —
            // never a fabricated non-real value.
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

    /// CT-007 #26/#27 INTEGRATION PROOF — the reproduced release blocker (gate-2 green drill,
    /// 2026-08-03): a build job MUTATED the shared digest-pinned base rootfs on the host (its
    /// canonical digest drifted `91ffb0fa… -> eb7248a1…` after one job), so the NEXT runner startup
    /// panicked `DigestMismatch` at asset re-verify. Cause: per-job mount-target creation / gofer
    /// writes landed in the SHARED base tree instead of a per-job ephemeral layer.
    ///
    /// This drives a REAL launch through `launch_with` -> `launch_compute_common_body` with a per-job
    /// rootfs overlay manager installed (deterministic mode: no `CAP_SYS_ADMIN`/kernel OverlayFS
    /// needed, but the SAME integration seam production uses — `materialize_job_guest_root` substitutes
    /// the overlay merged view for the base everywhere the base path flowed). The injected run closure
    /// stands in for runsc + the gofer: it WRITES into the guest root it is handed (a new mount-target
    /// directory + file, and a delete of a base file), exactly the host-side layout mutation that
    /// corrupted the base before. The property whose violation caused the panic is asserted directly:
    /// the base tree's canonical digest is BYTE-IDENTICAL before and after the job, and none of the
    /// job's writes reached the base — they were absorbed by the per-job overlay (a DIFFERENT path).
    #[test]
    fn compute_launch_guest_root_is_a_per_job_overlay_leaving_the_base_byte_pristine() {
        use crate::asset_registry::{GvisorAssetRegistry, RootfsAssetBinding};
        use crate::canonical_tree_sha256_hex;
        use crate::rootfs_overlay::{RootfsOverlayManager, RootfsOverlayMode};

        // A dedicated, isolated pinned base rootfs tree (never the shared per-process fixture dir).
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

        // A minimal image-bearing compute spec resolving to the pinned base above.
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
                    // The run closure receives the per-job guest root. It MUST be the overlay merged
                    // view, NOT the shared base.
                    assert_ne!(
                        rootfs, base_for_closure,
                        "the launch must NOT hand runsc the shared pinned base as its guest root"
                    );
                    // The merged view is a fully-populated copy of the verified base.
                    assert_eq!(
                        std::fs::read_to_string(rootfs.join("etc/keep")).unwrap(),
                        "keep"
                    );
                    // Simulate the exact host-side mutation that corrupted the base before: create a
                    // fresh mount-target directory + file, and delete a base file. All must land in
                    // the per-job upper, never the shared base.
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

        // THE property whose violation caused the DigestMismatch panic: the shared pinned base is
        // byte-identical before and after the job.
        assert_eq!(
            canonical_tree_sha256_hex(&base).unwrap(),
            base_digest_before,
            "the pinned base rootfs digest must be byte-identical after a job that wrote to its root"
        );
        // None of the job's host-side writes reached the base tree.
        assert!(
            base.join("delete-me").exists(),
            "a base file the job deleted (in the overlay) must still exist in the base"
        );
        assert!(
            !base.join("workspace/gofer-mount-target").exists(),
            "a mount target the job created (in the overlay) must NOT appear in the base"
        );
        // The guest root the run closure actually saw was a distinct per-job overlay path.
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

    /// Sol's required-tests list: "Enabled health checks precede reserve." Forces
    /// `userns_allocator.check_identity()` to fail deterministically (replacing the leases dir it
    /// locked at construction) — no real Btrfs/qgroup privilege needed — and proves `hooks.reserve`
    /// was never called by the time `launch_with` refuses.
    #[cfg(feature = "test-support")]
    #[test]
    fn enabled_health_checks_refuse_before_reserve_is_ever_called() {
        let Some((userns_allocator, leases_dir)) =
            real_userns_allocator_for_tests("health-precedes-reserve")
        else {
            return;
        };
        // Replace the leases dir AFTER construction so `check_identity()`'s re-stat disagrees with
        // the identity it locked at construction time — a deterministic, real failure.
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

    /// Sol's round-2 review (hard blocker for this activation commit, since `GvisorBackend::try_new`
    /// is now `pub`): the promised end-to-end drill through the REAL public activation path —
    /// `GvisorBackend::try_new(GvisorWorkspaceConfig::Enabled)` + `.launch(...)` — not merely manual
    /// orchestration of the individual pieces (those already have their own dedicated unit coverage:
    /// `bind_enabled_lease_given_*`, `finalize_runtime_*`, `gvisor_prod_exec_test`). This is the
    /// integration proof layered above that deterministic coverage.
    ///
    /// Sol's round-3 review: an EARLIER version of this drill generated its own fresh `leases_dir`
    /// under `std::env::temp_dir()` and treated `GvisorBackend::try_new(Enabled)`'s resulting
    /// GUARANTEED refusal (the strict allocator constructor can never accept a caller-generated,
    /// not-pre-provisioned leaf) as an ordinary host-dependent skip — making this drill an
    /// UNCONDITIONAL skip everywhere, never actually proving anything. Fixed: the ONLY legitimate
    /// skip condition now is `MYELIN_USERNS_DRILL_LEASES_DIR` being unset (this drill has no
    /// business fabricating that directory itself — see the const's own doc). Once an operator HAS
    /// supplied it, `GvisorBackend::try_new(Enabled)` is required to succeed (`.expect`, never a
    /// caught-and-skipped error) — reaching this point means the host is asserted to be correctly
    /// provisioned, so any further failure (construction OR `.launch()` itself) is a genuine
    /// regression this drill exists to catch, not a skip. The externally provisioned leases
    /// directory is NEVER removed by this drill (it is not this drill's to own or delete) — only the
    /// workspace base_dir this drill creates for itself is cleaned up.
    #[test]
    #[cfg(feature = "integration")]
    fn explicit_user_namespace_boots_through_the_real_enabled_backend_and_launch() {
        // Serializes against the checkout-preparation live drill, which shares the SAME
        // operator-provisioned `leases_dir` (see `USERNS_DRILL_LEASES_DIR_LOCK`'s own doc).
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
                     set — this drill needs an operator-provisioned leases directory satisfying the \
                     STRICT production allocator contract (pre-existing, euid-owned, mode 0700 or \
                     stricter, non-writable-by-us ancestor chain); it cannot fabricate one itself"
                );
                return;
            }
        };

        let tag = format!("{}-{}", std::process::id(), unique_suffix());
        // `std::env::temp_dir()` (`/tmp`) is frequently a separate tmpfs mount, not Btrfs — use a
        // `$HOME`-rooted path instead, matching every other real `WorkspaceManager` fixture in this
        // file (e.g. `real_workspace_manager_for_tests`).
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

        // Bind the spec to the SAME image the registry above was just built for (not
        // `fixture_image()`, which points at a different, throwaway fixture rootfs).
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

        // The leases dir is externally owned (an operator's install step) -- never removed here.
        let _ = std::fs::remove_dir_all(&workspace_base_dir);
    }

    #[test]
    fn gvisor_launch_drives_four_guarantees_on_the_same_trait() {
        // The SAME SandboxBackend trait + the SAME hardening — the named-second backend.
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
        // The reshaped seam carries the command result back (CT-001 stub).
        assert_eq!(launch.result.exit_code, Some(0));
        assert!(launch.result.passed());
        backend.kill(&launch.handle).unwrap();
    }

    /// CT-007 slice 3, piece 7a: `launch_with` (not the run closure) now generates `container_id`
    /// — this test proves the closure genuinely RECEIVES that same value (not an empty/placeholder
    /// one), in the expected shape, and that two separate launches never reuse it.
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

    // ───────── CT-007 slice 5b.3-6b: golden compute event-trace regression fence ─────────
    //
    // These three tests pin the OBSERVABLE ordered sequence of the ordinary compute path as it flows
    // through the `launch_with` wrapper into the extracted `launch_compute_with` body. 5b.3-6b moved
    // that body byte-for-byte and made `launch_with` a plain delegator; the point of these tests is a
    // regression fence — any future edit that reorders, drops, or duplicates an OBSERVABLE hook/run
    // step (isolation floor → reserve → launch permit → run spawn → settle) changes the recorded trace
    // and fails here. The fence covers the observable hook/run ordering and the two early-refusal
    // boundaries; it does NOT independently detect a reorder among non-observed internal steps (e.g.
    // moving container-id minting relative to reserve, or a duplicated registry lookup) — those are
    // covered by the mechanical byte-identity of the extraction plus the existing compute unit tests.
    // The Disabled (no-privilege) backend is used deliberately: the
    // Enabled-only steps (workspace-manager health check, `acquire_enabled_workspace`, Enabled
    // `RuntimePreparation`) are already fenced by the 6a acquire/settle + dispose matrices, and the
    // compute ORDERING these tests fence is identical regardless of workspace integration.

    /// Golden success trace: the exact ordered hook/run sequence for a compute launch, plus the stable
    /// `myelin-prod-*` workload id the run closure sees, the single live-map insert, and the
    /// byte-identical measured usage handed to `settle_completed`.
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
                // `fake_run` measures {cpu:1, mem:1}; `settle_completed` receives it VERBATIM.
                "settle:1:1".to_string(),
            ],
            "the ordered compute sequence through launch_with -> launch_compute_with is the fence"
        );

        // The container id the run closure receives is the freshly minted stable workload id.
        let observed = observed_container_id
            .lock()
            .unwrap()
            .clone()
            .expect("the run closure observed a container id");
        assert!(
            observed.starts_with(&format!("myelin-prod-{}-", std::process::id())),
            "the run closure sees the stable myelin-prod-* workload id, got {observed:?}"
        );
        // The successful run is inserted into the live map exactly once (keyed by runsc-<idem>).
        assert_eq!(
            backend.live.lock().unwrap().len(),
            1,
            "a successful compute launch inserts exactly one live entry"
        );
        assert!(launch.output_complete);
    }

    /// Golden failure variant 1 — a `git_wire_only()` backend (no registry) refuses an image-bearing
    /// job at registry resolve, AFTER the isolation floor but BEFORE `reserve`; the run closure never
    /// spawns. This fences the registry-None ordering (resolve precedes reserve).
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

    /// Golden failure variant 2 — a `reserve` refusal stops the sequence after the isolation floor and
    /// reserve, before the launch permit and the run spawn. Fences that reserve gates the launch.
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

    /// Sol's round-1 review: "all pre-permit compound-error combinations" -- when final
    /// attribution refuses AND releasing the now-unused reservation ALSO fails, the caller must see
    /// BOTH messages, never just the attribution error silently swallowing the release failure (or
    /// vice versa). Runs against `Disabled` (no privileged workspace needed) since this exercises
    /// the message-compounding logic itself, which is identical regardless of workspace
    /// integration -- `cleanup_diagnostics` is unconditionally empty for `Disabled`, so this proves
    /// the OTHER two of the three compounding sources (attribution error + release failure) meet
    /// correctly through the real `launch_with` code path, not just the pure `join_diagnostics`
    /// helper in isolation.
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

    /// The pre-existing leak this fix closes: previously, ANY error from `run(...)` propagated
    /// straight out of `launch_with` with NEITHER `release_unused` NOR `settle_completed` ever
    /// called — leaking the reservation on every single run failure. These tests prove each of the
    /// four `RunFailure` phases dispatches to the correct outcome, per Sol's corrected disposition
    /// table (phase × `CompletionSettlementOwner`):
    ///
    /// | Phase                    | `Hook` owner                       | `TerminalReporter` owner                  |
    /// |---------------------------|-------------------------------------|--------------------------------------------|
    /// | `Uncommitted`             | `release_unused`, then `Failed`     | `release_unused`, then `Failed`             |
    /// | `CommitOutcomeUnknown`    | `DurableOutcomeUnknown`             | `DurableOutcomeUnknown`                     |
    /// | `CommittedButNotExecuted` | settle zero, then `Failed`          | `RetryableAttempt(SandboxInfrastructure, 0)`|
    /// | `Executed`                | settle usage, then `Failed`         | `RetryableAttempt(SandboxInfrastructure, usage)`|
    ///
    /// `Uncommitted` and `CommitOutcomeUnknown` are owner-INDEPENDENT (an uncommitted attempt has no
    /// terminal report to defer to regardless of owner; an outcome-unknown attempt must never be
    /// guessed at either way) — only the two post-commit phases branch on ownership, since only they
    /// carry a real (if zero) measured cost a `TerminalReporter` must eventually account for.
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
            "release_unused must settle at zero even under reporter-owned completion — it is \
             owner-independent, unlike settle_completed"
        );
    }

    /// `CommitOutcomeUnknown` must NEVER release or settle — the durable commit outcome is
    /// genuinely unknown, and guessing either way misaccounts a real reservation. Owner-independent:
    /// this test uses `Hook` ownership specifically to prove the outcome-unknown path bypasses
    /// `settle_completed` entirely rather than merely happening to observe a reporter's no-op.
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

    /// `CommittedButNotExecuted` under `Hook` ownership settles zero synchronously, then surfaces
    /// `Failed` — a real terminal report IS expected here (unlike `Uncommitted`'s "none will ever
    /// follow"), and `Hook` ownership means the hook itself is the one committing that report.
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

    /// `CommittedButNotExecuted` under `TerminalReporter` ownership must NOT call `settle_completed`
    /// at all (it would silently no-op) — it must instead surface `RetryableAttempt` so the RUNNER
    /// routes it through the reporter's own `report_retryable_attempt` transaction, which durably
    /// accounts usage and either requeues or terminalizes the exact claim. This is the exact case
    /// Sol's review caught: the original fix called `settle_completed` here and returned an
    /// ordinary `Failed`, which under reporter ownership silently discarded the accounting with no
    /// terminal report ever following.
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
            "settle_completed must never be called directly here — the runner's retryable-attempt \
             transaction is the sole accounting path under reporter ownership"
        );
    }

    /// `Executed` under `Hook` ownership must settle the CONSERVATIVE fallback usage synchronously,
    /// never zero — a job engineered to fail exactly after the runtime was released to exec must not
    /// execute for free (the host-DoS surface Sol's design closes) — then surface `Failed`.
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

    /// `Executed` under `TerminalReporter` ownership must surface `RetryableAttempt` carrying the
    /// SAME conservative fallback usage (never zero) — the reporter's own transaction, not
    /// `settle_completed`, is what durably accounts it.
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
            "settle_completed must never be called directly here — the runner's retryable-attempt \
             transaction is the sole accounting path under reporter ownership"
        );
    }

    /// CT-007 gate 2/4 (f, corrected ordering): a RED isolation floor refuses BEFORE the registry
    /// lookup is ever consulted — proven by using a genuinely UNREGISTERED image (so if the
    /// (wrong-order) implementation queried the registry first, it would refuse there as
    /// `GvisorError::Image` WITHOUT the floor hook ever having been called, and `floor_called` would
    /// read `false`). Asserting `floor_called == true` alongside a `GvisorError::Hook` result is only
    /// possible if the floor really did run first, despite the image being unresolvable — which also
    /// means an exhausted-wallet caller cannot force the (now-cheap, but real) registry lookup by
    /// repeatedly failing the floor.
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
        // A fresh, otherwise-empty registry — the spec's image is deliberately NOT registered here,
        // so a wrong-order (registry-before-floor) implementation would refuse via `Image`, not
        // `Hook`, and would never call the floor closure at all.
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

    /// CT-007 gate 2/4 (f, still-correct half): a GREEN isolation floor + an unknown image still
    /// refuses before `reserve`/the `run` closure — none of them ever fire. This is the part of the
    /// original ordering test that was already right; it just now runs AFTER the floor instead of
    /// before it.
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
        // A fresh, otherwise-empty registry — the fixture image is deliberately NOT registered here.
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

    /// A committed regression pin for `GvisorBackend::git_wire_only()`'s refusal of ordinary launch:
    /// the behavior existed (see `launch_with`'s `self.registry.as_ref().ok_or_else(...)`) but had no
    /// test asserting it returns `GvisorError::Image` rather than panicking or hanging.
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

    /// The same refusal for the streaming entry point.
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
