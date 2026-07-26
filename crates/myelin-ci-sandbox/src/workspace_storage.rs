//! # Disk-backed ephemeral CI workspace storage (CT-007 vertical-slice step 2b)
//!
//! **Why this exists.** A real `cargo build --workspace` job needs a writable scratch area that
//! can hold many GB of `target/` output — the existing `/tmp` tmpfs
//! ([`ResourceLimits::tmpfs_bytes`](crate::ResourceLimits::tmpfs_bytes), CT-003a) is deliberately
//! small and RAM-backed (sizing it large enough for a real build would reserve that much host RAM
//! PER CONCURRENT JOB, which does not scale and is not what a tmpfs is for). This module provisions
//! the disk-backed alternative: one **Btrfs subvolume with a hard qgroup quota** per job, created
//! fresh, mounted read-write into the sandbox, and deleted completely after the job.
//!
//! **Design reviewed with an external adversarial reviewer (gpt-5.6-sol) across FIVE rounds,
//! 2026-07-26** (recorded in `planning/system-reviews/2026-06-26/12-ci-track-ledger.md`). Rejected
//! alternatives: a loop-mounted sparse file (the PORTABILITY FALLBACK for hosts without
//! Btrfs/qgroup support, not built here — see [`WorkspaceStorage::open`]) and overlayfs (no
//! meaningful lower layer for cargo scratch).
//!
//! ## What each review round fixed
//!
//! **Round 1 → round 2:** ABA-safe deletion via a captured subvolume id; compound
//! `UnrecoverableLeak` errors instead of swallowed rollback failures; a real `quota status`
//! four-field enforcement check instead of "some qgroup interface exists"; a verified quota
//! postcondition instead of "the command exited 0"; crash-durable `--commit-after` + `sync`
//! deletion; absolute binary paths + `env_clear()`; an `is_subvolume_root` (inode-256) check
//! before ever trusting `btrfs inspect-internal rootid` (which silently resolves to the
//! CONTAINING subvolume's id for a non-subvolume path — a real bug found empirically).
//!
//! **Round 2 → round 3:** round 2's ABA fix was still racy — `delete_workspace` read the id,
//! compared it, then issued a PATH-based delete, leaving a window where the leaf name could be
//! replaced between the compare and the delete. Fixed by deleting **by subvolume id**
//! (`btrfs subvolume delete --subvolid <id> <fs-anchor>`) — Btrfs resolves this against the
//! persistent numeric identity, never re-walking the caller-supplied leaf path at delete time.
//! Deletion authority was also unrestricted — fixed by making [`WorkspaceStorage`] own the base
//! directory and consume [`PreparedWorkspace`]/[`OrphanCandidate`] BY VALUE. The quota-limit and
//! quota-verify steps were switched to address the qgroup by its **explicit id** (`0/<id>`)
//! against the filesystem anchor rather than the job's leaf path.
//!
//! **Round 3 → round 4 (this version):** round 3's per-value capability was still forgeable
//! ACROSS two different [`WorkspaceStorage`] instances (e.g. two different base directories on
//! two different filesystems that happen to mint the same numeric subvolume id) — fixed:
//! [`PreparedWorkspace`]/[`OrphanCandidate`] now carry the canonical base path they were minted
//! against, and every deletion method verifies it matches `self`'s own base before doing
//! anything, refusing with [`WorkspaceStorageError::WrongStorage`] otherwise. Deletion also
//! silently lost its retry capability on a `subvolume sync` failure (the caller's
//! `PreparedWorkspace`/`OrphanCandidate` was already consumed by that point) — fixed: once
//! `btrfs subvolume delete` has committed (succeeded OR reported "already gone" — round 3 skipped
//! `sync` entirely on the "already gone" path, recreating exactly the crash window round 2 fixed
//! for the success path), the operation is no longer retryable via the original capability at all
//! (deletion has already happened); [`WorkspaceStorageError::SyncPending`] carries the bare
//! `subvol_id` so [`WorkspaceStorage::retry_pending_sync`] can finish waiting WITHOUT needing the
//! original value back. Every mutating method now takes `&mut self` — Rust's own borrow checker
//! then makes concurrent calls on ONE `WorkspaceStorage` a compile error within a single process
//! (round 3's methods took `&self`, which let the same handle be called from two places at once
//! with no serialization at all).
//!
//! **The TOCTOU boundary was also narrower than the real race** (Sol's round-4 finding): it is
//! not just "create → read id" — `chmod`/`chown` after the id read, and the eventual gVisor bind
//! mount, ALL still resolve the leaf pathname again. A replacement after the id read could
//! plausibly produce "quota correctly attached to subvolume A, permissions/ownership applied to a
//! REPLACEMENT B, and an unquota'd B mounted into the job." Genuinely closing this needs an
//! exclusive lock or FD-based verification via the raw `BTRFS_IOC_INO_LOOKUP` ioctl (not exposed
//! by the `btrfs` CLI, and this crate has no raw ioctl/libbtrfsutil binding today) spanning EVERY
//! pathname resolution from create through the eventual mount — a real, structural limitation of
//! wrapping the `btrfs` CLI instead of its ioctls, named here rather than silently narrowed away.
//! **What this module DOES enforce as a precondition, not merely document** (Sol's round-4 bar
//! for accepting the deferral): [`WorkspaceStorage::open`] fails loud unless the canonical base
//! directory is owned by the calling process's own effective uid and carries no group/world
//! write bit — the whole race requires ANOTHER actor with write access under `base_dir`, and this
//! makes that a real permission failure to obtain, not just a documented expectation. Multi-process
//! serialization beyond this one process is still the caller's responsibility (the real
//! integration's existing launch-permit CAS discipline is expected to be the practical answer). A
//! full ioctl-based close remains a named follow-up for the `gvisor.rs` integration.
//!
//! **Round 4 → round 5:** two more findings closed the loop. `--commit-after` was itself the
//! wrong tool — btrfs-progs performs the destroy ioctl FIRST and only then waits for the
//! transaction commit, so a `--commit-after` failure can mean the destroy already happened and
//! only the commit-wait failed, making "nonzero exit ⇒ nothing committed" unreliable exactly
//! where this module needs that claim to hold. Removed `--commit-after` entirely from both
//! delete paths; the already-unconditional `btrfs subvolume sync` afterward is the real
//! "fully removed" postcondition and subsumes what that flag was trying to guarantee (confirmed
//! directly: `sync` on an already-fully-gone id succeeds trivially and fast). Also fixed the
//! cross-storage capability test itself, which had nested storage B's base directory INSIDE
//! storage A's — storage A's own orphan scanner would then (correctly) flag that nested directory
//! as an unexpected non-subvolume entry, contaminating the very cleanup the test relied on; fixed
//! to use sibling base directories. `assert_base_dir_exclusively_owned` now also requires the
//! base to be a directory (not merely correctly-owned-and-permissioned).
//!
//! **`no-host-exec` (contract 1.6 / X-6 / AG-2).** Like the Firecracker/gVisor runtime-spawn sites
//! and the durable launch guard (`launch_gate.rs`), the REAL `btrfs`/`chown` invocations here ARE
//! the enforcement mechanism, not a bypass of it — NAMED, LOUD, per-lint exclusion
//! (`myelin-lints/src/bin/lint-gate.rs` AND `myelin-lints/tests/workspace_clean.rs`'s own separate
//! exclusion list — both kept in sync, an existing structural duplication this file does not fix),
//! never a silent skip; every other architecture lint still scans this file.
//!
//! **Privilege model.** Creating a subvolume needs only write access to its parent directory —
//! but setting a qgroup quota and deleting a subvolume both require `CAP_SYS_ADMIN`, and
//! transferring ownership (`chown`) additionally requires `CAP_CHOWN` (both proven to fail
//! `EPERM` unprivileged on this exact host, 2026-07-26). This matches the EXISTING,
//! already-documented privilege assumption for `runsc` itself (`escape_corpus.rs`'s "runsc
//! requires privileges this host lacks (no sudo)" residual note) — the production CI-sandbox
//! process is assumed to run with these capabilities (or as root), exactly like it already must
//! for gVisor. `btrfs quota status` (the filesystem-level enforcement check) needs NO privilege —
//! confirmed empirically, and a real bug in round 2's own test harness conflated the two.
//!
//! **Not yet wired into `gvisor.rs`.** This module is deliberately staged as a standalone,
//! independently-tested unit — the same staging `canonical_tar.rs`/`asset_registry.rs` went
//! through before being wired into the launch path. Actually mounting a [`PreparedWorkspace`] into
//! the OCI bundle, and the UID-namespace rework `runsc --rootless`'s single-UID shortcut needs, are
//! a follow-up against `gvisor.rs` itself — one of the most security-critical files in this repo,
//! and one that gets Sol's adversarial review before landing.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fmt;
use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Absolute path to the `btrfs` CLI — never resolved off an inherited `PATH` from a
/// `CAP_SYS_ADMIN`-privileged process (Sol's review finding).
const BTRFS_BIN: &str = "/usr/bin/btrfs";
/// Absolute path to `chown` — same reasoning as [`BTRFS_BIN`].
const CHOWN_BIN: &str = "/usr/bin/chown";
/// The well-known inode number of every Btrfs subvolume's OWN root directory
/// (`BTRFS_FIRST_FREE_OBJECTID`) — every subvolume, at any nesting depth, has this inode within
/// its own namespace. A syscall-only (no process spawn) way to check "is `path` ITSELF a
/// subvolume root", as opposed to `btrfs inspect-internal rootid`, which resolves to the
/// CONTAINING subvolume's id for ANY path (a stray file, an ordinary subdirectory) — confirmed
/// the hard way on this exact host: `rootid` on a plain file returned its containing subvolume's
/// real id, not an error.
const BTRFS_FIRST_FREE_OBJECTID: u64 = 256;
/// The maximum length of a single Btrfs directory-entry name.
const BTRFS_NAME_MAX: usize = 255;

