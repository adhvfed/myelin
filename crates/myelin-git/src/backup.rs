//! # `backup` — REAL git-repo backup + DESTRUCTIVE restore (GT-002 / E1.1)
//!
//! GT-001 made the on-disk bare repos REAL ([`crate::durable`]); GT-002 makes their BACKUP real.
//! Today backup/restore are *modeled* (a WAL offset, census SI-014/015 — that lives in
//! [`myelin_storage::backup`]/[`myelin_storage::restore`] and is the DB's deferred PITR floor
//! P-S12/P-S15). This module backs up the **repo BYTES**: it captures a bare repo's complete,
//! self-consistent object graph + refs into a single artifact from which — *with no access to the
//! original* — a full, `git fsck`-clean repo is reconstructed onto a CLEAN target.
//!
//! ## The real mechanism — a self-contained packfile + a ref snapshot (NOT a modeled offset)
//! The backup is the canonical git mechanism libgit2 (`git2`) exposes, and is exactly what
//! `git bundle` carries internally: **a ref-tip snapshot + a non-thin packfile of every object
//! reachable from those tips.**
//! - **Refs** — [`DurableGitRepo::list_refs`] snapshots `(name, oid)` for every ref FIRST. This is
//!   the consistency point: git objects are immutable / append-only, so once we have the tip oids,
//!   the closure reachable from them cannot change underneath us. A concurrent push that moves a
//!   ref after the snapshot is simply not in this backup (we pack the snapshotted tip's closure) —
//!   so **every backed-up ref points only at backed-up objects**, by construction.
//! - **Objects** — a `git2::PackBuilder` packs the full closure: a revwalk over all tips
//!   (`insert_walk`) captures every commit's ancestry + trees + blobs, and `insert_recursive` per
//!   tip additionally captures non-commit tips (annotated **tag objects**, which a revwalk peels
//!   away). libgit2 writes a complete, self-contained (non-thin) packfile with its own SHA trailer
//!   — re-hash-verifiable on ingest.
//!
//! We do NOT reimplement git packing/objects/refs: the pack is built by libgit2, ingested by
//! libgit2's indexer ([`git2::Odb::packwriter`]), and the refs are recreated through the GT-001
//! durable CAS path. The artifact's only bespoke part is a thin length-prefixed FRAME around
//! `(refs, pack)` so it is one file.
//!
//! ## Reconciling with the storage backup framework (compose, do not fork)
//! [`myelin_storage::backup`] orchestrates the OLTP PITR (continuous WAL archiving + base backups,
//! the `ContinuousArchiver`) and classifies every store into a [`StoreTier`]. The git repo backup
//! plugs in as a **real artifact at the T2 object tier** ([`GitRepoBackup::store_tier`] ==
//! [`StoreTier::Object`]): the git odb is content-addressed (every object keyed by its own hash,
//! immutable, append-only) — *exactly* the T2 "versioned + content-addressed → integrity
//! re-hash-verifiable" posture the framework already models. We reuse that tier vocabulary rather
//! than minting a second one. What stays DISTINCT (and still modeled / deferred) is the DB's
//! OLTP-WAL PITR offset tier — that is the SI-014/015 DB slice (`restore_to_offset`), a different
//! artifact tier from the repo bytes.
//!
//! ## Honest scope (EI-01 §1 — write the floor down)
//! - **Full-snapshot, not incremental.** Each [`GitRepoBackup::create`] packs the entire reachable
//!   closure. Incremental / continuous git backup (a since-marker thin pack, or WAL-style ref-log
//!   shipping) is NOT done here — full-only. The artifact is genuinely reconstructable alone, which
//!   is the GT-002 bar; incremental is a later optimisation.
//! - **Git-tier-real.** The repo bytes round-trip through a real libgit2 pack + a real
//!   destructive restore onto a clean target, proven by the real `git fsck --full --strict`
//!   external oracle (see `tests/git_backup_restore.rs`). The DB-PITR floor (live `pg_basebackup`
//!   / WAL replay) remains deferred — that is not this module.

use std::path::Path;

use crate::core::{Oid, RepoLoc};
use crate::durable::{DurableError, DurableGitRepo, DurableGitStore};
use crate::gix_backend::RepoPathResolver;

