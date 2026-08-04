//! The [`GvisorBackend`] itself — its construction/configuration, the live-container bookkeeping,
//! the compute launch path, and the [`SandboxBackend`] trait impl.

use super::*;
use crate::hardening::HardeningProfile;
use crate::runner::RetryableAttemptCause;
use crate::user_namespace::{
    UserNamespaceAllocator, UserNamespaceAllocatorError,
};
use crate::workspace_manager::{
    WorkspaceManager,
    WorkspaceManagerError, WorkspaceStorageMode,
};
use crate::{
    CompletionSettlementOwner, HookError, JobSpec,
    LaunchPermit, ReserveHandle, ResourceUsage, RunnerHooks, SandboxBackend, SandboxCancellation, SandboxCycleOutcome,
    SandboxHandle, SandboxLaunch, SandboxLaunchError, SandboxOutputSink,
    SandboxResult,
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
