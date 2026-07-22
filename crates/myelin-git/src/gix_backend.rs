//! # `GixCore` — the in-process read backend (GIT-P8 / P-269)
//!
//! The [`ReadBackend`] half of the [`crate::core::GitCore`] seam: read/diff/blame **in-process**,
//! no `git` fork per call (no-host-exec holds by construction — these are library calls, not a
//! `Command`). This is the hot, high-fan-out read path the front end + the code projection hammer.
//!
//! ## libgit2 in v1 (the architecture-named fallback) — a DOCUMENTED deviation (EI-01 §1)
//!
//! Architecture `01 §2.2` prefers `gix` (gitoxide) in-process and names **`libgit2` as the
//! fallback** "where `gix` lacks a read capability." At GIT-P8 the current `gix` release
//! (`gix 0.84` / `gix-hash`) **fails to compile** on the workspace toolchain (rustc 1.95, 2026-06)
//! — adding it would break `cargo build --workspace`. The architecture already sanctions `libgit2`
//! for exactly this gap, so the v1 in-process read backend is realised over **`git2` (libgit2)**: a
//! *real* read/diff/blame path that satisfies the seam's read port. The gix-preferred swap is the
//! **OQ-1 named floor (GIT-P33)** — a per-op flip behind the same [`ReadBackend`] port.
//!
//! `git2` carries an unsafe C FFI surface (the named cost, `01 §2.2`), but exposes a **safe** Rust
//! API; this crate stays `#![forbid(unsafe_code)]`. The path resolves objects against a real
//! on-disk repository (the placement_of(repo) → backend node the serving tier mounts — GIT-P13).

use crate::core::{BlameHunk, DiffLine, GitCoreError, Oid, ReadBackend, RepoLoc};
use std::path::PathBuf;

/// Resolves a [`RepoLoc`] to its on-disk repository path. The production resolver consults
/// `placement_of(repo)` + the residency pin (GIT-P13); this seam carries the lookup as a port so
/// the smoke test mounts a fixture repo and the serving tier mounts the real placement.
pub trait RepoPathResolver {
    /// The on-disk path of the bare repository for `repo` (the object DB the read ops open).
    fn repo_path(&self, repo: &RepoLoc) -> Result<PathBuf, GitCoreError>;
}

/// A resolver that maps `(tenant, region, repo)` under a fixed root — the v1 local-NVMe layout
/// behind the `BlobStore` trait (`01 §1`, "v1 packs on local NVMe behind the trait"). The serving
/// tier swaps the real placement resolver behind the same [`RepoPathResolver`] port (GIT-P13).
pub struct RootedResolver {
    root: PathBuf,
}

impl RootedResolver {
    /// Root the resolver at a directory holding `<tenant>/<region>/<repo>.git` bare repos.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl RepoPathResolver for RootedResolver {
    fn repo_path(&self, repo: &RepoLoc) -> Result<PathBuf, GitCoreError> {
        // SECURITY (the GT-001 isolation boundary): the locator components are user/URL-controlled
        // (the repo slug + the tenant/region from the request), so a raw `PathBuf::join` of
        // UNVALIDATED segments is a cross-tenant path-traversal breakout — e.g. a repo slug
        // `../../tenant-a/fr-par/secret` collapses onto another tenant's repo, and an absolute
        // `tenant = /abs` discards the root entirely (write-anywhere). We FAIL-CLOSED: every segment
        // is validated and a traversing/absolute/separator/NUL segment is REFUSED before any path is
        // built. The tenant/region are single segments; the repo MAY be namespaced (`team/app`) so it
        // is validated per `/`-separated piece (each piece still cannot be `.`/`..`/empty/absolute).
        // Both the READ backend ([`GixCore`]) and the WRITE/lifecycle store
        // ([`crate::durable::DurableGitStore`]) resolve through THIS one method, so this single check
        // closes both paths.
        validate_path_segment("tenant", &repo.tenant)?;
        validate_path_segment("region", &repo.region)?;
        let repo_dir = validate_repo_slug(&repo.repo)?;
        let mut path = self.root.clone();
        path.push(&repo.tenant);
        path.push(&repo.region);
        for piece in &repo_dir {
            path.push(piece);
        }
        // The final on-disk dir is the last repo piece with the `.git` suffix.
        let last = repo_dir
            .last()
            .expect("validate_repo_slug returns ≥1 piece or errors");
        path.set_file_name(format!("{last}.git"));
        Ok(path)
    }
}