/// The store-tier vocabulary the git backup reconciles with — reused from the storage framework
/// (NOT re-minted): the git odb is the content-addressed T2 object tier.
pub use myelin_storage::backup::StoreTier;

/// Artifact magic + versions. v2 adds a BLAKE3 checksum over the entire preceding frame; the reader
/// retains v1 compatibility so existing off-host backups remain restorable.
const MAGIC_V1: &[u8] = b"MYELIN-GIT-BACKUP-v1\0";
const MAGIC_V2: &[u8] = b"MYELIN-GIT-BACKUP-v2\0";
const MAGIC: &[u8] = MAGIC_V2;
const CHECKSUM_LEN: usize = 32;
const MAX_BACKUP_REFS: usize = 1_000_000;

// ───────────────────────────── errors ────────────────────────────────────────────────────────────

/// The error surface of the git-repo backup/restore. Loud + specific (a refusal is diagnosable —
/// EI-01 §3); never a silent wrong-bytes / partial restore.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitBackupError {
    /// An underlying GT-001 durable-store op failed (open / refs / repo lifecycle).
    Durable(DurableError),
    /// A `git2` (libgit2) op failed (packbuilder / odb / pack ingest). Carries the libgit2 message.
    Git(String),
    /// A filesystem op failed (reading/writing the off-host artifact file).
    Io(String),
    /// The backup artifact is malformed (bad magic, truncated frame, non-utf8 ref). A corrupt
    /// artifact is REFUSED — never a silent partial reconstruction.
    BadArtifact(String),
    /// A restore was asked to land on a target that is NOT clean (a repo already exists there). A
    /// destructive restore is genuine recovery onto an empty target — it never clobbers a live repo.
    TargetNotClean(String),
}

impl std::fmt::Display for GitBackupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitBackupError::Durable(e) => write!(f, "git backup durable op failed: {e}"),
            GitBackupError::Git(m) => write!(f, "git backup libgit2 op failed: {m}"),
            GitBackupError::Io(m) => write!(f, "git backup io failed: {m}"),
            GitBackupError::BadArtifact(m) => write!(f, "git backup artifact malformed: {m}"),
            GitBackupError::TargetNotClean(m) => write!(
                f,
                "git restore target is not clean: {m} — a destructive restore lands on an EMPTY \
                 target (genuine recovery), it never clobbers a live repo"
            ),
        }
    }
}

impl std::error::Error for GitBackupError {}

impl From<DurableError> for GitBackupError {
    fn from(e: DurableError) -> Self {
        GitBackupError::Durable(e)
    }
}

fn git_err(ctx: &str, e: git2::Error) -> GitBackupError {
    GitBackupError::Git(format!("{ctx}: {e}"))
}

// ───────────────────────────── the backup artifact ────────────────────────────────────────────────

/// **A real, self-contained backup of a single bare repo** — a ref-tip snapshot + a non-thin
/// packfile of every object reachable from those tips. From this artifact ALONE (no access to the
/// original) a full repo is reconstructed ([`restore_repo`]). It is tenant/region-agnostic in its
/// bytes (it carries no locator) — the tenant/region scope is the [`RepoLoc`] it is restored UNDER,
/// through the validated resolver, so a backup can never be restored across the tenant boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitRepoBackup {
    /// The ref snapshot `(fully-qualified name, tip oid)` — the consistency point. Every tip's
    /// closure is present in `pack`.
    refs: Vec<(String, Oid)>,
    /// The complete, self-contained (non-thin) packfile of every object reachable from `refs`.
    pack: Vec<u8>,
}

impl GitRepoBackup {
    /// The store-tier this backup plugs into in the storage framework: the **T2 object tier**. The
    /// git odb is content-addressed (immutable, hash-keyed objects) — the same re-hash-verifiable
    /// posture [`StoreTier::Object`] models. Reconciles the git tier with the framework's
    /// classification instead of forking a second backup taxonomy.
    pub fn store_tier() -> StoreTier {
        StoreTier::Object
    }

