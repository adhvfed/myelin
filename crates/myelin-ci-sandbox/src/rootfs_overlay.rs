//! Per-job OverlayFS rootfs primitive.
//!
//! This module provides the wired-but-dormant CoW primitive: it mounts the digest-verified base as
//! OverlayFS's read-only `lowerdir` and gives a job fresh `upperdir` and `workdir` directories, so a
//! job's writes land in the upper and never mutate the shared base. `GvisorBackend::
//! materialize_job_guest_root` already routes both launch paths (compute + checkout) through the
//! merged view WHEN a `RootfsOverlayManager` is installed. It stays DORMANT until then: production
//! has not yet installed a manager (`rootfs_overlay` is `None`), so `materialize_job_guest_root`
//! returns the bare verified base and behaviour is byte-identical to pre-integration. Activation is a
//! composition change (`with_rootfs_overlay_manager`) — see tasks #26/#27. Hosts used for unit tests select
//! [`RootfsOverlayMode::DeterministicDirectoryForTests`], which copies the exact same fd-bound lower
//! tree into `merged` without requiring `CAP_SYS_ADMIN` or OverlayFS support.
//!
//! The verified base pathname is opened once with `O_PATH|O_DIRECTORY|O_NOFOLLOW`, checked against
//! the `(device, inode)` captured by asset verification, and thereafter named only through
//! `/proc/self/fd/<fd>`. Both the OverlayFS `lowerdir` option and deterministic copy therefore use
//! the exact verified inode even if its old pathname is renamed after the check.
//!
//! Teardown is owned by the non-cloneable [`RootfsOverlay`] guard: unmount (production), then remove
//! only that guard's held per-job plain directory. An uncertain unmount, identity check, or removal
//! retains the capacity charge, poisons admission, and reports a reconciliation path. There is no
//! runtime shared-tree scan. Production initialization first unmounts and removes stale entries
//! beneath the dedicated overlay root, then enters a private runner mount namespace. Consequently,
//! normal guard teardown is deterministic, while runner exit (including `SIGKILL`) destroys any
//! remaining mounts with the namespace; restart cleanup runs before the new runner admits jobs.

use crate::asset_registry::VerifiedRootfs;
use crate::workspace_manager::IncidentSink;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{CString, OsStr, OsString};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
#[cfg(any(test, feature = "test-support"))]
use std::process::Command;
use std::sync::{Arc, Mutex, MutexGuard};

#[cfg(any(test, feature = "test-support"))]
const CP_BIN: &str = "/usr/bin/cp";

const OVERLAY_MOUNT_POLICY: &str = "metacopy=off,redirect_dir=nofollow,index=off,userxattr";
const OVERLAY_ROOT_MARKER: &str = ".myelin-overlay-root";
const OVERLAY_ROOT_MARKER_CONTENT: &[u8] = b"myelin-ci-sandbox overlay root v1\n";

/// Host-visible ownership and mode for the writable merged root.
///
/// The caller must pass the uid/gid that the workload's root uid maps to on the host. `0755` gives
/// that owner write access while keeping `/` traversable as a conventional root directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkloadRootPermissions {
    uid: u32,
    gid: u32,
    mode: u32,
}

impl WorkloadRootPermissions {
    pub fn new(uid: u32, gid: u32, mode: u32) -> Result<Self, RootfsOverlayError> {
        let owner_can_write_and_traverse = mode & 0o700 == 0o700;
        let root_is_traversable = mode & 0o055 == 0o055;
        let no_ambient_write = mode & 0o022 == 0;
        if mode & !0o777 == 0
            && owner_can_write_and_traverse
            && root_is_traversable
            && no_ambient_write
        {
            Ok(Self { uid, gid, mode })
        } else {
            Err(RootfsOverlayError::InvalidWorkloadRoot {
                uid,
                gid,
                mode,
                reason: "mode must grant owner rwx and group/other traversal without group/world \
                         write (use 0755)"
                    .to_string(),
            })
        }
    }

    pub fn uid(self) -> u32 {
        self.uid
    }

    pub fn gid(self) -> u32 {
        self.gid
    }

    pub fn mode(self) -> u32 {
        self.mode
    }
}

/// Storage substrate selected for per-job rootfs overlays.
#[derive(Clone, Debug)]
pub enum RootfsOverlayMode {
    /// Production: mount a kernel OverlayFS beneath `overlays_dir/<pid>/<job>`, with the verified
    /// base as its read-only lower layer and fresh plain directories as upper/work.
    OverlayFs { overlays_dir: PathBuf },
    /// Unit-test support: copy the fd-bound verified tree into the same per-job `merged` layout.
    #[cfg(any(test, feature = "test-support"))]
    DeterministicDirectoryForTests { overlays_dir: PathBuf },
}

impl RootfsOverlayMode {
    fn overlays_dir(&self) -> &Path {
        match self {
            Self::OverlayFs { overlays_dir } => overlays_dir,
            #[cfg(any(test, feature = "test-support"))]
            Self::DeterministicDirectoryForTests { overlays_dir } => overlays_dir,
        }
    }
}

/// Admission is monotonic: uncertain cleanup blocks new overlays until an external, boot-scoped
/// reconciliation has established a fresh overlay root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RootfsOverlayAdmission {
    Healthy,
    Poisoned { reason: String },
}

#[derive(Debug)]
pub enum RootfsOverlayError {
    InvalidJobKey {
        job_key: String,
    },
    InvalidWorkloadRoot {
        uid: u32,
        gid: u32,
        mode: u32,
        reason: String,
    },
    InvalidBase {
        path: PathBuf,
        reason: String,
    },
    Lifecycle {
        path: PathBuf,
        reason: String,
    },
    Poisoned {
        reason: String,
    },
    Create {
        path: PathBuf,
        reason: String,
    },
    CleanupUncertain {
        path: PathBuf,
        reason: String,
    },
}

impl std::fmt::Display for RootfsOverlayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidJobKey { job_key } => write!(
                f,
                "rootfs overlay job key {job_key:?} is not a safe path component"
            ),
            Self::InvalidWorkloadRoot {
                uid,
                gid,
                mode,
                reason,
            } => write!(
                f,
                "rootfs overlay workload root uid={uid} gid={gid} mode={mode:04o} is invalid: \
                 {reason}"
            ),
            Self::InvalidBase { path, reason } => write!(
                f,
                "rootfs overlay base {} is invalid: {reason}",
                path.display()
            ),
            Self::Lifecycle { path, reason } => write!(
                f,
                "initialize rootfs overlay lifecycle at {}: {reason}",
                path.display()
            ),
            Self::Poisoned { reason } => write!(
                f,
                "rootfs overlay manager is poisoned pending reconciliation: {reason}"
            ),
            Self::Create { path, reason } => {
                write!(f, "create rootfs overlay {}: {reason}", path.display())
            }
            Self::CleanupUncertain { path, reason } => write!(
                f,
                "rootfs overlay {} was not proven removed; reconciliation required: {reason}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for RootfsOverlayError {}

struct ManagerState {
    admission: RootfsOverlayAdmission,
    active: BTreeMap<PathBuf, String>,
    reconciliation: BTreeSet<PathBuf>,
}

struct Shared {
    mode: RootfsOverlayMode,
    overlay_root: Arc<File>,
    overlay_root_path: PathBuf,
    mount_namespace_identity: Option<(u64, u64)>,
    state: Mutex<ManagerState>,
    operations: Mutex<()>,
    incident_sink: IncidentSink,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, ManagerState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn report(&self, message: &str) {
        let sink = self.incident_sink.clone();
        let message = message.to_string();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || sink(&message)));
    }

    fn route_uncertainty(&self, path: &Path, job_key: Option<&str>, message: &str) {
        {
            let mut state = self.lock();
            if let Some(job_key) = job_key {
                state
                    .active
                    .entry(path.to_path_buf())
                    .or_insert_with(|| job_key.to_string());
            }
            state.reconciliation.insert(path.to_path_buf());
            if matches!(state.admission, RootfsOverlayAdmission::Healthy) {
                state.admission = RootfsOverlayAdmission::Poisoned {
                    reason: message.to_string(),
                };
            }
        }
        self.report(message);
    }

    fn cleanup(
        &self,
        path: &Path,
        resources: &mut OverlayResources,
    ) -> Result<(), RootfsOverlayError> {
        match resources.remove() {
            Ok(()) => {
                let mut state = self.lock();
                state.active.remove(path);
                state.reconciliation.remove(path);
                Ok(())
            }
            Err(reason) => {
                let error = RootfsOverlayError::CleanupUncertain {
                    path: path.to_path_buf(),
                    reason,
                };
                self.route_uncertainty(path, None, &error.to_string());
                Err(error)
            }
        }
    }
}

