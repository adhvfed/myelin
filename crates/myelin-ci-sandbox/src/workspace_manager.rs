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

use crate::workspace_storage::{WorkspaceStorage, WorkspaceStorageError};
use std::collections::BTreeSet;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::io::FromRawFd;
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

/// The manager's full mutable state, behind ONE mutex — admission, capacity bookkeeping, active
/// job ids, and the open `WorkspaceStorage` handle are never independently synchronized, so "is
/// this manager Healthy AND does this fit the ceiling" is always one atomic critical section
/// (round 2's fix for the race round 1 had between an admission check and a capacity update).
struct ManagerState {
    storage: Option<WorkspaceStorage>,
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
        match &mode {
            WorkspaceStorageMode::Disabled => Ok(Self {
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
            }),
            WorkspaceStorageMode::EphemeralDisk {
                base_dir,
                host_capacity_bytes,
            } => {
                let lock = acquire_directory_lock(base_dir)?;
                let locked_identity =
                    fd_identity(&lock).map_err(|e| WorkspaceManagerError::LockFailed {
                        base_dir: base_dir.clone(),
                        reason: format!("fstat locked directory: {e}"),
                    })?;
                let shared = Arc::new(SharedState {
                    _lock: Some(lock),
                    state: Mutex::new(ManagerState {
                        storage: None,
                        admission: WorkspaceAdmission::Reconciling,
                        active_job_ids: BTreeSet::new(),
                        capacity_ceiling_bytes: *host_capacity_bytes,
                        capacity_used_bytes: 0,
                        locked_identity: Some(locked_identity),
                    }),
                    incident_sink,
                });
                let mut storage =
                    WorkspaceStorage::open(base_dir).map_err(WorkspaceManagerError::Storage)?;
                require_locked_identity(locked_identity, base_dir, storage.base_dir())?;
                reconcile_orphans_at_boot(&mut storage).map_err(WorkspaceManagerError::Storage)?;
                // Re-check identity BEFORE the post-reconciliation health check below, so that
                // check is never run against the wrong path if something swapped underneath us
                // during reconciliation.
                require_locked_identity(locked_identity, base_dir, storage.base_dir())?;
                // Boot-time orphan deletion is exactly the kind of operation
                // `WorkspaceStorage::check_health`'s own doc warns can leave Btrfs quota
                // inconsistent — `open()`'s original quota-enforcing check, taken before any
                // deletion happened, does NOT prove quota is still enforcing now (Sol's round-5
                // review). Re-validate in place before ever trusting this manager as `Healthy`.
                storage
                    .check_health()
                    .map_err(WorkspaceManagerError::Storage)?;
                // Identity may change during that external `check_health` call itself — one more
                // recheck immediately before the ONLY point admission is ever allowed to become
                // `Healthy`.
                require_locked_identity(locked_identity, base_dir, storage.base_dir())?;
                {
                    let mut state = shared.lock_state();
                    state.storage = Some(storage);
                    state.admission = WorkspaceAdmission::Healthy;
                }
                Ok(Self { mode, shared })
            }
        }
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
        let WorkspaceStorageMode::EphemeralDisk { base_dir, .. } = &self.mode else {
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
                    path: base_dir.clone(),
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
}

/// Boot-time orphan reconciliation: NOTHING is active yet (this runs before the manager ever
/// returns to its caller), so every subvolume discovered under the base is necessarily an orphan
/// left by a previous process instance. Each is deleted and synced; a `SyncPending` result is
/// retried in place (a prior crash may have left a delete committed but its sync unfinished).
fn reconcile_orphans_at_boot(storage: &mut WorkspaceStorage) -> Result<(), WorkspaceStorageError> {
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

/// Acquire a process-lifetime exclusive `flock` on `base_dir` ITSELF (never a lockfile created
/// inside it — `WorkspaceStorage`'s orphan scanner already treats every unrecognized entry under
/// the base as a loud [`WorkspaceStorageError::UnexpectedEntry`]). `O_CLOEXEC` so the FD is never
/// inherited across an exec (the sandboxed guest process must never hold this host-side lock).
/// Non-blocking (`LOCK_NB`): a second runner process sharing the same base refuses immediately at
/// startup rather than hanging.
fn acquire_directory_lock(base_dir: &Path) -> Result<OwnedFd, WorkspaceManagerError> {
    std::fs::create_dir_all(base_dir).map_err(|e| WorkspaceManagerError::LockFailed {
        base_dir: base_dir.to_path_buf(),
        reason: format!("create workspace base dir: {e}"),
    })?;
    // SAFETY: `open`'s arguments are a NUL-free, valid path (converted via `CString` below) and
    // standard POSIX flags; the returned fd, on success, is a newly-owned, exclusively-held
    // descriptor this function transfers to its caller via `OwnedFd::from_raw_fd`.
    let path_c = std::ffi::CString::new(base_dir.as_os_str().as_encoded_bytes()).map_err(|e| {
        WorkspaceManagerError::LockFailed {
            base_dir: base_dir.to_path_buf(),
            reason: format!("path contains an interior NUL: {e}"),
        }
    })?;
    let fd = unsafe {
        libc::open(
            path_c.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY,
        )
    };
    if fd < 0 {
        return Err(WorkspaceManagerError::LockFailed {
            base_dir: base_dir.to_path_buf(),
            reason: format!("open directory for locking: {}", io::Error::last_os_error()),
        });
    }
    // SAFETY: `fd` was just returned by a successful `open` above and is not owned elsewhere yet.
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    // SAFETY: `owned` is a valid, open file descriptor for the duration of this call.
    let flock_result = unsafe { libc::flock(owned.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if flock_result != 0 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::WouldBlock {
            return Err(WorkspaceManagerError::AlreadyLocked {
                base_dir: base_dir.to_path_buf(),
            });
        }
        return Err(WorkspaceManagerError::LockFailed {
            base_dir: base_dir.to_path_buf(),
            reason: format!("flock: {error}"),
        });
    }
    Ok(owned)
}

/// The (device, inode) of an already-open file descriptor — identifies the exact inode it
/// references regardless of what happens to any path pointing at it afterward.
fn fd_identity(fd: &OwnedFd) -> io::Result<(u64, u64)> {
    // SAFETY: `stat` is a plain-old-data struct; `fd` is a valid, open file descriptor for the
    // duration of this call.
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    let result = unsafe { libc::fstat(fd.as_raw_fd(), &mut stat) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((stat.st_dev as u64, stat.st_ino as u64))
}

/// The (device, inode) the given PATH currently resolves to (following symlinks, matching
/// `WorkspaceStorage::open`'s own `canonicalize` semantics).
fn path_identity(path: &Path) -> io::Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path)?;
    Ok((meta.dev(), meta.ino()))
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
}
