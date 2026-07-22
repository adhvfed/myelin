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
//! - **Bounded Rust pack buffering, not a hard process-RSS claim.** The production
//!   [`GitRepoBackup::create_to_file`] / [`restore_repo_from_file`] path keeps bulk Rust-owned memory
//!   to bounded ref metadata plus one fixed-size copy buffer. libgit2 still retains per-object maps,
//!   delta-selection windows/cache while building, and index state while ingesting. Those costs are
//!   object-count/content dependent and require an outer worker/cgroup limit if a hard RSS ceiling is
//!   needed; streaming the pack bytes cannot remove them through the current `git2` API.
//! - **Git-tier-real.** The repo bytes round-trip through a real libgit2 pack + a real
//!   destructive restore onto a clean target, proven by the real `git fsck --full --strict`
//!   external oracle (see `tests/git_backup_restore.rs`). The DB-PITR floor (live `pg_basebackup`
//!   / WAL replay) remains deferred — that is not this module.

use std::io::{Read as _, Seek as _, Write as _};
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
/// The only whole-body storage used by the disk-backed path. Ref metadata remains bounded
/// separately; pack bytes are copied between libgit2 and disk in chunks of this size.
const STREAM_BUFFER_BYTES: usize = 64 * 1024;
const MAX_BACKUP_REFS: usize = crate::durable::WIRE_MAX_REFS;
/// The present backup object owns one complete pack in memory. Refuse beyond this operational
/// ceiling during construction, before a second repo-sized allocation can occur.
const MAX_BACKUP_PACK_BYTES: usize = 512 * 1024 * 1024;
/// Aggregate framing/ref-name/oid/checksum bytes outside the pack.
const MAX_BACKUP_REF_FRAME_BYTES: usize = 64 * 1024 * 1024;
/// Pack plus bounded ref-frame/checksum overhead accepted from off-host storage.
const MAX_BACKUP_ARTIFACT_BYTES: usize = MAX_BACKUP_PACK_BYTES + MAX_BACKUP_REF_FRAME_BYTES;

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

fn ref_frame_bytes(refs: &[(String, Oid)], checksum_bytes: usize) -> Result<usize, GitBackupError> {
    refs.iter().try_fold(
        MAGIC.len() + 4 + 8 + checksum_bytes,
        |total, (name, oid)| {
            u32::try_from(name.len()).map_err(|_| {
                GitBackupError::BadArtifact("backup ref name exceeds the u32 frame limit".into())
            })?;
            u32::try_from(oid.as_str().len()).map_err(|_| {
                GitBackupError::BadArtifact("backup ref oid exceeds the u32 frame limit".into())
            })?;
            total
                .checked_add(4)
                .and_then(|total| total.checked_add(name.len()))
                .and_then(|total| total.checked_add(4))
                .and_then(|total| total.checked_add(oid.as_str().len()))
                .ok_or_else(|| {
                    GitBackupError::BadArtifact("backup ref frame length overflow".into())
                })
        },
    )
}

fn snapshot_refs(repo: &DurableGitRepo) -> Result<Vec<(String, Oid)>, GitBackupError> {
    let refs = repo.list_refs_bounded(MAX_BACKUP_REFS)?;
    let bytes = ref_frame_bytes(&refs, CHECKSUM_LEN)?;
    if bytes > MAX_BACKUP_REF_FRAME_BYTES {
        return Err(GitBackupError::BadArtifact(format!(
            "backup ref frame exceeds the {MAX_BACKUP_REF_FRAME_BYTES}-byte artifact limit"
        )));
    }
    Ok(refs)
}

