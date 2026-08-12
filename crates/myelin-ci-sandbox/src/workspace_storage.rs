use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fmt;
use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BTRFS_BIN: &str = "/usr/bin/btrfs";
const CHOWN_BIN: &str = "/usr/bin/chown";
const BTRFS_FIRST_FREE_OBJECTID: u64 = 256;
const BTRFS_NAME_MAX: usize = 255;

#[derive(Debug)]
pub enum WorkspaceStorageError {
    NotBtrfs {
        base_dir: PathBuf,
    },
    QuotaNotEnforcing {
        base_dir: PathBuf,
        status: String,
    },
    InvalidJobId {
        job_id: String,
    },
    ZeroQuota,
    SubvolumeCreateFailed {
        path: PathBuf,
        stderr: String,
    },
    IdentityReadFailed {
        path: PathBuf,
        reason: String,
    },
    QuotaLimitFailed {
        path: PathBuf,
        stderr: String,
    },
    QuotaNotAsserted {
        path: PathBuf,
        requested: u64,
        observed: Option<u64>,
    },
    OwnershipFailed {
        path: PathBuf,
        reason: String,
    },
    UnrecoverableLeak {
        path: PathBuf,
        subvol_id: Option<u64>,
        provisioning_error: String,
        cleanup_error: String,
    },
    DeleteFailed {
        subvol_id: u64,
        stderr: String,
    },
    SyncPending {
        subvol_id: u64,
        reason: String,
    },
    WrongStorage {
        expected_base: PathBuf,
        actual_base: PathBuf,
    },
    BackendMismatch {
        detail: String,
    },
    DirectoryAbsenceUnproven {
        path: PathBuf,
        reason: String,
    },
    DirectoryQuotaExceeded {
        path: PathBuf,
        quota_bytes: u64,
        would_be_bytes: u64,
    },
    ListFailed {
        base_dir: PathBuf,
        reason: String,
    },
    UnexpectedEntry {
        path: PathBuf,
        reason: String,
    },
    Io {
        path: PathBuf,
        reason: String,
    },
}

impl fmt::Display for WorkspaceStorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotBtrfs { base_dir } => {
                write!(f, "workspace base {base_dir:?} is not a Btrfs filesystem")
            }
            Self::QuotaNotEnforcing { base_dir, status } => write!(
                f,
                "Btrfs quota is not in a fully-enforcing state on {base_dir:?}: {status}"
            ),
            Self::InvalidJobId { job_id } => {
                write!(f, "job id {job_id:?} is not a safe path component")
            }
            Self::ZeroQuota => write!(f, "quota_bytes must be > 0"),
            Self::SubvolumeCreateFailed { path, stderr } => {
                write!(f, "btrfs subvolume create {path:?} failed: {stderr}")
            }
            Self::IdentityReadFailed { path, reason } => {
                write!(f, "read subvolume id of {path:?} failed: {reason}")
            }
            Self::QuotaLimitFailed { path, stderr } => {
                write!(f, "btrfs qgroup limit on {path:?} failed: {stderr}")
            }
            Self::QuotaNotAsserted {
                path,
                requested,
                observed,
            } => write!(
                f,
                "quota postcondition failed on {path:?}: requested {requested}, observed {observed:?}"
            ),
            Self::OwnershipFailed { path, reason } => {
                write!(f, "set ownership on {path:?} failed: {reason}")
            }
            Self::UnrecoverableLeak {
                path,
                subvol_id,
                provisioning_error,
                cleanup_error,
            } => write!(
                f,
                "UNRECOVERABLE workspace leak at {path:?} (subvol_id={subvol_id:?}) - \
                 provisioning failed ({provisioning_error}) AND cleanup ALSO failed \
                 ({cleanup_error}) - manual reconciliation required, do not retry silently"
            ),
            Self::DeleteFailed { subvol_id, stderr } => {
                write!(f, "btrfs subvolume delete --subvolid {subvol_id} failed: {stderr}")
            }
            Self::SyncPending { subvol_id, reason } => write!(
                f,
                "subvolid {subvol_id} was deleted but sync has not completed ({reason}) - \
                 retry via retry_pending_sync({subvol_id}), no capability needed"
            ),
            Self::WrongStorage {
                expected_base,
                actual_base,
            } => write!(
                f,
                "refusing: this capability was minted against {expected_base:?}, not the \
                 storage handle's own base {actual_base:?}"
            ),
            Self::BackendMismatch { detail } => {
                write!(f, "cross-backend workspace capability refused: {detail}")
            }
            Self::DirectoryAbsenceUnproven { path, reason } => write!(
                f,
                "deterministic-directory workspace deletion left absence UNPROVEN at {path:?}: \
                 {reason}"
            ),
            Self::DirectoryQuotaExceeded {
                path,
                quota_bytes,
                would_be_bytes,
            } => write!(
                f,
                "byte-accounted test-quota write refused at {path:?}: {would_be_bytes} bytes would \
                 exceed the per-job quota of {quota_bytes} bytes"
            ),
            Self::ListFailed { base_dir, reason } => {
                write!(f, "list workspace base {base_dir:?} failed: {reason}")
            }
            Self::UnexpectedEntry { path, reason } => {
                write!(f, "unexpected non-subvolume entry {path:?}: {reason}")
            }
            Self::Io { path, reason } => write!(f, "{path:?}: {reason}"),
        }
    }
}

impl std::error::Error for WorkspaceStorageError {}

#[derive(Debug)]
enum WorkspaceIdentity {
    Btrfs {
        subvol_id: u64,
    },
    #[cfg(any(test, feature = "test-support"))]
    Directory {
        device: u64,
        inode: u64,
        quota_bytes: u64,
    },
}

#[derive(Debug)]
pub struct PreparedWorkspace {
    host_path: PathBuf,
    identity: Box<WorkspaceIdentity>,
    minted_from: PathBuf,
}

impl PreparedWorkspace {
    pub fn host_path(&self) -> &Path {
        &self.host_path
    }

