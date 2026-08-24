use std::collections::HashSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::core::{Oid, RepoLoc};
use crate::gix_backend::{RepoPathResolver, RootedResolver};
use crate::receive_pack::RefName;
pub use crate::refs_pagination::{
    CatalogueRepoState, RefKind, RefPageItem, RefsPage, RefsPageError, RefsPageRequest,
    RefsSummary, REFS_PAGE_DEFAULT_LIMIT, REFS_PAGE_MAX_LIMIT, REFS_PAGE_MAX_QUERY_BYTES,
};
pub use crate::tree_pagination::{
    TreePage, TreePageError, TreePageLookup, TreePageRequest, TREE_PAGE_DEFAULT_LIMIT,
    TREE_PAGE_LATEST_COMMIT_WALK_MAX, TREE_PAGE_MAX_LIMIT, TREE_PAGE_MAX_QUERY_BYTES,
    TREE_PAGE_SCAN_MAX_ENTRIES, TREE_PAGE_SCAN_MAX_NAME_BYTES, TREE_PAGE_SCAN_MAX_TOTAL_NAME_BYTES,
};
use myelin_events::{IdMinter, UlidMinter};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DurableError {
    Git(String),
    Io(String),
    InvalidInput(String),
    Conflict(String),
    CasMismatch {
        ref_name: String,
        expected: Option<String>,
        actual: Option<String>,
    },
    NotFound(String),
    Forbidden(String),
}

impl std::fmt::Display for DurableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DurableError::Git(m) => write!(f, "durable git op failed: {m}"),
            DurableError::Io(m) => write!(f, "durable git io failed: {m}"),
            DurableError::InvalidInput(m) => write!(f, "durable git input refused: {m}"),
            DurableError::Conflict(m) => write!(f, "durable git conflict: {m}"),
            DurableError::CasMismatch {
                ref_name,
                expected,
                actual,
            } => write!(
                f,
                "ref CAS rejected on {ref_name}: expected {expected:?} but the on-disk tip is \
                 {actual:?} - the ref did NOT move (non-fast-forward / lost-update)"
            ),
            DurableError::NotFound(m) => write!(f, "durable git not found: {m}"),
            DurableError::Forbidden(m) => write!(f, "durable git forbidden: {m}"),
        }
    }
}

impl std::error::Error for DurableError {}

fn git_err(ctx: &str, e: git2::Error) -> DurableError {
    DurableError::Git(format!("{ctx}: {e}"))
}

static ATOMIC_WRITE_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(crate) fn write_file_atomic(dir: &Path, file: &Path, bytes: &[u8]) -> Result<(), DurableError> {
    write_file_atomic_with(dir, file, |handle| {
        handle
            .write_all(bytes)
            .map_err(|e| DurableError::Io(format!("write {}: {e}", file.display())))
    })
}

