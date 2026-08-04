//! # CT-003b (SI-017) — the out-of-band memory-cgroup enforcer for the gVisor workload
//!
//! Rootless `runsc` does NOT enforce the OCI `linux.resources.memory.limit` (it cannot create/manage
//! a host cgroup), so an untrusted gVisor job's anonymous memory would be UNBOUNDED — able to drive
//! the HOST to OOM. The fix: bound the workload with a REAL cgroup v2 the host controls. gVisor runs
//! the entire guest inside the `runsc-sandbox` (sentry) process, so placing the `runsc` child process
//! tree (frontend + the sentry/gofer it forks) into a memory cgroup bounds the workload's anonymous
//! memory at the host level. The child is placed via `pre_exec` (writing its pid to `cgroup.procs`
//! BEFORE `exec`), so every process `runsc` forks is born inside the cgroup — no escape race.
//!
//! FAIL-CLOSED: if a memory cgroup cannot be established (no cgroup v2, or the `memory`/`cpu`
//! controller is not delegated), [`MemoryCgroup::create`] returns `Err` and the gVisor `launch()`
//! REFUSES — it never runs the workload unbounded. [`MemoryCgroup::quiesce`] is the authoritative,
//! verified teardown that mints [`CgroupQuiescenceEvidence`]; `Drop` is only a best-effort backstop.
//!
//! This module owns the cgroup concern in isolation: `MemoryCgroup`, its verified-quiescence evidence
//! type, the quiescence error taxonomy, and the pure `cgroup.events`/identity helpers they build on.

use std::io;
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use super::unique_suffix;

/// A child cgroup v2 (sibling of this process's own delegated cgroup) that HARD-BOUNDS the gVisor
/// `runsc` process tree's memory at `mem_bytes` (+ no swap escape hatch). Best-effort cleaned up on
/// drop (`cgroup.kill` every member, then `rmdir`) as a BACKSTOP only — [`Self::quiesce`] is the
/// authoritative, verified teardown path; `Drop` never mints evidence of anything.
pub struct MemoryCgroup {
    /// The created cgroup directory under `/sys/fs/cgroup`.
    dir: PathBuf,
    /// This cgroup directory's own (device, inode), captured immediately after creation — the
    /// identity [`Self::quiesce`] revalidates before trusting anything it reads from `dir`, and the
    /// identity a [`CgroupQuiescenceEvidence`] ultimately vouches for.
    identity: (u64, u64),
    /// `false` once this handle is known to no longer safely name the cgroup it was created as
    /// (set by [`Self::quiesce`] the moment it detects the path now names something else, or once
    /// removal has already succeeded) — [`Self::cleanup`] (and therefore `Drop`) checks this FIRST
    /// and refuses to mutate anything when disarmed, so a stale handle can never be tricked into
    /// killing/removing whatever now occupies its old path. Interior mutability because `cleanup`
    /// takes `&self` (called from both an owned reference and `Drop::drop`'s `&mut self`).
    active: std::cell::Cell<bool>,
}

/// Classify every memory-cgroup setup failure with the security consequence. These errors reach the
/// production runner/operator, so a host permission error must say that execution was refused rather
/// than looking like an ambiguous best-effort warning.
fn resource_cgroup_refusal(reason: impl core::fmt::Display) -> String {
    format!(
        "{reason} — refusing to run the gVisor workload without its hard resource bounds \
         (SI-017 fail-closed)"
    )
}

/// cgroup v2 CPU bandwidth period. `cpu_millis` is a thousandth of one core, so this period makes
/// the quota conversion exact above the kernel minimum: `quota = cpu_millis * 100` microseconds
/// (2000 millicpu becomes `200000 100000`, i.e. two cores of bandwidth per period). Linux rejects
/// quotas below 1000 microseconds, so 1..=9 millicpu must use that smallest expressible bandwidth.
const CPU_MAX_PERIOD_US: u64 = 100_000;
const CPU_MAX_MIN_QUOTA_US: u64 = 1_000;

fn cpu_max_value(cpu_millis: u32) -> Result<String, String> {
    if cpu_millis == 0 {
        return Err("cpu_millis is zero; cannot establish a nonzero cpu.max quota".to_string());
    }
    let quota_us = u64::from(cpu_millis)
        .checked_mul(CPU_MAX_PERIOD_US)
        .and_then(|value| value.checked_div(1000))
        .ok_or_else(|| format!("cpu.max quota overflow for cpu_millis={cpu_millis}"))?
        .max(CPU_MAX_MIN_QUOTA_US);
    Ok(format!("{quota_us} {CPU_MAX_PERIOD_US}"))
}

/// Write every hard per-job cgroup limit through an injectable writer. Keeping the writes in one
/// helper makes their shared-cgroup placement and fail-closed error propagation deterministic to
/// unit-test without touching `/sys/fs/cgroup`.
fn write_job_cgroup_limits_given<F>(
    dir: &Path,
    mem_bytes: u64,
    cpu_millis: u32,
    mut write: F,
) -> Result<(), String>
where
    F: FnMut(&Path, &[u8]) -> io::Result<()>,
{
    let memory_max = mem_bytes.to_string();
    write(dir.join("memory.max").as_path(), memory_max.as_bytes())
        .map_err(|e| format!("write memory.max={mem_bytes} to {dir:?}: {e}"))?;

    // Swap is structurally fixed at zero by ResourceLimits. Failure is fatal: ignoring it would let
    // a tenant spill beyond memory.max into shared host swap.
    write(dir.join("memory.swap.max").as_path(), b"0")
        .map_err(|e| format!("write memory.swap.max=0 to {dir:?}: {e}"))?;

    let cpu_max = cpu_max_value(cpu_millis)?;
    write(dir.join("cpu.max").as_path(), cpu_max.as_bytes())
        .map_err(|e| format!("write cpu.max={cpu_max} to {dir:?}: {e}"))?;
    Ok(())
}