    /// **Make a REAL backup of `repo`** — snapshot the refs (the consistency point), then pack the
    /// complete object closure reachable from those tips via libgit2 (we do NOT reimplement
    /// packing). The returned artifact is reconstructable on its own.
    pub fn create(repo: &DurableGitRepo) -> Result<GitRepoBackup, GitBackupError> {
        // (1) Ref-snapshot point FIRST. Objects are immutable/append-only, so the closure reachable
        // from these exact tips is frozen — the backed-up refs will only point at backed-up objects.
        let refs = repo.list_refs()?;
        if refs.len() > MAX_BACKUP_REFS {
            return Err(GitBackupError::BadArtifact(format!(
                "ref snapshot exceeds the {MAX_BACKUP_REFS} ref artifact limit"
            )));
        }

        // (2) Pack the full closure with libgit2. Open the same on-disk bare repo GT-001 manages.
        let git = git2::Repository::open(repo.path())
            .map_err(|e| git_err(&format!("open {}", repo.path().display()), e))?;
        let mut pb = git.packbuilder().map_err(|e| git_err("packbuilder", e))?;
        let mut walk = git.revwalk().map_err(|e| git_err("revwalk", e))?;
        for (name, oid) in &refs {
            let goid = git2::Oid::from_str(oid.as_str())
                .map_err(|e| GitBackupError::Git(format!("bad oid for {name}: {e}")))?;
            // The tip's own object closure — captures an annotated TAG object (a revwalk peels tags
            // to their commit, so the tag object itself would otherwise be missed), and is a no-op
            // dedup for a commit tip. libgit2's insert_commit does NOT walk parents, hence the walk
            // below for full ancestry.
            pb.insert_recursive(goid, None)
                .map_err(|e| git_err(&format!("insert_recursive {name}"), e))?;
            // Full ancestor history. A tip that does not peel to a commit (a ref straight at a
            // blob/tree) is already covered by insert_recursive above — ignore the push error.
            let _ = walk.push(goid);
        }
        // insert_walk packs every commit in the walk + its tree + blobs (full history closure).
        pb.insert_walk(&mut walk).map_err(|e| git_err("insert_walk", e))?;

        let mut buf = git2::Buf::new();
        pb.write_buf(&mut buf).map_err(|e| git_err("write pack buf", e))?;
        let pack = buf.to_vec();

        Ok(GitRepoBackup { refs, pack })
    }

    /// The ref snapshot captured in this backup.
    pub fn refs(&self) -> &[(String, Oid)] {
        &self.refs
    }

    /// The number of refs in the snapshot.
    pub fn ref_count(&self) -> usize {
        self.refs.len()
    }

    /// The packfile size in bytes (the backup-size signal; the real repo bytes, not a modeled len).
    pub fn pack_len(&self) -> usize {
        self.pack.len()
    }