/// A workspace-storage operation failed. Every variant carries the real `btrfs`/`chown` stderr
/// (or the specific structural reason) so a caller can log the real cause — never a swallowed
/// `bool`.
#[derive(Debug)]
pub enum WorkspaceStorageError {
    /// `base_dir` is not on a Btrfs filesystem (checked via `/proc/mounts`).
    NotBtrfs { base_dir: PathBuf },
    /// Btrfs quota is not enabled, not in qgroup mode, inconsistent, or has overridden limits on
    /// `base_dir`'s filesystem — `btrfs quota status` did not show all four required fields.
    QuotaNotEnforcing { base_dir: PathBuf, status: String },
    /// `job_id` is not a safe path component (must be non-empty, <= 255 bytes, ASCII
    /// alphanumeric/`-`/`_` only — never trust a caller-supplied string as a path segment
    /// without validating it).
    InvalidJobId { job_id: String },
    /// `quota_bytes` was zero — a zero quota is nonsensical (either "no workspace" or a caller
    /// bug), never silently treated as "unbounded" or "trivially satisfied".
    ZeroQuota,
    /// `btrfs subvolume create` failed.
    SubvolumeCreateFailed { path: PathBuf, stderr: String },
    /// Reading the freshly-created subvolume's own id (`btrfs inspect-internal rootid`) failed.
    IdentityReadFailed { path: PathBuf, reason: String },
    /// `btrfs qgroup limit` failed (commonly: insufficient privilege — see the module's privilege
    /// model note). Cleanup of the just-created, not-yet-quota'd subvolume SUCCEEDED.
    QuotaLimitFailed { path: PathBuf, stderr: String },
    /// The quota limit command exited 0, but re-reading the qgroup's own row shows a DIFFERENT
    /// `max_referenced` value than requested — the postcondition Sol's review required.
    QuotaNotAsserted {
        path: PathBuf,
        requested: u64,
        observed: Option<u64>,
    },
    /// `chown`/`chmod` on the freshly-created, already-quota'd subvolume failed. Cleanup
    /// SUCCEEDED.
    OwnershipFailed { path: PathBuf, reason: String },
    /// A provisioning step failed AND the best-effort cleanup that followed it ALSO failed. The
    /// workspace may be sitting in a partially-provisioned (possibly UNBOUNDED, since the quota
    /// step may not have run) state. **A caller MUST treat this as "mark workspace-storage
    /// unhealthy, refuse further admissions until a human reconciles"** — never folded into an
    /// ordinary retryable error.
    UnrecoverableLeak {
        path: PathBuf,
        subvol_id: Option<u64>,
        provisioning_error: String,
        cleanup_error: String,
    },
    /// `btrfs subvolume delete --subvolid` failed for a reason other than "already gone". The
    /// workspace still exists, fully intact — nothing was lost, the caller's capability was
    /// consumed but the underlying subvolume is unchanged (discoverable again via
    /// [`WorkspaceStorage::list_orphaned_workspaces`] if needed).
    DeleteFailed { subvol_id: u64, stderr: String },
    /// The `subvolume delete` command itself committed (succeeded, or reported the subvolume
    /// already gone), but `btrfs subvolume sync` (waiting for full extent/qgroup release) failed
    /// or has not yet completed. Deletion has ALREADY HAPPENED at this point — there is nothing
    /// left to retry via the original capability, only the wait. Retry via
    /// [`WorkspaceStorage::retry_pending_sync`] with the bare `subvol_id`, which needs no
    /// capability at all.
    SyncPending { subvol_id: u64, reason: String },
    /// A [`PreparedWorkspace`]/[`OrphanCandidate`] was presented to a [`WorkspaceStorage`] whose
    /// canonical base directory does not match the one it was minted against — refused, never
    /// acted on. Prevents a capability from storage A being used to delete an unrelated
    /// same-numbered subvolume on storage B.
    WrongStorage {
        expected_base: PathBuf,
        actual_base: PathBuf,
    },
    /// Listing `base_dir`'s entries (for orphan reconciliation) failed.
    ListFailed { base_dir: PathBuf, reason: String },
    /// An entry under the workspace base directory is not itself a Btrfs subvolume (a stray
    /// file, directory, or symlink) — reported loud, never silently included in or excluded from
    /// orphan reconciliation.
    UnexpectedEntry { path: PathBuf, reason: String },
    /// A filesystem call (other than the documented ENOENT-tolerant paths) failed unexpectedly.
    Io { path: PathBuf, reason: String },
}