/// `(device, inode)` of `dir` via a plain path-based `stat`. This is a STALE-HANDLE consistency
/// check ("does the path still name the exact cgroup this handle was created as"), not full TOCTOU
/// protection: the threat model here is bugs/races within this SAME process/euid family (this
/// crate never grants an untrusted guest any access to the host cgroup hierarchy), not an
/// adversary who could race a replacement between this `stat` and a subsequent `cgroup.kill`/
/// `cgroup.events`/`rmdir` call. If concurrent same-euid replacement ever needs defending against
/// for real, every one of those calls would need to go through an `O_DIRECTORY`-opened fd instead
/// of `dir`-relative paths — not just this initial check.
fn cgroup_identity(dir: &Path) -> io::Result<(u64, u64)> {
    let meta = std::fs::metadata(dir)?;
    Ok((meta.dev(), meta.ino()))
}

/// What [`MemoryCgroup::cleanup`] should do after re-`stat`ing its directory, given `expected` (the
/// identity it was created with).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdentityCheckOutcome {
    /// The identity still matches — safe to proceed with the kill/poll/rmdir sequence.
    Proceed,
    /// CONFIRMED absence (`NotFound`) or CONFIRMED replacement (a different identity) — never
    /// safe to touch again; the caller should disarm.
    Disarm,
    /// An unknown/transient stat failure (permission error, I/O error, etc.) — NOT a confirmed
    /// absence or replacement, so the caller must leave the handle armed for a later retry.
    LeaveArmed,
}

/// Pure decision extracted from [`MemoryCgroup::cleanup`] purely so this three-way distinction has
/// a seam for direct, deterministic test coverage (fabricated `io::Error`s) — `cleanup` itself only
/// ever calls this against a REAL `cgroup_identity(&self.dir)` result.
fn classify_identity_check(
    expected: (u64, u64),
    result: &io::Result<(u64, u64)>,
) -> IdentityCheckOutcome {
    match result {
        Ok(current) if *current == expected => IdentityCheckOutcome::Proceed,
        Ok(_) => IdentityCheckOutcome::Disarm,
        Err(e) if e.kind() == io::ErrorKind::NotFound => IdentityCheckOutcome::Disarm,
        Err(_) => IdentityCheckOutcome::LeaveArmed,
    }
}

/// Non-forgeable (outside tests) proof that a specific memory cgroup — identified by the exact
/// (device, inode) [`MemoryCgroup::identity`] captured at creation — was independently verified to
/// hold zero live processes and was then removed/retired, meaning it cannot be populated again. The
/// ONLY way to construct one in production is a successful [`MemoryCgroup::quiesce`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CgroupQuiescenceEvidence {
    cgroup_identity: (u64, u64),
}

impl CgroupQuiescenceEvidence {
    pub fn cgroup_identity(&self) -> (u64, u64) {
        self.cgroup_identity
    }

    // CT-007 slice 5b.3-6e.1: also available to the `test-support` runsc-driver seam so a
    // hardware-independent cycle can fabricate finalization evidence without a real teardown.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn assert_for_tests(cgroup_identity: (u64, u64)) -> Self {
        CgroupQuiescenceEvidence { cgroup_identity }
    }
}

/// Why [`MemoryCgroup::quiesce`] failed to produce verified quiescence evidence. Every variant means
/// NO evidence was minted — the cgroup's `Drop` (best-effort backstop only) runs as `self` goes out
/// of scope at the call site, which may itself attempt to kill/poll/remove the SAME cgroup again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CgroupQuiescenceError {
    /// Could not `stat` the cgroup directory at all to revalidate its identity.
    IdentityUnreadable(String),
    /// The path this `MemoryCgroup` names no longer resolves to the SAME (device, inode) it was
    /// created with — something else replaced/recreated a cgroup at this path out from under us.
    IdentityChanged {
        expected: (u64, u64),
        actual: (u64, u64),
    },
    /// Writing `cgroup.kill` failed — the containment sweep this method is meant to guarantee before
    /// trusting `populated` never actually ran.
    Kill(String),
    /// `cgroup.events` could not be read during polling.
    EventsUnreadable(String),
    /// `cgroup.events` was read but its content did not contain exactly one valid `populated 0|1`
    /// line.
    EventsMalformed(String),
    /// The polling deadline elapsed with `populated` still `1` — real evidence a process may still
    /// be alive in this cgroup.
    StillPopulated { waited: Duration },
    /// `populated` was observed `0`, but `rmdir` itself failed — the cgroup could still be
    /// re-populated (e.g. it still has a child cgroup), so quiescence is NOT stable and no evidence
    /// is minted.
    Remove(String),
}

impl std::fmt::Display for CgroupQuiescenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CgroupQuiescenceError::IdentityUnreadable(e) => {
                write!(
                    f,
                    "failed to stat the memory cgroup to revalidate its identity: {e}"
                )
            }
            CgroupQuiescenceError::IdentityChanged { expected, actual } => write!(
                f,
                "the memory cgroup's path no longer names the cgroup it was created as (expected \
                 identity {expected:?}, found {actual:?})"
            ),
            CgroupQuiescenceError::Kill(e) => {
                write!(f, "failed to write cgroup.kill: {e}")
            }
            CgroupQuiescenceError::EventsUnreadable(e) => {
                write!(f, "failed to read cgroup.events: {e}")
            }
            CgroupQuiescenceError::EventsMalformed(e) => {
                write!(f, "cgroup.events content was malformed: {e}")
            }
            CgroupQuiescenceError::StillPopulated { waited } => write!(
                f,
                "cgroup.events still reported populated=1 after waiting {waited:?}"
            ),
            CgroupQuiescenceError::Remove(e) => {
                write!(
                    f,
                    "cgroup.events reported populated=0, but rmdir failed: {e}"
                )
            }
        }
    }
}

impl std::error::Error for CgroupQuiescenceError {}

