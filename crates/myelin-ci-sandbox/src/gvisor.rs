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
use crate::{
    drain_capped, JobSpec, ResourceUsage, RunnerHooks, SandboxBackend, SandboxHandle,
    SandboxLaunch, SandboxResult, SANDBOX_CAPTURE_BOUND,
};
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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
        }
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
        format!(
            "{{\n  \"ociVersion\": \"1.0.0\",\n  \"process\": {{\n    \
             \"user\": {{ \"uid\": {uid}, \"gid\": {gid} }},\n    \
             \"args\": [{args}],\n    \"cwd\": \"/\",\n    \
             \"env\": [\"PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin\"],\n    \
             \"noNewPrivileges\": {nnp},\n    \
             \"capabilities\": {{ \"bounding\": [], \"effective\": [], \"permitted\": [] }}\n  }},\n  \
             \"root\": {{ \"path\": \"rootfs\", \"readonly\": {ro} }},\n  \
             \"mounts\": [ {{ \"destination\": \"/tmp\", \"type\": \"tmpfs\", \"source\": \"tmpfs\", \
             \"options\": [\"nosuid\", \"nodev\", \"mode=1777\", \"size={disk}\"] }} ],\n  \
             \"linux\": {{\n    \"resources\": {{ \"memory\": {{ \"limit\": {mem} }}, \
             \"pids\": {{ \"limit\": {pids} }} }},\n    \
             \"seccomp\": {{ \"defaultAction\": \"SCMP_ACT_ERRNO\" }},\n    \
             \"namespaces\": [ {net_ns} ]\n  }}\n}}",
            uid = UNTRUSTED_UID,
            gid = UNTRUSTED_GID,
            args = args,
            nnp = self.no_new_privileges,
            ro = self.root_readonly,
            disk = self.disk_bytes,
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
            .map_err(|e| format!("read /proc/self/cgroup: {e}"))?;
        let rel = content
            .lines()
            .find_map(|l| l.strip_prefix("0::"))
            .map(str::trim)
            .ok_or_else(|| {
                "no cgroup v2 unified hierarchy (`0::` line absent) — cannot establish a memory \
                 cgroup; refusing to run the gVisor workload unbounded (SI-017 fail-closed)"
                    .to_string()
            })?;
        let our_dir = PathBuf::from(ROOT).join(rel.trim_start_matches('/'));
        // The `memory` controller must be delegated to our own cgroup (⇒ available to siblings).
        let controllers =
            std::fs::read_to_string(our_dir.join("cgroup.controllers")).unwrap_or_default();
        if !controllers.split_whitespace().any(|c| c == "memory") {
            return Err(format!(
                "the `memory` cgroup controller is NOT delegated to {our_dir:?} (controllers: \
                 {controllers:?}) — cannot bound gVisor memory; refusing to run the workload \
                 unbounded (SI-017 fail-closed)"
            ));
        }
        let parent = our_dir.parent().ok_or_else(|| {
            "this process's cgroup has no parent — cannot create a sibling memory cgroup".to_string()
        })?;
        let dir = parent.join(format!(
            "myelin-mem-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let _ = std::fs::remove_dir(&dir);
        std::fs::create_dir(&dir).map_err(|e| format!("create memory cgroup {dir:?}: {e}"))?;
        // The sibling must actually have the `memory` controller (the parent delegated it). If not,
        // tear down and fail closed rather than run a workload an empty cgroup would not bound.
        let cg_controllers =
            std::fs::read_to_string(dir.join("cgroup.controllers")).unwrap_or_default();
        if !cg_controllers.split_whitespace().any(|c| c == "memory") {
            let _ = std::fs::remove_dir(&dir);
            return Err(format!(
                "the created cgroup {dir:?} has no `memory` controller (parent did not delegate it) \
                 — refusing to run the gVisor workload unbounded (SI-017 fail-closed)"
            ));
        }
        // The HARD host-RAM bound + close the swap escape hatch (so a hog OOMs rather than swaps).
        if let Err(e) = std::fs::write(dir.join("memory.max"), mem_bytes.to_string()) {
            let _ = std::fs::remove_dir(&dir);
            return Err(format!("write memory.max={mem_bytes} to {dir:?}: {e}"));
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
/// 1b. CT-003b (SI-017): establish an OUT-OF-BAND [`MemoryCgroup`] capped at `spec.limits.mem_bytes`
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

    let result = build_result(spec, &outcome);
    Ok(ContainerRun {
        child: Box::new(SpawnedRunsc { bin, container_id }),
        bundle_dir,
        result,
    })
}

/// A cheap monotonic-ish suffix to avoid bundle/container-id collisions within a process.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
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
    /// The container's REAL piped stdout (the runtime's fd, not in-container framing).
    stdout: Vec<u8>,
    /// The container's REAL piped stderr.
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
) -> Result<RunscOutcome, String> {
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
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Place the runsc child (and the sentry/gofer tree it forks) into the memory cgroup at birth.
    cgroup
        .place_child(&mut cmd)
        .map_err(|e| format!("bind runsc into the memory cgroup: {e}"))?;
    let mut child = cmd.spawn().map_err(|e| format!("spawn runsc: {e}"))?;

    let pid = child.id();
    let start = Instant::now();

    // Drain both pipes on threads so a chatty container cannot fill a pipe buffer and deadlock.
    let mut out = child.stdout.take().expect("piped stdout");
    let mut err = child.stderr.take().expect("piped stderr");
    // CT-002c: cap each stream at SANDBOX_CAPTURE_BOUND (head capture) and DISCARD the rest to EOF —
    // bounds host memory under a runaway container while still draining the pipe so the container
    // never blocks on a full pipe (no deadlock that would defeat the timeout).
    let th_out = std::thread::spawn(move || drain_capped(&mut out, SANDBOX_CAPTURE_BOUND).0);
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
        if start.elapsed() >= timeout {
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
            timed_out = true;
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let wall = start.elapsed();

    // The child has exited/been-killed ⇒ the pipes hit EOF ⇒ the drain threads finish.
    let stdout = th_out.join().unwrap_or_default();
    let stderr = th_err.join().unwrap_or_default();

    // The container + its sentry/gofer tree are gone; reap the cgroup (kill any straggler, rmdir).
    // `Drop` is the backstop, but tear it down deterministically here on the success/timeout path.
    cgroup.cleanup();

    Ok(RunscOutcome {
        exit,
        timed_out,
        stdout,
        stderr,
        wall,
        cpu_seconds: last_cpu,
    })
}

/// Build the [`SandboxResult`] from the runtime outcome. The exit code is the `runsc` child's REAL
/// exit status (gVisor returns the container process's exit directly — no forge surface); a timed-out
/// (killed) container has no trustworthy exit (`None`, never fabricated as 0). stdout/stderr are the
/// container's REAL piped streams, HEAD-bounded to [`SANDBOX_CAPTURE_BOUND`] (256 KiB) EACH — the same
/// bound the Firecracker backend uses (shared `lib.rs` const). Usage is the REAL measured figure: host
/// CPU-seconds of the `runsc` process (or a wall-clock ceiling fallback so a real run never
/// under-meters to 0) + mem-byte-seconds from the job's mem ceiling × wall-seconds.
fn build_result(spec: &JobSpec, o: &RunscOutcome) -> SandboxResult {
    let mut stdout = o.stdout.clone();
    if stdout.len() > SANDBOX_CAPTURE_BOUND {
        stdout.truncate(SANDBOX_CAPTURE_BOUND); // HEAD capture (documented; full stream → firehose)
    }
    let mut stderr = o.stderr.clone();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        EgressPolicy, IdemToken, ImageRef, JobKind, MeterTarget, ReserveHandle, ResourceLimits,
        ResourceUsage, RunTokenRef, TrustTier, WorkspaceSpec,
    };

    fn spec(allow: Vec<String>) -> JobSpec {
        JobSpec::new(
            JobKind::Agent,
            ImageRef::pinned("r/img@sha256:abc123def4567890").unwrap(),
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
}
