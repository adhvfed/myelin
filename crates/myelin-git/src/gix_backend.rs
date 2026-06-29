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
    fn read_blob(&self, repo: &RepoLoc, oid: &Oid) -> Result<Vec<u8>, GitCoreError> {
        let r = self.open(repo)?;
        let blob = r
            .find_blob(Self::parse_oid(oid)?)
            .map_err(|e| GitCoreError::Read(format!("find_blob {}: {e}", oid.as_str())))?;
        Ok(blob.content().to_vec())
    }

    fn diff_blobs(&self, repo: &RepoLoc, a: &Oid, b: &Oid) -> Result<Vec<DiffLine>, GitCoreError> {
        let r = self.open(repo)?;
        let blob_a = r
            .find_blob(Self::parse_oid(a)?)
            .map_err(|e| GitCoreError::Read(format!("find_blob {}: {e}", a.as_str())))?;
        let blob_b = r
            .find_blob(Self::parse_oid(b)?)
            .map_err(|e| GitCoreError::Read(format!("find_blob {}: {e}", b.as_str())))?;

        let mut lines: Vec<DiffLine> = Vec::new();
        // libgit2's blob-to-blob diff = the Myers/Histogram unified diff the anchor remap feeds on.
        let mut line_cb = |_delta: git2::DiffDelta<'_>,
                           _hunk: Option<git2::DiffHunk<'_>>,
                           line: git2::DiffLine<'_>| {
            let origin = line.origin();
            if matches!(origin, '+' | '-' | ' ') {
                lines.push(DiffLine {
                    origin,
                    content: String::from_utf8_lossy(line.content())
                        .trim_end_matches('\n')
                        .to_string(),
                });
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
        .map_err(|e| GitCoreError::Read(format!("diff_blobs: {e}")))?;
        Ok(lines)
    }

    fn blame(&self, repo: &RepoLoc, path: &str, at: &Oid) -> Result<Vec<BlameHunk>, GitCoreError> {
        let r = self.open(repo)?;
        let mut opts = git2::BlameOptions::new();
        opts.newest_commit(Self::parse_oid(at)?);
        let blame = r
            .blame_file(std::path::Path::new(path), Some(&mut opts))
            .map_err(|e| GitCoreError::Read(format!("blame {path}: {e}")))?;
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
}