fn with_packbuilder<T>(
    repo: &DurableGitRepo,
    refs: &[(String, Oid)],
    consume: impl FnOnce(&mut git2::PackBuilder<'_>) -> Result<T, GitBackupError>,
) -> Result<T, GitBackupError> {
    let git = git2::Repository::open(repo.path())
        .map_err(|e| git_err(&format!("open {}", repo.path().display()), e))?;
    let mut pb = git.packbuilder().map_err(|e| git_err("packbuilder", e))?;
    let mut walk = git.revwalk().map_err(|e| git_err("revwalk", e))?;
    for (name, oid) in refs {
        let goid = git2::Oid::from_str(oid.as_str())
            .map_err(|e| GitBackupError::Git(format!("bad oid for {name}: {e}")))?;
        // Preserve the tag/non-commit tip itself, then add full commit ancestry below.
        pb.insert_recursive(goid, None)
            .map_err(|e| git_err(&format!("insert_recursive {name}"), e))?;
        let _ = walk.push(goid);
    }
    pb.insert_walk(&mut walk)
        .map_err(|e| git_err("insert_walk", e))?;
    consume(&mut pb)
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
        Self::create_bounded(repo, MAX_BACKUP_PACK_BYTES)
    }

    fn create_bounded(
        repo: &DurableGitRepo,
        maximum_pack_bytes: usize,
    ) -> Result<GitRepoBackup, GitBackupError> {
        // (1) Ref-snapshot point FIRST. Objects are immutable/append-only, so the closure reachable
        // from these exact tips is frozen — the backed-up refs will only point at backed-up objects.
        let refs = snapshot_refs(repo)?;

        // (2) Pack the full closure with libgit2. The compatibility object still owns the complete
        // pack; production file callers use `create_to_file` below to keep these bytes on disk.
        let pack = with_packbuilder(repo, &refs, |pb| {
            let mut pack = Vec::new();
            let mut exceeded = false;
            let result = pb.foreach(|chunk| {
                if chunk.len() > maximum_pack_bytes.saturating_sub(pack.len()) {
                    exceeded = true;
                    return false;
                }
                pack.extend_from_slice(chunk);
                true
            });
            if exceeded {
                return Err(GitBackupError::BadArtifact(format!(
                    "backup pack exceeds the {maximum_pack_bytes}-byte construction limit"
                )));
            }
            result.map_err(|e| git_err("stream pack bytes", e))?;
            Ok(pack)
        })?;

        Ok(GitRepoBackup { refs, pack })
    }

    /// Create the current v2 artifact directly on disk without owning the pack or complete frame in
    /// Rust memory. The process-unique temp file is fsynced and atomically renamed only after the
    /// pack length is patched and the checksum over the corrected frame has been appended.
    pub fn create_to_file(
        repo: &DurableGitRepo,
        path: &Path,
    ) -> Result<VerifiedGitRepoBackupFile, GitBackupError> {
        Self::create_to_file_bounded(repo, path, MAX_BACKUP_PACK_BYTES)
    }

    fn create_to_file_bounded(
        repo: &DurableGitRepo,
        path: &Path,
        maximum_pack_bytes: usize,
    ) -> Result<VerifiedGitRepoBackupFile, GitBackupError> {
        // The refs are the consistency point and MUST precede any pack traversal.
        let refs = snapshot_refs(repo)?;
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let mut construction_error = None;
        let atomic_result = crate::durable::write_file_atomic_with(parent, path, |handle| {
            let result = Self::write_created_frame(
                repo,
                &refs,
                handle,
                path,
                maximum_pack_bytes.min(MAX_BACKUP_PACK_BYTES),
            );
            match result {
                Ok(()) => Ok(()),
                Err(error) => {
                    construction_error = Some(error);
                    Err(DurableError::Io(format!(
                        "construct backup artifact {} failed",
                        path.display()
                    )))
                }
            }
        });
        if let Some(error) = construction_error {
            return Err(error);
        }
        atomic_result.map_err(GitBackupError::Durable)?;
        VerifiedGitRepoBackupFile::open(path)
    }

    fn write_created_frame(
        repo: &DurableGitRepo,
        refs: &[(String, Oid)],
        handle: &mut std::fs::File,
        path: &Path,
        maximum_pack_bytes: usize,
    ) -> Result<(), GitBackupError> {
        let io = |operation: &str, error: std::io::Error| {
            GitBackupError::Io(format!(
                "{operation} backup artifact {}: {error}",
                path.display()
            ))
        };
        handle.write_all(MAGIC).map_err(|e| io("write", e))?;
        let ref_count = u32::try_from(refs.len())
            .map_err(|_| GitBackupError::BadArtifact("backup ref count exceeds u32".into()))?;
        handle
            .write_all(&ref_count.to_be_bytes())
            .map_err(|e| io("write", e))?;
        for (name, oid) in refs {
            let name = name.as_bytes();
            let oid = oid.as_str().as_bytes();
            let name_len = u32::try_from(name.len()).map_err(|_| {
                GitBackupError::BadArtifact("backup ref name exceeds the u32 frame limit".into())
            })?;
            let oid_len = u32::try_from(oid.len()).map_err(|_| {
                GitBackupError::BadArtifact("backup ref oid exceeds the u32 frame limit".into())
            })?;
            handle
                .write_all(&name_len.to_be_bytes())
                .and_then(|()| handle.write_all(name))
                .and_then(|()| handle.write_all(&oid_len.to_be_bytes()))
                .and_then(|()| handle.write_all(oid))
                .map_err(|e| io("write", e))?;
        }
        let pack_len_offset = handle.stream_position().map_err(|e| io("seek", e))?;
        handle
            .write_all(&0u64.to_be_bytes())
            .map_err(|e| io("write", e))?;
        let pack_offset = handle.stream_position().map_err(|e| io("seek", e))?;
        let maximum_frame_pack = (MAX_BACKUP_ARTIFACT_BYTES as u64)
            .checked_sub(pack_offset)
            .and_then(|remaining| remaining.checked_sub(CHECKSUM_LEN as u64))
            .ok_or_else(|| {
                GitBackupError::BadArtifact("backup ref frame leaves no room for a pack".into())
            })?;

        let pack_len = with_packbuilder(repo, refs, |pb| {
            let mut pack_len = 0u64;
            let mut callback_error = None;
            let result = pb.foreach(|chunk| {
                let next = match pack_len.checked_add(chunk.len() as u64) {
                    Some(next) => next,
                    None => {
                        callback_error = Some(GitBackupError::BadArtifact(
                            "backup pack length overflow".into(),
                        ));
                        return false;
                    }
                };
                if next > maximum_pack_bytes as u64 || next > maximum_frame_pack {
                    callback_error = Some(GitBackupError::BadArtifact(format!(
                        "backup pack exceeds the {maximum_pack_bytes}-byte construction limit"
                    )));
                    return false;
                }
                if let Err(error) = handle.write_all(chunk) {
                    callback_error = Some(io("write", error));
                    return false;
                }
                pack_len = next;
                true
            });
            if let Some(error) = callback_error {
                return Err(error);
            }
            result.map_err(|e| git_err("stream pack bytes", e))?;
            Ok(pack_len)
        })?;

        let frame_len = pack_offset
            .checked_add(pack_len)
            .ok_or_else(|| GitBackupError::BadArtifact("backup artifact length overflow".into()))?;
        handle
            .seek(std::io::SeekFrom::Start(pack_len_offset))
            .and_then(|_| handle.write_all(&pack_len.to_be_bytes()))
            .and_then(|()| handle.seek(std::io::SeekFrom::Start(0)).map(|_| ()))
            .map_err(|e| io("patch", e))?;

        // The length field precedes the pack, so preserving the exact v2 wire format requires a
        // second disk pass after it is known. This pass is fixed-memory and hashes the corrected
        // frame, not the placeholder bytes.
        let mut hasher = blake3::Hasher::new();
        let mut remaining = frame_len;
        let mut buffer = [0u8; STREAM_BUFFER_BYTES];
        while remaining != 0 {
            let take = usize::try_from(remaining.min(buffer.len() as u64)).unwrap_or(buffer.len());
            handle
                .read_exact(&mut buffer[..take])
                .map_err(|e| io("read for checksum", e))?;
            hasher.update(&buffer[..take]);
            remaining -= take as u64;
        }
        handle
            .seek(std::io::SeekFrom::Start(frame_len))
            .and_then(|_| handle.write_all(hasher.finalize().as_bytes()))
            .map_err(|e| io("append checksum to", e))?;
        Ok(())
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
        self.write_frame(&mut out)
            .expect("writing a backup frame into Vec cannot fail");
        out
    }

    fn write_frame(&self, writer: &mut impl std::io::Write) -> Result<(), std::io::Error> {
        fn hashed_write(
            writer: &mut impl std::io::Write,
            hasher: &mut blake3::Hasher,
            bytes: &[u8],
        ) -> Result<(), std::io::Error> {
            writer.write_all(bytes)?;
            hasher.update(bytes);
            Ok(())
        }

        let mut hasher = blake3::Hasher::new();
        hashed_write(writer, &mut hasher, MAGIC)?;
        hashed_write(writer, &mut hasher, &(self.refs.len() as u32).to_be_bytes())?;
        for (name, oid) in &self.refs {
            let name = name.as_bytes();
            let oid = oid.as_str().as_bytes();
            hashed_write(writer, &mut hasher, &(name.len() as u32).to_be_bytes())?;
            hashed_write(writer, &mut hasher, name)?;
            hashed_write(writer, &mut hasher, &(oid.len() as u32).to_be_bytes())?;
            hashed_write(writer, &mut hasher, oid)?;
        }
        hashed_write(writer, &mut hasher, &(self.pack.len() as u64).to_be_bytes())?;
        hashed_write(writer, &mut hasher, &self.pack)?;
        writer.write_all(hasher.finalize().as_bytes())
    }

    /// **Reconstruct a backup from its serialized bytes** — the artifact-alone path. Any shortfall
    /// (bad magic, a truncated length, non-utf8 ref) is a LOUD [`GitBackupError::BadArtifact`],
    /// never a silent partial parse.
    pub fn deserialize(bytes: &[u8]) -> Result<GitRepoBackup, GitBackupError> {
        Self::deserialize_bounded(bytes, MAX_BACKUP_ARTIFACT_BYTES, MAX_BACKUP_PACK_BYTES)
    }

    fn deserialize_bounded(
        bytes: &[u8],
        maximum_artifact_bytes: usize,
        maximum_pack_bytes: usize,
    ) -> Result<GitRepoBackup, GitBackupError> {
        if bytes.len() > maximum_artifact_bytes {
            return Err(GitBackupError::BadArtifact(format!(
                "backup artifact exceeds the {maximum_artifact_bytes}-byte read limit"
            )));
        }
        if bytes.len() < MAGIC.len() {
            return Err(GitBackupError::BadArtifact(
                "artifact is shorter than its magic".into(),
            ));
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
        if ref_count > MAX_BACKUP_REFS || minimum_ref_bytes.saturating_add(8) > cur.remaining() {
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
        if pack_len > maximum_pack_bytes {
            return Err(GitBackupError::BadArtifact(format!(
                "backup pack exceeds the {maximum_pack_bytes}-byte parse limit"
            )));
        }
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
        crate::durable::write_file_atomic_with(parent, path, |handle| {
            self.write_frame(handle).map_err(|error| {
                DurableError::Io(format!("write backup artifact {}: {error}", path.display()))
            })
        })
        .map_err(GitBackupError::Durable)
    }

    /// Read + reconstruct the artifact from an off-host file.
    pub fn read_from_file(path: &Path) -> Result<GitRepoBackup, GitBackupError> {
        Self::read_from_file_bounded(path, MAX_BACKUP_ARTIFACT_BYTES)
    }

    fn read_from_file_bounded(
        path: &Path,
        maximum_artifact_bytes: usize,
    ) -> Result<GitRepoBackup, GitBackupError> {
        let file = std::fs::File::open(path)
            .map_err(|e| GitBackupError::Io(format!("open artifact {}: {e}", path.display())))?;
        let file_bytes = file
            .metadata()
            .map_err(|e| GitBackupError::Io(format!("stat artifact {}: {e}", path.display())))?
            .len();
        if file_bytes > maximum_artifact_bytes as u64 {
            return Err(GitBackupError::BadArtifact(format!(
                "backup artifact exceeds the {maximum_artifact_bytes}-byte read limit"
            )));
        }
        let mut bytes = Vec::new();
        file.take((maximum_artifact_bytes as u64).saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|e| GitBackupError::Io(format!("read artifact {}: {e}", path.display())))?;
        if bytes.len() > maximum_artifact_bytes {
            return Err(GitBackupError::BadArtifact(format!(
                "backup artifact exceeds the {maximum_artifact_bytes}-byte read limit"
            )));
        }
        GitRepoBackup::deserialize_bounded(
            &bytes,
            maximum_artifact_bytes,
            MAX_BACKUP_PACK_BYTES.min(maximum_artifact_bytes),
        )
    }
}

/// A bounded, verified backup artifact whose pack remains on disk. The private fields ensure a
/// streamed restore can only start from an artifact that passed exact framing/ref validation and,
/// for v2, the outer BLAKE3 checksum. The opened handle is retained so replacing `path` after
/// verification cannot swap in different bytes at restore time.
#[derive(Debug)]
pub struct VerifiedGitRepoBackupFile {
    file: std::fs::File,
    refs: Vec<(String, Oid)>,
    pack_offset: u64,
    pack_len: u64,
    pack_digest: [u8; CHECKSUM_LEN],
}

impl VerifiedGitRepoBackupFile {
    /// Open and fully verify a v1 or v2 artifact with fixed-size pack reads. v1 remains readable for
    /// compatibility; it has no outer checksum by definition, but its pack digest is still pinned
    /// here and rechecked while libgit2 ingests the pack during restore.
    pub fn open(path: &Path) -> Result<Self, GitBackupError> {
        let mut file = std::fs::File::open(path)
            .map_err(|e| GitBackupError::Io(format!("open artifact {}: {e}", path.display())))?;
        let file_len = file
            .metadata()
            .map_err(|e| GitBackupError::Io(format!("stat artifact {}: {e}", path.display())))?
            .len();
        if file_len > MAX_BACKUP_ARTIFACT_BYTES as u64 {
            return Err(GitBackupError::BadArtifact(format!(
                "backup artifact exceeds the {MAX_BACKUP_ARTIFACT_BYTES}-byte read limit"
            )));
        }
        if file_len < MAGIC.len() as u64 {
            return Err(GitBackupError::BadArtifact(
                "artifact is shorter than its magic".into(),
            ));
        }

        let mut position = 0u64;
        let mut frame_hasher = blake3::Hasher::new();
        let mut magic = vec![0u8; MAGIC.len()];
        read_artifact_exact(&mut file, &mut magic, path)?;
        position += magic.len() as u64;
        frame_hasher.update(&magic);
        let checksum_bytes = if magic == MAGIC_V2 {
            CHECKSUM_LEN
        } else if magic == MAGIC_V1 {
            0
        } else {
            return Err(GitBackupError::BadArtifact(format!(
                "unknown backup artifact magic {:?}",
                &magic[..magic.len().min(MAGIC.len())]
            )));
        };
        let frame_end = file_len.checked_sub(checksum_bytes as u64).ok_or_else(|| {
            GitBackupError::BadArtifact("v2 artifact is missing its checksum".into())
        })?;
        if frame_end < position {
            return Err(GitBackupError::BadArtifact(
                "v2 artifact is missing its checksum".into(),
            ));
        }

        let ref_count =
            read_frame_u32(&mut file, &mut position, frame_end, &mut frame_hasher, path)? as usize;
        if ref_count > MAX_BACKUP_REFS {
            return Err(GitBackupError::BadArtifact(format!(
                "impossible or excessive ref count {ref_count}"
            )));
        }
        let minimum_ref_bytes = (ref_count as u64).checked_mul(8).ok_or_else(|| {
            GitBackupError::BadArtifact("ref-count size overflow in artifact frame".into())
        })?;
        if minimum_ref_bytes.saturating_add(8) > frame_end.saturating_sub(position) {
            return Err(GitBackupError::BadArtifact(format!(
                "impossible or excessive ref count {ref_count}"
            )));
        }

        let mut refs = Vec::new();
        refs.try_reserve(ref_count.min(1024)).map_err(|_| {
            GitBackupError::BadArtifact("cannot allocate backup ref parser state".into())
        })?;
        let mut ref_bytes = MAGIC.len() + 4 + 8 + checksum_bytes;
        for _ in 0..ref_count {
            let name_len =
                read_frame_u32(&mut file, &mut position, frame_end, &mut frame_hasher, path)?
                    as usize;
            ref_bytes = ref_bytes
                .checked_add(4)
                .and_then(|bytes| bytes.checked_add(name_len))
                .ok_or_else(|| GitBackupError::BadArtifact("ref frame length overflow".into()))?;
            if ref_bytes > MAX_BACKUP_REF_FRAME_BYTES
                || name_len as u64 > frame_end.saturating_sub(position)
            {
                return Err(GitBackupError::BadArtifact(
                    "backup ref frame exceeds its bounded length".into(),
                ));
            }
            let mut name = vec![0u8; name_len];
            read_hashed_frame_exact(
                &mut file,
                &mut name,
                &mut position,
                frame_end,
                &mut frame_hasher,
                path,
            )?;
            let name = String::from_utf8(name)
                .map_err(|e| GitBackupError::BadArtifact(format!("ref name not utf8: {e}")))?;
            if !git2::Reference::is_valid_name(&name) {
                return Err(GitBackupError::BadArtifact(format!(
                    "invalid fully-qualified ref name `{name}`"
                )));
            }

            let oid_len =
                read_frame_u32(&mut file, &mut position, frame_end, &mut frame_hasher, path)?
                    as usize;
            ref_bytes = ref_bytes
                .checked_add(4)
                .and_then(|bytes| bytes.checked_add(oid_len))
                .ok_or_else(|| GitBackupError::BadArtifact("ref frame length overflow".into()))?;
            if ref_bytes > MAX_BACKUP_REF_FRAME_BYTES
                || oid_len as u64 > frame_end.saturating_sub(position)
            {
                return Err(GitBackupError::BadArtifact(
                    "backup ref frame exceeds its bounded length".into(),
                ));
            }
            let mut oid = vec![0u8; oid_len];
            read_hashed_frame_exact(
                &mut file,
                &mut oid,
                &mut position,
                frame_end,
                &mut frame_hasher,
                path,
            )?;
            let oid = String::from_utf8(oid)
                .map_err(|e| GitBackupError::BadArtifact(format!("ref oid not utf8: {e}")))?;
            git2::Oid::from_str(&oid).map_err(|e| {
                GitBackupError::BadArtifact(format!("invalid oid for ref `{name}`: {e}"))
            })?;
            refs.push((name, Oid::new(oid)));
        }

        let pack_len =
            read_frame_u64(&mut file, &mut position, frame_end, &mut frame_hasher, path)?;
        if pack_len > MAX_BACKUP_PACK_BYTES as u64 {
            return Err(GitBackupError::BadArtifact(format!(
                "backup pack exceeds the {MAX_BACKUP_PACK_BYTES}-byte parse limit"
            )));
        }
        let pack_offset = position;
        let declared_end = pack_offset.checked_add(pack_len).ok_or_else(|| {
            GitBackupError::BadArtifact("pack length overflows the artifact frame".into())
        })?;
        if declared_end != frame_end {
            let detail = if declared_end < frame_end {
                format!(
                    "{} trailing bytes after backup frame",
                    frame_end - declared_end
                )
            } else {
                "artifact is truncated within its declared pack".into()
            };
            return Err(GitBackupError::BadArtifact(detail));
        }

        let mut pack_hasher = blake3::Hasher::new();
        let mut remaining = pack_len;
        let mut buffer = [0u8; STREAM_BUFFER_BYTES];
        while remaining != 0 {
            let take = remaining.min(buffer.len() as u64) as usize;
            read_hashed_frame_exact(
                &mut file,
                &mut buffer[..take],
                &mut position,
                frame_end,
                &mut frame_hasher,
                path,
            )?;
            pack_hasher.update(&buffer[..take]);
            remaining -= take as u64;
        }

        if checksum_bytes != 0 {
            let mut stored_checksum = [0u8; CHECKSUM_LEN];
            read_artifact_exact(&mut file, &mut stored_checksum, path)?;
            if frame_hasher.finalize().as_bytes() != &stored_checksum {
                return Err(GitBackupError::BadArtifact(
                    "v2 artifact checksum mismatch".into(),
                ));
            }
        }
        let mut trailing = [0u8; 1];
        if file
            .read(&mut trailing)
            .map_err(|e| GitBackupError::Io(format!("read artifact {}: {e}", path.display())))?
            != 0
        {
            return Err(GitBackupError::BadArtifact(
                "trailing bytes after backup artifact".into(),
            ));
        }

        Ok(Self {
            file,
            refs,
            pack_offset,
            pack_len,
            pack_digest: *pack_hasher.finalize().as_bytes(),
        })
    }

    /// The validated ref snapshot stored outside the on-disk pack.
    pub fn refs(&self) -> &[(String, Oid)] {
        &self.refs
    }

    /// The number of validated refs in the artifact.
    pub fn ref_count(&self) -> usize {
        self.refs.len()
    }

    /// The pack length without materialising the pack.
    pub fn pack_len(&self) -> u64 {
        self.pack_len
    }
}

fn read_artifact_exact(
    file: &mut std::fs::File,
    bytes: &mut [u8],
    path: &Path,
) -> Result<(), GitBackupError> {
    file.read_exact(bytes).map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            GitBackupError::BadArtifact("artifact is truncated".into())
        } else {
            GitBackupError::Io(format!("read artifact {}: {error}", path.display()))
        }
    })
}