/// Parse cgroup v2's `cgroup.events` content, returning the `populated` field. Tolerant of any key
/// order and additional/unrecognized lines (the kernel's own format is a set of `key value` lines,
/// not a fixed schema) — but requires EXACTLY one valid `populated 0` or `populated 1` line;
/// anything else (missing, duplicated, or a value other than `0`/`1`) is malformed.
fn parse_cgroup_events_populated(content: &str) -> Result<bool, String> {
    let mut found = None;
    for line in content.lines() {
        let mut parts = line.split_whitespace();
        let Some(key) = parts.next() else { continue };
        if key != "populated" {
            continue;
        }
        let Some(value) = parts.next() else {
            return Err(format!("`populated` line has no value: {line:?}"));
        };
        if parts.next().is_some() {
            return Err(format!("`populated` line has extra fields: {line:?}"));
        }
        let parsed = match value {
            "0" => false,
            "1" => true,
            other => return Err(format!("`populated` value is not 0/1: {other:?}")),
        };
        if found.replace(parsed).is_some() {
            return Err("`populated` key appears more than once".to_string());
        }
    }
    found.ok_or_else(|| "no `populated` key found".to_string())
}

/// Poll `events_path` (a `cgroup.events` file) until `populated` reads `0`, or `deadline` elapses.
/// Polls immediately on entry — a zero `deadline` still performs exactly one observation — then
/// sleeps only up to whatever budget remains between attempts, so the loop never overshoots
/// `deadline` by more than one short sleep interval.
fn wait_for_unpopulated(
    events_path: &Path,
    deadline: Duration,
) -> Result<(), CgroupQuiescenceError> {
    let start = Instant::now();
    loop {
        let content = std::fs::read_to_string(events_path)
            .map_err(|e| CgroupQuiescenceError::EventsUnreadable(e.to_string()))?;
        let populated = parse_cgroup_events_populated(&content)
            .map_err(CgroupQuiescenceError::EventsMalformed)?;
        if !populated {
            return Ok(());
        }
        let elapsed = start.elapsed();
        if elapsed >= deadline {
            return Err(CgroupQuiescenceError::StillPopulated { waited: elapsed });
        }
        std::thread::sleep((deadline - elapsed).min(Duration::from_millis(20)));
    }
}