    /// **Serialize to a single self-describing artifact** (the off-host backup blob). Length-prefixed
    /// binary frame: `MAGIC · u32 ref_count · {u32 name_len · name · u32 oid_len · oid}* · u64
    /// pack_len · pack · blake3(frame)`. Big-endian. From these bytes alone the repo is
    /// reconstructable, and corruption in either the ref snapshot or pack is detected before parse.
    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(MAGIC.len() + 16 + self.pack.len() + CHECKSUM_LEN);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&(self.refs.len() as u32).to_be_bytes());
        for (name, oid) in &self.refs {
            let n = name.as_bytes();
            let o = oid.as_str().as_bytes();
            out.extend_from_slice(&(n.len() as u32).to_be_bytes());
            out.extend_from_slice(n);
            out.extend_from_slice(&(o.len() as u32).to_be_bytes());
            out.extend_from_slice(o);
        }
        out.extend_from_slice(&(self.pack.len() as u64).to_be_bytes());
        out.extend_from_slice(&self.pack);
        let checksum = blake3::hash(&out);
        out.extend_from_slice(checksum.as_bytes());
        out
    }

    /// **Reconstruct a backup from its serialized bytes** — the artifact-alone path. Any shortfall
    /// (bad magic, a truncated length, non-utf8 ref) is a LOUD [`GitBackupError::BadArtifact`],
    /// never a silent partial parse.
    pub fn deserialize(bytes: &[u8]) -> Result<GitRepoBackup, GitBackupError> {
        if bytes.len() < MAGIC.len() {
            return Err(GitBackupError::BadArtifact("artifact is shorter than its magic".into()));
        }
        let magic = &bytes[..MAGIC.len()];
        let frame = if magic == MAGIC_V2 {
            let frame_len = bytes.len().checked_sub(CHECKSUM_LEN).ok_or_else(|| {
                GitBackupError::BadArtifact("v2 artifact is missing its checksum".into())
            })?;
            if frame_len < MAGIC.len() {
                return Err(GitBackupError::BadArtifact(
                    "v2 artifact is missing its checksum".into(),
                ));
            }
            let (frame, stored_checksum) = bytes.split_at(frame_len);
            let actual = blake3::hash(frame);
            if actual.as_bytes().as_slice() != stored_checksum {
                return Err(GitBackupError::BadArtifact(
                    "v2 artifact checksum mismatch".into(),
                ));
            }
            frame
        } else if magic == MAGIC_V1 {
            bytes
        } else {
            return Err(GitBackupError::BadArtifact(format!(
                "unknown backup artifact magic {:?}",
                &magic[..magic.len().min(MAGIC.len())]
            )));
        };

        let mut cur = Cursor::new(frame);
        let magic = cur.take(MAGIC.len())?;
        debug_assert!(magic == MAGIC_V1 || magic == MAGIC_V2);
        let ref_count = cur.take_u32()? as usize;
        let minimum_ref_bytes = ref_count.checked_mul(8).ok_or_else(|| {
            GitBackupError::BadArtifact("ref-count size overflow in artifact frame".into())
        })?;
        if ref_count > MAX_BACKUP_REFS
            || minimum_ref_bytes.saturating_add(8) > cur.remaining()
        {
            return Err(GitBackupError::BadArtifact(format!(
                "impossible or excessive ref count {ref_count}"
            )));
        }
        let mut refs = Vec::new();
        refs.try_reserve(ref_count.min(1024)).map_err(|_| {
            GitBackupError::BadArtifact("cannot allocate backup ref parser state".into())
        })?;
        for _ in 0..ref_count {
            let nlen = cur.take_u32()? as usize;
            let name = std::str::from_utf8(cur.take(nlen)?)
                .map_err(|e| GitBackupError::BadArtifact(format!("ref name not utf8: {e}")))?
                .to_string();
            if !git2::Reference::is_valid_name(&name) {
                return Err(GitBackupError::BadArtifact(format!(
                    "invalid fully-qualified ref name `{name}`"
                )));
            }
            let olen = cur.take_u32()? as usize;
            let oid = std::str::from_utf8(cur.take(olen)?)
                .map_err(|e| GitBackupError::BadArtifact(format!("ref oid not utf8: {e}")))?
                .to_string();
            git2::Oid::from_str(&oid).map_err(|e| {
                GitBackupError::BadArtifact(format!("invalid oid for ref `{name}`: {e}"))
            })?;
            refs.push((name, Oid::new(oid)));
        }
        let pack_len = usize::try_from(cur.take_u64()?).map_err(|_| {
            GitBackupError::BadArtifact("pack length exceeds this host's address space".into())
        })?;
        let pack = cur.take(pack_len)?.to_vec();
        if cur.remaining() != 0 {
            return Err(GitBackupError::BadArtifact(format!(
                "{} trailing bytes after backup frame",
                cur.remaining()
            )));
        }
        Ok(GitRepoBackup { refs, pack })
    }

    /// Write the artifact to an off-host file (the real backup blob on disk).
    pub fn write_to_file(&self, path: &Path) -> Result<(), GitBackupError> {
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        crate::durable::write_file_atomic(parent, path, &self.serialize())
            .map_err(GitBackupError::Durable)
    }

    /// Read + reconstruct the artifact from an off-host file.
    pub fn read_from_file(path: &Path) -> Result<GitRepoBackup, GitBackupError> {
        let bytes = std::fs::read(path)
            .map_err(|e| GitBackupError::Io(format!("read artifact {}: {e}", path.display())))?;
        GitRepoBackup::deserialize(&bytes)
    }
}

// ───────────────────────────── a tiny bounds-checked cursor (artifact parse) ───────────────────────

/// A minimal big-endian reader over the artifact bytes — every read is bounds-checked so a
/// truncated artifact is a loud [`GitBackupError::BadArtifact`], never an out-of-bounds panic.
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Cursor { bytes, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], GitBackupError> {
        let end = self.pos.checked_add(n).ok_or_else(|| {
            GitBackupError::BadArtifact("length overflow in artifact frame".into())
        })?;
        if end > self.bytes.len() {
            return Err(GitBackupError::BadArtifact(format!(
                "artifact truncated: needed {n} bytes at offset {}, only {} remain",
                self.pos,
                self.bytes.len().saturating_sub(self.pos)
            )));
        }
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }

    fn take_u32(&mut self) -> Result<u32, GitBackupError> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn take_u64(&mut self) -> Result<u64, GitBackupError> {
        let b = self.take(8)?;
        Ok(u64::from_be_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }
}

