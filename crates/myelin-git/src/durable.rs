//! # `durable` — REAL on-disk bare git repos: the durable STORAGE floor (GT-001 / E1.1)
//!
//! The genuinely-durable storage organ the rest of the Git track sits on. Today the
//! [`crate::receive_pack::RefStore`] kept refs + reflog in an in-memory `Mutex<BTreeMap>` whose
//! `open` loaded NOTHING (census SI-012 — the entry point of every repo lost on restart), and the
//! object/pack index was an in-memory `oid → hash` map rebuilt on open (F-git-2). This module makes
//! the WRITE / ref / repo-lifecycle path durable on the **same on-disk model the READ path already
//! uses** ([`crate::gix_backend::GixCore`] opens real bare repos via `git2::Repository::open`):
//!
//! - **Repo lifecycle on disk** — repo creation is `git2::Repository::init_bare` at the resolver
//!   path `<root>/<tenant>/<region>/<repo>.git`; existence/open loads the real bare repo.
//! - **Durable refs + reflog** — ref reads / writes / **compare-and-swap** go to the real on-disk
//!   repo's refs via `git2` (`reference` / `reference_matching` / `find_reference`), and a FRESH
//!   handle over the same on-disk root sees them (no empty map on open). The reflog is the real
//!   on-disk git reflog (`core.logallrefupdates`).
//! - **Durable objects** — object writes/reads go to the on-disk repo's **odb** (`git2`); the
//!   `oid → object` lookup IS the real on-disk odb (no in-memory index rebuilt on open).
//! - **Tenant/region pathing is the isolation boundary** — a repo lives under its `tenant/region`
//!   and is never reachable cross-tenant by path (the resolver mints the path from the locator).
//!
//! ## Anti-duplication — REUSE git2 + the read resolver, never reimplement git
//! This module does NOT reimplement git objects/refs/packing. It REUSES:
//! - [`crate::gix_backend::RepoPathResolver`] / [`crate::gix_backend::RootedResolver`] — the exact
//!   `<tenant>/<region>/<repo>.git` path mapping the read backend ([`crate::gix_backend::GixCore`])
//!   already resolves against, so the write/lifecycle path and the read path open the SAME repo.
//! - `git2` (libgit2) — the architecture-named in-process backend (gix-preferred is the OQ-1 floor,
//!   GIT-P33). Safe Rust API only; the crate stays `#![forbid(unsafe_code)]`.
//!
//! ## Scope — git object durability vs the generic blob tier (be precise, prompt §3)
//! The **git object tier** is the on-disk **odb** ([`DurableGitRepo::write_blob`] /
//! [`DurableGitRepo::read_object_bounded`]): real `fs`-backed git objects, `git fsck`-clean, survive
//! restart.
//! The generic content-addressed [`myelin_storage::FsBlobStore`] (the `Mutex<HashMap>` byte tier the
//! [`crate::pack_tier`] rides) is a SEPARATE track — its real on-disk/object-store byte backing is
//! P-ST-30 (census SI-014/015/029), already carried in the `no-in-memory-durable-store` baseline.
//! GT-001 fixes the **git object durability** here; the generic blob tier's backing swap is not this
//! prompt (and `myelin-git` is out of the spine lint's scan scope, so this module changes no ratchet).
//!
//! The smart-transport WIRE (`clone`/`push` over the network) is **GT-006** (sandbox-gated) — NOT
//! this module. This is the durable STORAGE the wire (and the API/UI/CLI) will sit on.

use std::collections::HashSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::core::{Oid, RepoLoc};
use crate::gix_backend::{RepoPathResolver, RootedResolver};
pub use crate::refs_pagination::{
    CatalogueRepoState, RefKind, RefPageItem, RefsPage, RefsPageError, RefsPageRequest,
    RefsSummary, REFS_PAGE_DEFAULT_LIMIT, REFS_PAGE_MAX_LIMIT, REFS_PAGE_MAX_QUERY_BYTES,
};
pub use crate::tree_pagination::{
    TreePage, TreePageError, TreePageLookup, TreePageRequest, TREE_PAGE_DEFAULT_LIMIT,
    TREE_PAGE_LATEST_COMMIT_WALK_MAX, TREE_PAGE_MAX_LIMIT, TREE_PAGE_MAX_QUERY_BYTES,
    TREE_PAGE_SCAN_MAX_ENTRIES, TREE_PAGE_SCAN_MAX_NAME_BYTES,
    TREE_PAGE_SCAN_MAX_TOTAL_NAME_BYTES,
};

// ───────────────────────────── errors ────────────────────────────────────────────────────────────

/// The error surface of the durable on-disk git store. Loud + specific (a refusal is diagnosable —
/// EI-01 §3); never a silent wrong-bytes / lost-write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DurableError {
    /// A `git2` (libgit2) op failed (open / odb / reference). Carries the libgit2 message.
    Git(String),
    /// A filesystem op failed (creating the tenant/region parent dirs for a new repo).
    Io(String),
    /// A compare-and-swap ref update was REJECTED because the ref's current tip did not match the
    /// expected-old the caller asserted (a non-fast-forward / lost-update race). The ref did NOT
    /// move — the LOUD reject the per-ref linearisation point owns (arch §3).
    CasMismatch {
        /// the fully-qualified ref the CAS targeted.
        ref_name: String,
        /// the tip the caller believed it was moving from (`None` = expected-to-not-exist / create).
        expected: Option<String>,
        /// the ref's actual current tip (`None` = the ref does not exist).
        actual: Option<String>,
    },
    /// An object / ref / repo asked for was not present on disk.
    NotFound(String),
    /// **A capability-scoped refusal (R2-exit).** The operation is well-formed and the object exists,
    /// but the principal is NOT authorized to perform it — e.g. a non-CI-producer principal attempting
    /// to report CI check facts (`git.checks.report` is a CI-PRODUCER capability, never an ordinary
    /// writer one). Maps to a fail-closed 403 at the edge. Loud + specific (a refusal is diagnosable).
    Forbidden(String),
}

impl std::fmt::Display for DurableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DurableError::Git(m) => write!(f, "durable git op failed: {m}"),
            DurableError::Io(m) => write!(f, "durable git io failed: {m}"),
            DurableError::CasMismatch {
                ref_name,
                expected,
                actual,
            } => write!(
                f,
                "ref CAS rejected on {ref_name}: expected {expected:?} but the on-disk tip is \
                 {actual:?} — the ref did NOT move (non-fast-forward / lost-update)"
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

static ATOMIC_WRITE_SEQUENCE: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Durably replace `file` with `bytes`: sync a process-unique temporary file, rename it into place,
/// then sync the parent directory so the rename itself survives a crash.
pub(crate) fn write_file_atomic(
    dir: &Path,
    file: &Path,
    bytes: &[u8],
) -> Result<(), DurableError> {
    write_file_atomic_with(dir, file, |handle| {
        handle
            .write_all(bytes)
            .map_err(|e| DurableError::Io(format!("write {}: {e}", file.display())))
    })
}

/// Durably replace `file` with bytes produced directly into its process-unique temporary file.
/// This is the streaming form used by large bounded artifacts to avoid a second whole-body buffer.
pub(crate) fn write_file_atomic_with(
    dir: &Path,
    file: &Path,
    write: impl FnOnce(&mut std::fs::File) -> Result<(), DurableError>,
) -> Result<(), DurableError> {
    std::fs::create_dir_all(dir)
        .map_err(|e| DurableError::Io(format!("create dir {}: {e}", dir.display())))?;
    let sequence = ATOMIC_WRITE_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let name = file.file_name().and_then(|value| value.to_str()).unwrap_or("record");
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
            DurableError::Io(format!("rename {} to {}: {e}", tmp.display(), file.display()))
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

// ───────────────────────────── durable per-ref generation counter (R0.4 / git #1 HIGH) ───────────

/// The git-config key holding the durable, monotonic generation of one ref (R0.4 / git #1 HIGH).
///
/// **Why this exists (the bug it replaces).** The crash reconciler + write path used to treat the
/// on-disk **reflog LENGTH** ([`DurableGitRepo::reflog_len`]) as the durable per-ref `update_seq`
/// generation. Reflog length is an OPERATION COUNT, not a monotonic generation: when a ref is
/// **deleted**, libgit2 removes that ref's reflog, so on a delete+recreate the count RESETS to 1 —
/// while the committed `update_seq` (the recovery fence) is monotonic and keeps climbing. After a
/// delete+recreate followed by a crash in the apply-after-outbox-commit window, the reconciler then
/// mis-compares (the restarted reflog is smaller than the committed seq of an already-applied move)
/// and can replay a stale move (CAS-mismatch) or leave a ref wrongly deleted. See git #1 HIGH.
///
/// **Why the config counter is correct.** This counter is keyed by the ref NAME and stored in the
/// repo's git-**config** (`[myelin "refgen"] <encoded-ref> = N`), which is a wholly separate on-disk
/// file from the ref's reflog. So it:
///  - **survives the ref's own deletion** — deleting a ref removes its reflog but never touches the
///    `myelin.refgen.*` config, so the generation does NOT reset on delete+recreate;
///  - **survives restart** — config is on disk; a fresh [`Self::open_git`] reopen reads it back;
///  - **is monotonic (max-wins, never decreases)** — every advancing CAS writes `current + 1`.
///
/// The ref name is **hex-encoded** (with a leading letter) so the config variable is always a valid
/// git identifier (`[a-zA-Z][a-zA-Z0-9-]*`) regardless of the ref's slashes/dots, and the encoding is
/// 1:1 — two distinct refs never collide onto one counter.
fn refgen_key(ref_name: &str) -> String {
    use std::fmt::Write as _;
    // Leading 'r' guarantees an alphabetic first char (a bare hex digit is a rejected config key).
    let mut var = String::with_capacity(ref_name.len() * 2 + 1);
    var.push('r');
    for b in ref_name.as_bytes() {
        let _ = write!(var, "{b:02x}");
    }
    format!("myelin.refgen.{var}")
}

/// Read one config-backed generation without conflating an absent key with corrupt configuration.
fn read_ref_generation(cfg: &git2::Config, ref_name: &str) -> Result<u64, DurableError> {
    match cfg.get_i64(&refgen_key(ref_name)) {
        Ok(value) => u64::try_from(value).map_err(|_| {
            DurableError::Git(format!("negative ref generation stored for {ref_name}"))
        }),
        Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(0),
        Err(e) => Err(git_err(&format!("read refgen for {ref_name}"), e)),
    }
}

/// Return the next generation only when it can still round-trip through git-config's signed
/// integer representation. The receive path uses this before committing its outbox witness, and
/// the disk apply path uses the same bound before mutating the ref.
pub(crate) fn next_ref_generation(current: u64) -> Option<u64> {
    current.checked_add(1).filter(|next| i64::try_from(*next).is_ok())
}

// ───────────────────────────── one on-disk reflog entry ──────────────────────────────────────────

/// One durable reflog entry read back from the on-disk git reflog. The reflog is durable (it is the
/// real git reflog on disk) — this is the read shape the [`crate::receive_pack::RefStore`] assembles
/// its [`crate::receive_pack::ReflogEntry`] view from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableReflogEntry {
    /// the old tip (`None` for the create entry — the zero oid in git's reflog).
    pub old_oid: Option<Oid>,
    /// the new tip.
    pub new_oid: Oid,
    /// the committer name recorded on the entry — the [`crate::receive_pack::RefStore`] writes the
    /// pusher PSEUDONYM here (never a raw identity — GIT-1), so it round-trips on read.
    pub committer: String,
    /// the reflog message.
    pub message: String,
}

// ───────────────────────────── commit log / diff raw read shapes (GT-004) ────────────────────────

/// Raw metadata for one commit read from the on-disk graph (libgit2). PII-free — `author_*` is the
/// GIT-1 tenant pseudonym the commit was authored with (never a raw identity).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitMeta {
    /// The full commit oid.
    pub oid: String,
    /// The commit summary (first line of the message).
    pub summary: String,
    /// The author name (the tenant pseudonym).
    pub author_name: String,
    /// The author email (the tenant pseudonym's `…@<tenant>.noreply`).
    pub author_email: String,
    /// Commit time, unix seconds.
    pub time: i64,
    /// The parent oids (0 = root; >1 = a merge commit).
    pub parents: Vec<String>,
}

/// Immutable object coordinates captured for the first page of a pull-request commit walk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrCommitSnapshot {
    /// Base tip resolved on page one, or `None` when the base ref did not exist.
    pub base_oid: Option<String>,
    /// Pull-request head commit resolved on page one.
    pub head_oid: String,
}

/// A pull-request commit snapshot page could not be served.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrCommitPageError {
    /// The requested page size or continuation position exceeds the finite interactive ceilings.
    InvalidPagination,
    /// A pinned head or base graph exceeds the finite reachability proof ceiling.
    CapacityExceeded,
    /// An object pinned by a previously minted continuation no longer exists.
    SnapshotExpired,
    /// Another durable Git operation failed.
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

/// Raw per-file delta in a commit diff (libgit2 `diff_tree_to_tree`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDelta {
    /// The (new) file path.
    pub path: String,
    /// The rename/copy source path (`None` otherwise).
    pub old_path: Option<String>,
    /// `A`/`M`/`D`/`R`/`C`.
    pub status: char,
    /// The unified-diff lines: `(origin, content)` where origin is `+`/`-`/` `.
    pub lines: Vec<(char, String)>,
}

