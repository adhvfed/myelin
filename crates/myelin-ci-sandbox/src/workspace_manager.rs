//! # Persistent workspace-storage manager (CT-007 slice 1)
//!
//! [`crate::workspace_storage::WorkspaceStorage`] is a raw Btrfs subvolume+qgroup PRIMITIVE — its
//! own doc directs a caller to open it "at runner startup AND on a caller-owned periodic health
//! check", and explicitly does NOT itself serialize concurrent same-process callers, track which
//! job ids are currently active (needed to correctly classify orphans), or account aggregate
//! host-disk capacity across concurrently-running jobs (a per-job qgroup limit only bounds ONE
//! job's own usage, not how many jobs run at once). This module is that caller: it owns exactly
//! ONE `WorkspaceStorage` for the life of the `GvisorBackend` process, holds a process-lifetime
//! exclusive lock on the base directory (so a second runner process sharing the same base refuses
//! at startup instead of corrupting each other's bookkeeping), runs boot-time orphan reconciliation
//! BEFORE ever admitting a new workspace, and tracks non-`Clone` disk-capacity leases.
//!
//! **Design consulted with Sol (gpt-5.6-sol) across TWO rounds, 2026-07-26** (recorded in
//! `planning/system-reviews/2026-06-26/12-ci-track-ledger.md`), as slice 1 of the 4-slice
//! `workspace_storage.rs` → `gvisor.rs` integration.
//!
//! **What round 1 got wrong, fixed in round 2:**
//! - `check_health()` delegated straight to `WorkspaceStorage::open()`, which itself unconditionally
//!   `create_dir_all`s a missing `base_dir` — directly contradicting the method's OWN claimed
//!   side-effect-free contract, and opened a SECOND, throwaway `WorkspaceStorage` instead of
//!   re-checking the one already stored under the manager's own lock. Fixed: the manager now
//!   caches the locked directory's (device, inode) at construction time, compares it against the
//!   CURRENT path on every health check (a mismatch means the path was deleted-and-recreated or
//!   renamed-and-replaced underneath the lock — poison, never silently accept it as healthy), and
//!   only then re-validates the ALREADY-OPEN, manager-owned `WorkspaceStorage` in place via the
//!   new [`crate::workspace_storage::WorkspaceStorage::check_health`] (itself genuinely
//!   side-effect-free — no `open()` call anywhere in this path).
//! - Admission and capacity bookkeeping were two SEPARATE mutexes — `acquire_capacity` checked
//!   `is_healthy()` and then, in a second, unsynchronized step, adjusted the capacity counter,
//!   racing against a concurrent poison. Fixed: both live in ONE `ManagerState` behind ONE mutex,
//!   so "is this manager Healthy, and does admitting `bytes` fit the ceiling" is one atomic
//!   critical section.
//! - `CapacityLease::release` marked itself `released` BEFORE the capacity-book mutex lock/update —
//!   if that lock were poisoned, `release()` would panic AFTER `released` was already set, so the
//!   subsequent `Drop` would see `released == true` and silently do NEITHER the bookkeeping NOR
//!   the abandonment poisoning, stranding the leaked bytes with no incident at all. Fixed: `lock_state`
//!   never panics on a poisoned `std::sync::Mutex` (it recovers the inner value and poisons the
//!   manager's OWN semantic admission state instead — the general fix for finding 4 below too), and
//!   `released` is only set true AFTER the capacity update has actually happened (or after the
//!   corruption case has been explicitly poisoned).
//! - A capacity-accounting underflow (releasing more than was ever recorded as leased — real
//!   corruption, not an ordinary race) was silently absorbed by `saturating_sub`. Fixed: an
//!   underflow now poisons the manager instead of being hidden.
//! - `SharedState::poison` called the external incident sink WHILE STILL HOLDING the admission
//!   lock — a reentrant sink (one that itself calls back into this manager) could deadlock; a
//!   panicking sink during unwind (e.g. called from `Drop`) could abort the process. Fixed: the
//!   lock is dropped before the sink is invoked, and the call is wrapped in `catch_unwind` so a
//!   panicking sink can never escape (in particular, can never escape a `Drop`).
//! - Every mutex access used `.lock().unwrap()`, contradicting `WorkspaceAdmission::Poisoned`'s own
//!   documented "an internally poisoned mutex also poisons this manager" claim — a poisoned
//!   `std::sync::Mutex` would have panicked instead. Fixed by centralizing every access through
//!   `SharedState::lock_state`.
//! - Five of the seven original tests silently skipped in ANY environment lacking real Btrfs+quota
//!   privilege (this session's own sandboxed bash included) even though the logic they exercised —
//!   lock contention, capacity bounds/release, abandoned-lease poisoning, admission refusal,
//!   incident-sink behavior — has NOTHING to do with Btrfs at all. Fixed: a `#[cfg(test)]`-only
//!   constructor takes the SAME process-lifetime directory lock (which works on any filesystem —
//!   `flock` needs no Btrfs) but skips opening a real `WorkspaceStorage` entirely, so these become
//!   ordinary, always-running unit tests. Only the genuine boot-reconciliation lifecycle test still
//!   gates on real privilege — and does so by actually attempting `create_workspace` (the exact
//!   operation that needs `CAP_SYS_ADMIN`/`CAP_CHOWN`), not merely by checking whether quota
//!   reporting works (round 1's `btrfs_available` conflated the two, so a host with quota-enforcing
//!   Btrfs but no capability would have FAILED that one test outright instead of skipping it).
//!
//! ## What this slice deliberately does NOT do
//! - Never calls `create_workspace`/`delete_workspace` for a REAL job — those calls belong to
//!   slice 3 (the `launch_with` lifecycle integration), which needs the container's mapped
//!   owner uid/gid from slice 2's UID-namespace subsystem first.
//! - Never mounts anything into an OCI bundle — that is also slice 3.
//! - `Disabled` mode performs NO Btrfs, lock, quota, or filesystem I/O at all. Production passes
//!   `Disabled` explicitly until slice 4's drills are complete — never a silent default that
//!   quietly starts provisioning real disk.

use crate::dirlock::{fd_identity, path_identity};
use crate::workspace_storage::{
    PreparedWorkspace, WorkspaceStorage, WorkspaceStorageBackend, WorkspaceStorageError,
};
#[cfg(any(test, feature = "test-support"))]
use crate::workspace_storage::DirectoryWorkspaceStorage;
use std::collections::BTreeSet;
use std::io;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

/// Whether (and how) a [`crate::gvisor::GvisorBackend`] provisions disk-backed ephemeral
/// workspaces for CI/agent jobs that need one.
#[derive(Clone, Debug)]
pub enum WorkspaceStorageMode {
    /// No disk-backed workspace is ever provisioned; `/workspace` is never mounted. The ONLY mode
    /// production may pass until CT-007 slice 4's drills are complete — a NAMED, deliberate floor,
    /// never an implicit fallback a caller could silently drift into.
    Disabled,
    /// Provision one Btrfs subvolume per job under `base_dir`, quota'd per-job by the spec's own
    /// `disk_bytes` (applied at create time, slice 3). `host_capacity_bytes` bounds the AGGREGATE
    /// bytes concurrently leased across every in-flight job on this host — a per-job qgroup limit
    /// alone only bounds one job's own usage, not how many jobs may run at once.
    EphemeralDisk {
        base_dir: PathBuf,
        host_capacity_bytes: u64,
    },
    /// **CT-007 slice 5b.3-6e.1b (DORMANT / test-support only): the deterministic plain-directory
    /// test substrate.** Provisions one plain directory per job under `base_dir` (no Btrfs, no
    /// qgroup, no `CAP_SYS_ADMIN`/subuid), so the checkout-capsule lifecycle RUNS — not soft-skips —
    /// on a host whose `/tmp` is tmpfs. `host_capacity_bytes` bounds the aggregate leased bytes
    /// exactly as `EphemeralDisk` does (the manager's aggregate capacity accounting stays REAL); the
    /// per-job limit becomes a byte-accounted TEST quota, deliberately not a hard quota. ABSENT from
    /// ordinary builds and NOT representable through `GvisorWorkspaceConfig`, env config, or any
    /// production composition root — a source pin keeps production construction at ZERO.
    #[cfg(any(test, feature = "test-support"))]
    DeterministicDirectoryForTests {
        base_dir: PathBuf,
        host_capacity_bytes: u64,
    },
}

/// The manager's own admission state. MONOTONIC toward [`WorkspaceAdmission::Poisoned`] for the
/// life of the process — never resets to `Healthy` once poisoned. A poisoned manager requires a
/// process restart, which re-runs boot-time reconciliation from a clean slate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceAdmission {
    /// Boot-time orphan reconciliation is in progress (or `Disabled` mode, vacuously). No new
    /// workspace may be created while `Reconciling`.
    Reconciling,
    /// Reconciliation completed cleanly (every orphan deleted and synced) — or `Disabled` mode,
    /// where this is the permanent, vacuous value since no workspace is ever created. New
    /// workspaces may be created.
    Healthy,
    /// An unexpected entry, a failed delete/sync, an internally poisoned mutex, or an abandoned
    /// capacity/workspace lease made this manager refuse all further admissions. `reason` is for
    /// operators, not machine-parsed.
    Poisoned { reason: String },
}

/// A `WorkspaceManager` construction/reconciliation/health-check failure — always fatal to
/// starting (or continuing to trust) the runner: an operator must fix the underlying condition (or
/// point `base_dir` elsewhere) and restart, never silently retried with a degraded manager.
#[derive(Debug)]
pub enum WorkspaceManagerError {
    /// Another process already holds the exclusive lock on this `base_dir` — two runner processes
    /// must never manage the same workspace base concurrently (their in-memory bookkeeping would
    /// silently diverge from the real on-disk state).
    AlreadyLocked { base_dir: PathBuf },
    /// Acquiring the process-lifetime directory lock (or reading its identity) failed for a reason
    /// other than contention.
    LockFailed { base_dir: PathBuf, reason: String },
    /// The underlying `WorkspaceStorage` primitive refused to open, refused a health re-check, or
    /// boot-time reconciliation failed outright (not merely found orphans — actually failed to
    /// list/delete/sync one).
    Storage(WorkspaceStorageError),
    /// The path this manager locked at construction time no longer names the SAME directory (a
    /// delete-and-recreate, or a rename-and-replace, happened underneath the lock). Never silently
    /// accepted as healthy — the replacement directory's own quota/ownership might be fine while
    /// still being the WRONG directory for this manager's bookkeeping to trust.
    BaseReplaced { base_dir: PathBuf },
}

impl std::fmt::Display for WorkspaceManagerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkspaceManagerError::AlreadyLocked { base_dir } => write!(
                f,
                "workspace base {base_dir:?} is already locked by another process — two runner \
                 processes must never manage the same workspace base concurrently"
            ),
            WorkspaceManagerError::LockFailed { base_dir, reason } => {
                write!(f, "failed to lock workspace base {base_dir:?}: {reason}")
            }
            WorkspaceManagerError::Storage(e) => {
                write!(f, "workspace-storage startup/health check failed: {e}")
            }
            WorkspaceManagerError::BaseReplaced { base_dir } => write!(
                f,
                "workspace base {base_dir:?} no longer names the directory this manager locked \
                 at startup — refusing to trust a replacement directory"
            ),
        }
    }
}

impl std::error::Error for WorkspaceManagerError {}

/// Called with a human-readable incident description whenever the manager poisons itself, or a
/// [`CapacityLease`] is abandoned abnormally. The production wiring is expected to route this to
/// whatever operational alerting the runner host already has; a test double just records calls.
/// Invoked OUTSIDE any internal lock, and any panic it raises is caught (never escapes, in
/// particular never escapes a [`CapacityLease`]'s [`Drop`]).
pub type IncidentSink = Arc<dyn Fn(&str) + Send + Sync>;

/// Why [`WorkspaceManager::acquire_capacity`] refused. A closed, typed vocabulary rather than a
/// single opaque "no" — an operator/caller needs to tell "this host is genuinely full right now"
/// apart from "this manager is unhealthy and needs a restart" apart from "workspace storage isn't
/// even configured here".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapacityRefusal {
    /// This manager is [`WorkspaceStorageMode::Disabled`] — there is no capacity ceiling to lease
    /// against at all.
    Disabled,
    /// Boot-time reconciliation has not finished yet.
    Reconciling,
    /// The manager is [`WorkspaceAdmission::Poisoned`] — refuses every admission until restarted.
    Poisoned,
    /// A zero-byte request is nonsensical (either "no workspace" or a caller bug) — never silently
    /// treated as "trivially satisfied".
    ZeroBytesRequested,
    /// `used_bytes + bytes` would overflow `u64` — refused rather than wrapping.
    Overflow,
    /// Admitting `requested` bytes would exceed the aggregate host ceiling. `available` is how
    /// much headroom actually remains right now.
    ExhaustedCapacity { requested: u64, available: u64 },
}

impl std::fmt::Display for CapacityRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CapacityRefusal::Disabled => {
                write!(
                    f,
                    "workspace storage is Disabled — no capacity ceiling exists"
                )
            }
            CapacityRefusal::Reconciling => {
                write!(
                    f,
                    "workspace manager has not finished boot-time reconciliation yet"
                )
            }
            CapacityRefusal::Poisoned => {
                write!(f, "workspace manager is poisoned — refusing all admission")
            }
            CapacityRefusal::ZeroBytesRequested => {
                write!(f, "a zero-byte capacity request is invalid")
            }
            CapacityRefusal::Overflow => {
                write!(
                    f,
                    "capacity accounting would overflow — refused rather than wrapping"
                )
            }
            CapacityRefusal::ExhaustedCapacity {
                requested,
                available,
            } => write!(
                f,
                "requested {requested} bytes but only {available} bytes of aggregate host \
                 capacity remain"
            ),
        }
    }
}

