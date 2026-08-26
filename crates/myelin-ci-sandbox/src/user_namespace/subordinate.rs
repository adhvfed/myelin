use std::ffi::CString;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use super::UserNamespaceAllocatorError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SubordinateRange {
    pub(super) start: u32,
    pub(super) count: u32,
}

pub(super) fn effective_username() -> Option<String> {
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

pub(super) fn range_contains(range: SubordinateRange, value: u32) -> bool {
    value >= range.start && value < range.start.saturating_add(range.count)
}

pub(super) fn parse_subordinate_range(
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