impl MemoryCgroup {
    /// Establish a job cgroup capped at `mem_bytes`, `cpu_millis`, and zero swap. FAIL-CLOSED:
    /// returns `Err` when cgroup v2 is absent, either required controller is not delegated, or any
    /// limit write fails. The sandbox cgroup is created as a SIBLING of this process's own cgroup;
    /// this respects cgroup v2's no-internal-process rule without relocating the supervisor.
    pub fn create(mem_bytes: u64, cpu_millis: u32) -> Result<MemoryCgroup, String> {
        const ROOT: &str = "/sys/fs/cgroup";
        // cgroup v2 unified hierarchy ⇒ /proc/self/cgroup has exactly one `0::<path>` line.
        let content = std::fs::read_to_string("/proc/self/cgroup")
            .map_err(|e| resource_cgroup_refusal(format!("read /proc/self/cgroup: {e}")))?;
        let rel = content
            .lines()
            .find_map(|l| l.strip_prefix("0::"))
            .map(str::trim)
            .ok_or_else(|| {
                resource_cgroup_refusal(
                    "no cgroup v2 unified hierarchy (`0::` line absent); cannot establish a job cgroup",
                )
            })?;
        let our_dir = PathBuf::from(ROOT).join(rel.trim_start_matches('/'));
        // Both controllers must be delegated to our own cgroup (⇒ available to siblings).
        let controllers =
            std::fs::read_to_string(our_dir.join("cgroup.controllers")).unwrap_or_default();
        for required in ["memory", "cpu"] {
            if !controllers.split_whitespace().any(|c| c == required) {
                return Err(resource_cgroup_refusal(format!(
                    "the `{required}` cgroup controller is NOT delegated to {our_dir:?} \
                     (controllers: {controllers:?}); cannot bound gVisor resources"
                )));
            }
        }
        let parent = our_dir.parent().ok_or_else(|| {
            resource_cgroup_refusal(
                "this process's cgroup has no parent; cannot create a sibling job cgroup",
            )
        })?;
        let dir = parent.join(format!(
            "myelin-job-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let _ = std::fs::remove_dir(&dir);
        std::fs::create_dir(&dir)
            .map_err(|e| resource_cgroup_refusal(format!("create job cgroup {dir:?}: {e}")))?;
        // The sibling must actually have both controllers. If not, tear down and fail closed rather
        // than run a workload whose cgroup cannot enforce the full resource policy.
        let cg_controllers =
            std::fs::read_to_string(dir.join("cgroup.controllers")).unwrap_or_default();
        for required in ["memory", "cpu"] {
            if !cg_controllers.split_whitespace().any(|c| c == required) {
                let _ = std::fs::remove_dir(&dir);
                return Err(resource_cgroup_refusal(format!(
                    "the created cgroup {dir:?} has no `{required}` controller \
                     (parent did not delegate it)"
                )));
            }
        }
        // All hard limits are installed before this handle can reach the runsc spawn path. Any
        // failure removes the empty cgroup and refuses launch; none is best-effort.
        if let Err(e) = write_job_cgroup_limits_given(&dir, mem_bytes, cpu_millis, |path, value| {
            std::fs::write(path, value)
        }) {
            let _ = std::fs::remove_dir(&dir);
            return Err(resource_cgroup_refusal(e));
        }
        let identity = cgroup_identity(&dir).map_err(|e| {
            let _ = std::fs::remove_dir(&dir);
            resource_cgroup_refusal(format!("stat freshly-created job cgroup {dir:?}: {e}"))
        })?;
        Ok(MemoryCgroup {
            dir,
            identity,
            active: std::cell::Cell::new(true),
        })
    }

    /// This cgroup directory's own (device, inode), captured at creation — the identity a caller
    /// should durably bind a [`crate::user_namespace::UserNamespaceLease`] to (via `bind`'s
    /// `cgroup_identity` parameter) BEFORE `runsc` ever execs, so a later [`Self::quiesce`]'s
    /// evidence can be checked against the SAME value the lease was bound to.
    pub fn identity(&self) -> (u64, u64) {
        self.identity
    }

    /// The cgroup directory path. Exposed only to the sibling `finalize_runtime` teardown tests in
    /// `gvisor` (which own `MemoryCgroup`'s consumption) so they can assert the directory was
    /// removed; the field itself stays private so no stale handle can be constructed from a path.
    #[cfg(all(test, feature = "test-support"))]
    pub(super) fn dir(&self) -> &Path {
        &self.dir
    }

    /// Arrange for `cmd`'s spawned child (and every process it forks — for `runsc` that is the
    /// sentry/gofer tree) to run INSIDE this memory cgroup, by writing the child's own pid to
    /// `cgroup.procs` in a `pre_exec` hook BEFORE `exec`. The `cgroup.procs` file is opened HERE (in
    /// the parent) and the descriptor inherited across `fork`; the hook does only async-signal-safe
    /// work (a `getpid` + a `write` of a stack-formatted decimal pid — no allocation, no locks).
    pub fn place_child(&self, cmd: &mut Command) -> std::io::Result<()> {
        let procs = std::fs::OpenOptions::new()
            .write(true)
            .open(self.dir.join("cgroup.procs"))?;
        // SAFETY: the closure runs in the forked child between `fork` and `exec`; it performs only
        // async-signal-safe operations (no heap allocation, no lock acquisition): a `getpid`, a
        // manual stack-buffer decimal format, and a `write` to a pre-opened descriptor.
        unsafe {
            cmd.pre_exec(move || {
                let mut pid = std::process::id();
                // Format the decimal pid + '\n' into a stack buffer (u32 ⇒ ≤ 10 digits).
                let mut buf = [0u8; 11];
                let mut i = buf.len();
                i -= 1;
                buf[i] = b'\n';
                if pid == 0 {
                    i -= 1;
                    buf[i] = b'0';
                } else {
                    while pid > 0 {
                        i -= 1;
                        buf[i] = b'0' + (pid % 10) as u8;
                        pid /= 10;
                    }
                }
                (&procs).write_all(&buf[i..])
            });
        }
        Ok(())
    }

    /// Open the kernel whole-cgroup kill switch for inheritance by the trusted launch watchdog.
    /// The untrusted guest cannot access this host fd: it is consumed by the host-side runsc process
    /// tree, outside the OCI container fd table.
    pub(super) fn kill_file(&self) -> std::io::Result<std::fs::File> {
        std::fs::OpenOptions::new()
            .write(true)
            .open(self.dir.join("cgroup.kill"))
    }

    /// Best-effort teardown on EVERY path (success / timeout-kill / error): `cgroup.kill` every
    /// remaining member (cgroup v2), wait briefly for members to exit, then `rmdir`. Idempotent —
    /// safe to call more than once (Drop also calls it) — but every step past the identity
    /// revalidation is best-effort (`let _ = ...`): unlike [`Self::quiesce`], a failure here is
    /// silently ignored, so this method NEVER proves quiescence and must never be treated as if it
    /// had.
    ///
    /// Refuses to touch anything once [`Self::quiesce`] has disarmed this handle (`active ==
    /// false`), or if the path no longer names the EXACT cgroup this handle was created as (a
    /// fresh `stat` no longer matches `self.identity`) — otherwise a stale handle (e.g. one
    /// `quiesce()` already determined was replaced out from under it) could kill/remove whatever
    /// now occupies its old path instead.
    pub fn cleanup(&self) {
        if !self.active.get() {
            return;
        }
        match classify_identity_check(self.identity, &cgroup_identity(&self.dir)) {
            IdentityCheckOutcome::Proceed => {}
            IdentityCheckOutcome::Disarm => {
                self.active.set(false);
                return;
            }
            IdentityCheckOutcome::LeaveArmed => return,
        }
        let _ = std::fs::write(self.dir.join("cgroup.kill"), b"1");
        for _ in 0..200 {
            match std::fs::read_to_string(self.dir.join("cgroup.procs")) {
                Ok(s) if s.split_whitespace().next().is_none() => break,
                Ok(_) => std::thread::sleep(Duration::from_millis(5)),
                Err(_) => break, // already gone
            }
        }
        // Only disarm once removal is actually confirmed (or the dir is already gone) — a failed
        // `rmdir` (e.g. a lingering child cgroup) means this is STILL our cgroup, so a later
        // explicit `cleanup()` call or `Drop`'s own backstop must remain free to retry, not find
        // itself silently disarmed by this attempt's own failure.
        match std::fs::remove_dir(&self.dir) {
            Ok(()) => self.active.set(false),
            Err(e) if e.kind() == io::ErrorKind::NotFound => self.active.set(false),
            Err(_) => {}
        }
    }

    /// Consume this cgroup, independently verifying — never merely trusting whatever the caller
    /// believes it already killed/reaped — that it holds zero live processes, then removing it so
    /// it can never be populated again, and ONLY THEN minting a
    /// [`CgroupQuiescenceEvidence`] binding this exact cgroup's (device, inode) identity. This is
    /// the authoritative safety check backing
    /// [`crate::user_namespace::UserNamespaceQuiescenceProof`]'s `cgroup_identity` — a caller must
    /// never construct that proof without a genuine `Ok` from here.
    ///
    /// Sequence: revalidate this cgroup's identity hasn't changed since creation, write
    /// `cgroup.kill` (an idempotent final containment sweep — the earlier checked `runsc kill`/
    /// `delete`/direct-child-reap steps prove different runtime obligations and still matter, but
    /// this is the barrier that also catches e.g. a descendant that escaped the runtime's own
    /// process-group signal), poll `cgroup.events` for `populated=0`, then `rmdir`.
    ///
    /// `timeout` bounds the `cgroup.events` polling budget only — NOT this method's total latency,
    /// and NOT the caller's total teardown latency: on ANY early return (an identity mismatch, a
    /// `cgroup.kill` failure, unreadable/malformed `cgroup.events`, the polling deadline itself
    /// exhausted, or a failed `rmdir`), NO evidence is minted, and this value's own `Drop`
    /// (best-effort backstop only, never a source of truth) fires as `self` goes out of scope at
    /// the call site — which may itself attempt to kill/poll/remove the SAME cgroup again, adding
    /// up to its own ~1s attempt on top of whatever `timeout` this call already spent. A zero
    /// `timeout` still performs exactly one `cgroup.events` observation before deciding.
    pub fn quiesce(
        self,
        timeout: Duration,
    ) -> Result<CgroupQuiescenceEvidence, CgroupQuiescenceError> {
        let current_identity = cgroup_identity(&self.dir)
            .map_err(|e| CgroupQuiescenceError::IdentityUnreadable(e.to_string()))?;
        if current_identity != self.identity {
            // The path no longer names OUR cgroup — disarm immediately so this value's own `Drop`
            // (which fires as `self` is dropped when this function returns) never touches
            // whatever now occupies it.
            self.active.set(false);
            return Err(CgroupQuiescenceError::IdentityChanged {
                expected: self.identity,
                actual: current_identity,
            });
        }
        std::fs::write(self.dir.join("cgroup.kill"), b"1")
            .map_err(|e| CgroupQuiescenceError::Kill(e.to_string()))?;
        wait_for_unpopulated(&self.dir.join("cgroup.events"), timeout)?;
        std::fs::remove_dir(&self.dir).map_err(|e| CgroupQuiescenceError::Remove(e.to_string()))?;
        self.active.set(false); // removal already succeeded — nothing left for Drop to do.
        Ok(CgroupQuiescenceEvidence {
            cgroup_identity: self.identity,
        })
    }
}

impl Drop for MemoryCgroup {
    fn drop(&mut self) {
        self.cleanup();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // The launch-watchdog test below is `test-support`-gated; its imports must be too.
    #[cfg(feature = "test-support")]
    use std::process::Stdio;
    #[cfg(feature = "test-support")]
    use crate::launch_gate::SandboxCommand;
    #[cfg(feature = "test-support")]
    use crate::LaunchPermit;

    /// This module's own source, for the launch-ordering source pin below.
    const CGROUP_SOURCE: &str = include_str!("cgroup.rs");
    /// The parent `gvisor` module's source — the launch continuation the cgroup feeds into lives there.
    const GVISOR_SOURCE: &str = include_str!("../gvisor.rs");

    /// The body of a named item within `source`, from its signature to the next top-level `}` at
    /// column 0 — enough to scope a source pin to one function without pulling in its neighbours.
    fn source_of_in(source: &'static str, signature: &str) -> &'static str {
        let start = source
            .find(signature)
            .unwrap_or_else(|| panic!("`{signature}` exists"));
        let rest = &source[start..];
        let end = rest
            .find("\n}\n")
            .unwrap_or_else(|| panic!("`{signature}` has a top-level close"));
        &rest[..end]
    }

    #[test]
    fn job_cgroup_limits_write_cpu_from_policy_to_same_cgroup_before_exec() {
        let dir = PathBuf::from("/deterministic/job-cgroup");
        let mut writes = Vec::<(PathBuf, Vec<u8>)>::new();
        write_job_cgroup_limits_given(&dir, 64 << 20, 2000, |path, value| {
            writes.push((path.to_path_buf(), value.to_vec()));
            Ok(())
        })
        .expect("all resource controls should be installed");

        assert_eq!(
            writes,
            vec![
                (
                    dir.join("memory.max"),
                    (64u64 << 20).to_string().into_bytes()
                ),
                (dir.join("memory.swap.max"), b"0".to_vec()),
                (dir.join("cpu.max"), b"200000 100000".to_vec()),
            ],
            "2000 millicpu must become a two-core cpu.max quota in the same job cgroup"
        );

        let create = source_of_in(CGROUP_SOURCE, "pub fn create(mem_bytes: u64, cpu_millis: u32)");
        assert!(
            create.find("write_job_cgroup_limits_given").unwrap()
                < create.find("Ok(MemoryCgroup").unwrap(),
            "MemoryCgroup::create must not return a launchable handle before every limit write succeeds"
        );
        let run = source_of_in(GVISOR_SOURCE, "fn run_production_container_streaming(");
        assert!(
            run.find("MemoryCgroup::create(spec.limits.mem_bytes, spec.limits.cpu_millis)")
                .unwrap()
                < run.find("let capture = ||").unwrap(),
            "the policy-derived cgroup must be complete before the runsc capture/exec continuation exists"
        );
    }

    #[test]
    fn cpu_max_quota_floors_sub_ten_millicpu_at_kernel_minimum() {
        for cpu_millis in 1..=9 {
            assert_eq!(
                cpu_max_value(cpu_millis).expect("positive millicpu must produce cpu.max"),
                "1000 100000",
                "{cpu_millis} millicpu must use the kernel's minimum accepted quota"
            );
        }
    }

    #[test]
    fn cpu_max_write_failure_aborts_cgroup_setup_fail_closed() {
        let dir = PathBuf::from("/deterministic/job-cgroup");
        let mut attempted = Vec::<PathBuf>::new();
        let error = write_job_cgroup_limits_given(&dir, 64 << 20, 2000, |path, _| {
            attempted.push(path.to_path_buf());
            if path == dir.join("cpu.max") {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected cpu.max denial",
                ))
            } else {
                Ok(())
            }
        })
        .expect_err("a cpu.max write failure must abort cgroup setup");

        assert_eq!(attempted.last(), Some(&dir.join("cpu.max")));
        assert!(error.contains("write cpu.max=200000 100000"));
        assert!(error.contains("injected cpu.max denial"));
    }

    #[test]
    fn swap_max_write_failure_aborts_cgroup_setup_fail_closed() {
        let dir = PathBuf::from("/deterministic/job-cgroup");
        let mut attempted = Vec::<PathBuf>::new();
        let error = write_job_cgroup_limits_given(&dir, 64 << 20, 2000, |path, _| {
            attempted.push(path.to_path_buf());
            if path == dir.join("memory.swap.max") {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected swap.max denial",
                ))
            } else {
                Ok(())
            }
        })
        .expect_err("a memory.swap.max write failure must abort cgroup setup");

        assert_eq!(
            attempted,
            vec![dir.join("memory.max"), dir.join("memory.swap.max")],
            "fail immediately at swap.max; cpu.max and launch must never be reached"
        );
        assert!(error.contains("write memory.swap.max=0"));
        assert!(error.contains("injected swap.max denial"));
    }