/// CT-007 slice 3: why [`WorkspaceManager::create_workspace`] refused, before ever touching
/// `WorkspaceStorage`. EVERY variant here hands the caller's [`CapacityLease`] BACK — a refusal at
/// this stage means NOTHING was provisioned, so the lease is exactly as valid as it was before the
/// call. Without this, a caller's mistake (e.g. passing a lease from the wrong manager) would drop
/// the lease at the end of this function's scope, triggering its OWN `Drop` — silently poisoning
/// an otherwise-perfectly-healthy, unrelated manager over what was really just a refused call. A
/// caller receiving one of these can retry against the right manager/arguments or call
/// `.release()` on the returned lease.
#[derive(Debug)]
pub enum WorkspaceRequestRefusal {
    /// This manager is [`WorkspaceStorageMode::Disabled`] — no workspace is ever provisioned.
    Disabled { capacity: CapacityLease },
    /// Boot-time reconciliation has not finished yet.
    Reconciling { capacity: CapacityLease },
    /// The manager is [`WorkspaceAdmission::Poisoned`] — refuses every admission until restarted.
    Poisoned { capacity: CapacityLease },
    /// `job_key` already has a checked-out, undeleted workspace — a caller bug (the same job key
    /// must never be provisioned twice concurrently) rather than something to silently paper over.
    JobAlreadyActive {
        job_key: String,
        capacity: CapacityLease,
    },
    /// The supplied [`CapacityLease`] was not leased from THIS manager — accepting a lease from a
    /// different manager instance would let one manager's admission decision authorize provisioning
    /// against another's bookkeeping entirely.
    WrongManager { capacity: CapacityLease },
    /// The supplied [`CapacityLease`]'s own `bytes()` does not EXACTLY equal the requested
    /// `quota_bytes` — both are meant to originate from the SAME `spec.limits.disk_bytes`, so any
    /// inequality indicates a caller bug, not something to silently reconcile by taking the smaller
    /// (or larger) of the two.
    CapacityMismatch {
        requested: u64,
        leased: u64,
        capacity: CapacityLease,
    },
}

impl WorkspaceRequestRefusal {
    /// Take back the [`CapacityLease`] this refusal returned — every variant carries one, since a
    /// refusal at THIS stage means nothing was provisioned and the lease remains exactly as valid
    /// as before the call.
    pub fn into_capacity(self) -> CapacityLease {
        match self {
            WorkspaceRequestRefusal::Disabled { capacity }
            | WorkspaceRequestRefusal::Reconciling { capacity }
            | WorkspaceRequestRefusal::Poisoned { capacity }
            | WorkspaceRequestRefusal::JobAlreadyActive { capacity, .. }
            | WorkspaceRequestRefusal::WrongManager { capacity }
            | WorkspaceRequestRefusal::CapacityMismatch { capacity, .. } => capacity,
        }
    }
}

impl std::fmt::Display for WorkspaceRequestRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkspaceRequestRefusal::Disabled { .. } => {
                write!(
                    f,
                    "workspace storage is Disabled — no workspace is ever provisioned"
                )
            }
            WorkspaceRequestRefusal::Reconciling { .. } => write!(
                f,
                "workspace manager has not finished boot-time reconciliation yet"
            ),
            WorkspaceRequestRefusal::Poisoned { .. } => {
                write!(f, "workspace manager is poisoned — refusing all admission")
            }
            WorkspaceRequestRefusal::JobAlreadyActive { job_key, .. } => write!(
                f,
                "job key {job_key:?} already has an active, undeleted workspace"
            ),
            WorkspaceRequestRefusal::WrongManager { .. } => write!(
                f,
                "the supplied capacity lease was not leased from this manager"
            ),
            WorkspaceRequestRefusal::CapacityMismatch {
                requested, leased, ..
            } => write!(
                f,
                "requested {requested} quota bytes but the supplied capacity lease holds \
                 {leased} bytes — both must originate from the same spec.limits.disk_bytes"
            ),
        }
    }
}

impl std::error::Error for WorkspaceRequestRefusal {}

/// CT-007 slice 3: the combined failure mode of [`WorkspaceManager::create_workspace`] SPECIFICALLY
/// (`delete_workspace` has its own dedicated [`DeleteWorkspaceError`] — the two are not, and must
/// not be, the same type, since a `delete_workspace` refusal hands back a whole
/// [`ManagedWorkspace`], not a bare [`CapacityLease`]). `Refused` covers every refusal that happens
/// BEFORE any real `WorkspaceStorage` operation is attempted — the caller's `CapacityLease` (via
/// [`WorkspaceRequestRefusal::into_capacity`]) is always recoverable from it. `Storage` covers a
/// real operation that was attempted and failed — by this point the caller's `CapacityLease` has
/// ALREADY been consumed internally (released back to the pool, or abandoned — poisoning this
/// manager — depending on what `WorkspaceStorage` itself proved), so there is nothing left to hand
/// back.
#[derive(Debug)]
pub enum WorkspaceProvisionError {
    Refused(WorkspaceRequestRefusal),
    Storage(WorkspaceStorageError),
    /// Sol's review, round 3: the disk operation genuinely SUCCEEDED, but this manager's own
    /// `active_job_ids` bookkeeping was already wrong beforehand (should be structurally
    /// impossible, given the immediately-preceding check happens under the SAME continuously-held
    /// lock — never silently trusted anyway). This is a HARD failure — NOT `Ok(ManagedWorkspace)`
    /// — so a caller like `launch_with` cannot mistake a corrupted invariant for authorization to
    /// proceed with launching the workload. The real, freshly-created `ManagedWorkspace`
    /// capability is still carried along (via [`Self::into_workspace_after_invariant_violation`])
    /// so the caller can still clean up the real subvolume via [`WorkspaceManager::delete_workspace`]
    /// — which does not itself refuse merely because admission is poisoned.
    InternalInvariantViolated {
        reason: String,
        workspace: Box<ManagedWorkspace>,
    },
}

impl WorkspaceProvisionError {
    /// Take back the [`ManagedWorkspace`] an `InternalInvariantViolated` failure carries — the
    /// real subvolume was actually created; this is the only way to eventually clean it up. Panics
    /// on any other variant (neither carries one — see the type's own doc).
    pub fn into_workspace_after_invariant_violation(self) -> ManagedWorkspace {
        match self {
            WorkspaceProvisionError::InternalInvariantViolated { workspace, .. } => *workspace,
            WorkspaceProvisionError::Refused(_) | WorkspaceProvisionError::Storage(_) => panic!(
                "only WorkspaceProvisionError::InternalInvariantViolated carries a \
                 ManagedWorkspace back"
            ),
        }
    }
}

impl std::fmt::Display for WorkspaceProvisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkspaceProvisionError::Refused(refusal) => write!(f, "{refusal}"),
            WorkspaceProvisionError::Storage(e) => {
                write!(f, "workspace-storage provisioning/deletion failed: {e}")
            }
            WorkspaceProvisionError::InternalInvariantViolated { reason, .. } => {
                write!(f, "internal invariant violated: {reason}")
            }
        }
    }
}

impl std::error::Error for WorkspaceProvisionError {}

/// The manager's full mutable state, behind ONE mutex — admission, capacity bookkeeping, active
/// job ids, and the open `WorkspaceStorage` handle are never independently synchronized, so "is
/// this manager Healthy AND does this fit the ceiling" is always one atomic critical section
/// (round 2's fix for the race round 1 had between an admission check and a capacity update).
struct ManagerState {
    storage: Option<WorkspaceStorageBackend>,
    admission: WorkspaceAdmission,
    /// Job ids with a currently-checked-out workspace — snapshotted for
    /// `list_orphaned_workspaces`'s active-set filter in a future (slice 3) periodic health pass.
    /// Empty for the entire life of slice 1 (nothing ever checks a workspace out yet).
    active_job_ids: BTreeSet<String>,
    capacity_ceiling_bytes: u64,
    capacity_used_bytes: u64,
    /// The (device, inode) of the locked base directory, captured once at construction. `None` in
    /// `Disabled` mode. Compared against the CURRENT path on every [`WorkspaceManager::check_health`]
    /// call to detect a delete-and-recreate or rename-and-replace underneath the lock.
    locked_identity: Option<(u64, u64)>,
}

struct SharedState {
    /// The process-lifetime exclusive lock on `base_dir` itself (an `flock` on the directory's own
    /// FD). Held HERE — never directly by [`WorkspaceManager`] alone (round 2's review caught this:
    /// a `WorkspaceManager` dropped while a [`CapacityLease`] was still outstanding would otherwise
    /// release the flock while the leased capacity — and any workspace it represents — was still
    /// logically live, letting a SECOND manager lock the same base and reconcile the first
    /// manager's still-live workspace as an orphan). Every `CapacityLease` holds an `Arc` to this
    /// SAME `SharedState`, so the lock now stays held for as long as EITHER the manager or any of
    /// its outstanding leases is alive — whichever drops last. `None` in `Disabled` mode.
    _lock: Option<OwnedFd>,
    state: Mutex<ManagerState>,
    incident_sink: IncidentSink,
}

impl SharedState {
    /// The ONE way any code in this module accesses `state` — a poisoned `std::sync::Mutex` (a
    /// prior panic while the lock was held) is recovered rather than propagated as a SECOND panic,
    /// and is itself treated as grounds to poison this manager's own semantic admission state.
    /// This is what actually implements `WorkspaceAdmission::Poisoned`'s documented "an internally
    /// poisoned mutex also poisons this manager" claim (round 1 documented this but every access
    /// still used a bare `.lock().unwrap()`, which would have panicked instead).
    ///
    /// NARROWED GUARANTEE (Sol's review): this specific internal-mutex-poisoning case does NOT
    /// invoke the incident sink — doing so safely would require deferring the sink call past every
    /// call site's own guard drop (the same reentrancy hazard [`Self::poison`]/[`Self::report_incident`]
    /// exist to avoid), which is disproportionate for an already-extremely-rare defensive backstop
    /// (it can only trigger if a PRIOR panic occurred while this exact lock was already held). The
    /// admission state IS still correctly flipped to `Poisoned` either way.
    fn lock_state(&self) -> MutexGuard<'_, ManagerState> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                let mut inner = poisoned.into_inner();
                if !matches!(inner.admission, WorkspaceAdmission::Poisoned { .. }) {
                    inner.admission = WorkspaceAdmission::Poisoned {
                        reason: "internal manager-state mutex was poisoned by a prior panic"
                            .to_string(),
                    };
                }
                inner
            }
        }
    }

    /// Invoke the incident sink. The ONLY way any code in this module calls it — NEVER while
    /// holding `state`'s lock (a reentrant sink that calls back into this manager would otherwise
    /// deadlock against itself), and always wrapped in `catch_unwind` so a panicking sink can never
    /// escape (in particular can never escape a [`CapacityLease`]'s [`Drop`], which would otherwise
    /// abort the process during an unwind). Callers MUST have already dropped their `state` guard
    /// before calling this.
    fn report_incident(&self, message: &str) {
        let sink = self.incident_sink.clone();
        let message = message.to_string();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || sink(&message)));
    }

    /// Poison the manager (idempotent/monotonic — the FIRST reason wins) and report the incident.
    /// The lock is held ONLY long enough to flip the admission state; [`Self::report_incident`] is
    /// always invoked with the lock already released.
    fn poison(&self, reason: impl Into<String>) {
        let reason = reason.into();
        {
            let mut state = self.lock_state();
            if !matches!(state.admission, WorkspaceAdmission::Poisoned { .. }) {
                state.admission = WorkspaceAdmission::Poisoned {
                    reason: reason.clone(),
                };
            }
        }
        self.report_incident(&reason);
    }
}

/// A non-`Clone` hold on `bytes` of the manager's aggregate host-disk-capacity ceiling.
///
/// **Two distinct ways to give this up, never confused:**
/// - [`CapacityLease::release`] — the ORDINARY path: the job's workspace (if any) has already been
///   fully deleted from disk, so these bytes are genuinely free again. Consumes `self`.
/// - An un-consumed `Drop` (a panic unwound past it, or a caller simply forgot) — the ABNORMAL
///   path. This does **NOT** return the bytes to the pool: a workspace may still exist on disk
///   consuming them, and silently freeing this capacity would let a future admission decision
///   undercount real host disk usage. It instead poisons the manager so a human must reconcile
///   before any further admission — the same fail-closed posture
///   [`crate::workspace_storage::WorkspaceStorageError::UnrecoverableLeak`] already established
///   for the primitive itself.
pub struct CapacityLease {
    bytes: u64,
    shared: Arc<SharedState>,
    released: bool,
}

impl std::fmt::Debug for CapacityLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CapacityLease")
            .field("bytes", &self.bytes)
            .field("released", &self.released)
            .finish()
    }
}