/// Persistent owner of overlay admission and fail-closed capacity accounting.
#[derive(Clone)]
pub struct RootfsOverlayManager {
    shared: Arc<Shared>,
}

impl RootfsOverlayManager {
    /// Initialize the overlay root before job admission.
    ///
    /// This startup-only constructor holds an exclusive lock on the dedicated overlay root for the
    /// manager's lifetime. It first recursively detaches stale mounts and removes stale entries. In
    /// production mode it then enters a new mount namespace and makes all propagation private. Call
    /// it before spawning runner worker threads; no overlay can be created without this lifecycle.
    pub fn initialize(
        mode: RootfsOverlayMode,
        incident_sink: IncidentSink,
    ) -> Result<Self, RootfsOverlayError> {
        let requested_root = mode.overlays_dir();
        let (overlay_root_path, overlay_root) = prepare_overlay_root(requested_root)?;
        startup_cleanup(&overlay_root_path, &overlay_root).map_err(|reason| {
            RootfsOverlayError::Lifecycle {
                path: overlay_root_path.clone(),
                reason,
            }
        })?;
        let mount_namespace_identity = if matches!(mode, RootfsOverlayMode::OverlayFs { .. }) {
            enter_private_mount_namespace().map_err(|reason| RootfsOverlayError::Lifecycle {
                path: overlay_root_path.clone(),
                reason,
            })?;
            Some(current_mount_namespace_identity().map_err(|reason| {
                RootfsOverlayError::Lifecycle {
                    path: overlay_root_path.clone(),
                    reason,
                }
            })?)
        } else {
            None
        };

        Ok(Self {
            shared: Arc::new(Shared {
                mode,
                overlay_root: Arc::new(overlay_root),
                overlay_root_path,
                mount_namespace_identity,
                state: Mutex::new(ManagerState {
                    admission: RootfsOverlayAdmission::Healthy,
                    active: BTreeMap::new(),
                    reconciliation: BTreeSet::new(),
                }),
                operations: Mutex::new(()),
                incident_sink,
            }),
        })
    }

    /// Create a writable per-job view whose lower layer is the exact once-verified base inode.
    pub fn create_overlay(
        &self,
        base: &VerifiedRootfs,
        job_key: &str,
        workload_root: WorkloadRootPermissions,
    ) -> Result<RootfsOverlay, RootfsOverlayError> {
        self.create_overlay_inner(base, job_key, workload_root, || {}, || {})
    }

