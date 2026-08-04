//! Git-wire error vocabulary + the (tenant, region, repo) path-confinement validators (the GT-001
//! cross-tenant isolation boundary). Pure host-side string/path validation with no runtime coupling:
//! a raw attacker-influenced locator can never reach a bind mount without passing through here.
//!
//! The segment/slug/resolver checks REPLICATE `myelin_git::gix_backend`'s validators byte-for-byte
//! (pinned by `tests/validator_drift_pin_test.rs`); [`assert_repo_under_root`] adds the CT-006b
//! symlink defence-in-depth over the resolved path before any mount.

use crate::HookError;
use std::path::{Path, PathBuf};

/// A git-wire backend error.
#[derive(Debug)]
pub enum WireError {
    /// The (tenant, region, repo) locator failed path-confinement validation — REFUSED before any
    /// mount (cross-tenant / `..` / separator / absolute / non-allowlisted segment).
    Path(String),
    /// The request body exceeded [`super::WIRE_STDIN_BOUND`] — refused fail-closed before spawning.
    StdinTooLarge {
        /// The offending body length.
        len: usize,
        /// The cap it breached.
        cap: usize,
    },
    /// The wire RESPONSE exceeded the generous wire cap (derived from `disk_bytes`, default
    /// [`super::WIRE_STDOUT_BOUND`]) — REFUSED fail-LOUD rather than returning a silently-truncated
    /// pack (a truncated pack fails the client's `index-pack` with "early EOF").
    OutputTooLarge {
        /// The cap it breached (bytes).
        cap: usize,
    },
    /// The mandatory hardening profile could not be asserted in force (fail-closed).
    Hardening(String),
    /// A four-guarantee hook failed (cost-exhausted / token-rejected / isolation-floor-not-met).
    Hook(HookError),
    /// The `runsc` runtime / bundle staging errored (absent git rootfs, spawn failure, …).
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
                "git-wire: upload-pack response exceeded the {cap}-byte wire cap — refusing a TRUNCATED \
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

/// Reject a single `(tenant|region|repo)` path segment that could escape the per-tenant/region root.
/// REPLICATES `myelin_git::gix_backend::validate_path_segment` byte-for-byte: empty, `.`, `..`, and any
/// char outside `[A-Za-z0-9._-]` (so separators `/`/`\`, NUL, control chars, and absolute components
/// are all refused). Fail-closed — refuses before any path is built.
pub fn validate_wire_segment(kind: &str, seg: &str) -> Result<(), WireError> {
    if seg.is_empty() {
        return Err(WireError::Path(format!(
            "invalid {kind} path segment: empty (fail-closed — refusing to resolve a path)"
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
                 [A-Za-z0-9._-] — separators/NUL/control chars are refused (path-traversal / \
                 absolute-component guard, fail-closed)"
            )));
        }
    }
    Ok(())
}

/// Validate a (possibly namespaced `team/app`) repo slug into its individually-validated `/`-pieces.
/// REPLICATES `myelin_git::gix_backend::validate_repo_slug`: a backslash/NUL slug is refused outright,
/// then each `/`-piece is held to [`validate_wire_segment`] (so `../../x`, `/etc/passwd`, `a//b`, a
/// trailing `/` all yield a `.`/`..`/empty piece and are REFUSED). Never returns an empty piece list.
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

/// Resolve the on-disk bare-repo path `<root>/<tenant>/<region>/<repo>.git`, FAIL-CLOSED on any
/// traversing/absolute/separator/non-allowlisted segment (the GT-001 cross-tenant isolation boundary).
/// This is the ONLY way a host path reaches a git-wire bind mount — a raw attacker-influenced path
/// can never be mounted. Mirrors `myelin_git::gix_backend::RootedResolver::repo_path`.
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