// ───────────────────────────── the destructive restore ────────────────────────────────────────────

/// **DESTRUCTIVE restore of a backup onto a CLEAN target** (the GT-002 headline). Reconstructs the
/// repo at `loc` (under `store`'s validated resolver — tenant/region-scoped) from `backup` ALONE:
///
/// 1. **Clean-target guard** — if a repo already exists at `loc`, REFUSE
///    ([`GitBackupError::TargetNotClean`]). A genuine recovery lands on an empty target; it never
///    clobbers a live repo.
/// 2. **Stage off to the side** — build the whole repo (init_bare + pack ingest + ref recreate) at a
///    SIBLING temp path under the same `<tenant>/<region>/` dir (same filesystem), then
///    [`std::fs::rename`] it onto the final `<repo>.git` path ONLY after every step succeeds.
/// 3. **Rollback on any error** — if the pack ingest or a ref write fails (e.g. a corrupt artifact:
///    libgit2's indexer hard-verifies the pack, a flipped ref oid points at a missing object), the
///    staging dir is removed and the FINAL path is left CLEAN. A failed restore never poisons its own
///    target — an immediate retry with a good artifact succeeds with no manual cleanup.
///
/// **Atomicity (approach (a) — temp + atomic rename):** the final path appears in one `rename` only
/// when the repo is complete, so a mid-restore failure (Err *or* a crash) leaves the final location
/// empty (at worst an orphan staging dir beside it, which never blocks the real locator).
///
/// The pack is ingested through libgit2's indexer ([`git2::Odb::packwriter`]) — objects are
/// re-validated as they land (a corrupt pack fails, not a silent bad-bytes write) — and the refs are
/// recreated through the GT-001 durable CAS create path (reflog-logged, on disk). The restored repo
/// is a valid bare repo whose refs point at the same object graph as the source — proven IDENTICAL on
/// read-back + `git fsck --full --strict` clean by the integration test.
pub fn restore_repo<P: RepoPathResolver>(
    store: &DurableGitStore<P>,
    loc: &RepoLoc,
    backup: &GitRepoBackup,
) -> Result<DurableGitRepo, GitBackupError> {
    // (1) Clean-target guard — never clobber a live repo (genuine recovery only).
    if store.repo_exists(loc) {
        let path = store
            .repo_path(loc)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "<unresolved>".into());
        return Err(GitBackupError::TargetNotClean(format!(
            "a repo already exists at {path}"
        )));
    }

    // Resolve (and thereby VALIDATE) the final path FIRST — a traversing locator is refused here,
    // before any staging dir is created (the resolver guard is never bypassed).
    let final_path = store.repo_path(loc)?;

    // (2) A sibling staging locator: same tenant/region (so it lands in the SAME parent dir → same
    // filesystem, making the publish rename atomic), with a unique temp repo slug. The `.` chars are
    // valid slug chars (the resolver accepts `[A-Za-z0-9._-]`); the `.restoring.<n>.tmp` suffix never
    // collides with the real locator, so it can never be reached as a live repo.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let staging_loc = RepoLoc::new(
        loc.tenant.clone(),
        loc.region.clone(),
        format!("{}.restoring.{nanos}.tmp", loc.repo),
    );
    let staging_path = store.repo_path(&staging_loc)?;

    // Build the full repo at the staging path. On ANY failure, remove the staging dir and return —
    // the final path is never touched (it stays CLEAN for an immediate retry).
    if let Err(e) = build_repo_at(store, &staging_loc, backup) {
        let _ = std::fs::remove_dir_all(&staging_path);
        return Err(e);
    }

    // (3) Atomic publish: move the complete staging repo onto the final path in one rename.
    if let Err(e) = std::fs::rename(&staging_path, &final_path) {
        let _ = std::fs::remove_dir_all(&staging_path);
        return Err(GitBackupError::Io(format!(
            "atomic publish rename {} -> {} failed: {e}",
            staging_path.display(),
            final_path.display()
        )));
    }

    // Open the published repo through the validated store (the returned handle is on the final path).
    store.open_repo(loc).map_err(GitBackupError::Durable)
}