    fn create_overlay_inner<F, G>(
        &self,
        base: &VerifiedRootfs,
        job_key: &str,
        workload_root: WorkloadRootPermissions,
        after_source_verified: F,
        after_layout_derived: G,
    ) -> Result<RootfsOverlay, RootfsOverlayError>
    where
        F: FnOnce(),
        G: FnOnce(),
    {
        validate_job_key(job_key)?;
        let _operation = self
            .shared
            .operations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let RootfsOverlayAdmission::Poisoned { reason } = &self.shared.lock().admission {
            return Err(RootfsOverlayError::Poisoned {
                reason: reason.clone(),
            });
        }
        if let Some(expected) = self.shared.mount_namespace_identity {
            let actual = current_mount_namespace_identity().map_err(|reason| {
                RootfsOverlayError::Lifecycle {
                    path: self.shared.overlay_root_path.clone(),
                    reason,
                }
            })?;
            if actual != expected {
                return Err(RootfsOverlayError::Lifecycle {
                    path: self.shared.overlay_root_path.clone(),
                    reason: format!(
                        "overlay creation escaped the initialized runner mount namespace \
                         (expected {expected:?}, found {actual:?})"
                    ),
                });
            }
        }

        let base_fd =
            open_path_directory(base.path()).map_err(|error| RootfsOverlayError::InvalidBase {
                path: base.path().to_path_buf(),
                reason: format!("open verified base with O_PATH|O_DIRECTORY|O_NOFOLLOW: {error}"),
            })?;
        let metadata = base_fd
            .metadata()
            .map_err(|error| RootfsOverlayError::InvalidBase {
                path: base.path().to_path_buf(),
                reason: format!("fstat fd-bound verified base: {error}"),
            })?;
        let actual_identity = (metadata.dev(), metadata.ino());
        if actual_identity != base.identity() {
            return Err(RootfsOverlayError::InvalidBase {
                path: base.path().to_path_buf(),
                reason: format!(
                    "verified base identity changed (expected {:?}, found {actual_identity:?})",
                    base.identity()
                ),
            });
        }

        // Test-only adversarial seam. Every derivation below consumes `base_fd`, never this path.
        after_source_verified();

        let leaf = format!("overlay-{job_key}-{}", next_overlay_sequence());
        let mut resources = match create_layout(
            &self.shared.overlay_root,
            &self.shared.overlay_root_path,
            &leaf,
        ) {
            Ok(resources) => resources,
            Err(error @ RootfsOverlayError::CleanupUncertain { .. }) => {
                if let RootfsOverlayError::CleanupUncertain { path, .. } = &error {
                    self.shared
                        .route_uncertainty(path, Some(job_key), &error.to_string());
                }
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        let path = resources.merged_path.clone();
        let derive = match &self.shared.mode {
            RootfsOverlayMode::OverlayFs { .. } => mount_overlay(&base_fd, &mut resources),
            #[cfg(any(test, feature = "test-support"))]
            RootfsOverlayMode::DeterministicDirectoryForTests { .. } => {
                copy_base_into_merged(&base_fd, &resources)
            }
        }
        .and_then(|()| {
            normalize_writable_root(&resources.upper_dir, workload_root).map_err(|reason| {
                format!(
                    "normalize upper root {} for workload uid/gid {}/{} mode {:04o}: {reason}",
                    resources.upper_path.display(),
                    workload_root.uid(),
                    workload_root.gid(),
                    workload_root.mode()
                )
            })
        })
        .and_then(|()| {
            normalize_writable_root(&resources.merged_dir, workload_root).map_err(|reason| {
                format!(
                    "normalize merged root {} for workload uid/gid {}/{} mode {:04o}: {reason}",
                    resources.merged_path.display(),
                    workload_root.uid(),
                    workload_root.gid(),
                    workload_root.mode()
                )
            })
        });
        if let Err(reason) = derive {
            let create_error = RootfsOverlayError::Create {
                path: path.clone(),
                reason,
            };
            return match self
                .shared
                .cleanup(&resources.job_path.clone(), &mut resources)
            {
                Ok(()) => Err(create_error),
                Err(error) => {
                    self.shared.route_uncertainty(
                        &resources.job_path,
                        Some(job_key),
                        &error.to_string(),
                    );
                    Err(error)
                }
            };
        }
        // Test-only unwind seam: `OverlayResources::drop` owns cleanup until the final guard exists.
        after_layout_derived();

        let job_path = resources.job_path.clone();
        {
            let mut state = self.shared.lock();
            if let RootfsOverlayAdmission::Poisoned { reason } = &state.admission {
                let reason = reason.clone();
                state.active.insert(job_path.clone(), job_key.to_string());
                drop(state);
                return match self.shared.cleanup(&job_path, &mut resources) {
                    Ok(()) => Err(RootfsOverlayError::Poisoned { reason }),
                    Err(error) => Err(error),
                };
            }
            if state
                .active
                .insert(job_path.clone(), job_key.to_string())
                .is_some()
            {
                drop(state);
                let _ = self.shared.cleanup(&job_path, &mut resources);
                return Err(RootfsOverlayError::Create {
                    path: job_path,
                    reason: "internal active-overlay collision".to_string(),
                });
            }
        }

        Ok(RootfsOverlay {
            path,
            job_path,
            upperdir: resources.upper_path.clone(),
            workdir: resources.work_path.clone(),
            lowerdir_source: proc_fd_path(&base_fd),
            resources: Some(resources),
            verified_base_digest: base.digest_hex().to_string(),
            verified_base_identity: base.identity(),
            base_fd,
            shared: self.shared.clone(),
        })
    }

    pub fn admission(&self) -> RootfsOverlayAdmission {
        self.shared.lock().admission.clone()
    }

    /// Number of per-job storage slots still charged. Cleanup uncertainty retains its slot.
    pub fn capacity_in_use(&self) -> usize {
        self.shared.lock().active.len()
    }

    /// Paths retained for external reconciliation. This is reporting only; it never scans or
    /// deletes a shared tree at runtime.
    pub fn reconciliation_paths(&self) -> BTreeSet<PathBuf> {
        self.shared.lock().reconciliation.clone()
    }
}

/// A non-cloneable writable rootfs view for the follow-on bundle-staging integration.
pub struct RootfsOverlay {
    path: PathBuf,
    job_path: PathBuf,
    upperdir: PathBuf,
    workdir: PathBuf,
    lowerdir_source: PathBuf,
    resources: Option<OverlayResources>,
    verified_base_digest: String,
    verified_base_identity: (u64, u64),
    // Held for the complete mount lifetime, matching the descriptor-derived lowerdir source.
    base_fd: File,
    shared: Arc<Shared>,
}

impl RootfsOverlay {
    /// Merged rootfs path to stage into the OCI bundle.
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn verified_base_digest(&self) -> &str {
        &self.verified_base_digest
    }

    pub fn verified_base_identity(&self) -> (u64, u64) {
        self.verified_base_identity
    }

    /// Descriptor-derived OverlayFS lowerdir source. The guard keeps its fd live.
    pub fn lowerdir_source(&self) -> &Path {
        debug_assert_eq!(
            self.lowerdir_source,
            proc_fd_path(&self.base_fd),
            "the published lowerdir must name the held base fd"
        );
        &self.lowerdir_source
    }

    /// Runner-owned plain directory receiving this job's OverlayFS changes.
    pub fn upperdir(&self) -> &Path {
        &self.upperdir
    }

    /// Runner-owned plain OverlayFS work directory paired with [`Self::upperdir`].
    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    /// Authoritative teardown; [`Drop`] performs the same operation during normal unwind.
    pub fn dispose(mut self) -> Result<(), RootfsOverlayError> {
        self.cleanup_once()
    }

    fn cleanup_once(&mut self) -> Result<(), RootfsOverlayError> {
        let Some(mut resources) = self.resources.take() else {
            return Ok(());
        };
        self.shared.cleanup(&self.job_path, &mut resources)
    }

    #[cfg(test)]
    fn job_path(&self) -> &Path {
        &self.job_path
    }
}

impl Drop for RootfsOverlay {
    fn drop(&mut self) {
        let _ = self.cleanup_once();
    }
}

struct OverlayLocation {
    root_dir: Arc<File>,
    pid_name: OsString,
    expected_pid_identity: (u64, u64),
    pid_dir: Arc<File>,
    leaf: OsString,
    expected_identity: (u64, u64),
}

struct OverlayResources {
    job_path: PathBuf,
    merged_path: PathBuf,
    upper_path: PathBuf,
    work_path: PathBuf,
    job_dir: File,
    merged_dir: File,
    upper_dir: File,
    work_dir: File,
    location: OverlayLocation,
    mounted: bool,
    cleanup_on_drop: bool,
    removed: bool,
}

impl OverlayResources {
    fn remove(&mut self) -> Result<(), String> {
        // Explicit guard/manager cleanup owns uncertainty reporting. Disarm the fallback Drop so a
        // failed authoritative attempt is not silently retried behind fail-closed accounting.
        self.cleanup_on_drop = false;
        self.remove_inner()
    }

    fn remove_inner(&mut self) -> Result<(), String> {
        if self.removed {
            return Ok(());
        }
        if self.mounted {
            unmount_held(&self.merged_dir)?;
            self.mounted = false;
        }
        let current =
            entry_identity_at(&self.location.pid_dir, &self.location.leaf).map_err(|error| {
                format!("identify guarded per-job directory before removal: {error}")
            })?;
        if current != self.location.expected_identity {
            return Err(format!(
                "guarded per-job directory identity changed (expected {:?}, found {current:?})",
                self.location.expected_identity
            ));
        }
        remove_directory_contents_fd_bound(&self.job_dir)?;
        let current =
            entry_identity_at(&self.location.pid_dir, &self.location.leaf).map_err(|error| {
                format!("recheck guarded per-job directory before unlinkat: {error}")
            })?;
        if current != self.location.expected_identity {
            return Err(format!(
                "guarded per-job directory identity changed before unlinkat (expected {:?}, found \
                 {current:?})",
                self.location.expected_identity
            ));
        }
        let leaf = component_cstring(&self.location.leaf).map_err(|error| error.to_string())?;
        // SAFETY: `leaf` is the guard's one checked component beneath its held PID directory.
        if unsafe {
            libc::unlinkat(
                self.location.pid_dir.as_raw_fd(),
                leaf.as_ptr(),
                libc::AT_REMOVEDIR,
            )
        } < 0
        {
            return Err(format!(
                "unlinkat guarded per-job directory: {}",
                io::Error::last_os_error()
            ));
        }
        readable_reopen(&self.location.pid_dir)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("fsync held PID directory after removal: {error}"))?;
        self.removed = true;
        remove_empty_pid_directory(&self.location);
        Ok(())
    }
}

impl Drop for OverlayResources {
    fn drop(&mut self) {
        if self.cleanup_on_drop {
            let _ = self.remove_inner();
        }
    }
}

fn remove_empty_pid_directory(location: &OverlayLocation) {
    let Ok(identity) = entry_identity_at(&location.root_dir, &location.pid_name) else {
        return;
    };
    if identity != location.expected_pid_identity {
        return;
    }
    let Ok(pid_name) = component_cstring(&location.pid_name) else {
        return;
    };
    // Best-effort organizational cleanup only: ENOTEMPTY is expected while sibling jobs exist.
    // SAFETY: identity was rechecked and the name is one component beneath the held overlay root.
    let _ = unsafe {
        libc::unlinkat(
            location.root_dir.as_raw_fd(),
            pid_name.as_ptr(),
            libc::AT_REMOVEDIR,
        )
    };
}

fn validate_job_key(job_key: &str) -> Result<(), RootfsOverlayError> {
    let valid = !job_key.is_empty()
        && job_key.len() <= 96
        && job_key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
    if valid {
        Ok(())
    } else {
        Err(RootfsOverlayError::InvalidJobKey {
            job_key: job_key.to_string(),
        })
    }
}

fn next_overlay_sequence() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn prepare_overlay_root(path: &Path) -> Result<(PathBuf, File), RootfsOverlayError> {
    if !path.is_absolute() || path == Path::new("/") {
        return Err(RootfsOverlayError::Lifecycle {
            path: path.to_path_buf(),
            reason: "overlay root must be a dedicated absolute directory other than /".to_string(),
        });
    }
    let created = match fs::create_dir(path) {
        Ok(()) => true,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => false,
        Err(error) => {
            return Err(RootfsOverlayError::Lifecycle {
                path: path.to_path_buf(),
                reason: format!("create dedicated overlay root: {error}"),
            });
        }
    };
    if created {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|error| {
            RootfsOverlayError::Lifecycle {
                path: path.to_path_buf(),
                reason: format!("set dedicated overlay root mode 0700: {error}"),
            }
        })?;
    }
    let path_root = open_path_directory(path).map_err(|error| RootfsOverlayError::Lifecycle {
        path: path.to_path_buf(),
        reason: format!(
            "open dedicated overlay root without following its final component: {error}"
        ),
    })?;
    let canonical =
        fs::read_link(proc_fd_path(&path_root)).map_err(|error| RootfsOverlayError::Lifecycle {
            path: path.to_path_buf(),
            reason: format!("resolve held dedicated overlay root: {error}"),
        })?;
    if canonical == Path::new("/") {
        return Err(RootfsOverlayError::Lifecycle {
            path: canonical,
            reason: "overlay root resolved to /; refusing shared-tree cleanup".to_string(),
        });
    }
    let root = readable_reopen(&path_root).map_err(|error| RootfsOverlayError::Lifecycle {
        path: canonical.clone(),
        reason: format!("reopen dedicated overlay root readably: {error}"),
    })?;
    // Acquire the exclusive advisory lock that makes this runner the SOLE owner of the dedicated
    // overlay root — the single-owner invariant the whole cleanup model rests on (`startup_cleanup`
    // may remove EVERY stale entry beneath the root precisely BECAUSE no other live runner can hold
    // it). `flock` is associated with the OPEN FILE DESCRIPTION, and a concurrent `fork()`+`execve()`
    // anywhere else in this process momentarily DUPLICATES this descriptor into the child across the
    // `[fork, exec]` window (`O_CLOEXEC` only closes it AT exec, not at fork). So immediately after a
    // previous owner in this same process released the lock, a sibling child racing to `exec` can
    // still pin it for a few milliseconds, surfacing a transient `EWOULDBLOCK`. That is NOT a
    // competing live runner: a real second runner holds the lock DURABLY, so it never clears within
    // the bounded budget and is still correctly refused below (single-owner is preserved — the budget
    // only ever elapses against a genuinely-held foreign lock). Production acquires this once at
    // single-threaded startup and never races; the retry hardens the crash-restart path (racing a
    // not-yet-reaped inherited fd) and makes parallel tests deterministic.
    let lock_deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        // SAFETY: `root` is a live descriptor; flock state is held by the returned file description.
        if unsafe { libc::flock(root.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            break;
        }
        let error = io::Error::last_os_error();
        // `EWOULDBLOCK == EAGAIN` on Linux — the single `EAGAIN` arm covers the flock "held" case.
        let transient = matches!(
            error.raw_os_error(),
            Some(libc::EAGAIN) | Some(libc::EINTR)
        );
        if transient && std::time::Instant::now() < lock_deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
            continue;
        }
        return Err(RootfsOverlayError::Lifecycle {
            path: canonical,
            reason: format!(
                "exclusively lock dedicated overlay root (another runner may own it): {error}"
            ),
        });
    }

    if created {
        let mut marker = openat_regular(
            &root,
            OsStr::new(OVERLAY_ROOT_MARKER),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
            0o600,
        )
        .map_err(|error| RootfsOverlayError::Lifecycle {
            path: canonical.clone(),
            reason: format!("create dedicated-root marker: {error}"),
        })?;
        marker
            .write_all(OVERLAY_ROOT_MARKER_CONTENT)
            .and_then(|()| marker.sync_all())
            .map_err(|error| RootfsOverlayError::Lifecycle {
                path: canonical.clone(),
                reason: format!("persist dedicated-root marker: {error}"),
            })?;
        root.sync_all()
            .map_err(|error| RootfsOverlayError::Lifecycle {
                path: canonical.clone(),
                reason: format!("persist dedicated overlay root: {error}"),
            })?;
    } else {
        let mut marker = openat_regular(&root, OsStr::new(OVERLAY_ROOT_MARKER), libc::O_RDONLY, 0)
            .map_err(|error| RootfsOverlayError::Lifecycle {
                path: canonical.clone(),
                reason: format!(
                "refuse cleanup of an unmarked directory; expected {OVERLAY_ROOT_MARKER}: {error}"
            ),
            })?;
        let mut content = Vec::new();
        marker
            .read_to_end(&mut content)
            .map_err(|error| RootfsOverlayError::Lifecycle {
                path: canonical.clone(),
                reason: format!("read dedicated-root marker: {error}"),
            })?;
        if content != OVERLAY_ROOT_MARKER_CONTENT {
            return Err(RootfsOverlayError::Lifecycle {
                path: canonical,
                reason: "dedicated-root marker has unexpected contents; refusing cleanup"
                    .to_string(),
            });
        }
    }
    Ok((canonical, root))
}

