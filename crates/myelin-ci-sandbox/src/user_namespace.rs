use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunscInvocationMode {
    Rootless,
    ExplicitUserNamespace(UserNamespaceConfig),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UserNamespaceConfig {
    runner_uid: u32,
    runner_gid: u32,
    subordinate_uid: u32,
    subordinate_gid: u32,
}

impl UserNamespaceConfig {
    pub fn runner_uid(&self) -> u32 {
        self.runner_uid
    }
    pub fn runner_gid(&self) -> u32 {
        self.runner_gid
    }
    pub fn subordinate_uid(&self) -> u32 {
        self.subordinate_uid
    }
    pub fn subordinate_gid(&self) -> u32 {
        self.subordinate_gid
    }

    #[cfg(test)]
    pub(crate) fn for_tests(
        runner_uid: u32,
        runner_gid: u32,
        subordinate_uid: u32,
        subordinate_gid: u32,
    ) -> Self {
        UserNamespaceConfig {
            runner_uid,
            runner_gid,
            subordinate_uid,
            subordinate_gid,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SubordinateRange {
    start: u32,
    count: u32,
}

fn effective_username() -> Option<String> {
    let uid = unsafe { libc::geteuid() };
    let mut buf = vec![0u8; 16384];
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    let ret = unsafe {
        libc::getpwuid_r(
            uid,
            &mut pwd,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut result,
        )
    };
    if ret != 0 || result.is_null() || pwd.pw_name.is_null() {
        return None;
    }
    let cstr = unsafe { std::ffi::CStr::from_ptr(pwd.pw_name) };
    cstr.to_str().ok().map(str::to_string)
}

fn read_subordinate_file(path: &Path, strict: bool) -> Result<String, UserNamespaceAllocatorError> {
    let malformed = |reason: String| UserNamespaceAllocatorError::SubordinateConfig {
        path: path.to_path_buf(),
        reason,
    };
    let path_c = CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|e| malformed(format!("path contains an interior NUL: {e}")))?;
    let fd = unsafe {
        libc::open(
            path_c.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(malformed(format!("open: {}", io::Error::last_os_error())));
    }
    let mut file = unsafe { <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(fd) };
    let meta = file
        .metadata()
        .map_err(|e| malformed(format!("fstat: {e}")))?;
    if !meta.is_file() {
        return Err(malformed("not a regular file".to_string()));
    }
    if strict {
        if meta.uid() != 0 {
            return Err(malformed(format!(
                "must be owned by root (uid 0), got uid {}",
                meta.uid()
            )));
        }
        if meta.mode() & 0o022 != 0 {
            return Err(malformed(format!(
                "must not be group/other-writable (mode {:o})",
                meta.mode() & 0o777
            )));
        }
    }
    let mut content = String::new();
    io::Read::read_to_string(&mut file, &mut content)
        .map_err(|e| malformed(format!("read: {e}")))?;
    Ok(content)
}

fn range_contains(range: SubordinateRange, value: u32) -> bool {
    value >= range.start && value < range.start.saturating_add(range.count)
}

fn parse_subordinate_range(
    path: &Path,
    uid: u32,
    username: Option<&str>,
    strict: bool,
) -> Result<SubordinateRange, UserNamespaceAllocatorError> {
    let malformed = |reason: String| UserNamespaceAllocatorError::SubordinateConfig {
        path: path.to_path_buf(),
        reason,
    };
    let content = read_subordinate_file(path, strict)?;
    let mut matches = Vec::new();
    let mut others = Vec::new();
    for (line_no, raw_line) in content.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(3, ':');
        let (owner, start, count) = match (parts.next(), parts.next(), parts.next()) {
            (Some(o), Some(s), Some(c)) => (o, s, c),
            _ => {
                return Err(malformed(format!(
                    "{path:?} line {}: expected `owner:start:count`, got {line:?}",
                    line_no + 1
                )))
            }
        };
        let start: u32 = start.parse().map_err(|_| {
            malformed(format!(
                "{path:?} line {}: non-numeric start {start:?}",
                line_no + 1
            ))
        })?;
        let count: u32 = count.parse().map_err(|_| {
            malformed(format!(
                "{path:?} line {}: non-numeric count {count:?}",
                line_no + 1
            ))
        })?;
        if count == 0 {
            return Err(malformed(format!(
                "{path:?} line {}: a zero-length subordinate range is refused",
                line_no + 1
            )));
        }
        start.checked_add(count).ok_or_else(|| {
            malformed(format!(
                "{path:?} line {}: start+count overflows u32",
                line_no + 1
            ))
        })?;
        let owner_is_match =
            owner.parse::<u32>().map(|n| n == uid).unwrap_or(false) || Some(owner) == username;
        if owner_is_match {
            matches.push(SubordinateRange { start, count });
        } else {
            others.push(SubordinateRange { start, count });
        }
    }
    match matches.len() {
        0 => Err(UserNamespaceAllocatorError::NoSubordinateEntry {
            path: path.to_path_buf(),
            uid,
        }),
        1 => {
            let selected = matches[0];
            if let Some(overlap) = others.iter().find(|o| ranges_overlap(selected, **o)) {
                return Err(malformed(format!(
                    "{path:?}: this uid's selected range {selected:?} overlaps another owner's \
                     range {overlap:?} - both would map the same host id, breaking the \
                     \"real, otherwise-unused subordinate id\" guarantee"
                )));
            }
            Ok(selected)
        }
        n => Err(malformed(format!(
            "{path:?}: {n} AMBIGUOUS entries match uid {uid} ({username:?}) - refusing to guess \
             which is authoritative"
        ))),
    }
}

fn ranges_overlap(a: SubordinateRange, b: SubordinateRange) -> bool {
    a.start < b.start.saturating_add(b.count) && b.start < a.start.saturating_add(a.count)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerInstanceId(u128);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseNonce(u128);

fn random_u128_from(mut entropy: impl io::Read) -> io::Result<u128> {
    let mut bytes = [0u8; 16];
    entropy.read_exact(&mut bytes)?;
    Ok(u128::from_le_bytes(bytes))
}

fn random_u128() -> io::Result<u128> {
    random_u128_from(std::fs::File::open("/dev/urandom")?)
}

fn runner_instance_id_from(entropy: impl io::Read) -> io::Result<RunnerInstanceId> {
    random_u128_from(entropy).map(RunnerInstanceId)
}

fn runner_instance_id() -> Result<RunnerInstanceId, String> {
    static CACHED: OnceLock<Result<RunnerInstanceId, String>> = OnceLock::new();
    CACHED
        .get_or_init(|| {
            let entropy = std::fs::File::open("/dev/urandom")
                .map_err(|error| format!("open /dev/urandom: {error}"))?;
            runner_instance_id_from(entropy).map_err(|error| format!("read /dev/urandom: {error}"))
        })
        .clone()
}

const LEASE_MARKER_SCHEMA_V1: u32 = 1;
const LEASE_MARKER_SCHEMA_V2: u32 = 2;

#[derive(Deserialize)]
struct SchemaPeek {
    schema_version: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LeaseMarkerV1 {
    schema_version: u32,
    lease_nonce: LeaseNonce,
    runner_instance_id: RunnerInstanceId,
    host_uid: u32,
    host_gid: u32,
    created_at_unix_secs: u64,
    phase: LeasePhaseV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum LeasePhaseV1 {
    Allocated,
    Bound {
        container_id: String,
        runsc_root_identity: (u64, u64),
        cgroup_identity: (u64, u64),
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LeaseMarkerV2 {
    schema_version: u32,
    lease_nonce: LeaseNonce,
    runner_instance_id: RunnerInstanceId,
    host_uid: u32,
    host_gid: u32,
    created_at_unix_secs: u64,
    phase: LeasePhaseV2,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum LeasePhaseV2 {
    Allocated,
    PreparationBound {
        container_id: String,
        runsc_root_identity: (u64, u64),
        cgroup_identity: (u64, u64),
    },
    Prepared {
        preparation_container_id: String,
        preparation_runsc_root_identity: (u64, u64),
        preparation_cgroup_identity: (u64, u64),
    },
    Bound {
        container_id: String,
        runsc_root_identity: (u64, u64),
        cgroup_identity: (u64, u64),
    },
}

fn marker_file_name(slot: u32) -> String {
    format!("slot-{slot:010}.json")
}

fn parse_marker_file_name(name: &str) -> Option<u32> {
    let digits = name.strip_prefix("slot-")?.strip_suffix(".json")?;
    if digits.len() != 10 {
        return None;
    }
    digits.parse().ok()
}

fn parse_stray_tmp_marker_file_name(name: &str) -> Option<u32> {
    let digits = name.strip_prefix("slot-")?.strip_suffix(".json.tmp")?;
    if digits.len() != 10 {
        return None;
    }
    digits.parse().ok()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UserNamespaceAdmission {
    Healthy,
    Poisoned { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserNamespaceRefusal {
    Poisoned { reason: String },
    PoolExhausted { pool_size: u32 },
}

impl std::fmt::Display for UserNamespaceRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UserNamespaceRefusal::Poisoned { reason } => {
                write!(f, "user-namespace allocator is poisoned: {reason}")
            }
            UserNamespaceRefusal::PoolExhausted { pool_size } => write!(
                f,
                "user-namespace subordinate-id pool ({pool_size} slots) is fully leased"
            ),
        }
    }
}

#[derive(Debug)]
pub enum UserNamespaceAllocatorError {
    AlreadyLocked { leases_dir: PathBuf },
    LockFailed { leases_dir: PathBuf, reason: String },
    SubordinateConfig { path: PathBuf, reason: String },
    NoSubordinateEntry { path: PathBuf, uid: u32 },
    UnsafeLeasesDir { leases_dir: PathBuf, reason: String },
    CorruptLeaseMarker { path: PathBuf, reason: String },
    PrivilegedRunner { euid: u32, egid: u32 },
    EntropyUnavailable { reason: String },
    PoolTooSmall { pool_size: u32, required: u32 },
}

impl std::fmt::Display for UserNamespaceAllocatorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UserNamespaceAllocatorError::AlreadyLocked { leases_dir } => write!(
                f,
                "user-namespace leases dir {leases_dir:?} is already locked by another process"
            ),
            UserNamespaceAllocatorError::LockFailed { leases_dir, reason } => {
                write!(
                    f,
                    "failed to lock user-namespace leases dir {leases_dir:?}: {reason}"
                )
            }
            UserNamespaceAllocatorError::SubordinateConfig { path, reason } => {
                write!(f, "subordinate-range config error at {path:?}: {reason}")
            }
            UserNamespaceAllocatorError::NoSubordinateEntry { path, uid } => {
                write!(f, "{path:?} has no subordinate-range entry for uid {uid}")
            }
            UserNamespaceAllocatorError::UnsafeLeasesDir { leases_dir, reason } => {
                write!(
                    f,
                    "leases dir {leases_dir:?} failed hardening policy: {reason}"
                )
            }
            UserNamespaceAllocatorError::CorruptLeaseMarker { path, reason } => write!(
                f,
                "corrupt/unrecognized lease marker at {path:?}: {reason} - refusing to start with \
                 an untrustworthy leases directory"
            ),
            UserNamespaceAllocatorError::PrivilegedRunner { euid, egid } => write!(
                f,
                "this process's own euid={euid}/egid={egid} must not be 0 (root) - refusing to \
                 start an allocator whose container-namespace-root mapping would resolve to host \
                 root"
            ),
            UserNamespaceAllocatorError::EntropyUnavailable { reason } => write!(
                f,
                "kernel entropy is unavailable for the runner-instance identity: {reason}"
            ),
            UserNamespaceAllocatorError::PoolTooSmall {
                pool_size,
                required,
            } => write!(
                f,
                "the computed pool size ({pool_size} slots) is smaller than the caller's stated \
                 minimum requirement ({required} slots)"
            ),
        }
    }
}

impl std::error::Error for UserNamespaceAllocatorError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserNamespaceReleaseError {
    ProofMismatch,
    MarkerMismatch,
    ProofDisagreesWithMarker,
    Poisoned,
    InternalInvariantViolated { reason: String },
    InvalidSessionState,
    LeaseMismatch,
}

impl std::fmt::Display for UserNamespaceReleaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UserNamespaceReleaseError::ProofMismatch => {
                write!(
                    f,
                    "the supplied quiescence proof was not minted for this lease"
                )
            }
            UserNamespaceReleaseError::MarkerMismatch => write!(
                f,
                "the durable marker no longer matches this lease's own identity"
            ),
            UserNamespaceReleaseError::ProofDisagreesWithMarker => write!(
                f,
                "the durable marker belongs to this lease but its phase disagrees with the \
                 supplied proof"
            ),
            UserNamespaceReleaseError::Poisoned => {
                write!(f, "releasing this lease had an ambiguous outcome")
            }
            UserNamespaceReleaseError::InternalInvariantViolated { reason } => {
                write!(f, "internal invariant violated while releasing: {reason}")
            }
            UserNamespaceReleaseError::InvalidSessionState => {
                write!(f, "the checkout session is not prepared for release")
            }
            UserNamespaceReleaseError::LeaseMismatch => write!(
                f,
                "the checkout session was asked to release a different userns lease"
            ),
        }
    }
}

impl std::error::Error for UserNamespaceReleaseError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PreparationConfirmationError {
    ProofMismatch,
    MarkerMismatch,
    ProofDisagreesWithMarker,
    Poisoned,
    InvalidSessionState,
    LeaseMismatch,
}

impl std::fmt::Display for PreparationConfirmationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PreparationConfirmationError::ProofMismatch => write!(
                f,
                "the supplied preparation quiescence proof was not minted for this lease"
            ),
            PreparationConfirmationError::MarkerMismatch => write!(
                f,
                "the durable marker no longer matches this lease's own identity"
            ),
            PreparationConfirmationError::ProofDisagreesWithMarker => write!(
                f,
                "the durable marker belongs to this lease but its phase disagrees with the \
                 supplied preparation proof"
            ),
            PreparationConfirmationError::Poisoned => write!(
                f,
                "confirming preparation quiescence had an ambiguous outcome"
            ),
            PreparationConfirmationError::InvalidSessionState => {
                write!(
                    f,
                    "the checkout session is not awaiting preparation confirmation"
                )
            }
            PreparationConfirmationError::LeaseMismatch => write!(
                f,
                "the preparation proof was presented with a different userns lease"
            ),
        }
    }
}

impl std::error::Error for PreparationConfirmationError {}

pub type IncidentSink = Arc<dyn Fn(&str) + Send + Sync>;

fn report_incident_standalone(sink: &IncidentSink, message: &str) {
    let sink = sink.clone();
    let message = message.to_string();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || sink(&message)));
}

struct AllocatorState {
    admission: UserNamespaceAdmission,
    quarantined_slots: BTreeSet<u32>,
    active_slots: BTreeSet<u32>,
    locked_identity: Option<(u64, u64)>,
}

fn insert_active_slot_checked(active_slots: &mut BTreeSet<u32>, slot: u32) -> Result<(), String> {
    if active_slots.insert(slot) {
        Ok(())
    } else {
        Err(format!(
            "slot {slot} was already marked active in active_slots despite being skipped as \
             taken moments earlier under the same lock hold - a bookkeeping invariant was \
             violated"
        ))
    }
}

struct SharedState {
    _lock: OwnedFd,
    state: Mutex<AllocatorState>,
    incident_sink: IncidentSink,
}

impl SharedState {
    fn lock_state(&self) -> MutexGuard<'_, AllocatorState> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                let mut inner = poisoned.into_inner();
                if !matches!(inner.admission, UserNamespaceAdmission::Poisoned { .. }) {
                    inner.admission = UserNamespaceAdmission::Poisoned {
                        reason: "internal allocator-state mutex was poisoned by a prior panic"
                            .to_string(),
                    };
                }
                inner
            }
        }
    }

    fn report_incident(&self, message: &str) {
        report_incident_standalone(&self.incident_sink, message);
    }

    fn poison(&self, reason: impl Into<String>) {
        let reason = reason.into();
        {
            let mut state = self.lock_state();
            if !matches!(state.admission, UserNamespaceAdmission::Poisoned { .. }) {
                state.admission = UserNamespaceAdmission::Poisoned {
                    reason: reason.clone(),
                };
            }
        }
        self.report_incident(&reason);
    }

    fn quarantine_slot(&self, slot: u32, reason: impl Into<String>) {
        let reason = reason.into();
        {
            let mut state = self.lock_state();
            state.active_slots.remove(&slot);
            state.quarantined_slots.insert(slot);
        }
        self.report_incident(&reason);
    }

    fn dir_fd(&self) -> RawFd {
        self._lock.as_raw_fd()
    }

    fn listing_path(&self) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}", self.dir_fd()))
    }

    fn fsync_locked_dir(&self) -> io::Result<()> {
        let ret = unsafe { libc::fsync(self.dir_fd()) };
        if ret != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

fn openat_marker(dir_fd: RawFd, name: &str, create: bool) -> io::Result<std::fs::File> {
    let name_c = CString::new(name)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
    let flags = if create {
        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW
    } else {
        libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW
    };
    let fd = unsafe { libc::openat(dir_fd, name_c.as_ptr(), flags, 0o600) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(fd) })
}