/// Raw full detail of one commit: metadata + full message + per-file diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitDetail {
    /// The commit metadata.
    pub meta: CommitMeta,
    /// The full commit message.
    pub message: String,
    /// The changed files.
    pub files: Vec<FileDelta>,
}

// ───────────────────────────── PR diff raw read shapes (R3.2 · G-7 N1) ───────────────────────────

/// The rendered kind of a changed file — drives the R-21 binary/LFS/submodule rows (never a garbled
/// text dump). `Text` is the default (hunks render); the others carry NO hunks (a pointer/size row).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Text,
    Binary,
    Lfs,
    Submodule,
}

impl FileKind {
    /// The stable wire token (the DiffViewer maps it to the row treatment).
    pub fn as_str(self) -> &'static str {
        match self {
            FileKind::Text => "text",
            FileKind::Binary => "binary",
            FileKind::Lfs => "lfs",
            FileKind::Submodule => "submodule",
        }
    }
}

/// One diff line with BOTH line numbers (anchors, SR prefixes, deep-links need them). `old_no` is
/// `None` on an added line, `new_no` is `None` on a removed line (a context line carries both).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLineDelta {
    /// `+` add / `-` remove / ` ` context.
    pub origin: char,
    /// The line content (newline-trimmed).
    pub content: String,
    /// The OLD-side line number (`None` on `+`).
    pub old_no: Option<u32>,
    /// The NEW-side line number (`None` on `-`).
    pub new_no: Option<u32>,
}

/// A bounded expand-context lookup. The caller can distinguish an absent object, a binary object,
/// and a text object that was refused by the pre-inflation ODB-size ceiling without materializing
/// arbitrary repository bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileLinesLookup {
    Found(Vec<DiffLineDelta>),
    Binary,
    TooLarge { size: u64, maximum: usize },
    Missing,
}

/// One hunk of a file delta — its `@@` header + boundaries + lines. Boundaries let the client render
/// collapsed unchanged runs and expand context (a flat `lines[]` can't).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffHunkDelta {
    /// The full hunk header (`@@ -104,7 +104,9 @@ impl DurableGitEdge {`).
    pub header: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLineDelta>,
}

/// One changed file in a PR diff — hunk-structured, with the kind + counts + size the R-21 rows need.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrFileDelta {
    pub path: String,
    pub old_path: Option<String>,
    /// New-side blob content address. `None` for deletions and submodules (whose new object is a
    /// commit, not a blob). Expand-context consumers must use this exact immutable object.
    pub new_blob_oid: Option<String>,
    /// `A`/`M`/`D`/`R`/`C`.
    pub status: char,
    pub kind: FileKind,
    pub additions: u32,
    pub deletions: u32,
    /// The new-side blob byte size (binary/LFS rows show it; `None` for a text/deleted file).
    pub size_bytes: Option<u64>,
    pub hunks: Vec<DiffHunkDelta>,
    /// `true` for a deleted file whose contents are collapsed by default ("Show deleted contents").
    pub deleted_body_available: bool,
    /// `true` when the per-file line cap was hit (the client offers "Expand all" → a refetch).
    pub truncated: bool,
}

/// A PR's three-dot diff (`merge-base(base, head) … head`) — the reviewer reviews the PR's OWN
/// changes, not drift in the base. `base_oid` is the merge-base ACTUALLY diffed (honest even under
/// the two-dot fallback — it is then the base tip, and the UI labels it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrDiff {
    /// The merge-base actually diffed against (or the base tip under the two-dot fallback).
    pub base_oid: String,
    /// The head snapshot this diff renders.
    pub head_oid: String,
    /// `true` iff `base_oid` is a real merge-base (three-dot); `false` = the two-dot fallback (the UI
    /// labels "compared against <ref> @ <oid>").
    pub three_dot: bool,
    pub files: Vec<PrFileDelta>,
    pub total_files: usize,
    pub total_additions: u32,
    pub total_deletions: u32,
}

/// One entry in a nested tree listing (R3.4 repo-browsing). `size` is the blob byte size for files
/// (`None` for directories) — the tree row's size affordance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeEntryInfo {
    /// The entry basename (never the full path).
    pub name: String,
    /// `true` for a subdirectory (links to `tree/…`), `false` for a file (links to `blob/…`).
    pub is_dir: bool,
    /// The blob byte size (files only).
    pub size: Option<u64>,
}

/// Resolving a `{...path}` under a ref as a BLOB (R3.4). [`BlobPathLookup::IsDir`] drives the
/// blob→tree client redirect (kind mismatch).
pub enum BlobPathLookup {
    /// The blob bytes + its content-address + server-side binary detection + byte size.
    Found {
        /// The raw file bytes.
        bytes: Vec<u8>,
        /// The blob content-address (the GF-6 CAS base).
        oid: Oid,
        /// `true` if the bytes look binary (a NUL in the first 8000 bytes) — the download-fallback gate.
        is_binary: bool,
        /// The blob byte size.
        size: u64,
    },
    /// The blob exists but exceeds a caller-supplied pre-allocation byte ceiling. Its object id is
    /// still available from the tree entry without inflating the object, so metadata-only callers
    /// can render an honest download fallback.
    TooLarge {
        size: u64,
        maximum: usize,
        oid: Oid,
    },
    /// The path resolves to a directory → the caller redirects to the tree route.
    IsDir,
    /// No such path under the ref.
    Missing,
}

/// The git binary-file heuristic: a NUL byte within the first 8000 bytes marks the content binary
/// (so the UI renders the download fallback instead of a garbled `split('\n')` text dump — R3.4).
fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8000).any(|b| *b == 0)
}

/// Maximum inclusive line span accepted by the expand-context API.
pub const FILE_LINES_MAX_RANGE: usize = 1_000;
/// Maximum blob size inflated by the expand-context API. Checked through the ODB header first.
pub const FILE_LINES_MAX_BLOB_BYTES: usize = 512 * 1024;
/// Maximum direct refs materialized by smart-HTTP and push-adjacent recovery/audit paths.
pub const WIRE_MAX_REFS: usize = 100_000;
/// Maximum entries materialized from one durable reflog during audit/export.
pub const REFLOG_MAX_ENTRIES_PER_REF: usize = 100_000;
/// Maximum on-disk bytes libgit2 may parse for one durable reflog during audit/export.
pub const REFLOG_MAX_BYTES_PER_REF: usize = 32 * 1024 * 1024;
/// Maximum entries materialized across every ref in one durable audit/export view.
pub const REFLOG_MAX_TOTAL_ENTRIES: usize = 100_000;
/// Maximum reflog input/output string bytes materialized by one durable audit/export view.
pub const REFLOG_MAX_TOTAL_BYTES: usize = 32 * 1024 * 1024;
/// Maximum encoded bytes loaded for one tree object during interactive browsing.
pub(crate) const TREE_OBJECT_MAX_BYTES: usize = 8 * 1024 * 1024;
/// Maximum changed files computed for one interactive pull-request diff.
pub const PR_DIFF_MAX_FILES: usize = 1_000;
/// Maximum commit parents represented in interactive metadata (matching the web read contract).
pub const COMMIT_META_MAX_PARENTS: usize = 64;
/// Maximum first-line commit summary bytes represented in interactive metadata.
pub const COMMIT_META_MAX_SUMMARY_BYTES: usize = 8 * 1024;
/// Maximum author name/email bytes represented in interactive metadata.
pub const COMMIT_META_MAX_IDENTITY_BYTES: usize = 1_024;
/// Deepest offset accepted by the interactive commit-log walker.
pub const COMMIT_LOG_MAX_OFFSET: usize = 100_000;
/// Largest commit page materialized by one interactive history read.
pub const COMMIT_LOG_MAX_PAGE: usize = 500;
/// Deepest position one pull-request commit snapshot page may scan from its pinned head.
pub const PR_COMMIT_MAX_POSITION: usize = 100_000;
/// Maximum unique commit OIDs retained while proving either pinned PR snapshot coordinate.
pub const PR_COMMIT_MAX_GRAPH_NODES_PER_PIN: usize = 100_000;
/// Maximum parent edges examined while proving either pinned PR snapshot coordinate.
pub const PR_COMMIT_MAX_GRAPH_EDGES_PER_PIN: usize = 1_000_000;
/// Maximum rows yielded by the real page walk, including the one-row `has_more` probe.
pub const PR_COMMIT_MAX_PAGE_WALK_OBSERVATIONS: usize =
    PR_COMMIT_MAX_POSITION + COMMIT_LOG_MAX_PAGE + 1;
/// Maximum libgit2 walk nodes the sorted/hidden walk may preprocess after successful preflights.
pub const PR_COMMIT_MAX_INTERNAL_WALK_NODES: usize = 2 * PR_COMMIT_MAX_GRAPH_NODES_PER_PIN;
/// Maximum parent edges the sorted/hidden walk may examine after successful graph preflights.
pub const PR_COMMIT_MAX_INTERNAL_WALK_EDGES: usize = 2 * PR_COMMIT_MAX_GRAPH_EDGES_PER_PIN;
/// Maximum files materialized for one interactive commit diff.
pub const COMMIT_DIFF_MAX_FILES: usize = 1_000;
/// Maximum rendered lines materialized for one file in an interactive commit diff.
pub const COMMIT_DIFF_MAX_LINES_PER_FILE: usize = 4_000;
/// Maximum UTF-8 bytes accepted for one rendered diff line.
pub const DIFF_MAX_LINE_BYTES: usize = 64 * 1024;
/// Maximum aggregate rendered line bytes retained for one interactive diff computation.
pub const DIFF_MAX_RENDERED_BYTES: usize = 4 * 1024 * 1024;
/// Maximum commit message bytes copied into an interactive commit response.
pub const COMMIT_DIFF_MAX_MESSAGE_BYTES: usize = 512 * 1024;

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