fn startup_cleanup(root_path: &Path, root: &File) -> Result<(), String> {
    let mountpoints = current_mountpoints()
        .map_err(|error| format!("read current mount namespace for stale overlays: {error}"))?;
    perform_startup_cleanup(root_path, mountpoints, unmount_stale_path, |_| {
        remove_overlay_root_stale_entries(root)
    })
}

fn perform_startup_cleanup<U, R>(
    root: &Path,
    mountpoints: Vec<PathBuf>,
    mut unmount: U,
    remove_stale_entries: R,
) -> Result<(), String>
where
    U: FnMut(&Path) -> Result<(), String>,
    R: FnOnce(&Path) -> Result<(), String>,
{
    if !root.is_absolute() || root == Path::new("/") {
        return Err(
            "startup cleanup root must be a dedicated absolute directory other than /".to_string(),
        );
    }
    let mut stale_mounts: Vec<_> = mountpoints
        .into_iter()
        .filter(|mountpoint| mountpoint != root && mountpoint.starts_with(root))
        .collect();
    stale_mounts.sort_by(|left, right| {
        right
            .components()
            .count()
            .cmp(&left.components().count())
            .then_with(|| right.cmp(left))
    });
    stale_mounts.dedup();
    for mountpoint in stale_mounts {
        unmount(&mountpoint)?;
    }
    remove_stale_entries(root)
}

fn current_mountpoints() -> io::Result<Vec<PathBuf>> {
    let mountinfo = fs::read("/proc/self/mountinfo")?;
    let mut mountpoints = Vec::new();
    for line in mountinfo.split(|byte| *byte == b'\n') {
        let Some(encoded) = line
            .split(|byte| *byte == b' ')
            .filter(|field| !field.is_empty())
            .nth(4)
        else {
            continue;
        };
        mountpoints.push(PathBuf::from(OsString::from_vec(decode_mountinfo_field(
            encoded,
        )?)));
    }
    Ok(mountpoints)
}

fn decode_mountinfo_field(encoded: &[u8]) -> io::Result<Vec<u8>> {
    let mut decoded = Vec::with_capacity(encoded.len());
    let mut index = 0;
    while index < encoded.len() {
        if encoded[index] == b'\\' {
            let Some(octal) = encoded.get(index + 1..index + 4) else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "truncated mountinfo escape",
                ));
            };
            if !octal.iter().all(u8::is_ascii_digit) || octal.iter().any(|byte| *byte > b'7') {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid mountinfo octal escape",
                ));
            }
            decoded.push((octal[0] - b'0') * 64 + (octal[1] - b'0') * 8 + octal[2] - b'0');
            index += 4;
        } else {
            decoded.push(encoded[index]);
            index += 1;
        }
    }
    Ok(decoded)
}