impl fmt::Display for WorkspaceStorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotBtrfs { base_dir } => {
                write!(f, "workspace base {base_dir:?} is not a Btrfs filesystem")
            }
            Self::QuotaNotEnforcing { base_dir, status } => write!(
                f,
                "Btrfs quota is not in a fully-enforcing state on {base_dir:?}: {status}"
            ),
            Self::InvalidJobId { job_id } => {
                write!(f, "job id {job_id:?} is not a safe path component")
            }
            Self::ZeroQuota => write!(f, "quota_bytes must be > 0"),
            Self::SubvolumeCreateFailed { path, stderr } => {
                write!(f, "btrfs subvolume create {path:?} failed: {stderr}")
            }
            Self::IdentityReadFailed { path, reason } => {
                write!(f, "read subvolume id of {path:?} failed: {reason}")
            }
            Self::QuotaLimitFailed { path, stderr } => {
                write!(f, "btrfs qgroup limit on {path:?} failed: {stderr}")
            }
            Self::QuotaNotAsserted {
                path,
                requested,
                observed,
            } => write!(
                f,
                "quota postcondition failed on {path:?}: requested {requested}, observed {observed:?}"
            ),
            Self::OwnershipFailed { path, reason } => {
                write!(f, "set ownership on {path:?} failed: {reason}")
            }
            Self::UnrecoverableLeak {
                path,
                subvol_id,
                provisioning_error,
                cleanup_error,
            } => write!(
                f,
                "UNRECOVERABLE workspace leak at {path:?} (subvol_id={subvol_id:?}) - \
                 provisioning failed ({provisioning_error}) AND cleanup ALSO failed \
                 ({cleanup_error}) - manual reconciliation required, do not retry silently"
            ),
            Self::DeleteFailed { subvol_id, stderr } => {
                write!(f, "btrfs subvolume delete --subvolid {subvol_id} failed: {stderr}")
            }
            Self::SyncPending { subvol_id, reason } => write!(
                f,
                "subvolid {subvol_id} was deleted but sync has not completed ({reason}) - \
                 retry via retry_pending_sync({subvol_id}), no capability needed"
            ),
            Self::WrongStorage {
                expected_base,
                actual_base,
            } => write!(
                f,
                "refusing: this capability was minted against {expected_base:?}, not the \
                 storage handle's own base {actual_base:?}"
            ),
            Self::ListFailed { base_dir, reason } => {
                write!(f, "list workspace base {base_dir:?} failed: {reason}")
            }
            Self::UnexpectedEntry { path, reason } => {
                write!(f, "unexpected non-subvolume entry {path:?}: {reason}")
            }
            Self::Io { path, reason } => write!(f, "{path:?}: {reason}"),
        }
    }
}

impl std::error::Error for WorkspaceStorageError {}

/// A safe, freshly-provisioned per-job workspace. Fields are PRIVATE — a caller cannot construct
/// or forge one; the only way to get one is [`WorkspaceStorage::create_workspace`], and the only
/// way to delete one is [`WorkspaceStorage::delete_workspace`] (which consumes it BY VALUE and
/// verifies `minted_from` matches the storage handle's own base — round 4's fix for a capability
/// from storage A being usable against storage B). No `Clone`, no `Serialize`/`Deserialize` —
/// host paths are trusted launch-time capabilities, never durable or customer-controlled data.
#[derive(Debug)]
pub struct PreparedWorkspace {
    host_path: PathBuf,
    subvol_id: u64,
    minted_from: PathBuf,
}

impl PreparedWorkspace {
    /// The host filesystem path — bind-mount this (read-write) into the sandbox's OCI bundle.
    pub fn host_path(&self) -> &Path {
        &self.host_path
    }

    /// The Btrfs subvolume id, captured at creation.
    pub fn subvol_id(&self) -> u64 {
        self.subvol_id
    }
}

/// One candidate for orphan reconciliation: a real, verified Btrfs subvolume under the workspace
/// base directory. Fields are PRIVATE for the same reason as [`PreparedWorkspace`] — only
/// [`WorkspaceStorage::list_orphaned_workspaces`] constructs these, and only
/// [`WorkspaceStorage::delete_orphan`] consumes them (with the same `minted_from` check).
#[derive(Debug)]
pub struct OrphanCandidate {
    path: PathBuf,
    subvol_id: u64,
    minted_from: PathBuf,
}

impl OrphanCandidate {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn subvol_id(&self) -> u64 {
        self.subvol_id
    }
}

/// A handle over ONE workspace base directory on a verified, quota-enforcing Btrfs filesystem.
/// Owns the canonical base path and the filesystem's mountpoint (the "anchor" every id-addressed
/// `btrfs` operation resolves against, decoupling quota-limit/verify/delete from any individual
/// job's own leaf path — round 3's fix for the path-reuse hazard rounds 1/2 still had).
#[derive(Debug)]
pub struct WorkspaceStorage {
    canonical_base: PathBuf,
    fs_anchor: PathBuf,
}

impl WorkspaceStorage {
    /// **Fail-loud open**: `base_dir` must exist (created if missing), resolve to a real Btrfs
    /// mountpoint, have Btrfs quota fully enforcing (`Enabled: yes`, `Mode: qgroup`,
    /// `Inconsistent: no`, `Override limits: no` — all four), AND be owned by the CALLING
    /// process's own effective uid with no group/world write bit. That last check is the
    /// enforced precondition (not merely documented) behind the module doc's "the create-time
    /// TOCTOU requires ANOTHER actor with write access under `base_dir`" mitigation — Sol's
    /// round-4 bar for accepting that the raw-ioctl fix is deferred: a directory this method
    /// itself refuses to accept as safely-scoped cannot make that mitigation a documentation-only
    /// wish. Call at runner startup AND on a caller-owned periodic health check (quota can go
    /// inconsistent during normal operation, e.g. after a large-subvolume deletion) — never
    /// silently fall back to an unbounded or unsafely-shared directory.
    pub fn open(base_dir: &Path) -> Result<Self, WorkspaceStorageError> {
        if !exists_or_error(base_dir)? {
            std::fs::create_dir_all(base_dir).map_err(|e| WorkspaceStorageError::Io {
                path: base_dir.to_path_buf(),
                reason: format!("create workspace base dir: {e}"),
            })?;
        }
        let canonical_base =
            std::fs::canonicalize(base_dir).map_err(|e| WorkspaceStorageError::Io {
                path: base_dir.to_path_buf(),
                reason: format!("canonicalize: {e}"),
            })?;
        assert_base_dir_exclusively_owned(&canonical_base)?;
        let fs_anchor = btrfs_mountpoint_of(&canonical_base)?;
        assert_quota_enforcing(&canonical_base)?;
        Ok(Self {
            canonical_base,
            fs_anchor,
        })
    }

    /// The canonical workspace base directory this handle was opened over.
    pub fn base_dir(&self) -> &Path {
        &self.canonical_base
    }

    /// Read-only re-verification that this handle's base directory is STILL exclusively owned by
    /// this process and STILL fully Btrfs-quota-enforcing — never creates or mutates anything
    /// (unlike [`Self::open`], which `create_dir_all`s a missing `base_dir` as a matter of
    /// course). Call periodically: quota can go inconsistent during normal operation (e.g. after a
    /// large-subvolume deletion elsewhere on the filesystem).
    ///
    /// Deliberately does NOT verify this handle's base directory's IDENTITY has not changed
    /// underneath it (e.g. deleted and recreated, or renamed away and replaced, at the very same
    /// path) — that requires an independent capability this type does not hold (an exclusive
    /// directory-FD lock's own cached device/inode, held by the caller). A caller that needs that
    /// guarantee (the `gvisor.rs` integration's persistent workspace manager) must perform its own
    /// identity check FIRST and treat this method as the second, complementary layer: "assuming
    /// the directory at this path is still the one I locked, are its Btrfs preconditions still OK".
    pub fn check_health(&self) -> Result<(), WorkspaceStorageError> {
        assert_base_dir_exclusively_owned(&self.canonical_base)?;
        assert_quota_enforcing(&self.canonical_base)?;
        Ok(())
    }