    pub fn subvol_id(&self) -> u64 {
        match self.identity.as_ref() {
            WorkspaceIdentity::Btrfs { subvol_id } => *subvol_id,
            #[cfg(any(test, feature = "test-support"))]
            WorkspaceIdentity::Directory { .. } => {
                panic!("subvol_id() is meaningless for a deterministic-directory test workspace")
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn for_tests(host_path: PathBuf, subvol_id: u64, minted_from: PathBuf) -> Self {
        PreparedWorkspace {
            host_path,
            identity: Box::new(WorkspaceIdentity::Btrfs { subvol_id }),
            minted_from,
        }
    }
}

#[derive(Debug)]
pub struct OrphanCandidate {
    path: PathBuf,
    identity: Box<WorkspaceIdentity>,
    minted_from: PathBuf,
}

impl OrphanCandidate {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn subvol_id(&self) -> u64 {
        match self.identity.as_ref() {
            WorkspaceIdentity::Btrfs { subvol_id } => *subvol_id,
            #[cfg(any(test, feature = "test-support"))]
            WorkspaceIdentity::Directory { .. } => {
                panic!("subvol_id() is meaningless for a deterministic-directory test orphan")
            }
        }
    }
}

#[derive(Debug)]
pub struct WorkspaceStorage {
    canonical_base: PathBuf,
    fs_anchor: PathBuf,
}

impl WorkspaceStorage {
    pub fn open(base_dir: &Path) -> Result<Self, WorkspaceStorageError> {
        if !exists_or_error(base_dir)? {
            std::fs::create_dir_all(base_dir).map_err(|e| WorkspaceStorageError::Io {
                path: base_dir.to_path_buf(),
                reason: format!("create workspace base dir: {e}"),
            })?;
        }
        let canonical_base =
            std::fs::canonicalize(base_dir).map_err(|e| WorkspaceStorageError::Io {
                path: base_dir.to_path_buf(),
                reason: format!("canonicalize: {e}"),
            })?;
        assert_base_dir_exclusively_owned(&canonical_base)?;
        let fs_anchor = btrfs_mountpoint_of(&canonical_base)?;
        assert_quota_enforcing(&canonical_base)?;
        Ok(Self {
            canonical_base,
            fs_anchor,
        })
    }

    pub fn base_dir(&self) -> &Path {
        &self.canonical_base
    }

    pub fn check_health(&self) -> Result<(), WorkspaceStorageError> {
        assert_base_dir_exclusively_owned(&self.canonical_base)?;
        assert_quota_enforcing(&self.canonical_base)?;
        Ok(())
    }

    pub fn create_workspace(
        &mut self,
        job_id: &str,
        quota_bytes: u64,
        owner_uid: u32,
        owner_gid: u32,
    ) -> Result<PreparedWorkspace, WorkspaceStorageError> {
        validate_job_id(job_id)?;
        if quota_bytes == 0 {
            return Err(WorkspaceStorageError::ZeroQuota);
        }
        let path = self.canonical_base.join(job_id);

        let create = run_btrfs(&[
            OsStr::new("subvolume"),
            OsStr::new("create"),
            path.as_os_str(),
        ])?;
        if !create.status.success() {
            return Err(WorkspaceStorageError::SubvolumeCreateFailed {
                path: path.clone(),
                stderr: stderr_of(&create),
            });
        }

        let subvol_id = match read_subvol_id(&path) {
            Ok(id) => id,
            Err(reason) => {
                if let Err(cleanup_err) = delete_by_path_unverified(&path) {
                    return Err(WorkspaceStorageError::UnrecoverableLeak {
                        path,
                        subvol_id: None,
                        provisioning_error: format!("read subvolume id: {reason}"),
                        cleanup_error: cleanup_err,
                    });
                }
                return Err(WorkspaceStorageError::IdentityReadFailed { path, reason });
            }
        };

        if let Err(provisioning_error) = self.apply_and_verify_quota(subvol_id, quota_bytes) {
            if let Err(cleanup_err) = self.delete_by_id(subvol_id) {
                return Err(WorkspaceStorageError::UnrecoverableLeak {
                    path,
                    subvol_id: Some(subvol_id),
                    provisioning_error: provisioning_error.to_string(),
                    cleanup_error: cleanup_err.to_string(),
                });
            }
            return Err(provisioning_error);
        }

        if let Err(e) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)) {
            let reason = format!("chmod 0755: {e}");
            if let Err(cleanup_err) = self.delete_by_id(subvol_id) {
                return Err(WorkspaceStorageError::UnrecoverableLeak {
                    path,
                    subvol_id: Some(subvol_id),
                    provisioning_error: reason,
                    cleanup_error: cleanup_err.to_string(),
                });
            }
            return Err(WorkspaceStorageError::OwnershipFailed { path, reason });
        }
        if let Err(reason) = chown_path(&path, owner_uid, owner_gid) {
            if let Err(cleanup_err) = self.delete_by_id(subvol_id) {
                return Err(WorkspaceStorageError::UnrecoverableLeak {
                    path,
                    subvol_id: Some(subvol_id),
                    provisioning_error: format!("chown: {reason}"),
                    cleanup_error: cleanup_err.to_string(),
                });
            }
            return Err(WorkspaceStorageError::OwnershipFailed { path, reason });
        }

        Ok(PreparedWorkspace {
            host_path: path,
            identity: Box::new(WorkspaceIdentity::Btrfs { subvol_id }),
            minted_from: self.canonical_base.clone(),
        })
    }

    fn apply_and_verify_quota(
        &self,
        subvol_id: u64,
        quota_bytes: u64,
    ) -> Result<(), WorkspaceStorageError> {
        let qgroup_id = format!("0/{subvol_id}");
        let quota_arg = quota_bytes.to_string();
        let limit = run_btrfs(&[
            OsStr::new("qgroup"),
            OsStr::new("limit"),
            OsStr::new(&quota_arg),
            OsStr::new(&qgroup_id),
            self.fs_anchor.as_os_str(),
        ])?;
        if !limit.status.success() {
            return Err(WorkspaceStorageError::QuotaLimitFailed {
                path: self.canonical_base.clone(),
                stderr: stderr_of(&limit),
            });
        }
        let observed = self.read_qgroup_max_referenced(subvol_id)?;
        if observed != Some(quota_bytes) {
            return Err(WorkspaceStorageError::QuotaNotAsserted {
                path: self.canonical_base.clone(),
                requested: quota_bytes,
                observed,
            });
        }
        Ok(())
    }

    fn read_qgroup_max_referenced(
        &self,
        subvol_id: u64,
    ) -> Result<Option<u64>, WorkspaceStorageError> {
        let show = run_btrfs(&[
            OsStr::new("qgroup"),
            OsStr::new("show"),
            OsStr::new("-r"),
            OsStr::new("--raw"),
            self.fs_anchor.as_os_str(),
        ])?;
        if !show.status.success() {
            return Err(WorkspaceStorageError::QuotaLimitFailed {
                path: self.canonical_base.clone(),
                stderr: stderr_of(&show),
            });
        }
        let want_id = format!("0/{subvol_id}");
        for line in stdout_of(&show).lines() {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() >= 4 && cols[0] == want_id {
                return Ok(cols[3].parse::<u64>().ok());
            }
        }
        Ok(None)
    }

    pub fn delete_workspace(
        &mut self,
        prepared: PreparedWorkspace,
    ) -> Result<(), WorkspaceStorageError> {
        assert_same_storage(&self.canonical_base, &prepared.minted_from)?;
        let subvol_id = btrfs_identity_or_backend_mismatch(&prepared.identity)?;
        self.delete_by_id(subvol_id)
    }

    pub fn delete_orphan(
        &mut self,
        candidate: OrphanCandidate,
    ) -> Result<(), WorkspaceStorageError> {
        assert_same_storage(&self.canonical_base, &candidate.minted_from)?;
        let subvol_id = btrfs_identity_or_backend_mismatch(&candidate.identity)?;
        self.delete_by_id(subvol_id)
    }

    pub fn retry_pending_sync(&mut self, subvol_id: u64) -> Result<(), WorkspaceStorageError> {
        self.sync_subvol_id(subvol_id)
    }

    fn delete_by_id(&mut self, subvol_id: u64) -> Result<(), WorkspaceStorageError> {
        let id_arg = subvol_id.to_string();
        let delete = run_btrfs(&[
            OsStr::new("subvolume"),
            OsStr::new("delete"),
            OsStr::new("--subvolid"),
            OsStr::new(&id_arg),
            self.fs_anchor.as_os_str(),
        ])?;
        if !delete.status.success() {
            let stderr = stderr_of(&delete);
            if !(stderr.contains("No such file or directory") || stderr.contains("do not exist")) {
                return Err(WorkspaceStorageError::DeleteFailed { subvol_id, stderr });
            }
        }
        self.sync_subvol_id(subvol_id)
    }

    fn sync_subvol_id(&mut self, subvol_id: u64) -> Result<(), WorkspaceStorageError> {
        let sync = run_btrfs(&[
            OsStr::new("subvolume"),
            OsStr::new("sync"),
            self.fs_anchor.as_os_str(),
            OsStr::new(&subvol_id.to_string()),
        ])?;
        if sync.status.success() {
            Ok(())
        } else {
            Err(WorkspaceStorageError::SyncPending {
                subvol_id,
                reason: stderr_of(&sync),
            })
        }
    }

    pub fn list_orphaned_workspaces(
        &mut self,
        active_job_ids: &BTreeSet<String>,
    ) -> Result<Vec<OrphanCandidate>, WorkspaceStorageError> {
        if !exists_or_error(&self.canonical_base)? {
            return Ok(Vec::new());
        }
        let entries = std::fs::read_dir(&self.canonical_base).map_err(|e| {
            WorkspaceStorageError::ListFailed {
                base_dir: self.canonical_base.clone(),
                reason: e.to_string(),
            }
        })?;
        let mut orphans = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| WorkspaceStorageError::ListFailed {
                base_dir: self.canonical_base.clone(),
                reason: e.to_string(),
            })?;
            let path = entry.path();
            let subvol_id = verify_and_read_subvol_id(&path).map_err(|reason| {
                WorkspaceStorageError::UnexpectedEntry {
                    path: path.clone(),
                    reason,
                }
            })?;
            let name_str = entry.file_name().to_string_lossy().into_owned();
            if active_job_ids.contains(&name_str) {
                continue;
            }
            orphans.push(OrphanCandidate {
                path,
                identity: Box::new(WorkspaceIdentity::Btrfs { subvol_id }),
                minted_from: self.canonical_base.clone(),
            });
        }
        Ok(orphans)
    }
}

