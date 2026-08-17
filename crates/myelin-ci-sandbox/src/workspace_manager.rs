use crate::dirlock::{fd_identity, path_identity};
use crate::workspace_storage::DirectoryWorkspaceStorage;
use crate::workspace_storage::{
    PreparedWorkspace, WorkspaceStorage, WorkspaceStorageBackend, WorkspaceStorageError,
};
use std::collections::BTreeSet;
use std::io;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Clone, Debug)]
pub enum WorkspaceStorageMode {
    Disabled,
    EphemeralDisk {
        base_dir: PathBuf,
        host_capacity_bytes: u64,
    },
    /// User-owned directories for an explicitly selected local-development runner.
    ///
    /// Capacity is admission-accounted but not a filesystem-enforced quota. Never use this
    /// mode for a shared or production runner.
    LocalDevelopmentDirectory {
        base_dir: PathBuf,
        host_capacity_bytes: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceAdmission {
    Reconciling,
    Healthy,
    Poisoned { reason: String },
}

#[derive(Debug)]
pub enum WorkspaceManagerError {
    AlreadyLocked { base_dir: PathBuf },
    LockFailed { base_dir: PathBuf, reason: String },
    Storage(WorkspaceStorageError),
    BaseReplaced { base_dir: PathBuf },
}

impl std::fmt::Display for WorkspaceManagerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkspaceManagerError::AlreadyLocked { base_dir } => write!(
                f,
                "workspace base {base_dir:?} is already locked by another process - two runner \
                 processes must never manage the same workspace base concurrently"
            ),
            WorkspaceManagerError::LockFailed { base_dir, reason } => {
                write!(f, "failed to lock workspace base {base_dir:?}: {reason}")
            }
            WorkspaceManagerError::Storage(e) => {
                write!(f, "workspace-storage startup/health check failed: {e}")
            }
            WorkspaceManagerError::BaseReplaced { base_dir } => write!(
                f,
                "workspace base {base_dir:?} no longer names the directory this manager locked \
                 at startup - refusing to trust a replacement directory"
            ),
        }
    }
}

impl std::error::Error for WorkspaceManagerError {}

pub type IncidentSink = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapacityRefusal {
    Disabled,
    Reconciling,
    Poisoned,
    ZeroBytesRequested,
    Overflow,
    ExhaustedCapacity { requested: u64, available: u64 },
}

impl std::fmt::Display for CapacityRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CapacityRefusal::Disabled => {
                write!(
                    f,
                    "workspace storage is Disabled - no capacity ceiling exists"
                )
            }
            CapacityRefusal::Reconciling => {
                write!(
                    f,
                    "workspace manager has not finished boot-time reconciliation yet"
                )
            }
            CapacityRefusal::Poisoned => {
                write!(f, "workspace manager is poisoned - refusing all admission")
            }
            CapacityRefusal::ZeroBytesRequested => {
                write!(f, "a zero-byte capacity request is invalid")
            }
            CapacityRefusal::Overflow => {
                write!(
                    f,
                    "capacity accounting would overflow - refused rather than wrapping"
                )
            }
            CapacityRefusal::ExhaustedCapacity {
                requested,
                available,
            } => write!(
                f,
                "requested {requested} bytes but only {available} bytes of aggregate host \
                 capacity remain"
            ),
        }
    }
}

#[derive(Debug)]
pub enum WorkspaceRequestRefusal {
    Disabled {
        capacity: CapacityLease,
    },
    Reconciling {
        capacity: CapacityLease,
    },
    Poisoned {
        capacity: CapacityLease,
    },
    JobAlreadyActive {
        job_key: String,
        capacity: CapacityLease,
    },
    WrongManager {
        capacity: CapacityLease,
    },
    CapacityMismatch {
        requested: u64,
        leased: u64,
        capacity: CapacityLease,
    },
}

impl WorkspaceRequestRefusal {
    pub fn into_capacity(self) -> CapacityLease {
        match self {
            WorkspaceRequestRefusal::Disabled { capacity }
            | WorkspaceRequestRefusal::Reconciling { capacity }
            | WorkspaceRequestRefusal::Poisoned { capacity }
            | WorkspaceRequestRefusal::JobAlreadyActive { capacity, .. }
            | WorkspaceRequestRefusal::WrongManager { capacity }
            | WorkspaceRequestRefusal::CapacityMismatch { capacity, .. } => capacity,
        }
    }
}

impl std::fmt::Display for WorkspaceRequestRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkspaceRequestRefusal::Disabled { .. } => {
                write!(
                    f,
                    "workspace storage is Disabled - no workspace is ever provisioned"
                )
            }
            WorkspaceRequestRefusal::Reconciling { .. } => write!(
                f,
                "workspace manager has not finished boot-time reconciliation yet"
            ),
            WorkspaceRequestRefusal::Poisoned { .. } => {
                write!(f, "workspace manager is poisoned - refusing all admission")
            }
            WorkspaceRequestRefusal::JobAlreadyActive { job_key, .. } => write!(
                f,
                "job key {job_key:?} already has an active, undeleted workspace"
            ),
            WorkspaceRequestRefusal::WrongManager { .. } => write!(
                f,
                "the supplied capacity lease was not leased from this manager"
            ),
            WorkspaceRequestRefusal::CapacityMismatch {
                requested, leased, ..
            } => write!(
                f,
                "requested {requested} quota bytes but the supplied capacity lease holds \
                 {leased} bytes - both must originate from the same spec.limits.disk_bytes"
            ),
        }
    }
}

impl std::error::Error for WorkspaceRequestRefusal {}

#[derive(Debug)]
pub enum WorkspaceProvisionError {
    Refused(WorkspaceRequestRefusal),
    Storage(WorkspaceStorageError),
    InternalInvariantViolated {
        reason: String,
        workspace: Box<ManagedWorkspace>,
    },
}

impl WorkspaceProvisionError {
    pub fn into_workspace_after_invariant_violation(self) -> Result<ManagedWorkspace, Self> {
        match self {
            WorkspaceProvisionError::InternalInvariantViolated { workspace, .. } => Ok(*workspace),
            other => Err(other),
        }
    }
}

impl std::fmt::Display for WorkspaceProvisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkspaceProvisionError::Refused(refusal) => write!(f, "{refusal}"),
            WorkspaceProvisionError::Storage(e) => {
                write!(f, "workspace-storage provisioning/deletion failed: {e}")
            }
            WorkspaceProvisionError::InternalInvariantViolated { reason, .. } => {
                write!(f, "internal invariant violated: {reason}")
            }
        }
    }
}

impl std::error::Error for WorkspaceProvisionError {}

struct ManagerState {
    storage: Option<WorkspaceStorageBackend>,
    admission: WorkspaceAdmission,
    active_job_ids: BTreeSet<String>,
    capacity_ceiling_bytes: u64,
    capacity_used_bytes: u64,
    locked_identity: Option<(u64, u64)>,
}

struct SharedState {
    _lock: Option<OwnedFd>,
    state: Mutex<ManagerState>,
    incident_sink: IncidentSink,
}

impl SharedState {
    fn lock_state(&self) -> MutexGuard<'_, ManagerState> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                let mut inner = poisoned.into_inner();
                if !matches!(inner.admission, WorkspaceAdmission::Poisoned { .. }) {
                    inner.admission = WorkspaceAdmission::Poisoned {
                        reason: "internal manager-state mutex was poisoned by a prior panic"
                            .to_string(),
                    };
                }
                inner
            }
        }
    }

    fn report_incident(&self, message: &str) {
        let sink = self.incident_sink.clone();
        let message = message.to_string();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || sink(&message)));
    }

    fn poison(&self, reason: impl Into<String>) {
        let reason = reason.into();
        {
            let mut state = self.lock_state();
            if !matches!(state.admission, WorkspaceAdmission::Poisoned { .. }) {
                state.admission = WorkspaceAdmission::Poisoned {
                    reason: reason.clone(),
                };
            }
        }
        self.report_incident(&reason);
    }
}