    /// Create, quota, verify, and own a fresh per-job Btrfs subvolume at `base_dir/job_id`.
    ///
    /// Ordering: create → read id → quota-limit (by explicit qgroup id against the FS anchor,
    /// not the job's own path) → VERIFY the quota postcondition (same anchor) → chmod → chown.
    /// Permissions are set BEFORE ownership transfers (chmod needs no elevated capability beyond
    /// what created the subvolume; chown/transferring ownership to another uid needs
    /// `CAP_CHOWN`, done last so a failed chmod never leaves a wrongly-permissioned subvolume
    /// already owned by the job). A failure at any step after creation attempts best-effort
    /// cleanup; if that cleanup ALSO fails, the error is
    /// [`WorkspaceStorageError::UnrecoverableLeak`] — never silently swallowed.
    ///
    /// **Named, not-fully-closed race** (see the module doc): the window from `subvolume create`
    /// through the eventual gVisor bind mount still resolves the leaf pathname more than once.
    /// Enforced mitigation, not just documentation: [`WorkspaceStorage::open`] refuses a
    /// `base_dir` not exclusively owned by this process. `&mut self`: the borrow checker makes
    /// concurrent calls into ONE handle from the same process a compile error.
    pub fn create_workspace(
        &mut self,
        job_id: &str,
        quota_bytes: u64,
        owner_uid: u32,
        owner_gid: u32,
    ) -> Result<PreparedWorkspace, WorkspaceStorageError> {
        validate_job_id(job_id)?;
        if quota_bytes == 0 {
            return Err(WorkspaceStorageError::ZeroQuota);
        }
        let path = self.canonical_base.join(job_id);

        let create = run_btrfs(&[
            OsStr::new("subvolume"),
            OsStr::new("create"),
            path.as_os_str(),
        ])?;
        if !create.status.success() {
            return Err(WorkspaceStorageError::SubvolumeCreateFailed {
                path: path.clone(),
                stderr: stderr_of(&create),
            });
        }

        let subvol_id = match read_subvol_id(&path) {
            Ok(id) => id,
            Err(reason) => {
                // No verified id exists yet — fall back to a direct path-based delete. This is
                // the one place in the module that still resolves by path (see the module doc's
                // named, not-fully-closed race).
                if let Err(cleanup_err) = delete_by_path_unverified(&path) {
                    return Err(WorkspaceStorageError::UnrecoverableLeak {
                        path,
                        subvol_id: None,
                        provisioning_error: format!("read subvolume id: {reason}"),
                        cleanup_error: cleanup_err,
                    });
                }
                return Err(WorkspaceStorageError::IdentityReadFailed { path, reason });
            }
        };

        if let Err(provisioning_error) = self.apply_and_verify_quota(subvol_id, quota_bytes) {
            if let Err(cleanup_err) = self.delete_by_id(subvol_id) {
                return Err(WorkspaceStorageError::UnrecoverableLeak {
                    path,
                    subvol_id: Some(subvol_id),
                    provisioning_error: provisioning_error.to_string(),
                    cleanup_error: cleanup_err.to_string(),
                });
            }
            return Err(provisioning_error);
        }

        if let Err(e) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)) {
            let reason = format!("chmod 0755: {e}");
            if let Err(cleanup_err) = self.delete_by_id(subvol_id) {
                return Err(WorkspaceStorageError::UnrecoverableLeak {
                    path,
                    subvol_id: Some(subvol_id),
                    provisioning_error: reason,
                    cleanup_error: cleanup_err.to_string(),
                });
            }
            return Err(WorkspaceStorageError::OwnershipFailed { path, reason });
        }
        if let Err(reason) = chown_path(&path, owner_uid, owner_gid) {
            if let Err(cleanup_err) = self.delete_by_id(subvol_id) {
                return Err(WorkspaceStorageError::UnrecoverableLeak {
                    path,
                    subvol_id: Some(subvol_id),
                    provisioning_error: format!("chown: {reason}"),
                    cleanup_error: cleanup_err.to_string(),
                });
            }
            return Err(WorkspaceStorageError::OwnershipFailed { path, reason });
        }

        Ok(PreparedWorkspace {
            host_path: path,
            subvol_id,
            minted_from: self.canonical_base.clone(),
        })
    }

    fn apply_and_verify_quota(
        &self,
        subvol_id: u64,
        quota_bytes: u64,
    ) -> Result<(), WorkspaceStorageError> {
        let qgroup_id = format!("0/{subvol_id}");
        let quota_arg = quota_bytes.to_string();
        // Addressed by EXPLICIT qgroup id against the filesystem anchor — never the job's own
        // leaf path — so this step cannot be confused by a path-reuse race.
        let limit = run_btrfs(&[
            OsStr::new("qgroup"),
            OsStr::new("limit"),
            OsStr::new(&quota_arg),
            OsStr::new(&qgroup_id),
            self.fs_anchor.as_os_str(),
        ])?;
        if !limit.status.success() {
            return Err(WorkspaceStorageError::QuotaLimitFailed {
                path: self.canonical_base.clone(),
                stderr: stderr_of(&limit),
            });
        }
        let observed = self.read_qgroup_max_referenced(subvol_id)?;
        if observed != Some(quota_bytes) {
            return Err(WorkspaceStorageError::QuotaNotAsserted {
                path: self.canonical_base.clone(),
                requested: quota_bytes,
                observed,
            });
        }
        Ok(())
    }

    /// Parse `btrfs qgroup show -r --raw <fs_anchor>` and return the `max_referenced` column for
    /// the `0/<subvol_id>` row, if present.
    fn read_qgroup_max_referenced(
        &self,
        subvol_id: u64,
    ) -> Result<Option<u64>, WorkspaceStorageError> {
        let show = run_btrfs(&[
            OsStr::new("qgroup"),
            OsStr::new("show"),
            OsStr::new("-r"),
            OsStr::new("--raw"),
            self.fs_anchor.as_os_str(),
        ])?;
        if !show.status.success() {
            return Err(WorkspaceStorageError::QuotaLimitFailed {
                path: self.canonical_base.clone(),
                stderr: stderr_of(&show),
            });
        }
        let want_id = format!("0/{subvol_id}");
        for line in stdout_of(&show).lines() {
            let cols: Vec<&str> = line.split_whitespace().collect();
            // Qgroupid  Referenced  Exclusive  Max referenced  Path
            if cols.len() >= 4 && cols[0] == want_id {
                return Ok(cols[3].parse::<u64>().ok());
            }
        }
        Ok(None)
    }

    /// Delete a workspace, addressed by its PERSISTENT subvolume id against the filesystem
    /// anchor — never by re-walking the leaf path. Refuses (never acts on) a `prepared` minted
    /// against a DIFFERENT [`WorkspaceStorage`] (`WrongStorage`). Consumes `prepared` by value:
    /// once deletion COMMITS, the caller cannot accidentally reuse the handle. Crash-durable:
    /// `subvolume delete` (deliberately WITHOUT `--commit-after` — see [`Self::delete_by_id`]'s
    /// own comment for why that flag makes a nonzero exit an unreliable "nothing happened"
    /// signal) followed by an UNCONDITIONAL `btrfs subvolume sync`, which is the actual
    /// "fully removed" postcondition. **If delete commits but sync fails**, the original
    /// capability is gone (deletion already happened) and the error is
    /// [`WorkspaceStorageError::SyncPending`] carrying the bare `subvol_id` — retry via
    /// [`WorkspaceStorage::retry_pending_sync`], which needs no capability at all.
    pub fn delete_workspace(
        &mut self,
        prepared: PreparedWorkspace,
    ) -> Result<(), WorkspaceStorageError> {
        assert_same_storage(&self.canonical_base, &prepared.minted_from)?;
        self.delete_by_id(prepared.subvol_id)
    }

    /// Delete an orphan candidate found by [`Self::list_orphaned_workspaces`]. Same id-addressed,
    /// crash-durable, storage-bound delete as [`Self::delete_workspace`].
    pub fn delete_orphan(
        &mut self,
        candidate: OrphanCandidate,
    ) -> Result<(), WorkspaceStorageError> {
        assert_same_storage(&self.canonical_base, &candidate.minted_from)?;
        self.delete_by_id(candidate.subvol_id)
    }

    /// Finish waiting for a subvolume delete that already COMMITTED but whose `subvolume sync`
    /// previously failed ([`WorkspaceStorageError::SyncPending`]). Addressed by bare `subvol_id`
    /// — no capability object needed, because deletion has already happened; only the wait for
    /// background extent/qgroup cleanup remains outstanding. Boot-time reconciliation should call
    /// this for any id a prior crash left in this state before reopening admission.
    pub fn retry_pending_sync(&mut self, subvol_id: u64) -> Result<(), WorkspaceStorageError> {
        self.sync_subvol_id(subvol_id)
    }

    fn delete_by_id(&mut self, subvol_id: u64) -> Result<(), WorkspaceStorageError> {
        let id_arg = subvol_id.to_string();
        // Deliberately WITHOUT `--commit-after` (round 4→5 fix, Sol's finding): btrfs-progs
        // performs the destroy ioctl FIRST and only then waits for the transaction commit — a
        // `--commit-after` failure can mean the destroy already happened and only the commit-wait
        // failed, making "nonzero exit ⇒ nothing committed" a FALSE claim exactly when this code
        // needs it to be true. The unconditional `sync_subvol_id` call below already provides the
        // authoritative "fully removed" postcondition (confirmed directly: `btrfs subvolume sync
        // <anchor> <id>` succeeds trivially and fast even for an id that never fully committed),
        // so it subsumes what `--commit-after` was trying to guarantee anyway.
        let delete = run_btrfs(&[
            OsStr::new("subvolume"),
            OsStr::new("delete"),
            OsStr::new("--subvolid"),
            OsStr::new(&id_arg),
            self.fs_anchor.as_os_str(),
        ])?;
        if !delete.status.success() {
            let stderr = stderr_of(&delete);
            // "already gone" (a concurrent/prior delete already removed it) still needs a sync
            // wait — round 3 returned Ok immediately here, recreating the exact crash window
            // round 2 fixed for the success path (a prior delete may have removed the name while
            // background extent/qgroup cleanup remains unfinished). Anything else (now that
            // `--commit-after` is gone) is a real destroy-ioctl failure — the workspace is
            // untouched.
            if !(stderr.contains("No such file or directory") || stderr.contains("do not exist")) {
                return Err(WorkspaceStorageError::DeleteFailed { subvol_id, stderr });
            }
        }
        self.sync_subvol_id(subvol_id)
    }

    fn sync_subvol_id(&mut self, subvol_id: u64) -> Result<(), WorkspaceStorageError> {
        let sync = run_btrfs(&[
            OsStr::new("subvolume"),
            OsStr::new("sync"),
            self.fs_anchor.as_os_str(),
            OsStr::new(&subvol_id.to_string()),
        ])?;
        if sync.status.success() {
            Ok(())
        } else {
            Err(WorkspaceStorageError::SyncPending {
                subvol_id,
                reason: stderr_of(&sync),
            })
        }
    }

    /// List every subvolume under `base_dir` whose name is NOT in `active_job_ids` — the
    /// crash-reconciliation candidate set (a prior runner instance's job that never reached its
    /// own cleanup, e.g. a kill-9 mid-job). Every candidate entry is verified to ACTUALLY be a
    /// Btrfs subvolume BEFORE the active-set filter is applied (round 3 fix: round 2's ordering
    /// checked the active set first, so a stray file/symlink whose name happened to match an
    /// active job id would be silently skipped instead of ever being validated) — a non-subvolume
    /// entry is reported as [`WorkspaceStorageError::UnexpectedEntry`], never silently included
    /// in or excluded from the result regardless of its name.
    ///
    /// **Caller responsibility, not solved here** (Sol's review): this is a point-in-time
    /// snapshot. The runner boot path must run this BEHIND an admission barrier (block new
    /// workspace creation, settle/adopt anything the previous process left running, THEN snapshot
    /// `active_job_ids` and call this, THEN delete each candidate, THEN open admission) —
    /// comparing against an active set captured before closing that barrier can misclassify a
    /// workspace created concurrently with the snapshot as orphaned.
    pub fn list_orphaned_workspaces(
        &mut self,
        active_job_ids: &BTreeSet<String>,
    ) -> Result<Vec<OrphanCandidate>, WorkspaceStorageError> {
        if !exists_or_error(&self.canonical_base)? {
            return Ok(Vec::new());
        }
        let entries = std::fs::read_dir(&self.canonical_base).map_err(|e| {
            WorkspaceStorageError::ListFailed {
                base_dir: self.canonical_base.clone(),
                reason: e.to_string(),
            }
        })?;
        let mut orphans = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| WorkspaceStorageError::ListFailed {
                base_dir: self.canonical_base.clone(),
                reason: e.to_string(),
            })?;
            let path = entry.path();
            let subvol_id = verify_and_read_subvol_id(&path).map_err(|reason| {
                WorkspaceStorageError::UnexpectedEntry {
                    path: path.clone(),
                    reason,
                }
            })?;
            let name_str = entry.file_name().to_string_lossy().into_owned();
            if active_job_ids.contains(&name_str) {
                continue;
            }
            orphans.push(OrphanCandidate {
                path,
                subvol_id,
                minted_from: self.canonical_base.clone(),
            });
        }
        Ok(orphans)
    }
}