pub(crate) fn write_file_atomic_with(
    dir: &Path,
    file: &Path,
    write: impl FnOnce(&mut std::fs::File) -> Result<(), DurableError>,
) -> Result<(), DurableError> {
    std::fs::create_dir_all(dir)
        .map_err(|e| DurableError::Io(format!("create dir {}: {e}", dir.display())))?;
    let sequence = ATOMIC_WRITE_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let name = file
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("record");
    let tmp = dir.join(format!(".{name}.{}.{}.tmp", std::process::id(), sequence));
    let result = (|| {
        let mut handle = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&tmp)
            .map_err(|e| DurableError::Io(format!("create {}: {e}", tmp.display())))?;
        write(&mut handle)?;
        handle
            .sync_all()
            .map_err(|e| DurableError::Io(format!("sync {}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, file).map_err(|e| {
            DurableError::Io(format!(
                "rename {} to {}: {e}",
                tmp.display(),
                file.display()
            ))
        })?;
        std::fs::File::open(dir)
            .and_then(|directory| directory.sync_all())
            .map_err(|e| DurableError::Io(format!("sync dir {}: {e}", dir.display())))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn refgen_key(ref_name: &str) -> String {
    use std::fmt::Write as _;
    let mut var = String::with_capacity(ref_name.len() * 2 + 1);
    var.push('r');
    for b in ref_name.as_bytes() {
        let _ = write!(var, "{b:02x}");
    }
    format!("myelin.refgen.{var}")
}

fn read_ref_generation(cfg: &git2::Config, ref_name: &str) -> Result<u64, DurableError> {
    match cfg.get_i64(&refgen_key(ref_name)) {
        Ok(value) => u64::try_from(value).map_err(|_| {
            DurableError::Git(format!("negative ref generation stored for {ref_name}"))
        }),
        Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(0),
        Err(e) => Err(git_err(&format!("read refgen for {ref_name}"), e)),
    }
}

pub(crate) fn next_ref_generation(current: u64) -> Option<u64> {
    current
        .checked_add(1)
        .filter(|next| i64::try_from(*next).is_ok())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableReflogEntry {
    pub old_oid: Option<Oid>,
    pub new_oid: Oid,
    pub committer: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReflogCommitMessage {
    pub oid: Oid,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitMeta {
    pub oid: String,
    pub summary: String,
    pub author_name: String,
    pub author_email: String,
    pub time: i64,
    pub parents: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrCommitSnapshot {
    pub base_oid: Option<String>,
    pub head_oid: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrCommitPageError {
    InvalidPagination,
    CapacityExceeded,
    SnapshotExpired,
    Durable(DurableError),
}

impl std::fmt::Display for PrCommitPageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPagination => f.write_str("pull-request commit pagination is invalid"),
            Self::CapacityExceeded => {
                f.write_str("pull-request commit snapshot exceeds the reachability limit")
            }
            Self::SnapshotExpired => f.write_str("pull-request commit snapshot expired"),
            Self::Durable(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for PrCommitPageError {}

impl From<DurableError> for PrCommitPageError {
    fn from(error: DurableError) -> Self {
        Self::Durable(error)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDelta {
    pub path: String,
    pub old_path: Option<String>,
    pub status: char,
    pub lines: Vec<(char, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitDetail {
    pub meta: CommitMeta,
    pub message: String,
    pub files: Vec<FileDelta>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Text,
    Binary,
    Lfs,
    Submodule,
}

impl FileKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FileKind::Text => "text",
            FileKind::Binary => "binary",
            FileKind::Lfs => "lfs",
            FileKind::Submodule => "submodule",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLineDelta {
    pub origin: char,
    pub content: String,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileLinesLookup {
    Found(Vec<DiffLineDelta>),
    Binary,
    TooLarge { size: u64, maximum: usize },
    Missing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffHunkDelta {
    pub header: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLineDelta>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrFileDelta {
    pub path: String,
    pub old_path: Option<String>,
    pub new_blob_oid: Option<String>,
    pub status: char,
    pub kind: FileKind,
    pub additions: u32,
    pub deletions: u32,
    pub size_bytes: Option<u64>,
    pub hunks: Vec<DiffHunkDelta>,
    pub deleted_body_available: bool,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrDiff {
    pub base_oid: String,
    pub head_oid: String,
    pub three_dot: bool,
    pub files: Vec<PrFileDelta>,
    pub total_files: usize,
    pub total_additions: u32,
    pub total_deletions: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeEntryInfo {
    pub name: String,
    pub is_dir: bool,
    pub size: Option<u64>,
}

pub enum BlobPathLookup {
    Found {
        bytes: Vec<u8>,
        oid: Oid,
        is_binary: bool,
        size: u64,
    },
    TooLarge {
        size: u64,
        maximum: usize,
        oid: Oid,
    },
    IsDir,
    Missing,
}

fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8000).any(|b| *b == 0)
}

pub const FILE_LINES_MAX_RANGE: usize = 1_000;
pub const FILE_LINES_MAX_BLOB_BYTES: usize = 512 * 1024;
pub const WIRE_MAX_REFS: usize = 100_000;
pub const REFLOG_MAX_ENTRIES_PER_REF: usize = 100_000;
pub const REFLOG_MAX_BYTES_PER_REF: usize = 32 * 1024 * 1024;
pub const REFLOG_MAX_TOTAL_ENTRIES: usize = 100_000;
pub const REFLOG_MAX_TOTAL_BYTES: usize = 32 * 1024 * 1024;
pub const TREE_OBJECT_MAX_BYTES: usize = 8 * 1024 * 1024;
pub const PR_DIFF_MAX_FILES: usize = 1_000;
pub const COMMIT_META_MAX_PARENTS: usize = 64;
pub const COMMIT_META_MAX_SUMMARY_BYTES: usize = 8 * 1024;
pub const COMMIT_META_MAX_IDENTITY_BYTES: usize = 1_024;
pub const COMMIT_META_BATCH_MAX: usize = 10_000;
pub const REF_LOOKUP_BATCH_MAX: usize = 10_000;
pub const COMMIT_LOG_MAX_OFFSET: usize = 100_000;
pub const COMMIT_LOG_MAX_PAGE: usize = 500;
pub const PR_COMMIT_MAX_POSITION: usize = 100_000;
pub const PR_COMMIT_MAX_GRAPH_NODES_PER_PIN: usize = 100_000;
pub const PR_COMMIT_MAX_GRAPH_EDGES_PER_PIN: usize = 1_000_000;
pub const PR_COMMIT_MAX_PAGE_WALK_OBSERVATIONS: usize =
    PR_COMMIT_MAX_POSITION + COMMIT_LOG_MAX_PAGE + 1;
pub const PR_COMMIT_MAX_INTERNAL_WALK_NODES: usize = 2 * PR_COMMIT_MAX_GRAPH_NODES_PER_PIN;
pub const PR_COMMIT_MAX_INTERNAL_WALK_EDGES: usize = 2 * PR_COMMIT_MAX_GRAPH_EDGES_PER_PIN;
pub const COMMIT_DIFF_MAX_FILES: usize = 1_000;
pub const COMMIT_DIFF_MAX_LINES_PER_FILE: usize = 4_000;
pub const DIFF_MAX_LINE_BYTES: usize = 64 * 1024;
pub const DIFF_MAX_RENDERED_BYTES: usize = 4 * 1024 * 1024;
pub const COMMIT_DIFF_MAX_MESSAGE_BYTES: usize = 512 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedFileCommit {
    pub commit: Oid,
    pub blob: Oid,
    pub trees: Vec<Oid>,
}

#[derive(Clone, Copy)]
struct CommitDiffLimits {
    files: usize,
    lines_per_file: usize,
    line_bytes: usize,
    rendered_bytes: usize,
    message_bytes: usize,
}

const COMMIT_DIFF_LIMITS: CommitDiffLimits = CommitDiffLimits {
    files: COMMIT_DIFF_MAX_FILES,
    lines_per_file: COMMIT_DIFF_MAX_LINES_PER_FILE,
    line_bytes: DIFF_MAX_LINE_BYTES,
    rendered_bytes: DIFF_MAX_RENDERED_BYTES,
    message_bytes: COMMIT_DIFF_MAX_MESSAGE_BYTES,
};

fn is_safe_tree_path(clean: &str) -> bool {
    if clean.is_empty() {
        return true;
    }
    clean
        .split('/')
        .all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

fn validate_file_edit_path(path: &str) -> Result<(), DurableError> {
    let clean = path.trim_matches('/');
    if clean.is_empty() || clean != path || !is_safe_tree_path(clean) {
        return Err(DurableError::InvalidInput(
            "file edit path is not safe".into(),
        ));
    }
    if clean
        .split('/')
        .any(|component| component.eq_ignore_ascii_case(".git"))
    {
        return Err(DurableError::InvalidInput(
            "file edit path contains a reserved Git administrative component".into(),
        ));
    }
    Ok(())
}

fn utf8_prefix(value: &str, maximum_bytes: usize) -> String {
    if value.len() <= maximum_bytes {
        return value.to_string();
    }
    let mut end = maximum_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

fn commit_meta(c: &git2::Commit<'_>) -> CommitMeta {
    let author = c.author();
    let message = c.message().unwrap_or("");
    CommitMeta {
        oid: c.id().to_string(),
        summary: utf8_prefix(
            message.lines().next().unwrap_or(""),
            COMMIT_META_MAX_SUMMARY_BYTES,
        ),
        author_name: utf8_prefix(author.name().unwrap_or(""), COMMIT_META_MAX_IDENTITY_BYTES),
        author_email: utf8_prefix(author.email().unwrap_or(""), COMMIT_META_MAX_IDENTITY_BYTES),
        time: c.time().seconds(),
        parents: c
            .parent_ids()
            .take(COMMIT_META_MAX_PARENTS)
            .map(|p| p.to_string())
            .collect(),
    }
}

fn pr_commit_walk_error(context: &str, error: git2::Error) -> PrCommitPageError {
    if error.code() == git2::ErrorCode::NotFound {
        PrCommitPageError::SnapshotExpired
    } else {
        PrCommitPageError::Durable(git_err(context, error))
    }
}

fn preflight_pr_commit_reachability(
    repo: &git2::Repository,
    start: git2::Oid,
    maximum_nodes: usize,
    maximum_edges: usize,
    label: &str,
) -> Result<(), PrCommitPageError> {
    if maximum_nodes == 0 {
        return Err(PrCommitPageError::CapacityExceeded);
    }
    let mut scheduled = HashSet::with_capacity(maximum_nodes.min(4_096));
    let mut frontier = Vec::new();
    scheduled.insert(start);
    frontier.push(start);
    let mut examined_edges = 0usize;
    while let Some(oid) = frontier.pop() {
        let commit = repo.find_commit(oid).map_err(|error| {
            pr_commit_walk_error(&format!("find pinned {label} preflight commit"), error)
        })?;
        for parent in commit.parent_ids() {
            if examined_edges >= maximum_edges {
                return Err(PrCommitPageError::CapacityExceeded);
            }
            examined_edges += 1;
            if !scheduled.contains(&parent) {
                if scheduled.len() >= maximum_nodes {
                    return Err(PrCommitPageError::CapacityExceeded);
                }
                scheduled.insert(parent);
                frontier.push(parent);
            }
        }
    }
    Ok(())
}

#[derive(Debug)]
pub struct DurableGitRepo {
    path: PathBuf,
}

impl DurableGitRepo {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn open_git(&self) -> Result<git2::Repository, DurableError> {
        git2::Repository::open(&self.path)
            .map_err(|e| git_err(&format!("open bare repo {}", self.path.display()), e))
    }

    pub(crate) fn lock_ref_exclusive(&self, ref_name: &str) -> Result<std::fs::File, DurableError> {
        let lock_dir = self.path.join("myelin-ref-locks");
        std::fs::create_dir_all(&lock_dir).map_err(|e| {
            DurableError::Io(format!(
                "create durable ref lock directory {}: {e}",
                lock_dir.display()
            ))
        })?;
        let lock_path = lock_dir.join(refgen_key(ref_name));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|e| {
                DurableError::Io(format!(
                    "open durable ref lock {}: {e}",
                    lock_path.display()
                ))
            })?;
        fs4::fs_std::FileExt::lock_exclusive(&file).map_err(|e| {
            DurableError::Io(format!(
                "acquire durable ref lock {}: {e}",
                lock_path.display()
            ))
        })?;
        Ok(file)
    }

    pub fn init_quarantine(
        dir: &Path,
        alternate_objects: &Path,
    ) -> Result<DurableGitRepo, DurableError> {
        git2::Repository::init_bare(dir).map_err(|e| git_err("init quarantine repo", e))?;
        let info = dir.join("objects").join("info");
        std::fs::create_dir_all(&info)
            .map_err(|e| DurableError::Io(format!("mkdir {}: {e}", info.display())))?;
        std::fs::write(
            info.join("alternates"),
            format!("{}\n", alternate_objects.display()),
        )
        .map_err(|e| DurableError::Io(format!("write quarantine alternates: {e}")))?;
        Ok(DurableGitRepo {
            path: dir.to_path_buf(),
        })
    }

    fn parse_oid(oid: &Oid) -> Result<git2::Oid, DurableError> {
        git2::Oid::from_str(oid.as_str())
            .map_err(|e| DurableError::Git(format!("bad oid {}: {e}", oid.as_str())))
    }

    pub fn write_blob(&self, bytes: &[u8]) -> Result<Oid, DurableError> {
        let repo = self.open_git()?;
        let oid = repo.blob(bytes).map_err(|e| git_err("write blob", e))?;
        Ok(Oid::new(oid.to_string()))
    }

    pub fn import_commit_closure_from(
        &self,
        source: &DurableGitRepo,
        locked_head: &Oid,
    ) -> Result<(), DurableError> {
        let source_git = source.open_git()?;
        let head = Self::parse_oid(locked_head)?;
        source_git
            .find_commit(head)
            .map_err(|e| git_err("locked fork head is not a source commit", e))?;

        let mut pack = source_git
            .packbuilder()
            .map_err(|e| git_err("create fork import pack", e))?;
        let mut walk = source_git
            .revwalk()
            .map_err(|e| git_err("create locked fork ancestry walk", e))?;
        walk.push(head)
            .map_err(|e| git_err("start locked fork ancestry walk", e))?;
        pack.insert_walk(&mut walk)
            .map_err(|e| git_err("pack locked fork commit closure", e))?;
        let mut bytes = git2::Buf::new();
        pack.write_buf(&mut bytes)
            .map_err(|e| git_err("write fork import pack", e))?;

        let pack_dir = self.path.join("objects").join("pack");
        std::fs::create_dir_all(&pack_dir)
            .map_err(|e| DurableError::Io(format!("mkdir {}: {e}", pack_dir.display())))?;
        let mut indexer = git2::Indexer::new(None, &pack_dir, 0, true)
            .map_err(|e| git_err("create verified non-thin fork pack indexer", e))?;
        indexer
            .write_all(bytes.as_ref())
            .map_err(|e| DurableError::Io(format!("install fork import pack: {e}")))?;
        indexer
            .commit()
            .map_err(|e| git_err("verify and commit fork import pack", e))?;

        self.open_git()?
            .find_commit(head)
            .map_err(|e| git_err("verify imported fork head in target ODB", e))?;
        Ok(())
    }

    pub fn write_tree(&self, entries: &[(&str, &Oid)]) -> Result<Oid, DurableError> {
        let repo = self.open_git()?;
        let mut builder = repo
            .treebuilder(None)
            .map_err(|e| git_err("treebuilder", e))?;
        for (name, blob) in entries {
            builder
                .insert(name, Self::parse_oid(blob)?, 0o100644)
                .map_err(|e| git_err(&format!("tree insert {name}"), e))?;
        }
        let oid = builder.write().map_err(|e| git_err("write tree", e))?;
        Ok(Oid::new(oid.to_string()))
    }

    pub fn write_commit(
        &self,
        tree: &Oid,
        parents: &[&Oid],
        message: &str,
        author_name: &str,
        author_email: &str,
    ) -> Result<Oid, DurableError> {
        let repo = self.open_git()?;
        let tree_oid = Self::parse_oid(tree)?;
        let tree_obj = repo
            .find_tree(tree_oid)
            .map_err(|e| git_err("find tree", e))?;
        let sig =
            git2::Signature::now(author_name, author_email).map_err(|e| git_err("signature", e))?;
        let parent_commits: Vec<git2::Commit<'_>> = parents
            .iter()
            .map(|p| {
                let oid = Self::parse_oid(p)?;
                repo.find_commit(oid).map_err(|e| git_err("find parent", e))
            })
            .collect::<Result<_, _>>()?;
        let parent_refs: Vec<&git2::Commit<'_>> = parent_commits.iter().collect();
        let oid = repo
            .commit(None, &sig, &sig, message, &tree_obj, &parent_refs)
            .map_err(|e| git_err("write commit", e))?;
        Ok(Oid::new(oid.to_string()))
    }

    pub fn read_object_bounded(
        &self,
        oid: &Oid,
        maximum_bytes: usize,
    ) -> Result<Vec<u8>, DurableError> {
        let repo = self.open_git()?;
        let odb = repo.odb().map_err(|e| git_err("odb", e))?;
        let git_oid = Self::parse_oid(oid)?;
        let (size, _) = odb
            .read_header(git_oid)
            .map_err(|e| DurableError::NotFound(format!("object {}: {e}", oid.as_str())))?;
        if size > maximum_bytes {
            return Err(DurableError::Git(format!(
                "object read limit exceeded: {size} bytes exceeds {maximum_bytes}"
            )));
        }
        let obj = odb
            .read(git_oid)
            .map_err(|e| DurableError::NotFound(format!("object {}: {e}", oid.as_str())))?;
        Ok(obj.data().to_vec())
    }

    pub fn has_object(&self, oid: &Oid) -> bool {
        let Ok(repo) = self.open_git() else {
            return false;
        };
        let Ok(odb) = repo.odb() else { return false };
        let Ok(goid) = git2::Oid::from_str(oid.as_str()) else {
            return false;
        };
        odb.exists(goid)
    }

    pub fn read_ref(&self, name: &str) -> Result<Option<Oid>, DurableError> {
        let repo = self.open_git()?;
        let reference = repo.find_reference(name);
        match reference {
            Ok(r) => {
                let oid = r
                    .target()
                    .ok_or_else(|| DurableError::Git(format!("ref {name} is symbolic")))?;
                Ok(Some(Oid::new(oid.to_string())))
            }
            Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(e) => Err(git_err(&format!("find_reference {name}"), e)),
        }
    }

    /// Resolves an exact bounded set of direct refs while opening the repository once.
    /// Missing refs are absent; malformed or symbolic inputs fail the whole read closed.
    pub fn read_refs_at_names(
        &self,
        names: &[RefName],
    ) -> Result<Vec<(RefName, Oid)>, DurableError> {
        if names.len() > REF_LOOKUP_BATCH_MAX {
            return Err(DurableError::InvalidInput(format!(
                "at most {REF_LOOKUP_BATCH_MAX} refs may be read at once"
            )));
        }
        if names.is_empty() {
            return Ok(Vec::new());
        }
        let repo = self.open_git()?;
        let mut seen = HashSet::new();
        let mut refs = Vec::new();
        for name in names {
            name.validate()
                .map_err(|_| DurableError::InvalidInput("Git ref name is malformed".into()))?;
            if !seen.insert(name.clone()) {
                continue;
            }
            match repo.find_reference(&name.0) {
                Ok(reference) => {
                    let target = reference
                        .target()
                        .ok_or_else(|| DurableError::Git(format!("ref {} is symbolic", name.0)))?;
                    refs.push((name.clone(), Oid::new(target.to_string())));
                }
                Err(error) if error.code() == git2::ErrorCode::NotFound => {}
                Err(error) => {
                    return Err(git_err(&format!("find_reference {}", name.0), error));
                }
            }
        }
        Ok(refs)
    }

    pub fn list_refs_bounded(&self, maximum: usize) -> Result<Vec<(String, Oid)>, DurableError> {
        let repo = self.open_git()?;
        let refs = repo.references().map_err(|e| git_err("references", e))?;
        let mut out = Vec::new();
        for r in refs {
            let r = r.map_err(|e| git_err("reference iter", e))?;
            if let Some(oid) = r.target() {
                if out.len() >= maximum {
                    return Err(DurableError::Git(
                        "wire ref limit exceeded: direct ref count".into(),
                    ));
                }
                let name = r
                    .name()
                    .map_err(|_| DurableError::Git("reference name is not valid UTF-8".into()))?;
                out.push((name.to_string(), Oid::new(oid.to_string())));
            }
        }
        out.sort();
        Ok(out)
    }

    pub fn update_ref_cas(
        &self,
        name: &str,
        expected: Option<&Oid>,
        new: Option<&Oid>,
        reflog_msg: &str,
        committer_pseudonym: &str,
    ) -> Result<(), DurableError> {
        let repo = self.open_git()?;
        let actual = self.read_ref(name)?;
        let expected_norm = expected.cloned();
        if actual != expected_norm {
            return Err(DurableError::CasMismatch {
                ref_name: name.to_string(),
                expected: expected_norm.map(|o| o.0),
                actual: actual.map(|o| o.0),
            });
        }

        if !matches!((expected, new), (None, None)) {
            let cfg = repo
                .config()
                .map_err(|e| git_err("config (refgen preflight)", e))?;
            let current = read_ref_generation(&cfg, name)?;
            next_ref_generation(current)
                .ok_or_else(|| DurableError::Git(format!("ref generation exhausted for {name}")))?;
        }

        {
            let mut cfg = repo.config().map_err(|e| git_err("config", e))?;
            cfg.set_str("user.name", committer_pseudonym)
                .map_err(|e| git_err("set user.name", e))?;
            cfg.set_str("user.email", committer_pseudonym)
                .map_err(|e| git_err("set user.email", e))?;
        }

        match (expected, new) {
            (None, Some(new_oid)) => {
                repo.reference(name, Self::parse_oid(new_oid)?, false, reflog_msg)
                    .map_err(|e| git_err(&format!("create ref {name}"), e))?;
            }
            (Some(exp), Some(new_oid)) => {
                repo.reference_matching(
                    name,
                    Self::parse_oid(new_oid)?,
                    true,
                    Self::parse_oid(exp)?,
                    reflog_msg,
                )
                .map_err(|e| git_err(&format!("update ref {name}"), e))?;
            }
            (Some(exp), None) => {
                let mut r = repo
                    .find_reference(name)
                    .map_err(|e| git_err(&format!("find ref to delete {name}"), e))?;
                let cur = r.target().map(|o| Oid::new(o.to_string()));
                if cur.as_ref() != Some(exp) {
                    return Err(DurableError::CasMismatch {
                        ref_name: name.to_string(),
                        expected: Some(exp.0.clone()),
                        actual: cur.map(|o| o.0),
                    });
                }
                r.delete()
                    .map_err(|e| git_err(&format!("delete ref {name}"), e))?;
            }
            (None, None) => {}
        }

        if !matches!((expected, new), (None, None)) {
            self.bump_generation(&repo, name)?;
        }
        Ok(())
    }

    fn bump_generation(&self, repo: &git2::Repository, name: &str) -> Result<(), DurableError> {
        let key = refgen_key(name);
        let mut cfg = repo.config().map_err(|e| git_err("config (refgen)", e))?;
        let current = read_ref_generation(&cfg, name)?;
        let next = next_ref_generation(current)
            .ok_or_else(|| DurableError::Git(format!("ref generation exhausted for {name}")))?;
        let next = i64::try_from(next).map_err(|_| {
            DurableError::Git(format!("ref generation exceeds durable range for {name}"))
        })?;
        cfg.set_i64(&key, next)
            .map_err(|e| git_err(&format!("set refgen for {name}"), e))?;
        Ok(())
    }

    pub(crate) fn repair_ref_generation(
        &self,
        name: &str,
        expected_current: u64,
    ) -> Result<u64, DurableError> {
        let repo = self.open_git()?;
        let cfg = repo
            .config()
            .map_err(|e| git_err("config (refgen repair check)", e))?;
        let current = read_ref_generation(&cfg, name)?;
        if current != expected_current {
            return Err(DurableError::Git(format!(
                "ref generation changed during repair for {name}: expected {expected_current}, actual {current}"
            )));
        }
        drop(cfg);
        self.bump_generation(&repo, name)?;
        next_ref_generation(current)
            .ok_or_else(|| DurableError::Git(format!("ref generation exhausted for {name}")))
    }

    pub fn ref_generation(&self, name: &str) -> Result<u64, DurableError> {
        let repo = self.open_git()?;
        let cfg = repo.config().map_err(|e| git_err("config (refgen)", e))?;
        read_ref_generation(&cfg, name)
    }

    pub fn reflog_len(&self, name: &str) -> Result<usize, DurableError> {
        if !git2::Reference::is_valid_name(name) {
            return Err(DurableError::Git("invalid reflog ref name".into()));
        }
        let _lock = self.lock_ref_exclusive(name)?;
        let repo = self.open_git()?;
        let path = repo.path().join("logs").join(name);
        match std::fs::metadata(path) {
            Ok(metadata) if metadata.len() > REFLOG_MAX_BYTES_PER_REF as u64 => {
                return Err(DurableError::Git(
                    "audit reflog limit exceeded: on-disk bytes".into(),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(DurableError::Io(format!("stat reflog {name}: {error}"))),
        }
        match repo.reflog(name) {
            Ok(log) if log.len() <= REFLOG_MAX_ENTRIES_PER_REF => Ok(log.len()),
            Ok(_) => Err(DurableError::Git(
                "audit reflog limit exceeded: entry count".into(),
            )),
            Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(0),
            Err(e) => Err(git_err(&format!("reflog {name}"), e)),
        }
    }

    /// Finds the earliest commit named by a ref's durable reflog whose message contains an exact
    /// trailer line. Choosing the original match keeps later commits from shadowing an operation;
    /// the reflog also lets retries recognize it after the branch advances or is force-updated.
    pub fn find_reflog_commit_by_trailer(
        &self,
        ref_name: &str,
        trailer: &str,
    ) -> Result<Option<ReflogCommitMessage>, DurableError> {
        if trailer.is_empty()
            || trailer.len() > 512
            || trailer.contains('\n')
            || trailer.contains('\r')
        {
            return Err(DurableError::Git(
                "commit operation trailer is invalid".into(),
            ));
        }
        let (entries, _, _) = self.reflog_entries_bounded(
            ref_name,
            REFLOG_MAX_ENTRIES_PER_REF,
            REFLOG_MAX_BYTES_PER_REF,
        )?;
        let repo = self.open_git()?;
        for entry in entries {
            let oid = Self::parse_oid(&entry.new_oid)?;
            let commit = match repo.find_commit(oid) {
                Ok(commit) => commit,
                Err(error) if error.code() == git2::ErrorCode::NotFound => continue,
                Err(error) => return Err(git_err("find reflog operation commit", error)),
            };
            let message = commit.message().map_err(|_| {
                DurableError::Git("reflog operation commit message is not UTF-8".into())
            })?;
            if message.lines().any(|line| line == trailer) {
                return Ok(Some(ReflogCommitMessage {
                    oid: entry.new_oid,
                    message: message.to_string(),
                }));
            }
        }
        Ok(None)
    }

    pub(crate) fn reflog_entries_bounded(
        &self,
        name: &str,
        maximum_entries: usize,
        maximum_bytes: usize,
    ) -> Result<(Vec<DurableReflogEntry>, usize, u64), DurableError> {
        if !git2::Reference::is_valid_name(name) {
            return Err(DurableError::Git("invalid reflog ref name".into()));
        }
        let _lock = self.lock_ref_exclusive(name)?;
        let repo = self.open_git()?;
        let path = repo.path().join("logs").join(name);
        let on_disk_bytes = match std::fs::metadata(&path) {
            Ok(metadata) if metadata.len() > maximum_bytes as u64 => {
                return Err(DurableError::Git(
                    "audit reflog limit exceeded: on-disk bytes".into(),
                ));
            }
            Ok(metadata) => usize::try_from(metadata.len()).map_err(|_| {
                DurableError::Git("audit reflog limit exceeded: on-disk bytes".into())
            })?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let cfg = repo
                    .config()
                    .map_err(|e| git_err("config (reflog audit)", e))?;
                return Ok((Vec::new(), 0, read_ref_generation(&cfg, name)?));
            }
            Err(error) => {
                return Err(DurableError::Io(format!(
                    "stat {}: {error}",
                    path.display()
                )));
            }
        };
        let log = match repo.reflog(name) {
            Ok(log) => log,
            Err(e) if e.code() == git2::ErrorCode::NotFound => {
                let cfg = repo
                    .config()
                    .map_err(|error| git_err("config (reflog audit)", error))?;
                return Ok((Vec::new(), 0, read_ref_generation(&cfg, name)?));
            }
            Err(e) => return Err(git_err(&format!("reflog {name}"), e)),
        };
        if log.len() > maximum_entries {
            return Err(DurableError::Git(
                "audit reflog limit exceeded: entry count".into(),
            ));
        }
        let mut out = Vec::with_capacity(log.len());
        for entry in log.iter().rev() {
            let old = entry.id_old();
            let old_oid = if old.is_zero() {
                None
            } else {
                Some(Oid::new(old.to_string()))
            };
            let signature = entry.committer();
            let committer = signature.name().map_err(|_| {
                DurableError::Git(format!("reflog {name} committer is not valid UTF-8"))
            })?;
            let message = entry
                .message()
                .map_err(|e| git_err(&format!("read reflog message for {name}"), e))?
                .unwrap_or_default();
            out.push(DurableReflogEntry {
                old_oid,
                new_oid: Oid::new(entry.id_new().to_string()),
                committer: committer.to_string(),
                message: message.to_string(),
            });
        }
        let observed_bytes = std::fs::metadata(&path)
            .map_err(|error| DurableError::Io(format!("stat {}: {error}", path.display())))?
            .len();
        if observed_bytes != on_disk_bytes as u64 || observed_bytes > maximum_bytes as u64 {
            return Err(DurableError::Git(
                "audit reflog changed while it was read".into(),
            ));
        }
        let cfg = repo
            .config()
            .map_err(|e| git_err("config (reflog audit)", e))?;
        let generation = read_ref_generation(&cfg, name)?;
        Ok((out, on_disk_bytes, generation))
    }

    pub(crate) fn resolve_commit<'r>(
        &self,
        repo: &'r git2::Repository,
        revspec: &str,
    ) -> Result<Option<git2::Commit<'r>>, DurableError> {
        match repo.revparse_single(revspec) {
            Ok(obj) => match obj.peel_to_commit() {
                Ok(c) => Ok(Some(c)),
                Err(e)
                    if matches!(
                        e.code(),
                        git2::ErrorCode::NotFound | git2::ErrorCode::InvalidSpec
                    ) =>
                {
                    Ok(None)
                }
                Err(e) => Err(git_err("peel_to_commit", e)),
            },
            Err(e)
                if matches!(
                    e.code(),
                    git2::ErrorCode::NotFound | git2::ErrorCode::InvalidSpec
                ) =>
            {
                Ok(None)
            }
            Err(e) => Err(git_err("revparse_single", e)),
        }
    }

    pub fn resolve_commit_oid(&self, revspec: &str) -> Result<Option<Oid>, DurableError> {
        let repo = self.open_git()?;
        let commit = self.resolve_commit(&repo, revspec)?;
        Ok(commit.map(|commit| Oid::new(commit.id().to_string())))
    }

    fn tip_commit(
        &self,
        repo: &git2::Repository,
        ref_name: &str,
    ) -> Result<Option<git2::Oid>, DurableError> {
        match repo.find_reference(ref_name) {
            Ok(r) => Ok(r.target()),
            Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(e) => Err(git_err(&format!("find_reference {ref_name}"), e)),
        }
    }

    pub fn read_blob_at_commit_oid_bounded(
        &self,
        oid: &Oid,
        path: &str,
        maximum_bytes: usize,
    ) -> Result<BlobPathLookup, DurableError> {
        let repo = self.open_git()?;
        let oid = Self::parse_oid(oid)?;
        let oid_text = oid.to_string();
        let commit = match repo.find_commit(oid) {
            Ok(commit) => commit,
            Err(error) if error.code() == git2::ErrorCode::NotFound => {
                return Err(DurableError::Git(format!(
                    "exact commit {oid_text} not found"
                )))
            }
            Err(error) => return Err(git_err("find exact commit", error)),
        };
        Self::read_blob_from_commit(&repo, &commit, path, maximum_bytes)
    }

    pub fn read_blob_at_path_bounded(
        &self,
        ref_name: &str,
        path: &str,
        maximum_bytes: usize,
    ) -> Result<BlobPathLookup, DurableError> {
        let repo = self.open_git()?;
        let Some(commit) = self.resolve_commit(&repo, ref_name)? else {
            return Ok(BlobPathLookup::Missing);
        };
        Self::read_blob_from_commit(&repo, &commit, path, maximum_bytes)
    }

    pub fn blob_oid_at_path(
        &self,
        ref_name: &str,
        path: &str,
    ) -> Result<Option<Oid>, DurableError> {
        match self.read_blob_at_path_bounded(ref_name, path, 0)? {
            BlobPathLookup::Found { oid, .. } | BlobPathLookup::TooLarge { oid, .. } => {
                Ok(Some(oid))
            }
            BlobPathLookup::IsDir | BlobPathLookup::Missing => Ok(None),
        }
    }

    fn read_blob_from_commit(
        repo: &git2::Repository,
        commit: &git2::Commit<'_>,
        path: &str,
        maximum_bytes: usize,
    ) -> Result<BlobPathLookup, DurableError> {
        let root = commit.tree().map_err(|e| git_err("commit tree", e))?;
        let clean = path.trim_matches('/');
        if clean.is_empty() {
            return Ok(BlobPathLookup::IsDir);
        }
        if !is_safe_tree_path(clean) {
            return Ok(BlobPathLookup::Missing);
        }
        let entry = match root.get_path(std::path::Path::new(clean)) {
            Ok(e) => e,
            Err(e) if e.code() == git2::ErrorCode::NotFound => return Ok(BlobPathLookup::Missing),
            Err(e) => return Err(git_err("tree get_path", e)),
        };
        match entry.kind() {
            Some(git2::ObjectType::Blob) => {
                let odb = repo.odb().map_err(|e| git_err("open object database", e))?;
                let (object_size, object_kind) = odb
                    .read_header(entry.id())
                    .map_err(|e| git_err("read object header", e))?;
                if object_kind != git2::ObjectType::Blob {
                    return Ok(BlobPathLookup::Missing);
                }
                if object_size > maximum_bytes {
                    return Ok(BlobPathLookup::TooLarge {
                        size: object_size as u64,
                        maximum: maximum_bytes,
                        oid: Oid::new(entry.id().to_string()),
                    });
                }
                let obj = entry
                    .to_object(repo)
                    .map_err(|e| git_err("entry object", e))?;
                let blob = obj
                    .as_blob()
                    .ok_or_else(|| DurableError::Git("blob object not a blob".into()))?;
                let bytes = blob.content().to_vec();
                let is_binary = looks_binary(&bytes);
                let size = bytes.len() as u64;
                Ok(BlobPathLookup::Found {
                    bytes,
                    oid: Oid::new(entry.id().to_string()),
                    is_binary,
                    size,
                })
            }
            Some(git2::ObjectType::Tree) => Ok(BlobPathLookup::IsDir),
            _ => Ok(BlobPathLookup::Missing),
        }
    }

    pub fn heal_head_symref(&self) -> Result<(), DurableError> {
        let repo = self.open_git()?;
        if repo.head().is_ok() {
            return Ok(());
        }
        let branches: Vec<String> = self
            .list_refs_bounded(WIRE_MAX_REFS)?
            .into_iter()
            .filter_map(|(n, _)| n.strip_prefix("refs/heads/").map(str::to_string))
            .collect();
        let target = if branches.iter().any(|b| b == "main") {
            "main".to_string()
        } else if let Some(first) = branches.first() {
            first.clone()
        } else {
            return Ok(());
        };
        repo.set_head(&format!("refs/heads/{target}"))
            .map_err(|e| git_err("set HEAD symref (F9)", e))?;
        Ok(())
    }

    pub fn latest_commits_for_entries_at_snapshot(
        &self,
        snapshot_oid: &Oid,
        dir_path: &str,
        entries: &[TreeEntryInfo],
        cap: usize,
    ) -> Result<std::collections::BTreeMap<String, CommitMeta>, DurableError> {
        if entries.len() > TREE_PAGE_MAX_LIMIT {
            return Err(DurableError::Git(
                "tree page metadata limit exceeded: entry count".into(),
            ));
        }
        if cap > TREE_PAGE_LATEST_COMMIT_WALK_MAX {
            return Err(DurableError::Git(
                "tree page metadata limit exceeded: commit walk".into(),
            ));
        }
        let clean_path = dir_path.trim_matches('/');
        if dir_path.starts_with('/')
            || !is_safe_tree_path(clean_path)
            || entries.iter().any(|entry| {
                entry.name.is_empty()
                    || entry.name.len() > TREE_PAGE_SCAN_MAX_NAME_BYTES
                    || entry.name.contains(['\0', '/'])
            })
        {
            return Err(DurableError::Git(
                "tree page metadata contains an unsafe path".into(),
            ));
        }
        if entries.is_empty() || cap == 0 {
            return Ok(Default::default());
        }
        let repo = self.open_git()?;
        let tip = Self::parse_oid(snapshot_oid)?;
        repo.find_commit(tip)
            .map_err(|error| git_err("find snapshot commit", error))?;
        self.latest_commits_for_entries_from_tip(&repo, tip, clean_path, entries, cap)
    }

    pub fn commit_meta_at_oid(&self, oid: &Oid) -> Result<Option<CommitMeta>, DurableError> {
        let git_oid = match git2::Oid::from_str(oid.as_str()) {
            Ok(oid) => oid,
            Err(_) => return Ok(None),
        };
        let repo = self.open_git()?;
        let result = match repo.find_commit(git_oid) {
            Ok(commit) => Ok(Some(commit_meta(&commit))),
            Err(error)
                if matches!(
                    error.code(),
                    git2::ErrorCode::NotFound | git2::ErrorCode::InvalidSpec
                ) =>
            {
                Ok(None)
            }
            Err(error) => Err(git_err("find commit metadata", error)),
        };
        result
    }

    /// Reads an exact bounded metadata set while opening the repository once. Malformed and
    /// missing object ids are absent, matching the single-object lookup without turning a card
    /// viewport into one repository open per commit.
    pub fn commit_meta_at_oids(&self, oids: &[Oid]) -> Result<Vec<CommitMeta>, DurableError> {
        if oids.len() > COMMIT_META_BATCH_MAX {
            return Err(DurableError::InvalidInput(format!(
                "at most {COMMIT_META_BATCH_MAX} commit metadata rows may be read at once"
            )));
        }
        if oids.is_empty() {
            return Ok(Vec::new());
        }
        let repo = self.open_git()?;
        let mut seen = HashSet::new();
        let mut commits = Vec::new();
        for oid in oids {
            let Ok(oid) = git2::Oid::from_str(oid.as_str()) else {
                continue;
            };
            if !seen.insert(oid) {
                continue;
            }
            match repo.find_commit(oid) {
                Ok(commit) => commits.push(commit_meta(&commit)),
                Err(error)
                    if matches!(
                        error.code(),
                        git2::ErrorCode::NotFound | git2::ErrorCode::InvalidSpec
                    ) => {}
                Err(error) => return Err(git_err("find commit metadata batch", error)),
            }
        }
        Ok(commits)
    }

    fn latest_commits_for_entries_from_tip(
        &self,
        repo: &git2::Repository,
        tip: git2::Oid,
        dir_path: &str,
        entries: &[TreeEntryInfo],
        cap: usize,
    ) -> Result<std::collections::BTreeMap<String, CommitMeta>, DurableError> {
        let prefix = {
            let c = dir_path.trim_matches('/');
            if c.is_empty() {
                String::new()
            } else {
                format!("{c}/")
            }
        };
        let requested: std::collections::BTreeSet<&str> =
            entries.iter().map(|entry| entry.name.as_str()).collect();
        let mut walk = repo.revwalk().map_err(|e| git_err("revwalk", e))?;
        walk.set_sorting(git2::Sort::TIME)
            .map_err(|e| git_err("revwalk sort", e))?;
        walk.push(tip).map_err(|e| git_err("revwalk push", e))?;
        let mut out: std::collections::BTreeMap<String, CommitMeta> = Default::default();
        for (seen, oid_res) in walk.enumerate() {
            if seen >= cap {
                break;
            }
            let oid = oid_res.map_err(|e| git_err("revwalk next", e))?;
            let commit = repo
                .find_commit(oid)
                .map_err(|e| git_err("find_commit", e))?;
            let tree = commit.tree().map_err(|e| git_err("commit tree", e))?;
            let parent_tree = if commit.parent_count() > 0 {
                Some(
                    commit
                        .parent(0)
                        .map_err(|e| git_err("parent", e))?
                        .tree()
                        .map_err(|e| git_err("parent tree", e))?,
                )
            } else {
                None
            };
            let mut opts = git2::DiffOptions::new();
            opts.disable_pathspec_match(true);
            for entry in entries {
                let path = format!("{prefix}{}", entry.name);
                opts.pathspec(if entry.is_dir {
                    format!("{path}/")
                } else {
                    path
                });
            }
            let diff = repo
                .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut opts))
                .map_err(|e| git_err("diff_tree_to_tree", e))?;
            let meta = commit_meta(&commit);
            for delta in diff.deltas() {
                for file in [delta.new_file().path(), delta.old_file().path()]
                    .into_iter()
                    .flatten()
                {
                    let p = file.to_string_lossy();
                    let Some(rel) = p.strip_prefix(&prefix) else {
                        continue;
                    };
                    let child = rel.split('/').next().unwrap_or_default();
                    if child.is_empty() || !requested.contains(child) {
                        continue;
                    }
                    out.entry(child.to_string()).or_insert_with(|| meta.clone());
                }
            }
            if out.len() == requested.len() {
                break;
            }
        }
        Ok(out)
    }

    pub fn commit_log(
        &self,
        ref_name: &str,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<CommitMeta>, bool), DurableError> {
        if offset > COMMIT_LOG_MAX_OFFSET || limit > COMMIT_LOG_MAX_PAGE {
            return Err(DurableError::Git(
                "commit log pagination limit exceeded".into(),
            ));
        }
        let repo = self.open_git()?;
        let Some(tip) = self.tip_commit(&repo, ref_name)? else {
            return Ok((Vec::new(), false));
        };
        let mut walk = repo.revwalk().map_err(|e| git_err("revwalk", e))?;
        walk.set_sorting(git2::Sort::TIME)
            .map_err(|e| git_err("revwalk sort", e))?;
        walk.push(tip).map_err(|e| git_err("revwalk push", e))?;
        let mut seen = 0usize;
        let mut out = Vec::new();
        let mut has_more = false;
        for oid_res in walk {
            let oid = oid_res.map_err(|e| git_err("revwalk next", e))?;
            if seen < offset {
                seen += 1;
                continue;
            }
            if out.len() == limit {
                has_more = true;
                break;
            }
            let c = repo
                .find_commit(oid)
                .map_err(|e| git_err("find_commit", e))?;
            out.push(commit_meta(&c));
            seen += 1;
        }
        Ok((out, has_more))
    }

    pub fn pr_commit_snapshot(
        &self,
        base_ref: &str,
        head_oid: &str,
    ) -> Result<Option<PrCommitSnapshot>, DurableError> {
        let repo = self.open_git()?;
        let head = match git2::Oid::from_str(head_oid) {
            Ok(oid) => oid,
            Err(_) => return Ok(None),
        };
        match repo.find_commit(head) {
            Ok(_) => {}
            Err(error) if error.code() == git2::ErrorCode::NotFound => return Ok(None),
            Err(error) => return Err(git_err("find pull-request head", error)),
        }
        Ok(Some(PrCommitSnapshot {
            base_oid: self.tip_commit(&repo, base_ref)?.map(|oid| oid.to_string()),
            head_oid: head.to_string(),
        }))
    }

    pub fn commits_in_pr_snapshot(
        &self,
        base_oid: Option<&str>,
        head_oid: &str,
        position: usize,
        limit: usize,
    ) -> Result<(Vec<CommitMeta>, bool), PrCommitPageError> {
        self.commits_in_pr_snapshot_with_graph_caps(
            base_oid,
            head_oid,
            position,
            limit,
            PR_COMMIT_MAX_GRAPH_NODES_PER_PIN,
            PR_COMMIT_MAX_GRAPH_EDGES_PER_PIN,
        )
    }

    fn commits_in_pr_snapshot_with_graph_caps(
        &self,
        base_oid: Option<&str>,
        head_oid: &str,
        position: usize,
        limit: usize,
        node_cap: usize,
        edge_cap: usize,
    ) -> Result<(Vec<CommitMeta>, bool), PrCommitPageError> {
        if position > PR_COMMIT_MAX_POSITION || limit == 0 || limit > COMMIT_LOG_MAX_PAGE {
            return Err(PrCommitPageError::InvalidPagination);
        }
        let repo = self.open_git()?;
        let head = git2::Oid::from_str(head_oid).map_err(|_| PrCommitPageError::SnapshotExpired)?;
        if let Err(error) = repo.find_commit(head) {
            return if error.code() == git2::ErrorCode::NotFound {
                Err(PrCommitPageError::SnapshotExpired)
            } else {
                Err(PrCommitPageError::Durable(git_err(
                    "find pinned pull-request head",
                    error,
                )))
            };
        }
        let base = base_oid
            .map(|value| git2::Oid::from_str(value).map_err(|_| PrCommitPageError::SnapshotExpired))
            .transpose()?;
        if let Some(base) = base {
            if let Err(error) = repo.find_commit(base) {
                return if error.code() == git2::ErrorCode::NotFound {
                    Err(PrCommitPageError::SnapshotExpired)
                } else {
                    Err(PrCommitPageError::Durable(git_err(
                        "find pinned pull-request base",
                        error,
                    )))
                };
            }
        }

        preflight_pr_commit_reachability(&repo, head, node_cap, edge_cap, "head")?;
        if let Some(base) = base {
            preflight_pr_commit_reachability(&repo, base, node_cap, edge_cap, "base")?;
        }

        let mut walk = repo
            .revwalk()
            .map_err(|error| PrCommitPageError::Durable(git_err("pull-request revwalk", error)))?;
        walk.set_sorting(git2::Sort::TIME).map_err(|error| {
            PrCommitPageError::Durable(git_err("pull-request revwalk sort", error))
        })?;
        walk.push(head)
            .map_err(|error| pr_commit_walk_error("pull-request revwalk push head", error))?;
        if let Some(base) = base {
            walk.hide(base)
                .map_err(|error| pr_commit_walk_error("pull-request revwalk hide base", error))?;
        }

        let mut seen = 0usize;
        let mut out = Vec::new();
        let mut has_more = false;
        for oid_result in walk {
            let oid = oid_result
                .map_err(|error| pr_commit_walk_error("pull-request revwalk next", error))?;
            if seen < position {
                seen += 1;
                continue;
            }
            if out.len() == limit {
                has_more = true;
                break;
            }
            let commit = repo.find_commit(oid).map_err(|error| {
                if error.code() == git2::ErrorCode::NotFound {
                    PrCommitPageError::SnapshotExpired
                } else {
                    PrCommitPageError::Durable(git_err("find pull-request commit", error))
                }
            })?;
            out.push(commit_meta(&commit));
            seen += 1;
        }
        Ok((out, has_more))
    }

    pub fn commits_in_pr(
        &self,
        base_ref: &str,
        head_oid: &str,
        limit: usize,
    ) -> Result<(Vec<CommitMeta>, bool), DurableError> {
        if limit > COMMIT_LOG_MAX_PAGE {
            return Err(DurableError::Git(
                "commit log pagination limit exceeded".into(),
            ));
        }
        let Some(snapshot) = self.pr_commit_snapshot(base_ref, head_oid)? else {
            return Ok((Vec::new(), false));
        };
        self.commits_in_pr_snapshot(snapshot.base_oid.as_deref(), &snapshot.head_oid, 0, limit)
            .map_err(|error| match error {
                PrCommitPageError::InvalidPagination => {
                    DurableError::Git("commit log pagination limit exceeded".into())
                }
                PrCommitPageError::CapacityExceeded => {
                    DurableError::Git("pull-request commit history limit exceeded".into())
                }
                PrCommitPageError::SnapshotExpired => {
                    DurableError::NotFound("pull-request commit snapshot expired".into())
                }
                PrCommitPageError::Durable(error) => error,
            })
    }

    pub fn commit_detail(&self, oid_str: &str) -> Result<Option<CommitDetail>, DurableError> {
        self.commit_detail_bounded(oid_str, COMMIT_DIFF_LIMITS)
    }

    fn commit_detail_bounded(
        &self,
        oid_str: &str,
        limits: CommitDiffLimits,
    ) -> Result<Option<CommitDetail>, DurableError> {
        let repo = self.open_git()?;
        let goid = match git2::Oid::from_str(oid_str) {
            Ok(o) => o,
            Err(_) => return Ok(None),
        };
        let commit = match repo.find_commit(goid) {
            Ok(c) => c,
            Err(e) if e.code() == git2::ErrorCode::NotFound => return Ok(None),
            Err(e) => return Err(git_err("find_commit", e)),
        };
        let tree = commit.tree().map_err(|e| git_err("commit tree", e))?;
        let parent_tree = if commit.parent_count() > 0 {
            Some(
                commit
                    .parent(0)
                    .map_err(|e| git_err("parent", e))?
                    .tree()
                    .map_err(|e| git_err("parent tree", e))?,
            )
        } else {
            None
        };
        let mut opts = git2::DiffOptions::new();
        let diff = repo
            .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut opts))
            .map_err(|e| git_err("diff_tree_to_tree", e))?;
        if diff.deltas().len() > limits.files {
            return Err(DurableError::Git(format!(
                "commit diff computation limit exceeded: commit changes more than {} files",
                limits.files
            )));
        }
        let message = commit.message().unwrap_or("");
        if message.len() > limits.message_bytes {
            return Err(DurableError::Git(format!(
                "commit diff computation limit exceeded: commit message exceeds {} bytes",
                limits.message_bytes
            )));
        }

        let files: std::cell::RefCell<Vec<FileDelta>> = std::cell::RefCell::new(Vec::new());
        let rendered_bytes = std::cell::Cell::new(0usize);
        let limit_exceeded = std::cell::Cell::new(false);
        let mut file_cb = |delta: git2::DiffDelta<'_>, _progress: f32| {
            let path = delta
                .new_file()
                .path()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            let old_path = delta
                .old_file()
                .path()
                .map(|p| p.to_string_lossy().to_string());
            let status = match delta.status() {
                git2::Delta::Added => 'A',
                git2::Delta::Deleted => 'D',
                git2::Delta::Renamed => 'R',
                git2::Delta::Copied => 'C',
                _ => 'M',
            };
            let old_path = old_path.filter(|o| o != &path);
            files.borrow_mut().push(FileDelta {
                path,
                old_path,
                status,
                lines: Vec::new(),
            });
            true
        };
        let mut line_cb = |_delta: git2::DiffDelta<'_>,
                           _hunk: Option<git2::DiffHunk<'_>>,
                           line: git2::DiffLine<'_>| {
            let origin = line.origin();
            if matches!(origin, '+' | '-' | ' ') {
                let content = String::from_utf8_lossy(line.content())
                    .trim_end_matches('\n')
                    .to_string();
                if let Some(f) = files.borrow_mut().last_mut() {
                    let next_bytes = rendered_bytes.get().checked_add(content.len());
                    if f.lines.len() == limits.lines_per_file
                        || content.len() > limits.line_bytes
                        || next_bytes.is_none_or(|bytes| bytes > limits.rendered_bytes)
                    {
                        limit_exceeded.set(true);
                        return false;
                    }
                    rendered_bytes.set(next_bytes.unwrap_or(limits.rendered_bytes));
                    f.lines.push((origin, content));
                }
            }
            true
        };
        let traversal = diff.foreach(&mut file_cb, None, None, Some(&mut line_cb));
        if limit_exceeded.get() {
            return Err(DurableError::Git(
                "commit diff computation limit exceeded: commit diff content is too large".into(),
            ));
        }
        traversal.map_err(|e| git_err("diff foreach", e))?;

        Ok(Some(CommitDetail {
            meta: commit_meta(&commit),
            message: message.to_string(),
            files: files.into_inner(),
        }))
    }

    pub fn pr_diff(
        &self,
        base_ref: &str,
        head_oid: &str,
        per_file_line_cap: usize,
    ) -> Result<Option<PrDiff>, DurableError> {
        self.pr_diff_bounded(
            base_ref,
            head_oid,
            per_file_line_cap,
            PR_DIFF_MAX_FILES,
            DIFF_MAX_LINE_BYTES,
            DIFF_MAX_RENDERED_BYTES,
        )
    }

    fn pr_diff_bounded(
        &self,
        base_ref: &str,
        head_oid: &str,
        per_file_line_cap: usize,
        maximum_files: usize,
        maximum_line_bytes: usize,
        maximum_rendered_bytes: usize,
    ) -> Result<Option<PrDiff>, DurableError> {
        let repo = self.open_git()?;
        let head = match git2::Oid::from_str(head_oid) {
            Ok(o) => o,
            Err(_) => return Ok(None),
        };
        let head_commit = match repo.find_commit(head) {
            Ok(c) => c,
            Err(e) if e.code() == git2::ErrorCode::NotFound => return Ok(None),
            Err(e) => return Err(git_err("find head commit", e)),
        };
        let base_tip = match self.tip_commit(&repo, base_ref)? {
            Some(t) => Some(t),
            None if base_ref.starts_with("refs/") => None,
            None => self.tip_commit(&repo, &format!("refs/heads/{base_ref}"))?,
        };
        let (base_oid, three_dot): (Option<git2::Oid>, bool) = match base_tip {
            Some(bt) => match repo.merge_base(bt, head) {
                Ok(mb) => (Some(mb), true),
                Err(_) => (Some(bt), false),
            },
            None => (None, false),
        };

        let head_tree = head_commit.tree().map_err(|e| git_err("head tree", e))?;
        let base_tree = match base_oid {
            Some(o) => {
                let base_commit = repo
                    .find_commit(o)
                    .map_err(|e| git_err("find base commit", e))?;
                Some(base_commit.tree().map_err(|e| git_err("base tree", e))?)
            }
            None => None,
        };

        let mut opts = git2::DiffOptions::new();
        opts.include_typechange(true);
        let diff = repo
            .diff_tree_to_tree(base_tree.as_ref(), Some(&head_tree), Some(&mut opts))
            .map_err(|e| git_err("pr diff_tree_to_tree", e))?;
        if diff.deltas().len() > maximum_files {
            return Err(DurableError::Git(format!(
                "pr diff computation limit exceeded: pull request changes more than {maximum_files} files"
            )));
        }

        let files: std::cell::RefCell<Vec<PrFileDelta>> = std::cell::RefCell::new(Vec::new());
        let rendered = std::cell::RefCell::new(0usize);
        let rendered_bytes = std::cell::Cell::new(0usize);
        let limit_exceeded = std::cell::Cell::new(false);
        let mut file_cb = |delta: git2::DiffDelta<'_>, _p: f32| {
            *rendered.borrow_mut() = 0;
            let path = delta
                .new_file()
                .path()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            let old_path = delta
                .old_file()
                .path()
                .map(|p| p.to_string_lossy().to_string())
                .filter(|o| o != &path);
            let status = match delta.status() {
                git2::Delta::Added => 'A',
                git2::Delta::Deleted => 'D',
                git2::Delta::Renamed => 'R',
                git2::Delta::Copied => 'C',
                _ => 'M',
            };
            let kind = if matches!(delta.new_file().mode(), git2::FileMode::Commit)
                || matches!(delta.old_file().mode(), git2::FileMode::Commit)
            {
                FileKind::Submodule
            } else if delta.flags().contains(git2::DiffFlags::BINARY) {
                FileKind::Binary
            } else {
                FileKind::Text
            };
            let new_oid = delta.new_file().id();
            let new_blob_oid = if status != 'D' && kind != FileKind::Submodule && !new_oid.is_zero()
            {
                Some(new_oid.to_string())
            } else {
                None
            };
            let size_bytes = delta.new_file().size();
            let size_bytes = if size_bytes > 0 {
                Some(size_bytes)
            } else {
                None
            };
            files.borrow_mut().push(PrFileDelta {
                path,
                old_path,
                new_blob_oid,
                status,
                kind,
                additions: 0,
                deletions: 0,
                size_bytes,
                hunks: Vec::new(),
                deleted_body_available: status == 'D',
                truncated: false,
            });
            true
        };
        let mut binary_cb = |_d: git2::DiffDelta<'_>, _b: git2::DiffBinary<'_>| {
            if let Some(f) = files.borrow_mut().last_mut() {
                if f.kind == FileKind::Text {
                    f.kind = FileKind::Binary;
                }
            }
            true
        };
        let mut hunk_cb = |_d: git2::DiffDelta<'_>, hunk: git2::DiffHunk<'_>| {
            if let Some(f) = files.borrow_mut().last_mut() {
                f.hunks.push(DiffHunkDelta {
                    header: String::from_utf8_lossy(hunk.header())
                        .trim_end_matches('\n')
                        .to_string(),
                    old_start: hunk.old_start(),
                    old_lines: hunk.old_lines(),
                    new_start: hunk.new_start(),
                    new_lines: hunk.new_lines(),
                    lines: Vec::new(),
                });
            }
            true
        };
        let mut line_cb =
            |_d: git2::DiffDelta<'_>, _h: Option<git2::DiffHunk<'_>>, line: git2::DiffLine<'_>| {
                let origin = line.origin();
                if !matches!(origin, '+' | '-' | ' ') {
                    return true;
                }
                let mut fs = files.borrow_mut();
                if let Some(f) = fs.last_mut() {
                    match origin {
                        '+' => f.additions += 1,
                        '-' => f.deletions += 1,
                        _ => {}
                    }
                    let mut r = rendered.borrow_mut();
                    if per_file_line_cap > 0 && *r >= per_file_line_cap {
                        f.truncated = true;
                    } else {
                        let content = String::from_utf8_lossy(line.content())
                            .trim_end_matches('\n')
                            .to_string();
                        let next_bytes = rendered_bytes.get().checked_add(content.len());
                        if content.len() > maximum_line_bytes
                            || next_bytes.is_none_or(|bytes| bytes > maximum_rendered_bytes)
                        {
                            limit_exceeded.set(true);
                            return false;
                        }
                        rendered_bytes.set(next_bytes.unwrap_or(maximum_rendered_bytes));
                        if f.kind == FileKind::Text
                            && origin == '+'
                            && content.starts_with("version https://git-lfs")
                        {
                            f.kind = FileKind::Lfs;
                        }
                        if let Some(h) = f.hunks.last_mut() {
                            h.lines.push(DiffLineDelta {
                                origin,
                                content,
                                old_no: line.old_lineno(),
                                new_no: line.new_lineno(),
                            });
                            *r += 1;
                        }
                    }
                }
                true
            };
        let traversal = diff.foreach(
            &mut file_cb,
            Some(&mut binary_cb),
            Some(&mut hunk_cb),
            Some(&mut line_cb),
        );
        if limit_exceeded.get() {
            return Err(DurableError::Git(
                "pr diff computation limit exceeded: rendered diff content is too large".into(),
            ));
        }
        traversal.map_err(|e| git_err("pr diff foreach", e))?;

        let mut files = files.into_inner();
        for f in &mut files {
            if f.kind != FileKind::Text {
                f.hunks.clear();
            }
            if f.truncated {
                f.hunks.retain(|h| !h.lines.is_empty());
            }
        }
        let total_files = files.len();
        let total_additions = files.iter().map(|f| f.additions).sum();
        let total_deletions = files.iter().map(|f| f.deletions).sum();
        Ok(Some(PrDiff {
            base_oid: base_oid.map(|o| o.to_string()).unwrap_or_default(),
            head_oid: head_commit.id().to_string(),
            three_dot,
            files,
            total_files,
            total_additions,
            total_deletions,
        }))
    }

    pub fn file_lines(
        &self,
        oid: &str,
        start: usize,
        end: usize,
    ) -> Result<FileLinesLookup, DurableError> {
        if start == 0
            || end < start
            || end > u32::MAX as usize
            || end - start + 1 > FILE_LINES_MAX_RANGE
        {
            return Err(DurableError::Git("invalid file line range".into()));
        }
        if oid.len() != 40
            || !oid
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Ok(FileLinesLookup::Missing);
        }
        let repo = self.open_git()?;
        let goid = match git2::Oid::from_str(oid) {
            Ok(o) => o,
            Err(_) => return Ok(FileLinesLookup::Missing),
        };
        let odb = repo
            .odb()
            .map_err(|error| git_err("open object database", error))?;
        let (object_size, object_kind) = match odb.read_header(goid) {
            Ok(header) => header,
            Err(error) if error.code() == git2::ErrorCode::NotFound => {
                return Ok(FileLinesLookup::Missing)
            }
            Err(error) => return Err(git_err("read object header", error)),
        };
        if object_kind != git2::ObjectType::Blob {
            return Ok(FileLinesLookup::Missing);
        }
        if object_size > FILE_LINES_MAX_BLOB_BYTES {
            return Ok(FileLinesLookup::TooLarge {
                size: object_size as u64,
                maximum: FILE_LINES_MAX_BLOB_BYTES,
            });
        }
        let blob = match repo.find_blob(goid) {
            Ok(b) => b,
            Err(e) if e.code() == git2::ErrorCode::NotFound => return Ok(FileLinesLookup::Missing),
            Err(e) => return Err(git_err("find blob", e)),
        };
        if blob.is_binary() {
            return Ok(FileLinesLookup::Binary);
        }
        let text = String::from_utf8_lossy(blob.content());
        let out = text
            .split('\n')
            .enumerate()
            .map(|(i, l)| (i + 1, l))
            .skip(start - 1)
            .take(end - start + 1)
            .map(|(n, l)| DiffLineDelta {
                origin: ' ',
                content: l.trim_end_matches('\r').to_string(),
                old_no: None,
                new_no: Some(n as u32),
            })
            .collect();
        Ok(FileLinesLookup::Found(out))
    }

    pub fn build_file_commit(
        &self,
        ref_name: &str,
        path: &str,
        contents: &[u8],
        message: &str,
        author_name: &str,
        author_email: &str,
    ) -> Result<(Oid, Oid, Option<Oid>), DurableError> {
        self.build_file_commit_from_ref(
            ref_name,
            None,
            path,
            contents,
            message,
            author_name,
            author_email,
        )
    }

    /// Build a file commit on `ref_name`, optionally using another ref as the first parent when the
    /// target branch does not exist yet. The returned optional OID is always the prior target tip,
    /// which keeps the caller's ref-CAS expectation independent from the commit's start point.
    #[allow(clippy::too_many_arguments)]
    pub fn build_file_commit_from_ref(
        &self,
        ref_name: &str,
        start_ref: Option<&str>,
        path: &str,
        contents: &[u8],
        message: &str,
        author_name: &str,
        author_email: &str,
    ) -> Result<(Oid, Oid, Option<Oid>), DurableError> {
        let repo = self.open_git()?;
        let target_oid = self.tip_commit(&repo, ref_name)?;
        let parent_oid = match (&target_oid, start_ref) {
            (Some(oid), _) => Some(*oid),
            (None, Some(source)) => Some(self.tip_commit(&repo, source)?.ok_or_else(|| {
                DurableError::NotFound(format!("branch start ref `{source}` does not exist"))
            })?),
            (None, None) => None,
        };
        let parent = parent_oid.map(|oid| Oid::new(oid.to_string()));
        let prepared = self.prepare_file_commit(
            parent.as_ref(),
            path,
            contents,
            message,
            author_name,
            author_email,
        )?;
        Ok((
            prepared.commit,
            prepared.blob,
            target_oid.map(|p| Oid::new(p.to_string())),
        ))
    }

    /// Constructs an immutable file commit in this repository's object database without moving a
    /// ref. Callers that accept untrusted content should invoke this on a quarantine repository,
    /// evaluate the complete returned object set, and promote it only after admission succeeds.
    pub fn prepare_file_commit(
        &self,
        parent_oid: Option<&Oid>,
        path: &str,
        contents: &[u8],
        message: &str,
        author_name: &str,
        author_email: &str,
    ) -> Result<PreparedFileCommit, DurableError> {
        validate_file_edit_path(path)?;
        let clean_path = path;
        let repo = self.open_git()?;

        let blob_oid = repo.blob(contents).map_err(|e| git_err("write blob", e))?;
        let parent_oid = parent_oid.map(Self::parse_oid).transpose()?;
        let base_tree = match parent_oid {
            Some(parent) => {
                let commit = repo
                    .find_commit(parent)
                    .map_err(|e| git_err("find parent", e))?;
                Some(commit.tree().map_err(|e| git_err("parent tree", e))?)
            }
            None => None,
        };
        let segments = clean_path.split('/').collect::<Vec<_>>();
        let mut trees = Vec::with_capacity(segments.len());
        let tree_oid =
            Self::write_blob_tree(&repo, base_tree.as_ref(), &segments, blob_oid, &mut trees)?;
        let tree_obj = repo
            .find_tree(tree_oid)
            .map_err(|e| git_err("find tree", e))?;

        let sig =
            git2::Signature::now(author_name, author_email).map_err(|e| git_err("signature", e))?;
        let parent_commits: Vec<git2::Commit<'_>> = match parent_oid {
            Some(parent) => vec![repo
                .find_commit(parent)
                .map_err(|e| git_err("find parent", e))?],
            None => Vec::new(),
        };
        let parent_refs: Vec<&git2::Commit<'_>> = parent_commits.iter().collect();
        let commit_oid = repo
            .commit(None, &sig, &sig, message, &tree_obj, &parent_refs)
            .map_err(|e| git_err("write commit", e))?;

        Ok(PreparedFileCommit {
            commit: Oid::new(commit_oid.to_string()),
            blob: Oid::new(blob_oid.to_string()),
            trees,
        })
    }

    fn write_blob_tree(
        repo: &git2::Repository,
        base_tree: Option<&git2::Tree<'_>>,
        path: &[&str],
        blob_oid: git2::Oid,
        written_trees: &mut Vec<Oid>,
    ) -> Result<git2::Oid, DurableError> {
        let (name, descendants) = path
            .split_first()
            .ok_or_else(|| DurableError::Git("web edit path is empty".into()))?;
        let mut builder = repo
            .treebuilder(base_tree)
            .map_err(|error| git_err("treebuilder", error))?;
        if descendants.is_empty() {
            builder
                .insert(name, blob_oid, 0o100644)
                .map_err(|error| git_err(&format!("tree insert {name}"), error))?;
        } else {
            let child_tree = match base_tree.and_then(|tree| tree.get_name(name)) {
                Some(entry) if entry.kind() == Some(git2::ObjectType::Tree) => Some(
                    repo.find_tree(entry.id())
                        .map_err(|error| git_err(&format!("find tree {name}"), error))?,
                ),
                Some(_) => {
                    return Err(DurableError::Git(format!(
                        "web edit path component {name} is not a directory"
                    )))
                }
                None => None,
            };
            let child_oid = Self::write_blob_tree(
                repo,
                child_tree.as_ref(),
                descendants,
                blob_oid,
                written_trees,
            )?;
            builder
                .insert(name, child_oid, 0o040000)
                .map_err(|error| git_err(&format!("tree insert {name}"), error))?;
        }
        let oid = builder
            .write()
            .map_err(|error| git_err("write tree", error))?;
        written_trees.push(Oid::new(oid.to_string()));
        Ok(oid)
    }

    pub fn object_is_commit(&self, oid: &Oid) -> bool {
        let Ok(repo) = self.open_git() else {
            return false;
        };
        let Ok(goid) = git2::Oid::from_str(oid.as_str()) else {
            return false;
        };
        let is_commit = repo.find_commit(goid).is_ok();
        is_commit
    }

    pub fn is_fast_forward(
        &self,
        base_tip: Option<&Oid>,
        head: &Oid,
    ) -> Result<bool, DurableError> {
        let repo = self.open_git()?;
        let head_g = Self::parse_oid(head)?;
        if repo.find_commit(head_g).is_err() {
            return Ok(false);
        }
        match base_tip {
            None => Ok(true),
            Some(base) => {
                let base_g = Self::parse_oid(base)?;
                if base_g == head_g {
                    return Ok(true);
                }
                repo.graph_descendant_of(head_g, base_g)
                    .map_err(|e| git_err("graph_descendant_of", e))
            }
        }
    }

    pub fn fsck(&self) -> Result<(), DurableError> {
        let repo = self.open_git()?;
        let odb = repo.odb().map_err(|e| git_err("odb", e))?;
        let mut count = 0usize;
        odb.foreach(|oid| {
            if odb.read(*oid).is_err() {
                return false;
            }
            count += 1;
            true
        })
        .map_err(|e| git_err("odb foreach (corrupt object?)", e))?;
        for (name, tip) in self.list_refs_bounded(WIRE_MAX_REFS)? {
            let goid = Self::parse_oid(&tip)?;
            if !odb.exists(goid) {
                return Err(DurableError::Git(format!(
                    "fsck: ref {name} points at missing object {}",
                    tip.0
                )));
            }
        }
        Ok(())
    }

    pub fn write_raw_object(&self, kind: &str, payload: &[u8]) -> Result<Oid, DurableError> {
        let obj_type = match kind {
            "commit" => git2::ObjectType::Commit,
            "tree" => git2::ObjectType::Tree,
            "blob" => git2::ObjectType::Blob,
            "tag" => git2::ObjectType::Tag,
            other => {
                return Err(DurableError::Git(format!(
                    "refusing to migrate an object of unknown type `{other}` into the durable repo"
                )))
            }
        };
        let repo = self.open_git()?;
        let odb = repo.odb().map_err(|e| git_err("odb", e))?;
        let oid = odb
            .write(obj_type, payload)
            .map_err(|e| git_err(&format!("write {kind} object"), e))?;
        Ok(Oid::new(oid.to_string()))
    }

    pub fn commit_tree_complete(&self, tip: &Oid) -> Result<bool, DurableError> {
        let repo = self.open_git()?;
        let goid = Self::parse_oid(tip)?;
        let commit = match repo.find_commit(goid) {
            Ok(c) => c,
            Err(_) => return Ok(false),
        };
        let tree = match commit.tree() {
            Ok(t) => t,
            Err(_) => return Ok(false),
        };
        let odb = repo.odb().map_err(|e| git_err("odb", e))?;
        Self::tree_objects_present(&odb, &tree)
    }

    fn tree_objects_present(odb: &git2::Odb, tree: &git2::Tree) -> Result<bool, DurableError> {
        let mut complete = true;
        tree.walk(git2::TreeWalkMode::PreOrder, |_root, entry| {
            match entry.kind() {
                Some(git2::ObjectType::Tree) | Some(git2::ObjectType::Blob)
                    if !odb.exists(entry.id()) =>
                {
                    complete = false;
                    git2::TreeWalkResult::Abort
                }
                _ => git2::TreeWalkResult::Ok,
            }
        })
        .map_err(|e| git_err("tree walk", e))?;
        Ok(complete)
    }

    pub fn history_connectivity_complete(
        &self,
        new_tip: &Oid,
        existing_tips: &[Oid],
    ) -> Result<bool, DurableError> {
        let repo = self.open_git()?;
        let odb = repo.odb().map_err(|e| git_err("odb", e))?;
        let tip_g = Self::parse_oid(new_tip)?;
        if repo.find_commit(tip_g).is_err() {
            return Ok(false);
        }

        let mut walk = repo.revwalk().map_err(|e| git_err("revwalk", e))?;
        walk.push(tip_g)
            .map_err(|e| git_err("revwalk push new_tip", e))?;
        for t in existing_tips {
            if let Ok(g) = Self::parse_oid(t) {
                if odb.exists(g) {
                    let _ = walk.hide(g);
                }
            }
        }

        for step in walk {
            let commit_oid = match step {
                Ok(o) => o,
                Err(_) => return Ok(false),
            };
            let commit = match repo.find_commit(commit_oid) {
                Ok(c) => c,
                Err(_) => return Ok(false),
            };
            let tree = match commit.tree() {
                Ok(t) => t,
                Err(_) => return Ok(false),
            };
            if !Self::tree_objects_present(&odb, &tree)? {
                return Ok(false);
            }
            for parent_oid in commit.parent_ids() {
                if !odb.exists(parent_oid) {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
}

const REPOSITORY_CATALOGUE_KEY_FILE: &str = "myelin-catalogue-key";
pub const REPOSITORY_CATALOGUE_KEY_MAX_BYTES: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RepositoryCatalogueKey(String);

impl RepositoryCatalogueKey {
    fn parse(value: impl Into<String>) -> Result<Self, DurableError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > REPOSITORY_CATALOGUE_KEY_MAX_BYTES
            || !value.bytes().all(|byte| byte.is_ascii_alphanumeric())
        {
            return Err(DurableError::Git(
                "repository catalogue key is malformed".into(),
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn read_repository_catalogue_key(
    repo_path: &Path,
) -> Result<Option<RepositoryCatalogueKey>, DurableError> {
    let path = repo_path.join(REPOSITORY_CATALOGUE_KEY_FILE);
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(DurableError::Io(format!(
                "inspect repository catalogue key {}: {error}",
                path.display()
            )))
        }
    };
    if !metadata.file_type().is_file()
        || metadata.len() == 0
        || metadata.len() > REPOSITORY_CATALOGUE_KEY_MAX_BYTES as u64
    {
        return Err(DurableError::Git(
            "repository catalogue key is malformed".into(),
        ));
    }
    let value = std::fs::read_to_string(&path).map_err(|error| {
        DurableError::Io(format!(
            "read repository catalogue key {}: {error}",
            path.display()
        ))
    })?;
    RepositoryCatalogueKey::parse(value).map(Some)
}

fn persist_repository_catalogue_key(
    repo_path: &Path,
    key: &RepositoryCatalogueKey,
) -> Result<(), DurableError> {
    if read_repository_catalogue_key(repo_path)?.is_some() {
        return Ok(());
    }
    write_file_atomic(
        repo_path,
        &repo_path.join(REPOSITORY_CATALOGUE_KEY_FILE),
        key.0.as_bytes(),
    )
}

pub struct DurableGitStore<P: RepoPathResolver = RootedResolver> {
    resolver: P,
    minter: Arc<dyn IdMinter>,
}

pub enum RepoCreationClaim {
    Existing(DurableGitRepo),
    Acquired(RepoCreationGuard),
}

pub struct RepoCreationGuard {
    repo_path: PathBuf,
    owner_path: PathBuf,
    catalogue_key: RepositoryCatalogueKey,
    _lock: std::fs::File,
}

impl RepoCreationGuard {
    pub fn initialize(&self) -> Result<DurableGitRepo, DurableError> {
        let repo = init_bare_repo(&self.repo_path)?;
        persist_repository_catalogue_key(&self.repo_path, &self.catalogue_key)?;
        Ok(repo)
    }

    pub fn complete(self) -> Result<(), DurableError> {
        match std::fs::remove_file(&self.owner_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(DurableError::Io(format!(
                    "remove completed repository claim {}: {error}",
                    self.owner_path.display()
                )))
            }
        }
        let parent = self.owner_path.parent().ok_or_else(|| {
            DurableError::Io("repository creation claim has no parent directory".into())
        })?;
        std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| {
                DurableError::Io(format!(
                    "sync completed repository claim directory {}: {error}",
                    parent.display()
                ))
            })?;
        Ok(())
    }

    pub fn create(self) -> Result<DurableGitRepo, DurableError> {
        let repo = self.initialize()?;
        self.complete()?;
        Ok(repo)
    }
}

fn init_bare_repo(path: &Path) -> Result<DurableGitRepo, DurableError> {
    let git_repo = git2::Repository::init_bare(path)
        .map_err(|error| git_err(&format!("init_bare {}", path.display()), error))?;
    git_repo
        .config()
        .and_then(|mut config| config.set_bool("core.logallrefupdates", true))
        .map_err(|error| git_err("enable logallrefupdates", error))?;
    Ok(DurableGitRepo {
        path: path.to_path_buf(),
    })
}

impl DurableGitStore<RootedResolver> {
    pub fn rooted(root: impl Into<PathBuf>) -> Self {
        Self::rooted_with_minter(root, Arc::new(UlidMinter::new()))
    }

    pub fn rooted_with_minter(root: impl Into<PathBuf>, minter: Arc<dyn IdMinter>) -> Self {
        Self {
            resolver: RootedResolver::new(root),
            minter,
        }
    }
}

impl<P: RepoPathResolver> DurableGitStore<P> {
    pub fn new(resolver: P) -> Self {
        Self::with_minter(resolver, Arc::new(UlidMinter::new()))
    }

    pub fn with_minter(resolver: P, minter: Arc<dyn IdMinter>) -> Self {
        Self { resolver, minter }
    }

    pub fn repo_path(&self, repo: &RepoLoc) -> Result<PathBuf, DurableError> {
        self.resolver
            .repo_path(repo)
            .map_err(|e| DurableError::Git(e.to_string()))
    }

    pub fn create_repo(&self, repo: &RepoLoc) -> Result<DurableGitRepo, DurableError> {
        match self.claim_repo_creation(repo, "myelin:internal")? {
            RepoCreationClaim::Existing(repo) => Ok(repo),
            RepoCreationClaim::Acquired(claim) => claim.create(),
        }
    }

    pub fn repository_catalogue_key(
        &self,
        repo: &RepoLoc,
    ) -> Result<Option<RepositoryCatalogueKey>, DurableError> {
        read_repository_catalogue_key(&self.repo_path(repo)?)
    }

    pub fn claim_repo_creation(
        &self,
        repo: &RepoLoc,
        owner: &str,
    ) -> Result<RepoCreationClaim, DurableError> {
        const MAX_OWNER_BYTES: usize = 4096;
        const OWNER_FINGERPRINT_BYTES: u64 = 64;

        if owner.is_empty() || owner.len() > MAX_OWNER_BYTES {
            return Err(DurableError::Git(
                "repository creation owner is missing or exceeds its storage limit".into(),
            ));
        }
        let owner = blake3::hash(owner.as_bytes()).to_hex().to_string();
        let repo_path = self.repo_path(repo)?;
        let parent = repo_path
            .parent()
            .ok_or_else(|| DurableError::Io("repository path has no parent directory".into()))?;
        std::fs::create_dir_all(parent).map_err(|error| {
            DurableError::Io(format!(
                "create repository parent {}: {error}",
                parent.display()
            ))
        })?;
        let repo_name = repo_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| DurableError::Io("repository path has no UTF-8 file name".into()))?;
        let lock_path = parent.join(format!(".{repo_name}.creation.lock"));
        let owner_path = parent.join(format!(".{repo_name}.creation-owner"));
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|error| {
                DurableError::Io(format!(
                    "open repository creation lock {}: {error}",
                    lock_path.display()
                ))
            })?;
        fs4::fs_std::FileExt::lock_exclusive(&lock).map_err(|error| {
            DurableError::Io(format!(
                "acquire repository creation lock {}: {error}",
                lock_path.display()
            ))
        })?;

        let catalogue_key = match std::fs::metadata(&owner_path) {
            Ok(metadata) => {
                if metadata.len() != OWNER_FINGERPRINT_BYTES {
                    return Err(DurableError::Git(
                        "repository creation claim is malformed".into(),
                    ));
                }
                let claimed_owner = std::fs::read_to_string(&owner_path).map_err(|error| {
                    DurableError::Io(format!(
                        "read repository creation claim {}: {error}",
                        owner_path.display()
                    ))
                })?;
                if claimed_owner != owner {
                    return Err(DurableError::Conflict(format!(
                        "repository `{}` is already being created by another principal",
                        repo.repo
                    )));
                }
                RepositoryCatalogueKey::parse(self.minter.mint().0)?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if let Ok(repository) = git2::Repository::open(&repo_path) {
                    drop(repository);
                    return Ok(RepoCreationClaim::Existing(DurableGitRepo {
                        path: repo_path,
                    }));
                }
                let catalogue_key = RepositoryCatalogueKey::parse(self.minter.mint().0)?;
                write_file_atomic(parent, &owner_path, owner.as_bytes())?;
                catalogue_key
            }
            Err(error) => {
                return Err(DurableError::Io(format!(
                    "inspect repository creation claim {}: {error}",
                    owner_path.display()
                )))
            }
        };

        Ok(RepoCreationClaim::Acquired(RepoCreationGuard {
            repo_path,
            owner_path,
            catalogue_key,
            _lock: lock,
        }))
    }

    pub fn open_repo(&self, repo: &RepoLoc) -> Result<DurableGitRepo, DurableError> {
        let path = self.repo_path(repo)?;
        git2::Repository::open(&path)
            .map_err(|_| DurableError::NotFound(format!("bare repo {}", repo.repo)))?;
        Ok(DurableGitRepo { path })
    }

    pub fn repo_exists(&self, repo: &RepoLoc) -> bool {
        let Ok(path) = self.repo_path(repo) else {
            return false;
        };
        git2::Repository::open(&path).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        p.push(format!("myelin-durable-{tag}-{nanos}"));
        p
    }

    fn loc() -> RepoLoc {
        RepoLoc::new("acme", "fr-par", "core")
    }

    fn seed_commit(repo: &DurableGitRepo, content: &[u8]) -> Oid {
        let blob = repo.write_blob(content).expect("blob");
        let tree = repo.write_tree(&[("file.txt", &blob)]).expect("tree");
        repo.write_commit(
            &tree,
            &[],
            "feat: seed",
            "psn-7@acme.noreply",
            "psn-7@acme.noreply",
        )
        .expect("commit")
    }

    #[test]
    fn file_edits_distinguish_project_metadata_from_git_administration() {
        for path in [
            "README.md",
            ".gitignore",
            ".github/workflows/test.yml",
            "docs/.gitkeep",
        ] {
            assert!(
                validate_file_edit_path(path).is_ok(),
                "ordinary project path `{path}` remains editable"
            );
        }

        for path in [".git/config", ".GIT/config", "nested/.GiT/hooks/pre-commit"] {
            assert!(matches!(
                validate_file_edit_path(path),
                Err(DurableError::InvalidInput(message))
                    if message.contains("reserved Git administrative component")
            ));
        }
        assert!(matches!(
            validate_file_edit_path("docs/../secrets.txt"),
            Err(DurableError::InvalidInput(message)) if message == "file edit path is not safe"
        ));
    }

    #[test]
    fn a_new_branch_commit_can_start_from_an_existing_branch_without_changing_target_cas() {
        let root = temp_root("file-commit-start-ref");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create repo");
        let main = seed_commit(&repo, b"main branch\n");
        repo.update_ref_cas(
            "refs/heads/main",
            None,
            Some(&main),
            "create main",
            "psn-7@acme.noreply",
        )
        .expect("create main ref");

        let (feature, _, prior_target) = repo
            .build_file_commit_from_ref(
                "refs/heads/feature",
                Some("refs/heads/main"),
                "nested/feature.txt",
                b"feature branch\n",
                "feat: branch from main",
                "psn-7@acme.noreply",
                "psn-7@acme.noreply",
            )
            .expect("build feature commit");

        assert_eq!(prior_target, None, "the target branch does not exist yet");
        assert_eq!(
            repo.read_ref("refs/heads/feature").expect("read feature"),
            None,
            "building the commit does not publish the target ref"
        );
        let git = repo.open_git().expect("open git");
        let commit = git
            .find_commit(DurableGitRepo::parse_oid(&feature).expect("feature oid"))
            .expect("feature commit");
        assert_eq!(commit.parent_count(), 1);
        assert_eq!(
            commit.parent_id(0).expect("first parent").to_string(),
            main.0
        );
        assert!(
            commit
                .tree()
                .expect("feature tree")
                .get_path(std::path::Path::new("file.txt"))
                .is_ok(),
            "the source branch tree is inherited"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    fn write_commit_at(
        repo: &DurableGitRepo,
        tree: &Oid,
        parents: &[&Oid],
        message: &str,
        seconds: i64,
    ) -> Oid {
        let git = repo.open_git().expect("open git");
        let tree = git
            .find_tree(DurableGitRepo::parse_oid(tree).expect("tree oid"))
            .expect("find tree");
        let parents = parents
            .iter()
            .map(|parent| {
                git.find_commit(DurableGitRepo::parse_oid(parent).expect("parent oid"))
                    .expect("find parent")
            })
            .collect::<Vec<_>>();
        let parent_refs = parents.iter().collect::<Vec<_>>();
        let time = git2::Time::new(seconds, 0);
        let signature =
            git2::Signature::new("psn@acme.noreply", "psn@acme.noreply", &time).expect("signature");
        Oid::new(
            git.commit(None, &signature, &signature, message, &tree, &parent_refs)
                .expect("commit")
                .to_string(),
        )
    }

    #[test]
    fn durable_ref_lock_serializes_independent_repo_handles() {
        let root = temp_root("ref-lock");
        let store = DurableGitStore::rooted(&root);
        let first_repo = store.create_repo(&loc()).expect("create");
        let second_repo = store
            .open_repo(&loc())
            .expect("second process-style handle");
        let first_lock = first_repo
            .lock_ref_exclusive("refs/heads/main")
            .expect("first lock");
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (acquired_tx, acquired_rx) = std::sync::mpsc::channel();

        let waiter = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let _second_lock = second_repo
                .lock_ref_exclusive("refs/heads/main")
                .expect("second lock after release");
            acquired_tx.send(()).unwrap();
        });
        started_rx.recv().unwrap();
        assert!(
            acquired_rx
                .recv_timeout(std::time::Duration::from_millis(50))
                .is_err(),
            "a second durable handle cannot enter the same ref window concurrently"
        );

        drop(first_lock);
        acquired_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("releasing the first lock wakes the second handle");
        waiter.join().unwrap();
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn create_repo_inits_a_real_on_disk_bare_repo() {
        let root = temp_root("lifecycle");
        let store = DurableGitStore::rooted(&root);
        assert!(!store.repo_exists(&loc()), "absent before create");

        let repo = store.create_repo(&loc()).expect("create");
        assert_eq!(
            repo.path(),
            root.join("acme").join("fr-par").join("core.git")
        );
        assert!(
            repo.path().is_dir(),
            "the bare repo is a real on-disk directory"
        );
        let catalogue_key = store
            .repository_catalogue_key(&loc())
            .expect("read catalogue key")
            .expect("new repositories carry durable catalogue order");
        assert!(store.repo_exists(&loc()), "present after create");
        assert!(store.create_repo(&loc()).is_ok());
        assert_eq!(
            store.repository_catalogue_key(&loc()).unwrap(),
            Some(catalogue_key),
            "idempotent create preserves the repository's original position"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn repositories_from_before_catalogue_order_remain_a_stable_legacy_tier() {
        let root = temp_root("legacy-catalogue");
        let store = DurableGitStore::rooted(&root);
        let repo_path = store.repo_path(&loc()).unwrap();
        std::fs::create_dir_all(repo_path.parent().unwrap()).unwrap();
        init_bare_repo(&repo_path).expect("simulate a repository created by the prior format");

        assert_eq!(store.repository_catalogue_key(&loc()).unwrap(), None);
        store
            .create_repo(&loc())
            .expect("legacy repository remains usable");
        assert_eq!(
            store.repository_catalogue_key(&loc()).unwrap(),
            None,
            "opening an existing repository must not pretend it was just created"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_initialized_but_unfinished_repo_remains_bound_to_its_creator() {
        let root = temp_root("unfinished-create");
        let store = DurableGitStore::rooted(&root);
        let interrupted = match store
            .claim_repo_creation(&loc(), "principal:alice")
            .expect("alice claims the repository")
        {
            RepoCreationClaim::Acquired(claim) => claim,
            RepoCreationClaim::Existing(_) => panic!("the repository starts absent"),
        };
        let on_disk_claim = std::fs::read_to_string(&interrupted.owner_path)
            .expect("the durable claim is readable");
        assert_eq!(on_disk_claim.len(), 64);
        assert!(
            !on_disk_claim.contains("alice"),
            "the claim stores only an owner fingerprint"
        );
        interrupted
            .initialize()
            .expect("Git initialization reached disk");
        let initial_catalogue_key = store
            .repository_catalogue_key(&loc())
            .unwrap()
            .expect("initialization durably records catalogue order");
        drop(interrupted);

        assert!(matches!(
            store.claim_repo_creation(&loc(), "principal:bob"),
            Err(DurableError::Conflict(_))
        ));
        let resumed = match store
            .claim_repo_creation(&loc(), "principal:alice")
            .expect("alice resumes the unfinished create")
        {
            RepoCreationClaim::Acquired(claim) => claim,
            RepoCreationClaim::Existing(_) => panic!("the durable claim still marks it unfinished"),
        };
        resumed
            .create()
            .expect("the original creator finishes the repository");
        assert_eq!(
            store.repository_catalogue_key(&loc()).unwrap(),
            Some(initial_catalogue_key),
            "recovery cannot make an interrupted repository look newly created"
        );
        assert!(matches!(
            store
                .claim_repo_creation(&loc(), "principal:bob")
                .expect("a finished repository is reported normally"),
            RepoCreationClaim::Existing(_)
        ));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn f9_heal_head_symref_points_head_at_the_pushed_default_branch() {
        let root = temp_root("f9-head");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");

        let g = git2::Repository::open(repo.path()).unwrap();
        assert!(
            g.head().is_err(),
            "fresh init_bare HEAD dangles (a clone would warn)"
        );

        let c = seed_commit(&repo, b"first push\n");
        repo.update_ref_cas(
            "refs/heads/main",
            None,
            Some(&c),
            "create",
            "psn@acme.noreply",
        )
        .expect("create main");
        assert!(
            git2::Repository::open(repo.path()).unwrap().head().is_err(),
            "HEAD still dangles after the push until it is healed"
        );

        repo.heal_head_symref().expect("heal HEAD");

        let g2 = git2::Repository::open(repo.path()).unwrap();
        let head = g2.head().expect("HEAD resolves after heal");
        assert_eq!(
            head.name().unwrap(),
            "refs/heads/main",
            "F9: HEAD points at the pushed default branch"
        );

        repo.heal_head_symref().expect("heal is idempotent");
        assert_eq!(
            git2::Repository::open(repo.path())
                .unwrap()
                .head()
                .unwrap()
                .name()
                .unwrap(),
            "refs/heads/main"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn f9_heal_head_symref_follows_the_first_branch_when_no_main() {
        let root = temp_root("f9-head-develop");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");
        let c = seed_commit(&repo, b"develop\n");
        repo.update_ref_cas(
            "refs/heads/develop",
            None,
            Some(&c),
            "create",
            "psn@acme.noreply",
        )
        .expect("create develop");
        repo.heal_head_symref().expect("heal");
        assert_eq!(
            git2::Repository::open(repo.path())
                .unwrap()
                .head()
                .unwrap()
                .name()
                .unwrap(),
            "refs/heads/develop",
            "F9: with no main, HEAD follows the first branch pushed"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn ref_and_object_survive_a_fresh_handle_over_the_same_root() {
        let root = temp_root("restart");
        let commit;
        {
            let store = DurableGitStore::rooted(&root);
            let repo = store.create_repo(&loc()).expect("create");
            commit = seed_commit(&repo, b"hello durable world\n");
            repo.update_ref_cas(
                "refs/heads/main",
                None,
                Some(&commit),
                "push: create main",
                "psn-7@acme.noreply",
            )
            .expect("create ref");
        }

        let store2 = DurableGitStore::rooted(&root);
        let repo2 = store2.open_repo(&loc()).expect("open after restart");
        assert_eq!(
            repo2.read_ref("refs/heads/main").expect("read ref"),
            Some(commit.clone()),
            "the ref survived the restart (SI-012 fixed - open loads from disk)"
        );
        let refs = repo2
            .read_refs_at_names(&[
                RefName::new("refs/heads/missing"),
                RefName::new("refs/heads/main"),
                RefName::new("refs/heads/main"),
            ])
            .expect("read an exact ref batch");
        assert_eq!(
            refs,
            vec![(RefName::new("refs/heads/main"), commit.clone())]
        );
        assert!(repo2.read_refs_at_names(&[RefName::new("main")]).is_err());
        assert!(
            repo2.has_object(&commit),
            "the commit object survived the restart (F-git-2 - on-disk odb)"
        );
        let bytes = repo2
            .read_object_bounded(&commit, 64 * 1024 * 1024)
            .expect("read object");
        assert!(
            std::str::from_utf8(&bytes)
                .unwrap()
                .contains("psn-7@acme.noreply"),
            "the durable commit carries the pseudonymous author"
        );
        assert_eq!(
            repo2
                .read_object_bounded(&commit, bytes.len())
                .expect("exact object read limit"),
            bytes
        );
        assert!(matches!(
            repo2.read_object_bounded(&commit, bytes.len() - 1),
            Err(DurableError::Git(message)) if message.starts_with("object read limit exceeded:")
        ));
        assert!(matches!(
            repo2.list_refs_bounded(0),
            Err(DurableError::Git(message))
                if message == "wire ref limit exceeded: direct ref count"
        ));
        assert_eq!(
            repo2.list_refs_bounded(WIRE_MAX_REFS).expect("list"),
            vec![("refs/heads/main".to_string(), commit)]
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn fork_import_moves_verified_commit_closure_without_source_refs() {
        let root = temp_root("fork-import");
        let store = DurableGitStore::rooted(&root);
        let source_loc = RepoLoc::new("acme", "fr-par", "contributor-fork");
        let target_loc = RepoLoc::new("acme", "fr-par", "core");
        let source = store.create_repo(&source_loc).expect("create source");
        let target = store.create_repo(&target_loc).expect("create target");
        assert_ne!(
            source.path(),
            target.path(),
            "the proof must use distinct ODBs"
        );

        let parent = seed_commit(&source, b"parent from fork\n");
        let child_blob = source
            .write_blob(b"locked fork head\n")
            .expect("child blob");
        let child_tree = source
            .write_tree(&[("file.txt", &child_blob)])
            .expect("child tree");
        let child = source
            .write_commit(
                &child_tree,
                &[&parent],
                "feat: fork head",
                "psn@acme.noreply",
                "psn@acme.noreply",
            )
            .expect("child commit");
        source
            .update_ref_cas(
                "refs/heads/contributor/change",
                None,
                Some(&child),
                "seed source branch",
                "psn@acme.noreply",
            )
            .expect("source ref");

        assert!(!target.has_object(&child));
        assert!(!target.has_object(&parent));
        target
            .import_commit_closure_from(&source, &child)
            .expect("verified non-thin import");

        assert!(target.object_is_commit(&child), "locked head imported");
        assert!(target.object_is_commit(&parent), "parent ancestry imported");
        assert!(
            target.has_object(&child_blob),
            "referenced tree/blob closure imported"
        );
        assert_eq!(
            target
                .read_ref("refs/heads/contributor/change")
                .expect("target ref read"),
            None,
            "object import must not copy or create source refs"
        );
        assert!(target
            .commit_tree_complete(&child)
            .expect("target connectivity"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn commit_log_and_diff_read_the_real_graph() {
        let root = temp_root("log");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");

        let b1 = repo.write_blob(b"line one\n").unwrap();
        let t1 = repo.write_tree(&[("file.txt", &b1)]).unwrap();
        let c1 = repo
            .write_commit(
                &t1,
                &[],
                "feat: first",
                "psn@acme.noreply",
                "psn@acme.noreply",
            )
            .unwrap();
        repo.update_ref_cas(
            "refs/heads/main",
            None,
            Some(&c1),
            "create",
            "psn@acme.noreply",
        )
        .unwrap();

        let b2 = repo.write_blob(b"line one\nline two\n").unwrap();
        let t2 = repo.write_tree(&[("file.txt", &b2)]).unwrap();
        let c2 = repo
            .write_commit(
                &t2,
                &[&c1],
                "feat: second",
                "psn@acme.noreply",
                "psn@acme.noreply",
            )
            .unwrap();
        repo.update_ref_cas(
            "refs/heads/main",
            Some(&c1),
            Some(&c2),
            "ff",
            "psn@acme.noreply",
        )
        .unwrap();

        let (rows, more) = repo.commit_log("refs/heads/main", 0, 10).expect("log");
        assert!(!more);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].oid, c2.0);
        assert_eq!(rows[0].summary, "feat: second");
        let metadata = repo
            .commit_meta_at_oids(&[
                c2.clone(),
                Oid::new("not-an-object-id"),
                c1.clone(),
                c2.clone(),
                Oid::new("0000000000000000000000000000000000000000"),
            ])
            .expect("exact commit metadata batch");
        assert_eq!(
            metadata
                .iter()
                .map(|commit| commit.summary.as_str())
                .collect::<Vec<_>>(),
            vec!["feat: second", "feat: first"]
        );
        assert_eq!(rows[0].parents, vec![c1.0.clone()]);
        assert_eq!(rows[1].oid, c1.0);

        let (p0, more0) = repo.commit_log("refs/heads/main", 0, 1).unwrap();
        assert!(more0 && p0.len() == 1 && p0[0].oid == c2.0);
        let (p1, more1) = repo.commit_log("refs/heads/main", 1, 1).unwrap();
        assert!(!more1 && p1.len() == 1 && p1[0].oid == c1.0);
        assert!(matches!(
            repo.commit_log("refs/heads/main", COMMIT_LOG_MAX_OFFSET + 1, 1),
            Err(DurableError::Git(message)) if message == "commit log pagination limit exceeded"
        ));
        assert!(matches!(
            repo.commit_log("refs/heads/main", 0, COMMIT_LOG_MAX_PAGE + 1),
            Err(DurableError::Git(message)) if message == "commit log pagination limit exceeded"
        ));

        let detail = repo.commit_detail(&c2.0).expect("detail").expect("present");
        assert_eq!(detail.meta.oid, c2.0);
        assert_eq!(detail.files.len(), 1);
        assert_eq!(detail.files[0].path, "file.txt");
        assert_eq!(detail.files[0].status, 'M');
        assert!(detail.files[0]
            .lines
            .iter()
            .any(|(o, c)| *o == '+' && c == "line two"));

        for limits in [
            CommitDiffLimits {
                files: 0,
                ..COMMIT_DIFF_LIMITS
            },
            CommitDiffLimits {
                lines_per_file: 0,
                ..COMMIT_DIFF_LIMITS
            },
            CommitDiffLimits {
                line_bytes: 1,
                ..COMMIT_DIFF_LIMITS
            },
            CommitDiffLimits {
                rendered_bytes: 1,
                ..COMMIT_DIFF_LIMITS
            },
            CommitDiffLimits {
                message_bytes: 1,
                ..COMMIT_DIFF_LIMITS
            },
        ] {
            assert!(matches!(
                repo.commit_detail_bounded(&c2.0, limits),
                Err(DurableError::Git(message)) if message.starts_with("commit diff computation limit exceeded:")
            ));
        }

        let root_detail = repo.commit_detail(&c1.0).unwrap().unwrap();
        assert_eq!(root_detail.files[0].status, 'A');

        assert!(repo.commit_detail("not-a-real-oid").unwrap().is_none());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn pr_commit_snapshot_pages_are_bounded_stable_and_expire_loudly() {
        let root = temp_root("pr-commit-pages");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");
        let blob = repo.write_blob(b"snapshot\n").unwrap();
        let tree = repo.write_tree(&[("file.txt", &blob)]).unwrap();
        let base = write_commit_at(&repo, &tree, &[], "base", 1_700_000_000);
        repo.update_ref_cas(
            "refs/heads/main",
            None,
            Some(&base),
            "base",
            "psn@acme.noreply",
        )
        .unwrap();
        let second = write_commit_at(&repo, &tree, &[&base], "second", 1_700_000_001);
        let third = write_commit_at(&repo, &tree, &[&second], "third", 1_700_000_002);
        let head = write_commit_at(&repo, &tree, &[&third], "head", 1_700_000_003);

        let snapshot = repo
            .pr_commit_snapshot("refs/heads/main", &head.0)
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.base_oid.as_deref(), Some(base.0.as_str()));
        assert_eq!(snapshot.head_oid, head.0);
        let exact_head_cap = repo
            .commits_in_pr_snapshot_with_graph_caps(
                snapshot.base_oid.as_deref(),
                &snapshot.head_oid,
                0,
                1,
                4,
                3,
            )
            .unwrap();
        assert_eq!(
            exact_head_cap.0[0].oid, head.0,
            "a head graph exactly at the preflight cap is admitted"
        );
        assert_eq!(
            repo.commits_in_pr_snapshot_with_graph_caps(
                snapshot.base_oid.as_deref(),
                &snapshot.head_oid,
                0,
                1,
                3,
                100,
            ),
            Err(PrCommitPageError::CapacityExceeded),
            "the cap+1 head row is observed and refused"
        );
        let no_base = repo
            .pr_commit_snapshot("refs/heads/not-created", &head.0)
            .unwrap()
            .unwrap();
        assert_eq!(no_base.base_oid, None);
        assert_eq!(
            repo.commits_in_pr_snapshot(None, &no_base.head_oid, 0, COMMIT_LOG_MAX_PAGE)
                .unwrap()
                .0
                .len(),
            4
        );

        let expected = [head.0.clone(), third.0.clone(), second.0.clone()];
        let mut seen = Vec::new();
        for position in 0..expected.len() {
            let (rows, has_more) = repo
                .commits_in_pr_snapshot(
                    snapshot.base_oid.as_deref(),
                    &snapshot.head_oid,
                    position,
                    1,
                )
                .unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(has_more, position + 1 < expected.len());
            seen.push(rows[0].oid.clone());
        }
        assert_eq!(seen, expected);
        let first_walk = repo
            .commits_in_pr_snapshot(
                snapshot.base_oid.as_deref(),
                &snapshot.head_oid,
                0,
                COMMIT_LOG_MAX_PAGE,
            )
            .unwrap();
        let repeated_walk = repo
            .commits_in_pr_snapshot(
                snapshot.base_oid.as_deref(),
                &snapshot.head_oid,
                0,
                COMMIT_LOG_MAX_PAGE,
            )
            .unwrap();
        assert_eq!(
            first_walk, repeated_walk,
            "a fixed snapshot walk is deterministic"
        );

        repo.update_ref_cas(
            "refs/heads/main",
            Some(&base),
            Some(&second),
            "advance base",
            "psn@acme.noreply",
        )
        .unwrap();
        let (repeat, _) = repo
            .commits_in_pr_snapshot(snapshot.base_oid.as_deref(), &snapshot.head_oid, 1, 1)
            .unwrap();
        assert_eq!(repeat[0].oid, third.0);

        assert_eq!(
            repo.commits_in_pr_snapshot(
                snapshot.base_oid.as_deref(),
                &snapshot.head_oid,
                PR_COMMIT_MAX_POSITION + 1,
                1,
            ),
            Err(PrCommitPageError::InvalidPagination)
        );
        assert_eq!(
            repo.commits_in_pr_snapshot(
                snapshot.base_oid.as_deref(),
                &snapshot.head_oid,
                0,
                COMMIT_LOG_MAX_PAGE + 1,
            ),
            Err(PrCommitPageError::InvalidPagination)
        );
        assert_eq!(
            repo.commits_in_pr_snapshot(None, &"f".repeat(40), 0, 1),
            Err(PrCommitPageError::SnapshotExpired)
        );
        assert_eq!(
            repo.commits_in_pr_snapshot(Some(&"e".repeat(40)), &snapshot.head_oid, 0, 1),
            Err(PrCommitPageError::SnapshotExpired)
        );
        assert_eq!(
            PR_COMMIT_MAX_PAGE_WALK_OBSERVATIONS,
            PR_COMMIT_MAX_POSITION + COMMIT_LOG_MAX_PAGE + 1
        );
        assert_eq!(
            PR_COMMIT_MAX_INTERNAL_WALK_NODES,
            2 * PR_COMMIT_MAX_GRAPH_NODES_PER_PIN
        );
        assert_eq!(
            PR_COMMIT_MAX_INTERNAL_WALK_EDGES,
            2 * PR_COMMIT_MAX_GRAPH_EDGES_PER_PIN
        );
        let at_cap = repo
            .commits_in_pr_snapshot(
                snapshot.base_oid.as_deref(),
                &snapshot.head_oid,
                PR_COMMIT_MAX_POSITION,
                COMMIT_LOG_MAX_PAGE,
            )
            .unwrap();
        assert!(at_cap.0.is_empty());
        assert!(!at_cap.1);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn pr_commit_snapshot_preflights_a_disjoint_base_at_the_same_hard_cap() {
        let root = temp_root("pr-commit-disjoint-base-cap");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");
        let blob = repo.write_blob(b"snapshot\n").unwrap();
        let tree = repo.write_tree(&[("file.txt", &blob)]).unwrap();
        let head = write_commit_at(&repo, &tree, &[], "head", 1_700_000_010);
        let base_one = write_commit_at(&repo, &tree, &[], "base one", 1_700_000_000);
        let base_two = write_commit_at(&repo, &tree, &[&base_one], "base two", 1_700_000_001);
        let base_three = write_commit_at(&repo, &tree, &[&base_two], "base three", 1_700_000_002);

        let exact_base_cap = repo
            .commits_in_pr_snapshot_with_graph_caps(Some(&base_three.0), &head.0, 0, 1, 3, 2)
            .unwrap();
        assert_eq!(
            exact_base_cap.0[0].oid, head.0,
            "a disjoint base graph exactly at the cap is admitted"
        );
        let base_four = write_commit_at(&repo, &tree, &[&base_three], "base four", 1_700_000_003);
        assert_eq!(
            repo.commits_in_pr_snapshot_with_graph_caps(Some(&base_four.0), &head.0, 0, 1, 3, 100,),
            Err(PrCommitPageError::CapacityExceeded),
            "a disjoint base cap+1 row is observed and refused"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn pr_commit_snapshot_caps_wide_frontiers_and_repeated_parent_edges() {
        let root = temp_root("pr-commit-graph-shape-caps");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");
        let blob = repo.write_blob(b"snapshot\n").unwrap();
        let tree = repo.write_tree(&[("file.txt", &blob)]).unwrap();

        let wide_one = write_commit_at(&repo, &tree, &[], "wide one", 1_700_000_000);
        let wide_two = write_commit_at(&repo, &tree, &[], "wide two", 1_700_000_001);
        let wide_three = write_commit_at(&repo, &tree, &[], "wide three", 1_700_000_002);
        let wide_head = write_commit_at(
            &repo,
            &tree,
            &[&wide_one, &wide_two, &wide_three],
            "wide head",
            1_700_000_003,
        );
        assert_eq!(
            repo.commits_in_pr_snapshot_with_graph_caps(None, &wide_head.0, 0, 1, 3, 100),
            Err(PrCommitPageError::CapacityExceeded),
            "the fourth unique OID is refused while discovering a wide parent frontier"
        );

        let root_commit = write_commit_at(&repo, &tree, &[], "root", 1_700_000_010);
        let left = write_commit_at(&repo, &tree, &[&root_commit], "left", 1_700_000_011);
        let right = write_commit_at(&repo, &tree, &[&root_commit], "right", 1_700_000_012);
        let dense_head =
            write_commit_at(&repo, &tree, &[&left, &right], "dense head", 1_700_000_013);
        assert!(repo
            .commits_in_pr_snapshot_with_graph_caps(None, &dense_head.0, 0, 1, 4, 4)
            .is_ok());
        assert_eq!(
            repo.commits_in_pr_snapshot_with_graph_caps(None, &dense_head.0, 0, 1, 4, 3),
            Err(PrCommitPageError::CapacityExceeded),
            "the repeated edge to the already-scheduled root counts toward aggregate edge work"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn pr_commit_snapshot_sorted_walk_setup_maps_not_found_races_to_expiry() {
        for context in [
            "pull-request revwalk push head",
            "pull-request revwalk hide base",
        ] {
            let missing = git2::Error::new(
                git2::ErrorCode::NotFound,
                git2::ErrorClass::Object,
                "object disappeared after preflight",
            );
            assert_eq!(
                pr_commit_walk_error(context, missing),
                PrCommitPageError::SnapshotExpired
            );
        }
    }

    #[test]
    fn pr_commit_snapshot_maps_a_missing_interior_object_to_expiry() {
        let root = temp_root("pr-commit-missing-interior");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");
        let blob = repo.write_blob(b"snapshot\n").unwrap();
        let tree = repo.write_tree(&[("file.txt", &blob)]).unwrap();
        let base = repo
            .write_commit(&tree, &[], "base", "psn@acme.noreply", "psn@acme.noreply")
            .unwrap();
        let interior = repo
            .write_commit(
                &tree,
                &[&base],
                "interior",
                "psn@acme.noreply",
                "psn@acme.noreply",
            )
            .unwrap();
        let head = repo
            .write_commit(
                &tree,
                &[&interior],
                "head",
                "psn@acme.noreply",
                "psn@acme.noreply",
            )
            .unwrap();

        let (directory, filename) = interior.0.split_at(2);
        std::fs::remove_file(repo.path().join("objects").join(directory).join(filename))
            .expect("remove loose interior commit object");
        assert_eq!(
            repo.commits_in_pr_snapshot(Some(&base.0), &head.0, 0, COMMIT_LOG_MAX_PAGE),
            Err(PrCommitPageError::SnapshotExpired)
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn pr_diff_is_three_dot_and_carries_line_numbers() {
        let root = temp_root("prdiff");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");

        let b0 = repo.write_blob(b"a\nb\nc\n").unwrap();
        let t0 = repo.write_tree(&[("file.txt", &b0)]).unwrap();
        let base = repo
            .write_commit(&t0, &[], "base", "psn@acme.noreply", "psn@acme.noreply")
            .unwrap();
        repo.update_ref_cas(
            "refs/heads/main",
            None,
            Some(&base),
            "create",
            "psn@acme.noreply",
        )
        .unwrap();

        let bh = repo.write_blob(b"a\nB\nc\nd\n").unwrap();
        let th = repo.write_tree(&[("file.txt", &bh)]).unwrap();
        let head = repo
            .write_commit(
                &th,
                &[&base],
                "head",
                "psn@acme.noreply",
                "psn@acme.noreply",
            )
            .unwrap();

        let bd = repo.write_blob(b"unrelated\n").unwrap();
        let td = repo
            .write_tree(&[("file.txt", &b0), ("other.txt", &bd)])
            .unwrap();
        let drift = repo
            .write_commit(
                &td,
                &[&base],
                "drift on main",
                "psn@acme.noreply",
                "psn@acme.noreply",
            )
            .unwrap();
        repo.update_ref_cas(
            "refs/heads/main",
            Some(&base),
            Some(&drift),
            "ff",
            "psn@acme.noreply",
        )
        .unwrap();

        let diff = repo
            .pr_diff("refs/heads/main", &head.0, 4000)
            .unwrap()
            .unwrap();
        assert!(
            diff.three_dot,
            "durable repos are libgit2-backed → real merge-base"
        );
        assert_eq!(
            diff.base_oid, base.0,
            "base = merge-base(main, head), NOT main's tip"
        );
        assert_eq!(
            diff.total_files, 1,
            "three-dot shows only the PR's own files"
        );
        assert_eq!(diff.files[0].path, "file.txt");
        assert_eq!(diff.files[0].status, 'M');
        assert_eq!(diff.files[0].kind, FileKind::Text);
        assert_eq!(diff.files[0].new_blob_oid.as_deref(), Some(bh.0.as_str()));
        let FileLinesLookup::Found(expanded) = repo
            .file_lines(diff.files[0].new_blob_oid.as_deref().unwrap(), 2, 3)
            .unwrap()
        else {
            panic!("the projected new-side blob oid must feed the bounded context reader")
        };
        assert_eq!(
            expanded
                .iter()
                .map(|line| line.content.as_str())
                .collect::<Vec<_>>(),
            ["B", "c"]
        );
        assert_eq!(
            diff.files[0].additions, 2,
            "line 2 changed (+B) + line 4 added (+d)"
        );
        assert_eq!(diff.files[0].deletions, 1, "line 2's old (-b)");
        let hunk = &diff.files[0].hunks[0];
        let added_d = hunk
            .lines
            .iter()
            .find(|l| l.origin == '+' && l.content == "d")
            .unwrap();
        assert_eq!(added_d.new_no, Some(4));
        assert_eq!(added_d.old_no, None);
        let removed_b = hunk
            .lines
            .iter()
            .find(|l| l.origin == '-' && l.content == "b")
            .unwrap();
        assert_eq!(removed_b.old_no, Some(2));
        assert_eq!(removed_b.new_no, None);
        let ctx_a = hunk
            .lines
            .iter()
            .find(|l| l.origin == ' ' && l.content == "a")
            .unwrap();
        assert_eq!(ctx_a.old_no, Some(1));
        assert_eq!(ctx_a.new_no, Some(1));

        assert!(matches!(
            repo.pr_diff_bounded(
                "refs/heads/main",
                &head.0,
                4000,
                0,
                DIFF_MAX_LINE_BYTES,
                DIFF_MAX_RENDERED_BYTES,
            ),
            Err(DurableError::Git(message)) if message.starts_with("pr diff computation limit exceeded:")
        ));
        for (line_bytes, rendered_bytes) in [(0, DIFF_MAX_RENDERED_BYTES), (DIFF_MAX_LINE_BYTES, 1)]
        {
            assert!(matches!(
                repo.pr_diff_bounded(
                    "refs/heads/main",
                    &head.0,
                    4000,
                    PR_DIFF_MAX_FILES,
                    line_bytes,
                    rendered_bytes,
                ),
                Err(DurableError::Git(message)) if message.starts_with("pr diff computation limit exceeded:")
            ));
        }

        assert!(repo
            .pr_diff("refs/heads/main", "not-an-oid", 4000)
            .unwrap()
            .is_none());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn pr_diff_caps_a_single_huge_hunk_not_only_at_hunk_boundaries() {
        let root = temp_root("prcap");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");

        let t0 = repo.write_tree(&[]).unwrap();
        let base = repo
            .write_commit(&t0, &[], "base", "psn@acme.noreply", "psn@acme.noreply")
            .unwrap();
        repo.update_ref_cas(
            "refs/heads/main",
            None,
            Some(&base),
            "create",
            "psn@acme.noreply",
        )
        .unwrap();

        let big: String = (0..5000).map(|i| format!("line {i}\n")).collect();
        let bh = repo.write_blob(big.as_bytes()).unwrap();
        let th = repo.write_tree(&[("big.txt", &bh)]).unwrap();
        let head = repo
            .write_commit(
                &th,
                &[&base],
                "add big",
                "psn@acme.noreply",
                "psn@acme.noreply",
            )
            .unwrap();

        let cap = 100;
        let diff = repo
            .pr_diff("refs/heads/main", &head.0, cap)
            .unwrap()
            .unwrap();
        assert_eq!(diff.total_files, 1);
        let f = &diff.files[0];
        assert_eq!(f.path, "big.txt");
        assert!(f.truncated, "a file over the cap MUST be flagged truncated");
        let rendered: usize = f.hunks.iter().map(|h| h.lines.len()).sum();
        assert!(
            rendered <= cap,
            "rendered lines ({rendered}) must not exceed the cap ({cap})"
        );
        assert_eq!(
            f.additions, 5000,
            "the diffstat still reports the TRUE addition count"
        );
        let full = repo
            .pr_diff("refs/heads/main", &head.0, 0)
            .unwrap()
            .unwrap();
        let full_rendered: usize = full.files[0].hunks.iter().map(|h| h.lines.len()).sum();
        assert_eq!(full_rendered, 5000, "cap=0 is uncapped");
        assert!(!full.files[0].truncated);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn pr_diff_flags_binary_with_no_hunk_dump() {
        let root = temp_root("prbin");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");
        let base = seed_commit(&repo, b"a\n");
        repo.update_ref_cas(
            "refs/heads/main",
            None,
            Some(&base),
            "c",
            "psn@acme.noreply",
        )
        .unwrap();
        let bin = repo.write_blob(&[0u8, 1, 2, 0, 255, 3]).unwrap();
        let tb = repo.write_tree(&[("logo.png", &bin)]).unwrap();
        let head = repo
            .write_commit(
                &tb,
                &[&base],
                "add binary",
                "psn@acme.noreply",
                "psn@acme.noreply",
            )
            .unwrap();
        let diff = repo
            .pr_diff("refs/heads/main", &head.0, 4000)
            .unwrap()
            .unwrap();
        let f = diff.files.iter().find(|f| f.path == "logo.png").unwrap();
        assert_eq!(f.kind, FileKind::Binary);
        assert_eq!(f.new_blob_oid.as_deref(), Some(bin.0.as_str()));
        assert!(f.hunks.is_empty(), "a binary file carries NO text hunks");
        assert!(
            f.size_bytes.is_some(),
            "the size is available for the binary row"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn pr_diff_omits_new_blob_oid_for_a_deletion() {
        let root = temp_root("prdeletedoid");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");
        let base = seed_commit(&repo, b"removed\n");
        repo.update_ref_cas(
            "refs/heads/main",
            None,
            Some(&base),
            "c",
            "psn@acme.noreply",
        )
        .unwrap();
        let empty = repo.write_tree(&[]).unwrap();
        let head = repo
            .write_commit(
                &empty,
                &[&base],
                "delete file",
                "psn@acme.noreply",
                "psn@acme.noreply",
            )
            .unwrap();
        let diff = repo
            .pr_diff("refs/heads/main", &head.0, 4000)
            .unwrap()
            .unwrap();
        assert_eq!(diff.files[0].status, 'D');
        assert_eq!(diff.files[0].new_blob_oid, None);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn file_lines_expands_context_and_is_binary_safe() {
        let root = temp_root("filelines");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");
        let blob = repo.write_blob(b"one\ntwo\nthree\nfour\nfive\n").unwrap();
        let FileLinesLookup::Found(lines) = repo.file_lines(&blob.0, 2, 4).unwrap() else {
            panic!("text blob must return lines")
        };
        assert_eq!(lines.len(), 3, "lines 2..=4");
        assert_eq!(lines[0].content, "two");
        assert_eq!(lines[0].new_no, Some(2));
        assert_eq!(lines[2].content, "four");

        let exact_body = (1..=FILE_LINES_MAX_RANGE)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        let exact_blob = repo.write_blob(exact_body.as_bytes()).unwrap();
        let FileLinesLookup::Found(exact_lines) = repo
            .file_lines(&exact_blob.0, 1, FILE_LINES_MAX_RANGE)
            .unwrap()
        else {
            panic!("the exact line-range cap must remain valid")
        };
        assert_eq!(exact_lines.len(), FILE_LINES_MAX_RANGE);
        assert_eq!(
            exact_lines.last().unwrap().new_no,
            Some(FILE_LINES_MAX_RANGE as u32)
        );
        assert_eq!(
            repo.file_lines("not-an-oid", 1, 10).unwrap(),
            FileLinesLookup::Missing,
        );
        let bin = repo.write_blob(&[0u8, 1, 2, 0]).unwrap();
        assert_eq!(
            repo.file_lines(&bin.0, 1, 10).unwrap(),
            FileLinesLookup::Binary
        );

        let large = repo
            .write_blob(&vec![b'x'; FILE_LINES_MAX_BLOB_BYTES + 1])
            .unwrap();
        assert!(matches!(
            repo.file_lines(&large.0, 1, 10).unwrap(),
            FileLinesLookup::TooLarge { .. }
        ));
        assert!(repo.file_lines(&blob.0, 0, 1).is_err());
        assert!(repo.file_lines(&blob.0, 2, 1).is_err());
        assert!(repo
            .file_lines(&blob.0, 1, FILE_LINES_MAX_RANGE + 1)
            .is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn fsck_is_clean_on_a_well_formed_repo() {
        let root = temp_root("fsck");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");
        let commit = seed_commit(&repo, b"fsck me\n");
        repo.update_ref_cas(
            "refs/heads/main",
            None,
            Some(&commit),
            "create",
            "psn@acme.noreply",
        )
        .expect("ref");
        repo.fsck().expect("fsck clean on a valid repo");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn ref_cas_rejects_a_stale_expected_old() {
        let root = temp_root("cas");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");
        let c1 = seed_commit(&repo, b"v1\n");
        let blob2 = repo.write_blob(b"v2\n").unwrap();
        let tree2 = repo.write_tree(&[("file.txt", &blob2)]).unwrap();
        let c2 = repo
            .write_commit(&tree2, &[&c1], "v2", "psn@acme.noreply", "psn@acme.noreply")
            .unwrap();

        repo.update_ref_cas(
            "refs/heads/main",
            None,
            Some(&c1),
            "create",
            "psn@acme.noreply",
        )
        .expect("create");

        let stale = repo.update_ref_cas(
            "refs/heads/main",
            None,
            Some(&c2),
            "stale create",
            "psn@acme.noreply",
        );
        assert!(
            matches!(stale, Err(DurableError::CasMismatch { .. })),
            "a stale expected-old is rejected, got {stale:?}"
        );
        assert_eq!(repo.read_ref("refs/heads/main").unwrap(), Some(c1.clone()));

        repo.update_ref_cas(
            "refs/heads/main",
            Some(&c1),
            Some(&c2),
            "ff to v2",
            "psn@acme.noreply",
        )
        .expect("ff update");
        assert_eq!(repo.read_ref("refs/heads/main").unwrap(), Some(c2));
        assert_eq!(
            repo.reflog_len("refs/heads/main"),
            Ok(2),
            "two updates logged"
        );
        assert!(matches!(
            repo.reflog_entries_bounded("refs/heads/main", 1, REFLOG_MAX_BYTES_PER_REF),
            Err(DurableError::Git(message))
                if message == "audit reflog limit exceeded: entry count"
        ));
        assert_eq!(
            repo.reflog_entries_bounded("refs/heads/main", 2, REFLOG_MAX_BYTES_PER_REF)
                .expect("bounded reflog")
                .0
                .len(),
            2
        );
        assert!(matches!(
            repo.reflog_entries_bounded("refs/heads/main", 2, 1),
            Err(DurableError::Git(message))
                if message == "audit reflog limit exceeded: on-disk bytes"
        ));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn reflog_operation_lookup_keeps_the_original_match() {
        let root = temp_root("reflog-operation");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");
        let blob = repo.write_blob(b"versioned\n").unwrap();
        let tree = repo.write_tree(&[("file.txt", &blob)]).unwrap();
        let trailer = "Myelin-Operation: opaque-digest";
        let first = repo
            .write_commit(
                &tree,
                &[],
                &format!("first\n\n{trailer}"),
                "psn@acme.noreply",
                "psn@acme.noreply",
            )
            .unwrap();
        let later = repo
            .write_commit(
                &tree,
                &[&first],
                &format!("later copy\n\n{trailer}"),
                "psn@acme.noreply",
                "psn@acme.noreply",
            )
            .unwrap();
        repo.update_ref_cas(
            "refs/heads/main",
            None,
            Some(&first),
            "first",
            "psn@acme.noreply",
        )
        .unwrap();
        repo.update_ref_cas(
            "refs/heads/main",
            Some(&first),
            Some(&later),
            "later",
            "psn@acme.noreply",
        )
        .unwrap();

        let found = repo
            .find_reflog_commit_by_trailer("refs/heads/main", trailer)
            .unwrap()
            .unwrap();
        assert_eq!(
            found.oid, first,
            "a copied trailer cannot shadow its origin"
        );
        assert!(repo
            .find_reflog_commit_by_trailer("refs/heads/main", "Myelin-Operation: opaque")
            .unwrap()
            .is_none());

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn tenant_isolation_by_path() {
        let root = temp_root("isolation");
        let store = DurableGitStore::rooted(&root);
        let a = RepoLoc::new("tenant-a", "fr-par", "secret");
        let b = RepoLoc::new("tenant-b", "fr-par", "secret");

        let repo_a = store.create_repo(&a).expect("create a");
        let commit = seed_commit(&repo_a, b"tenant a private\n");
        repo_a
            .update_ref_cas(
                "refs/heads/main",
                None,
                Some(&commit),
                "create",
                "psn@tenant-a.noreply",
            )
            .expect("ref a");

        assert_ne!(store.repo_path(&a).unwrap(), store.repo_path(&b).unwrap());
        assert!(store.repo_exists(&a));
        assert!(
            !store.repo_exists(&b),
            "tenant B cannot reach A's repo by path"
        );
        let repo_b = store.create_repo(&b).expect("create b");
        assert!(
            !repo_b.has_object(&commit),
            "tenant A's object is NOT in tenant B's on-disk odb (path isolation)"
        );
        assert_eq!(
            repo_b.read_ref("refs/heads/main").unwrap(),
            None,
            "tenant B's main is empty - A's ref did not bleed across the tenant path"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn delete_cas_removes_ref_but_does_not_reset_durable_generation() {
        let root = temp_root("delete");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");
        let c1 = seed_commit(&repo, b"x\n");
        repo.update_ref_cas(
            "refs/heads/tmp",
            None,
            Some(&c1),
            "create",
            "psn@acme.noreply",
        )
        .unwrap();
        assert_eq!(repo.read_ref("refs/heads/tmp").unwrap(), Some(c1.clone()));
        assert_eq!(
            repo.ref_generation("refs/heads/tmp"),
            Ok(1),
            "create is generation 1"
        );

        repo.update_ref_cas(
            "refs/heads/tmp",
            Some(&c1),
            None,
            "delete",
            "psn@acme.noreply",
        )
        .expect("delete");
        assert_eq!(
            repo.read_ref("refs/heads/tmp").unwrap(),
            None,
            "ref deleted"
        );
        assert_eq!(
            repo.ref_generation("refs/heads/tmp"),
            Ok(2),
            "the delete ADVANCES the durable generation (a delete is a generation-advancing event)"
        );

        repo.update_ref_cas(
            "refs/heads/tmp",
            None,
            Some(&c1),
            "recreate",
            "psn@acme.noreply",
        )
        .expect("recreate");
        assert_eq!(
            repo.reflog_len("refs/heads/tmp"),
            Ok(1),
            "the recreated ref's reflog restarts (libgit2 behaviour - why reflog_len was wrong)"
        );
        assert_eq!(
            repo.ref_generation("refs/heads/tmp"),
            Ok(3),
            "the durable per-ref generation is monotonic across delete+recreate (R0.4 fix)"
        );

        drop(repo);
        let store2 = DurableGitStore::rooted(&root);
        let repo2 = store2.open_repo(&loc()).expect("reopen");
        assert_eq!(
            repo2.ref_generation("refs/heads/tmp"),
            Ok(3),
            "the durable generation survives a process restart (config is on disk)"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn ref_generation_distinguishes_absent_from_corrupt_config() {
        let root = temp_root("refgen-corrupt");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");
        let ref_name = "refs/heads/main";
        assert_eq!(
            repo.ref_generation(ref_name),
            Ok(0),
            "an absent counter starts at zero"
        );

        let raw = repo.open_git().expect("open raw repo");
        let mut cfg = raw.config().expect("open config");
        cfg.set_str(&refgen_key(ref_name), "not-an-integer")
            .expect("write malformed fixture");
        assert!(
            matches!(repo.ref_generation(ref_name), Err(DurableError::Git(_))),
            "a malformed counter must not be treated as generation zero"
        );

        cfg.set_i64(&refgen_key(ref_name), -1)
            .expect("write negative fixture");
        assert_eq!(
            repo.ref_generation(ref_name),
            Err(DurableError::Git(format!(
                "negative ref generation stored for {ref_name}"
            )))
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn exhausted_ref_generation_rejects_before_moving_ref() {
        let root = temp_root("refgen-exhausted");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");
        let commit = seed_commit(&repo, b"capacity\n");
        let ref_name = "refs/heads/main";
        let raw = repo.open_git().expect("open raw repo");
        raw.config()
            .expect("open config")
            .set_i64(&refgen_key(ref_name), i64::MAX)
            .expect("write exhausted fixture");

        let result = repo.update_ref_cas(
            ref_name,
            None,
            Some(&commit),
            "must not move",
            "psn@acme.noreply",
        );
        assert!(
            matches!(result, Err(DurableError::Git(message)) if message.contains("generation exhausted")),
            "generation exhaustion must be loud"
        );
        assert_eq!(
            repo.read_ref(ref_name).unwrap(),
            None,
            "the ref must remain unchanged when its recovery fence cannot advance"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn reflog_entries_reject_non_utf8_audit_identity() {
        let root = temp_root("reflog-identity-corrupt");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");
        let commit = seed_commit(&repo, b"audit\n");
        let pseudonym = b"psn@acme.noreply";
        repo.update_ref_cas(
            "refs/heads/main",
            None,
            Some(&commit),
            "create",
            std::str::from_utf8(pseudonym).unwrap(),
        )
        .expect("create ref");

        let path = repo.path().join("logs/refs/heads/main");
        let mut bytes = std::fs::read(&path).expect("read reflog fixture");
        let offset = bytes
            .windows(pseudonym.len())
            .position(|window| window == pseudonym)
            .expect("pseudonym recorded in reflog");
        bytes[offset] = 0xff;
        std::fs::write(&path, bytes).expect("corrupt reflog identity fixture");

        assert!(
            matches!(
                repo.reflog_entries_bounded("refs/heads/main", 10, REFLOG_MAX_BYTES_PER_REF),
                Err(DurableError::Git(_))
            ),
            "invalid audit identity bytes must not become an empty committer"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    fn copy_object(src: &DurableGitRepo, dst: &DurableGitRepo, oid: &Oid, kind: &str) {
        let bytes = src
            .read_object_bounded(oid, 64 * 1024 * 1024)
            .expect("read src object");
        let written = dst
            .write_raw_object(kind, &bytes)
            .expect("write dst object");
        assert_eq!(written.0, oid.0, "the re-hashed copy keeps the same oid");
    }

    #[allow(clippy::type_complexity)]
    fn seed_three_commit_history() -> (PathBuf, DurableGitRepo, Vec<(Oid, Oid, Oid)>) {
        let root = temp_root("conn-src");
        let repo = DurableGitStore::rooted(&root)
            .create_repo(&loc())
            .expect("create src");
        let mut chain: Vec<(Oid, Oid, Oid)> = Vec::new();
        let mut parent: Option<Oid> = None;
        for i in 0..3u8 {
            let blob = repo.write_blob(format!("line {i}\n").as_bytes()).unwrap();
            let tree = repo.write_tree(&[("file.txt", &blob)]).unwrap();
            let parents: Vec<&Oid> = parent.iter().collect();
            let commit = repo
                .write_commit(
                    &tree,
                    &parents,
                    &format!("c{i}"),
                    "psn@acme.noreply",
                    "psn@acme.noreply",
                )
                .unwrap();
            parent = Some(commit.clone());
            chain.push((blob, tree, commit));
        }
        (root, repo, chain)
    }

    #[test]
    fn history_connectivity_rejects_a_missing_ancestor_commit() {
        let (src_root, src, chain) = seed_three_commit_history();
        let (b1, t1, c1) = chain[0].clone();
        let (b2, t2, c2) = chain[1].clone();
        let (b3, t3, c3) = chain[2].clone();

        let dst_root = temp_root("conn-dst-missing");
        let dst = DurableGitStore::rooted(&dst_root)
            .create_repo(&loc())
            .expect("create dst");
        copy_object(&src, &dst, &b1, "blob");
        copy_object(&src, &dst, &t1, "tree");
        copy_object(&src, &dst, &b2, "blob");
        copy_object(&src, &dst, &t2, "tree");
        copy_object(&src, &dst, &b3, "blob");
        copy_object(&src, &dst, &t3, "tree");
        copy_object(&src, &dst, &c2, "commit");
        copy_object(&src, &dst, &c3, "commit");
        assert!(
            !dst.has_object(&c1),
            "the ancestor commit is absent from the target odb"
        );

        assert!(
            dst.commit_tree_complete(&c3).unwrap(),
            "the tip-only check ACCEPTS - the tip's tree is complete (this is the hole)"
        );
        assert!(
            !dst.history_connectivity_complete(&c3, &[]).unwrap(),
            "R0.7-D: a missing ANCESTOR commit rejects the push (fail-closed) - the ref must not move"
        );

        std::fs::remove_dir_all(&src_root).ok();
        std::fs::remove_dir_all(&dst_root).ok();
    }

    #[test]
    fn history_connectivity_accepts_full_history() {
        let (src_root, src, chain) = seed_three_commit_history();
        let dst_root = temp_root("conn-dst-full");
        let dst = DurableGitStore::rooted(&dst_root)
            .create_repo(&loc())
            .expect("create dst");
        for (b, t, c) in &chain {
            copy_object(&src, &dst, b, "blob");
            copy_object(&src, &dst, t, "tree");
            copy_object(&src, &dst, c, "commit");
        }
        let c3 = &chain[2].2;
        assert!(
            dst.history_connectivity_complete(c3, &[]).unwrap(),
            "a fully self-contained new history is ACCEPTED (branch create)"
        );
        std::fs::remove_dir_all(&src_root).ok();
        std::fs::remove_dir_all(&dst_root).ok();
    }

    #[test]
    fn history_connectivity_thin_push_hides_existing_tips() {
        let (src_root, src, chain) = seed_three_commit_history();
        let dst_root = temp_root("conn-dst-thin");
        let dst = DurableGitStore::rooted(&dst_root)
            .create_repo(&loc())
            .expect("create dst");
        for (b, t, c) in &chain {
            copy_object(&src, &dst, b, "blob");
            copy_object(&src, &dst, t, "tree");
            copy_object(&src, &dst, c, "commit");
        }
        let c2 = chain[1].2.clone();
        let c3 = chain[2].2.clone();

        assert!(
            dst.history_connectivity_complete(&c3, std::slice::from_ref(&c2))
                .unwrap(),
            "a thin push onto a present base tip is accepted (only the delta is walked)"
        );
        let bogus = Oid::new("0".repeat(39) + "1");
        assert!(
            dst.history_connectivity_complete(&c3, &[c2, bogus])
                .unwrap(),
            "hiding a non-existent existing tip is graceful (skipped, never an error)"
        );
        std::fs::remove_dir_all(&src_root).ok();
        std::fs::remove_dir_all(&dst_root).ok();
    }

    #[test]
    fn history_connectivity_rejects_missing_parent_of_a_new_commit_even_with_existing_tips() {
        let (src_root, src, chain) = seed_three_commit_history();
        let (b2, t2, c2) = chain[1].clone();
        let (b3, t3, c3) = chain[2].clone();

        let dst_root = temp_root("conn-dst-thin-missing");
        let dst = DurableGitStore::rooted(&dst_root)
            .create_repo(&loc())
            .expect("create dst");
        copy_object(&src, &dst, &b2, "blob");
        copy_object(&src, &dst, &t2, "tree");
        copy_object(&src, &dst, &b3, "blob");
        copy_object(&src, &dst, &t3, "tree");
        copy_object(&src, &dst, &c3, "commit");
        assert!(
            !dst.has_object(&c2),
            "the new tip's parent commit is absent"
        );

        assert!(
            !dst.history_connectivity_complete(&c3, &[c2]).unwrap(),
            "a new commit whose parent is genuinely absent is rejected regardless of existing_tips"
        );
        assert!(
            !dst.history_connectivity_complete(&Oid::new("0".repeat(39) + "1"), &[])
                .unwrap(),
            "a new_tip that is not a present commit is rejected (fail-closed)"
        );
        std::fs::remove_dir_all(&src_root).ok();
        std::fs::remove_dir_all(&dst_root).ok();
    }

    fn seed_nested_repo(root: &std::path::Path) -> DurableGitRepo {
        let store = DurableGitStore::rooted(root);
        let repo = store.create_repo(&loc()).expect("create");
        let git = git2::Repository::open_bare(repo.path()).expect("open bare");

        let readme = git.blob(b"# nested repo\n\nhello\n").unwrap();
        let deep = git.blob(b"pub fn deep() {}\n").unwrap();
        let binary = git
            .blob(&[0x89, b'P', b'N', b'G', 0x00, 0x01, 0x02, 0x00, 0xff])
            .unwrap();

        let mut b = git.treebuilder(None).unwrap();
        b.insert("deep.rs", deep, 0o100644).unwrap();
        let inner = b.write().unwrap();
        let mut b = git.treebuilder(None).unwrap();
        b.insert("inner", inner, 0o040000).unwrap();
        let crates = b.write().unwrap();
        let mut b = git.treebuilder(None).unwrap();
        b.insert("logo.bin", binary, 0o100644).unwrap();
        let assets = b.write().unwrap();
        let mut b = git.treebuilder(None).unwrap();
        b.insert("README.md", readme, 0o100644).unwrap();
        b.insert("crates", crates, 0o040000).unwrap();
        b.insert("assets", assets, 0o040000).unwrap();
        let root_tree = b.write().unwrap();

        let sig = git2::Signature::now("psn-7@acme.noreply", "psn-7@acme.noreply").unwrap();
        let tree_obj = git.find_tree(root_tree).unwrap();
        let commit = git
            .commit(None, &sig, &sig, "feat: nested seed", &tree_obj, &[])
            .unwrap();
        repo.update_ref_cas(
            "refs/heads/main",
            None,
            Some(&Oid::new(commit.to_string())),
            "seed main",
            "psn-7@acme.noreply",
        )
        .expect("set main");
        repo.update_ref_cas(
            "refs/tags/v1.0",
            None,
            Some(&Oid::new(commit.to_string())),
            "tag v1.0",
            "psn-7@acme.noreply",
        )
        .expect("set tag");
        repo
    }

    #[test]
    fn ref_resolving_to_a_non_commit_object_is_a_clean_empty_browse_not_an_err() {
        let root = temp_root("noncommit-ref");
        let repo = seed_nested_repo(&root);
        let tree_spec = "main^{tree}";
        assert!(matches!(
            repo.read_blob_at_path_bounded(tree_spec, "README.md", 1024)
                .unwrap(),
            BlobPathLookup::Missing
        ));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn read_blob_at_path_detects_binary_and_flags_a_dir() {
        let root = temp_root("blob-nested");
        let repo = seed_nested_repo(&root);

        let BlobPathLookup::Found {
            bytes,
            oid,
            is_binary,
            size,
        } = repo
            .read_blob_at_path_bounded("main", "crates/inner/deep.rs", 1024)
            .unwrap()
        else {
            panic!("deep.rs is a blob");
        };
        assert!(!is_binary, "a text file is not binary");
        assert_eq!(size as usize, bytes.len());
        assert!(String::from_utf8_lossy(&bytes).contains("deep"));
        assert_eq!(
            repo.blob_oid_at_path("main", "crates/inner/deep.rs")
                .unwrap(),
            Some(oid)
        );
        assert!(matches!(
            repo.read_blob_at_path_bounded("main", "crates/inner/deep.rs", 1).unwrap(),
            BlobPathLookup::TooLarge { size, maximum: 1, oid } if size > 1 && oid.as_str().len() == 40
        ));

        let BlobPathLookup::Found { is_binary, .. } = repo
            .read_blob_at_path_bounded("main", "assets/logo.bin", 1024)
            .unwrap()
        else {
            panic!("logo.bin is a blob");
        };
        assert!(
            is_binary,
            "a file with NUL bytes is detected binary server-side"
        );

        assert!(matches!(
            repo.read_blob_at_path_bounded("main", "crates/inner", 1024)
                .unwrap(),
            BlobPathLookup::IsDir
        ));
        assert!(matches!(
            repo.read_blob_at_path_bounded("main", "", 1024).unwrap(),
            BlobPathLookup::IsDir
        ));
        assert!(matches!(
            repo.read_blob_at_path_bounded("main", "no/such/file", 1024)
                .unwrap(),
            BlobPathLookup::Missing
        ));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn nested_path_traversal_is_contained() {
        let root = temp_root("traversal");
        let repo = seed_nested_repo(&root);
        for escape in [
            "../../../etc/passwd",
            "crates/../../etc/passwd",
            "crates/inner/../../../../secret",
            "/etc/passwd",
        ] {
            if escape.contains("etc/passwd") || escape.contains("secret") {
                assert!(
                    matches!(
                        repo.read_blob_at_path_bounded("main", escape, 1024)
                            .unwrap(),
                        BlobPathLookup::Missing
                    ),
                    "traversal `{escape}` must not read host bytes"
                );
            }
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn commit_metadata_truncates_oversized_summary_at_utf8_boundary() {
        let root = temp_root("commit-meta-summary");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");
        let blob = repo.write_blob(b"x\n").expect("blob");
        let tree = repo.write_tree(&[("x.txt", &blob)]).expect("tree");
        let message = format!("{}é\nbody", "x".repeat(COMMIT_META_MAX_SUMMARY_BYTES - 1));
        let commit = repo
            .write_commit(&tree, &[], &message, "psn@acme.noreply", "psn@acme.noreply")
            .expect("commit");
        repo.update_ref_cas(
            "refs/heads/main",
            None,
            Some(&commit),
            "create",
            "psn@acme.noreply",
        )
        .expect("create ref");

        let meta = repo
            .commit_log("refs/heads/main", 0, 1)
            .unwrap()
            .0
            .remove(0);
        assert_eq!(meta.summary.len(), COMMIT_META_MAX_SUMMARY_BYTES - 1);
        assert!(meta.summary.is_char_boundary(meta.summary.len()));

        std::fs::remove_dir_all(&root).ok();
    }
}