fn read_hashed_frame_exact(
    file: &mut std::fs::File,
    bytes: &mut [u8],
    position: &mut u64,
    frame_end: u64,
    hasher: &mut blake3::Hasher,
    path: &Path,
) -> Result<(), GitBackupError> {
    let end = position
        .checked_add(bytes.len() as u64)
        .ok_or_else(|| GitBackupError::BadArtifact("length overflow in artifact frame".into()))?;
    if end > frame_end {
        return Err(GitBackupError::BadArtifact(format!(
            "artifact truncated: needed {} bytes at offset {}",
            bytes.len(),
            position
        )));
    }
    read_artifact_exact(file, bytes, path)?;
    hasher.update(bytes);
    *position = end;
    Ok(())
}

fn read_frame_u32(
    file: &mut std::fs::File,
    position: &mut u64,
    frame_end: u64,
    hasher: &mut blake3::Hasher,
    path: &Path,
) -> Result<u32, GitBackupError> {
    let mut bytes = [0u8; 4];
    read_hashed_frame_exact(file, &mut bytes, position, frame_end, hasher, path)?;
    Ok(u32::from_be_bytes(bytes))
}

fn read_frame_u64(
    file: &mut std::fs::File,
    position: &mut u64,
    frame_end: u64,
    hasher: &mut blake3::Hasher,
    path: &Path,
) -> Result<u64, GitBackupError> {
    let mut bytes = [0u8; 8];
    read_hashed_frame_exact(file, &mut bytes, position, frame_end, hasher, path)?;
    Ok(u64::from_be_bytes(bytes))
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
    restore_repo_staged(store, loc, |repo| build_repo_from_memory(repo, backup))
}