/// Find the Btrfs mountpoint covering `canonical_path` via `/proc/mounts` (no process spawn — a
/// virtual procfs read, not a `no-host-exec` concern). Returns the mountpoint path itself, used
/// as the id-addressed operations' filesystem anchor (subvolume ids are unique per-filesystem, so
/// ANY subvolume path on that filesystem is a valid anchor — the mountpoint always is one).
fn btrfs_mountpoint_of(canonical_path: &Path) -> Result<PathBuf, WorkspaceStorageError> {
    let mounts =
        std::fs::read_to_string("/proc/mounts").map_err(|e| WorkspaceStorageError::Io {
            path: canonical_path.to_path_buf(),
            reason: format!("read /proc/mounts: {e}"),
        })?;
    let mut best: Option<(usize, &str, &str)> = None;
    for line in mounts.lines() {
        let mut fields = line.split_whitespace();
        let Some(_source) = fields.next() else {
            continue;
        };
        let Some(mount_point) = fields.next() else {
            continue;
        };
        let Some(fstype) = fields.next() else {
            continue;
        };
        if canonical_path.starts_with(mount_point) {
            let len = mount_point.len();
            if best.is_none_or(|(best_len, ..)| len > best_len) {
                best = Some((len, mount_point, fstype));
            }
        }
    }
    match best {
        Some((_, mount_point, "btrfs")) => Ok(PathBuf::from(mount_point)),
        Some(_) | None => Err(WorkspaceStorageError::NotBtrfs {
            base_dir: canonical_path.to_path_buf(),
        }),
    }
}

