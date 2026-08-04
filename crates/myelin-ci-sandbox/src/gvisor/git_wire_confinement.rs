use crate::HookError;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum WireError {
    Path(String),
    StdinTooLarge {
        len: usize,
        cap: usize,
    },
    OutputTooLarge {
        cap: usize,
    },
    Hardening(String),
    Hook(HookError),
    Runtime(String),
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WireError::Path(s) => write!(f, "git-wire: path confinement refused: {s}"),
            WireError::StdinTooLarge { len, cap } => write!(
                f,
                "git-wire: request body {len} bytes exceeds the {cap}-byte cap (refused fail-closed)"
            ),
            WireError::OutputTooLarge { cap } => write!(
                f,
                "git-wire: upload-pack response exceeded the {cap}-byte wire cap - refusing a TRUNCATED \
                 pack (a short packfile fails the client's `index-pack` with 'early EOF'); fail-loud"
            ),
            WireError::Hardening(s) => write!(f, "git-wire: hardening not enforced: {s}"),
            WireError::Hook(e) => write!(f, "git-wire: guarantee hook failed: {e}"),
            WireError::Runtime(s) => write!(f, "git-wire: runsc/bundle error: {s}"),
        }
    }
}

impl std::error::Error for WireError {}

impl From<HookError> for WireError {
    fn from(e: HookError) -> Self {
        WireError::Hook(e)
    }
}

pub fn validate_wire_segment(kind: &str, seg: &str) -> Result<(), WireError> {
    if seg.is_empty() {
        return Err(WireError::Path(format!(
            "invalid {kind} path segment: empty (fail-closed - refusing to resolve a path)"
        )));
    }
    if seg == "." || seg == ".." {
        return Err(WireError::Path(format!(
            "invalid {kind} path segment {seg:?}: path-traversal component refused (fail-closed)"
        )));
    }
    for c in seg.chars() {
        let ok = c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');
        if !ok {
            return Err(WireError::Path(format!(
                "invalid {kind} path segment {seg:?}: character {c:?} not in the allowlist \
                 [A-Za-z0-9._-] - separators/NUL/control chars are refused (path-traversal / \
                 absolute-component guard, fail-closed)"
            )));
        }
    }
    Ok(())
}

pub fn validate_wire_repo_slug(repo: &str) -> Result<Vec<String>, WireError> {
    if repo.contains('\\') || repo.contains('\0') {
        return Err(WireError::Path(format!(
            "invalid repo slug {repo:?}: contains a backslash/NUL (path-traversal guard, fail-closed)"
        )));
    }
    let pieces: Vec<String> = repo.split('/').map(|s| s.to_string()).collect();
    for piece in &pieces {
        validate_wire_segment("repo", piece)?;
    }
    Ok(pieces)
}

pub fn resolve_bare_repo_path(
    root: &Path,
    tenant: &str,
    region: &str,
    repo: &str,
) -> Result<PathBuf, WireError> {
    validate_wire_segment("tenant", tenant)?;
    validate_wire_segment("region", region)?;
    let pieces = validate_wire_repo_slug(repo)?;
    let mut path = root.to_path_buf();
    path.push(tenant);
    path.push(region);
    for piece in &pieces {
        path.push(piece);
    }
    let last = pieces
        .last()
        .expect("validate_wire_repo_slug returns ≥1 piece or errors");
    path.set_file_name(format!("{last}.git"));
    Ok(path)
}

pub fn assert_repo_under_root(root: &Path, repo_host_path: &Path) -> Result<(), WireError> {
    let meta = std::fs::symlink_metadata(repo_host_path).map_err(|e| {
        WireError::Path(format!(
            "repo path {repo_host_path:?} is not present/stat-able ({e}) - refused before mount \
             (fail-closed)"
        ))
    })?;
    if meta.file_type().is_symlink() {
        return Err(WireError::Path(format!(
            "repo path {repo_host_path:?} is a SYMLINK - refused before mount (a symlinked \
             `<repo>.git` could make the bind-mount follow OUT of the tenant tree; defence in depth)"
        )));
    }
    if !meta.is_dir() {
        return Err(WireError::Path(format!(
            "repo path {repo_host_path:?} is not a directory - refused before mount (fail-closed)"
        )));
    }
    let canon_root = std::fs::canonicalize(root).map_err(|e| {
        WireError::Path(format!(
            "git root {root:?} could not be canonicalized ({e}) - refused before mount (fail-closed)"
        ))
    })?;
    let rel = repo_host_path.strip_prefix(root).map_err(|_| {
        WireError::Path(format!(
            "repo path {repo_host_path:?} is not under the configured git root {root:?} - refused \
             before mount (fail-closed)"
        ))
    })?;
    let mut cur = canon_root.clone();
    for comp in rel.components() {
        cur = cur.join(comp.as_os_str());
        let m = std::fs::symlink_metadata(&cur).map_err(|e| {
            WireError::Path(format!(
                "repo path component {cur:?} is not present/stat-able ({e}) - refused before mount"
            ))
        })?;
        if m.file_type().is_symlink() {
            return Err(WireError::Path(format!(
                "repo path component {cur:?} is a SYMLINK - refused before mount (an intermediate \
                 symlink, even one resolving UNDER the root, is a bind-mount-follow vector; FU-2)"
            )));
        }
    }
    let canon_repo = std::fs::canonicalize(repo_host_path).map_err(|e| {
        WireError::Path(format!(
            "repo path {repo_host_path:?} could not be canonicalized ({e}) - refused before mount \
             (fail-closed)"
        ))
    })?;
    if !canon_repo.starts_with(&canon_root) {
        return Err(WireError::Path(format!(
            "resolved repo path {canon_repo:?} escapes the canonical git root {canon_root:?} (a \
             symlinked component would leave the tenant tree) - refused before mount (fail-closed)"
        )));
    }
    Ok(())
}