fn btrfs_mountpoint_of(canonical_path: &Path) -> Result<PathBuf, WorkspaceStorageError> {
    let mounts =
        std::fs::read_to_string("/proc/mounts").map_err(|e| WorkspaceStorageError::Io {
            path: canonical_path.to_path_buf(),
            reason: format!("read /proc/mounts: {e}"),
        })?;
    let mut best: Option<(usize, &str, &str)> = None;
    for line in mounts.lines() {
        let mut fields = line.split_whitespace();
        let Some(_source) = fields.next() else {
            continue;
        };
        let Some(mount_point) = fields.next() else {
            continue;
        };
        let Some(fstype) = fields.next() else {
            continue;
        };
        if canonical_path.starts_with(mount_point) {
            let len = mount_point.len();
            if best.is_none_or(|(best_len, ..)| len > best_len) {
                best = Some((len, mount_point, fstype));
            }
        }
    }
    match best {
        Some((_, mount_point, "btrfs")) => Ok(PathBuf::from(mount_point)),
        Some(_) | None => Err(WorkspaceStorageError::NotBtrfs {
            base_dir: canonical_path.to_path_buf(),
        }),
    }
}

fn assert_base_dir_exclusively_owned(canonical_base: &Path) -> Result<(), WorkspaceStorageError> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(canonical_base).map_err(|e| WorkspaceStorageError::Io {
        path: canonical_base.to_path_buf(),
        reason: format!("stat: {e}"),
    })?;
    let euid = unsafe { libc::geteuid() };
    let group_or_world_writable = meta.mode() & 0o022 != 0;
    if !meta.is_dir() || meta.uid() != euid || group_or_world_writable {
        return Err(WorkspaceStorageError::Io {
            path: canonical_base.to_path_buf(),
            reason: format!(
                "workspace base must be a directory owned by this process's own uid ({euid}) \
                 with no group/world write bit; found is_dir={} uid={} mode={:o}",
                meta.is_dir(),
                meta.uid(),
                meta.mode() & 0o7777
            ),
        });
    }
    Ok(())
}

fn assert_same_storage(
    self_base: &Path,
    capability_base: &Path,
) -> Result<(), WorkspaceStorageError> {
    if self_base == capability_base {
        Ok(())
    } else {
        Err(WorkspaceStorageError::WrongStorage {
            expected_base: capability_base.to_path_buf(),
            actual_base: self_base.to_path_buf(),
        })
    }
}