fn unmount_stale_path(path: &Path) -> Result<(), String> {
    let target = path_cstring(path).map_err(|error| format!("encode stale mountpoint: {error}"))?;
    // MNT_DETACH removes the stale namespace attachment even if a killed runner left references.
    // SAFETY: `target` is one mountpoint selected from mountinfo beneath the dedicated root.
    if unsafe { libc::umount2(target.as_ptr(), libc::MNT_DETACH) } < 0 {
        Err(format!(
            "detach stale overlay mount {}: {}",
            path.display(),
            io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

fn remove_overlay_root_stale_entries(root: &File) -> Result<(), String> {
    for name in read_dir_names(root)
        .map_err(|error| format!("enumerate dedicated overlay root: {error}"))?
    {
        if name == OsStr::new(OVERLAY_ROOT_MARKER) {
            continue;
        }
        remove_entry_fd_bound(root, &name)?;
    }
    readable_reopen(root)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("fsync dedicated overlay root after startup cleanup: {error}"))
}

fn enter_private_mount_namespace() -> Result<(), String> {
    // This must run before runner worker threads are spawned. Namespace destruction on process exit
    // (including SIGKILL) then tears down every per-job overlay still mounted inside it.
    // SAFETY: unshare changes only the calling process/thread's mount-namespace membership.
    if unsafe { libc::unshare(libc::CLONE_NEWNS) } < 0 {
        return Err(format!(
            "unshare runner mount namespace with CLONE_NEWNS: {}",
            io::Error::last_os_error()
        ));
    }
    // A copied namespace can inherit shared propagation (commonly from systemd). Recursively making
    // it private ensures subsequent overlay mounts never propagate back to the parent namespace.
    // SAFETY: null source/fs/data plus `/` and propagation-only flags are the mount(2) API contract.
    if unsafe {
        libc::mount(
            std::ptr::null(),
            c"/".as_ptr(),
            std::ptr::null(),
            libc::MS_REC | libc::MS_PRIVATE,
            std::ptr::null(),
        )
    } < 0
    {
        return Err(format!(
            "make runner mount namespace recursively private: {}",
            io::Error::last_os_error()
        ));
    }
    Ok(())
}

fn current_mount_namespace_identity() -> Result<(u64, u64), String> {
    // MUST read `/proc/thread-self`, NOT `/proc/self`: `unshare(CLONE_NEWNS)` moves only the CALLING
    // thread into the new namespace, but `/proc/self` resolves to the thread-group LEADER — so a
    // `/proc/self` read compares the leader's namespace to itself and can NEVER fire (a tautology,
    // false confidence). `/proc/thread-self` reflects the calling thread's own namespace, so the
    // init-capture and the per-job check genuinely verify that `create_overlay` runs inside the
    // private namespace `initialize` unshared into (a `std::thread::scope` child inherits it, so this
    // passes under the real runner thread model while catching a future thread that does not).
    let metadata = fs::metadata("/proc/thread-self/ns/mnt")
        .map_err(|error| format!("identify current runner mount namespace: {error}"))?;
    Ok((metadata.dev(), metadata.ino()))
}

fn create_layout(
    root: &Arc<File>,
    overlays_dir: &Path,
    leaf: &str,
) -> Result<OverlayResources, RootfsOverlayError> {
    let pid_name = OsString::from(std::process::id().to_string());
    mkdirat_if_absent(root, &pid_name).map_err(|error| RootfsOverlayError::Create {
        path: overlays_dir.join(&pid_name),
        reason: format!("create runner-PID directory: {error}"),
    })?;
    let pid_dir = Arc::new(openat_path_directory(root, &pid_name).map_err(|error| {
        RootfsOverlayError::Create {
            path: overlays_dir.join(&pid_name),
            reason: format!("open runner-PID directory without following symlinks: {error}"),
        }
    })?);
    let pid_metadata = pid_dir
        .metadata()
        .map_err(|error| RootfsOverlayError::Create {
            path: overlays_dir.join(&pid_name),
            reason: format!("fstat runner-PID directory: {error}"),
        })?;
    let leaf = OsString::from(leaf);
    let job_path = overlays_dir.join(&pid_name).join(&leaf);
    mkdirat_new(&pid_dir, &leaf).map_err(|error| RootfsOverlayError::Create {
        path: job_path.clone(),
        reason: format!("create per-job directory: {error}"),
    })?;
    let job_dir = openat_path_directory(&pid_dir, &leaf).map_err(|error| {
        RootfsOverlayError::CleanupUncertain {
            path: job_path.clone(),
            reason: format!("open newly-created per-job directory: {error}"),
        }
    })?;
    let metadata = job_dir
        .metadata()
        .map_err(|error| RootfsOverlayError::CleanupUncertain {
            path: job_path.clone(),
            reason: format!("fstat newly-created per-job directory: {error}"),
        })?;
    for name in ["upper", "work", "merged"] {
        mkdirat_new(&job_dir, OsStr::new(name)).map_err(|error| {
            RootfsOverlayError::CleanupUncertain {
                path: job_path.clone(),
                reason: format!("create {name} directory: {error}"),
            }
        })?;
    }
    let upper_dir = openat_path_directory(&job_dir, OsStr::new("upper")).map_err(|error| {
        RootfsOverlayError::CleanupUncertain {
            path: job_path.clone(),
            reason: format!("open upper directory: {error}"),
        }
    })?;
    let work_dir = openat_path_directory(&job_dir, OsStr::new("work")).map_err(|error| {
        RootfsOverlayError::CleanupUncertain {
            path: job_path.clone(),
            reason: format!("open work directory: {error}"),
        }
    })?;
    let merged_dir = openat_path_directory(&job_dir, OsStr::new("merged")).map_err(|error| {
        RootfsOverlayError::CleanupUncertain {
            path: job_path.clone(),
            reason: format!("open merged directory: {error}"),
        }
    })?;
    Ok(OverlayResources {
        merged_path: job_path.join("merged"),
        upper_path: job_path.join("upper"),
        work_path: job_path.join("work"),
        location: OverlayLocation {
            root_dir: root.clone(),
            pid_name,
            expected_pid_identity: (pid_metadata.dev(), pid_metadata.ino()),
            pid_dir,
            leaf,
            expected_identity: (metadata.dev(), metadata.ino()),
        },
        job_path,
        job_dir,
        merged_dir,
        upper_dir,
        work_dir,
        mounted: false,
        cleanup_on_drop: true,
        removed: false,
    })
}

fn mount_overlay(base_fd: &File, resources: &mut OverlayResources) -> Result<(), String> {
    let target = path_cstring(&proc_fd_path(&resources.merged_dir))
        .map_err(|error| format!("encode fd-bound merged mountpoint: {error}"))?;
    let source = c"overlay";
    let filesystem = c"overlay";
    let options = overlay_mount_options(
        &proc_fd_path(base_fd),
        &proc_fd_path(&resources.upper_dir),
        &proc_fd_path(&resources.work_dir),
    )?;
    // SAFETY: all strings are NUL-terminated and every procfs path names a live held directory fd.
    let result = unsafe {
        libc::mount(
            source.as_ptr(),
            target.as_ptr(),
            filesystem.as_ptr(),
            libc::MS_NODEV | libc::MS_NOSUID,
            options.as_ptr().cast(),
        )
    };
    if result < 0 {
        return Err(format!(
            "mount fd-bound OverlayFS lower/upper/work/merged: {}",
            io::Error::last_os_error()
        ));
    }
    resources.mounted = true;
    match openat_path_directory(&resources.job_dir, OsStr::new("merged")) {
        Ok(mounted_dir) => {
            // The descriptor opened before `mount` refers to the covered directory. Reopen the
            // component now so teardown's descriptor belongs to the mounted OverlayFS itself.
            resources.merged_dir = mounted_dir;
            Ok(())
        }
        Err(open_error) => {
            let unmount = unmount_path(&resources.merged_path);
            if unmount.is_ok() {
                resources.mounted = false;
            }
            Err(match unmount {
                Ok(()) => format!(
                    "OverlayFS mounted but its merged root could not be reopened through the held \
                     job directory; the mount was removed: {open_error}"
                ),
                Err(unmount_error) => format!(
                    "OverlayFS mounted but its merged root could not be reopened through the held \
                     job directory ({open_error}); rollback also failed: {unmount_error}"
                ),
            })
        }
    }
}

fn overlay_mount_options(lower: &Path, upper: &Path, work: &Path) -> Result<CString, String> {
    // Full data copy-up plus no redirect following/index eliminates dependence on undigested lower
    // metadata xattrs. `userxattr` is explicit for the non-root workload rather than host-default
    // dependent; with metacopy and redirects disabled it cannot reinterpret lower xattrs.
    CString::new(format!(
        "lowerdir={},upperdir={},workdir={},{OVERLAY_MOUNT_POLICY}",
        lower.display(),
        upper.display(),
        work.display()
    ))
    .map_err(|_| "OverlayFS mount options contain NUL".to_string())
}

fn normalize_writable_root(
    directory: &File,
    permissions: WorkloadRootPermissions,
) -> Result<(), String> {
    let directory = readable_reopen(directory)
        .map_err(|error| format!("reopen held root directory readably: {error}"))?;
    // Ownership is applied only to upper/merged descriptors. The fd-bound verified lower is never
    // passed here, preserving both its contents and metadata byte-for-byte.
    // SAFETY: `directory` is a live descriptor for the intended root directory.
    if unsafe { libc::fchown(directory.as_raw_fd(), permissions.uid(), permissions.gid()) } < 0 {
        return Err(format!(
            "fchown held writable root: {}",
            io::Error::last_os_error()
        ));
    }
    // SAFETY: `directory` is live and the validated mode contains only permission bits.
    if unsafe { libc::fchmod(directory.as_raw_fd(), permissions.mode()) } < 0 {
        return Err(format!(
            "fchmod held writable root: {}",
            io::Error::last_os_error()
        ));
    }
    Ok(())
}

fn unmount_held(merged: &File) -> Result<(), String> {
    unmount_path(&proc_fd_path(merged))
}

fn unmount_path(path: &Path) -> Result<(), String> {
    let target =
        path_cstring(path).map_err(|error| format!("encode merged mountpoint: {error}"))?;
    // SAFETY: target is a live descriptor-derived path to this guard's merged mountpoint.
    if unsafe { libc::umount2(target.as_ptr(), 0) } < 0 {
        Err(format!(
            "unmount merged OverlayFS: {}",
            io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

#[cfg(any(test, feature = "test-support"))]
fn copy_base_into_merged(base_fd: &File, resources: &OverlayResources) -> Result<(), String> {
    let inherited_base = duplicate_inheritable(base_fd)
        .map_err(|error| format!("duplicate held base fd for deterministic copy: {error}"))?;
    let inherited_merged = duplicate_inheritable(&resources.merged_dir)
        .map_err(|error| format!("duplicate held merged fd for deterministic copy: {error}"))?;
    let source = format!("/proc/self/fd/{}/.", inherited_base.as_raw_fd());
    let destination = format!("/proc/self/fd/{}/.", inherited_merged.as_raw_fd());
    let output = Command::new(CP_BIN)
        .env_clear()
        .args([OsStr::new("-a"), OsStr::new("--")])
        .arg(source)
        .arg(destination)
        .output()
        .map_err(|error| format!("spawn {CP_BIN} for fd-bound deterministic copy: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

fn open_path_directory(path: &Path) -> io::Result<File> {
    let path = path_cstring(path)?;
    // SAFETY: the path is NUL-terminated and success returns a new owned descriptor.
    let fd = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: `fd` is freshly returned by `open` and uniquely owned here.
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}

fn openat_path_directory(parent: &File, name: &OsStr) -> io::Result<File> {
    let name = component_cstring(name)?;
    // SAFETY: `name` is one component and `parent` remains live for this call.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: `fd` is freshly returned by `openat` and uniquely owned here.
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}

fn openat_regular(parent: &File, name: &OsStr, flags: i32, mode: libc::mode_t) -> io::Result<File> {
    let name = component_cstring(name)?;
    // SAFETY: `name` is one component, `parent` remains live, and success returns an owned fd.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            mode,
        )
    };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: `fd` is freshly returned by openat and uniquely owned here.
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}

fn mkdirat_if_absent(parent: &File, name: &OsStr) -> io::Result<()> {
    match mkdirat_new(parent, name) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error),
    }
}

fn mkdirat_new(parent: &File, name: &OsStr) -> io::Result<()> {
    let name = component_cstring(name)?;
    // SAFETY: `name` is one checked component below the held directory.
    if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn path_cstring(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))
}

fn component_cstring(name: &OsStr) -> io::Result<CString> {
    if name.as_bytes().contains(&b'/') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "directory-entry name contains slash",
        ));
    }
    CString::new(name.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "name contains NUL"))
}

fn proc_fd_path(directory: &File) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()))
}

