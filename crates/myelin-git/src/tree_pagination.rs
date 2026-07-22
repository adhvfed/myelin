//! Bounded, snapshot-stable pagination over one durable Git tree directory.
//!
//! The requested ref and path are resolved normally on every call. Cursor object ids are compared
//! with that resolved commit and directory tree only as consistency fences; they are never used to
//! select objects. A scan accounts the complete directory under wire ceilings while retaining only
//! the smallest `limit + 1` matching rows after the keyset. Blob headers are read after truncation,
//! for returned file rows only.

use std::cmp::Ordering;

use base64::Engine as _;

use crate::core::Oid;
use crate::durable::{DurableError, DurableGitRepo, TreeEntryInfo, TREE_OBJECT_MAX_BYTES};

/// Default and maximum number of rows in one tree page.
pub const TREE_PAGE_DEFAULT_LIMIT: usize = 100;
pub const TREE_PAGE_MAX_LIMIT: usize = 100;
/// Maximum raw and normalized basename-query bytes.
pub const TREE_PAGE_MAX_QUERY_BYTES: usize = 256;
/// Maximum entries inspected in one directory scan.
pub const TREE_PAGE_SCAN_MAX_ENTRIES: usize = 100_000;
/// Maximum UTF-8 bytes in one scanned entry basename.
pub const TREE_PAGE_SCAN_MAX_NAME_BYTES: usize = 4 * 1024;
/// Maximum aggregate UTF-8 basename bytes inspected by one scan.
pub const TREE_PAGE_SCAN_MAX_TOTAL_NAME_BYTES: usize = 32 * 1024 * 1024;
/// Maximum history entries inspected for latest-commit metadata on a selected page.
pub const TREE_PAGE_LATEST_COMMIT_WALK_MAX: usize = 500;

const CURSOR_PREFIX: &str = "gt1_";
const CURSOR_VERSION: u8 = 1;
const CURSOR_FIXED_BYTES: usize = 1 + 20 + 20 + 32 + 1 + 2;
const CURSOR_MAX_DECODED_BYTES: usize = CURSOR_FIXED_BYTES + TREE_PAGE_SCAN_MAX_NAME_BYTES;

#[derive(Clone, Copy)]
struct ScanLimits {
    entries: usize,
    one_name_bytes: usize,
    total_name_bytes: usize,
}

const WIRE_SCAN_LIMITS: ScanLimits = ScanLimits {
    entries: TREE_PAGE_SCAN_MAX_ENTRIES,
    one_name_bytes: TREE_PAGE_SCAN_MAX_NAME_BYTES,
    total_name_bytes: TREE_PAGE_SCAN_MAX_TOTAL_NAME_BYTES,
};

/// Additive tree-page request. `query` matches normalized immediate-child basenames only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreePageRequest {
    pub limit: usize,
    pub query: Option<String>,
    pub cursor: Option<String>,
}

impl Default for TreePageRequest {
    fn default() -> Self {
        Self {
            limit: TREE_PAGE_DEFAULT_LIMIT,
            query: None,
            cursor: None,
        }
    }
}

/// One immutable directory page. README content is intentionally not part of this durable method.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreePage {
    /// Commit actually resolved from the requested ref for this page.
    pub snapshot_oid: Oid,
    /// Directory tree object actually resolved at the requested path.
    pub tree_oid: Oid,
    pub entries: Vec<TreeEntryInfo>,
    pub next_cursor: Option<String>,
}

/// Typed path outcome matching the existing tree browse surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreePageLookup {
    Dir(TreePage),
    IsFile,
    Missing,
}

/// Typed request and cursor failures, separate from durable repository failures.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreePageError {
    Durable(DurableError),
    InvalidLimit { supplied: usize },
    QueryTooLong { maximum: usize },
    InvalidQuery,
    MalformedCursor,
    CursorScopeMismatch,
    CursorStale,
}

impl std::fmt::Display for TreePageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Durable(error) => error.fmt(f),
            Self::InvalidLimit { supplied } => write!(
                f,
                "tree page limit {supplied} is outside 1..={TREE_PAGE_MAX_LIMIT}"
            ),
            Self::QueryTooLong { maximum } => {
                write!(f, "tree query exceeds the {maximum}-byte limit")
            }
            Self::InvalidQuery => write!(f, "tree query contains control characters"),
            Self::MalformedCursor => write!(f, "malformed tree-page cursor"),
            Self::CursorScopeMismatch => write!(f, "tree-page cursor belongs to another scope"),
            Self::CursorStale => write!(f, "tree-page cursor is stale"),
        }
    }
}

impl std::error::Error for TreePageError {}

impl From<DurableError> for TreePageError {
    fn from(value: DurableError) -> Self {
        Self::Durable(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum EntryKind {
    Dir,
    File,
}

impl EntryKind {
    fn byte(self) -> u8 {
        match self {
            Self::Dir => 0,
            Self::File => 1,
        }
    }

    fn from_byte(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Dir),
            1 => Some(Self::File),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
struct EntryKey<'a> {
    kind: EntryKind,
    name: &'a str,
}

impl PartialEq for EntryKey<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind && self.name.as_bytes() == other.name.as_bytes()
    }
}

impl Eq for EntryKey<'_> {}

impl PartialOrd for EntryKey<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for EntryKey<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.kind
            .cmp(&other.kind)
            .then_with(|| self.name.as_bytes().cmp(other.name.as_bytes()))
    }
}