impl CapacityLease {
    /// The leased byte ceiling.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Ordinary release: the associated workspace (if any) is confirmed fully deleted. Returns
    /// `bytes` to the pool. `released` is only set true AFTER the capacity update genuinely
    /// happens (round 2's fix: round 1 set it BEFORE locking/updating, so a poisoned lock would
    /// have let `Drop` observe `released == true` and silently skip BOTH the bookkeeping and the
    /// abandonment incident — stranding the bytes with no incident at all).
    pub fn release(mut self) {
        let mut state = self.shared.lock_state();
        match state.capacity_used_bytes.checked_sub(self.bytes) {
            Some(next) => {
                state.capacity_used_bytes = next;
                drop(state);
                self.released = true;
            }
            None => {
                // Real corruption (releasing more than was ever recorded as leased) — never
                // silently absorbed via `saturating_sub`.
                let used = state.capacity_used_bytes;
                let bytes = self.bytes;
                drop(state);
                // Mark released FIRST so Drop does not ALSO report a second, misleading
                // "abandoned lease" incident on top of the corruption incident below.
                self.released = true;
                self.shared.poison(format!(
                    "capacity-accounting corruption: releasing {bytes} bytes but only {used} \
                     bytes were recorded as used"
                ));
            }
        }
    }

    /// CT-007 slice 3: consumed when a workspace provisioning/deletion failure leaves the byte
    /// accounting genuinely unreconciled (e.g. [`crate::workspace_storage::WorkspaceStorageError::UnrecoverableLeak`]
    /// — the subvolume may still exist on disk, still consuming this capacity). Poisons the
    /// manager with the SPECIFIC provisioning/deletion reason (not the generic "dropped without an
    /// explicit release" message `Drop` would otherwise report), and marks itself released so
    /// `Drop` stays silent — the specific, more useful incident has already fired.
    fn abandon_with_reason(mut self, reason: impl Into<String>) {
        self.shared.poison(reason);
        self.released = true;
    }
}

impl Drop for CapacityLease {
    fn drop(&mut self) {
        if !self.released {
            self.shared.poison(format!(
                "a {}-byte disk-capacity lease was dropped without an explicit release — a \
                 workspace may still be consuming this capacity on disk; refusing further \
                 admission until a human reconciles",
                self.bytes
            ));
        }
    }
}

/// CT-007 slice 3: a real, checked-out workspace bound to the [`CapacityLease`] that authorized
/// it. Sol's review: a borrowed `&CapacityLease` would let ONE lease authorize multiple workspace
/// creations — this type CONSUMES the lease instead, so "a workspace is checked out" and "its
/// capacity is spoken for" can never drift apart. Non-`Clone`; the only way to get one is
/// [`WorkspaceManager::create_workspace`], the only way to give one up is
/// [`WorkspaceManager::delete_workspace`] (which consumes it by value).
pub struct ManagedWorkspace {
    job_key: String,
    /// `None` only in the brief window inside [`WorkspaceManager::delete_workspace`] after both
    /// fields have been taken out for the actual delete call — never observable by any external
    /// caller.
    prepared: Option<PreparedWorkspace>,
    capacity: Option<CapacityLease>,
    shared: Arc<SharedState>,
    released: bool,
}

impl std::fmt::Debug for ManagedWorkspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ManagedWorkspace")
            .field("job_key", &self.job_key)
            .field("host_path", &self.prepared.as_ref().map(|p| p.host_path()))
            .field("released", &self.released)
            .finish()
    }
}

impl ManagedWorkspace {
    /// The job key this workspace was provisioned for.
    pub fn job_key(&self) -> &str {
        &self.job_key
    }

    /// The host filesystem path — bind-mount this (read-write) into the sandbox's OCI bundle.
    /// `None` only during the brief internal window inside [`WorkspaceManager::delete_workspace`].
    pub fn host_path(&self) -> &Path {
        self.prepared
            .as_ref()
            .expect("host_path() called after this workspace was already consumed by delete")
            .host_path()
    }

    /// The capacity (bytes) this workspace's [`CapacityLease`] holds.
    pub fn capacity_bytes(&self) -> u64 {
        self.capacity
            .as_ref()
            .expect("capacity_bytes() called after this workspace was already consumed by delete")
            .bytes()
    }

    /// CT-007 slice 3, Sol's review (round 3): a test-only clean teardown for a `ManagedWorkspace`
    /// built via the `apply_create_result` seam (no real `WorkspaceStorage` exists to delete
    /// against, so the real `WorkspaceManager::delete_workspace` cannot be used) — releases the
    /// held `CapacityLease` properly and marks this workspace released, so neither its own `Drop`
    /// nor the lease's fires an incident. `mem::forget`, used here in an earlier round, would have
    /// silently leaked the lease's `Arc<SharedState>`, this workspace's directory-lock FD, and its
    /// heap allocation for the remainder of the test process.
    #[cfg(test)]
    fn dismantle_for_tests(mut self) {
        if let Some(capacity) = self.capacity.take() {
            capacity.release();
        }
        self.released = true;
    }
}

/// CT-007 slice 5b.3-6e.1b: the byte-accounted test-quota checked I/O, exposed to the sealed
/// checkout-capsule execution seam ONLY (`test-support`). Delegates to the workspace's private
/// [`PreparedWorkspace`] typed `Directory` capability — the manager never lets a caller reach the
/// capability itself, only these checked operations.
#[cfg(any(test, feature = "test-support"))]
impl ManagedWorkspace {
    /// The substituted Hop B's checked sentinel write: scans + checked-adds regular-file bytes and
    /// refuses an over-quota write BEFORE any mutation. Refuses on a non-directory-backed workspace
    /// ([`WorkspaceStorageError::BackendMismatch`]).
    pub(crate) fn checked_test_quota_write(
        &self,
        file_name: &str,
        bytes: &[u8],
    ) -> Result<(), WorkspaceStorageError> {
        self.prepared
            .as_ref()
            .expect("checked_test_quota_write after this workspace was consumed by delete")
            .checked_directory_write(file_name, bytes)
    }

    /// Re-scan the total regular-file bytes currently under this workspace — the checkpoint the seam
    /// re-runs at the controlled workload/cleanup boundaries.
    pub(crate) fn scan_used_bytes(&self) -> Result<u64, WorkspaceStorageError> {
        self.prepared
            .as_ref()
            .expect("scan_used_bytes after this workspace was consumed by delete")
            .scan_used_bytes()
    }
}

impl Drop for ManagedWorkspace {
    fn drop(&mut self) {
        if !self.released {
            self.shared.poison(format!(
                "a managed workspace for job {:?} (host path {:?}) was dropped without being \
                 deleted — it may still exist on disk, still consuming its leased capacity; \
                 refusing further admission until a human reconciles",
                self.job_key,
                self.prepared.as_ref().map(|p| p.host_path())
            ));
            // The capacity lease, if still held, is abandoned TOGETHER with this workspace — one
            // comprehensive incident above, not a second, less-specific "abandoned lease" message
            // from `CapacityLease`'s own `Drop` immediately after.
            if let Some(mut capacity) = self.capacity.take() {
                capacity.released = true;
            }
        }
    }
}

/// The persistent per-process owner of workspace-storage admission, health, and disk-capacity
/// bookkeeping. `GvisorBackend` holds exactly one; it is never opened per launch.
pub struct WorkspaceManager {
    mode: WorkspaceStorageMode,
    /// The process-lifetime exclusive lock on `base_dir` lives on [`SharedState`] (`shared._lock`),
    /// not here — round 3's review caught that a `CapacityLease` retaining only `Arc<SharedState>`
    /// while the lock lived directly on `WorkspaceManager` could outlive the manager's own lock
    /// (`drop(manager)` released the `flock` while a lease — and the workspace it represents — was
    /// still logically live), letting a second manager falsely lock the same base and reconcile the
    /// first manager's still-live workspace as an orphan. See [`SharedState::_lock`].
    shared: Arc<SharedState>,
}

impl WorkspaceManager {
    /// Construct the manager, running boot-time orphan reconciliation before ever returning
    /// `Healthy` admission. Fallible and explicit — there is no infallible constructor, so a
    /// caller cannot accidentally start a runner with a degraded or unlocked workspace manager.
    ///
    /// For [`WorkspaceStorageMode::Disabled`]: performs no I/O at all, admission is vacuously
    /// `Healthy` (nothing is ever gated on it), and no lock is taken.
    ///
    /// For [`WorkspaceStorageMode::EphemeralDisk`]: acquires the process-lifetime lock FIRST (so a
    /// second runner process sharing the same base refuses immediately, before touching anything),
    /// records the locked directory's (device, inode) identity, opens the `WorkspaceStorage`
    /// primitive (which itself fails loud if the base is not a quota-enforcing Btrfs filesystem
    /// owned exclusively by this process), then reconciles: at boot, NOTHING is active yet, so
    /// every discovered subvolume is necessarily an orphan from a previous process instance (a
    /// crash, a kill -9 mid-job) — each is deleted and synced before admission ever opens.
    pub fn try_new(
        mode: WorkspaceStorageMode,
        incident_sink: IncidentSink,
    ) -> Result<Self, WorkspaceManagerError> {
        // The `Disabled` fast path performs no I/O; every ENABLED backend (Btrfs, or the dormant
        // deterministic-directory test substrate) shares the SAME lock → identity → open →
        // reconcile → health → admit sequence, differing only in which `WorkspaceStorageBackend`
        // arm `open_enabled_backend` constructs.
        let (base_dir, host_capacity_bytes) = match &mode {
            WorkspaceStorageMode::Disabled => {
                return Ok(Self {
                    mode,
                    shared: Arc::new(SharedState {
                        _lock: None,
                        state: Mutex::new(ManagerState {
                            storage: None,
                            admission: WorkspaceAdmission::Healthy,
                            active_job_ids: BTreeSet::new(),
                            capacity_ceiling_bytes: 0,
                            capacity_used_bytes: 0,
                            locked_identity: None,
                        }),
                        incident_sink,
                    }),
                });
            }
            WorkspaceStorageMode::EphemeralDisk {
                base_dir,
                host_capacity_bytes,
            } => (base_dir.clone(), *host_capacity_bytes),
            #[cfg(any(test, feature = "test-support"))]
            WorkspaceStorageMode::DeterministicDirectoryForTests {
                base_dir,
                host_capacity_bytes,
            } => (base_dir.clone(), *host_capacity_bytes),
        };
        let lock = acquire_directory_lock(&base_dir)?;
        let locked_identity = fd_identity(&lock).map_err(|e| WorkspaceManagerError::LockFailed {
            base_dir: base_dir.clone(),
            reason: format!("fstat locked directory: {e}"),
        })?;
        let shared = Arc::new(SharedState {
            _lock: Some(lock),
            state: Mutex::new(ManagerState {
                storage: None,
                admission: WorkspaceAdmission::Reconciling,
                active_job_ids: BTreeSet::new(),
                capacity_ceiling_bytes: host_capacity_bytes,
                capacity_used_bytes: 0,
                locked_identity: Some(locked_identity),
            }),
            incident_sink,
        });
        let mut storage = open_enabled_backend(&mode, &base_dir)?;
        require_locked_identity(locked_identity, &base_dir, storage.base_dir())?;
        reconcile_orphans_at_boot(&mut storage).map_err(WorkspaceManagerError::Storage)?;
        // Re-check identity BEFORE the post-reconciliation health check below, so that check is
        // never run against the wrong path if something swapped underneath us during reconciliation.
        require_locked_identity(locked_identity, &base_dir, storage.base_dir())?;
        // Boot-time orphan deletion is exactly the kind of operation `check_health`'s own doc warns
        // can leave Btrfs quota inconsistent (Sol's round-5 review). Re-validate in place before
        // ever trusting this manager as `Healthy`.
        storage
            .check_health()
            .map_err(WorkspaceManagerError::Storage)?;
        // Identity may change during that external `check_health` call itself — one more recheck
        // immediately before the ONLY point admission is ever allowed to become `Healthy`.
        require_locked_identity(locked_identity, &base_dir, storage.base_dir())?;
        {
            let mut state = shared.lock_state();
            state.storage = Some(storage);
            state.admission = WorkspaceAdmission::Healthy;
        }
        Ok(Self { mode, shared })
    }

    /// Test-only constructor for the manager-STATE logic (admission, capacity leasing, poisoning)
    /// that has nothing to do with Btrfs: takes the SAME real process-lifetime directory lock
    /// (`flock` needs no Btrfs — it works on any filesystem, including an ordinary tmp directory),
    /// but skips opening a `WorkspaceStorage` entirely and starts `Healthy` immediately. This is
    /// what lets lock-contention, capacity-bounds, abandoned-lease-poisoning, and incident-sink
    /// tests run as ordinary, always-on unit tests instead of silently skipping in any environment
    /// lacking real Btrfs+quota privilege (round 1's mistake).
    #[cfg(test)]
    fn new_for_state_tests(
        base_dir: &Path,
        host_capacity_bytes: u64,
        incident_sink: IncidentSink,
    ) -> Result<Self, WorkspaceManagerError> {
        let lock = acquire_directory_lock(base_dir)?;
        let locked_identity =
            fd_identity(&lock).map_err(|e| WorkspaceManagerError::LockFailed {
                base_dir: base_dir.to_path_buf(),
                reason: format!("fstat locked directory: {e}"),
            })?;
        let shared = Arc::new(SharedState {
            _lock: Some(lock),
            state: Mutex::new(ManagerState {
                storage: None,
                admission: WorkspaceAdmission::Healthy,
                active_job_ids: BTreeSet::new(),
                capacity_ceiling_bytes: host_capacity_bytes,
                capacity_used_bytes: 0,
                locked_identity: Some(locked_identity),
            }),
            incident_sink,
        });
        Ok(Self {
            mode: WorkspaceStorageMode::EphemeralDisk {
                base_dir: base_dir.to_path_buf(),
                host_capacity_bytes,
            },
            shared,
        })
    }