fn btrfs_identity_or_backend_mismatch(
    identity: &WorkspaceIdentity,
) -> Result<u64, WorkspaceStorageError> {
    match identity {
        WorkspaceIdentity::Btrfs { subvol_id } => Ok(*subvol_id),
        #[cfg(any(test, feature = "test-support"))]
        WorkspaceIdentity::Directory { .. } => Err(WorkspaceStorageError::BackendMismatch {
            detail: "a deterministic-directory workspace capability was presented to the Btrfs \
                     backend - the two backends' identities are not interchangeable"
                .to_string(),
        }),
    }
}

fn assert_quota_enforcing(canonical_base: &Path) -> Result<(), WorkspaceStorageError> {
    let status = run_btrfs(&[
        OsStr::new("quota"),
        OsStr::new("status"),
        canonical_base.as_os_str(),
    ])?;
    let text = stdout_of(&status);
    let field = |name: &str| {
        text.lines()
            .find(|l| l.trim_start().starts_with(name))
            .map(str::to_owned)
    };
    let enforcing = status.status.success()
        && field("Enabled:").is_some_and(|l| l.contains("yes"))
        && field("Mode:").is_some_and(|l| l.contains("qgroup"))
        && field("Inconsistent:").is_some_and(|l| l.contains("no"))
        && field("Override limits:").is_some_and(|l| l.contains("no"));
    if enforcing {
        Ok(())
    } else {
        Err(WorkspaceStorageError::QuotaNotEnforcing {
            base_dir: canonical_base.to_path_buf(),
            status: if status.status.success() {
                text
            } else {
                stderr_of(&status)
            },
        })
    }
}

fn validate_job_id(job_id: &str) -> Result<(), WorkspaceStorageError> {
    let safe = !job_id.is_empty()
        && job_id.len() <= BTRFS_NAME_MAX
        && job_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if safe {
        Ok(())
    } else {
        Err(WorkspaceStorageError::InvalidJobId {
            job_id: job_id.to_string(),
        })
    }
}

fn is_subvolume_root(path: &Path) -> Result<bool, String> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(path).map_err(|e| format!("stat: {e}"))?;
    if meta.file_type().is_symlink() {
        return Ok(false);
    }
    Ok(meta.is_dir() && meta.ino() == BTRFS_FIRST_FREE_OBJECTID)
}

fn read_subvol_id(path: &Path) -> Result<u64, String> {
    let output = Command::new(BTRFS_BIN)
        .env_clear()
        .args([
            OsStr::new("inspect-internal"),
            OsStr::new("rootid"),
            path.as_os_str(),
        ])
        .output()
        .map_err(|e| format!("spawn btrfs inspect-internal rootid: {e}"))?;
    if !output.status.success() {
        return Err(stderr_of(&output));
    }
    stdout_of(&output)
        .trim()
        .parse::<u64>()
        .map_err(|e| format!("parse rootid output: {e}"))
}

fn verify_and_read_subvol_id(path: &Path) -> Result<u64, String> {
    if !is_subvolume_root(path)? {
        return Err(format!("{path:?} is not a Btrfs subvolume root"));
    }
    read_subvol_id(path)
}

fn delete_by_path_unverified(path: &Path) -> Result<(), String> {
    let delete = Command::new(BTRFS_BIN)
        .env_clear()
        .args([
            OsStr::new("subvolume"),
            OsStr::new("delete"),
            path.as_os_str(),
        ])
        .output()
        .map_err(|e| format!("spawn btrfs subvolume delete: {e}"))?;
    if !delete.status.success() {
        return Err(stderr_of(&delete));
    }
    let parent = path.parent().unwrap_or(path);
    let sync = Command::new(BTRFS_BIN)
        .env_clear()
        .args([
            OsStr::new("subvolume"),
            OsStr::new("sync"),
            parent.as_os_str(),
        ])
        .output()
        .map_err(|e| format!("spawn btrfs subvolume sync: {e}"))?;
    if sync.status.success() {
        Ok(())
    } else {
        Err(stderr_of(&sync))
    }
}