fn readable_reopen(directory: &File) -> io::Result<File> {
    File::open(proc_fd_path(directory))
}

#[cfg(any(test, feature = "test-support"))]
fn duplicate_inheritable(file: &File) -> io::Result<File> {
    // F_DUPFD deliberately clears CLOEXEC so the child sees the same held inode by descriptor.
    // SAFETY: `file` is live; success returns a fresh descriptor uniquely owned below.
    let fd = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_DUPFD, 3) };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: `fd` is freshly returned by fcntl and uniquely owned here.
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}

fn entry_identity_at(parent: &File, name: &OsStr) -> io::Result<(u64, u64)> {
    let directory = openat_path_directory(parent, name)?;
    let metadata = directory.metadata()?;
    Ok((metadata.dev(), metadata.ino()))
}

fn read_dir_names(directory: &File) -> io::Result<Vec<OsString>> {
    fs::read_dir(proc_fd_path(directory))?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect()
}

fn remove_directory_contents_fd_bound(directory: &File) -> Result<(), String> {
    for name in read_dir_names(directory)
        .map_err(|error| format!("enumerate guarded per-job directory: {error}"))?
    {
        remove_entry_fd_bound(directory, &name)?;
    }
    readable_reopen(directory)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("fsync guarded per-job directory: {error}"))
}