#[derive(Clone, Debug)]
struct ScannedEntry {
    kind: EntryKind,
    name: String,
    oid: git2::Oid,
}

impl ScannedEntry {
    fn key(&self) -> EntryKey<'_> {
        EntryKey {
            kind: self.kind,
            name: &self.name,
        }
    }
}

#[derive(Clone, Debug)]
struct Cursor {
    snapshot_oid: [u8; 20],
    tree_oid: [u8; 20],
    scope: [u8; 32],
    kind: EntryKind,
    name: String,
}

impl Cursor {
    fn encode(&self) -> String {
        let mut frame = Vec::with_capacity(CURSOR_FIXED_BYTES + self.name.len());
        frame.push(CURSOR_VERSION);
        frame.extend_from_slice(&self.snapshot_oid);
        frame.extend_from_slice(&self.tree_oid);
        frame.extend_from_slice(&self.scope);
        frame.push(self.kind.byte());
        frame.extend_from_slice(&(self.name.len() as u16).to_be_bytes());
        frame.extend_from_slice(self.name.as_bytes());
        format!(
            "{CURSOR_PREFIX}{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(frame)
        )
    }

    fn parse(value: &str) -> Result<Self, TreePageError> {
        let encoded = value
            .strip_prefix(CURSOR_PREFIX)
            .ok_or(TreePageError::MalformedCursor)?;
        if encoded.is_empty() || encoded.len() > encoded_len(CURSOR_MAX_DECODED_BYTES) {
            return Err(TreePageError::MalformedCursor);
        }
        let frame = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| TreePageError::MalformedCursor)?;
        if frame.len() < CURSOR_FIXED_BYTES || frame.len() > CURSOR_MAX_DECODED_BYTES {
            return Err(TreePageError::MalformedCursor);
        }
        if base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&frame) != encoded
            || frame[0] != CURSOR_VERSION
        {
            return Err(TreePageError::MalformedCursor);
        }
        let mut snapshot_oid = [0_u8; 20];
        snapshot_oid.copy_from_slice(&frame[1..21]);
        let mut tree_oid = [0_u8; 20];
        tree_oid.copy_from_slice(&frame[21..41]);
        let mut scope = [0_u8; 32];
        scope.copy_from_slice(&frame[41..73]);
        let kind = EntryKind::from_byte(frame[73]).ok_or(TreePageError::MalformedCursor)?;
        let name_len = usize::from(u16::from_be_bytes([frame[74], frame[75]]));
        if name_len == 0
            || name_len > TREE_PAGE_SCAN_MAX_NAME_BYTES
            || frame.len() != CURSOR_FIXED_BYTES + name_len
        {
            return Err(TreePageError::MalformedCursor);
        }
        let name = std::str::from_utf8(&frame[CURSOR_FIXED_BYTES..])
            .map_err(|_| TreePageError::MalformedCursor)?
            .to_string();
        if name.contains(['\0', '/']) {
            return Err(TreePageError::MalformedCursor);
        }
        Ok(Self {
            snapshot_oid,
            tree_oid,
            scope,
            kind,
            name,
        })
    }
}

const fn encoded_len(decoded: usize) -> usize {
    decoded.div_ceil(3) * 4
}

impl DurableGitRepo {
    /// Return one bounded, snapshot-stable page from the directory at `path` under `ref_name`.
    pub fn tree_page(
        &self,
        ref_name: &str,
        path: &str,
        request: TreePageRequest,
    ) -> Result<TreePageLookup, TreePageError> {
        if request.limit == 0 || request.limit > TREE_PAGE_MAX_LIMIT {
            return Err(TreePageError::InvalidLimit {
                supplied: request.limit,
            });
        }
        let query = normalize_query(request.query.as_deref())?;
        let repo = self.open_git()?;
        let Some(commit) = self.resolve_commit(&repo, ref_name)? else {
            return Ok(TreePageLookup::Missing);
        };
        let root = find_tree_bounded(&repo, commit.tree_id(), TREE_OBJECT_MAX_BYTES)?;
        let Some(clean_path) = normalize_safe_path(path) else {
            return Ok(TreePageLookup::Missing);
        };
        let tree = if clean_path.is_empty() {
            root
        } else {
            let entry = match root.get_path(std::path::Path::new(clean_path)) {
                Ok(entry) => entry,
                Err(error) if error.code() == git2::ErrorCode::NotFound => {
                    return Ok(TreePageLookup::Missing);
                }
                Err(error) => {
                    return Err(DurableError::Git(format!("tree get_path: {error}")).into());
                }
            };
            match entry.kind() {
                Some(git2::ObjectType::Tree) => {
                    find_tree_bounded(&repo, entry.id(), TREE_OBJECT_MAX_BYTES)?
                }
                Some(git2::ObjectType::Blob) => return Ok(TreePageLookup::IsFile),
                _ => return Ok(TreePageLookup::Missing),
            }
        };

        let snapshot_git_oid = commit.id();
        let tree_git_oid = tree.id();
        let snapshot_cursor_oid = oid_frame(snapshot_git_oid);
        let tree_cursor_oid = oid_frame(tree_git_oid);
        let scope = self.tree_scope_hash(ref_name, clean_path, query.as_deref())?;
        let cursor = request.cursor.as_deref().map(Cursor::parse).transpose()?;
        if cursor.as_ref().is_some_and(|value| value.scope != scope) {
            return Err(TreePageError::CursorScopeMismatch);
        }
        if cursor.as_ref().is_some_and(|value| {
            value.snapshot_oid.as_slice() != snapshot_git_oid.as_bytes()
                || value.tree_oid.as_slice() != tree_git_oid.as_bytes()
        }) {
            return Err(TreePageError::CursorStale);
        }

        let scan = scan_tree(
            &tree,
            query.as_deref(),
            cursor.as_ref(),
            request.limit + 1,
            WIRE_SCAN_LIMITS,
        )?;
        if cursor.is_some() && !scan.cursor_key_seen {
            return Err(TreePageError::MalformedCursor);
        }
        let odb = repo
            .odb()
            .map_err(|error| DurableError::Git(format!("open object database: {error}")))?;
        let finished = finish_page(scan.rows, request.limit, |oid| {
            let (size, kind) = odb
                .read_header(oid)
                .map_err(|error| DurableError::Git(format!("read tree entry header: {error}")))?;
            Ok((kind == git2::ObjectType::Blob).then_some(size as u64))
        })?;
        let next_cursor = finished.next_key.map(|(kind, name)| {
            Cursor {
                snapshot_oid: snapshot_cursor_oid,
                tree_oid: tree_cursor_oid,
                scope,
                kind,
                name,
            }
            .encode()
        });
        Ok(TreePageLookup::Dir(TreePage {
            snapshot_oid: Oid::new(snapshot_git_oid.to_string()),
            tree_oid: Oid::new(tree_git_oid.to_string()),
            entries: finished.entries,
            next_cursor,
        }))
    }