/// Restore a verified file-backed artifact without allocating its pack. The held file handle is
/// sought to the verified pack offset and copied into libgit2's indexer in fixed-size chunks. Its
/// pack digest is recomputed during that copy and must still match before the indexer is committed,
/// refs are recreated, or the staging repo is published; this catches post-open inode mutation.
pub fn restore_repo_from_file<P: RepoPathResolver>(
    store: &DurableGitStore<P>,
    loc: &RepoLoc,
    backup: &mut VerifiedGitRepoBackupFile,
) -> Result<DurableGitRepo, GitBackupError> {
    restore_repo_staged(store, loc, |repo| build_repo_from_file(repo, backup))
}

fn restore_repo_staged<P: RepoPathResolver>(
    store: &DurableGitStore<P>,
    loc: &RepoLoc,
    build: impl FnOnce(&DurableGitRepo) -> Result<(), GitBackupError>,
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
    let build_result = store
        .create_repo(&staging_loc)
        .map_err(GitBackupError::Durable)
        .and_then(|repo| build(&repo));
    if let Err(e) = build_result {
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
    let parent = final_path.parent().ok_or_else(|| {
        GitBackupError::Io(format!(
            "restore target {} has no parent directory",
            final_path.display()
        ))
    })?;
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|e| {
            GitBackupError::Io(format!(
                "sync restore publish directory {}: {e}",
                parent.display()
            ))
        })?;

    // Open the published repo through the validated store (the returned handle is on the final path).
    store.open_repo(loc).map_err(GitBackupError::Durable)
}