pub struct CapacityLease {
    bytes: u64,
    shared: Arc<SharedState>,
    released: bool,
}

impl std::fmt::Debug for CapacityLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CapacityLease")
            .field("bytes", &self.bytes)
            .field("released", &self.released)
            .finish()
    }
}

impl CapacityLease {
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    pub fn release(mut self) {
        let mut state = self.shared.lock_state();
        match state.capacity_used_bytes.checked_sub(self.bytes) {
            Some(next) => {
                state.capacity_used_bytes = next;
                drop(state);
                self.released = true;
            }
            None => {
                let used = state.capacity_used_bytes;
                let bytes = self.bytes;
                drop(state);
                self.released = true;
                self.shared.poison(format!(
                    "capacity-accounting corruption: releasing {bytes} bytes but only {used} \
                     bytes were recorded as used"
                ));
            }
        }
    }

    fn abandon_with_reason(mut self, reason: impl Into<String>) {
        self.shared.poison(reason);
        self.released = true;
    }
}

impl Drop for CapacityLease {
    fn drop(&mut self) {
        if !self.released {
            self.shared.poison(format!(
                "a {}-byte disk-capacity lease was dropped without an explicit release - a \
                 workspace may still be consuming this capacity on disk; refusing further \
                 admission until a human reconciles",
                self.bytes
            ));
        }
    }
}

pub struct ManagedWorkspace {
    job_key: String,
    prepared: Option<PreparedWorkspace>,
    capacity: Option<CapacityLease>,
    shared: Arc<SharedState>,
    released: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceAccessError {
    job_key: String,
    capability: &'static str,
}

impl std::fmt::Display for WorkspaceAccessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "managed workspace for job {:?} no longer owns its {} capability",
            self.job_key, self.capability
        )
    }
}

impl std::error::Error for WorkspaceAccessError {}

impl std::fmt::Debug for ManagedWorkspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ManagedWorkspace")
            .field("job_key", &self.job_key)
            .field("host_path", &self.prepared.as_ref().map(|p| p.host_path()))
            .field("released", &self.released)
            .finish()
    }
}

impl ManagedWorkspace {
    pub fn job_key(&self) -> &str {
        &self.job_key
    }

    pub fn host_path(&self) -> Result<&Path, WorkspaceAccessError> {
        self.prepared
            .as_ref()
            .ok_or_else(|| WorkspaceAccessError {
                job_key: self.job_key.clone(),
                capability: "prepared-storage",
            })
            .map(|prepared| prepared.host_path())
    }

    pub fn capacity_bytes(&self) -> Result<u64, WorkspaceAccessError> {
        self.capacity
            .as_ref()
            .ok_or_else(|| WorkspaceAccessError {
                job_key: self.job_key.clone(),
                capability: "capacity",
            })
            .map(CapacityLease::bytes)
    }

    #[cfg(test)]
    fn dismantle_for_tests(mut self) {
        if let Some(capacity) = self.capacity.take() {
            capacity.release();
        }
        self.released = true;
    }
}

#[cfg(any(test, feature = "test-support"))]
impl ManagedWorkspace {
    pub(crate) fn checked_test_quota_write(
        &self,
        file_name: &str,
        bytes: &[u8],
    ) -> Result<(), WorkspaceStorageError> {
        self.prepared
            .as_ref()
            .expect("checked_test_quota_write after this workspace was consumed by delete")
            .checked_directory_write(file_name, bytes)
    }

    pub(crate) fn scan_used_bytes(&self) -> Result<u64, WorkspaceStorageError> {
        self.prepared
            .as_ref()
            .expect("scan_used_bytes after this workspace was consumed by delete")
            .scan_used_bytes()
    }
}

impl Drop for ManagedWorkspace {
    fn drop(&mut self) {
        if !self.released {
            self.shared.poison(format!(
                "a managed workspace for job {:?} (host path {:?}) was dropped without being \
                 deleted - it may still exist on disk, still consuming its leased capacity; \
                 refusing further admission until a human reconciles",
                self.job_key,
                self.prepared.as_ref().map(|p| p.host_path())
            ));
            if let Some(mut capacity) = self.capacity.take() {
                capacity.released = true;
            }
        }
    }
}

pub struct WorkspaceManager {
    mode: WorkspaceStorageMode,
    shared: Arc<SharedState>,
}

impl WorkspaceManager {
    pub fn try_new(
        mode: WorkspaceStorageMode,
        incident_sink: IncidentSink,
    ) -> Result<Self, WorkspaceManagerError> {
        let (base_dir, host_capacity_bytes) = match &mode {
            WorkspaceStorageMode::Disabled => {
                return Ok(Self {
                    mode,
                    shared: Arc::new(SharedState {
                        _lock: None,
                        state: Mutex::new(ManagerState {
                            storage: None,
                            admission: WorkspaceAdmission::Healthy,
                            active_job_ids: BTreeSet::new(),
                            capacity_ceiling_bytes: 0,
                            capacity_used_bytes: 0,
                            locked_identity: None,
                        }),
                        incident_sink,
                    }),
                });
            }
            WorkspaceStorageMode::EphemeralDisk {
                base_dir,
                host_capacity_bytes,
            } => (base_dir.clone(), *host_capacity_bytes),
            WorkspaceStorageMode::LocalDevelopmentDirectory {
                base_dir,
                host_capacity_bytes,
            } => (base_dir.clone(), *host_capacity_bytes),
        };
        let lock = acquire_directory_lock(&base_dir)?;
        let locked_identity =
            fd_identity(&lock).map_err(|e| WorkspaceManagerError::LockFailed {
                base_dir: base_dir.clone(),
                reason: format!("fstat locked directory: {e}"),
            })?;
        let shared = Arc::new(SharedState {
            _lock: Some(lock),
            state: Mutex::new(ManagerState {
                storage: None,
                admission: WorkspaceAdmission::Reconciling,
                active_job_ids: BTreeSet::new(),
                capacity_ceiling_bytes: host_capacity_bytes,
                capacity_used_bytes: 0,
                locked_identity: Some(locked_identity),
            }),
            incident_sink,
        });
        let mut storage = open_enabled_backend(&mode, &base_dir)?;
        require_locked_identity(locked_identity, &base_dir, storage.base_dir())?;
        reconcile_orphans_at_boot(&mut storage).map_err(WorkspaceManagerError::Storage)?;
        require_locked_identity(locked_identity, &base_dir, storage.base_dir())?;
        storage
            .check_health()
            .map_err(WorkspaceManagerError::Storage)?;
        require_locked_identity(locked_identity, &base_dir, storage.base_dir())?;
        {
            let mut state = shared.lock_state();
            state.storage = Some(storage);
            state.admission = WorkspaceAdmission::Healthy;
        }
        Ok(Self { mode, shared })
    }

    #[cfg(test)]
    fn new_for_state_tests(
        base_dir: &Path,
        host_capacity_bytes: u64,
        incident_sink: IncidentSink,
    ) -> Result<Self, WorkspaceManagerError> {
        let lock = acquire_directory_lock(base_dir)?;
        let locked_identity =
            fd_identity(&lock).map_err(|e| WorkspaceManagerError::LockFailed {
                base_dir: base_dir.to_path_buf(),
                reason: format!("fstat locked directory: {e}"),
            })?;
        let shared = Arc::new(SharedState {
            _lock: Some(lock),
            state: Mutex::new(ManagerState {
                storage: None,
                admission: WorkspaceAdmission::Healthy,
                active_job_ids: BTreeSet::new(),
                capacity_ceiling_bytes: host_capacity_bytes,
                capacity_used_bytes: 0,
                locked_identity: Some(locked_identity),
            }),
            incident_sink,
        });
        Ok(Self {
            mode: WorkspaceStorageMode::EphemeralDisk {
                base_dir: base_dir.to_path_buf(),
                host_capacity_bytes,
            },
            shared,
        })
    }