fn rewrite_marker_atomically(dir_fd: RawFd, name: &str, content: &[u8]) -> io::Result<()> {
    let tmp_name = format!("{name}.tmp");
    let tmp_name_c = CString::new(tmp_name.as_str())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
    let fd = unsafe {
        libc::openat(
            dir_fd,
            tmp_name_c.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0o600,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut tmp_file = unsafe { <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(fd) };
    let write_and_sync =
        io::Write::write_all(&mut tmp_file, content).and_then(|()| tmp_file.sync_all());
    if let Err(e) = write_and_sync {
        let _ = unsafe { libc::unlinkat(dir_fd, tmp_name_c.as_ptr(), 0) };
        return Err(e);
    }
    drop(tmp_file);
    let name_c = CString::new(name)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
    let rename_result =
        unsafe { libc::renameat(dir_fd, tmp_name_c.as_ptr(), dir_fd, name_c.as_ptr()) };
    if rename_result != 0 {
        let rename_error = io::Error::last_os_error();
        let _ = unsafe { libc::unlinkat(dir_fd, tmp_name_c.as_ptr(), 0) };
        return Err(rename_error);
    }
    let fsync_result = unsafe { libc::fsync(dir_fd) };
    if fsync_result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn unlinkat_marker(dir_fd: RawFd, name: &str) -> io::Result<()> {
    let name_c = CString::new(name)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
    let ret = unsafe { libc::unlinkat(dir_fd, name_c.as_ptr(), 0) };
    if ret != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

const MAX_MARKER_BYTES: usize = 4096;

const MAX_CONTAINER_ID_LEN: usize = 256;

fn is_valid_container_id(container_id: &str) -> bool {
    !container_id.is_empty()
        && container_id.len() <= MAX_CONTAINER_ID_LEN
        && container_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

fn read_and_verify_marker(dir_fd: RawFd, name: &str) -> io::Result<String> {
    let name_c = CString::new(name)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
    let flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC;
    let fd = unsafe { libc::openat(dir_fd, name_c.as_ptr(), flags) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut file = unsafe { <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(fd) };
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "marker entry is not a regular file",
        ));
    }
    let our_uid = unsafe { libc::geteuid() };
    if meta.uid() != our_uid {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("marker owned by uid {} (expected {our_uid})", meta.uid()),
        ));
    }
    if meta.mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "marker is group/other-accessible",
        ));
    }
    let mut buf = vec![0u8; MAX_MARKER_BYTES + 1];
    let mut total = 0usize;
    loop {
        let n = io::Read::read(&mut file, &mut buf[total..])?;
        if n == 0 {
            break;
        }
        total += n;
        if total >= buf.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "marker exceeds the maximum expected size",
            ));
        }
    }
    buf.truncate(total);
    String::from_utf8(buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
}

fn verify_ancestors_not_writable_by_us(dir: &Path) -> Result<(), UserNamespaceAllocatorError> {
    crate::dirlock::verify_ancestors_not_writable_by_us(dir).map_err(|reason| {
        UserNamespaceAllocatorError::UnsafeLeasesDir {
            leases_dir: dir.to_path_buf(),
            reason,
        }
    })
}

fn harden_and_verify_leases_dir(
    dir: &Path,
    strict: bool,
) -> Result<(), UserNamespaceAllocatorError> {
    let unsafe_dir = |reason: String| UserNamespaceAllocatorError::UnsafeLeasesDir {
        leases_dir: dir.to_path_buf(),
        reason,
    };
    if strict {
        verify_ancestors_not_writable_by_us(dir)?;
        return verify_leases_dir_leaf_strict(dir);
    }
    match std::fs::symlink_metadata(dir) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                return Err(unsafe_dir(
                    "the leases directory path is a symlink - refusing to trust a directory \
                     reached through one"
                        .to_string(),
                ));
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            std::fs::create_dir_all(dir)
                .map_err(|e| unsafe_dir(format!("create leases dir: {e}")))?;
            let mut perms = std::fs::metadata(dir)
                .map_err(|e| unsafe_dir(format!("stat freshly-created leases dir: {e}")))?
                .permissions();
            perms.set_mode(0o700);
            std::fs::set_permissions(dir, perms)
                .map_err(|e| unsafe_dir(format!("chmod 0700 leases dir: {e}")))?;
        }
        Err(e) => return Err(unsafe_dir(format!("stat leases dir: {e}"))),
    }
    let meta = std::fs::metadata(dir).map_err(|e| unsafe_dir(format!("stat leases dir: {e}")))?;
    let our_uid = unsafe { libc::geteuid() };
    if meta.uid() != our_uid {
        return Err(unsafe_dir(format!(
            "leases dir is owned by uid {} (expected this process's own euid {our_uid})",
            meta.uid()
        )));
    }
    if meta.mode() & 0o077 != 0 {
        return Err(unsafe_dir(format!(
            "leases dir mode {:o} is group/other-accessible - expected 0700 or stricter",
            meta.mode() & 0o777
        )));
    }
    Ok(())
}

fn verify_leases_dir_leaf_strict(dir: &Path) -> Result<(), UserNamespaceAllocatorError> {
    let unsafe_dir = |reason: String| UserNamespaceAllocatorError::UnsafeLeasesDir {
        leases_dir: dir.to_path_buf(),
        reason,
    };
    let meta = std::fs::symlink_metadata(dir).map_err(|e| {
        unsafe_dir(format!(
            "stat leases dir: {e} - the leases directory must be pre-provisioned in production; \
             this preflight does not create it"
        ))
    })?;
    if meta.file_type().is_symlink() {
        return Err(unsafe_dir(
            "the leases directory path is a symlink - refusing to trust a directory reached \
             through one"
                .to_string(),
        ));
    }
    if !meta.is_dir() {
        return Err(unsafe_dir(
            "the leases directory path is not a directory".to_string(),
        ));
    }
    let our_uid = unsafe { libc::geteuid() };
    if meta.uid() != our_uid {
        return Err(unsafe_dir(format!(
            "leases dir is owned by uid {} (expected this process's own euid {our_uid})",
            meta.uid()
        )));
    }
    if meta.mode() & 0o077 != 0 {
        return Err(unsafe_dir(format!(
            "leases dir mode {:o} is group/other-accessible - expected 0700 or stricter",
            meta.mode() & 0o777
        )));
    }
    if meta.mode() & 0o700 != 0o700 {
        return Err(unsafe_dir(format!(
            "leases dir mode {:o} does not grant this process's own owner bits full rwx - \
             required to create/read lease markers under it",
            meta.mode() & 0o777
        )));
    }
    Ok(())
}

pub struct UserNamespaceQuiescenceProof {
    lease_nonce: LeaseNonce,
    container_id: String,
    runsc_root_identity: (u64, u64),
    cgroup_identity: (u64, u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RuntimeEvidenceError {
    RootlessEvidence,
}

impl std::fmt::Display for RuntimeEvidenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuntimeEvidenceError::RootlessEvidence => write!(
                f,
                "the runtime evidence is Rootless - it never had a runsc-root identity to check, \
                 so it cannot back a userns lease release"
            ),
        }
    }
}

impl std::error::Error for RuntimeEvidenceError {}

impl UserNamespaceQuiescenceProof {
    pub(crate) fn from_runtime_evidence(
        lease: &UserNamespaceLease,
        evidence: &crate::gvisor::RuntimeQuiescenceEvidence,
    ) -> Result<Self, RuntimeEvidenceError> {
        let runsc_root_identity = match evidence.namespace() {
            crate::gvisor::RuntimeNamespaceQuiescence::Rootless => {
                return Err(RuntimeEvidenceError::RootlessEvidence);
            }
            crate::gvisor::RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                runsc_root_identity,
            } => runsc_root_identity,
        };
        Ok(UserNamespaceQuiescenceProof {
            lease_nonce: lease.lease_nonce,
            container_id: evidence.container_id().to_string(),
            runsc_root_identity,
            cgroup_identity: evidence.cgroup().cgroup_identity(),
        })
    }

    #[cfg(test)]
    pub(crate) fn assert_for_tests(
        lease_nonce: LeaseNonce,
        container_id: String,
        runsc_root_identity: (u64, u64),
        cgroup_identity: (u64, u64),
    ) -> Self {
        UserNamespaceQuiescenceProof {
            lease_nonce,
            container_id,
            runsc_root_identity,
            cgroup_identity,
        }
    }
}

pub(crate) struct PreparationQuiescenceProof {
    lease_nonce: LeaseNonce,
    container_id: String,
    runsc_root_identity: (u64, u64),
    cgroup_identity: (u64, u64),
}

impl PreparationQuiescenceProof {
    pub(crate) fn from_runtime_evidence(
        lease: &UserNamespaceLease,
        evidence: &crate::gvisor::RuntimeQuiescenceEvidence,
    ) -> Result<Self, RuntimeEvidenceError> {
        let runsc_root_identity = match evidence.namespace() {
            crate::gvisor::RuntimeNamespaceQuiescence::Rootless => {
                return Err(RuntimeEvidenceError::RootlessEvidence);
            }
            crate::gvisor::RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                runsc_root_identity,
            } => runsc_root_identity,
        };
        Ok(PreparationQuiescenceProof {
            lease_nonce: lease.lease_nonce,
            container_id: evidence.container_id().to_string(),
            runsc_root_identity,
            cgroup_identity: evidence.cgroup().cgroup_identity(),
        })
    }

    #[cfg(test)]
    pub(crate) fn assert_for_tests(
        lease_nonce: LeaseNonce,
        container_id: String,
        runsc_root_identity: (u64, u64),
        cgroup_identity: (u64, u64),
    ) -> Self {
        PreparationQuiescenceProof {
            lease_nonce,
            container_id,
            runsc_root_identity,
            cgroup_identity,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserNamespaceBindError {
    MarkerMismatch,
    Poisoned,
    InvalidContainerId,
    MarkerTooLarge,
    InvalidSessionState,
    LeaseMismatch,
}

impl std::fmt::Display for UserNamespaceBindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UserNamespaceBindError::MarkerMismatch => write!(
                f,
                "the durable marker no longer matches this lease's own identity in its \
                 required source phase"
            ),
            UserNamespaceBindError::Poisoned => {
                write!(f, "the durable binding transition had an ambiguous outcome")
            }
            UserNamespaceBindError::InvalidContainerId => write!(
                f,
                "container_id is empty, too long, or contains a character outside the safe subset"
            ),
            UserNamespaceBindError::MarkerTooLarge => write!(
                f,
                "the serialized target marker would exceed the maximum marker size"
            ),
            UserNamespaceBindError::InvalidSessionState => {
                write!(
                    f,
                    "the checkout session is not ready for this binding transition"
                )
            }
            UserNamespaceBindError::LeaseMismatch => write!(
                f,
                "the checkout session was asked to bind a different userns lease"
            ),
        }
    }
}

impl std::error::Error for UserNamespaceBindError {}

pub struct UserNamespaceLease {
    slot: u32,
    host_uid: u32,
    host_gid: u32,
    runner_uid: u32,
    runner_gid: u32,
    lease_nonce: LeaseNonce,
    runner_instance_id: RunnerInstanceId,
    shared: Arc<SharedState>,
    released: bool,
}

impl std::fmt::Debug for UserNamespaceLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UserNamespaceLease")
            .field("slot", &self.slot)
            .field("host_uid", &self.host_uid)
            .field("host_gid", &self.host_gid)
            .field("released", &self.released)
            .finish()
    }
}

impl UserNamespaceLease {
    pub fn host_uid(&self) -> u32 {
        self.host_uid
    }

    pub fn host_gid(&self) -> u32 {
        self.host_gid
    }

    pub fn config(&self) -> UserNamespaceConfig {
        UserNamespaceConfig {
            runner_uid: self.runner_uid,
            runner_gid: self.runner_gid,
            subordinate_uid: self.host_uid,
            subordinate_gid: self.host_gid,
        }
    }

    #[cfg(test)]
    pub(crate) fn nonce_for_tests(&self) -> LeaseNonce {
        self.lease_nonce
    }

    pub fn bind(
        &mut self,
        container_id: String,
        runsc_root_identity: (u64, u64),
        cgroup_identity: (u64, u64),
    ) -> Result<(), UserNamespaceBindError> {
        if !is_valid_container_id(&container_id) {
            return Err(UserNamespaceBindError::InvalidContainerId);
        }
        let name = marker_file_name(self.slot);
        let current = read_and_verify_marker(self.shared.dir_fd(), &name)
            .ok()
            .and_then(|content| serde_json::from_str::<LeaseMarkerV2>(&content).ok())
            .filter(|marker| {
                marker.schema_version == LEASE_MARKER_SCHEMA_V2
                    && marker.lease_nonce == self.lease_nonce
                    && marker.runner_instance_id == self.runner_instance_id
                    && marker.host_uid == self.host_uid
                    && marker.host_gid == self.host_gid
                    && marker.phase == LeasePhaseV2::Allocated
            });
        let Some(marker) = current else {
            self.released = true;
            self.shared.poison(format!(
                "binding slot {} (host_uid={}): the durable marker no longer matches this \
                 lease's own identity in the Allocated phase - treating as a global-trust failure",
                self.slot, self.host_uid
            ));
            return Err(UserNamespaceBindError::MarkerMismatch);
        };
        let bound_marker = LeaseMarkerV2 {
            phase: LeasePhaseV2::Bound {
                container_id,
                runsc_root_identity,
                cgroup_identity,
            },
            ..marker
        };
        let marker_json = match serde_json::to_string(&bound_marker) {
            Ok(json) => json,
            Err(e) => {
                self.released = true;
                self.shared.poison(format!(
                    "binding slot {} (host_uid={}): failed to serialize the Bound marker: {e}",
                    self.slot, self.host_uid
                ));
                return Err(UserNamespaceBindError::Poisoned);
            }
        };
        if marker_json.len() > MAX_MARKER_BYTES {
            return Err(UserNamespaceBindError::MarkerTooLarge);
        }
        match rewrite_marker_atomically(self.shared.dir_fd(), &name, marker_json.as_bytes()) {
            Ok(()) => Ok(()),
            Err(e) => {
                self.released = true;
                self.shared.poison(format!(
                    "binding slot {} (host_uid={}): failed to durably rewrite the marker to \
                     Bound ({e}) - the on-disk phase is now ambiguous",
                    self.slot, self.host_uid
                ));
                Err(UserNamespaceBindError::Poisoned)
            }
        }
    }

    pub(crate) fn bind_preparation(
        &mut self,
        container_id: String,
        runsc_root_identity: (u64, u64),
        cgroup_identity: (u64, u64),
    ) -> Result<(), UserNamespaceBindError> {
        if !is_valid_container_id(&container_id) {
            return Err(UserNamespaceBindError::InvalidContainerId);
        }
        let name = marker_file_name(self.slot);
        let current = read_and_verify_marker(self.shared.dir_fd(), &name)
            .ok()
            .and_then(|content| serde_json::from_str::<LeaseMarkerV2>(&content).ok())
            .filter(|marker| {
                marker.schema_version == LEASE_MARKER_SCHEMA_V2
                    && marker.lease_nonce == self.lease_nonce
                    && marker.runner_instance_id == self.runner_instance_id
                    && marker.host_uid == self.host_uid
                    && marker.host_gid == self.host_gid
                    && marker.phase == LeasePhaseV2::Allocated
            });
        let Some(marker) = current else {
            self.released = true;
            self.shared.poison(format!(
                "binding slot {} (host_uid={}) to a preparation runtime: the durable marker no \
                 longer matches this lease's own identity in the Allocated phase - treating as a \
                 global-trust failure",
                self.slot, self.host_uid
            ));
            return Err(UserNamespaceBindError::MarkerMismatch);
        };
        let bound_marker = LeaseMarkerV2 {
            phase: LeasePhaseV2::PreparationBound {
                container_id,
                runsc_root_identity,
                cgroup_identity,
            },
            ..marker
        };
        let marker_json = match serde_json::to_string(&bound_marker) {
            Ok(json) => json,
            Err(e) => {
                self.released = true;
                self.shared.poison(format!(
                    "binding slot {} (host_uid={}) to a preparation runtime: failed to serialize \
                     the PreparationBound marker: {e}",
                    self.slot, self.host_uid
                ));
                return Err(UserNamespaceBindError::Poisoned);
            }
        };
        if marker_json.len() > MAX_MARKER_BYTES {
            return Err(UserNamespaceBindError::MarkerTooLarge);
        }
        match rewrite_marker_atomically(self.shared.dir_fd(), &name, marker_json.as_bytes()) {
            Ok(()) => Ok(()),
            Err(e) => {
                self.released = true;
                self.shared.poison(format!(
                    "binding slot {} (host_uid={}) to a preparation runtime: failed to durably \
                     rewrite the marker to PreparationBound ({e}) - the on-disk phase is now \
                     ambiguous",
                    self.slot, self.host_uid
                ));
                Err(UserNamespaceBindError::Poisoned)
            }
        }
    }

    pub(crate) fn confirm_prepared(
        &mut self,
        proof: PreparationQuiescenceProof,
    ) -> Result<(), PreparationConfirmationError> {
        if proof.lease_nonce != self.lease_nonce {
            return Err(PreparationConfirmationError::ProofMismatch);
        }
        let name = marker_file_name(self.slot);
        let marker = read_and_verify_marker(self.shared.dir_fd(), &name)
            .ok()
            .and_then(|content| serde_json::from_str::<LeaseMarkerV2>(&content).ok());
        let marker = match marker {
            Some(marker)
                if marker.schema_version == LEASE_MARKER_SCHEMA_V2
                    && marker.lease_nonce == self.lease_nonce
                    && marker.runner_instance_id == self.runner_instance_id
                    && marker.host_uid == self.host_uid
                    && marker.host_gid == self.host_gid =>
            {
                marker
            }
            _ => {
                self.released = true;
                self.shared.poison(format!(
                    "confirming preparation quiescence for slot {} (host_uid={}): the durable marker \
                     no longer matches this lease's own identity (schema/nonce/runner/host_uid/ \
                     host_gid) - treating as a global-trust failure",
                    self.slot, self.host_uid
                ));
                return Err(PreparationConfirmationError::MarkerMismatch);
            }
        };
        let (
            preparation_container_id,
            preparation_runsc_root_identity,
            preparation_cgroup_identity,
        ) = match &marker.phase {
            LeasePhaseV2::PreparationBound {
                container_id,
                runsc_root_identity,
                cgroup_identity,
            } if container_id == &proof.container_id
                && runsc_root_identity == &proof.runsc_root_identity
                && cgroup_identity == &proof.cgroup_identity =>
            {
                (container_id.clone(), *runsc_root_identity, *cgroup_identity)
            }
            _ => return Err(PreparationConfirmationError::ProofDisagreesWithMarker),
        };
        let prepared_marker = LeaseMarkerV2 {
            phase: LeasePhaseV2::Prepared {
                preparation_container_id,
                preparation_runsc_root_identity,
                preparation_cgroup_identity,
            },
            ..marker
        };
        let marker_json = match serde_json::to_string(&prepared_marker) {
            Ok(json) => json,
            Err(e) => {
                self.released = true;
                self.shared.poison(format!(
                    "confirming preparation quiescence for slot {} (host_uid={}): failed to \
                     serialize the Prepared marker: {e}",
                    self.slot, self.host_uid
                ));
                return Err(PreparationConfirmationError::Poisoned);
            }
        };
        if marker_json.len() > MAX_MARKER_BYTES {
            self.released = true;
            self.shared.poison(format!(
                "confirming preparation quiescence for slot {} (host_uid={}): the serialized \
                 Prepared marker would exceed the maximum marker size",
                self.slot, self.host_uid
            ));
            return Err(PreparationConfirmationError::Poisoned);
        }
        match rewrite_marker_atomically(self.shared.dir_fd(), &name, marker_json.as_bytes()) {
            Ok(()) => Ok(()),
            Err(e) => {
                self.released = true;
                self.shared.poison(format!(
                    "confirming preparation quiescence for slot {} (host_uid={}): failed to \
                     durably rewrite the marker to Prepared ({e}) - the on-disk phase is now \
                     ambiguous",
                    self.slot, self.host_uid
                ));
                Err(PreparationConfirmationError::Poisoned)
            }
        }
    }

