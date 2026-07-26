//! Shared process-lifetime directory-locking primitive.
//!
//! An exclusive `flock` on a directory's OWN file descriptor (never a lockfile created inside
//! it), plus (device, inode) identity capture/verification for that same directory. Extracted so
//! [`crate::workspace_manager`] (the Btrfs workspace base) and [`crate::user_namespace`] (the
//! subordinate-id lease directory) share ONE implementation of this security-relevant primitive
//! rather than maintaining two independently-evolving copies.

use std::ffi::{CStr, CString};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::Path;

/// A directory-lock acquisition failure — the caller maps this into its own richer error type.
#[derive(Debug)]
pub(crate) enum DirLockError {
    /// Another process already holds the exclusive lock (non-blocking `flock` returned
    /// `EWOULDBLOCK`).
    AlreadyLocked,
    /// Acquiring the lock (or a preparatory step) failed for a reason OTHER than contention.
    Failed(String),
}

impl std::fmt::Display for DirLockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DirLockError::AlreadyLocked => write!(f, "already locked by another process"),
            DirLockError::Failed(reason) => write!(f, "{reason}"),
        }
    }
}

/// Acquire a process-lifetime exclusive `flock` on `dir` ITSELF — never a lockfile created inside
/// it (a caller whose directory contents are meaningful, like `WorkspaceStorage`'s orphan scanner,
/// can then treat every entry under `dir` as data, with no lockfile of ours to filter out).
/// `O_CLOEXEC` so the FD is never inherited across an exec (a sandboxed guest process must never
/// hold this host-side lock). Non-blocking (`LOCK_NB`): a second process sharing the same
/// directory refuses immediately at startup rather than hanging.
pub(crate) fn acquire_directory_lock(dir: &Path) -> Result<OwnedFd, DirLockError> {
    std::fs::create_dir_all(dir).map_err(|e| DirLockError::Failed(format!("create dir: {e}")))?;
    // SAFETY: `open`'s arguments are a NUL-free, valid path (converted via `CString` below) and
    // standard POSIX flags; the returned fd, on success, is a newly-owned, exclusively-held
    // descriptor this function transfers to its caller via `OwnedFd::from_raw_fd`.
    let path_c = CString::new(dir.as_os_str().as_encoded_bytes())
        .map_err(|e| DirLockError::Failed(format!("path contains an interior NUL: {e}")))?;
    let fd = unsafe {
        libc::open(
            path_c.as_ptr(),
            // `O_NOFOLLOW`: refuse to lock through a symlink at `dir` itself — no legitimate
            // caller needs the base of a process-lifetime exclusive lock to be a symlink, and
            // allowing one would let it be silently repointed at a different real directory
            // between separate path-based operations (Sol's `user_namespace.rs` review).
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(DirLockError::Failed(format!(
            "open directory for locking: {}",
            io::Error::last_os_error()
        )));
    }
    // SAFETY: `fd` was just returned by a successful `open` above and is not owned elsewhere yet.
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    // SAFETY: `owned` is a valid, open file descriptor for the duration of this call.
    let flock_result = unsafe { libc::flock(owned.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if flock_result != 0 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::WouldBlock {
            return Err(DirLockError::AlreadyLocked);
        }
        return Err(DirLockError::Failed(format!("flock: {error}")));
    }
    Ok(owned)
}

/// The (device, inode) of an already-open file descriptor — identifies the exact inode it
/// references regardless of what happens to any path pointing at it afterward.
pub(crate) fn fd_identity(fd: &OwnedFd) -> io::Result<(u64, u64)> {
    // SAFETY: `stat` is a plain-old-data struct; `fd` is a valid, open file descriptor for the
    // duration of this call.
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    let result = unsafe { libc::fstat(fd.as_raw_fd(), &mut stat) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((stat.st_dev as u64, stat.st_ino as u64))
}

/// The (device, inode) the given PATH currently resolves to (following symlinks, matching
/// `WorkspaceStorage::open`'s own `canonicalize` semantics).
pub(crate) fn path_identity(path: &Path) -> io::Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path)?;
    Ok((meta.dev(), meta.ino()))
}

/// Open exactly one path component as a directory, relative to `dir_fd`, with `O_NOFOLLOW` — a
/// symlinked component fails to open at all (`ELOOP`) rather than being silently traversed.
pub(crate) fn open_dir_component_no_follow(dir_fd: RawFd, name: &CStr) -> io::Result<OwnedFd> {
    // SAFETY: `dir_fd` is a valid, open directory file descriptor for the duration of this call;
    // `name` is a NUL-terminated component name.
    let fd = unsafe {
        libc::openat(
            dir_fd,
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` was just returned by a successful `openat` above and is not owned elsewhere.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Verify EVERY ancestor of `dir`, from `/` down to (but not including) `dir` itself, is neither
/// OWNED by nor WRITABLE by this process's own EFFECTIVE identity. Shared by
/// [`crate::user_namespace`] (the subordinate-id leases directory) and [`crate::gvisor`] (the
/// explicit-user-namespace helper directory naming `newuidmap`/`newgidmap`) — both need the exact
/// same "this directory cannot be relocated out from under a caller trusting its current location"
/// guarantee. An immediate-parent-only, mode-bits-only check misses THREE live attacks (Sol's
/// review): (1) this process OWNS the immediate parent but its current mode denies write —
/// ownership alone lets it `chmod` the directory writable at will, regardless of the mode bits
/// observed a moment earlier; (2) the immediate parent is safe but a HIGHER ancestor (grandparent
/// or above) is owned/writable by this process — renaming or replacing THAT ancestor relocates
/// everything beneath it, including a directory whose own immediate parent looked fine in
/// isolation; (3) an ancestor is itself a symlink this process could repoint. This function walks
/// the FULL chain via `openat`, each component opened with `O_NOFOLLOW` (a symlinked ancestor fails
/// to open at all, rather than being silently followed), checking both ownership (`fstat`) and
/// effective-identity writability (`faccessat(..., AT_EACCESS)` — deliberately NOT the
/// real-uid-only `access(2)`, since production could plausibly run under a real/effective uid
/// split) at every level. Returns a plain `String` reason on failure; callers wrap it into their
/// own richer error type.
pub(crate) fn verify_ancestors_not_writable_by_us(dir: &Path) -> Result<(), String> {
    let parent = match dir.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => return Err("the directory has no parent to anchor against".to_string()),
    };
    if !parent.is_absolute() {
        return Err(
            "the directory must be an absolute path to verify its ancestor chain".to_string(),
        );
    }

    let root_c = CString::new("/").unwrap();
    // SAFETY: a fixed, NUL-free literal path; standard POSIX flags; `O_NOFOLLOW` refuses to
    // traverse through a symlink (not a concern for `/` itself, but kept for uniformity).
    let root_fd = unsafe {
        libc::open(
            root_c.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if root_fd < 0 {
        return Err(format!("open /: {}", io::Error::last_os_error()));
    }
    // SAFETY: `root_fd` was just returned by a successful `open` above and is not owned elsewhere.
    let mut current = unsafe { OwnedFd::from_raw_fd(root_fd) };
    check_ancestor_not_owned_or_writable(&current, Path::new("/"))?;

    for component in parent.components() {
        let name =
            match component {
                std::path::Component::RootDir => continue, // already opened and checked above
                std::path::Component::Normal(n) => n,
                _ => return Err(
                    "the directory's parent path must contain only plain components (no `.`/`..`)"
                        .to_string(),
                ),
            };
        let name_c = CString::new(name.as_encoded_bytes())
            .map_err(|e| format!("path component contains an interior NUL: {e}"))?;
        current = open_dir_component_no_follow(current.as_raw_fd(), &name_c)
            .map_err(|e| format!("openat ancestor component {name:?}: {e}"))?;
        check_ancestor_not_owned_or_writable(&current, Path::new(name))?;
    }
    Ok(())
}

/// The per-ancestor check [`verify_ancestors_not_writable_by_us`]'s walk applies at every level:
/// refuse if `fd` is owned by this process's own euid (ownership alone permits `chmod`ing it
/// writable later, regardless of its CURRENT mode), or if it is writable by this process's
/// EFFECTIVE identity right now. Pulled out into its own function (not an inline closure) so a
/// test can exercise it directly against a single fd, without needing every ancestor ABOVE that fd
/// in a real filesystem to also be safe — the full walk's own earlier levels would otherwise refuse
/// first against any fixture a non-privileged test itself creates under a writable temp directory.
pub(crate) fn check_ancestor_not_owned_or_writable(fd: &OwnedFd, label: &Path) -> Result<(), String> {
    // SAFETY: `stat` is plain-old-data; `fd` is a valid, open file descriptor.
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut stat) } != 0 {
        return Err(format!(
            "fstat ancestor {label:?}: {}",
            io::Error::last_os_error()
        ));
    }
    let our_uid = unsafe { libc::geteuid() };
    if stat.st_uid == our_uid {
        return Err(format!(
            "ancestor {label:?} is owned by this process's own euid {our_uid} — it could be \
             `chmod`'d writable at any time regardless of its current mode, which would let this \
             process rename/replace anything beneath it"
        ));
    }
    let empty = CString::new("").unwrap();
    // SAFETY: `fd` is a valid, open directory file descriptor; `empty` is a NUL-terminated empty
    // path used with `AT_EMPTY_PATH` to query the fd's own target rather than a fresh,
    // separately-racable path-based lookup.
    let rc = unsafe {
        libc::faccessat(
            fd.as_raw_fd(),
            empty.as_ptr(),
            libc::W_OK,
            libc::AT_EACCESS | libc::AT_EMPTY_PATH,
        )
    };
    if rc == 0 {
        return Err(format!(
            "ancestor {label:?} is writable by this process's EFFECTIVE identity — it could be \
             renamed/replaced, relocating everything beneath it"
        ));
    }
    // Sol's review, round 4: `rc != 0` alone does NOT prove non-writability — only `EACCES` does.
    // `EINVAL`/`ENOSYS`/`EBADF`/anything else means the check itself failed to run, which must be
    // treated as "writability could not be established" (fail closed) rather than silently
    // admitting the ancestor.
    let errno = io::Error::last_os_error();
    if errno.raw_os_error() != Some(libc::EACCES) {
        return Err(format!(
            "faccessat on ancestor {label:?} failed in a way that does not prove it is \
             non-writable ({errno}) — refusing rather than assuming safety"
        ));
    }
    Ok(())
}