    pub fn mode(&self) -> &WorkspaceStorageMode {
        &self.mode
    }

    pub fn admission(&self) -> WorkspaceAdmission {
        self.shared.lock_state().admission.clone()
    }

    pub fn is_healthy(&self) -> bool {
        matches!(self.admission(), WorkspaceAdmission::Healthy)
    }

    pub fn check_health(&self) -> Result<(), WorkspaceManagerError> {
        let Some(base_dir) = enabled_base_dir(&self.mode) else {
            return Ok(());
        };
        let state = self.shared.lock_state();
        let Some(locked_identity) = state.locked_identity else {
            let error = WorkspaceManagerError::LockFailed {
                base_dir: base_dir.to_path_buf(),
                reason: "enabled workspace manager has no recorded locked-directory identity"
                    .to_string(),
            };
            return self.poison_and_report(state, error);
        };

        if let Err(error) = check_path_matches_locked_identity(locked_identity, base_dir) {
            return self.poison_and_report(state, error);
        }

        let storage_base_dir = match state.storage.as_ref() {
            Some(storage) => storage.base_dir().to_path_buf(),
            None => {
                let error = WorkspaceManagerError::Storage(WorkspaceStorageError::Io {
                    path: base_dir.to_path_buf(),
                    reason: "internal invariant violated: an EphemeralDisk manager has no open \
                             WorkspaceStorage handle"
                        .to_string(),
                });
                return self.poison_and_report(state, error);
            }
        };
        if let Err(error) = check_path_matches_locked_identity(locked_identity, &storage_base_dir) {
            return self.poison_and_report(state, error);
        }

        let health_result = state.storage.as_ref().map_or_else(
            || {
                Err(WorkspaceStorageError::Io {
                    path: base_dir.to_path_buf(),
                    reason: "workspace storage disappeared during a locked health check".into(),
                })
            },
            WorkspaceStorageBackend::check_health,
        );
        match health_result {
            Ok(()) => {
                if let Err(error) = check_path_matches_locked_identity(locked_identity, base_dir) {
                    return self.poison_and_report(state, error);
                }
                if let Err(error) =
                    check_path_matches_locked_identity(locked_identity, &storage_base_dir)
                {
                    return self.poison_and_report(state, error);
                }
                Ok(())
            }
            Err(storage_error) => {
                self.poison_and_report(state, WorkspaceManagerError::Storage(storage_error))
            }
        }
    }

    fn poison_and_report(
        &self,
        mut state: MutexGuard<'_, ManagerState>,
        error: WorkspaceManagerError,
    ) -> Result<(), WorkspaceManagerError> {
        let message = error.to_string();
        if !matches!(state.admission, WorkspaceAdmission::Poisoned { .. }) {
            state.admission = WorkspaceAdmission::Poisoned {
                reason: message.clone(),
            };
        }
        drop(state);
        self.shared.report_incident(&message);
        Err(error)
    }

    pub fn acquire_capacity(&self, bytes: u64) -> Result<CapacityLease, CapacityRefusal> {
        if matches!(self.mode, WorkspaceStorageMode::Disabled) {
            return Err(CapacityRefusal::Disabled);
        }
        if bytes == 0 {
            return Err(CapacityRefusal::ZeroBytesRequested);
        }
        let mut state = self.shared.lock_state();
        match &state.admission {
            WorkspaceAdmission::Healthy => {}
            WorkspaceAdmission::Reconciling => return Err(CapacityRefusal::Reconciling),
            WorkspaceAdmission::Poisoned { .. } => return Err(CapacityRefusal::Poisoned),
        }
        let Some(next) = state.capacity_used_bytes.checked_add(bytes) else {
            return Err(CapacityRefusal::Overflow);
        };
        if next > state.capacity_ceiling_bytes {
            return Err(CapacityRefusal::ExhaustedCapacity {
                requested: bytes,
                available: state
                    .capacity_ceiling_bytes
                    .saturating_sub(state.capacity_used_bytes),
            });
        }
        state.capacity_used_bytes = next;
        drop(state);
        Ok(CapacityLease {
            bytes,
            shared: self.shared.clone(),
            released: false,
        })
    }

    pub fn capacity_used_bytes(&self) -> u64 {
        self.shared.lock_state().capacity_used_bytes
    }

    pub fn active_job_ids(&self) -> BTreeSet<String> {
        self.shared.lock_state().active_job_ids.clone()
    }

    pub fn create_workspace(
        &self,
        job_key: &str,
        quota_bytes: u64,
        owner_uid: u32,
        owner_gid: u32,
        capacity: CapacityLease,
    ) -> Result<ManagedWorkspace, WorkspaceProvisionError> {
        if matches!(self.mode, WorkspaceStorageMode::Disabled) {
            return Err(WorkspaceProvisionError::Refused(
                WorkspaceRequestRefusal::Disabled { capacity },
            ));
        }
        if !Arc::ptr_eq(&self.shared, &capacity.shared) {
            return Err(WorkspaceProvisionError::Refused(
                WorkspaceRequestRefusal::WrongManager { capacity },
            ));
        }
        if capacity.bytes() != quota_bytes {
            let leased = capacity.bytes();
            return Err(WorkspaceProvisionError::Refused(
                WorkspaceRequestRefusal::CapacityMismatch {
                    requested: quota_bytes,
                    leased,
                    capacity,
                },
            ));
        }
        let mut state = self.shared.lock_state();
        match &state.admission {
            WorkspaceAdmission::Healthy => {}
            WorkspaceAdmission::Reconciling => {
                return Err(WorkspaceProvisionError::Refused(
                    WorkspaceRequestRefusal::Reconciling { capacity },
                ))
            }
            WorkspaceAdmission::Poisoned { .. } => {
                return Err(WorkspaceProvisionError::Refused(
                    WorkspaceRequestRefusal::Poisoned { capacity },
                ))
            }
        }
        if state.active_job_ids.contains(job_key) {
            return Err(WorkspaceProvisionError::Refused(
                WorkspaceRequestRefusal::JobAlreadyActive {
                    job_key: job_key.to_string(),
                    capacity,
                },
            ));
        }
        let Some(storage) = state.storage.as_mut() else {
            drop(state);
            self.shared.poison(
                "healthy enabled workspace manager has no storage backend; refusing provisioning",
            );
            return Err(WorkspaceProvisionError::Refused(
                WorkspaceRequestRefusal::Poisoned { capacity },
            ));
        };
        let result = storage.create_workspace(job_key, quota_bytes, owner_uid, owner_gid);
        self.apply_create_result(state, job_key, capacity, result)
    }