    /// The mode this manager was constructed with.
    pub fn mode(&self) -> &WorkspaceStorageMode {
        &self.mode
    }

    /// The current admission state. `Disabled` mode always reports `Healthy` (vacuously — nothing
    /// is ever gated on it).
    pub fn admission(&self) -> WorkspaceAdmission {
        self.shared.lock_state().admission.clone()
    }

    /// Whether a new workspace may be admitted right now — `false` for anything other than
    /// `Healthy` (including `Disabled`'s own callers, which should never reach here: a `Disabled`
    /// manager's caller must check [`Self::mode`] and skip workspace admission entirely, not rely
    /// on this returning `false`).
    pub fn is_healthy(&self) -> bool {
        matches!(self.admission(), WorkspaceAdmission::Healthy)
    }

    /// A read-only health re-check. Three layers, in order:
    /// 1. **Base identity**: does the CURRENT `base_dir` path still name the exact directory this
    ///    manager locked at construction (compared by device+inode, cached from the lock's own file
    ///    descriptor at startup)? A mismatch (deleted-and-recreated, or renamed-and-replaced,
    ///    underneath the lock) poisons the manager.
    /// 2. **Storage identity**: does the manager-owned `WorkspaceStorage`'s own canonical
    ///    `base_dir()` ALSO still resolve to that same locked identity? Checking only layer 1 is
    ///    not enough — `WorkspaceStorage::open` independently `canonicalize`s `base_dir` at
    ///    construction time, so a symlink retargeted and restored around that one call could leave
    ///    the open `WorkspaceStorage` permanently bound to a DIFFERENT canonical directory than the
    ///    one the flock actually protects, while `base_dir` itself reads as unchanged on every
    ///    later check (Sol's round-4 review; see [`require_locked_identity`]'s doc for the full
    ///    TOCTOU). A replacement directory is NEVER accepted as healthy no matter what its own
    ///    quota/ownership look like.
    /// 3. **Preconditions**: assuming both identities hold, is the manager-owned `WorkspaceStorage`
    ///    (re-validated IN PLACE via [`crate::workspace_storage::WorkspaceStorage::check_health`] —
    ///    never a second, separate `open()`) still exclusively-owned and quota-enforcing?
    ///
    /// A no-op returning `Ok(())` in `Disabled` mode. Poisons the manager on any failure via
    /// [`Self::poison_and_report`], which takes the `MutexGuard` BY VALUE specifically so it can
    /// `drop` it before invoking the incident sink — round 3's review caught a bug here where an
    /// earlier helper only received a `&mut MutexGuard` (a borrow, not an owned guard), so the
    /// CALLER's own guard was still held for the sink call's entire duration despite a comment
    /// claiming otherwise. A reentrant sink would have deadlocked.
    pub fn check_health(&self) -> Result<(), WorkspaceManagerError> {
        let Some(base_dir) = enabled_base_dir(&self.mode) else {
            return Ok(());
        };
        let state = self.shared.lock_state();
        let locked_identity = state
            .locked_identity
            .expect("an EphemeralDisk manager always records a locked identity at construction");

        if let Err(error) = check_path_matches_locked_identity(locked_identity, base_dir) {
            return self.poison_and_report(state, error);
        }

        let storage_base_dir = match state.storage.as_ref() {
            Some(storage) => storage.base_dir().to_path_buf(),
            // An `EphemeralDisk` manager with NO open `WorkspaceStorage` is a broken internal
            // invariant for any real (`try_new`-constructed) manager — `storage` is always `Some`
            // by the time admission can ever be anything but `Reconciling`. Never silently treated
            // as healthy (round 1 did — masking the invariant violation as an `Ok`).
            None => {
                let error = WorkspaceManagerError::Storage(WorkspaceStorageError::Io {
                    path: base_dir.to_path_buf(),
                    reason: "internal invariant violated: an EphemeralDisk manager has no open \
                             WorkspaceStorage handle"
                        .to_string(),
                });
                return self.poison_and_report(state, error);
            }
        };
        if let Err(error) = check_path_matches_locked_identity(locked_identity, &storage_base_dir) {
            return self.poison_and_report(state, error);
        }

        let health_result = state
            .storage
            .as_ref()
            .expect("checked Some above")
            .check_health();
        match health_result {
            Ok(()) => {
                // Identity may have changed DURING that external `check_health` call itself
                // (Sol's round-5 review: the same TOCTOU discipline construction now applies
                // here) — recheck both paths once more before ever returning `Ok`.
                if let Err(error) = check_path_matches_locked_identity(locked_identity, base_dir) {
                    return self.poison_and_report(state, error);
                }
                if let Err(error) =
                    check_path_matches_locked_identity(locked_identity, &storage_base_dir)
                {
                    return self.poison_and_report(state, error);
                }
                Ok(())
            }
            Err(storage_error) => {
                self.poison_and_report(state, WorkspaceManagerError::Storage(storage_error))
            }
        }
    }

    /// Poison the manager (if not already) and report the incident, given a `MutexGuard` ALREADY
    /// held by the caller. Takes `state` BY VALUE (not `&mut`) specifically so this method — not
    /// the caller — controls exactly when the lock is released: it mutates `admission` while still
    /// holding `state`, then `drop`s it, THEN invokes [`SharedState::report_incident`]. This is the
    /// fix for round 3's finding: an earlier version of this helper took `&mut MutexGuard`, a
    /// borrow the callee could never actually drop, so the caller's own guard remained held for the
    /// sink call's entire duration despite a comment claiming otherwise.
    fn poison_and_report(
        &self,
        mut state: MutexGuard<'_, ManagerState>,
        error: WorkspaceManagerError,
    ) -> Result<(), WorkspaceManagerError> {
        let message = error.to_string();
        if !matches!(state.admission, WorkspaceAdmission::Poisoned { .. }) {
            state.admission = WorkspaceAdmission::Poisoned {
                reason: message.clone(),
            };
        }
        drop(state);
        self.shared.report_incident(&message);
        Err(error)
    }

    /// Acquire a [`CapacityLease`] for `bytes` of the aggregate host-disk ceiling, atomically
    /// checking admission AND capacity in one critical section (round 2's fix for a race round 1
    /// had between an `is_healthy()` check and the capacity update). `Disabled` mode always
    /// refuses (there is no ceiling to lease against at all).
    pub fn acquire_capacity(&self, bytes: u64) -> Result<CapacityLease, CapacityRefusal> {
        if matches!(self.mode, WorkspaceStorageMode::Disabled) {
            return Err(CapacityRefusal::Disabled);
        }
        if bytes == 0 {
            return Err(CapacityRefusal::ZeroBytesRequested);
        }
        let mut state = self.shared.lock_state();
        match &state.admission {
            WorkspaceAdmission::Healthy => {}
            WorkspaceAdmission::Reconciling => return Err(CapacityRefusal::Reconciling),
            WorkspaceAdmission::Poisoned { .. } => return Err(CapacityRefusal::Poisoned),
        }
        let Some(next) = state.capacity_used_bytes.checked_add(bytes) else {
            return Err(CapacityRefusal::Overflow);
        };
        if next > state.capacity_ceiling_bytes {
            return Err(CapacityRefusal::ExhaustedCapacity {
                requested: bytes,
                available: state
                    .capacity_ceiling_bytes
                    .saturating_sub(state.capacity_used_bytes),
            });
        }
        state.capacity_used_bytes = next;
        drop(state);
        Ok(CapacityLease {
            bytes,
            shared: self.shared.clone(),
            released: false,
        })
    }

    /// Bytes currently leased against the aggregate ceiling (test/observability hook).
    pub fn capacity_used_bytes(&self) -> u64 {
        self.shared.lock_state().capacity_used_bytes
    }

    /// The job ids with a currently-checked-out workspace — empty for the entire life of slice 1
    /// (nothing checks a workspace out yet; slice 3's launch-path integration is the first real
    /// caller of the corresponding insert/remove bookkeeping, needed so a later periodic
    /// `list_orphaned_workspaces` sweep can correctly exclude genuinely-live jobs from
    /// reconciliation).
    pub fn active_job_ids(&self) -> BTreeSet<String> {
        self.shared.lock_state().active_job_ids.clone()
    }

    /// CT-007 slice 3: provision a real, checked-out workspace for `job_key`, CONSUMING `capacity`
    /// (Sol's review: a borrowed lease could authorize multiple workspace creations — consuming it
    /// makes "a workspace is checked out" and "its capacity is spoken for" the same fact). Every
    /// PRE-ATTEMPT refusal ([`WorkspaceRequestRefusal`], reachable via
    /// [`WorkspaceProvisionError::Refused`]) hands `capacity` straight back — nothing was
    /// provisioned, so the lease remains exactly as valid as before this call; the caller may
    /// retry or `.release()` it. Only once a REAL `WorkspaceStorage::create_workspace` attempt is
    /// made is `capacity` actually consumed: released back to the pool if `WorkspaceStorage`
    /// itself proved its own rollback succeeded, or abandoned (poisoning this manager, never
    /// silently freed) if the failure is an `UnrecoverableLeak` — the subvolume may still exist.
    pub fn create_workspace(
        &self,
        job_key: &str,
        quota_bytes: u64,
        owner_uid: u32,
        owner_gid: u32,
        capacity: CapacityLease,
    ) -> Result<ManagedWorkspace, WorkspaceProvisionError> {
        // Checked FIRST, before `WrongManager`/`CapacityMismatch`: a `Disabled` manager can never
        // have issued a real capacity lease of its own (`acquire_capacity` always refuses on
        // `Disabled`), so ANY lease passed here already fails the ptr-equality check below —
        // `WrongManager` would always fire first and mask the actually more informative "this
        // manager doesn't do workspaces at all" reason.
        if matches!(self.mode, WorkspaceStorageMode::Disabled) {
            return Err(WorkspaceProvisionError::Refused(
                WorkspaceRequestRefusal::Disabled { capacity },
            ));
        }
        if !Arc::ptr_eq(&self.shared, &capacity.shared) {
            return Err(WorkspaceProvisionError::Refused(
                WorkspaceRequestRefusal::WrongManager { capacity },
            ));
        }
        if capacity.bytes() != quota_bytes {
            let leased = capacity.bytes();
            return Err(WorkspaceProvisionError::Refused(
                WorkspaceRequestRefusal::CapacityMismatch {
                    requested: quota_bytes,
                    leased,
                    capacity,
                },
            ));
        }
        let mut state = self.shared.lock_state();
        match &state.admission {
            WorkspaceAdmission::Healthy => {}
            WorkspaceAdmission::Reconciling => {
                return Err(WorkspaceProvisionError::Refused(
                    WorkspaceRequestRefusal::Reconciling { capacity },
                ))
            }
            WorkspaceAdmission::Poisoned { .. } => {
                return Err(WorkspaceProvisionError::Refused(
                    WorkspaceRequestRefusal::Poisoned { capacity },
                ))
            }
        }
        if state.active_job_ids.contains(job_key) {
            return Err(WorkspaceProvisionError::Refused(
                WorkspaceRequestRefusal::JobAlreadyActive {
                    job_key: job_key.to_string(),
                    capacity,
                },
            ));
        }
        let storage = state
            .storage
            .as_mut()
            .expect("Healthy admission implies storage is Some for an EphemeralDisk manager");
        let result = storage.create_workspace(job_key, quota_bytes, owner_uid, owner_gid);
        self.apply_create_result(state, job_key, capacity, result)
    }

    /// The capacity/active-job-id/admission state transition given the OUTCOME of a
    /// `WorkspaceStorage::create_workspace` attempt — pulled out of [`Self::create_workspace`] so
    /// this transition logic is testable WITHOUT real Btrfs (Sol's review: the safety-critical
    /// post-attempt branches — recoverable-failure release, `UnrecoverableLeak` abandonment,
    /// the impossible-invariant check below — had no always-on coverage, since minting a REAL
    /// `ManagedWorkspace` required privileged storage). Takes the ALREADY-HELD `state` guard (the
    /// real caller holds it continuously from the admission check through the storage call
    /// itself); a test acquires the lock itself and calls this directly with an INJECTED
    /// `Result` (via [`crate::workspace_storage::PreparedWorkspace::for_tests`]), never touching
    /// real storage at all.
    fn apply_create_result(
        &self,
        mut state: MutexGuard<'_, ManagerState>,
        job_key: &str,
        capacity: CapacityLease,
        result: Result<PreparedWorkspace, WorkspaceStorageError>,
    ) -> Result<ManagedWorkspace, WorkspaceProvisionError> {
        match result {
            Ok(prepared) => {
                let inserted = state.active_job_ids.insert(job_key.to_string());
                drop(state);
                let workspace = ManagedWorkspace {
                    job_key: job_key.to_string(),
                    prepared: Some(prepared),
                    capacity: Some(capacity),
                    shared: self.shared.clone(),
                    released: false,
                };
                if !inserted {
                    // Sol's review (round 3): the immediately-preceding check
                    // (`!active_job_ids.contains`) happened under this SAME continuously-held
                    // lock, so this should be structurally impossible — never silently trusted
                    // anyway. This is a HARD failure, NOT `Ok(ManagedWorkspace)` — a caller like
                    // `launch_with` must never mistake a corrupted invariant for authorization to
                    // launch the workload. The real, freshly-created `ManagedWorkspace` capability
                    // is still carried IN the error (not discarded) so the caller can still clean
                    // up the real subvolume via `delete_workspace`, which does not itself refuse
                    // merely because admission is poisoned.
                    let reason = format!(
                        "job {job_key:?} was already in active_job_ids despite the \
                         immediately-preceding locked check — a workspace was just created on \
                         disk for it anyway and must still be cleaned up via delete_workspace"
                    );
                    self.shared.poison(reason.clone());
                    return Err(WorkspaceProvisionError::InternalInvariantViolated {
                        reason,
                        workspace: Box::new(workspace),
                    });
                }
                Ok(workspace)
            }
            Err(error @ WorkspaceStorageError::UnrecoverableLeak { .. }) => {
                drop(state);
                let job_key = job_key.to_string();
                capacity.abandon_with_reason(format!(
                    "workspace provisioning for job {job_key:?} failed unrecoverably: {error} — \
                     capacity retained rather than freed, since the subvolume may still exist"
                ));
                Err(WorkspaceProvisionError::Storage(error))
            }
            Err(error) => {
                drop(state);
                // `WorkspaceStorage::create_workspace`'s own doc guarantees any OTHER failure means
                // its internal rollback already fully cleaned up (no subvolume left behind) — the
                // bytes are genuinely free again.
                capacity.release();
                Err(WorkspaceProvisionError::Storage(error))
            }
        }
    }

