//! # Explicit user-namespace subsystem (CT-007 slice 2)
//!
//! `gvisor.rs`'s `runsc` invocations today ALWAYS pass `--rootless` — `runsc` installs its OWN
//! user namespace internally, mapping the untrusted workload to OCI `user.uid/gid = 65534/65534`
//! (`UNTRUSTED_UID`/`UNTRUSTED_GID`) via whatever mapping `--rootless` itself sets up. Nothing this
//! process controls decides what HOST identity that guest 65534 actually resolves to.
//!
//! This module provisions an alternative: `RunscInvocationMode::ExplicitUserNamespace`, backed by
//! a [`UserNamespaceLease`] naming a REAL, otherwise-unused subordinate host uid/gid pair (drawn
//! from this host's `/etc/subuid`/`/etc/subgid` ranges) that guest uid/gid 65534 maps to, with
//! guest uid/gid 0 mapping back to THIS process's own real (unprivileged) identity. Slice 3 needs
//! this because a workspace subvolume chowned to a specific host uid must be writable by exactly
//! the container that holds the matching lease — and by no other concurrently-running container.
//!
//! **Design consulted with Sol (gpt-5.6-sol) across 2 design rounds + one live spike against the
//! pinned `runsc` build (release-20260608.0), THEN a full adversarial-review round on this exact
//! implementation, 2026-07-26** (recorded in
//! `planning/system-reviews/2026-06-26/12-ci-track-ledger.md`), as slice 2 of the 4-slice
//! `workspace_storage.rs`/UID-namespace → `gvisor.rs` integration.
//!
//! ## What the spike settled (before any code was written)
//! - `runsc` consumes the OCI `user` namespace + `uidMappings`/`gidMappings` fields NATIVELY —
//!   this process never calls `newuidmap`/`newgidmap` itself. `runsc` invokes them internally
//!   (confirmed empirically: a 2-entry, non-identity mapping was correctly applied with `runsc`
//!   run as an ordinary unprivileged user, no external helper call from this process).
//! - Dropping `--rootless` surfaces a REAL cgroup-setup requirement `runsc`'s OWN cgroupfs manager
//!   cannot satisfy without root. The `-ignore-cgroups` GLOBAL flag makes `runsc` skip that
//!   entirely — WITHOUT weakening the existing `MemoryCgroup` mechanism (`gvisor.rs`).
//!
//! ## What the review round fixed (real bugs, not hypothetical)
//! - **u128-through-`serde_json::Value` precision loss**: boot reconciliation originally parsed
//!   each marker into a `serde_json::Value` first (to peek `schema_version`) before deserializing
//!   the concrete type. Without the `arbitrary_precision` feature, `Value::Number` cannot
//!   losslessly represent a real random `u128` (it silently becomes an approximate `f64`), so
//!   deserializing that back into a `u128` field FAILS outright — meaning almost EVERY real marker
//!   (not the tiny hardcoded `42`/`7` values the original tests used) would have been rejected as
//!   corrupt on every restart. Fixed: a minimal [`SchemaPeek`] struct reads only `schema_version`
//!   directly from the raw string; the full marker is THEN deserialized directly from that SAME
//!   string — `serde_json::Value` never appears anywhere in this path anymore.
//! - **Path-based TOCTOU**: every marker operation (list/create/delete) resolved `leases_dir` by
//!   PATH, even though a directory FD was already held under an exclusive `flock`. A rename-and-
//!   replace of `leases_dir` between the lock and a later operation could let a second allocator
//!   over the NEW directory interact with markers meant for the lock holder's OLD one. Fixed:
//!   marker creation/deletion now go through real `openat`/`unlinkat` against the held lock FD
//!   (`O_NOFOLLOW`); boot-time listing reads via `/proc/self/fd/<lock fd>` (the SAME open file
//!   description, immune to the original path being renamed away); the directory `fsync` targets
//!   the lock FD directly, never a path-based reopen.
//! - **Concurrent `lease()` races**: the ORIGINAL `lease()` released its admission-check lock
//!   before scanning the directory and creating a marker — two real concurrent callers could both
//!   pass the admission check, both pick the same free slot, and the LOSER of the `O_EXCL` race
//!   would poison the WHOLE allocator over an entirely expected collision. Fixed: `lease()` now
//!   holds `SharedState`'s ONE state mutex for its ENTIRE body (select → create → sync) —
//!   allocation is fully serialized process-wide (this subsystem is not a hot path) — AND treats
//!   `AlreadyExists` from the atomic `O_CREAT|O_EXCL` create as an ordinary, expected per-candidate
//!   collision rather than poisoning. Slot selection no longer scans the directory at all (nothing
//!   in this module ever needed it for CORRECTNESS, only as an efficiency shortcut this pool size
//!   does not need); it tries every slot `0..pool_size` in order, skipping ones known-quarantined
//!   in memory, and lets `O_EXCL` be the sole source of truth for which are actually free.
//! - **Boot accepted semantically inconsistent markers**: only `slot < pool_size` was checked — a
//!   surviving marker whose `host_uid`/`host_gid` no longer matches what the CURRENT subordinate
//!   range implies for its own slot number (i.e. the range start changed since the marker was
//!   written) could let a NEW allocation at a DIFFERENT slot silently reissue the exact host
//!   uid/gid a still-quarantined old marker names. Fixed: boot now requires
//!   `marker.host_uid == uid_start + slot && marker.host_gid == gid_start + slot`, poisoning
//!   construction on any mismatch.
//! - **Release was not bound to the lease**: `UserNamespaceQuiescenceProof` carried no identity, so
//!   a proof minted for lease A could release lease B's marker. Fixed: the proof now carries the
//!   SAME [`LeaseNonce`] as the lease it was minted for; `release` refuses (`ProofMismatch`) on any
//!   mismatch, and separately re-reads the durable marker to confirm it STILL carries this exact
//!   lease's nonce/host_uid/host_gid before ever unlinking it (`MarkerMismatch` otherwise, which
//!   poisons the whole allocator — a marker that doesn't match what we expect at our own locked
//!   slot is a global-trust failure, not a per-lease one).
//! - **`UserNamespaceConfig` was publicly forgeable**: every field was `pub`, so any caller could
//!   construct a mapping bypassing the allocator entirely. Fixed: the type is now opaque (private
//!   fields, read accessors) and only mintable from a real [`UserNamespaceLease`] (or a
//!   `#[cfg(test)]`-only constructor).
//! - **Silent entropy-failure degradation**: a `/dev/urandom` read failure during `lease()`
//!   previously fell back to a lease nonce of `0`. Fixed: an entropy failure during `lease()` now
//!   poisons the allocator rather than minting a predictable/colliding identifier.
//!
//! ## The allocator
//! One subordinate uid AND one subordinate gid are leased TOGETHER as a single numbered "slot":
//! slot `i` maps to host uid `uid_range.start + i` and host gid `gid_range.start + i`, for
//! `i` in `0..min(uid_range.count, gid_range.count)` (the two ranges are never assumed equal).
//! Cross-process-safe via the SAME [`crate::dirlock`] exclusive-`flock`-on-a-directory primitive
//! [`crate::workspace_manager`] uses — ONE runner process is the pool's exclusive owner for its
//! lifetime (an explicit, single-host-owner design choice: if multiple runner processes per host
//! ever become a requirement, this needs a short-duration allocation lock plus durable per-slot
//! markers instead of a lifetime directory lock).
//!
//! Each outstanding lease is backed by a DURABLE marker file (`slot-<NNNNNNNNNN>.json`,
//! [`LeaseMarkerV2`], mode `0600`) — written `O_CREAT|O_EXCL|O_NOFOLLOW`, `fsync`'d, then the
//! locked directory FD itself `fsync`'d, so a crash between "decided to allocate" and "process
//! exited" can never lose the record. Boot-time reconciliation NEVER deletes a surviving marker —
//! a runner process can die while its `runsc`, sentry, or gofer descendant remains alive, so the
//! disappearance of OUR OWN lock proves nothing about whether the leased host uid/gid is still in
//! use elsewhere. Every marker found at boot is QUARANTINED (never reissued) and reported as an
//! incident; an unparseable/unrecognized/non-regular/slot-inconsistent entry POISONS THE WHOLE
//! ALLOCATOR (construction fails outright) rather than guessing at its meaning.
//!
//! An abandoned lease (a panic, or a caller simply forgetting to release) quarantines ONLY that
//! one slot plus a mandatory incident — never the whole allocator. `UserNamespaceLease::release`
//! requires a [`UserNamespaceQuiescenceProof`] bound to the SAME lease's nonce; its production
//! constructor does not exist yet (slice 3 adds it, once `launch_with` has the kill/delete/reap +
//! cgroup-quiescence evidence needed to construct one honestly).

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// How a [`crate::gvisor::GvisorBackend`] invokes `runsc` for a single container: the CENTRAL
/// choice every one of `run`/`kill`/`delete` (and any future reconciliation `list`) must consult —
/// no call site independently decides `--rootless` vs. explicit-userns flags/OCI fields after this
/// is threaded through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunscInvocationMode {
    /// Today's ONLY production behavior: `runsc --rootless ...`, no OCI `user` namespace declared
    /// (a doubly-declared userns makes the rootless gofer fork/exec fail). Byte-identical to the
    /// pre-slice-2 command line and OCI JSON — the git-wire path stays on this until deliberately
    /// migrated and drilled on its own.
    Rootless,
    /// `runsc` invoked WITHOUT `--rootless`, WITH `-ignore-cgroups`, and the OCI config carries a
    /// `user` namespace plus the exact two-entry `uidMappings`/`gidMappings` from a
    /// [`UserNamespaceConfig`].
    ExplicitUserNamespace(UserNamespaceConfig),
}

/// The exact two-entry OCI uid/gid mapping an [`RunscInvocationMode::ExplicitUserNamespace`]
/// container gets: container uid/gid 0 (the gofer/sentry's own namespace-root, needed for gofer
/// setup — NOT a privilege grant to the workload, which still runs as 65534) maps to THIS
/// process's real, unprivileged identity; container uid/gid 65534 (the untrusted workload,
/// [`crate::gvisor::UNTRUSTED_UID`]/[`crate::gvisor::UNTRUSTED_GID`]) maps to the leased
/// subordinate host uid/gid.
///
/// OPAQUE by construction (private fields) — only mintable from a real [`UserNamespaceLease`] (via
/// [`UserNamespaceLease::config`]) or, in tests, [`UserNamespaceConfig::for_tests`]. A `pub`
/// struct with `pub` fields would let ANY caller construct a host-identity mapping bypassing the
/// allocator entirely — the allocator's whole job is to be the ONE authority that ever mints one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UserNamespaceConfig {
    runner_uid: u32,
    runner_gid: u32,
    subordinate_uid: u32,
    subordinate_gid: u32,
}

impl UserNamespaceConfig {
    pub fn runner_uid(&self) -> u32 {
        self.runner_uid
    }
    pub fn runner_gid(&self) -> u32 {
        self.runner_gid
    }
    pub fn subordinate_uid(&self) -> u32 {
        self.subordinate_uid
    }
    pub fn subordinate_gid(&self) -> u32 {
        self.subordinate_gid
    }

    #[cfg(test)]
    pub(crate) fn for_tests(
        runner_uid: u32,
        runner_gid: u32,
        subordinate_uid: u32,
        subordinate_gid: u32,
    ) -> Self {
        UserNamespaceConfig {
            runner_uid,
            runner_gid,
            subordinate_uid,
            subordinate_gid,
        }
    }
}

/// A parsed, validated `start:count` range from one line of `/etc/subuid` or `/etc/subgid`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SubordinateRange {
    start: u32,
    count: u32,
}

/// Resolve the CURRENT effective username via `getpwuid_r` (used to match `/etc/subuid`/
/// `/etc/subgid` lines that name a username rather than a numeric uid). `None` if the lookup
/// fails for any reason — callers still accept a purely-numeric-uid match, which is unambiguous
/// and does not depend on this resolution succeeding.
fn effective_username() -> Option<String> {
    let uid = unsafe { libc::geteuid() };
    let mut buf = vec![0u8; 16384];
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: `buf` is a valid, appropriately-sized buffer for the duration of the call; `pwd` and
    // `result` are valid out-parameters. `getpwuid_r` writes into `buf` and, on success, points
    // `result` at `pwd` (never at unrelated memory).
    let ret = unsafe {
        libc::getpwuid_r(
            uid,
            &mut pwd,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut result,
        )
    };
    if ret != 0 || result.is_null() || pwd.pw_name.is_null() {
        return None;
    }
    // SAFETY: `pwd.pw_name` is a valid, NUL-terminated string owned by `buf` (still in scope) —
    // guaranteed by a successful `getpwuid_r`.
    let cstr = unsafe { std::ffi::CStr::from_ptr(pwd.pw_name) };
    cstr.to_str().ok().map(str::to_string)
}

/// Open `path` (`/etc/subuid`/`/etc/subgid` in production; a test fixture in tests) with
/// `O_NOFOLLOW` (refuses a symlink at the leaf atomically — no separate stat-then-open TOCTOU),
/// verify via `fstat` ON THE ALREADY-OPEN FD that it is a regular file, then read its content.
/// When `strict` (the production constructor only — `#[cfg(test)]` fixtures are owned by the
/// test process itself, never root), ALSO requires root ownership and refuses group/other-write
/// access: these are security-authority files this process must trust completely in production; a
/// writable-by-anyone-but-root copy could be tampered with to grant an attacker a colliding range.
fn read_subordinate_file(path: &Path, strict: bool) -> Result<String, UserNamespaceAllocatorError> {
    let malformed = |reason: String| UserNamespaceAllocatorError::SubordinateConfig {
        path: path.to_path_buf(),
        reason,
    };
    let path_c = CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|e| malformed(format!("path contains an interior NUL: {e}")))?;
    // SAFETY: `path_c` is a valid, NUL-terminated path for the duration of this call.
    let fd = unsafe {
        libc::open(
            path_c.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(malformed(format!("open: {}", io::Error::last_os_error())));
    }
    // SAFETY: `fd` was just returned by a successful `open` above and is not owned elsewhere.
    let mut file = unsafe { <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(fd) };
    let meta = file
        .metadata()
        .map_err(|e| malformed(format!("fstat: {e}")))?;
    if !meta.is_file() {
        return Err(malformed("not a regular file".to_string()));
    }
    if strict {
        if meta.uid() != 0 {
            return Err(malformed(format!(
                "must be owned by root (uid 0), got uid {}",
                meta.uid()
            )));
        }
        if meta.mode() & 0o022 != 0 {
            return Err(malformed(format!(
                "must not be group/other-writable (mode {:o})",
                meta.mode() & 0o777
            )));
        }
    }
    let mut content = String::new();
    io::Read::read_to_string(&mut file, &mut content)
        .map_err(|e| malformed(format!("read: {e}")))?;
    Ok(content)
}

/// Whether `value` falls within `range` (`[start, start+count)`).
fn range_contains(range: SubordinateRange, value: u32) -> bool {
    value >= range.start && value < range.start.saturating_add(range.count)
}

/// Fail-closed parse of one `start:count` range for `uid_or_name` out of the file at `path`
/// (`/etc/subuid` or `/etc/subgid` format: `owner:start:count`, one entry per line, `#`-comments
/// and blank lines skipped). A malformed line — on ANY owner, not just a matching one — a zero
/// count, a `start+count` overflow, or (deliberately) MORE THAN ONE matching line all refuse
/// rather than guessing which is authoritative. These are root-maintained security-authority
/// files: an unrelated user cannot edit them to deny this process its own valid range, but a
/// malformed entry ANYWHERE can conceal an overlapping assignment, so it is never silently
/// skipped.
fn parse_subordinate_range(
    path: &Path,
    uid: u32,
    username: Option<&str>,
    strict: bool,
) -> Result<SubordinateRange, UserNamespaceAllocatorError> {
    let malformed = |reason: String| UserNamespaceAllocatorError::SubordinateConfig {
        path: path.to_path_buf(),
        reason,
    };
    let content = read_subordinate_file(path, strict)?;
    let mut matches = Vec::new();
    let mut others = Vec::new();
    for (line_no, raw_line) in content.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(3, ':');
        let (owner, start, count) = match (parts.next(), parts.next(), parts.next()) {
            (Some(o), Some(s), Some(c)) => (o, s, c),
            _ => {
                return Err(malformed(format!(
                    "{path:?} line {}: expected `owner:start:count`, got {line:?}",
                    line_no + 1
                )))
            }
        };
        let start: u32 = start.parse().map_err(|_| {
            malformed(format!(
                "{path:?} line {}: non-numeric start {start:?}",
                line_no + 1
            ))
        })?;
        let count: u32 = count.parse().map_err(|_| {
            malformed(format!(
                "{path:?} line {}: non-numeric count {count:?}",
                line_no + 1
            ))
        })?;
        if count == 0 {
            return Err(malformed(format!(
                "{path:?} line {}: a zero-length subordinate range is refused",
                line_no + 1
            )));
        }
        start.checked_add(count).ok_or_else(|| {
            malformed(format!(
                "{path:?} line {}: start+count overflows u32",
                line_no + 1
            ))
        })?;
        let owner_is_match =
            owner.parse::<u32>().map(|n| n == uid).unwrap_or(false) || Some(owner) == username;
        if owner_is_match {
            matches.push(SubordinateRange { start, count });
        } else {
            others.push(SubordinateRange { start, count });
        }
    }
    match matches.len() {
        0 => Err(UserNamespaceAllocatorError::NoSubordinateEntry {
            path: path.to_path_buf(),
            uid,
        }),
        1 => {
            let selected = matches[0];
            // Sol's review: a syntactically valid entry for ANOTHER owner can still overlap this
            // uid's own selected range — both processes would then map the same host uid/gid,
            // contradicting the "real, otherwise-unused subordinate id" guarantee this allocator's
            // whole design rests on. Checked against every OTHER owner's range, not just this
            // uid's own (already-deduplicated-by-ambiguity-check) entries.
            if let Some(overlap) = others.iter().find(|o| ranges_overlap(selected, **o)) {
                return Err(malformed(format!(
                    "{path:?}: this uid's selected range {selected:?} overlaps another owner's \
                     range {overlap:?} — both would map the same host id, breaking the \
                     \"real, otherwise-unused subordinate id\" guarantee"
                )));
            }
            Ok(selected)
        }
        n => Err(malformed(format!(
            "{path:?}: {n} AMBIGUOUS entries match uid {uid} ({username:?}) — refusing to guess \
             which is authoritative"
        ))),
    }
}

fn ranges_overlap(a: SubordinateRange, b: SubordinateRange) -> bool {
    a.start < b.start.saturating_add(b.count) && b.start < a.start.saturating_add(a.count)
}

/// A process-lifetime-unique identifier distinguishing THIS allocator instance's own outstanding
/// leases from ones recovered (quarantined) at boot from a prior instance. Generated once per
/// process from `/dev/urandom`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerInstanceId(u128);

/// A per-lease unique nonce — the identity a [`UserNamespaceQuiescenceProof`] must match before
/// `release` will act on the corresponding lease's marker.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseNonce(u128);

fn random_u128() -> io::Result<u128> {
    let mut bytes = [0u8; 16];
    // `/dev/urandom` — never falls short on Linux; a read failure is a genuine, surfaced error
    // rather than a silently-weaker fallback.
    let mut file = std::fs::File::open("/dev/urandom")?;
    io::Read::read_exact(&mut file, &mut bytes)?;
    Ok(u128::from_le_bytes(bytes))
}

fn runner_instance_id() -> RunnerInstanceId {
    static CACHED: OnceLock<RunnerInstanceId> = OnceLock::new();
    *CACHED.get_or_init(|| {
        RunnerInstanceId(random_u128().unwrap_or_else(|_| {
            // /dev/urandom is unavailable — extremely abnormal on Linux. Fall back to a value that
            // is at least process-unique (pid + a monotonic counter), never a constant. This
            // fallback is acceptable here (unlike a per-LEASE nonce failure, which now poisons):
            // this id is a diagnostic discriminator, never the sole thing standing between two
            // leases sharing a host identity.
            static FALLBACK_COUNTER: AtomicU64 = AtomicU64::new(0);
            (std::process::id() as u128) << 64
                | FALLBACK_COUNTER.fetch_add(1, Ordering::Relaxed) as u128
        }))
    })
}

const LEASE_MARKER_SCHEMA_V1: u32 = 1;
const LEASE_MARKER_SCHEMA_V2: u32 = 2;

/// The MINIMAL shape read first out of a marker's raw JSON — deliberately just enough to
/// dispatch on `schema_version`, and deliberately NEVER `serde_json::Value` (which cannot
/// losslessly round-trip a `u128` without the `arbitrary_precision` feature this workspace does
/// not enable). The full record is deserialized directly from the SAME raw string afterward.
#[derive(Deserialize)]
struct SchemaPeek {
    schema_version: u32,
}

/// FROZEN legacy shape (CT-007 slice 3) — read-only from this point forward. Every ACTIVE lease
/// this process ever mints is written as [`LeaseMarkerV2`] (`lease()` never writes this shape
/// again); this type exists solely so boot reconciliation can still recognize and quarantine a
/// `schema_version: 1` marker left behind by a PRE-5b.1 binary without misreading it as V2 (or
/// vice versa) — see [`LeaseMarkerV2`]'s own doc for why a new phase shape required a genuine
/// version bump rather than growing this type in place (Sol's review, 2026-07-27: reusing
/// `schema_version: 1` for a 4-variant phase shape would mean an OLDER binary's own 2-variant
/// `LeasePhaseV1` could encounter a `PreparationBound`/`Prepared` JSON value under a
/// `schema_version` it believes it fully understands, producing a confusing "corrupt marker"
/// diagnosis instead of an honest "unrecognized/newer schema" refusal).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LeaseMarkerV1 {
    schema_version: u32,
    lease_nonce: LeaseNonce,
    runner_instance_id: RunnerInstanceId,
    host_uid: u32,
    host_gid: u32,
    created_at_unix_secs: u64,
    phase: LeasePhaseV1,
}

/// FROZEN legacy phase shape — see [`LeaseMarkerV1`]'s doc. Never constructed by production code
/// after this slice; only ever read at boot reconciliation for a marker surviving from before it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum LeasePhaseV1 {
    Allocated,
    Bound {
        container_id: String,
        runsc_root_identity: (u64, u64),
        cgroup_identity: (u64, u64),
    },
}

/// The durable, versioned content of one lease marker file. `#[serde(deny_unknown_fields)]` so an
/// unexpected extra field is treated as corruption rather than silently ignored. CT-007 slice 5b.1
/// bumped this to schema_version 2 (see [`LeaseMarkerV1`]'s doc for why) — `lease()` is the ONE
/// place that mints a brand new marker, and it always writes THIS shape; every other method in
/// `impl UserNamespaceLease` therefore only ever reads/rewrites a V2 marker for any lease `this
/// process itself` issued, never a V1 one (V1 markers are boot-reconciliation-only artifacts from
/// an older binary, always already quarantined and never touched again by any bind/release call).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LeaseMarkerV2 {
    schema_version: u32,
    lease_nonce: LeaseNonce,
    runner_instance_id: RunnerInstanceId,
    host_uid: u32,
    host_gid: u32,
    /// Diagnostic ONLY — never relied on for safety.
    created_at_unix_secs: u64,
    phase: LeasePhaseV2,
}

/// `Allocated`: a slot has been reserved but never associated with any container —
/// [`UserNamespaceLease::release_unused`] is the only way to give one of these up (no runtime
/// evidence to prove, since none ever ran with this identity). `Bound`: durably associated, via
/// [`UserNamespaceLease::bind`], with the SPECIFIC runtime instance (`container_id`, the pinned
/// `runsc` state-root's own (device, inode) identity, and the `MemoryCgroup`'s own (device, inode)
/// identity) about to be exposed to this lease's uid/gid — deliberately a phase enum, not `Option`
/// fields bolted onto one struct, which would let nonsensical partial combinations exist (e.g. a
/// `container_id` with no cgroup identity). [`UserNamespaceLease::release`] is the only way to
/// give one of THESE up, and only against a matching quiescence proof.
///
/// `Bound` specifically tracks the identity of the FINAL settlement/workload runtime this lease is
/// billed against — NOT every preparatory runtime that may have used this identity first. A
/// checkout-bearing job's lease instead visits `PreparationBound` and `Prepared` FIRST (CT-007
/// slice 5b.1), durably recording that the identity was exposed to a preparation runtime before
/// the workload ever runs — a crash between the two runs must never be able to erase that fact and
/// let boot reconciliation mistake the slot for one that was never actually used.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum LeasePhaseV2 {
    Allocated,
    /// Durably associated, via [`UserNamespaceLease::bind_preparation`], with a checkout
    /// PREPARATION runtime (never the billed workload). [`UserNamespaceLease::confirm_prepared`]
    /// is the only way to move on from this phase, and only against a matching preparation
    /// quiescence proof for this EXACT identity.
    PreparationBound {
        container_id: String,
        runsc_root_identity: (u64, u64),
        cgroup_identity: (u64, u64),
    },
    /// The preparation runtime named above was durably proven fully torn down
    /// (`confirm_prepared` verified a matching quiescence proof) before this identity is ever
    /// exposed to the real workload. Retains the preparation runtime's own identity purely as
    /// crash-recovery/audit evidence of what ran — never re-checked once this phase is reached.
    /// [`UserNamespaceLease::bind_workload`] is the only forward transition (to `Bound`);
    /// [`UserNamespaceLease::release_prepared`] is the only way to give this identity up WITHOUT a
    /// workload ever running (e.g. a failure between checkout and workload launch permit
    /// acquisition) — never [`UserNamespaceLease::release_unused`], which requires `Allocated`.
    Prepared {
        preparation_container_id: String,
        preparation_runsc_root_identity: (u64, u64),
        preparation_cgroup_identity: (u64, u64),
    },
    Bound {
        container_id: String,
        runsc_root_identity: (u64, u64),
        cgroup_identity: (u64, u64),
    },
}

fn marker_file_name(slot: u32) -> String {
    format!("slot-{slot:010}.json")
}

/// Parse a leases-directory entry's file name back into its slot index — the ONLY recognized
/// shape; anything else is an unexpected entry that poisons the whole allocator.
fn parse_marker_file_name(name: &str) -> Option<u32> {
    let digits = name.strip_prefix("slot-")?.strip_suffix(".json")?;
    if digits.len() != 10 {
        return None;
    }
    digits.parse().ok()
}

/// Parse a leases-directory entry as a STALE `bind()`-rewrite temp file left behind by a crash
/// between creating `<marker>.tmp` and the `renameat` that would have made it the real marker
/// (see [`rewrite_marker_atomically`]). Recognized SPECIFICALLY so boot reconciliation can
/// conservatively quarantine just the slot it names, rather than treating this entirely expected
/// crash artifact as unrecognized-entry corruption that poisons the whole allocator.
fn parse_stray_tmp_marker_file_name(name: &str) -> Option<u32> {
    let digits = name.strip_prefix("slot-")?.strip_suffix(".json.tmp")?;
    if digits.len() != 10 {
        return None;
    }
    digits.parse().ok()
}

/// The allocator's own admission state. MONOTONIC toward [`UserNamespaceAdmission::Poisoned`] —
/// never resets to `Healthy` once poisoned; a poisoned allocator requires a process restart (which
/// re-runs boot-time marker reconciliation from a clean slate).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UserNamespaceAdmission {
    Healthy,
    Poisoned { reason: String },
}