    pub(crate) fn release_prepared(self) -> Result<(), UserNamespaceReleaseError> {
        self.release_prepared_given(unlinkat_marker)
    }

    fn release_prepared_given(
        mut self,
        unlink: impl FnOnce(RawFd, &str) -> io::Result<()>,
    ) -> Result<(), UserNamespaceReleaseError> {
        let name = marker_file_name(self.slot);
        let marker_matches = read_and_verify_marker(self.shared.dir_fd(), &name)
            .ok()
            .and_then(|content| serde_json::from_str::<LeaseMarkerV2>(&content).ok())
            .map(|marker| {
                marker.schema_version == LEASE_MARKER_SCHEMA_V2
                    && marker.lease_nonce == self.lease_nonce
                    && marker.runner_instance_id == self.runner_instance_id
                    && marker.host_uid == self.host_uid
                    && marker.host_gid == self.host_gid
                    && matches!(marker.phase, LeasePhaseV2::Prepared { .. })
            })
            .unwrap_or(false);
        if !marker_matches {
            self.released = true;
            self.shared.poison(format!(
                "release_prepared on slot {} (host_uid={}): the durable marker is not (or no \
                 longer) Prepared matching this lease's own identity - either it was already \
                 Bound (use release() with a real workload quiescence proof instead), never \
                 reached Prepared at all, or the on-disk state has diverged; treating as a \
                 global-trust failure",
                self.slot, self.host_uid
            ));
            return Err(UserNamespaceReleaseError::MarkerMismatch);
        }
        match unlink(self.shared.dir_fd(), &name) {
            Ok(()) => match self.shared.fsync_locked_dir() {
                Ok(()) => {
                    self.released = true;
                    let removed = self.shared.lock_state().active_slots.remove(&self.slot);
                    if !removed {
                        let reason = format!(
                            "release_prepared on slot {} (host_uid={}): its marker was durably \
                             unlinked but active_slots did not contain it - a bookkeeping \
                             invariant was violated",
                            self.slot, self.host_uid
                        );
                        self.shared.poison(reason.clone());
                        return Err(UserNamespaceReleaseError::InternalInvariantViolated {
                            reason,
                        });
                    }
                    Ok(())
                }
                Err(e) => {
                    self.released = true;
                    self.shared.poison(format!(
                        "release_prepared on slot {} (host_uid={}): marker unlinked but syncing \
                         the leases directory failed ({e}) - the release outcome is ambiguous",
                        self.slot, self.host_uid
                    ));
                    Err(UserNamespaceReleaseError::Poisoned)
                }
            },
            Err(e) => {
                self.released = true;
                self.shared.poison(format!(
                    "release_prepared on slot {} (host_uid={}): failed to unlink its marker ({e}) \
                     - the release outcome is ambiguous",
                    self.slot, self.host_uid
                ));
                Err(UserNamespaceReleaseError::Poisoned)
            }
        }
    }

    pub(crate) fn bind_workload(
        &mut self,
        container_id: String,
        runsc_root_identity: (u64, u64),
        cgroup_identity: (u64, u64),
    ) -> Result<(), UserNamespaceBindError> {
        if !is_valid_container_id(&container_id) {
            return Err(UserNamespaceBindError::InvalidContainerId);
        }
        let name = marker_file_name(self.slot);
        let current = read_and_verify_marker(self.shared.dir_fd(), &name)
            .ok()
            .and_then(|content| serde_json::from_str::<LeaseMarkerV2>(&content).ok())
            .filter(|marker| {
                marker.schema_version == LEASE_MARKER_SCHEMA_V2
                    && marker.lease_nonce == self.lease_nonce
                    && marker.runner_instance_id == self.runner_instance_id
                    && marker.host_uid == self.host_uid
                    && marker.host_gid == self.host_gid
                    && matches!(marker.phase, LeasePhaseV2::Prepared { .. })
            });
        let Some(marker) = current else {
            self.released = true;
            self.shared.poison(format!(
                "binding slot {} (host_uid={}) to a workload runtime: the durable marker no \
                 longer matches this lease's own identity in the Prepared phase - treating as a \
                 global-trust failure",
                self.slot, self.host_uid
            ));
            return Err(UserNamespaceBindError::MarkerMismatch);
        };
        let bound_marker = LeaseMarkerV2 {
            phase: LeasePhaseV2::Bound {
                container_id,
                runsc_root_identity,
                cgroup_identity,
            },
            ..marker
        };
        let marker_json = match serde_json::to_string(&bound_marker) {
            Ok(json) => json,
            Err(e) => {
                self.released = true;
                self.shared.poison(format!(
                    "binding slot {} (host_uid={}) to a workload runtime: failed to serialize the \
                     Bound marker: {e}",
                    self.slot, self.host_uid
                ));
                return Err(UserNamespaceBindError::Poisoned);
            }
        };
        if marker_json.len() > MAX_MARKER_BYTES {
            return Err(UserNamespaceBindError::MarkerTooLarge);
        }
        match rewrite_marker_atomically(self.shared.dir_fd(), &name, marker_json.as_bytes()) {
            Ok(()) => Ok(()),
            Err(e) => {
                self.released = true;
                self.shared.poison(format!(
                    "binding slot {} (host_uid={}) to a workload runtime: failed to durably \
                     rewrite the marker to Bound ({e}) - the on-disk phase is now ambiguous",
                    self.slot, self.host_uid
                ));
                Err(UserNamespaceBindError::Poisoned)
            }
        }
    }

    pub fn release_unused(mut self) -> Result<(), UserNamespaceReleaseError> {
        let name = marker_file_name(self.slot);
        let marker_matches = read_and_verify_marker(self.shared.dir_fd(), &name)
            .ok()
            .and_then(|content| serde_json::from_str::<LeaseMarkerV2>(&content).ok())
            .map(|marker| {
                marker.schema_version == LEASE_MARKER_SCHEMA_V2
                    && marker.lease_nonce == self.lease_nonce
                    && marker.runner_instance_id == self.runner_instance_id
                    && marker.host_uid == self.host_uid
                    && marker.host_gid == self.host_gid
                    && marker.phase == LeasePhaseV2::Allocated
            })
            .unwrap_or(false);
        if !marker_matches {
            self.released = true;
            self.shared.poison(format!(
                "release_unused on slot {} (host_uid={}): the durable marker is not (or no \
                 longer) Allocated matching this lease's own identity - either it was already \
                 Bound (use release() with a real quiescence proof instead) or the on-disk state \
                 has diverged; treating as a global-trust failure",
                self.slot, self.host_uid
            ));
            return Err(UserNamespaceReleaseError::MarkerMismatch);
        }
        match unlinkat_marker(self.shared.dir_fd(), &name) {
            Ok(()) => match self.shared.fsync_locked_dir() {
                Ok(()) => {
                    self.released = true;
                    let removed = self.shared.lock_state().active_slots.remove(&self.slot);
                    if !removed {
                        let reason = format!(
                            "release_unused on slot {} (host_uid={}): its marker was durably \
                             unlinked but active_slots did not contain it - a bookkeeping \
                             invariant was violated",
                            self.slot, self.host_uid
                        );
                        self.shared.poison(reason.clone());
                        return Err(UserNamespaceReleaseError::InternalInvariantViolated {
                            reason,
                        });
                    }
                    Ok(())
                }
                Err(e) => {
                    self.released = true;
                    self.shared.poison(format!(
                        "release_unused on slot {} (host_uid={}): marker unlinked but syncing \
                         the leases directory failed ({e}) - the release outcome is ambiguous",
                        self.slot, self.host_uid
                    ));
                    Err(UserNamespaceReleaseError::Poisoned)
                }
            },
            Err(e) => {
                self.released = true;
                self.shared.poison(format!(
                    "release_unused on slot {} (host_uid={}): failed to unlink its marker ({e}) \
                     - the release outcome is ambiguous",
                    self.slot, self.host_uid
                ));
                Err(UserNamespaceReleaseError::Poisoned)
            }
        }
    }

    pub fn release(
        mut self,
        proof: UserNamespaceQuiescenceProof,
    ) -> Result<(), UserNamespaceReleaseError> {
        if proof.lease_nonce != self.lease_nonce {
            return Err(UserNamespaceReleaseError::ProofMismatch);
        }
        let name = marker_file_name(self.slot);
        let marker = read_and_verify_marker(self.shared.dir_fd(), &name)
            .ok()
            .and_then(|content| serde_json::from_str::<LeaseMarkerV2>(&content).ok());
        let base_identity_matches = marker.as_ref().is_some_and(|marker| {
            marker.schema_version == LEASE_MARKER_SCHEMA_V2
                && marker.lease_nonce == self.lease_nonce
                && marker.runner_instance_id == self.runner_instance_id
                && marker.host_uid == self.host_uid
                && marker.host_gid == self.host_gid
        });
        if !base_identity_matches {
            self.released = true;
            self.shared.poison(format!(
                "releasing slot {} (host_uid={}): the durable marker no longer matches this \
                 lease's own identity (schema/nonce/runner/host_uid/host_gid) - treating as a \
                 global-trust failure",
                self.slot, self.host_uid
            ));
            return Err(UserNamespaceReleaseError::MarkerMismatch);
        }
        let phase_matches_proof = marker.as_ref().is_some_and(|marker| {
            marker.phase
                == LeasePhaseV2::Bound {
                    container_id: proof.container_id.clone(),
                    runsc_root_identity: proof.runsc_root_identity,
                    cgroup_identity: proof.cgroup_identity,
                }
        });
        if !phase_matches_proof {
            return Err(UserNamespaceReleaseError::ProofDisagreesWithMarker);
        }
        match unlinkat_marker(self.shared.dir_fd(), &name) {
            Ok(()) => match self.shared.fsync_locked_dir() {
                Ok(()) => {
                    self.released = true;
                    let removed = self.shared.lock_state().active_slots.remove(&self.slot);
                    if !removed {
                        let reason = format!(
                            "releasing slot {} (host_uid={}): its marker was durably unlinked \
                             but active_slots did not contain it - a bookkeeping invariant was \
                             violated",
                            self.slot, self.host_uid
                        );
                        self.shared.poison(reason.clone());
                        return Err(UserNamespaceReleaseError::InternalInvariantViolated {
                            reason,
                        });
                    }
                    Ok(())
                }
                Err(e) => {
                    self.released = true;
                    self.shared.poison(format!(
                        "releasing slot {} (host_uid={}): marker unlinked but syncing the \
                         leases directory failed ({e}) - the release outcome is ambiguous",
                        self.slot, self.host_uid
                    ));
                    Err(UserNamespaceReleaseError::Poisoned)
                }
            },
            Err(e) => {
                self.released = true;
                self.shared.poison(format!(
                    "releasing slot {} (host_uid={}): failed to unlink its marker ({e}) - the \
                     release outcome is ambiguous",
                    self.slot, self.host_uid
                ));
                Err(UserNamespaceReleaseError::Poisoned)
            }
        }
    }
}

impl Drop for UserNamespaceLease {
    fn drop(&mut self) {
        if !self.released {
            self.shared.quarantine_slot(
                self.slot,
                format!(
                    "user-namespace lease for slot {} (host_uid={}, host_gid={}) was dropped \
                     without an explicit release - its marker is left in place; this slot is \
                     quarantined and will never be reissued by this allocator instance",
                    self.slot, self.host_uid, self.host_gid
                ),
            );
        }
    }
}