/// Build a complete bare repo (init_bare + pack ingest + ref recreate) at `loc`. Used to materialise
/// the restore at a STAGING locator; the caller atomically publishes it (or rolls it back on error).
fn build_repo_at<P: RepoPathResolver>(
    store: &DurableGitStore<P>,
    loc: &RepoLoc,
    backup: &GitRepoBackup,
) -> Result<(), GitBackupError> {
    // init_bare a FRESH repo through the validated resolver (tenant/region-scoped).
    let repo = store.create_repo(loc)?;

    // Ingest the self-contained pack into the empty odb (libgit2 indexes + verifies it).
    if !backup.pack.is_empty() {
        let git = git2::Repository::open(repo.path())
            .map_err(|e| git_err(&format!("open restore target {}", repo.path().display()), e))?;
        let odb = git.odb().map_err(|e| git_err("restore odb", e))?;
        let mut pw = odb.packwriter().map_err(|e| git_err("restore packwriter", e))?;
        {
            use std::io::Write;
            pw.write_all(&backup.pack)
                .map_err(|e| GitBackupError::Io(format!("feed pack to indexer: {e}")))?;
        }
        pw.commit().map_err(|e| git_err("commit ingested pack", e))?;
    }

    // Recreate each ref through the durable CAS create path (on-disk, reflog-logged). The restored
    // ref points at an object the ingested pack brought back (a flipped oid → missing object → Err).
    for (name, oid) in &backup.refs {
        repo.update_ref_cas(name, None, Some(oid), "restore: recreate ref", "restore@myelin.noreply")
            .map_err(GitBackupError::Durable)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_root(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        p.push(format!("myelin-gt002-unit-{tag}-{nanos}"));
        p
    }

    fn loc() -> RepoLoc {
        RepoLoc::new("acme", "fr-par", "core")
    }

    /// Seed a small real history (two commits on main) + a branch, return (refname→oid) we expect.
    fn seed(repo: &DurableGitRepo) -> Vec<(String, Oid)> {
        let psn = "psn-7@acme.noreply";
        let b1 = repo.write_blob(b"v1\n").unwrap();
        let t1 = repo.write_tree(&[("file.txt", &b1)]).unwrap();
        let c1 = repo.write_commit(&t1, &[], "c1", psn, psn).unwrap();
        let b2 = repo.write_blob(b"v2\n").unwrap();
        let t2 = repo.write_tree(&[("file.txt", &b2)]).unwrap();
        let c2 = repo.write_commit(&t2, &[&c1], "c2", psn, psn).unwrap();
        repo.update_ref_cas("refs/heads/main", None, Some(&c2), "create main", psn)
            .unwrap();
        repo.update_ref_cas("refs/heads/feature", None, Some(&c1), "create feature", psn)
            .unwrap();
        let mut want = vec![
            ("refs/heads/feature".to_string(), c1),
            ("refs/heads/main".to_string(), c2),
        ];
        want.sort();
        want
    }

    /// The store tier reconciles with the framework as the content-addressed T2 object tier (backed
    /// up, not derived) — we reuse the vocabulary, not a forked taxonomy.
    #[test]
    fn git_backup_plugs_into_the_t2_object_tier() {
        assert_eq!(GitRepoBackup::store_tier(), StoreTier::Object);
        assert!(GitRepoBackup::store_tier().is_backed_up());
        assert!(!GitRepoBackup::store_tier().is_rebuilt_from_source());
    }

    /// The artifact round-trips through serialize/deserialize byte-for-byte (reconstructable alone).
    #[test]
    fn artifact_serialize_roundtrips() {
        let root = temp_root("ser");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).unwrap();
        let want = seed(&repo);

        let backup = GitRepoBackup::create(&repo).unwrap();
        assert_eq!(backup.refs(), want.as_slice());
        assert!(backup.pack_len() > 0, "a real packfile, not an empty modeled blob");

        let bytes = backup.serialize();
        let back = GitRepoBackup::deserialize(&bytes).unwrap();
        assert_eq!(back, backup, "artifact reconstructs identically from its bytes alone");

        std::fs::remove_dir_all(&root).ok();
    }

    /// A corrupt artifact is REFUSED loudly (bad magic / truncation), never a silent partial parse.
    #[test]
    fn corrupt_artifact_is_refused() {
        assert!(matches!(
            GitRepoBackup::deserialize(b"not a myelin backup"),
            Err(GitBackupError::BadArtifact(_))
        ));
        // A valid prefix then truncated mid-frame.
        let mut t = MAGIC.to_vec();
        t.extend_from_slice(&5u32.to_be_bytes()); // claims 5 refs, then nothing
        assert!(matches!(
            GitRepoBackup::deserialize(&t),
            Err(GitBackupError::BadArtifact(_))
        ));

        let mut count_bomb = MAGIC.to_vec();
        count_bomb.extend_from_slice(&u32::MAX.to_be_bytes());
        assert!(matches!(
            GitRepoBackup::deserialize(&count_bomb),
            Err(GitBackupError::BadArtifact(_))
        ));

        let empty = GitRepoBackup {
            refs: Vec::new(),
            pack: Vec::new(),
        };
        let mut trailing = empty.serialize();
        trailing.extend_from_slice(b"garbage");
        assert!(matches!(
            GitRepoBackup::deserialize(&trailing),
            Err(GitBackupError::BadArtifact(_))
        ));

        let mut valid = empty.serialize();
        let ref_count_offset = MAGIC.len();
        valid[ref_count_offset + 3] ^= 1;
        assert!(matches!(
            GitRepoBackup::deserialize(&valid),
            Err(GitBackupError::BadArtifact(message)) if message.contains("checksum")
        ));

        let mut legacy_v1 = MAGIC_V1.to_vec();
        legacy_v1.extend_from_slice(&0u32.to_be_bytes());
        legacy_v1.extend_from_slice(&0u64.to_be_bytes());
        assert_eq!(
            GitRepoBackup::deserialize(&legacy_v1).unwrap(),
            empty,
            "existing v1 artifacts remain readable"
        );
    }

    /// **DESTRUCTIVE round-trip into a CLEAN target (the unit slice).** Back up → restore into a
    /// brand-new empty root (the original never touched) → refs + a sample object identical +
    /// in-process fsck clean. (The full external `git fsck` oracle runs in `tests/`.)
    #[test]
    fn destructive_restore_into_a_clean_root_reads_back_identical() {
        let src_root = temp_root("src");
        let src_store = DurableGitStore::rooted(&src_root);
        let src_repo = src_store.create_repo(&loc()).unwrap();
        let want = seed(&src_repo);
        let backup = GitRepoBackup::create(&src_repo).unwrap();
        // Persist + reload the artifact from disk so the restore truly reads bytes alone.
        let artifact = temp_root("artifact");
        std::fs::create_dir_all(&artifact).unwrap();
        let artifact_file = artifact.join("core.gitbackup");
        backup.write_to_file(&artifact_file).unwrap();

        // A genuinely CLEAN target root — the original src_root is NOT visible to it.
        let dst_root = temp_root("dst");
        let dst_store = DurableGitStore::rooted(&dst_root);
        assert!(!dst_store.repo_exists(&loc()), "target starts clean/empty");

        let reloaded = GitRepoBackup::read_from_file(&artifact_file).unwrap();
        let restored = restore_repo(&dst_store, &loc(), &reloaded).unwrap();

        // Every ref reads back identical.
        assert_eq!(restored.list_refs().unwrap(), want);
        // Every source object exists + reads back byte-identical in the restored odb.
        for (_, tip) in &want {
            let src_bytes = src_repo.read_object(tip).unwrap();
            let dst_bytes = restored.read_object(tip).unwrap();
            assert_eq!(src_bytes, dst_bytes, "object {} bytes identical", tip.as_str());
        }
        restored.fsck().expect("in-process fsck clean on the restored repo");

        // Restoring AGAIN over the now-present repo is refused (clean-target guard).
        assert!(matches!(
            restore_repo(&dst_store, &loc(), &reloaded),
            Err(GitBackupError::TargetNotClean(_))
        ));

        std::fs::remove_dir_all(&src_root).ok();
        std::fs::remove_dir_all(&dst_root).ok();
        std::fs::remove_dir_all(&artifact).ok();
    }
}
