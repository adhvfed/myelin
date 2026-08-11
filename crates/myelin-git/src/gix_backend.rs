use crate::core::{BlameHunk, DiffLine, GitCoreError, Oid, ReadBackend, RepoLoc};
use std::path::PathBuf;

pub trait RepoPathResolver {
    fn repo_path(&self, repo: &RepoLoc) -> Result<PathBuf, GitCoreError>;
}

pub struct RootedResolver {
    root: PathBuf,
}

impl RootedResolver {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl RepoPathResolver for RootedResolver {
    fn repo_path(&self, repo: &RepoLoc) -> Result<PathBuf, GitCoreError> {
        validate_path_segment("tenant", &repo.tenant)?;
        validate_path_segment("region", &repo.region)?;
        let repo_dir = validate_repo_slug(&repo.repo)?;
        let mut path = self.root.clone();
        path.push(&repo.tenant);
        path.push(&repo.region);
        for piece in &repo_dir {
            path.push(piece);
        }
        let last = repo_dir
            .last()
            .expect("validate_repo_slug returns ≥1 piece or errors");
        path.set_file_name(format!("{last}.git"));
        Ok(path)
    }
}

pub fn validate_path_segment(kind: &str, seg: &str) -> Result<(), GitCoreError> {
    if seg.is_empty() {
        return Err(GitCoreError::Read(format!(
            "invalid {kind} path segment: empty (fail-closed - refusing to resolve a path)"
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
                 [A-Za-z0-9._-] - separators/NUL/control chars are refused (path-traversal / \
                 absolute-component guard, fail-closed)"
            )));
        }
    }
    Ok(())
}

pub fn validate_repo_slug(repo: &str) -> Result<Vec<String>, GitCoreError> {
    if repo.contains('\\') || repo.contains('\0') {
        return Err(GitCoreError::Read(format!(
            "invalid repo slug {repo:?}: contains a backslash/NUL (path-traversal guard, fail-closed)"
        )));
    }
    let pieces: Vec<String> = repo.split('/').map(|s| s.to_string()).collect();
    for piece in &pieces {
        validate_path_segment("repo", piece)?;
    }
    if pieces
        .iter()
        .take(pieces.len().saturating_sub(1))
        .any(|piece| piece.to_ascii_lowercase().ends_with(".git"))
    {
        return Err(GitCoreError::Read(format!(
            "invalid repo slug {repo:?}: a namespace segment cannot end in `.git` because it \
             would resolve inside another bare repository (fail-closed)"
        )));
    }
    Ok(pieces)
}

pub struct GixCore<P: RepoPathResolver> {
    resolver: P,
}

impl<P: RepoPathResolver> GixCore<P> {
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

    #[test]
    fn a_namespace_cannot_resolve_inside_another_bare_repository() {
        for slug in ["victim.git/tools", "victim.GIT/tools"] {
            let resolved = root().repo_path(&RepoLoc::new("acme", "fr-par", slug));
            assert!(
                matches!(resolved, Err(GitCoreError::Read(_))),
                "{slug:?} must not resolve beneath victim.git, got {resolved:?}"
            );
        }

        assert_eq!(
            root()
                .repo_path(&RepoLoc::new("acme", "fr-par", "team/service.git"))
                .expect("a final .git suffix cannot create a nested repository"),
            std::path::Path::new("/srv/git-root/acme/fr-par/team/service.git.git")
        );
    }

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

    #[test]
    fn absolute_component_is_refused() {
        assert!(matches!(
            root().repo_path(&RepoLoc::new("/abs/path", "fr-par", "core")),
            Err(GitCoreError::Read(_))
        ));
        assert!(matches!(
            root().repo_path(&RepoLoc::new("acme", "fr-par", "/etc/passwd")),
            Err(GitCoreError::Read(_))
        ));
    }

    #[test]
    fn nul_backslash_and_dot_segments_are_refused() {
        for bad in [
            RepoLoc::new("acme", "fr-par", "a\\b"),
            RepoLoc::new("acme", "fr-par", "a\0b"),
            RepoLoc::new("acme", "fr-par", ".."),
            RepoLoc::new("acme", "fr-par", "."),
            RepoLoc::new("acme", "fr-par", ""),
            RepoLoc::new("acme", "..", "core"),
            RepoLoc::new("..", "fr-par", "core"),
            RepoLoc::new("acme", "fr-par", "a/../../b"),
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