fn remove_entry_fd_bound(parent: &File, name: &OsStr) -> Result<(), String> {
    let name_c = component_cstring(name).map_err(|error| error.to_string())?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: name and parent are live, and `stat` points at writable storage.
    if unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name_c.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } < 0
    {
        return Err(format!(
            "lstat guarded entry: {}",
            io::Error::last_os_error()
        ));
    }
    // SAFETY: successful fstatat initialized `stat`.
    let stat = unsafe { stat.assume_init() };
    let is_directory = stat.st_mode & libc::S_IFMT == libc::S_IFDIR;
    let is_symlink = stat.st_mode & libc::S_IFMT == libc::S_IFLNK;
    let flags = if is_directory && !is_symlink {
        let child = openat_path_directory(parent, name)
            .map_err(|error| format!("open guarded child directory: {error}"))?;
        remove_directory_contents_fd_bound(&child)?;
        libc::AT_REMOVEDIR
    } else {
        0
    };
    // SAFETY: deletion is one enumerated component beneath the held directory.
    if unsafe { libc::unlinkat(parent.as_raw_fd(), name_c.as_ptr(), flags) } < 0 {
        Err(format!(
            "unlink guarded entry: {}",
            io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset_registry::{GvisorAssetRegistry, RootfsAssetBinding};
    use crate::{canonical_tree_sha256_hex, ImageRef};

    struct Fixture {
        root: PathBuf,
        base: PathBuf,
        overlays: PathBuf,
        image: ImageRef,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "myelin-rootfs-overlay-{tag}-{}-{}",
                std::process::id(),
                next_overlay_sequence()
            ));
            let base = root.join("immutable-base");
            let overlays = root.join("overlays");
            fs::create_dir_all(base.join("etc")).unwrap();
            fs::create_dir(base.join("workspace")).unwrap();
            fs::write(base.join("etc/keep"), b"keep").unwrap();
            fs::write(base.join("delete-me"), b"delete").unwrap();
            let digest = canonical_tree_sha256_hex(&base).unwrap();
            let image = ImageRef::pinned(format!("test.local/{tag}@sha256:{digest}")).unwrap();
            Self {
                root,
                base,
                overlays,
                image,
            }
        }

        fn verified(&self) -> VerifiedRootfs {
            let registry = GvisorAssetRegistry::from_bindings(vec![RootfsAssetBinding {
                image: self.image.clone(),
                rootfs: self.base.clone(),
            }])
            .unwrap();
            registry.resolve(&self.image).unwrap().clone()
        }

        fn manager(&self, incidents: Arc<Mutex<Vec<String>>>) -> RootfsOverlayManager {
            RootfsOverlayManager::initialize(
                RootfsOverlayMode::DeterministicDirectoryForTests {
                    overlays_dir: self.overlays.clone(),
                },
                Arc::new(move |message| incidents.lock().unwrap().push(message.to_string())),
            )
            .unwrap()
        }

        fn workload_root(&self) -> WorkloadRootPermissions {
            // Unit tests run without CAP_CHOWN. The explicit identity is still verified against the
            // created inode; production passes the host-visible subuid (normally 65534) here.
            WorkloadRootPermissions::new(
                unsafe { libc::geteuid() },
                unsafe { libc::getegid() },
                0o755,
            )
            .unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn overlay_rootfs_isolation_keeps_base_byte_unchanged() {
        let fixture = Fixture::new("isolation");
        let manager = fixture.manager(Arc::new(Mutex::new(Vec::new())));
        let verified = fixture.verified();
        let before = canonical_tree_sha256_hex(&fixture.base).unwrap();
        let overlay = manager
            .create_overlay(&verified, "job-isolation", fixture.workload_root())
            .unwrap();

        fs::create_dir(overlay.path().join("workspace/job-output")).unwrap();
        fs::write(overlay.path().join("workspace/job-output/new"), b"new").unwrap();
        fs::remove_file(overlay.path().join("delete-me")).unwrap();

        assert_eq!(canonical_tree_sha256_hex(&fixture.base).unwrap(), before);
        assert!(fixture.base.join("delete-me").exists());
        assert!(!fixture.base.join("workspace/job-output").exists());
        assert!(overlay.path().join("workspace/job-output/new").exists());
    }

    #[test]
    fn overlay_rootfs_views_are_independent() {
        let fixture = Fixture::new("independence");
        let manager = fixture.manager(Arc::new(Mutex::new(Vec::new())));
        let verified = fixture.verified();
        let first = manager
            .create_overlay(&verified, "job-a", fixture.workload_root())
            .unwrap();
        let second = manager
            .create_overlay(&verified, "job-b", fixture.workload_root())
            .unwrap();
        fs::write(first.path().join("only-a"), b"a").unwrap();
        fs::write(second.path().join("only-b"), b"b").unwrap();
        assert!(first.path().join("only-a").exists());
        assert!(!first.path().join("only-b").exists());
        assert!(second.path().join("only-b").exists());
        assert!(!second.path().join("only-a").exists());
        assert!(!fixture.base.join("only-a").exists());
        assert!(!fixture.base.join("only-b").exists());
    }

    #[test]
    fn overlay_rootfs_upper_root_has_workload_ownership_and_traversable_mode() {
        let fixture = Fixture::new("root-permissions");
        let manager = fixture.manager(Arc::new(Mutex::new(Vec::new())));
        let verified = fixture.verified();
        let before_digest = canonical_tree_sha256_hex(&fixture.base).unwrap();
        let before_metadata = fs::metadata(&fixture.base).unwrap();
        let workload_root = fixture.workload_root();
        let overlay = manager
            .create_overlay(&verified, "root-permissions", workload_root)
            .unwrap();

        let upper = fs::metadata(overlay.upperdir()).unwrap();
        assert_eq!(upper.uid(), workload_root.uid());
        assert_eq!(upper.gid(), workload_root.gid());
        assert_eq!(upper.mode() & 0o777, 0o755);
        let merged = fs::metadata(overlay.path()).unwrap();
        assert_eq!(merged.uid(), workload_root.uid());
        assert_eq!(merged.gid(), workload_root.gid());
        assert_eq!(merged.mode() & 0o777, 0o755);

        assert_eq!(
            canonical_tree_sha256_hex(&fixture.base).unwrap(),
            before_digest
        );
        let after_metadata = fs::metadata(&fixture.base).unwrap();
        assert_eq!(after_metadata.uid(), before_metadata.uid());
        assert_eq!(after_metadata.gid(), before_metadata.gid());
        assert_eq!(after_metadata.mode(), before_metadata.mode());

        let production_subuid = WorkloadRootPermissions::new(65_534, 65_534, 0o755).unwrap();
        assert_eq!(production_subuid.uid(), 65_534);
        assert_eq!(production_subuid.gid(), 65_534);
    }

    #[test]
    fn overlay_rootfs_mount_options_pin_xattr_independent_policy() {
        let options =
            overlay_mount_options(Path::new("/lower"), Path::new("/upper"), Path::new("/work"))
                .unwrap();
        assert_eq!(
            options.to_str().unwrap(),
            "lowerdir=/lower,upperdir=/upper,workdir=/work,metacopy=off,redirect_dir=nofollow,\
             index=off,userxattr"
        );
    }

    #[test]
    fn overlay_rootfs_startup_cleanup_is_scoped_and_unmounts_before_remove() {
        use std::cell::RefCell;

        let root = Path::new("/run/myelin/overlays");
        let events = RefCell::new(Vec::new());
        perform_startup_cleanup(
            root,
            vec![
                PathBuf::from("/shared/tree/mount"),
                PathBuf::from("/run/myelin/overlays/12/overlay-a/merged"),
                PathBuf::from("/run/myelin/overlays/12/overlay-a/merged/nested"),
                PathBuf::from("/run/myelin/overlays-not-ours/merged"),
            ],
            |path| {
                events
                    .borrow_mut()
                    .push(format!("unmount:{}", path.display()));
                Ok(())
            },
            |path| {
                events
                    .borrow_mut()
                    .push(format!("remove:{}", path.display()));
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(
            events.into_inner(),
            vec![
                "unmount:/run/myelin/overlays/12/overlay-a/merged/nested",
                "unmount:/run/myelin/overlays/12/overlay-a/merged",
                "remove:/run/myelin/overlays",
            ]
        );
    }

    #[test]
    fn overlay_rootfs_restart_cleanup_removes_stale_plain_entries() {
        let fixture = Fixture::new("restart-cleanup");
        let manager = fixture.manager(Arc::new(Mutex::new(Vec::new())));
        drop(manager);
        let stale = fixture.overlays.join("old-runner/old-job");
        fs::create_dir_all(stale.join("upper")).unwrap();
        fs::create_dir(stale.join("work")).unwrap();
        fs::create_dir(stale.join("merged")).unwrap();

        let manager = fixture.manager(Arc::new(Mutex::new(Vec::new())));
        assert!(!fixture.overlays.join("old-runner").exists());
        assert!(fixture.overlays.join(OVERLAY_ROOT_MARKER).is_file());
        drop(manager);
    }

    #[test]
    fn overlay_rootfs_startup_cleanup_refuses_unmarked_shared_directory() {
        let fixture = Fixture::new("unmarked-root");
        fs::create_dir(&fixture.overlays).unwrap();
        let sentinel = fixture.overlays.join("operator-owned-sentinel");
        fs::write(&sentinel, b"keep").unwrap();

        let result = RootfsOverlayManager::initialize(
            RootfsOverlayMode::DeterministicDirectoryForTests {
                overlays_dir: fixture.overlays.clone(),
            },
            Arc::new(|_| {}),
        );
        assert!(matches!(result, Err(RootfsOverlayError::Lifecycle { .. })));
        assert_eq!(fs::read(sentinel).unwrap(), b"keep");
    }

    #[test]
    fn overlay_rootfs_lifecycle_has_private_mount_namespace_seam() {
        let source = include_str!("rootfs_overlay.rs");
        assert!(source.contains("libc::unshare(libc::CLONE_NEWNS)"));
        assert!(source.contains("libc::MS_REC | libc::MS_PRIVATE"));
        assert!(source.contains("startup_cleanup(&overlay_root_path, &overlay_root)"));
        assert!(
            source
                .find("startup_cleanup(&overlay_root_path, &overlay_root)")
                .unwrap()
                < source.find("enter_private_mount_namespace()").unwrap()
        );
    }

    #[test]
    fn overlay_resources_drop_cleans_pre_guard_unwind() {
        let fixture = Fixture::new("pre-guard-unwind");
        let manager = fixture.manager(Arc::new(Mutex::new(Vec::new())));
        let verified = fixture.verified();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = manager.create_overlay_inner(
                &verified,
                "panic-before-guard",
                fixture.workload_root(),
                || {},
                || panic!("simulated failure before RootfsOverlay construction"),
            );
        }));
        assert!(result.is_err());
        assert_eq!(manager.capacity_in_use(), 0);
        assert_eq!(
            fs::read_dir(&fixture.overlays)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect::<Vec<_>>(),
            vec![OsString::from(OVERLAY_ROOT_MARKER)]
        );
    }

    #[test]
    fn overlay_rootfs_drop_cleans_plain_dirs_and_uncertainty_reconciles() {
        let fixture = Fixture::new("cleanup");
        let incidents = Arc::new(Mutex::new(Vec::new()));
        let manager = fixture.manager(incidents.clone());
        let verified = fixture.verified();
        let (job_path, upper, work) = {
            let overlay = manager
                .create_overlay(&verified, "clean", fixture.workload_root())
                .unwrap();
            assert_eq!(manager.capacity_in_use(), 1);
            (
                overlay.job_path().to_path_buf(),
                overlay.upperdir().to_path_buf(),
                overlay.workdir().to_path_buf(),
            )
        };
        assert!(!job_path.exists());
        assert!(!upper.exists());
        assert!(!work.exists());
        assert!(fixture.base.exists());
        assert_eq!(manager.capacity_in_use(), 0);

        let overlay = manager
            .create_overlay(&verified, "uncertain", fixture.workload_root())
            .unwrap();
        let guarded_path = overlay.job_path().to_path_buf();
        let displaced = fixture.root.join("displaced-job");
        fs::rename(&guarded_path, &displaced).unwrap();
        fs::create_dir(&guarded_path).unwrap();
        drop(overlay);
        assert!(matches!(
            manager.admission(),
            RootfsOverlayAdmission::Poisoned { .. }
        ));
        assert_eq!(manager.capacity_in_use(), 1);
        assert!(manager.reconciliation_paths().contains(&guarded_path));
        assert_eq!(incidents.lock().unwrap().len(), 1);
        assert!(displaced.join("upper").is_dir());
        assert!(fixture.base.exists());
    }

    #[test]
    fn overlay_rootfs_verify_to_use_is_fd_bound_across_base_rename() {
        let fixture = Fixture::new("fd-bound");
        let verified = fixture.verified();
        let manager = fixture.manager(Arc::new(Mutex::new(Vec::new())));
        let attacker = fixture.root.join("attacker-base");
        fs::create_dir_all(attacker.join("etc")).unwrap();
        fs::create_dir(attacker.join("workspace")).unwrap();
        fs::write(attacker.join("etc/keep"), b"attacker").unwrap();
        fs::write(attacker.join("attacker-only"), b"attacker").unwrap();
        let old_path = fixture.base.clone();
        let displaced = fixture.root.join("verified-base-renamed");

        let overlay = manager
            .create_overlay_inner(
                &verified,
                "rename",
                fixture.workload_root(),
                move || {
                    fs::rename(&old_path, &displaced).unwrap();
                    fs::rename(&attacker, &old_path).unwrap();
                },
                || {},
            )
            .unwrap();

        assert_eq!(
            fs::read(fixture.base.join("etc/keep")).unwrap(),
            b"attacker"
        );
        assert_eq!(fs::read(overlay.path().join("etc/keep")).unwrap(), b"keep");
        assert!(!overlay.path().join("attacker-only").exists());
        assert_eq!(overlay.verified_base_digest(), verified.digest_hex());
        assert_eq!(overlay.verified_base_identity(), verified.identity());
        assert!(overlay.lowerdir_source().starts_with("/proc/self/fd/"));

        let replaced = Fixture::new("replaced-before-open");
        let replaced_verified = replaced.verified();
        let replaced_manager = replaced.manager(Arc::new(Mutex::new(Vec::new())));
        fs::rename(&replaced.base, replaced.root.join("original-base")).unwrap();
        fs::create_dir(&replaced.base).unwrap();
        let error = replaced_manager
            .create_overlay(
                &replaced_verified,
                "must-fail-closed",
                replaced.workload_root(),
            )
            .err()
            .expect("replacement before the fd is opened must fail its identity check");
        assert!(matches!(error, RootfsOverlayError::InvalidBase { .. }));
        assert_eq!(replaced_manager.capacity_in_use(), 0);
    }

    #[test]
    fn overlay_rootfs_teardown_is_job_scoped_without_delete_helpers_or_runtime_scan() {
        let fixture = Fixture::new("scoped-cleanup");
        let manager = fixture.manager(Arc::new(Mutex::new(Vec::new())));
        let verified = fixture.verified();
        let overlay = manager
            .create_overlay(&verified, "guarded", fixture.workload_root())
            .unwrap();
        let pid_dir = overlay.job_path().parent().unwrap().to_path_buf();
        let sibling = pid_dir.join("operator-owned-sibling");
        fs::create_dir(&sibling).unwrap();
        fs::write(sibling.join("keep"), b"keep").unwrap();
        drop(overlay);
        assert_eq!(fs::read(sibling.join("keep")).unwrap(), b"keep");

        let source = include_str!("rootfs_overlay.rs");
        assert!(!source.contains(concat!("fn reconcile_", "orphans")));
        assert!(!source.contains(concat!("subvolume ", "delete")));
        assert!(!source.contains(concat!("/usr/bin/", "btrfs")));
        assert!(source.contains("wired-but-dormant CoW primitive"));
        assert!(!source.contains(concat!("Production mounts the digest-verified ", "base")));
        assert!(!source.contains(concat!("merged mount is the ", "only path")));
    }
}