    fn tree_scope_hash(
        &self,
        ref_name: &str,
        path: &str,
        query: Option<&str>,
    ) -> Result<[u8; 32], DurableError> {
        let verified_path = std::fs::canonicalize(self.path()).map_err(|error| {
            DurableError::Io(format!(
                "canonicalize repository {} for tree cursor: {error}",
                self.path().display()
            ))
        })?;
        let mut hash = blake3::Hasher::new();
        hash.update(b"myelin.git.tree.scope.v1\0");
        hash_field(&mut hash, verified_path.as_os_str().as_encoded_bytes());
        hash_field(&mut hash, ref_name.as_bytes());
        hash_field(&mut hash, path.as_bytes());
        match query {
            Some(value) => {
                hash.update(&[1]);
                hash_field(&mut hash, value.as_bytes());
            }
            None => {
                hash.update(&[0]);
            }
        }
        Ok(*hash.finalize().as_bytes())
    }
}

fn oid_frame(oid: git2::Oid) -> [u8; 20] {
    oid.as_bytes()
        .try_into()
        .expect("libgit2 object ids are SHA-1 frames")
}

fn hash_field(hash: &mut blake3::Hasher, value: &[u8]) {
    hash.update(&(value.len() as u64).to_be_bytes());
    hash.update(value);
}

fn normalize_query(query: Option<&str>) -> Result<Option<String>, TreePageError> {
    let Some(raw) = query else { return Ok(None) };
    if raw.len() > TREE_PAGE_MAX_QUERY_BYTES {
        return Err(TreePageError::QueryTooLong {
            maximum: TREE_PAGE_MAX_QUERY_BYTES,
        });
    }
    if raw.chars().any(char::is_control) {
        return Err(TreePageError::InvalidQuery);
    }
    let normalized = raw.trim().to_lowercase();
    if normalized.len() > TREE_PAGE_MAX_QUERY_BYTES {
        return Err(TreePageError::QueryTooLong {
            maximum: TREE_PAGE_MAX_QUERY_BYTES,
        });
    }
    Ok((!normalized.is_empty()).then_some(normalized))
}

fn normalize_safe_path(path: &str) -> Option<&str> {
    if path.starts_with('/') {
        return None;
    }
    let clean = path.trim_end_matches('/');
    (clean.is_empty()
        || clean
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != ".."))
    .then_some(clean)
}

fn find_tree_bounded<'repo>(
    repo: &'repo git2::Repository,
    oid: git2::Oid,
    maximum_bytes: usize,
) -> Result<git2::Tree<'repo>, DurableError> {
    let odb = repo
        .odb()
        .map_err(|error| DurableError::Git(format!("open object database: {error}")))?;
    let (size, kind) = odb
        .read_header(oid)
        .map_err(|error| DurableError::Git(format!("read tree header: {error}")))?;
    if kind != git2::ObjectType::Tree {
        return Err(DurableError::Git("tree object has the wrong kind".into()));
    }
    if size > maximum_bytes {
        return Err(DurableError::Git(format!(
            "tree page limit exceeded: tree object is larger than {maximum_bytes} bytes"
        )));
    }
    repo.find_tree(oid)
        .map_err(|error| DurableError::Git(format!("find tree: {error}")))
}

struct ScanResult {
    rows: Vec<ScannedEntry>,
    cursor_key_seen: bool,
}