/// Reject a single path segment that could escape the per-tenant/region root. The security-critical
/// rejects: empty, `.`, `..` (traversal), an absolute/rooted segment, and any segment containing a
/// path separator (`/`, `\`), a NUL, or a non-allowlisted char. The allowlist is `[A-Za-z0-9._-]`
/// (git owner/repo names — and ULID-style tenant ids — are already so constrained; uppercase is
/// permitted because it is not a path-traversal vector and ULID tenant ids are upper-base32).
pub fn validate_path_segment(kind: &str, seg: &str) -> Result<(), GitCoreError> {
    if seg.is_empty() {
        return Err(GitCoreError::Read(format!(
            "invalid {kind} path segment: empty (fail-closed — refusing to resolve a path)"
        )));
    }
    if seg == "." || seg == ".." {
        return Err(GitCoreError::Read(format!(
            "invalid {kind} path segment {seg:?}: path-traversal component refused (fail-closed)"
        )));
    }
    for c in seg.chars() {
        let ok = c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');
        if !ok {
            return Err(GitCoreError::Read(format!(
                "invalid {kind} path segment {seg:?}: character {c:?} not in the allowlist \
                 [A-Za-z0-9._-] — separators/NUL/control chars are refused (path-traversal / \
                 absolute-component guard, fail-closed)"
            )));
        }
    }
    Ok(())
}

/// Validate a (possibly namespaced) repo slug and return its `/`-separated, individually-validated
/// pieces. A namespaced slug (`team/app`) is admitted, but each piece is held to
/// [`validate_path_segment`] — so `../../x`, `/etc/passwd`, `a//b`, and a trailing `/` are all
/// REFUSED (each yields a `.`/`..`/empty piece). Never returns an empty piece list.
pub fn validate_repo_slug(repo: &str) -> Result<Vec<String>, GitCoreError> {
    // A backslash never separates here, but it IS a Windows separator + a traversal vector — reject
    // it outright (validate_path_segment also rejects it per-piece, but a slug-level guard is loud).
    if repo.contains('\\') || repo.contains('\0') {
        return Err(GitCoreError::Read(format!(
            "invalid repo slug {repo:?}: contains a backslash/NUL (path-traversal guard, fail-closed)"
        )));
    }
    let pieces: Vec<String> = repo.split('/').map(|s| s.to_string()).collect();
    for piece in &pieces {
        validate_path_segment("repo", piece)?;
    }
    Ok(pieces)
}

/// The in-process read backend over `git2` (libgit2 — the architecture-named fallback; gix-preferred
/// is OQ-1/GIT-P33). Generic over a [`RepoPathResolver`] so the fixture-mount (smoke test) and the
/// placement-resolved mount (serving tier) swap cleanly.
pub struct GixCore<P: RepoPathResolver> {
    resolver: P,
}

impl<P: RepoPathResolver> GixCore<P> {
    /// Build the read backend over a repo-path resolver.
    pub fn new(resolver: P) -> Self {
        Self { resolver }
    }

    fn open(&self, repo: &RepoLoc) -> Result<git2::Repository, GitCoreError> {
        let path = self.resolver.repo_path(repo)?;
        git2::Repository::open(&path)
            .map_err(|e| GitCoreError::Read(format!("open {}: {e}", path.display())))
    }

    fn parse_oid(oid: &Oid) -> Result<git2::Oid, GitCoreError> {
        git2::Oid::from_str(oid.as_str())
            .map_err(|e| GitCoreError::Read(format!("bad oid {}: {e}", oid.as_str())))
    }
}