#[derive(Debug)]
pub(crate) struct CheckoutPreparationSession {
    state: CheckoutPreparationSessionState,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct WorkloadBindingIdentity {
    container_id: String,
    runsc_root_identity: (u64, u64),
    cgroup_identity: (u64, u64),
}

impl WorkloadBindingIdentity {
    #[must_use]
    pub(crate) fn into_parts(self) -> (String, (u64, u64), (u64, u64)) {
        (
            self.container_id,
            self.runsc_root_identity,
            self.cgroup_identity,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckoutPreparationSessionState {
    NotStarted,
    PreparationBound { lease_nonce: LeaseNonce },
    Prepared { lease_nonce: LeaseNonce },
    Done,
    Unreleasable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckoutSessionCleanup {
    NeverBound,
    TeardownUnproven,
    Prepared,
    WorkloadBound,
    Unreleasable,
}

impl CheckoutPreparationSession {
    pub(crate) fn new() -> Self {
        CheckoutPreparationSession {
            state: CheckoutPreparationSessionState::NotStarted,
        }
    }

    #[cfg(test)]
    pub(crate) fn is_unreleasable(&self) -> bool {
        self.state == CheckoutPreparationSessionState::Unreleasable
    }

    pub(crate) fn bind_preparation(
        &mut self,
        lease: &mut UserNamespaceLease,
        container_id: String,
        runsc_root_identity: (u64, u64),
        cgroup_identity: (u64, u64),
    ) -> Result<(), UserNamespaceBindError> {
        if self.state != CheckoutPreparationSessionState::NotStarted {
            return Err(UserNamespaceBindError::InvalidSessionState);
        }
        let lease_nonce = lease.lease_nonce;
        match lease.bind_preparation(container_id, runsc_root_identity, cgroup_identity) {
            Ok(()) => {
                self.state = CheckoutPreparationSessionState::PreparationBound { lease_nonce };
                Ok(())
            }
            Err(e @ UserNamespaceBindError::InvalidContainerId)
            | Err(e @ UserNamespaceBindError::MarkerTooLarge) => Err(e),
            Err(e) => {
                self.state = CheckoutPreparationSessionState::Unreleasable;
                Err(e)
            }
        }
    }

    pub(crate) fn confirm_prepared(
        &mut self,
        lease: &mut UserNamespaceLease,
        proof: PreparationQuiescenceProof,
    ) -> Result<(), PreparationConfirmationError> {
        let CheckoutPreparationSessionState::PreparationBound { lease_nonce } = self.state else {
            return Err(PreparationConfirmationError::InvalidSessionState);
        };
        if lease_nonce != lease.lease_nonce {
            return Err(PreparationConfirmationError::LeaseMismatch);
        }
        match lease.confirm_prepared(proof) {
            Ok(()) => {
                self.state = CheckoutPreparationSessionState::Prepared { lease_nonce };
                Ok(())
            }
            Err(e) => {
                self.state = CheckoutPreparationSessionState::Unreleasable;
                Err(e)
            }
        }
    }

    pub(crate) fn release_prepared(
        self,
        lease: UserNamespaceLease,
    ) -> Result<(), UserNamespaceReleaseError> {
        let CheckoutPreparationSessionState::Prepared { lease_nonce } = self.state else {
            return Err(UserNamespaceReleaseError::InvalidSessionState);
        };
        if lease_nonce != lease.lease_nonce {
            return Err(UserNamespaceReleaseError::LeaseMismatch);
        }
        lease.release_prepared()
    }

    pub(crate) fn bind_workload(
        &mut self,
        lease: &mut UserNamespaceLease,
        container_id: String,
        runsc_root_identity: (u64, u64),
        cgroup_identity: (u64, u64),
    ) -> Result<WorkloadBindingIdentity, UserNamespaceBindError> {
        let CheckoutPreparationSessionState::Prepared { lease_nonce } = self.state else {
            return Err(UserNamespaceBindError::InvalidSessionState);
        };
        if lease_nonce != lease.lease_nonce {
            return Err(UserNamespaceBindError::LeaseMismatch);
        }
        match lease.bind_workload(container_id.clone(), runsc_root_identity, cgroup_identity) {
            Ok(()) => {
                self.state = CheckoutPreparationSessionState::Done;
                Ok(WorkloadBindingIdentity {
                    container_id,
                    runsc_root_identity,
                    cgroup_identity,
                })
            }
            Err(e @ UserNamespaceBindError::InvalidContainerId)
            | Err(e @ UserNamespaceBindError::MarkerTooLarge) => Err(e),
            Err(e) => {
                self.state = CheckoutPreparationSessionState::Unreleasable;
                Err(e)
            }
        }
    }

    pub(crate) fn cleanup_disposition(&self) -> CheckoutSessionCleanup {
        match self.state {
            CheckoutPreparationSessionState::NotStarted => CheckoutSessionCleanup::NeverBound,
            CheckoutPreparationSessionState::PreparationBound { .. } => {
                CheckoutSessionCleanup::TeardownUnproven
            }
            CheckoutPreparationSessionState::Prepared { .. } => CheckoutSessionCleanup::Prepared,
            CheckoutPreparationSessionState::Done => CheckoutSessionCleanup::WorkloadBound,
            CheckoutPreparationSessionState::Unreleasable => CheckoutSessionCleanup::Unreleasable,
        }
    }
}

pub struct UserNamespaceAllocator {
    leases_dir: PathBuf,
    pool_size: u32,
    uid_start: u32,
    gid_start: u32,
    runner_uid: u32,
    runner_gid: u32,
    runner_instance_id: RunnerInstanceId,
    shared: Arc<SharedState>,
}

impl UserNamespaceAllocator {
    pub fn try_new(
        leases_dir: PathBuf,
        min_pool_size: u32,
        incident_sink: IncidentSink,
    ) -> Result<Self, UserNamespaceAllocatorError> {
        Self::try_new_impl(
            leases_dir,
            Path::new("/etc/subuid"),
            Path::new("/etc/subgid"),
            min_pool_size,
            true,
            true,
            incident_sink,
        )
    }

    /// Builds the allocator for an explicitly selected single-user development runner.
    ///
    /// The system-owned subordinate-ID files remain strictly validated. Only the lease
    /// directory's ancestor rule is relaxed, allowing ephemeral state below the developer's own
    /// private data directory. Production callers must use [`Self::try_new`].
    pub fn try_new_local_development(
        leases_dir: PathBuf,
        min_pool_size: u32,
        incident_sink: IncidentSink,
    ) -> Result<Self, UserNamespaceAllocatorError> {
        Self::try_new_impl(
            leases_dir,
            Path::new("/etc/subuid"),
            Path::new("/etc/subgid"),
            min_pool_size,
            true,
            false,
            incident_sink,
        )
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn try_new_for_tests(
        leases_dir: PathBuf,
        subuid_path: &Path,
        subgid_path: &Path,
        min_pool_size: u32,
        incident_sink: IncidentSink,
    ) -> Result<Self, UserNamespaceAllocatorError> {
        const MAX_ATTEMPTS: u32 = 20;
        for attempt in 1..=MAX_ATTEMPTS {
            match Self::try_new_impl(
                leases_dir.clone(),
                subuid_path,
                subgid_path,
                min_pool_size,
                false,
                false,
                incident_sink.clone(),
            ) {
                Err(UserNamespaceAllocatorError::AlreadyLocked { .. })
                    if attempt < MAX_ATTEMPTS =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    continue;
                }
                result => return result,
            }
        }
        unreachable!("loop always returns on its final attempt");
    }

    fn try_new_impl(
        leases_dir: PathBuf,
        subuid_path: &Path,
        subgid_path: &Path,
        min_pool_size: u32,
        strict_subordinate_config: bool,
        strict_leases_dir: bool,
        incident_sink: IncidentSink,
    ) -> Result<Self, UserNamespaceAllocatorError> {
        let runner_uid = unsafe { libc::geteuid() };
        let runner_gid = unsafe { libc::getegid() };
        if runner_uid == 0 || runner_gid == 0 {
            return Err(UserNamespaceAllocatorError::PrivilegedRunner {
                euid: runner_uid,
                egid: runner_gid,
            });
        }
        let runner_instance_id = runner_instance_id()
            .map_err(|reason| UserNamespaceAllocatorError::EntropyUnavailable { reason })?;
        let username = effective_username();
        let uid_range = parse_subordinate_range(
            subuid_path,
            runner_uid,
            username.as_deref(),
            strict_subordinate_config,
        )?;
        let gid_range = parse_subordinate_range(
            subgid_path,
            runner_uid,
            username.as_deref(),
            strict_subordinate_config,
        )?;
        if range_contains(uid_range, 0) || range_contains(uid_range, runner_uid) {
            return Err(UserNamespaceAllocatorError::SubordinateConfig {
                path: subuid_path.to_path_buf(),
                reason: format!(
                    "subordinate uid range {uid_range:?} must not contain 0 or this process's own \
                     euid {runner_uid}"
                ),
            });
        }
        if range_contains(gid_range, 0) || range_contains(gid_range, runner_gid) {
            return Err(UserNamespaceAllocatorError::SubordinateConfig {
                path: subgid_path.to_path_buf(),
                reason: format!(
                    "subordinate gid range {gid_range:?} must not contain 0 or this process's own \
                     egid {runner_gid}"
                ),
            });
        }
        let pool_size = uid_range.count.min(gid_range.count);
        debug_assert!(
            pool_size > 0,
            "parse_subordinate_range already refuses a zero count on either file"
        );
        if pool_size < min_pool_size {
            return Err(UserNamespaceAllocatorError::PoolTooSmall {
                pool_size,
                required: min_pool_size,
            });
        }

        harden_and_verify_leases_dir(&leases_dir, strict_leases_dir)?;
        let lock =
            crate::dirlock::acquire_directory_lock(&leases_dir).map_err(|error| match error {
                crate::dirlock::DirLockError::AlreadyLocked => {
                    UserNamespaceAllocatorError::AlreadyLocked {
                        leases_dir: leases_dir.clone(),
                    }
                }
                crate::dirlock::DirLockError::Failed(reason) => {
                    UserNamespaceAllocatorError::LockFailed {
                        leases_dir: leases_dir.clone(),
                        reason,
                    }
                }
            })?;
        let locked_identity = crate::dirlock::fd_identity(&lock).map_err(|e| {
            UserNamespaceAllocatorError::LockFailed {
                leases_dir: leases_dir.clone(),
                reason: format!("fstat locked directory: {e}"),
            }
        })?;

        let shared = Arc::new(SharedState {
            _lock: lock,
            state: Mutex::new(AllocatorState {
                admission: UserNamespaceAdmission::Healthy,
                quarantined_slots: BTreeSet::new(),
                active_slots: BTreeSet::new(),
                locked_identity: Some(locked_identity),
            }),
            incident_sink,
        });

        let mut quarantined = BTreeSet::new();
        let mut incidents = Vec::new();
        let mut stray_tmp_entries: Vec<(u32, String, PathBuf)> = Vec::new();
        for entry in std::fs::read_dir(shared.listing_path()).map_err(|e| {
            UserNamespaceAllocatorError::LockFailed {
                leases_dir: leases_dir.clone(),
                reason: format!("read_dir via locked fd: {e}"),
            }
        })? {
            let entry = entry.map_err(|e| UserNamespaceAllocatorError::CorruptLeaseMarker {
                path: leases_dir.clone(),
                reason: format!("read_dir entry: {e}"),
            })?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            let Some(slot) = parse_marker_file_name(&name_str) else {
                if let Some(tmp_slot) = parse_stray_tmp_marker_file_name(&name_str) {
                    if tmp_slot >= pool_size {
                        return Err(UserNamespaceAllocatorError::CorruptLeaseMarker {
                            path: entry.path(),
                            reason: format!(
                                "stale bind-rewrite temp file {name_str:?} names slot \
                                 {tmp_slot}, outside the current pool size {pool_size}"
                            ),
                        });
                    }
                    stray_tmp_entries.push((tmp_slot, name_str.into_owned(), entry.path()));
                    continue;
                }
                return Err(UserNamespaceAllocatorError::CorruptLeaseMarker {
                    path: entry.path(),
                    reason: format!("unrecognized entry in leases dir: {name_str:?}"),
                });
            };
            let content = read_and_verify_marker(shared.dir_fd(), &name_str).map_err(|e| {
                UserNamespaceAllocatorError::CorruptLeaseMarker {
                    path: entry.path(),
                    reason: format!("read marker: {e}"),
                }
            })?;
            let peek: SchemaPeek = serde_json::from_str(&content).map_err(|e| {
                UserNamespaceAllocatorError::CorruptLeaseMarker {
                    path: entry.path(),
                    reason: format!("marker is not valid JSON / has no schema_version: {e}"),
                }
            })?;
            match peek.schema_version {
                v if v == LEASE_MARKER_SCHEMA_V1 => {
                    let marker: LeaseMarkerV1 = serde_json::from_str(&content).map_err(|e| {
                        UserNamespaceAllocatorError::CorruptLeaseMarker {
                            path: entry.path(),
                            reason: format!(
                                "marker claims schema_version=1 but does not parse as \
                                 LeaseMarkerV1: {e}"
                            ),
                        }
                    })?;
                    if slot >= pool_size {
                        return Err(UserNamespaceAllocatorError::CorruptLeaseMarker {
                            path: entry.path(),
                            reason: format!(
                                "marker names slot {slot}, outside the current pool size \
                                 {pool_size} - subordinate-range configuration likely changed \
                                 incompatibly since this marker was written"
                            ),
                        });
                    }
                    let expected_uid = uid_range.start + slot;
                    let expected_gid = gid_range.start + slot;
                    if marker.host_uid != expected_uid || marker.host_gid != expected_gid {
                        return Err(UserNamespaceAllocatorError::CorruptLeaseMarker {
                            path: entry.path(),
                            reason: format!(
                                "marker for slot {slot} names host_uid={}, host_gid={}, but the \
                                 CURRENT subordinate ranges imply host_uid={expected_uid}, \
                                 host_gid={expected_gid} for this slot - the range start likely \
                                 changed since this marker was written; refusing to guess which \
                                 identity is authoritative",
                                marker.host_uid, marker.host_gid
                            ),
                        });
                    }
                    quarantined.insert(slot);
                    let phase_desc = match &marker.phase {
                        LeasePhaseV1::Allocated => "Allocated".to_string(),
                        LeasePhaseV1::Bound { container_id, .. } => {
                            format!("Bound (container_id={container_id:?})")
                        }
                    };
                    incidents.push(format!(
                        "boot reconciliation: slot {slot} (host_uid={}, host_gid={}) has a \
                         surviving legacy schema_version=1 {phase_desc} marker from runner \
                         instance {:?} - quarantined, will never be reissued by this allocator \
                         instance",
                        marker.host_uid, marker.host_gid, marker.runner_instance_id
                    ));
                }
                v if v == LEASE_MARKER_SCHEMA_V2 => {
                    let marker: LeaseMarkerV2 = serde_json::from_str(&content).map_err(|e| {
                        UserNamespaceAllocatorError::CorruptLeaseMarker {
                            path: entry.path(),
                            reason: format!(
                                "marker claims schema_version=2 but does not parse as \
                                 LeaseMarkerV2: {e}"
                            ),
                        }
                    })?;
                    if slot >= pool_size {
                        return Err(UserNamespaceAllocatorError::CorruptLeaseMarker {
                            path: entry.path(),
                            reason: format!(
                                "marker names slot {slot}, outside the current pool size \
                                 {pool_size} - subordinate-range configuration likely changed \
                                 incompatibly since this marker was written"
                            ),
                        });
                    }
                    let expected_uid = uid_range.start + slot;
                    let expected_gid = gid_range.start + slot;
                    if marker.host_uid != expected_uid || marker.host_gid != expected_gid {
                        return Err(UserNamespaceAllocatorError::CorruptLeaseMarker {
                            path: entry.path(),
                            reason: format!(
                                "marker for slot {slot} names host_uid={}, host_gid={}, but the \
                                 CURRENT subordinate ranges imply host_uid={expected_uid}, \
                                 host_gid={expected_gid} for this slot - the range start likely \
                                 changed since this marker was written; refusing to guess which \
                                 identity is authoritative",
                                marker.host_uid, marker.host_gid
                            ),
                        });
                    }
                    quarantined.insert(slot);
                    let phase_desc = match &marker.phase {
                        LeasePhaseV2::Allocated => "Allocated".to_string(),
                        LeasePhaseV2::PreparationBound { container_id, .. } => {
                            format!("PreparationBound (container_id={container_id:?})")
                        }
                        LeasePhaseV2::Prepared {
                            preparation_container_id,
                            ..
                        } => format!(
                            "Prepared (preparation_container_id={preparation_container_id:?})"
                        ),
                        LeasePhaseV2::Bound { container_id, .. } => {
                            format!("Bound (container_id={container_id:?})")
                        }
                    };
                    incidents.push(format!(
                        "boot reconciliation: slot {slot} (host_uid={}, host_gid={}) has a \
                         surviving {phase_desc} marker from runner instance {:?} - quarantined, \
                         will never be reissued by this allocator instance",
                        marker.host_uid, marker.host_gid, marker.runner_instance_id
                    ));
                }
                other => {
                    return Err(UserNamespaceAllocatorError::CorruptLeaseMarker {
                        path: entry.path(),
                        reason: format!("unrecognized schema_version: {other}"),
                    });
                }
            }
        }

        let had_stray_tmp_entries = !stray_tmp_entries.is_empty();
        for (tmp_slot, tmp_name, tmp_path) in stray_tmp_entries {
            if !quarantined.contains(&tmp_slot) {
                return Err(UserNamespaceAllocatorError::CorruptLeaseMarker {
                    path: tmp_path,
                    reason: format!(
                        "stale bind-rewrite temp file {tmp_name:?} names slot {tmp_slot}, but no \
                         primary marker for that slot survived - refusing to guess whether the \
                         slot is safe to reissue"
                    ),
                });
            }
            unlinkat_marker(shared.dir_fd(), &tmp_name).map_err(|e| {
                UserNamespaceAllocatorError::CorruptLeaseMarker {
                    path: tmp_path.clone(),
                    reason: format!(
                        "failed to remove stale bind-rewrite temp file {tmp_name:?}, alongside \
                         its slot's already-quarantined primary marker: {e}"
                    ),
                }
            })?;
            incidents.push(format!(
                "boot reconciliation: slot {tmp_slot} had a stale bind-rewrite temp file \
                 ({tmp_name:?}) alongside its durably quarantined primary marker - removed"
            ));
        }
        if had_stray_tmp_entries {
            shared
                .fsync_locked_dir()
                .map_err(|e| UserNamespaceAllocatorError::LockFailed {
                    leases_dir: leases_dir.clone(),
                    reason: format!(
                        "syncing the leases directory after removing stale bind-rewrite temp \
                         file(s) failed: {e}"
                    ),
                })?;
        }

        {
            let mut state = shared.lock_state();
            state.quarantined_slots = quarantined;
        }
        for message in incidents {
            shared.report_incident(&message);
        }

        Ok(Self {
            leases_dir,
            pool_size,
            uid_start: uid_range.start,
            gid_start: gid_range.start,
            runner_uid,
            runner_gid,
            runner_instance_id,
            shared,
        })
    }

    pub fn admission(&self) -> UserNamespaceAdmission {
        self.shared.lock_state().admission.clone()
    }

    pub fn is_healthy(&self) -> bool {
        matches!(self.admission(), UserNamespaceAdmission::Healthy)
    }

    pub fn pool_size(&self) -> u32 {
        self.pool_size
    }

    pub fn quarantined_slots(&self) -> BTreeSet<u32> {
        self.shared.lock_state().quarantined_slots.clone()
    }

    pub fn check_identity(&self) -> Result<(), UserNamespaceRefusal> {
        let state = self.shared.lock_state();
        let Some(locked_identity) = state.locked_identity else {
            drop(state);
            let reason = "userns allocator has no recorded locked-directory identity".to_string();
            self.shared.poison(reason.clone());
            return Err(UserNamespaceRefusal::Poisoned { reason });
        };
        drop(state);
        match crate::dirlock::path_identity(&self.leases_dir) {
            Ok(current) if current == locked_identity => Ok(()),
            Ok(_) => {
                let reason = format!(
                    "{:?} no longer names the directory this allocator locked at construction",
                    self.leases_dir
                );
                self.shared.poison(reason.clone());
                Err(UserNamespaceRefusal::Poisoned { reason })
            }
            Err(e) => {
                let reason = format!("stat {:?}: {e}", self.leases_dir);
                self.shared.poison(reason.clone());
                Err(UserNamespaceRefusal::Poisoned { reason })
            }
        }
    }

    pub fn lease(&self) -> Result<UserNamespaceLease, UserNamespaceRefusal> {
        let mut state = self.shared.lock_state();
        if let UserNamespaceAdmission::Poisoned { reason } = &state.admission {
            return Err(UserNamespaceRefusal::Poisoned {
                reason: reason.clone(),
            });
        }

        for slot in 0..self.pool_size {
            if state.active_slots.contains(&slot) || state.quarantined_slots.contains(&slot) {
                continue;
            }
            let host_uid = self.uid_start + slot;
            let host_gid = self.gid_start + slot;
            let lease_nonce = match random_u128() {
                Ok(n) => LeaseNonce(n),
                Err(e) => {
                    let reason = format!("lease: failed to generate a lease nonce: {e}");
                    state.admission = UserNamespaceAdmission::Poisoned {
                        reason: reason.clone(),
                    };
                    drop(state);
                    self.shared.report_incident(&reason);
                    return Err(UserNamespaceRefusal::Poisoned { reason });
                }
            };
            let marker = LeaseMarkerV2 {
                schema_version: LEASE_MARKER_SCHEMA_V2,
                lease_nonce,
                runner_instance_id: self.runner_instance_id,
                host_uid,
                host_gid,
                created_at_unix_secs: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
                phase: LeasePhaseV2::Allocated,
            };
            let marker_json = match serde_json::to_string(&marker) {
                Ok(json) => json,
                Err(e) => {
                    let reason = format!("lease: failed to serialize a new marker: {e}");
                    state.admission = UserNamespaceAdmission::Poisoned {
                        reason: reason.clone(),
                    };
                    drop(state);
                    self.shared.report_incident(&reason);
                    return Err(UserNamespaceRefusal::Poisoned { reason });
                }
            };
            let name = marker_file_name(slot);
            let write_result =
                openat_marker(self.shared.dir_fd(), &name, true).and_then(|mut file| {
                    io::Write::write_all(&mut file, marker_json.as_bytes())?;
                    file.sync_all()
                });
            match write_result {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    let reason = format!(
                        "lease: slot {slot} already has an untracked marker (neither \
                         quarantined nor currently leased by this allocator instance) - the \
                         leases directory was modified outside this allocator's own bookkeeping"
                    );
                    state.admission = UserNamespaceAdmission::Poisoned {
                        reason: reason.clone(),
                    };
                    drop(state);
                    self.shared.report_incident(&reason);
                    return Err(UserNamespaceRefusal::Poisoned { reason });
                }
                Err(e) => {
                    let reason = format!(
                        "lease: creating marker for slot {slot} had an ambiguous outcome: {e}"
                    );
                    state.admission = UserNamespaceAdmission::Poisoned {
                        reason: reason.clone(),
                    };
                    drop(state);
                    self.shared.report_incident(&reason);
                    return Err(UserNamespaceRefusal::Poisoned { reason });
                }
            }
            if let Err(e) = self.shared.fsync_locked_dir() {
                let reason = format!(
                    "lease: slot {slot}'s marker was written but syncing the leases directory \
                     failed ({e}) - the marker's durability is unproven"
                );
                state.admission = UserNamespaceAdmission::Poisoned {
                    reason: reason.clone(),
                };
                drop(state);
                self.shared.report_incident(&reason);
                return Err(UserNamespaceRefusal::Poisoned { reason });
            }
            if let Err(reason) = insert_active_slot_checked(&mut state.active_slots, slot) {
                let reason = format!("lease: {reason}");
                state.admission = UserNamespaceAdmission::Poisoned {
                    reason: reason.clone(),
                };
                drop(state);
                self.shared.report_incident(&reason);
                return Err(UserNamespaceRefusal::Poisoned { reason });
            }
            drop(state);

            return Ok(UserNamespaceLease {
                slot,
                host_uid,
                host_gid,
                runner_uid: self.runner_uid,
                runner_gid: self.runner_gid,
                lease_nonce,
                runner_instance_id: self.runner_instance_id,
                shared: Arc::clone(&self.shared),
                released: false,
            });
        }
        Err(UserNamespaceRefusal::PoolExhausted {
            pool_size: self.pool_size,
        })
    }
}

#[cfg(test)]
mod tests {
    // these tests exercise real uid/lease semantics; a fake-root environment
    // (euid 0, e.g. this crate's own tests running inside myelin's CI sandbox)
    // cannot express them - the allocator itself refuses a privileged runner.
    // the skip is loud, and MYELIN_REQUIRE_USERNS_TESTS=1 turns it into a
    // hard failure on hosts that must prove the semantics.
    macro_rules! require_unprivileged_euid {
        () => {
            if unsafe { libc::geteuid() } == 0 {
                if std::env::var_os("MYELIN_REQUIRE_USERNS_TESTS").is_some() {
                    panic!(
                        "MYELIN_REQUIRE_USERNS_TESTS=1 but this environment reports euid=0"
                    );
                }
                eprintln!(
                    "SKIP (loud, NOT a silent pass): euid=0 (fake-root) cannot express \
                     distinct-uid lease semantics; run on an unprivileged host to prove them"
                );
                return;
            }
        };
    }

    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex as StdMutex;

    #[test]
    fn runner_instance_identity_requires_a_complete_entropy_read() {
        let bytes: Vec<u8> = (0..16).collect();
        let identity = runner_instance_id_from(std::io::Cursor::new(bytes.clone()))
            .expect("sixteen entropy bytes mint an instance identity");
        assert_eq!(
            identity,
            RunnerInstanceId(u128::from_le_bytes(bytes.try_into().unwrap()))
        );

        let error = runner_instance_id_from(std::io::Cursor::new([0_u8; 15]))
            .expect_err("a short entropy read must never mint a predictable identity");
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }

    fn unique_suffix() -> u64 {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        NEXT.fetch_add(1, Ordering::Relaxed)
    }

    fn test_base(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "myelin-user-namespace-tests-{tag}-{}-{}",
            std::process::id(),
            unique_suffix()
        ))
    }

    fn recording_sink() -> (IncidentSink, Arc<StdMutex<Vec<String>>>) {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let log_for_sink = Arc::clone(&log);
        let sink: IncidentSink = Arc::new(move |message: &str| {
            log_for_sink.lock().unwrap().push(message.to_string());
        });
        (sink, log)
    }

    fn write_subordinate_file(path: &Path, start: u32, count: u32) {
        let uid = unsafe { libc::geteuid() };
        std::fs::write(path, format!("{uid}:{start}:{count}\n")).unwrap();
    }

    fn new_allocator_for_test(
        tag: &str,
        uid_count: u32,
        gid_count: u32,
    ) -> (UserNamespaceAllocator, PathBuf, Arc<StdMutex<Vec<String>>>) {
        let base = test_base(tag);
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, uid_count);
        write_subordinate_file(&subgid, 200_000, gid_count);
        let (sink, log) = recording_sink();
        let allocator = UserNamespaceAllocator::try_new_for_tests(
            leases_dir.clone(),
            &subuid,
            &subgid,
            uid_count.min(gid_count),
            sink,
        )
        .unwrap();
        (allocator, base, log)
    }