fn scan_tree(
    tree: &git2::Tree<'_>,
    query: Option<&str>,
    cursor: Option<&Cursor>,
    retain: usize,
    limits: ScanLimits,
) -> Result<ScanResult, DurableError> {
    if tree.len() > limits.entries {
        return Err(DurableError::Git(
            "tree page limit exceeded: scanned entry count".into(),
        ));
    }
    let mut rows = Vec::with_capacity(retain);
    let mut total_name_bytes = 0_usize;
    let mut cursor_key_seen = cursor.is_none();
    for entry in tree.iter() {
        let name = entry
            .name()
            .map_err(|_| DurableError::Git("tree entry name is not valid UTF-8".into()))?;
        if name.len() > limits.one_name_bytes {
            return Err(DurableError::Git(
                "tree page limit exceeded: one entry name".into(),
            ));
        }
        total_name_bytes = total_name_bytes
            .checked_add(name.len())
            .ok_or_else(|| DurableError::Git("tree page limit exceeded: name bytes".into()))?;
        if total_name_bytes > limits.total_name_bytes {
            return Err(DurableError::Git(
                "tree page limit exceeded: name bytes".into(),
            ));
        }
        let kind = if entry.kind() == Some(git2::ObjectType::Tree) {
            EntryKind::Dir
        } else {
            EntryKind::File
        };
        let matches_query = query.is_none_or(|needle| name.to_lowercase().contains(needle));
        if cursor.is_some_and(|value| {
            value.kind == kind && value.name.as_bytes() == name.as_bytes() && matches_query
        }) {
            cursor_key_seen = true;
        }
        let key = EntryKey { kind, name };
        let after_cursor = cursor.is_none_or(|value| {
            key > EntryKey {
                kind: value.kind,
                name: &value.name,
            }
        });
        if retain != 0 && matches_query && after_cursor {
            insert_bounded(
                &mut rows,
                ScannedEntry {
                    kind,
                    name: name.to_string(),
                    oid: entry.id(),
                },
                retain,
            );
        }
    }
    Ok(ScanResult {
        rows,
        cursor_key_seen,
    })
}

fn insert_bounded(rows: &mut Vec<ScannedEntry>, entry: ScannedEntry, retain: usize) {
    let index = rows
        .binary_search_by(|existing| existing.key().cmp(&entry.key()))
        .unwrap_or_else(|index| index);
    rows.insert(index, entry);
    if rows.len() > retain {
        rows.pop();
    }
}

struct FinishedPage {
    entries: Vec<TreeEntryInfo>,
    next_key: Option<(EntryKind, String)>,
}

