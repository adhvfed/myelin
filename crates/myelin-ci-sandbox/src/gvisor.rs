//! # The gVisor (`runsc`) second `SandboxBackend` (CI-P2 → P-237, M2; satisfies the CI-P28 floor early)
//!
//! **Owning architecture (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md`
//! §5.1 ("gVisor is the named second backend behind the same `SandboxBackend` trait") + §5.3 (the
//! backend-independent hardening profile applied identically) + `sketches/01-isolation-model.md`
//! (Candidate A — gVisor / the `runsc` OCI runtime). **Contract:** `contract-index.md` row 8.4.
//!
//! ## Reconcile: the CI-P28 "gVisor second backend" — built early (P-237), PROMOTED at CI-P28 (P-423)
//! The original plan deferred gVisor to **CI-P28** (density/latency-economics-triggered). The
//! CI-P2 handoff INVERTED that: the host has `runsc` installed, so the backend SHAPE shipped early
//! (P-237) as the **named-second backend behind the SAME trait**. **CI-P28 (P-423) PROMOTES it**:
//! the escape drill now RE-RUNS the full adversarial corpus inside a real `runsc` (gVisor) sandbox
//! via a hardened OCI bundle (see [`build_gvisor_corpus_script`] / [`gvisor_drill_config_json`] +
//! `tests/escape_drill_gvisor_test.rs`), emitting a dated green attestation with gVisor EXERCISED —
//! the contract-8.4 permanent gate re-greened on the second backend.
//!
//! DEVIATION (documented, EI-01 §1): the FORMAL density/latency-economics trigger (measured at
//! CI-P30 / P-490) is downstream of P-423 and has NOT fired. The promotion is justified instead by
//! the binding DEV-REAL policy — this host has KVM + gVisor, so the sandbox-escape gate is a REAL
//! drill on both backends, not a floor. Proving the gate on the second backend NOW (rather than
//! waiting for the economics trigger) is strictly safer: it is a real green attestation, never a
//! weakened threshold.
//!
//! This is reconciliation, not a fork: the SAME [`SandboxBackend`](crate::SandboxBackend) trait, the
//! SAME mandatory [`HardeningProfile`](crate::hardening::HardeningProfile), the SAME four-guarantee
//! [`RunnerHooks`](crate::RunnerHooks) order, and the SAME host-side parser + attestation format.
//! gVisor uses the OCI/`runsc` path; Firecracker uses the microVM path. The drill governs which is
//! the production default (microVM, §5.1).
//!
//! ## `no-host-exec` (contract 1.6 / X-6 / AG-2)
//! Like the Firecracker backend, the REAL `runsc`-spawn site IS the sandbox seam's enforcement
//! mechanism (it *creates* the userspace-kernel boundary), not a bypass of it — a NAMED, LOUD
//! exclusion of this one file (registered in `lint-gate` + `tests/workspace_clean.rs`).

use crate::hardening::HardeningProfile;
use crate::redaction::RedactionPlan;
use crate::{
    drain_capped, EgressPolicy, HookError, IdemToken, ImageRef, JobKind, JobSpec, MeterTarget,
    ResourceLimits, ResourceUsage, RunTokenRef, RunnerHooks, SandboxBackend, SandboxHandle,
    SandboxLaunch, SandboxResult, TrustTier, WorkspaceSpec, SANDBOX_CAPTURE_BOUND,
};
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Env var naming the `runsc` binary; defaults to `runsc` on `PATH`.
pub const ENV_RUNSC_BIN: &str = "MYELIN_RUNSC_BIN";

fn runsc_bin() -> String {
    std::env::var(ENV_RUNSC_BIN).unwrap_or_else(|_| "runsc".to_string())
}

/// The unprivileged uid/gid the untrusted `spec.command` runs as INSIDE the gVisor sandbox. Untrusted
/// code must NEVER be uid 0 even within the userspace-kernel boundary (defense in depth + hygiene);
/// 65534 = nobody/nogroup (numeric ⇒ no `/etc/passwd` lookup). Unlike Firecracker, gVisor's exit
/// capture needs no forge defense — `runsc run` returns the container process's REAL exit code
/// directly to THIS host process (there is no shared serial console to spoof) — but the workload is
/// still dropped to this non-root uid/gid in the OCI config so it never runs as root in the sandbox.
const UNTRUSTED_UID: u32 = 65534;
const UNTRUSTED_GID: u32 = 65534;
static NEVER_CANCELLED: AtomicBool = AtomicBool::new(false);

/// The OCI runtime config (`config.json`) the gVisor `runsc` path consumes, built from a [`JobSpec`]
/// and the mandatory [`HardeningProfile`]. Every hardening field maps to a real OCI posture: the
/// root is `readonly: true`, all capabilities are dropped, `no_new_privileges: true`, a seccomp
/// profile is attached, the network namespace carries no interface when egress is default-deny, and
/// the untrusted process runs as a NON-ROOT uid/gid ([`UNTRUSTED_UID`]/[`UNTRUSTED_GID`]). This is a
/// RUNNABLE OCI config (`process.cwd` + `process.env` are set) — `runsc run --bundle` executes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OciConfig {
    args: Vec<String>,
    root_readonly: bool,
    drop_all_caps: bool,
    no_new_privileges: bool,
    seccomp: bool,
    has_network: bool,
    pids_max: u32,
    /// The memory ceiling (bytes) — emitted as `linux.resources.memory.limit`. IMPORTANT (CT-003b /
    /// SI-017): `runsc --rootless` does NOT enforce this OCI field (rootless runsc cannot manage a
    /// host cgroup), so this value is ADVISORY here (it would be honored by a non-rootless `runsc`).
    /// The REAL host-RAM bound for the gVisor workload is the OUT-OF-BAND [`MemoryCgroup`] the
    /// production run path places the `runsc` process tree into — that is what OOM-kills a memory hog
    /// within the limit and keeps it from consuming host RAM beyond `mem_bytes`.
    mem_bytes: u64,
    /// The scratch-disk quota (bytes) — the size of the bounded writable `/tmp` tmpfs (CT-003a). gVisor
    /// would otherwise auto-mount an UNBOUNDED host-RAM-backed tmpfs at `/tmp`; sizing it caps a disk
    /// fill at ENOSPC (the SI-017 host-DoS escape D2 surfaced through the production `launch()`).
    disk_bytes: u64,
    /// CT-006a (the git wire): EXTRA bind mounts injected into the bundle (host→guest, with a
    /// `readonly` flag). EMPTY for every CI/agent job (so the prod-exec posture is byte-unchanged);
    /// the git-wire launch path populates it with the **read-only bare-repo mount** at
    /// [`WIRE_REPO_MOUNT`] and an optional **writable quarantine mount** at [`WIRE_QUARANTINE_MOUNT`].
    /// Rendered into the SAME OCI `mounts` array the `/tmp` tmpfs uses (reusing the proven machinery).
    extra_mounts: Vec<WireMount>,
    /// CT-006a: EXTRA `process.env` entries (`"KEY=VALUE"`) appended after the base `PATH`. EMPTY for
    /// every CI/agent job; the git-wire path sets `GIT_PROTOCOL=version=2` / `GIT_EXEC_PATH` so the
    /// sandboxed canonical `git` speaks protocol-v2 and finds its `git-core` helpers.
    extra_env: Vec<String>,
    /// CT-006a: an ABSOLUTE OCI `root.path` override. `None` ⇒ `"rootfs"` (relative to the bundle — the
    /// prod-exec path symlinks `rootfs` into the bundle, byte-unchanged). The git-wire path sets the
    /// absolute staged-rootfs path here so the bundle needs NO `rootfs` symlink — a symlinked root.path
    /// COMBINED with a host bind mount makes the rootless `runsc` gofer fail to bring up the sandbox
    /// ("cannot read client sync file"), whereas an absolute root.path + a bind mount works.
    root_path: Option<PathBuf>,
}

/// CT-006a (the git wire): a single host→guest bind mount injected into the hardened OCI bundle, with
/// an explicit `readonly` flag. The **bare repo** is mounted `readonly: true` (a serve can NEVER
/// mutate it — `upload-pack` is read-only by construction; the RO is enforced by runsc, not advisory);
/// the **quarantine** (push object intake) is mounted writable so the sandboxed `receive-pack` writes
/// objects THERE, never into the real repo (the in-process ref-CAS policy inspects the quarantine on
/// the host AFTER the run — CT-006b). The host source is ALWAYS a resolver-validated path (see
/// [`resolve_bare_repo_path`]); a raw attacker-influenced path NEVER reaches a mount.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireMount {
    /// The host path to bind into the guest (a validated bare-repo / quarantine dir — never raw).
    pub host_source: PathBuf,
    /// The fixed guest mount point (e.g. `/repo`, `/quarantine`).
    pub guest_dest: String,
    /// `true` ⇒ the bind is `ro` (runsc-enforced read-only); `false` ⇒ writable (`rw`).
    pub readonly: bool,
}

impl OciConfig {
    /// Build the OCI config from a job + its derived hardening profile (the same profile the
    /// Firecracker backend enforces — backend-independent).
    pub fn from_spec(spec: &JobSpec, profile: &HardeningProfile) -> OciConfig {
        OciConfig {
            args: spec.command.clone(),
            root_readonly: profile.read_only_root,
            drop_all_caps: profile.drop_all_caps,
            no_new_privileges: profile.no_new_privileges,
            seccomp: profile.seccomp,
            has_network: profile.network_device,
            pids_max: profile.pids_max,
            mem_bytes: spec.limits.mem_bytes,
            // The hardening profile's scratch-disk quota (= `spec.limits.disk_bytes`).
            disk_bytes: profile.scratch_quota_bytes,
            // No extra bind mounts / env by default — a CI/agent job's posture is byte-unchanged.
            extra_mounts: Vec::new(),
            extra_env: Vec::new(),
            root_path: None,
        }
    }

    /// CT-006a: override the OCI `root.path` with an ABSOLUTE staged-rootfs path (so the bundle needs no
    /// `rootfs` symlink — required when a host bind mount is present). Consuming builder.
    pub fn with_root_path(mut self, path: PathBuf) -> OciConfig {
        self.root_path = Some(path);
        self
    }

    /// CT-006a: attach the git-wire bind mounts (RO repo + optional writable quarantine) to this
    /// config — they render into the SAME OCI `mounts` array the `/tmp` tmpfs uses. Consuming builder.
    pub fn with_extra_mounts(mut self, mounts: Vec<WireMount>) -> OciConfig {
        self.extra_mounts = mounts;
        self
    }

    /// CT-006a: append extra `process.env` entries (`"KEY=VALUE"`) after the base `PATH`. Consuming
    /// builder; used to set `GIT_PROTOCOL=version=2` / `GIT_EXEC_PATH` for the sandboxed `git`.
    pub fn with_extra_env(mut self, env: Vec<String>) -> OciConfig {
        self.extra_env = env;
        self
    }

    /// Serialize to a minimal OCI `config.json` (`runsc run --bundle <dir>` consumes it). The
    /// posture flags reflect the real enforced state, so a test over this JSON asserts the posture.
    pub fn to_json(&self) -> String {
        let args = self
            .args
            .iter()
            .map(|a| format!("{a:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        let net_ns = if self.has_network {
            "{ \"type\": \"network\" }"
        } else {
            // No network namespace interface — egress closed at the namespace level.
            "{ \"type\": \"network\", \"path\": \"\" }"
        };
        // `process.env`: the base PATH first, then any extra entries (e.g. GIT_PROTOCOL) — JSON-quoted.
        let mut envs = vec![format!(
            "{:?}",
            "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
        )];
        for e in &self.extra_env {
            envs.push(format!("{e:?}"));
        }
        let env_json = envs.join(", ");
        // `mounts`: the size-bounded writable `/tmp` tmpfs first (byte-unchanged), then any extra bind
        // mounts (CT-006a: the RO repo + optional writable quarantine). Source/dest are JSON-escaped via
        // `{:?}` so a path can carry no JSON-injection. A RO bind carries `ro`; a writable one `rw`.
        let mut mounts = vec![format!(
            "{{ \"destination\": \"/tmp\", \"type\": \"tmpfs\", \"source\": \"tmpfs\", \
             \"options\": [\"nosuid\", \"nodev\", \"mode=1777\", \"size={}\"] }}",
            self.disk_bytes
        )];
        for m in &self.extra_mounts {
            let src = m.host_source.to_string_lossy();
            let mode = if m.readonly { "ro" } else { "rw" };
            mounts.push(format!(
                "{{ \"destination\": {dest:?}, \"type\": \"bind\", \"source\": {src:?}, \
                 \"options\": [\"bind\", \"{mode}\", \"nosuid\", \"nodev\"] }}",
                dest = m.guest_dest,
            ));
        }
        let mounts_json = mounts.join(", ");
        // `root.path`: the absolute staged rootfs for the git wire, else the bundle-relative `rootfs`.
        let root_path = self
            .root_path
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "rootfs".to_string());
        format!(
            "{{\n  \"ociVersion\": \"1.0.0\",\n  \"process\": {{\n    \
             \"user\": {{ \"uid\": {uid}, \"gid\": {gid} }},\n    \
             \"args\": [{args}],\n    \"cwd\": \"/\",\n    \
             \"env\": [{env_json}],\n    \
             \"noNewPrivileges\": {nnp},\n    \
             \"capabilities\": {{ \"bounding\": [], \"effective\": [], \"permitted\": [] }}\n  }},\n  \
             \"root\": {{ \"path\": {root_path:?}, \"readonly\": {ro} }},\n  \
             \"mounts\": [ {mounts_json} ],\n  \
             \"linux\": {{\n    \"resources\": {{ \"memory\": {{ \"limit\": {mem} }}, \
             \"pids\": {{ \"limit\": {pids} }} }},\n    \
             \"seccomp\": {{ \"defaultAction\": \"SCMP_ACT_ERRNO\" }},\n    \
             \"namespaces\": [ {net_ns} ]\n  }}\n}}",
            uid = UNTRUSTED_UID,
            gid = UNTRUSTED_GID,
            args = args,
            env_json = env_json,
            nnp = self.no_new_privileges,
            ro = self.root_readonly,
            mounts_json = mounts_json,
            mem = self.mem_bytes,
            pids = self.pids_max,
            net_ns = net_ns,
        )
    }