    fn release_for_tests(mut lease: UserNamespaceLease) {
        let nonce = lease.nonce_for_tests();
        lease
            .bind("test-container".to_string(), (0, 0), (0, 0))
            .expect("bind must succeed for a fresh Allocated lease");
        lease
            .release(UserNamespaceQuiescenceProof::assert_for_tests(
                nonce,
                "test-container".to_string(),
                (0, 0),
                (0, 0),
            ))
            .expect("release with the lease's own nonce and bound identity must succeed");
    }

    fn create_hardened_leases_dir(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        let mut perms = std::fs::metadata(dir).unwrap().permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(dir, perms).unwrap();
    }

    #[test]
    fn subordinate_range_parsing_rejects_a_missing_entry() {
        require_unprivileged_euid!();
        let base = test_base("subrange-missing-entry");
        std::fs::create_dir_all(&base).unwrap();
        let subuid = base.join("subuid");
        std::fs::write(&subuid, "someoneelse:100000:65536\n").unwrap();
        let result = parse_subordinate_range(&subuid, unsafe { libc::geteuid() }, None, false);
        assert!(matches!(
            result,
            Err(UserNamespaceAllocatorError::NoSubordinateEntry { .. })
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn subordinate_range_parsing_rejects_a_zero_count() {
        require_unprivileged_euid!();
        let base = test_base("subrange-zero-count");
        std::fs::create_dir_all(&base).unwrap();
        let subuid = base.join("subuid");
        let uid = unsafe { libc::geteuid() };
        std::fs::write(&subuid, format!("{uid}:100000:0\n")).unwrap();
        let result = parse_subordinate_range(&subuid, uid, None, false);
        assert!(matches!(
            result,
            Err(UserNamespaceAllocatorError::SubordinateConfig { .. })
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn subordinate_range_parsing_rejects_ambiguous_duplicate_entries() {
        require_unprivileged_euid!();
        let base = test_base("subrange-ambiguous");
        std::fs::create_dir_all(&base).unwrap();
        let subuid = base.join("subuid");
        let uid = unsafe { libc::geteuid() };
        std::fs::write(&subuid, format!("{uid}:100000:65536\n{uid}:200000:1000\n")).unwrap();
        let result = parse_subordinate_range(&subuid, uid, None, false);
        assert!(matches!(
            result,
            Err(UserNamespaceAllocatorError::SubordinateConfig { .. })
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn subordinate_range_parsing_rejects_an_overlap_with_another_owners_range() {
        require_unprivileged_euid!();
        let base = test_base("subrange-overlap");
        std::fs::create_dir_all(&base).unwrap();
        let subuid = base.join("subuid");
        let uid = unsafe { libc::geteuid() };
        std::fs::write(
            &subuid,
            format!("{uid}:100000:65536\nsomeoneelse:165000:10000\n"),
        )
        .unwrap();
        let result = parse_subordinate_range(&subuid, uid, None, false);
        assert!(matches!(
            result,
            Err(UserNamespaceAllocatorError::SubordinateConfig { .. })
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn subordinate_range_parsing_accepts_a_non_overlapping_other_owner_entry() {
        require_unprivileged_euid!();
        let base = test_base("subrange-no-overlap");
        std::fs::create_dir_all(&base).unwrap();
        let subuid = base.join("subuid");
        let uid = unsafe { libc::geteuid() };
        std::fs::write(
            &subuid,
            format!("{uid}:100000:65536\nsomeoneelse:200000:65536\n"),
        )
        .unwrap();
        let result = parse_subordinate_range(&subuid, uid, None, false).unwrap();
        assert_eq!(
            result,
            SubordinateRange {
                start: 100000,
                count: 65536
            }
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn subordinate_range_parsing_rejects_overflowing_ranges() {
        require_unprivileged_euid!();
        let base = test_base("subrange-overflow");
        std::fs::create_dir_all(&base).unwrap();
        let subuid = base.join("subuid");
        let uid = unsafe { libc::geteuid() };
        std::fs::write(&subuid, format!("{uid}:{}:100\n", u32::MAX - 1)).unwrap();
        let result = parse_subordinate_range(&subuid, uid, None, false);
        assert!(matches!(
            result,
            Err(UserNamespaceAllocatorError::SubordinateConfig { .. })
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn subordinate_range_parsing_accepts_a_username_match() {
        require_unprivileged_euid!();
        let base = test_base("subrange-username-match");
        std::fs::create_dir_all(&base).unwrap();
        let subuid = base.join("subuid");
        std::fs::write(&subuid, "totally-not-our-uid:100000:65536\n").unwrap();
        let result = parse_subordinate_range(&subuid, 0, Some("totally-not-our-uid"), false);
        assert_eq!(
            result.unwrap(),
            SubordinateRange {
                start: 100_000,
                count: 65_536
            }
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn subordinate_range_parsing_rejects_a_symlinked_file() {
        require_unprivileged_euid!();
        let base = test_base("subrange-symlink-refused");
        std::fs::create_dir_all(&base).unwrap();
        let real = base.join("real-subuid");
        let uid = unsafe { libc::geteuid() };
        std::fs::write(&real, format!("{uid}:100000:65536\n")).unwrap();
        let link = base.join("subuid-link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let result = parse_subordinate_range(&link, uid, None, false);
        assert!(matches!(
            result,
            Err(UserNamespaceAllocatorError::SubordinateConfig { .. })
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn pool_size_is_the_minimum_of_the_two_ranges() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("pool-size-min", 5, 3);
        assert_eq!(allocator.pool_size(), 3);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn two_concurrent_leases_get_distinct_uid_gid_pairs() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("distinct-pairs", 5, 5);
        let lease_a = allocator.lease().unwrap();
        let lease_b = allocator.lease().unwrap();
        assert_ne!(lease_a.host_uid(), lease_b.host_uid());
        assert_ne!(lease_a.host_gid(), lease_b.host_gid());
        release_for_tests(lease_a);
        release_for_tests(lease_b);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn concurrent_lease_calls_never_poison_the_allocator() {
        require_unprivileged_euid!();
        const THREADS: u32 = 8;
        let (allocator, base, _log) = new_allocator_for_test("real-concurrency", THREADS, THREADS);
        let allocator = Arc::new(allocator);
        let barrier = Arc::new(std::sync::Barrier::new(THREADS as usize));
        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let allocator = Arc::clone(&allocator);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    allocator.lease()
                })
            })
            .collect();
        let leases: Vec<UserNamespaceLease> = handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap()
                    .expect("every concurrent lease() call must succeed, never observe poisoning")
            })
            .collect();
        let mut host_uids = BTreeSet::new();
        for lease in &leases {
            assert!(
                host_uids.insert(lease.host_uid()),
                "two threads leased the SAME host_uid - the race this test targets"
            );
        }
        assert!(allocator.is_healthy());
        for lease in leases {
            release_for_tests(lease);
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn lease_config_reports_the_exact_two_entry_mapping_shape() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("config-shape", 5, 5);
        let lease = allocator.lease().unwrap();
        let config = lease.config();
        assert_eq!(config.runner_uid(), unsafe { libc::geteuid() });
        assert_eq!(config.runner_gid(), unsafe { libc::getegid() });
        assert_eq!(config.subordinate_uid(), lease.host_uid());
        assert_eq!(config.subordinate_gid(), lease.host_gid());
        release_for_tests(lease);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn pool_exhaustion_is_a_typed_refusal_not_poisoning() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("pool-exhaustion", 2, 2);
        let lease_a = allocator.lease().unwrap();
        let lease_b = allocator.lease().unwrap();
        let refusal = allocator.lease().unwrap_err();
        assert_eq!(
            refusal,
            UserNamespaceRefusal::PoolExhausted { pool_size: 2 }
        );
        assert!(
            allocator.is_healthy(),
            "pool exhaustion must never poison the allocator"
        );
        release_for_tests(lease_a);
        release_for_tests(lease_b);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn releasing_a_lease_frees_its_slot_for_reuse() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("release-frees-slot", 1, 1);
        let lease = allocator.lease().unwrap();
        let freed_uid = lease.host_uid();
        release_for_tests(lease);
        let lease_again = allocator.lease().unwrap();
        assert_eq!(lease_again.host_uid(), freed_uid);
        release_for_tests(lease_again);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn from_runtime_evidence_mints_a_matching_proof_and_releases() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("from-runtime-evidence-ok", 1, 1);
        let mut lease = allocator.lease().unwrap();
        lease
            .bind("container-1".to_string(), (7, 8), (9, 10))
            .expect("bind must succeed for a fresh Allocated lease");
        let evidence = crate::gvisor::RuntimeQuiescenceEvidence::assert_for_tests(
            "container-1".to_string(),
            crate::gvisor::RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                runsc_root_identity: (7, 8),
            },
            crate::gvisor::CgroupQuiescenceEvidence::assert_for_tests((9, 10)),
        );
        let proof = UserNamespaceQuiescenceProof::from_runtime_evidence(&lease, &evidence)
            .expect("matching ExplicitUserNamespace evidence must mint a proof");
        lease
            .release(proof)
            .expect("release with the minted proof must succeed");
        assert!(allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn from_runtime_evidence_refuses_rootless_evidence() {
        require_unprivileged_euid!();
        let (allocator, base, _log) =
            new_allocator_for_test("from-runtime-evidence-rootless", 1, 1);
        let lease = allocator.lease().unwrap();
        let evidence = crate::gvisor::RuntimeQuiescenceEvidence::assert_for_tests(
            "container-1".to_string(),
            crate::gvisor::RuntimeNamespaceQuiescence::Rootless,
            crate::gvisor::CgroupQuiescenceEvidence::assert_for_tests((9, 10)),
        );
        let result = UserNamespaceQuiescenceProof::from_runtime_evidence(&lease, &evidence);
        assert!(matches!(
            result,
            Err(RuntimeEvidenceError::RootlessEvidence)
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn bind_then_release_succeeds_with_a_matching_proof() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("bind-then-release", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let nonce = lease.nonce_for_tests();
        lease
            .bind("container-1".to_string(), (7, 8), (9, 10))
            .expect("bind must succeed for a fresh Allocated lease");
        lease
            .release(UserNamespaceQuiescenceProof::assert_for_tests(
                nonce,
                "container-1".to_string(),
                (7, 8),
                (9, 10),
            ))
            .expect("release with a proof matching the bound identity must succeed");
        assert!(allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn bind_refuses_a_lease_that_is_already_bound() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("bind-twice", 1, 1);
        let mut lease = allocator.lease().unwrap();
        lease
            .bind("container-1".to_string(), (1, 1), (1, 1))
            .expect("first bind must succeed");
        let result = lease.bind("container-2".to_string(), (2, 2), (2, 2));
        assert_eq!(result, Err(UserNamespaceBindError::MarkerMismatch));
        assert!(
            !allocator.is_healthy(),
            "a second bind attempt on an already-Bound marker must poison the allocator"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn bind_refuses_an_oversized_container_id_without_rewriting_the_marker() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("bind-oversized-id", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let marker_path = base.join("leases").join(marker_file_name(0));
        let before = std::fs::read_to_string(&marker_path).unwrap();
        let oversized_id = "x".repeat(MAX_CONTAINER_ID_LEN + 1);
        let result = lease.bind(oversized_id, (1, 1), (1, 1));
        assert_eq!(result, Err(UserNamespaceBindError::InvalidContainerId));
        let after = std::fs::read_to_string(&marker_path).unwrap();
        assert_eq!(
            before, after,
            "an invalid container_id must be refused before any disk write is attempted"
        );
        assert!(
            allocator.is_healthy(),
            "an oversized container_id is a caller bug, not a global-trust failure - it must \
             not poison the allocator"
        );
        lease
            .release_unused()
            .expect("the lease is still Allocated and usable after the refused bind");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn bind_refuses_a_container_id_with_an_unsafe_character() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("bind-unsafe-char", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let result = lease.bind("has a space".to_string(), (1, 1), (1, 1));
        assert_eq!(result, Err(UserNamespaceBindError::InvalidContainerId));
        assert!(allocator.is_healthy());
        lease.release_unused().expect("lease is still Allocated");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn release_refuses_a_proof_whose_bound_identity_disagrees_with_the_durable_marker() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("release-wrong-identity", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let nonce = lease.nonce_for_tests();
        lease
            .bind("real-container".to_string(), (1, 1), (2, 2))
            .expect("bind must succeed");
        let result = lease.release(UserNamespaceQuiescenceProof::assert_for_tests(
            nonce,
            "different-container".to_string(),
            (1, 1),
            (2, 2),
        ));
        assert_eq!(
            result,
            Err(UserNamespaceReleaseError::ProofDisagreesWithMarker)
        );
        assert!(
            allocator.is_healthy(),
            "a proof with the wrong bound identity is an ordinary wrong proof for a valid lease, \
             not corruption - it must NOT poison the whole allocator"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn release_refuses_a_lease_that_was_never_bound() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("release-never-bound", 1, 1);
        let lease = allocator.lease().unwrap();
        let nonce = lease.nonce_for_tests();
        let result = lease.release(UserNamespaceQuiescenceProof::assert_for_tests(
            nonce,
            "container-1".to_string(),
            (1, 1),
            (1, 1),
        ));
        assert_eq!(
            result,
            Err(UserNamespaceReleaseError::ProofDisagreesWithMarker)
        );
        assert!(
            allocator.is_healthy(),
            "a never-bound marker still genuinely belongs to this lease - releasing it with a \
             real-looking proof is an ordinary wrong proof, not corruption"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn release_unused_succeeds_for_a_never_bound_lease_and_frees_its_slot() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("release-unused-ok", 1, 1);
        let lease = allocator.lease().unwrap();
        let freed_uid = lease.host_uid();
        lease
            .release_unused()
            .expect("release_unused must succeed for a never-bound Allocated lease");
        assert!(allocator.is_healthy());
        let lease_again = allocator.lease().unwrap();
        assert_eq!(
            lease_again.host_uid(),
            freed_uid,
            "release_unused must genuinely free the slot for reuse"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn release_unused_refuses_a_lease_that_was_already_bound() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("release-unused-wrong-path", 1, 1);
        let mut lease = allocator.lease().unwrap();
        lease
            .bind("container-1".to_string(), (1, 1), (1, 1))
            .expect("bind must succeed");
        let result = lease.release_unused();
        assert_eq!(result, Err(UserNamespaceReleaseError::MarkerMismatch));
        assert!(
            !allocator.is_healthy(),
            "release_unused on an already-Bound lease must poison the allocator, not silently \
             unlink real runtime-binding evidence"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn release_surfaces_an_internal_invariant_violation_when_active_slots_lost_the_entry() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("release-invariant-violation", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let nonce = lease.nonce_for_tests();
        lease
            .bind("container-1".to_string(), (1, 1), (1, 1))
            .expect("bind must succeed");
        let slot = lease.slot;
        lease.shared.lock_state().active_slots.remove(&slot);
        let result = lease.release(UserNamespaceQuiescenceProof::assert_for_tests(
            nonce,
            "container-1".to_string(),
            (1, 1),
            (1, 1),
        ));
        match result {
            Err(UserNamespaceReleaseError::InternalInvariantViolated { reason }) => {
                assert!(reason.contains("bookkeeping invariant"));
            }
            other => panic!("expected InternalInvariantViolated, got {other:?}"),
        }
        assert!(
            !allocator.is_healthy(),
            "a lost active_slots entry is a genuine bookkeeping corruption and must poison the \
             whole allocator"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn release_unused_surfaces_an_internal_invariant_violation_when_active_slots_lost_the_entry() {
        require_unprivileged_euid!();
        let (allocator, base, _log) =
            new_allocator_for_test("release-unused-invariant-violation", 1, 1);
        let lease = allocator.lease().unwrap();
        let slot = lease.slot;
        lease.shared.lock_state().active_slots.remove(&slot);
        let result = lease.release_unused();
        match result {
            Err(UserNamespaceReleaseError::InternalInvariantViolated { reason }) => {
                assert!(reason.contains("bookkeeping invariant"));
            }
            other => panic!("expected InternalInvariantViolated, got {other:?}"),
        }
        assert!(!allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn insert_active_slot_checked_detects_a_bookkeeping_invariant_violation() {
        require_unprivileged_euid!();
        let mut active_slots = BTreeSet::new();
        active_slots.insert(3);
        let result = insert_active_slot_checked(&mut active_slots, 3);
        assert!(result.unwrap_err().contains("bookkeeping invariant"));
    }

    #[test]
    fn insert_active_slot_checked_succeeds_for_a_fresh_slot() {
        require_unprivileged_euid!();
        let mut active_slots = BTreeSet::new();
        assert!(insert_active_slot_checked(&mut active_slots, 3).is_ok());
        assert!(active_slots.contains(&3));
    }

    #[test]
    fn a_proof_minted_for_one_lease_cannot_release_another() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("proof-cannot-cross-leases", 2, 2);
        let lease_a = allocator.lease().unwrap();
        let lease_b = allocator.lease().unwrap();
        let wrong_proof = UserNamespaceQuiescenceProof::assert_for_tests(
            lease_a.nonce_for_tests(),
            "irrelevant".to_string(),
            (0, 0),
            (0, 0),
        );
        let result = lease_b.release(wrong_proof);
        assert_eq!(result, Err(UserNamespaceReleaseError::ProofMismatch));
        assert!(
            allocator.is_healthy(),
            "a proof mismatch must not poison the WHOLE allocator"
        );
        assert!(
            allocator.quarantined_slots().contains(&lease_b_slot_hint()),
            "lease_b's slot must be quarantined once its (consumed-by-the-failed-call) value drops"
        );
        release_for_tests(lease_a);
        let _ = std::fs::remove_dir_all(&base);
    }

    fn lease_b_slot_hint() -> u32 {
        1
    }

    #[test]
    fn abandoning_a_lease_quarantines_only_that_slot_and_reports_an_incident() {
        require_unprivileged_euid!();
        let (allocator, base, log) = new_allocator_for_test("abandon-quarantines-one", 2, 2);
        let lease_a = allocator.lease().unwrap();
        let slot_a_uid = lease_a.host_uid();
        drop(lease_a);
        assert!(
            log.lock()
                .unwrap()
                .iter()
                .any(|m| m.contains("quarantined")),
            "an abandoned lease must report an incident"
        );
        assert!(
            allocator.is_healthy(),
            "an abandoned lease quarantines ONE slot, never the whole allocator"
        );
        let lease_b = allocator.lease().unwrap();
        assert_ne!(lease_b.host_uid(), slot_a_uid);
        let refusal = allocator.lease().unwrap_err();
        assert_eq!(
            refusal,
            UserNamespaceRefusal::PoolExhausted { pool_size: 2 }
        );
        release_for_tests(lease_b);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_second_allocator_over_the_same_leases_dir_refuses_the_lock() {
        require_unprivileged_euid!();
        let base = test_base("second-allocator-refused");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 5);
        write_subordinate_file(&subgid, 200_000, 5);
        let (sink_a, _log_a) = recording_sink();
        let _allocator_a = UserNamespaceAllocator::try_new_for_tests(
            leases_dir.clone(),
            &subuid,
            &subgid,
            1,
            sink_a,
        )
        .unwrap();
        let (sink_b, _log_b) = recording_sink();
        let result_b =
            UserNamespaceAllocator::try_new_for_tests(leases_dir, &subuid, &subgid, 1, sink_b);
        assert!(matches!(
            result_b,
            Err(UserNamespaceAllocatorError::AlreadyLocked { .. })
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn dropping_the_allocator_while_a_lease_is_outstanding_keeps_the_lock_held() {
        require_unprivileged_euid!();
        let base = test_base("lock-outlives-allocator-via-lease");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 5);
        write_subordinate_file(&subgid, 200_000, 5);
        let (sink, _log) = recording_sink();
        let allocator = UserNamespaceAllocator::try_new_for_tests(
            leases_dir.clone(),
            &subuid,
            &subgid,
            1,
            sink,
        )
        .unwrap();
        let lease = allocator.lease().unwrap();
        drop(allocator);

        let (second_sink, _second_log) = recording_sink();
        let second_attempt =
            UserNamespaceAllocator::try_new_for_tests(leases_dir, &subuid, &subgid, 1, second_sink);
        match second_attempt {
            Err(UserNamespaceAllocatorError::AlreadyLocked { .. }) => {}
            Err(other) => panic!("expected AlreadyLocked, got a different error: {other:?}"),
            Ok(_) => panic!(
                "expected a second allocator to be refused while the first allocator's lease is \
                 still outstanding, but it succeeded"
            ),
        }

        drop(lease);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_genuinely_random_abandoned_lease_survives_reopening_and_is_quarantined() {
        require_unprivileged_euid!();
        let base = test_base("real-random-marker-survives-reboot");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 5);
        write_subordinate_file(&subgid, 200_000, 5);

        let (sink, _log) = recording_sink();
        let allocator = UserNamespaceAllocator::try_new_for_tests(
            leases_dir.clone(),
            &subuid,
            &subgid,
            1,
            sink,
        )
        .unwrap();
        let lease = allocator.lease().unwrap();
        let leaked_uid = lease.host_uid();
        drop(lease);
        drop(allocator);

        let (sink2, log2) = recording_sink();
        let reopened =
            UserNamespaceAllocator::try_new_for_tests(leases_dir, &subuid, &subgid, 1, sink2)
                .unwrap();
        assert!(
            reopened.is_healthy(),
            "a real random-u128 marker must parse successfully at boot, not be treated as corrupt"
        );
        assert!(reopened.quarantined_slots().contains(&0));
        assert!(log2
            .lock()
            .unwrap()
            .iter()
            .any(|m| m.contains("surviving Allocated marker")),);
        let lease2 = reopened.lease().unwrap();
        assert_ne!(
            lease2.host_uid(),
            leaked_uid,
            "the leaked slot's host_uid must never be reissued"
        );
        release_for_tests(lease2);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_bound_lease_survives_reopening_and_is_quarantined() {
        require_unprivileged_euid!();
        let base = test_base("bound-marker-survives-reboot");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 5);
        write_subordinate_file(&subgid, 200_000, 5);

        let (sink, _log) = recording_sink();
        let allocator = UserNamespaceAllocator::try_new_for_tests(
            leases_dir.clone(),
            &subuid,
            &subgid,
            1,
            sink,
        )
        .unwrap();
        let mut lease = allocator.lease().unwrap();
        let leaked_uid = lease.host_uid();
        lease
            .bind("crashed-container".to_string(), (7, 8), (9, 10))
            .expect("bind must succeed for a fresh Allocated lease");
        drop(lease);
        drop(allocator);

        let (sink2, log2) = recording_sink();
        let reopened =
            UserNamespaceAllocator::try_new_for_tests(leases_dir, &subuid, &subgid, 1, sink2)
                .unwrap();
        assert!(
            reopened.is_healthy(),
            "a surviving Bound marker must parse successfully at boot, not be treated as corrupt"
        );
        assert!(reopened.quarantined_slots().contains(&0));
        assert!(log2
            .lock()
            .unwrap()
            .iter()
            .any(|m| m.contains("Bound") && m.contains("crashed-container")));
        let lease2 = reopened.lease().unwrap();
        assert_ne!(
            lease2.host_uid(),
            leaked_uid,
            "the leaked slot's host_uid must never be reissued"
        );
        release_for_tests(lease2);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_legacy_schema_v1_bound_marker_survives_reopening_and_is_quarantined() {
        require_unprivileged_euid!();
        let base = test_base("legacy-v1-marker-survives-reboot");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 5);
        write_subordinate_file(&subgid, 200_000, 5);
        create_hardened_leases_dir(&leases_dir);
        let legacy_marker = LeaseMarkerV1 {
            schema_version: LEASE_MARKER_SCHEMA_V1,
            lease_nonce: LeaseNonce(1),
            runner_instance_id: RunnerInstanceId(1),
            host_uid: 100_000,
            host_gid: 200_000,
            created_at_unix_secs: 0,
            phase: LeasePhaseV1::Bound {
                container_id: "pre-5b1-container".to_string(),
                runsc_root_identity: (7, 8),
                cgroup_identity: (9, 10),
            },
        };
        let legacy_marker_path = leases_dir.join(marker_file_name(0));
        std::fs::write(
            &legacy_marker_path,
            serde_json::to_string(&legacy_marker).unwrap(),
        )
        .unwrap();
        std::fs::set_permissions(&legacy_marker_path, std::fs::Permissions::from_mode(0o600))
            .unwrap();

        let (sink, log) = recording_sink();
        let allocator = UserNamespaceAllocator::try_new_for_tests(
            leases_dir.clone(),
            &subuid,
            &subgid,
            1,
            sink,
        )
        .unwrap();
        assert!(
            allocator.is_healthy(),
            "a legacy schema_version=1 marker must parse successfully at boot, not be treated as \
             corrupt"
        );
        assert!(allocator.quarantined_slots().contains(&0));
        assert!(log
            .lock()
            .unwrap()
            .iter()
            .any(|m| m.contains("legacy schema_version=1")
                && m.contains("Bound")
                && m.contains("pre-5b1-container")));
        let fresh = allocator.lease().unwrap();
        let fresh_marker: String =
            std::fs::read_to_string(leases_dir.join(marker_file_name(fresh.host_uid() - 100_000)))
                .unwrap();
        assert!(
            fresh_marker.contains("\"schema_version\":2"),
            "every NEW marker this process mints must be schema_version 2, never 1 again"
        );
        release_for_tests(fresh);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_stray_bind_tmp_file_survives_reopening_and_only_quarantines_its_own_slot() {
        require_unprivileged_euid!();
        let base = test_base("stray-bind-tmp-survives-reboot");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 5);
        write_subordinate_file(&subgid, 200_000, 5);

        let (sink, _log) = recording_sink();
        let allocator = UserNamespaceAllocator::try_new_for_tests(
            leases_dir.clone(),
            &subuid,
            &subgid,
            2,
            sink,
        )
        .unwrap();
        let lease_a = allocator.lease().unwrap();
        let lease_b = allocator.lease().unwrap();
        let leaked_uid = lease_a.host_uid();
        std::fs::write(
            leases_dir.join(format!("{}.tmp", marker_file_name(0))),
            b"whatever bind() had written - content is irrelevant, only the name matters",
        )
        .unwrap();
        drop(lease_a);
        drop(lease_b);
        drop(allocator);

        let (sink2, log2) = recording_sink();
        let reopened = UserNamespaceAllocator::try_new_for_tests(
            leases_dir.clone(),
            &subuid,
            &subgid,
            2,
            sink2,
        )
        .unwrap();
        assert!(
            reopened.is_healthy(),
            "a stray bind-rewrite temp file must never poison the whole allocator"
        );
        assert!(
            reopened.quarantined_slots().contains(&0),
            "the slot the stray temp file names must still be quarantined, conservatively"
        );
        assert!(reopened.quarantined_slots().contains(&1));
        assert!(log2
            .lock()
            .unwrap()
            .iter()
            .any(|m| m.contains("stale bind-rewrite temp file")));
        assert!(
            !leases_dir
                .join(format!("{}.tmp", marker_file_name(0)))
                .exists(),
            "the stray temp file must be cleaned up once its slot is quarantined"
        );
        let lease2 = reopened.lease().unwrap();
        assert_ne!(lease2.host_uid(), leaked_uid);
        release_for_tests(lease2);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_stray_bind_tmp_file_without_its_primary_marker_refuses_construction() {
        require_unprivileged_euid!();
        let base = test_base("stray-bind-tmp-without-primary");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 2);
        write_subordinate_file(&subgid, 200_000, 2);
        let (sink, _log) = recording_sink();
        drop(
            UserNamespaceAllocator::try_new_for_tests(
                leases_dir.clone(),
                &subuid,
                &subgid,
                2,
                sink,
            )
            .unwrap(),
        );
        std::fs::write(
            leases_dir.join(format!("{}.tmp", marker_file_name(0))),
            b"whatever bind() had written - content is irrelevant, only the name matters",
        )
        .unwrap();

        let (sink2, _log2) = recording_sink();
        let result = UserNamespaceAllocator::try_new_for_tests(
            leases_dir.clone(),
            &subuid,
            &subgid,
            2,
            sink2,
        );
        match result {
            Err(UserNamespaceAllocatorError::CorruptLeaseMarker { reason, .. }) => {
                assert!(
                    reason.contains("no primary marker for that slot survived"),
                    "unexpected reason: {reason}"
                );
            }
            Err(other) => panic!("expected CorruptLeaseMarker, got {other:?}"),
            Ok(_) => panic!("expected CorruptLeaseMarker, got Ok"),
        }
        assert!(
            leases_dir
                .join(format!("{}.tmp", marker_file_name(0)))
                .exists(),
            "refusing construction must leave the only surviving evidence untouched, not delete it"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn boot_reconciliation_poisons_construction_on_an_unrecognized_entry() {
        require_unprivileged_euid!();
        let base = test_base("boot-poisons-on-unrecognized-entry");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 5);
        write_subordinate_file(&subgid, 200_000, 5);
        create_hardened_leases_dir(&leases_dir);
        std::fs::write(leases_dir.join("not-a-marker.txt"), b"garbage").unwrap();

        let (sink, _log) = recording_sink();
        let result =
            UserNamespaceAllocator::try_new_for_tests(leases_dir, &subuid, &subgid, 1, sink);
        assert!(matches!(
            result,
            Err(UserNamespaceAllocatorError::CorruptLeaseMarker { .. })
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn boot_reconciliation_poisons_construction_on_an_unknown_schema_version() {
        require_unprivileged_euid!();
        let base = test_base("boot-poisons-on-unknown-schema");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 5);
        write_subordinate_file(&subgid, 200_000, 5);
        create_hardened_leases_dir(&leases_dir);
        std::fs::write(
            leases_dir.join(marker_file_name(0)),
            r#"{"schema_version": 999, "nonsense": true}"#,
        )
        .unwrap();

        let (sink, _log) = recording_sink();
        let result =
            UserNamespaceAllocator::try_new_for_tests(leases_dir, &subuid, &subgid, 1, sink);
        assert!(matches!(
            result,
            Err(UserNamespaceAllocatorError::CorruptLeaseMarker { .. })
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn boot_reconciliation_poisons_construction_when_a_marker_names_an_out_of_range_slot() {
        require_unprivileged_euid!();
        let base = test_base("boot-poisons-out-of-range-slot");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 2);
        write_subordinate_file(&subgid, 200_000, 2);
        create_hardened_leases_dir(&leases_dir);
        let marker = LeaseMarkerV2 {
            schema_version: LEASE_MARKER_SCHEMA_V2,
            lease_nonce: LeaseNonce(1),
            runner_instance_id: RunnerInstanceId(1),
            host_uid: 100_005,
            host_gid: 200_005,
            created_at_unix_secs: 0,
            phase: LeasePhaseV2::Allocated,
        };
        std::fs::write(
            leases_dir.join(marker_file_name(5)),
            serde_json::to_string(&marker).unwrap(),
        )
        .unwrap();

        let (sink, _log) = recording_sink();
        let result =
            UserNamespaceAllocator::try_new_for_tests(leases_dir, &subuid, &subgid, 1, sink);
        assert!(matches!(
            result,
            Err(UserNamespaceAllocatorError::CorruptLeaseMarker { .. })
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn boot_reconciliation_poisons_construction_on_a_range_start_mismatch() {
        require_unprivileged_euid!();
        let base = test_base("boot-poisons-range-start-mismatch");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_005, 5);
        write_subordinate_file(&subgid, 200_005, 5);
        create_hardened_leases_dir(&leases_dir);
        let marker = LeaseMarkerV2 {
            schema_version: LEASE_MARKER_SCHEMA_V2,
            lease_nonce: LeaseNonce(1),
            runner_instance_id: RunnerInstanceId(1),
            host_uid: 100_000,
            host_gid: 200_000,
            created_at_unix_secs: 0,
            phase: LeasePhaseV2::Allocated,
        };
        std::fs::write(
            leases_dir.join(marker_file_name(0)),
            serde_json::to_string(&marker).unwrap(),
        )
        .unwrap();

        let (sink, _log) = recording_sink();
        let result =
            UserNamespaceAllocator::try_new_for_tests(leases_dir, &subuid, &subgid, 1, sink);
        assert!(matches!(
            result,
            Err(UserNamespaceAllocatorError::CorruptLeaseMarker { .. })
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn boot_reconciliation_poisons_construction_on_a_symlinked_marker_entry() {
        require_unprivileged_euid!();
        let base = test_base("boot-poisons-symlinked-marker");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 5);
        write_subordinate_file(&subgid, 200_000, 5);
        create_hardened_leases_dir(&leases_dir);
        let real = base.join("real-marker.json");
        std::fs::write(&real, b"irrelevant").unwrap();
        std::os::unix::fs::symlink(&real, leases_dir.join(marker_file_name(0))).unwrap();

        let (sink, _log) = recording_sink();
        let result =
            UserNamespaceAllocator::try_new_for_tests(leases_dir, &subuid, &subgid, 1, sink);
        assert!(matches!(
            result,
            Err(UserNamespaceAllocatorError::CorruptLeaseMarker { .. })
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn leases_dir_as_a_symlink_is_refused() {
        require_unprivileged_euid!();
        let base = test_base("leases-dir-symlink-refused");
        std::fs::create_dir_all(&base).unwrap();
        let real = base.join("real-leases");
        std::fs::create_dir_all(&real).unwrap();
        let link = base.join("leases-link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 5);
        write_subordinate_file(&subgid, 200_000, 5);

        let (sink, _log) = recording_sink();
        let result = UserNamespaceAllocator::try_new_for_tests(link, &subuid, &subgid, 1, sink);
        assert!(matches!(
            result,
            Err(UserNamespaceAllocatorError::UnsafeLeasesDir { .. })
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_panicking_incident_sink_never_escapes_an_abandoned_lease() {
        require_unprivileged_euid!();
        let base = test_base("panicking-sink-abandon");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 5);
        write_subordinate_file(&subgid, 200_000, 5);
        let sink: IncidentSink = Arc::new(|_message: &str| panic!("injected sink panic"));
        let allocator =
            UserNamespaceAllocator::try_new_for_tests(leases_dir, &subuid, &subgid, 1, sink)
                .unwrap();
        let lease = allocator.lease().unwrap();
        drop(lease);
        assert!(allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn check_identity_succeeds_while_the_leases_dir_is_unchanged() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("check-identity-happy-path", 5, 5);
        assert!(allocator.check_identity().is_ok());
        assert!(allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn check_identity_detects_a_replaced_leases_dir_and_poisons_the_allocator() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("check-identity-replaced", 5, 5);
        let leases_dir = base.join("leases");
        std::fs::remove_dir_all(&leases_dir).unwrap();
        std::fs::create_dir_all(&leases_dir).expect("recreate a replacement directory");
        let result = allocator.check_identity();
        assert!(matches!(result, Err(UserNamespaceRefusal::Poisoned { .. })));
        assert!(!allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn strict_construction_refuses_a_leases_dir_whose_parent_is_writable_by_us() {
        require_unprivileged_euid!();
        let base = test_base("strict-refuses-writable-parent");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        create_hardened_leases_dir(&leases_dir);
        let result = verify_ancestors_not_writable_by_us(&leases_dir);
        assert!(matches!(
            result,
            Err(UserNamespaceAllocatorError::UnsafeLeasesDir { .. })
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn verify_leases_dir_leaf_strict_refuses_an_owner_non_writable_directory() {
        require_unprivileged_euid!();
        let base = test_base("leases-dir-0500");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        std::fs::create_dir_all(&leases_dir).unwrap();
        let mut perms = std::fs::metadata(&leases_dir).unwrap().permissions();
        perms.set_mode(0o500);
        std::fs::set_permissions(&leases_dir, perms).unwrap();
        let result = verify_leases_dir_leaf_strict(&leases_dir);
        let mut restore = std::fs::metadata(&leases_dir).unwrap().permissions();
        restore.set_mode(0o700);
        std::fs::set_permissions(&leases_dir, restore).unwrap();
        let _ = std::fs::remove_dir_all(&base);
        assert!(
            result.is_err(),
            "an owner-non-writable leases dir must be refused even with no group/other bits: \
             {result:?}"
        );
    }

    #[test]
    fn ancestor_owned_by_us_is_refused_even_when_its_current_mode_denies_write() {
        require_unprivileged_euid!();
        let base = test_base("ancestor-owned-read-only");
        std::fs::create_dir_all(&base).unwrap();
        let mut perms = std::fs::metadata(&base).unwrap().permissions();
        perms.set_mode(0o555);
        std::fs::set_permissions(&base, perms).unwrap();

        let base_c = CString::new(base.as_os_str().as_encoded_bytes()).unwrap();
        let base_fd = unsafe {
            libc::open(
                base_c.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        };
        assert!(
            base_fd >= 0,
            "open base dir: {}",
            io::Error::last_os_error()
        );
        use std::os::fd::FromRawFd;
        let owned_fd = unsafe { OwnedFd::from_raw_fd(base_fd) };
        let result = crate::dirlock::check_ancestor_not_owned_or_writable(&owned_fd, &base);

        let mut restore = std::fs::metadata(&base).unwrap().permissions();
        restore.set_mode(0o755);
        std::fs::set_permissions(&base, restore).unwrap();
        assert!(
            result.is_err(),
            "an ancestor owned by this process must be refused regardless of its current mode, \
             since ownership alone permits chmod'ing it writable: {result:?}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    #[cfg_attr(
        not(feature = "privileged-host-tests"),
        ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) - run on the host lane with --features privileged-host-tests"
    )]
    fn open_dir_component_no_follow_refuses_a_symlinked_component() {
        let base = test_base("symlinked-ancestor");
        std::fs::create_dir_all(&base).unwrap();
        let real_dir = base.join("real");
        std::fs::create_dir_all(&real_dir).unwrap();
        let symlink_name = "sym";
        std::os::unix::fs::symlink(&real_dir, base.join(symlink_name)).unwrap();

        let base_c = CString::new(base.as_os_str().as_encoded_bytes()).unwrap();
        let base_fd = unsafe {
            libc::open(
                base_c.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        };
        assert!(
            base_fd >= 0,
            "open base dir: {}",
            io::Error::last_os_error()
        );
        let name_c = CString::new(symlink_name).unwrap();
        let result = crate::dirlock::open_dir_component_no_follow(base_fd, &name_c);
        unsafe { libc::close(base_fd) };
        assert!(
            result.is_err(),
            "a symlinked component must be refused rather than followed"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn try_new_refuses_a_pool_smaller_than_the_callers_stated_minimum() {
        require_unprivileged_euid!();
        let base = test_base("pool-too-small");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 2);
        write_subordinate_file(&subgid, 200_000, 2);
        let (sink, _log) = recording_sink();
        let result =
            UserNamespaceAllocator::try_new_for_tests(leases_dir, &subuid, &subgid, 5, sink);
        assert_eq!(
            result.err().map(|e| matches!(
                e,
                UserNamespaceAllocatorError::PoolTooSmall {
                    pool_size: 2,
                    required: 5
                }
            )),
            Some(true)
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn try_new_refuses_a_subordinate_uid_range_containing_the_runners_own_euid() {
        require_unprivileged_euid!();
        let base = test_base("subrange-contains-runner-euid");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        let runner_uid = unsafe { libc::geteuid() };
        let start = runner_uid.saturating_sub(10);
        write_subordinate_file(&subuid, start, 20);
        write_subordinate_file(&subgid, 200_000, 5);
        let (sink, _log) = recording_sink();
        let result =
            UserNamespaceAllocator::try_new_for_tests(leases_dir, &subuid, &subgid, 1, sink);
        assert!(matches!(
            result,
            Err(UserNamespaceAllocatorError::SubordinateConfig { .. })
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn try_new_refuses_a_subordinate_range_containing_uid_zero() {
        require_unprivileged_euid!();
        let base = test_base("subrange-contains-zero");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 0, 5);
        write_subordinate_file(&subgid, 200_000, 5);
        let (sink, _log) = recording_sink();
        let result =
            UserNamespaceAllocator::try_new_for_tests(leases_dir, &subuid, &subgid, 1, sink);
        assert!(matches!(
            result,
            Err(UserNamespaceAllocatorError::SubordinateConfig { .. })
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn lease_poisons_on_an_untracked_marker_it_never_issued_or_quarantined() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("untracked-marker-poisons", 5, 5);
        let leases_dir = base.join("leases");
        let marker = LeaseMarkerV2 {
            schema_version: LEASE_MARKER_SCHEMA_V2,
            lease_nonce: LeaseNonce(1),
            runner_instance_id: RunnerInstanceId(1),
            host_uid: 100_000,
            host_gid: 200_000,
            created_at_unix_secs: 0,
            phase: LeasePhaseV2::Allocated,
        };
        std::fs::write(
            leases_dir.join(marker_file_name(0)),
            serde_json::to_string(&marker).unwrap(),
        )
        .unwrap();
        let result = allocator.lease();
        assert!(matches!(result, Err(UserNamespaceRefusal::Poisoned { .. })));
        assert!(!allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn lease_never_recreates_a_slot_whose_marker_was_externally_deleted_while_still_active() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("active-marker-deleted", 1, 1);
        let leases_dir = base.join("leases");
        let lease = allocator.lease().unwrap();
        std::fs::remove_file(leases_dir.join(marker_file_name(0))).unwrap();
        let result = allocator.lease();
        assert_eq!(
            result.unwrap_err(),
            UserNamespaceRefusal::PoolExhausted { pool_size: 1 },
            "a slot this allocator still considers active must never be recreated, even if its \
             on-disk marker is gone"
        );
        let _ = lease.release_unused();
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn lease_never_reissues_a_quarantined_slot_even_after_its_marker_is_externally_deleted() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("quarantined-marker-deleted", 1, 1);
        let leases_dir = base.join("leases");
        let lease = allocator.lease().unwrap();
        drop(lease);
        assert!(
            allocator.is_healthy(),
            "abandonment must quarantine only the one slot, never poison the whole allocator"
        );
        std::fs::remove_file(leases_dir.join(marker_file_name(0))).unwrap();
        let result = allocator.lease();
        assert_eq!(
            result.unwrap_err(),
            UserNamespaceRefusal::PoolExhausted { pool_size: 1 },
            "a quarantined slot must never be reissued, even after its on-disk marker is gone"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn release_detects_an_externally_deleted_marker_as_tampering_and_poisons() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("release-detects-deletion", 1, 1);
        let leases_dir = base.join("leases");
        let lease = allocator.lease().unwrap();
        std::fs::remove_file(leases_dir.join(marker_file_name(0))).unwrap();
        let nonce = lease.nonce_for_tests();
        let result = lease.release(UserNamespaceQuiescenceProof::assert_for_tests(
            nonce,
            "test-container".to_string(),
            (0, 0),
            (0, 0),
        ));
        assert_eq!(result, Err(UserNamespaceReleaseError::MarkerMismatch));
        assert!(
            !allocator.is_healthy(),
            "an externally deleted marker must poison the whole allocator, not just this slot"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn bind_preparation_succeeds_from_allocated_and_transitions_to_preparation_bound() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("prep-bind-ok", 1, 1);
        let mut lease = allocator.lease().unwrap();
        lease
            .bind_preparation("prep-container".to_string(), (1, 1), (2, 2))
            .expect("bind_preparation must succeed for a fresh Allocated lease");
        assert!(allocator.is_healthy());
        let result = lease.bind_workload("workload-container".to_string(), (3, 3), (4, 4));
        assert_eq!(result, Err(UserNamespaceBindError::MarkerMismatch));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn bind_preparation_refuses_a_lease_that_is_already_preparation_bound() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("prep-bind-twice", 1, 1);
        let mut lease = allocator.lease().unwrap();
        lease
            .bind_preparation("prep-1".to_string(), (1, 1), (1, 1))
            .expect("first bind_preparation must succeed");
        let result = lease.bind_preparation("prep-2".to_string(), (2, 2), (2, 2));
        assert_eq!(result, Err(UserNamespaceBindError::MarkerMismatch));
        assert!(
            !allocator.is_healthy(),
            "a second bind_preparation attempt on an already-PreparationBound marker must poison \
             the allocator"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn bind_preparation_refuses_an_oversized_container_id_without_rewriting_the_marker() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("prep-bind-oversized-id", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let marker_path = base.join("leases").join(marker_file_name(0));
        let before = std::fs::read_to_string(&marker_path).unwrap();
        let oversized_id = "x".repeat(MAX_CONTAINER_ID_LEN + 1);
        let result = lease.bind_preparation(oversized_id, (1, 1), (1, 1));
        assert_eq!(result, Err(UserNamespaceBindError::InvalidContainerId));
        let after = std::fs::read_to_string(&marker_path).unwrap();
        assert_eq!(
            before, after,
            "an invalid container_id must be refused before any disk write is attempted"
        );
        assert!(
            allocator.is_healthy(),
            "an oversized container_id is a caller bug, not a global-trust failure"
        );
        lease
            .release_unused()
            .expect("the lease is still Allocated and usable after the refused bind_preparation");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn bind_preparation_poisons_on_an_ambiguous_rewrite_failure() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("prep-bind-rewrite-fails", 1, 1);
        let leases_dir = base.join("leases");
        let mut lease = allocator.lease().unwrap();
        std::fs::write(
            leases_dir.join(format!("{}.tmp", marker_file_name(0))),
            b"stray tmp file blocking the real rewrite's O_EXCL create",
        )
        .unwrap();
        let result = lease.bind_preparation("prep-container".to_string(), (1, 1), (2, 2));
        assert_eq!(result, Err(UserNamespaceBindError::Poisoned));
        assert!(
            !allocator.is_healthy(),
            "an ambiguous durable rewrite outcome must poison the whole allocator"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn confirm_prepared_poisons_on_an_ambiguous_rewrite_failure() {
        require_unprivileged_euid!();
        let (allocator, base, _log) =
            new_allocator_for_test("confirm-prepared-rewrite-fails", 1, 1);
        let leases_dir = base.join("leases");
        let mut lease = allocator.lease().unwrap();
        let nonce = lease.nonce_for_tests();
        lease
            .bind_preparation("prep-container".to_string(), (1, 1), (2, 2))
            .expect("bind_preparation must succeed");
        std::fs::write(
            leases_dir.join(format!("{}.tmp", marker_file_name(0))),
            b"stray tmp file blocking the real rewrite's O_EXCL create",
        )
        .unwrap();
        let result = lease.confirm_prepared(PreparationQuiescenceProof::assert_for_tests(
            nonce,
            "prep-container".to_string(),
            (1, 1),
            (2, 2),
        ));
        assert_eq!(result, Err(PreparationConfirmationError::Poisoned));
        assert!(
            !allocator.is_healthy(),
            "an ambiguous durable rewrite outcome must poison the whole allocator"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn bind_workload_poisons_on_an_ambiguous_rewrite_failure() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("bind-workload-rewrite-fails", 1, 1);
        let leases_dir = base.join("leases");
        let mut lease = allocator.lease().unwrap();
        let nonce = lease.nonce_for_tests();
        lease
            .bind_preparation("prep-container".to_string(), (1, 1), (2, 2))
            .expect("bind_preparation must succeed");
        lease
            .confirm_prepared(PreparationQuiescenceProof::assert_for_tests(
                nonce,
                "prep-container".to_string(),
                (1, 1),
                (2, 2),
            ))
            .expect("confirm_prepared must succeed");
        std::fs::write(
            leases_dir.join(format!("{}.tmp", marker_file_name(0))),
            b"stray tmp file blocking the real rewrite's O_EXCL create",
        )
        .unwrap();
        let result = lease.bind_workload("workload-container".to_string(), (3, 3), (4, 4));
        assert_eq!(result, Err(UserNamespaceBindError::Poisoned));
        assert!(
            !allocator.is_healthy(),
            "an ambiguous durable rewrite outcome must poison the whole allocator"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn release_prepared_poisons_on_an_ambiguous_unlink_failure() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("release-prepared-unlink-fails", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let nonce = lease.nonce_for_tests();
        lease
            .bind_preparation("prep-container".to_string(), (1, 1), (2, 2))
            .expect("bind_preparation must succeed");
        lease
            .confirm_prepared(PreparationQuiescenceProof::assert_for_tests(
                nonce,
                "prep-container".to_string(),
                (1, 1),
                (2, 2),
            ))
            .expect("confirm_prepared must succeed");
        let result = lease.release_prepared_given(|_dir_fd, _name| {
            Err(io::Error::from_raw_os_error(libc::EACCES))
        });
        assert_eq!(result, Err(UserNamespaceReleaseError::Poisoned));
        assert!(
            !allocator.is_healthy(),
            "an ambiguous unlink outcome must poison the whole allocator"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn confirm_prepared_succeeds_and_transitions_to_prepared() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("confirm-prepared-ok", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let nonce = lease.nonce_for_tests();
        lease
            .bind_preparation("prep-container".to_string(), (1, 1), (2, 2))
            .expect("bind_preparation must succeed");
        lease
            .confirm_prepared(PreparationQuiescenceProof::assert_for_tests(
                nonce,
                "prep-container".to_string(),
                (1, 1),
                (2, 2),
            ))
            .expect("confirm_prepared must succeed with a matching proof");
        assert!(allocator.is_healthy());
        lease
            .bind_workload("workload-container".to_string(), (3, 3), (4, 4))
            .expect("bind_workload must succeed once genuinely Prepared");
        lease
            .release(UserNamespaceQuiescenceProof::assert_for_tests(
                nonce,
                "workload-container".to_string(),
                (3, 3),
                (4, 4),
            ))
            .expect("ordinary release() must accept a workload identity reached via bind_workload");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn confirm_prepared_refuses_a_proof_whose_identity_disagrees_with_the_marker() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("confirm-prepared-wrong-id", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let nonce = lease.nonce_for_tests();
        lease
            .bind_preparation("real-prep".to_string(), (1, 1), (2, 2))
            .expect("bind_preparation must succeed");
        let result = lease.confirm_prepared(PreparationQuiescenceProof::assert_for_tests(
            nonce,
            "different-prep".to_string(),
            (1, 1),
            (2, 2),
        ));
        assert_eq!(
            result,
            Err(PreparationConfirmationError::ProofDisagreesWithMarker)
        );
        assert!(
            allocator.is_healthy(),
            "a proof with the wrong preparation-bound identity is an ordinary wrong proof, not \
             corruption - it must NOT poison the whole allocator, and the marker must be left \
             untouched since a preparation runtime this proof doesn't vouch for may still be alive"
        );
        lease
            .confirm_prepared(PreparationQuiescenceProof::assert_for_tests(
                nonce,
                "real-prep".to_string(),
                (1, 1),
                (2, 2),
            ))
            .expect("confirm_prepared with the correct proof must still succeed after a refusal");
        lease
            .release_prepared()
            .expect("release_prepared must succeed for a genuinely Prepared lease");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn confirm_prepared_refuses_a_proof_with_the_wrong_nonce() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("confirm-prepared-wrong-nonce", 1, 1);
        let mut lease = allocator.lease().unwrap();
        lease
            .bind_preparation("prep-container".to_string(), (1, 1), (2, 2))
            .expect("bind_preparation must succeed");
        let wrong_nonce = LeaseNonce(lease.nonce_for_tests().0.wrapping_add(1));
        let result = lease.confirm_prepared(PreparationQuiescenceProof::assert_for_tests(
            wrong_nonce,
            "prep-container".to_string(),
            (1, 1),
            (2, 2),
        ));
        assert_eq!(result, Err(PreparationConfirmationError::ProofMismatch));
        assert!(
            allocator.is_healthy(),
            "a wrong-nonce proof must not poison the allocator or touch this lease's own marker"
        );
        drop(lease);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn release_prepared_succeeds_after_confirm_prepared_and_frees_the_slot() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("release-prepared-ok", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let nonce = lease.nonce_for_tests();
        let freed_uid = lease.host_uid();
        lease
            .bind_preparation("prep-container".to_string(), (1, 1), (2, 2))
            .expect("bind_preparation must succeed");
        lease
            .confirm_prepared(PreparationQuiescenceProof::assert_for_tests(
                nonce,
                "prep-container".to_string(),
                (1, 1),
                (2, 2),
            ))
            .expect("confirm_prepared must succeed");
        lease
            .release_prepared()
            .expect("release_prepared must succeed for a genuinely Prepared lease");
        assert!(allocator.is_healthy());
        let lease_again = allocator.lease().unwrap();
        assert_eq!(
            lease_again.host_uid(),
            freed_uid,
            "release_prepared must genuinely free the slot for reuse"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn release_prepared_refuses_a_lease_still_only_preparation_bound() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("release-prepared-too-early", 1, 1);
        let mut lease = allocator.lease().unwrap();
        lease
            .bind_preparation("prep-container".to_string(), (1, 1), (2, 2))
            .expect("bind_preparation must succeed");
        let result = lease.release_prepared();
        assert_eq!(result, Err(UserNamespaceReleaseError::MarkerMismatch));
        assert!(
            !allocator.is_healthy(),
            "release_prepared on a marker that is only PreparationBound (a runtime may still be \
             live) must poison the whole allocator, never silently unlink"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn release_prepared_refuses_a_lease_already_bound_to_workload() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("release-prepared-too-late", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let nonce = lease.nonce_for_tests();
        lease
            .bind_preparation("prep-container".to_string(), (1, 1), (2, 2))
            .expect("bind_preparation must succeed");
        lease
            .confirm_prepared(PreparationQuiescenceProof::assert_for_tests(
                nonce,
                "prep-container".to_string(),
                (1, 1),
                (2, 2),
            ))
            .expect("confirm_prepared must succeed");
        lease
            .bind_workload("workload-container".to_string(), (3, 3), (4, 4))
            .expect("bind_workload must succeed");
        let result = lease.release_prepared();
        assert_eq!(result, Err(UserNamespaceReleaseError::MarkerMismatch));
        assert!(
            !allocator.is_healthy(),
            "release_prepared on a marker already Bound to the real workload must poison the \
             allocator - use release() with a real workload quiescence proof instead"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn bind_workload_refuses_a_lease_still_only_preparation_bound() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("bind-workload-too-early", 1, 1);
        let mut lease = allocator.lease().unwrap();
        lease
            .bind_preparation("prep-container".to_string(), (1, 1), (2, 2))
            .expect("bind_preparation must succeed");
        let result = lease.bind_workload("workload-container".to_string(), (3, 3), (4, 4));
        assert_eq!(result, Err(UserNamespaceBindError::MarkerMismatch));
        assert!(
            !allocator.is_healthy(),
            "bind_workload on a marker that is only PreparationBound (never durably Prepared) \
             must poison the allocator"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn bind_workload_refuses_a_lease_that_was_never_prepared() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("bind-workload-never-prepared", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let result = lease.bind_workload("workload-container".to_string(), (1, 1), (2, 2));
        assert_eq!(result, Err(UserNamespaceBindError::MarkerMismatch));
        assert!(!allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_preparation_bound_lease_survives_reopening_and_is_quarantined() {
        require_unprivileged_euid!();
        let base = test_base("prep-bound-marker-survives-reboot");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 5);
        write_subordinate_file(&subgid, 200_000, 5);

        let (sink, _log) = recording_sink();
        let allocator = UserNamespaceAllocator::try_new_for_tests(
            leases_dir.clone(),
            &subuid,
            &subgid,
            1,
            sink,
        )
        .unwrap();
        let mut lease = allocator.lease().unwrap();
        let leaked_uid = lease.host_uid();
        lease
            .bind_preparation("crashed-prep-container".to_string(), (7, 8), (9, 10))
            .expect("bind_preparation must succeed for a fresh Allocated lease");
        drop(lease);
        drop(allocator);

        let (sink2, log2) = recording_sink();
        let reopened =
            UserNamespaceAllocator::try_new_for_tests(leases_dir, &subuid, &subgid, 1, sink2)
                .unwrap();
        assert!(
            reopened.is_healthy(),
            "a surviving PreparationBound marker must parse successfully at boot, not be treated \
             as corrupt"
        );
        assert!(reopened.quarantined_slots().contains(&0));
        assert!(log2
            .lock()
            .unwrap()
            .iter()
            .any(|m| m.contains("PreparationBound") && m.contains("crashed-prep-container")));
        let lease2 = reopened.lease().unwrap();
        assert_ne!(
            lease2.host_uid(),
            leaked_uid,
            "the leaked slot's host_uid must never be reissued"
        );
        release_for_tests(lease2);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_prepared_lease_survives_reopening_and_is_quarantined() {
        require_unprivileged_euid!();
        let base = test_base("prepared-marker-survives-reboot");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 5);
        write_subordinate_file(&subgid, 200_000, 5);

        let (sink, _log) = recording_sink();
        let allocator = UserNamespaceAllocator::try_new_for_tests(
            leases_dir.clone(),
            &subuid,
            &subgid,
            1,
            sink,
        )
        .unwrap();
        let mut lease = allocator.lease().unwrap();
        let leaked_uid = lease.host_uid();
        let nonce = lease.nonce_for_tests();
        lease
            .bind_preparation("crashed-prep-container".to_string(), (7, 8), (9, 10))
            .expect("bind_preparation must succeed");
        lease
            .confirm_prepared(PreparationQuiescenceProof::assert_for_tests(
                nonce,
                "crashed-prep-container".to_string(),
                (7, 8),
                (9, 10),
            ))
            .expect("confirm_prepared must succeed");
        drop(lease);
        drop(allocator);

        let (sink2, log2) = recording_sink();
        let reopened =
            UserNamespaceAllocator::try_new_for_tests(leases_dir, &subuid, &subgid, 1, sink2)
                .unwrap();
        assert!(
            reopened.is_healthy(),
            "a surviving Prepared marker must parse successfully at boot, not be treated as corrupt"
        );
        assert!(reopened.quarantined_slots().contains(&0));
        assert!(log2
            .lock()
            .unwrap()
            .iter()
            .any(|m| m.contains("Prepared") && m.contains("crashed-prep-container")));
        let lease2 = reopened.lease().unwrap();
        assert_ne!(
            lease2.host_uid(),
            leaked_uid,
            "the leaked slot's host_uid must never be reissued"
        );
        release_for_tests(lease2);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn checkout_preparation_session_happy_path_produces_a_workload_bound_lease() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("session-happy-path", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let nonce = lease.nonce_for_tests();
        let mut session = CheckoutPreparationSession::new();
        session
            .bind_preparation(&mut lease, "prep-container".to_string(), (1, 1), (2, 2))
            .expect("bind_preparation must succeed");
        session
            .confirm_prepared(
                &mut lease,
                PreparationQuiescenceProof::assert_for_tests(
                    nonce,
                    "prep-container".to_string(),
                    (1, 1),
                    (2, 2),
                ),
            )
            .expect("confirm_prepared must succeed");
        session
            .bind_workload(&mut lease, "workload-container".to_string(), (3, 3), (4, 4))
            .expect("bind_workload must succeed");
        lease
            .release(UserNamespaceQuiescenceProof::assert_for_tests(
                nonce,
                "workload-container".to_string(),
                (3, 3),
                (4, 4),
            ))
            .expect(
                "ordinary release() must accept a workload-bound identity reached via a session",
            );
        assert!(allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn checkout_preparation_session_confirm_prepared_refuses_a_substituted_lease() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("session-cross-lease-confirm", 2, 2);
        let mut lease_a = allocator.lease().unwrap();
        let mut lease_b = allocator.lease().unwrap();
        let nonce_a = lease_a.nonce_for_tests();
        let nonce_b = lease_b.nonce_for_tests();
        let mut session = CheckoutPreparationSession::new();
        session
            .bind_preparation(&mut lease_a, "prep-a".to_string(), (1, 1), (1, 1))
            .expect("bind_preparation on lease_a must succeed");
        lease_b
            .bind_preparation("prep-b".to_string(), (2, 2), (2, 2))
            .expect("bind_preparation on lease_b must succeed");
        let result = session.confirm_prepared(
            &mut lease_b,
            PreparationQuiescenceProof::assert_for_tests(
                nonce_b,
                "prep-b".to_string(),
                (2, 2),
                (2, 2),
            ),
        );
        assert_eq!(result, Err(PreparationConfirmationError::LeaseMismatch));
        session
            .confirm_prepared(
                &mut lease_a,
                PreparationQuiescenceProof::assert_for_tests(
                    nonce_a,
                    "prep-a".to_string(),
                    (1, 1),
                    (1, 1),
                ),
            )
            .expect("the session still owns lease_a after refusing lease_b");
        session.release_prepared(lease_a).unwrap();
        lease_b
            .confirm_prepared(PreparationQuiescenceProof::assert_for_tests(
                nonce_b,
                "prep-b".to_string(),
                (2, 2),
                (2, 2),
            ))
            .unwrap();
        lease_b.release_prepared().unwrap();
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn checkout_preparation_session_bind_workload_refuses_a_substituted_lease() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("session-cross-lease-workload", 2, 2);
        let mut lease_a = allocator.lease().unwrap();
        let mut lease_b = allocator.lease().unwrap();
        let nonce_a = lease_a.nonce_for_tests();
        let nonce_b = lease_b.nonce_for_tests();
        let mut session = CheckoutPreparationSession::new();
        session
            .bind_preparation(&mut lease_a, "prep-a".to_string(), (1, 1), (1, 1))
            .expect("bind_preparation on lease_a must succeed");
        session
            .confirm_prepared(
                &mut lease_a,
                PreparationQuiescenceProof::assert_for_tests(
                    nonce_a,
                    "prep-a".to_string(),
                    (1, 1),
                    (1, 1),
                ),
            )
            .expect("confirm_prepared on lease_a must succeed");
        lease_b
            .bind_preparation("prep-b".to_string(), (2, 2), (2, 2))
            .expect("bind_preparation on lease_b must succeed");
        lease_b
            .confirm_prepared(PreparationQuiescenceProof::assert_for_tests(
                nonce_b,
                "prep-b".to_string(),
                (2, 2),
                (2, 2),
            ))
            .expect("confirm_prepared on lease_b must succeed");
        assert_eq!(
            session.bind_workload(&mut lease_b, "workload".to_string(), (3, 3), (3, 3)),
            Err(UserNamespaceBindError::LeaseMismatch)
        );
        session
            .bind_workload(&mut lease_a, "workload-a".to_string(), (3, 3), (3, 3))
            .expect("the session still owns lease_a after refusing lease_b");
        lease_a
            .release(UserNamespaceQuiescenceProof::assert_for_tests(
                nonce_a,
                "workload-a".to_string(),
                (3, 3),
                (3, 3),
            ))
            .unwrap();
        lease_b.release_prepared().unwrap();
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn checkout_preparation_session_bind_workload_survives_a_retryable_refusal() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("session-workload-retry", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let nonce = lease.nonce_for_tests();
        let mut session = CheckoutPreparationSession::new();
        session
            .bind_preparation(&mut lease, "prep-container".to_string(), (1, 1), (2, 2))
            .expect("bind_preparation must succeed");
        session
            .confirm_prepared(
                &mut lease,
                PreparationQuiescenceProof::assert_for_tests(
                    nonce,
                    "prep-container".to_string(),
                    (1, 1),
                    (2, 2),
                ),
            )
            .expect("confirm_prepared must succeed");
        let oversized_id = "x".repeat(MAX_CONTAINER_ID_LEN + 1);
        let refused = session.bind_workload(&mut lease, oversized_id, (3, 3), (4, 4));
        assert_eq!(refused, Err(UserNamespaceBindError::InvalidContainerId));
        assert!(
            !session.is_unreleasable(),
            "a retryable bind_workload refusal must not abandon the session"
        );
        session
            .bind_workload(&mut lease, "workload-container".to_string(), (3, 3), (4, 4))
            .expect("bind_workload must succeed on a corrected retry");
        lease
            .release(UserNamespaceQuiescenceProof::assert_for_tests(
                nonce,
                "workload-container".to_string(),
                (3, 3),
                (4, 4),
            ))
            .expect("ordinary release() must accept the workload identity reached on retry");
        assert!(allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn checkout_preparation_session_release_prepared_path() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("session-release-prepared", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let nonce = lease.nonce_for_tests();
        let mut session = CheckoutPreparationSession::new();
        session
            .bind_preparation(&mut lease, "prep-container".to_string(), (1, 1), (2, 2))
            .expect("bind_preparation must succeed");
        session
            .confirm_prepared(
                &mut lease,
                PreparationQuiescenceProof::assert_for_tests(
                    nonce,
                    "prep-container".to_string(),
                    (1, 1),
                    (2, 2),
                ),
            )
            .expect("confirm_prepared must succeed");
        session
            .release_prepared(lease)
            .expect("release_prepared via the session must succeed for a genuinely Prepared lease");
        assert!(allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn checkout_preparation_session_refuses_a_second_preparation_bind() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("session-bind-prep-twice", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let mut session = CheckoutPreparationSession::new();
        session
            .bind_preparation(&mut lease, "prep-1".to_string(), (1, 1), (1, 1))
            .expect("first bind_preparation must succeed");
        let nonce = lease.nonce_for_tests();
        assert_eq!(
            session.bind_preparation(&mut lease, "prep-2".to_string(), (2, 2), (2, 2)),
            Err(UserNamespaceBindError::InvalidSessionState)
        );
        session
            .confirm_prepared(
                &mut lease,
                PreparationQuiescenceProof::assert_for_tests(
                    nonce,
                    "prep-1".to_string(),
                    (1, 1),
                    (1, 1),
                ),
            )
            .unwrap();
        session.release_prepared(lease).unwrap();
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn checkout_preparation_session_refuses_confirmation_before_binding() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("session-confirm-too-early", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let nonce = lease.nonce_for_tests();
        let mut session = CheckoutPreparationSession::new();
        let result = session.confirm_prepared(
            &mut lease,
            PreparationQuiescenceProof::assert_for_tests(
                nonce,
                "prep-container".to_string(),
                (1, 1),
                (2, 2),
            ),
        );
        assert_eq!(
            result,
            Err(PreparationConfirmationError::InvalidSessionState)
        );
        release_for_tests(lease);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn checkout_preparation_session_refuses_workload_before_preparation() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("session-workload-too-early", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let mut session = CheckoutPreparationSession::new();
        assert_eq!(
            session.bind_workload(&mut lease, "workload".to_string(), (1, 1), (2, 2)),
            Err(UserNamespaceBindError::InvalidSessionState)
        );
        release_for_tests(lease);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn checkout_preparation_session_marks_unreleasable_on_a_poisoning_bind_preparation_failure() {
        require_unprivileged_euid!();
        let (allocator, base, _log) = new_allocator_for_test("session-marks-unreleasable", 1, 1);
        let mut lease = allocator.lease().unwrap();
        lease
            .bind_preparation("already-there".to_string(), (9, 9), (9, 9))
            .expect("planting the PreparationBound state directly must succeed");
        let mut session = CheckoutPreparationSession::new();
        let result = session.bind_preparation(&mut lease, "prep-2".to_string(), (2, 2), (2, 2));
        assert_eq!(result, Err(UserNamespaceBindError::MarkerMismatch));
        assert!(session.is_unreleasable());
        assert!(!allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn checkout_preparation_session_confirm_prepared_wrong_proof_is_a_terminal_abandonment() {
        require_unprivileged_euid!();
        let (allocator, base, _log) =
            new_allocator_for_test("session-confirm-wrong-proof-terminal", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let nonce = lease.nonce_for_tests();
        let leaked_uid = lease.host_uid();
        let mut session = CheckoutPreparationSession::new();
        session
            .bind_preparation(&mut lease, "real-prep".to_string(), (1, 1), (2, 2))
            .expect("bind_preparation must succeed");
        let wrong_proof_result = session.confirm_prepared(
            &mut lease,
            PreparationQuiescenceProof::assert_for_tests(
                nonce,
                "different-prep".to_string(),
                (1, 1),
                (2, 2),
            ),
        );
        assert_eq!(
            wrong_proof_result,
            Err(PreparationConfirmationError::ProofDisagreesWithMarker)
        );
        assert!(
            allocator.is_healthy(),
            "an ordinary wrong proof at the raw lease level must not poison the allocator"
        );
        assert!(
            session.is_unreleasable(),
            "the SESSION must terminally abandon on ANY confirm_prepared failure, unlike the raw \
             lease API it wraps"
        );
        assert_eq!(
            session.confirm_prepared(
                &mut lease,
                PreparationQuiescenceProof::assert_for_tests(
                    nonce,
                    "real-prep".to_string(),
                    (1, 1),
                    (2, 2),
                ),
            ),
            Err(PreparationConfirmationError::InvalidSessionState),
            "a later correct proof must never advance a terminally abandoned session"
        );
        drop(lease);
        assert!(
            allocator
                .quarantined_slots()
                .contains(&(leaked_uid - 100_000)),
            "dropping the still-outstanding lease must quarantine exactly its own slot"
        );
        assert!(
            allocator.is_healthy(),
            "abandoning one lease must not poison the whole allocator"
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