    fn apply_create_result(
        &self,
        mut state: MutexGuard<'_, ManagerState>,
        job_key: &str,
        capacity: CapacityLease,
        result: Result<PreparedWorkspace, WorkspaceStorageError>,
    ) -> Result<ManagedWorkspace, WorkspaceProvisionError> {
        match result {
            Ok(prepared) => {
                let inserted = state.active_job_ids.insert(job_key.to_string());
                drop(state);
                let workspace = ManagedWorkspace {
                    job_key: job_key.to_string(),
                    prepared: Some(prepared),
                    capacity: Some(capacity),
                    shared: self.shared.clone(),
                    released: false,
                };
                if !inserted {
                    let reason = format!(
                        "job {job_key:?} was already in active_job_ids despite the \
                         immediately-preceding locked check - a workspace was just created on \
                         disk for it anyway and must still be cleaned up via delete_workspace"
                    );
                    self.shared.poison(reason.clone());
                    return Err(WorkspaceProvisionError::InternalInvariantViolated {
                        reason,
                        workspace: Box::new(workspace),
                    });
                }
                Ok(workspace)
            }
            Err(error @ WorkspaceStorageError::UnrecoverableLeak { .. }) => {
                drop(state);
                let job_key = job_key.to_string();
                capacity.abandon_with_reason(format!(
                    "workspace provisioning for job {job_key:?} failed unrecoverably: {error} - \
                     capacity retained rather than freed, since the subvolume may still exist"
                ));
                Err(WorkspaceProvisionError::Storage(error))
            }
            Err(error) => {
                drop(state);
                capacity.release();
                Err(WorkspaceProvisionError::Storage(error))
            }
        }
    }

    pub fn delete_workspace(
        &self,
        workspace: ManagedWorkspace,
    ) -> Result<(), DeleteWorkspaceError> {
        if !Arc::ptr_eq(&self.shared, &workspace.shared) {
            return Err(DeleteWorkspaceError::WrongManager { workspace });
        }
        let mut workspace = workspace;
        let job_key = workspace.job_key.clone();
        let (prepared, capacity) = match (workspace.prepared.take(), workspace.capacity.take()) {
            (Some(prepared), Some(capacity)) => (prepared, capacity),
            (prepared, capacity) => {
                workspace.released = true;
                let reason = format!(
                    "managed workspace for job {job_key:?} lost its prepared storage or capacity capability before deletion"
                );
                drop(prepared);
                if let Some(capacity) = capacity {
                    capacity.abandon_with_reason(reason.clone());
                } else {
                    self.shared.poison(reason.clone());
                }
                return Err(DeleteWorkspaceError::InternalInvariantViolated { reason });
            }
        };
        workspace.released = true;

        let mut state = self.shared.lock_state();
        let Some(storage) = state.storage.as_mut() else {
            drop(state);
            capacity.abandon_with_reason(format!(
                "workspace storage backend disappeared before deleting job {job_key:?}; capacity retained pending reconciliation"
            ));
            return Err(DeleteWorkspaceError::InternalInvariantViolated {
                reason: format!(
                    "workspace storage backend disappeared before deleting job {job_key:?}"
                ),
            });
        };
        let result = storage.delete_workspace(prepared);
        self.apply_delete_result(state, &job_key, capacity, result)
    }

    fn apply_delete_result(
        &self,
        mut state: MutexGuard<'_, ManagerState>,
        job_key: &str,
        capacity: CapacityLease,
        result: Result<(), WorkspaceStorageError>,
    ) -> Result<(), DeleteWorkspaceError> {
        match result {
            Ok(()) => {
                let removed = state.active_job_ids.remove(job_key);
                drop(state);
                capacity.release();
                if !removed {
                    let reason = format!(
                        "internal invariant violated: job {job_key:?} was not in active_job_ids \
                         even though its workspace was just successfully deleted from disk"
                    );
                    self.shared.poison(reason.clone());
                    return Err(DeleteWorkspaceError::InternalInvariantViolated { reason });
                }
                Ok(())
            }
            Err(error) => {
                drop(state);
                capacity.abandon_with_reason(format!(
                    "workspace delete/sync for job {job_key:?} failed: {error} - capacity \
                     retained and the job's active entry left in place pending reconciliation"
                ));
                Err(DeleteWorkspaceError::Storage(error))
            }
        }
    }
}

#[derive(Debug)]
pub enum DeleteWorkspaceError {
    WrongManager { workspace: ManagedWorkspace },
    Storage(WorkspaceStorageError),
    InternalInvariantViolated { reason: String },
}

impl DeleteWorkspaceError {
    pub fn into_workspace(self) -> Result<ManagedWorkspace, Self> {
        match self {
            DeleteWorkspaceError::WrongManager { workspace } => Ok(workspace),
            other => Err(other),
        }
    }
}

impl std::fmt::Display for DeleteWorkspaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeleteWorkspaceError::WrongManager { .. } => write!(
                f,
                "the supplied workspace was not checked out from this manager"
            ),
            DeleteWorkspaceError::Storage(e) => {
                write!(f, "workspace delete/sync failed: {e}")
            }
            DeleteWorkspaceError::InternalInvariantViolated { reason } => {
                write!(f, "internal invariant violated: {reason}")
            }
        }
    }
}

impl std::error::Error for DeleteWorkspaceError {}

fn enabled_base_dir(mode: &WorkspaceStorageMode) -> Option<&Path> {
    match mode {
        WorkspaceStorageMode::Disabled => None,
        WorkspaceStorageMode::EphemeralDisk { base_dir, .. } => Some(base_dir),
        WorkspaceStorageMode::LocalDevelopmentDirectory { base_dir, .. } => Some(base_dir),
    }
}

fn open_enabled_backend(
    mode: &WorkspaceStorageMode,
    base_dir: &Path,
) -> Result<WorkspaceStorageBackend, WorkspaceManagerError> {
    match mode {
        WorkspaceStorageMode::EphemeralDisk { .. } => Ok(WorkspaceStorageBackend::Btrfs(
            WorkspaceStorage::open(base_dir).map_err(WorkspaceManagerError::Storage)?,
        )),
        WorkspaceStorageMode::LocalDevelopmentDirectory { .. } => {
            Ok(WorkspaceStorageBackend::LocalDevelopmentDirectory(
                DirectoryWorkspaceStorage::open(base_dir)
                    .map_err(WorkspaceManagerError::Storage)?,
            ))
        }
        WorkspaceStorageMode::Disabled => {
            Err(WorkspaceManagerError::Storage(WorkspaceStorageError::Io {
                path: base_dir.to_path_buf(),
                reason: "disabled workspace mode has no storage backend".into(),
            }))
        }
    }
}