fn build_repo_from_memory(
    repo: &DurableGitRepo,
    backup: &GitRepoBackup,
) -> Result<(), GitBackupError> {
    // Ingest the self-contained pack into the empty odb (libgit2 indexes + verifies it).
    if !backup.pack.is_empty() {
        let git = git2::Repository::open(repo.path())
            .map_err(|e| git_err(&format!("open restore target {}", repo.path().display()), e))?;
        let odb = git.odb().map_err(|e| git_err("restore odb", e))?;
        let mut pw = odb
            .packwriter()
            .map_err(|e| git_err("restore packwriter", e))?;
        {
            use std::io::Write;
            pw.write_all(&backup.pack)
                .map_err(|e| GitBackupError::Io(format!("feed pack to indexer: {e}")))?;
        }
        pw.commit()
            .map_err(|e| git_err("commit ingested pack", e))?;
    }

    recreate_refs(repo, &backup.refs)
}

fn build_repo_from_file(
    repo: &DurableGitRepo,
    backup: &mut VerifiedGitRepoBackupFile,
) -> Result<(), GitBackupError> {
    backup
        .file
        .seek(std::io::SeekFrom::Start(backup.pack_offset))
        .map_err(|e| GitBackupError::Io(format!("seek verified backup pack: {e}")))?;
    let mut pack_hasher = blake3::Hasher::new();
    if backup.pack_len != 0 {
        let git = git2::Repository::open(repo.path())
            .map_err(|e| git_err(&format!("open restore target {}", repo.path().display()), e))?;
        let odb = git.odb().map_err(|e| git_err("restore odb", e))?;
        let mut pw = odb
            .packwriter()
            .map_err(|e| git_err("restore packwriter", e))?;
        let mut remaining = backup.pack_len;
        let mut buffer = [0u8; STREAM_BUFFER_BYTES];
        while remaining != 0 {
            let take = remaining.min(buffer.len() as u64) as usize;
            backup
                .file
                .read_exact(&mut buffer[..take])
                .map_err(|error| {
                    if error.kind() == std::io::ErrorKind::UnexpectedEof {
                        GitBackupError::BadArtifact(
                            "verified backup pack was truncated after open".into(),
                        )
                    } else {
                        GitBackupError::Io(format!("read verified backup pack: {error}"))
                    }
                })?;
            pack_hasher.update(&buffer[..take]);
            pw.write_all(&buffer[..take])
                .map_err(|e| GitBackupError::Io(format!("feed pack to indexer: {e}")))?;
            remaining -= take as u64;
        }
        if pack_hasher.finalize().as_bytes() != &backup.pack_digest {
            return Err(GitBackupError::BadArtifact(
                "verified backup pack changed after open".into(),
            ));
        }
        pw.commit()
            .map_err(|e| git_err("commit ingested pack", e))?;
    } else if pack_hasher.finalize().as_bytes() != &backup.pack_digest {
        return Err(GitBackupError::BadArtifact(
            "verified backup pack changed after open".into(),
        ));
    }

    recreate_refs(repo, &backup.refs)
}