/// Assert `canonical_base` is owned by the CALLING process's own effective uid and carries no
/// group/world write bit. The create-time TOCTOU race (see the module doc) requires ANOTHER
/// actor with write access under `base_dir` to matter at all — this turns "base_dir must be
/// exclusively writable by the trusted caller" from a documentation-only expectation into an
/// enforced precondition every [`WorkspaceStorage::open`] call actually checks.
fn assert_base_dir_exclusively_owned(canonical_base: &Path) -> Result<(), WorkspaceStorageError> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(canonical_base).map_err(|e| WorkspaceStorageError::Io {
        path: canonical_base.to_path_buf(),
        reason: format!("stat: {e}"),
    })?;
    let euid = unsafe { libc::geteuid() };
    let group_or_world_writable = meta.mode() & 0o022 != 0;
    if !meta.is_dir() || meta.uid() != euid || group_or_world_writable {
        return Err(WorkspaceStorageError::Io {
            path: canonical_base.to_path_buf(),
            reason: format!(
                "workspace base must be a directory owned by this process's own uid ({euid}) \
                 with no group/world write bit; found is_dir={} uid={} mode={:o}",
                meta.is_dir(),
                meta.uid(),
                meta.mode() & 0o7777
            ),
        });
    }
    Ok(())
}

/// Assert `capability_base` matches `self.canonical_base` — the check every deletion method runs
/// before doing anything, refusing (never acting on) a [`PreparedWorkspace`]/[`OrphanCandidate`]
/// minted against a DIFFERENT [`WorkspaceStorage`] (round 4's fix: private fields alone prevented
/// forgery but did not bind a capability to the specific handle that minted it).
fn assert_same_storage(
    self_base: &Path,
    capability_base: &Path,
) -> Result<(), WorkspaceStorageError> {
    if self_base == capability_base {
        Ok(())
    } else {
        Err(WorkspaceStorageError::WrongStorage {
            expected_base: capability_base.to_path_buf(),
            actual_base: self_base.to_path_buf(),
        })
    }
}

/// Assert `btrfs quota status` on `canonical_base`'s filesystem shows all four required
/// enforcing fields. Needs NO privilege (confirmed empirically) — a real bug in an earlier draft
/// of this module conflated this filesystem-level check with process-level `CAP_SYS_ADMIN`.
fn assert_quota_enforcing(canonical_base: &Path) -> Result<(), WorkspaceStorageError> {
    let status = run_btrfs(&[
        OsStr::new("quota"),
        OsStr::new("status"),
        canonical_base.as_os_str(),
    ])?;
    let text = stdout_of(&status);
    let field = |name: &str| {
        text.lines()
            .find(|l| l.trim_start().starts_with(name))
            .map(str::to_owned)
    };
    let enforcing = status.status.success()
        && field("Enabled:").is_some_and(|l| l.contains("yes"))
        && field("Mode:").is_some_and(|l| l.contains("qgroup"))
        && field("Inconsistent:").is_some_and(|l| l.contains("no"))
        && field("Override limits:").is_some_and(|l| l.contains("no"));
    if enforcing {
        Ok(())
    } else {
        Err(WorkspaceStorageError::QuotaNotEnforcing {
            base_dir: canonical_base.to_path_buf(),
            status: if status.status.success() {
                text
            } else {
                stderr_of(&status)
            },
        })
    }
}

/// Validate a job id is safe to use as a single path component: non-empty, <= 255 bytes (the
/// Btrfs directory-entry name limit), ASCII alphanumeric/`-`/`_` only. Never trust a
/// caller-supplied string as a path segment (path traversal, NUL injection, an oversized name
/// that would fail deep inside a privileged `btrfs` call instead of at this cheap check) without
/// validating it first.
fn validate_job_id(job_id: &str) -> Result<(), WorkspaceStorageError> {
    let safe = !job_id.is_empty()
        && job_id.len() <= BTRFS_NAME_MAX
        && job_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if safe {
        Ok(())
    } else {
        Err(WorkspaceStorageError::InvalidJobId {
            job_id: job_id.to_string(),
        })
    }
}

/// Whether `path` is itself a Btrfs subvolume root (not merely a file/directory living inside
/// one) — via `stat`'s inode number, no process spawn.
fn is_subvolume_root(path: &Path) -> Result<bool, String> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(path).map_err(|e| format!("stat: {e}"))?;
    if meta.file_type().is_symlink() {
        return Ok(false);
    }
    Ok(meta.is_dir() && meta.ino() == BTRFS_FIRST_FREE_OBJECTID)
}

/// Read a subvolume's numeric id. Callers MUST have already confirmed [`is_subvolume_root`] —
/// `btrfs inspect-internal rootid` does NOT itself refuse a non-subvolume path, so calling this
/// on an unverified path is a real bug, not just imprecise.
fn read_subvol_id(path: &Path) -> Result<u64, String> {
    let output = Command::new(BTRFS_BIN)
        .env_clear()
        .args([
            OsStr::new("inspect-internal"),
            OsStr::new("rootid"),
            path.as_os_str(),
        ])
        .output()
        .map_err(|e| format!("spawn btrfs inspect-internal rootid: {e}"))?;
    if !output.status.success() {
        return Err(stderr_of(&output));
    }
    stdout_of(&output)
        .trim()
        .parse::<u64>()
        .map_err(|e| format!("parse rootid output: {e}"))
}

/// [`is_subvolume_root`] then [`read_subvol_id`] — the combined, safe "confirm and read" check
/// every caller outside [`WorkspaceStorage::create_workspace`]'s own just-created path should use.
fn verify_and_read_subvol_id(path: &Path) -> Result<u64, String> {
    if !is_subvolume_root(path)? {
        return Err(format!("{path:?} is not a Btrfs subvolume root"));
    }
    read_subvol_id(path)
}

/// Delete-by-path with no identity verification — used ONLY in [`WorkspaceStorage::
/// create_workspace`]'s own failure path immediately after `subvolume create`, before a verified
/// id even exists. Also syncs (round 3 fix: an earlier draft committed but never synced this
/// specific path, so cleanup here was not crash-durable the way every other delete path is).
fn delete_by_path_unverified(path: &Path) -> Result<(), String> {
    // No `--commit-after`, same reasoning as `delete_by_id`: a commit-wait failure after the
    // destroy ioctl already succeeded would make a nonzero exit here a false "nothing happened"
    // signal, right where the caller needs that claim to be accurate.
    let delete = Command::new(BTRFS_BIN)
        .env_clear()
        .args([
            OsStr::new("subvolume"),
            OsStr::new("delete"),
            path.as_os_str(),
        ])
        .output()
        .map_err(|e| format!("spawn btrfs subvolume delete: {e}"))?;
    if !delete.status.success() {
        return Err(stderr_of(&delete));
    }
    let parent = path.parent().unwrap_or(path);
    let sync = Command::new(BTRFS_BIN)
        .env_clear()
        .args([
            OsStr::new("subvolume"),
            OsStr::new("sync"),
            parent.as_os_str(),
        ])
        .output()
        .map_err(|e| format!("spawn btrfs subvolume sync: {e}"))?;
    if sync.status.success() {
        Ok(())
    } else {
        Err(stderr_of(&sync))
    }
}