/// **Symlink-path defence-in-depth (CT-006b 4a).** [`resolve_bare_repo_path`] closes the textual
/// path-traversal vector, but a textually-clean path can STILL escape the tenant tree at the
/// FILESYSTEM layer: a `<repo>.git` (or any resolved component) that is a SYMLINK would make the RO
/// bind-mount follow OUT of `<root>/<tenant>/<region>`. This asserts, AFTER resolution and BEFORE any
/// mount, that the resolved repo path is a REAL directory whose canonicalized location stays UNDER the
/// canonicalized `root`.
///
/// THREE complementary checks (defence in depth):
///   - `symlink_metadata` (does NOT follow the FINAL component) ⇒ a `<repo>.git` symlink is caught
///     even when it points back INSIDE the root;
///   - **per-component lstat of every attacker-influenced segment** (CT-006b FU-2) ⇒ a symlinked
///     INTERMEDIATE component is REFUSED *even when it resolves UNDER the root* — a
///     `canonicalize`+`starts_with` check alone would "launder" such a symlink, yet the bind mount
///     binds the NON-canonical path and would FOLLOW it;
///   - `canonicalize` + `starts_with(canonical_root)` ⇒ a final symlink pointing OUTSIDE is caught.
///
/// The check→mount TOCTOU is closed as far as practical: ANY symlink in the path is refused, the
/// segments are allowlist-validated single names, and the bind is READ-ONLY + non-root (a successful
/// race still cannot mutate the victim tree). A fully race-free guarantee needs
/// `openat2(RESOLVE_NO_SYMLINKS|RESOLVE_BENEATH)` fd-passing the OCI/runsc bundle API does not expose.
pub fn assert_repo_under_root(root: &Path, repo_host_path: &Path) -> Result<(), WireError> {
    let meta = std::fs::symlink_metadata(repo_host_path).map_err(|e| {
        WireError::Path(format!(
            "repo path {repo_host_path:?} is not present/stat-able ({e}) — refused before mount \
             (fail-closed)"
        ))
    })?;
    if meta.file_type().is_symlink() {
        return Err(WireError::Path(format!(
            "repo path {repo_host_path:?} is a SYMLINK — refused before mount (a symlinked \
             `<repo>.git` could make the bind-mount follow OUT of the tenant tree; defence in depth)"
        )));
    }
    if !meta.is_dir() {
        return Err(WireError::Path(format!(
            "repo path {repo_host_path:?} is not a directory — refused before mount (fail-closed)"
        )));
    }
    let canon_root = std::fs::canonicalize(root).map_err(|e| {
        WireError::Path(format!(
            "git root {root:?} could not be canonicalized ({e}) — refused before mount (fail-closed)"
        ))
    })?;
    // FU-2: lstat EVERY attacker-influenced segment below the root (`<tenant>/<region>/<repo>.git`).
    // A symlink at ANY of them is refused — even one that resolves UNDER the root — because the bind
    // mount follows the non-canonical path. The segments are the suffix of `repo_host_path` past `root`.
    let rel = repo_host_path.strip_prefix(root).map_err(|_| {
        WireError::Path(format!(
            "repo path {repo_host_path:?} is not under the configured git root {root:?} — refused \
             before mount (fail-closed)"
        ))
    })?;
    let mut cur = canon_root.clone();
    for comp in rel.components() {
        cur = cur.join(comp.as_os_str());
        let m = std::fs::symlink_metadata(&cur).map_err(|e| {
            WireError::Path(format!(
                "repo path component {cur:?} is not present/stat-able ({e}) — refused before mount"
            ))
        })?;
        if m.file_type().is_symlink() {
            return Err(WireError::Path(format!(
                "repo path component {cur:?} is a SYMLINK — refused before mount (an intermediate \
                 symlink, even one resolving UNDER the root, is a bind-mount-follow vector; FU-2)"
            )));
        }
    }
    let canon_repo = std::fs::canonicalize(repo_host_path).map_err(|e| {
        WireError::Path(format!(
            "repo path {repo_host_path:?} could not be canonicalized ({e}) — refused before mount \
             (fail-closed)"
        ))
    })?;
    if !canon_repo.starts_with(&canon_root) {
        return Err(WireError::Path(format!(
            "resolved repo path {canon_repo:?} escapes the canonical git root {canon_root:?} (a \
             symlinked component would leave the tenant tree) — refused before mount (fail-closed)"
        )));
    }
    Ok(())
}
