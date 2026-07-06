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
//! [`DurableGitRepo::read_object`]): real `fs`-backed git objects, `git fsck`-clean, survive restart.
//! The generic content-addressed [`myelin_storage::FsBlobStore`] (the `Mutex<HashMap>` byte tier the
//! [`crate::pack_tier`] rides) is a SEPARATE track — its real on-disk/object-store byte backing is
//! P-ST-30 (census SI-014/015/029), already carried in the `no-in-memory-durable-store` baseline.
//! GT-001 fixes the **git object durability** here; the generic blob tier's backing swap is not this
//! prompt (and `myelin-git` is out of the spine lint's scan scope, so this module changes no ratchet).
//!
//! The smart-transport WIRE (`clone`/`push` over the network) is **GT-006** (sandbox-gated) — NOT
//! this module. This is the durable STORAGE the wire (and the API/UI/CLI) will sit on.

use std::path::{Path, PathBuf};

use crate::core::{Oid, RepoLoc};
use crate::gix_backend::{RepoPathResolver, RootedResolver};

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
        }
    }
}

impl std::error::Error for DurableError {}

fn git_err(ctx: &str, e: git2::Error) -> DurableError {
    DurableError::Git(format!("{ctx}: {e}"))
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

/// Project a libgit2 commit into the PII-free [`CommitMeta`] read shape.
fn commit_meta(c: &git2::Commit<'_>) -> CommitMeta {
    let author = c.author();
    // `Commit::summary` takes `&mut self`; derive the first message line (a `&self` accessor) instead.
    let message = c.message().unwrap_or("");
    CommitMeta {
        oid: c.id().to_string(),
        summary: message.lines().next().unwrap_or("").to_string(),
        author_name: author.name().unwrap_or("").to_string(),
        author_email: author.email().unwrap_or("").to_string(),
        time: c.time().seconds(),
        parents: c.parent_ids().map(|p| p.to_string()).collect(),
    }
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

    fn open_git(&self) -> Result<git2::Repository, DurableError> {
        git2::Repository::open(&self.path)
            .map_err(|e| git_err(&format!("open bare repo {}", self.path.display()), e))
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

    /// Read an object's raw bytes from the on-disk odb by its git oid. `NotFound` if absent — the
    /// lookup is the real odb, NOT an in-memory index (F-git-2).
    pub fn read_object(&self, oid: &Oid) -> Result<Vec<u8>, DurableError> {
        let repo = self.open_git()?;
        let odb = repo.odb().map_err(|e| git_err("odb", e))?;
        let obj = odb
            .read(Self::parse_oid(oid)?)
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

    /// List every ref `(name, tip)` on disk. The repo's entry points — loaded from disk, never an
    /// empty map (SI-012).
    pub fn list_refs(&self) -> Result<Vec<(String, Oid)>, DurableError> {
        let repo = self.open_git()?;
        let refs = repo.references().map_err(|e| git_err("references", e))?;
        let mut out = Vec::new();
        for r in refs {
            let r = r.map_err(|e| git_err("reference iter", e))?;
            if let Some(oid) = r.target() {
                let name = r.name().unwrap_or_default();
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
        // Set the committer identity on this op so the reflog records the pusher PSEUDONYM (GIT-1)
        // — libgit2 reads `user.name`/`user.email` for the reflog committer.
        {
            let mut cfg = repo.config().map_err(|e| git_err("config", e))?;
            cfg.set_str("user.name", committer_pseudonym)
                .map_err(|e| git_err("set user.name", e))?;
            cfg.set_str("user.email", committer_pseudonym)
                .map_err(|e| git_err("set user.email", e))?;
        }

        let actual = self.read_ref(name)?;
        let expected_norm = expected.cloned();
        if actual != expected_norm {
            return Err(DurableError::CasMismatch {
                ref_name: name.to_string(),
                expected: expected_norm.map(|o| o.0),
                actual: actual.map(|o| o.0),
            });
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
        let current = cfg.get_i64(&key).unwrap_or(0).max(0);
        cfg.set_i64(&key, current + 1)
            .map_err(|e| git_err(&format!("set refgen for {name}"), e))?;
        Ok(())
    }

    /// **The durable, monotonic per-ref generation** (R0.4 / git #1 HIGH — the recovery fence the
    /// reconciler compares `update_seq` against). Reads the `myelin.refgen.<encoded-ref>` config counter
    /// (0 if the ref was never written). Unlike [`Self::reflog_len`], this does NOT reset when a ref is
    /// deleted and recreated (it is keyed by name in config, not tied to the ref's reflog), and it
    /// survives a process restart (config is on disk). This is the source of truth both the write path
    /// ([`crate::receive_pack`]) and the reconciler ([`crate::reconcile`]) use for `update_seq`.
    pub fn ref_generation(&self, name: &str) -> u64 {
        let Ok(repo) = self.open_git() else { return 0 };
        let Ok(cfg) = repo.config() else { return 0 };
        cfg.get_i64(&refgen_key(name)).unwrap_or(0).max(0) as u64
    }

    /// The number of entries in a ref's on-disk reflog (0 if the ref / reflog does not exist). This is
    /// the reflog ENTRY COUNT — used only for the reflog listing view / entry-count assertions. It is
    /// **NOT** the durable generation (R0.4 / git #1 HIGH): the reflog is destroyed when a ref is
    /// deleted, so this count RESETS on a delete+recreate while the true generation must keep climbing.
    /// Use [`Self::ref_generation`] for the monotonic per-ref generation / recovery fence.
    pub fn reflog_len(&self, name: &str) -> usize {
        let Ok(repo) = self.open_git() else { return 0 };
        match repo.reflog(name) {
            Ok(log) => log.len(),
            Err(_) => 0,
        }
    }

    /// Read a ref's durable on-disk reflog, oldest-first (git stores newest-first; we reverse so the
    /// Nth entry is the Nth update — the `update_seq` ordering the [`crate::receive_pack::RefStore`]
    /// view expects).
    pub fn reflog_entries(&self, name: &str) -> Result<Vec<DurableReflogEntry>, DurableError> {
        let repo = self.open_git()?;
        let log = match repo.reflog(name) {
            Ok(log) => log,
            Err(e) if e.code() == git2::ErrorCode::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(git_err(&format!("reflog {name}"), e)),
        };
        let mut out = Vec::with_capacity(log.len());
        // git reflog is stored newest-first; iterate in reverse for oldest-first (update order).
        for entry in log.iter().rev() {
            let old = entry.id_old();
            let old_oid = if old.is_zero() {
                None
            } else {
                Some(Oid::new(old.to_string()))
            };
            out.push(DurableReflogEntry {
                old_oid,
                new_oid: Oid::new(entry.id_new().to_string()),
                committer: entry
                    .committer()
                    .name()
                    .map(|s| s.to_string())
                    .unwrap_or_default(),
                message: entry
                    .message()
                    .ok()
                    .flatten()
                    .map(|s| s.to_string())
                    .unwrap_or_default(),
            });
        }
        Ok(out)
    }

    // ── working-tree reads + the single-file commit build (GT-003 web-edit) ──

    /// Resolve a ref to its tip commit (`None` if the ref does not exist).
    fn tip_commit(&self, repo: &git2::Repository, ref_name: &str) -> Result<Option<git2::Oid>, DurableError> {
        match repo.find_reference(ref_name) {
            Ok(r) => Ok(r.target()),
            Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(e) => Err(git_err(&format!("find_reference {ref_name}"), e)),
        }
    }

    /// Read a single TOP-LEVEL file at a ref (`Some((bytes, blob_oid))`, or `None` if the ref/file is
    /// absent). The blob oid is the GF-6 content-address base the web-edit CAS keys on. (The edge router
    /// matches a single path segment — nested paths are the URL-codec follow-on; this reads the top tree.)
    pub fn read_file_at_ref(
        &self,
        ref_name: &str,
        path: &str,
    ) -> Result<Option<(Vec<u8>, Oid)>, DurableError> {
        let repo = self.open_git()?;
        let Some(tip) = self.tip_commit(&repo, ref_name)? else {
            return Ok(None);
        };
        let commit = repo.find_commit(tip).map_err(|e| git_err("find commit", e))?;
        let tree = commit.tree().map_err(|e| git_err("commit tree", e))?;
        let entry = match tree.get_name(path) {
            Some(e) => e,
            None => return Ok(None),
        };
        let obj = entry.to_object(&repo).map_err(|e| git_err("entry object", e))?;
        let blob = match obj.as_blob() {
            Some(b) => b,
            None => return Ok(None), // a dir at that name — not a file
        };
        Ok(Some((blob.content().to_vec(), Oid::new(entry.id().to_string()))))
    }

    /// List a ref's TOP-LEVEL tree entries `(name, is_dir)` (empty if the ref does not exist). The repo
    /// home ViewModel's file tree (durable — read from the real on-disk tree, never a seeded list).
    pub fn tree_entries_at_ref(&self, ref_name: &str) -> Result<Vec<(String, bool)>, DurableError> {
        let repo = self.open_git()?;
        let Some(tip) = self.tip_commit(&repo, ref_name)? else {
            return Ok(Vec::new());
        };
        let commit = repo.find_commit(tip).map_err(|e| git_err("find commit", e))?;
        let tree = commit.tree().map_err(|e| git_err("commit tree", e))?;
        let mut out = Vec::new();
        for entry in tree.iter() {
            let is_dir = matches!(entry.kind(), Some(git2::ObjectType::Tree));
            out.push((entry.name().unwrap_or_default().to_string(), is_dir));
        }
        out.sort();
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

    /// The full detail of one commit (`None` if the oid is malformed or absent): its metadata, full
    /// message, and the per-file unified diff against the FIRST parent (the root commit diffs against
    /// the empty tree). Reuses libgit2's `diff_tree_to_tree` over the REAL on-disk trees.
    pub fn commit_detail(&self, oid_str: &str) -> Result<Option<CommitDetail>, DurableError> {
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

        // Two cooperating callbacks share one accumulator via RefCell: file_cb opens a new file delta,
        // line_cb appends lines to the current (last) one. libgit2 calls file_cb before its lines.
        let files: std::cell::RefCell<Vec<FileDelta>> = std::cell::RefCell::new(Vec::new());
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
                    f.lines.push((origin, content));
                }
            }
            true
        };
        diff.foreach(&mut file_cb, None, None, Some(&mut line_cb))
            .map_err(|e| git_err("diff foreach", e))?;

        Ok(Some(CommitDetail {
            meta: commit_meta(&commit),
            message: commit.message().unwrap_or("").to_string(),
            files: files.into_inner(),
        }))
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
        for (name, tip) in self.list_refs()? {
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
        git2::Repository::open(&path)
            .map_err(|_| DurableError::NotFound(format!("bare repo {}", path.display())))?;
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
        let bytes = repo2.read_object(&commit).expect("read object");
        assert!(
            std::str::from_utf8(&bytes).unwrap().contains("psn-7@acme.noreply"),
            "the durable commit carries the pseudonymous author"
        );
        // list_refs loads the entry point from disk (not an empty map).
        assert_eq!(
            repo2.list_refs().expect("list"),
            vec![("refs/heads/main".to_string(), commit)]
        );

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

        // The ROOT commit diffs against the empty tree → file.txt ADDED.
        let root_detail = repo.commit_detail(&c1.0).unwrap().unwrap();
        assert_eq!(root_detail.files[0].status, 'A');

        // A malformed/absent oid → None (a clean 404 upstream; never a panic).
        assert!(repo.commit_detail("not-a-real-oid").unwrap().is_none());

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
        assert_eq!(repo.reflog_len("refs/heads/main"), 2, "two updates logged");

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
        assert_eq!(repo.ref_generation("refs/heads/tmp"), 1, "create is generation 1");

        repo.update_ref_cas("refs/heads/tmp", Some(&c1), None, "delete", "psn@acme.noreply")
            .expect("delete");
        assert_eq!(repo.read_ref("refs/heads/tmp").unwrap(), None, "ref deleted");
        assert_eq!(
            repo.ref_generation("refs/heads/tmp"),
            2,
            "the delete ADVANCES the durable generation (a delete is a generation-advancing event)"
        );

        repo.update_ref_cas("refs/heads/tmp", None, Some(&c1), "recreate", "psn@acme.noreply")
            .expect("recreate");
        // libgit2 restarts the ref's reflog on recreate — that is the OLD (wrong) generation source.
        assert_eq!(
            repo.reflog_len("refs/heads/tmp"),
            1,
            "the recreated ref's reflog restarts (libgit2 behaviour — why reflog_len was wrong)"
        );
        // The DURABLE generation does NOT reset — it keeps climbing across the delete (the fix).
        assert_eq!(
            repo.ref_generation("refs/heads/tmp"),
            3,
            "the durable per-ref generation is monotonic across delete+recreate (R0.4 fix)"
        );

        // And it survives a restart: a FRESH store + handle over the same root reads the same value.
        drop(repo);
        let store2 = DurableGitStore::rooted(&root);
        let repo2 = store2.open_repo(&loc()).expect("reopen");
        assert_eq!(
            repo2.ref_generation("refs/heads/tmp"),
            3,
            "the durable generation survives a process restart (config is on disk)"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    // ── R0.7-D / DELTA N4: full push-connectivity (missing-ancestor rejection) ──

    /// Copy one object's raw bytes from `src`'s odb into `dst`'s odb, preserving its oid (git2
    /// re-hashes on write, so we assert the oid is identical — a forged copy is impossible). Used to
    /// stage a `dst` odb that is MISSING a chosen ancestor commit while its tip tree is complete.
    fn copy_object(src: &DurableGitRepo, dst: &DurableGitRepo, oid: &Oid, kind: &str) {
        let bytes = src.read_object(oid).expect("read src object");
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
        assert_eq!(
            dst.history_connectivity_complete(&c3, &[]).unwrap(),
            false,
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
            dst.history_connectivity_complete(&c3, &[c2.clone()]).unwrap(),
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
        assert_eq!(
            dst.history_connectivity_complete(&c3, &[c2]).unwrap(),
            false,
            "a new commit whose parent is genuinely absent is rejected regardless of existing_tips"
        );
        // A missing NEW tip is itself a reject (never a swallowed error).
        assert_eq!(
            dst.history_connectivity_complete(&Oid::new("0".repeat(39) + "1"), &[]).unwrap(),
            false,
            "a new_tip that is not a present commit is rejected (fail-closed)"
        );
        std::fs::remove_dir_all(&src_root).ok();
        std::fs::remove_dir_all(&dst_root).ok();
    }
}