    #[test]
    fn memory_cgroup_round_trips_or_fails_closed() {
        // On a host with cgroup v2 + a delegated `memory` controller, create() establishes a real
        // child cgroup with memory.max set, places nothing (we only assert the knobs), and cleans up
        // (no leaked cgroup dir). On a host WITHOUT it, create() MUST fail closed (Err) — never a
        // silently-unbounded cgroup. Either branch is a valid host posture; both are asserted honest.
        match MemoryCgroup::create(64 << 20, 1000) {
            Ok(cg) => {
                let dir = cg.dir.clone();
                let max = std::fs::read_to_string(dir.join("memory.max")).unwrap_or_default();
                assert_eq!(
                    max.trim(),
                    (64u64 << 20).to_string(),
                    "memory.max must be written to the cap (the REAL out-of-band bound)"
                );
                assert!(
                    std::fs::read_to_string(dir.join("cgroup.controllers"))
                        .unwrap_or_default()
                        .split_whitespace()
                        .any(|c| c == "memory"),
                    "the created cgroup must actually carry the memory controller"
                );
                cg.cleanup();
                assert!(!dir.exists(), "cleanup must rmdir the cgroup (no leaks)");
            }
            Err(e) => {
                assert!(
                    e.contains("fail-closed"),
                    "an unestablishable memory cgroup must fail CLOSED with a clear reason, got: {e}"
                );
            }
        }
    }