impl<P: RepoPathResolver> ReadBackend for GixCore<P> {
    fn read_blob_bounded(
        &self,
        repo: &RepoLoc,
        oid: &Oid,
        maximum_bytes: usize,
    ) -> Result<Vec<u8>, GitCoreError> {
        let r = self.open(repo)?;
        let oid = Self::parse_oid(oid)?;
        let odb = r
            .odb()
            .map_err(|e| GitCoreError::Read(format!("open object database: {e}")))?;
        let (size, kind) = odb
            .read_header(oid)
            .map_err(|e| GitCoreError::Read(format!("read blob header {oid}: {e}")))?;
        if kind != git2::ObjectType::Blob {
            return Err(GitCoreError::Read(format!("object {oid} is not a blob")));
        }
        if size > maximum_bytes {
            return Err(GitCoreError::Read(format!(
                "blob read limit exceeded: {size} bytes exceeds {maximum_bytes}"
            )));
        }
        let blob = r
            .find_blob(oid)
            .map_err(|e| GitCoreError::Read(format!("find_blob {oid}: {e}")))?;
        Ok(blob.content().to_vec())
    }

    fn diff_blobs_bounded(
        &self,
        repo: &RepoLoc,
        a: &Oid,
        b: &Oid,
        maximum_blob_bytes: usize,
        maximum_lines: usize,
        maximum_output_bytes: usize,
    ) -> Result<Vec<DiffLine>, GitCoreError> {
        let r = self.open(repo)?;
        let a = Self::parse_oid(a)?;
        let b = Self::parse_oid(b)?;
        let odb = r
            .odb()
            .map_err(|e| GitCoreError::Read(format!("open object database: {e}")))?;
        for (side, oid) in [("left", a), ("right", b)] {
            let (size, kind) = odb.read_header(oid).map_err(|e| {
                GitCoreError::Read(format!("read {side} diff blob header {oid}: {e}"))
            })?;
            if kind != git2::ObjectType::Blob {
                return Err(GitCoreError::Read(format!(
                    "{side} diff object {oid} is not a blob"
                )));
            }
            if size > maximum_blob_bytes {
                return Err(GitCoreError::Read(format!(
                    "diff blob limit exceeded: {side} blob has {size} bytes, maximum is {maximum_blob_bytes}"
                )));
            }
        }
        let blob_a = r
            .find_blob(a)
            .map_err(|e| GitCoreError::Read(format!("find_blob {a}: {e}")))?;
        let blob_b = r
            .find_blob(b)
            .map_err(|e| GitCoreError::Read(format!("find_blob {b}: {e}")))?;

        let mut lines: Vec<DiffLine> = Vec::new();
        let mut output_bytes = 0usize;
        let mut limit_error = None;
        let result = {
            // libgit2's blob-to-blob diff = the Myers/Histogram unified diff the anchor remap feeds
            // on. Scope the callback so its mutable borrows end before the limit verdict is read.
            let mut line_cb = |_delta: git2::DiffDelta<'_>,
                               _hunk: Option<git2::DiffHunk<'_>>,
                               line: git2::DiffLine<'_>| {
                let origin = line.origin();
                if matches!(origin, '+' | '-' | ' ') {
                    if lines.len() >= maximum_lines {
                        limit_error = Some(format!(
                            "diff output line limit exceeded: maximum is {maximum_lines}"
                        ));
                        return false;
                    }
                    let content = String::from_utf8_lossy(line.content())
                        .trim_end_matches('\n')
                        .to_string();
                    let Some(next_output_bytes) = output_bytes.checked_add(content.len()) else {
                        limit_error = Some("diff output byte count overflowed".into());
                        return false;
                    };
                    if next_output_bytes > maximum_output_bytes {
                        limit_error = Some(format!(
                            "diff output byte limit exceeded: maximum is {maximum_output_bytes}"
                        ));
                        return false;
                    }
                    output_bytes = next_output_bytes;
                    lines.push(DiffLine { origin, content });
                }
                true
            };
            r.diff_blobs(
                Some(&blob_a),
                None,
                Some(&blob_b),
                None,
                None,
                None,
                None,
                None,
                Some(&mut line_cb),
            )
        };
        if let Some(error) = limit_error {
            return Err(GitCoreError::Read(error));
        }
        result.map_err(|e| GitCoreError::Read(format!("diff_blobs: {e}")))?;
        Ok(lines)
    }

    fn blame_bounded(
        &self,
        repo: &RepoLoc,
        path: &str,
        at: &Oid,
        maximum_path_bytes: usize,
        maximum_blob_bytes: usize,
        maximum_hunks: usize,
    ) -> Result<Vec<BlameHunk>, GitCoreError> {
        if path.len() > maximum_path_bytes {
            return Err(GitCoreError::Read(format!(
                "blame path limit exceeded: {} bytes exceeds {maximum_path_bytes}",
                path.len()
            )));
        }
        if path.is_empty()
            || path.starts_with('/')
            || path.contains('\\')
            || path.contains('\0')
            || path
                .split('/')
                .any(|component| component.is_empty() || matches!(component, "." | ".."))
        {
            return Err(GitCoreError::Read(
                "blame path must be a normalized repository-relative path".into(),
            ));
        }
        let r = self.open(repo)?;
        let at = Self::parse_oid(at)?;
        let commit = r
            .find_commit(at)
            .map_err(|e| GitCoreError::Read(format!("find blame commit {at}: {e}")))?;
        let tree = commit
            .tree()
            .map_err(|e| GitCoreError::Read(format!("read blame commit tree {at}: {e}")))?;
        let entry = tree
            .get_path(std::path::Path::new(path))
            .map_err(|e| GitCoreError::Read(format!("resolve blame path {path}: {e}")))?;
        let odb = r
            .odb()
            .map_err(|e| GitCoreError::Read(format!("open object database: {e}")))?;
        let (size, kind) = odb
            .read_header(entry.id())
            .map_err(|e| GitCoreError::Read(format!("read blame blob header {}: {e}", entry.id())))?;
        if kind != git2::ObjectType::Blob {
            return Err(GitCoreError::Read(format!(
                "blame path {path} does not resolve to a blob"
            )));
        }
        if size > maximum_blob_bytes {
            return Err(GitCoreError::Read(format!(
                "blame blob limit exceeded: {size} bytes exceeds {maximum_blob_bytes}"
            )));
        }
        let mut opts = git2::BlameOptions::new();
        opts.newest_commit(at);
        let blame = r
            .blame_file(std::path::Path::new(path), Some(&mut opts))
            .map_err(|e| GitCoreError::Read(format!("blame {path}: {e}")))?;
        if blame.len() > maximum_hunks {
            return Err(GitCoreError::Read(format!(
                "blame hunk limit exceeded: {} hunks exceeds {maximum_hunks}",
                blame.len()
            )));
        }
        let mut hunks = Vec::with_capacity(blame.len());
        for h in blame.iter() {
            hunks.push(BlameHunk {
                final_start_line: h.final_start_line(),
                lines: h.lines_in_hunk(),
                commit: Oid::new(h.final_commit_id().to_string()),
            });
        }
        Ok(hunks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedResolver(PathBuf);

    impl RepoPathResolver for FixedResolver {
        fn repo_path(&self, _repo: &RepoLoc) -> Result<PathBuf, GitCoreError> {
            Ok(self.0.clone())
        }
    }

    fn root() -> RootedResolver {
        RootedResolver::new("/srv/git-root")
    }

    /// A benign locator resolves to exactly `<root>/<tenant>/<region>/<repo>.git`.
    #[test]
    fn benign_locator_resolves_under_the_root() {
        let p = root()
            .repo_path(&RepoLoc::new("acme", "fr-par", "core"))
            .expect("benign locator resolves");
        assert_eq!(
            p,
            std::path::Path::new("/srv/git-root/acme/fr-par/core.git")
        );
    }

    /// A namespaced (sub-grouped) repo slug `team/app` is admitted — and resolves WITHIN the root.
    #[test]
    fn namespaced_repo_slug_is_admitted_within_root() {
        let p = root()
            .repo_path(&RepoLoc::new("acme", "fr-par", "team/app"))
            .expect("namespaced slug resolves");
        assert_eq!(
            p,
            std::path::Path::new("/srv/git-root/acme/fr-par/team/app.git")
        );
        assert!(p.starts_with("/srv/git-root/acme/fr-par"));
    }

    /// **SECURITY: the cross-tenant breakout via a `..` repo slug is REFUSED (the damning vector).**
    /// `repo = ../../tenant-a/fr-par/secret` would collapse onto tenant A's repo — it must fail-closed.
    #[test]
    fn cross_tenant_dotdot_repo_slug_is_refused() {
        let r = root().repo_path(&RepoLoc::new(
            "tenant-b",
            "fr-par",
            "../../tenant-a/fr-par/secret",
        ));
        assert!(
            matches!(r, Err(GitCoreError::Read(_))),
            "a `..` traversal repo slug must be refused, got {r:?}"
        );
    }

    /// **SECURITY: an absolute / rooted tenant segment is REFUSED (write-anywhere vector).**
    #[test]
    fn absolute_component_is_refused() {
        // A `/`-bearing tenant (absolute escape) is rejected (tenant must be one safe segment).
        assert!(matches!(
            root().repo_path(&RepoLoc::new("/abs/path", "fr-par", "core")),
            Err(GitCoreError::Read(_))
        ));
        // A repo slug that starts at root is rejected (empty first piece).
        assert!(matches!(
            root().repo_path(&RepoLoc::new("acme", "fr-par", "/etc/passwd")),
            Err(GitCoreError::Read(_))
        ));
    }

    /// **SECURITY: NUL, backslash, and bare `.`/`..` segments are all REFUSED.**
    #[test]
    fn nul_backslash_and_dot_segments_are_refused() {
        for bad in [
            RepoLoc::new("acme", "fr-par", "a\\b"),       // backslash (Windows sep / traversal)
            RepoLoc::new("acme", "fr-par", "a\0b"),       // NUL
            RepoLoc::new("acme", "fr-par", ".."),          // bare parent
            RepoLoc::new("acme", "fr-par", "."),           // bare current
            RepoLoc::new("acme", "fr-par", ""),            // empty
            RepoLoc::new("acme", "..", "core"),            // `..` region
            RepoLoc::new("..", "fr-par", "core"),          // `..` tenant
            RepoLoc::new("acme", "fr-par", "a/../../b"),   // mid-slug traversal
        ] {
            assert!(
                matches!(root().repo_path(&bad), Err(GitCoreError::Read(_))),
                "expected refusal for {bad:?}"
            );
        }
    }

    #[test]
    fn blob_read_rejects_from_header_above_the_caller_limit() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("test clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "myelin-gix-read-bound-{}-{nonce}.git",
            std::process::id()
        ));
        let repository = git2::Repository::init_bare(&path).expect("init bare test repository");
        let payload = b"bounded blob";
        let oid = repository.blob(payload).expect("write test blob");
        drop(repository);

        let reader = GixCore::new(FixedResolver(path.clone()));
        let repo = RepoLoc::new("acme", "eu-west", "widgets");
        let oid = Oid::new(oid.to_string());
        assert_eq!(
            reader
                .read_blob_bounded(&repo, &oid, payload.len())
                .expect("exact limit is accepted"),
            payload
        );
        let error = reader
            .read_blob_bounded(&repo, &oid, payload.len() - 1)
            .expect_err("cap plus one is rejected");
        assert!(error.to_string().contains("blob read limit exceeded"));

        std::fs::remove_dir_all(&path).expect("remove isolated test repository");
    }

    #[test]
    fn blob_diff_enforces_input_line_and_output_byte_limits() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("test clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "myelin-gix-diff-bound-{}-{nonce}.git",
            std::process::id()
        ));
        let repository = git2::Repository::init_bare(&path).expect("init bare test repository");
        let left = Oid::new(repository.blob(b"a\n").expect("write left blob").to_string());
        let right = Oid::new(repository.blob(b"b\n").expect("write right blob").to_string());
        drop(repository);

        let reader = GixCore::new(FixedResolver(path.clone()));
        let repo = RepoLoc::new("acme", "eu-west", "widgets");
        let exact = reader
            .diff_blobs_bounded(&repo, &left, &right, 2, 2, 2)
            .expect("exact limits are accepted");
        assert_eq!(exact.len(), 2);
        assert!(reader.diff_blobs_bounded(&repo, &left, &right, 1, 2, 2).is_err());
        assert!(reader.diff_blobs_bounded(&repo, &left, &right, 2, 1, 2).is_err());
        assert!(reader.diff_blobs_bounded(&repo, &left, &right, 2, 2, 1).is_err());

        std::fs::remove_dir_all(&path).expect("remove isolated test repository");
    }
}