fn finish_page(
    mut rows: Vec<ScannedEntry>,
    limit: usize,
    mut file_size: impl FnMut(git2::Oid) -> Result<Option<u64>, DurableError>,
) -> Result<FinishedPage, DurableError> {
    let has_more = rows.len() > limit;
    rows.truncate(limit);
    let next_key = has_more
        .then(|| rows.last().map(|last| (last.kind, last.name.clone())))
        .flatten();
    let mut entries = Vec::with_capacity(rows.len());
    for row in rows {
        let is_dir = row.kind == EntryKind::Dir;
        let size = if is_dir { None } else { file_size(row.oid)? };
        entries.push(TreeEntryInfo {
            name: row.name,
            is_dir,
            size,
        });
    }
    Ok(FinishedPage { entries, next_key })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::core::RepoLoc;
    use crate::durable::DurableGitStore;

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        root: std::path::PathBuf,
        repo: DurableGitRepo,
    }

    impl Fixture {
        fn new(label: &str) -> Self {
            let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "myelin-tree-page-{label}-{}-{sequence}",
                std::process::id()
            ));
            let store = DurableGitStore::rooted(&root);
            let repo = store
                .create_repo(&RepoLoc::new("tenant", "eu-north", label))
                .expect("create bare repository");
            Self { root, repo }
        }

        fn git(&self) -> git2::Repository {
            self.repo.open_git().expect("open repository")
        }

        fn commit_tree(
            &self,
            tree_oid: git2::Oid,
            parents: &[git2::Oid],
            message: &str,
        ) -> git2::Oid {
            let git = self.git();
            let tree = git.find_tree(tree_oid).expect("find tree");
            let parent_commits = parents
                .iter()
                .map(|oid| git.find_commit(*oid).expect("find parent"))
                .collect::<Vec<_>>();
            let parent_refs = parent_commits.iter().collect::<Vec<_>>();
            let signature = git2::Signature::now("psn@tenant.noreply", "psn@tenant.noreply")
                .expect("signature");
            git.commit(None, &signature, &signature, message, &tree, &parent_refs)
                .expect("write commit")
        }

        fn set_main(&self, oid: git2::Oid) {
            self.git()
                .reference("refs/heads/main", oid, true, "test main")
                .expect("set main");
        }

        fn flat_files(&self, count: usize) -> (git2::Oid, git2::Oid) {
            let git = self.git();
            let blob = git.blob(b"page\n").expect("blob");
            let mut builder = git.treebuilder(None).expect("tree builder");
            for index in 0..count {
                builder
                    .insert(format!("file-{index:04}.txt"), blob, 0o100644)
                    .expect("tree entry");
            }
            let tree = builder.write().expect("tree");
            let commit = self.commit_tree(tree, &[], "initial page");
            self.set_main(commit);
            (tree, commit)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).ok();
        }
    }

    fn request(limit: usize) -> TreePageRequest {
        TreePageRequest {
            limit,
            ..TreePageRequest::default()
        }
    }

    fn dir_page(lookup: TreePageLookup) -> TreePage {
        let TreePageLookup::Dir(page) = lookup else {
            panic!("expected a directory page")
        };
        page
    }

    #[test]
    fn one_thousand_and_one_entries_page_without_duplicates_or_skips() {
        let fixture = Fixture::new("large");
        let (tree, commit) = fixture.flat_files(1_001);
        let mut names = Vec::new();
        let mut cursor = None;
        loop {
            let page = dir_page(
                fixture
                    .repo
                    .tree_page(
                        "main",
                        "",
                        TreePageRequest {
                            limit: 73,
                            cursor,
                            ..TreePageRequest::default()
                        },
                    )
                    .expect("page"),
            );
            assert_eq!(page.snapshot_oid.as_str(), commit.to_string());
            assert_eq!(page.tree_oid.as_str(), tree.to_string());
            names.extend(page.entries.into_iter().map(|entry| entry.name));
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(names.len(), 1_001);
        assert_eq!(
            names,
            (0..1_001)
                .map(|index| format!("file-{index:04}.txt"))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn directories_precede_files_in_raw_utf8_name_order() {
        let fixture = Fixture::new("order");
        let git = fixture.git();
        let blob = git.blob(b"x\n").expect("blob");
        let empty_dir = git.treebuilder(None).unwrap().write().expect("empty tree");
        let mut builder = git.treebuilder(None).expect("tree builder");
        for name in ["z-dir", "å-dir"] {
            builder.insert(name, empty_dir, 0o040000).expect("dir");
        }
        for name in ["A.txt", "z.txt", "å.txt"] {
            builder.insert(name, blob, 0o100644).expect("file");
        }
        let tree = builder.write().expect("tree");
        drop(builder);
        drop(git);
        let commit = fixture.commit_tree(tree, &[], "ordered");
        fixture.set_main(commit);

        let page = dir_page(
            fixture
                .repo
                .tree_page("main", "", request(100))
                .expect("ordered page"),
        );
        assert_eq!(
            page.entries
                .iter()
                .map(|entry| (entry.is_dir, entry.name.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (true, "z-dir"),
                (true, "å-dir"),
                (false, "A.txt"),
                (false, "z.txt"),
                (false, "å.txt"),
            ]
        );
    }

    #[test]
    fn exact_limits_and_normalized_query_are_enforced() {
        let fixture = Fixture::new("limits-query");
        fixture.flat_files(101);
        assert!(matches!(
            fixture.repo.tree_page("main", "", request(0)),
            Err(TreePageError::InvalidLimit { supplied: 0 })
        ));
        assert!(matches!(
            fixture.repo.tree_page("main", "", request(101)),
            Err(TreePageError::InvalidLimit { supplied: 101 })
        ));
        let maximum = dir_page(
            fixture
                .repo
                .tree_page("main", "", request(TREE_PAGE_MAX_LIMIT))
                .expect("maximum page"),
        );
        assert_eq!(maximum.entries.len(), TREE_PAGE_MAX_LIMIT);
        assert!(maximum.next_cursor.is_some());

        let git = fixture.git();
        let blob = git.blob(b"query\n").expect("blob");
        let mut builder = git.treebuilder(None).expect("tree builder");
        for name in ["Feature-One", "feature-two", "other"] {
            builder.insert(name, blob, 0o100644).expect("file");
        }
        let tree = builder.write().expect("tree");
        drop(builder);
        drop(git);
        let commit = fixture.commit_tree(tree, &[], "query");
        fixture.set_main(commit);
        let first = dir_page(
            fixture
                .repo
                .tree_page(
                    "main",
                    "",
                    TreePageRequest {
                        limit: 1,
                        query: Some("  FeAtUrE  ".into()),
                        cursor: None,
                    },
                )
                .expect("query first"),
        );
        assert_eq!(first.entries[0].name, "Feature-One");
        let second = dir_page(
            fixture
                .repo
                .tree_page(
                    "main",
                    "",
                    TreePageRequest {
                        limit: 1,
                        query: Some("feature".into()),
                        cursor: first.next_cursor,
                    },
                )
                .expect("normalized query continuation"),
        );
        assert_eq!(second.entries[0].name, "feature-two");
        assert!(second.next_cursor.is_none());
        assert!(matches!(
            fixture.repo.tree_page(
                "main",
                "",
                TreePageRequest {
                    query: Some("x".repeat(TREE_PAGE_MAX_QUERY_BYTES + 1)),
                    ..TreePageRequest::default()
                }
            ),
            Err(TreePageError::QueryTooLong { .. })
        ));
        assert!(matches!(
            fixture.repo.tree_page(
                "main",
                "",
                TreePageRequest {
                    query: Some("bad\nquery".into()),
                    ..TreePageRequest::default()
                }
            ),
            Err(TreePageError::InvalidQuery)
        ));
    }

    fn first_cursor(fixture: &Fixture, query: Option<&str>) -> String {
        dir_page(
            fixture
                .repo
                .tree_page(
                    "main",
                    "",
                    TreePageRequest {
                        limit: 1,
                        query: query.map(str::to_string),
                        cursor: None,
                    },
                )
                .expect("first page"),
        )
        .next_cursor
        .expect("continuation")
    }

    #[test]
    fn malformed_wrong_scope_stale_and_forged_cursors_are_typed() {
        let fixture = Fixture::new("cursor");
        let (tree, first_commit) = fixture.flat_files(3);
        let cursor = first_cursor(&fixture, None);
        for malformed in [
            "not-a-cursor".to_string(),
            format!("{cursor}="),
            "gt1_AA".to_string(),
            format!(
                "gt1_{}",
                "A".repeat(encoded_len(CURSOR_MAX_DECODED_BYTES) + 1)
            ),
        ] {
            assert!(matches!(
                fixture.repo.tree_page(
                    "main",
                    "",
                    TreePageRequest {
                        limit: 1,
                        cursor: Some(malformed),
                        ..TreePageRequest::default()
                    }
                ),
                Err(TreePageError::MalformedCursor)
            ));
        }
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let encoded = cursor.strip_prefix(CURSOR_PREFIX).expect("cursor prefix");
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .expect("cursor frame");
        let discarded_mask = match decoded.len() % 3 {
            1 => 0x0f,
            2 => 0x03,
            _ => panic!("fixture cursor must have discarded base64 bits"),
        };
        let mut noncanonical = cursor.clone();
        let final_char = noncanonical.pop().expect("cursor char");
        let index = alphabet
            .iter()
            .position(|byte| *byte == final_char as u8)
            .expect("base64url char");
        noncanonical
            .push(alphabet[(index & !discarded_mask) | ((index + 1) & discarded_mask)] as char);
        assert!(matches!(
            fixture.repo.tree_page(
                "main",
                "",
                TreePageRequest {
                    limit: 1,
                    cursor: Some(noncanonical),
                    ..TreePageRequest::default()
                }
            ),
            Err(TreePageError::MalformedCursor)
        ));
        let mut wrong_version_frame = decoded;
        wrong_version_frame[0] = CURSOR_VERSION + 1;
        let wrong_version = format!(
            "{CURSOR_PREFIX}{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(wrong_version_frame)
        );
        assert!(matches!(
            fixture.repo.tree_page(
                "main",
                "",
                TreePageRequest {
                    limit: 1,
                    cursor: Some(wrong_version),
                    ..TreePageRequest::default()
                }
            ),
            Err(TreePageError::MalformedCursor)
        ));
        assert!(matches!(
            fixture.repo.tree_page(
                "main",
                "",
                TreePageRequest {
                    limit: 1,
                    query: Some("other-scope".into()),
                    cursor: Some(cursor.clone()),
                }
            ),
            Err(TreePageError::CursorScopeMismatch)
        ));

        let second_commit = fixture.commit_tree(tree, &[first_commit], "move branch");
        fixture.set_main(second_commit);
        assert!(matches!(
            fixture.repo.tree_page(
                "main",
                "",
                TreePageRequest {
                    limit: 1,
                    cursor: Some(cursor.clone()),
                    ..TreePageRequest::default()
                }
            ),
            Err(TreePageError::CursorStale)
        ));
        fixture.set_main(first_commit);

        let git = fixture.git();
        let changed_blob = git.blob(b"changed path tree\n").expect("changed blob");
        let mut changed_builder = git.treebuilder(None).expect("tree builder");
        changed_builder
            .insert("changed.txt", changed_blob, 0o100644)
            .expect("changed entry");
        let changed_tree = changed_builder.write().expect("changed tree");
        drop(changed_builder);
        drop(git);
        let changed_commit = fixture.commit_tree(changed_tree, &[first_commit], "change path tree");
        fixture.set_main(changed_commit);
        assert!(matches!(
            fixture.repo.tree_page(
                "main",
                "",
                TreePageRequest {
                    limit: 1,
                    cursor: Some(cursor.clone()),
                    ..TreePageRequest::default()
                }
            ),
            Err(TreePageError::CursorStale)
        ));
        fixture.set_main(first_commit);

        // A cursor naming present but unreachable objects cannot select them: normal main/path
        // resolution happens first, then the forged consistency ids fail stale.
        let git = fixture.git();
        let secret_blob = git.blob(b"unreachable secret\n").expect("secret blob");
        let mut builder = git.treebuilder(None).expect("tree builder");
        builder
            .insert("secret.txt", secret_blob, 0o100644)
            .expect("secret entry");
        let secret_tree = builder.write().expect("secret tree");
        drop(builder);
        drop(git);
        let secret_commit = fixture.commit_tree(secret_tree, &[], "unreachable");
        let mut forged = Cursor::parse(&cursor).expect("cursor frame");
        forged.tree_oid = oid_frame(secret_tree);
        assert!(matches!(
            fixture.repo.tree_page(
                "main",
                "",
                TreePageRequest {
                    limit: 1,
                    cursor: Some(forged.encode()),
                    ..TreePageRequest::default()
                }
            ),
            Err(TreePageError::CursorStale)
        ));
        forged.snapshot_oid = oid_frame(secret_commit);
        assert!(matches!(
            fixture.repo.tree_page(
                "main",
                "",
                TreePageRequest {
                    limit: 1,
                    cursor: Some(forged.encode()),
                    ..TreePageRequest::default()
                }
            ),
            Err(TreePageError::CursorStale)
        ));
        let mut missing_key = Cursor::parse(&cursor).expect("cursor frame");
        missing_key.name = "not-an-entry".into();
        assert!(matches!(
            fixture.repo.tree_page(
                "main",
                "",
                TreePageRequest {
                    limit: 1,
                    cursor: Some(missing_key.encode()),
                    ..TreePageRequest::default()
                }
            ),
            Err(TreePageError::MalformedCursor)
        ));
    }

    #[test]
    fn safe_path_file_missing_and_readme_row_outcomes_match_browse_semantics() {
        let fixture = Fixture::new("paths");
        let git = fixture.git();
        let readme_bytes = b"# content must not be returned by tree_page\n";
        let readme = git.blob(readme_bytes).unwrap();
        let nested_file = git.blob(b"nested\n").unwrap();
        let mut nested_builder = git.treebuilder(None).unwrap();
        nested_builder
            .insert("child.txt", nested_file, 0o100644)
            .unwrap();
        let nested_tree = nested_builder.write().unwrap();
        let mut root_builder = git.treebuilder(None).unwrap();
        root_builder.insert("README.md", readme, 0o100644).unwrap();
        root_builder.insert("dir", nested_tree, 0o040000).unwrap();
        let root_tree = root_builder.write().unwrap();
        drop(root_builder);
        drop(nested_builder);
        drop(git);
        let commit = fixture.commit_tree(root_tree, &[], "paths");
        fixture.set_main(commit);

        let root = dir_page(
            fixture
                .repo
                .tree_page("main", "", request(100))
                .expect("root"),
        );
        let readme_row = root
            .entries
            .iter()
            .find(|entry| entry.name == "README.md")
            .expect("README is an ordinary row");
        assert!(!readme_row.is_dir);
        assert_eq!(readme_row.size, Some(readme_bytes.len() as u64));
        assert!(matches!(
            fixture.repo.tree_page("main", "README.md", request(100)),
            Ok(TreePageLookup::IsFile)
        ));
        assert!(matches!(
            fixture.repo.tree_page("main", "dir", request(100)),
            Ok(TreePageLookup::Dir(_))
        ));
        let nested = dir_page(
            fixture
                .repo
                .tree_page("main", "dir", request(100))
                .expect("nested directory"),
        );
        assert_eq!(nested.entries.len(), 1);
        assert_eq!(nested.entries[0].name, "child.txt");
        assert_eq!(nested.entries[0].size, Some(b"nested\n".len() as u64));
        for missing in [
            "missing",
            "../dir",
            "dir/../../secret",
            "../../../etc/passwd",
            "crates/../../etc/passwd",
            "crates/inner/../../../../secret",
            "/dir",
            "/etc/passwd",
        ] {
            assert!(matches!(
                fixture.repo.tree_page("main", missing, request(100)),
                Ok(TreePageLookup::Missing)
            ));
        }
        assert!(matches!(
            fixture.repo.tree_page("missing-ref", "", request(100)),
            Ok(TreePageLookup::Missing)
        ));
        assert!(matches!(
            fixture.repo.tree_page("main^{tree}", "", request(100)),
            Ok(TreePageLookup::Missing)
        ));
    }

    #[test]
    fn file_header_reads_happen_only_after_page_truncation() {
        let rows = (0..101)
            .map(|index| ScannedEntry {
                kind: if index < 2 {
                    EntryKind::Dir
                } else {
                    EntryKind::File
                },
                name: format!("row-{index:03}"),
                oid: git2::Oid::ZERO_SHA1,
            })
            .collect::<Vec<_>>();
        let mut header_reads = 0_usize;
        let finished = finish_page(rows, 100, |_| {
            header_reads += 1;
            Ok(Some(7))
        })
        .expect("finish page");
        assert_eq!(finished.entries.len(), 100);
        assert!(finished.next_key.is_some());
        assert_eq!(
            header_reads,
            finished
                .entries
                .iter()
                .filter(|entry| !entry.is_dir)
                .count(),
            "the lookahead and discarded rows perform no ODB header work"
        );
        assert_eq!(header_reads, 98);
    }

    #[test]
    fn scan_count_name_and_tree_header_ceilings_fail_closed() {
        let fixture = Fixture::new("scan-bounds");
        let (tree_oid, _) = fixture.flat_files(3);
        let git = fixture.git();
        let tree =
            find_tree_bounded(&git, tree_oid, TREE_OBJECT_MAX_BYTES).expect("bounded tree");
        assert!(
            scan_tree(
                &tree,
                None,
                None,
                2,
                ScanLimits {
                    entries: 2,
                    ..WIRE_SCAN_LIMITS
                }
            )
            .is_err()
        );
        assert!(
            scan_tree(
                &tree,
                None,
                None,
                2,
                ScanLimits {
                    one_name_bytes: 4,
                    ..WIRE_SCAN_LIMITS
                }
            )
            .is_err()
        );
        let total: usize = tree.iter().map(|entry| entry.name_bytes().len()).sum();
        assert!(
            scan_tree(
                &tree,
                None,
                None,
                2,
                ScanLimits {
                    total_name_bytes: total,
                    ..WIRE_SCAN_LIMITS
                }
            )
            .is_ok()
        );
        assert!(
            scan_tree(
                &tree,
                None,
                None,
                2,
                ScanLimits {
                    total_name_bytes: total - 1,
                    ..WIRE_SCAN_LIMITS
                }
            )
            .is_err()
        );
        assert!(find_tree_bounded(&git, tree_oid, 1).is_err());
    }

    #[test]
    fn latest_commit_metadata_uses_the_page_snapshot_after_branch_moves() {
        let fixture = Fixture::new("snapshot-meta");
        let git = fixture.git();
        let first_blob = git.blob(b"first\n").unwrap();
        let mut first_builder = git.treebuilder(None).unwrap();
        first_builder
            .insert("file.txt", first_blob, 0o100644)
            .unwrap();
        let first_tree = first_builder.write().unwrap();
        drop(first_builder);
        drop(git);
        let first_commit = fixture.commit_tree(first_tree, &[], "first snapshot");
        fixture.set_main(first_commit);
        let page = dir_page(
            fixture
                .repo
                .tree_page("main", "", request(100))
                .expect("snapshot page"),
        );

        let git = fixture.git();
        let second_blob = git.blob(b"second\n").unwrap();
        let mut second_builder = git.treebuilder(None).unwrap();
        second_builder
            .insert("file.txt", second_blob, 0o100644)
            .unwrap();
        let second_tree = second_builder.write().unwrap();
        drop(second_builder);
        drop(git);
        let second_commit = fixture.commit_tree(second_tree, &[first_commit], "second snapshot");
        fixture.set_main(second_commit);

        let immutable = fixture
            .repo
            .latest_commits_for_entries_at_snapshot(
                &page.snapshot_oid,
                "",
                &page.entries,
                TREE_PAGE_LATEST_COMMIT_WALK_MAX,
            )
            .expect("immutable metadata");
        assert_eq!(immutable["file.txt"].summary, "first snapshot");
        assert_eq!(
            fixture
                .repo
                .commit_meta_at_oid(&page.snapshot_oid)
                .expect("snapshot metadata")
                .expect("snapshot commit")
                .summary,
            "first snapshot"
        );
        assert!(fixture
            .repo
            .commit_meta_at_oid(&Oid::new("malformed"))
            .expect("malformed oid is a miss")
            .is_none());
        assert!(fixture
            .repo
            .commit_meta_at_oid(&Oid::new("0".repeat(40)))
            .expect("absent oid is a miss")
            .is_none());
        assert!(fixture
            .repo
            .commit_meta_at_oid(&Oid::new(first_tree.to_string()))
            .expect("tree oid is not a commit")
            .is_none());
        let too_many = vec![
            TreeEntryInfo {
                name: "x".into(),
                is_dir: false,
                size: Some(1),
            };
            TREE_PAGE_MAX_LIMIT + 1
        ];
        assert!(
            fixture
                .repo
                .latest_commits_for_entries_at_snapshot(
                    &page.snapshot_oid,
                    "",
                    &too_many,
                    TREE_PAGE_LATEST_COMMIT_WALK_MAX,
                )
                .is_err()
        );
        assert!(
            fixture
                .repo
                .latest_commits_for_entries_at_snapshot(
                    &page.snapshot_oid,
                    "",
                    &page.entries,
                    TREE_PAGE_LATEST_COMMIT_WALK_MAX + 1,
                )
                .is_err()
        );
        assert!(
            fixture
                .repo
                .latest_commits_for_entries_at_snapshot(
                    &page.snapshot_oid,
                    "/unsafe",
                    &page.entries,
                    TREE_PAGE_LATEST_COMMIT_WALK_MAX,
                )
                .is_err()
        );
    }

    #[test]
    fn snapshot_metadata_ignores_deleted_historical_siblings() {
        let fixture = Fixture::new("historical-siblings");
        let git = fixture.git();
        let blob = git.blob(b"same\n").expect("blob");
        let mut wide_builder = git.treebuilder(None).expect("wide tree builder");
        wide_builder
            .insert("keep.txt", blob, 0o100644)
            .expect("kept entry");
        for index in 0..=1_000 {
            wide_builder
                .insert(format!("removed-{index}.txt"), blob, 0o100644)
                .expect("historical sibling");
        }
        let wide_tree = wide_builder.write().expect("wide tree");
        drop(wide_builder);
        let mut narrow_builder = git.treebuilder(None).expect("narrow tree builder");
        narrow_builder
            .insert("keep.txt", blob, 0o100644)
            .expect("kept entry");
        let narrow_tree = narrow_builder.write().expect("narrow tree");
        drop(narrow_builder);
        drop(git);

        let parent = fixture.commit_tree(wide_tree, &[], "wide parent");
        let head = fixture.commit_tree(narrow_tree, &[parent], "delete historical siblings");
        fixture.set_main(head);
        let page = dir_page(
            fixture
                .repo
                .tree_page("main", "", request(100))
                .expect("narrow current page"),
        );
        assert_eq!(
            page.entries.len(),
            1,
            "the current directory is safely narrow"
        );

        let metadata = fixture
            .repo
            .latest_commits_for_entries_at_snapshot(
                &page.snapshot_oid,
                "",
                &page.entries,
                TREE_PAGE_LATEST_COMMIT_WALK_MAX,
            )
            .expect("snapshot metadata");
        assert_eq!(
            metadata.len(),
            1,
            "deleted siblings never enter the result map"
        );
        assert!(metadata.contains_key("keep.txt"));
    }
}