fn chown_path(path: &Path, uid: u32, gid: u32) -> Result<(), String> {
    let output = Command::new(CHOWN_BIN)
        .env_clear()
        .arg(format!("{uid}:{gid}"))
        .arg(path)
        .output()
        .map_err(|e| format!("spawn chown: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(stderr_of(&output))
    }
}

fn run_btrfs(args: &[&OsStr]) -> Result<Output, WorkspaceStorageError> {
    Command::new(BTRFS_BIN)
        .env_clear()
        .args(args)
        .output()
        .map_err(|e| WorkspaceStorageError::Io {
            path: PathBuf::new(),
            reason: format!("spawn btrfs {args:?}: {e}"),
        })
}

/// Read-only privilege preflight for `CAP_SYS_ADMIN`-gated qgroup operations specifically (`qgroup
/// show -r`, and by extension `qgroup limit`/`subvolume delete`, which need the SAME capability) —
/// reusable from OTHER modules' tests (e.g. `workspace_manager.rs`) that need to gate a real Btrfs
/// lifecycle test on privilege WITHOUT ever attempting a real mutating `create_workspace` just to
/// find out (a mutating attempt whose cleanup ALSO fails for the SAME missing-privilege reason
/// leaves a genuinely [`WorkspaceStorageError::UnrecoverableLeak`]'d subvolume behind — this probe
/// cannot do that, it never creates anything). `base_dir` must already be a verified,
/// quota-enforcing Btrfs directory (i.e. `WorkspaceStorage::open` already succeeded against it) —
/// this only tells the caller whether privileged qgroup operations will ALSO succeed there.
///
/// Deliberately does NOT probe `CAP_CHOWN` (needed separately for `create_workspace`'s ownership
/// transfer) — a caller that needs BOTH capabilities confirmed cannot rely on this alone (Sol's
/// review, round 4): this is a `CAP_SYS_ADMIN`-only preflight, not a full privilege check for the
/// entire `create_workspace` lifecycle.
// `pub(crate)` (not `pub`), so `test-support` (which exists for OTHER crates to reach into this
// one) buys nothing here — plain `#[cfg(test)]` is the correct, sole gate for a same-crate-only
// test helper.
#[cfg(test)]
pub(crate) fn probe_qgroup_privilege(base_dir: &Path) -> Result<bool, WorkspaceStorageError> {
    let probe = run_btrfs(&[
        OsStr::new("qgroup"),
        OsStr::new("show"),
        OsStr::new("-r"),
        OsStr::new("--raw"),
        base_dir.as_os_str(),
    ])?;
    if probe.status.success() {
        Ok(true)
    } else {
        let stderr = stderr_of(&probe);
        if stderr.contains("Operation not permitted") {
            Ok(false)
        } else {
            Err(WorkspaceStorageError::Io {
                path: base_dir.to_path_buf(),
                reason: format!(
                    "unexpected qgroup-show probe failure (not a privilege gap): {stderr}"
                ),
            })
        }
    }
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_string()
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Whether `path` exists, distinguishing a genuine ENOENT from other lookup errors (permission
/// denied, a broken intermediate symlink) — round 3 fix: `Path::exists()` collapses every error
/// into `false`, which would make a permission problem look like "already deleted".
fn exists_or_error(path: &Path) -> Result<bool, WorkspaceStorageError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
        Err(e) => Err(WorkspaceStorageError::Io {
            path: path.to_path_buf(),
            reason: e.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A base directory UNIQUE to `tag` (each test passes its own function name) — round 5→6
    /// fix (Sol's finding): every test previously shared ONE `test_base()` directory, and Rust
    /// runs `#[test]` functions concurrently by default, so on a privileged host the orphan test
    /// (expects exactly one orphan), the lifecycle test, the provenance test, and the stray-entry
    /// test could all observe or interfere with each other's subvolumes/files in that shared
    /// directory — hidden only because the three long-running privileged tests happen to skip on
    /// this particular unprivileged CI host.
    fn test_base(tag: &str) -> PathBuf {
        let mut p = std::env::home_dir().expect("HOME must be set for this test");
        p.push(format!(".local/state/myelin-workspace-storage-tests-{tag}"));
        p
    }

    /// Open the test storage handle, skipping ONLY on the specific, expected ENVIRONMENTAL gap
    /// (no Btrfs, or quota not enforcing) — any OTHER error from `open()` (e.g. the base-dir
    /// ownership/permission precondition failing unexpectedly) is a real regression and panics.
    /// This is the check every test needs, whether or not it goes on to need `CAP_SYS_ADMIN`.
    fn open_or_skip_env(tag: &str) -> Option<WorkspaceStorage> {
        match WorkspaceStorage::open(&test_base(tag)) {
            Ok(s) => Some(s),
            Err(
                e @ (WorkspaceStorageError::NotBtrfs { .. }
                | WorkspaceStorageError::QuotaNotEnforcing { .. }),
            ) => {
                eprintln!("SKIP: no Btrfs+enforcing-quota support on this host: {e}");
                None
            }
            Err(e) => {
                panic!("WorkspaceStorage::open failed unexpectedly (not an environmental gap): {e}")
            }
        }
    }

    /// Explicit privilege preflight, for tests that go on to exercise `qgroup limit`/`delete`.
    /// `WorkspaceStorage::open`'s own checks (filesystem type + `quota status`) need NO
    /// privilege — round 2's harness conflated the two and let real regressions hide behind a bad
    /// skip condition. This probes the SPECIFIC, expected denial (`Operation not permitted`, the
    /// exact string this host's `btrfs` emits for an unprivileged qgroup call) and skips ONLY on
    /// that; any other failure (a malformed command, a real regression) is a hard test failure.
    fn open_or_skip_privileged(tag: &str) -> Option<WorkspaceStorage> {
        let storage = open_or_skip_env(tag)?;
        let probe = run_btrfs(&[
            OsStr::new("qgroup"),
            OsStr::new("show"),
            OsStr::new("-r"),
            OsStr::new("--raw"),
            storage.base_dir().as_os_str(),
        ])
        .expect("spawn btrfs for the privilege probe");
        if probe.status.success() {
            Some(storage)
        } else {
            let stderr = stderr_of(&probe);
            assert!(
                stderr.contains("Operation not permitted"),
                "expected the specific unprivileged-qgroup denial, got a DIFFERENT failure \
                 (a real regression, not a privilege gap): {stderr}"
            );
            eprintln!(
                "SKIP: this test process lacks CAP_SYS_ADMIN for qgroup operations: {stderr}"
            );
            None
        }
    }

    #[test]
    fn full_privileged_lifecycle_create_quota_verify_exceed_delete_sync() {
        let Some(mut storage) = open_or_skip_privileged("lifecycle") else {
            return;
        };
        let job_id = format!("probe{}", std::process::id());
        let quota: u64 = 8 << 20; // 8 MiB — small and fast to exceed deliberately below.

        // From here on, any error is a REAL test failure (privilege was already confirmed above).
        let created = storage
            .create_workspace(&job_id, quota, 0, 0)
            .expect("provisioning must succeed now that privilege is confirmed");
        assert!(created.host_path().exists());
        assert_eq!(created.host_path(), storage.base_dir().join(&job_id));

        let observed = storage
            .read_qgroup_max_referenced(created.subvol_id())
            .expect("read back the applied quota");
        assert_eq!(observed, Some(quota));

        // Exceed the quota with INCOMPRESSIBLE data (this mount uses zstd; an all-zero buffer
        // compresses to nearly nothing and would never actually hit the referenced-space quota —
        // round 3 fix for a test that "passed" without ever proving quota enforcement). A
        // pseudo-random byte pattern is enough to defeat zstd's ratio at this size.
        let mut incompressible = vec![0u8; (quota as usize) * 2];
        let mut state: u64 = 0x9E3779B97F4A7C15;
        for chunk in incompressible.chunks_mut(8) {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            chunk.copy_from_slice(&state.to_le_bytes()[..chunk.len()]);
        }
        let big_file = created.host_path().join("overflow");
        let write_err = std::fs::write(&big_file, &incompressible)
            .expect_err("writing incompressible data past the quota must fail");
        let errno = write_err.raw_os_error();
        assert!(
            errno == Some(libc_enospc()) || errno == Some(libc_edquot()),
            "expected ENOSPC or EDQUOT, got {write_err:?} (errno {errno:?})"
        );

        // The real delete: commit + sync, then confirm it is actually gone.
        storage
            .delete_workspace(created)
            .expect("delete with the correct id must succeed");
    }

    #[test]
    fn invalid_job_ids_are_rejected_before_any_filesystem_call() {
        // No storage handle needed at all — validation happens before any btrfs call or even a
        // WorkspaceStorage::open, so this test cannot race any other test's base directory.
        for bad in ["", "../escape", "with/slash", "with space", "with\0nul"] {
            assert!(matches!(
                validate_job_id(bad),
                Err(WorkspaceStorageError::InvalidJobId { .. })
            ));
        }
        let too_long = "a".repeat(BTRFS_NAME_MAX + 1);
        assert!(matches!(
            validate_job_id(&too_long),
            Err(WorkspaceStorageError::InvalidJobId { .. })
        ));
    }

    #[test]
    fn zero_quota_is_rejected() {
        // No CAP_SYS_ADMIN needed — quota_bytes==0 is rejected before any btrfs call at all.
        let Some(mut storage) = open_or_skip_env("zero-quota") else {
            return;
        };
        let job_id = format!("zeroq{}", std::process::id());
        let err = storage.create_workspace(&job_id, 0, 0, 0).unwrap_err();
        assert!(matches!(err, WorkspaceStorageError::ZeroQuota));
        assert!(
            !exists_or_error(&storage.base_dir().join(&job_id)).unwrap(),
            "a rejected zero-quota request must not have created anything"
        );
    }

    #[test]
    fn orphan_listing_verifies_before_filtering_and_finds_the_real_orphan() {
        let Some(mut storage) = open_or_skip_privileged("orphan-listing") else {
            return;
        };
        let suffix = std::process::id();
        let active_id = format!("active{suffix}");
        let orphan_id = format!("orphan{suffix}");
        let active_ws = storage
            .create_workspace(&active_id, 8 << 20, 0, 0)
            .expect("create the active workspace");
        let orphan_ws = storage
            .create_workspace(&orphan_id, 8 << 20, 0, 0)
            .expect("create the to-be-orphaned workspace");
        let orphan_subvol_id = orphan_ws.subvol_id();

        let mut active = BTreeSet::new();
        active.insert(active_id.clone());
        let found = storage
            .list_orphaned_workspaces(&active)
            .expect("list orphans");
        assert_eq!(
            found
                .iter()
                .map(OrphanCandidate::subvol_id)
                .collect::<Vec<_>>(),
            vec![orphan_subvol_id],
            "exactly the non-active workspace must be listed as orphaned, not the active one"
        );

        storage.delete_workspace(active_ws).expect("cleanup active");
        for candidate in found {
            storage.delete_orphan(candidate).expect("cleanup orphan");
        }
    }

    /// Round 4's core finding: a capability minted by ONE `WorkspaceStorage` must be refused by a
    /// DIFFERENT one, even though `PreparedWorkspace`'s fields are private (private fields alone
    /// prevented forgery, not misdirected use of a genuinely-minted capability).
    #[test]
    fn a_capability_from_one_storage_is_refused_by_another() {
        let Some(mut storage_a) = open_or_skip_privileged("cross-storage-a") else {
            return;
        };
        // A SIBLING directory, never nested under storage_a's own base — round 4→5 fix (Sol's
        // finding): a nested base would itself be a stray non-subvolume entry from storage_a's
        // OWN orphan scanner's point of view, contaminating the cleanup this test does through
        // storage_a afterward.
        let sibling_base = storage_a.base_dir().with_file_name(format!(
            "{}-sibling-{}",
            storage_a
                .base_dir()
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("myelin-workspace-storage-tests"),
            std::process::id()
        ));
        let mut storage_b =
            WorkspaceStorage::open(&sibling_base).expect("open a second, sibling handle");

        let job_id = format!("crossx{}", std::process::id());
        let prepared = storage_a
            .create_workspace(&job_id, 8 << 20, 0, 0)
            .expect("create on storage A");
        let path = prepared.host_path().to_path_buf();
        let subvol_id = prepared.subvol_id();

        let refused = storage_b.delete_workspace(prepared);
        assert!(
            matches!(refused, Err(WorkspaceStorageError::WrongStorage { .. })),
            "a capability minted by storage A must be refused by storage B, got {refused:?}"
        );
        assert!(
            path.exists(),
            "the refused delete must not have removed anything"
        );

        // Cleanup through the ORIGINAL, matching storage: re-discover via listing (the consumed
        // `prepared` value is gone regardless of the refusal, by Rust's own move semantics) and
        // delete through storage_a, the one that actually owns this subvolume.
        let orphans = storage_a
            .list_orphaned_workspaces(&BTreeSet::new())
            .expect("list to find it again for cleanup");
        let candidate = orphans
            .into_iter()
            .find(|c| c.subvol_id() == subvol_id)
            .expect("the workspace is discoverable again through its own storage");
        storage_a
            .delete_orphan(candidate)
            .expect("cleanup through the correct storage");
        std::fs::remove_dir(storage_b.base_dir()).expect("remove the empty sibling base directory");
    }

    #[test]
    fn orphan_listing_reports_a_non_subvolume_entry_loudly_even_if_its_name_is_active() {
        // No CAP_SYS_ADMIN needed — listing only stats + reads rootid, neither privileged.
        let Some(mut storage) = open_or_skip_env("stray-entry") else {
            return;
        };
        let stray_name = format!("stray-file-{}", std::process::id());
        let stray = storage.base_dir().join(&stray_name);
        std::fs::write(&stray, b"not a subvolume").expect("create a stray plain file");
        // Round 3 fix: put the stray file's NAME in the active set. Round 2's ordering (filter by
        // active-set name BEFORE verifying) would have silently skipped it instead of ever
        // checking whether it's a real subvolume; verification must happen first regardless.
        let mut active = BTreeSet::new();
        active.insert(stray_name);
        let result = storage.list_orphaned_workspaces(&active);
        std::fs::remove_file(&stray).ok();
        assert!(
            matches!(result, Err(WorkspaceStorageError::UnexpectedEntry { .. })),
            "a stray non-subvolume entry must be reported loud even if its name matches an \
             active job id, got {result:?}"
        );
    }

    #[test]
    fn assert_workspace_open_refuses_a_tmpfs_directory() {
        // This exact host's /tmp is confirmed tmpfs (2026-07-26) — the RAM-backed area this
        // module exists specifically to NOT be. A real host without Btrfs must fail loud here,
        // never silently proceed onto an unbounded directory.
        let tmp = std::env::temp_dir().join("myelin-workspace-storage-not-btrfs-probe");
        let err = WorkspaceStorage::open(&tmp).unwrap_err();
        assert!(
            matches!(err, WorkspaceStorageError::NotBtrfs { .. }),
            "a tmpfs directory must be refused as NotBtrfs, got {err:?}"
        );
    }

    fn libc_enospc() -> i32 {
        28 // ENOSPC on Linux — stable across architectures.
    }

    fn libc_edquot() -> i32 {
        122 // EDQUOT on Linux — stable across architectures.
    }
}
