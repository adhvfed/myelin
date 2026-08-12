use std::ffi::{CStr, CString};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::Path;

#[derive(Debug)]
pub(crate) enum DirLockError {
    AlreadyLocked,
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

pub(crate) fn acquire_directory_lock(dir: &Path) -> Result<OwnedFd, DirLockError> {
    std::fs::create_dir_all(dir).map_err(|e| DirLockError::Failed(format!("create dir: {e}")))?;
    let path_c = CString::new(dir.as_os_str().as_encoded_bytes())
        .map_err(|e| DirLockError::Failed(format!("path contains an interior NUL: {e}")))?;
    let fd = unsafe {
        libc::open(
            path_c.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(DirLockError::Failed(format!(
            "open directory for locking: {}",
            io::Error::last_os_error()
        )));
    }
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
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

pub(crate) fn fd_identity(fd: &OwnedFd) -> io::Result<(u64, u64)> {
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    let result = unsafe { libc::fstat(fd.as_raw_fd(), &mut stat) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((stat.st_dev as u64, stat.st_ino as u64))
}

pub(crate) fn path_identity(path: &Path) -> io::Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path)?;
    Ok((meta.dev(), meta.ino()))
}

pub(crate) fn open_dir_component_no_follow(dir_fd: RawFd, name: &CStr) -> io::Result<OwnedFd> {
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
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

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
    let root_fd = unsafe {
        libc::open(
            root_c.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if root_fd < 0 {
        return Err(format!("open /: {}", io::Error::last_os_error()));
    }
    let mut current = unsafe { OwnedFd::from_raw_fd(root_fd) };
    check_ancestor_not_owned_or_writable(&current, Path::new("/"))?;

    for component in parent.components() {
        let name =
            match component {
                std::path::Component::RootDir => continue,
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

pub(crate) fn check_ancestor_not_owned_or_writable(
    fd: &OwnedFd,
    label: &Path,
) -> Result<(), String> {
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
            "ancestor {label:?} is owned by this process's own euid {our_uid} - it could be \
             `chmod`'d writable at any time regardless of its current mode, which would let this \
             process rename/replace anything beneath it"
        ));
    }
    let empty = CString::new("").unwrap();
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
            "ancestor {label:?} is writable by this process's EFFECTIVE identity - it could be \
             renamed/replaced, relocating everything beneath it"
        ));
    }
    let errno = io::Error::last_os_error();
    if errno.raw_os_error() != Some(libc::EACCES) {
        return Err(format!(
            "faccessat on ancestor {label:?} failed in a way that does not prove it is \
             non-writable ({errno}) - refusing rather than assuming safety"
        ));
    }
    Ok(())
}