/// A refusal to lease a slot right now — never a panic, always a typed, inspectable value. Pool
/// exhaustion is a typed refusal, never treated as poisoning — an exhausted pool says nothing bad
/// about the allocator's own bookkeeping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserNamespaceRefusal {
    Poisoned { reason: String },
    PoolExhausted { pool_size: u32 },
}

impl std::fmt::Display for UserNamespaceRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UserNamespaceRefusal::Poisoned { reason } => {
                write!(f, "user-namespace allocator is poisoned: {reason}")
            }
            UserNamespaceRefusal::PoolExhausted { pool_size } => write!(
                f,
                "user-namespace subordinate-id pool ({pool_size} slots) is fully leased"
            ),
        }
    }
}

/// A [`UserNamespaceAllocator`] construction failure — always fatal to starting (or continuing to
/// trust) the allocator.
#[derive(Debug)]
pub enum UserNamespaceAllocatorError {
    AlreadyLocked {
        leases_dir: PathBuf,
    },
    LockFailed {
        leases_dir: PathBuf,
        reason: String,
    },
    /// `/etc/subuid`/`/etc/subgid` parsing/validation failed for a reason OTHER than "no entry for
    /// this uid" — malformed line, zero count, overflow, ambiguous multiple entries, the file
    /// itself is a symlink/unsafe, or this uid's range overlaps another owner's range. A caller
    /// deciding whether to skip gracefully (e.g. a drill run on a host that simply lacks
    /// subordinate-id configuration) must NOT treat this the same as [`Self::NoSubordinateEntry`]
    /// — every one of these indicates something is actually wrong, not merely absent.
    SubordinateConfig {
        path: PathBuf,
        reason: String,
    },
    /// `/etc/subuid`/`/etc/subgid` parsed cleanly but contains no entry naming this process's uid
    /// or effective username — the ONLY subordinate-range failure that means "this host simply
    /// isn't configured for subordinate ids yet," as opposed to "the configuration present is
    /// unsafe/malformed." Split out from [`Self::SubordinateConfig`] (Sol's review) so a caller can
    /// skip gracefully on this ONE variant while still treating every other subordinate-range
    /// failure as a real, fail-closed error.
    NoSubordinateEntry {
        path: PathBuf,
        uid: u32,
    },
    /// `leases_dir` failed this module's own hardening policy (a symlink at the leaf, wrong
    /// owner, or group/other-accessible mode) — checked BEFORE the shared [`crate::dirlock`]
    /// primitive is ever invoked.
    UnsafeLeasesDir {
        leases_dir: PathBuf,
        reason: String,
    },
    /// Boot-time reconciliation found a leases-directory entry it cannot trust: an unrecognized
    /// file name, a non-regular-file entry, unparseable JSON, an unknown `schema_version`, or a
    /// marker whose `host_uid`/`host_gid` no longer matches what the CURRENT subordinate ranges
    /// imply for its own slot (a range-start change since the marker was written). POISONS
    /// CONSTRUCTION ITSELF — never guessed at, never silently skipped.
    CorruptLeaseMarker {
        path: PathBuf,
        reason: String,
    },
    /// This process's own effective uid or gid is 0 (root). Container namespace root (guest uid
    /// 0) maps to the RUNNER's real identity — if that identity were root, the container's own
    /// namespace-root would map directly to host root, defeating the entire point of this
    /// subsystem. Refused unconditionally; there is no hardening posture that makes running this
    /// allocator as root safe.
    PrivilegedRunner {
        euid: u32,
        egid: u32,
    },
    /// The computed pool size (`min(uid_range.count, gid_range.count)`) is smaller than the
    /// caller-supplied `min_pool_size` — the caller's own stated concurrency requirement (e.g.
    /// "must support at least 2 concurrent leases") cannot be met by the subordinate ranges this
    /// host actually has configured.
    PoolTooSmall {
        pool_size: u32,
        required: u32,
    },
}

impl std::fmt::Display for UserNamespaceAllocatorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UserNamespaceAllocatorError::AlreadyLocked { leases_dir } => write!(
                f,
                "user-namespace leases dir {leases_dir:?} is already locked by another process"
            ),
            UserNamespaceAllocatorError::LockFailed { leases_dir, reason } => {
                write!(
                    f,
                    "failed to lock user-namespace leases dir {leases_dir:?}: {reason}"
                )
            }
            UserNamespaceAllocatorError::SubordinateConfig { path, reason } => {
                write!(f, "subordinate-range config error at {path:?}: {reason}")
            }
            UserNamespaceAllocatorError::NoSubordinateEntry { path, uid } => {
                write!(f, "{path:?} has no subordinate-range entry for uid {uid}")
            }
            UserNamespaceAllocatorError::UnsafeLeasesDir { leases_dir, reason } => {
                write!(
                    f,
                    "leases dir {leases_dir:?} failed hardening policy: {reason}"
                )
            }
            UserNamespaceAllocatorError::CorruptLeaseMarker { path, reason } => write!(
                f,
                "corrupt/unrecognized lease marker at {path:?}: {reason} — refusing to start with \
                 an untrustworthy leases directory"
            ),
            UserNamespaceAllocatorError::PrivilegedRunner { euid, egid } => write!(
                f,
                "this process's own euid={euid}/egid={egid} must not be 0 (root) — refusing to \
                 start an allocator whose container-namespace-root mapping would resolve to host \
                 root"
            ),
            UserNamespaceAllocatorError::PoolTooSmall {
                pool_size,
                required,
            } => write!(
                f,
                "the computed pool size ({pool_size} slots) is smaller than the caller's stated \
                 minimum requirement ({required} slots)"
            ),
        }
    }
}

impl std::error::Error for UserNamespaceAllocatorError {}

/// A [`UserNamespaceLease::release`] refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserNamespaceReleaseError {
    /// The supplied [`UserNamespaceQuiescenceProof`] was not minted for THIS lease (a different
    /// lease's proof was supplied). `release` takes `self` by value, so this call still consumes
    /// the lease — there is no retry with the same value; the lease is simply dropped as `release`
    /// returns, quarantining its slot exactly as any other abandoned lease would.
    ProofMismatch,
    /// The durable marker no longer matches this lease's own identity (nonce/host_uid/host_gid) —
    /// a global-trust failure, POISONS the whole allocator.
    MarkerMismatch,
    /// The durable marker genuinely belongs to this lease (base identity matches), but its phase
    /// disagrees with the supplied proof — either the marker was never `Bound` at all, or it is
    /// `Bound` to a different runtime identity than the proof claims. This is an ordinary wrong
    /// proof, NOT corruption: the lease remains outstanding (`self.released` stays `false`) and is
    /// quarantined via ordinary `Drop`, exactly like [`Self::ProofMismatch`] — never a global
    /// poison.
    ProofDisagreesWithMarker,
    /// Unlink/dir-sync had an ambiguous outcome — POISONS the whole allocator (the release outcome
    /// itself is unproven).
    Poisoned,
    /// The marker was durably unlinked (the slot is physically free), but this allocator's own
    /// `active_slots` bookkeeping did not agree the slot was active — an internal invariant was
    /// violated. POISONS the whole allocator, since its in-memory bookkeeping can no longer be
    /// trusted to reflect disk state.
    InternalInvariantViolated { reason: String },
}

impl std::fmt::Display for UserNamespaceReleaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UserNamespaceReleaseError::ProofMismatch => {
                write!(
                    f,
                    "the supplied quiescence proof was not minted for this lease"
                )
            }
            UserNamespaceReleaseError::MarkerMismatch => write!(
                f,
                "the durable marker no longer matches this lease's own identity"
            ),
            UserNamespaceReleaseError::ProofDisagreesWithMarker => write!(
                f,
                "the durable marker belongs to this lease but its phase disagrees with the \
                 supplied proof"
            ),
            UserNamespaceReleaseError::Poisoned => {
                write!(f, "releasing this lease had an ambiguous outcome")
            }
            UserNamespaceReleaseError::InternalInvariantViolated { reason } => {
                write!(f, "internal invariant violated while releasing: {reason}")
            }
        }
    }
}

impl std::error::Error for UserNamespaceReleaseError {}

/// A [`UserNamespaceLease::confirm_prepared`] refusal. Deliberately a DISTINCT type from
/// [`UserNamespaceReleaseError`] (Sol's review, 2026-07-27) rather than a reused one:
/// `UserNamespaceReleaseError::ProofMismatch`'s own doc states the lease is consumed by the call
/// that produced it, which is true for [`UserNamespaceLease::release`] (`self` by value) but false
/// for `confirm_prepared` (`&mut self` — the lease remains usable, e.g. to retry with a corrected
/// proof, or to call [`UserNamespaceLease::release_prepared`] later). Reusing the release-side type
/// would have made that documented consumption claim wrong for half its callers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PreparationConfirmationError {
    /// The supplied [`PreparationQuiescenceProof`] was not minted for THIS lease. `confirm_prepared`
    /// takes `&mut self` — the lease is NOT consumed; the caller may retry with the correct proof.
    ProofMismatch,
    /// The durable marker no longer matches this lease's own identity (nonce/host_uid/host_gid) —
    /// a global-trust failure, POISONS the whole allocator.
    MarkerMismatch,
    /// The durable marker genuinely belongs to this lease, but its phase disagrees with the
    /// supplied proof — either it was never `PreparationBound` at all, or is `PreparationBound` to
    /// a different runtime identity than the proof claims. An ordinary wrong proof, NOT corruption:
    /// the marker is left untouched (a preparation runtime this proof doesn't vouch for may still
    /// be alive), and the caller may retry `confirm_prepared` with the correct proof.
    ProofDisagreesWithMarker,
    /// The durable rewrite to `Prepared` itself had an ambiguous outcome (serialize/write/fsync/
    /// rename failure) — POISONS the whole allocator (the on-disk phase is no longer provably
    /// `PreparationBound`, but also not provably `Prepared`).
    Poisoned,
}

impl std::fmt::Display for PreparationConfirmationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PreparationConfirmationError::ProofMismatch => write!(
                f,
                "the supplied preparation quiescence proof was not minted for this lease"
            ),
            PreparationConfirmationError::MarkerMismatch => write!(
                f,
                "the durable marker no longer matches this lease's own identity"
            ),
            PreparationConfirmationError::ProofDisagreesWithMarker => write!(
                f,
                "the durable marker belongs to this lease but its phase disagrees with the \
                 supplied preparation proof"
            ),
            PreparationConfirmationError::Poisoned => write!(
                f,
                "confirming preparation quiescence had an ambiguous outcome"
            ),
        }
    }
}

impl std::error::Error for PreparationConfirmationError {}

/// Invoked on any critical incident (boot-time quarantine of a surviving marker; an abandoned
/// lease's `Drop`). Never invoked while any internal lock is held, always wrapped in
/// `catch_unwind`.
pub type IncidentSink = Arc<dyn Fn(&str) + Send + Sync>;

fn report_incident_standalone(sink: &IncidentSink, message: &str) {
    let sink = sink.clone();
    let message = message.to_string();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || sink(&message)));
}

struct AllocatorState {
    admission: UserNamespaceAdmission,
    /// Slots known-bad THIS process instance (found stale at boot, or abandoned via `Drop`) — an
    /// observability/bookkeeping set. Reuse-safety does NOT depend on this set alone: the durable
    /// marker file's mere presence on disk is what actually blocks reallocation.
    quarantined_slots: BTreeSet<u32>,
    /// Slots THIS process instance currently holds an outstanding, un-released
    /// [`UserNamespaceLease`] for. Inserted when `lease()` grants a slot; removed when
    /// `release()` succeeds (an abandoned lease moves to `quarantined_slots` instead — it is
    /// removed from here but never returns to being available). Used to distinguish an ORDINARY
    /// `EEXIST` collision during `lease()` (a slot this same process already knows is taken) from
    /// an UNEXPECTED one (a marker this allocator never issued or quarantined has appeared —
    /// meaning the leases directory changed outside this allocator's own bookkeeping, which is a
    /// global-trust failure, not an ordinary collision).
    active_slots: BTreeSet<u32>,
    locked_identity: Option<(u64, u64)>,
}

/// Record `slot` as active, refusing (rather than silently trusting) if it was already there —
/// which `lease()`'s own single-pass-under-one-lock-hold structure means should be impossible: a
/// slot is only ever attempted after `!active_slots.contains(&slot)` is observed absent under the
/// SAME lock hold, with no unlocked window between that check and this call (even though real
/// marker I/O happens in between) that could let another caller race in. Extracted as its own
/// function (rather than inlined in `lease()`) purely to give this "should be impossible"
/// transition a seam for always-on test coverage — `lease()`'s own structure makes the violation
/// itself unreachable through the public API in a single-threaded test.
fn insert_active_slot_checked(active_slots: &mut BTreeSet<u32>, slot: u32) -> Result<(), String> {
    if active_slots.insert(slot) {
        Ok(())
    } else {
        Err(format!(
            "slot {slot} was already marked active in active_slots despite being skipped as \
             taken moments earlier under the same lock hold — a bookkeeping invariant was \
             violated"
        ))
    }
}

struct SharedState {
    /// The process-lifetime exclusive lock on `leases_dir` itself — held here (not directly on
    /// [`UserNamespaceAllocator`]) so it survives as long as EITHER the allocator or any
    /// outstanding lease is alive, whichever drops last. ALSO the capability every marker
    /// operation is now relative to (`openat`/`unlinkat`/`/proc/self/fd/<fd>` listing) — never a
    /// second, independent path-based open of `leases_dir`.
    _lock: OwnedFd,
    state: Mutex<AllocatorState>,
    incident_sink: IncidentSink,
}

impl SharedState {
    /// The ONE way any code in this module accesses `state` — recovers a poisoned
    /// `std::sync::Mutex` rather than propagating a second panic, itself flips admission to
    /// `Poisoned` (without invoking the incident sink for this specific, already-extremely-rare
    /// backstop, mirroring `workspace_manager::SharedState::lock_state`'s documented reasoning).
    fn lock_state(&self) -> MutexGuard<'_, AllocatorState> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                let mut inner = poisoned.into_inner();
                if !matches!(inner.admission, UserNamespaceAdmission::Poisoned { .. }) {
                    inner.admission = UserNamespaceAdmission::Poisoned {
                        reason: "internal allocator-state mutex was poisoned by a prior panic"
                            .to_string(),
                    };
                }
                inner
            }
        }
    }

    fn report_incident(&self, message: &str) {
        report_incident_standalone(&self.incident_sink, message);
    }

    /// Poison the WHOLE allocator (idempotent/monotonic — first reason wins) and report the
    /// incident with the lock already released. Reserved for loss of GLOBAL trust.
    fn poison(&self, reason: impl Into<String>) {
        let reason = reason.into();
        {
            let mut state = self.lock_state();
            if !matches!(state.admission, UserNamespaceAdmission::Poisoned { .. }) {
                state.admission = UserNamespaceAdmission::Poisoned {
                    reason: reason.clone(),
                };
            }
        }
        self.report_incident(&reason);
    }

    /// Quarantine exactly ONE slot (never the whole allocator) and report the incident with the
    /// lock already released. The durable marker for `slot` is left COMPLETELY untouched.
    fn quarantine_slot(&self, slot: u32, reason: impl Into<String>) {
        let reason = reason.into();
        {
            let mut state = self.lock_state();
            state.active_slots.remove(&slot);
            state.quarantined_slots.insert(slot);
        }
        self.report_incident(&reason);
    }

    fn dir_fd(&self) -> RawFd {
        self._lock.as_raw_fd()
    }

    /// A path resolving to the SAME open directory this `SharedState` holds locked, usable ONLY
    /// for read-only listing (`std::fs::read_dir`) — Linux's `/proc/self/fd/<fd>` magic-symlink
    /// binds to the open file description itself, not the original path, so this remains correct
    /// even if `leases_dir`'s original path was renamed/replaced out from under the lock.
    fn listing_path(&self) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}", self.dir_fd()))
    }

    /// `fsync` the locked directory FD directly — never a path-based reopen.
    fn fsync_locked_dir(&self) -> io::Result<()> {
        // SAFETY: `self._lock` is a valid, open file descriptor for the duration of this call.
        let ret = unsafe { libc::fsync(self.dir_fd()) };
        if ret != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

/// `openat(2)` a marker file BY NAME relative to `dir_fd` — `O_NOFOLLOW` refuses a symlinked
/// marker name; `create` selects `O_CREAT|O_EXCL` (fresh allocation) vs. plain `O_RDONLY` (reading
/// back an existing marker to verify it before release).
fn openat_marker(dir_fd: RawFd, name: &str, create: bool) -> io::Result<std::fs::File> {
    let name_c = CString::new(name)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
    let flags = if create {
        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW
    } else {
        libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW
    };
    // SAFETY: `dir_fd` is a valid, open directory FD held by the caller for the duration of this
    // call; `name_c` is a valid, NUL-terminated path. On success this call uniquely owns the
    // returned fd, which `File::from_raw_fd` takes ownership of below.
    let fd = unsafe { libc::openat(dir_fd, name_c.as_ptr(), flags, 0o600) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` was just returned by a successful `openat` above and is not owned elsewhere.
    Ok(unsafe { <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(fd) })
}

/// CT-007 slice 3: durably OVERWRITE an EXISTING marker's content — the `Allocated` → `Bound`
/// phase transition. Deliberately NEVER a truncate-in-place (`O_WRONLY|O_TRUNC` on the existing
/// file): if the process crashes mid-write, an in-place truncate could leave a marker that is
/// PARTIALLY old content and partially new, and — worse — a truncate-then-write leaves a window
/// where the file is SHORTER than either valid version, which a concurrent (or crash-recovery)
/// reader could observe as a plausible-looking but wrong-length JSON fragment. Instead: write the
/// full new content to a FRESH sibling file (`<name>.tmp`, `O_CREAT|O_EXCL` — this lease's `&mut
/// self` borrow already makes a concurrent second bind on the SAME slot impossible at the type
/// level, so `O_EXCL` here is a belt-and-suspenders invariant check, not a concurrency primitive),
/// `fsync` IT (the new content is durable before it can ever become visible at the real name),
/// `renameat` it over `name` (POSIX guarantees an FD-relative rename within the SAME directory is
/// atomic — any reader, including a crash-recovery boot scan, observes EITHER the fully-old or the
/// fully-new content, never a mix), then `fsync` the directory (durability for the rename/unlink
/// of the old name the rename implies).
fn rewrite_marker_atomically(dir_fd: RawFd, name: &str, content: &[u8]) -> io::Result<()> {
    let tmp_name = format!("{name}.tmp");
    let tmp_name_c = CString::new(tmp_name.as_str())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
    // SAFETY: `dir_fd` is a valid, open directory FD held by the caller for the duration of this
    // call; `tmp_name_c` is a valid, NUL-terminated path. `O_EXCL` refuses if a stale `.tmp` file
    // somehow already exists (a prior crash mid-bind) rather than silently overwriting it.
    let fd = unsafe {
        libc::openat(
            dir_fd,
            tmp_name_c.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0o600,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` was just returned by a successful `openat` above and is not owned elsewhere.
    let mut tmp_file = unsafe { <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(fd) };
    let write_and_sync =
        io::Write::write_all(&mut tmp_file, content).and_then(|()| tmp_file.sync_all());
    if let Err(e) = write_and_sync {
        // Best-effort cleanup of the half-written tmp file; the caller treats the overall
        // operation as failed (and poisons) regardless of whether this cleanup itself succeeds.
        let _ = unsafe { libc::unlinkat(dir_fd, tmp_name_c.as_ptr(), 0) };
        return Err(e);
    }
    drop(tmp_file);
    let name_c = CString::new(name)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
    // SAFETY: `dir_fd` is a valid, open directory FD; both name `CString`s are NUL-terminated;
    // renaming within the SAME directory (both paths relative to the identical `dir_fd`).
    let rename_result =
        unsafe { libc::renameat(dir_fd, tmp_name_c.as_ptr(), dir_fd, name_c.as_ptr()) };
    if rename_result != 0 {
        let rename_error = io::Error::last_os_error();
        let _ = unsafe { libc::unlinkat(dir_fd, tmp_name_c.as_ptr(), 0) };
        return Err(rename_error);
    }
    // The rename may already be visible to other readers even if this fsync fails, so a failure
    // here is an ambiguous outcome for the caller to treat as such (poison), never a plain retry.
    // SAFETY: `dir_fd` is a valid, open directory FD held by the caller for the duration of this
    // call.
    let fsync_result = unsafe { libc::fsync(dir_fd) };
    if fsync_result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// `unlinkat(2)` a marker file BY NAME relative to `dir_fd`.
fn unlinkat_marker(dir_fd: RawFd, name: &str) -> io::Result<()> {
    let name_c = CString::new(name)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
    // SAFETY: `dir_fd` is a valid, open directory FD held by the caller for the duration of this
    // call; `name_c` is a valid, NUL-terminated path.
    let ret = unsafe { libc::unlinkat(dir_fd, name_c.as_ptr(), 0) };
    if ret != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// The maximum bytes this module ever trusts a single marker file to contain — a `LeaseMarkerV2`
/// serializes to well under this; anything past it is itself grounds for suspicion, not merely an
/// oversized read.
const MAX_MARKER_BYTES: usize = 4096;

/// The longest `container_id` [`UserNamespaceLease::bind`] will ever accept — well under
/// [`MAX_MARKER_BYTES`] so a valid-length id can never by itself push a serialized `Bound` marker
/// over the limit.
const MAX_CONTAINER_ID_LEN: usize = 256;

/// `true` iff `container_id` is non-empty, at most [`MAX_CONTAINER_ID_LEN`] bytes, and contains
/// only ASCII alphanumerics, `-`, `_`, or `.` — the safe subset every `container_id` this codebase
/// actually generates (`format!("myelin-...-{pid}-{suffix}")`) already falls within. Rejects
/// anything else BEFORE it is ever serialized into a marker or passed to `runsc`.
fn is_valid_container_id(container_id: &str) -> bool {
    !container_id.is_empty()
        && container_id.len() <= MAX_CONTAINER_ID_LEN
        && container_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// Read a marker file BY NAME relative to `dir_fd`, closing the leaf-level TOCTOU Sol's review
/// found: `openat(O_NOFOLLOW|O_NONBLOCK)` FIRST (the name can never resolve through a symlink, and
/// `O_NONBLOCK` means opening a FIFO planted at that name returns immediately instead of blocking
/// this allocator indefinitely), THEN `fstat` the ALREADY-OPEN fd (never a fresh path-based stat)
/// to require a regular file owned by this process's own euid with no group/other access, THEN a
/// BOUNDED read. Used for both boot-time reconciliation reads and `release()`'s pre-unlink
/// verification — the ONE way any code in this module ever reads a marker's content.
fn read_and_verify_marker(dir_fd: RawFd, name: &str) -> io::Result<String> {
    let name_c = CString::new(name)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
    let flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC;
    // SAFETY: `dir_fd` is a valid, open directory FD held by the caller for the duration of this
    // call; `name_c` is a valid, NUL-terminated path.
    let fd = unsafe { libc::openat(dir_fd, name_c.as_ptr(), flags) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` was just returned by a successful `openat` above and is not owned elsewhere.
    let mut file = unsafe { <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(fd) };
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "marker entry is not a regular file",
        ));
    }
    let our_uid = unsafe { libc::geteuid() };
    if meta.uid() != our_uid {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("marker owned by uid {} (expected {our_uid})", meta.uid()),
        ));
    }
    if meta.mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "marker is group/other-accessible",
        ));
    }
    let mut buf = vec![0u8; MAX_MARKER_BYTES + 1];
    let mut total = 0usize;
    loop {
        let n = io::Read::read(&mut file, &mut buf[total..])?;
        if n == 0 {
            break;
        }
        total += n;
        if total >= buf.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "marker exceeds the maximum expected size",
            ));
        }
    }
    buf.truncate(total);
    String::from_utf8(buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
}

/// Thin wrapper around the shared [`crate::dirlock::verify_ancestors_not_writable_by_us`]
/// (also used by [`crate::gvisor`]'s explicit-user-namespace helper-directory preflight), mapping
/// its plain `String` reason into this module's own [`UserNamespaceAllocatorError::UnsafeLeasesDir`].
fn verify_ancestors_not_writable_by_us(dir: &Path) -> Result<(), UserNamespaceAllocatorError> {
    crate::dirlock::verify_ancestors_not_writable_by_us(dir).map_err(|reason| {
        UserNamespaceAllocatorError::UnsafeLeasesDir {
            leases_dir: dir.to_path_buf(),
            reason,
        }
    })
}

/// This module's own hardening policy for `leases_dir`, layered ON TOP of the generic
/// [`crate::dirlock`] primitive (which imposes no such policy — Sol's review: the generic helper
/// need not carry this module's stricter requirements). Sol's review, round 6: the earlier version
/// auto-created the leaf with `create_dir_all` in BOTH strict and non-strict modes before checking
/// anything — internally contradictory in strict mode specifically, since creating a missing leaf
/// requires write access to its own parent, which is exactly what the ancestor-writability check
/// below exists to forbid this process from having; "auto-create succeeded" and "the parent chain
/// is safely non-writable by us" can never both be true at once. It also meant a FAILED strict
/// construction could still leave a freshly-created directory behind. Fixed: when `strict`
/// (production), performs NO MUTATION at all — verifies the ancestor chain FIRST (an unsafe
/// deployment is rejected before ever looking at the leaf), then requires the leaf to ALREADY
/// EXIST as a real (non-symlink) directory, owned by this process's own euid, mode `0700` or
/// stricter; pre-provisioning the leaf becomes the CALLER's (a real deployment's install step's)
/// responsibility. When NOT strict (test-only, via `try_new_for_tests`), retains the original
/// auto-create-at-0700 convenience — test fixtures have no equivalent "install step" and gain
/// nothing from mirroring production's stricter contract here.
fn harden_and_verify_leases_dir(
    dir: &Path,
    strict: bool,
) -> Result<(), UserNamespaceAllocatorError> {
    let unsafe_dir = |reason: String| UserNamespaceAllocatorError::UnsafeLeasesDir {
        leases_dir: dir.to_path_buf(),
        reason,
    };
    if strict {
        verify_ancestors_not_writable_by_us(dir)?;
        return verify_leases_dir_leaf_strict(dir);
    }
    match std::fs::symlink_metadata(dir) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                return Err(unsafe_dir(
                    "the leases directory path is a symlink — refusing to trust a directory \
                     reached through one"
                        .to_string(),
                ));
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            std::fs::create_dir_all(dir)
                .map_err(|e| unsafe_dir(format!("create leases dir: {e}")))?;
            let mut perms = std::fs::metadata(dir)
                .map_err(|e| unsafe_dir(format!("stat freshly-created leases dir: {e}")))?
                .permissions();
            perms.set_mode(0o700);
            std::fs::set_permissions(dir, perms)
                .map_err(|e| unsafe_dir(format!("chmod 0700 leases dir: {e}")))?;
        }
        Err(e) => return Err(unsafe_dir(format!("stat leases dir: {e}"))),
    }
    let meta = std::fs::metadata(dir).map_err(|e| unsafe_dir(format!("stat leases dir: {e}")))?;
    let our_uid = unsafe { libc::geteuid() };
    if meta.uid() != our_uid {
        return Err(unsafe_dir(format!(
            "leases dir is owned by uid {} (expected this process's own euid {our_uid})",
            meta.uid()
        )));
    }
    if meta.mode() & 0o077 != 0 {
        return Err(unsafe_dir(format!(
            "leases dir mode {:o} is group/other-accessible — expected 0700 or stricter",
            meta.mode() & 0o777
        )));
    }
    Ok(())
}