    // ───────────────────── CT-007 slice 3: MemoryCgroup::quiesce() ─────────────────────

    #[test]
    fn parse_cgroup_events_populated_accepts_reordered_and_extra_keys() {
        assert_eq!(
            parse_cgroup_events_populated("populated 0\nfrozen 0\n"),
            Ok(false)
        );
        assert_eq!(
            parse_cgroup_events_populated("frozen 1\npopulated 1\n"),
            Ok(true)
        );
        assert_eq!(
            parse_cgroup_events_populated("some_future_key 42\npopulated 0\nfrozen 0\n"),
            Ok(false),
            "unrecognized keys must not be treated as malformed — the kernel's format is not a \
             fixed schema"
        );
    }

    #[test]
    fn parse_cgroup_events_populated_rejects_a_missing_key() {
        assert!(parse_cgroup_events_populated("frozen 0\n").is_err());
    }

    #[test]
    fn parse_cgroup_events_populated_rejects_a_duplicate_key() {
        assert!(parse_cgroup_events_populated("populated 0\npopulated 1\n").is_err());
    }

    #[test]
    fn parse_cgroup_events_populated_rejects_a_non_bit_value() {
        assert!(parse_cgroup_events_populated("populated 2\n").is_err());
    }

    #[test]
    fn parse_cgroup_events_populated_rejects_extra_fields_on_the_line() {
        assert!(parse_cgroup_events_populated("populated 0 extra\n").is_err());
    }