    /// True iff the OCI root is read-only.
    pub fn root_readonly(&self) -> bool {
        self.root_readonly
    }
    /// True iff a network interface is present (false == egress closed at the namespace level).
    pub fn has_network(&self) -> bool {
        self.has_network
    }
}

/// A gVisor backend error.
#[derive(Debug)]
pub enum GvisorError {
    /// A four-guarantee hook failed.
    Hook(crate::HookError),
    /// The mandatory hardening profile could not be asserted in force (fail-closed).
    Hardening(String),
    /// The `runsc` runtime errored.
    Runtime(String),
}

impl std::fmt::Display for GvisorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GvisorError::Hook(e) => write!(f, "gvisor backend: guarantee hook failed: {e}"),
            GvisorError::Hardening(s) => write!(f, "gvisor backend: hardening not enforced: {s}"),
            GvisorError::Runtime(s) => write!(f, "gvisor backend: runsc error: {s}"),
        }
    }
}

impl std::error::Error for GvisorError {}

impl From<crate::HookError> for GvisorError {
    fn from(e: crate::HookError) -> Self {
        GvisorError::Hook(e)
    }
}

// ---------------------------------------------------------------------------------------------
// CT-003b (SI-017) — the OUT-OF-BAND memory enforcer for the gVisor workload.
//
// THE DEFECT: `runsc --rootless` does NOT enforce the OCI `linux.resources.memory.limit` (rootless
// runsc cannot create/manage a host cgroup), so an untrusted gVisor job's ANONYMOUS memory was
// UNBOUNDED — it could drive the HOST to OOM (a supply-chain availability compromise). The `/tmp`
// disk quota and Firecracker's hard guest-RAM cap were fine; this was gVisor-anonymous-memory-only.
//
// THE FIX: bound the workload's memory with a REAL cgroup v2 the host (not rootless runsc) controls.
// gVisor runs the ENTIRE guest inside the `runsc-sandbox` (sentry) process, and the guest's anonymous
// memory is backed by the sentry's host RSS — so placing the `runsc` child PROCESS TREE (frontend +
// the sentry/gofer it forks) into a memory cgroup BOUNDS the workload's anonymous memory at the host
// level. A `memory.max` breach OOM-kills the sentry within the limit; host RAM is never consumed
// beyond `mem_bytes` (proven: a 1 GiB anon hog under a 256 MiB cap is OOM-killed, host MemAvailable
// unmoved). We place the child via `pre_exec` (writing its pid to `cgroup.procs` BEFORE `exec`), so
// every process `runsc` forks is born inside the cgroup — no race where the sentry escapes it.
//
// FAIL-CLOSED: if a memory cgroup CANNOT be established (no cgroup v2, or the `memory` controller is
// not delegated to us), [`MemoryCgroup::create`] returns `Err` and the gVisor `launch()` REFUSES —
// it NEVER runs the workload unbounded. (Firecracker is unaffected: its microVM hard-caps guest RAM.)
// ---------------------------------------------------------------------------------------------

/// A child cgroup v2 (sibling of this process's own delegated cgroup) that HARD-BOUNDS the gVisor
/// `runsc` process tree's memory at `mem_bytes` (+ no swap escape hatch). Cleaned up on drop
/// (`cgroup.kill` every member, then `rmdir`) so no cgroup leaks on any teardown path.
pub struct MemoryCgroup {
    /// The created cgroup directory under `/sys/fs/cgroup`.
    dir: PathBuf,
}

/// Classify every memory-cgroup setup failure with the security consequence. These errors reach the
/// production runner/operator, so a host permission error must say that execution was refused rather
/// than looking like an ambiguous best-effort warning.
fn memory_cgroup_refusal(reason: impl core::fmt::Display) -> String {
    format!(
        "{reason} — refusing to run the gVisor workload unbounded (SI-017 fail-closed)"
    )
}