/// The STRICT-mode leaf checks [`harden_and_verify_leases_dir`] applies (pre-existing, real
/// directory, not a symlink, owned by this process's own euid, private mode), pulled out into its
/// own function so a test can exercise them directly against a fixture whose ANCESTORS are not
/// necessarily hardened — the full function's own ancestor check would otherwise refuse first
/// against any fixture a non-privileged test creates under a writable temp directory, proving
/// nothing about the leaf-specific checks this function targets.
fn verify_leases_dir_leaf_strict(dir: &Path) -> Result<(), UserNamespaceAllocatorError> {
    let unsafe_dir = |reason: String| UserNamespaceAllocatorError::UnsafeLeasesDir {
        leases_dir: dir.to_path_buf(),
        reason,
    };
    let meta = std::fs::symlink_metadata(dir).map_err(|e| {
        unsafe_dir(format!(
            "stat leases dir: {e} — the leases directory must be pre-provisioned in production; \
             this preflight does not create it"
        ))
    })?;
    if meta.file_type().is_symlink() {
        return Err(unsafe_dir(
            "the leases directory path is a symlink — refusing to trust a directory reached \
             through one"
                .to_string(),
        ));
    }
    if !meta.is_dir() {
        return Err(unsafe_dir(
            "the leases directory path is not a directory".to_string(),
        ));
    }
    let our_uid = unsafe { libc::geteuid() };
    if meta.uid() != our_uid {
        return Err(unsafe_dir(format!(
            "leases dir is owned by uid {} (expected this process's own euid {our_uid})",
            meta.uid()
        )));
    }
    if meta.mode() & 0o077 != 0 {
        return Err(unsafe_dir(format!(
            "leases dir mode {:o} is group/other-accessible — expected 0700 or stricter",
            meta.mode() & 0o777
        )));
    }
    // Sol's review, round 7: rejecting group/other bits alone still admits `0500`/`0000` — modes
    // this process itself could never actually create/read markers under. The owner must retain
    // full `rwx`.
    if meta.mode() & 0o700 != 0o700 {
        return Err(unsafe_dir(format!(
            "leases dir mode {:o} does not grant this process's own owner bits full rwx — \
             required to create/read lease markers under it",
            meta.mode() & 0o777
        )));
    }
    Ok(())
}

/// Bound to the SAME [`LeaseNonce`] as the lease it is minted for, AND to the specific
/// `container_id`/`runsc_root_identity`/`cgroup_identity` the lease was durably [`bound
/// <UserNamespaceLease::bind>`](UserNamespaceLease::bind) to — `release` refuses (`ProofMismatch`)
/// any proof whose nonce disagrees, and refuses (`ProofDisagreesWithMarker`) any proof whose
/// runtime identity disagrees with what the durable marker's own `Bound` phase records (or whose
/// marker was never `Bound` at all), even if the nonce happens to match — an ordinary wrong proof
/// for a lease that genuinely belongs to the caller, not corruption.
pub struct UserNamespaceQuiescenceProof {
    lease_nonce: LeaseNonce,
    container_id: String,
    runsc_root_identity: (u64, u64),
    cgroup_identity: (u64, u64),
}

/// Why [`UserNamespaceQuiescenceProof::from_runtime_evidence`] refused to mint a proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RuntimeEvidenceError {
    /// The evidence's own namespace is [`crate::gvisor::RuntimeNamespaceQuiescence::Rootless`] — a
    /// rootless run never had a runsc-root identity to check, so it can never honestly back a real
    /// userns lease release.
    RootlessEvidence,
}

impl std::fmt::Display for RuntimeEvidenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuntimeEvidenceError::RootlessEvidence => write!(
                f,
                "the runtime evidence is Rootless — it never had a runsc-root identity to check, \
                 so it cannot back a userns lease release"
            ),
        }
    }
}

impl std::error::Error for RuntimeEvidenceError {}

impl UserNamespaceQuiescenceProof {
    /// CT-007 slice 3, piece 7c: the ONE production constructor — mints a proof directly from a
    /// genuine [`crate::gvisor::RuntimeQuiescenceEvidence`] (itself only ever produced by a
    /// successful `finalize_runtime`), taking the nonce straight from `lease` so a caller can never
    /// supply an arbitrary one (a caller-suppliable nonce would let ANY code mint a proof for ANY
    /// lease it merely holds a reference to, not just the one that was actually checked-torn-down).
    pub(crate) fn from_runtime_evidence(
        lease: &UserNamespaceLease,
        evidence: &crate::gvisor::RuntimeQuiescenceEvidence,
    ) -> Result<Self, RuntimeEvidenceError> {
        let runsc_root_identity = match evidence.namespace() {
            crate::gvisor::RuntimeNamespaceQuiescence::Rootless => {
                return Err(RuntimeEvidenceError::RootlessEvidence);
            }
            crate::gvisor::RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                runsc_root_identity,
            } => runsc_root_identity,
        };
        Ok(UserNamespaceQuiescenceProof {
            lease_nonce: lease.lease_nonce,
            container_id: evidence.container_id().to_string(),
            runsc_root_identity,
            cgroup_identity: evidence.cgroup().cgroup_identity(),
        })
    }

    #[cfg(test)]
    pub(crate) fn assert_for_tests(
        lease_nonce: LeaseNonce,
        container_id: String,
        runsc_root_identity: (u64, u64),
        cgroup_identity: (u64, u64),
    ) -> Self {
        UserNamespaceQuiescenceProof {
            lease_nonce,
            container_id,
            runsc_root_identity,
            cgroup_identity,
        }
    }
}

/// CT-007 slice 5b.1: the checkout-preparation counterpart to [`UserNamespaceQuiescenceProof`] —
/// bound to the SAME [`LeaseNonce`] and to the specific preparation-runtime identity the lease was
/// durably [`bound <UserNamespaceLease::bind_preparation>`](UserNamespaceLease::bind_preparation)
/// to. [`UserNamespaceLease::confirm_prepared`] refuses (`ProofMismatch`) any proof whose nonce
/// disagrees, and refuses (`ProofDisagreesWithMarker`) any proof whose runtime identity disagrees
/// with what the durable marker's own `PreparationBound` phase records — deliberately a DISTINCT
/// type from `UserNamespaceQuiescenceProof`, not a reused/aliased one, so a caller can never pass a
/// real WORKLOAD quiescence proof to `confirm_prepared` (or vice versa) and have it type-check.
pub(crate) struct PreparationQuiescenceProof {
    lease_nonce: LeaseNonce,
    container_id: String,
    runsc_root_identity: (u64, u64),
    cgroup_identity: (u64, u64),
}

impl PreparationQuiescenceProof {
    /// CT-007 slice 5b.1: the ONE production constructor — mints a proof directly from a genuine
    /// [`crate::gvisor::RuntimeQuiescenceEvidence`] for the PREPARATION runtime, taking the nonce
    /// straight from `lease` for the same reason `UserNamespaceQuiescenceProof::from_runtime_evidence`
    /// does (a caller-suppliable nonce would let any code mint a proof for any lease it merely holds
    /// a reference to). Not yet called outside tests — its real production caller (the checkout
    /// preparation runtime's own finalize path) is slice 5b.2's job, mirroring slice 1's own
    /// precedent of leaving a not-yet-consumed production seam unwired rather than forced in early.
    #[allow(dead_code)]
    pub(crate) fn from_runtime_evidence(
        lease: &UserNamespaceLease,
        evidence: &crate::gvisor::RuntimeQuiescenceEvidence,
    ) -> Result<Self, RuntimeEvidenceError> {
        let runsc_root_identity = match evidence.namespace() {
            crate::gvisor::RuntimeNamespaceQuiescence::Rootless => {
                return Err(RuntimeEvidenceError::RootlessEvidence);
            }
            crate::gvisor::RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                runsc_root_identity,
            } => runsc_root_identity,
        };
        Ok(PreparationQuiescenceProof {
            lease_nonce: lease.lease_nonce,
            container_id: evidence.container_id().to_string(),
            runsc_root_identity,
            cgroup_identity: evidence.cgroup().cgroup_identity(),
        })
    }

    #[cfg(test)]
    pub(crate) fn assert_for_tests(
        lease_nonce: LeaseNonce,
        container_id: String,
        runsc_root_identity: (u64, u64),
        cgroup_identity: (u64, u64),
    ) -> Self {
        PreparationQuiescenceProof {
            lease_nonce,
            container_id,
            runsc_root_identity,
            cgroup_identity,
        }
    }
}

/// Why a durable binding transition failed — shared by [`UserNamespaceLease::bind`] (`Allocated`
/// -> `Bound`), [`UserNamespaceLease::bind_preparation`] (`Allocated` -> `PreparationBound`), and
/// [`UserNamespaceLease::bind_workload`] (`Prepared` -> `Bound`). Messages below deliberately say
/// "required source phase" and "target marker," never naming a specific phase on either side,
/// since both differ per caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserNamespaceBindError {
    /// The durable marker no longer matches this lease's own identity in its required source
    /// phase (already past it, corrupted, or externally tampered with) — a global-trust failure,
    /// POISONS the whole allocator.
    MarkerMismatch,
    /// The durable binding rewrite itself had an ambiguous outcome (serialize/write/fsync/rename
    /// failure) — POISONS the whole allocator (the on-disk phase is no longer provably its
    /// required source phase, but also not provably its target phase).
    Poisoned,
    /// `container_id` is empty, exceeds [`MAX_CONTAINER_ID_LEN`], or contains a character outside
    /// the safe subset — a caller bug, NOT a global-trust failure. Refused before touching disk at
    /// all: does NOT poison, and the lease is left exactly as it was (still in its required source
    /// phase, still usable — the caller may retry with a corrected id).
    InvalidContainerId,
    /// The serialized target marker would exceed [`MAX_MARKER_BYTES`] — writing it would produce
    /// a marker `read_and_verify_marker`/boot reconciliation can never parse back, permanently
    /// bricking this slot. Refused before any disk write is attempted: does NOT poison, and the
    /// lease is left exactly as it was (still in its required source phase, still usable).
    MarkerTooLarge,
}

impl std::fmt::Display for UserNamespaceBindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UserNamespaceBindError::MarkerMismatch => write!(
                f,
                "the durable marker no longer matches this lease's own identity in its \
                 required source phase"
            ),
            UserNamespaceBindError::Poisoned => {
                write!(f, "the durable binding transition had an ambiguous outcome")
            }
            UserNamespaceBindError::InvalidContainerId => write!(
                f,
                "container_id is empty, too long, or contains a character outside the safe subset"
            ),
            UserNamespaceBindError::MarkerTooLarge => write!(
                f,
                "the serialized target marker would exceed the maximum marker size"
            ),
        }
    }
}

impl std::error::Error for UserNamespaceBindError {}

/// A non-`Clone` hold on exactly one subordinate uid/gid slot. See this module's doc for the full
/// lifecycle contract.
pub struct UserNamespaceLease {
    slot: u32,
    host_uid: u32,
    host_gid: u32,
    runner_uid: u32,
    runner_gid: u32,
    lease_nonce: LeaseNonce,
    runner_instance_id: RunnerInstanceId,
    shared: Arc<SharedState>,
    released: bool,
}

impl std::fmt::Debug for UserNamespaceLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UserNamespaceLease")
            .field("slot", &self.slot)
            .field("host_uid", &self.host_uid)
            .field("host_gid", &self.host_gid)
            .field("released", &self.released)
            .finish()
    }
}

impl UserNamespaceLease {
    pub fn host_uid(&self) -> u32 {
        self.host_uid
    }

    pub fn host_gid(&self) -> u32 {
        self.host_gid
    }

    /// The exact two-entry OCI mapping this lease implies, ready for
    /// [`RunscInvocationMode::ExplicitUserNamespace`].
    pub fn config(&self) -> UserNamespaceConfig {
        UserNamespaceConfig {
            runner_uid: self.runner_uid,
            runner_gid: self.runner_gid,
            subordinate_uid: self.host_uid,
            subordinate_gid: self.host_gid,
        }
    }

    #[cfg(test)]
    pub(crate) fn nonce_for_tests(&self) -> LeaseNonce {
        self.lease_nonce
    }

    /// CT-007 slice 3: durably transition this lease from `Allocated` to `Bound`, associating it
    /// with the SPECIFIC runtime instance (`container_id`, the pinned `runsc` state-root's own
    /// (device, inode) identity, and the `MemoryCgroup`'s own (device, inode) identity) about to be
    /// exposed to this lease's uid/gid — MUST succeed BEFORE `runsc` ever execs with this identity.
    /// Without a durable `Bound` record, a crash between spawn and settlement would leave no
    /// evidence of which runtime instance this lease's identity was exposed to, making
    /// [`Self::release`]'s whole "the runtime this lease was exposed to is confirmed torn down"
    /// claim unverifiable at boot-recovery time.
    ///
    /// Re-reads the durable marker first, requiring it to STILL be `Allocated` and match this
    /// lease's own identity (schema/nonce/runner/host_uid/host_gid) — refuses (`MarkerMismatch`,
    /// POISONING the whole allocator) if not, since that means the on-disk state no longer agrees
    /// with what this in-memory lease believes. Rewrites the marker via
    /// [`rewrite_marker_atomically`] (write-tmp-then-rename — never a truncate-in-place, which
    /// could leave a crash-recovery reader observing a corrupt, partially-written marker).
    pub fn bind(
        &mut self,
        container_id: String,
        runsc_root_identity: (u64, u64),
        cgroup_identity: (u64, u64),
    ) -> Result<(), UserNamespaceBindError> {
        if !is_valid_container_id(&container_id) {
            // A caller bug, not a global-trust failure — nothing has touched disk yet, so the
            // lease is left exactly as it was and the caller may retry with a corrected id.
            return Err(UserNamespaceBindError::InvalidContainerId);
        }
        let name = marker_file_name(self.slot);
        let current = read_and_verify_marker(self.shared.dir_fd(), &name)
            .ok()
            .and_then(|content| serde_json::from_str::<LeaseMarkerV2>(&content).ok())
            .filter(|marker| {
                marker.schema_version == LEASE_MARKER_SCHEMA_V2
                    && marker.lease_nonce == self.lease_nonce
                    && marker.runner_instance_id == self.runner_instance_id
                    && marker.host_uid == self.host_uid
                    && marker.host_gid == self.host_gid
                    && marker.phase == LeasePhaseV2::Allocated
            });
        let Some(marker) = current else {
            self.released = true; // avoid a redundant abandonment incident from Drop on top.
            self.shared.poison(format!(
                "binding slot {} (host_uid={}): the durable marker no longer matches this \
                 lease's own identity in the Allocated phase — treating as a global-trust failure",
                self.slot, self.host_uid
            ));
            return Err(UserNamespaceBindError::MarkerMismatch);
        };
        let bound_marker = LeaseMarkerV2 {
            phase: LeasePhaseV2::Bound {
                container_id,
                runsc_root_identity,
                cgroup_identity,
            },
            ..marker
        };
        let marker_json = match serde_json::to_string(&bound_marker) {
            Ok(json) => json,
            Err(e) => {
                self.released = true;
                self.shared.poison(format!(
                    "binding slot {} (host_uid={}): failed to serialize the Bound marker: {e}",
                    self.slot, self.host_uid
                ));
                return Err(UserNamespaceBindError::Poisoned);
            }
        };
        if marker_json.len() > MAX_MARKER_BYTES {
            // Refused BEFORE any disk write: the marker is still `Allocated` and untouched, so
            // the lease is left exactly as it was (not poisoned, not consumed) — a caller bug
            // (an oversized/highly-escaped container_id), never a global-trust failure.
            return Err(UserNamespaceBindError::MarkerTooLarge);
        }
        match rewrite_marker_atomically(self.shared.dir_fd(), &name, marker_json.as_bytes()) {
            Ok(()) => Ok(()),
            Err(e) => {
                self.released = true;
                self.shared.poison(format!(
                    "binding slot {} (host_uid={}): failed to durably rewrite the marker to \
                     Bound ({e}) — the on-disk phase is now ambiguous",
                    self.slot, self.host_uid
                ));
                Err(UserNamespaceBindError::Poisoned)
            }
        }
    }

    /// CT-007 slice 5b.1: durably transition this lease from `Allocated` to `PreparationBound`,
    /// associating it with a checkout PREPARATION runtime instance — the counterpart to [`Self::bind`]
    /// for the preparation phase of a checkout-bearing job. Deliberately a SEPARATE method (not a
    /// parameterized `bind`) so the already-hardened `bind`/`release`/`release_unused` call sites for
    /// the ordinary (non-checkout) single-runtime path stay byte-for-byte untouched. Same precondition
    /// as `bind` (current durable phase must be `Allocated`) and the same refusal/poisoning shape.
    pub(crate) fn bind_preparation(
        &mut self,
        container_id: String,
        runsc_root_identity: (u64, u64),
        cgroup_identity: (u64, u64),
    ) -> Result<(), UserNamespaceBindError> {
        if !is_valid_container_id(&container_id) {
            return Err(UserNamespaceBindError::InvalidContainerId);
        }
        let name = marker_file_name(self.slot);
        let current = read_and_verify_marker(self.shared.dir_fd(), &name)
            .ok()
            .and_then(|content| serde_json::from_str::<LeaseMarkerV2>(&content).ok())
            .filter(|marker| {
                marker.schema_version == LEASE_MARKER_SCHEMA_V2
                    && marker.lease_nonce == self.lease_nonce
                    && marker.runner_instance_id == self.runner_instance_id
                    && marker.host_uid == self.host_uid
                    && marker.host_gid == self.host_gid
                    && marker.phase == LeasePhaseV2::Allocated
            });
        let Some(marker) = current else {
            self.released = true;
            self.shared.poison(format!(
                "binding slot {} (host_uid={}) to a preparation runtime: the durable marker no \
                 longer matches this lease's own identity in the Allocated phase — treating as a \
                 global-trust failure",
                self.slot, self.host_uid
            ));
            return Err(UserNamespaceBindError::MarkerMismatch);
        };
        let bound_marker = LeaseMarkerV2 {
            phase: LeasePhaseV2::PreparationBound {
                container_id,
                runsc_root_identity,
                cgroup_identity,
            },
            ..marker
        };
        let marker_json = match serde_json::to_string(&bound_marker) {
            Ok(json) => json,
            Err(e) => {
                self.released = true;
                self.shared.poison(format!(
                    "binding slot {} (host_uid={}) to a preparation runtime: failed to serialize \
                     the PreparationBound marker: {e}",
                    self.slot, self.host_uid
                ));
                return Err(UserNamespaceBindError::Poisoned);
            }
        };
        if marker_json.len() > MAX_MARKER_BYTES {
            return Err(UserNamespaceBindError::MarkerTooLarge);
        }
        match rewrite_marker_atomically(self.shared.dir_fd(), &name, marker_json.as_bytes()) {
            Ok(()) => Ok(()),
            Err(e) => {
                self.released = true;
                self.shared.poison(format!(
                    "binding slot {} (host_uid={}) to a preparation runtime: failed to durably \
                     rewrite the marker to PreparationBound ({e}) — the on-disk phase is now \
                     ambiguous",
                    self.slot, self.host_uid
                ));
                Err(UserNamespaceBindError::Poisoned)
            }
        }
    }

    /// CT-007 slice 5b.1: durably transition this lease from `PreparationBound` to `Prepared`,
    /// verifying `proof` was minted for THIS lease AND for the SAME preparation-runtime identity the
    /// durable marker's `PreparationBound` phase records — the preparation-phase counterpart to
    /// [`Self::release`]'s proof verification, except this REWRITES the marker (retaining the
    /// identity as evidence) instead of unlinking it, since the lease's identity is about to be
    /// reused by the real workload, not given back to the pool.
    pub(crate) fn confirm_prepared(
        &mut self,
        proof: PreparationQuiescenceProof,
    ) -> Result<(), PreparationConfirmationError> {
        if proof.lease_nonce != self.lease_nonce {
            return Err(PreparationConfirmationError::ProofMismatch);
        }
        let name = marker_file_name(self.slot);
        let marker = read_and_verify_marker(self.shared.dir_fd(), &name)
            .ok()
            .and_then(|content| serde_json::from_str::<LeaseMarkerV2>(&content).ok());
        let base_identity_matches = marker.as_ref().is_some_and(|marker| {
            marker.schema_version == LEASE_MARKER_SCHEMA_V2
                && marker.lease_nonce == self.lease_nonce
                && marker.runner_instance_id == self.runner_instance_id
                && marker.host_uid == self.host_uid
                && marker.host_gid == self.host_gid
        });
        if !base_identity_matches {
            self.released = true;
            self.shared.poison(format!(
                "confirming preparation quiescence for slot {} (host_uid={}): the durable marker \
                 no longer matches this lease's own identity (schema/nonce/runner/host_uid/ \
                 host_gid) — treating as a global-trust failure",
                self.slot, self.host_uid
            ));
            return Err(PreparationConfirmationError::MarkerMismatch);
        }
        let Some(marker) = marker else {
            unreachable!("base_identity_matches is only true when marker is Some");
        };
        let phase_matches_proof = marker.phase
            == LeasePhaseV2::PreparationBound {
                container_id: proof.container_id.clone(),
                runsc_root_identity: proof.runsc_root_identity,
                cgroup_identity: proof.cgroup_identity,
            };
        if !phase_matches_proof {
            // Genuinely belongs to this lease, but was never PreparationBound at all, or is
            // PreparationBound to a different identity than the proof claims — an ordinary wrong
            // proof, not corruption. Leave `self.released` false so Drop quarantines only this one
            // lease (never a global poison) — and, critically, do NOT touch the marker: a
            // preparation runtime this proof does not vouch for may still be alive.
            return Err(PreparationConfirmationError::ProofDisagreesWithMarker);
        }
        let (
            preparation_container_id,
            preparation_runsc_root_identity,
            preparation_cgroup_identity,
        ) = match &marker.phase {
            LeasePhaseV2::PreparationBound {
                container_id,
                runsc_root_identity,
                cgroup_identity,
            } => (container_id.clone(), *runsc_root_identity, *cgroup_identity),
            _ => unreachable!("phase_matches_proof only true for PreparationBound"),
        };
        let prepared_marker = LeaseMarkerV2 {
            phase: LeasePhaseV2::Prepared {
                preparation_container_id,
                preparation_runsc_root_identity,
                preparation_cgroup_identity,
            },
            ..marker
        };
        let marker_json = match serde_json::to_string(&prepared_marker) {
            Ok(json) => json,
            Err(e) => {
                self.released = true;
                self.shared.poison(format!(
                    "confirming preparation quiescence for slot {} (host_uid={}): failed to \
                     serialize the Prepared marker: {e}",
                    self.slot, self.host_uid
                ));
                return Err(PreparationConfirmationError::Poisoned);
            }
        };
        if marker_json.len() > MAX_MARKER_BYTES {
            self.released = true;
            self.shared.poison(format!(
                "confirming preparation quiescence for slot {} (host_uid={}): the serialized \
                 Prepared marker would exceed the maximum marker size",
                self.slot, self.host_uid
            ));
            return Err(PreparationConfirmationError::Poisoned);
        }
        match rewrite_marker_atomically(self.shared.dir_fd(), &name, marker_json.as_bytes()) {
            Ok(()) => Ok(()),
            Err(e) => {
                self.released = true;
                self.shared.poison(format!(
                    "confirming preparation quiescence for slot {} (host_uid={}): failed to \
                     durably rewrite the marker to Prepared ({e}) — the on-disk phase is now \
                     ambiguous",
                    self.slot, self.host_uid
                ));
                Err(PreparationConfirmationError::Poisoned)
            }
        }
    }

    /// CT-007 slice 5b.1: release a lease that reached `Prepared` (a real preparation runtime ran
    /// and was proven torn down) but whose real workload never launched (e.g. a failure acquiring
    /// the workload's launch permit) — the `Prepared`-phase counterpart to [`Self::release_unused`].
    /// Needs no additional quiescence proof (the preparation runtime's teardown was already proven
    /// by [`Self::confirm_prepared`]), but DOES require the durable marker to STILL be `Prepared`:
    /// if it is already `Bound`, this is the WRONG release path (use [`Self::release`] with a real
    /// workload quiescence proof instead), and refuses (`MarkerMismatch`, POISONING the whole
    /// allocator) rather than silently unlinking a marker whose `Bound` runtime evidence a caller
    /// might still need. Deliberately distinct from `release_unused`, which requires `Allocated` —
    /// calling `release_unused` on a `Prepared` lease would (correctly) refuse for the same reason.
    pub(crate) fn release_prepared(self) -> Result<(), UserNamespaceReleaseError> {
        self.release_prepared_given(unlinkat_marker)
    }

    /// The deterministic-failure-testable seam behind [`Self::release_prepared`]: `unlink` is
    /// exactly [`unlinkat_marker`] in production, or an injected failure in tests — a real
    /// permission-based `EACCES` is not reliably reproducible across every environment this suite
    /// might run in (e.g. a process carrying `CAP_DAC_OVERRIDE` bypasses the DAC check entirely),
    /// so the ambiguous-unlink-outcome disposition is proven via this seam instead.
    fn release_prepared_given(
        mut self,
        unlink: impl FnOnce(RawFd, &str) -> io::Result<()>,
    ) -> Result<(), UserNamespaceReleaseError> {
        let name = marker_file_name(self.slot);
        let marker_matches = read_and_verify_marker(self.shared.dir_fd(), &name)
            .ok()
            .and_then(|content| serde_json::from_str::<LeaseMarkerV2>(&content).ok())
            .map(|marker| {
                marker.schema_version == LEASE_MARKER_SCHEMA_V2
                    && marker.lease_nonce == self.lease_nonce
                    && marker.runner_instance_id == self.runner_instance_id
                    && marker.host_uid == self.host_uid
                    && marker.host_gid == self.host_gid
                    && matches!(marker.phase, LeasePhaseV2::Prepared { .. })
            })
            .unwrap_or(false);
        if !marker_matches {
            self.released = true;
            self.shared.poison(format!(
                "release_prepared on slot {} (host_uid={}): the durable marker is not (or no \
                 longer) Prepared matching this lease's own identity — either it was already \
                 Bound (use release() with a real workload quiescence proof instead), never \
                 reached Prepared at all, or the on-disk state has diverged; treating as a \
                 global-trust failure",
                self.slot, self.host_uid
            ));
            return Err(UserNamespaceReleaseError::MarkerMismatch);
        }
        match unlink(self.shared.dir_fd(), &name) {
            Ok(()) => match self.shared.fsync_locked_dir() {
                Ok(()) => {
                    self.released = true;
                    let removed = self.shared.lock_state().active_slots.remove(&self.slot);
                    if !removed {
                        let reason = format!(
                            "release_prepared on slot {} (host_uid={}): its marker was durably \
                             unlinked but active_slots did not contain it — a bookkeeping \
                             invariant was violated",
                            self.slot, self.host_uid
                        );
                        self.shared.poison(reason.clone());
                        return Err(UserNamespaceReleaseError::InternalInvariantViolated {
                            reason,
                        });
                    }
                    Ok(())
                }
                Err(e) => {
                    self.released = true;
                    self.shared.poison(format!(
                        "release_prepared on slot {} (host_uid={}): marker unlinked but syncing \
                         the leases directory failed ({e}) — the release outcome is ambiguous",
                        self.slot, self.host_uid
                    ));
                    Err(UserNamespaceReleaseError::Poisoned)
                }
            },
            Err(e) => {
                self.released = true;
                self.shared.poison(format!(
                    "release_prepared on slot {} (host_uid={}): failed to unlink its marker ({e}) \
                     — the release outcome is ambiguous",
                    self.slot, self.host_uid
                ));
                Err(UserNamespaceReleaseError::Poisoned)
            }
        }
    }

