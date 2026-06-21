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
        Ok(self
            .root
            .join(&repo.tenant)
            .join(&repo.region)
            .join(format!("{}.git", repo.repo)))
    }
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