impl MemoryCgroup {
    /// Establish a memory cgroup capped at `mem_bytes` (swap disabled). FAIL-CLOSED: returns `Err`
    /// when cgroup v2 is absent or the `memory` controller is not delegated to this process — the
    /// caller MUST then refuse to run the workload (never unbounded). The sandbox cgroup is created
    /// as a SIBLING of this process's own cgroup (whose parent already delegates the `memory`
    /// controller into its `subtree_control`, since our own cgroup carries it as a controller); this
    /// respects cgroup v2's no-internal-process rule without relocating this supervisor process.
    pub fn create(mem_bytes: u64) -> Result<MemoryCgroup, String> {
        const ROOT: &str = "/sys/fs/cgroup";
        // cgroup v2 unified hierarchy ⇒ /proc/self/cgroup has exactly one `0::<path>` line.
        let content = std::fs::read_to_string("/proc/self/cgroup")
            .map_err(|e| memory_cgroup_refusal(format!("read /proc/self/cgroup: {e}")))?;
        let rel = content
            .lines()
            .find_map(|l| l.strip_prefix("0::"))
            .map(str::trim)
            .ok_or_else(|| {
                memory_cgroup_refusal(
                    "no cgroup v2 unified hierarchy (`0::` line absent); cannot establish a memory cgroup",
                )
            })?;
        let our_dir = PathBuf::from(ROOT).join(rel.trim_start_matches('/'));
        // The `memory` controller must be delegated to our own cgroup (⇒ available to siblings).
        let controllers =
            std::fs::read_to_string(our_dir.join("cgroup.controllers")).unwrap_or_default();
        if !controllers.split_whitespace().any(|c| c == "memory") {
            return Err(memory_cgroup_refusal(format!(
                "the `memory` cgroup controller is NOT delegated to {our_dir:?} \
                 (controllers: {controllers:?}); cannot bound gVisor memory"
            )));
        }
        let parent = our_dir.parent().ok_or_else(|| {
            memory_cgroup_refusal(
                "this process's cgroup has no parent; cannot create a sibling memory cgroup",
            )
        })?;
        let dir = parent.join(format!(
            "myelin-mem-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let _ = std::fs::remove_dir(&dir);
        std::fs::create_dir(&dir).map_err(|e| {
            memory_cgroup_refusal(format!("create memory cgroup {dir:?}: {e}"))
        })?;
        // The sibling must actually have the `memory` controller (the parent delegated it). If not,
        // tear down and fail closed rather than run a workload an empty cgroup would not bound.
        let cg_controllers =
            std::fs::read_to_string(dir.join("cgroup.controllers")).unwrap_or_default();
        if !cg_controllers.split_whitespace().any(|c| c == "memory") {
            let _ = std::fs::remove_dir(&dir);
            return Err(memory_cgroup_refusal(format!(
                "the created cgroup {dir:?} has no `memory` controller (parent did not delegate it)"
            )));
        }
        // The HARD host-RAM bound + close the swap escape hatch (so a hog OOMs rather than swaps).
        if let Err(e) = std::fs::write(dir.join("memory.max"), mem_bytes.to_string()) {
            let _ = std::fs::remove_dir(&dir);
            return Err(memory_cgroup_refusal(format!(
                "write memory.max={mem_bytes} to {dir:?}: {e}"
            )));
        }
        // Best-effort: a host without a swap controller has nothing to cap (swap.max absent ⇒ no
        // swap to escape into). Where present, 0 forces an OOM-kill instead of swapping the hog out.
        let _ = std::fs::write(dir.join("memory.swap.max"), b"0");
        Ok(MemoryCgroup { dir })
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

    /// Tear the cgroup down on EVERY path (success / timeout-kill / error): `cgroup.kill` every
    /// remaining member (cgroup v2), wait briefly for the kernel to reap them, then `rmdir`. No
    /// leaked cgroups. Idempotent — safe to call more than once (Drop also calls it).
    pub fn cleanup(&self) {
        let _ = std::fs::write(self.dir.join("cgroup.kill"), b"1");
        for _ in 0..200 {
            match std::fs::read_to_string(self.dir.join("cgroup.procs")) {
                Ok(s) if s.split_whitespace().next().is_none() => break,
                Ok(_) => std::thread::sleep(Duration::from_millis(5)),
                Err(_) => break, // already gone
            }
        }
        let _ = std::fs::remove_dir(&self.dir);
    }
}

impl Drop for MemoryCgroup {
    fn drop(&mut self) {
        self.cleanup();
    }
}

/// A live gVisor container handle (the OCI/`runsc` container id, killable on teardown). Its
/// lifecycle is RECONCILED with the Firecracker [`VmmChild`](crate::firecracker::VmmChild): both
/// expose `kill` (whole-guest teardown) AND `wait` (block until the command exits). For gVisor the
/// REAL exit is captured directly from the `runsc` child's exit status during the run (no separate
/// wait), so [`wait`](RunscChild::wait) is retained only for lifecycle-shape parity with `VmmChild`.
pub trait RunscChild {
    fn kill(&mut self) -> Result<(), String>;
    /// Wait for the container's process to exit; returns its exit code (0 == clean). Reconciles with
    /// `VmmChild::wait` so both backends share the same launch→run→wait→result lifecycle shape.
    fn wait(&mut self) -> Result<i32, String>;
}

/// The gVisor (`runsc`) second backend — same trait, same hardening, OCI/`runsc` path.
#[derive(Default)]
pub struct GvisorBackend {
    /// guest_id → the live container's teardown state (its `runsc` child + bundle temp dir). Ephemeral;
    /// one job per container, never reused.
    live: Mutex<std::collections::HashMap<String, RunscProc>>,
}

/// A live container's teardown state: the (already-exited/killed) `runsc` child + the bundle temp dir
/// to remove on teardown. Mirrors the Firecracker `GuestProc` (one job per sandbox).
struct RunscProc {
    child: Box<dyn RunscChild + Send>,
    bundle_dir: PathBuf,
}

/// What a launch's run-closure hands back to [`GvisorBackend::launch_with`]: the spawned `runsc` child
/// (already exited/killed by the time this is returned; carried for idempotent teardown), the bundle
/// temp dir (removed on teardown), and the **already-captured** [`SandboxResult`]. Mirrors the
/// Firecracker `GuestRun` — the CT-001 seam now carries a REAL result, no longer a stub. The real
/// production closure ([`run_production_container`]) runs `runsc run --bundle` and fills this from the
/// container's REAL runtime result; unit tests inject a fake child + a canned result so the
/// four-guarantee control flow is testable without a runtime (the injectable-spawn seam — preserved).
pub struct ContainerRun {
    /// The spawned (and, by the time this is returned, already-exited/killed) `runsc` child.
    pub child: Box<dyn RunscChild + Send>,
    /// The bundle temp dir to remove on teardown.
    pub bundle_dir: PathBuf,
    /// The captured command result (exit / timeout / usage / bounded streams).
    pub result: SandboxResult,
}

impl GvisorBackend {
    /// A new backend with no live containers.
    pub fn new() -> GvisorBackend {
        GvisorBackend::default()
    }

    /// Build the OCI config a launch WOULD use for `spec` (the hardened profile derived + the OCI
    /// JSON assembled), without running. Asserts the mandatory profile is in force.
    pub fn oci_config(spec: &JobSpec) -> Result<OciConfig, GvisorError> {
        let profile = HardeningProfile::derive(spec);
        profile.assert_enforced().map_err(GvisorError::Hardening)?;
        Ok(OciConfig::from_spec(spec, &profile))
    }

    /// Drive the four-guarantee seam in the mandated order — **isolation floor → hardening assert →
    /// attribution → reserve → run → settle** — fail-closed at every step, then hand the captured
    /// [`SandboxResult`] back behind the redrawn CT-001 seam. The `run` closure does the actual run:
    /// it stages an OCI bundle from the built [`OciConfig`], runs `runsc run --bundle` (the untrusted
    /// `spec.command`), captures the real exit/streams/usage and enforces `spec.limits.timeout_secs`,
    /// and returns a [`ContainerRun`]. The trait `launch` passes [`run_production_container`] (a REAL
    /// `runsc` container); unit tests pass a closure returning a fake child + a canned result so the
    /// control flow is testable without a runtime (the injectable-spawn seam — preserved). `run` is
    /// only invoked AFTER reserve succeeds, so an exhausted wallet / unmet isolation floor
    /// refuses-to-start and `runsc` never spawns (CT-002b: the result is CONSUMED from the run, never
    /// hardcoded — reconciles with the Firecracker `launch_with`).
    fn launch_with<F>(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
        run: F,
    ) -> Result<SandboxLaunch, GvisorError>
    where
        F: FnOnce(&JobSpec, &OciConfig) -> Result<ContainerRun, String>,
    {
        (hooks.isolation_floor)(spec)?;
        let profile = HardeningProfile::derive(spec);
        profile.assert_enforced().map_err(GvisorError::Hardening)?;
        (hooks.attribute)(&spec.run_token)?;
        let reserve = (hooks.reserve)(&spec.meter_to)?;

        let cfg = OciConfig::from_spec(spec, &profile);
        // Run the container + capture the REAL result (the ONE legitimate `runsc`-spawn site — the
        // sandbox seam's mechanism; the `no-host-exec` named exclusion). `run` cleans up its own
        // bundle/container on error.
        let ContainerRun {
            child,
            bundle_dir,
            result,
        } = run(spec, &cfg).map_err(GvisorError::Runtime)?;

        let guest_id = format!("runsc-{}", spec.idem_token.0);
        self.live
            .lock()
            .unwrap()
            .insert(guest_id.clone(), RunscProc { child, bundle_dir });

        // Settle against the result's REAL measured usage (CT-002b) — never interrupt in-flight.
        (hooks.settle)(&reserve, result.usage)?;

        Ok(SandboxLaunch {
            handle: SandboxHandle { guest_id },
            result,
        })
    }
}

impl SandboxBackend for GvisorBackend {
    type Error = GvisorError;

    /// Run a digest-pinned [`JobSpec`] inside a REAL `runsc` (gVisor) sandbox. Blocks until the
    /// container has run and the four guarantees have fired. The REAL `runsc` container is spawned
    /// here — the one legitimate runtime-spawn site (the `no-host-exec` named exclusion; this seam IS
    /// the unified sandbox, not a bypass of it).
    fn launch(&self, spec: &JobSpec, hooks: &RunnerHooks) -> Result<SandboxLaunch, Self::Error> {
        self.launch_with(spec, hooks, run_production_container)
    }

    /// Whole-container kill on teardown: best-effort destroy the container + remove its bundle temp
    /// dir. The container is ephemeral, never reused. Idempotent — the run path has already deleted
    /// the container + bundle on completion, so killing an already-gone container is a no-op success.
    fn kill(&self, h: &SandboxHandle) -> Result<(), Self::Error> {
        let proc = self.live.lock().unwrap().remove(&h.guest_id);
        if let Some(mut proc) = proc {
            let r = proc.child.kill();
            let _ = std::fs::remove_dir_all(&proc.bundle_dir);
            r.map_err(GvisorError::Runtime)?;
        }
        Ok(())
    }
}

/// The REAL production run path (CT-002b → P-544): build a hardened OCI bundle from the spec and RUN
/// the untrusted `spec.command` inside a REAL `runsc` (gVisor) sandbox, capturing its REAL result.
///
/// **Why gVisor needs no forge defense (unlike Firecracker):** `runsc run` returns the container
/// process's REAL exit status directly to THIS host process — there is NO shared serial console the
/// workload could write — so the exit code is taken from the `runsc` child's real `ExitStatus`, never
/// from any in-container output. stdout/stderr are the runtime's OWN piped fds (two separate streams),
/// not a channel the host trusts the payload to frame; there is no nonce/base64 framing because there
/// is nothing for the payload to forge. The workload STILL runs NON-ROOT (`process.user` =
/// 65534/65534 in the OCI config) — defense in depth; untrusted code is never uid 0 in the sandbox.
///
/// Mechanism (REUSING the proven bundle pattern from `escape_drill_gvisor_test::stage_bundle`):
/// 1. Stage a temp bundle dir: a `rootfs` symlink → [`resolved_gvisor_rootfs`] + a `config.json` =
///    [`OciConfig::to_json`] (read-only root, caps dropped, no-new-privs, seccomp, no netns when
///    egress is default-deny, pids ceiling, NON-ROOT user, an advisory `memory.limit`, and a
///    size-bounded writable `/tmp` tmpfs from `spec.limits.disk_bytes` — CT-003a: a disk fill hits
///    ENOSPC at the quota, never an unbounded host-RAM-backed tmpfs).
///    1b. CT-003b (SI-017): establish an OUT-OF-BAND [`MemoryCgroup`] capped at `spec.limits.mem_bytes`
///    and place the `runsc` process tree (frontend + the sentry/gofer it forks) into it. This is the
///    REAL memory bound — rootless runsc ignores the OCI `memory.limit`, so without this an untrusted
///    job's anonymous memory was UNBOUNDED (a host-DoS escape). FAIL-CLOSED: if the cgroup cannot be
///    established the run REFUSES (never runs the workload unbounded).
/// 2. Run `runsc --rootless --network=none run -bundle <dir> <cid>` with stdout/stderr piped, waiting
///    at most `spec.limits.timeout_secs`; on expiry the WHOLE CONTAINER is killed (`runsc kill <cid>
///    KILL` + the child) ⇒ `timed_out=true`, `exit_code=None`.
/// 3. Best-effort `runsc delete -force <cid>` + remove the bundle dir on EVERY path (no leaks).
fn run_production_container(spec: &JobSpec, cfg: &OciConfig) -> Result<ContainerRun, String> {
    let bin = runsc_bin();
    let rootfs = resolved_gvisor_rootfs();
    // Honest fail-closed: a runtime/start precondition failure surfaces as an error (never a
    // fabricated exit). An absent rootfs cannot produce a valid bundle.
    if !rootfs.exists() {
        return Err(format!(
            "staged gVisor rootfs absent: {} (cannot build a valid OCI bundle)",
            rootfs.display()
        ));
    }
    let bundle_dir = stage_production_bundle(cfg, &rootfs)?;
    let container_id = format!("myelin-prod-{}-{}", std::process::id(), unique_suffix());

    let timeout = Duration::from_secs(spec.limits.timeout_secs as u64);
    let outcome = match run_and_capture(
        &bin,
        &bundle_dir,
        &container_id,
        timeout,
        spec.limits.mem_bytes,
        RunCaptureOptions {
            stdin: None, // CI/agent jobs receive no stdin (the git-wire path supplies the body).
            stdout_mode: StdoutMode::CappedHead,
            cancellation: &NEVER_CANCELLED,
        },
    ) {
        Ok(o) => o,
        Err(e) => {
            // Spawning/waiting failed before a trustworthy result — clean up + surface honestly.
            delete_container(&bin, &container_id);
            let _ = std::fs::remove_dir_all(&bundle_dir);
            return Err(e);
        }
    };
    // The container has exited (or been timeout-killed) — best-effort delete (idempotent; `runsc run`
    // usually self-deletes on a clean exit, but the timeout path leaves it for us to reap).
    delete_container(&bin, &container_id);

    let result = build_result(spec, &outcome, &RedactionPlan::for_job(spec));
    Ok(ContainerRun {
        child: Box::new(SpawnedRunsc { bin, container_id }),
        bundle_dir,
        result,
    })
}

/// A unique suffix for bundle dirs / container ids. The wall-clock nanos alone are NOT unique: two
/// launches on different threads can read the SAME nanosecond (the clock resolution is coarser than a
/// launch), colliding on the bundle path (`symlink rootfs: File exists`). Mixing in a per-process
/// monotonically-incrementing counter makes the suffix collision-proof WITHIN the process; the nanos
/// keep it unique ACROSS processes.
fn unique_suffix() -> u128 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed) as u128;
    // Shift the nanos up and OR in the sequence so both contribute to a unique value.
    (nanos << 24) | (seq & 0xff_ffff)
}

/// Stage a self-contained OCI bundle in a temp dir — the SAME pattern as
/// `escape_drill_gvisor_test::stage_bundle` (no forked recipe): a `rootfs` symlink → the staged
/// minimal rootfs (`runsc` reads `root.path = "rootfs"` relative to the bundle) + the production
/// `config.json` from [`OciConfig::to_json`]. Returns the bundle dir.
fn stage_production_bundle(cfg: &OciConfig, rootfs: &Path) -> Result<PathBuf, String> {
    let bundle = std::env::temp_dir().join(format!(
        "myelin-gvisor-prod-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let _ = std::fs::remove_dir_all(&bundle);
    std::fs::create_dir_all(&bundle).map_err(|e| format!("create bundle dir {bundle:?}: {e}"))?;
    #[cfg(unix)]
    std::os::unix::fs::symlink(rootfs, bundle.join("rootfs"))
        .map_err(|e| format!("symlink rootfs into bundle: {e}"))?;
    std::fs::write(bundle.join("config.json"), cfg.to_json())
        .map_err(|e| format!("write config.json: {e}"))?;
    Ok(bundle)
}

/// Best-effort idempotent container delete (`runsc --rootless delete -force <cid>`). Deleting an
/// already-gone container is a harmless no-op — called on EVERY teardown path so no container leaks.
fn delete_container(bin: &str, container_id: &str) {
    let _ = Command::new(bin)
        .arg("--rootless")
        .arg("delete")
        .arg("-force")
        .arg(container_id)
        .output();
}

/// Read the `runsc` process's cumulative CPU time (utime+stime) from `/proc/<pid>/stat`, in whole
/// seconds (USER_HZ = 100 on Linux). Mirrors the Firecracker backend's measurement (a small,
/// backend-specific `/proc` read of the spawned runtime's pid). `None` if `/proc` is unavailable or
/// unparseable (then the caller falls back to a wall-clock figure — a real run never under-meters to 0).
fn read_proc_cpu_seconds(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The comm field (field 2) is parenthesised and may contain spaces/`)`; skip past the LAST ')'.
    let rest = &stat[stat.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // After comm: state(3) ppid(4) ... utime(14) stime(15) ... ⇒ rest indices 11/12.
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some((utime + stime) / 100)
}

/// The raw outcome of running `runsc run` to completion (or to a timeout-kill) — consumed by
/// [`build_result`] into a [`SandboxResult`].
struct RunscOutcome {
    /// The `runsc` child's REAL exit status = the container process's actual exit code. `None` if the
    /// container was timeout-killed (no trustworthy code — never fabricated).
    exit: Option<i32>,
    /// True iff the wall-clock `timeout_secs` ceiling fired and the whole container was killed.
    timed_out: bool,
    /// The container's REAL piped stdout (the runtime's fd, not in-container framing). Bounded by the
    /// stream's [`StdoutMode`] (the 256 KiB head bound for CI/agent logs; the GENEROUS git-wire cap,
    /// disk-streamed, for the wire path).
    stdout: Vec<u8>,
    /// True iff the stdout stream exceeded its [`StdoutMode`] bound (head-truncated). For the CI/agent
    /// path this is benign (logs are head-captured by design); for the git-wire path it is FATAL — a
    /// truncated packfile fails the client's `index-pack` with "early EOF", so the wire seam REFUSES it
    /// loudly (never returns a silently-truncated pack). See [`run_git_wire_container`].
    stdout_truncated: bool,
    /// The container's REAL piped stderr (always 256 KiB head-bounded — it is error text, not payload).
    stderr: Vec<u8>,
    /// Wall-clock duration the container ran.
    wall: Duration,
    /// Host-side CPU-seconds of the `runsc` process (utime+stime from `/proc`), if readable.
    cpu_seconds: Option<u64>,
}

/// Spawn the REAL `runsc` container (`runsc --rootless --network=none run -bundle <dir> <cid>`) — THE
/// one legitimate runtime-spawn site (the `no-host-exec` named exclusion; the mechanism that CREATES
/// the userspace-kernel boundary, not a bypass) — drain its stdout/stderr on dedicated threads (so a
/// chatty container cannot fill a pipe buffer and deadlock), and wait at most `timeout`. On expiry the
/// WHOLE CONTAINER is killed (`runsc kill <cid> KILL` then the child) and `timed_out` is set. The exit
/// code is the `runsc` child's REAL `ExitStatus.code()` — the container process's actual exit, never
/// parsed from container output (the structural reason gVisor needs no forge defense).
fn run_and_capture(
    bin: &str,
    bundle: &Path,
    container_id: &str,
    timeout: Duration,
    mem_bytes: u64,
    options: RunCaptureOptions<'_>,
) -> Result<RunscOutcome, String> {
    let RunCaptureOptions {
        stdin,
        stdout_mode,
        cancellation,
    } = options;
    // CT-003b (SI-017): establish the OUT-OF-BAND memory cgroup BEFORE spawning runsc and FAIL
    // CLOSED if it cannot be established (rootless runsc would otherwise run the workload's anonymous
    // memory UNBOUNDED — a host-DoS escape). The cgroup is torn down on every path (its `Drop`).
    let cgroup = MemoryCgroup::create(mem_bytes)?;

    let mut cmd = Command::new(bin);
    cmd.arg("--rootless")
        .arg("--network=none")
        .arg("run")
        .arg("-bundle")
        .arg(bundle)
        .arg(container_id)
        // CT-006a: the git-wire path pipes the stateless-rpc request body in; CI/agent jobs get no
        // stdin (`null`). The bytes are already bounded by [`WIRE_STDIN_BOUND`] before we get here.
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Place the runsc child (and the sentry/gofer tree it forks) into the memory cgroup at birth.
    cgroup
        .place_child(&mut cmd)
        .map_err(|e| format!("bind runsc into the memory cgroup: {e}"))?;
    let mut child = cmd.spawn().map_err(|e| format!("spawn runsc: {e}"))?;

    // CT-006a: feed the bounded request body to the container's stdin on a DEDICATED thread (so a
    // large body + a slow in-guest reader cannot deadlock against our stdout/stderr drains), then drop
    // the handle to deliver EOF (the stateless-rpc request terminator). None ⇒ stdin was `null`.
    let stdin_th = stdin.map(|bytes| {
        let mut si = child.stdin.take().expect("piped stdin");
        std::thread::spawn(move || {
            let _ = si.write_all(&bytes);
            // `si` drops here ⇒ the write end closes ⇒ the guest `git` sees EOF on its request body.
        })
    });

    let pid = child.id();
    let start = Instant::now();

    // Drain both pipes on threads so a chatty container cannot fill a pipe buffer and deadlock.
    let mut out = child.stdout.take().expect("piped stdout");
    let mut err = child.stderr.take().expect("piped stderr");
    // stdout draining depends on the stream's mode (CT-006c):
    //   - `CappedHead` (CI/agent logs): cap at SANDBOX_CAPTURE_BOUND (256 KiB head capture) + DISCARD the
    //     rest to EOF — bounds host memory under a runaway container, byte-unchanged from CT-002c.
    //   - `StreamToFile` (the git wire): stream straight to a host TEMP FILE under a GENEROUS cap so a
    //     real-size packfile (megabytes, not 256 KiB) comes through WHOLE while host RAM stays one chunk
    //     (the bytes land on disk, not in a growing Vec). Over the generous cap ⇒ `truncated` (the wire
    //     seam then REFUSES loudly — never a silently-truncated pack). Both keep reading past the bound
    //     so the container never blocks on a full pipe (no deadlock that would defeat the timeout).
    let th_out = std::thread::spawn(move || match stdout_mode {
        StdoutMode::CappedHead => drain_capped(&mut out, SANDBOX_CAPTURE_BOUND),
        StdoutMode::StreamToFile { bound } => drain_to_temp_file(&mut out, bound),
    });
    // stderr is ALWAYS the 256 KiB head bound — it is error text folded into a message, never payload.
    let th_err = std::thread::spawn(move || drain_capped(&mut err, SANDBOX_CAPTURE_BOUND).0);

    let mut timed_out = false;
    let mut last_cpu: Option<u64> = None;
    let exit = loop {
        if let Some(status) = child.try_wait().map_err(|e| format!("wait runsc: {e}"))? {
            break status.code();
        }
        if let Some(c) = read_proc_cpu_seconds(pid) {
            last_cpu = Some(c);
        }
        let cancelled = cancellation.load(Ordering::Acquire);
        if cancelled || start.elapsed() >= timeout {
            // Wall-clock ceiling hit: whole-CONTAINER kill (SIGKILL the container's PID1 via the
            // runtime), then reap the `runsc` child process so the pipes hit EOF.
            let _ = Command::new(bin)
                .arg("--rootless")
                .arg("kill")
                .arg(container_id)
                .arg("KILL")
                .output();
            let _ = child.kill();
            let _ = child.wait();
            timed_out = !cancelled;
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let wall = start.elapsed();

    // The child has exited/been-killed ⇒ the pipes hit EOF ⇒ the drain threads finish.
    let (stdout, stdout_truncated) = th_out.join().unwrap_or_default();
    let stderr = th_err.join().unwrap_or_default();
    // The writer thread has finished (the child read its request body, or it exited and the write
    // EPIPE'd — either way `write_all` returned). Join so no thread outlives the run.
    if let Some(t) = stdin_th {
        let _ = t.join();
    }

    // The container + its sentry/gofer tree are gone; reap the cgroup (kill any straggler, rmdir).
    // `Drop` is the backstop, but tear it down deterministically here on the success/timeout path.
    cgroup.cleanup();

    Ok(RunscOutcome {
        exit,
        timed_out,
        stdout,
        stdout_truncated,
        stderr,
        wall,
        cpu_seconds: last_cpu,
    })
}

/// How the container's stdout is drained (CT-006c). The non-wire (CI/agent) path keeps the byte-unchanged
/// 256 KiB head capture; the git-wire path streams to a host temp file under a generous cap so a
/// real-size packfile survives whole while host RAM stays bounded to one chunk.
enum StdoutMode {
    /// CI/agent logs: head-capture the first [`SANDBOX_CAPTURE_BOUND`] bytes in RAM, discard the rest.
    CappedHead,
    /// The git wire: stream straight to a host temp file under `bound` bytes (host RAM stays one 64 KiB
    /// chunk regardless of pack size), then materialize back. Over `bound` ⇒ the returned `truncated`
    /// flag is set and the wire seam refuses loudly.
    StreamToFile { bound: usize },
}

struct RunCaptureOptions<'a> {
    stdin: Option<Vec<u8>>,
    stdout_mode: StdoutMode,
    cancellation: &'a AtomicBool,
}

/// **Drain a child stream straight to a host TEMP FILE under a generous byte cap (the git-wire path,
/// CT-006c).** Host MEMORY stays bounded to ONE 64 KiB chunk regardless of how large the packfile is —
/// the bytes are written to disk as they arrive, NOT buffered in a growing Vec. Keeps reading past the
/// cap (draining + discarding) so the container never blocks on a full pipe (no deadlock that would
/// defeat the timeout). Returns the materialized head (≤ `cap` bytes, read back from the temp file) and
/// whether the cap was exceeded (`truncated`). The temp file is removed before returning (no leak).
///
/// NOTE (future, documented): materializing back into a `Vec` still costs `min(pack, cap)` host RAM at
/// the end — true end-to-end streaming would need a `WireOutput`/`SandboxResult` streaming-body API
/// change. For CT-006c, disk-staging the drain (so RAM is bounded DURING the run) + a generous cap is
/// sufficient: real-size clones come through whole, and an over-cap response fails loud rather than
/// returning a truncated pack.
fn drain_to_temp_file<R: Read>(mut r: R, cap: usize) -> (Vec<u8>, bool) {
    let path = std::env::temp_dir().join(format!(
        "myelin-gitwire-stdout-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let _ = std::fs::remove_file(&path);
    let mut file = match std::fs::File::create(&path) {
        Ok(f) => f,
        // If we cannot stage to disk, fall back to the in-RAM capped drain under the SAME generous cap
        // (still bounded, still reports truncation) rather than losing the deadlock-free pipe drain.
        Err(_) => return drain_capped(&mut r, cap),
    };
    let mut written: usize = 0;
    let mut truncated = false;
    let mut chunk = [0u8; 64 * 1024];
    loop {
        match r.read(&mut chunk) {
            Ok(0) => break, // EOF: the guest pipe closed (child exited / whole-container-killed).
            Ok(n) => {
                if written < cap {
                    let take = (cap - written).min(n);
                    if file.write_all(&chunk[..take]).is_err() {
                        // A disk write error ⇒ stop staging but KEEP draining to EOF (no pipe-fill
                        // deadlock); treat as truncated so the wire seam refuses rather than serving a
                        // short pack.
                        truncated = true;
                        written = cap;
                    } else {
                        written += take;
                        if take < n {
                            truncated = true; // overflowed the cap this read
                        }
                    }
                } else {
                    truncated = true; // already at the cap: drain + discard the remainder
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break, // pipe error ⇒ end-of-stream; the wait/kill loop owns lifecycle.
        }
    }
    let _ = file.flush();
    drop(file);
    // Read the staged bytes back (≤ cap). A read-back failure is treated as truncated (fail-closed:
    // the wire seam refuses rather than serving a short/empty pack).
    let head = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(_) => {
            truncated = true;
            Vec::new()
        }
    };
    let _ = std::fs::remove_file(&path);
    (head, truncated)
}

/// Build the [`SandboxResult`] from the runtime outcome. The exit code is the `runsc` child's REAL
/// exit status (gVisor returns the container process's exit directly — no forge surface); a timed-out
/// (killed) container has no trustworthy exit (`None`, never fabricated as 0). stdout/stderr are the
/// container's REAL piped streams, HEAD-bounded to [`SANDBOX_CAPTURE_BOUND`] (256 KiB) EACH — the same
/// bound the Firecracker backend uses (shared `lib.rs` const). Usage is the REAL measured figure: host
/// CPU-seconds of the `runsc` process (or a wall-clock ceiling fallback so a real run never
/// under-meters to 0) + mem-byte-seconds from the job's mem ceiling × wall-seconds.
fn build_result(spec: &JobSpec, o: &RunscOutcome, redaction: &RedactionPlan) -> SandboxResult {
    // BOUNDARY REDACTION (CT-004f sub-step 1): mask the job's CI-managed secret needles in the captured
    // streams HERE — the last step before the bytes populate `SandboxResult` and cross back toward the
    // durable log pipeline — so no injected secret is sealed into the content-addressed log store. The
    // plan is EMPTY today (nothing injects secrets), so this is a no-op; it is a REQUIRED argument so no
    // capture path can forward un-redacted bytes, and CI-1 secret injection must populate it (see
    // `crate::redaction`). Redaction runs on the already-per-stream-bounded bytes.
    //
    // The drain threads ALREADY applied the correct per-stream bound ([`run_and_capture`]): stdout is
    // bounded by its [`StdoutMode`] (256 KiB head for CI/agent; the generous git-wire cap, disk-staged),
    // stderr by [`SANDBOX_CAPTURE_BOUND`]. Re-truncating stdout to 256 KiB HERE would corrupt a real-size
    // wire packfile (CT-006c FU-1 — the original silent-truncation defect), so the already-bounded bytes
    // pass through unchanged. stderr is belt-and-braces clamped (it is already ≤ the bound).
    let stdout = redaction.redact(&o.stdout);
    let mut stderr = redaction.redact(&o.stderr);
    if stderr.len() > SANDBOX_CAPTURE_BOUND {
        stderr.truncate(SANDBOX_CAPTURE_BOUND);
    }
    // A timed-out container was killed mid-flight ⇒ no trustworthy exit code (do NOT fabricate one).
    let exit_code = if o.timed_out { None } else { o.exit };

    let wall_secs_ceil = o.wall.as_secs() + u64::from(o.wall.subsec_nanos() > 0);
    let cpu_seconds = o.cpu_seconds.filter(|c| *c > 0).unwrap_or(wall_secs_ceil);
    let mem_byte_seconds = spec.limits.mem_bytes.saturating_mul(wall_secs_ceil);

    SandboxResult {
        exit_code,
        timed_out: o.timed_out,
        usage: ResourceUsage {
            cpu_seconds,
            mem_byte_seconds,
        },
        stdout,
        stderr,
    }
}

/// The post-run container teardown handle (the container has already exited/been-killed + been
/// deleted by the run path). [`kill`](RunscChild::kill) is an idempotent best-effort `runsc delete
/// -force` (no-op if already gone). [`wait`](RunscChild::wait) is a no-op for trait parity with the
/// Firecracker `VmmChild` — the REAL exit was already captured from the `runsc` child's exit status.
struct SpawnedRunsc {
    bin: String,
    container_id: String,
}
impl RunscChild for SpawnedRunsc {
    fn kill(&mut self) -> Result<(), String> {
        delete_container(&self.bin, &self.container_id);
        Ok(())
    }
    fn wait(&mut self) -> Result<i32, String> {
        // The real exit was already captured from the `runsc` child's exit status (no real wait left).
        Ok(0)
    }
}

// ---------------------------------------------------------------------------------------------
// CI-P28 (P-423) — the escape drill RE-RUNS on the gVisor backend (the permanent gate, 8.4).
//
// The Firecracker drill (CI-P5) boots a microVM and runs the corpus as PID1; gVisor's drill runs
// the SAME seven adversarial families inside a real `runsc` (gVisor) userspace-kernel sandbox via a
// minimal OCI bundle (`runsc run --bundle`). The bundle expresses the SAME mandatory hardening
// posture the [`HardeningProfile`] computes (read-only root, all caps dropped, no-new-privs, pids
// ceiling, NO network namespace), so the corpus is contained by the SAME profile, enforced through
// gVisor's mechanism (the OCI spec) instead of Firecracker's (the microVM drive/NIC config).
//
// BACKEND-SHAPED PROBE (an honest, documented deviation — EI-01 §1). The corpus tests a *property*
// (no raw physical-memory / I/O-port access); the in-guest *probe* of that property is necessarily
// backend-shaped. On Firecracker the privileged device nodes are real and EPERM on write; on gVisor
// they are simply ABSENT and creating one is denied (no CAP_MKNOD). So the gVisor corpus probes
// `K2_devmem`/`K3_ioport` as "the node is absent AND mknod is denied" rather than "dd EPERMs" — the
// SAME contained property, the faithful gVisor expression of it. The marker ids + the host-side
// parser ([`parse_console`]) are IDENTICAL across backends, so the gate predicate is one path.
// ---------------------------------------------------------------------------------------------

/// Env var naming the staged minimal OCI rootfs the gVisor escape drill runs the corpus in (a clean
/// rootfs with NO privileged device nodes — busybox-class). Defaults to the staged asset dir.
pub const ENV_GVISOR_ROOTFS: &str = "MYELIN_GVISOR_ROOTFS";

/// The resolved minimal rootfs path for the gVisor drill (env override →
/// `~/.local/share/gvisor-assets/rootfs`). The drill SKIPS gracefully if it is absent.
pub fn resolved_gvisor_rootfs() -> std::path::PathBuf {
    if let Ok(p) = std::env::var(ENV_GVISOR_ROOTFS) {
        return std::path::PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    std::path::PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("gvisor-assets")
        .join("rootfs")
}

/// The name of the in-guest corpus script the OCI bundle runs (placed at the rootfs root). It runs
/// as the container's `process.args` entrypoint and prints the SAME `<id> CONTAINED|ESCAPED` markers
/// the Firecracker corpus does, bracketed by the SAME begin/end markers, so [`parse_console`] over
/// the captured gVisor console produces a [`DrillReport`](crate::escape_corpus::DrillReport) exactly
/// as for Firecracker (ONE gate predicate, two backends).
pub const GVISOR_CORPUS_SCRIPT: &str = "myelin-agd4-corpus.sh";

/// Build the in-guest corpus script for the gVisor backend — the SAME seven adversarial families as
/// [`build_corpus_script`](crate::escape_corpus::build_corpus_script), expressed for gVisor's device
/// model (mknod-denial for the raw-device family; `--network=none` ⇒ no route for the egress family;
/// the OCI `pids.limit` for the fork bomb; read-only root for disk fill). `pids_max` is the OCI
/// `linux.resources.pids.limit` the bundle sets. The markers + ids are byte-identical to the
/// Firecracker corpus so the host-side parser is one path.
pub fn build_gvisor_corpus_script(pids_max: u32) -> String {
    use crate::escape_corpus::{BEGIN_MARKER, CORPUS_VERSION, END_MARKER};
    // `pids_max` is a u32 decimal literal — no shell-injection surface.
    format!(
        r#"#!/bin/sh
# AG-D4 / CI-T1 adversarial escape corpus (corpus_version={cv}) — gVisor (runsc) backend.
# Run inside a real runsc userspace-kernel sandbox via a hardened OCI bundle. Markers parsed by the
# SAME host-side parser as the Firecracker drill: "<id> CONTAINED" iff the attack genuinely failed.
echo "{begin} corpus_version={cv} kernel=$(uname -r 2>/dev/null) guest_euid=$(id -u 2>/dev/null)"

# ---- kernel-exploit primitives (run UNDER the bundle's all-caps-dropped + no-new-privs posture) ----
# K1: load a kernel module — gVisor has no module support; EPERM/unsupported ⇒ contained.
if modprobe dummy 2>/dev/null || insmod /x.ko 2>/dev/null; then echo "K1_module ESCAPED"; else echo "K1_module CONTAINED"; fi
# K2: raw physical-memory device — ABSENT in the hardened rootfs AND mknod is denied (no CAP_MKNOD).
if [ -e /dev/mem ]; then echo "K2_devmem ESCAPED present"; elif mknod /dev/mem c 1 1 2>/dev/null; then echo "K2_devmem ESCAPED mknod"; else echo "K2_devmem CONTAINED"; fi
# K3: raw I/O-port device — same (absent + mknod denied).
if [ -e /dev/port ]; then echo "K3_ioport ESCAPED present"; elif mknod /dev/port c 1 4 2>/dev/null; then echo "K3_ioport ESCAPED mknod"; else echo "K3_ioport CONTAINED"; fi
# K4: privileged mount — denied (no CAP_SYS_ADMIN).
if mount -t tmpfs none /mnt 2>/dev/null; then echo "K4_mount ESCAPED"; else echo "K4_mount CONTAINED"; fi

# ---- egress family: the OCI bundle has NO network namespace (--network=none) ⇒ only loopback,
#      no route to any external/metadata/control-plane/cross-tenant host. ----
if timeout 3 wget -q -T 2 -O- http://169.254.169.254/latest/meta-data/ 2>/dev/null; then echo "M1_metadata_ssrf ESCAPED"; else echo "M1_metadata_ssrf CONTAINED"; fi
if timeout 3 nc -w 2 10.0.0.1 8080 </dev/null 2>/dev/null; then echo "C1_controlplane ESCAPED"; else echo "C1_controlplane CONTAINED"; fi
if timeout 3 nc -w 2 192.168.13.37 443 </dev/null 2>/dev/null; then echo "T1_crosstenant ESCAPED"; else echo "T1_crosstenant CONTAINED"; fi
SECRET="myelin-drill-canary-$$"
if timeout 3 nc -w 2 93.184.216.34 80 </dev/null 2>/dev/null; then echo "E1_secret_exfil ESCAPED $SECRET"; else echo "E1_secret_exfil CONTAINED"; fi

# ---- fork bomb: the OCI bundle sets linux.resources.pids.limit={pids_max}; the ceiling must HOLD and
#      the guest must STAY UP (reaching the END marker proves it survived). ----
n=0; while [ $n -lt 300 ]; do /bin/sleep 30 & n=$((n+1)); done 2>/dev/null
echo "F1_forkbomb CONTAINED survived=yes ceiling={pids_max}"

# ---- disk fill + read-only root ----
if echo x 2>/dev/null > /root_write_probe; then echo "D1_root_readonly ESCAPED"; rm -f /root_write_probe 2>/dev/null; else echo "D1_root_readonly CONTAINED"; fi
mkdir -p /tmp/scratch 2>/dev/null
if dd if=/dev/zero of=/tmp/scratch/fill bs=1M count=512 2>/dev/null; then echo "D2_diskfill ESCAPED"; else echo "D2_diskfill CONTAINED"; fi

# ---- anonymous-memory hog (Mx_memhog) — bounded by the OUT-OF-BAND host memory cgroup the
#      production run path places the runsc process tree into (rootless runsc does NOT enforce the
#      OCI memory.limit, so the cgroup is the real enforcer). An oversized anon alloc breaches
#      memory.max and the kernel OOM-kills the sentry within the limit ⇒ the WHOLE container dies
#      mid-alloc. So containment is STRUCTURAL: the ATTEMPT sentinel + the END marker are printed
#      (and flushed) BEFORE the alloc; the ESCAPED line prints ONLY if the oversized alloc HELD (the
#      bound failed / the cgroup was dropped). The host-side parser reads ATTEMPT-present-and-ESCAPED-
#      absent as CONTAINED. A regression dropping the cgroup => the hog HELDs => ESCAPED => RED. ----
echo "{memhog_id} ATTEMPT bytes={hog}"
echo "{end}"
( S=aaaaaaaaaaaaaaaa; n=0; while [ $n -lt 26 ]; do S="$S$S"; n=$((n+1)); done; echo "{memhog_id} ESCAPED held=${{#S}}" ) 2>/dev/null
echo "{memhog_id} CONTAINED via_cgroup"
"#,
        cv = CORPUS_VERSION,
        begin = BEGIN_MARKER,
        end = END_MARKER,
        pids_max = pids_max,
        memhog_id = crate::escape_corpus::MEMHOG_ID,
        hog = crate::escape_corpus::MEMHOG_BYTES,
    )
}

/// Build the hardened OCI `config.json` for the gVisor escape-drill bundle from a [`JobSpec`]'s
/// derived [`HardeningProfile`]. It expresses the SAME mandatory posture the Firecracker backend
/// enforces, through the OCI spec gVisor consumes: **read-only root**, **all caps dropped**,
/// **no-new-privileges**, the **pids ceiling**, and **no network namespace** (so `--network=none`
/// leaves only loopback). The entrypoint runs the in-guest corpus script (placed at `/{script}`).
///
/// NOTE: the `user` namespace is deliberately NOT listed — `runsc --rootless` adds its own user
/// namespace, and a doubly-declared userns makes the rootless gofer fork/exec fail. This is the one
/// rootless-specific deviation; the security posture (caps/nnp/ro-root/no-net/pids) is unchanged.
pub fn gvisor_drill_config_json(spec: &JobSpec, script_name: &str) -> Result<String, GvisorError> {
    let profile = HardeningProfile::derive(spec);
    profile.assert_enforced().map_err(GvisorError::Hardening)?;
    // No network namespace ⇒ with `runsc --network=none` only loopback exists (egress closed).
    let json = format!(
        r#"{{
  "ociVersion": "1.0.0",
  "process": {{
    "terminal": false,
    "user": {{ "uid": 0, "gid": 0 }},
    "args": ["/bin/sh", "/{script}"],
    "env": ["PATH=/bin:/sbin:/usr/bin:/usr/sbin"],
    "cwd": "/",
    "noNewPrivileges": {nnp},
    "capabilities": {{ "bounding": [], "effective": [], "permitted": [], "inheritable": [], "ambient": [] }}
  }},
  "root": {{ "path": "rootfs", "readonly": {ro} }},
  "hostname": "myelin-agd4",
  "mounts": [
    {{ "destination": "/proc", "type": "proc", "source": "proc" }},
    {{ "destination": "/dev", "type": "tmpfs", "source": "tmpfs", "options": ["nosuid", "strictatime", "mode=755", "size=65536k"] }},
    {{ "destination": "/tmp", "type": "tmpfs", "source": "tmpfs", "options": ["nosuid", "nodev", "size=8m"] }}
  ],
  "linux": {{
    "namespaces": [
      {{ "type": "pid" }}, {{ "type": "ipc" }}, {{ "type": "uts" }}, {{ "type": "mount" }}
    ],
    "resources": {{ "pids": {{ "limit": {pids} }} }},
    "seccomp": {{ "defaultAction": "SCMP_ACT_ALLOW" }}
  }}
}}"#,
        script = script_name,
        nnp = profile.no_new_privileges,
        ro = profile.read_only_root,
        pids = profile.pids_max,
    );
    Ok(json)
}

// ═══════════════════════════════════════════════════════════════════════════════════════════════
// CT-006a (GT-006 / SI-013) — the SANDBOXED GIT-WIRE capability.
//
// The git smart-transport wire (`upload-pack` = clone/fetch, `receive-pack` = push) is canonical
// `git` processing UNTRUSTED client pack/negotiation bytes, so it MUST run under the SAME proven
// hardening this backend already enforces for CI/agent jobs (ro-root, all-caps-dropped, no-new-privs,
// seccomp, no-netns egress-deny, non-root uid, bounded mem/pids/disk, whole-container kill + cleanup,
// bounded capture). On top of that floor the git wire needs THREE things this section adds, all by
// REUSING the machinery above:
//   1. the bare repo BIND-MOUNTED READ-ONLY at `/repo` (a serve can never mutate it) — a [`WireMount`]
//      rendered into the SAME OCI `mounts` array as the `/tmp` tmpfs;
//   2. a WRITABLE QUARANTINE bind-mount at `/quarantine` for `receive-pack` object intake — the
//      sandboxed receive-pack writes objects THERE, the host-side ref-CAS policy (CT-006b) inspects it
//      AFTER the run; it NEVER touches the real repo;
//   3. BOUNDED stdin delivery (the stateless-rpc request body, capped at [`WIRE_STDIN_BOUND`]) piped to
//      the runsc child + captured stdout (the response) via the existing [`drain_capped`] bound.
//
// SECURITY — path confinement (the GT-001 isolation boundary, replicated): the (tenant, region, repo)
// locator is URL/client-influenced, so a raw `PathBuf::join` is a cross-tenant path-traversal breakout.
// [`resolve_bare_repo_path`] VALIDATES every segment against the allowlist `[A-Za-z0-9._-]` (no empty /
// `.` / `..` / separator / NUL / absolute) and REFUSES before any path is built — the byte-for-byte
// guarantee of `myelin_git::gix_backend::validate_path_segment` (the GT-001 fix). It is REPLICATED here
// (not imported) because `myelin-git` is a dev-dep only — the CI sandbox must carry NO production edge
// to the git crate (X-1 acyclic: CI emits, Git reads); a drift test in CT-006b pins the two in sync.
// ═══════════════════════════════════════════════════════════════════════════════════════════════

/// The fixed guest mount point for the READ-ONLY bare repo (the git argv ends with this path).
pub const WIRE_REPO_MOUNT: &str = "/repo";
/// The fixed guest mount point for the WRITABLE push-object quarantine (`receive-pack` intake).
pub const WIRE_QUARANTINE_MOUNT: &str = "/quarantine";

/// **The receive-pack PACK-INGEST script (CT-006d) — the in-sandbox writable-quarantine solution.**
///
/// The push write path's hardest constraint: a host bind-mounted `/quarantine` is NOT writable by the
/// in-guest non-root uid 65534 under rootless `runsc` (a write fails EINVAL on the gofer). So the
/// quarantine lives entirely in the guest's OWN writable `/tmp` tmpfs (writable by 65534 — the SAME
/// tmpfs `git upload-pack` already uses for `HOME`), and the ingested objects are streamed OUT to the
/// host on stdout (captured by the existing wire stdout streaming), NEVER through a host bind.
///
/// Step by step (busybox `sh`; the canonical `git` does ALL pack parsing — the UNTRUSTED client pack is
/// parsed only HERE, inside the hardened sandbox, never on the host):
///   1. `git init --bare /tmp/q` — a throwaway quarantine repo on the writable tmpfs.
///   2. `objects/info/alternates → /repo/objects` — so a THIN pack's delta bases (objects already in the
///      RO real repo) resolve. `/repo` stays READ-ONLY (alternates only READ it).
///   3. `git index-pack --stdin --fix-thin` — ingest the pushed pack from stdin into the quarantine's
///      `objects/pack/`, VALIDATING every object's sha + the pack checksum and resolving deltas against
///      the alternates. A corrupt/forged/incomplete pack (a base neither sent nor present) makes
///      `index-pack` exit non-zero → the launch reports the failure and the host rejects the push (no
///      objects migrate, no ref moves). Its progress/stats go to stderr (`1>&2`) so they never corrupt
///      the object stream on stdout.
///   4. `verify-pack -v … | awk` — list the oids the push INTRODUCED (exactly the objects in the received
///      pack); `git cat-file --batch` then streams each as a FULLY-RESOLVED raw object
///      (`<oid> SP <type> SP <size>\n<payload>\n`) on stdout — deltas already applied. The host parses
///      this trivial framing (NO pack indexer runs on the host over untrusted bytes), re-hashes each
///      object (a forged oid is impossible — git2's `odb.write` recomputes the sha), runs the in-process
///      policy + connectivity check, and ONLY THEN migrates the objects into the real repo under the
///      one-tx ref-CAS + outbox. `git receive-pack` is deliberately NEVER run server-side (it would
///      update the RO real repo's refs) — the in-sandbox git ONLY ingests bytes; the ref move is the
///      trusted in-process host code's job alone.
///
/// NO untrusted data is interpolated into this script — the pushed pack arrives only on stdin, never as
/// an argv/shell token; the ref-update commands are parsed by the host BEFORE the sandbox is invoked.
pub const RECEIVE_PACK_INGEST_SCRIPT: &str = "set -e
export HOME=/tmp
export GIT_CONFIG_NOSYSTEM=1
export GIT_EXEC_PATH=/usr/lib/git-core
export GIT_CONFIG_COUNT=1
export GIT_CONFIG_KEY_0=safe.directory
export GIT_CONFIG_VALUE_0=*
export GIT_DIR=/tmp/q
rm -rf /tmp/q
git init --bare -q /tmp/q
printf '%s\\n' /repo/objects > /tmp/q/objects/info/alternates
git index-pack --stdin --fix-thin 1>&2
git verify-pack -v /tmp/q/objects/pack/pack-*.idx | awk '$2 ~ /^(commit|tree|blob|tag)$/ {print $1}' > /tmp/oids
git cat-file --batch < /tmp/oids
";
/// The documented cap on the stateless-rpc REQUEST BODY (stdin) the wire delivers into the guest —
/// 64 MiB. A larger body is REFUSED fail-closed before any container spawns (a client cannot force the
/// host to buffer an unbounded request). CT-006b may revisit this for very large pushes (chunked
/// intake); for CT-006a it bounds the negotiation/advertise request bodies with margin to spare.
pub const WIRE_STDIN_BOUND: usize = 64 * 1024 * 1024;
/// The DEFAULT generous cap on the git-wire RESPONSE (the upload-pack packfile / advertisement) the
/// container streams to stdout — 512 MiB, matching the serving tier's default `disk_bytes` scratch
/// quota. UNLIKE the 256 KiB [`SANDBOX_CAPTURE_BOUND`] used for CI/agent logs, the wire response is a
/// real packfile (megabytes for a real repo), so it gets this large bound — STREAMED to a host temp
/// file ([`drain_to_temp_file`]) so host RAM stays bounded to one chunk during the run. The LIVE cap is
/// derived per-launch from `spec.limits.disk_bytes` (so it is configurable via [`ResourceLimits`]); this
/// const documents the default. A response that exceeds the live cap is REFUSED loudly at the wire seam
/// ([`WireError::OutputTooLarge`]) — never a silently-truncated pack (which would fail the client's
/// `index-pack` with "early EOF"). True end-to-end streaming (no materialization) is a future
/// `WireOutput` streaming-body API change.
pub const WIRE_STDOUT_BOUND: usize = 512 * 1024 * 1024;
/// Env var naming a staged rootfs that CONTAINS `git` (busybox-class rootfs + `git` + its `git-core`
/// helpers + the shared-lib closure). Defaults to `~/.local/share/gvisor-assets/git-rootfs`. The git
/// wire REQUIRES a real `git` in the guest — see the staging recipe in `tests/git_wire_prod_exec_test.rs`.
pub const ENV_GVISOR_GIT_ROOTFS: &str = "MYELIN_GVISOR_GIT_ROOTFS";

/// The resolved rootfs the git-wire container runs in (env override → the staged git-rootfs asset).
/// SEPARATE from [`resolved_gvisor_rootfs`] because the escape-drill rootfs is busybox-only (no `git`);
/// the git wire needs a `git`-bearing rootfs. The launch fails closed (honest `Runtime` error) if it
/// is absent — it never fabricates a result.
pub fn resolved_gvisor_git_rootfs() -> PathBuf {
    if let Ok(p) = std::env::var(ENV_GVISOR_GIT_ROOTFS) {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("gvisor-assets")
        .join("git-rootfs")
}

/// A git-wire backend error.
#[derive(Debug)]
pub enum WireError {
    /// The (tenant, region, repo) locator failed path-confinement validation — REFUSED before any
    /// mount (cross-tenant / `..` / separator / absolute / non-allowlisted segment).
    Path(String),
    /// The request body exceeded [`WIRE_STDIN_BOUND`] — refused fail-closed before spawning.
    StdinTooLarge {
        /// The offending body length.
        len: usize,
        /// The cap it breached.
        cap: usize,
    },
    /// The wire RESPONSE (upload-pack packfile / advertisement) exceeded the generous wire cap (derived
    /// from `disk_bytes`, default [`WIRE_STDOUT_BOUND`]) — REFUSED fail-LOUD rather than returning a
    /// silently-truncated pack (a truncated pack fails the client's `index-pack` with "early EOF").
    OutputTooLarge {
        /// The cap it breached (bytes).
        cap: usize,
    },
    /// The mandatory hardening profile could not be asserted in force (fail-closed).
    Hardening(String),
    /// A four-guarantee hook failed (cost-exhausted / token-rejected / isolation-floor-not-met).
    Hook(HookError),
    /// The `runsc` runtime / bundle staging errored (absent git rootfs, spawn failure, …).
    Runtime(String),
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WireError::Path(s) => write!(f, "git-wire: path confinement refused: {s}"),
            WireError::StdinTooLarge { len, cap } => write!(
                f,
                "git-wire: request body {len} bytes exceeds the {cap}-byte cap (refused fail-closed)"
            ),
            WireError::OutputTooLarge { cap } => write!(
                f,
                "git-wire: upload-pack response exceeded the {cap}-byte wire cap — refusing a TRUNCATED \
                 pack (a short packfile fails the client's `index-pack` with 'early EOF'); fail-loud"
            ),
            WireError::Hardening(s) => write!(f, "git-wire: hardening not enforced: {s}"),
            WireError::Hook(e) => write!(f, "git-wire: guarantee hook failed: {e}"),
            WireError::Runtime(s) => write!(f, "git-wire: runsc/bundle error: {s}"),
        }
    }
}

impl std::error::Error for WireError {}

impl From<HookError> for WireError {
    fn from(e: HookError) -> Self {
        WireError::Hook(e)
    }
}

/// Reject a single `(tenant|region|repo)` path segment that could escape the per-tenant/region root.
/// REPLICATES `myelin_git::gix_backend::validate_path_segment` byte-for-byte: empty, `.`, `..`, and any
/// char outside `[A-Za-z0-9._-]` (so separators `/`/`\`, NUL, control chars, and absolute components
/// are all refused). Fail-closed — refuses before any path is built.
pub fn validate_wire_segment(kind: &str, seg: &str) -> Result<(), WireError> {
    if seg.is_empty() {
        return Err(WireError::Path(format!(
            "invalid {kind} path segment: empty (fail-closed — refusing to resolve a path)"
        )));
    }
    if seg == "." || seg == ".." {
        return Err(WireError::Path(format!(
            "invalid {kind} path segment {seg:?}: path-traversal component refused (fail-closed)"
        )));
    }
    for c in seg.chars() {
        let ok = c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');
        if !ok {
            return Err(WireError::Path(format!(
                "invalid {kind} path segment {seg:?}: character {c:?} not in the allowlist \
                 [A-Za-z0-9._-] — separators/NUL/control chars are refused (path-traversal / \
                 absolute-component guard, fail-closed)"
            )));
        }
    }
    Ok(())
}

/// Validate a (possibly namespaced `team/app`) repo slug into its individually-validated `/`-pieces.
/// REPLICATES `myelin_git::gix_backend::validate_repo_slug`: a backslash/NUL slug is refused outright,
/// then each `/`-piece is held to [`validate_wire_segment`] (so `../../x`, `/etc/passwd`, `a//b`, a
/// trailing `/` all yield a `.`/`..`/empty piece and are REFUSED). Never returns an empty piece list.
pub fn validate_wire_repo_slug(repo: &str) -> Result<Vec<String>, WireError> {
    if repo.contains('\\') || repo.contains('\0') {
        return Err(WireError::Path(format!(
            "invalid repo slug {repo:?}: contains a backslash/NUL (path-traversal guard, fail-closed)"
        )));
    }
    let pieces: Vec<String> = repo.split('/').map(|s| s.to_string()).collect();
    for piece in &pieces {
        validate_wire_segment("repo", piece)?;
    }
    Ok(pieces)
}

/// Resolve the on-disk bare-repo path `<root>/<tenant>/<region>/<repo>.git`, FAIL-CLOSED on any
/// traversing/absolute/separator/non-allowlisted segment (the GT-001 cross-tenant isolation boundary).
/// This is the ONLY way a host path reaches a [`WireMount`] — a raw attacker-influenced path can never
/// be mounted. Mirrors `myelin_git::gix_backend::RootedResolver::repo_path`.
pub fn resolve_bare_repo_path(
    root: &Path,
    tenant: &str,
    region: &str,
    repo: &str,
) -> Result<PathBuf, WireError> {
    validate_wire_segment("tenant", tenant)?;
    validate_wire_segment("region", region)?;
    let pieces = validate_wire_repo_slug(repo)?;
    let mut path = root.to_path_buf();
    path.push(tenant);
    path.push(region);
    for piece in &pieces {
        path.push(piece);
    }
    let last = pieces
        .last()
        .expect("validate_wire_repo_slug returns ≥1 piece or errors");
    path.set_file_name(format!("{last}.git"));
    Ok(path)
}

/// **Symlink-path defence-in-depth (CT-006b 4a).** [`resolve_bare_repo_path`] closes the textual
/// path-traversal vector (`..` / separators / absolute / non-allowlist), but a textually-clean path
/// can STILL escape the tenant tree at the FILESYSTEM layer: a `<repo>.git` that is a SYMLINK (or any
/// resolved component that is) would make the RO bind-mount follow OUT of `<root>/<tenant>/<region>`
/// into, e.g., another tenant's tree or `/etc`. This asserts, AFTER resolution and BEFORE any mount,
/// that the resolved repo path is a REAL directory whose canonicalized location stays UNDER the
/// canonicalized `root`. Fail-closed: a symlinked final component, a non-directory, an unstat-able
/// path, or a canonical path that leaves the root is REFUSED (`WireError::Path`).
///
/// THREE complementary checks (defence in depth):
///   - `symlink_metadata` (does NOT follow the FINAL component) ⇒ a `<repo>.git` symlink is caught
///     even when it points back INSIDE the root;
///   - **per-component lstat of every attacker-influenced segment** (CT-006b FU-2) ⇒ a symlinked
///     INTERMEDIATE component (`<tenant>` or `<region>` planted as a symlink) is REFUSED *even when it
///     resolves UNDER the root* — a `canonicalize`+`starts_with` check alone would "launder" such an
///     intermediate symlink (it resolves under root, so `starts_with` passes), yet the bind mount binds
///     the NON-canonical path and would FOLLOW that symlink. lstat-ing each segment closes that gap;
///   - `canonicalize` + `starts_with(canonical_root)` ⇒ a final symlink pointing OUTSIDE (belt-and-
///     braces with the first check) is caught.
///
/// **The check→mount TOCTOU.** These checks run on the host immediately before the OCI bundle is
/// staged + `runsc` is spawned; a sufficiently-privileged local attacker could in principle swap a
/// path component between the check and the gofer's `open` (a classic TOCTOU). It is closed AS FAR AS
/// PRACTICAL here by (a) refusing ANY symlink in the path (so the only swap that helps is creating a
/// brand-new symlink in a window of microseconds — and the path segments are allowlist-validated single
/// names a tenant cannot point cross-tenant), and (b) the repo is bound READ-ONLY (a follow cannot
/// WRITE the victim). A fully race-free guarantee needs `openat2(RESOLVE_NO_SYMLINKS|RESOLVE_BENEATH)` +
/// passing the resolved fd to the runtime, which the OCI/runsc bundle API does not yet expose — noted
/// as the residual; the bind stays RO and non-root so a successful race still cannot mutate the tree.
pub fn assert_repo_under_root(root: &Path, repo_host_path: &Path) -> Result<(), WireError> {
    let meta = std::fs::symlink_metadata(repo_host_path).map_err(|e| {
        WireError::Path(format!(
            "repo path {repo_host_path:?} is not present/stat-able ({e}) — refused before mount \
             (fail-closed)"
        ))
    })?;
    if meta.file_type().is_symlink() {
        return Err(WireError::Path(format!(
            "repo path {repo_host_path:?} is a SYMLINK — refused before mount (a symlinked \
             `<repo>.git` could make the bind-mount follow OUT of the tenant tree; defence in depth)"
        )));
    }
    if !meta.is_dir() {
        return Err(WireError::Path(format!(
            "repo path {repo_host_path:?} is not a directory — refused before mount (fail-closed)"
        )));
    }
    let canon_root = std::fs::canonicalize(root).map_err(|e| {
        WireError::Path(format!(
            "git root {root:?} could not be canonicalized ({e}) — refused before mount (fail-closed)"
        ))
    })?;
    // FU-2: lstat EVERY attacker-influenced segment below the root (`<tenant>/<region>/<repo>.git`).
    // A symlink at ANY of them is refused — even one that resolves UNDER the root — because the bind
    // mount follows the non-canonical path. The segments are the suffix of `repo_host_path` past `root`.
    let rel = repo_host_path.strip_prefix(root).map_err(|_| {
        WireError::Path(format!(
            "repo path {repo_host_path:?} is not under the configured git root {root:?} — refused \
             before mount (fail-closed)"
        ))
    })?;
    let mut cur = canon_root.clone();
    for comp in rel.components() {
        cur = cur.join(comp.as_os_str());
        let m = std::fs::symlink_metadata(&cur).map_err(|e| {
            WireError::Path(format!(
                "repo path component {cur:?} is not present/stat-able ({e}) — refused before mount"
            ))
        })?;
        if m.file_type().is_symlink() {
            return Err(WireError::Path(format!(
                "repo path component {cur:?} is a SYMLINK — refused before mount (an intermediate \
                 symlink, even one resolving UNDER the root, is a bind-mount-follow vector; FU-2)"
            )));
        }
    }
    let canon_repo = std::fs::canonicalize(repo_host_path).map_err(|e| {
        WireError::Path(format!(
            "repo path {repo_host_path:?} could not be canonicalized ({e}) — refused before mount \
             (fail-closed)"
        ))
    })?;
    if !canon_repo.starts_with(&canon_root) {
        return Err(WireError::Path(format!(
            "resolved repo path {canon_repo:?} escapes the canonical git root {canon_root:?} (a \
             symlinked component would leave the tenant tree) — refused before mount (fail-closed)"
        )));
    }
    Ok(())
}

/// **A sandboxed git-wire invocation** — the git-shaped analogue of a [`JobSpec`]. It carries a
/// RESOLVER-VALIDATED bare-repo host path (RO-mounted at [`WIRE_REPO_MOUNT`]), the canonical `git` argv
/// (WITHOUT the repo path — the launch appends `/repo`), the bounded stdin request body, optional extra
/// env, an optional WRITABLE quarantine host dir (bound at [`WIRE_QUARANTINE_MOUNT`]), and the limits +
/// four-guarantee tokens. Build it with [`GitWireSpec::for_repo`], which performs the path confinement.
#[derive(Clone, Debug)]
pub struct GitWireSpec {
    repo_host_path: PathBuf,
    /// The on-disk root the locator resolved UNDER — retained so the launch can assert (defence in
    /// depth, CT-006b) that the resolved repo path stays a REAL directory beneath the canonicalized
    /// root before it is bind-mounted (a symlinked `<repo>.git` / component cannot escape the tree).
    root: PathBuf,
    git_argv: Vec<String>,
    stdin: Vec<u8>,
    env: Vec<String>,
    quarantine_host_path: Option<PathBuf>,
    limits: ResourceLimits,
    run_token: RunTokenRef,
    meter_to: MeterTarget,
    idem_token: IdemToken,
}

impl GitWireSpec {
    /// Build a git-wire spec for `(root, tenant, region, repo)`, RESOLVING + VALIDATING the bare-repo
    /// path through [`resolve_bare_repo_path`] (fail-closed on any cross-tenant/`..`/absolute segment —
    /// the locator NEVER reaches a mount raw). `git_argv` is the canonical subcommand + flags WITHOUT
    /// the repo path (e.g. `["upload-pack", "--stateless-rpc", "--advertise-refs"]`); the launch appends
    /// `/repo`. `quarantine_host_path` (if set) is bound WRITABLE at `/quarantine` for receive-pack.
    #[allow(clippy::too_many_arguments)]
    pub fn for_repo(
        root: &Path,
        tenant: &str,
        region: &str,
        repo: &str,
        git_argv: Vec<String>,
        stdin: Vec<u8>,
        env: Vec<String>,
        quarantine_host_path: Option<PathBuf>,
        limits: ResourceLimits,
        run_token: RunTokenRef,
        meter_to: MeterTarget,
        idem_token: IdemToken,
    ) -> Result<GitWireSpec, WireError> {
        let repo_host_path = resolve_bare_repo_path(root, tenant, region, repo)?;
        Ok(GitWireSpec {
            repo_host_path,
            root: root.to_path_buf(),
            git_argv,
            stdin,
            env,
            quarantine_host_path,
            limits,
            run_token,
            meter_to,
            idem_token,
        })
    }

    /// The resolver-validated bare-repo host path that will be RO-mounted at `/repo`.
    pub fn repo_host_path(&self) -> &Path {
        &self.repo_host_path
    }
}

impl GvisorBackend {
    /// **Run a canonical-`git` wire op in the hardened gVisor sandbox (CT-006a).** Drives the SAME
    /// four-guarantee seam as [`launch`](SandboxBackend::launch) (isolation floor → hardening assert →
    /// attribution → reserve → run → settle), with the git-wire additions: the bare repo is bound
    /// READ-ONLY at `/repo`, an optional writable quarantine at `/quarantine`, the bounded request body
    /// is piped to stdin, and the response is captured (bounded). The command is `git <argv> /repo`,
    /// run under the full hardening (ro-root, caps dropped, no-new-privs, seccomp, no-netns, non-root
    /// uid, mem/pids/disk bounded, whole-container kill + cleanup). Fail-closed: an oversize body, an
    /// unmet floor / exhausted wallet, or an absent git rootfs all REFUSE before/instead of a result.
    pub fn launch_git_wire(
        &self,
        spec: &GitWireSpec,
        hooks: &RunnerHooks,
    ) -> Result<SandboxLaunch, WireError> {
        // The guest command: `git <argv> /repo` (the RO repo mount is the final argument).
        let mut command = Vec::with_capacity(spec.git_argv.len() + 2);
        command.push("git".to_string());
        command.extend(spec.git_argv.iter().cloned());
        command.push(WIRE_REPO_MOUNT.to_string());
        self.launch_git_command(spec, hooks, command, &NEVER_CANCELLED)
    }

    /// Git-wire launch with cooperative process-shutdown cancellation. Once `cancellation` becomes
    /// true, the wait loop kills container PID 1 and reaps `runsc` exactly like a wall timeout, but
    /// does not misreport the operator-requested cancellation as a timeout.
    pub fn launch_git_wire_until_cancelled(
        &self,
        spec: &GitWireSpec,
        hooks: &RunnerHooks,
        cancellation: &AtomicBool,
    ) -> Result<SandboxLaunch, WireError> {
        let mut command = Vec::with_capacity(spec.git_argv.len() + 2);
        command.push("git".to_string());
        command.extend(spec.git_argv.iter().cloned());
        command.push(WIRE_REPO_MOUNT.to_string());
        self.launch_git_command(spec, hooks, command, cancellation)
    }

    /// **Ingest a pushed packfile in the hardened sandbox (CT-006d — the push write path's untrusted-pack
    /// intake).** The pushed pack is piped to stdin; the guest runs [`RECEIVE_PACK_INGEST_SCRIPT`]
    /// (`git index-pack --fix-thin` into a writable `/tmp` tmpfs quarantine, NEVER the RO `/repo`) and
    /// streams the VALIDATED, self-contained quarantine pack back on stdout. The host then stages that
    /// pack in a throwaway odb, runs the in-process policy + fsck, and migrates it under the one-tx
    /// ref-CAS — the in-sandbox `git` ONLY ingests bytes, it never moves a ref or touches the real repo.
    /// Same four-guarantee seam + hardening floor + RO `/repo` mount as [`Self::launch_git_wire`].
    pub fn launch_git_receive_pack(
        &self,
        spec: &GitWireSpec,
        hooks: &RunnerHooks,
    ) -> Result<SandboxLaunch, WireError> {
        // The guest entrypoint is busybox `sh` running the FIXED ingest script — no `/repo` is appended
        // (the script references the RO `/repo` mount itself, for the thin-pack alternates), and no
        // untrusted data is interpolated (the pack arrives only on stdin).
        let command = vec![
            "sh".to_string(),
            "-c".to_string(),
            RECEIVE_PACK_INGEST_SCRIPT.to_string(),
        ];
        self.launch_git_command(spec, hooks, command, &NEVER_CANCELLED)
    }

    /// Receive-pack ingest with the same cooperative shutdown cancellation as wire serving.
    pub fn launch_git_receive_pack_until_cancelled(
        &self,
        spec: &GitWireSpec,
        hooks: &RunnerHooks,
        cancellation: &AtomicBool,
    ) -> Result<SandboxLaunch, WireError> {
        let command = vec![
            "sh".to_string(),
            "-c".to_string(),
            RECEIVE_PACK_INGEST_SCRIPT.to_string(),
        ];
        self.launch_git_command(spec, hooks, command, cancellation)
    }

    /// The shared git-wire launch body (CT-006a/d): bound stdin, symlink-confine the repo, build the
    /// hardened internal `JobSpec`, drive the four-guarantee seam, mount the RO repo (+ optional writable
    /// quarantine bind), run the container streaming the bounded request body to stdin + capturing the
    /// bounded response from stdout. `command` is the fully-built guest argv (`git <argv> /repo` for the
    /// wire serve, or the `sh -c <ingest>` for receive-pack) — the ONLY thing that differs between callers.
    fn launch_git_command(
        &self,
        spec: &GitWireSpec,
        hooks: &RunnerHooks,
        command: Vec<String>,
        cancellation: &AtomicBool,
    ) -> Result<SandboxLaunch, WireError> {
        if cancellation.load(Ordering::Acquire) {
            return Err(WireError::Runtime(
                "Git wire launch cancelled by process shutdown".into(),
            ));
        }
        // Bound the request body BEFORE anything spawns (a client cannot force unbounded host buffering).
        if spec.stdin.len() > WIRE_STDIN_BOUND {
            return Err(WireError::StdinTooLarge {
                len: spec.stdin.len(),
                cap: WIRE_STDIN_BOUND,
            });
        }
        // Symlink-path defence in depth (CT-006b 4a): the resolved repo MUST be a real directory under
        // the canonicalized root before it is bind-mounted (a symlinked `<repo>.git`/component cannot
        // make the RO mount follow out of the tenant tree). REFUSED here, before any container spawns.
        assert_repo_under_root(&spec.root, &spec.repo_host_path)?;
        // Build the internal hardened JobSpec so the git wire inherits the SAME profile + guarantees as
        // every CI/agent job (the image is a placeholder — gVisor runs the staged rootfs, not an image).
        let image = ImageRef::pinned(
            "sandbox/git-wire@sha256:0000000000000000000000000000000000000000000000000000000000000000",
        )
        .map_err(|e| WireError::Runtime(e.to_string()))?;
        let job = JobSpec::new(
            JobKind::Ci,
            image,
            command,
            vec![],
            vec![],
            EgressPolicy::deny_all(), // a serve needs no egress (egress-deny / no-netns).
            spec.limits,
            WorkspaceSpec::default(),
            TrustTier::Trusted,
            spec.run_token.clone(),
            spec.meter_to.clone(),
            spec.idem_token.clone(),
        )
        .map_err(|e| WireError::Runtime(e.to_string()))?;

        // The four-guarantee seam, in the mandated order (identical to `launch_with`).
        (hooks.isolation_floor)(&job)?;
        let profile = HardeningProfile::derive(&job);
        profile.assert_enforced().map_err(WireError::Hardening)?;
        (hooks.attribute)(&job.run_token)?;
        let reserve = (hooks.reserve)(&job.meter_to)?;

        // The git-wire mounts: the RO bare repo, then (if requested) the writable quarantine.
        let mut mounts = vec![WireMount {
            host_source: spec.repo_host_path.clone(),
            guest_dest: WIRE_REPO_MOUNT.to_string(),
            readonly: true,
        }];
        if let Some(q) = &spec.quarantine_host_path {
            mounts.push(WireMount {
                host_source: q.clone(),
                guest_dest: WIRE_QUARANTINE_MOUNT.to_string(),
                readonly: false,
            });
        }
        // The staged rootfs is referenced by an ABSOLUTE `root.path` (no symlink — required alongside
        // the host bind mounts). Canonicalize so the path is absolute; an absent rootfs is caught
        // (fail-closed) in `run_git_wire_container`.
        let rootfs = resolved_gvisor_git_rootfs();
        let root_abs = std::fs::canonicalize(&rootfs).unwrap_or_else(|_| rootfs.clone());
        let cfg = OciConfig::from_spec(&job, &profile)
            .with_extra_env(spec.env.clone())
            .with_extra_mounts(mounts)
            .with_root_path(root_abs);

        let (
            ContainerRun {
                child,
                bundle_dir,
                result,
            },
            stdout_truncated,
        ) = run_git_wire_container(&job, &cfg, spec.stdin.clone(), &rootfs, cancellation)
            .map_err(WireError::Runtime)?;

        // FAIL LOUD at the seam (CT-006c FU-1): if the response overflowed the generous wire cap, the
        // captured pack is TRUNCATED — refuse rather than hand back a short pack the client's
        // `index-pack` would reject with "early EOF". Tear down the just-run container's bundle (the
        // container itself is already deleted by `run_git_wire_container`).
        if stdout_truncated {
            let _ = std::fs::remove_dir_all(&bundle_dir);
            return Err(WireError::OutputTooLarge {
                cap: job.limits.disk_bytes as usize,
            });
        }

        let guest_id = format!("runsc-gitwire-{}", job.idem_token.0);
        self.live
            .lock()
            .unwrap()
            .insert(guest_id.clone(), RunscProc { child, bundle_dir });

        // Settle against the REAL measured usage (never interrupt in-flight).
        (hooks.settle)(&reserve, result.usage)?;

        Ok(SandboxLaunch {
            handle: SandboxHandle { guest_id },
            result,
        })
    }
}

/// The git-wire production run path — mirrors [`run_production_container`] but stages the GIT-bearing
/// rootfs ([`resolved_gvisor_git_rootfs`]) and pipes the bounded request body to the container's stdin.
/// The bind mounts (RO repo + optional writable quarantine) are already in `cfg`'s `mounts` array. The
/// container + bundle are cleaned up on EVERY path (no leaks); an absent git rootfs fails closed.
fn run_git_wire_container(
    job: &JobSpec,
    cfg: &OciConfig,
    stdin: Vec<u8>,
    rootfs: &Path,
    cancellation: &AtomicBool,
) -> Result<(ContainerRun, bool), String> {
    let bin = runsc_bin();
    if !rootfs.exists() {
        return Err(format!(
            "staged gVisor git rootfs absent: {} (the git wire REQUIRES a real `git` in the guest — \
             stage a git-bearing rootfs and point {ENV_GVISOR_GIT_ROOTFS} at it; see \
             tests/git_wire_prod_exec_test.rs)",
            rootfs.display()
        ));
    }
    // Stage a config-only bundle: `cfg`'s `root.path` is the ABSOLUTE staged rootfs (set by
    // `launch_git_wire`), so no `rootfs` symlink is staged (a symlinked root.path + a host bind mount
    // makes the rootless gofer fail to start the sandbox; an absolute root.path + bind mount works).
    let bundle_dir = stage_git_wire_bundle(cfg)?;
    let container_id = format!("myelin-gitwire-{}-{}", std::process::id(), unique_suffix());

    let timeout = Duration::from_secs(job.limits.timeout_secs as u64);
    // The git-wire response (the packfile / advertisement) is STREAMED to a host temp file under a
    // GENEROUS cap derived from the job's `disk_bytes` scratch quota (configurable; default
    // [`WIRE_STDOUT_BOUND`] = 512 MiB) — NOT the 256 KiB CI/agent log bound — so a real-size pack comes
    // through whole while host RAM stays bounded to one chunk. Over the cap ⇒ `outcome.stdout_truncated`,
    // which the caller turns into a LOUD [`WireError::OutputTooLarge`] (never a silently-short pack).
    let wire_cap = job.limits.disk_bytes as usize;
    let outcome = match run_and_capture(
        &bin,
        &bundle_dir,
        &container_id,
        timeout,
        job.limits.mem_bytes,
        RunCaptureOptions {
            stdin: Some(stdin),
            stdout_mode: StdoutMode::StreamToFile { bound: wire_cap },
            cancellation,
        },
    ) {
        Ok(o) => o,
        Err(e) => {
            delete_container(&bin, &container_id);
            let _ = std::fs::remove_dir_all(&bundle_dir);
            return Err(e);
        }
    };
    delete_container(&bin, &container_id);

    let stdout_truncated = outcome.stdout_truncated;
    // The git-wire path's stdout is the git smart-transport packfile/protocol stream (StreamToFile),
    // NOT job LOG output — it never reaches the durable log pipeline, and masking arbitrary bytes in a
    // binary packfile would corrupt it. So this path NEVER redacts (an explicit `none()`, not
    // `for_job`) — a deliberate distinction that matters the day CI-1 injection makes `for_job`
    // non-empty. Boundary redaction is a LOG-path concern (the CappedHead capture in
    // `run_production_container`), not a transport-path concern.
    let result = build_result(job, &outcome, &RedactionPlan::none());
    Ok((
        ContainerRun {
            child: Box::new(SpawnedRunsc { bin, container_id }),
            bundle_dir,
            result,
        },
        stdout_truncated,
    ))
}

/// Stage a CONFIG-ONLY OCI bundle (just `config.json`) for the git wire — the rootfs is referenced by
/// the config's ABSOLUTE `root.path`, so no `rootfs` symlink is staged. Returns the bundle dir (removed
/// on teardown).
fn stage_git_wire_bundle(cfg: &OciConfig) -> Result<PathBuf, String> {
    let bundle = std::env::temp_dir().join(format!(
        "myelin-gitwire-bundle-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let _ = std::fs::remove_dir_all(&bundle);
    std::fs::create_dir_all(&bundle).map_err(|e| format!("create bundle dir {bundle:?}: {e}"))?;
    std::fs::write(bundle.join("config.json"), cfg.to_json())
        .map_err(|e| format!("write config.json: {e}"))?;
    Ok(bundle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ReserveHandle;

    fn spec(allow: Vec<String>) -> JobSpec {
        JobSpec::new(
            JobKind::Agent,
            ImageRef::pinned("r/img@sha256:abc123def4567890abc123def4567890abc123def4567890abc123def4567890").unwrap(),
            vec!["python3".into(), "-c".into(), "print(1)".into()],
            vec![],
            vec![],
            EgressPolicy { allow },
            ResourceLimits {
                cpu_millis: 1000,
                mem_bytes: 256 << 20,
                disk_bytes: 1 << 30,
                pids_max: 64,
                timeout_secs: 120,
            },
            WorkspaceSpec::default(),
            TrustTier::UntrustedFork,
            RunTokenRef { jti: "j".into() },
            MeterTarget {
                reserve_id: "r".into(),
            },
            IdemToken("idem-runsc-1".into()),
        )
        .unwrap()
    }

    fn ok_hooks() -> RunnerHooks {
        RunnerHooks {
            reserve: Box::new(|m| Ok(ReserveHandle(m.reserve_id.clone()))),
            settle: Box::new(|_h, _u| Ok(())),
            attribute: Box::new(|_t| Ok(())),
            isolation_floor: Box::new(|_s| Ok(())),
        }
    }

    fn outcome(stdout: &[u8], stderr: &[u8]) -> RunscOutcome {
        RunscOutcome {
            exit: Some(0),
            timed_out: false,
            stdout: stdout.to_vec(),
            stdout_truncated: false,
            stderr: stderr.to_vec(),
            wall: Duration::from_secs(1),
            cpu_seconds: Some(1),
        }
    }

    // CT-004f sub-step 1: `build_result` APPLIES the redaction plan to both captured streams — the
    // boundary seam is wired, not just the `RedactionPlan` unit. A populated plan (the shape CI-1
    // injection will pass) masks the needle before it reaches `SandboxResult`.
    #[test]
    fn build_result_masks_needles_in_both_streams() {
        let s = spec(vec![]);
        let plan = RedactionPlan::for_needles([b"AKIAsecret".to_vec()]);
        let o = outcome(b"deploying with AKIAsecret now", b"error: AKIAsecret invalid");
        let res = build_result(&s, &o, &plan);
        assert_eq!(res.stdout, b"deploying with *** now".to_vec());
        assert_eq!(res.stderr, b"error: *** invalid".to_vec());
    }

    // The empty plan (the ONLY state reachable today — nothing injects secrets) is a pass-through:
    // captured output is byte-unchanged.
    #[test]
    fn build_result_empty_plan_is_byte_identity() {
        let s = spec(vec![]);
        let o = outcome(b"ordinary build log line", b"warning: deprecated");
        let res = build_result(&s, &o, &RedactionPlan::none());
        assert_eq!(res.stdout, b"ordinary build log line".to_vec());
        assert_eq!(res.stderr, b"warning: deprecated".to_vec());
    }

    struct FakeRunsc;
    impl RunscChild for FakeRunsc {
        fn kill(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn wait(&mut self) -> Result<i32, String> {
            Ok(0)
        }
    }

    /// A canned [`ContainerRun`] for the fake path (no real `runsc`): a clean exit-0 result + a fake
    /// child + a non-existent bundle dir (its removal on teardown is a harmless no-op).
    fn fake_run() -> ContainerRun {
        ContainerRun {
            child: Box::new(FakeRunsc),
            bundle_dir: std::env::temp_dir().join("myelin-gvisor-fake-bundle-does-not-exist"),
            result: SandboxResult::stub_ok(ResourceUsage {
                cpu_seconds: 1,
                mem_byte_seconds: 1,
            }),
        }
    }

    /// CT-006c (the streaming fix): the git-wire stdout drain stages straight to a host temp file under
    /// a generous cap with host memory bounded to one chunk. A response WITHIN the cap comes through
    /// WHOLE (no 256 KiB truncation); a response OVER the cap is head-bounded AND flagged `truncated`
    /// (which the wire seam turns into a LOUD `WireError::OutputTooLarge` — never a silent short pack).
    #[test]
    fn drain_to_temp_file_streams_whole_under_cap_and_flags_over_cap() {
        // A 1 MiB stream (FAR past the 256 KiB SANDBOX_CAPTURE_BOUND) under a 4 MiB cap → WHOLE, untruncated.
        let big = vec![0xABu8; 1024 * 1024];
        let (out, truncated) = drain_to_temp_file(&big[..], 4 * 1024 * 1024);
        assert_eq!(out.len(), big.len(), "a real-size pack under the cap comes through WHOLE");
        assert_eq!(out, big, "the bytes are byte-identical (no corruption via the temp file)");
        assert!(!truncated, "within the cap ⇒ not truncated");

        // The SAME stream under a 64 KiB cap → head-bounded to the cap AND flagged truncated (fail-loud).
        let (head, over) = drain_to_temp_file(&big[..], 64 * 1024);
        assert_eq!(head.len(), 64 * 1024, "over the cap ⇒ exactly the cap bytes are kept");
        assert!(over, "over the cap ⇒ truncated flag set (the wire seam then refuses loudly)");
    }

    #[test]
    fn oci_config_enforces_the_backend_independent_hardening() {
        let cfg = GvisorBackend::oci_config(&spec(vec![])).unwrap();
        assert!(cfg.root_readonly());
        assert!(!cfg.has_network(), "no allowlist ⇒ no network interface");
        let json = cfg.to_json();
        assert!(json.contains("\"readonly\": true"));
        assert!(json.contains("\"noNewPrivileges\": true"));
        assert!(
            json.contains("SCMP_ACT_ERRNO"),
            "a seccomp profile is attached"
        );
        assert!(
            json.contains("\"bounding\": []"),
            "all capabilities dropped"
        );
        // CT-002b: the untrusted process runs NON-ROOT (defense in depth — never uid 0 in the
        // sandbox) and the config is RUNNABLE (`cwd` set, else `runsc run` rejects the spec).
        assert!(
            json.contains("\"uid\": 65534") && json.contains("\"gid\": 65534"),
            "the untrusted process must run as a non-root uid/gid (65534)"
        );
        assert!(
            json.contains("\"cwd\": \"/\""),
            "process.cwd must be set or the OCI runtime rejects the spec"
        );
        // CT-003a/CT-003b (SI-017): the OCI emits an (advisory — rootless runsc ignores it) memory
        // ceiling from spec.limits.mem_bytes; the REAL host-RAM bound is the out-of-band MemoryCgroup
        // the production run path places the runsc tree into (see MemoryCgroup). It also mounts a
        // SIZE-BOUNDED writable `/tmp` tmpfs (sized from the scratch quota) so a disk fill hits
        // ENOSPC instead of an unbounded host-RAM-backed tmpfs. spec()'s limits are mem=256 MiB,
        // disk=1 GiB.
        assert!(
            json.contains(&format!("\"limit\": {}", 256u64 << 20)),
            "the OCI config must carry the memory ceiling (linux.resources.memory.limit) from spec.limits.mem_bytes"
        );
        assert!(
            json.contains("\"destination\": \"/tmp\"") && json.contains("\"type\": \"tmpfs\""),
            "a size-bounded writable /tmp tmpfs must be mounted (no unbounded host-RAM-backed scratch)"
        );
        assert!(
            json.contains(&format!("size={}", 1u64 << 30)) && json.contains("mode=1777"),
            "the /tmp tmpfs must be sized from spec.limits.disk_bytes and writable by the non-root payload"
        );
    }

    #[test]
    fn gvisor_launch_drives_four_guarantees_on_the_same_trait() {
        // The SAME SandboxBackend trait + the SAME hardening — the named-second backend.
        let backend = GvisorBackend::new();
        let launch = backend
            .launch_with(&spec(vec![]), &ok_hooks(), |_spec, _cfg| Ok(fake_run()))
            .unwrap();
        assert_eq!(launch.handle.guest_id, "runsc-idem-runsc-1");
        // The reshaped seam carries the command result back (CT-001 stub).
        assert_eq!(launch.result.exit_code, Some(0));
        assert!(launch.result.passed());
        backend.kill(&launch.handle).unwrap();
    }

    #[test]
    fn gvisor_corpus_script_carries_every_catalogued_attack_and_the_posture() {
        // The gVisor corpus probes the SAME catalogued attack ids the host-side parser keys on, so
        // the gate predicate is one path across backends. A drift here would let a family silently
        // not run on the gVisor backend.
        let script = build_gvisor_corpus_script(64);
        for atk in crate::escape_corpus::CORPUS {
            assert!(
                script.contains(atk.id),
                "catalogued attack `{}` is missing from the gVisor corpus script",
                atk.id
            );
        }
        assert!(script.contains(crate::escape_corpus::BEGIN_MARKER));
        assert!(script.contains(crate::escape_corpus::END_MARKER));
        // The raw-device family is probed as mknod-denial (the faithful gVisor expression of the
        // contained property — the node is absent and creating one is denied).
        assert!(script.contains("mknod /dev/mem"));
        assert!(script.contains("mknod /dev/port"));
        // The fork-bomb ceiling is carried from the arg.
        assert!(script.contains("ceiling=64"));
        // CT-003b: the anon-memory hog's ATTEMPT sentinel + the END marker precede the oversized
        // alloc, so the corpus COMPLETES even when the contained hog OOM-kills the whole sentry
        // mid-alloc (the host cgroup bounds host RAM). The ESCAPED line follows only if it HELD.
        let attempt = script
            .find(&format!("{} ATTEMPT", crate::escape_corpus::MEMHOG_ID))
            .expect("memhog ATTEMPT sentinel in the gVisor corpus");
        let end = script.find(crate::escape_corpus::END_MARKER).unwrap();
        assert!(attempt < end, "the memhog ATTEMPT sentinel must precede the END marker");
        // Pure-shell doubling allocator (holds the anon memory in the sh process itself; ~1 GiB) —
        // the host cgroup OOM-kills the sentry when it breaches memory.max, never a false held=0.
        assert!(script.contains(r#"S="$S$S""#) && script.contains("while [ $n -lt 26 ]"));
    }

    #[test]
    fn memory_cgroup_round_trips_or_fails_closed() {
        // On a host with cgroup v2 + a delegated `memory` controller, create() establishes a real
        // child cgroup with memory.max set, places nothing (we only assert the knobs), and cleans up
        // (no leaked cgroup dir). On a host WITHOUT it, create() MUST fail closed (Err) — never a
        // silently-unbounded cgroup. Either branch is a valid host posture; both are asserted honest.
        match MemoryCgroup::create(64 << 20) {
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

    #[test]
    fn gvisor_drill_config_expresses_the_mandatory_posture() {
        let json = gvisor_drill_config_json(&spec(vec![]), GVISOR_CORPUS_SCRIPT).unwrap();
        // Read-only root, no-new-privs, all caps dropped, the pids ceiling — the SAME mandatory
        // profile the Firecracker backend enforces, expressed through the OCI spec gVisor consumes.
        assert!(json.contains("\"readonly\": true"));
        assert!(json.contains("\"noNewPrivileges\": true"));
        assert!(json.contains("\"bounding\": []"));
        assert!(json.contains("\"limit\": 64"));
        // NO network namespace ⇒ with --network=none only loopback exists (egress closed).
        assert!(
            !json.contains("\"type\": \"network\""),
            "no network namespace ⇒ egress closed (--network=none leaves only loopback)"
        );
        // The rootless deviation: NO user namespace (runsc --rootless adds its own).
        assert!(
            !json.contains("\"type\": \"user\""),
            "the rootless gofer fork fails with a doubly-declared user namespace"
        );
        // The entrypoint runs the corpus script.
        assert!(json.contains(GVISOR_CORPUS_SCRIPT));
    }

    #[test]
    fn gvisor_refuses_to_start_on_exhaustion() {
        let backend = GvisorBackend::new();
        let hooks = RunnerHooks {
            reserve: Box::new(|_m| Err(crate::HookError("exhausted".into()))),
            settle: Box::new(|_h, _u| Ok(())),
            attribute: Box::new(|_t| Ok(())),
            isolation_floor: Box::new(|_s| Ok(())),
        };
        let r = backend.launch_with(&spec(vec![]), &hooks, |_spec, _cfg| Ok(fake_run()));
        assert!(matches!(r, Err(GvisorError::Hook(_))));
    }

    #[test]
    fn cancelled_git_wire_refuses_before_reserve_or_spawn() {
        let cancelled = AtomicBool::new(true);
        let spec = GitWireSpec {
            repo_host_path: PathBuf::from("/absent/repo.git"),
            root: PathBuf::from("/absent"),
            git_argv: vec!["upload-pack".into()],
            stdin: Vec::new(),
            env: Vec::new(),
            quarantine_host_path: None,
            limits: ResourceLimits {
                cpu_millis: 1,
                mem_bytes: 1,
                disk_bytes: 1,
                pids_max: 1,
                timeout_secs: 1,
            },
            run_token: RunTokenRef { jti: "cancel".into() },
            meter_to: MeterTarget { reserve_id: "cancel".into() },
            idem_token: IdemToken("cancel".into()),
        };
        let result = GvisorBackend::new().launch_git_wire_until_cancelled(
            &spec,
            &ok_hooks(),
            &cancelled,
        );
        assert!(
            matches!(result, Err(WireError::Runtime(message)) if message.contains("cancelled by process shutdown"))
        );
    }

    /// **CT-006b 4a — symlink-path defence in depth (no runsc needed).** A textually-clean repo
    /// locator whose resolved `<repo>.git` is a SYMLINK out of the tenant tree is REFUSED by
    /// [`assert_repo_under_root`] BEFORE any mount, while a REAL directory under the root is admitted.
    #[test]
    fn symlinked_repo_path_is_refused_before_mount() {
        let tmp = std::env::temp_dir().join(format!(
            "myelin-gitwire-symlink-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let root = tmp.join("git-root");
        let outside = tmp.join("outside-the-tree");
        std::fs::create_dir_all(root.join("acme").join("fr-par")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();

        // (1) A REAL bare-repo directory under the root is admitted.
        let real = resolve_bare_repo_path(&root, "acme", "fr-par", "widgets").unwrap();
        std::fs::create_dir_all(&real).unwrap();
        assert!(
            assert_repo_under_root(&root, &real).is_ok(),
            "a real directory under the root must be admitted"
        );

        // (2) A SYMLINKED `<repo>.git` pointing OUT of the tenant tree is refused (final-symlink check
        //     AND the canonical-escape check would both catch it).
        let evil = resolve_bare_repo_path(&root, "acme", "fr-par", "evil").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &evil).unwrap();
        let r = assert_repo_under_root(&root, &evil);
        assert!(
            matches!(r, Err(WireError::Path(_))),
            "a symlinked repo path escaping the tree must be refused, got {r:?}"
        );

        // (3) A symlinked INTERMEDIATE component (the tenant dir → /tmp) is caught by the canonical
        //     starts_with check even though the final `<repo>.git` is a real dir under the symlink.
        let root2 = tmp.join("git-root-2");
        std::fs::create_dir_all(&root2).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, root2.join("acme")).unwrap();
        let leak_parent = outside.join("fr-par");
        std::fs::create_dir_all(leak_parent.join("widgets.git")).unwrap();
        let via_symlinked_component =
            resolve_bare_repo_path(&root2, "acme", "fr-par", "widgets").unwrap();
        let r2 = assert_repo_under_root(&root2, &via_symlinked_component);
        assert!(
            matches!(r2, Err(WireError::Path(_))),
            "a symlinked intermediate component leaving the root must be refused, got {r2:?}"
        );

        // (4) An absent repo path fails closed (never a silent admit).
        let absent = resolve_bare_repo_path(&root, "acme", "fr-par", "ghost").unwrap();
        assert!(matches!(
            assert_repo_under_root(&root, &absent),
            Err(WireError::Path(_))
        ));

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