fn recreate_refs(repo: &DurableGitRepo, refs: &[(String, Oid)]) -> Result<(), GitBackupError> {
    // Recreate each ref through the durable CAS create path (on-disk, reflog-logged). A ref can only
    // publish after its object was accepted by the indexer above.
    for (name, oid) in refs {
        repo.update_ref_cas(
            name,
            None,
            Some(oid),
            "restore: recreate ref",
            "restore@myelin.noreply",
        )
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
        assert!(
            backup.pack_len() > 0,
            "a real packfile, not an empty modeled blob"
        );

        let bytes = backup.serialize();
        let back = GitRepoBackup::deserialize(&bytes).unwrap();
        assert_eq!(
            back, backup,
            "artifact reconstructs identically from its bytes alone"
        );

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

        let legacy_file = temp_root("legacy-v1-file");
        std::fs::write(&legacy_file, &legacy_v1).unwrap();
        let mut verified = VerifiedGitRepoBackupFile::open(&legacy_file).unwrap();
        assert_eq!(verified.ref_count(), 0);
        assert_eq!(verified.pack_len(), 0);
        let legacy_restore_root = temp_root("legacy-v1-restore");
        let legacy_store = DurableGitStore::rooted(&legacy_restore_root);
        let restored = restore_repo_from_file(&legacy_store, &loc(), &mut verified).unwrap();
        restored.fsck().unwrap();
        std::fs::remove_file(&legacy_file).ok();
        std::fs::remove_dir_all(&legacy_restore_root).ok();

        let corrupt_file = temp_root("corrupt-v2-file");
        let mut corrupt_v2 = empty.serialize();
        *corrupt_v2.last_mut().unwrap() ^= 1;
        std::fs::write(&corrupt_file, corrupt_v2).unwrap();
        assert!(matches!(
            VerifiedGitRepoBackupFile::open(&corrupt_file),
            Err(GitBackupError::BadArtifact(message)) if message.contains("checksum")
        ));
        std::fs::remove_file(&corrupt_file).ok();
    }

    #[test]
    fn backup_pack_and_artifact_limits_fail_before_unbounded_allocation() {
        let root = temp_root("bounded-pack");
        let store = DurableGitStore::rooted(&root);
        let repo = store.create_repo(&loc()).unwrap();
        seed(&repo);
        let normal = GitRepoBackup::create(&repo).unwrap();
        assert_eq!(
            GitRepoBackup::create_bounded(&repo, normal.pack_len())
                .unwrap()
                .pack_len(),
            normal.pack_len(),
            "the exact pack limit succeeds"
        );
        assert!(matches!(
            GitRepoBackup::create_bounded(&repo, normal.pack_len() - 1),
            Err(GitBackupError::BadArtifact(message)) if message.contains("construction limit")
        ));

        let exact_artifact = root.join("exact.gitbackup");
        let exact =
            GitRepoBackup::create_to_file_bounded(&repo, &exact_artifact, normal.pack_len())
                .unwrap();
        assert_eq!(exact.pack_len(), normal.pack_len() as u64);
        assert_eq!(
            GitRepoBackup::read_from_file(&exact_artifact).unwrap(),
            normal,
            "the streamed v2 artifact remains readable by the existing in-memory API"
        );

        let preserved_artifact = root.join("preserved.gitbackup");
        let prior = b"previous valid publication remains untouched";
        std::fs::write(&preserved_artifact, prior).unwrap();
        assert!(matches!(
            GitRepoBackup::create_to_file_bounded(
                &repo,
                &preserved_artifact,
                normal.pack_len() - 1,
            ),
            Err(GitBackupError::BadArtifact(message)) if message.contains("construction limit")
        ));
        assert_eq!(std::fs::read(&preserved_artifact).unwrap(), prior);

        let mut declared_pack = MAGIC_V1.to_vec();
        declared_pack.extend_from_slice(&0u32.to_be_bytes());
        declared_pack.extend_from_slice(&2u64.to_be_bytes());
        assert!(matches!(
            GitRepoBackup::deserialize_bounded(&declared_pack, 64, 1),
            Err(GitBackupError::BadArtifact(message)) if message.contains("parse limit")
        ));

        let artifact = temp_root("oversized-artifact");
        std::fs::write(&artifact, [0u8; 17]).unwrap();
        assert!(matches!(
            GitRepoBackup::read_from_file_bounded(&artifact, 16),
            Err(GitBackupError::BadArtifact(message)) if message.contains("read limit")
        ));

        std::fs::remove_file(&artifact).ok();
        std::fs::remove_dir_all(&root).ok();
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
        // Persist + reload the artifact from disk so the restore truly reads bytes alone.
        let artifact = temp_root("artifact");
        std::fs::create_dir_all(&artifact).unwrap();
        let artifact_file = artifact.join("core.gitbackup");
        GitRepoBackup::create_to_file(&src_repo, &artifact_file).unwrap();

        // A genuinely CLEAN target root — the original src_root is NOT visible to it.
        let dst_root = temp_root("dst");
        let dst_store = DurableGitStore::rooted(&dst_root);
        assert!(!dst_store.repo_exists(&loc()), "target starts clean/empty");

        let mut reloaded = VerifiedGitRepoBackupFile::open(&artifact_file).unwrap();
        let restored = restore_repo_from_file(&dst_store, &loc(), &mut reloaded).unwrap();

        // Every ref reads back identical.
        assert_eq!(restored.list_refs_bounded(MAX_BACKUP_REFS).unwrap(), want);
        // Every source object exists + reads back byte-identical in the restored odb.
        for (_, tip) in &want {
            let src_bytes = src_repo.read_object_bounded(tip, 64 * 1024 * 1024).unwrap();
            let dst_bytes = restored.read_object_bounded(tip, 64 * 1024 * 1024).unwrap();
            assert_eq!(
                src_bytes,
                dst_bytes,
                "object {} bytes identical",
                tip.as_str()
            );
        }
        restored
            .fsck()
            .expect("in-process fsck clean on the restored repo");

        // Restoring AGAIN over the now-present repo is refused (clean-target guard).
        assert!(matches!(
            restore_repo_from_file(&dst_store, &loc(), &mut reloaded),
            Err(GitBackupError::TargetNotClean(_))
        ));

        std::fs::remove_dir_all(&src_root).ok();
        std::fs::remove_dir_all(&dst_root).ok();
        std::fs::remove_dir_all(&artifact).ok();
    }

    #[test]
    fn pack_mutation_after_verified_open_is_refused_before_publish() {
        let src_root = temp_root("mutated-pack-src");
        let src_store = DurableGitStore::rooted(&src_root);
        let src_repo = src_store.create_repo(&loc()).unwrap();
        seed(&src_repo);
        let artifact = temp_root("mutated-pack-artifact");
        let mut verified = GitRepoBackup::create_to_file(&src_repo, &artifact).unwrap();
        assert!(verified.pack_len > 0);

        let mut writer = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&artifact)
            .unwrap();
        let mutation_offset = verified.pack_offset + verified.pack_len - 1;
        writer
            .seek(std::io::SeekFrom::Start(mutation_offset))
            .unwrap();
        let mut byte = [0u8; 1];
        writer.read_exact(&mut byte).unwrap();
        byte[0] ^= 0xff;
        writer
            .seek(std::io::SeekFrom::Start(mutation_offset))
            .unwrap();
        writer.write_all(&byte).unwrap();
        writer.sync_all().unwrap();

        let dst_root = temp_root("mutated-pack-dst");
        let dst_store = DurableGitStore::rooted(&dst_root);
        let error = restore_repo_from_file(&dst_store, &loc(), &mut verified).unwrap_err();
        assert!(matches!(
            error,
            GitBackupError::BadArtifact(message) if message.contains("changed after open")
        ));
        assert!(!dst_store.repo_exists(&loc()));

        std::fs::remove_dir_all(&src_root).ok();
        std::fs::remove_dir_all(&dst_root).ok();
        std::fs::remove_file(&artifact).ok();
    }
}