    #[test]
    fn wait_for_unpopulated_times_out_on_a_permanently_populated_fixture() {
        let path = std::env::temp_dir().join(format!(
            "myelin-cgroup-events-fixture-populated-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        std::fs::write(&path, b"populated 1\nfrozen 0\n").unwrap();
        let result = wait_for_unpopulated(&path, Duration::from_millis(30));
        assert!(matches!(
            result,
            Err(CgroupQuiescenceError::StillPopulated { .. })
        ));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn wait_for_unpopulated_performs_exactly_one_observation_at_a_zero_deadline() {
        let path = std::env::temp_dir().join(format!(
            "myelin-cgroup-events-fixture-zero-deadline-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        std::fs::write(&path, b"populated 0\n").unwrap();
        assert_eq!(wait_for_unpopulated(&path, Duration::ZERO), Ok(()));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn wait_for_unpopulated_surfaces_an_unreadable_events_file() {
        let path = std::env::temp_dir().join(format!(
            "myelin-cgroup-events-fixture-missing-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let result = wait_for_unpopulated(&path, Duration::from_millis(10));
        assert!(matches!(
            result,
            Err(CgroupQuiescenceError::EventsUnreadable(_))
        ));
    }

    #[test]
    fn wait_for_unpopulated_surfaces_malformed_events_content() {
        let path = std::env::temp_dir().join(format!(
            "myelin-cgroup-events-fixture-malformed-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        std::fs::write(&path, b"not a valid cgroup.events file\n").unwrap();
        let result = wait_for_unpopulated(&path, Duration::from_millis(10));
        assert!(matches!(
            result,
            Err(CgroupQuiescenceError::EventsMalformed(_))
        ));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn cgroup_quiescence_evidence_assert_for_tests_round_trips() {
        let evidence = CgroupQuiescenceEvidence::assert_for_tests((7, 8));
        assert_eq!(evidence.cgroup_identity(), (7, 8));
    }

    #[cfg(feature = "test-support")]
    #[test]
    #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
    fn quiesce_succeeds_on_an_empty_real_cgroup_and_removes_it() {
        let cg = MemoryCgroup::create(64 << 20, 1000)
            .expect("this test-support gate requires a real delegated cgroup");
        let dir = cg.dir.clone();
        let identity = cg.identity();
        let evidence = cg
            .quiesce(Duration::from_millis(500))
            .expect("an empty cgroup with no processes ever placed must quiesce cleanly");
        assert_eq!(evidence.cgroup_identity(), identity);
        assert!(!dir.exists(), "quiesce must rmdir the cgroup on success");
    }

    #[cfg(feature = "test-support")]
    #[test]
    #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
    fn quiesce_kills_a_descendant_detached_outside_the_runtime_process_group() {
        // The whole point of `cgroup.kill` (over a mere process-group signal) is that it reaches
        // EVERY member of the cgroup subtree, regardless of session/process-group — including a
        // descendant that has `setsid`'d itself away, exactly the shape a sentry/gofer escape
        // would take. Reuses the exact detached-descendant spawning pattern the existing watchdog
        // test uses, but calls `quiesce()` directly instead of the watchdog's own cgroup.kill.
        let cg = MemoryCgroup::create(64 << 20, 1000)
            .expect("this test-support gate requires a real delegated cgroup");
        let dir = cg.dir.clone();
        let armed = std::env::temp_dir().join(format!(
            "myelin-gvisor-quiesce-armed-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let escaped = std::env::temp_dir().join(format!(
            "myelin-gvisor-quiesce-escaped-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(
                "setsid /bin/sh -c 'printf \"%s\" \"$$\" > \"$1\"; \
                 sleep 0.35; printf escaped > \"$2\"; sleep 5' \
                 myelin-detached \"$1\" \"$2\" & sleep 5",
            )
            .arg("myelin-quiesce-detached")
            .arg(&armed)
            .arg(&escaped)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        cg.place_child(&mut command)
            .expect("place the readiness process in the real gVisor cgroup");
        let mut child = command.spawn().expect("release detached-descendant probe");
        let deadline = Instant::now() + Duration::from_secs(1);
        while !armed.exists() {
            assert!(
                Instant::now() < deadline,
                "detached cgroup descendant did not publish readiness"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let evidence = cg.quiesce(Duration::from_secs(2)).expect(
            "quiesce must kill every cgroup member (including the detached descendant) \
                     and observe populated=0",
        );
        assert!(!dir.exists());
        // Give the killed detached descendant's own sleep-then-write a chance to have raced past
        // the kill, in case cgroup.kill somehow missed it — it must NOT have.
        std::thread::sleep(Duration::from_millis(450));
        assert!(
            !escaped.exists(),
            "a descendant outside the runtime process group survived quiesce()'s cgroup.kill"
        );
        let _ = evidence;
        let _ = child.wait();
        let _ = std::fs::remove_file(armed);
        let _ = std::fs::remove_file(escaped);
    }

    #[cfg(feature = "test-support")]
    #[test]
    #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
    fn quiesce_succeeds_without_the_caller_ever_reaping_a_killed_direct_child() {
        // `populated` tracks LIVENESS, not reap status — quiesce() must not depend on the caller
        // having `wait()`-ed its child first, only on `cgroup.kill` having actually killed it.
        let cg = MemoryCgroup::create(64 << 20, 1000)
            .expect("this test-support gate requires a real delegated cgroup");
        let dir = cg.dir.clone();
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg("sleep 30")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        cg.place_child(&mut command)
            .expect("place the direct child in the real gVisor cgroup");
        let mut child = command.spawn().expect("spawn the direct child");
        // Deliberately do NOT wait()/kill() the child ourselves before quiescing.
        let evidence = cg
            .quiesce(Duration::from_secs(2))
            .expect("quiesce must kill and observe quiescence without the caller reaping first");
        assert!(!dir.exists());
        let _ = evidence;
        // Reap it now, purely so the test process doesn't leave a zombie behind.
        let _ = child.wait();
    }

    #[cfg(feature = "test-support")]
    #[test]
    #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
    fn quiesce_refuses_to_mint_evidence_when_an_unexpected_child_cgroup_blocks_removal() {
        // `populated=0` can be true (this cgroup itself holds no processes) while `rmdir` still
        // refuses because a child cgroup exists underneath it — quiescence must not be treated as
        // stable (the hierarchy could still be re-populated) until removal itself succeeds.
        let cg = MemoryCgroup::create(64 << 20, 1000)
            .expect("this test-support gate requires a real delegated cgroup");
        let dir = cg.dir.clone();
        let nested = dir.join("myelin-unexpected-child");
        std::fs::create_dir(&nested).expect("create an unexpected nested cgroup");
        let result = cg.quiesce(Duration::from_millis(500));
        assert!(
            matches!(result, Err(CgroupQuiescenceError::Remove(_))),
            "expected Remove, got {result:?}"
        );
        assert!(
            dir.exists(),
            "a refused removal must not silently vanish the cgroup"
        );
        let _ = std::fs::remove_dir(&nested);
        let _ = std::fs::remove_dir(&dir);
    }

    /// Sol's round-2 review: a failed explicit `cleanup()` (e.g. blocked by a lingering child
    /// cgroup) must NOT disarm this handle — this cgroup is still genuinely ours, so `Drop`'s own
    /// backstop must remain free to retry once whatever was blocking removal is gone, rather than
    /// silently giving up because THIS attempt failed.
    #[cfg(feature = "test-support")]
    #[test]
    #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
    fn drop_retries_removal_after_an_earlier_failed_cleanup_call() {
        let cg = MemoryCgroup::create(64 << 20, 1000)
            .expect("this test-support gate requires a real delegated cgroup");
        let dir = cg.dir.clone();
        let nested = dir.join("myelin-unexpected-child");
        std::fs::create_dir(&nested).expect("create an unexpected nested cgroup");

        cg.cleanup(); // rmdir must fail (nested child still present) — must NOT disarm.
        assert!(
            dir.exists(),
            "the blocked cleanup must not have vanished the cgroup"
        );

        std::fs::remove_dir(&nested).expect("clear the way for the retry");
        drop(cg); // Drop's own backstop must still be armed and retry successfully now.
        assert!(
            !dir.exists(),
            "Drop must retry removal after an earlier failed explicit cleanup() — a disarmed \
             handle would have leaked this cgroup"
        );
    }

    // Sol's round-4 review (non-blocking suggestion): a dedicated test for the identity check's
    // `Err(_)` (unknown/transient stat failure) branch would be useful. A self-referential symlink
    // planted AT the cgroup's own path (to produce `ELOOP` without a real production seam) turns
    // out not to work here — cgroupfs refuses `symlink()` inside it (`EPERM`), since it only
    // supports directories and its own fixed control files. Rather than fight the filesystem,
    // `classify_identity_check` was extracted as its own pure function, tested directly below
    // against fabricated `io::Error`s.

    #[test]
    fn classify_identity_check_proceeds_when_identity_matches() {
        assert_eq!(
            classify_identity_check((1, 2), &Ok((1, 2))),
            IdentityCheckOutcome::Proceed
        );
    }

    #[test]
    fn classify_identity_check_disarms_on_a_confirmed_replacement() {
        assert_eq!(
            classify_identity_check((1, 2), &Ok((3, 4))),
            IdentityCheckOutcome::Disarm
        );
    }

    #[test]
    fn classify_identity_check_disarms_on_a_confirmed_absence() {
        assert_eq!(
            classify_identity_check((1, 2), &Err(io::Error::from(io::ErrorKind::NotFound))),
            IdentityCheckOutcome::Disarm
        );
    }

    #[test]
    fn classify_identity_check_leaves_armed_on_a_transient_error() {
        assert_eq!(
            classify_identity_check(
                (1, 2),
                &Err(io::Error::from(io::ErrorKind::PermissionDenied))
            ),
            IdentityCheckOutcome::LeaveArmed
        );
    }

    #[cfg(feature = "test-support")]
    #[test]
    #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
    fn quiesce_detects_an_identity_change_and_refuses_to_mint_evidence() {
        let cg = MemoryCgroup::create(64 << 20, 1000)
            .expect("this test-support gate requires a real delegated cgroup");
        let dir = cg.dir.clone();
        let original_identity = cg.identity();
        // Simulate something else having replaced the cgroup at this exact path out from under
        // us: remove it and recreate a fresh one at the SAME path (a new inode) — WITH a live,
        // controlled process placed in the replacement, so the assertions below can prove Drop's
        // backstop cleanup never touches it (Sol's round-1 finding: an unguarded Drop would
        // otherwise kill/rmdir whatever now occupies the stale handle's old path).
        std::fs::remove_dir(&dir).unwrap();
        std::fs::create_dir(&dir).unwrap();
        let mut replacement_command = Command::new("/bin/sh");
        replacement_command
            .arg("-c")
            .arg("sleep 30")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        cg.place_child(&mut replacement_command)
            .expect("place a live process in the replacement cgroup");
        let mut replacement_child = replacement_command
            .spawn()
            .expect("spawn the replacement cgroup's own controlled member");

        // `quiesce` takes `self` by value, so `cg`'s own `Drop` already ran (inside `quiesce`'s
        // stack frame, as it returns) by the time this call returns — the assertions below are
        // already checking POST-Drop state, not something that needs a separate explicit drop.
        let result = cg.quiesce(Duration::from_millis(500));
        match result {
            Err(CgroupQuiescenceError::IdentityChanged { expected, actual }) => {
                assert_eq!(expected, original_identity);
                assert_ne!(actual, original_identity);
            }
            other => panic!("expected IdentityChanged, got {other:?}"),
        }
        assert!(
            dir.exists(),
            "an identity mismatch must never let Drop remove the replacement cgroup"
        );
        assert!(
            replacement_child
                .try_wait()
                .expect("waitpid on the replacement's controlled member")
                .is_none(),
            "an identity mismatch must never let Drop kill the replacement cgroup's own process"
        );
        let _ = replacement_child.kill();
        let _ = replacement_child.wait();
        let _ = std::fs::write(dir.join("cgroup.kill"), b"1");
        for _ in 0..200 {
            match std::fs::read_to_string(dir.join("cgroup.procs")) {
                Ok(s) if s.split_whitespace().next().is_none() => break,
                Ok(_) => std::thread::sleep(Duration::from_millis(5)),
                Err(_) => break,
            }
        }
        let _ = std::fs::remove_dir(&dir);
    }

    #[cfg(feature = "test-support")]
    #[test]
    #[cfg_attr(not(feature = "privileged-host-tests"), ignore = "requires privileged host substrate (delegated cgroup v2 / btrfs / runsc+staged gvisor-assets / userns) — run on the host lane with --features privileged-host-tests")]
    fn launch_watchdog_cgroup_kills_a_descendant_outside_the_runtime_process_group() {
        use std::time::Instant;
        let cgroup = MemoryCgroup::create(64 << 20, 1000)
            .expect("the all-feature gVisor cgroup watchdog gate requires a real delegated cgroup");
        let armed = std::env::temp_dir().join(format!(
            "myelin-gvisor-cgroup-watchdog-armed-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let escaped = std::env::temp_dir().join(format!(
            "myelin-gvisor-cgroup-watchdog-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let permit = LaunchPermit::retained(|| Ok(crate::LaunchOwnership::immediate()));
        let mut command =
            SandboxCommand::new("/bin/sh", Some(permit), Some(Duration::from_secs(2))).unwrap();
        command
            .command_mut()
            .arg("-c")
            .arg(
                "setsid /bin/sh -c 'printf \"%s\" \"$$\" > \"$1\"; \
                 sleep 0.35; printf escaped > \"$2\"; sleep 5' \
                 myelin-detached \"$1\" \"$2\" & sleep 5",
            )
            .arg("myelin-cgroup-watchdog")
            .arg(&armed)
            .arg(&escaped)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        cgroup
            .place_child(command.command_mut())
            .expect("place launch guard in the real gVisor cgroup");
        command.kill_cgroup_on_liveness_loss(
            cgroup
                .kill_file()
                .expect("open the real whole-cgroup kill switch"),
        );
        let mut child = command.spawn().expect("release detached-descendant probe");
        let runtime_group = child.id() as i32;
        let deadline = Instant::now() + Duration::from_secs(1);
        while !armed.exists() {
            assert!(
                Instant::now() < deadline,
                "detached cgroup descendant did not publish readiness"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let detached_pid: i32 = std::fs::read_to_string(&armed)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // SAFETY: detached_pid came from the live test-owned setsid child.
        let detached_group = unsafe { libc::getpgid(detached_pid) };
        assert!(detached_group > 0);
        assert_ne!(
            detached_group, runtime_group,
            "the readiness process must genuinely be outside the runtime process group"
        );
        let membership = std::fs::read_to_string(format!("/proc/{detached_pid}/cgroup")).unwrap();
        let cgroup_name = cgroup.dir.file_name().unwrap().to_string_lossy();
        assert!(
            membership.contains(cgroup_name.as_ref()),
            "the detached descendant must remain in the exact gVisor memory cgroup"
        );
        child.kill_and_wait();
        std::thread::sleep(Duration::from_millis(450));
        assert!(
            !escaped.exists(),
            "a sentry/gofer-shaped descendant outside the runtime process group survived cgroup.kill"
        );
        let _ = std::fs::remove_file(armed);
        let _ = std::fs::remove_file(escaped);
    }
}