/// **Nested-path traversal guard (R3.4).** A tree-relative path may never carry a `..` or `.` segment
/// or be absolute — such a path can only be an attempt to escape the committed tree (or a malformed
/// client path), so it resolves to "no such path" cleanly (never a host-file read, never a 500). An
/// empty/root path is safe (it selects the root tree). Called before `Tree::get_path`.
fn is_safe_tree_path(clean: &str) -> bool {
    if clean.is_empty() {
        return true;
    }
    clean
        .split('/')
        .all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

/// Project a libgit2 commit into the PII-free [`CommitMeta`] read shape.
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
    // `Commit::summary` takes `&mut self`; derive the first message line (a `&self` accessor) instead.
    let message = c.message().unwrap_or("");
    CommitMeta {
        oid: c.id().to_string(),
        summary: utf8_prefix(
            message.lines().next().unwrap_or(""),
            COMMIT_META_MAX_SUMMARY_BYTES,
        ),
        author_name: utf8_prefix(
            author.name().unwrap_or(""),
            COMMIT_META_MAX_IDENTITY_BYTES,
        ),
        author_email: utf8_prefix(
            author.email().unwrap_or(""),
            COMMIT_META_MAX_IDENTITY_BYTES,
        ),
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

/// Prove one pinned coordinate's reachable graph is finite before enabling libgit2's preprocessing
/// modes. The retained `scheduled` set includes both the frontier and visited OIDs, so its length is
/// the peak stored unique-OID bound. Every parent relation is counted separately, including edges to
/// an already-scheduled commit. Capacity takes precedence when discovering a missing parent would
/// exceed a cap; otherwise that object is classified as expired when it is popped and loaded.
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

// ───────────────────────────── the per-repo durable handle ───────────────────────────────────────

/// **A real on-disk bare git repository.** Wraps the resolved `<root>/<tenant>/<region>/<repo>.git`
/// path; every op opens the repo via `git2` (the same per-call open the read backend [`GixCore`] uses
/// — libgit2 caches the odb/refdb, and this keeps the handle `Send`-cheap with no long-lived FFI
/// borrow). Refs, reflog, and objects all live ON DISK and survive a process restart.
#[derive(Debug)]
pub struct DurableGitRepo {
    /// the bare repo's on-disk path (`…/<repo>.git`).
    path: PathBuf,
}

impl DurableGitRepo {
    /// The bare repo's on-disk path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn open_git(&self) -> Result<git2::Repository, DurableError> {
        git2::Repository::open(&self.path)
            .map_err(|e| git_err(&format!("open bare repo {}", self.path.display()), e))
    }

    /// Acquire the durable cross-process linearisation lock for one ref. The returned file owns the
    /// exclusive lock until dropped. Lock filenames use the same injective hex encoding as ref
    /// generations, and callers acquire multiple refs in sorted order to remain deadlock-free.
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
                DurableError::Io(format!("open durable ref lock {}: {e}", lock_path.display()))
            })?;
        fs4::fs_std::FileExt::lock_exclusive(&file).map_err(|e| {
            DurableError::Io(format!("acquire durable ref lock {}: {e}", lock_path.display()))
        })?;
        Ok(file)
    }

    /// **Open a THROWAWAY host-side quarantine bare repo (CT-006d push staging).** `init_bare` at `dir`
    /// with its odb alternating to `alternate_objects` (the REAL repo's `objects/` dir, READ-only) — the
    /// staging area where the sandbox-validated pushed objects are written + inspected (policy +
    /// connectivity) BEFORE any of them migrate into the real repo. The alternate lets a thin delta's
    /// base + existing-history connectivity resolve against the real repo without writing to it. The
    /// caller removes `dir` after the push resolves (accept OR reject — the quarantine is never kept).
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

    // ── objects (the on-disk odb — F-git-2: the oid→object lookup IS the real odb) ──

    /// Write a blob object into the on-disk odb and return its **real git oid** (computed by git, not
    /// a hand-rolled hash). Durable: a fresh handle over the same path reads it back.
    pub fn write_blob(&self, bytes: &[u8]) -> Result<Oid, DurableError> {
        let repo = self.open_git()?;
        let oid = repo.blob(bytes).map_err(|e| git_err("write blob", e))?;
        Ok(Oid::new(oid.to_string()))
    }

    /// Import the complete object closure reachable from `locked_head` out of another durable
    /// repository into this repository's ODB.
    ///
    /// This is the fork-merge object boundary: PR metadata may lock a head that lives in a distinct
    /// source repository, while the eventual target ref may only name objects present in the target
    /// ODB.  Libgit2 builds a self-contained pack from the exact locked commit (no source refs are
    /// copied), then its verified indexer installs that pack in the target. Passing no target ODB to
    /// the indexer deliberately rejects thin packs whose delta bases are absent from the pack.
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

        // Reopen after installation so verification cannot be satisfied by a stale ODB cache.
        self.open_git()?
            .find_commit(head)
            .map_err(|e| git_err("verify imported fork head in target ODB", e))?;
        Ok(())
    }

    /// Write a tree from `(name, blob_oid)` entries (regular-file mode `0o100644`) into the odb,
    /// returning the tree's real oid. The minimal real tree the durable write path (and GT-003) build
    /// a commit over.
    pub fn write_tree(&self, entries: &[(&str, &Oid)]) -> Result<Oid, DurableError> {
        let repo = self.open_git()?;
        let mut builder = repo.treebuilder(None).map_err(|e| git_err("treebuilder", e))?;
        for (name, blob) in entries {
            builder
                .insert(name, Self::parse_oid(blob)?, 0o100644)
                .map_err(|e| git_err(&format!("tree insert {name}"), e))?;
        }
        let oid = builder.write().map_err(|e| git_err("write tree", e))?;
        Ok(Oid::new(oid.to_string()))
    }

    /// Write a commit object into the odb (the real, `git fsck`-clean commit a ref points at).
    /// `author_name`/`author_email` are the pseudonymous identity (GIT-1 — the caller passes the
    /// tenant pseudonym, never a raw identity). Returns the commit's real oid.
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
        let tree_obj = repo.find_tree(tree_oid).map_err(|e| git_err("find tree", e))?;
        let sig = git2::Signature::now(author_name, author_email)
            .map_err(|e| git_err("signature", e))?;
        let parent_commits: Vec<git2::Commit<'_>> = parents
            .iter()
            .map(|p| {
                let oid = Self::parse_oid(p)?;
                repo.find_commit(oid).map_err(|e| git_err("find parent", e))
            })
            .collect::<Result<_, _>>()?;
        let parent_refs: Vec<&git2::Commit<'_>> = parent_commits.iter().collect();
        let oid = repo
            // `None` target ref — write the object only; the ref move is the explicit CAS step below.
            .commit(None, &sig, &sig, message, &tree_obj, &parent_refs)
            .map_err(|e| git_err("write commit", e))?;
        Ok(Oid::new(oid.to_string()))
    }

    /// Read an object's raw bytes from the on-disk odb by its git oid, rejecting from the object
    /// header before allocation when it exceeds `maximum_bytes`. `NotFound` if absent — the lookup
    /// is the real odb, NOT an in-memory index (F-git-2).
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

    /// Whether an object exists in the on-disk odb.
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

    // ── refs (durable; SI-012: read/CAS go to the on-disk repo, open loads from disk) ──

    /// Read a ref's current tip from disk (`None` if the ref does not exist). A FRESH handle over the
    /// same on-disk root reads the same value — the durability the in-memory `open` lacked (SI-012).
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

    /// List direct refs with an explicit materialization ceiling. Wire-facing callers use this
    /// before advertising or walking all tips so tenant-created ref cardinality remains finite.
    pub fn list_refs_bounded(
        &self,
        maximum: usize,
    ) -> Result<Vec<(String, Oid)>, DurableError> {
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
                let name = r.name().map_err(|_| {
                    DurableError::Git("reference name is not valid UTF-8".into())
                })?;
                out.push((name.to_string(), Oid::new(oid.to_string())));
            }
        }
        out.sort();
        Ok(out)
    }

    /// **Compare-and-swap a ref** (the per-ref linearisation point, arch §3). Atomically moves
    /// `name` from `expected` to `new` ONLY if its on-disk tip equals `expected`; otherwise the ref
    /// does NOT move and [`DurableError::CasMismatch`] is returned (the LOUD non-fast-forward reject).
    ///
    /// - `expected = None` → CREATE (the ref must not yet exist).
    /// - `new = None` → DELETE (the ref must currently equal `expected`).
    /// - both `Some` → UPDATE (the ref must currently equal `expected`).
    ///
    /// The update is written through libgit2's `reference_matching` (the C `current_id` guard is the
    /// real CAS) so it is durable + reflog-logged (`core.logallrefupdates` is set on creation).
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

        // Ref generations live in git-config's signed integer domain. Prove there is capacity BEFORE
        // mutating the ref; discovering exhaustion after the CAS would leave the tip advanced without
        // its recovery fence. The receive path performs the same check before its outbox commit.
        if !matches!((expected, new), (None, None)) {
            let cfg = repo.config().map_err(|e| git_err("config (refgen preflight)", e))?;
            let current = read_ref_generation(&cfg, name)?;
            next_ref_generation(current).ok_or_else(|| {
                DurableError::Git(format!("ref generation exhausted for {name}"))
            })?;
        }

        // Set the committer identity only after the CAS and generation preconditions pass, so a
        // rejected operation does not mutate repository configuration. Libgit2 reads these values
        // for the reflog committer (GIT-1).
        {
            let mut cfg = repo.config().map_err(|e| git_err("config", e))?;
            cfg.set_str("user.name", committer_pseudonym)
                .map_err(|e| git_err("set user.name", e))?;
            cfg.set_str("user.email", committer_pseudonym)
                .map_err(|e| git_err("set user.email", e))?;
        }

        match (expected, new) {
            // CREATE — the ref must not exist; `reference` with force=false fails if it does.
            (None, Some(new_oid)) => {
                repo.reference(name, Self::parse_oid(new_oid)?, false, reflog_msg)
                    .map_err(|e| git_err(&format!("create ref {name}"), e))?;
            }
            // UPDATE — `reference_matching` only moves the ref if its current value == `current_id`
            // (the real compare-and-swap), force=true to permit the value change.
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
            // DELETE — find the ref, re-check the tip under the open, then delete.
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
                r.delete().map_err(|e| git_err(&format!("delete ref {name}"), e))?;
            }
            // A no-op (delete a non-existent ref): nothing to do, already absent.
            (None, None) => {}
        }

        // R0.4 / git #1 HIGH: bump the durable per-ref generation on every successful, non-noop CAS —
        // create, update, AND delete alike (a delete is a generation-advancing event too). The bump is
        // `previous + 1`, keyed by ref NAME in git-config, so it is monotonic ACROSS the ref's own
        // deletion (the reflog dies with the ref; this counter does not) and across restart. This
        // replaces reflog-LENGTH-as-generation, which reset on delete+recreate and broke the reconciler
        // fence. See [`refgen_key`]. The `(None, None)` no-op above is deliberately excluded.
        if !matches!((expected, new), (None, None)) {
            self.bump_generation(&repo, name)?;
        }
        Ok(())
    }

    /// Advance the durable per-ref generation to `current + 1` (R0.4). Reads the current value from the
    /// repo's git-config (0 if never written), writes `+1` back at the repo-local config level (the same
    /// config handle pattern `update_ref_cas` uses for `user.name`/`user.email`). Monotonic — a bump
    /// never decreases the stored value.
    fn bump_generation(&self, repo: &git2::Repository, name: &str) -> Result<(), DurableError> {
        let key = refgen_key(name);
        let mut cfg = repo.config().map_err(|e| git_err("config (refgen)", e))?;
        let current = read_ref_generation(&cfg, name)?;
        let next = next_ref_generation(current).ok_or_else(|| {
            DurableError::Git(format!("ref generation exhausted for {name}"))
        })?;
        let next = i64::try_from(next).expect("next_ref_generation guarantees signed range");
        cfg.set_i64(&key, next)
            .map_err(|e| git_err(&format!("set refgen for {name}"), e))?;
        Ok(())
    }

    /// Repair the one-step crash window where the ref CAS reached its committed new tip but the
    /// following config-backed generation bump did not. The reconciler proves the witness is exactly
    /// `expected_current + 1` and the tip already equals its new state before calling this method.
    /// Rechecking the current generation here makes a concurrent or repeated repair fail safely.
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

    /// **The durable, monotonic per-ref generation** (R0.4 / git #1 HIGH — the recovery fence the
    /// reconciler compares `update_seq` against). Reads the `myelin.refgen.<encoded-ref>` config counter
    /// (0 if the ref was never written). Unlike [`Self::reflog_len`], this does NOT reset when a ref is
    /// deleted and recreated (it is keyed by name in config, not tied to the ref's reflog), and it
    /// survives a process restart (config is on disk). This is the source of truth both the write path
    /// ([`crate::receive_pack`]) and the reconciler ([`crate::reconcile`]) use for `update_seq`.
    pub fn ref_generation(&self, name: &str) -> Result<u64, DurableError> {
        let repo = self.open_git()?;
        let cfg = repo.config().map_err(|e| git_err("config (refgen)", e))?;
        read_ref_generation(&cfg, name)
    }

    /// The number of entries in a ref's on-disk reflog (0 if the ref / reflog does not exist). This is
    /// the reflog ENTRY COUNT — used only for the reflog listing view / entry-count assertions. It is
    /// **NOT** the durable generation (R0.4 / git #1 HIGH): the reflog is destroyed when a ref is
    /// deleted, so this count RESETS on a delete+recreate while the true generation must keep climbing.
    /// Use [`Self::ref_generation`] for the monotonic per-ref generation / recovery fence.
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

    /// Read a ref's durable on-disk reflog, oldest-first (git stores newest-first; we reverse it for
    /// the [`crate::receive_pack::RefStore`] audit view). The file-size header is checked before
    /// libgit2 parses the reflog, and the durable generation is captured under the same ref lock.
    pub(crate) fn reflog_entries_bounded(
        &self,
        name: &str,
        maximum_entries: usize,
        maximum_bytes: usize,
    ) -> Result<(Vec<DurableReflogEntry>, usize, u64), DurableError> {
        if !git2::Reference::is_valid_name(name) {
            return Err(DurableError::Git("invalid reflog ref name".into()));
        }
        // The same cross-process lock gates Myelin's ref writers, so the reflog and its durable
        // generation are one stable snapshot while libgit2 parses the bounded file.
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
                let cfg = repo.config().map_err(|e| git_err("config (reflog audit)", e))?;
                return Ok((Vec::new(), 0, read_ref_generation(&cfg, name)?));
            }
            Err(error) => {
                return Err(DurableError::Io(format!("stat {}: {error}", path.display())));
            }
        };
        let log = match repo.reflog(name) {
            Ok(log) => log,
            Err(e) if e.code() == git2::ErrorCode::NotFound => {
                let cfg = repo.config().map_err(|error| git_err("config (reflog audit)", error))?;
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
        // git reflog is stored newest-first; iterate in reverse for oldest-first (update order).
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
        let cfg = repo.config().map_err(|e| git_err("config (reflog audit)", e))?;
        let generation = read_ref_generation(&cfg, name)?;
        Ok((out, on_disk_bytes, generation))
    }

    // ── working-tree reads + the single-file commit build (GT-003 web-edit) ──

    /// Resolve a ref to its tip commit (`None` if the ref does not exist).
    /// Resolve a revspec (a bare branch/tag name, an oid, or a fully-qualified `refs/…`) to its commit
    /// — `None` if it does not resolve (an absent ref/oid is an empty browse, not an error). Uses
    /// libgit2's `revparse_single` (the same resolution `git show <rev>` uses) so branches, tags, and
    /// oids all work through the one browse path (R3.4).
    pub(crate) fn resolve_commit<'r>(
        &self,
        repo: &'r git2::Repository,
        revspec: &str,
    ) -> Result<Option<git2::Commit<'r>>, DurableError> {
        match repo.revparse_single(revspec) {
            Ok(obj) => match obj.peel_to_commit() {
                Ok(c) => Ok(Some(c)),
                // A revspec that resolves to a NON-commit object (e.g. a bare tree oid, `main^{tree}`)
                // is a client input error, not a server fault: peel fails with InvalidSpec. Treat it
                // like NotFound → an empty browse (404), never a 500 (R3.4 verifier finding 1; honours
                // the module invariant "an absent ref/oid is an empty browse, not an error").
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

    fn tip_commit(&self, repo: &git2::Repository, ref_name: &str) -> Result<Option<git2::Oid>, DurableError> {
        match repo.find_reference(ref_name) {
            Ok(r) => Ok(r.target()),
            Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(e) => Err(git_err(&format!("find_reference {ref_name}"), e)),
        }
    }

    /// Read a blob from one exact immutable commit object id. Unlike the browse-oriented
    /// [`Self::read_blob_at_path_bounded`], this API never accepts a revspec.
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
                return Err(DurableError::Git(format!("exact commit {oid_text} not found")))
            }
            Err(error) => return Err(git_err("find exact commit", error)),
        };
        Self::read_blob_from_commit(&repo, &commit, path, maximum_bytes)
    }

    /// Resolve a nested blob while checking its ODB header before inflating/materializing content.
    /// A directory returns [`BlobPathLookup::IsDir`], an absent or unsafe path returns
    /// [`BlobPathLookup::Missing`], and binary detection happens server-side only after the header
    /// admits the allocation.
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

    /// Resolve only the blob object id for a path. The tree entry is authoritative, so callers that
    /// need a CAS base do not have to inflate or copy the blob contents first.
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
            return Ok(BlobPathLookup::IsDir); // the repo root is a tree, not a file.
        }
        if !is_safe_tree_path(clean) {
            return Ok(BlobPathLookup::Missing); // a `..`/absolute path cannot name an in-tree blob.
        }
        let entry = match root.get_path(std::path::Path::new(clean)) {
            Ok(e) => e,
            Err(e) if e.code() == git2::ErrorCode::NotFound => return Ok(BlobPathLookup::Missing),
            Err(e) => return Err(git_err("tree get_path", e)),
        };
        match entry.kind() {
            Some(git2::ObjectType::Blob) => {
                let odb = repo.odb().map_err(|e| git_err("open object database", e))?;
                let (object_size, object_kind) = odb.read_header(entry.id())
                    .map_err(|e| git_err("read object header", e))?;
                if object_kind != git2::ObjectType::Blob { return Ok(BlobPathLookup::Missing); }
                if object_size > maximum_bytes {
                    return Ok(BlobPathLookup::TooLarge {
                        size: object_size as u64,
                        maximum: maximum_bytes,
                        oid: Oid::new(entry.id().to_string()),
                    });
                }
                let obj = entry.to_object(repo).map_err(|e| git_err("entry object", e))?;
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

    /// **F9 (R4.1 dogfood) — heal a dangling HEAD symref so a fresh `git clone` checks out.** libgit2's
    /// `init_bare` leaves HEAD symbolically pointing at `refs/heads/master`, but Myelin pushes land on
    /// `main` (or whatever the FIRST branch pushed is) — so a freshly-created repo has a DANGLING HEAD
    /// and `git clone` warns "remote HEAD refers to nonexistent ref, unable to checkout" (the refs +
    /// objects ARE present; only the HEAD pointer is wrong). Call on the first push that lands a branch:
    /// if HEAD does NOT already resolve to a live branch, repoint it (the WRITE side) at the default
    /// branch — `main` if present, else the first branch by sorted name — mirroring the read-side
    /// paginated ref summary. A HEAD that already resolves to a live branch is left
    /// UNTOUCHED (an admin-chosen default is preserved). No-op when the repo has no branch yet (an
    /// empty repo clones cleanly with a still-symbolic HEAD). Idempotent.
    pub fn heal_head_symref(&self) -> Result<(), DurableError> {
        let repo = self.open_git()?;
        // `head()` errs (`UnbornBranch`) exactly when HEAD's symbolic target names a nonexistent
        // branch — the dangling-HEAD condition. If it resolves, HEAD is already fine: leave it.
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
            return Ok(()); // no branch on disk yet — nothing to point HEAD at (empty repo is fine).
        };
        repo.set_head(&format!("refs/heads/{target}"))
            .map_err(|e| git_err("set HEAD symref (F9)", e))?;
        Ok(())
    }

    /// Resolve latest-commit metadata for one already-selected tree page against its immutable
    /// commit snapshot. This never re-resolves a mutable branch between page selection and metadata
    /// projection. Tree pages contain at most 100 rows and this snapshot walk is capped at 500 commits.
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

    /// Read compact metadata for one exact immutable commit object id. This never accepts or
    /// resolves a ref/revspec; malformed, absent, and non-commit object ids return `None`.
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
        walk.set_sorting(git2::Sort::TIME).map_err(|e| git_err("revwalk sort", e))?;
        walk.push(tip).map_err(|e| git_err("revwalk push", e))?;
        let mut out: std::collections::BTreeMap<String, CommitMeta> = Default::default();
        for (seen, oid_res) in walk.enumerate() {
            if seen >= cap {
                break;
            }
            let oid = oid_res.map_err(|e| git_err("revwalk next", e))?;
            let commit = repo.find_commit(oid).map_err(|e| git_err("find_commit", e))?;
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
                    // The immediate child of dir_path (a file directly here, or the dir it lives in).
                    let child = rel.split('/').next().unwrap_or_default();
                    if child.is_empty() || !requested.contains(child) {
                        continue;
                    }
                    // First (newest) commit to touch this child wins; later (older) commits don't override.
                    out.entry(child.to_string()).or_insert_with(|| meta.clone());
                }
            }
            if out.len() == requested.len() {
                break;
            }
        }
        Ok(out)
    }

    // ── commit log + commit diff (the browse surface — GT-004; libgit2 revwalk + tree diff) ──

    /// Walk the commit log from a ref tip (newest-first), returning a page of [`CommitMeta`] plus a
    /// `has_more` flag (the cursor the edge advances). Reuses libgit2's `revwalk` over the REAL on-disk
    /// commit graph — never a reimplemented walk. An absent ref yields an empty page (not an error).
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
        walk.set_sorting(git2::Sort::TIME).map_err(|e| git_err("revwalk sort", e))?;
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
            let c = repo.find_commit(oid).map_err(|e| git_err("find_commit", e))?;
            out.push(commit_meta(&c));
            seen += 1;
        }
        Ok((out, has_more))
    }

    /// Resolve the immutable object coordinates for the first page of a pull-request commit walk.
    /// An absent/malformed head keeps the historical empty-list behavior; an absent base ref is an
    /// explicit `None` snapshot coordinate and therefore remains absent on continuation pages.
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

    /// Walk one immutable pull-request commit snapshot page. Explicit stack/`HashSet` preflights
    /// first prove the pinned head and base graphs each stay within
    /// [`PR_COMMIT_MAX_GRAPH_NODES_PER_PIN`] retained unique OIDs and
    /// [`PR_COMMIT_MAX_GRAPH_EDGES_PER_PIN`] examined parent edges. Only then is libgit2's existing
    /// `TIME` + hide ordering enabled; it may preprocess a union bounded by
    /// [`PR_COMMIT_MAX_INTERNAL_WALK_NODES`] nodes and [`PR_COMMIT_MAX_INTERNAL_WALK_EDGES`] edges.
    /// These are graph bookkeeping/work bounds, not a total RSS claim: libgit2 and commit-object
    /// parsing have their own finite allocations. The page iterator yields at most
    /// [`PR_COMMIT_MAX_PAGE_WALK_OBSERVATIONS`] rows, while skipped rows and the one-row `has_more`
    /// probe are never expanded into [`CommitMeta`].
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

    /// **The commits IN a PR (R3.3 N2) — reachable from `head_oid` but NOT from `base_ref`'s tip.**
    /// Compatibility wrapper used by the overview's bounded count. Interactive pagination calls
    /// [`Self::commits_in_pr_snapshot`] directly with cursor-pinned object coordinates.
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

    /// The full detail of one commit (`None` if the oid is malformed or absent): its metadata, full
    /// message, and the per-file unified diff against the FIRST parent (the root commit diffs against
    /// the empty tree). Reuses libgit2's `diff_tree_to_tree` over the REAL on-disk trees.
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

        // Two cooperating callbacks share one accumulator via RefCell: file_cb opens a new file delta,
        // line_cb appends lines to the current (last) one. libgit2 calls file_cb before its lines.
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
            // A rename only carries old_path when it actually differs from path.
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

    /// **The PR three-dot diff (R3.2 · G-7 N1) — `merge-base(base_ref, head_oid) … head_oid`.** The
    /// reviewer reviews the PR's OWN changes, never drift the base picked up. Reuses libgit2's
    /// `merge_base` + `diff_tree_to_tree` over the REAL on-disk trees (no reimplementation). Hunk-
    /// structured, with per-line old/new numbers, binary/submodule kinds, and a per-file line cap
    /// (`truncated`). A malformed/absent head → `None` (the edge maps that to a dignified state, not a
    /// 500). If `merge_base` can't resolve (a foreign/absent base), falls back to the base TIP as the
    /// diff base and flags `three_dot == false` (the honest two-dot floor the UI labels).
    ///
    /// `per_file_line_cap` bounds each file's rendered line count (0 = uncapped); a file over the cap
    /// keeps its first-cap lines + `truncated == true`.
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
        // Resolve the base tip (try the ref as-is, then refs/heads/<ref> — R3.3's stored form varies).
        let base_tip = match self.tip_commit(&repo, base_ref)? {
            Some(t) => Some(t),
            None if base_ref.starts_with("refs/") => None,
            None => self.tip_commit(&repo, &format!("refs/heads/{base_ref}"))?,
        };
        // Three-dot: the merge-base of base and head. On any failure (no base, foreign histories),
        // fall back to the base tip (two-dot) and flag it — honest, never a silent wrong base.
        // `None` base_oid = no base ref at all → diff head against the empty tree (a brand-new branch).
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
                let base_commit = repo.find_commit(o).map_err(|e| git_err("find base commit", e))?;
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
        // Rendered-line count for the CURRENT file — bounds memory: once a file reaches its cap we
        // stop ACCUMULATING lines (still counting the true diffstat), so a huge single-hunk file never
        // materializes wholesale in the response Vec (R3.2 verifier HOLD — the cap is a load bound, not
        // just a post-hoc trim). Reset when a new file delta begins.
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
            // Kind: a submodule is a gitlink (filemode Commit); a binary is libgit2's binary flag; LFS
            // is a text pointer file we sniff below once its content is seen. size = the new blob size.
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
            let new_blob_oid = if status != 'D' && kind != FileKind::Submodule && !new_oid.is_zero() {
                Some(new_oid.to_string())
            } else {
                None
            };
            let size_bytes = delta.new_file().size();
            let size_bytes = if size_bytes > 0 { Some(size_bytes) } else { None };
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
        let mut line_cb = |_d: git2::DiffDelta<'_>,
                           _h: Option<git2::DiffHunk<'_>>,
                           line: git2::DiffLine<'_>| {
            let origin = line.origin();
            if !matches!(origin, '+' | '-' | ' ') {
                return true; // skip file/hunk header context lines libgit2 emits with other origins.
            }
            let mut fs = files.borrow_mut();
            if let Some(f) = fs.last_mut() {
                // The diffstat (additions/deletions) always reflects the TRUE totals, even past the cap.
                match origin {
                    '+' => f.additions += 1,
                    '-' => f.deletions += 1,
                    _ => {}
                }
                // Bound accumulation at the per-file cap: past it, count but don't store the line, and
                // flag the file truncated. `cap == 0` means unbounded (caller opted out).
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
                    // LFS sniff: a pointer file's first added line is `version https://git-lfs…`.
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
        // Binary/LFS/submodule files carry NO text hunks (never a garbled dump).
        for f in &mut files {
            if f.kind != FileKind::Text {
                f.hunks.clear();
            }
            // `line_cb` already bounded accumulation at `per_file_line_cap` (within a hunk, so a huge
            // single-hunk file is capped — R3.2 verifier HOLD). A truncated file may carry trailing
            // hunks that received no lines (the cap hit before them); drop those empty hunks so the
            // wire never ships a bare hunk header.
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

    /// **Expand-context (R3.2 · G-7 N2) — the raw lines of a blob at `oid`, `start..=end` (1-based).**
    /// Serves Expand ↑/↓/all and "Show deleted contents" (via the old-side blob oid). Returns context
    /// lines (origin `' '`) carrying their blob line number in `new_no` (the client maps the old-side
    /// column from the surrounding hunk offset). A malformed/absent oid or a non-blob returns
    /// [`FileLinesLookup::Missing`]. The object-check is the caller's (the edge Pull-guards it exactly
    /// like the blob route). The range and ODB byte ceilings are enforced here as defense in depth,
    /// before blob materialization.
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
            Err(e) if e.code() == git2::ErrorCode::NotFound => {
                return Ok(FileLinesLookup::Missing)
            }
            Err(e) => return Err(git_err("find blob", e)),
        };
        if blob.is_binary() {
            return Ok(FileLinesLookup::Binary); // never expand a binary into a garbled dump.
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

    /// **Build a single-file web-edit commit (GT-003).** Write `contents` as a blob, rebuild the ref's
    /// top-level tree with `path` set to that blob (seeded from the current tree so OTHER entries are
    /// preserved; empty for a first commit), and write a commit whose parent is the ref's current tip.
    /// Returns `(new_commit_oid, new_blob_oid, parent_commit_oid)`. Does NOT move the ref — the durable
    /// per-ref CAS ([`crate::receive_pack::RefStore`]) is the explicit next step (one write path, GF-6).
    pub fn build_file_commit(
        &self,
        ref_name: &str,
        path: &str,
        contents: &[u8],
        message: &str,
        author_name: &str,
        author_email: &str,
    ) -> Result<(Oid, Oid, Option<Oid>), DurableError> {
        let repo = self.open_git()?;
        let parent_oid = self.tip_commit(&repo, ref_name)?;

        let blob_oid = repo.blob(contents).map_err(|e| git_err("write blob", e))?;

        // Seed the tree builder from the parent's tree so other files survive the single-file edit.
        let base_tree = match parent_oid {
            Some(p) => {
                let c = repo.find_commit(p).map_err(|e| git_err("find parent", e))?;
                Some(c.tree().map_err(|e| git_err("parent tree", e))?)
            }
            None => None,
        };
        let mut builder = repo
            .treebuilder(base_tree.as_ref())
            .map_err(|e| git_err("treebuilder", e))?;
        builder
            .insert(path, blob_oid, 0o100644)
            .map_err(|e| git_err(&format!("tree insert {path}"), e))?;
        let tree_oid = builder.write().map_err(|e| git_err("write tree", e))?;
        let tree_obj = repo.find_tree(tree_oid).map_err(|e| git_err("find tree", e))?;

        let sig = git2::Signature::now(author_name, author_email)
            .map_err(|e| git_err("signature", e))?;
        let parent_commits: Vec<git2::Commit<'_>> = match parent_oid {
            Some(p) => vec![repo.find_commit(p).map_err(|e| git_err("find parent", e))?],
            None => Vec::new(),
        };
        let parent_refs: Vec<&git2::Commit<'_>> = parent_commits.iter().collect();
        let commit_oid = repo
            .commit(None, &sig, &sig, message, &tree_obj, &parent_refs)
            .map_err(|e| git_err("write commit", e))?;

        Ok((
            Oid::new(commit_oid.to_string()),
            Oid::new(blob_oid.to_string()),
            parent_oid.map(|p| Oid::new(p.to_string())),
        ))
    }

    // ── merge-target validation (GT-003 — never advance a protected ref to an arbitrary oid) ──

    /// Whether `oid` exists in the odb AND is a commit (a ref must point at a real commit). Used to
    /// reject a merge that names a non-existent / non-commit `head_oid`.
    pub fn object_is_commit(&self, oid: &Oid) -> bool {
        let Ok(repo) = self.open_git() else { return false };
        let Ok(goid) = git2::Oid::from_str(oid.as_str()) else {
            return false;
        };
        let is_commit = repo.find_commit(goid).is_ok();
        is_commit
    }

    /// Is advancing a ref from `base_tip` to `head` a fast-forward (the only durable merge advance v1
    /// admits — never advance a protected ref to an unrelated/arbitrary oid)? `head` must be a real
    /// commit; `base_tip = None` (creating the ref) is allowed; otherwise `head` must equal OR be a
    /// descendant of `base_tip` (the connectivity/ancestry check the empty-quarantine path lacked).
    pub fn is_fast_forward(
        &self,
        base_tip: Option<&Oid>,
        head: &Oid,
    ) -> Result<bool, DurableError> {
        let repo = self.open_git()?;
        let head_g = Self::parse_oid(head)?;
        if repo.find_commit(head_g).is_err() {
            return Ok(false); // head is not a real commit on disk
        }
        match base_tip {
            None => Ok(true), // creating the ref — any real commit is a valid initial tip
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

    // ── integrity (the external-oracle discipline, in-process slice) ──

    /// **In-process integrity check** — the `git fsck`-equivalent slice runnable in `src` (no host
    /// exec). Verifies (a) the odb opens and every loose/packed object parses (re-hash-on-read is
    /// libgit2's own — a corrupt object fails to read), and (b) every ref resolves to an object that
    /// EXISTS in the odb (no dangling ref). The TESTS additionally run the real `git fsck` binary
    /// (the full external oracle) — see `tests/`.
    pub fn fsck(&self) -> Result<(), DurableError> {
        let repo = self.open_git()?;
        let odb = repo.odb().map_err(|e| git_err("odb", e))?;
        // (a) every object in the odb parses (libgit2 verifies the object on read).
        let mut count = 0usize;
        odb.foreach(|oid| {
            if odb.read(*oid).is_err() {
                return false; // abort the walk → reported as a corrupt object below.
            }
            count += 1;
            true
        })
        .map_err(|e| git_err("odb foreach (corrupt object?)", e))?;
        // (b) every ref points at an object present in the odb (no dangling ref).
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

    // ── push intake: migrate a sandbox-validated quarantine object into the durable odb (CT-006d) ──

    /// **Write a raw `(type, payload)` git object into this repo's on-disk odb (CT-006d push migration).**
    /// `kind` is `commit`/`tree`/`blob`/`tag`; `payload` is the object body WITHOUT the `"<type> <len>\0"`
    /// header (exactly what `git cat-file --batch` emits and what `read_object` returns). git2 RE-HASHES
    /// the content and returns the computed oid — a forged/mismatched oid is structurally impossible, and
    /// a content-addressed re-write of an object that already exists is an idempotent no-op. This is the
    /// TRUSTED in-process migration: the sandboxed `index-pack` already validated the untrusted pack; the
    /// host only promotes the resulting fully-resolved objects, AFTER the in-process policy admits them.
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

    /// Whether `tip` is a real commit whose OWN root tree is present + readable in the odb — the
    /// **tip-only** slice (trees + blobs reachable from THIS commit's tree exist). It walks exactly one
    /// commit's tree and says NOTHING about the commit's ancestry.
    ///
    /// **R0.7-D / DELTA N4 — why this is NOT the full connectivity check.** A push whose tip tree is
    /// complete can still reference a MISSING ANCESTOR commit (`index-pack --fix-thin` resolves delta
    /// bases, never missing parent COMMITS). Accepting on the tip-tree alone lets one crafted push wedge
    /// a branch's clonability: the accept gate says "ok", but a later `clone`/`fetch` fails client-side
    /// walking into the absent parent — a durable-integrity DoS. The push-accept gate MUST instead use
    /// [`Self::history_connectivity_complete`], which verifies EVERY new commit's tree AND that every
    /// parent oid is present. This method is retained only as the single-commit tree slice (reused by
    /// the full walk via the shared [`Self::tree_objects_present`] helper).
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

    /// The shared tree-walk: whether EVERY tree/blob reachable from `tree` is present in `odb`. Factored
    /// out of [`Self::commit_tree_complete`] so the full connectivity walk
    /// ([`Self::history_connectivity_complete`]) checks each commit's tree with the SAME logic (no
    /// duplicated object walking — the anti-duplication discipline). A missing tree/blob → `Ok(false)`
    /// (a walk that aborts early); only a libgit2 walk failure surfaces as `Err`.
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

    /// **R0.7-D / DELTA N4 (MEDIUM) — full push-connectivity check: is EVERY object reachable from
    /// `new_tip` and NOT already reachable from `existing_tips` present + connected in the odb?**
    ///
    /// The push-accept gate MUST call this INSTEAD of the tip-only [`Self::commit_tree_complete`]. The
    /// tip-only check verifies just the tip commit's own tree, so a crafted push whose tip references a
    /// MISSING ANCESTOR commit is accepted (the tip's tree is complete) yet leaves the branch
    /// un-clonable — a later `clone`/`fetch` fails walking into the absent parent. That is a durable-
    /// integrity DoS one push can inflict; this walk closes it by proving the WHOLE newly-introduced
    /// history is self-contained before the ref moves.
    ///
    /// **The walk (thin-push cheap).** A libgit2 revwalk pushes `new_tip` and HIDES each `existing_tips`
    /// entry, so only the commits this push actually INTRODUCES are visited — a thin push pays for its
    /// delta, not the whole history each time. `existing_tips` empty (a repo/branch CREATE) is correct:
    /// the walk then covers the full new history, which a fresh branch must be entirely self-contained
    /// to satisfy. Hiding a non-existent / unparseable existing tip is done gracefully (skipped): it
    /// contributes nothing to reachability, so failing to hide it only WIDENS the walk — the fail-closed
    /// direction (we verify more, never less).
    ///
    /// For EACH new commit the walk yields we assert three things, reusing existing helpers:
    /// - the commit object exists (`find_commit`),
    /// - its root tree is complete ([`Self::tree_objects_present`] — the shared tree walk), and
    /// - EVERY parent oid is present in the odb (`odb.exists`) — a missing ancestor commit.
    ///
    /// **Fail-closed mapping (deliberate).** A genuinely-missing ancestor manifests two ways, and BOTH
    /// map to `Ok(false)` (REJECT the push), never `Err`:
    /// 1. `odb.exists(parent)` is `false` for a walked commit's parent — the deterministic catch;
    /// 2. the revwalk step itself ERRORS because libgit2 tried to load a missing parent to continue the
    ///    traversal — mapped to `Ok(false)`.
    ///
    /// Returning `Err` for a missing object would be dangerous: a caller might treat an `Err` as a
    /// transient/infra failure and retry-then-accept, re-opening the hole. So a missing object is a
    /// hard, first-class REJECT (`Ok(false)`), distinct from the genuine infrastructure errors that DO
    /// surface as `Err` (`open_git` / `odb` acquisition / a `new_tip` that is not a parseable oid). On
    /// any doubt within the walk we fail CLOSED — reject, never accept.
    pub fn history_connectivity_complete(
        &self,
        new_tip: &Oid,
        existing_tips: &[Oid],
    ) -> Result<bool, DurableError> {
        let repo = self.open_git()?;
        let odb = repo.odb().map_err(|e| git_err("odb", e))?;
        let tip_g = Self::parse_oid(new_tip)?;
        // The new tip itself must be a present commit — a ref never advances to a missing/non-commit
        // tip (mirrors the tip-only check's first guard).
        if repo.find_commit(tip_g).is_err() {
            return Ok(false);
        }

        let mut walk = repo.revwalk().map_err(|e| git_err("revwalk", e))?;
        walk.push(tip_g)
            .map_err(|e| git_err("revwalk push new_tip", e))?;
        // Hide each already-reachable existing tip → only the NEW commits are walked (thin-push cheap).
        // A tip we cannot parse or that is absent from the odb is skipped gracefully (see doc): failing
        // to hide only widens the walk, which is the fail-closed direction.
        for t in existing_tips {
            if let Ok(g) = Self::parse_oid(t) {
                if odb.exists(g) {
                    // A hide of a genuinely-present commit; ignore a benign libgit2 hide error (still
                    // fail-closed — an un-hidden tip only means we verify more objects).
                    let _ = walk.hide(g);
                }
            }
        }

        for step in walk {
            // A revwalk step that ERRORS is libgit2 failing to load a commit it must traverse (a
            // missing ancestor) → REJECT (fail closed), never a swallowed-into-accept `Err`.
            let commit_oid = match step {
                Ok(o) => o,
                Err(_) => return Ok(false),
            };
            let commit = match repo.find_commit(commit_oid) {
                Ok(c) => c,
                Err(_) => return Ok(false),
            };
            // (a) this commit's own tree must be complete (the shared tree-walk helper).
            let tree = match commit.tree() {
                Ok(t) => t,
                Err(_) => return Ok(false),
            };
            if !Self::tree_objects_present(&odb, &tree)? {
                return Ok(false);
            }
            // (b) every PARENT commit oid must be present in the odb — the missing-ancestor catch the
            // tip-only check lacked. A boundary commit whose parent is the absent ancestor is itself
            // NEW (reachable, not hidden), so this deterministically rejects the wedge push.
            for parent_oid in commit.parent_ids() {
                if !odb.exists(parent_oid) {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
}

// ───────────────────────────── the rooted durable store (repo lifecycle) ─────────────────────────

/// **The durable on-disk git store** — repo lifecycle (`init_bare` / open / exists) over a
/// [`RepoPathResolver`], the WRITE-side companion to the read backend [`GixCore`]. Generic over the
/// resolver so a test injects a temp root and the serving tier injects the real placement resolver
/// (the same seam [`GixCore`] uses — GIT-P13). The default resolver is [`RootedResolver`]
/// (`<root>/<tenant>/<region>/<repo>.git`).
pub struct DurableGitStore<P: RepoPathResolver = RootedResolver> {
    resolver: P,
}

impl DurableGitStore<RootedResolver> {
    /// Root a durable store at a directory holding `<tenant>/<region>/<repo>.git` bare repos — the
    /// v1 local-NVMe layout (the SAME root [`GixCore`]'s [`RootedResolver`] reads from, so the write
    /// path and the read path open the same repos).
    pub fn rooted(root: impl Into<PathBuf>) -> Self {
        Self {
            resolver: RootedResolver::new(root),
        }
    }
}

impl<P: RepoPathResolver> DurableGitStore<P> {
    /// Build the durable store over a repo-path resolver (the serving-tier placement resolver swaps
    /// in here behind the same port [`GixCore`] uses).
    pub fn new(resolver: P) -> Self {
        Self { resolver }
    }

    /// The on-disk path a repo resolves to (`<root>/<tenant>/<region>/<repo>.git`). The tenant/region
    /// pathing IS the isolation boundary — a repo under tenant A's locator never resolves under
    /// tenant B's.
    pub fn repo_path(&self, repo: &RepoLoc) -> Result<PathBuf, DurableError> {
        self.resolver
            .repo_path(repo)
            .map_err(|e| DurableError::Git(e.to_string()))
    }

    /// **Create a repo on disk** = `git2::Repository::init_bare` at the resolver path (creating the
    /// `<tenant>/<region>/` parent dirs first). Idempotent: if the bare repo already exists it is
    /// opened, not clobbered. Sets `core.logallrefupdates=true` so ref CASes are reflog-logged
    /// durably (bare repos default it off).
    pub fn create_repo(&self, repo: &RepoLoc) -> Result<DurableGitRepo, DurableError> {
        let path = self.repo_path(repo)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| DurableError::Io(format!("create parent {}: {e}", parent.display())))?;
        }
        let git_repo = git2::Repository::init_bare(&path)
            .map_err(|e| git_err(&format!("init_bare {}", path.display()), e))?;
        // Durable reflog for ALL refs (a bare repo defaults logallrefupdates off; arch §3 reflog).
        git_repo
            .config()
            .and_then(|mut c| c.set_bool("core.logallrefupdates", true))
            .map_err(|e| git_err("enable logallrefupdates", e))?;
        Ok(DurableGitRepo { path })
    }

    /// **Open an existing repo on disk.** `NotFound` if the bare repo is not present (the lifecycle
    /// reject the front door surfaces — never auto-create on a read path).
    pub fn open_repo(&self, repo: &RepoLoc) -> Result<DurableGitRepo, DurableError> {
        let path = self.repo_path(repo)?;
        // Probe with a real open so a missing/!valid repo is a clean NotFound, not a later op error.
        // Peer-review finding 2026-07-16 #5: the NotFound message names the repo by its LOGICAL slug
        // (which the caller already supplied), NEVER the on-disk `path.display()` — a granted-but-
        // missing repo must not leak the server's filesystem layout (the authz guard runs upstream, so
        // this is not an existence oracle, but the host path is still not the client's to see).
        git2::Repository::open(&path)
            .map_err(|_| DurableError::NotFound(format!("bare repo {}", repo.repo)))?;
        Ok(DurableGitRepo { path })
    }

    /// Whether a repo exists on disk (a valid bare git repo at the resolver path).
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

    /// A unique temp root under the scratch dir for an isolated on-disk store per test.
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

    /// Build a real, `fsck`-clean commit (blob → tree → commit) authored to a tenant pseudonym.
    fn seed_commit(repo: &DurableGitRepo, content: &[u8]) -> Oid {
        let blob = repo.write_blob(content).expect("blob");
        let tree = repo.write_tree(&[("file.txt", &blob)]).expect("tree");
        repo.write_commit(&tree, &[], "feat: seed", "psn-7@acme.noreply", "psn-7@acme.noreply")
            .expect("commit")
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
        let second_repo = store.open_repo(&loc()).expect("second process-style handle");
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

    /// **Repo lifecycle on disk: create = init_bare; the bare repo is a real on-disk git dir.**
    #[test]
    fn create_repo_inits_a_real_on_disk_bare_repo() {
        let root = temp_root("lifecycle");
        let store = DurableGitStore::rooted(&root);
        assert!(!store.repo_exists(&loc()), "absent before create");

        let repo = store.create_repo(&loc()).expect("create");
        // The path is exactly <root>/<tenant>/<region>/<repo>.git
        assert_eq!(repo.path(), root.join("acme").join("fr-par").join("core.git"));
        assert!(repo.path().is_dir(), "the bare repo is a real on-disk directory");
        assert!(store.repo_exists(&loc()), "present after create");
        // Idempotent: a second create opens, does not clobber.
        assert!(store.create_repo(&loc()).is_ok());

        std::fs::remove_dir_all(&root).ok();
    }

    /// **F9 (R4.1 dogfood) — the first push heals a dangling HEAD so a fresh `git clone` checks out.**
    /// `init_bare` leaves HEAD symbolically at `refs/heads/master`, but pushes land on `main` — so HEAD
    /// dangles until healed and a clone warns "remote HEAD refers to nonexistent ref, unable to
    /// checkout". After landing the first branch on `main` + [`DurableGitRepo::heal_head_symref`], HEAD
    /// resolves to `refs/heads/main`.
    #[test]
    fn f9_heal_head_symref_points_head_at_the_pushed_default_branch() {
        let root = temp_root("f9-head");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");

        // A fresh bare repo: HEAD is unborn/dangling (points at the nonexistent `master`).
        let g = git2::Repository::open(repo.path()).unwrap();
        assert!(g.head().is_err(), "fresh init_bare HEAD dangles (a clone would warn)");

        // Land the first branch on `main` (exactly what a real first push does).
        let c = seed_commit(&repo, b"first push\n");
        repo.update_ref_cas("refs/heads/main", None, Some(&c), "create", "psn@acme.noreply")
            .expect("create main");
        // Still dangling until we heal — the read-side workaround exists, but a clone reads on-disk HEAD.
        assert!(
            git2::Repository::open(repo.path()).unwrap().head().is_err(),
            "HEAD still dangles after the push until it is healed"
        );

        repo.heal_head_symref().expect("heal HEAD");

        // HEAD now resolves to refs/heads/main → a fresh clone checks out `main` with no warning.
        let g2 = git2::Repository::open(repo.path()).unwrap();
        let head = g2.head().expect("HEAD resolves after heal");
        assert_eq!(
            head.name().unwrap(),
            "refs/heads/main",
            "F9: HEAD points at the pushed default branch"
        );

        // Idempotent + non-clobbering: a repo whose HEAD already resolves is left untouched.
        repo.heal_head_symref().expect("heal is idempotent");
        assert_eq!(
            git2::Repository::open(repo.path()).unwrap().head().unwrap().name().unwrap(),
            "refs/heads/main"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// **F9 — a first push to a NON-`main` branch heals HEAD to that branch (git's own behavior).** If
    /// the first branch a repo receives is e.g. `develop`, HEAD should follow it (not stay dangling).
    #[test]
    fn f9_heal_head_symref_follows_the_first_branch_when_no_main() {
        let root = temp_root("f9-head-develop");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");
        let c = seed_commit(&repo, b"develop\n");
        repo.update_ref_cas("refs/heads/develop", None, Some(&c), "create", "psn@acme.noreply")
            .expect("create develop");
        repo.heal_head_symref().expect("heal");
        assert_eq!(
            git2::Repository::open(repo.path()).unwrap().head().unwrap().name().unwrap(),
            "refs/heads/develop",
            "F9: with no main, HEAD follows the first branch pushed"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// **THE CORE PROOF — durability across restart.** Write a ref + a real commit object via one
    /// store, then open a FRESH store + handle over the SAME on-disk root (a simulated process
    /// restart) and read both back — present + correct. A test that hit an in-memory store would NOT
    /// survive the fresh handle.
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
        } // drop everything — nothing in-memory carries over.

        // A completely FRESH store + handle over the same root (the "restart").
        let store2 = DurableGitStore::rooted(&root);
        let repo2 = store2.open_repo(&loc()).expect("open after restart");
        assert_eq!(
            repo2.read_ref("refs/heads/main").expect("read ref"),
            Some(commit.clone()),
            "the ref survived the restart (SI-012 fixed — open loads from disk)"
        );
        assert!(
            repo2.has_object(&commit),
            "the commit object survived the restart (F-git-2 — on-disk odb)"
        );
        // The object bytes round-trip (it is a real git commit).
        let bytes = repo2
            .read_object_bounded(&commit, 64 * 1024 * 1024)
            .expect("read object");
        assert!(
            std::str::from_utf8(&bytes).unwrap().contains("psn-7@acme.noreply"),
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
        // list_refs loads the entry point from disk (not an empty map).
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

    /// A fork PR's locked head can live in a physically distinct bare repository. Importing it
    /// installs the full commit/tree/blob ancestry in the target ODB without copying any source ref.
    #[test]
    fn fork_import_moves_verified_commit_closure_without_source_refs() {
        let root = temp_root("fork-import");
        let store = DurableGitStore::rooted(&root);
        let source_loc = RepoLoc::new("acme", "fr-par", "contributor-fork");
        let target_loc = RepoLoc::new("acme", "fr-par", "core");
        let source = store.create_repo(&source_loc).expect("create source");
        let target = store.create_repo(&target_loc).expect("create target");
        assert_ne!(source.path(), target.path(), "the proof must use distinct ODBs");

        let parent = seed_commit(&source, b"parent from fork\n");
        let child_blob = source.write_blob(b"locked fork head\n").expect("child blob");
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
        assert!(target.has_object(&child_blob), "referenced tree/blob closure imported");
        assert_eq!(
            target
                .read_ref("refs/heads/contributor/change")
                .expect("target ref read"),
            None,
            "object import must not copy or create source refs"
        );
        assert!(target.commit_tree_complete(&child).expect("target connectivity"));

        std::fs::remove_dir_all(&root).ok();
    }

    /// **GT-004 browse: the commit log + commit diff read the REAL on-disk graph** (libgit2 revwalk +
    /// tree diff), newest-first, paginated, with the root commit diffing against the empty tree.
    #[test]
    fn commit_log_and_diff_read_the_real_graph() {
        let root = temp_root("log");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");

        let b1 = repo.write_blob(b"line one\n").unwrap();
        let t1 = repo.write_tree(&[("file.txt", &b1)]).unwrap();
        let c1 = repo
            .write_commit(&t1, &[], "feat: first", "psn@acme.noreply", "psn@acme.noreply")
            .unwrap();
        repo.update_ref_cas("refs/heads/main", None, Some(&c1), "create", "psn@acme.noreply")
            .unwrap();

        let b2 = repo.write_blob(b"line one\nline two\n").unwrap();
        let t2 = repo.write_tree(&[("file.txt", &b2)]).unwrap();
        let c2 = repo
            .write_commit(&t2, &[&c1], "feat: second", "psn@acme.noreply", "psn@acme.noreply")
            .unwrap();
        repo.update_ref_cas("refs/heads/main", Some(&c1), Some(&c2), "ff", "psn@acme.noreply")
            .unwrap();

        // Newest-first, both commits, no more within a generous page.
        let (rows, more) = repo.commit_log("refs/heads/main", 0, 10).expect("log");
        assert!(!more);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].oid, c2.0);
        assert_eq!(rows[0].summary, "feat: second");
        assert_eq!(rows[0].parents, vec![c1.0.clone()]);
        assert_eq!(rows[1].oid, c1.0);

        // Pagination: page-of-1 reports has_more; offset 1 returns the older commit.
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

        // Diff of c2 vs its parent: file.txt MODIFIED with an added "line two".
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
            CommitDiffLimits { files: 0, ..COMMIT_DIFF_LIMITS },
            CommitDiffLimits { lines_per_file: 0, ..COMMIT_DIFF_LIMITS },
            CommitDiffLimits { line_bytes: 1, ..COMMIT_DIFF_LIMITS },
            CommitDiffLimits { rendered_bytes: 1, ..COMMIT_DIFF_LIMITS },
            CommitDiffLimits { message_bytes: 1, ..COMMIT_DIFF_LIMITS },
        ] {
            assert!(matches!(
                repo.commit_detail_bounded(&c2.0, limits),
                Err(DurableError::Git(message)) if message.starts_with("commit diff computation limit exceeded:")
            ));
        }

        // The ROOT commit diffs against the empty tree → file.txt ADDED.
        let root_detail = repo.commit_detail(&c1.0).unwrap().unwrap();
        assert_eq!(root_detail.files[0].status, 'A');

        // A malformed/absent oid → None (a clean 404 upstream; never a panic).
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

        // Moving the mutable base ref after page one does not alter the cursor-pinned snapshot.
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

    /// **R3.2 · G-7 N1 — the PR diff is THREE-DOT: `merge-base(base, head) … head`.** It shows the
    /// PR's OWN changes and excludes drift the base picked up after the branch point. Line numbers
    /// (old_no/new_no), hunk boundaries, and status are all correct.
    #[test]
    fn pr_diff_is_three_dot_and_carries_line_numbers() {
        let root = temp_root("prdiff");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");

        // base @ main: file.txt = a,b,c
        let b0 = repo.write_blob(b"a\nb\nc\n").unwrap();
        let t0 = repo.write_tree(&[("file.txt", &b0)]).unwrap();
        let base = repo
            .write_commit(&t0, &[], "base", "psn@acme.noreply", "psn@acme.noreply")
            .unwrap();
        repo.update_ref_cas("refs/heads/main", None, Some(&base), "create", "psn@acme.noreply")
            .unwrap();

        // head branch off base: modify line 2 + add line 4
        let bh = repo.write_blob(b"a\nB\nc\nd\n").unwrap();
        let th = repo.write_tree(&[("file.txt", &bh)]).unwrap();
        let head = repo
            .write_commit(&th, &[&base], "head", "psn@acme.noreply", "psn@acme.noreply")
            .unwrap();

        // DRIFT: main advances past the branch point with an unrelated file the PR never touched.
        let bd = repo.write_blob(b"unrelated\n").unwrap();
        let td = repo.write_tree(&[("file.txt", &b0), ("other.txt", &bd)]).unwrap();
        let drift = repo
            .write_commit(&td, &[&base], "drift on main", "psn@acme.noreply", "psn@acme.noreply")
            .unwrap();
        repo.update_ref_cas("refs/heads/main", Some(&base), Some(&drift), "ff", "psn@acme.noreply")
            .unwrap();

        let diff = repo.pr_diff("refs/heads/main", &head.0, 4000).unwrap().unwrap();
        assert!(diff.three_dot, "durable repos are libgit2-backed → real merge-base");
        assert_eq!(diff.base_oid, base.0, "base = merge-base(main, head), NOT main's tip");
        // ONLY file.txt — other.txt is main's drift, NOT the PR's change (three-dot excludes it).
        assert_eq!(diff.total_files, 1, "three-dot shows only the PR's own files");
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
        assert_eq!(diff.files[0].additions, 2, "line 2 changed (+B) + line 4 added (+d)");
        assert_eq!(diff.files[0].deletions, 1, "line 2's old (-b)");
        // Line numbers: the added "d" carries new_no == 4 and old_no == None.
        let hunk = &diff.files[0].hunks[0];
        let added_d = hunk.lines.iter().find(|l| l.origin == '+' && l.content == "d").unwrap();
        assert_eq!(added_d.new_no, Some(4));
        assert_eq!(added_d.old_no, None);
        let removed_b = hunk.lines.iter().find(|l| l.origin == '-' && l.content == "b").unwrap();
        assert_eq!(removed_b.old_no, Some(2));
        assert_eq!(removed_b.new_no, None);
        // Context line "a" carries BOTH numbers.
        let ctx_a = hunk.lines.iter().find(|l| l.origin == ' ' && l.content == "a").unwrap();
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
        for (line_bytes, rendered_bytes) in [
            (0, DIFF_MAX_RENDERED_BYTES),
            (DIFF_MAX_LINE_BYTES, 1),
        ] {
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

        // A malformed/absent head → None (the edge renders the empty state, not a 500).
        assert!(repo.pr_diff("refs/heads/main", "not-an-oid", 4000).unwrap().is_none());
        std::fs::remove_dir_all(&root).ok();
    }

    /// **R3.2 verifier HOLD: a huge SINGLE-HUNK file (a new file = one `@@ -0,0 +1,N @@` hunk) is
    /// capped, not dumped wholesale.** The cap must bind WITHIN a hunk (the pre-fix code only dropped
    /// whole trailing hunks, so a one-hunk file sailed past the cap). The diffstat stays truthful.
    #[test]
    fn pr_diff_caps_a_single_huge_hunk_not_only_at_hunk_boundaries() {
        let root = temp_root("prcap");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");

        // base: empty tree (no files).
        let t0 = repo.write_tree(&[]).unwrap();
        let base = repo
            .write_commit(&t0, &[], "base", "psn@acme.noreply", "psn@acme.noreply")
            .unwrap();
        repo.update_ref_cas("refs/heads/main", None, Some(&base), "create", "psn@acme.noreply")
            .unwrap();

        // head: add ONE new 5000-line file — a single `@@ -0,0 +1,5000 @@` hunk.
        let big: String = (0..5000).map(|i| format!("line {i}\n")).collect();
        let bh = repo.write_blob(big.as_bytes()).unwrap();
        let th = repo.write_tree(&[("big.txt", &bh)]).unwrap();
        let head = repo
            .write_commit(&th, &[&base], "add big", "psn@acme.noreply", "psn@acme.noreply")
            .unwrap();

        let cap = 100;
        let diff = repo.pr_diff("refs/heads/main", &head.0, cap).unwrap().unwrap();
        assert_eq!(diff.total_files, 1);
        let f = &diff.files[0];
        assert_eq!(f.path, "big.txt");
        assert!(f.truncated, "a file over the cap MUST be flagged truncated");
        let rendered: usize = f.hunks.iter().map(|h| h.lines.len()).sum();
        assert!(rendered <= cap, "rendered lines ({rendered}) must not exceed the cap ({cap})");
        assert_eq!(f.additions, 5000, "the diffstat still reports the TRUE addition count");
        // Uncapped (cap=0) renders the whole file — the opt-out path.
        let full = repo.pr_diff("refs/heads/main", &head.0, 0).unwrap().unwrap();
        let full_rendered: usize = full.files[0].hunks.iter().map(|h| h.lines.len()).sum();
        assert_eq!(full_rendered, 5000, "cap=0 is uncapped");
        assert!(!full.files[0].truncated);
        std::fs::remove_dir_all(&root).ok();
    }

    /// **R3.2 · G-7 — a binary file diffs as `kind=binary` with NO text hunks (never a garbled dump).**
    #[test]
    fn pr_diff_flags_binary_with_no_hunk_dump() {
        let root = temp_root("prbin");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");
        let base = seed_commit(&repo, b"a\n");
        repo.update_ref_cas("refs/heads/main", None, Some(&base), "c", "psn@acme.noreply").unwrap();
        // A NUL-containing blob is binary to libgit2.
        let bin = repo.write_blob(&[0u8, 1, 2, 0, 255, 3]).unwrap();
        let tb = repo.write_tree(&[("logo.png", &bin)]).unwrap();
        let head = repo
            .write_commit(&tb, &[&base], "add binary", "psn@acme.noreply", "psn@acme.noreply")
            .unwrap();
        let diff = repo.pr_diff("refs/heads/main", &head.0, 4000).unwrap().unwrap();
        let f = diff.files.iter().find(|f| f.path == "logo.png").unwrap();
        assert_eq!(f.kind, FileKind::Binary);
        assert_eq!(f.new_blob_oid.as_deref(), Some(bin.0.as_str()));
        assert!(f.hunks.is_empty(), "a binary file carries NO text hunks");
        assert!(f.size_bytes.is_some(), "the size is available for the binary row");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn pr_diff_omits_new_blob_oid_for_a_deletion() {
        let root = temp_root("prdeletedoid");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");
        let base = seed_commit(&repo, b"removed\n");
        repo.update_ref_cas("refs/heads/main", None, Some(&base), "c", "psn@acme.noreply")
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
        let diff = repo.pr_diff("refs/heads/main", &head.0, 4000).unwrap().unwrap();
        assert_eq!(diff.files[0].status, 'D');
        assert_eq!(diff.files[0].new_blob_oid, None);
        std::fs::remove_dir_all(&root).ok();
    }

    /// **R3.2 · G-7 N2 — expand-context reads a blob's lines; a binary blob never dumps garbled text.**
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
        assert_eq!(exact_lines.last().unwrap().new_no, Some(FILE_LINES_MAX_RANGE as u32));
        // A malformed oid → Missing (a stale expand never 500s).
        assert_eq!(
            repo.file_lines("not-an-oid", 1, 10).unwrap(),
            FileLinesLookup::Missing,
        );
        // A binary blob → an empty expansion (never a garbled dump).
        let bin = repo.write_blob(&[0u8, 1, 2, 0]).unwrap();
        assert_eq!(repo.file_lines(&bin.0, 1, 10).unwrap(), FileLinesLookup::Binary);

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

    /// **fsck-clean: the in-process integrity check passes on a well-formed repo.** (The full
    /// external `git fsck` oracle runs in `tests/`.)
    #[test]
    fn fsck_is_clean_on_a_well_formed_repo() {
        let root = temp_root("fsck");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");
        let commit = seed_commit(&repo, b"fsck me\n");
        repo.update_ref_cas("refs/heads/main", None, Some(&commit), "create", "psn@acme.noreply")
            .expect("ref");
        repo.fsck().expect("fsck clean on a valid repo");
        std::fs::remove_dir_all(&root).ok();
    }

    /// **Ref CAS: a stale expected-old is REJECTED (the ref does not move).**
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

        repo.update_ref_cas("refs/heads/main", None, Some(&c1), "create", "psn@acme.noreply")
            .expect("create");

        // A CAS that believes main is still absent (stale) is rejected; the ref stays at c1.
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

        // A correct CAS (expected = c1) moves it to c2 and bumps the reflog generation.
        repo.update_ref_cas(
            "refs/heads/main",
            Some(&c1),
            Some(&c2),
            "ff to v2",
            "psn@acme.noreply",
        )
        .expect("ff update");
        assert_eq!(repo.read_ref("refs/heads/main").unwrap(), Some(c2));
        assert_eq!(repo.reflog_len("refs/heads/main"), Ok(2), "two updates logged");
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

    /// **Tenant isolation by path: a repo under tenant A is NOT reachable via tenant B's locator.**
    #[test]
    fn tenant_isolation_by_path() {
        let root = temp_root("isolation");
        let store = DurableGitStore::rooted(&root);
        let a = RepoLoc::new("tenant-a", "fr-par", "secret");
        let b = RepoLoc::new("tenant-b", "fr-par", "secret");

        let repo_a = store.create_repo(&a).expect("create a");
        let commit = seed_commit(&repo_a, b"tenant a private\n");
        repo_a
            .update_ref_cas("refs/heads/main", None, Some(&commit), "create", "psn@tenant-a.noreply")
            .expect("ref a");

        // Tenant B's locator resolves to a DIFFERENT path; B's repo does not even exist yet.
        assert_ne!(store.repo_path(&a).unwrap(), store.repo_path(&b).unwrap());
        assert!(store.repo_exists(&a));
        assert!(!store.repo_exists(&b), "tenant B cannot reach A's repo by path");
        // Even after B creates its own repo, A's object/ref are not visible in B's odb.
        let repo_b = store.create_repo(&b).expect("create b");
        assert!(
            !repo_b.has_object(&commit),
            "tenant A's object is NOT in tenant B's on-disk odb (path isolation)"
        );
        assert_eq!(
            repo_b.read_ref("refs/heads/main").unwrap(),
            None,
            "tenant B's main is empty — A's ref did not bleed across the tenant path"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// **R0.4 / git #1 HIGH — a delete+recreate does NOT reset the durable generation.** A delete CAS
    /// removes the ref durably; the ref's on-disk REFLOG restarts (that is libgit2 behaviour — the
    /// reflog dies with the ref), but the durable per-ref GENERATION counter is keyed by name in config
    /// and is monotonic ACROSS the delete: create→delete→recreate advances it 1→2→3, never resetting.
    /// This is exactly the invariant reflog-length-as-generation violated (git #1 HIGH): reflog_len
    /// resets to 1 on recreate while `ref_generation` correctly reaches 3.
    #[test]
    fn delete_cas_removes_ref_but_does_not_reset_durable_generation() {
        let root = temp_root("delete");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).expect("create");
        let c1 = seed_commit(&repo, b"x\n");
        repo.update_ref_cas("refs/heads/tmp", None, Some(&c1), "create", "psn@acme.noreply")
            .unwrap();
        assert_eq!(repo.read_ref("refs/heads/tmp").unwrap(), Some(c1.clone()));
        assert_eq!(repo.ref_generation("refs/heads/tmp"), Ok(1), "create is generation 1");

        repo.update_ref_cas("refs/heads/tmp", Some(&c1), None, "delete", "psn@acme.noreply")
            .expect("delete");
        assert_eq!(repo.read_ref("refs/heads/tmp").unwrap(), None, "ref deleted");
        assert_eq!(
            repo.ref_generation("refs/heads/tmp"),
            Ok(2),
            "the delete ADVANCES the durable generation (a delete is a generation-advancing event)"
        );

        repo.update_ref_cas("refs/heads/tmp", None, Some(&c1), "recreate", "psn@acme.noreply")
            .expect("recreate");
        // libgit2 restarts the ref's reflog on recreate — that is the OLD (wrong) generation source.
        assert_eq!(
            repo.reflog_len("refs/heads/tmp"),
            Ok(1),
            "the recreated ref's reflog restarts (libgit2 behaviour — why reflog_len was wrong)"
        );
        // The DURABLE generation does NOT reset — it keeps climbing across the delete (the fix).
        assert_eq!(
            repo.ref_generation("refs/heads/tmp"),
            Ok(3),
            "the durable per-ref generation is monotonic across delete+recreate (R0.4 fix)"
        );

        // And it survives a restart: a FRESH store + handle over the same root reads the same value.
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
        assert_eq!(repo.ref_generation(ref_name), Ok(0), "an absent counter starts at zero");

        let raw = repo.open_git().expect("open raw repo");
        let mut cfg = raw.config().expect("open config");
        cfg.set_str(&refgen_key(ref_name), "not-an-integer")
            .expect("write malformed fixture");
        assert!(
            matches!(repo.ref_generation(ref_name), Err(DurableError::Git(_))),
            "a malformed counter must not be treated as generation zero"
        );

        cfg.set_i64(&refgen_key(ref_name), -1).expect("write negative fixture");
        assert_eq!(
            repo.ref_generation(ref_name),
            Err(DurableError::Git(format!("negative ref generation stored for {ref_name}")))
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
                repo.reflog_entries_bounded(
                    "refs/heads/main",
                    10,
                    REFLOG_MAX_BYTES_PER_REF
                ),
                Err(DurableError::Git(_))
            ),
            "invalid audit identity bytes must not become an empty committer"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    // ── R0.7-D / DELTA N4: full push-connectivity (missing-ancestor rejection) ──

    /// Copy one object's raw bytes from `src`'s odb into `dst`'s odb, preserving its oid (git2
    /// re-hashes on write, so we assert the oid is identical — a forged copy is impossible). Used to
    /// stage a `dst` odb that is MISSING a chosen ancestor commit while its tip tree is complete.
    fn copy_object(src: &DurableGitRepo, dst: &DurableGitRepo, oid: &Oid, kind: &str) {
        let bytes = src
            .read_object_bounded(oid, 64 * 1024 * 1024)
            .expect("read src object");
        let written = dst.write_raw_object(kind, &bytes).expect("write dst object");
        assert_eq!(written.0, oid.0, "the re-hashed copy keeps the same oid");
    }

    /// Build a linear `c1 <- c2 <- c3` history in a fresh source repo and return
    /// `(root, repo, [(blob,tree,commit); 3])` so a target odb can be assembled with a chosen subset.
    #[allow(clippy::type_complexity)]
    fn seed_three_commit_history() -> (PathBuf, DurableGitRepo, Vec<(Oid, Oid, Oid)>) {
        let root = temp_root("conn-src");
        let repo = DurableGitStore::rooted(&root).create_repo(&loc()).expect("create src");
        let mut chain: Vec<(Oid, Oid, Oid)> = Vec::new();
        let mut parent: Option<Oid> = None;
        for i in 0..3u8 {
            let blob = repo.write_blob(format!("line {i}\n").as_bytes()).unwrap();
            let tree = repo.write_tree(&[("file.txt", &blob)]).unwrap();
            let parents: Vec<&Oid> = parent.iter().collect();
            let commit = repo
                .write_commit(&tree, &parents, &format!("c{i}"), "psn@acme.noreply", "psn@acme.noreply")
                .unwrap();
            parent = Some(commit.clone());
            chain.push((blob, tree, commit));
        }
        (root, repo, chain)
    }

    /// **THE R0.7-D REGRESSION (DELTA N4).** A push whose TIP commit's tree is COMPLETE but whose
    /// PARENT (ancestor) commit is MISSING from the odb must be REJECTED by the full-connectivity check
    /// — this is exactly the state the tip-only [`DurableGitRepo::commit_tree_complete`] ACCEPTS today
    /// (proven here) and that would wedge the branch's clonability. `existing_tips` is empty (a branch
    /// create): a fresh branch must be fully self-contained.
    #[test]
    fn history_connectivity_rejects_a_missing_ancestor_commit() {
        let (src_root, src, chain) = seed_three_commit_history();
        let (b1, t1, c1) = chain[0].clone();
        let (b2, t2, c2) = chain[1].clone();
        let (b3, t3, c3) = chain[2].clone();

        // Target odb: everything EXCEPT the ANCESTOR commit c1 (its tree/blob copied so nothing else
        // is missing — the ONLY hole is the parent COMMIT c1).
        let dst_root = temp_root("conn-dst-missing");
        let dst = DurableGitStore::rooted(&dst_root).create_repo(&loc()).expect("create dst");
        copy_object(&src, &dst, &b1, "blob");
        copy_object(&src, &dst, &t1, "tree");
        copy_object(&src, &dst, &b2, "blob");
        copy_object(&src, &dst, &t2, "tree");
        copy_object(&src, &dst, &b3, "blob");
        copy_object(&src, &dst, &t3, "tree");
        copy_object(&src, &dst, &c2, "commit");
        copy_object(&src, &dst, &c3, "commit");
        // c1 (the ancestor commit) is deliberately NOT copied.
        assert!(!dst.has_object(&c1), "the ancestor commit is absent from the target odb");

        // What passes today: the tip's OWN tree is complete (the tip-only slice says "ok").
        assert!(
            dst.commit_tree_complete(&c3).unwrap(),
            "the tip-only check ACCEPTS — the tip's tree is complete (this is the hole)"
        );
        // The FIX: full connectivity REJECTS (missing ancestor → a branch a clone cannot walk).
        assert!(
            !dst.history_connectivity_complete(&c3, &[]).unwrap(),
            "R0.7-D: a missing ANCESTOR commit rejects the push (fail-closed) — the ref must not move"
        );

        std::fs::remove_dir_all(&src_root).ok();
        std::fs::remove_dir_all(&dst_root).ok();
    }

    /// A normal push whose FULL history is present is ACCEPTED (the full-connectivity walk finds every
    /// commit + tree + parent). Both the branch-create (`existing_tips == []`) form and the tip-only
    /// slice agree here — the fix is never MORE permissive on a well-formed push.
    #[test]
    fn history_connectivity_accepts_full_history() {
        let (src_root, src, chain) = seed_three_commit_history();
        let dst_root = temp_root("conn-dst-full");
        let dst = DurableGitStore::rooted(&dst_root).create_repo(&loc()).expect("create dst");
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

    /// **Thin-push cheapness + correctness.** With `existing_tips = [c2]` the walk HIDES the existing
    /// history and visits ONLY the newly-introduced commit c3 — so a push whose delta base c2 is present
    /// is accepted WITHOUT re-verifying the whole chain, AND a non-existent existing tip is hidden
    /// gracefully (skipped, only widening the walk — fail-closed).
    #[test]
    fn history_connectivity_thin_push_hides_existing_tips() {
        let (src_root, src, chain) = seed_three_commit_history();
        let dst_root = temp_root("conn-dst-thin");
        let dst = DurableGitStore::rooted(&dst_root).create_repo(&loc()).expect("create dst");
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
        // A bogus / non-existent existing tip is hidden gracefully — the push is still correctly judged.
        let bogus = Oid::new("0".repeat(39) + "1");
        assert!(
            dst.history_connectivity_complete(&c3, &[c2, bogus]).unwrap(),
            "hiding a non-existent existing tip is graceful (skipped, never an error)"
        );
        std::fs::remove_dir_all(&src_root).ok();
        std::fs::remove_dir_all(&dst_root).ok();
    }

    /// A thin push whose DELTA BASE is present but whose deeper ancestor is missing is still ACCEPTED
    /// when that ancestor is already reachable from `existing_tips` (it is present, just hidden); but a
    /// push introducing a commit whose parent is genuinely absent is REJECTED even with a non-empty
    /// `existing_tips`. This pins the boundary: the check verifies parents of every NEW commit.
    #[test]
    fn history_connectivity_rejects_missing_parent_of_a_new_commit_even_with_existing_tips() {
        let (src_root, src, chain) = seed_three_commit_history();
        let (b2, t2, c2) = chain[1].clone();
        let (b3, t3, c3) = chain[2].clone();

        // Target odb has c3 + its tree/blob and c2's tree/blob, but NOT c2 (the parent of the new tip).
        let dst_root = temp_root("conn-dst-thin-missing");
        let dst = DurableGitStore::rooted(&dst_root).create_repo(&loc()).expect("create dst");
        copy_object(&src, &dst, &b2, "blob");
        copy_object(&src, &dst, &t2, "tree");
        copy_object(&src, &dst, &b3, "blob");
        copy_object(&src, &dst, &t3, "tree");
        copy_object(&src, &dst, &c3, "commit");
        assert!(!dst.has_object(&c2), "the new tip's parent commit is absent");

        // existing_tips names c2, but c2 is NOT in the odb → hidden gracefully (skipped), so the walk
        // still visits c3 and finds its parent c2 missing → REJECT (fail-closed).
        assert!(
            !dst.history_connectivity_complete(&c3, &[c2]).unwrap(),
            "a new commit whose parent is genuinely absent is rejected regardless of existing_tips"
        );
        // A missing NEW tip is itself a reject (never a swallowed error).
        assert!(
            !dst.history_connectivity_complete(&Oid::new("0".repeat(39) + "1"), &[])
                .unwrap(),
            "a new_tip that is not a present commit is rejected (fail-closed)"
        );
        std::fs::remove_dir_all(&src_root).ok();
        std::fs::remove_dir_all(&dst_root).ok();
    }

    // ── R3.4 repo-browsing completeness: nested tree/blob, kind mismatch, binary, refs, bounded walk ──

    /// Build a real commit with NESTED content on `refs/heads/main`:
    /// `README.md` (text), `crates/inner/deep.rs` (text), `assets/logo.bin` (binary — has NUL bytes).
    /// Returns the seeded [`DurableGitRepo`]. Uses git2 directly (the crate's dep) to construct subtrees
    /// bottom-up, since [`DurableGitRepo::write_tree`] only builds flat trees.
    fn seed_nested_repo(root: &std::path::Path) -> DurableGitRepo {
        let store = DurableGitStore::rooted(root);
        let repo = store.create_repo(&loc()).expect("create");
        let git = git2::Repository::open_bare(repo.path()).expect("open bare");

        let readme = git.blob(b"# nested repo\n\nhello\n").unwrap();
        let deep = git.blob(b"pub fn deep() {}\n").unwrap();
        let binary = git.blob(&[0x89, b'P', b'N', b'G', 0x00, 0x01, 0x02, 0x00, 0xff]).unwrap();

        // inner tree { deep.rs }
        let mut b = git.treebuilder(None).unwrap();
        b.insert("deep.rs", deep, 0o100644).unwrap();
        let inner = b.write().unwrap();
        // crates tree { inner/ }
        let mut b = git.treebuilder(None).unwrap();
        b.insert("inner", inner, 0o040000).unwrap();
        let crates = b.write().unwrap();
        // assets tree { logo.bin }
        let mut b = git.treebuilder(None).unwrap();
        b.insert("logo.bin", binary, 0o100644).unwrap();
        let assets = b.write().unwrap();
        // root tree { README.md, crates/, assets/ }
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
        // Also tag it, to prove the ref switcher separates branches from tags.
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

    /// **A ref that resolves to a NON-commit object (a bare tree oid, `main^{tree}`) is a clean
    /// empty browse, never a 500 (R3.4 verifier finding 1).** `revparse_single` succeeds but
    /// `peel_to_commit` fails with InvalidSpec; `resolve_commit` maps that to `None`, so the browse
    /// surfaces `Missing`/`None` rather than propagating a server error the edge would render as 500.
    #[test]
    fn ref_resolving_to_a_non_commit_object_is_a_clean_empty_browse_not_an_err() {
        let root = temp_root("noncommit-ref");
        let repo = seed_nested_repo(&root);
        // `main^{tree}` peels the tip to its TREE object — a non-commit revspec a client can supply.
        let tree_spec = "main^{tree}";
        // Blob resolution stays a clean Missing, not an internal error.
        assert!(matches!(
            repo.read_blob_at_path_bounded(tree_spec, "README.md", 1024).unwrap(),
            BlobPathLookup::Missing
        ));
        std::fs::remove_dir_all(&root).ok();
    }

    /// **Nested blob read + server-side binary detection + the blob→dir kind-mismatch hint (R3.4).**
    #[test]
    fn read_blob_at_path_detects_binary_and_flags_a_dir() {
        let root = temp_root("blob-nested");
        let repo = seed_nested_repo(&root);

        // A nested TEXT blob: not binary, real size, real oid.
        let BlobPathLookup::Found { bytes, oid, is_binary, size } =
            repo.read_blob_at_path_bounded("main", "crates/inner/deep.rs", 1024).unwrap()
        else {
            panic!("deep.rs is a blob");
        };
        assert!(!is_binary, "a text file is not binary");
        assert_eq!(size as usize, bytes.len());
        assert!(String::from_utf8_lossy(&bytes).contains("deep"));
        assert_eq!(
            repo.blob_oid_at_path("main", "crates/inner/deep.rs").unwrap(),
            Some(oid)
        );
        assert!(matches!(
            repo.read_blob_at_path_bounded("main", "crates/inner/deep.rs", 1).unwrap(),
            BlobPathLookup::TooLarge { size, maximum: 1, oid } if size > 1 && oid.as_str().len() == 40
        ));

        // A BINARY blob (NUL bytes): flagged binary so the UI never split('\n')s it.
        let BlobPathLookup::Found { is_binary, .. } =
            repo.read_blob_at_path_bounded("main", "assets/logo.bin", 1024).unwrap()
        else {
            panic!("logo.bin is a blob");
        };
        assert!(is_binary, "a file with NUL bytes is detected binary server-side");

        // A DIRECTORY requested under blob/ → IsDir (the client redirects to tree), not a 404.
        assert!(matches!(
            repo.read_blob_at_path_bounded("main", "crates/inner", 1024).unwrap(),
            BlobPathLookup::IsDir
        ));
        // The repo root under blob/ is a dir too.
        assert!(matches!(
            repo.read_blob_at_path_bounded("main", "", 1024).unwrap(),
            BlobPathLookup::IsDir
        ));
        // An absent file → Missing.
        assert!(matches!(
            repo.read_blob_at_path_bounded("main", "no/such/file", 1024).unwrap(),
            BlobPathLookup::Missing
        ));
        std::fs::remove_dir_all(&root).ok();
    }

    /// **Nested-path traversal safety (R3.4): a `../`-laden path never escapes the tree** — libgit2's
    /// `Tree::get_path` treats the path as tree-relative and refuses to walk above the root, so a
    /// traversal attempt resolves to Missing (never a file OUTSIDE the committed tree, never a panic).
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
            // The load-bearing property: blob resolution never yields bytes from outside the tree.
            if escape.contains("etc/passwd") || escape.contains("secret") {
                assert!(
                    matches!(repo.read_blob_at_path_bounded("main", escape, 1024).unwrap(), BlobPathLookup::Missing),
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
            .write_commit(
                &tree,
                &[],
                &message,
                "psn@acme.noreply",
                "psn@acme.noreply",
            )
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