    /// CT-007 slice 5b.1: durably transition this lease from `Prepared` to `Bound`, associating it
    /// with the REAL WORKLOAD runtime instance — the checkout-bearing-job counterpart to
    /// [`Self::bind`], which instead requires (and is still used for) an `Allocated` current phase.
    /// Requires the durable marker to STILL be `Prepared` (refuses `MarkerMismatch`, POISONING the
    /// whole allocator, otherwise) — deliberately does NOT re-check anything about the PREPARATION
    /// identity recorded in that `Prepared` phase: once preparation is durably proven torn down, only
    /// the fact that this exact lease reached `Prepared` matters, not which specific preparation
    /// runtime achieved it. On success, the marker's phase is the SAME `LeasePhaseV2::Bound` the
    /// ordinary non-checkout `bind()` produces, so every existing `release()`/final-settlement call
    /// site handles a checkout-bearing job's workload identically, with zero changes.
    pub(crate) fn bind_workload(
        &mut self,
        container_id: String,
        runsc_root_identity: (u64, u64),
        cgroup_identity: (u64, u64),
    ) -> Result<(), UserNamespaceBindError> {
        if !is_valid_container_id(&container_id) {
            return Err(UserNamespaceBindError::InvalidContainerId);
        }
        let name = marker_file_name(self.slot);
        let current = read_and_verify_marker(self.shared.dir_fd(), &name)
            .ok()
            .and_then(|content| serde_json::from_str::<LeaseMarkerV2>(&content).ok())
            .filter(|marker| {
                marker.schema_version == LEASE_MARKER_SCHEMA_V2
                    && marker.lease_nonce == self.lease_nonce
                    && marker.runner_instance_id == self.runner_instance_id
                    && marker.host_uid == self.host_uid
                    && marker.host_gid == self.host_gid
                    && matches!(marker.phase, LeasePhaseV2::Prepared { .. })
            });
        let Some(marker) = current else {
            self.released = true;
            self.shared.poison(format!(
                "binding slot {} (host_uid={}) to a workload runtime: the durable marker no \
                 longer matches this lease's own identity in the Prepared phase — treating as a \
                 global-trust failure",
                self.slot, self.host_uid
            ));
            return Err(UserNamespaceBindError::MarkerMismatch);
        };
        let bound_marker = LeaseMarkerV2 {
            phase: LeasePhaseV2::Bound {
                container_id,
                runsc_root_identity,
                cgroup_identity,
            },
            ..marker
        };
        let marker_json = match serde_json::to_string(&bound_marker) {
            Ok(json) => json,
            Err(e) => {
                self.released = true;
                self.shared.poison(format!(
                    "binding slot {} (host_uid={}) to a workload runtime: failed to serialize the \
                     Bound marker: {e}",
                    self.slot, self.host_uid
                ));
                return Err(UserNamespaceBindError::Poisoned);
            }
        };
        if marker_json.len() > MAX_MARKER_BYTES {
            return Err(UserNamespaceBindError::MarkerTooLarge);
        }
        match rewrite_marker_atomically(self.shared.dir_fd(), &name, marker_json.as_bytes()) {
            Ok(()) => Ok(()),
            Err(e) => {
                self.released = true;
                self.shared.poison(format!(
                    "binding slot {} (host_uid={}) to a workload runtime: failed to durably \
                     rewrite the marker to Bound ({e}) — the on-disk phase is now ambiguous",
                    self.slot, self.host_uid
                ));
                Err(UserNamespaceBindError::Poisoned)
            }
        }
    }

    /// CT-007 slice 3: release a lease that was NEVER [`bound <Self::bind>`](Self::bind) to any
    /// runtime — a pre-spawn failure, a refused launch permit, or any other path where this
    /// lease's identity was never actually exposed to a container. Needs NO quiescence proof
    /// (nothing ever ran with this identity, so there is nothing to prove torn down) — but DOES
    /// re-read the durable marker first, requiring it to STILL be `Allocated`: if it is already
    /// `Bound`, this is the WRONG release path (the identity WAS exposed to something — use
    /// [`Self::release`] with a real quiescence proof instead), and this call refuses
    /// (`MarkerMismatch`, POISONING the whole allocator) rather than silently unlinking a marker
    /// whose `Bound` runtime evidence a caller might still need. Without this path, an ordinary
    /// pre-spawn failure or a refused launch permit would permanently quarantine the subordinate
    /// id for no reason — `Drop` would treat it as abandoned, since it was never released.
    pub fn release_unused(mut self) -> Result<(), UserNamespaceReleaseError> {
        let name = marker_file_name(self.slot);
        let marker_matches = read_and_verify_marker(self.shared.dir_fd(), &name)
            .ok()
            .and_then(|content| serde_json::from_str::<LeaseMarkerV2>(&content).ok())
            .map(|marker| {
                marker.schema_version == LEASE_MARKER_SCHEMA_V2
                    && marker.lease_nonce == self.lease_nonce
                    && marker.runner_instance_id == self.runner_instance_id
                    && marker.host_uid == self.host_uid
                    && marker.host_gid == self.host_gid
                    && marker.phase == LeasePhaseV2::Allocated
            })
            .unwrap_or(false);
        if !marker_matches {
            self.released = true; // avoid a redundant abandonment incident from Drop on top.
            self.shared.poison(format!(
                "release_unused on slot {} (host_uid={}): the durable marker is not (or no \
                 longer) Allocated matching this lease's own identity — either it was already \
                 Bound (use release() with a real quiescence proof instead) or the on-disk state \
                 has diverged; treating as a global-trust failure",
                self.slot, self.host_uid
            ));
            return Err(UserNamespaceReleaseError::MarkerMismatch);
        }
        match unlinkat_marker(self.shared.dir_fd(), &name) {
            Ok(()) => match self.shared.fsync_locked_dir() {
                Ok(()) => {
                    self.released = true;
                    let removed = self.shared.lock_state().active_slots.remove(&self.slot);
                    if !removed {
                        let reason = format!(
                            "release_unused on slot {} (host_uid={}): its marker was durably \
                             unlinked but active_slots did not contain it — a bookkeeping \
                             invariant was violated",
                            self.slot, self.host_uid
                        );
                        self.shared.poison(reason.clone());
                        return Err(UserNamespaceReleaseError::InternalInvariantViolated {
                            reason,
                        });
                    }
                    Ok(())
                }
                Err(e) => {
                    self.released = true;
                    self.shared.poison(format!(
                        "release_unused on slot {} (host_uid={}): marker unlinked but syncing \
                         the leases directory failed ({e}) — the release outcome is ambiguous",
                        self.slot, self.host_uid
                    ));
                    Err(UserNamespaceReleaseError::Poisoned)
                }
            },
            Err(e) => {
                self.released = true;
                self.shared.poison(format!(
                    "release_unused on slot {} (host_uid={}): failed to unlink its marker ({e}) \
                     — the release outcome is ambiguous",
                    self.slot, self.host_uid
                ));
                Err(UserNamespaceReleaseError::Poisoned)
            }
        }
    }

    /// Ordinary release: the caller has ALREADY proven (via `proof`) that the container/gofer/
    /// sentry this lease's identity was exposed to is fully torn down. Verifies `proof` was minted
    /// for THIS lease AND for the SAME `container_id`/`runsc_root_identity`/`cgroup_identity` the
    /// durable marker's own `Bound` phase records, re-reads the durable marker to confirm it STILL
    /// matches this lease's own identity in that exact `Bound` state, and ONLY THEN unlinks it
    /// (`openat`/`unlinkat` relative to the held lock FD — never a path-based reopen) and syncs the
    /// directory FD.
    ///
    /// NOTE on `ProofMismatch`/`ProofDisagreesWithMarker`: `release` takes `self` BY VALUE, so this
    /// call consumes the lease regardless of outcome — there is no way to "retry with the correct
    /// proof" using the SAME lease value once this method has been called at all. On either of
    /// those two outcomes specifically, `self.released` is left `false`, so `self`'s ordinary
    /// `Drop` (which fires as this call returns) quarantines the slot exactly as any other
    /// abandoned lease would — a caller unsure of the right proof should check BEFORE calling
    /// `release`, not rely on retrying after.
    pub fn release(
        mut self,
        proof: UserNamespaceQuiescenceProof,
    ) -> Result<(), UserNamespaceReleaseError> {
        if proof.lease_nonce != self.lease_nonce {
            return Err(UserNamespaceReleaseError::ProofMismatch);
        }
        let name = marker_file_name(self.slot);
        let marker = read_and_verify_marker(self.shared.dir_fd(), &name)
            .ok()
            .and_then(|content| serde_json::from_str::<LeaseMarkerV2>(&content).ok());
        // Split in two: does the durable marker even BELONG to this lease (base identity — real
        // corruption/tampering if not, a global-trust failure), versus does its PHASE agree with
        // what the proof claims (an ordinary wrong-proof-for-a-valid-lease situation if not, no
        // different in kind from the nonce-mismatch case above — never a global poison).
        let base_identity_matches = marker.as_ref().is_some_and(|marker| {
            marker.schema_version == LEASE_MARKER_SCHEMA_V2
                && marker.lease_nonce == self.lease_nonce
                && marker.runner_instance_id == self.runner_instance_id
                && marker.host_uid == self.host_uid
                && marker.host_gid == self.host_gid
        });
        if !base_identity_matches {
            self.released = true; // Avoid a redundant abandonment incident from Drop on top.
            self.shared.poison(format!(
                "releasing slot {} (host_uid={}): the durable marker no longer matches this \
                 lease's own identity (schema/nonce/runner/host_uid/host_gid) — treating as a \
                 global-trust failure",
                self.slot, self.host_uid
            ));
            return Err(UserNamespaceReleaseError::MarkerMismatch);
        }
        let phase_matches_proof = marker.as_ref().is_some_and(|marker| {
            marker.phase
                == LeasePhaseV2::Bound {
                    container_id: proof.container_id.clone(),
                    runsc_root_identity: proof.runsc_root_identity,
                    cgroup_identity: proof.cgroup_identity,
                }
        });
        if !phase_matches_proof {
            // The marker genuinely belongs to this lease (verified above) — it was either never
            // bound, or is Bound to a different runtime identity than this proof claims. An
            // ordinary wrong proof, not corruption: leave `self.released` false so Drop quarantines
            // only this one lease, and do NOT poison the allocator.
            return Err(UserNamespaceReleaseError::ProofDisagreesWithMarker);
        }
        match unlinkat_marker(self.shared.dir_fd(), &name) {
            Ok(()) => match self.shared.fsync_locked_dir() {
                Ok(()) => {
                    self.released = true;
                    let removed = self.shared.lock_state().active_slots.remove(&self.slot);
                    if !removed {
                        let reason = format!(
                            "releasing slot {} (host_uid={}): its marker was durably unlinked \
                             but active_slots did not contain it — a bookkeeping invariant was \
                             violated",
                            self.slot, self.host_uid
                        );
                        self.shared.poison(reason.clone());
                        return Err(UserNamespaceReleaseError::InternalInvariantViolated {
                            reason,
                        });
                    }
                    Ok(())
                }
                Err(e) => {
                    self.released = true;
                    self.shared.poison(format!(
                        "releasing slot {} (host_uid={}): marker unlinked but syncing the \
                         leases directory failed ({e}) — the release outcome is ambiguous",
                        self.slot, self.host_uid
                    ));
                    Err(UserNamespaceReleaseError::Poisoned)
                }
            },
            Err(e) => {
                self.released = true;
                self.shared.poison(format!(
                    "releasing slot {} (host_uid={}): failed to unlink its marker ({e}) — the \
                     release outcome is ambiguous",
                    self.slot, self.host_uid
                ));
                Err(UserNamespaceReleaseError::Poisoned)
            }
        }
    }
}

impl Drop for UserNamespaceLease {
    fn drop(&mut self) {
        if !self.released {
            self.shared.quarantine_slot(
                self.slot,
                format!(
                    "user-namespace lease for slot {} (host_uid={}, host_gid={}) was dropped \
                     without an explicit release — its marker is left in place; this slot is \
                     quarantined and will never be reissued by this allocator instance",
                    self.slot, self.host_uid, self.host_gid
                ),
            );
        }
    }
}

/// CT-007 slice 5b.1: this session's own in-memory view of a checkout-bearing job's two-phase
/// lease lifecycle, kept STRICTLY in lockstep with the durable marker's own phase
/// (`LeasePhaseV2::PreparationBound`/`Prepared`). This is defense in depth ON TOP OF the durable
/// checks each `UserNamespaceLease` method already performs — not a replacement for them: even a
/// caller who forgets this session entirely and calls the lease's own methods directly still gets
/// the exact same durable-marker safety. What THIS type adds is that a caller cannot reorder or
/// skip a transition and have it silently no-op or panic somewhere deep inside `UserNamespaceLease`
/// with no earlier, cheaper signal — calling a transition out of this session's own expected order
/// is a caller bug (this module's own orchestration code is the only caller), so it panics
/// immediately, before ever touching the lease or its durable marker.
///
/// Slice 5b.1 deliberately does not wire this into `GvisorBackend`/`launch_with` yet (mirroring
/// slice 1's own precedent: a real production consumer is a LATER slice's job) — `#[allow(dead_code)]`
/// here and on its methods below is temporary and expected to be removed once 5b.3 adds that
/// consumer, not a sign this is unreachable code.
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct CheckoutPreparationSession {
    state: CheckoutPreparationSessionState,
}

/// The exact identity triple a [`CheckoutPreparationSession::bind_workload`] call just durably
/// committed to `Bound` — minted only on success, from the SAME values the durable rewrite wrote,
/// never separately re-derived. Fields are deliberately private and this type is NOT `Clone`: the
/// only way to obtain one is a genuine successful `bind_workload`, and the only way to consume one
/// is [`Self::into_parts`], so it can never be forged or duplicated elsewhere in the crate. The
/// 5b.3 caller constructs `LeaseBindState::Bound` from the parts this yields, so the two can never
/// silently diverge.
#[derive(Debug, PartialEq, Eq)]
#[allow(dead_code)] // temporary, see CheckoutPreparationSession's own doc above.
pub(crate) struct WorkloadBindingIdentity {
    container_id: String,
    runsc_root_identity: (u64, u64),
    cgroup_identity: (u64, u64),
}