    /// CT-007 slice 3: delete a [`ManagedWorkspace`] and release its capacity back to the pool.
    /// Refuses (handing the WHOLE `workspace` straight back, unconsumed — via
    /// [`DeleteWorkspaceError::WrongManager`] — dropping it here would incorrectly trigger its own
    /// abandonment `Drop`) if it was checked out from a DIFFERENT manager instance. On success,
    /// removes `job_key` from the active set and releases the workspace's own [`CapacityLease`].
    /// On a delete/sync failure, the active-job entry is LEFT IN PLACE (this job's workspace state
    /// is now unknown, not confirmed absent), the capacity is abandoned (poisoning this manager —
    /// never silently freed, since the subvolume may still exist and still be consuming it), and
    /// an incident is reported.
    pub fn delete_workspace(
        &self,
        workspace: ManagedWorkspace,
    ) -> Result<(), DeleteWorkspaceError> {
        if !Arc::ptr_eq(&self.shared, &workspace.shared) {
            return Err(DeleteWorkspaceError::WrongManager { workspace });
        }
        let mut workspace = workspace;
        let job_key = workspace.job_key.clone();
        let prepared = workspace
            .prepared
            .take()
            .expect("not yet consumed by a prior delete_workspace call");
        let capacity = workspace
            .capacity
            .take()
            .expect("not yet consumed by a prior delete_workspace call");
        workspace.released = true; // this local binding's own Drop must now be a no-op.

        let mut state = self.shared.lock_state();
        let storage = state
            .storage
            .as_mut()
            .expect("a ManagedWorkspace can only exist for an EphemeralDisk manager with storage");
        let result = storage.delete_workspace(prepared);
        self.apply_delete_result(state, &job_key, capacity, result)
    }

    /// The capacity/active-job-id state transition given the OUTCOME of a
    /// `WorkspaceStorage::delete_workspace` attempt — pulled out of [`Self::delete_workspace`] for
    /// the SAME always-on-testability reason as [`Self::apply_create_result`].
    fn apply_delete_result(
        &self,
        mut state: MutexGuard<'_, ManagerState>,
        job_key: &str,
        capacity: CapacityLease,
        result: Result<(), WorkspaceStorageError>,
    ) -> Result<(), DeleteWorkspaceError> {
        match result {
            Ok(()) => {
                let removed = state.active_job_ids.remove(job_key);
                drop(state);
                // The disk deletion genuinely succeeded — release capacity regardless of the
                // bookkeeping check below (Sol's review: the bytes are proven free either way).
                capacity.release();
                if !removed {
                    // Disk deletion succeeded but this job was never actually tracked as active —
                    // a real bookkeeping corruption, never silently reported as a healthy `Ok(())`.
                    let reason = format!(
                        "internal invariant violated: job {job_key:?} was not in active_job_ids \
                         even though its workspace was just successfully deleted from disk"
                    );
                    self.shared.poison(reason.clone());
                    return Err(DeleteWorkspaceError::InternalInvariantViolated { reason });
                }
                Ok(())
            }
            Err(error) => {
                drop(state);
                capacity.abandon_with_reason(format!(
                    "workspace delete/sync for job {job_key:?} failed: {error} — capacity \
                     retained and the job's active entry left in place pending reconciliation"
                ));
                Err(DeleteWorkspaceError::Storage(error))
            }
        }
    }
}

/// CT-007 slice 3: why [`WorkspaceManager::delete_workspace`] failed. `WrongManager` hands the
/// WHOLE, still-intact [`ManagedWorkspace`] back (nothing was attempted — dropping it here would
/// incorrectly trigger its own abandonment `Drop`). `Storage` means a real delete was ATTEMPTED and
/// FAILED — by this point the workspace and its capacity have ALREADY been consumed internally
/// (capacity abandoned, poisoning this manager), so there is nothing left to hand back.
/// `InternalInvariantViolated` means the delete itself SUCCEEDED (the disk is genuinely clean, and
/// capacity has already been released) but this job's `active_job_ids` bookkeeping was already
/// wrong beforehand — a real corruption, surfaced rather than masked by a healthy `Ok(())`.
#[derive(Debug)]
pub enum DeleteWorkspaceError {
    WrongManager { workspace: ManagedWorkspace },
    Storage(WorkspaceStorageError),
    InternalInvariantViolated { reason: String },
}

impl DeleteWorkspaceError {
    /// Take back the [`ManagedWorkspace`] a `WrongManager` refusal returned. Panics on any other
    /// variant (neither carries one back — see the type's own doc).
    pub fn into_workspace(self) -> ManagedWorkspace {
        match self {
            DeleteWorkspaceError::WrongManager { workspace } => workspace,
            DeleteWorkspaceError::Storage(_)
            | DeleteWorkspaceError::InternalInvariantViolated { .. } => panic!(
                "only DeleteWorkspaceError::WrongManager carries a recoverable ManagedWorkspace \
                 back — every other variant means it was already consumed internally"
            ),
        }
    }
}

impl std::fmt::Display for DeleteWorkspaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeleteWorkspaceError::WrongManager { .. } => write!(
                f,
                "the supplied workspace was not checked out from this manager"
            ),
            DeleteWorkspaceError::Storage(e) => {
                write!(f, "workspace delete/sync failed: {e}")
            }
            DeleteWorkspaceError::InternalInvariantViolated { reason } => {
                write!(f, "internal invariant violated: {reason}")
            }
        }
    }
}

impl std::error::Error for DeleteWorkspaceError {}

/// Boot-time orphan reconciliation: NOTHING is active yet (this runs before the manager ever
/// returns to its caller), so every subvolume discovered under the base is necessarily an orphan
/// left by a previous process instance. Each is deleted and synced; a `SyncPending` result is
/// retried in place (a prior crash may have left a delete committed but its sync unfinished).
/// The base directory of an ENABLED mode (Btrfs or the dormant directory substrate), or `None` for
/// `Disabled` — the two enabled modes share every identity/health check, keyed off this path.
fn enabled_base_dir(mode: &WorkspaceStorageMode) -> Option<&Path> {
    match mode {
        WorkspaceStorageMode::Disabled => None,
        WorkspaceStorageMode::EphemeralDisk { base_dir, .. } => Some(base_dir),
        #[cfg(any(test, feature = "test-support"))]
        WorkspaceStorageMode::DeterministicDirectoryForTests { base_dir, .. } => Some(base_dir),
    }
}

/// Construct the [`WorkspaceStorageBackend`] the given ENABLED mode selects — the Btrfs primitive
/// for `EphemeralDisk`, or the deterministic plain-directory substrate for the dormant
/// `DeterministicDirectoryForTests`. `Disabled` never reaches here (its fast path returns before
/// any backend is opened).
fn open_enabled_backend(
    mode: &WorkspaceStorageMode,
    base_dir: &Path,
) -> Result<WorkspaceStorageBackend, WorkspaceManagerError> {
    match mode {
        WorkspaceStorageMode::EphemeralDisk { .. } => Ok(WorkspaceStorageBackend::Btrfs(
            WorkspaceStorage::open(base_dir).map_err(WorkspaceManagerError::Storage)?,
        )),
        #[cfg(any(test, feature = "test-support"))]
        WorkspaceStorageMode::DeterministicDirectoryForTests { .. } => {
            Ok(WorkspaceStorageBackend::DeterministicDirectoryForTests(
                DirectoryWorkspaceStorage::open(base_dir).map_err(WorkspaceManagerError::Storage)?,
            ))
        }
        WorkspaceStorageMode::Disabled => unreachable!(
            "open_enabled_backend is only called for an enabled mode (Disabled returns earlier)"
        ),
    }
}