fn reconcile_orphans_at_boot(
    storage: &mut WorkspaceStorageBackend,
) -> Result<(), WorkspaceStorageError> {
    let empty_active_set = BTreeSet::new();
    let orphans = storage.list_orphaned_workspaces(&empty_active_set)?;
    for orphan in orphans {
        match storage.delete_orphan(orphan) {
            Ok(()) => {}
            Err(WorkspaceStorageError::SyncPending { subvol_id, .. }) => {
                storage.retry_pending_sync(subvol_id)?;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn acquire_directory_lock(base_dir: &Path) -> Result<OwnedFd, WorkspaceManagerError> {
    crate::dirlock::acquire_directory_lock(base_dir).map_err(|error| match error {
        crate::dirlock::DirLockError::AlreadyLocked => WorkspaceManagerError::AlreadyLocked {
            base_dir: base_dir.to_path_buf(),
        },
        crate::dirlock::DirLockError::Failed(reason) => WorkspaceManagerError::LockFailed {
            base_dir: base_dir.to_path_buf(),
            reason,
        },
    })
}

fn check_path_matches_locked_identity(
    locked_identity: (u64, u64),
    path: &Path,
) -> Result<(), WorkspaceManagerError> {
    let current = path_identity(path).map_err(|io_error| {
        WorkspaceManagerError::Storage(WorkspaceStorageError::Io {
            path: path.to_path_buf(),
            reason: if io_error.kind() == io::ErrorKind::NotFound {
                "path is missing".to_string()
            } else {
                io_error.to_string()
            },
        })
    })?;
    if current != locked_identity {
        return Err(WorkspaceManagerError::BaseReplaced {
            base_dir: path.to_path_buf(),
        });
    }
    Ok(())
}

fn require_locked_identity(
    locked_identity: (u64, u64),
    base_dir: &Path,
    storage_base_dir: &Path,
) -> Result<(), WorkspaceManagerError> {
    check_path_matches_locked_identity(locked_identity, base_dir)?;
    check_path_matches_locked_identity(locked_identity, storage_base_dir)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn unique_suffix() -> u64 {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        NEXT.fetch_add(1, Ordering::Relaxed)
    }

    fn test_base(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "myelin-workspace-manager-{tag}-{}-{}",
            std::process::id(),
            unique_suffix()
        ))
    }

    fn btrfs_test_base(tag: &str) -> PathBuf {
        let mut p = std::env::home_dir().expect("HOME must be set for this test");
        p.push(format!(
            ".local/state/myelin-workspace-manager-tests-{tag}-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        p
    }

    fn recording_sink() -> (IncidentSink, Arc<Mutex<Vec<String>>>) {
        let incidents: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let recorded = incidents.clone();
        let sink: IncidentSink = Arc::new(move |message: &str| {
            recorded.lock().unwrap().push(message.to_string());
        });
        (sink, incidents)
    }

    #[test]
    fn disabled_mode_performs_no_filesystem_io() {
        let bogus_base = test_base("disabled-bogus-parent")
            .join("nested")
            .join("deep");
        let (sink, _incidents) = recording_sink();
        let manager = WorkspaceManager::try_new(WorkspaceStorageMode::Disabled, sink)
            .expect("Disabled mode must never fail to construct");
        assert!(manager.is_healthy());
        assert_eq!(manager.capacity_used_bytes(), 0);
        assert_eq!(
            manager.acquire_capacity(1).unwrap_err(),
            CapacityRefusal::Disabled
        );
        assert!(manager.check_health().is_ok());
        assert!(
            !bogus_base.exists(),
            "Disabled mode must not touch the filesystem at all, even for a path it never saw"
        );
    }

    #[test]
    fn a_second_manager_over_the_same_base_refuses_the_lock() {
        let base = test_base("lock-contention");
        let (sink, _incidents) = recording_sink();
        let first = WorkspaceManager::new_for_state_tests(&base, 1 << 30, sink.clone())
            .expect("first manager locks cleanly");
        assert!(first.is_healthy());

        let second = WorkspaceManager::new_for_state_tests(&base, 1 << 30, sink);
        match second {
            Err(WorkspaceManagerError::AlreadyLocked { .. }) => {}
            Err(other) => panic!("expected AlreadyLocked, got a different error: {other}"),
            Ok(_) => panic!("a second manager over the same base must refuse, not succeed"),
        }
        drop(first);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn capacity_leases_are_bounded_and_release_frees_bytes_for_reuse() {
        let base = test_base("capacity-bounds");
        let (sink, _incidents) = recording_sink();
        let manager = WorkspaceManager::new_for_state_tests(&base, 100, sink).unwrap();

        let first = manager.acquire_capacity(60).expect("60 <= 100 must admit");
        assert_eq!(manager.capacity_used_bytes(), 60);
        assert_eq!(
            manager.acquire_capacity(50).unwrap_err(),
            CapacityRefusal::ExhaustedCapacity {
                requested: 50,
                available: 40
            },
            "60 + 50 > 100 must refuse rather than over-admit, reporting the real headroom"
        );
        let second = manager
            .acquire_capacity(40)
            .expect("60 + 40 == 100 must admit exactly at the ceiling");
        assert_eq!(manager.capacity_used_bytes(), 100);
        first.release();
        assert_eq!(manager.capacity_used_bytes(), 40);
        let third = manager
            .acquire_capacity(60)
            .expect("freed capacity must be reusable");
        assert_eq!(manager.capacity_used_bytes(), 100);
        second.release();
        third.release();
        assert_eq!(manager.capacity_used_bytes(), 0);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn acquire_capacity_rejects_zero_byte_requests() {
        let base = test_base("capacity-zero");
        let (sink, _incidents) = recording_sink();
        let manager = WorkspaceManager::new_for_state_tests(&base, 100, sink).unwrap();
        assert_eq!(
            manager.acquire_capacity(0).unwrap_err(),
            CapacityRefusal::ZeroBytesRequested
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn consumed_workspace_capabilities_are_typed_errors() {
        let base = test_base("consumed-access");
        let (sink, _incidents) = recording_sink();
        let manager = WorkspaceManager::new_for_state_tests(&base, 100, sink).unwrap();
        let workspace = ManagedWorkspace {
            job_key: "job-consumed".into(),
            prepared: None,
            capacity: None,
            shared: Arc::clone(&manager.shared),
            released: true,
        };

        assert_eq!(
            workspace.host_path().unwrap_err().to_string(),
            "managed workspace for job \"job-consumed\" no longer owns its prepared-storage capability"
        );
        assert_eq!(
            workspace.capacity_bytes().unwrap_err().to_string(),
            "managed workspace for job \"job-consumed\" no longer owns its capacity capability"
        );
    }

    #[test]
    fn abandoning_a_capacity_lease_poisons_the_manager_and_never_frees_its_bytes() {
        let base = test_base("capacity-abandon");
        let (sink, incidents) = recording_sink();
        let manager = WorkspaceManager::new_for_state_tests(&base, 100, sink).unwrap();
        {
            let lease = manager.acquire_capacity(30).expect("30 <= 100 must admit");
            drop(lease);
        }
        assert_eq!(
            manager.capacity_used_bytes(),
            30,
            "an abandoned lease's bytes must NOT be returned to the pool"
        );
        assert!(
            matches!(manager.admission(), WorkspaceAdmission::Poisoned { .. }),
            "an abandoned capacity lease must poison the manager"
        );
        assert_eq!(
            incidents.lock().unwrap().len(),
            1,
            "exactly one incident must be reported"
        );
        assert_eq!(
            manager.acquire_capacity(1).unwrap_err(),
            CapacityRefusal::Poisoned
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn poisoning_is_monotonic_and_never_reverts_to_healthy() {
        let (sink, incidents) = recording_sink();
        let manager = WorkspaceManager::try_new(WorkspaceStorageMode::Disabled, sink).unwrap();
        manager.shared.poison("first reason");
        manager.shared.poison("second reason");
        match manager.admission() {
            WorkspaceAdmission::Poisoned { reason } => assert_eq!(reason, "first reason"),
            other => panic!("expected Poisoned, got {other:?}"),
        }
        assert_eq!(
            incidents.lock().unwrap().len(),
            2,
            "every poisoning attempt reports an incident, even once already poisoned"
        );
        assert!(!manager.is_healthy());
    }

    #[test]
    fn a_panicking_incident_sink_never_escapes_poison() {
        let base = test_base("panicking-sink");
        let sink: IncidentSink = Arc::new(|_message: &str| panic!("injected sink panic"));
        let manager = WorkspaceManager::new_for_state_tests(&base, 100, sink).unwrap();
        let lease = manager.acquire_capacity(10).unwrap();
        drop(lease);
        assert!(matches!(
            manager.admission(),
            WorkspaceAdmission::Poisoned { .. }
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn check_health_detects_a_deleted_base_without_recreating_it() {
        let base = test_base("health-check-deleted-base");
        let (sink, _incidents) = recording_sink();
        let manager = WorkspaceManager::new_for_state_tests(&base, 1 << 30, sink).unwrap();
        std::fs::remove_dir_all(&base).expect("remove the base out from under the manager");
        let result = manager.check_health();
        assert!(
            result.is_err(),
            "a deleted base must fail health, not be silently recreated"
        );
        assert!(
            !base.exists(),
            "check_health must never recreate the base directory"
        );
        assert!(matches!(
            manager.admission(),
            WorkspaceAdmission::Poisoned { .. }
        ));
    }

    #[test]
    fn check_health_detects_a_replaced_base_even_when_the_replacement_looks_fine() {
        let base = test_base("health-check-replaced-base");
        let (sink, _incidents) = recording_sink();
        let manager = WorkspaceManager::new_for_state_tests(&base, 1 << 30, sink).unwrap();
        std::fs::remove_dir_all(&base).unwrap();
        std::fs::create_dir_all(&base).expect("recreate a replacement directory at the same path");
        let result = manager.check_health();
        assert!(
            matches!(result, Err(WorkspaceManagerError::BaseReplaced { .. })),
            "a same-path replacement directory must be caught by the identity check, got {result:?}"
        );
        assert!(matches!(
            manager.admission(),
            WorkspaceAdmission::Poisoned { .. }
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn require_locked_identity_rejects_a_storage_base_dir_mismatch() {
        let base = test_base("identity-storage-mismatch-locked");
        let other = test_base("identity-storage-mismatch-divergent");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let locked_identity = path_identity(&base).unwrap();

        assert!(check_path_matches_locked_identity(locked_identity, &base).is_ok());
        let result = require_locked_identity(locked_identity, &base, &other);
        assert!(
            matches!(result, Err(WorkspaceManagerError::BaseReplaced { .. })),
            "a diverged storage base dir must be caught even when base_dir itself still matches, \
             got {result:?}"
        );

        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&other);
    }

    #[test]
    fn a_reentrant_sink_through_check_health_never_deadlocks() {
        let base = test_base("health-check-reentrant-sink");
        let manager_slot: Arc<std::sync::OnceLock<WorkspaceManager>> =
            Arc::new(std::sync::OnceLock::new());
        let slot_for_sink = Arc::downgrade(&manager_slot);
        let sink: IncidentSink = Arc::new(move |_message: &str| {
            if let Some(slot) = slot_for_sink.upgrade() {
                if let Some(manager) = slot.get() {
                    let _ = manager.admission();
                }
            }
        });
        let manager = WorkspaceManager::new_for_state_tests(&base, 1 << 30, sink).unwrap();
        manager_slot
            .set(manager)
            .unwrap_or_else(|_| panic!("slot must still be empty"));
        let manager = manager_slot.get().unwrap();
        std::fs::remove_dir_all(&base).expect("remove the base out from under the manager");
        let result = manager.check_health();
        assert!(result.is_err(), "a deleted base must still fail health");
        assert!(matches!(
            manager.admission(),
            WorkspaceAdmission::Poisoned { .. }
        ));
    }

    #[test]
    fn dropping_the_manager_while_a_lease_is_outstanding_keeps_the_lock_held() {
        let base = test_base("lock-outlives-manager-via-lease");
        let (sink, _incidents) = recording_sink();
        let manager = WorkspaceManager::new_for_state_tests(&base, 1 << 30, sink).unwrap();
        let lease = manager.acquire_capacity(10).unwrap();
        drop(manager);

        let (second_sink, _second_incidents) = recording_sink();
        let second_attempt = WorkspaceManager::new_for_state_tests(&base, 1 << 30, second_sink);
        match second_attempt {
            Err(WorkspaceManagerError::AlreadyLocked { .. }) => {}
            Err(other) => panic!(
                "expected AlreadyLocked while the first manager's lease is still outstanding, \
                 got a different error: {other:?}"
            ),
            Ok(_) => panic!(
                "expected a second manager over the same base to be refused while the first \
                 manager's lease is still outstanding, but it succeeded"
            ),
        }

        drop(lease);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn create_workspace_refuses_a_capacity_lease_from_a_different_manager() {
        let base_a = test_base("create-wrong-manager-a");
        let base_b = test_base("create-wrong-manager-b");
        let (sink_a, _log_a) = recording_sink();
        let (sink_b, _log_b) = recording_sink();
        let manager_a = WorkspaceManager::new_for_state_tests(&base_a, 100, sink_a).unwrap();
        let manager_b = WorkspaceManager::new_for_state_tests(&base_b, 100, sink_b).unwrap();
        let capacity_from_b = manager_b.acquire_capacity(10).unwrap();
        let result = manager_a.create_workspace("job-1", 10, 1000, 1000, capacity_from_b);
        let Err(WorkspaceProvisionError::Refused(refusal)) = result else {
            panic!("expected a WrongManager refusal, got {result:?}");
        };
        assert!(matches!(
            refusal,
            WorkspaceRequestRefusal::WrongManager { .. }
        ));
        assert_eq!(manager_b.capacity_used_bytes(), 10);
        refusal.into_capacity().release();
        assert_eq!(manager_b.capacity_used_bytes(), 0);
        assert!(
            manager_b.is_healthy(),
            "handing the lease back and releasing it normally must not poison its real owner"
        );
        let _ = std::fs::remove_dir_all(&base_a);
        let _ = std::fs::remove_dir_all(&base_b);
    }

    #[test]
    fn create_workspace_refuses_a_capacity_lease_whose_bytes_disagree_with_quota() {
        let base = test_base("create-capacity-mismatch");
        let (sink, _log) = recording_sink();
        let manager = WorkspaceManager::new_for_state_tests(&base, 100, sink).unwrap();
        let capacity = manager.acquire_capacity(10).unwrap();
        let result = manager.create_workspace("job-1", 20, 1000, 1000, capacity);
        let Err(WorkspaceProvisionError::Refused(refusal)) = result else {
            panic!("expected a CapacityMismatch refusal, got {result:?}");
        };
        assert!(matches!(
            refusal,
            WorkspaceRequestRefusal::CapacityMismatch {
                requested: 20,
                leased: 10,
                ..
            }
        ));
        assert_eq!(
            manager.capacity_used_bytes(),
            10,
            "a mismatched-quota refusal must not touch the caller's own capacity accounting"
        );
        refusal.into_capacity().release();
        assert_eq!(manager.capacity_used_bytes(), 0);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn create_workspace_refuses_when_disabled() {
        let base = test_base("create-disabled-donor");
        let (donor_sink, _donor_log) = recording_sink();
        let donor = WorkspaceManager::new_for_state_tests(&base, 100, donor_sink).unwrap();
        let lease = donor.acquire_capacity(10).unwrap();

        let (sink, _log) = recording_sink();
        let manager = WorkspaceManager::try_new(WorkspaceStorageMode::Disabled, sink).unwrap();
        let result = manager.create_workspace("job-1", 10, 1000, 1000, lease);
        let Err(WorkspaceProvisionError::Refused(refusal)) = result else {
            panic!("expected a Disabled refusal, got {result:?}");
        };
        assert!(matches!(refusal, WorkspaceRequestRefusal::Disabled { .. }));
        refusal.into_capacity().release();
        assert_eq!(donor.capacity_used_bytes(), 0);
        assert!(donor.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn create_workspace_refuses_an_already_active_job_key() {
        let base = test_base("create-already-active");
        let (sink, _log) = recording_sink();
        let manager = WorkspaceManager::new_for_state_tests(&base, 100, sink).unwrap();
        manager
            .shared
            .lock_state()
            .active_job_ids
            .insert("job-1".to_string());
        let capacity = manager.acquire_capacity(10).unwrap();
        let result = manager.create_workspace("job-1", 10, 1000, 1000, capacity);
        let Err(WorkspaceProvisionError::Refused(refusal)) = result else {
            panic!("expected a JobAlreadyActive refusal, got {result:?}");
        };
        assert!(matches!(
            &refusal,
            WorkspaceRequestRefusal::JobAlreadyActive { job_key, .. } if job_key == "job-1"
        ));
        refusal.into_capacity().release();
        assert_eq!(manager.capacity_used_bytes(), 0);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn create_workspace_refuses_when_poisoned() {
        let base = test_base("create-poisoned");
        let (sink, _log) = recording_sink();
        let manager = WorkspaceManager::new_for_state_tests(&base, 100, sink).unwrap();
        let capacity = manager.acquire_capacity(10).unwrap();
        let abandoned = manager.acquire_capacity(10).unwrap();
        drop(abandoned);
        assert!(!manager.is_healthy());
        let result = manager.create_workspace("job-1", 10, 1000, 1000, capacity);
        let Err(WorkspaceProvisionError::Refused(refusal)) = result else {
            panic!("expected a Poisoned refusal, got {result:?}");
        };
        assert!(matches!(refusal, WorkspaceRequestRefusal::Poisoned { .. }));
        refusal.into_capacity().release();
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn apply_create_result_releases_capacity_on_a_recoverable_failure_without_poisoning() {
        let base = test_base("apply-create-recoverable-failure");
        let (sink, _log) = recording_sink();
        let manager = WorkspaceManager::new_for_state_tests(&base, 100, sink).unwrap();
        let capacity = manager.acquire_capacity(10).unwrap();
        let state = manager.shared.lock_state();
        let result = manager.apply_create_result(
            state,
            "job-1",
            capacity,
            Err(WorkspaceStorageError::SubvolumeCreateFailed {
                path: base.join("job-1"),
                stderr: "injected failure".to_string(),
            }),
        );
        assert!(matches!(
            result,
            Err(WorkspaceProvisionError::Storage(
                WorkspaceStorageError::SubvolumeCreateFailed { .. }
            ))
        ));
        assert_eq!(
            manager.capacity_used_bytes(),
            0,
            "a recoverable provisioning failure must release the capacity back to the pool"
        );
        assert!(manager.is_healthy());
        assert!(
            !manager.active_job_ids().contains("job-1"),
            "a failed create must never leave the job key marked active"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn apply_create_result_retains_capacity_and_poisons_on_an_unrecoverable_leak() {
        let base = test_base("apply-create-leak");
        let (sink, _log) = recording_sink();
        let manager = WorkspaceManager::new_for_state_tests(&base, 100, sink).unwrap();
        let capacity = manager.acquire_capacity(10).unwrap();
        let state = manager.shared.lock_state();
        let result = manager.apply_create_result(
            state,
            "job-1",
            capacity,
            Err(WorkspaceStorageError::UnrecoverableLeak {
                path: base.join("job-1"),
                subvol_id: None,
                provisioning_error: "injected provisioning error".to_string(),
                cleanup_error: "injected cleanup error".to_string(),
            }),
        );
        assert!(matches!(
            result,
            Err(WorkspaceProvisionError::Storage(
                WorkspaceStorageError::UnrecoverableLeak { .. }
            ))
        ));
        assert_eq!(
            manager.capacity_used_bytes(),
            10,
            "an UnrecoverableLeak must retain (never silently free) the capacity - the subvolume \
             may still exist"
        );
        assert!(!manager.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn apply_create_result_succeeds_and_tracks_the_job_key() {
        let base = test_base("apply-create-success");
        let (sink, _log) = recording_sink();
        let manager = WorkspaceManager::new_for_state_tests(&base, 100, sink).unwrap();
        let capacity = manager.acquire_capacity(10).unwrap();
        let state = manager.shared.lock_state();
        let fake = PreparedWorkspace::for_tests(base.join("job-1"), 42, base.clone());
        let workspace = manager
            .apply_create_result(state, "job-1", capacity, Ok(fake))
            .expect("an injected Ok outcome must succeed");
        assert_eq!(workspace.job_key(), "job-1");
        assert_eq!(workspace.capacity_bytes().unwrap(), 10);
        assert!(manager.active_job_ids().contains("job-1"));
        assert!(manager.is_healthy());
        workspace.dismantle_for_tests();
        assert_eq!(manager.capacity_used_bytes(), 0);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn abandoning_a_managed_workspace_from_the_seam_poisons_with_exactly_one_incident() {
        let base = test_base("apply-create-abandon");
        let (sink, incidents) = recording_sink();
        let manager = WorkspaceManager::new_for_state_tests(&base, 100, sink).unwrap();
        let capacity = manager.acquire_capacity(10).unwrap();
        let state = manager.shared.lock_state();
        let fake = PreparedWorkspace::for_tests(base.join("job-1"), 42, base.clone());
        let workspace = manager
            .apply_create_result(state, "job-1", capacity, Ok(fake))
            .unwrap();
        drop(workspace);
        assert!(!manager.is_healthy());
        let log = incidents.lock().unwrap();
        assert_eq!(log.len(), 1, "expected exactly one incident, got {log:?}");
        assert!(log[0].contains("job-1"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn apply_delete_result_abandons_capacity_and_leaves_the_active_entry_on_failure() {
        let base = test_base("apply-delete-failure");
        let (sink, _log) = recording_sink();
        let manager = WorkspaceManager::new_for_state_tests(&base, 100, sink).unwrap();
        manager
            .shared
            .lock_state()
            .active_job_ids
            .insert("job-1".to_string());
        let capacity = manager.acquire_capacity(10).unwrap();
        let state = manager.shared.lock_state();
        let result = manager.apply_delete_result(
            state,
            "job-1",
            capacity,
            Err(WorkspaceStorageError::DeleteFailed {
                subvol_id: 42,
                stderr: "injected delete failure".to_string(),
            }),
        );
        assert!(matches!(result, Err(DeleteWorkspaceError::Storage(_))));
        assert_eq!(
            manager.capacity_used_bytes(),
            10,
            "a delete/sync failure must retain (never silently free) the capacity"
        );
        assert!(
            manager.active_job_ids().contains("job-1"),
            "a delete/sync failure must leave the active-job entry in place, not confirmed absent"
        );
        assert!(!manager.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn apply_delete_result_succeeds_and_clears_bookkeeping() {
        let base = test_base("apply-delete-success");
        let (sink, _log) = recording_sink();
        let manager = WorkspaceManager::new_for_state_tests(&base, 100, sink).unwrap();
        manager
            .shared
            .lock_state()
            .active_job_ids
            .insert("job-1".to_string());
        let capacity = manager.acquire_capacity(10).unwrap();
        let state = manager.shared.lock_state();
        let result = manager.apply_delete_result(state, "job-1", capacity, Ok(()));
        assert!(result.is_ok());
        assert_eq!(manager.capacity_used_bytes(), 0);
        assert!(!manager.active_job_ids().contains("job-1"));
        assert!(manager.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn apply_delete_result_surfaces_the_invariant_violation_when_the_job_key_was_never_active() {
        let base = test_base("apply-delete-invariant");
        let (sink, _log) = recording_sink();
        let manager = WorkspaceManager::new_for_state_tests(&base, 100, sink).unwrap();
        let capacity = manager.acquire_capacity(10).unwrap();
        let state = manager.shared.lock_state();
        let result = manager.apply_delete_result(state, "job-1", capacity, Ok(()));
        assert!(matches!(
            result,
            Err(DeleteWorkspaceError::InternalInvariantViolated { .. })
        ));
        assert_eq!(
            manager.capacity_used_bytes(),
            0,
            "capacity must still be released - the disk deletion itself genuinely succeeded"
        );
        assert!(!manager.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn apply_create_result_surfaces_the_invariant_violation_when_the_job_key_was_already_active() {
        let base = test_base("apply-create-invariant");
        let (sink, _log) = recording_sink();
        let manager = WorkspaceManager::new_for_state_tests(&base, 100, sink).unwrap();
        manager
            .shared
            .lock_state()
            .active_job_ids
            .insert("job-1".to_string());
        let capacity = manager.acquire_capacity(10).unwrap();
        let state = manager.shared.lock_state();
        let fake = PreparedWorkspace::for_tests(base.join("job-1"), 42, base.clone());
        let result = manager.apply_create_result(state, "job-1", capacity, Ok(fake));
        assert!(matches!(
            result,
            Err(WorkspaceProvisionError::InternalInvariantViolated { .. })
        ));
        let error = result.unwrap_err();
        assert!(error.to_string().contains("job-1"));
        let workspace = error.into_workspace_after_invariant_violation().unwrap();
        assert_eq!(
            manager.capacity_used_bytes(),
            10,
            "the capacity must still be tracked as used - the real subvolume now exists on disk"
        );
        assert!(!manager.is_healthy());
        assert_eq!(workspace.job_key(), "job-1");
        workspace.dismantle_for_tests();
        assert_eq!(manager.capacity_used_bytes(), 0);
        let _ = std::fs::remove_dir_all(&base);
    }

    fn ephemeral_disk_available(base: &Path) -> bool {
        match WorkspaceStorage::open(base) {
            Ok(_) => {}
            Err(WorkspaceStorageError::NotBtrfs { .. })
            | Err(WorkspaceStorageError::QuotaNotEnforcing { .. }) => {
                eprintln!(
                    "[workspace_manager] SKIP: no Btrfs+enforcing-quota support on this host"
                );
                return false;
            }
            Err(other) => panic!("WorkspaceStorage::open failed unexpectedly: {other}"),
        };
        match crate::workspace_storage::probe_qgroup_privilege(base) {
            Ok(true) => true,
            Ok(false) => {
                eprintln!(
                    "[workspace_manager] SKIP: this test process lacks CAP_SYS_ADMIN for qgroup \
                     operations"
                );
                false
            }
            Err(e) => panic!("qgroup privilege probe failed unexpectedly: {e}"),
        }
    }

    #[test]
    #[cfg_attr(
        not(feature = "privileged-host-tests"),
        ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) - run on the host lane with --features privileged-host-tests"
    )]
    fn boot_reconciliation_deletes_a_preexisting_orphan_before_reporting_healthy() {
        let base = btrfs_test_base("boot-reconcile");
        if !ephemeral_disk_available(&base) {
            return;
        }
        {
            let mut storage = WorkspaceStorage::open(&base).unwrap();
            let (euid, egid) = unsafe { (libc::geteuid(), libc::getegid()) };
            let prepared = storage
                .create_workspace("orphaned-job", 8 << 20, euid, egid)
                .expect("create a real orphan subvolume to reconcile");
            let path = prepared.host_path().to_path_buf();
            std::mem::forget(prepared);
            assert!(
                path.exists(),
                "the orphan subvolume must really exist before reconciliation"
            );
        }
        let (sink, _incidents) = recording_sink();
        let manager = WorkspaceManager::try_new(
            WorkspaceStorageMode::EphemeralDisk {
                base_dir: base.clone(),
                host_capacity_bytes: 1 << 30,
            },
            sink,
        )
        .expect("boot reconciliation must succeed and open Healthy");
        assert!(manager.is_healthy());
        assert!(
            !base.join("orphaned-job").exists(),
            "boot reconciliation must have deleted the pre-existing orphan"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    #[cfg_attr(
        not(feature = "privileged-host-tests"),
        ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) - run on the host lane with --features privileged-host-tests"
    )]
    fn check_health_succeeds_against_a_real_still_healthy_backend() {
        let base = btrfs_test_base("health-check-real-happy-path");
        if !ephemeral_disk_available(&base) {
            return;
        }
        let (sink, _incidents) = recording_sink();
        let manager = WorkspaceManager::try_new(
            WorkspaceStorageMode::EphemeralDisk {
                base_dir: base.clone(),
                host_capacity_bytes: 1 << 30,
            },
            sink,
        )
        .unwrap();
        assert!(manager.check_health().is_ok());
        assert!(manager.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    #[cfg_attr(
        not(feature = "privileged-host-tests"),
        ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) - run on the host lane with --features privileged-host-tests"
    )]
    fn create_workspace_then_delete_workspace_releases_capacity_and_clears_active_job_id() {
        let base = btrfs_test_base("create-then-delete");
        if !ephemeral_disk_available(&base) {
            return;
        }
        let (sink, _incidents) = recording_sink();
        let manager = WorkspaceManager::try_new(
            WorkspaceStorageMode::EphemeralDisk {
                base_dir: base.clone(),
                host_capacity_bytes: 1 << 30,
            },
            sink,
        )
        .unwrap();
        let (euid, egid) = unsafe { (libc::geteuid(), libc::getegid()) };
        let capacity = manager.acquire_capacity(8 << 20).unwrap();
        let workspace = manager
            .create_workspace("real-job", 8 << 20, euid, egid, capacity)
            .expect("create_workspace must succeed against a real, privileged Btrfs backend");
        let host_path = workspace.host_path().unwrap().to_path_buf();
        assert!(
            host_path.exists(),
            "the workspace must really exist on disk"
        );
        assert_eq!(workspace.job_key(), "real-job");
        assert_eq!(workspace.capacity_bytes().unwrap(), 8 << 20);
        assert!(manager.active_job_ids().contains("real-job"));
        assert_eq!(manager.capacity_used_bytes(), 8 << 20);

        manager
            .delete_workspace(workspace)
            .expect("delete_workspace must succeed against a real, privileged Btrfs backend");
        assert!(
            !host_path.exists(),
            "the workspace subvolume must be gone from disk after delete_workspace"
        );
        assert!(!manager.active_job_ids().contains("real-job"));
        assert_eq!(manager.capacity_used_bytes(), 0);
        assert!(manager.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    #[cfg_attr(
        not(feature = "privileged-host-tests"),
        ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) - run on the host lane with --features privileged-host-tests"
    )]
    fn dropping_a_managed_workspace_without_deleting_poisons_the_manager_with_one_incident() {
        let base = btrfs_test_base("drop-without-delete");
        if !ephemeral_disk_available(&base) {
            return;
        }
        let (sink, incidents) = recording_sink();
        let manager = WorkspaceManager::try_new(
            WorkspaceStorageMode::EphemeralDisk {
                base_dir: base.clone(),
                host_capacity_bytes: 1 << 30,
            },
            sink,
        )
        .unwrap();
        let (euid, egid) = unsafe { (libc::geteuid(), libc::getegid()) };
        let capacity = manager.acquire_capacity(8 << 20).unwrap();
        let workspace = manager
            .create_workspace("abandoned-job", 8 << 20, euid, egid, capacity)
            .expect("create_workspace must succeed against a real, privileged Btrfs backend");
        let host_path = workspace.host_path().unwrap().to_path_buf();
        drop(workspace);

        assert!(
            !manager.is_healthy(),
            "an abandoned ManagedWorkspace must poison the manager"
        );
        let log = incidents.lock().unwrap();
        assert_eq!(
            log.len(),
            1,
            "exactly ONE comprehensive incident must fire - not a second, generic \
             CapacityLease-abandonment message on top of it: {log:?}"
        );
        assert!(log[0].contains("abandoned-job"));
        drop(log);
        assert!(host_path.exists());

        drop(manager);
        let (sink2, _incidents2) = recording_sink();
        let fresh = WorkspaceManager::try_new(
            WorkspaceStorageMode::EphemeralDisk {
                base_dir: base.clone(),
                host_capacity_bytes: 1 << 30,
            },
            sink2,
        )
        .expect("a fresh manager's own boot reconciliation must clean up the orphan and succeed");
        assert!(fresh.is_healthy());
        assert!(
            !host_path.exists(),
            "boot reconciliation must have deleted the abandoned subvolume for real"
        );
        drop(fresh);
        let _ = std::fs::remove_dir_all(&base);
    }
}