fn chown_path(path: &Path, uid: u32, gid: u32) -> Result<(), String> {
    let output = Command::new(CHOWN_BIN)
        .env_clear()
        .arg(format!("{uid}:{gid}"))
        .arg(path)
        .output()
        .map_err(|e| format!("spawn chown: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(stderr_of(&output))
    }
}

fn run_btrfs(args: &[&OsStr]) -> Result<Output, WorkspaceStorageError> {
    Command::new(BTRFS_BIN)
        .env_clear()
        .args(args)
        .output()
        .map_err(|e| WorkspaceStorageError::Io {
            path: PathBuf::new(),
            reason: format!("spawn btrfs {args:?}: {e}"),
        })
}

#[cfg(test)]
pub(crate) fn probe_qgroup_privilege(base_dir: &Path) -> Result<bool, WorkspaceStorageError> {
    let probe = run_btrfs(&[
        OsStr::new("qgroup"),
        OsStr::new("show"),
        OsStr::new("-r"),
        OsStr::new("--raw"),
        base_dir.as_os_str(),
    ])?;
    if probe.status.success() {
        Ok(true)
    } else {
        let stderr = stderr_of(&probe);
        if stderr.contains("Operation not permitted") {
            Ok(false)
        } else {
            Err(WorkspaceStorageError::Io {
                path: base_dir.to_path_buf(),
                reason: format!(
                    "unexpected qgroup-show probe failure (not a privilege gap): {stderr}"
                ),
            })
        }
    }
}

#[derive(Debug)]
pub(crate) enum WorkspaceStorageBackend {
    Btrfs(WorkspaceStorage),
    #[cfg(any(test, feature = "test-support"))]
    DeterministicDirectoryForTests(DirectoryWorkspaceStorage),
}

impl WorkspaceStorageBackend {
    pub(crate) fn base_dir(&self) -> &Path {
        match self {
            Self::Btrfs(storage) => storage.base_dir(),
            #[cfg(any(test, feature = "test-support"))]
            Self::DeterministicDirectoryForTests(storage) => storage.base_dir(),
        }
    }

    pub(crate) fn check_health(&self) -> Result<(), WorkspaceStorageError> {
        match self {
            Self::Btrfs(storage) => storage.check_health(),
            #[cfg(any(test, feature = "test-support"))]
            Self::DeterministicDirectoryForTests(storage) => storage.check_health(),
        }
    }

    pub(crate) fn create_workspace(
        &mut self,
        job_id: &str,
        quota_bytes: u64,
        owner_uid: u32,
        owner_gid: u32,
    ) -> Result<PreparedWorkspace, WorkspaceStorageError> {
        match self {
            Self::Btrfs(storage) => {
                storage.create_workspace(job_id, quota_bytes, owner_uid, owner_gid)
            }
            #[cfg(any(test, feature = "test-support"))]
            Self::DeterministicDirectoryForTests(storage) => {
                storage.create_workspace(job_id, quota_bytes, owner_uid, owner_gid)
            }
        }
    }

    pub(crate) fn delete_workspace(
        &mut self,
        prepared: PreparedWorkspace,
    ) -> Result<(), WorkspaceStorageError> {
        match self {
            Self::Btrfs(storage) => storage.delete_workspace(prepared),
            #[cfg(any(test, feature = "test-support"))]
            Self::DeterministicDirectoryForTests(storage) => storage.delete_workspace(prepared),
        }
    }

    pub(crate) fn delete_orphan(
        &mut self,
        candidate: OrphanCandidate,
    ) -> Result<(), WorkspaceStorageError> {
        match self {
            Self::Btrfs(storage) => storage.delete_orphan(candidate),
            #[cfg(any(test, feature = "test-support"))]
            Self::DeterministicDirectoryForTests(storage) => storage.delete_orphan(candidate),
        }
    }

    pub(crate) fn retry_pending_sync(
        &mut self,
        subvol_id: u64,
    ) -> Result<(), WorkspaceStorageError> {
        match self {
            Self::Btrfs(storage) => storage.retry_pending_sync(subvol_id),
            #[cfg(any(test, feature = "test-support"))]
            Self::DeterministicDirectoryForTests(_) => Ok(()),
        }
    }

    pub(crate) fn list_orphaned_workspaces(
        &mut self,
        active_job_ids: &BTreeSet<String>,
    ) -> Result<Vec<OrphanCandidate>, WorkspaceStorageError> {
        match self {
            Self::Btrfs(storage) => storage.list_orphaned_workspaces(active_job_ids),
            #[cfg(any(test, feature = "test-support"))]
            Self::DeterministicDirectoryForTests(storage) => {
                storage.list_orphaned_workspaces(active_job_ids)
            }
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
fn directory_identity_or_backend_mismatch(
    identity: &WorkspaceIdentity,
) -> Result<(u64, u64), WorkspaceStorageError> {
    match identity {
        WorkspaceIdentity::Directory { device, inode, .. } => Ok((*device, *inode)),
        WorkspaceIdentity::Btrfs { .. } => Err(WorkspaceStorageError::BackendMismatch {
            detail: "a Btrfs workspace capability was presented to the deterministic-directory \
                     backend - the two backends' identities are not interchangeable"
                .to_string(),
        }),
    }
}

#[cfg(any(test, feature = "test-support"))]
fn scan_regular_file_bytes(dir: &Path) -> Result<u64, WorkspaceStorageError> {
    use std::os::unix::fs::MetadataExt;
    let mut total: u64 = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = std::fs::read_dir(&current).map_err(|e| WorkspaceStorageError::Io {
            path: current.clone(),
            reason: format!("scan read_dir: {e}"),
        })?;
        for entry in entries {
            let entry = entry.map_err(|e| WorkspaceStorageError::Io {
                path: current.clone(),
                reason: format!("scan read_dir entry: {e}"),
            })?;
            let path = entry.path();
            let meta = std::fs::symlink_metadata(&path).map_err(|e| WorkspaceStorageError::Io {
                path: path.clone(),
                reason: format!("scan stat: {e}"),
            })?;
            let ft = meta.file_type();
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() {
                total =
                    total
                        .checked_add(meta.size())
                        .ok_or_else(|| WorkspaceStorageError::Io {
                            path: path.clone(),
                            reason: "byte-accounting overflow while scanning workspace".to_string(),
                        })?;
            }
        }
    }
    Ok(total)
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Debug)]
pub(crate) struct DirectoryWorkspaceStorage {
    canonical_base: PathBuf,
}

#[cfg(any(test, feature = "test-support"))]
impl DirectoryWorkspaceStorage {
    pub(crate) fn open(base_dir: &Path) -> Result<Self, WorkspaceStorageError> {
        if !exists_or_error(base_dir)? {
            std::fs::create_dir_all(base_dir).map_err(|e| WorkspaceStorageError::Io {
                path: base_dir.to_path_buf(),
                reason: format!("create workspace base dir: {e}"),
            })?;
        }
        let canonical_base =
            std::fs::canonicalize(base_dir).map_err(|e| WorkspaceStorageError::Io {
                path: base_dir.to_path_buf(),
                reason: format!("canonicalize: {e}"),
            })?;
        assert_base_dir_exclusively_owned(&canonical_base)?;
        Ok(Self { canonical_base })
    }

    pub(crate) fn base_dir(&self) -> &Path {
        &self.canonical_base
    }

    pub(crate) fn check_health(&self) -> Result<(), WorkspaceStorageError> {
        assert_base_dir_exclusively_owned(&self.canonical_base)
    }

    pub(crate) fn create_workspace(
        &mut self,
        job_id: &str,
        quota_bytes: u64,
        _owner_uid: u32,
        _owner_gid: u32,
    ) -> Result<PreparedWorkspace, WorkspaceStorageError> {
        use std::os::unix::fs::MetadataExt;
        validate_job_id(job_id)?;
        if quota_bytes == 0 {
            return Err(WorkspaceStorageError::ZeroQuota);
        }
        let path = self.canonical_base.join(job_id);
        if let Err(e) = std::fs::create_dir(&path) {
            if e.kind() == ErrorKind::AlreadyExists {
                return Err(WorkspaceStorageError::UnrecoverableLeak {
                    path,
                    subvol_id: None,
                    provisioning_error:
                        "an untracked workspace leaf already exists at this job key".to_string(),
                    cleanup_error: "left in place - a pre-existing residual is not this call's to \
                                    remove; a human must reconcile"
                        .to_string(),
                });
            }
            return Err(WorkspaceStorageError::Io {
                path,
                reason: format!("create workspace leaf: {e}"),
            });
        }
        if let Err(e) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)) {
            return Err(
                self.rollback_fresh_leaf(&path, format!("chmod 0755 on the fresh leaf: {e}"))
            );
        }
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(e) => {
                return Err(self.rollback_fresh_leaf(
                    &path,
                    format!("capture (device, inode) of the fresh leaf: {e}"),
                ));
            }
        };
        Ok(PreparedWorkspace {
            host_path: path,
            identity: Box::new(WorkspaceIdentity::Directory {
                device: meta.dev(),
                inode: meta.ino(),
                quota_bytes,
            }),
            minted_from: self.canonical_base.clone(),
        })
    }

    fn rollback_fresh_leaf(
        &self,
        path: &Path,
        provisioning_error: String,
    ) -> WorkspaceStorageError {
        let leak = |cleanup_error: String| WorkspaceStorageError::UnrecoverableLeak {
            path: path.to_path_buf(),
            subvol_id: None,
            provisioning_error: provisioning_error.clone(),
            cleanup_error,
        };
        if let Err(e) = std::fs::remove_dir(path) {
            return leak(format!("remove_dir of the fresh leaf failed: {e}"));
        }
        if let Err(e) = fsync_dir(&self.canonical_base) {
            return leak(format!(
                "fsync of the parent base after rollback failed: {e}"
            ));
        }
        match exists_or_error(path) {
            Ok(false) => WorkspaceStorageError::Io {
                path: path.to_path_buf(),
                reason: format!("{provisioning_error} (the fresh leaf was rolled back cleanly)"),
            },
            Ok(true) => leak("the leaf is STILL present after remove_dir + fsync".to_string()),
            Err(e) => leak(format!("post-rollback absence re-check failed: {e}")),
        }
    }

    pub(crate) fn delete_workspace(
        &mut self,
        prepared: PreparedWorkspace,
    ) -> Result<(), WorkspaceStorageError> {
        assert_same_storage(&self.canonical_base, &prepared.minted_from)?;
        let (device, inode) = directory_identity_or_backend_mismatch(&prepared.identity)?;
        self.delete_verified(&prepared.host_path, device, inode)
    }

    pub(crate) fn delete_orphan(
        &mut self,
        candidate: OrphanCandidate,
    ) -> Result<(), WorkspaceStorageError> {
        assert_same_storage(&self.canonical_base, &candidate.minted_from)?;
        let (device, inode) = directory_identity_or_backend_mismatch(&candidate.identity)?;
        self.delete_verified(&candidate.path, device, inode)
    }

    fn delete_verified(
        &self,
        leaf: &Path,
        expected_device: u64,
        expected_inode: u64,
    ) -> Result<(), WorkspaceStorageError> {
        use std::os::unix::fs::MetadataExt;
        let unproven = |reason: String| WorkspaceStorageError::DirectoryAbsenceUnproven {
            path: leaf.to_path_buf(),
            reason,
        };
        if leaf.parent() != Some(self.canonical_base.as_path()) {
            return Err(unproven(format!(
                "leaf is not a direct child of the controlled base {:?}",
                self.canonical_base
            )));
        }
        let Some(name) = leaf.file_name().and_then(|n| n.to_str()) else {
            return Err(unproven(
                "leaf name is not a valid single component".to_string(),
            ));
        };
        if validate_job_id(name).is_err() {
            return Err(unproven(format!(
                "leaf name {name:?} is not a safe component"
            )));
        }
        let meta = std::fs::symlink_metadata(leaf)
            .map_err(|e| unproven(format!("stat before delete: {e}")))?;
        if meta.file_type().is_symlink() {
            return Err(unproven(
                "leaf is a symlink - refusing to delete".to_string(),
            ));
        }
        if !meta.is_dir() {
            return Err(unproven("leaf is not a directory".to_string()));
        }
        if meta.dev() != expected_device || meta.ino() != expected_inode {
            return Err(unproven(format!(
                "leaf (device, inode) = ({}, {}) does not match the captured ({}, {})",
                meta.dev(),
                meta.ino(),
                expected_device,
                expected_inode
            )));
        }
        std::fs::remove_dir_all(leaf)
            .map_err(|e| unproven(format!("recursive remove_dir_all: {e}")))?;
        if let Err(e) = fsync_dir(&self.canonical_base) {
            return Err(unproven(format!("fsync parent base dir after delete: {e}")));
        }
        match exists_or_error(leaf) {
            Ok(false) => Ok(()),
            Ok(true) => Err(unproven(
                "the leaf is STILL present after remove_dir_all + fsync".to_string(),
            )),
            Err(e) => Err(unproven(format!(
                "post-delete absence re-check failed: {e}"
            ))),
        }
    }

    pub(crate) fn list_orphaned_workspaces(
        &mut self,
        active_job_ids: &BTreeSet<String>,
    ) -> Result<Vec<OrphanCandidate>, WorkspaceStorageError> {
        use std::os::unix::fs::MetadataExt;
        if !exists_or_error(&self.canonical_base)? {
            return Ok(Vec::new());
        }
        let entries = std::fs::read_dir(&self.canonical_base).map_err(|e| {
            WorkspaceStorageError::ListFailed {
                base_dir: self.canonical_base.clone(),
                reason: e.to_string(),
            }
        })?;
        let mut orphans = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| WorkspaceStorageError::ListFailed {
                base_dir: self.canonical_base.clone(),
                reason: e.to_string(),
            })?;
            let path = entry.path();
            let meta = std::fs::symlink_metadata(&path).map_err(|e| {
                WorkspaceStorageError::UnexpectedEntry {
                    path: path.clone(),
                    reason: format!("stat: {e}"),
                }
            })?;
            if meta.file_type().is_symlink() || !meta.is_dir() {
                return Err(WorkspaceStorageError::UnexpectedEntry {
                    path,
                    reason: "not an ordinary directory (a file, symlink, or special entry)"
                        .to_string(),
                });
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if validate_job_id(&name).is_err() {
                return Err(WorkspaceStorageError::UnexpectedEntry {
                    path,
                    reason: format!("child directory name {name:?} is not a safe component"),
                });
            }
            if active_job_ids.contains(&name) {
                continue;
            }
            orphans.push(OrphanCandidate {
                path,
                identity: Box::new(WorkspaceIdentity::Directory {
                    device: meta.dev(),
                    inode: meta.ino(),
                    quota_bytes: 0,
                }),
                minted_from: self.canonical_base.clone(),
            });
        }
        Ok(orphans)
    }
}