#[allow(dead_code)] // temporary, see CheckoutPreparationSession's own doc above.
impl WorkloadBindingIdentity {
    #[must_use]
    pub(crate) fn into_parts(self) -> (String, (u64, u64), (u64, u64)) {
        (
            self.container_id,
            self.runsc_root_identity,
            self.cgroup_identity,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // temporary, see CheckoutPreparationSession's own doc above.
enum CheckoutPreparationSessionState {
    NotStarted,
    PreparationBound { lease_nonce: LeaseNonce },
    Prepared { lease_nonce: LeaseNonce },
    Done,
    Unreleasable,
}

/// The ONLY correct cleanup a [`CheckoutPreparationSession`]'s current durable state permits for the
/// workspace+lease it is paired with (CT-007 slice 5b.3-6a). A disposal caller reads this from the
/// session's OWN authoritative state and dispatches accordingly — it must never guess the lease
/// phase, because calling the wrong release method (`release_unused` on a `Prepared` lease, or
/// `release_prepared`/`release_unused` on a `PreparationBound`/poisoned one) either poisons the
/// allocator or reissues a subordinate id while its chowned workspace may still be live. Each variant
/// names EXACTLY one safe disposition, so the wrong release is unreachable by construction rather than
/// by the caller's own care.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // temporary, see CheckoutPreparationSession's own doc above.
pub(crate) enum CheckoutSessionCleanup {
    /// `NotStarted` — Hop B never bound; the lease is provably still `Allocated`. Delete the
    /// workspace, then `release_unused` the lease.
    NeverBound,
    /// `PreparationBound` — Hop B bound the preparation runtime but its teardown was NEVER
    /// independently proven. Quarantine BOTH the workspace and the lease (never release, never
    /// delete): the runtime's non-access is unproven.
    TeardownUnproven,
    /// `Prepared` — Hop B's teardown WAS proven but the workload never bound. Delete the workspace,
    /// then `release_prepared` the lease.
    Prepared,
    /// `Done` — the workload was durably `Bound`; the existing finalization/settlement path owns this
    /// lease. Disposal must NOT release it.
    WorkloadBound,
    /// `Unreleasable` — a poisoning transition already abandoned the lease on its own side.
    /// Quarantine; never release.
    Unreleasable,
}

#[allow(dead_code)] // temporary, see CheckoutPreparationSession's own doc above.
impl CheckoutPreparationSession {
    pub(crate) fn new() -> Self {
        CheckoutPreparationSession {
            state: CheckoutPreparationSessionState::NotStarted,
        }
    }

    #[cfg(test)]
    pub(crate) fn is_unreleasable(&self) -> bool {
        self.state == CheckoutPreparationSessionState::Unreleasable
    }

    /// Durably bind `lease` to the preparation runtime. Panics if this session has already left
    /// `NotStarted` (a caller bug — `bind_preparation` must be called at most once, first).
    pub(crate) fn bind_preparation(
        &mut self,
        lease: &mut UserNamespaceLease,
        container_id: String,
        runsc_root_identity: (u64, u64),
        cgroup_identity: (u64, u64),
    ) -> Result<(), UserNamespaceBindError> {
        assert_eq!(
            self.state,
            CheckoutPreparationSessionState::NotStarted,
            "bind_preparation called out of order (session state was {:?})",
            self.state
        );
        let lease_nonce = lease.lease_nonce;
        match lease.bind_preparation(container_id, runsc_root_identity, cgroup_identity) {
            Ok(()) => {
                self.state = CheckoutPreparationSessionState::PreparationBound { lease_nonce };
                Ok(())
            }
            // Caller-fixable, and the lease is provably untouched (still Allocated) — leave this
            // session at `NotStarted` so a caller may correct the input and retry.
            Err(e @ UserNamespaceBindError::InvalidContainerId)
            | Err(e @ UserNamespaceBindError::MarkerTooLarge) => Err(e),
            // MarkerMismatch/Poisoned already poisoned the allocator and marked the lease
            // released/abandoned on the lease's own side — mirror that here.
            Err(e) => {
                self.state = CheckoutPreparationSessionState::Unreleasable;
                Err(e)
            }
        }
    }

    /// Durably confirm the preparation runtime's proven teardown, transitioning `lease` from
    /// `PreparationBound` to `Prepared`. Panics if this session is not currently `PreparationBound`.
    pub(crate) fn confirm_prepared(
        &mut self,
        lease: &mut UserNamespaceLease,
        proof: PreparationQuiescenceProof,
    ) -> Result<(), PreparationConfirmationError> {
        let CheckoutPreparationSessionState::PreparationBound { lease_nonce } = self.state else {
            panic!(
                "confirm_prepared called out of order (session state was {:?})",
                self.state
            );
        };
        assert_eq!(
            lease_nonce, lease.lease_nonce,
            "confirm_prepared called with a lease different from the one this session was bound \
             to by bind_preparation"
        );
        match lease.confirm_prepared(proof) {
            Ok(()) => {
                self.state = CheckoutPreparationSessionState::Prepared { lease_nonce };
                Ok(())
            }
            // Sol's review: unlike the raw lease API (where the marker is left genuinely
            // untouched, so a caller COULD retry with a corrected proof), THIS session offers no
            // retry for ANY confirm_prepared failure, including an ordinary wrong/mismatched
            // proof — a real caller has exactly one proof-minting opportunity per real
            // preparation run (from the one RuntimeQuiescenceEvidence that run produced), so a
            // later "correct" proof isn't a real scenario worth the risk of reasoning about;
            // abandon the slot/workspace instead of leaving that door open.
            Err(e) => {
                self.state = CheckoutPreparationSessionState::Unreleasable;
                Err(e)
            }
        }
    }

    /// Release `lease` after a proven-`Prepared` session whose real workload never launched (e.g.
    /// a failure acquiring the workload's own launch permit). Panics if this session is not
    /// currently `Prepared`. Consumes `self`: there is nothing further a `CheckoutPreparationSession`
    /// can do once its lease is given up.
    pub(crate) fn release_prepared(
        self,
        lease: UserNamespaceLease,
    ) -> Result<(), UserNamespaceReleaseError> {
        let CheckoutPreparationSessionState::Prepared { lease_nonce } = self.state else {
            panic!(
                "release_prepared called out of order (session state was {:?})",
                self.state
            );
        };
        assert_eq!(
            lease_nonce, lease.lease_nonce,
            "release_prepared called with a lease different from the one this session was bound to"
        );
        lease.release_prepared()
    }

    /// Durably bind `lease` to the real workload runtime, transitioning it from `Prepared` to
    /// `Bound` — the same `LeasePhaseV2::Bound` the ordinary non-checkout path produces, so every
    /// existing final-settlement call site takes over unchanged from here. Panics if this session
    /// is not currently `Prepared`, or if `lease` is not the SAME lease this session was bound to.
    /// On success, returns a [`WorkloadBindingIdentity`] minted from the EXACT triple the durable
    /// rewrite just committed — the caller constructs `LeaseBindState::Bound` from this returned
    /// value, never by separately re-deriving/cloning the arguments passed in here, so the
    /// in-memory bookkeeping can never silently diverge from what was actually written to disk.
    pub(crate) fn bind_workload(
        &mut self,
        lease: &mut UserNamespaceLease,
        container_id: String,
        runsc_root_identity: (u64, u64),
        cgroup_identity: (u64, u64),
    ) -> Result<WorkloadBindingIdentity, UserNamespaceBindError> {
        let CheckoutPreparationSessionState::Prepared { lease_nonce } = self.state else {
            panic!(
                "bind_workload called out of order (session state was {:?})",
                self.state
            );
        };
        assert_eq!(
            lease_nonce, lease.lease_nonce,
            "bind_workload called with a lease different from the one this session was bound to"
        );
        match lease.bind_workload(container_id.clone(), runsc_root_identity, cgroup_identity) {
            Ok(()) => {
                self.state = CheckoutPreparationSessionState::Done;
                Ok(WorkloadBindingIdentity {
                    container_id,
                    runsc_root_identity,
                    cgroup_identity,
                })
            }
            // Caller-fixable, and the lease is provably still Prepared and untouched — leave this
            // session at Prepared so a caller may correct the input and retry (Sol's review: the
            // original by-value signature destroyed the only preparation capability even here).
            Err(e @ UserNamespaceBindError::InvalidContainerId)
            | Err(e @ UserNamespaceBindError::MarkerTooLarge) => Err(e),
            Err(e) => {
                self.state = CheckoutPreparationSessionState::Unreleasable;
                Err(e)
            }
        }
    }

    /// The one safe cleanup this session's CURRENT durable state permits (CT-007 slice 5b.3-6a).
    /// Read by the 5b.3-6 capsule's disposal routing so the correct release/quarantine is chosen from
    /// the session's own authoritative state, never guessed. Pure — touches neither the lease nor its
    /// marker.
    pub(crate) fn cleanup_disposition(&self) -> CheckoutSessionCleanup {
        match self.state {
            CheckoutPreparationSessionState::NotStarted => CheckoutSessionCleanup::NeverBound,
            CheckoutPreparationSessionState::PreparationBound { .. } => {
                CheckoutSessionCleanup::TeardownUnproven
            }
            CheckoutPreparationSessionState::Prepared { .. } => CheckoutSessionCleanup::Prepared,
            CheckoutPreparationSessionState::Done => CheckoutSessionCleanup::WorkloadBound,
            CheckoutPreparationSessionState::Unreleasable => CheckoutSessionCleanup::Unreleasable,
        }
    }
}

/// The persistent per-process owner of subordinate-uid/gid-slot admission and leasing.
/// `GvisorBackend` will hold one starting in slice 3 (this slice deliberately does not wire it in).
pub struct UserNamespaceAllocator {
    leases_dir: PathBuf,
    pool_size: u32,
    uid_start: u32,
    gid_start: u32,
    runner_uid: u32,
    runner_gid: u32,
    #[allow(dead_code)] // observability only for now; slice 3's incident tooling reads this.
    runner_instance_id: RunnerInstanceId,
    shared: Arc<SharedState>,
}

impl UserNamespaceAllocator {
    /// The canonical production constructor: fixed to the REAL `/etc/subuid`/`/etc/subgid`
    /// (Sol's review: a public constructor that accepts arbitrary subordinate-config paths would
    /// let a caller point it at an untrusted file), full hardening enforced (leases-dir parent not
    /// writable by this process; `/etc/subuid`/`/etc/subgid` must be root-owned,
    /// non-group/other-writable). `min_pool_size` is the CALLER's own stated concurrency
    /// requirement (e.g. "I need to support at least N concurrent leases") — construction refuses
    /// if the subordinate ranges this host actually has configured cannot meet it.
    pub fn try_new(
        leases_dir: PathBuf,
        min_pool_size: u32,
        incident_sink: IncidentSink,
    ) -> Result<Self, UserNamespaceAllocatorError> {
        Self::try_new_impl(
            leases_dir,
            Path::new("/etc/subuid"),
            Path::new("/etc/subgid"),
            min_pool_size,
            true,
            incident_sink,
        )
    }

    /// Test-only: accepts fixture `/etc/subuid`/`/etc/subgid`-format paths (never owned by root in
    /// a test process, so the strict-ownership check is skipped) and a leases dir under a
    /// writable-by-us temp directory (so the parent-not-writable check is also skipped — a test
    /// cannot set up a root-owned parent without privilege this session may not have).
    ///
    /// Retries a bounded number of times on `AlreadyLocked` (task #75, Sol's root-cause read):
    /// under `cargo test`'s default parallelism, an unrelated concurrent test's `Command::spawn()`
    /// can transiently inherit ANOTHER test's directory-lock fd during the fork-to-exec window
    /// (before `O_CLOEXEC` takes effect at `exec`). A same-test drop-then-immediately-reopen that
    /// races that window can spuriously see the lock as still held. This is a TEST-ONLY affordance
    /// — the real, process-lifetime allocator (`try_new`) must keep failing closed on the first
    /// `AlreadyLocked`, since in production that error means a second real runner process, not a
    /// transient fork-window artifact.
    #[cfg(test)]
    pub(crate) fn try_new_for_tests(
        leases_dir: PathBuf,
        subuid_path: &Path,
        subgid_path: &Path,
        min_pool_size: u32,
        incident_sink: IncidentSink,
    ) -> Result<Self, UserNamespaceAllocatorError> {
        const MAX_ATTEMPTS: u32 = 20;
        for attempt in 1..=MAX_ATTEMPTS {
            match Self::try_new_impl(
                leases_dir.clone(),
                subuid_path,
                subgid_path,
                min_pool_size,
                false,
                incident_sink.clone(),
            ) {
                Err(UserNamespaceAllocatorError::AlreadyLocked { .. }) if attempt < MAX_ATTEMPTS => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    continue;
                }
                result => return result,
            }
        }
        unreachable!("loop always returns on its final attempt");
    }

    /// Construct the allocator: refuse a privileged (uid/gid 0) runner; parse+validate BOTH
    /// subordinate ranges (fail-closed on any malformed/missing/ambiguous config, and refusing a
    /// range that contains 0 or this process's own uid/gid); harden+verify `leases_dir` itself (no
    /// symlink, owned by this process, mode `0700` or stricter, and — in `strict` mode — a parent
    /// this process cannot rename/replace it from); enforce `min_pool_size`; acquire the
    /// process-lifetime lock; then scan it via the LOCKED FD (never a second path-based open) —
    /// every surviving valid, slot-consistent marker (schema_version 1, the frozen legacy 2-variant
    /// shape, OR schema_version 2, the current 4-variant shape — see `LeaseMarkerV1`/`LeaseMarkerV2`)
    /// is QUARANTINED (never deleted, never reissued) with an incident reported per marker; any
    /// unrecognized/unparseable/non-regular/slot-inconsistent entry, or any OTHER schema_version,
    /// POISONS CONSTRUCTION ITSELF (returns `Err`, never a partially-trusted `Ok`).
    fn try_new_impl(
        leases_dir: PathBuf,
        subuid_path: &Path,
        subgid_path: &Path,
        min_pool_size: u32,
        strict: bool,
        incident_sink: IncidentSink,
    ) -> Result<Self, UserNamespaceAllocatorError> {
        let runner_uid = unsafe { libc::geteuid() };
        let runner_gid = unsafe { libc::getegid() };
        if runner_uid == 0 || runner_gid == 0 {
            return Err(UserNamespaceAllocatorError::PrivilegedRunner {
                euid: runner_uid,
                egid: runner_gid,
            });
        }
        let username = effective_username();
        let uid_range =
            parse_subordinate_range(subuid_path, runner_uid, username.as_deref(), strict)?;
        let gid_range =
            parse_subordinate_range(subgid_path, runner_uid, username.as_deref(), strict)?;
        if range_contains(uid_range, 0) || range_contains(uid_range, runner_uid) {
            return Err(UserNamespaceAllocatorError::SubordinateConfig {
                path: subuid_path.to_path_buf(),
                reason: format!(
                    "subordinate uid range {uid_range:?} must not contain 0 or this process's own \
                     euid {runner_uid}"
                ),
            });
        }
        if range_contains(gid_range, 0) || range_contains(gid_range, runner_gid) {
            return Err(UserNamespaceAllocatorError::SubordinateConfig {
                path: subgid_path.to_path_buf(),
                reason: format!(
                    "subordinate gid range {gid_range:?} must not contain 0 or this process's own \
                     egid {runner_gid}"
                ),
            });
        }
        let pool_size = uid_range.count.min(gid_range.count);
        debug_assert!(
            pool_size > 0,
            "parse_subordinate_range already refuses a zero count on either file"
        );
        if pool_size < min_pool_size {
            return Err(UserNamespaceAllocatorError::PoolTooSmall {
                pool_size,
                required: min_pool_size,
            });
        }

        harden_and_verify_leases_dir(&leases_dir, strict)?;
        let lock =
            crate::dirlock::acquire_directory_lock(&leases_dir).map_err(|error| match error {
                crate::dirlock::DirLockError::AlreadyLocked => {
                    UserNamespaceAllocatorError::AlreadyLocked {
                        leases_dir: leases_dir.clone(),
                    }
                }
                crate::dirlock::DirLockError::Failed(reason) => {
                    UserNamespaceAllocatorError::LockFailed {
                        leases_dir: leases_dir.clone(),
                        reason,
                    }
                }
            })?;
        let locked_identity = crate::dirlock::fd_identity(&lock).map_err(|e| {
            UserNamespaceAllocatorError::LockFailed {
                leases_dir: leases_dir.clone(),
                reason: format!("fstat locked directory: {e}"),
            }
        })?;

        let shared = Arc::new(SharedState {
            _lock: lock,
            state: Mutex::new(AllocatorState {
                admission: UserNamespaceAdmission::Healthy,
                quarantined_slots: BTreeSet::new(),
                active_slots: BTreeSet::new(),
                locked_identity: Some(locked_identity),
            }),
            incident_sink,
        });

        let mut quarantined = BTreeSet::new();
        let mut incidents = Vec::new();
        // Stray `bind()`-rewrite temp files, collected during the pass below but only ACTED on
        // afterward (see the second pass past the end of this loop) — a temp file is deleted ONLY
        // once we know its slot has a validated, durably quarantined primary marker of its own.
        // Deleting it purely on the strength of an in-memory quarantine (this pass alone) would
        // erase the only durable evidence a slot was ever touched if the primary marker happens to
        // be absent, letting a later boot reissue it — exactly the never-reissue guarantee this
        // module exists to uphold.
        let mut stray_tmp_entries: Vec<(u32, String, PathBuf)> = Vec::new();
        // Read via the LOCKED FD's `/proc/self/fd/<fd>` alias — immune to `leases_dir`'s original
        // path being renamed/replaced out from under the lock (Sol's review).
        for entry in std::fs::read_dir(shared.listing_path()).map_err(|e| {
            UserNamespaceAllocatorError::LockFailed {
                leases_dir: leases_dir.clone(),
                reason: format!("read_dir via locked fd: {e}"),
            }
        })? {
            let entry = entry.map_err(|e| UserNamespaceAllocatorError::CorruptLeaseMarker {
                path: leases_dir.clone(),
                reason: format!("read_dir entry: {e}"),
            })?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            let Some(slot) = parse_marker_file_name(&name_str) else {
                if let Some(tmp_slot) = parse_stray_tmp_marker_file_name(&name_str) {
                    // A crash between creating `<marker>.tmp` and the `renameat` that would have
                    // made it the real marker can leave this exact artifact behind. Recognize it
                    // specifically so it doesn't fall into the generic unrecognized-entry error
                    // below — but do NOT act on it yet: whether it's safe to delete depends on
                    // whether this slot's primary marker survived too, which this single pass
                    // cannot yet know (directory iteration order is unspecified).
                    if tmp_slot >= pool_size {
                        return Err(UserNamespaceAllocatorError::CorruptLeaseMarker {
                            path: entry.path(),
                            reason: format!(
                                "stale bind-rewrite temp file {name_str:?} names slot \
                                 {tmp_slot}, outside the current pool size {pool_size}"
                            ),
                        });
                    }
                    stray_tmp_entries.push((tmp_slot, name_str.into_owned(), entry.path()));
                    continue;
                }
                return Err(UserNamespaceAllocatorError::CorruptLeaseMarker {
                    path: entry.path(),
                    reason: format!("unrecognized entry in leases dir: {name_str:?}"),
                });
            };
            // `read_and_verify_marker` opens BY NAME relative to the locked dir FD
            // (`O_NOFOLLOW|O_NONBLOCK`) FIRST, then `fstat`s the ALREADY-OPEN fd to confirm a
            // regular file owned by this process — never a separate `file_type()`-then-reopen-
            // by-path sequence, which is exactly the leaf-level TOCTOU Sol's review found (the
            // entry could be replaced between the type check and the path-based reopen).
            let content = read_and_verify_marker(shared.dir_fd(), &name_str).map_err(|e| {
                UserNamespaceAllocatorError::CorruptLeaseMarker {
                    path: entry.path(),
                    reason: format!("read marker: {e}"),
                }
            })?;
            // Peek `schema_version` directly from the raw string — NEVER through
            // `serde_json::Value` (see this module's doc: that path cannot losslessly represent a
            // real random `u128`, which would reject every genuine marker as corrupt).
            let peek: SchemaPeek = serde_json::from_str(&content).map_err(|e| {
                UserNamespaceAllocatorError::CorruptLeaseMarker {
                    path: entry.path(),
                    reason: format!("marker is not valid JSON / has no schema_version: {e}"),
                }
            })?;
            match peek.schema_version {
                // CT-007 slice 5b.1: a legacy 2-variant marker from a pre-5b.1 binary. Never
                // written by this code (see `LeaseMarkerV1`'s own doc) — recognized here purely so
                // it quarantines cleanly instead of falling into the `other` arm's "unrecognized
                // schema_version" refusal, which would incorrectly suggest this is a NEWER, not
                // OLDER, format this binary cannot understand.
                v if v == LEASE_MARKER_SCHEMA_V1 => {
                    let marker: LeaseMarkerV1 = serde_json::from_str(&content).map_err(|e| {
                        UserNamespaceAllocatorError::CorruptLeaseMarker {
                            path: entry.path(),
                            reason: format!(
                                "marker claims schema_version=1 but does not parse as \
                                 LeaseMarkerV1: {e}"
                            ),
                        }
                    })?;
                    if slot >= pool_size {
                        return Err(UserNamespaceAllocatorError::CorruptLeaseMarker {
                            path: entry.path(),
                            reason: format!(
                                "marker names slot {slot}, outside the current pool size \
                                 {pool_size} — subordinate-range configuration likely changed \
                                 incompatibly since this marker was written"
                            ),
                        });
                    }
                    let expected_uid = uid_range.start + slot;
                    let expected_gid = gid_range.start + slot;
                    if marker.host_uid != expected_uid || marker.host_gid != expected_gid {
                        return Err(UserNamespaceAllocatorError::CorruptLeaseMarker {
                            path: entry.path(),
                            reason: format!(
                                "marker for slot {slot} names host_uid={}, host_gid={}, but the \
                                 CURRENT subordinate ranges imply host_uid={expected_uid}, \
                                 host_gid={expected_gid} for this slot — the range start likely \
                                 changed since this marker was written; refusing to guess which \
                                 identity is authoritative",
                                marker.host_uid, marker.host_gid
                            ),
                        });
                    }
                    quarantined.insert(slot);
                    let phase_desc = match &marker.phase {
                        LeasePhaseV1::Allocated => "Allocated".to_string(),
                        LeasePhaseV1::Bound { container_id, .. } => {
                            format!("Bound (container_id={container_id:?})")
                        }
                    };
                    incidents.push(format!(
                        "boot reconciliation: slot {slot} (host_uid={}, host_gid={}) has a \
                         surviving legacy schema_version=1 {phase_desc} marker from runner \
                         instance {:?} — quarantined, will never be reissued by this allocator \
                         instance",
                        marker.host_uid, marker.host_gid, marker.runner_instance_id
                    ));
                }
                v if v == LEASE_MARKER_SCHEMA_V2 => {
                    let marker: LeaseMarkerV2 = serde_json::from_str(&content).map_err(|e| {
                        UserNamespaceAllocatorError::CorruptLeaseMarker {
                            path: entry.path(),
                            reason: format!(
                                "marker claims schema_version=2 but does not parse as \
                                 LeaseMarkerV2: {e}"
                            ),
                        }
                    })?;
                    if slot >= pool_size {
                        return Err(UserNamespaceAllocatorError::CorruptLeaseMarker {
                            path: entry.path(),
                            reason: format!(
                                "marker names slot {slot}, outside the current pool size \
                                 {pool_size} — subordinate-range configuration likely changed \
                                 incompatibly since this marker was written"
                            ),
                        });
                    }
                    let expected_uid = uid_range.start + slot;
                    let expected_gid = gid_range.start + slot;
                    if marker.host_uid != expected_uid || marker.host_gid != expected_gid {
                        return Err(UserNamespaceAllocatorError::CorruptLeaseMarker {
                            path: entry.path(),
                            reason: format!(
                                "marker for slot {slot} names host_uid={}, host_gid={}, but the \
                                 CURRENT subordinate ranges imply host_uid={expected_uid}, \
                                 host_gid={expected_gid} for this slot — the range start likely \
                                 changed since this marker was written; refusing to guess which \
                                 identity is authoritative",
                                marker.host_uid, marker.host_gid
                            ),
                        });
                    }
                    quarantined.insert(slot);
                    let phase_desc = match &marker.phase {
                        LeasePhaseV2::Allocated => "Allocated".to_string(),
                        LeasePhaseV2::PreparationBound { container_id, .. } => {
                            format!("PreparationBound (container_id={container_id:?})")
                        }
                        LeasePhaseV2::Prepared {
                            preparation_container_id,
                            ..
                        } => format!(
                            "Prepared (preparation_container_id={preparation_container_id:?})"
                        ),
                        LeasePhaseV2::Bound { container_id, .. } => {
                            format!("Bound (container_id={container_id:?})")
                        }
                    };
                    incidents.push(format!(
                        "boot reconciliation: slot {slot} (host_uid={}, host_gid={}) has a \
                         surviving {phase_desc} marker from runner instance {:?} — quarantined, \
                         will never be reissued by this allocator instance",
                        marker.host_uid, marker.host_gid, marker.runner_instance_id
                    ));
                }
                other => {
                    return Err(UserNamespaceAllocatorError::CorruptLeaseMarker {
                        path: entry.path(),
                        reason: format!("unrecognized schema_version: {other}"),
                    });
                }
            }
        }

        // Second pass: now that every primary marker has been validated and its slot durably
        // quarantined (`quarantined` above), decide what to do with each stray bind-rewrite temp
        // file. A temp file is only ever cleanup residue for a slot whose OWN primary marker
        // survived (and is therefore already durably quarantined) — if that primary is absent, we
        // cannot tell whether the slot was ever truly exposed to a runtime from the temp file
        // alone, so refuse construction rather than silently deleting the only surviving evidence.
        let had_stray_tmp_entries = !stray_tmp_entries.is_empty();
        for (tmp_slot, tmp_name, tmp_path) in stray_tmp_entries {
            if !quarantined.contains(&tmp_slot) {
                return Err(UserNamespaceAllocatorError::CorruptLeaseMarker {
                    path: tmp_path,
                    reason: format!(
                        "stale bind-rewrite temp file {tmp_name:?} names slot {tmp_slot}, but no \
                         primary marker for that slot survived — refusing to guess whether the \
                         slot is safe to reissue"
                    ),
                });
            }
            unlinkat_marker(shared.dir_fd(), &tmp_name).map_err(|e| {
                UserNamespaceAllocatorError::CorruptLeaseMarker {
                    path: tmp_path.clone(),
                    reason: format!(
                        "failed to remove stale bind-rewrite temp file {tmp_name:?}, alongside \
                         its slot's already-quarantined primary marker: {e}"
                    ),
                }
            })?;
            incidents.push(format!(
                "boot reconciliation: slot {tmp_slot} had a stale bind-rewrite temp file \
                 ({tmp_name:?}) alongside its durably quarantined primary marker — removed"
            ));
        }
        if had_stray_tmp_entries {
            shared
                .fsync_locked_dir()
                .map_err(|e| UserNamespaceAllocatorError::LockFailed {
                    leases_dir: leases_dir.clone(),
                    reason: format!(
                        "syncing the leases directory after removing stale bind-rewrite temp \
                         file(s) failed: {e}"
                    ),
                })?;
        }

        {
            let mut state = shared.lock_state();
            state.quarantined_slots = quarantined;
        }
        for message in incidents {
            shared.report_incident(&message);
        }

        Ok(Self {
            leases_dir,
            pool_size,
            uid_start: uid_range.start,
            gid_start: gid_range.start,
            runner_uid,
            runner_gid,
            runner_instance_id: runner_instance_id(),
            shared,
        })
    }

    /// The current admission state.
    pub fn admission(&self) -> UserNamespaceAdmission {
        self.shared.lock_state().admission.clone()
    }

    pub fn is_healthy(&self) -> bool {
        matches!(self.admission(), UserNamespaceAdmission::Healthy)
    }

    /// The total number of slots this allocator's paired uid/gid ranges provide (leased + free +
    /// quarantined).
    pub fn pool_size(&self) -> u32 {
        self.pool_size
    }

    /// Slots currently known-bad (quarantined) this process instance — observability only.
    pub fn quarantined_slots(&self) -> BTreeSet<u32> {
        self.shared.lock_state().quarantined_slots.clone()
    }

    /// A read-only check that `leases_dir` still names the EXACT directory this allocator locked
    /// at construction (compared by device+inode). Never called automatically; a caller wanting
    /// periodic re-verification calls this on its own schedule. Poisons the WHOLE allocator on a
    /// mismatch or a stat failure.
    pub fn check_identity(&self) -> Result<(), UserNamespaceRefusal> {
        let locked_identity = self
            .shared
            .lock_state()
            .locked_identity
            .expect("a try_new-constructed allocator always records a locked identity");
        match crate::dirlock::path_identity(&self.leases_dir) {
            Ok(current) if current == locked_identity => Ok(()),
            Ok(_) => {
                let reason = format!(
                    "{:?} no longer names the directory this allocator locked at construction",
                    self.leases_dir
                );
                self.shared.poison(reason.clone());
                Err(UserNamespaceRefusal::Poisoned { reason })
            }
            Err(e) => {
                let reason = format!("stat {:?}: {e}", self.leases_dir);
                self.shared.poison(reason.clone());
                Err(UserNamespaceRefusal::Poisoned { reason })
            }
        }
    }

    /// Lease the lowest-numbered free slot. The ENTIRE operation (admission check, slot selection,
    /// marker creation, directory sync) runs under ONE hold of the allocator's state mutex —
    /// fully serializing concurrent `lease()` calls process-wide, so two real concurrent callers
    /// can never both observe the same slot as free and race on `O_EXCL` (that race previously
    /// mispoisoned the whole allocator over an entirely expected collision).
    pub fn lease(&self) -> Result<UserNamespaceLease, UserNamespaceRefusal> {
        let mut state = self.shared.lock_state();
        if let UserNamespaceAdmission::Poisoned { reason } = &state.admission {
            return Err(UserNamespaceRefusal::Poisoned {
                reason: reason.clone(),
            });
        }

        // Slot selection tries EVERY slot from 0 upward unconditionally — deliberately NOT
        // pre-filtered by a directory scan. `state`'s mutex already fully serializes `lease()`
        // process-wide, so slot selection needs no directory scan at all to be correct, only to
        // be efficient (which does not matter at the pool sizes this allocator manages). An
        // earlier version of this method DID poison on any `AlreadyExists`, which Sol's review
        // correctly flagged as wrong; the fix is this per-candidate retry.
        //
        // A slot already in `active_slots` or `quarantined_slots` is skipped UNCONDITIONALLY,
        // BEFORE ever attempting to create its marker (Sol's review, round 3): checking these sets
        // only in response to `AlreadyExists` left a real race — if a slot's on-disk marker was
        // removed (by a racing `release()`'s `unlinkat_marker`, which happens BEFORE that same
        // `release()` re-acquires the lock just to update `active_slots`, or by external deletion)
        // while `active_slots` still names the slot as taken, `openat_marker`'s `O_EXCL` create
        // would SUCCEED (no file in the way) and this allocator would hand out a slot its own
        // bookkeeping still considers live — silently destroying another lease's identity. Since
        // `state`'s mutex is held for this whole method, and `release()` only ever removes a slot
        // from `active_slots` AFTER its unlink has durably completed, treating `active_slots`/
        // `quarantined_slots` membership as authoritative BEFORE attempting creation makes that
        // race impossible: a slot never becomes eligible for a fresh create until the in-memory
        // bookkeeping agrees it is free, regardless of what has or hasn't happened on disk yet.
        // Given that, `AlreadyExists` on a create attempt is now ALWAYS unexplained: a marker
        // exists for a slot this allocator's own bookkeeping said was free, meaning the leases
        // directory changed outside this allocator entirely — a global-trust failure, so it
        // poisons unconditionally rather than treating any `AlreadyExists` as ordinary.
        for slot in 0..self.pool_size {
            if state.active_slots.contains(&slot) || state.quarantined_slots.contains(&slot) {
                continue;
            }
            let host_uid = self.uid_start + slot;
            let host_gid = self.gid_start + slot;
            let lease_nonce = match random_u128() {
                Ok(n) => LeaseNonce(n),
                Err(e) => {
                    // An entropy failure must never mint a predictable/colliding identifier —
                    // poison rather than silently defaulting to a fixed nonce.
                    let reason = format!("lease: failed to generate a lease nonce: {e}");
                    state.admission = UserNamespaceAdmission::Poisoned {
                        reason: reason.clone(),
                    };
                    drop(state);
                    self.shared.report_incident(&reason);
                    return Err(UserNamespaceRefusal::Poisoned { reason });
                }
            };
            let marker = LeaseMarkerV2 {
                schema_version: LEASE_MARKER_SCHEMA_V2,
                lease_nonce,
                runner_instance_id: self.runner_instance_id,
                host_uid,
                host_gid,
                created_at_unix_secs: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
                phase: LeasePhaseV2::Allocated,
            };
            let marker_json = match serde_json::to_string(&marker) {
                Ok(json) => json,
                Err(e) => {
                    let reason = format!("lease: failed to serialize a new marker: {e}");
                    state.admission = UserNamespaceAdmission::Poisoned {
                        reason: reason.clone(),
                    };
                    drop(state);
                    self.shared.report_incident(&reason);
                    return Err(UserNamespaceRefusal::Poisoned { reason });
                }
            };
            let name = marker_file_name(slot);
            let write_result =
                openat_marker(self.shared.dir_fd(), &name, true).and_then(|mut file| {
                    io::Write::write_all(&mut file, marker_json.as_bytes())?;
                    file.sync_all()
                });
            match write_result {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    // Known (active/quarantined) slots were already skipped above, before this
                    // create was ever attempted — so an `AlreadyExists` here is ALWAYS unexplained:
                    // a marker exists for a slot this allocator's own bookkeeping said was free.
                    // Never guessed at.
                    let reason = format!(
                        "lease: slot {slot} already has an untracked marker (neither \
                         quarantined nor currently leased by this allocator instance) — the \
                         leases directory was modified outside this allocator's own bookkeeping"
                    );
                    state.admission = UserNamespaceAdmission::Poisoned {
                        reason: reason.clone(),
                    };
                    drop(state);
                    self.shared.report_incident(&reason);
                    return Err(UserNamespaceRefusal::Poisoned { reason });
                }
                Err(e) => {
                    let reason = format!(
                        "lease: creating marker for slot {slot} had an ambiguous outcome: {e}"
                    );
                    state.admission = UserNamespaceAdmission::Poisoned {
                        reason: reason.clone(),
                    };
                    drop(state);
                    self.shared.report_incident(&reason);
                    return Err(UserNamespaceRefusal::Poisoned { reason });
                }
            }
            if let Err(e) = self.shared.fsync_locked_dir() {
                let reason = format!(
                    "lease: slot {slot}'s marker was written but syncing the leases directory \
                     failed ({e}) — the marker's durability is unproven"
                );
                state.admission = UserNamespaceAdmission::Poisoned {
                    reason: reason.clone(),
                };
                drop(state);
                self.shared.report_incident(&reason);
                return Err(UserNamespaceRefusal::Poisoned { reason });
            }
            // The marker is already durably written and synced at this point, so silently
            // returning `Ok` on a failed insert would hand out a lease this allocator's own
            // bookkeeping cannot distinguish from a slot it already considers active.
            if let Err(reason) = insert_active_slot_checked(&mut state.active_slots, slot) {
                let reason = format!("lease: {reason}");
                state.admission = UserNamespaceAdmission::Poisoned {
                    reason: reason.clone(),
                };
                drop(state);
                self.shared.report_incident(&reason);
                return Err(UserNamespaceRefusal::Poisoned { reason });
            }
            drop(state);

            return Ok(UserNamespaceLease {
                slot,
                host_uid,
                host_gid,
                runner_uid: self.runner_uid,
                runner_gid: self.runner_gid,
                lease_nonce,
                runner_instance_id: self.runner_instance_id,
                shared: Arc::clone(&self.shared),
                released: false,
            });
        }
        Err(UserNamespaceRefusal::PoolExhausted {
            pool_size: self.pool_size,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex as StdMutex;

    fn unique_suffix() -> u64 {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        NEXT.fetch_add(1, Ordering::Relaxed)
    }

    fn test_base(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "myelin-user-namespace-tests-{tag}-{}-{}",
            std::process::id(),
            unique_suffix()
        ))
    }

    fn recording_sink() -> (IncidentSink, Arc<StdMutex<Vec<String>>>) {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let log_for_sink = Arc::clone(&log);
        let sink: IncidentSink = Arc::new(move |message: &str| {
            log_for_sink.lock().unwrap().push(message.to_string());
        });
        (sink, log)
    }

    /// Write a real, valid `subuid`/`subgid`-format file naming the CURRENT effective uid, with
    /// the given range, so allocator tests never depend on this host's REAL `/etc/subuid`.
    fn write_subordinate_file(path: &Path, start: u32, count: u32) {
        let uid = unsafe { libc::geteuid() };
        std::fs::write(path, format!("{uid}:{start}:{count}\n")).unwrap();
    }

    fn new_allocator_for_test(
        tag: &str,
        uid_count: u32,
        gid_count: u32,
    ) -> (UserNamespaceAllocator, PathBuf, Arc<StdMutex<Vec<String>>>) {
        let base = test_base(tag);
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, uid_count);
        write_subordinate_file(&subgid, 200_000, gid_count);
        let (sink, log) = recording_sink();
        let allocator = UserNamespaceAllocator::try_new_for_tests(
            leases_dir.clone(),
            &subuid,
            &subgid,
            uid_count.min(gid_count),
            sink,
        )
        .unwrap();
        (allocator, base, log)
    }

    fn release_for_tests(mut lease: UserNamespaceLease) {
        let nonce = lease.nonce_for_tests();
        lease
            .bind("test-container".to_string(), (0, 0), (0, 0))
            .expect("bind must succeed for a fresh Allocated lease");
        lease
            .release(UserNamespaceQuiescenceProof::assert_for_tests(
                nonce,
                "test-container".to_string(),
                (0, 0),
                (0, 0),
            ))
            .expect("release with the lease's own nonce and bound identity must succeed");
    }

    /// Tests that plant markers directly (bypassing `UserNamespaceAllocator::try_new`'s own
    /// directory creation) must create `leases_dir` at mode `0700` themselves, or
    /// `harden_and_verify_leases_dir` correctly refuses the group/other-accessible default mode
    /// `create_dir_all` leaves it at (subject to the process umask) before ever reaching the
    /// marker-scanning logic these tests actually target.
    fn create_hardened_leases_dir(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        let mut perms = std::fs::metadata(dir).unwrap().permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(dir, perms).unwrap();
    }

    #[test]
    fn subordinate_range_parsing_rejects_a_missing_entry() {
        let base = test_base("subrange-missing-entry");
        std::fs::create_dir_all(&base).unwrap();
        let subuid = base.join("subuid");
        std::fs::write(&subuid, "someoneelse:100000:65536\n").unwrap();
        let result = parse_subordinate_range(&subuid, unsafe { libc::geteuid() }, None, false);
        assert!(matches!(
            result,
            Err(UserNamespaceAllocatorError::NoSubordinateEntry { .. })
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn subordinate_range_parsing_rejects_a_zero_count() {
        let base = test_base("subrange-zero-count");
        std::fs::create_dir_all(&base).unwrap();
        let subuid = base.join("subuid");
        let uid = unsafe { libc::geteuid() };
        std::fs::write(&subuid, format!("{uid}:100000:0\n")).unwrap();
        let result = parse_subordinate_range(&subuid, uid, None, false);
        assert!(matches!(
            result,
            Err(UserNamespaceAllocatorError::SubordinateConfig { .. })
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn subordinate_range_parsing_rejects_ambiguous_duplicate_entries() {
        let base = test_base("subrange-ambiguous");
        std::fs::create_dir_all(&base).unwrap();
        let subuid = base.join("subuid");
        let uid = unsafe { libc::geteuid() };
        std::fs::write(&subuid, format!("{uid}:100000:65536\n{uid}:200000:1000\n")).unwrap();
        let result = parse_subordinate_range(&subuid, uid, None, false);
        assert!(matches!(
            result,
            Err(UserNamespaceAllocatorError::SubordinateConfig { .. })
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Sol's review, round 3: a syntactically valid entry for ANOTHER owner can overlap this
    /// uid's own selected range — both would then map the same host id, contradicting the "real,
    /// otherwise-unused subordinate id" guarantee.
    #[test]
    fn subordinate_range_parsing_rejects_an_overlap_with_another_owners_range() {
        let base = test_base("subrange-overlap");
        std::fs::create_dir_all(&base).unwrap();
        let subuid = base.join("subuid");
        let uid = unsafe { libc::geteuid() };
        // `uid`'s own range is 100000..165536; "someoneelse" overlaps it at 165000..175000.
        std::fs::write(
            &subuid,
            format!("{uid}:100000:65536\nsomeoneelse:165000:10000\n"),
        )
        .unwrap();
        let result = parse_subordinate_range(&subuid, uid, None, false);
        assert!(matches!(
            result,
            Err(UserNamespaceAllocatorError::SubordinateConfig { .. })
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn subordinate_range_parsing_accepts_a_non_overlapping_other_owner_entry() {
        let base = test_base("subrange-no-overlap");
        std::fs::create_dir_all(&base).unwrap();
        let subuid = base.join("subuid");
        let uid = unsafe { libc::geteuid() };
        std::fs::write(
            &subuid,
            format!("{uid}:100000:65536\nsomeoneelse:200000:65536\n"),
        )
        .unwrap();
        let result = parse_subordinate_range(&subuid, uid, None, false).unwrap();
        assert_eq!(
            result,
            SubordinateRange {
                start: 100000,
                count: 65536
            }
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn subordinate_range_parsing_rejects_overflowing_ranges() {
        let base = test_base("subrange-overflow");
        std::fs::create_dir_all(&base).unwrap();
        let subuid = base.join("subuid");
        let uid = unsafe { libc::geteuid() };
        std::fs::write(&subuid, format!("{uid}:{}:100\n", u32::MAX - 1)).unwrap();
        let result = parse_subordinate_range(&subuid, uid, None, false);
        assert!(matches!(
            result,
            Err(UserNamespaceAllocatorError::SubordinateConfig { .. })
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn subordinate_range_parsing_accepts_a_username_match() {
        let base = test_base("subrange-username-match");
        std::fs::create_dir_all(&base).unwrap();
        let subuid = base.join("subuid");
        std::fs::write(&subuid, "totally-not-our-uid:100000:65536\n").unwrap();
        let result = parse_subordinate_range(
            &subuid,
            /* a uid that will not numerically match */ 0,
            Some("totally-not-our-uid"),
            false,
        );
        assert_eq!(
            result.unwrap(),
            SubordinateRange {
                start: 100_000,
                count: 65_536
            }
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn subordinate_range_parsing_rejects_a_symlinked_file() {
        let base = test_base("subrange-symlink-refused");
        std::fs::create_dir_all(&base).unwrap();
        let real = base.join("real-subuid");
        let uid = unsafe { libc::geteuid() };
        std::fs::write(&real, format!("{uid}:100000:65536\n")).unwrap();
        let link = base.join("subuid-link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let result = parse_subordinate_range(&link, uid, None, false);
        assert!(matches!(
            result,
            Err(UserNamespaceAllocatorError::SubordinateConfig { .. })
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn pool_size_is_the_minimum_of_the_two_ranges() {
        let (allocator, base, _log) = new_allocator_for_test("pool-size-min", 5, 3);
        assert_eq!(allocator.pool_size(), 3);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn two_concurrent_leases_get_distinct_uid_gid_pairs() {
        let (allocator, base, _log) = new_allocator_for_test("distinct-pairs", 5, 5);
        let lease_a = allocator.lease().unwrap();
        let lease_b = allocator.lease().unwrap();
        assert_ne!(lease_a.host_uid(), lease_b.host_uid());
        assert_ne!(lease_a.host_gid(), lease_b.host_gid());
        release_for_tests(lease_a);
        release_for_tests(lease_b);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A genuine concurrency test (Sol's review — the sequential test above cannot exercise the
    /// race that used to mispoison the allocator): real OS threads race on `lease()` over a pool
    /// exactly as large as the thread count. Every thread must get a distinct slot; NONE may
    /// observe a poisoned allocator.
    #[test]
    fn concurrent_lease_calls_never_poison_the_allocator() {
        const THREADS: u32 = 8;
        let (allocator, base, _log) = new_allocator_for_test("real-concurrency", THREADS, THREADS);
        let allocator = Arc::new(allocator);
        let barrier = Arc::new(std::sync::Barrier::new(THREADS as usize));
        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let allocator = Arc::clone(&allocator);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    allocator.lease()
                })
            })
            .collect();
        // Collect EVERY lease first — never release one while any sibling thread might still be
        // outstanding. Releasing a lease frees its slot for reuse (by design), so releasing
        // lease 0 here WHILE thread 7 is still mid-retry would let thread 7 legitimately re-lease
        // that freed slot — a genuine, CORRECT allocator behavior that would masquerade as "two
        // threads leased the same host_uid" if release happened interleaved with the joins below.
        let leases: Vec<UserNamespaceLease> = handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap()
                    .expect("every concurrent lease() call must succeed, never observe poisoning")
            })
            .collect();
        let mut host_uids = BTreeSet::new();
        for lease in &leases {
            assert!(
                host_uids.insert(lease.host_uid()),
                "two threads leased the SAME host_uid — the race this test targets"
            );
        }
        assert!(allocator.is_healthy());
        for lease in leases {
            release_for_tests(lease);
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn lease_config_reports_the_exact_two_entry_mapping_shape() {
        let (allocator, base, _log) = new_allocator_for_test("config-shape", 5, 5);
        let lease = allocator.lease().unwrap();
        let config = lease.config();
        assert_eq!(config.runner_uid(), unsafe { libc::geteuid() });
        assert_eq!(config.runner_gid(), unsafe { libc::getegid() });
        assert_eq!(config.subordinate_uid(), lease.host_uid());
        assert_eq!(config.subordinate_gid(), lease.host_gid());
        release_for_tests(lease);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn pool_exhaustion_is_a_typed_refusal_not_poisoning() {
        let (allocator, base, _log) = new_allocator_for_test("pool-exhaustion", 2, 2);
        let lease_a = allocator.lease().unwrap();
        let lease_b = allocator.lease().unwrap();
        let refusal = allocator.lease().unwrap_err();
        assert_eq!(
            refusal,
            UserNamespaceRefusal::PoolExhausted { pool_size: 2 }
        );
        assert!(
            allocator.is_healthy(),
            "pool exhaustion must never poison the allocator"
        );
        release_for_tests(lease_a);
        release_for_tests(lease_b);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn releasing_a_lease_frees_its_slot_for_reuse() {
        let (allocator, base, _log) = new_allocator_for_test("release-frees-slot", 1, 1);
        let lease = allocator.lease().unwrap();
        let freed_uid = lease.host_uid();
        release_for_tests(lease);
        let lease_again = allocator.lease().unwrap();
        assert_eq!(lease_again.host_uid(), freed_uid);
        release_for_tests(lease_again);
        let _ = std::fs::remove_dir_all(&base);
    }

    // ───────────────── CT-007 slice 3, piece 7c: from_runtime_evidence ─────────────────

    #[test]
    fn from_runtime_evidence_mints_a_matching_proof_and_releases() {
        let (allocator, base, _log) = new_allocator_for_test("from-runtime-evidence-ok", 1, 1);
        let mut lease = allocator.lease().unwrap();
        lease
            .bind("container-1".to_string(), (7, 8), (9, 10))
            .expect("bind must succeed for a fresh Allocated lease");
        let evidence = crate::gvisor::RuntimeQuiescenceEvidence::assert_for_tests(
            "container-1".to_string(),
            crate::gvisor::RuntimeNamespaceQuiescence::ExplicitUserNamespace {
                runsc_root_identity: (7, 8),
            },
            crate::gvisor::CgroupQuiescenceEvidence::assert_for_tests((9, 10)),
        );
        let proof = UserNamespaceQuiescenceProof::from_runtime_evidence(&lease, &evidence)
            .expect("matching ExplicitUserNamespace evidence must mint a proof");
        lease
            .release(proof)
            .expect("release with the minted proof must succeed");
        assert!(allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn from_runtime_evidence_refuses_rootless_evidence() {
        let (allocator, base, _log) =
            new_allocator_for_test("from-runtime-evidence-rootless", 1, 1);
        let lease = allocator.lease().unwrap();
        let evidence = crate::gvisor::RuntimeQuiescenceEvidence::assert_for_tests(
            "container-1".to_string(),
            crate::gvisor::RuntimeNamespaceQuiescence::Rootless,
            crate::gvisor::CgroupQuiescenceEvidence::assert_for_tests((9, 10)),
        );
        let result = UserNamespaceQuiescenceProof::from_runtime_evidence(&lease, &evidence);
        assert!(matches!(
            result,
            Err(RuntimeEvidenceError::RootlessEvidence)
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    // ───────────────────── CT-007 slice 3: bind/release_unused lifecycle ─────────────────────

    #[test]
    fn bind_then_release_succeeds_with_a_matching_proof() {
        let (allocator, base, _log) = new_allocator_for_test("bind-then-release", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let nonce = lease.nonce_for_tests();
        lease
            .bind("container-1".to_string(), (7, 8), (9, 10))
            .expect("bind must succeed for a fresh Allocated lease");
        lease
            .release(UserNamespaceQuiescenceProof::assert_for_tests(
                nonce,
                "container-1".to_string(),
                (7, 8),
                (9, 10),
            ))
            .expect("release with a proof matching the bound identity must succeed");
        assert!(allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn bind_refuses_a_lease_that_is_already_bound() {
        let (allocator, base, _log) = new_allocator_for_test("bind-twice", 1, 1);
        let mut lease = allocator.lease().unwrap();
        lease
            .bind("container-1".to_string(), (1, 1), (1, 1))
            .expect("first bind must succeed");
        let result = lease.bind("container-2".to_string(), (2, 2), (2, 2));
        assert_eq!(result, Err(UserNamespaceBindError::MarkerMismatch));
        assert!(
            !allocator.is_healthy(),
            "a second bind attempt on an already-Bound marker must poison the allocator"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn bind_refuses_an_oversized_container_id_without_rewriting_the_marker() {
        let (allocator, base, _log) = new_allocator_for_test("bind-oversized-id", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let marker_path = base.join("leases").join(marker_file_name(0));
        let before = std::fs::read_to_string(&marker_path).unwrap();
        let oversized_id = "x".repeat(MAX_CONTAINER_ID_LEN + 1);
        let result = lease.bind(oversized_id, (1, 1), (1, 1));
        assert_eq!(result, Err(UserNamespaceBindError::InvalidContainerId));
        let after = std::fs::read_to_string(&marker_path).unwrap();
        assert_eq!(
            before, after,
            "an invalid container_id must be refused before any disk write is attempted"
        );
        assert!(
            allocator.is_healthy(),
            "an oversized container_id is a caller bug, not a global-trust failure — it must \
             not poison the allocator"
        );
        lease
            .release_unused()
            .expect("the lease is still Allocated and usable after the refused bind");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn bind_refuses_a_container_id_with_an_unsafe_character() {
        let (allocator, base, _log) = new_allocator_for_test("bind-unsafe-char", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let result = lease.bind("has a space".to_string(), (1, 1), (1, 1));
        assert_eq!(result, Err(UserNamespaceBindError::InvalidContainerId));
        assert!(allocator.is_healthy());
        lease.release_unused().expect("lease is still Allocated");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Sol's review: `release` must verify the proof's claimed runtime identity against what the
    /// durable marker's `Bound` phase ACTUALLY records, not merely the lease nonce — a proof
    /// minted for the right lease but the WRONG runtime instance must still be refused.
    #[test]
    fn release_refuses_a_proof_whose_bound_identity_disagrees_with_the_durable_marker() {
        let (allocator, base, _log) = new_allocator_for_test("release-wrong-identity", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let nonce = lease.nonce_for_tests();
        lease
            .bind("real-container".to_string(), (1, 1), (2, 2))
            .expect("bind must succeed");
        let result = lease.release(UserNamespaceQuiescenceProof::assert_for_tests(
            nonce,
            "different-container".to_string(), // right nonce, WRONG bound identity
            (1, 1),
            (2, 2),
        ));
        assert_eq!(
            result,
            Err(UserNamespaceReleaseError::ProofDisagreesWithMarker)
        );
        assert!(
            allocator.is_healthy(),
            "a proof with the wrong bound identity is an ordinary wrong proof for a valid lease, \
             not corruption — it must NOT poison the whole allocator"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn release_refuses_a_lease_that_was_never_bound() {
        let (allocator, base, _log) = new_allocator_for_test("release-never-bound", 1, 1);
        let lease = allocator.lease().unwrap();
        let nonce = lease.nonce_for_tests();
        // Never called `.bind()` — the marker is still Allocated, not Bound.
        let result = lease.release(UserNamespaceQuiescenceProof::assert_for_tests(
            nonce,
            "container-1".to_string(),
            (1, 1),
            (1, 1),
        ));
        assert_eq!(
            result,
            Err(UserNamespaceReleaseError::ProofDisagreesWithMarker)
        );
        assert!(
            allocator.is_healthy(),
            "a never-bound marker still genuinely belongs to this lease — releasing it with a \
             real-looking proof is an ordinary wrong proof, not corruption"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The whole point of `release_unused`: a pre-spawn failure or a refused launch permit must
    /// not permanently quarantine the subordinate id just because the lease was never exposed to
    /// a runtime.
    #[test]
    fn release_unused_succeeds_for_a_never_bound_lease_and_frees_its_slot() {
        let (allocator, base, _log) = new_allocator_for_test("release-unused-ok", 1, 1);
        let lease = allocator.lease().unwrap();
        let freed_uid = lease.host_uid();
        lease
            .release_unused()
            .expect("release_unused must succeed for a never-bound Allocated lease");
        assert!(allocator.is_healthy());
        let lease_again = allocator.lease().unwrap();
        assert_eq!(
            lease_again.host_uid(),
            freed_uid,
            "release_unused must genuinely free the slot for reuse"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// `release_unused` must REFUSE a lease that WAS bound — that identity was actually exposed to
    /// a runtime, so the caller needed a real quiescence proof (`release`), not the no-proof-needed
    /// path.
    #[test]
    fn release_unused_refuses_a_lease_that_was_already_bound() {
        let (allocator, base, _log) = new_allocator_for_test("release-unused-wrong-path", 1, 1);
        let mut lease = allocator.lease().unwrap();
        lease
            .bind("container-1".to_string(), (1, 1), (1, 1))
            .expect("bind must succeed");
        let result = lease.release_unused();
        assert_eq!(result, Err(UserNamespaceReleaseError::MarkerMismatch));
        assert!(
            !allocator.is_healthy(),
            "release_unused on an already-Bound lease must poison the allocator, not silently \
             unlink real runtime-binding evidence"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Sol's round-2 review: the newly checked bookkeeping invariants (`active_slots.remove()`'s
    /// return value on the successful-unlink path) had no always-on coverage. Simulate the
    /// "should be impossible under this method's own lock hold" corruption directly, by removing
    /// the slot from `active_slots` behind the lease's back before it is ever released.
    #[test]
    fn release_surfaces_an_internal_invariant_violation_when_active_slots_lost_the_entry() {
        let (allocator, base, _log) = new_allocator_for_test("release-invariant-violation", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let nonce = lease.nonce_for_tests();
        lease
            .bind("container-1".to_string(), (1, 1), (1, 1))
            .expect("bind must succeed");
        let slot = lease.slot;
        lease.shared.lock_state().active_slots.remove(&slot);
        let result = lease.release(UserNamespaceQuiescenceProof::assert_for_tests(
            nonce,
            "container-1".to_string(),
            (1, 1),
            (1, 1),
        ));
        match result {
            Err(UserNamespaceReleaseError::InternalInvariantViolated { reason }) => {
                assert!(reason.contains("bookkeeping invariant"));
            }
            other => panic!("expected InternalInvariantViolated, got {other:?}"),
        }
        assert!(
            !allocator.is_healthy(),
            "a lost active_slots entry is a genuine bookkeeping corruption and must poison the \
             whole allocator"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Same invariant, exercised via `release_unused`'s never-bound path.
    #[test]
    fn release_unused_surfaces_an_internal_invariant_violation_when_active_slots_lost_the_entry() {
        let (allocator, base, _log) =
            new_allocator_for_test("release-unused-invariant-violation", 1, 1);
        let lease = allocator.lease().unwrap();
        let slot = lease.slot;
        lease.shared.lock_state().active_slots.remove(&slot);
        let result = lease.release_unused();
        match result {
            Err(UserNamespaceReleaseError::InternalInvariantViolated { reason }) => {
                assert!(reason.contains("bookkeeping invariant"));
            }
            other => panic!("expected InternalInvariantViolated, got {other:?}"),
        }
        assert!(!allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Sol's round-2 review: `lease()`'s newly checked `active_slots.insert()` return value also
    /// had no always-on coverage. `lease()` itself holds its state mutex for its entire duration,
    /// so the violation this check defends against is unreachable through the public API in a
    /// single-threaded test — there is no window to mutate `active_slots` between the pre-check
    /// skip and the `insert` call. So: test the extracted [`insert_active_slot_checked`] helper
    /// directly (Sol's suggestion) — it holds the actual logic `lease()` calls, just without
    /// requiring a whole allocator/lock to exercise it.
    #[test]
    fn insert_active_slot_checked_detects_a_bookkeeping_invariant_violation() {
        let mut active_slots = BTreeSet::new();
        active_slots.insert(3);
        let result = insert_active_slot_checked(&mut active_slots, 3);
        assert!(result.unwrap_err().contains("bookkeeping invariant"));
    }

    #[test]
    fn insert_active_slot_checked_succeeds_for_a_fresh_slot() {
        let mut active_slots = BTreeSet::new();
        assert!(insert_active_slot_checked(&mut active_slots, 3).is_ok());
        assert!(active_slots.contains(&3));
    }

    /// Sol's review: a proof minted for lease A must never be able to release lease B.
    #[test]
    fn a_proof_minted_for_one_lease_cannot_release_another() {
        let (allocator, base, _log) = new_allocator_for_test("proof-cannot-cross-leases", 2, 2);
        let lease_a = allocator.lease().unwrap();
        let lease_b = allocator.lease().unwrap();
        let wrong_proof = UserNamespaceQuiescenceProof::assert_for_tests(
            lease_a.nonce_for_tests(),
            "irrelevant".to_string(),
            (0, 0),
            (0, 0),
        );
        let result = lease_b.release(wrong_proof);
        assert_eq!(result, Err(UserNamespaceReleaseError::ProofMismatch));
        // lease_b was returned by the failed `release` call above via `Err` — but `release` takes
        // `self` by value, so on ProofMismatch the lease is DROPPED here (Rust drops the `self` it
        // was given even though the method returned early) — meaning it quarantines exactly as an
        // abandoned lease would. This IS the correct fail-closed outcome: a caller that supplies
        // the wrong proof and has no other reference to the lease cannot retry, so quarantining is
        // the safe default. Assert that outcome explicitly.
        assert!(
            allocator.is_healthy(),
            "a proof mismatch must not poison the WHOLE allocator"
        );
        assert!(
            allocator.quarantined_slots().contains(&lease_b_slot_hint()),
            "lease_b's slot must be quarantined once its (consumed-by-the-failed-call) value drops"
        );
        release_for_tests(lease_a);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Slot 1 is deterministic given the 2x2 pool and lease-order in the test above (lease_a takes
    /// slot 0, lease_b takes slot 1) — named out for the assertion's clarity.
    fn lease_b_slot_hint() -> u32 {
        1
    }

    #[test]
    fn abandoning_a_lease_quarantines_only_that_slot_and_reports_an_incident() {
        let (allocator, base, log) = new_allocator_for_test("abandon-quarantines-one", 2, 2);
        let lease_a = allocator.lease().unwrap();
        let slot_a_uid = lease_a.host_uid();
        drop(lease_a); // abandoned — never released
        assert!(
            log.lock()
                .unwrap()
                .iter()
                .any(|m| m.contains("quarantined")),
            "an abandoned lease must report an incident"
        );
        assert!(
            allocator.is_healthy(),
            "an abandoned lease quarantines ONE slot, never the whole allocator"
        );
        let lease_b = allocator.lease().unwrap();
        assert_ne!(lease_b.host_uid(), slot_a_uid);
        let refusal = allocator.lease().unwrap_err();
        assert_eq!(
            refusal,
            UserNamespaceRefusal::PoolExhausted { pool_size: 2 }
        );
        release_for_tests(lease_b);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_second_allocator_over_the_same_leases_dir_refuses_the_lock() {
        let base = test_base("second-allocator-refused");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 5);
        write_subordinate_file(&subgid, 200_000, 5);
        let (sink_a, _log_a) = recording_sink();
        let _allocator_a = UserNamespaceAllocator::try_new_for_tests(
            leases_dir.clone(),
            &subuid,
            &subgid,
            1,
            sink_a,
        )
        .unwrap();
        let (sink_b, _log_b) = recording_sink();
        let result_b =
            UserNamespaceAllocator::try_new_for_tests(leases_dir, &subuid, &subgid, 1, sink_b);
        assert!(matches!(
            result_b,
            Err(UserNamespaceAllocatorError::AlreadyLocked { .. })
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn dropping_the_allocator_while_a_lease_is_outstanding_keeps_the_lock_held() {
        let base = test_base("lock-outlives-allocator-via-lease");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 5);
        write_subordinate_file(&subgid, 200_000, 5);
        let (sink, _log) = recording_sink();
        let allocator = UserNamespaceAllocator::try_new_for_tests(
            leases_dir.clone(),
            &subuid,
            &subgid,
            1,
            sink,
        )
        .unwrap();
        let lease = allocator.lease().unwrap();
        drop(allocator); // would release the flock IF the lock lived on the allocator alone.

        let (second_sink, _second_log) = recording_sink();
        let second_attempt =
            UserNamespaceAllocator::try_new_for_tests(leases_dir, &subuid, &subgid, 1, second_sink);
        match second_attempt {
            Err(UserNamespaceAllocatorError::AlreadyLocked { .. }) => {}
            Err(other) => panic!("expected AlreadyLocked, got a different error: {other:?}"),
            Ok(_) => panic!(
                "expected a second allocator to be refused while the first allocator's lease is \
                 still outstanding, but it succeeded"
            ),
        }

        drop(lease); // abandoned (never released) — releases the lock; poisons this dead
                     // allocator's own state, which is fine, nothing else observes it.
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The real end-to-end scenario the u128-precision bug broke: a genuinely random lease is
    /// created, abandoned (its marker survives, exactly as a crash would leave it), and a FRESH
    /// allocator reopening the same leases dir must successfully parse and quarantine it — never
    /// treat a real random `u128` marker as corrupt.
    #[test]
    fn a_genuinely_random_abandoned_lease_survives_reopening_and_is_quarantined() {
        let base = test_base("real-random-marker-survives-reboot");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 5);
        write_subordinate_file(&subgid, 200_000, 5);

        let (sink, _log) = recording_sink();
        let allocator = UserNamespaceAllocator::try_new_for_tests(
            leases_dir.clone(),
            &subuid,
            &subgid,
            1,
            sink,
        )
        .unwrap();
        let lease = allocator.lease().unwrap();
        let leaked_uid = lease.host_uid();
        drop(lease); // abandoned — its marker (with a REAL random u128 nonce) survives on disk.
        drop(allocator); // releases the lock (simulating the runner process exiting).

        let (sink2, log2) = recording_sink();
        let reopened =
            UserNamespaceAllocator::try_new_for_tests(leases_dir, &subuid, &subgid, 1, sink2)
                .unwrap();
        assert!(
            reopened.is_healthy(),
            "a real random-u128 marker must parse successfully at boot, not be treated as corrupt"
        );
        assert!(reopened.quarantined_slots().contains(&0));
        assert!(log2
            .lock()
            .unwrap()
            .iter()
            .any(|m| m.contains("surviving Allocated marker")),);
        let lease2 = reopened.lease().unwrap();
        assert_ne!(
            lease2.host_uid(),
            leaked_uid,
            "the leaked slot's host_uid must never be reissued"
        );
        release_for_tests(lease2);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Sol's round-1 review: no existing test proved a surviving `Bound` marker (as opposed to an
    /// `Allocated` one) parses and quarantines correctly at boot — the boot-reconciliation loop
    /// deserializes `LeaseMarkerV2` regardless of phase, but nothing exercised the `Bound` arm.
    #[test]
    fn a_bound_lease_survives_reopening_and_is_quarantined() {
        let base = test_base("bound-marker-survives-reboot");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 5);
        write_subordinate_file(&subgid, 200_000, 5);

        let (sink, _log) = recording_sink();
        let allocator = UserNamespaceAllocator::try_new_for_tests(
            leases_dir.clone(),
            &subuid,
            &subgid,
            1,
            sink,
        )
        .unwrap();
        let mut lease = allocator.lease().unwrap();
        let leaked_uid = lease.host_uid();
        lease
            .bind("crashed-container".to_string(), (7, 8), (9, 10))
            .expect("bind must succeed for a fresh Allocated lease");
        drop(lease); // abandoned mid-run — its Bound marker survives on disk, exactly as a crash
                     // between bind() and a real teardown proof would leave it.
        drop(allocator); // releases the lock (simulating the runner process exiting).

        let (sink2, log2) = recording_sink();
        let reopened =
            UserNamespaceAllocator::try_new_for_tests(leases_dir, &subuid, &subgid, 1, sink2)
                .unwrap();
        assert!(
            reopened.is_healthy(),
            "a surviving Bound marker must parse successfully at boot, not be treated as corrupt"
        );
        assert!(reopened.quarantined_slots().contains(&0));
        assert!(log2
            .lock()
            .unwrap()
            .iter()
            .any(|m| m.contains("Bound") && m.contains("crashed-container")));
        let lease2 = reopened.lease().unwrap();
        assert_ne!(
            lease2.host_uid(),
            leaked_uid,
            "the leaked slot's host_uid must never be reissued"
        );
        release_for_tests(lease2);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Sol's review, 2026-07-27: a marker written by a PRE-5b.1 binary (the frozen, 2-variant
    /// `schema_version: 1` shape) must still be recognized and quarantined cleanly by THIS code —
    /// proving the schema version bump didn't break reading genuinely old markers, only writing
    /// them (this process never writes schema_version 1 again; see `LeaseMarkerV1`'s own doc).
    #[test]
    fn a_legacy_schema_v1_bound_marker_survives_reopening_and_is_quarantined() {
        let base = test_base("legacy-v1-marker-survives-reboot");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 5);
        write_subordinate_file(&subgid, 200_000, 5);
        create_hardened_leases_dir(&leases_dir);
        let legacy_marker = LeaseMarkerV1 {
            schema_version: LEASE_MARKER_SCHEMA_V1,
            lease_nonce: LeaseNonce(1),
            runner_instance_id: RunnerInstanceId(1),
            host_uid: 100_000,
            host_gid: 200_000,
            created_at_unix_secs: 0,
            phase: LeasePhaseV1::Bound {
                container_id: "pre-5b1-container".to_string(),
                runsc_root_identity: (7, 8),
                cgroup_identity: (9, 10),
            },
        };
        let legacy_marker_path = leases_dir.join(marker_file_name(0));
        std::fs::write(
            &legacy_marker_path,
            serde_json::to_string(&legacy_marker).unwrap(),
        )
        .unwrap();
        // `read_and_verify_marker` requires owner-only permissions; match what the real
        // `rewrite_marker_atomically` creates its own tmp files with (0600), since the default
        // umask-applied mode from a plain `std::fs::write` is group/other-readable.
        std::fs::set_permissions(&legacy_marker_path, std::fs::Permissions::from_mode(0o600))
            .unwrap();

        let (sink, log) = recording_sink();
        let allocator = UserNamespaceAllocator::try_new_for_tests(
            leases_dir.clone(),
            &subuid,
            &subgid,
            1,
            sink,
        )
        .unwrap();
        assert!(
            allocator.is_healthy(),
            "a legacy schema_version=1 marker must parse successfully at boot, not be treated as \
             corrupt"
        );
        assert!(allocator.quarantined_slots().contains(&0));
        assert!(log
            .lock()
            .unwrap()
            .iter()
            .any(|m| m.contains("legacy schema_version=1")
                && m.contains("Bound")
                && m.contains("pre-5b1-container")));
        // A freshly minted lease (necessarily a DIFFERENT slot -- slot 0 stays quarantined forever)
        // must always be written as V2 going forward.
        let fresh = allocator.lease().unwrap();
        let fresh_marker: String =
            std::fs::read_to_string(leases_dir.join(marker_file_name(fresh.host_uid() - 100_000)))
                .unwrap();
        assert!(
            fresh_marker.contains("\"schema_version\":2"),
            "every NEW marker this process mints must be schema_version 2, never 1 again"
        );
        release_for_tests(fresh);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Sol's round-1 review: a crash between creating `<marker>.tmp` (inside `bind()`) and the
    /// `renameat` that would have made it the real marker leaves an undefined artifact behind —
    /// boot reconciliation must recognize it specifically and quarantine only that slot, not treat
    /// it as unrecognized-entry corruption that poisons the whole allocator.
    #[test]
    fn a_stray_bind_tmp_file_survives_reopening_and_only_quarantines_its_own_slot() {
        let base = test_base("stray-bind-tmp-survives-reboot");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 5);
        write_subordinate_file(&subgid, 200_000, 5);

        let (sink, _log) = recording_sink();
        let allocator = UserNamespaceAllocator::try_new_for_tests(
            leases_dir.clone(),
            &subuid,
            &subgid,
            2,
            sink,
        )
        .unwrap();
        let lease_a = allocator.lease().unwrap();
        let lease_b = allocator.lease().unwrap();
        let leaked_uid = lease_a.host_uid();
        // Simulate the exact crash window `rewrite_marker_atomically` is meant to close: the tmp
        // file was created and fsynced, but the process died before the rename made it real — so
        // BOTH the original (still Allocated) marker and the orphaned tmp file coexist on disk.
        std::fs::write(
            leases_dir.join(format!("{}.tmp", marker_file_name(0))),
            b"whatever bind() had written - content is irrelevant, only the name matters",
        )
        .unwrap();
        drop(lease_a); // abandoned mid-crash — slot 0's real marker is untouched (still Allocated).
        drop(lease_b); // abandon slot 1's own real marker too, so both slots are exercised.
        drop(allocator);

        let (sink2, log2) = recording_sink();
        let reopened = UserNamespaceAllocator::try_new_for_tests(
            leases_dir.clone(),
            &subuid,
            &subgid,
            2,
            sink2,
        )
        .unwrap();
        assert!(
            reopened.is_healthy(),
            "a stray bind-rewrite temp file must never poison the whole allocator"
        );
        assert!(
            reopened.quarantined_slots().contains(&0),
            "the slot the stray temp file names must still be quarantined, conservatively"
        );
        assert!(reopened.quarantined_slots().contains(&1));
        assert!(log2
            .lock()
            .unwrap()
            .iter()
            .any(|m| m.contains("stale bind-rewrite temp file")));
        assert!(
            !leases_dir
                .join(format!("{}.tmp", marker_file_name(0)))
                .exists(),
            "the stray temp file must be cleaned up once its slot is quarantined"
        );
        let lease2 = reopened.lease().unwrap();
        assert_ne!(lease2.host_uid(), leaked_uid);
        release_for_tests(lease2);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Sol's round-2 review: a stray bind-rewrite temp file with NO corresponding primary marker
    /// (the primary was deleted, moved, or never existed for some other reason) must REFUSE
    /// construction rather than silently deleting the only surviving evidence for that slot and
    /// letting a later boot reissue it — that would violate the never-reissue guarantee.
    #[test]
    fn a_stray_bind_tmp_file_without_its_primary_marker_refuses_construction() {
        let base = test_base("stray-bind-tmp-without-primary");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 2);
        write_subordinate_file(&subgid, 200_000, 2);
        // A first construction creates and hardens `leases_dir` for us (0700, owned by us) —
        // exactly as it would exist in production before any crash.
        let (sink, _log) = recording_sink();
        drop(
            UserNamespaceAllocator::try_new_for_tests(
                leases_dir.clone(),
                &subuid,
                &subgid,
                2,
                sink,
            )
            .unwrap(),
        );
        // No primary marker at all for slot 0 — only the crash-window temp file survives.
        std::fs::write(
            leases_dir.join(format!("{}.tmp", marker_file_name(0))),
            b"whatever bind() had written - content is irrelevant, only the name matters",
        )
        .unwrap();

        let (sink2, _log2) = recording_sink();
        let result = UserNamespaceAllocator::try_new_for_tests(
            leases_dir.clone(),
            &subuid,
            &subgid,
            2,
            sink2,
        );
        match result {
            Err(UserNamespaceAllocatorError::CorruptLeaseMarker { reason, .. }) => {
                assert!(
                    reason.contains("no primary marker for that slot survived"),
                    "unexpected reason: {reason}"
                );
            }
            Err(other) => panic!("expected CorruptLeaseMarker, got {other:?}"),
            Ok(_) => panic!("expected CorruptLeaseMarker, got Ok"),
        }
        assert!(
            leases_dir
                .join(format!("{}.tmp", marker_file_name(0)))
                .exists(),
            "refusing construction must leave the only surviving evidence untouched, not delete it"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn boot_reconciliation_poisons_construction_on_an_unrecognized_entry() {
        let base = test_base("boot-poisons-on-unrecognized-entry");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 5);
        write_subordinate_file(&subgid, 200_000, 5);
        create_hardened_leases_dir(&leases_dir);
        std::fs::write(leases_dir.join("not-a-marker.txt"), b"garbage").unwrap();

        let (sink, _log) = recording_sink();
        let result =
            UserNamespaceAllocator::try_new_for_tests(leases_dir, &subuid, &subgid, 1, sink);
        assert!(matches!(
            result,
            Err(UserNamespaceAllocatorError::CorruptLeaseMarker { .. })
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn boot_reconciliation_poisons_construction_on_an_unknown_schema_version() {
        let base = test_base("boot-poisons-on-unknown-schema");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 5);
        write_subordinate_file(&subgid, 200_000, 5);
        create_hardened_leases_dir(&leases_dir);
        std::fs::write(
            leases_dir.join(marker_file_name(0)),
            r#"{"schema_version": 999, "nonsense": true}"#,
        )
        .unwrap();

        let (sink, _log) = recording_sink();
        let result =
            UserNamespaceAllocator::try_new_for_tests(leases_dir, &subuid, &subgid, 1, sink);
        assert!(matches!(
            result,
            Err(UserNamespaceAllocatorError::CorruptLeaseMarker { .. })
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn boot_reconciliation_poisons_construction_when_a_marker_names_an_out_of_range_slot() {
        let base = test_base("boot-poisons-out-of-range-slot");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 2);
        write_subordinate_file(&subgid, 200_000, 2);
        create_hardened_leases_dir(&leases_dir);
        let marker = LeaseMarkerV2 {
            schema_version: LEASE_MARKER_SCHEMA_V2,
            lease_nonce: LeaseNonce(1),
            runner_instance_id: RunnerInstanceId(1),
            host_uid: 100_005,
            host_gid: 200_005,
            created_at_unix_secs: 0,
            phase: LeasePhaseV2::Allocated,
        };
        std::fs::write(
            leases_dir.join(marker_file_name(5)),
            serde_json::to_string(&marker).unwrap(),
        )
        .unwrap();

        let (sink, _log) = recording_sink();
        let result =
            UserNamespaceAllocator::try_new_for_tests(leases_dir, &subuid, &subgid, 1, sink);
        assert!(matches!(
            result,
            Err(UserNamespaceAllocatorError::CorruptLeaseMarker { .. })
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Sol's review: a surviving marker whose `host_uid`/`host_gid` disagree with what the
    /// CURRENT subordinate ranges imply for its own slot number (the range start changed since the
    /// marker was written) must refuse construction — accepting it could let a NEW allocation at a
    /// DIFFERENT slot reissue the exact host identity the stale marker still names.
    #[test]
    fn boot_reconciliation_poisons_construction_on_a_range_start_mismatch() {
        let base = test_base("boot-poisons-range-start-mismatch");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        // Marker was written when the range started at 100000 (slot 0 -> host_uid 100000).
        // The range has since moved to start at 100005 — slot 0 now implies host_uid 100005.
        write_subordinate_file(&subuid, 100_005, 5);
        write_subordinate_file(&subgid, 200_005, 5);
        create_hardened_leases_dir(&leases_dir);
        let marker = LeaseMarkerV2 {
            schema_version: LEASE_MARKER_SCHEMA_V2,
            lease_nonce: LeaseNonce(1),
            runner_instance_id: RunnerInstanceId(1),
            host_uid: 100_000,
            host_gid: 200_000,
            created_at_unix_secs: 0,
            phase: LeasePhaseV2::Allocated,
        };
        std::fs::write(
            leases_dir.join(marker_file_name(0)),
            serde_json::to_string(&marker).unwrap(),
        )
        .unwrap();

        let (sink, _log) = recording_sink();
        let result =
            UserNamespaceAllocator::try_new_for_tests(leases_dir, &subuid, &subgid, 1, sink);
        assert!(matches!(
            result,
            Err(UserNamespaceAllocatorError::CorruptLeaseMarker { .. })
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn boot_reconciliation_poisons_construction_on_a_symlinked_marker_entry() {
        let base = test_base("boot-poisons-symlinked-marker");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 5);
        write_subordinate_file(&subgid, 200_000, 5);
        create_hardened_leases_dir(&leases_dir);
        let real = base.join("real-marker.json");
        std::fs::write(&real, b"irrelevant").unwrap();
        std::os::unix::fs::symlink(&real, leases_dir.join(marker_file_name(0))).unwrap();

        let (sink, _log) = recording_sink();
        let result =
            UserNamespaceAllocator::try_new_for_tests(leases_dir, &subuid, &subgid, 1, sink);
        assert!(matches!(
            result,
            Err(UserNamespaceAllocatorError::CorruptLeaseMarker { .. })
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn leases_dir_as_a_symlink_is_refused() {
        let base = test_base("leases-dir-symlink-refused");
        std::fs::create_dir_all(&base).unwrap();
        let real = base.join("real-leases");
        std::fs::create_dir_all(&real).unwrap();
        let link = base.join("leases-link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 5);
        write_subordinate_file(&subgid, 200_000, 5);

        let (sink, _log) = recording_sink();
        let result = UserNamespaceAllocator::try_new_for_tests(link, &subuid, &subgid, 1, sink);
        assert!(matches!(
            result,
            Err(UserNamespaceAllocatorError::UnsafeLeasesDir { .. })
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_panicking_incident_sink_never_escapes_an_abandoned_lease() {
        let base = test_base("panicking-sink-abandon");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 5);
        write_subordinate_file(&subgid, 200_000, 5);
        let sink: IncidentSink = Arc::new(|_message: &str| panic!("injected sink panic"));
        let allocator =
            UserNamespaceAllocator::try_new_for_tests(leases_dir, &subuid, &subgid, 1, sink)
                .unwrap();
        let lease = allocator.lease().unwrap();
        drop(lease); // triggers Drop -> quarantine_slot -> the panicking sink; must not propagate.
        assert!(allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn check_identity_succeeds_while_the_leases_dir_is_unchanged() {
        let (allocator, base, _log) = new_allocator_for_test("check-identity-happy-path", 5, 5);
        assert!(allocator.check_identity().is_ok());
        assert!(allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn check_identity_detects_a_replaced_leases_dir_and_poisons_the_allocator() {
        let (allocator, base, _log) = new_allocator_for_test("check-identity-replaced", 5, 5);
        let leases_dir = base.join("leases");
        std::fs::remove_dir_all(&leases_dir).unwrap();
        std::fs::create_dir_all(&leases_dir).expect("recreate a replacement directory");
        let result = allocator.check_identity();
        assert!(matches!(result, Err(UserNamespaceRefusal::Poisoned { .. })));
        assert!(!allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Sol's review, the main blocker of round 2: FD-relative marker operations protect one
    /// allocator from a REPLACEMENT directory, but nothing stopped THIS SAME process from
    /// renaming `leases_dir` away and creating a fresh one at the same path, letting a second
    /// allocator falsely lock the "new" (empty) directory while the first still believed it
    /// exclusively owned its slots. The fix: the STRICT (production) constructor refuses to start
    /// at all unless `leases_dir`'s PARENT is not writable by this process's own euid — since this
    /// test's temp-dir parent (like virtually every host's `/tmp`) IS world-writable, the strict
    /// constructor must refuse it outright, never merely warn.
    #[test]
    fn strict_construction_refuses_a_leases_dir_whose_parent_is_writable_by_us() {
        // Isolates JUST the ancestor-writability check (`verify_ancestors_not_writable_by_us`,
        // called by `harden_and_verify_leases_dir` only in strict/production mode) rather than
        // driving the full `try_new_impl(..., strict = true)`, whose OTHER strict checks (the
        // subordinate files must be root-owned) would refuse first against this test's own
        // fixture files — that is a real, separate, ALSO-correct refusal, not the one this test
        // targets.
        let base = test_base("strict-refuses-writable-parent");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        create_hardened_leases_dir(&leases_dir);
        // `base` (this test's own temp directory, owned by this process) is `leases_dir`'s
        // parent — writable by us, exactly the condition this check must refuse.
        let result = verify_ancestors_not_writable_by_us(&leases_dir);
        assert!(matches!(
            result,
            Err(UserNamespaceAllocatorError::UnsafeLeasesDir { .. })
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Sol's review, round 7: rejecting group/other bits alone still admits a mode like `0500`
    /// (owner cannot write) or `0000` (owner cannot even search it) — both unusable for actually
    /// creating/reading lease markers, despite passing the group/other-only check. Isolates JUST
    /// `verify_leases_dir_leaf_strict` (not the full `harden_and_verify_leases_dir`, whose
    /// ancestor check would refuse first against any fixture under a writable temp directory).
    #[test]
    fn verify_leases_dir_leaf_strict_refuses_an_owner_non_writable_directory() {
        let base = test_base("leases-dir-0500");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        std::fs::create_dir_all(&leases_dir).unwrap();
        let mut perms = std::fs::metadata(&leases_dir).unwrap().permissions();
        perms.set_mode(0o500); // r-x------: owner cannot write, though group/other bits are clear.
        std::fs::set_permissions(&leases_dir, perms).unwrap();
        let result = verify_leases_dir_leaf_strict(&leases_dir);
        let mut restore = std::fs::metadata(&leases_dir).unwrap().permissions();
        restore.set_mode(0o700);
        std::fs::set_permissions(&leases_dir, restore).unwrap();
        let _ = std::fs::remove_dir_all(&base);
        assert!(
            result.is_err(),
            "an owner-non-writable leases dir must be refused even with no group/other bits: \
             {result:?}"
        );
    }

    /// Sol's review, round 3: the round-2 check used plain `access(2)` (mode bits only), which an
    /// ancestor's OWNER can defeat trivially — owning a directory permits `chmod`'ing it writable
    /// at any time, regardless of its CURRENT mode. `0o555` (no write bit set for anyone) must
    /// still be refused when this process is the owner.
    #[test]
    fn ancestor_owned_by_us_is_refused_even_when_its_current_mode_denies_write() {
        // Isolates JUST `crate::dirlock::check_ancestor_not_owned_or_writable` against a real fd
        // for the owned, mode-0o555 fixture directly, rather than driving the full
        // `verify_ancestors_not_writable_by_us` walk from `/` — that walk would refuse first at
        // `/tmp` itself (world-writable on virtually every host), never actually reaching this
        // fixture, which would prove nothing about the ownership-vs-mode gap this test targets
        // (Sol's review, round 4).
        let base = test_base("ancestor-owned-read-only");
        std::fs::create_dir_all(&base).unwrap();
        let mut perms = std::fs::metadata(&base).unwrap().permissions();
        perms.set_mode(0o555); // r-xr-xr-x: no write bit for anyone, including the owner (us).
        std::fs::set_permissions(&base, perms).unwrap();

        let base_c = CString::new(base.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: `base_c` is a valid, NUL-terminated path to a real directory this test created.
        let base_fd = unsafe {
            libc::open(
                base_c.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        };
        assert!(
            base_fd >= 0,
            "open base dir: {}",
            io::Error::last_os_error()
        );
        use std::os::fd::FromRawFd;
        // SAFETY: `base_fd` was just returned by a successful `open` above.
        let owned_fd = unsafe { OwnedFd::from_raw_fd(base_fd) };
        let result = crate::dirlock::check_ancestor_not_owned_or_writable(&owned_fd, &base);

        // Restore write access so the test harness can clean up `base` afterward.
        let mut restore = std::fs::metadata(&base).unwrap().permissions();
        restore.set_mode(0o755);
        std::fs::set_permissions(&base, restore).unwrap();
        assert!(
            result.is_err(),
            "an ancestor owned by this process must be refused regardless of its current mode, \
             since ownership alone permits chmod'ing it writable: {result:?}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Sol's review, round 3: a symlinked ancestor anywhere in the chain (not only at the leaf)
    /// must be refused rather than silently followed. Isolates JUST
    /// `crate::dirlock::open_dir_component_no_follow` (the primitive
    /// `verify_ancestors_not_writable_by_us`'s walk uses for every component) against a real
    /// symlink, rather than driving the full walk — the full walk's OWNERSHIP check would refuse
    /// first against any ancestor this non-privileged test itself creates (every such ancestor is
    /// necessarily owned by the test process), which is a real, separate, ALSO-correct refusal but
    /// not the one this test targets.
    #[test]
    fn open_dir_component_no_follow_refuses_a_symlinked_component() {
        let base = test_base("symlinked-ancestor");
        std::fs::create_dir_all(&base).unwrap();
        let real_dir = base.join("real");
        std::fs::create_dir_all(&real_dir).unwrap();
        let symlink_name = "sym";
        std::os::unix::fs::symlink(&real_dir, base.join(symlink_name)).unwrap();

        let base_c = CString::new(base.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: `base_c` is a valid, NUL-terminated path to a real directory this test created.
        let base_fd = unsafe {
            libc::open(
                base_c.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        };
        assert!(
            base_fd >= 0,
            "open base dir: {}",
            io::Error::last_os_error()
        );
        let name_c = CString::new(symlink_name).unwrap();
        let result = crate::dirlock::open_dir_component_no_follow(base_fd, &name_c);
        unsafe { libc::close(base_fd) };
        assert!(
            result.is_err(),
            "a symlinked component must be refused rather than followed"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn try_new_refuses_a_pool_smaller_than_the_callers_stated_minimum() {
        let base = test_base("pool-too-small");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 2);
        write_subordinate_file(&subgid, 200_000, 2);
        let (sink, _log) = recording_sink();
        let result = UserNamespaceAllocator::try_new_for_tests(
            leases_dir, &subuid, &subgid, /* min_pool_size */ 5, sink,
        );
        assert_eq!(
            result.err().map(|e| matches!(
                e,
                UserNamespaceAllocatorError::PoolTooSmall {
                    pool_size: 2,
                    required: 5
                }
            )),
            Some(true)
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn try_new_refuses_a_subordinate_uid_range_containing_the_runners_own_euid() {
        let base = test_base("subrange-contains-runner-euid");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        let runner_uid = unsafe { libc::geteuid() };
        // A range that happens to straddle this process's own euid.
        let start = runner_uid.saturating_sub(10);
        write_subordinate_file(&subuid, start, 20);
        write_subordinate_file(&subgid, 200_000, 5);
        let (sink, _log) = recording_sink();
        let result =
            UserNamespaceAllocator::try_new_for_tests(leases_dir, &subuid, &subgid, 1, sink);
        assert!(matches!(
            result,
            Err(UserNamespaceAllocatorError::SubordinateConfig { .. })
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn try_new_refuses_a_subordinate_range_containing_uid_zero() {
        let base = test_base("subrange-contains-zero");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 0, 5);
        write_subordinate_file(&subgid, 200_000, 5);
        let (sink, _log) = recording_sink();
        let result =
            UserNamespaceAllocator::try_new_for_tests(leases_dir, &subuid, &subgid, 1, sink);
        assert!(matches!(
            result,
            Err(UserNamespaceAllocatorError::SubordinateConfig { .. })
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Sol's review: `EEXIST` during `lease()` must be treated as an ordinary collision ONLY for a
    /// slot this allocator's own bookkeeping already explains (quarantined or currently active) —
    /// an untracked marker planted directly (bypassing the allocator entirely) means the leases
    /// directory changed outside this allocator's own bookkeeping, which must poison rather than
    /// be silently skipped as if it were an ordinary collision.
    #[test]
    fn lease_poisons_on_an_untracked_marker_it_never_issued_or_quarantined() {
        let (allocator, base, _log) = new_allocator_for_test("untracked-marker-poisons", 5, 5);
        let leases_dir = base.join("leases");
        // Plant a marker for slot 0 directly — bypassing `lease()` entirely, so this allocator's
        // own `active_slots`/`quarantined_slots` bookkeeping knows nothing about it.
        let marker = LeaseMarkerV2 {
            schema_version: LEASE_MARKER_SCHEMA_V2,
            lease_nonce: LeaseNonce(1),
            runner_instance_id: RunnerInstanceId(1),
            host_uid: 100_000,
            host_gid: 200_000,
            created_at_unix_secs: 0,
            phase: LeasePhaseV2::Allocated,
        };
        std::fs::write(
            leases_dir.join(marker_file_name(0)),
            serde_json::to_string(&marker).unwrap(),
        )
        .unwrap();
        let result = allocator.lease();
        assert!(matches!(result, Err(UserNamespaceRefusal::Poisoned { .. })));
        assert!(!allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Sol's review, round 3: `lease()` must treat `active_slots` membership as authoritative
    /// BEFORE ever attempting to create a slot's marker — not only in response to `AlreadyExists`.
    /// Without that ordering, a slot whose on-disk marker was removed out from under a still-live
    /// lease (whether by external tampering, as simulated here, or by a specific interleaving with
    /// a racing `release()`) could be silently handed out a SECOND time while the original lease
    /// object still believes it holds it exclusively.
    #[test]
    fn lease_never_recreates_a_slot_whose_marker_was_externally_deleted_while_still_active() {
        let (allocator, base, _log) = new_allocator_for_test("active-marker-deleted", 1, 1);
        let leases_dir = base.join("leases");
        let lease = allocator.lease().unwrap();
        std::fs::remove_file(leases_dir.join(marker_file_name(0))).unwrap();
        let result = allocator.lease();
        assert_eq!(
            result.unwrap_err(),
            UserNamespaceRefusal::PoolExhausted { pool_size: 1 },
            "a slot this allocator still considers active must never be recreated, even if its \
             on-disk marker is gone"
        );
        // Clean up: the ORIGINAL lease's marker is gone, so re-verifying it will (correctly)
        // detect the tampering and poison — covered by a dedicated test below (this test targets
        // `lease()`'s behavior, not `release()`'s). Consume it through this expected-to-fail
        // `release_unused()` call rather than `mem::forget`, so the held lock FD is actually
        // dropped instead of leaked for the rest of the test process's lifetime.
        let _ = lease.release_unused();
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn lease_never_reissues_a_quarantined_slot_even_after_its_marker_is_externally_deleted() {
        let (allocator, base, _log) = new_allocator_for_test("quarantined-marker-deleted", 1, 1);
        let leases_dir = base.join("leases");
        let lease = allocator.lease().unwrap();
        drop(lease); // abandoned — quarantines slot 0, marker left on disk by design.
        assert!(
            allocator.is_healthy(),
            "abandonment must quarantine only the one slot, never poison the whole allocator"
        );
        std::fs::remove_file(leases_dir.join(marker_file_name(0))).unwrap();
        let result = allocator.lease();
        assert_eq!(
            result.unwrap_err(),
            UserNamespaceRefusal::PoolExhausted { pool_size: 1 },
            "a quarantined slot must never be reissued, even after its on-disk marker is gone"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Sol's review, round 3: if a lease's marker is removed by something OTHER than that lease's
    /// own `release()` while the lease is still outstanding, `release()` must detect this as
    /// tampering (via its own re-read-and-verify) and poison, rather than silently succeeding.
    #[test]
    fn release_detects_an_externally_deleted_marker_as_tampering_and_poisons() {
        let (allocator, base, _log) = new_allocator_for_test("release-detects-deletion", 1, 1);
        let leases_dir = base.join("leases");
        let lease = allocator.lease().unwrap();
        std::fs::remove_file(leases_dir.join(marker_file_name(0))).unwrap();
        let nonce = lease.nonce_for_tests();
        let result = lease.release(UserNamespaceQuiescenceProof::assert_for_tests(
            nonce,
            "test-container".to_string(),
            (0, 0),
            (0, 0),
        ));
        assert_eq!(result, Err(UserNamespaceReleaseError::MarkerMismatch));
        assert!(
            !allocator.is_healthy(),
            "an externally deleted marker must poison the whole allocator, not just this slot"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    // --- CT-007 slice 5b.1: the checkout preparation-phase lease lifecycle ---

    #[test]
    fn bind_preparation_succeeds_from_allocated_and_transitions_to_preparation_bound() {
        let (allocator, base, _log) = new_allocator_for_test("prep-bind-ok", 1, 1);
        let mut lease = allocator.lease().unwrap();
        lease
            .bind_preparation("prep-container".to_string(), (1, 1), (2, 2))
            .expect("bind_preparation must succeed for a fresh Allocated lease");
        assert!(allocator.is_healthy());
        // `bind_workload` (which requires Prepared, not PreparationBound) must refuse here — the
        // ordinary confirm_prepared step was skipped.
        let result = lease.bind_workload("workload-container".to_string(), (3, 3), (4, 4));
        assert_eq!(result, Err(UserNamespaceBindError::MarkerMismatch));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn bind_preparation_refuses_a_lease_that_is_already_preparation_bound() {
        let (allocator, base, _log) = new_allocator_for_test("prep-bind-twice", 1, 1);
        let mut lease = allocator.lease().unwrap();
        lease
            .bind_preparation("prep-1".to_string(), (1, 1), (1, 1))
            .expect("first bind_preparation must succeed");
        let result = lease.bind_preparation("prep-2".to_string(), (2, 2), (2, 2));
        assert_eq!(result, Err(UserNamespaceBindError::MarkerMismatch));
        assert!(
            !allocator.is_healthy(),
            "a second bind_preparation attempt on an already-PreparationBound marker must poison \
             the allocator"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn bind_preparation_refuses_an_oversized_container_id_without_rewriting_the_marker() {
        let (allocator, base, _log) = new_allocator_for_test("prep-bind-oversized-id", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let marker_path = base.join("leases").join(marker_file_name(0));
        let before = std::fs::read_to_string(&marker_path).unwrap();
        let oversized_id = "x".repeat(MAX_CONTAINER_ID_LEN + 1);
        let result = lease.bind_preparation(oversized_id, (1, 1), (1, 1));
        assert_eq!(result, Err(UserNamespaceBindError::InvalidContainerId));
        let after = std::fs::read_to_string(&marker_path).unwrap();
        assert_eq!(
            before, after,
            "an invalid container_id must be refused before any disk write is attempted"
        );
        assert!(
            allocator.is_healthy(),
            "an oversized container_id is a caller bug, not a global-trust failure"
        );
        lease
            .release_unused()
            .expect("the lease is still Allocated and usable after the refused bind_preparation");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Sol's review: the new bind_preparation/confirm_prepared/bind_workload methods each copy
    /// bind()/release()'s own "the durable rewrite itself had an ambiguous outcome" disposition,
    /// but nothing exercised that copied path for real. `rewrite_marker_atomically`'s `openat(...,
    /// O_EXCL)` on its `<marker>.tmp` staging file is the deterministic way in: pre-creating that
    /// exact file makes the real rewrite fail immediately with `EEXIST`, with no race required.
    #[test]
    fn bind_preparation_poisons_on_an_ambiguous_rewrite_failure() {
        let (allocator, base, _log) = new_allocator_for_test("prep-bind-rewrite-fails", 1, 1);
        let leases_dir = base.join("leases");
        let mut lease = allocator.lease().unwrap();
        std::fs::write(
            leases_dir.join(format!("{}.tmp", marker_file_name(0))),
            b"stray tmp file blocking the real rewrite's O_EXCL create",
        )
        .unwrap();
        let result = lease.bind_preparation("prep-container".to_string(), (1, 1), (2, 2));
        assert_eq!(result, Err(UserNamespaceBindError::Poisoned));
        assert!(
            !allocator.is_healthy(),
            "an ambiguous durable rewrite outcome must poison the whole allocator"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn confirm_prepared_poisons_on_an_ambiguous_rewrite_failure() {
        let (allocator, base, _log) =
            new_allocator_for_test("confirm-prepared-rewrite-fails", 1, 1);
        let leases_dir = base.join("leases");
        let mut lease = allocator.lease().unwrap();
        let nonce = lease.nonce_for_tests();
        lease
            .bind_preparation("prep-container".to_string(), (1, 1), (2, 2))
            .expect("bind_preparation must succeed");
        std::fs::write(
            leases_dir.join(format!("{}.tmp", marker_file_name(0))),
            b"stray tmp file blocking the real rewrite's O_EXCL create",
        )
        .unwrap();
        let result = lease.confirm_prepared(PreparationQuiescenceProof::assert_for_tests(
            nonce,
            "prep-container".to_string(),
            (1, 1),
            (2, 2),
        ));
        assert_eq!(result, Err(PreparationConfirmationError::Poisoned));
        assert!(
            !allocator.is_healthy(),
            "an ambiguous durable rewrite outcome must poison the whole allocator"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn bind_workload_poisons_on_an_ambiguous_rewrite_failure() {
        let (allocator, base, _log) = new_allocator_for_test("bind-workload-rewrite-fails", 1, 1);
        let leases_dir = base.join("leases");
        let mut lease = allocator.lease().unwrap();
        let nonce = lease.nonce_for_tests();
        lease
            .bind_preparation("prep-container".to_string(), (1, 1), (2, 2))
            .expect("bind_preparation must succeed");
        lease
            .confirm_prepared(PreparationQuiescenceProof::assert_for_tests(
                nonce,
                "prep-container".to_string(),
                (1, 1),
                (2, 2),
            ))
            .expect("confirm_prepared must succeed");
        std::fs::write(
            leases_dir.join(format!("{}.tmp", marker_file_name(0))),
            b"stray tmp file blocking the real rewrite's O_EXCL create",
        )
        .unwrap();
        let result = lease.bind_workload("workload-container".to_string(), (3, 3), (4, 4));
        assert_eq!(result, Err(UserNamespaceBindError::Poisoned));
        assert!(
            !allocator.is_healthy(),
            "an ambiguous durable rewrite outcome must poison the whole allocator"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Sol's review: `release_prepared`'s unlink/sync ambiguity disposition, exercised through the
    /// `release_prepared_given` seam with an injected failure rather than a real permission
    /// change — a chmod-based `EACCES` is not reliably reproducible in every environment this
    /// suite might run in (a process carrying `CAP_DAC_OVERRIDE` bypasses the DAC check entirely),
    /// so this seam proves the disposition deterministically instead.
    #[test]
    fn release_prepared_poisons_on_an_ambiguous_unlink_failure() {
        let (allocator, base, _log) = new_allocator_for_test("release-prepared-unlink-fails", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let nonce = lease.nonce_for_tests();
        lease
            .bind_preparation("prep-container".to_string(), (1, 1), (2, 2))
            .expect("bind_preparation must succeed");
        lease
            .confirm_prepared(PreparationQuiescenceProof::assert_for_tests(
                nonce,
                "prep-container".to_string(),
                (1, 1),
                (2, 2),
            ))
            .expect("confirm_prepared must succeed");
        let result = lease.release_prepared_given(|_dir_fd, _name| {
            Err(io::Error::from_raw_os_error(libc::EACCES))
        });
        assert_eq!(result, Err(UserNamespaceReleaseError::Poisoned));
        assert!(
            !allocator.is_healthy(),
            "an ambiguous unlink outcome must poison the whole allocator"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn confirm_prepared_succeeds_and_transitions_to_prepared() {
        let (allocator, base, _log) = new_allocator_for_test("confirm-prepared-ok", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let nonce = lease.nonce_for_tests();
        lease
            .bind_preparation("prep-container".to_string(), (1, 1), (2, 2))
            .expect("bind_preparation must succeed");
        lease
            .confirm_prepared(PreparationQuiescenceProof::assert_for_tests(
                nonce,
                "prep-container".to_string(),
                (1, 1),
                (2, 2),
            ))
            .expect("confirm_prepared must succeed with a matching proof");
        assert!(allocator.is_healthy());
        // Now genuinely Prepared: bind_workload must succeed, producing the ordinary Bound phase.
        lease
            .bind_workload("workload-container".to_string(), (3, 3), (4, 4))
            .expect("bind_workload must succeed once genuinely Prepared");
        lease
            .release(UserNamespaceQuiescenceProof::assert_for_tests(
                nonce,
                "workload-container".to_string(),
                (3, 3),
                (4, 4),
            ))
            .expect("ordinary release() must accept a workload identity reached via bind_workload");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn confirm_prepared_refuses_a_proof_whose_identity_disagrees_with_the_marker() {
        let (allocator, base, _log) = new_allocator_for_test("confirm-prepared-wrong-id", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let nonce = lease.nonce_for_tests();
        lease
            .bind_preparation("real-prep".to_string(), (1, 1), (2, 2))
            .expect("bind_preparation must succeed");
        let result = lease.confirm_prepared(PreparationQuiescenceProof::assert_for_tests(
            nonce,
            "different-prep".to_string(), // right nonce, WRONG preparation-bound identity
            (1, 1),
            (2, 2),
        ));
        assert_eq!(
            result,
            Err(PreparationConfirmationError::ProofDisagreesWithMarker)
        );
        assert!(
            allocator.is_healthy(),
            "a proof with the wrong preparation-bound identity is an ordinary wrong proof, not \
             corruption — it must NOT poison the whole allocator, and the marker must be left \
             untouched since a preparation runtime this proof doesn't vouch for may still be alive"
        );
        // Retry with the CORRECT proof must still succeed — proving the marker was untouched.
        lease
            .confirm_prepared(PreparationQuiescenceProof::assert_for_tests(
                nonce,
                "real-prep".to_string(),
                (1, 1),
                (2, 2),
            ))
            .expect("confirm_prepared with the correct proof must still succeed after a refusal");
        lease
            .release_prepared()
            .expect("release_prepared must succeed for a genuinely Prepared lease");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn confirm_prepared_refuses_a_proof_with_the_wrong_nonce() {
        let (allocator, base, _log) = new_allocator_for_test("confirm-prepared-wrong-nonce", 1, 1);
        let mut lease = allocator.lease().unwrap();
        lease
            .bind_preparation("prep-container".to_string(), (1, 1), (2, 2))
            .expect("bind_preparation must succeed");
        let wrong_nonce = LeaseNonce(lease.nonce_for_tests().0.wrapping_add(1));
        let result = lease.confirm_prepared(PreparationQuiescenceProof::assert_for_tests(
            wrong_nonce,
            "prep-container".to_string(),
            (1, 1),
            (2, 2),
        ));
        assert_eq!(result, Err(PreparationConfirmationError::ProofMismatch));
        assert!(
            allocator.is_healthy(),
            "a wrong-nonce proof must not poison the allocator or touch this lease's own marker"
        );
        // The marker is untouched (still PreparationBound) — nothing to release from this phase
        // except with the correct proof (covered by a dedicated test above); dropping here is an
        // ordinary abandonment, not a leak.
        drop(lease);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn release_prepared_succeeds_after_confirm_prepared_and_frees_the_slot() {
        let (allocator, base, _log) = new_allocator_for_test("release-prepared-ok", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let nonce = lease.nonce_for_tests();
        let freed_uid = lease.host_uid();
        lease
            .bind_preparation("prep-container".to_string(), (1, 1), (2, 2))
            .expect("bind_preparation must succeed");
        lease
            .confirm_prepared(PreparationQuiescenceProof::assert_for_tests(
                nonce,
                "prep-container".to_string(),
                (1, 1),
                (2, 2),
            ))
            .expect("confirm_prepared must succeed");
        lease
            .release_prepared()
            .expect("release_prepared must succeed for a genuinely Prepared lease");
        assert!(allocator.is_healthy());
        let lease_again = allocator.lease().unwrap();
        assert_eq!(
            lease_again.host_uid(),
            freed_uid,
            "release_prepared must genuinely free the slot for reuse"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn release_prepared_refuses_a_lease_still_only_preparation_bound() {
        let (allocator, base, _log) = new_allocator_for_test("release-prepared-too-early", 1, 1);
        let mut lease = allocator.lease().unwrap();
        lease
            .bind_preparation("prep-container".to_string(), (1, 1), (2, 2))
            .expect("bind_preparation must succeed");
        // confirm_prepared was never called — the marker is PreparationBound, not Prepared.
        let result = lease.release_prepared();
        assert_eq!(result, Err(UserNamespaceReleaseError::MarkerMismatch));
        assert!(
            !allocator.is_healthy(),
            "release_prepared on a marker that is only PreparationBound (a runtime may still be \
             live) must poison the whole allocator, never silently unlink"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn release_prepared_refuses_a_lease_already_bound_to_workload() {
        let (allocator, base, _log) = new_allocator_for_test("release-prepared-too-late", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let nonce = lease.nonce_for_tests();
        lease
            .bind_preparation("prep-container".to_string(), (1, 1), (2, 2))
            .expect("bind_preparation must succeed");
        lease
            .confirm_prepared(PreparationQuiescenceProof::assert_for_tests(
                nonce,
                "prep-container".to_string(),
                (1, 1),
                (2, 2),
            ))
            .expect("confirm_prepared must succeed");
        lease
            .bind_workload("workload-container".to_string(), (3, 3), (4, 4))
            .expect("bind_workload must succeed");
        let result = lease.release_prepared();
        assert_eq!(result, Err(UserNamespaceReleaseError::MarkerMismatch));
        assert!(
            !allocator.is_healthy(),
            "release_prepared on a marker already Bound to the real workload must poison the \
             allocator — use release() with a real workload quiescence proof instead"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn bind_workload_refuses_a_lease_still_only_preparation_bound() {
        let (allocator, base, _log) = new_allocator_for_test("bind-workload-too-early", 1, 1);
        let mut lease = allocator.lease().unwrap();
        lease
            .bind_preparation("prep-container".to_string(), (1, 1), (2, 2))
            .expect("bind_preparation must succeed");
        let result = lease.bind_workload("workload-container".to_string(), (3, 3), (4, 4));
        assert_eq!(result, Err(UserNamespaceBindError::MarkerMismatch));
        assert!(
            !allocator.is_healthy(),
            "bind_workload on a marker that is only PreparationBound (never durably Prepared) \
             must poison the allocator"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn bind_workload_refuses_a_lease_that_was_never_prepared() {
        let (allocator, base, _log) = new_allocator_for_test("bind-workload-never-prepared", 1, 1);
        let mut lease = allocator.lease().unwrap();
        // Still plain Allocated — never even entered the preparation phase.
        let result = lease.bind_workload("workload-container".to_string(), (1, 1), (2, 2));
        assert_eq!(result, Err(UserNamespaceBindError::MarkerMismatch));
        assert!(!allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_preparation_bound_lease_survives_reopening_and_is_quarantined() {
        let base = test_base("prep-bound-marker-survives-reboot");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 5);
        write_subordinate_file(&subgid, 200_000, 5);

        let (sink, _log) = recording_sink();
        let allocator = UserNamespaceAllocator::try_new_for_tests(
            leases_dir.clone(),
            &subuid,
            &subgid,
            1,
            sink,
        )
        .unwrap();
        let mut lease = allocator.lease().unwrap();
        let leaked_uid = lease.host_uid();
        lease
            .bind_preparation("crashed-prep-container".to_string(), (7, 8), (9, 10))
            .expect("bind_preparation must succeed for a fresh Allocated lease");
        drop(lease); // abandoned mid-preparation — its PreparationBound marker survives on disk.
        drop(allocator);

        let (sink2, log2) = recording_sink();
        let reopened =
            UserNamespaceAllocator::try_new_for_tests(leases_dir, &subuid, &subgid, 1, sink2)
                .unwrap();
        assert!(
            reopened.is_healthy(),
            "a surviving PreparationBound marker must parse successfully at boot, not be treated \
             as corrupt"
        );
        assert!(reopened.quarantined_slots().contains(&0));
        assert!(log2
            .lock()
            .unwrap()
            .iter()
            .any(|m| m.contains("PreparationBound") && m.contains("crashed-prep-container")));
        let lease2 = reopened.lease().unwrap();
        assert_ne!(
            lease2.host_uid(),
            leaked_uid,
            "the leaked slot's host_uid must never be reissued"
        );
        release_for_tests(lease2);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_prepared_lease_survives_reopening_and_is_quarantined() {
        let base = test_base("prepared-marker-survives-reboot");
        std::fs::create_dir_all(&base).unwrap();
        let leases_dir = base.join("leases");
        let subuid = base.join("subuid");
        let subgid = base.join("subgid");
        write_subordinate_file(&subuid, 100_000, 5);
        write_subordinate_file(&subgid, 200_000, 5);

        let (sink, _log) = recording_sink();
        let allocator = UserNamespaceAllocator::try_new_for_tests(
            leases_dir.clone(),
            &subuid,
            &subgid,
            1,
            sink,
        )
        .unwrap();
        let mut lease = allocator.lease().unwrap();
        let leaked_uid = lease.host_uid();
        let nonce = lease.nonce_for_tests();
        lease
            .bind_preparation("crashed-prep-container".to_string(), (7, 8), (9, 10))
            .expect("bind_preparation must succeed");
        lease
            .confirm_prepared(PreparationQuiescenceProof::assert_for_tests(
                nonce,
                "crashed-prep-container".to_string(),
                (7, 8),
                (9, 10),
            ))
            .expect("confirm_prepared must succeed");
        drop(lease); // abandoned between preparation and the real workload — Prepared survives.
        drop(allocator);

        let (sink2, log2) = recording_sink();
        let reopened =
            UserNamespaceAllocator::try_new_for_tests(leases_dir, &subuid, &subgid, 1, sink2)
                .unwrap();
        assert!(
            reopened.is_healthy(),
            "a surviving Prepared marker must parse successfully at boot, not be treated as corrupt"
        );
        assert!(reopened.quarantined_slots().contains(&0));
        assert!(log2
            .lock()
            .unwrap()
            .iter()
            .any(|m| m.contains("Prepared") && m.contains("crashed-prep-container")));
        let lease2 = reopened.lease().unwrap();
        assert_ne!(
            lease2.host_uid(),
            leaked_uid,
            "the leaked slot's host_uid must never be reissued"
        );
        release_for_tests(lease2);
        let _ = std::fs::remove_dir_all(&base);
    }

    // --- CheckoutPreparationSession: the capability wrapper enforcing correct ordering ---

    #[test]
    fn checkout_preparation_session_happy_path_produces_a_workload_bound_lease() {
        let (allocator, base, _log) = new_allocator_for_test("session-happy-path", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let nonce = lease.nonce_for_tests();
        let mut session = CheckoutPreparationSession::new();
        session
            .bind_preparation(&mut lease, "prep-container".to_string(), (1, 1), (2, 2))
            .expect("bind_preparation must succeed");
        session
            .confirm_prepared(
                &mut lease,
                PreparationQuiescenceProof::assert_for_tests(
                    nonce,
                    "prep-container".to_string(),
                    (1, 1),
                    (2, 2),
                ),
            )
            .expect("confirm_prepared must succeed");
        session
            .bind_workload(&mut lease, "workload-container".to_string(), (3, 3), (4, 4))
            .expect("bind_workload must succeed");
        // From here on the SAME ordinary release() the non-checkout path uses takes over unchanged.
        lease
            .release(UserNamespaceQuiescenceProof::assert_for_tests(
                nonce,
                "workload-container".to_string(),
                (3, 3),
                (4, 4),
            ))
            .expect(
                "ordinary release() must accept a workload-bound identity reached via a session",
            );
        assert!(allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Sol's review: a session's transition methods must validate the SUPPLIED lease against the
    /// SPECIFIC one it was bound to by `bind_preparation` — otherwise a session prepared with
    /// lease A could confirm/release/bind-workload using an entirely independent lease B.
    #[test]
    #[should_panic(expected = "confirm_prepared called with a lease different from the one")]
    fn checkout_preparation_session_confirm_prepared_refuses_a_substituted_lease() {
        let (allocator, base, _log) = new_allocator_for_test("session-cross-lease-confirm", 2, 2);
        let mut lease_a = allocator.lease().unwrap();
        let mut lease_b = allocator.lease().unwrap();
        let nonce_b = lease_b.nonce_for_tests();
        let mut session = CheckoutPreparationSession::new();
        session
            .bind_preparation(&mut lease_a, "prep-a".to_string(), (1, 1), (1, 1))
            .expect("bind_preparation on lease_a must succeed");
        // lease_b independently reaches PreparationBound too, entirely outside this session.
        lease_b
            .bind_preparation("prep-b".to_string(), (2, 2), (2, 2))
            .expect("bind_preparation on lease_b must succeed");
        // The session was bound to lease_a -- using lease_b here must panic, never silently
        // operate on the wrong lease's durable marker.
        let _ = session.confirm_prepared(
            &mut lease_b,
            PreparationQuiescenceProof::assert_for_tests(
                nonce_b,
                "prep-b".to_string(),
                (2, 2),
                (2, 2),
            ),
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    #[should_panic(expected = "bind_workload called with a lease different from the one")]
    fn checkout_preparation_session_bind_workload_refuses_a_substituted_lease() {
        let (allocator, base, _log) = new_allocator_for_test("session-cross-lease-workload", 2, 2);
        let mut lease_a = allocator.lease().unwrap();
        let mut lease_b = allocator.lease().unwrap();
        let nonce_a = lease_a.nonce_for_tests();
        let nonce_b = lease_b.nonce_for_tests();
        let mut session = CheckoutPreparationSession::new();
        session
            .bind_preparation(&mut lease_a, "prep-a".to_string(), (1, 1), (1, 1))
            .expect("bind_preparation on lease_a must succeed");
        session
            .confirm_prepared(
                &mut lease_a,
                PreparationQuiescenceProof::assert_for_tests(
                    nonce_a,
                    "prep-a".to_string(),
                    (1, 1),
                    (1, 1),
                ),
            )
            .expect("confirm_prepared on lease_a must succeed");
        // lease_b independently reaches Prepared too, entirely outside this session.
        lease_b
            .bind_preparation("prep-b".to_string(), (2, 2), (2, 2))
            .expect("bind_preparation on lease_b must succeed");
        lease_b
            .confirm_prepared(PreparationQuiescenceProof::assert_for_tests(
                nonce_b,
                "prep-b".to_string(),
                (2, 2),
                (2, 2),
            ))
            .expect("confirm_prepared on lease_b must succeed");
        // The session was bound to lease_a -- using lease_b here must panic.
        let _ = session.bind_workload(&mut lease_b, "workload".to_string(), (3, 3), (3, 3));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Sol's review: the ORIGINAL by-value `bind_workload` destroyed the only preparation
    /// capability even on a caller-fixable, retryable refusal. Proves the fixed `&mut self`
    /// signature keeps the session (and the underlying lease) usable for a corrected retry.
    #[test]
    fn checkout_preparation_session_bind_workload_survives_a_retryable_refusal() {
        let (allocator, base, _log) = new_allocator_for_test("session-workload-retry", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let nonce = lease.nonce_for_tests();
        let mut session = CheckoutPreparationSession::new();
        session
            .bind_preparation(&mut lease, "prep-container".to_string(), (1, 1), (2, 2))
            .expect("bind_preparation must succeed");
        session
            .confirm_prepared(
                &mut lease,
                PreparationQuiescenceProof::assert_for_tests(
                    nonce,
                    "prep-container".to_string(),
                    (1, 1),
                    (2, 2),
                ),
            )
            .expect("confirm_prepared must succeed");
        let oversized_id = "x".repeat(MAX_CONTAINER_ID_LEN + 1);
        let refused = session.bind_workload(&mut lease, oversized_id, (3, 3), (4, 4));
        assert_eq!(refused, Err(UserNamespaceBindError::InvalidContainerId));
        assert!(
            !session.is_unreleasable(),
            "a retryable bind_workload refusal must not abandon the session"
        );
        // Retry with a corrected id must still succeed, proving the session AND the underlying
        // lease were both left genuinely usable.
        session
            .bind_workload(&mut lease, "workload-container".to_string(), (3, 3), (4, 4))
            .expect("bind_workload must succeed on a corrected retry");
        lease
            .release(UserNamespaceQuiescenceProof::assert_for_tests(
                nonce,
                "workload-container".to_string(),
                (3, 3),
                (4, 4),
            ))
            .expect("ordinary release() must accept the workload identity reached on retry");
        assert!(allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn checkout_preparation_session_release_prepared_path() {
        let (allocator, base, _log) = new_allocator_for_test("session-release-prepared", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let nonce = lease.nonce_for_tests();
        let mut session = CheckoutPreparationSession::new();
        session
            .bind_preparation(&mut lease, "prep-container".to_string(), (1, 1), (2, 2))
            .expect("bind_preparation must succeed");
        session
            .confirm_prepared(
                &mut lease,
                PreparationQuiescenceProof::assert_for_tests(
                    nonce,
                    "prep-container".to_string(),
                    (1, 1),
                    (2, 2),
                ),
            )
            .expect("confirm_prepared must succeed");
        session
            .release_prepared(lease)
            .expect("release_prepared via the session must succeed for a genuinely Prepared lease");
        assert!(allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    #[should_panic(expected = "bind_preparation called out of order")]
    fn checkout_preparation_session_bind_preparation_twice_panics() {
        let (allocator, base, _log) = new_allocator_for_test("session-bind-prep-twice", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let mut session = CheckoutPreparationSession::new();
        session
            .bind_preparation(&mut lease, "prep-1".to_string(), (1, 1), (1, 1))
            .expect("first bind_preparation must succeed");
        let _ = session.bind_preparation(&mut lease, "prep-2".to_string(), (2, 2), (2, 2));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    #[should_panic(expected = "confirm_prepared called out of order")]
    fn checkout_preparation_session_confirm_prepared_before_bind_preparation_panics() {
        let (allocator, base, _log) = new_allocator_for_test("session-confirm-too-early", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let nonce = lease.nonce_for_tests();
        let mut session = CheckoutPreparationSession::new();
        let _ = session.confirm_prepared(
            &mut lease,
            PreparationQuiescenceProof::assert_for_tests(
                nonce,
                "prep-container".to_string(),
                (1, 1),
                (2, 2),
            ),
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    #[should_panic(expected = "bind_workload called out of order")]
    fn checkout_preparation_session_bind_workload_before_prepared_panics() {
        let (allocator, base, _log) = new_allocator_for_test("session-workload-too-early", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let mut session = CheckoutPreparationSession::new();
        let _ = session.bind_workload(&mut lease, "workload".to_string(), (1, 1), (2, 2));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn checkout_preparation_session_marks_unreleasable_on_a_poisoning_bind_preparation_failure() {
        let (allocator, base, _log) = new_allocator_for_test("session-marks-unreleasable", 1, 1);
        let mut lease = allocator.lease().unwrap();
        // Bind the underlying lease directly (bypassing the session entirely) so the marker is
        // already PreparationBound by the time a FRESH session's own bind_preparation call runs
        // against it — that call must then hit MarkerMismatch (a poisoning outcome).
        lease
            .bind_preparation("already-there".to_string(), (9, 9), (9, 9))
            .expect("planting the PreparationBound state directly must succeed");
        let mut session = CheckoutPreparationSession::new();
        let result = session.bind_preparation(&mut lease, "prep-2".to_string(), (2, 2), (2, 2));
        assert_eq!(result, Err(UserNamespaceBindError::MarkerMismatch));
        assert!(session.is_unreleasable());
        assert!(!allocator.is_healthy());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Sol's review: the session's stricter no-retry `confirm_prepared` policy needs its own
    /// direct test proving the FULL terminal disposition -- an ordinary wrong proof still leaves
    /// the ALLOCATOR globally healthy (it's not corruption), but the SESSION itself becomes
    /// permanently `Unreleasable`, a later correct proof cannot advance it (the destructuring
    /// match on the session's own state panics rather than silently re-attempting), and dropping
    /// the still-outstanding lease quarantines exactly its one slot.
    #[test]
    fn checkout_preparation_session_confirm_prepared_wrong_proof_is_a_terminal_abandonment() {
        let (allocator, base, _log) =
            new_allocator_for_test("session-confirm-wrong-proof-terminal", 1, 1);
        let mut lease = allocator.lease().unwrap();
        let nonce = lease.nonce_for_tests();
        let leaked_uid = lease.host_uid();
        let mut session = CheckoutPreparationSession::new();
        session
            .bind_preparation(&mut lease, "real-prep".to_string(), (1, 1), (2, 2))
            .expect("bind_preparation must succeed");
        let wrong_proof_result = session.confirm_prepared(
            &mut lease,
            PreparationQuiescenceProof::assert_for_tests(
                nonce,
                "different-prep".to_string(), // right nonce, WRONG preparation-bound identity
                (1, 1),
                (2, 2),
            ),
        );
        assert_eq!(
            wrong_proof_result,
            Err(PreparationConfirmationError::ProofDisagreesWithMarker)
        );
        assert!(
            allocator.is_healthy(),
            "an ordinary wrong proof at the raw lease level must not poison the allocator"
        );
        assert!(
            session.is_unreleasable(),
            "the SESSION must terminally abandon on ANY confirm_prepared failure, unlike the raw \
             lease API it wraps"
        );
        // A later CORRECT proof must not be able to advance this session -- the destructuring
        // match on session state panics because the session is Unreleasable, not PreparationBound.
        let correct_proof_attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            session.confirm_prepared(
                &mut lease,
                PreparationQuiescenceProof::assert_for_tests(
                    nonce,
                    "real-prep".to_string(),
                    (1, 1),
                    (2, 2),
                ),
            )
        }));
        assert!(
            correct_proof_attempt.is_err(),
            "a later correct proof must never be able to advance a terminally abandoned session"
        );
        drop(lease); // abandoned -- never released after the session gave up on it.
        assert!(
            allocator
                .quarantined_slots()
                .contains(&(leaked_uid - 100_000)),
            "dropping the still-outstanding lease must quarantine exactly its own slot"
        );
        assert!(
            allocator.is_healthy(),
            "abandoning one lease must not poison the whole allocator"
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