fn reconcile_orphans_at_boot(
    storage: &mut WorkspaceStorageBackend,
) -> Result<(), WorkspaceStorageError> {
    let empty_active_set = BTreeSet::new();
    let orphans = storage.list_orphaned_workspaces(&empty_active_set)?;
    for orphan in orphans {
        match storage.delete_orphan(orphan) {
            Ok(()) => {}
            Err(WorkspaceStorageError::SyncPending { subvol_id, .. }) => {
                storage.retry_pending_sync(subvol_id)?;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Acquire a process-lifetime exclusive `flock` on `base_dir` ITSELF — thin wrapper over the
/// shared [`crate::dirlock`] primitive (also used by [`crate::user_namespace`]'s lease directory),
/// mapping [`crate::dirlock::DirLockError`] into this module's own richer error type.
/// `WorkspaceStorage`'s orphan scanner already treats every unrecognized entry under the base as a
/// loud [`WorkspaceStorageError::UnexpectedEntry`] — never a lockfile created inside it.
fn acquire_directory_lock(base_dir: &Path) -> Result<OwnedFd, WorkspaceManagerError> {
    crate::dirlock::acquire_directory_lock(base_dir).map_err(|error| match error {
        crate::dirlock::DirLockError::AlreadyLocked => WorkspaceManagerError::AlreadyLocked {
            base_dir: base_dir.to_path_buf(),
        },
        crate::dirlock::DirLockError::Failed(reason) => WorkspaceManagerError::LockFailed {
            base_dir: base_dir.to_path_buf(),
            reason,
        },
    })
}

/// Verify `path` CURRENTLY resolves to `locked_identity` — the single check both construction and
/// `check_health` build on. A stat failure is reported as a `WorkspaceManagerError::Storage(Io)`; a
/// resolvable-but-mismatched identity is reported as `WorkspaceManagerError::BaseReplaced`.
fn check_path_matches_locked_identity(
    locked_identity: (u64, u64),
    path: &Path,
) -> Result<(), WorkspaceManagerError> {
    let current = path_identity(path).map_err(|io_error| {
        WorkspaceManagerError::Storage(WorkspaceStorageError::Io {
            path: path.to_path_buf(),
            reason: if io_error.kind() == io::ErrorKind::NotFound {
                "path is missing".to_string()
            } else {
                io_error.to_string()
            },
        })
    })?;
    if current != locked_identity {
        return Err(WorkspaceManagerError::BaseReplaced {
            base_dir: path.to_path_buf(),
        });
    }
    Ok(())
}

/// Require BOTH `base_dir` and the `WorkspaceStorage` handle's own canonical `base_dir()` to
/// currently resolve to the SAME `locked_identity` captured from the lock's file descriptor at
/// construction time.
///
/// This closes a genuine TOCTOU Sol's round-4 review found: `acquire_directory_lock` locks and
/// records the identity of whatever `base_dir` resolves to AT THAT INSTANT (call it A), but
/// `WorkspaceStorage::open` independently `canonicalize`s `base_dir` a moment later — if a symlink
/// at `base_dir` were repointed A → B and back to A between those two calls (or during the
/// boot-time reconciliation window that follows), the manager could reconcile and admit against a
/// `WorkspaceStorage` permanently bound to canonical path B while `locked_identity` (and every
/// later `check_health` call, which only ever re-checked `base_dir` itself) still reads A — both
/// checks passing independently while the ADMITTED capability is for a directory different from
/// the one the flock actually protects. Comparing `storage.base_dir()`'s identity too catches this:
/// B's own current identity will never equal `locked_identity` (A).
fn require_locked_identity(
    locked_identity: (u64, u64),
    base_dir: &Path,
    storage_base_dir: &Path,
) -> Result<(), WorkspaceManagerError> {
    check_path_matches_locked_identity(locked_identity, base_dir)?;
    check_path_matches_locked_identity(locked_identity, storage_base_dir)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn unique_suffix() -> u64 {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        NEXT.fetch_add(1, Ordering::Relaxed)
    }

    /// For the manager-STATE tests (lock contention, capacity, poisoning) — these never touch
    /// Btrfs at all (`flock` needs no particular filesystem), so an ordinary tmpfs `/tmp` base is
    /// fine and keeps them fast/hermetic.
    fn test_base(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "myelin-workspace-manager-{tag}-{}-{}",
            std::process::id(),
            unique_suffix()
        ))
    }

    /// For the REAL Btrfs lifecycle tests — `std::env::temp_dir()` (`/tmp`) is `tmpfs` on this
    /// host (confirmed via `mount`/`stat -f /tmp`), which is a real filesystem-type mismatch, not
    /// a privilege gap; `WorkspaceStorage::open` would correctly refuse it as `NotBtrfs` regardless
    /// of capability, silently masking any REAL privilege gap this test actually wants to probe.
    /// Mirrors `workspace_storage.rs`'s own test convention: `$HOME` is on this host's actual
    /// Btrfs root filesystem.
    fn btrfs_test_base(tag: &str) -> PathBuf {
        let mut p = std::env::home_dir().expect("HOME must be set for this test");
        p.push(format!(
            ".local/state/myelin-workspace-manager-tests-{tag}-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        p
    }

    fn recording_sink() -> (IncidentSink, Arc<Mutex<Vec<String>>>) {
        let incidents: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let recorded = incidents.clone();
        let sink: IncidentSink = Arc::new(move |message: &str| {
            recorded.lock().unwrap().push(message.to_string());
        });
        (sink, incidents)
    }

    // ───────────────────────── Disabled mode: zero I/O, always healthy ─────────────────────────

    #[test]
    fn disabled_mode_performs_no_filesystem_io() {
        let bogus_base = test_base("disabled-bogus-parent")
            .join("nested")
            .join("deep");
        let (sink, _incidents) = recording_sink();
        let manager = WorkspaceManager::try_new(WorkspaceStorageMode::Disabled, sink)
            .expect("Disabled mode must never fail to construct");
        assert!(manager.is_healthy());
        assert_eq!(manager.capacity_used_bytes(), 0);
        assert_eq!(
            manager.acquire_capacity(1).unwrap_err(),
            CapacityRefusal::Disabled
        );
        assert!(manager.check_health().is_ok());
        assert!(
            !bogus_base.exists(),
            "Disabled mode must not touch the filesystem at all, even for a path it never saw"
        );
    }

    // ───────────────────── Manager-state logic: always-on, no Btrfs needed ─────────────────────

    /// A second manager over the SAME base directory must refuse immediately (never hang) —
    /// proving the process-lifetime lock actually serializes two would-be owners. Uses the
    /// state-test constructor: `flock` needs no Btrfs, so this runs unconditionally.
    #[test]
    fn a_second_manager_over_the_same_base_refuses_the_lock() {
        let base = test_base("lock-contention");
        let (sink, _incidents) = recording_sink();
        let first = WorkspaceManager::new_for_state_tests(&base, 1 << 30, sink.clone())
            .expect("first manager locks cleanly");
        assert!(first.is_healthy());

        let second = WorkspaceManager::new_for_state_tests(&base, 1 << 30, sink);
        match second {
            Err(WorkspaceManagerError::AlreadyLocked { .. }) => {}
            Err(other) => panic!("expected AlreadyLocked, got a different error: {other}"),
            Ok(_) => panic!("a second manager over the same base must refuse, not succeed"),
        }
        drop(first);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn capacity_leases_are_bounded_and_release_frees_bytes_for_reuse() {
        let base = test_base("capacity-bounds");
        let (sink, _incidents) = recording_sink();
        let manager = WorkspaceManager::new_for_state_tests(&base, 100, sink).unwrap();

        let first = manager.acquire_capacity(60).expect("60 <= 100 must admit");
        assert_eq!(manager.capacity_used_bytes(), 60);
        assert_eq!(
            manager.acquire_capacity(50).unwrap_err(),
            CapacityRefusal::ExhaustedCapacity {
                requested: 50,
                available: 40
            },
            "60 + 50 > 100 must refuse rather than over-admit, reporting the real headroom"
        );
        let second = manager
            .acquire_capacity(40)
            .expect("60 + 40 == 100 must admit exactly at the ceiling");
        assert_eq!(manager.capacity_used_bytes(), 100);
        first.release();
        assert_eq!(manager.capacity_used_bytes(), 40);
        let third = manager
            .acquire_capacity(60)
            .expect("freed capacity must be reusable");
        assert_eq!(manager.capacity_used_bytes(), 100);
        second.release();
        third.release();
        assert_eq!(manager.capacity_used_bytes(), 0);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn acquire_capacity_rejects_zero_byte_requests() {
        let base = test_base("capacity-zero");
        let (sink, _incidents) = recording_sink();
        let manager = WorkspaceManager::new_for_state_tests(&base, 100, sink).unwrap();
        assert_eq!(
            manager.acquire_capacity(0).unwrap_err(),
            CapacityRefusal::ZeroBytesRequested
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// An ABNORMAL drop (no explicit `release()`) must NOT return the bytes to the pool, and must
    /// poison the manager — the exact behavior that keeps a leaked-workspace's capacity from being
    /// silently double-counted as free.
    #[test]
    fn abandoning_a_capacity_lease_poisons_the_manager_and_never_frees_its_bytes() {
        let base = test_base("capacity-abandon");
        let (sink, incidents) = recording_sink();
        let manager = WorkspaceManager::new_for_state_tests(&base, 100, sink).unwrap();
        {
            let lease = manager.acquire_capacity(30).expect("30 <= 100 must admit");
            drop(lease); // abandoned — no `release()` call
        }
        assert_eq!(
            manager.capacity_used_bytes(),
            30,
            "an abandoned lease's bytes must NOT be returned to the pool"
        );
        assert!(
            matches!(manager.admission(), WorkspaceAdmission::Poisoned { .. }),
            "an abandoned capacity lease must poison the manager"
        );
        assert_eq!(
            incidents.lock().unwrap().len(),
            1,
            "exactly one incident must be reported"
        );
        // Poisoned admission must ALSO now refuse any further capacity acquisition, atomically.
        assert_eq!(
            manager.acquire_capacity(1).unwrap_err(),
            CapacityRefusal::Poisoned
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Poisoning is MONOTONIC: a second, different poisoning reason must never overwrite the
    /// first, and admission must never un-poison itself within the life of one manager.
    #[test]
    fn poisoning_is_monotonic_and_never_reverts_to_healthy() {
        let (sink, incidents) = recording_sink();
        let manager = WorkspaceManager::try_new(WorkspaceStorageMode::Disabled, sink).unwrap();
        manager.shared.poison("first reason");
        manager.shared.poison("second reason");
        match manager.admission() {
            WorkspaceAdmission::Poisoned { reason } => assert_eq!(reason, "first reason"),
            other => panic!("expected Poisoned, got {other:?}"),
        }
        assert_eq!(
            incidents.lock().unwrap().len(),
            2,
            "every poisoning attempt reports an incident, even once already poisoned"
        );
        assert!(!manager.is_healthy());
    }

    /// A panicking incident sink must never escape `poison()` — in particular, must never escape
    /// a [`CapacityLease`]'s abnormal `Drop`, which would otherwise abort the process during an
    /// unwind.
    #[test]
    fn a_panicking_incident_sink_never_escapes_poison() {
        let base = test_base("panicking-sink");
        let sink: IncidentSink = Arc::new(|_message: &str| panic!("injected sink panic"));
        let manager = WorkspaceManager::new_for_state_tests(&base, 100, sink).unwrap();
        let lease = manager.acquire_capacity(10).unwrap();
        drop(lease); // triggers Drop -> poison -> the panicking sink; must not propagate here.
        assert!(matches!(
            manager.admission(),
            WorkspaceAdmission::Poisoned { .. }
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// `check_health()`'s identity layer: if the path is deleted out from under the lock, it must
    /// report failure (never silently recreate it) and poison the manager.
    #[test]
    fn check_health_detects_a_deleted_base_without_recreating_it() {
        let base = test_base("health-check-deleted-base");
        let (sink, _incidents) = recording_sink();
        let manager = WorkspaceManager::new_for_state_tests(&base, 1 << 30, sink).unwrap();
        std::fs::remove_dir_all(&base).expect("remove the base out from under the manager");
        let result = manager.check_health();
        assert!(
            result.is_err(),
            "a deleted base must fail health, not be silently recreated"
        );
        assert!(
            !base.exists(),
            "check_health must never recreate the base directory"
        );
        assert!(matches!(
            manager.admission(),
            WorkspaceAdmission::Poisoned { .. }
        ));
    }

    /// `check_health()`'s identity layer: if the base is deleted and a DIFFERENT directory is
    /// created at the exact same path (simulating a delete-and-recreate race), the identity check
    /// must catch the device/inode mismatch and refuse — even though the replacement directory
    /// itself is a perfectly ordinary, accessible directory.
    #[test]
    fn check_health_detects_a_replaced_base_even_when_the_replacement_looks_fine() {
        let base = test_base("health-check-replaced-base");
        let (sink, _incidents) = recording_sink();
        let manager = WorkspaceManager::new_for_state_tests(&base, 1 << 30, sink).unwrap();
        std::fs::remove_dir_all(&base).unwrap();
        std::fs::create_dir_all(&base).expect("recreate a replacement directory at the same path");
        let result = manager.check_health();
        assert!(
            matches!(result, Err(WorkspaceManagerError::BaseReplaced { .. })),
            "a same-path replacement directory must be caught by the identity check, got {result:?}"
        );
        assert!(matches!(
            manager.admission(),
            WorkspaceAdmission::Poisoned { .. }
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Round 4's finding: checking `base_dir`'s identity alone is NOT enough — the manager-owned
    /// `WorkspaceStorage`'s own canonical `base_dir()` can independently diverge from it (a symlink
    /// retargeted and restored around `WorkspaceStorage::open`'s `canonicalize` call). This exercises
    /// [`require_locked_identity`] directly as a pure function over two ordinary directories (no real
    /// Btrfs needed, unlike the full lifecycle scenario Sol described): `locked_identity` matches
    /// `base_dir` but NOT a stand-in "storage base dir" — must be refused as `BaseReplaced`, exactly
    /// as `check_health` now requires of both paths before ever trusting `Healthy`.
    #[test]
    fn require_locked_identity_rejects_a_storage_base_dir_mismatch() {
        let base = test_base("identity-storage-mismatch-locked");
        let other = test_base("identity-storage-mismatch-divergent");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let locked_identity = path_identity(&base).unwrap();

        // `base_dir` itself still matches `locked_identity`...
        assert!(check_path_matches_locked_identity(locked_identity, &base).is_ok());
        // ...but a `WorkspaceStorage` whose own canonical base diverged to `other` must NOT be
        // accepted, even though `base_dir` alone looked fine.
        let result = require_locked_identity(locked_identity, &base, &other);
        assert!(
            matches!(result, Err(WorkspaceManagerError::BaseReplaced { .. })),
            "a diverged storage base dir must be caught even when base_dir itself still matches, \
             got {result:?}"
        );

        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&other);
    }

    /// Round 3's finding #1: `check_health()` must NEVER hold `state`'s lock while invoking the
    /// incident sink — a reentrant sink (one that calls back into the manager) would otherwise
    /// deadlock against itself. The earlier `a_panicking_incident_sink_never_escapes_poison` test
    /// only exercises `SharedState::poison()`'s own call path (via an abandoned `CapacityLease`),
    /// never `check_health()`'s — this test targets `check_health()`'s failure path specifically,
    /// with a sink that calls straight back into `manager.admission()`.
    #[test]
    fn a_reentrant_sink_through_check_health_never_deadlocks() {
        let base = test_base("health-check-reentrant-sink");
        let manager_slot: Arc<std::sync::OnceLock<WorkspaceManager>> =
            Arc::new(std::sync::OnceLock::new());
        // `Weak`, not `Arc::clone` — the sink lives inside `manager_slot`'s own `WorkspaceManager`
        // (via `shared.incident_sink`), so a strong reference here would form
        // `manager_slot -> WorkspaceManager -> shared -> incident_sink -> manager_slot`, an `Arc`
        // cycle neither side would ever release (Sol's round-4 nonblocking hygiene note).
        let slot_for_sink = Arc::downgrade(&manager_slot);
        let sink: IncidentSink = Arc::new(move |_message: &str| {
            if let Some(slot) = slot_for_sink.upgrade() {
                if let Some(manager) = slot.get() {
                    // Reentrant: called FROM check_health()'s own failure path. If check_health()
                    // ever regressed to holding `state`'s lock across this call, `admission()`'s
                    // own `lock_state()` call below would deadlock and this test would hang
                    // instead of completing.
                    let _ = manager.admission();
                }
            }
        });
        let manager = WorkspaceManager::new_for_state_tests(&base, 1 << 30, sink).unwrap();
        manager_slot
            .set(manager)
            .unwrap_or_else(|_| panic!("slot must still be empty"));
        let manager = manager_slot.get().unwrap();
        std::fs::remove_dir_all(&base).expect("remove the base out from under the manager");
        let result = manager.check_health();
        assert!(result.is_err(), "a deleted base must still fail health");
        assert!(matches!(
            manager.admission(),
            WorkspaceAdmission::Poisoned { .. }
        ));
    }

    /// Round 3's finding #2: the process-lifetime directory lock must outlive the `WorkspaceManager`
    /// itself whenever a `CapacityLease` is still outstanding — otherwise dropping the manager while
    /// a lease is held would release the `flock`, letting a SECOND manager falsely lock the same
    /// base and reconcile the first manager's still-logically-live workspace as an orphan.
    #[test]
    fn dropping_the_manager_while_a_lease_is_outstanding_keeps_the_lock_held() {
        let base = test_base("lock-outlives-manager-via-lease");
        let (sink, _incidents) = recording_sink();
        let manager = WorkspaceManager::new_for_state_tests(&base, 1 << 30, sink).unwrap();
        let lease = manager.acquire_capacity(10).unwrap();
        drop(manager); // Would release the flock IF the lock lived on `WorkspaceManager` alone.

        let (second_sink, _second_incidents) = recording_sink();
        let second_attempt = WorkspaceManager::new_for_state_tests(&base, 1 << 30, second_sink);
        match second_attempt {
            Err(WorkspaceManagerError::AlreadyLocked { .. }) => {}
            Err(other) => panic!(
                "expected AlreadyLocked while the first manager's lease is still outstanding, \
                 got a different error: {other:?}"
            ),
            Ok(_) => panic!(
                "expected a second manager over the same base to be refused while the first \
                 manager's lease is still outstanding, but it succeeded"
            ),
        }

        drop(lease); // Releases the lock; cleanup below can now proceed unimpeded.
        let _ = std::fs::remove_dir_all(&base);
    }

    // ──────────── CT-007 slice 3: create_workspace/delete_workspace refusal paths ────────────
    // These all return before ever touching the (possibly-`None`, state-test-only) `storage`
    // field, so they run unconditionally — no real Btrfs needed.

    #[test]
    fn create_workspace_refuses_a_capacity_lease_from_a_different_manager() {
        let base_a = test_base("create-wrong-manager-a");
        let base_b = test_base("create-wrong-manager-b");
        let (sink_a, _log_a) = recording_sink();
        let (sink_b, _log_b) = recording_sink();
        let manager_a = WorkspaceManager::new_for_state_tests(&base_a, 100, sink_a).unwrap();
        let manager_b = WorkspaceManager::new_for_state_tests(&base_b, 100, sink_b).unwrap();
        let capacity_from_b = manager_b.acquire_capacity(10).unwrap();
        let result = manager_a.create_workspace("job-1", 10, 1000, 1000, capacity_from_b);
        let Err(WorkspaceProvisionError::Refused(refusal)) = result else {
            panic!("expected a WrongManager refusal, got {result:?}");
        };
        assert!(matches!(
            refusal,
            WorkspaceRequestRefusal::WrongManager { .. }
        ));
        // The rejected lease is handed straight back (never consumed on a pre-attempt refusal) —
        // the caller can release it cleanly, and `manager_b`'s own capacity accounting must be
        // unaffected either way.
        assert_eq!(manager_b.capacity_used_bytes(), 10);
        refusal.into_capacity().release();
        assert_eq!(manager_b.capacity_used_bytes(), 0);
        assert!(
            manager_b.is_healthy(),
            "handing the lease back and releasing it normally must not poison its real owner"
        );
        let _ = std::fs::remove_dir_all(&base_a);
        let _ = std::fs::remove_dir_all(&base_b);
    }

    #[test]
    fn create_workspace_refuses_a_capacity_lease_whose_bytes_disagree_with_quota() {
        let base = test_base("create-capacity-mismatch");
        let (sink, _log) = recording_sink();
        let manager = WorkspaceManager::new_for_state_tests(&base, 100, sink).unwrap();
        let capacity = manager.acquire_capacity(10).unwrap();
        let result = manager.create_workspace("job-1", 20, 1000, 1000, capacity);
        let Err(WorkspaceProvisionError::Refused(refusal)) = result else {
            panic!("expected a CapacityMismatch refusal, got {result:?}");
        };
        assert!(matches!(
            refusal,
            WorkspaceRequestRefusal::CapacityMismatch {
                requested: 20,
                leased: 10,
                ..
            }
        ));
        assert_eq!(
            manager.capacity_used_bytes(),
            10,
            "a mismatched-quota refusal must not touch the caller's own capacity accounting"
        );
        refusal.into_capacity().release();
        assert_eq!(manager.capacity_used_bytes(), 0);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A `Disabled` manager can never have issued its own capacity lease (`acquire_capacity`
    /// always refuses `Disabled` outright), so `Disabled` is checked BEFORE the ptr-equality
    /// check — otherwise `WrongManager` would always fire first and mask the more informative
    /// "this manager doesn't do workspaces at all" reason. Uses a lease from an unrelated donor
    /// manager to prove `Disabled` wins regardless of the lease's own origin, and that the donor
    /// is unaffected once the returned lease is released back to it.
    #[test]
    fn create_workspace_refuses_when_disabled() {
        let base = test_base("create-disabled-donor");
        let (donor_sink, _donor_log) = recording_sink();
        let donor = WorkspaceManager::new_for_state_tests(&base, 100, donor_sink).unwrap();
        let lease = donor.acquire_capacity(10).unwrap();

        let (sink, _log) = recording_sink();
        let manager = WorkspaceManager::try_new(WorkspaceStorageMode::Disabled, sink).unwrap();
        let result = manager.create_workspace("job-1", 10, 1000, 1000, lease);
        let Err(WorkspaceProvisionError::Refused(refusal)) = result else {
            panic!("expected a Disabled refusal, got {result:?}");
        };
        assert!(matches!(refusal, WorkspaceRequestRefusal::Disabled { .. }));
        refusal.into_capacity().release();
        assert_eq!(donor.capacity_used_bytes(), 0);
        assert!(donor.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn create_workspace_refuses_an_already_active_job_key() {
        let base = test_base("create-already-active");
        let (sink, _log) = recording_sink();
        let manager = WorkspaceManager::new_for_state_tests(&base, 100, sink).unwrap();
        manager
            .shared
            .lock_state()
            .active_job_ids
            .insert("job-1".to_string());
        let capacity = manager.acquire_capacity(10).unwrap();
        let result = manager.create_workspace("job-1", 10, 1000, 1000, capacity);
        let Err(WorkspaceProvisionError::Refused(refusal)) = result else {
            panic!("expected a JobAlreadyActive refusal, got {result:?}");
        };
        assert!(matches!(
            &refusal,
            WorkspaceRequestRefusal::JobAlreadyActive { job_key, .. } if job_key == "job-1"
        ));
        refusal.into_capacity().release();
        assert_eq!(manager.capacity_used_bytes(), 0);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Acquires a real capacity lease WHILE the manager is still healthy, THEN poisons it via a
    /// SEPARATE abandoned lease (mirroring `acquire_capacity`'s own refusal, which cannot itself
    /// be poisoned-and-still-hold-a-valid-lease at the same time) — proving `create_workspace`
    /// hands the already-held, still-valid lease back rather than consuming it into a doomed call.
    #[test]
    fn create_workspace_refuses_when_poisoned() {
        let base = test_base("create-poisoned");
        let (sink, _log) = recording_sink();
        let manager = WorkspaceManager::new_for_state_tests(&base, 100, sink).unwrap();
        let capacity = manager.acquire_capacity(10).unwrap();
        let abandoned = manager.acquire_capacity(10).unwrap();
        drop(abandoned); // an un-released Drop poisons the manager.
        assert!(!manager.is_healthy());
        let result = manager.create_workspace("job-1", 10, 1000, 1000, capacity);
        let Err(WorkspaceProvisionError::Refused(refusal)) = result else {
            panic!("expected a Poisoned refusal, got {result:?}");
        };
        assert!(matches!(refusal, WorkspaceRequestRefusal::Poisoned { .. }));
        // `release()` doesn't check admission state — releasing cleanly here (rather than
        // dropping) avoids a second, redundant "abandoned lease" incident on top of the one the
        // earlier deliberately-abandoned lease already fired.
        refusal.into_capacity().release();
        let _ = std::fs::remove_dir_all(&base);
    }

    // ──── CT-007 slice 3: post-attempt state transitions, always-on via an injected outcome ────
    // Sol's review: these branches previously had NO always-on coverage — both tests that mint a
    // real `ManagedWorkspace` require privileged Btrfs and SKIP on this host. Exercised here via
    // `apply_create_result`/`apply_delete_result` directly, fed a FAKE `Result` (using
    // `PreparedWorkspace::for_tests` where a value is needed), never touching real storage.

    #[test]
    fn apply_create_result_releases_capacity_on_a_recoverable_failure_without_poisoning() {
        let base = test_base("apply-create-recoverable-failure");
        let (sink, _log) = recording_sink();
        let manager = WorkspaceManager::new_for_state_tests(&base, 100, sink).unwrap();
        let capacity = manager.acquire_capacity(10).unwrap();
        let state = manager.shared.lock_state();
        let result = manager.apply_create_result(
            state,
            "job-1",
            capacity,
            Err(WorkspaceStorageError::SubvolumeCreateFailed {
                path: base.join("job-1"),
                stderr: "injected failure".to_string(),
            }),
        );
        assert!(matches!(
            result,
            Err(WorkspaceProvisionError::Storage(
                WorkspaceStorageError::SubvolumeCreateFailed { .. }
            ))
        ));
        assert_eq!(
            manager.capacity_used_bytes(),
            0,
            "a recoverable provisioning failure must release the capacity back to the pool"
        );
        assert!(manager.is_healthy());
        assert!(
            !manager.active_job_ids().contains("job-1"),
            "a failed create must never leave the job key marked active"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn apply_create_result_retains_capacity_and_poisons_on_an_unrecoverable_leak() {
        let base = test_base("apply-create-leak");
        let (sink, _log) = recording_sink();
        let manager = WorkspaceManager::new_for_state_tests(&base, 100, sink).unwrap();
        let capacity = manager.acquire_capacity(10).unwrap();
        let state = manager.shared.lock_state();
        let result = manager.apply_create_result(
            state,
            "job-1",
            capacity,
            Err(WorkspaceStorageError::UnrecoverableLeak {
                path: base.join("job-1"),
                subvol_id: None,
                provisioning_error: "injected provisioning error".to_string(),
                cleanup_error: "injected cleanup error".to_string(),
            }),
        );
        assert!(matches!(
            result,
            Err(WorkspaceProvisionError::Storage(
                WorkspaceStorageError::UnrecoverableLeak { .. }
            ))
        ));
        assert_eq!(
            manager.capacity_used_bytes(),
            10,
            "an UnrecoverableLeak must retain (never silently free) the capacity — the subvolume \
             may still exist"
        );
        assert!(!manager.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn apply_create_result_succeeds_and_tracks_the_job_key() {
        let base = test_base("apply-create-success");
        let (sink, _log) = recording_sink();
        let manager = WorkspaceManager::new_for_state_tests(&base, 100, sink).unwrap();
        let capacity = manager.acquire_capacity(10).unwrap();
        let state = manager.shared.lock_state();
        let fake = PreparedWorkspace::for_tests(base.join("job-1"), 42, base.clone());
        let workspace = manager
            .apply_create_result(state, "job-1", capacity, Ok(fake))
            .expect("an injected Ok outcome must succeed");
        assert_eq!(workspace.job_key(), "job-1");
        assert_eq!(workspace.capacity_bytes(), 10);
        assert!(manager.active_job_ids().contains("job-1"));
        assert!(manager.is_healthy());
        workspace.dismantle_for_tests(); // no real storage to delete_workspace() against.
        assert_eq!(manager.capacity_used_bytes(), 0);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The state-only equivalent of the privileged
    /// `dropping_a_managed_workspace_without_deleting_poisons_the_manager_with_one_incident` test —
    /// exercises the SAME `ManagedWorkspace::drop` abandonment path this seam lets us reach without
    /// real Btrfs, proving exactly ONE incident fires (not a duplicate from `CapacityLease`'s own
    /// `Drop`).
    #[test]
    fn abandoning_a_managed_workspace_from_the_seam_poisons_with_exactly_one_incident() {
        let base = test_base("apply-create-abandon");
        let (sink, incidents) = recording_sink();
        let manager = WorkspaceManager::new_for_state_tests(&base, 100, sink).unwrap();
        let capacity = manager.acquire_capacity(10).unwrap();
        let state = manager.shared.lock_state();
        let fake = PreparedWorkspace::for_tests(base.join("job-1"), 42, base.clone());
        let workspace = manager
            .apply_create_result(state, "job-1", capacity, Ok(fake))
            .unwrap();
        drop(workspace); // simulates a crash: never calls delete_workspace.
        assert!(!manager.is_healthy());
        let log = incidents.lock().unwrap();
        assert_eq!(log.len(), 1, "expected exactly one incident, got {log:?}");
        assert!(log[0].contains("job-1"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn apply_delete_result_abandons_capacity_and_leaves_the_active_entry_on_failure() {
        let base = test_base("apply-delete-failure");
        let (sink, _log) = recording_sink();
        let manager = WorkspaceManager::new_for_state_tests(&base, 100, sink).unwrap();
        manager
            .shared
            .lock_state()
            .active_job_ids
            .insert("job-1".to_string());
        let capacity = manager.acquire_capacity(10).unwrap();
        let state = manager.shared.lock_state();
        let result = manager.apply_delete_result(
            state,
            "job-1",
            capacity,
            // `DeleteFailed` (not `SubvolumeCreateFailed`, which `delete_workspace` can never
            // actually return — Sol's review) models a genuinely reachable delete-path result.
            Err(WorkspaceStorageError::DeleteFailed {
                subvol_id: 42,
                stderr: "injected delete failure".to_string(),
            }),
        );
        assert!(matches!(result, Err(DeleteWorkspaceError::Storage(_))));
        assert_eq!(
            manager.capacity_used_bytes(),
            10,
            "a delete/sync failure must retain (never silently free) the capacity"
        );
        assert!(
            manager.active_job_ids().contains("job-1"),
            "a delete/sync failure must leave the active-job entry in place, not confirmed absent"
        );
        assert!(!manager.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn apply_delete_result_succeeds_and_clears_bookkeeping() {
        let base = test_base("apply-delete-success");
        let (sink, _log) = recording_sink();
        let manager = WorkspaceManager::new_for_state_tests(&base, 100, sink).unwrap();
        manager
            .shared
            .lock_state()
            .active_job_ids
            .insert("job-1".to_string());
        let capacity = manager.acquire_capacity(10).unwrap();
        let state = manager.shared.lock_state();
        let result = manager.apply_delete_result(state, "job-1", capacity, Ok(()));
        assert!(result.is_ok());
        assert_eq!(manager.capacity_used_bytes(), 0);
        assert!(!manager.active_job_ids().contains("job-1"));
        assert!(manager.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Sol's review: a delete that genuinely succeeds on disk but finds `job_key` was NEVER
    /// actually tracked as active is a real bookkeeping corruption — must release capacity (the
    /// disk is proven clean either way) but poison the manager and surface a typed error, never a
    /// silently-healthy `Ok(())`.
    #[test]
    fn apply_delete_result_surfaces_the_invariant_violation_when_the_job_key_was_never_active() {
        let base = test_base("apply-delete-invariant");
        let (sink, _log) = recording_sink();
        let manager = WorkspaceManager::new_for_state_tests(&base, 100, sink).unwrap();
        // Deliberately do NOT insert "job-1" into active_job_ids — simulating the impossible case.
        let capacity = manager.acquire_capacity(10).unwrap();
        let state = manager.shared.lock_state();
        let result = manager.apply_delete_result(state, "job-1", capacity, Ok(()));
        assert!(matches!(
            result,
            Err(DeleteWorkspaceError::InternalInvariantViolated { .. })
        ));
        assert_eq!(
            manager.capacity_used_bytes(),
            0,
            "capacity must still be released — the disk deletion itself genuinely succeeded"
        );
        assert!(!manager.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Sol's review (round 3): the create-side mirror of the delete-invariant test above — the
    /// disk operation genuinely SUCCEEDS, but `job_key` was ALREADY (impossibly) in
    /// `active_job_ids` beforehand. Unlike delete's case, this must NOT be treated the same as an
    /// ordinary failure: the real subvolume now exists on disk, so the typed error carries the
    /// `ManagedWorkspace` capability along specifically so cleanup remains possible — proven here
    /// by actually extracting it and dismantling it.
    #[test]
    fn apply_create_result_surfaces_the_invariant_violation_when_the_job_key_was_already_active() {
        let base = test_base("apply-create-invariant");
        let (sink, _log) = recording_sink();
        let manager = WorkspaceManager::new_for_state_tests(&base, 100, sink).unwrap();
        // Deliberately pre-insert "job-1" — simulating the impossible case the immediately
        // preceding `create_workspace` check is supposed to make unreachable.
        manager
            .shared
            .lock_state()
            .active_job_ids
            .insert("job-1".to_string());
        let capacity = manager.acquire_capacity(10).unwrap();
        let state = manager.shared.lock_state();
        let fake = PreparedWorkspace::for_tests(base.join("job-1"), 42, base.clone());
        let result = manager.apply_create_result(state, "job-1", capacity, Ok(fake));
        assert!(matches!(
            result,
            Err(WorkspaceProvisionError::InternalInvariantViolated { .. })
        ));
        let error = result.unwrap_err();
        assert!(error.to_string().contains("job-1"));
        // Uses the PUBLIC recovery accessor (not a direct field destructure) — pins its contract.
        let workspace = error.into_workspace_after_invariant_violation();
        assert_eq!(
            manager.capacity_used_bytes(),
            10,
            "the capacity must still be tracked as used — the real subvolume now exists on disk"
        );
        assert!(!manager.is_healthy());
        // The real capability is genuinely usable for cleanup, exactly as the error's own
        // contract promises — dismantle it here (no real storage exists in this state-only test
        // to run a real `delete_workspace` against).
        assert_eq!(workspace.job_key(), "job-1");
        workspace.dismantle_for_tests();
        assert_eq!(manager.capacity_used_bytes(), 0);
        let _ = std::fs::remove_dir_all(&base);
    }

    // ───────────────────── Real Btrfs lifecycle: privileged, gated, unfaked ─────────────────────

    /// Probes whether privileged qgroup operations (`CAP_SYS_ADMIN`) — the ones `create_workspace`
    /// actually needs for `qgroup limit`/`subvolume delete` — will succeed here, NOT merely whether
    /// Btrfs quota REPORTING works (round 1's `btrfs_available` conflated the two: a
    /// quota-enforcing host lacking capability would have made the one real lifecycle test FAIL
    /// outright instead of skipping it). Uses
    /// [`crate::workspace_storage::probe_qgroup_privilege`]'s READ-ONLY `qgroup show` check —
    /// deliberately NEVER attempts a real, mutating `create_workspace` just to detect a privilege
    /// gap: this exact function's OWN first version did that, and when the qgroup-limit step
    /// failed for lacking privilege, the best-effort cleanup delete failed for the SAME reason,
    /// producing a genuine `UnrecoverableLeak` — two real, small (8 MiB quota) subvolumes were
    /// left stuck on this host's actual root Btrfs filesystem under
    /// `$HOME/.local/state/myelin-workspace-manager-tests-*` before this was caught and fixed (see
    /// the ledger entry for this slice — they need a privileged `btrfs subvolume delete` to clean
    /// up, which this session could not perform).
    ///
    /// NOTE (Sol's round-4 review): this probe does NOT confirm `CAP_CHOWN`, which
    /// `create_workspace`'s ownership transfer separately requires — a host could pass this probe
    /// and still fail the real lifecycle test on the `chown` step. The one test below that reaches
    /// `create_workspace` for real still exercises that path honestly; this probe only saves it
    /// from a doomed, leak-prone attempt when the more common `CAP_SYS_ADMIN` gap is present.
    fn ephemeral_disk_available(base: &Path) -> bool {
        match WorkspaceStorage::open(base) {
            Ok(_) => {}
            Err(WorkspaceStorageError::NotBtrfs { .. })
            | Err(WorkspaceStorageError::QuotaNotEnforcing { .. }) => {
                eprintln!(
                    "[workspace_manager] SKIP: no Btrfs+enforcing-quota support on this host"
                );
                return false;
            }
            Err(other) => panic!("WorkspaceStorage::open failed unexpectedly: {other}"),
        };
        match crate::workspace_storage::probe_qgroup_privilege(base) {
            Ok(true) => true,
            Ok(false) => {
                eprintln!(
                    "[workspace_manager] SKIP: this test process lacks CAP_SYS_ADMIN for qgroup \
                     operations"
                );
                false
            }
            Err(e) => panic!("qgroup privilege probe failed unexpectedly: {e}"),
        }
    }

    /// Boot-time reconciliation actually deletes a pre-existing orphan (simulating a prior
    /// process's crash mid-job) BEFORE the manager ever reports `Healthy`.
    #[test]
    #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
    fn boot_reconciliation_deletes_a_preexisting_orphan_before_reporting_healthy() {
        let base = btrfs_test_base("boot-reconcile");
        if !ephemeral_disk_available(&base) {
            return;
        }
        {
            let mut storage = WorkspaceStorage::open(&base).unwrap();
            // SAFETY: read-only identity syscalls.
            let (euid, egid) = unsafe { (libc::geteuid(), libc::getegid()) };
            let prepared = storage
                .create_workspace("orphaned-job", 8 << 20, euid, egid)
                .expect("create a real orphan subvolume to reconcile");
            let path = prepared.host_path().to_path_buf();
            std::mem::forget(prepared); // simulate a crash: never call delete_workspace
            assert!(
                path.exists(),
                "the orphan subvolume must really exist before reconciliation"
            );
        }
        let (sink, _incidents) = recording_sink();
        let manager = WorkspaceManager::try_new(
            WorkspaceStorageMode::EphemeralDisk {
                base_dir: base.clone(),
                host_capacity_bytes: 1 << 30,
            },
            sink,
        )
        .expect("boot reconciliation must succeed and open Healthy");
        assert!(manager.is_healthy());
        assert!(
            !base.join("orphaned-job").exists(),
            "boot reconciliation must have deleted the pre-existing orphan"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// `check_health()`'s SECOND layer (the manager-owned `WorkspaceStorage`'s own precondition
    /// re-check) against a REAL Btrfs backend that is genuinely still healthy — proving the happy
    /// path re-validates in place rather than merely never being exercised.
    #[test]
    #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
    fn check_health_succeeds_against_a_real_still_healthy_backend() {
        let base = btrfs_test_base("health-check-real-happy-path");
        if !ephemeral_disk_available(&base) {
            return;
        }
        let (sink, _incidents) = recording_sink();
        let manager = WorkspaceManager::try_new(
            WorkspaceStorageMode::EphemeralDisk {
                base_dir: base.clone(),
                host_capacity_bytes: 1 << 30,
            },
            sink,
        )
        .unwrap();
        assert!(manager.check_health().is_ok());
        assert!(manager.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// CT-007 slice 3's real create/delete lifecycle: a genuine Btrfs workspace is provisioned,
    /// confirmed live (on disk, tracked in `active_job_ids`, its capacity leased), then deleted —
    /// confirming it disappears from disk, its `job_key` leaves the active set, and its capacity
    /// is released back to the pool.
    #[test]
    #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
    fn create_workspace_then_delete_workspace_releases_capacity_and_clears_active_job_id() {
        let base = btrfs_test_base("create-then-delete");
        if !ephemeral_disk_available(&base) {
            return;
        }
        let (sink, _incidents) = recording_sink();
        let manager = WorkspaceManager::try_new(
            WorkspaceStorageMode::EphemeralDisk {
                base_dir: base.clone(),
                host_capacity_bytes: 1 << 30,
            },
            sink,
        )
        .unwrap();
        // SAFETY: read-only identity syscalls.
        let (euid, egid) = unsafe { (libc::geteuid(), libc::getegid()) };
        let capacity = manager.acquire_capacity(8 << 20).unwrap();
        let workspace = manager
            .create_workspace("real-job", 8 << 20, euid, egid, capacity)
            .expect("create_workspace must succeed against a real, privileged Btrfs backend");
        let host_path = workspace.host_path().to_path_buf();
        assert!(
            host_path.exists(),
            "the workspace must really exist on disk"
        );
        assert_eq!(workspace.job_key(), "real-job");
        assert_eq!(workspace.capacity_bytes(), 8 << 20);
        assert!(manager.active_job_ids().contains("real-job"));
        assert_eq!(manager.capacity_used_bytes(), 8 << 20);

        manager
            .delete_workspace(workspace)
            .expect("delete_workspace must succeed against a real, privileged Btrfs backend");
        assert!(
            !host_path.exists(),
            "the workspace subvolume must be gone from disk after delete_workspace"
        );
        assert!(!manager.active_job_ids().contains("real-job"));
        assert_eq!(manager.capacity_used_bytes(), 0);
        assert!(manager.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Dropping a [`ManagedWorkspace`] WITHOUT deleting it (simulating a crash mid-job) must
    /// poison the manager with ONE comprehensive incident naming the job/path — and must NOT
    /// separately fire the generic `CapacityLease` abandonment message on top of it, since the
    /// capacity lease is abandoned together with (not independently of) the workspace it backs.
    #[test]
    #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
    fn dropping_a_managed_workspace_without_deleting_poisons_the_manager_with_one_incident() {
        let base = btrfs_test_base("drop-without-delete");
        if !ephemeral_disk_available(&base) {
            return;
        }
        let (sink, incidents) = recording_sink();
        let manager = WorkspaceManager::try_new(
            WorkspaceStorageMode::EphemeralDisk {
                base_dir: base.clone(),
                host_capacity_bytes: 1 << 30,
            },
            sink,
        )
        .unwrap();
        // SAFETY: read-only identity syscalls.
        let (euid, egid) = unsafe { (libc::geteuid(), libc::getegid()) };
        let capacity = manager.acquire_capacity(8 << 20).unwrap();
        let workspace = manager
            .create_workspace("abandoned-job", 8 << 20, euid, egid, capacity)
            .expect("create_workspace must succeed against a real, privileged Btrfs backend");
        let host_path = workspace.host_path().to_path_buf();
        drop(workspace); // simulates a crash: never calls delete_workspace.

        assert!(
            !manager.is_healthy(),
            "an abandoned ManagedWorkspace must poison the manager"
        );
        let log = incidents.lock().unwrap();
        assert_eq!(
            log.len(),
            1,
            "exactly ONE comprehensive incident must fire — not a second, generic \
             CapacityLease-abandonment message on top of it: {log:?}"
        );
        assert!(log[0].contains("abandoned-job"));
        drop(log);
        // The subvolume itself is still real and still on disk — this manager instance is now
        // poisoned and cannot clean it up.
        assert!(host_path.exists());

        // Sol's review: `remove_dir_all` CANNOT remove a real Btrfs subvolume (it needs a
        // privileged `btrfs subvolume delete`) — the earlier version of this test called it here
        // anyway, leaking a real subvolume on every run on a capable host. Fixed by exercising the
        // ACTUAL claimed crash-recovery path directly: drop this poisoned manager (releasing its
        // lock), open a FRESH manager on the SAME base, and let ITS OWN boot-time reconciliation
        // (already proven by `boot_reconciliation_deletes_a_preexisting_orphan_before_reporting_healthy`)
        // find and delete the orphan for real — only THEN is `remove_dir_all` safe to call on the
        // (now subvolume-free) base directory itself.
        drop(manager);
        let (sink2, _incidents2) = recording_sink();
        let fresh = WorkspaceManager::try_new(
            WorkspaceStorageMode::EphemeralDisk {
                base_dir: base.clone(),
                host_capacity_bytes: 1 << 30,
            },
            sink2,
        )
        .expect("a fresh manager's own boot reconciliation must clean up the orphan and succeed");
        assert!(fresh.is_healthy());
        assert!(
            !host_path.exists(),
            "boot reconciliation must have deleted the abandoned subvolume for real"
        );
        drop(fresh);
        let _ = std::fs::remove_dir_all(&base);
    }
}