#[cfg(any(test, feature = "test-support"))]
fn fsync_dir(dir: &Path) -> std::io::Result<()> {
    let file = std::fs::File::open(dir)?;
    file.sync_all()
}

#[cfg(any(test, feature = "test-support"))]
impl PreparedWorkspace {
    pub(crate) fn directory_quota_bytes(&self) -> Option<u64> {
        match self.identity.as_ref() {
            WorkspaceIdentity::Directory { quota_bytes, .. } => Some(*quota_bytes),
            WorkspaceIdentity::Btrfs { .. } => None,
        }
    }

    pub(crate) fn scan_used_bytes(&self) -> Result<u64, WorkspaceStorageError> {
        directory_identity_or_backend_mismatch(&self.identity)?;
        scan_regular_file_bytes(&self.host_path)
    }

    pub(crate) fn checked_directory_write(
        &self,
        file_name: &str,
        bytes: &[u8],
    ) -> Result<(), WorkspaceStorageError> {
        use std::os::unix::fs::MetadataExt;
        directory_identity_or_backend_mismatch(&self.identity)?;
        let quota_bytes = self
            .directory_quota_bytes()
            .expect("a directory identity always carries a quota");
        let safe_name = !file_name.is_empty()
            && file_name.len() <= BTRFS_NAME_MAX
            && !file_name.contains('/')
            && !file_name.contains('\0')
            && file_name != "."
            && file_name != "..";
        if !safe_name {
            return Err(WorkspaceStorageError::Io {
                path: self.host_path.join(file_name),
                reason: format!(
                    "checked write name {file_name:?} is not a safe filename component"
                ),
            });
        }
        let target = self.host_path.join(file_name);
        let used = scan_regular_file_bytes(&self.host_path)?;
        let existing = match std::fs::symlink_metadata(&target) {
            Ok(meta) if meta.file_type().is_file() => meta.size(),
            _ => 0,
        };
        let would_be = used
            .saturating_sub(existing)
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| WorkspaceStorageError::Io {
                path: target.clone(),
                reason: "byte-accounting overflow computing the post-write total".to_string(),
            })?;
        if would_be > quota_bytes {
            return Err(WorkspaceStorageError::DirectoryQuotaExceeded {
                path: target,
                quota_bytes,
                would_be_bytes: would_be,
            });
        }
        std::fs::write(&target, bytes).map_err(|e| WorkspaceStorageError::Io {
            path: target,
            reason: format!("checked write: {e}"),
        })
    }
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_string()
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn exists_or_error(path: &Path) -> Result<bool, WorkspaceStorageError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
        Err(e) => Err(WorkspaceStorageError::Io {
            path: path.to_path_buf(),
            reason: e.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_base(tag: &str) -> PathBuf {
        let mut p = std::env::home_dir().expect("HOME must be set for this test");
        p.push(format!(".local/state/myelin-workspace-storage-tests-{tag}"));
        p
    }

    fn open_or_skip_env(tag: &str) -> Option<WorkspaceStorage> {
        match WorkspaceStorage::open(&test_base(tag)) {
            Ok(s) => Some(s),
            Err(
                e @ (WorkspaceStorageError::NotBtrfs { .. }
                | WorkspaceStorageError::QuotaNotEnforcing { .. }),
            ) => {
                eprintln!("SKIP: no Btrfs+enforcing-quota support on this host: {e}");
                None
            }
            Err(e) => {
                panic!("WorkspaceStorage::open failed unexpectedly (not an environmental gap): {e}")
            }
        }
    }

    fn open_or_skip_privileged(tag: &str) -> Option<WorkspaceStorage> {
        let storage = open_or_skip_env(tag)?;
        let probe = run_btrfs(&[
            OsStr::new("qgroup"),
            OsStr::new("show"),
            OsStr::new("-r"),
            OsStr::new("--raw"),
            storage.base_dir().as_os_str(),
        ])
        .expect("spawn btrfs for the privilege probe");
        if probe.status.success() {
            Some(storage)
        } else {
            let stderr = stderr_of(&probe);
            assert!(
                stderr.contains("Operation not permitted"),
                "expected the specific unprivileged-qgroup denial, got a DIFFERENT failure \
                 (a real regression, not a privilege gap): {stderr}"
            );
            eprintln!(
                "SKIP: this test process lacks CAP_SYS_ADMIN for qgroup operations: {stderr}"
            );
            None
        }
    }

    #[test]
    #[cfg_attr(
        not(feature = "privileged-host-tests"),
        ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) - run on the host lane with --features privileged-host-tests"
    )]
    fn full_privileged_lifecycle_create_quota_verify_exceed_delete_sync() {
        let Some(mut storage) = open_or_skip_privileged("lifecycle") else {
            return;
        };
        let job_id = format!("probe{}", std::process::id());
        let quota: u64 = 8 << 20;

        let created = storage
            .create_workspace(&job_id, quota, 0, 0)
            .expect("provisioning must succeed now that privilege is confirmed");
        assert!(created.host_path().exists());
        assert_eq!(created.host_path(), storage.base_dir().join(&job_id));

        let observed = storage
            .read_qgroup_max_referenced(created.subvol_id())
            .expect("read back the applied quota");
        assert_eq!(observed, Some(quota));

        let mut incompressible = vec![0u8; (quota as usize) * 2];
        let mut state: u64 = 0x9E3779B97F4A7C15;
        for chunk in incompressible.chunks_mut(8) {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            chunk.copy_from_slice(&state.to_le_bytes()[..chunk.len()]);
        }
        let big_file = created.host_path().join("overflow");
        let write_err = std::fs::write(&big_file, &incompressible)
            .expect_err("writing incompressible data past the quota must fail");
        let errno = write_err.raw_os_error();
        assert!(
            errno == Some(libc_enospc()) || errno == Some(libc_edquot()),
            "expected ENOSPC or EDQUOT, got {write_err:?} (errno {errno:?})"
        );

        storage
            .delete_workspace(created)
            .expect("delete with the correct id must succeed");
    }

    #[test]
    fn invalid_job_ids_are_rejected_before_any_filesystem_call() {
        for bad in ["", "../escape", "with/slash", "with space", "with\0nul"] {
            assert!(matches!(
                validate_job_id(bad),
                Err(WorkspaceStorageError::InvalidJobId { .. })
            ));
        }
        let too_long = "a".repeat(BTRFS_NAME_MAX + 1);
        assert!(matches!(
            validate_job_id(&too_long),
            Err(WorkspaceStorageError::InvalidJobId { .. })
        ));
    }

    #[test]
    #[cfg_attr(
        not(feature = "privileged-host-tests"),
        ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) - run on the host lane with --features privileged-host-tests"
    )]
    fn zero_quota_is_rejected() {
        let Some(mut storage) = open_or_skip_env("zero-quota") else {
            return;
        };
        let job_id = format!("zeroq{}", std::process::id());
        let err = storage.create_workspace(&job_id, 0, 0, 0).unwrap_err();
        assert!(matches!(err, WorkspaceStorageError::ZeroQuota));
        assert!(
            !exists_or_error(&storage.base_dir().join(&job_id)).unwrap(),
            "a rejected zero-quota request must not have created anything"
        );
    }

    #[test]
    #[cfg_attr(
        not(feature = "privileged-host-tests"),
        ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) - run on the host lane with --features privileged-host-tests"
    )]
    fn orphan_listing_verifies_before_filtering_and_finds_the_real_orphan() {
        let Some(mut storage) = open_or_skip_privileged("orphan-listing") else {
            return;
        };
        let suffix = std::process::id();
        let active_id = format!("active{suffix}");
        let orphan_id = format!("orphan{suffix}");
        let active_ws = storage
            .create_workspace(&active_id, 8 << 20, 0, 0)
            .expect("create the active workspace");
        let orphan_ws = storage
            .create_workspace(&orphan_id, 8 << 20, 0, 0)
            .expect("create the to-be-orphaned workspace");
        let orphan_subvol_id = orphan_ws.subvol_id();

        let mut active = BTreeSet::new();
        active.insert(active_id.clone());
        let found = storage
            .list_orphaned_workspaces(&active)
            .expect("list orphans");
        assert_eq!(
            found
                .iter()
                .map(OrphanCandidate::subvol_id)
                .collect::<Vec<_>>(),
            vec![orphan_subvol_id],
            "exactly the non-active workspace must be listed as orphaned, not the active one"
        );

        storage.delete_workspace(active_ws).expect("cleanup active");
        for candidate in found {
            storage.delete_orphan(candidate).expect("cleanup orphan");
        }
    }

    #[test]
    #[cfg_attr(
        not(feature = "privileged-host-tests"),
        ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) - run on the host lane with --features privileged-host-tests"
    )]
    fn a_capability_from_one_storage_is_refused_by_another() {
        let Some(mut storage_a) = open_or_skip_privileged("cross-storage-a") else {
            return;
        };
        let sibling_base = storage_a.base_dir().with_file_name(format!(
            "{}-sibling-{}",
            storage_a
                .base_dir()
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("myelin-workspace-storage-tests"),
            std::process::id()
        ));
        let mut storage_b =
            WorkspaceStorage::open(&sibling_base).expect("open a second, sibling handle");

        let job_id = format!("crossx{}", std::process::id());
        let prepared = storage_a
            .create_workspace(&job_id, 8 << 20, 0, 0)
            .expect("create on storage A");
        let path = prepared.host_path().to_path_buf();
        let subvol_id = prepared.subvol_id();

        let refused = storage_b.delete_workspace(prepared);
        assert!(
            matches!(refused, Err(WorkspaceStorageError::WrongStorage { .. })),
            "a capability minted by storage A must be refused by storage B, got {refused:?}"
        );
        assert!(
            path.exists(),
            "the refused delete must not have removed anything"
        );

        let orphans = storage_a
            .list_orphaned_workspaces(&BTreeSet::new())
            .expect("list to find it again for cleanup");
        let candidate = orphans
            .into_iter()
            .find(|c| c.subvol_id() == subvol_id)
            .expect("the workspace is discoverable again through its own storage");
        storage_a
            .delete_orphan(candidate)
            .expect("cleanup through the correct storage");
        std::fs::remove_dir(storage_b.base_dir()).expect("remove the empty sibling base directory");
    }

    #[test]
    #[cfg_attr(
        not(feature = "privileged-host-tests"),
        ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) - run on the host lane with --features privileged-host-tests"
    )]
    fn orphan_listing_reports_a_non_subvolume_entry_loudly_even_if_its_name_is_active() {
        let Some(mut storage) = open_or_skip_env("stray-entry") else {
            return;
        };
        let stray_name = format!("stray-file-{}", std::process::id());
        let stray = storage.base_dir().join(&stray_name);
        std::fs::write(&stray, b"not a subvolume").expect("create a stray plain file");
        let mut active = BTreeSet::new();
        active.insert(stray_name);
        let result = storage.list_orphaned_workspaces(&active);
        std::fs::remove_file(&stray).ok();
        assert!(
            matches!(result, Err(WorkspaceStorageError::UnexpectedEntry { .. })),
            "a stray non-subvolume entry must be reported loud even if its name matches an \
             active job id, got {result:?}"
        );
    }

    #[test]
    fn assert_workspace_open_refuses_a_tmpfs_directory() {
        let tmp = std::env::temp_dir().join("myelin-workspace-storage-not-btrfs-probe");
        let err = WorkspaceStorage::open(&tmp).unwrap_err();
        assert!(
            matches!(err, WorkspaceStorageError::NotBtrfs { .. }),
            "a tmpfs directory must be refused as NotBtrfs, got {err:?}"
        );
    }

    #[test]
    fn directory_rollback_that_cannot_remove_the_leaf_is_an_unrecoverable_leak() {
        let base = std::env::temp_dir().join(format!(
            "myelin-dir-rollback-leak-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&base).unwrap();
        let storage = DirectoryWorkspaceStorage::open(&base).expect("open directory backend");
        let leaf = storage.base_dir().join("leaf");
        std::fs::create_dir(&leaf).unwrap();
        std::fs::write(leaf.join("occupant"), b"x").unwrap();
        let err = storage.rollback_fresh_leaf(&leaf, "injected provisioning failure".to_string());
        assert!(
            matches!(err, WorkspaceStorageError::UnrecoverableLeak { .. }),
            "a rollback that cannot remove the leaf is an UnrecoverableLeak, got {err:?}"
        );
        assert!(
            leaf.exists(),
            "the un-removable residual survives - surfaced via retain+poison, never a clean release"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    fn libc_enospc() -> i32 {
        28
    }

    fn libc_edquot() -> i32 {
        122
    }
}
