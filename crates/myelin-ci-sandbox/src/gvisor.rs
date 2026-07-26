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
use crate::launch_gate::{SandboxCommand, SpawnPhase};
use crate::redaction::RedactionPlan;
use crate::runner::RetryableAttemptCause;
use crate::user_namespace::{RunscInvocationMode, UserNamespaceConfig};
use crate::{
    drain_capped, CompletionSettlementOwner, EgressPolicy, HookError, IdemToken, ImageRef, JobKind,
    JobSpec, LaunchPermit, MeterTarget, ReserveHandle, ResourceLimits, ResourceUsage,
    RunTokenCredential, RunnerHooks, SandboxBackend, SandboxCancellation, SandboxHandle,
    SandboxLaunch, SandboxLaunchError, SandboxOutputSink, SandboxOutputStream, SandboxResult,
    TrustTier, WorkspaceSpec, SANDBOX_CAPTURE_BOUND,
};
use std::io;
use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Env var naming the `runsc` binary; defaults to `runsc` on `PATH`.
pub const ENV_RUNSC_BIN: &str = "MYELIN_RUNSC_BIN";

/// The resolved, CANONICALIZED, ABSOLUTE `runsc` binary path — computed ONCE and cached (Sol's
/// review, round 2: re-reading the env var on every launch means a boot preflight validating one
/// binary and a later launch resolving a DIFFERENT one, if the environment changed mid-process,
/// could silently diverge — caching makes "the boot-validated binary is the one every launch uses"
/// an actual invariant, not merely usually true). Round 3: [`preflight_gvisor_runner_host`], if
/// called, populates this SAME cell with its own already-canonicalized, already-probed path. Round
/// 4 correction: a `set()` conflict is NOT a harmless no-op — [`preflight_gvisor_runner_host`]
/// treats it as a hard preflight FAILURE (something else already cached a DIFFERENT path before
/// preflight ran), since silently keeping the earlier, unvalidated value would let preflight report
/// success for a binary no launch actually uses. A test or tool that calls
/// [`run_and_capture`]/[`delete_container`] directly, without ever calling preflight, gets the lazy
/// fallback below instead (best-effort, matching this function's pre-round-3 behavior).
static RESOLVED_RUNSC_BIN: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

fn resolved_runsc_bin_path() -> &'static Path {
    RESOLVED_RUNSC_BIN.get_or_init(|| {
        let configured = std::env::var(ENV_RUNSC_BIN).unwrap_or_else(|_| "runsc".to_string());
        let candidate = if configured.contains('/') {
            PathBuf::from(&configured)
        } else {
            std::env::var("PATH")
                .ok()
                .and_then(|paths| {
                    paths
                        .split(':')
                        .map(|dir| Path::new(dir).join(&configured))
                        .find(|candidate| candidate.exists())
                })
                .unwrap_or_else(|| PathBuf::from(&configured))
        };
        // Canonicalize when possible (resolves symlinks, makes the cached value absolute) —
        // falls back to the unresolved candidate when canonicalization fails (e.g. a test
        // environment with no `runsc` installed at all); resolution failure surfaces naturally as
        // a spawn error at first actual use, exactly as before this round's hardening.
        candidate.canonicalize().unwrap_or(candidate)
    })
}

/// Sol's review, round 4: returns the resolved `&Path` directly (was `&str` via `.to_str().
/// unwrap_or("runsc")`) — falling back to the unrelated literal `"runsc"` for a non-UTF-8 resolved
/// path would have silently swapped in a DIFFERENT, unvalidated binary resolution. `Command::new`/
/// `SandboxCommand::new` both accept `impl AsRef<OsStr>`, so callers pass this straight through.
fn runsc_bin() -> &'static Path {
    resolved_runsc_bin_path()
}

/// How a [`probe_runsc_version`] boot preflight failed; the caller owns the operator-facing wording.
#[derive(Debug, PartialEq, Eq)]
pub enum RunscProbeError {
    /// The binary could not be spawned for its `--version` probe.
    CouldNotExecute,
    /// The binary ran but did not identify itself as `runsc`.
    NotRunsc,
}

/// Boot-preflight probe: verify `path` identifies itself as `runsc` (via `--version`). Lives in
/// this crate — not the caller's — because the sandbox seam is the one sanctioned host-exec site
/// (no-host-exec, X-6/AG-2); serving binaries that require the sandbox at boot call this instead
/// of spawning the probe themselves.
pub fn probe_runsc_version(path: &Path) -> Result<(), RunscProbeError> {
    let output = Command::new(path)
        .arg("--version")
        .output()
        .map_err(|_| RunscProbeError::CouldNotExecute)?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() || !stdout.starts_with("runsc version ") {
        return Err(RunscProbeError::NotRunsc);
    }
    Ok(())
}

/// Boot preflight for a production gVisor runner host. This is deliberately stronger than waiting
/// for the first claimed job: activation must refuse before intake unless the exact runtime,
/// immutable base rootfs, and delegated memory-cgroup boundary are all usable.
pub fn preflight_gvisor_runner_host(runsc: &Path, rootfs: &Path) -> Result<(), String> {
    if !runsc.is_absolute() {
        return Err("MYELIN_RUNSC_BIN must be an absolute path".into());
    }
    let runsc = runsc
        .canonicalize()
        .map_err(|error| format!("MYELIN_RUNSC_BIN must name an existing executable: {error}"))?;
    if !is_executable_file(&runsc) {
        return Err("MYELIN_RUNSC_BIN must name an executable file".into());
    }
    probe_runsc_version(&runsc).map_err(|error| match error {
        RunscProbeError::CouldNotExecute => {
            "MYELIN_RUNSC_BIN could not execute its version probe".to_string()
        }
        RunscProbeError::NotRunsc => {
            "MYELIN_RUNSC_BIN did not identify itself as runsc".to_string()
        }
    })?;
    // Feed THIS canonicalized, probed path into the SAME cell `runsc_bin()` reads, so every
    // subsequent launch uses the exact binary this preflight just validated (Sol's review, round
    // 3) rather than a separately (if identically) re-derived path. Round 4: a `set()` failure
    // (something already cached a DIFFERENT value before this preflight ran) is now a PREFLIGHT
    // FAILURE, not a silently ignored no-op — otherwise preflight could report success having
    // validated binary B while every launch keeps using a stale, previously-cached binary A, with
    // nothing surfacing the divergence.
    if RESOLVED_RUNSC_BIN.set(runsc.clone()).is_err() {
        let already_cached = RESOLVED_RUNSC_BIN
            .get()
            .expect("set() just failed, so the cell must already be initialized");
        if already_cached != &runsc {
            return Err(format!(
                "MYELIN_RUNSC_BIN preflight validated {runsc:?}, but {already_cached:?} was \
                 already cached by an earlier resolution — refusing rather than leaving launches \
                 on a stale, unvalidated binary"
            ));
        }
    }

    if !rootfs.is_absolute() {
        return Err("MYELIN_GVISOR_ROOTFS must be an absolute path".into());
    }
    let rootfs = rootfs.canonicalize().map_err(|error| {
        format!("MYELIN_GVISOR_ROOTFS must name an existing directory: {error}")
    })?;
    if rootfs.parent().is_none() || !rootfs.is_dir() {
        return Err("MYELIN_GVISOR_ROOTFS must resolve to a non-root directory".into());
    }
    for relative in ["bin/sh", "bin/false"] {
        let executable = rootfs.join(relative);
        if !is_executable_file(&executable) {
            return Err(format!(
                "MYELIN_GVISOR_ROOTFS must contain executable {}",
                executable.display()
            ));
        }
    }

    let config = OciConfig {
        args: vec!["/bin/false".into()],
        root_readonly: true,
        drop_all_caps: true,
        no_new_privileges: true,
        seccomp: true,
        has_network: false,
        pids_max: 128,
        mem_bytes: 256 * 1024 * 1024,
        tmpfs_bytes: 1024 * 1024 * 1024,
        extra_mounts: Vec::new(),
        extra_env: Vec::new(),
        root_path: None,
        user_namespace: None,
    };
    let bundle = stage_production_bundle(&config, &rootfs)
        .map_err(|error| format!("CI runner sandbox host preflight failed: {error}"))?;
    let container_id = format!(
        "myelin-preflight-{}-{}",
        std::process::id(),
        unique_suffix()
    );
    let outcome = run_and_capture(
        &runsc,
        &bundle,
        &container_id,
        Duration::from_secs(5),
        config.mem_bytes,
        RunCaptureOptions {
            stdin: None,
            stdout_mode: StdoutMode::CappedHead,
            cancellation: &NEVER_CANCELLED,
            output: None,
        },
        None,
        config.invocation_mode(),
    );
    delete_container(&runsc, &container_id, config.invocation_mode());
    let _ = std::fs::remove_dir_all(&bundle);
    let outcome =
        outcome.map_err(|error| format!("CI runner sandbox host preflight failed: {error}"))?;
    if outcome.timed_out || outcome.exit != Some(1) {
        let stderr = String::from_utf8_lossy(&outcome.stderr);
        let stderr: String = stderr.chars().take(512).collect();
        return Err(format!(
            "CI runner sandbox host preflight `/bin/false` returned {:?}: {stderr}",
            outcome.exit,
        ));
    }
    Ok(())
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
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
    /// Emitted twice deliberately: OCI `linux.resources.pids.limit` for cgroup-capable runtimes and
    /// `RLIMIT_NPROC` for rootless `runsc`, which cannot install the host pids cgroup itself.
    pids_max: u32,
    /// The memory ceiling (bytes) — emitted as `linux.resources.memory.limit`. IMPORTANT (CT-003b /
    /// SI-017): `runsc --rootless` does NOT enforce this OCI field (rootless runsc cannot manage a
    /// host cgroup), so this value is ADVISORY here (it would be honored by a non-rootless `runsc`).
    /// The REAL host-RAM bound for the gVisor workload is the OUT-OF-BAND [`MemoryCgroup`] the
    /// production run path places the `runsc` process tree into — that is what OOM-kills a memory hog
    /// within the limit and keeps it from consuming host RAM beyond `mem_bytes`.
    mem_bytes: u64,
    /// The RAM-backed `/tmp` tmpfs ceiling (bytes) (CT-003a). gVisor would otherwise auto-mount an
    /// UNBOUNDED host-RAM-backed tmpfs at `/tmp`; sizing it caps a disk fill at ENOSPC (the SI-017
    /// host-DoS escape D2 surfaced through the production `launch()`). Sourced from
    /// [`ResourceLimits::tmpfs_bytes`](crate::ResourceLimits::tmpfs_bytes), NOT
    /// `disk_bytes` (that field is the disk-backed ephemeral-workspace quota — unrelated to this
    /// RAM-backed tmpfs).
    tmpfs_bytes: u64,
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
    /// CT-007 slice 2: `None` ⇒ [`RunscInvocationMode::Rootless`] (today's ONLY production
    /// behavior, BYTE-IDENTICAL JSON to before this field existed — no `user` namespace declared,
    /// no `uidMappings`/`gidMappings` emitted). `Some(config)` ⇒
    /// [`RunscInvocationMode::ExplicitUserNamespace`]: this field is the ONE source of truth for
    /// which mode a given config implies (see [`Self::invocation_mode`]) — a caller can never pass
    /// a `RunscInvocationMode` to `run_and_capture`/`delete_container` that disagrees with what
    /// this same `OciConfig` serializes, because both are always derived from this ONE field at
    /// the same call site.
    user_namespace: Option<UserNamespaceConfig>,
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
            // The hardening profile's scratch-tmpfs quota (= `spec.limits.tmpfs_bytes`).
            tmpfs_bytes: profile.scratch_quota_bytes,
            // No extra bind mounts / env by default — a CI/agent job's posture is byte-unchanged.
            extra_mounts: Vec::new(),
            extra_env: Vec::new(),
            root_path: None,
            // Rootless by default — CT-007 slice 2 makes ExplicitUserNamespace mode POSSIBLE, it
            // does not change any existing caller's default behavior.
            user_namespace: None,
        }
    }

    /// CT-006a: override the OCI `root.path` with an ABSOLUTE staged-rootfs path (so the bundle needs no
    /// `rootfs` symlink — required when a host bind mount is present). Consuming builder.
    pub fn with_root_path(mut self, path: PathBuf) -> OciConfig {
        self.root_path = Some(path);
        self
    }

    /// CT-007 slice 2: attach an explicit user-namespace mapping — the resulting `to_json` gains a
    /// `user` namespace entry plus the exact two-entry `uidMappings`/`gidMappings`, and
    /// [`Self::invocation_mode`] reports [`RunscInvocationMode::ExplicitUserNamespace`]. Consuming
    /// builder.
    pub fn with_user_namespace(mut self, config: UserNamespaceConfig) -> OciConfig {
        self.user_namespace = Some(config);
        self
    }

    /// The [`RunscInvocationMode`] this config implies — the ONE place that decision is made,
    /// derived structurally from [`Self::user_namespace`] so it can never disagree with what
    /// [`Self::to_json`] actually serializes.
    pub fn invocation_mode(&self) -> RunscInvocationMode {
        match self.user_namespace {
            Some(config) => RunscInvocationMode::ExplicitUserNamespace(config),
            None => RunscInvocationMode::Rootless,
        }
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
            self.tmpfs_bytes
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
        // CT-007 slice 2: `Rootless` (the ONLY production behavior before this field existed)
        // emits BYTE-IDENTICAL JSON — no `user` namespace, no `uidMappings`/`gidMappings`.
        // `ExplicitUserNamespace` adds a `user` namespace entry alongside the always-present
        // network one, plus the exact two-entry uid/gid maps (container 0 -> this process's real
        // identity, container 65534 -> the leased subordinate host uid/gid).
        let (namespaces_json, id_mappings_json) = match &self.user_namespace {
            None => (net_ns.to_string(), String::new()),
            Some(cfg) => (
                format!("{net_ns}, {{ \"type\": \"user\" }}"),
                format!(
                    ",\n    \"uidMappings\": [ {{ \"containerID\": 0, \"hostID\": {ruid}, \
                     \"size\": 1 }}, {{ \"containerID\": {untrusted_uid}, \"hostID\": {suid}, \
                     \"size\": 1 }} ],\n    \
                     \"gidMappings\": [ {{ \"containerID\": 0, \"hostID\": {rgid}, \"size\": 1 }}, \
                     {{ \"containerID\": {untrusted_gid}, \"hostID\": {sgid}, \"size\": 1 }} ]",
                    ruid = cfg.runner_uid(),
                    rgid = cfg.runner_gid(),
                    suid = cfg.subordinate_uid(),
                    sgid = cfg.subordinate_gid(),
                    untrusted_uid = UNTRUSTED_UID,
                    untrusted_gid = UNTRUSTED_GID,
                ),
            ),
        };
        format!(
            "{{\n  \"ociVersion\": \"1.0.0\",\n  \"process\": {{\n    \
             \"user\": {{ \"uid\": {uid}, \"gid\": {gid} }},\n    \
             \"args\": [{args}],\n    \"cwd\": \"/\",\n    \
             \"env\": [{env_json}],\n    \
             \"noNewPrivileges\": {nnp},\n    \
             \"rlimits\": [{{ \"type\": \"RLIMIT_NPROC\", \"hard\": {pids}, \"soft\": {pids} }}],\n    \
             \"capabilities\": {{ \"bounding\": [], \"effective\": [], \"permitted\": [] }}\n  }},\n  \
             \"root\": {{ \"path\": {root_path:?}, \"readonly\": {ro} }},\n  \
             \"mounts\": [ {mounts_json} ],\n  \
             \"linux\": {{\n    \"resources\": {{ \"memory\": {{ \"limit\": {mem} }}, \
             \"pids\": {{ \"limit\": {pids} }} }},\n    \
             \"seccomp\": {{ \"defaultAction\": \"SCMP_ACT_ERRNO\" }},\n    \
             \"namespaces\": [ {namespaces_json} ]{id_mappings_json}\n  }}\n}}",
            uid = UNTRUSTED_UID,
            gid = UNTRUSTED_GID,
            args = args,
            env_json = env_json,
            nnp = self.no_new_privileges,
            ro = self.root_readonly,
            mounts_json = mounts_json,
            mem = self.mem_bytes,
            pids = self.pids_max,
            namespaces_json = namespaces_json,
            id_mappings_json = id_mappings_json,
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
    /// `spec.image` could not be resolved against the [`GvisorAssetRegistry`]'s already-verified
    /// entries BEFORE any resource was reserved — an unregistered reference (registry construction
    /// itself refuses an unsupported digest algorithm, an invalid rootfs path, or a canonical-tree
    /// digest mismatch, so none of those can surface here). Refused in `launch_with` AFTER
    /// `enforce_isolation_floor`/the hardening assert but BEFORE `reserve`/anything else.
    Image(String),
}

impl std::fmt::Display for GvisorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GvisorError::Hook(e) => write!(f, "gvisor backend: guarantee hook failed: {e}"),
            GvisorError::Hardening(s) => write!(f, "gvisor backend: hardening not enforced: {s}"),
            GvisorError::Runtime(s) => write!(f, "gvisor backend: runsc error: {s}"),
            GvisorError::Image(s) => write!(f, "gvisor backend: image resolution refused: {s}"),
        }
    }
}

impl std::error::Error for GvisorError {}

impl From<crate::HookError> for GvisorError {
    fn from(e: crate::HookError) -> Self {
        GvisorError::Hook(e)
    }
}

impl From<crate::asset_registry::AssetRegistryError> for GvisorError {
    fn from(e: crate::asset_registry::AssetRegistryError) -> Self {
        GvisorError::Image(e.to_string())
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
fn memory_cgroup_refusal(reason: impl core::fmt::Display) -> String {
    format!("{reason} — refusing to run the gVisor workload unbounded (SI-017 fail-closed)")
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

    #[cfg(test)]
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
        std::fs::create_dir(&dir)
            .map_err(|e| memory_cgroup_refusal(format!("create memory cgroup {dir:?}: {e}")))?;
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
        let identity = cgroup_identity(&dir).map_err(|e| {
            let _ = std::fs::remove_dir(&dir);
            memory_cgroup_refusal(format!("stat freshly-created memory cgroup {dir:?}: {e}"))
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
    fn kill_file(&self) -> std::io::Result<std::fs::File> {
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
///
/// **No `Default`, deliberately.** An ordinary (non-git-wire) launch resolves `spec.image` through a
/// [`GvisorAssetRegistry`](crate::asset_registry::GvisorAssetRegistry) — there is no
/// registry-less production backend a caller could construct by accident. [`GvisorBackend::new`]
/// requires one explicitly; [`GvisorBackend::git_wire_only`] is the SEPARATE, loudly-named
/// constructor for the git-wire receive/upload-pack path (which resolves its OWN rootfs via
/// [`resolved_gvisor_git_rootfs`] and never consults the registry) and REFUSES ordinary
/// `launch`/`launch_streaming`.
pub struct GvisorBackend {
    /// guest_id → the live container's teardown state (its `runsc` child + bundle temp dir). Ephemeral;
    /// one job per container, never reused.
    live: Mutex<std::collections::HashMap<String, RunscProc>>,
    /// The image→rootfs authority an ordinary launch resolves `spec.image` through. `None` only for
    /// a [`GvisorBackend::git_wire_only`] backend, which refuses ordinary launch outright (so this
    /// is never consulted from that path either).
    registry: Option<Arc<crate::asset_registry::GvisorAssetRegistry>>,
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
    /// A post-spawn transport/cancellation failure. Usage is still settled before launch refuses.
    pub run_error: Option<String>,
}

impl GvisorBackend {
    /// A new backend with no live containers, resolving every ordinary (non-git-wire) launch's
    /// `spec.image` through `registry` — the real launch authority (CT-007 gate 2/4). There is no
    /// argument-less constructor: a registry MUST be supplied explicitly (see
    /// [`GvisorBackend::git_wire_only`] for the one legitimate case that needs none).
    pub fn new(registry: Arc<crate::asset_registry::GvisorAssetRegistry>) -> GvisorBackend {
        GvisorBackend {
            live: Mutex::new(std::collections::HashMap::new()),
            registry: Some(registry),
        }
    }

    /// A backend for the git-wire receive/upload-pack path ONLY
    /// ([`launch_git_wire`](Self::launch_git_wire) / [`launch_git_receive_pack`](Self::launch_git_receive_pack)
    /// and their `_until_cancelled` variants) — that path resolves its OWN rootfs via
    /// [`resolved_gvisor_git_rootfs`], a separate, pre-existing, deliberately different mechanism
    /// from ordinary job launch, and never consults an image registry. A backend built this way has
    /// NO registry at all, so an ordinary [`SandboxBackend::launch`]/[`SandboxBackend::launch_streaming`]
    /// call REFUSES with [`GvisorError::Image`] — a git-wire-only instance can never accidentally be
    /// used to launch an ordinary, image-bearing job.
    pub fn git_wire_only() -> GvisorBackend {
        GvisorBackend {
            live: Mutex::new(std::collections::HashMap::new()),
            registry: None,
        }
    }

    /// Build the OCI config a launch WOULD use for `spec` (the hardened profile derived + the OCI
    /// JSON assembled), without running. Asserts the mandatory profile is in force.
    pub fn oci_config(spec: &JobSpec) -> Result<OciConfig, GvisorError> {
        let profile = HardeningProfile::derive(spec);
        profile.assert_enforced().map_err(GvisorError::Hardening)?;
        Ok(OciConfig::from_spec(spec, &profile))
    }

    /// Drive the four-guarantee seam in the mandated order — **isolation floor → hardening assert →
    /// image resolution (now a cheap, already-verified lookup) → reserve → final attribution/claim
    /// CAS → run → settle** — fail-closed at every step, then hand the captured [`SandboxResult`]
    /// back behind the redrawn CT-001 seam. This mirrors the Firecracker backend's own `launch_with`
    /// ordering exactly, with the image lookup inserted after the hardening assert and before
    /// reserve. `spec.image` is looked up against the registry's ALREADY-VERIFIED entries (the
    /// canonical-tree digest work happened ONCE, at [`GvisorAssetRegistry::from_bindings`]
    /// construction time — see `crate::asset_registry` — never per launch); an unknown image still
    /// refuses before any resource is reserved or any launch permit is granted (CT-007 gate 2/4), but
    /// a RED isolation floor now refuses BEFORE the registry is even consulted, so an exhausted-wallet
    /// caller cannot force a (now-cheap, but still real) lookup with zero chance of ever launching,
    /// and the floor is honoured even for callers naming an image the registry doesn't know about.
    /// The `run` closure does the actual run: it stages an OCI bundle from the built [`OciConfig`] +
    /// the verified rootfs path, runs `runsc run --bundle` (the untrusted `spec.command`), captures
    /// the real exit/streams/usage and enforces `spec.limits.timeout_secs`, and returns a
    /// [`ContainerRun`]. The trait `launch` passes [`run_production_container`] (a REAL `runsc`
    /// container); unit tests pass a closure returning a fake child + a canned result so the control
    /// flow is testable without a runtime (the injectable-spawn seam — preserved). `run` is only
    /// invoked AFTER reserve succeeds, so an exhausted wallet / unmet isolation floor refuses-to-start
    /// and `runsc` never spawns (CT-002b: the result is CONSUMED from the run, never hardcoded —
    /// reconciles with the Firecracker `launch_with`).
    fn launch_with<F>(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
        run: F,
    ) -> Result<SandboxLaunch, SandboxLaunchError<GvisorError>>
    where
        F: FnOnce(&JobSpec, &OciConfig, LaunchPermit, &Path) -> Result<ContainerRun, RunFailure>,
    {
        // #4 isolation floor FIRST — the hardening profile must hold before any code (including the
        // registry lookup) runs. Mirrors the Firecracker backend's own ordering. Every early refusal
        // here is an ordinary pre-commit `Failed` — nothing durable has been claimed yet.
        hooks
            .enforce_isolation_floor(spec)
            .map_err(|e| SandboxLaunchError::Failed(e.into()))?;
        let profile = HardeningProfile::derive(spec);
        profile
            .assert_enforced()
            .map_err(|e| SandboxLaunchError::Failed(GvisorError::Hardening(e)))?;

        // CT-007 gate 2/4: resolve `spec.image` against the registry's ALREADY-VERIFIED entries — a
        // cheap O(1) lookup now (verification happened once, at registry construction). Still BEFORE
        // reserve/the launch-permit CAS — an unregistered image never reserves or spawns. A
        // `git_wire_only()` backend has no registry at all and refuses here, so it can never launch
        // an ordinary image-bearing job.
        let registry = self.registry.as_ref().ok_or_else(|| {
            SandboxLaunchError::Failed(GvisorError::Image(
                "this GvisorBackend was constructed via GvisorBackend::git_wire_only() (no asset \
                 registry) and cannot launch an ordinary image-bearing job — construct it via \
                 GvisorBackend::new(registry) for CI/agent job launch"
                    .to_string(),
            ))
        })?;
        let verified_rootfs = registry
            .resolve(&spec.image)
            .map_err(|e| SandboxLaunchError::Failed(e.into()))?;

        let reserve = hooks
            .reserve(spec)
            .map_err(|e| SandboxLaunchError::Failed(e.into()))?;
        let cfg = OciConfig::from_spec(spec, &profile);
        let launch_permit = match hooks.acquire_launch_permit(spec) {
            Ok(permit) => permit,
            Err(attribute_error) => {
                hooks
                    .release_unused(spec, &reserve)
                    .map_err(|e| SandboxLaunchError::Failed(e.into()))?;
                return Err(SandboxLaunchError::Failed(attribute_error.into()));
            }
        };
        // Run the container + capture the REAL result (the ONE legitimate `runsc`-spawn site — the
        // sandbox seam's mechanism; the `no-host-exec` named exclusion). `run` cleans up its own
        // bundle/container on error.
        //
        // `run`'s failure carries the phase the launch reached before it failed (Sol's design, fixing
        // a genuine pre-existing leak: the OLD code just propagated the error here and returned early,
        // never calling `release_unused` NOR `settle_completed` — leaking the reservation forever on
        // ANY run() failure, including ones where a real sandbox process actually executed). The
        // correct disposition ALSO depends on `hooks.completion_settlement_owner()`: under
        // `TerminalReporter` ownership, `settle_completed` is a documented no-op (deferred to the
        // real reporter), so a post-commit failure can only be honestly recorded by routing it
        // through `SandboxLaunchError::RetryableAttempt` — the runner then calls the reporter's own
        // `report_retryable_attempt` transaction, which durably accounts usage AND requeues the exact
        // claim without emitting `job.done`. Returning a bare `Failed` here for a post-commit failure
        // under reporter ownership would silently discard the accounting the reporter exists to do.
        let ContainerRun {
            child,
            bundle_dir,
            result,
            run_error,
        } = match run(spec, &cfg, launch_permit, verified_rootfs.path()) {
            Ok(container_run) => container_run,
            Err(run_failure) => {
                return Err(self.dispose_run_failure(spec, hooks, &reserve, run_failure));
            }
        };

        let guest_id = format!("runsc-{}", spec.idem_token.0);
        self.live
            .lock()
            .unwrap()
            .insert(guest_id.clone(), RunscProc { child, bundle_dir });

        // Settle against the result's REAL measured usage (CT-002b) — never interrupt in-flight.
        if let Err(error) = hooks.settle_completed(spec, &reserve, result.usage) {
            let _ = self.kill(&SandboxHandle {
                guest_id: guest_id.clone(),
            });
            return Err(SandboxLaunchError::Failed(error.into()));
        }

        Ok(SandboxLaunch {
            handle: SandboxHandle { guest_id },
            result,
            output_complete: run_error.is_none(),
        })
    }

    /// Dispose of a post-`reserve` [`RunFailure`] into the correct [`SandboxLaunchError`] variant,
    /// per phase AND per `hooks.completion_settlement_owner()` (Sol's disposition table):
    ///
    /// | Phase                     | `Hook` owner                        | `TerminalReporter` owner             |
    /// |----------------------------|--------------------------------------|----------------------------------------|
    /// | `Uncommitted`              | `release_unused`, then `Failed`       | `release_unused`, then `Failed`         |
    /// | `CommitOutcomeUnknown`     | `DurableOutcomeUnknown`               | `DurableOutcomeUnknown`                 |
    /// | `CommittedButNotExecuted`  | settle zero, then `Failed`            | `RetryableAttempt(SandboxInfrastructure, zero)` |
    /// | `Executed`                 | settle carried usage, then `Failed`   | `RetryableAttempt(SandboxInfrastructure, usage)` |
    ///
    /// `Uncommitted` and `CommitOutcomeUnknown` are owner-independent: an uncommitted attempt has no
    /// terminal report to defer to regardless of who owns completion, and an outcome-unknown attempt
    /// must never be guessed at either way. Only the two post-commit phases branch on ownership,
    /// because only they have a real (if zero) measured cost a `TerminalReporter` must account.
    fn dispose_run_failure(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
        reserve: &ReserveHandle,
        run_failure: RunFailure,
    ) -> SandboxLaunchError<GvisorError> {
        let message = run_failure.to_string();
        match run_failure {
            RunFailure::Uncommitted { .. } => {
                if let Err(settle_error) = hooks.release_unused(spec, reserve) {
                    return SandboxLaunchError::Failed(GvisorError::Runtime(format!(
                        "run() failed (uncommitted: {message}) AND release_unused also failed \
                         ({settle_error}) — reservation may be leaked"
                    )));
                }
                SandboxLaunchError::Failed(GvisorError::Runtime(message))
            }
            RunFailure::CommitOutcomeUnknown { .. } => {
                // Neither release nor settle — the durable store may or may not have actually
                // committed. Guessing either way misaccounts a real reservation; durable
                // reconciliation (the existing lease/claim reaper) is the only honest owner here.
                SandboxLaunchError::DurableOutcomeUnknown(GvisorError::Runtime(message))
            }
            RunFailure::CommittedButNotExecuted { .. } => {
                let zero = ResourceUsage {
                    cpu_seconds: 0,
                    mem_byte_seconds: 0,
                };
                match hooks.completion_settlement_owner() {
                    CompletionSettlementOwner::TerminalReporter => {
                        SandboxLaunchError::RetryableAttempt {
                            source: GvisorError::Runtime(message),
                            cause: RetryableAttemptCause::SandboxInfrastructure,
                            usage: zero,
                        }
                    }
                    CompletionSettlementOwner::Hook => {
                        if let Err(settle_error) = hooks.settle_completed(spec, reserve, zero) {
                            return SandboxLaunchError::Failed(GvisorError::Runtime(format!(
                                "run() failed (committed but not executed: {message}) AND its \
                                 zero-usage settlement also failed ({settle_error}) — \
                                 reservation may be leaked"
                            )));
                        }
                        SandboxLaunchError::Failed(GvisorError::Runtime(message))
                    }
                }
            }
            RunFailure::Executed { usage, .. } => match hooks.completion_settlement_owner() {
                CompletionSettlementOwner::TerminalReporter => {
                    SandboxLaunchError::RetryableAttempt {
                        source: GvisorError::Runtime(message),
                        cause: RetryableAttemptCause::SandboxInfrastructure,
                        usage,
                    }
                }
                CompletionSettlementOwner::Hook => {
                    if let Err(settle_error) = hooks.settle_completed(spec, reserve, usage) {
                        return SandboxLaunchError::Failed(GvisorError::Runtime(format!(
                            "run() failed (executed: {message}) AND its conservative-usage \
                             settlement also failed ({settle_error}) — reservation may be leaked"
                        )));
                    }
                    SandboxLaunchError::Failed(GvisorError::Runtime(message))
                }
            },
        }
    }
}

impl SandboxBackend for GvisorBackend {
    type Error = GvisorError;

    /// Run a digest-pinned [`JobSpec`] inside a REAL `runsc` (gVisor) sandbox. Blocks until the
    /// container has run and the four guarantees have fired. The REAL `runsc` container is spawned
    /// here — the one legitimate runtime-spawn site (the `no-host-exec` named exclusion; this seam IS
    /// the unified sandbox, not a bypass of it).
    fn launch(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
    ) -> Result<SandboxLaunch, SandboxLaunchError<Self::Error>> {
        self.launch_with(spec, hooks, |spec, cfg, permit, rootfs| {
            run_production_container(spec, cfg, permit, rootfs)
        })
    }

    fn launch_streaming(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
        output: Arc<dyn SandboxOutputSink>,
        cancellation: SandboxCancellation,
    ) -> Result<SandboxLaunch, SandboxLaunchError<Self::Error>> {
        self.launch_with(spec, hooks, move |spec, cfg, permit, rootfs| {
            run_production_container_streaming(
                spec,
                cfg,
                permit,
                rootfs,
                Some(output),
                cancellation,
            )
        })
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
/// 1. Stage a temp bundle dir: a `rootfs` symlink → the caller-supplied, ALREADY-VERIFIED rootfs
///    path (`GvisorBackend::launch_with` resolved + digest-verified it from `spec.image` against the
///    [`GvisorAssetRegistry`](crate::asset_registry::GvisorAssetRegistry) BEFORE calling here — CT-007
///    gate 2/4; this function no longer calls [`resolved_gvisor_rootfs`] itself) + a `config.json` =
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
fn run_production_container(
    spec: &JobSpec,
    cfg: &OciConfig,
    launch_permit: LaunchPermit,
    rootfs: &Path,
) -> Result<ContainerRun, RunFailure> {
    run_production_container_streaming(
        spec,
        cfg,
        launch_permit,
        rootfs,
        None,
        SandboxCancellation::new(),
    )
}

fn run_production_container_streaming(
    spec: &JobSpec,
    cfg: &OciConfig,
    launch_permit: LaunchPermit,
    rootfs: &Path,
    output: Option<Arc<dyn SandboxOutputSink>>,
    cancellation: SandboxCancellation,
) -> Result<ContainerRun, RunFailure> {
    let bin = runsc_bin();
    // `rootfs` is the ALREADY-VERIFIED path `GvisorBackend::launch_with` resolved from `spec.image`
    // via the `GvisorAssetRegistry` (CT-007 gate 2/4) — this production run path no longer calls
    // `resolved_gvisor_rootfs()` itself. That resolver remains the resolution mechanism the
    // registry's own construction (`GvisorAssetRegistry::from_bindings`, called once by
    // `production_gvisor_registry()`) uses, plus test scaffolding and the git-wire path's OWN
    // separate resolver — ordinary per-launch code no longer calls it directly. Honest fail-closed: a
    // runtime/start precondition failure surfaces as an error (never a fabricated exit). An absent
    // rootfs cannot produce a valid bundle (defense in depth — construction-time verification already
    // checked this, but a TOCTOU window between verification and staging is still handled honestly).
    // Both errors below occur before any spawn attempt — Uncommitted.
    if !rootfs.exists() {
        return Err(RunFailure::uncommitted(format!(
            "staged gVisor rootfs absent: {} (cannot build a valid OCI bundle)",
            rootfs.display()
        )));
    }
    let bundle_dir = stage_production_bundle(cfg, rootfs).map_err(RunFailure::uncommitted)?;
    let container_id = format!("myelin-prod-{}-{}", std::process::id(), unique_suffix());

    let timeout = Duration::from_secs(spec.limits.timeout_secs as u64);
    let redaction = RedactionPlan::for_job(spec);
    let mode = cfg.invocation_mode();
    let outcome = match run_and_capture(
        bin,
        &bundle_dir,
        &container_id,
        timeout,
        spec.limits.mem_bytes,
        RunCaptureOptions {
            stdin: None, // CI/agent jobs receive no stdin (the git-wire path supplies the body).
            stdout_mode: StdoutMode::CappedHead,
            cancellation: cancellation.as_atomic(),
            output: output.map(|sink| StreamingOutput {
                sink,
                redaction: redaction.clone(),
            }),
        },
        Some(launch_permit),
        mode,
    ) {
        Ok(o) => o,
        Err(e) => {
            // Spawning/waiting failed before a trustworthy result — clean up + surface honestly.
            delete_container(bin, &container_id, mode);
            let _ = std::fs::remove_dir_all(&bundle_dir);
            return Err(e);
        }
    };
    // The container has exited (or been timeout-killed) — best-effort delete (idempotent; `runsc run`
    // usually self-deletes on a clean exit, but the timeout path leaves it for us to reap).
    delete_container(bin, &container_id, mode);

    let result = build_result(spec, &outcome, &redaction);
    Ok(ContainerRun {
        child: Box::new(SpawnedRunsc {
            bin,
            container_id,
            mode,
        }),
        bundle_dir,
        result,
        run_error: outcome.stream_error,
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

/// The `runsc` GLOBAL flags (BEFORE the subcommand) `mode` implies — the ONE place any of
/// `run`/`kill`/`delete` decides this, so no call site makes an independent flag decision (CT-007
/// slice 2). `Rootless` is byte-identical to the pre-slice-2 flag (just `--rootless`).
/// `ExplicitUserNamespace` drops `--rootless` and adds `-ignore-cgroups` — confirmed by a live
/// spike against the pinned `runsc` build that dropping `--rootless` surfaces a REAL cgroup-setup
/// requirement `runsc`'s own cgroupfs manager cannot satisfy without root (even under a cgroup
/// path nested entirely under this process's own delegated slice); `-ignore-cgroups` makes it skip
/// that internal management entirely WITHOUT weakening [`MemoryCgroup`], which places the spawned
/// `runsc` process into a real, already-owned cgroup externally, independent of this flag.
/// Env var naming the directory `runsc` resolves `newuidmap`/`newgidmap` through, under
/// [`RunscInvocationMode::ExplicitUserNamespace`] — the ONLY entry in the (otherwise cleared)
/// `PATH` that mode's `runsc` invocation sees. Defaults to `/usr/bin` (where this host's real
/// setuid helpers live); a production deployment SHOULD point this at a dedicated, curated
/// directory containing ONLY the two validated helpers (Sol's review) — [`preflight_explicit_userns_helpers`]
/// validates whatever directory is actually configured, it does not require it to be minimal.
pub const ENV_EXPLICIT_USERNS_HELPER_DIR: &str = "MYELIN_EXPLICIT_USERNS_HELPER_DIR";

/// Resolved ONCE and cached (Sol's review, round 3: re-reading the env var inside every
/// `run`/`kill`/`delete` call meant an environment mutation mid-process could launch a container
/// under one helper directory and later kill/delete it under a DIFFERENT one — caching makes "one
/// resolved value for the whole process" an actual invariant). Not itself validated (this is a
/// plain resolver, matching [`resolved_explicit_userns_runsc_root`]'s role) — a caller enabling
/// [`RunscInvocationMode::ExplicitUserNamespace`] in production reads this value and passes it to
/// [`preflight_explicit_userns_policy`] once at startup (mirroring how [`preflight_gvisor_runner_host`]'s
/// own caller resolves `MYELIN_RUNSC_BIN` itself before calling in).
pub fn resolved_explicit_userns_helper_dir() -> &'static Path {
    static CACHED: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    CACHED.get_or_init(|| {
        std::env::var(ENV_EXPLICIT_USERNS_HELPER_DIR)
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/usr/bin"))
    })
}

/// Env var naming the `runsc` state-root directory used ONLY under
/// [`RunscInvocationMode::ExplicitUserNamespace`] — passed explicitly (`--root=<path>`) so
/// container-state lookup never depends on `$XDG_RUNTIME_DIR` (cleared, along with the rest of
/// the environment, for this mode — Sol's review: clearing the environment without ALSO fixing
/// `--root` could make startup fail or state lookup diverge from whatever `runsc`'s own default
/// resolution would have picked).
pub const ENV_EXPLICIT_USERNS_RUNSC_ROOT: &str = "MYELIN_EXPLICIT_USERNS_RUNSC_ROOT";

/// Resolved ONCE and cached (Sol's review, round 3: a relative `--root=` would resolve against
/// `runsc`'s own current working directory at spawn time, which this process does not control — a
/// state root that silently moved between launches would fragment container-state lookup). A
/// relative configured value is joined onto this process's current directory AT THE MOMENT OF
/// FIRST RESOLUTION, making the RESULT absolute in the ordinary case — but this resolver alone does
/// NOT guarantee absoluteness (if `current_dir()` itself fails, the relative value is returned
/// as-is; round 4's doc comment overclaimed this). What actually enforces absoluteness is
/// [`preflight_explicit_userns_policy`], which explicitly refuses a non-absolute `runsc_root`
/// before ever installing it into [`EXPLICIT_USERNS_POLICY`] — this resolver is a best-effort
/// convenience the caller feeds INTO that real gate, not the gate itself.
pub fn resolved_explicit_userns_runsc_root() -> &'static Path {
    static CACHED: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    CACHED.get_or_init(|| {
        let configured = std::env::var(ENV_EXPLICIT_USERNS_RUNSC_ROOT)
            .ok()
            .map(PathBuf::from);
        let default = || {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
            PathBuf::from(home)
                .join(".local")
                .join("state")
                .join("myelin-runsc-explicit-userns")
        };
        let resolved = configured.unwrap_or_else(default);
        if resolved.is_absolute() {
            resolved
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(&resolved))
                .unwrap_or(resolved)
        }
    })
}

/// Boot preflight for [`RunscInvocationMode::ExplicitUserNamespace`] (mirrors
/// [`preflight_gvisor_runner_host`]'s role for the base runtime): verify `helper_dir` is an
/// absolute, non-symlinked, root-owned, not-group/other-writable directory whose own ANCESTOR
/// chain this process cannot rename/replace (Sol's review, round 3: an earlier version used
/// `std::fs::metadata`, which FOLLOWS a symlink at `helper_dir` itself, silently validating
/// whatever the symlink pointed at instead of refusing it — fixed by checking
/// `symlink_metadata` first), and that it contains `newuidmap`/`newgidmap` as regular, root-owned,
/// setuid files, never group/other-writable, and executable BY THIS PROCESS'S OWN EFFECTIVE
/// IDENTITY specifically (checked via the kernel's own `faccessat(..., AT_EACCESS)` rather than
/// "some execute bit is set somewhere," which could pass on a root-only-executable file this
/// process could never actually run) — the trust chain `runsc`'s own internal resolution depends
/// on once `PATH` is fixed to exactly this directory. Never called automatically; a caller enabling
/// `ExplicitUserNamespace` mode in production calls this once at startup.
pub fn preflight_explicit_userns_helpers(helper_dir: &Path) -> Result<(), String> {
    if !helper_dir.is_absolute() {
        return Err(format!("{helper_dir:?} must be an absolute path"));
    }
    // `symlink_metadata` (NOT `metadata`) so a symlinked `helper_dir` is refused outright rather
    // than transparently validating whatever it points at.
    let dir_meta =
        std::fs::symlink_metadata(helper_dir).map_err(|e| format!("stat {helper_dir:?}: {e}"))?;
    if dir_meta.file_type().is_symlink() {
        return Err(format!("{helper_dir:?} must not be a symlink"));
    }
    if !dir_meta.is_dir() {
        return Err(format!("{helper_dir:?} is not a directory"));
    }
    if dir_meta.uid() != 0 {
        return Err(format!(
            "{helper_dir:?} must be owned by root (uid 0), got uid {}",
            dir_meta.uid()
        ));
    }
    if dir_meta.mode() & 0o022 != 0 {
        return Err(format!(
            "{helper_dir:?} must not be group/other-writable (mode {:o})",
            dir_meta.mode() & 0o777
        ));
    }
    crate::dirlock::verify_ancestors_not_writable_by_us(helper_dir).map_err(|reason| {
        format!("{helper_dir:?}'s ancestor chain is not safely anchored: {reason}")
    })?;
    for helper in ["newuidmap", "newgidmap"] {
        let path = helper_dir.join(helper);
        let meta = std::fs::symlink_metadata(&path).map_err(|e| format!("stat {path:?}: {e}"))?;
        if meta.file_type().is_symlink() {
            return Err(format!("{path:?} must not be a symlink"));
        }
        if !meta.is_file() {
            return Err(format!("{path:?} must be a regular file"));
        }
        if meta.uid() != 0 {
            return Err(format!(
                "{path:?} must be owned by root (uid 0), got uid {}",
                meta.uid()
            ));
        }
        if meta.mode() & 0o4000 == 0 {
            return Err(format!(
                "{path:?} must be setuid (mode {:o} lacks the setuid bit)",
                meta.mode() & 0o7777
            ));
        }
        if meta.mode() & 0o022 != 0 {
            return Err(format!(
                "{path:?} must not be group/other-writable (mode {:o})",
                meta.mode() & 0o777
            ));
        }
        let path_c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
            .map_err(|e| format!("{path:?} contains an interior NUL: {e}"))?;
        // SAFETY: `path_c` is a valid, NUL-terminated path; `faccessat` only queries permission
        // bits, it never mutates anything. `AT_EACCESS` checks this process's EFFECTIVE identity
        // (matching the identity `runsc` — spawned by this same process — will actually run as),
        // rather than "does some execute bit exist," which could pass for a root-only-executable
        // file this process could never actually invoke.
        let executable_by_us = unsafe {
            libc::faccessat(
                libc::AT_FDCWD,
                path_c.as_ptr(),
                libc::X_OK,
                libc::AT_EACCESS,
            )
        } == 0;
        if !executable_by_us {
            return Err(format!(
                "{path:?} is not executable by this process's effective identity"
            ));
        }
    }
    Ok(())
}

/// The runsc release this slice's `ExplicitUserNamespace` OCI/CLI contract (multi-ID `uidMappings`/
/// `gidMappings`, `-ignore-cgroups`, explicit `--root=`) was actually validated against (the live
/// spike + every drill run in this repo's own development). Sol's review, round 4: the new
/// contract is "explicitly justified and accepted against that release" specifically, not against
/// "whatever identifies itself as runsc" — pin the exact version string AND the binary's own
/// content digest, rather than accepting a same-named-but-different build.
const PINNED_EXPLICIT_USERNS_RUNSC_VERSION: &str = "runsc version release-20260608.0";
/// SHA-256 of the exact `runsc` binary this repo's `ExplicitUserNamespace` contract was validated
/// against (computed once, off this development host's own pinned install).
const PINNED_EXPLICIT_USERNS_RUNSC_SHA256_HEX: &str =
    "4ec073363641a44cc5d171f63f1e23b76016ef632eb3269395c79ac8aecb71bc";

fn sha256_hex_of_file(path: &Path) -> io::Result<String> {
    use sha2::{Digest, Sha256};
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = io::Read::read(&mut file, &mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// Verify `bin` is EXACTLY the runsc release+build this slice's `ExplicitUserNamespace` contract
/// was validated against — both the binary's own content digest AND the reported version string.
/// Sol's review, round 6: HASH FIRST, EXECUTE ONLY AFTER the digest matches — the previous version
/// ran `bin --version` before ever checking the digest, meaning ANY candidate at `bin` (forged,
/// corrupted, or attacker-planted) got arbitrary host execution before this function could ever
/// reject it. Hashing first means a candidate that doesn't match the pinned digest is NEVER
/// executed at all. This function alone does not close the TOCTOU between the hash-read and the
/// `--version` exec a moment later — that gap is closed by requiring the CALLER to have already
/// established the path's immutability (via [`harden_explicit_userns_runsc_binary`]) before this
/// function ever runs, so nothing could swap the file's content in between.
fn verify_pinned_explicit_userns_runsc(bin: &Path) -> Result<(), String> {
    let digest = sha256_hex_of_file(bin).map_err(|e| format!("hash {bin:?}: {e}"))?;
    if digest != PINNED_EXPLICIT_USERNS_RUNSC_SHA256_HEX {
        return Err(format!(
            "{bin:?}'s content digest {digest} does not match the pinned \
             {PINNED_EXPLICIT_USERNS_RUNSC_SHA256_HEX} — refusing to execute a candidate that \
             hasn't already been proven byte-identical to the trusted build"
        ));
    }
    let output = Command::new(bin)
        .arg("--version")
        .output()
        .map_err(|e| format!("{bin:?} --version: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "{bin:?} --version exited {:?} (expected success)",
            output.status.code()
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let version_line = stdout.lines().next().unwrap_or("");
    if version_line != PINNED_EXPLICIT_USERNS_RUNSC_VERSION {
        return Err(format!(
            "{bin:?} reports {version_line:?}, but ExplicitUserNamespace mode is pinned to \
             exactly {PINNED_EXPLICIT_USERNS_RUNSC_VERSION:?}"
        ));
    }
    Ok(())
}

/// The fully resolved, atomically-validated set of values `ExplicitUserNamespace` mode's `runsc`
/// invocation depends on (Sol's review, round 4: three independently-cached `OnceLock`s do not
/// bind VALIDATION to USE — a caller could validate directory B while a stale, independently
/// resolved cache still points at directory A). Installed ONCE, as a single unit, only after every
/// field has been checked TOGETHER — there is no way to observe a partially-validated policy.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedExplicitUsernsPolicy {
    helper_dir: PathBuf,
    runsc_root: PathBuf,
}

static EXPLICIT_USERNS_POLICY: std::sync::OnceLock<ResolvedExplicitUsernsPolicy> =
    std::sync::OnceLock::new();

/// Verify `bin` (the runsc binary `ExplicitUserNamespace` mode will execute) cannot be replaced
/// between THIS preflight and any later `run`/`kill`/`delete` — a real, non-symlinked, root-owned,
/// non-group/other-writable regular file, whose FULL ancestor chain is neither owned nor writable
/// by this process (Sol's review, round 5: the version+digest pin alone only proves what WAS true
/// AT preflight time via a path-based open+hash — a runner-writable binary, or a runner-writable
/// ANCESTOR of it, could still be replaced before or between any later invocation, which would
/// silently execute the replacement despite the installed policy claiming a validated binary).
/// Reuses the exact same ancestor-walk [`crate::user_namespace`]'s leases directory relies on.
fn harden_explicit_userns_runsc_binary(bin: &Path) -> Result<(), String> {
    if !bin.is_absolute() {
        return Err(format!("{bin:?} must be an absolute path"));
    }
    let meta = std::fs::symlink_metadata(bin).map_err(|e| format!("stat {bin:?}: {e}"))?;
    if meta.file_type().is_symlink() {
        return Err(format!("{bin:?} must not be a symlink"));
    }
    if !meta.is_file() {
        return Err(format!("{bin:?} must be a regular file"));
    }
    if meta.uid() != 0 {
        return Err(format!(
            "{bin:?} must be owned by root (uid 0), got uid {}",
            meta.uid()
        ));
    }
    if meta.mode() & 0o022 != 0 {
        return Err(format!(
            "{bin:?} must not be group/other-writable (mode {:o})",
            meta.mode() & 0o777
        ));
    }
    crate::dirlock::verify_ancestors_not_writable_by_us(bin)
        .map_err(|reason| format!("{bin:?}'s ancestor chain is not safely anchored: {reason}"))
}

/// This module's own hardening policy for the `runsc` explicit-userns state root. Sol's review,
/// round 6: the earlier version auto-created the leaf with `create_dir_all` before checking
/// anything — which is INTERNALLY CONTRADICTORY with the ancestor-writability requirement below,
/// since creating a missing leaf REQUIRES write access to its parent, meaning "auto-create
/// succeeded" and "the parent chain is safely non-writable by us" can never BOTH be true at once.
/// It also meant a FAILED preflight could still leave a freshly-created directory behind as a side
/// effect. Fixed by performing NO MUTATION at all: verifies the ancestor chain FIRST (so an unsafe
/// deployment is rejected before even looking at the leaf), then requires the leaf to ALREADY
/// EXIST as a real (non-symlink) directory, owned by this process's own euid, with a private mode
/// (`0700` or stricter) — pre-provisioning the leaf is now the CALLER's responsibility (a real
/// deployment's install step, or a test fixture), exactly mirroring the split
/// [`crate::user_namespace`]'s own leases-directory hardening now uses between its strict
/// production path and non-strict test setup.
fn harden_explicit_userns_runsc_root(dir: &Path) -> Result<(), String> {
    if !dir.is_absolute() {
        return Err(format!("{dir:?} must be an absolute path"));
    }
    crate::dirlock::verify_ancestors_not_writable_by_us(dir)
        .map_err(|reason| format!("{dir:?}'s ancestor chain is not safely anchored: {reason}"))?;
    verify_explicit_userns_runsc_root_leaf(dir)
}

/// The LEAF-only checks [`harden_explicit_userns_runsc_root`] applies, pulled out into its own
/// function so a test can exercise them directly against a fixture whose ANCESTORS are not
/// necessarily hardened (the full function's own ancestor check would otherwise refuse first
/// against any fixture a non-privileged test creates under a writable temp directory, proving
/// nothing about the leaf-specific checks this function targets).
fn verify_explicit_userns_runsc_root_leaf(dir: &Path) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(dir).map_err(|e| {
        format!(
            "stat {dir:?}: {e} — the explicit-userns runsc state root must be pre-provisioned; \
             this preflight does not create it"
        )
    })?;
    if meta.file_type().is_symlink() {
        return Err(format!("{dir:?} must not be a symlink"));
    }
    if !meta.is_dir() {
        return Err(format!("{dir:?} must be a directory"));
    }
    let our_uid = unsafe { libc::geteuid() };
    if meta.uid() != our_uid {
        return Err(format!(
            "{dir:?} is owned by uid {} (expected this process's own euid {our_uid})",
            meta.uid()
        ));
    }
    if meta.mode() & 0o077 != 0 {
        return Err(format!(
            "{dir:?} mode {:o} is group/other-accessible — expected 0700 or stricter",
            meta.mode() & 0o777
        ));
    }
    // Sol's review, round 7: rejecting group/other bits alone still admits `0500`/`0000` — modes
    // this process itself could never actually write into (state-marker creation) or search
    // through. The owner must retain full `rwx`.
    if meta.mode() & 0o700 != 0o700 {
        return Err(format!(
            "{dir:?} mode {:o} does not grant this process's own owner bits full rwx — required \
             to create/search state under it",
            meta.mode() & 0o777
        ));
    }
    Ok(())
}

/// Validate `helper_dir` (via [`preflight_explicit_userns_helpers`]), harden+validate `runsc_root`
/// (via [`harden_explicit_userns_runsc_root`]), and validate the currently-resolved `runsc` binary
/// ([`runsc_bin`]) both against the exact pinned release+digest this contract was accepted against
/// ([`verify_pinned_explicit_userns_runsc`]) AND against replacement
/// ([`harden_explicit_userns_runsc_binary`]) — then atomically install all of it as the ONE
/// [`ResolvedExplicitUsernsPolicy`] [`apply_runsc_invocation_policy`]'s `ExplicitUserNamespace`
/// branch will use for the rest of this process's lifetime. Never called automatically; a caller
/// enabling `ExplicitUserNamespace` mode in production calls this once at startup —
/// `apply_runsc_invocation_policy` REFUSES that mode outright (rather than falling back to ad hoc
/// unvalidated resolution) if this was never called or never succeeded.
pub fn preflight_explicit_userns_policy(
    helper_dir: &Path,
    runsc_root: &Path,
) -> Result<(), String> {
    let bin = runsc_bin();
    // Order matters (Sol's review, round 6): harden the PATHNAME first (no execution at all in
    // this step) so the file cannot be swapped out from under us — only THEN does
    // `verify_pinned_explicit_userns_runsc` hash and (only on a matching digest) execute it.
    harden_explicit_userns_runsc_binary(bin)?;
    verify_pinned_explicit_userns_runsc(bin)?;
    preflight_explicit_userns_helpers(helper_dir)?;
    harden_explicit_userns_runsc_root(runsc_root)?;
    let policy = ResolvedExplicitUsernsPolicy {
        helper_dir: helper_dir.to_path_buf(),
        runsc_root: runsc_root.to_path_buf(),
    };
    if EXPLICIT_USERNS_POLICY.set(policy.clone()).is_err() {
        let already = EXPLICIT_USERNS_POLICY
            .get()
            .expect("set() just failed, so the cell must already be initialized");
        if already != &policy {
            return Err(format!(
                "explicit-userns policy already installed as {already:?}, which disagrees with \
                 this preflight's {policy:?} — refusing rather than leaving some callers on a \
                 stale policy"
            ));
        }
    }
    Ok(())
}

/// Apply the COMPLETE `runsc` invocation policy for `mode` to `cmd` — the ONE place `run`/`kill`/
/// `delete` decide BOTH the global flags AND the environment, so no call site makes an
/// independent decision (Sol's review). `Rootless` is BYTE-IDENTICAL to the pre-slice-2 behavior
/// (only `--rootless`; the inherited environment is untouched). `ExplicitUserNamespace` REFUSES
/// outright unless [`preflight_explicit_userns_policy`] has already succeeded (Sol's review, round
/// 4: binding validation to use, not merely resolving a value that happens to usually agree with
/// what was validated) — otherwise drops `--rootless`, adds `-ignore-cgroups` and an absolute
/// `--root=<state-root>` (never depending on `$XDG_RUNTIME_DIR`), clears the ENTIRE inherited
/// environment, and sets `PATH` to name ONLY the trusted helper directory `runsc` resolves
/// `newuidmap`/`newgidmap` through internally (per the live spike + gVisor's own docs: OCI-native
/// multi-ID mappings make `runsc` itself invoke these helpers — this process never does — so the
/// only lever we have is WHERE `runsc`'s own lookup can find them).
fn apply_runsc_invocation_policy(
    cmd: &mut Command,
    mode: RunscInvocationMode,
) -> Result<(), String> {
    apply_runsc_invocation_policy_given(cmd, mode, EXPLICIT_USERNS_POLICY.get())
}

/// The actual decision logic behind [`apply_runsc_invocation_policy`], taking the installed policy
/// as an EXPLICIT `Option` parameter rather than reading the process-global `OnceLock` itself. Pulled
/// out so a test can deterministically prove the "no policy installed yet" refusal by passing `None`
/// directly — reading the real global would be ordering-dependent (once ANY test in the same test
/// binary's process installs a policy, it stays installed for every other test sharing that
/// process; Sol's review, round 5, flagged the previous test's silent skip-on-wrong-ordering as
/// non-deterministic).
fn apply_runsc_invocation_policy_given(
    cmd: &mut Command,
    mode: RunscInvocationMode,
    policy: Option<&ResolvedExplicitUsernsPolicy>,
) -> Result<(), String> {
    match mode {
        RunscInvocationMode::Rootless => {
            cmd.arg("--rootless");
            Ok(())
        }
        RunscInvocationMode::ExplicitUserNamespace(_) => {
            let policy = policy.ok_or_else(|| {
                "ExplicitUserNamespace mode requires preflight_explicit_userns_policy to have \
                 succeeded first — refusing rather than falling back to unvalidated resolution"
                    .to_string()
            })?;
            apply_explicit_userns_env(cmd, policy);
            Ok(())
        }
    }
}

/// The pure `Command` mutation `ExplicitUserNamespace` mode applies, GIVEN an already-validated
/// [`ResolvedExplicitUsernsPolicy`]. Factored out of [`apply_runsc_invocation_policy`] so a test can
/// exercise this mechanism directly against a hand-built policy value, without depending on the
/// process-global [`EXPLICIT_USERNS_POLICY`] `OnceLock` (which, once set by any test in the same
/// test binary, stays set for every other test sharing that process) or on a real pinned `runsc`
/// binary being present to satisfy [`preflight_explicit_userns_policy`]'s digest check.
fn apply_explicit_userns_env(cmd: &mut Command, policy: &ResolvedExplicitUsernsPolicy) {
    cmd.arg("-ignore-cgroups");
    cmd.arg(format!("--root={}", policy.runsc_root.display()));
    cmd.env_clear();
    cmd.env("PATH", &policy.helper_dir);
}

/// Best-effort idempotent container delete (`runsc <mode's global args> delete -force <cid>`).
/// Deleting an already-gone container is a harmless no-op — called on EVERY teardown path so no
/// container leaks. `mode` MUST be the same one the container was launched with (CT-007 slice 2:
/// [`SpawnedRunsc`] carries it alongside `bin`/`container_id` for exactly this reason). If the
/// invocation policy can't be applied (only possible if `ExplicitUserNamespace`'s policy was
/// somehow never validated, which the ORIGINAL launch that created this container already
/// required — practically unreachable), this is a silent no-op: there is nothing safe to delete
/// with, and this path is best-effort cleanup already, never the sole source of truth for container
/// lifecycle.
fn delete_container(bin: &Path, container_id: &str, mode: RunscInvocationMode) {
    let mut cmd = Command::new(bin);
    if apply_runsc_invocation_policy(&mut cmd, mode).is_err() {
        return;
    }
    let _ = cmd.arg("delete").arg("-force").arg(container_id).output();
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
    /// Callback/read/cancellation failure observed after the container started.
    stream_error: Option<String>,
}

/// A phase-tagged [`run_and_capture`] failure — CT-007 vertical-slice step 2b's gvisor.rs-
/// integration planning found a real, pre-existing gap this closes: `launch_with` used to
/// propagate a bare `String` from this function and return early WITHOUT ever calling
/// `hooks.release_unused` or `hooks.settle_completed`, silently leaking the job's cost/capacity
/// reservation on every failure path — regardless of whether the sandbox had actually spawned
/// and consumed real host resources by the time it failed. The four variants (mirroring
/// [`crate::launch_gate::SpawnPhase`]'s own four-way distinction from the durable-launch-commit
/// boundary) tell the caller exactly which of `release_unused` / `DurableOutcomeUnknown` /
/// `settle_completed(zero)` / `settle_completed(usage)` is correct — `Executed` makes its usage
/// mandatory (not an `Option`), so an "executed but no usage was ever computed" state cannot be
/// constructed at all. This prevents a MISSING usage, not a zero one — a caller can still
/// construct `Executed` with a genuinely zero `ResourceUsage`; every real call site computes it via
/// `executed_fallback_usage`, which floors elapsed wall-time at 1 second precisely so that never
/// happens in practice.
#[derive(Debug)]
enum RunFailure {
    /// No durable launch CAS committed. Safe to release the reservation at zero cost.
    Uncommitted { message: String },
    /// The durable launch CAS returned an error, but whether it actually committed durably is
    /// UNKNOWN. Neither release nor settle is safe; the caller must defer to reconciliation.
    CommitOutcomeUnknown { message: String },
    /// The durable launch CAS committed, but the runtime never got to exec. The reservation's
    /// cost is real (the commit itself is durably accounted) even though no workload ran.
    CommittedButNotExecuted { message: String },
    /// The runtime was released to exec (or genuinely started, for an unfenced spawn) by the
    /// time this failure occurred. `usage` is the conservative fallback accounting — mandatory
    /// (an "executed but no usage was ever computed" state cannot be constructed), computed by
    /// every real call site via `executed_fallback_usage`'s 1-second wall-time floor so a job
    /// engineered to fail exactly after exec cannot run for free in practice.
    Executed {
        message: String,
        usage: ResourceUsage,
    },
}

impl RunFailure {
    fn uncommitted(message: impl Into<String>) -> Self {
        Self::Uncommitted {
            message: message.into(),
        }
    }

    fn commit_outcome_unknown(message: impl Into<String>) -> Self {
        Self::CommitOutcomeUnknown {
            message: message.into(),
        }
    }

    fn committed_but_not_executed(message: impl Into<String>) -> Self {
        Self::CommittedButNotExecuted {
            message: message.into(),
        }
    }

    fn executed(message: impl Into<String>, usage: ResourceUsage) -> Self {
        Self::Executed {
            message: message.into(),
            usage,
        }
    }
}

impl std::fmt::Display for RunFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunFailure::Uncommitted { message }
            | RunFailure::CommitOutcomeUnknown { message }
            | RunFailure::CommittedButNotExecuted { message }
            | RunFailure::Executed { message, .. } => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for RunFailure {}

/// Spawn the REAL `runsc` container (`runsc --rootless --network=none run -bundle <dir> <cid>`) — THE
/// one legitimate runtime-spawn site (the `no-host-exec` named exclusion; the mechanism that CREATES
/// the userspace-kernel boundary, not a bypass) — drain its stdout/stderr on dedicated threads (so a
/// chatty container cannot fill a pipe buffer and deadlock), and wait at most `timeout`. On expiry the
/// WHOLE CONTAINER is killed (`runsc kill <cid> KILL` then the child) and `timed_out` is set. The exit
/// code is the `runsc` child's REAL `ExitStatus.code()` — the container process's actual exit, never
/// parsed from container output (the structural reason gVisor needs no forge defense).
#[allow(clippy::too_many_arguments)]
fn run_and_capture(
    bin: &Path,
    bundle: &Path,
    container_id: &str,
    timeout: Duration,
    mem_bytes: u64,
    options: RunCaptureOptions<'_>,
    launch_permit: Option<LaunchPermit>,
    mode: RunscInvocationMode,
) -> Result<RunscOutcome, RunFailure> {
    let RunCaptureOptions {
        stdin,
        stdout_mode,
        cancellation,
        output,
    } = options;
    let has_streaming_output = output.is_some();
    // CT-003b (SI-017): establish the OUT-OF-BAND memory cgroup BEFORE spawning runsc and FAIL
    // CLOSED if it cannot be established (rootless runsc would otherwise run the workload's anonymous
    // memory UNBOUNDED — a host-DoS escape). The cgroup is torn down on every path (its `Drop`).
    // Every error up to (and including) `sandbox_command.spawn()` itself is Uncommitted-or-typed:
    // nothing durable happened yet, so a caller-side `release_unused` is correct.
    let cgroup = MemoryCgroup::create(mem_bytes).map_err(RunFailure::uncommitted)?;

    let watchdog_timeout = launch_permit.as_ref().map(|_| timeout);
    let mut sandbox_command = SandboxCommand::new(bin, launch_permit, watchdog_timeout)
        .map_err(|error| RunFailure::uncommitted(format!("prepare runsc launch gate: {error}")))?;
    let fenced = sandbox_command.is_fenced();
    {
        let cmd = sandbox_command.command_mut();
        apply_runsc_invocation_policy(cmd, mode).map_err(RunFailure::uncommitted)?;
        cmd.arg("--network=none")
            .arg("run")
            .arg("-bundle")
            .arg(bundle)
            .arg(container_id)
            // CT-006a: the git-wire path pipes the stateless-rpc request body in; CI/agent jobs get no
            // stdin (`null`). The bytes are already bounded by [`WIRE_STDIN_BOUND`] before we get here.
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if !fenced {
            cmd.stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            });
        }
        // Place the runsc child (and the sentry/gofer tree it forks) into the memory cgroup at birth.
        cgroup.place_child(cmd).map_err(|e| {
            RunFailure::uncommitted(format!("bind runsc into the memory cgroup: {e}"))
        })?;
    }
    if fenced {
        let kill_file = cgroup.kill_file().map_err(|error| {
            RunFailure::uncommitted(format!("open runsc cgroup kill switch: {error}"))
        })?;
        sandbox_command.kill_cgroup_on_liveness_loss(kill_file);
    }
    let mut child = sandbox_command.spawn().map_err(|spawn_failure| {
        let message = format!("spawn runsc: {}", spawn_failure.message());
        match spawn_failure.phase() {
            SpawnPhase::Uncommitted => RunFailure::uncommitted(message),
            SpawnPhase::CommitOutcomeUnknown => RunFailure::commit_outcome_unknown(message),
            SpawnPhase::CommittedButNotExecuted => RunFailure::committed_but_not_executed(message),
            SpawnPhase::Executed => {
                let elapsed = spawn_failure
                    .executed_at()
                    .expect("Executed phase always carries executed_at")
                    .elapsed();
                RunFailure::executed(message, executed_fallback_usage(mem_bytes, elapsed, None))
            }
        }
    })?;

    // From here on, `sandbox_command.spawn()` has succeeded — per its own contract, the runtime
    // has ALREADY been released to exec (the fenced case's gate release + ownership.release()
    // both succeeded before `Ok(SandboxChild)` is ever returned). Every failure below is
    // therefore `Executed`: the sandbox may already be consuming real host resources, and must
    // never be charged zero. `executed_at` is the TRUE release/spawn moment `SandboxChild`
    // captured (immediately before the gate write for a fenced launch, immediately before
    // `Command::spawn` for an unfenced one) — never a LATER `Instant::now()` taken here, which
    // would silently exclude real gate-release/pipe-setup execution time from every fallback
    // computation below, AND from the timeout deadline and successful `wall` duration further on.
    let executed_at = child.executed_at();

    // CT-006a: feed the bounded request body to the container's stdin on a DEDICATED thread (so a
    // large body + a slow in-guest reader cannot deadlock against our stdout/stderr drains), then drop
    // the handle to deliver EOF (the stateless-rpc request terminator). None ⇒ stdin was `null`.
    let stdin_pipe = if stdin.is_some() {
        child.stdin().take()
    } else {
        None
    };
    if stdin.is_some() && stdin_pipe.is_none() {
        child.kill_and_wait();
        return Err(RunFailure::executed(
            "runsc stdin pipe unavailable",
            executed_fallback_usage(mem_bytes, executed_at.elapsed(), None),
        ));
    }
    let stdin_th = stdin.zip(stdin_pipe).map(|(bytes, mut si)| {
        std::thread::spawn(move || {
            let result = si.write_all(&bytes);
            // `si` drops here ⇒ the write end closes ⇒ the guest `git` sees EOF on its request body.
            result
        })
    });

    let pid = child.id();

    // Drain both pipes on threads so a chatty container cannot fill a pipe buffer and deadlock.
    let (Some(mut out), Some(mut err)) = (child.stdout().take(), child.stderr().take()) else {
        child.kill_and_wait();
        if let Some(t) = stdin_th {
            let _ = t.join();
        }
        return Err(RunFailure::executed(
            "runsc output pipe unavailable",
            executed_fallback_usage(mem_bytes, executed_at.elapsed(), None),
        ));
    };
    // stdout draining depends on the stream's mode (CT-006c):
    //   - `CappedHead` (CI/agent logs): cap at SANDBOX_CAPTURE_BOUND (256 KiB head capture) + DISCARD the
    //     rest to EOF — bounds host memory under a runaway container, byte-unchanged from CT-002c.
    //   - `StreamToFile` (the git wire): stream straight to a host TEMP FILE under a GENEROUS cap so a
    //     real-size packfile (megabytes, not 256 KiB) comes through WHOLE while host RAM stays one chunk
    //     (the bytes land on disk, not in a growing Vec). Over the generous cap ⇒ `truncated` (the wire
    //     seam then REFUSES loudly — never a silently-truncated pack). Both keep reading past the bound
    //     so the container never blocks on a full pipe (no deadlock that would defeat the timeout).
    let stdout_output = output.clone();
    let th_out = std::thread::spawn(move || match (stdout_mode, stdout_output) {
        (StdoutMode::CappedHead, Some(output)) => drain_capped_streaming(
            &mut out,
            SANDBOX_CAPTURE_BOUND,
            SandboxOutputStream::Stdout,
            &output,
        ),
        (StdoutMode::CappedHead, None) => {
            let (head, truncated) = drain_capped(&mut out, SANDBOX_CAPTURE_BOUND);
            (head, truncated, None)
        }
        (StdoutMode::StreamToFile { bound }, _) => {
            let (head, truncated) = drain_to_temp_file(&mut out, bound);
            (head, truncated, None)
        }
    });
    // stderr is ALWAYS the 256 KiB head bound — it is error text folded into a message, never payload.
    let th_err = std::thread::spawn(move || match output {
        Some(output) => {
            let (head, _, error) = drain_capped_streaming(
                &mut err,
                SANDBOX_CAPTURE_BOUND,
                SandboxOutputStream::Stderr,
                &output,
            );
            (head, error)
        }
        None => (drain_capped(&mut err, SANDBOX_CAPTURE_BOUND).0, None),
    });

    let timed_out;
    let mut last_cpu: Option<u64> = None;
    let exit = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                timed_out = child.watchdog_deadline_expired();
                break status.code();
            }
            Ok(None) => {}
            Err(error) => {
                // Kill/reap FIRST so the pipes hit EOF, THEN join every thread that was reading
                // them — no thread may outlive this run, even on a wait-syscall failure (a bare
                // early return here would leak the stdin/stdout/stderr threads, still blocked on
                // pipes from a child nothing ever killed).
                child.kill_and_wait();
                let _ = th_out.join();
                let _ = th_err.join();
                if let Some(t) = stdin_th {
                    let _ = t.join();
                }
                return Err(RunFailure::executed(
                    format!("wait runsc: {error}"),
                    executed_fallback_usage(mem_bytes, executed_at.elapsed(), last_cpu),
                ));
            }
        }
        if let Some(c) = read_proc_cpu_seconds(pid) {
            last_cpu = Some(c);
        }
        let cancelled = cancellation.load(Ordering::Acquire);
        if cancelled || executed_at.elapsed() >= timeout {
            // Wall-clock ceiling hit: whole-CONTAINER kill (SIGKILL the container's PID1 via the
            // runtime), then reap the `runsc` child process so the pipes hit EOF.
            // The SAME `mode` already passed `apply_runsc_invocation_policy` once, at this exact
            // container's spawn earlier in this function call — the policy this reads from is a
            // process-lifetime `OnceLock` that cannot have changed since, so failure here is not
            // reachable in practice. Best-effort regardless: if it somehow did fail, skip the
            // `runsc kill` (nothing safe to send it with) but still reap the host-side child below.
            let mut kill_cmd = Command::new(bin);
            if apply_runsc_invocation_policy(&mut kill_cmd, mode).is_ok() {
                let _ = kill_cmd.arg("kill").arg(container_id).arg("KILL").output();
            }
            child.kill_and_wait();
            timed_out = !cancelled;
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let wall = executed_at.elapsed();

    // The child has exited/been-killed ⇒ the pipes hit EOF ⇒ the drain threads finish.
    let stdout_result = th_out.join();
    let stderr_result = th_err.join();
    // The writer thread has finished (the child read its request body, or it exited and the write
    // EPIPE'd — either way `write_all` returned). Join so no thread outlives the run.
    let stdin_result = stdin_th.map(std::thread::JoinHandle::join);

    let (stdout, stdout_truncated, stdout_error) = stdout_result.map_err(|_| {
        RunFailure::executed(
            "runsc stdout drain thread panicked",
            executed_fallback_usage(mem_bytes, wall, last_cpu),
        )
    })?;
    let (stderr, stderr_error) = stderr_result.map_err(|_| {
        RunFailure::executed(
            "runsc stderr drain thread panicked",
            executed_fallback_usage(mem_bytes, wall, last_cpu),
        )
    })?;
    if let Some(write_result) = stdin_result {
        let write_result = write_result.map_err(|_| {
            RunFailure::executed(
                "runsc stdin writer thread panicked",
                executed_fallback_usage(mem_bytes, wall, last_cpu),
            )
        })?;
        // A child that failed or was killed may legitimately close stdin early; its primary outcome
        // remains authoritative. A successful child, however, must have received the full bounded
        // request body or the Git wire exchange is incomplete.
        if exit == Some(0) {
            write_result.map_err(|e| {
                RunFailure::executed(
                    format!("write runsc stdin: {e}"),
                    executed_fallback_usage(mem_bytes, wall, last_cpu),
                )
            })?;
        }
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
        stream_error: stdout_error.or(stderr_error).or_else(|| {
            (has_streaming_output && cancellation.load(Ordering::Acquire))
                .then(|| "sandbox execution cancelled by durable log consumer".into())
        }),
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
    output: Option<StreamingOutput>,
}

#[derive(Clone)]
struct StreamingOutput {
    sink: Arc<dyn SandboxOutputSink>,
    redaction: RedactionPlan,
}

/// Drain a complete guest stream while retaining only its bounded diagnostic head and forwarding
/// every bounded chunk to the durable-output callback.
///
/// A callback failure is remembered, but the pipe is still drained to EOF so the guest cannot
/// deadlock behind a full pipe and defeat timeout/teardown. Redaction is applied before the callback.
/// Today every plan is empty because secret injection is absent; CI-1 owns the already-documented
/// cross-chunk streaming masker obligation when it introduces real needles.
fn drain_capped_streaming<R: Read>(
    mut reader: R,
    limit: usize,
    stream: SandboxOutputStream,
    output: &StreamingOutput,
) -> (Vec<u8>, bool, Option<String>) {
    let mut head = Vec::new();
    let mut truncated = false;
    let mut first_output_error = None;
    let mut chunk = [0u8; 64 * 1024];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if head.len() < limit {
                    let take = (limit - head.len()).min(n);
                    head.extend_from_slice(&chunk[..take]);
                    truncated |= take < n;
                } else {
                    truncated = true;
                }
                if first_output_error.is_none() {
                    let redacted = output.redaction.redact(&chunk[..n]);
                    if let Err(error) = output.sink.emit(stream, &redacted) {
                        first_output_error = Some(error);
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                truncated = true;
                if first_output_error.is_none() {
                    first_output_error = Some(format!("read guest output: {error}"));
                }
                break;
            }
        }
    }
    (head, truncated, first_output_error)
}

/// **Drain a child stream straight to a host TEMP FILE under a generous byte cap (the git-wire path,
/// CT-006c).** Host MEMORY stays bounded to ONE 64 KiB chunk regardless of how large the packfile is —
/// the bytes are written to disk as they arrive, NOT buffered in a growing Vec. Keeps reading past the
/// cap (draining + discarding) so the container never blocks on a full pipe (no deadlock that would
/// defeat the timeout). Returns the materialized head (≤ `cap` bytes, read back from the temp file) and
/// whether the cap was exceeded or the stream could not be read through EOF (`truncated`). The temp
/// file is removed before returning (no leak).
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
            Err(_) => {
                // Preserve the diagnostic prefix, but force the wire seam to reject it as incomplete.
                truncated = true;
                break;
            }
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

/// Fallback usage for a [`RunFailure`] in the `Executed` phase (CT-007 vertical-slice step 2b's
/// gvisor.rs-integration planning, Sol's review): the runtime had already been released to exec
/// by the time the failure occurred, so it must never be charged zero (that would let a spawn
/// that ran real, if briefly, execute for free — a host-DoS surface via jobs engineered to fail
/// exactly this way). Deliberately similar to, but NOT reusing, [`build_result`]'s own formula:
/// this rounds elapsed time UP TO AT LEAST ONE SECOND (a `.max(1)` floor `build_result`'s own,
/// separately-tested success-path formula does not have and must not silently gain) — the
/// distinct floor Sol's review specified for a conservative FAILURE estimate, not a change to
/// already-tested successful-completion accounting.
fn executed_fallback_usage(
    mem_bytes: u64,
    elapsed: Duration,
    cpu_seconds: Option<u64>,
) -> ResourceUsage {
    let wall_secs_ceil = (elapsed.as_secs() + u64::from(elapsed.subsec_nanos() > 0)).max(1);
    let cpu_seconds = cpu_seconds.filter(|c| *c > 0).unwrap_or(wall_secs_ceil);
    ResourceUsage {
        cpu_seconds,
        mem_byte_seconds: mem_bytes.saturating_mul(wall_secs_ceil),
    }
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
    bin: &'static Path,
    container_id: String,
    /// The SAME mode this container was launched with (CT-007 slice 2) — `kill`'s `delete`
    /// invocation MUST use identical global flags to the original `run`, or `runsc` may fail to
    /// locate/manage a container whose namespace/cgroup posture it was never told to expect.
    mode: RunscInvocationMode,
}
impl RunscChild for SpawnedRunsc {
    fn kill(&mut self) -> Result<(), String> {
        delete_container(self.bin, &self.container_id, self.mode);
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

/// The canonical-tree sha256 of the STAGED base/`linux-small-v1` rootfs [`resolved_gvisor_rootfs`]
/// resolves by default — the SAME digest `.myelin/ci.toml` already pins for the founder-dogfood
/// pipeline's `myelin.local/linux-small-v1-rootfs` image (`scripts/dogfood.sh`'s `verify_ci_rootfs`
/// asserts this on that ONE file; kept here as a single Rust-side source of truth for composition
/// roots/tests that need to build a real [`crate::asset_registry::GvisorAssetRegistry`] entry for
/// it, rather than re-typing the hex string at every call site).
pub const LINUX_SMALL_V1_ROOTFS_SHA256: &str =
    "f9bd3926a7b47e1dd4729e5788d40dc6daf4ce159a91db169ef5bb803e73ec1f";

/// The canonical-tree sha256 of the STAGED `linux-rust-v1` rootfs [`resolved_gvisor_rust_rootfs`]
/// resolves by default — the SAME digest committed in `runner-assets.toml`'s `linux-rust-v1` row.
pub const LINUX_RUST_V1_ROOTFS_SHA256: &str =
    "6feada1e0ef7b739d71c7f198b03dcaab494f35ea86182dd887d23f5df0c6083";

/// Env var naming the staged Rust-capable gVisor rootfs (mirrors `runner-assets.toml`'s
/// `linux-rust-v1` row: `env_var = "MYELIN_GVISOR_RUST_ROOTFS"`).
pub const ENV_GVISOR_RUST_ROOTFS: &str = "MYELIN_GVISOR_RUST_ROOTFS";

/// The resolved Rust-capable rootfs path (env override → `~/.local/share/gvisor-assets/rust-rootfs`,
/// `runner-assets.toml`'s `linux-rust-v1` row `default_path`). SEPARATE from
/// [`resolved_gvisor_rootfs`] because this asset carries a real Rust toolchain the plain
/// busybox-class base rootfs does not; nothing dispatches jobs against it by default today — only
/// the registry entry the CT-007 gate-2 composition root registers, and the rust-capability
/// prod-exec self-test.
pub fn resolved_gvisor_rust_rootfs() -> std::path::PathBuf {
    if let Ok(p) = std::env::var(ENV_GVISOR_RUST_ROOTFS) {
        return std::path::PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    std::path::PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("gvisor-assets")
        .join("rust-rootfs")
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

# ---- fork bomb: the OCI bundle sets both linux.resources.pids.limit and RLIMIT_NPROC={pids_max}.
#      Run the saturation loop in a child shell: BusyBox exits that child with status 2 when the
#      rlimit rejects fork, while the corpus PID 1 survives to reap children and continue D1/D2/Mx. ----
F1_COUNT=/tmp/myelin-f1-count
: > "$F1_COUNT"
(
  children=""
  cleanup_f1() {{ for child in $children; do kill "$child" 2>/dev/null || true; done; wait 2>/dev/null || true; }}
  trap cleanup_f1 EXIT
  n=0; admitted=0
  while [ $n -lt 300 ]; do
    /bin/sleep 30 &
    child=$!
    children="$children $child"
    admitted=$((admitted+1))
    echo "$admitted" > "$F1_COUNT"
    n=$((n+1))
  done
  if [ $admitted -le {pids_max} ]; then exit 0; else exit 42; fi
) 2>/dev/null
f1_status=$?
admitted=0
[ -r "$F1_COUNT" ] && read admitted < "$F1_COUNT"
rm -f "$F1_COUNT" 2>/dev/null || true
if [ "$f1_status" -eq 42 ] || [ "$admitted" -gt {pids_max} ]; then echo "F1_forkbomb ESCAPED admitted=$admitted ceiling={pids_max} status=$f1_status"; elif [ "$f1_status" -eq 0 ] || [ "$f1_status" -eq 2 ]; then echo "F1_forkbomb CONTAINED survived=yes admitted=$admitted ceiling={pids_max} status=$f1_status"; else echo "F1_forkbomb ESCAPED unexpected_probe_status=$f1_status admitted=$admitted ceiling={pids_max}"; fi

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
    "rlimits": [{{ "type": "RLIMIT_NPROC", "hard": {pids}, "soft": {pids} }}],
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
    run_token: RunTokenCredential,
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
        run_token: RunTokenCredential,
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
    /// reserve → final attribution → run → settle), with the git-wire additions: the bare repo is bound
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

        // The git wire is a direct synchronous API call (an HTTP-shaped request/response), NOT a
        // `RunnerAgent`-mediated job with a terminal reporter parked above it. There is no
        // `report_retryable_attempt` mechanism this path can route a post-commit failure through —
        // refuse loudly here, before reserve, rather than silently mis-defer a real measured attempt
        // the way `TerminalReporter` ownership would (Sol's finding: git-wire must require `Hook`
        // ownership, unconditionally).
        if hooks.completion_settlement_owner() == CompletionSettlementOwner::TerminalReporter {
            return Err(WireError::Runtime(
                "git-wire launch requires Hook-owned completion settlement — it is a direct \
                 synchronous path with no terminal reporter above it to defer a retryable-attempt \
                 accounting to"
                    .to_string(),
            ));
        }

        // Derive the isolation/config posture before the final mutable launch boundary.
        hooks.enforce_isolation_floor(&job)?;
        let profile = HardeningProfile::derive(&job);
        profile.assert_enforced().map_err(WireError::Hardening)?;

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
        let reserve = hooks.reserve(&job)?;
        // Thread the REAL launch permit through to `run_and_capture` (Sol's fix): the old
        // `hooks.attribute(&job)` eagerly committed-and-released attribution HERE, before any
        // spawn attempt — decoupling the durable commit from the actual OS spawn entirely, which
        // made every subsequent `RunFailure` phase from `run_git_wire_container` structurally
        // mislabeled (e.g. a post-exec pipe failure would be reported `Uncommitted` even though
        // attribution had already, durably, committed). Acquiring (not immediately committing) the
        // permit and passing it into `run_and_capture` makes the SAME durable-launch-commit gate
        // that the CI/agent path uses also govern the git-wire spawn, so its phase reporting
        // becomes truthful.
        let launch_permit = match hooks.acquire_launch_permit(&job) {
            Ok(permit) => permit,
            Err(attribute_error) => {
                hooks.release_unused(&job, &reserve)?;
                return Err(attribute_error.into());
            }
        };

        // Same pre-existing leak, same fix, as `launch_with` above: `run_git_wire_container`'s
        // failure carries the phase the launch reached, so the reservation is released/settled
        // correctly instead of leaking on every failure path. Git-wire has no terminal reporter
        // above it (refused up front, above) so every post-commit phase settles synchronously here
        // — there is no `RetryableAttempt`/reporter path to defer to.
        let (
            ContainerRun {
                child,
                bundle_dir,
                result,
                run_error,
            },
            stdout_truncated,
        ) = match run_git_wire_container(
            &job,
            &cfg,
            spec.stdin.clone(),
            &rootfs,
            cancellation,
            launch_permit,
        ) {
            Ok(run_and_truncated) => run_and_truncated,
            Err(run_failure) => {
                return Err(dispose_git_wire_run_failure(
                    hooks,
                    &job,
                    &reserve,
                    run_failure,
                ))
            }
        };

        // FAIL LOUD at the seam (CT-006c FU-1): if the response overflowed the generous wire cap, the
        // captured pack is TRUNCATED — refuse rather than hand back a short pack the client's
        // `index-pack` would reject with "early EOF". A real execution genuinely happened (this is
        // deterministic for the same request/limit, never retryable), so settle its already-measured
        // usage SYNCHRONOUSLY before returning — never leave a real, completed attempt unsettled.
        // Tear down the just-run container's bundle (the container itself is already deleted by
        // `run_git_wire_container`).
        if stdout_truncated {
            let _ = std::fs::remove_dir_all(&bundle_dir);
            if let Err(settle_error) = hooks.settle_completed(&job, &reserve, result.usage) {
                return Err(WireError::Runtime(format!(
                    "response exceeded the wire cap AND settling its measured usage also failed \
                     ({settle_error}) — reservation may be leaked"
                )));
            }
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
        if let Err(error) = hooks.settle_completed(&job, &reserve, result.usage) {
            let _ = self.kill(&SandboxHandle {
                guest_id: guest_id.clone(),
            });
            return Err(error.into());
        }

        if let Some(error) = run_error {
            let _ = self.kill(&SandboxHandle { guest_id });
            return Err(WireError::Runtime(error));
        }

        Ok(SandboxLaunch {
            handle: SandboxHandle { guest_id },
            result,
            output_complete: true,
        })
    }
}

/// Dispose of a post-reserve [`RunFailure`] from `run_git_wire_container` into the correct
/// [`WireError`], settling/releasing the reservation along the way wherever that is safe.
/// `CommitOutcomeUnknown` still settles/releases NOTHING here, same as gVisor's own
/// [`GvisorBackend::dispose_run_failure`] — the durable commit outcome is genuinely unknown
/// regardless of backend. What differs is the OTHER three phases: git-wire has NO terminal
/// reporter above it (`launch_git_command` refuses reporter-owned hooks before reserve), so
/// `CommittedButNotExecuted`/`Executed` always settle synchronously here — there is no
/// `RetryableAttempt`/reporter path to ever defer to. Extracted as a standalone function (rather
/// than only inlined in `launch_git_command`) so it is unit-testable without a real `runsc` binary.
fn dispose_git_wire_run_failure(
    hooks: &RunnerHooks,
    job: &JobSpec,
    reserve: &ReserveHandle,
    run_failure: RunFailure,
) -> WireError {
    let message = run_failure.to_string();
    match run_failure {
        RunFailure::Uncommitted { .. } => {
            if let Err(settle_error) = hooks.release_unused(job, reserve) {
                return WireError::Runtime(format!(
                    "run_git_wire_container() failed (uncommitted: {message}) AND \
                     release_unused also failed ({settle_error}) — reservation may be leaked"
                ));
            }
            WireError::Runtime(message)
        }
        RunFailure::CommitOutcomeUnknown { .. } => {
            // Neither release nor settle — the durable commit outcome is genuinely unknown;
            // guessing either way misaccounts a real reservation.
            WireError::Runtime(format!(
                "durable launch commit outcome unknown, needs reconciliation: {message}"
            ))
        }
        RunFailure::CommittedButNotExecuted { .. } => {
            let zero = ResourceUsage {
                cpu_seconds: 0,
                mem_byte_seconds: 0,
            };
            if let Err(settle_error) = hooks.settle_completed(job, reserve, zero) {
                return WireError::Runtime(format!(
                    "run_git_wire_container() failed (committed but not executed: {message}) \
                     AND its zero-usage settlement also failed ({settle_error}) — reservation \
                     may be leaked"
                ));
            }
            WireError::Runtime(message)
        }
        RunFailure::Executed { usage, .. } => {
            if let Err(settle_error) = hooks.settle_completed(job, reserve, usage) {
                return WireError::Runtime(format!(
                    "run_git_wire_container() failed (executed: {message}) AND its \
                     conservative-usage settlement also failed ({settle_error}) — reservation \
                     may be leaked"
                ));
            }
            WireError::Runtime(message)
        }
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
    launch_permit: LaunchPermit,
) -> Result<(ContainerRun, bool), RunFailure> {
    let bin = runsc_bin();
    if !rootfs.exists() {
        return Err(RunFailure::uncommitted(format!(
            "staged gVisor git rootfs absent: {} (the git wire REQUIRES a real `git` in the guest — \
             stage a git-bearing rootfs and point {ENV_GVISOR_GIT_ROOTFS} at it; see \
             tests/git_wire_prod_exec_test.rs)",
            rootfs.display()
        )));
    }
    // Stage a config-only bundle: `cfg`'s `root.path` is the ABSOLUTE staged rootfs (set by
    // `launch_git_wire`), so no `rootfs` symlink is staged (a symlinked root.path + a host bind mount
    // makes the rootless gofer fail to start the sandbox; an absolute root.path + bind mount works).
    let bundle_dir = stage_git_wire_bundle(cfg).map_err(RunFailure::uncommitted)?;
    let container_id = format!("myelin-gitwire-{}-{}", std::process::id(), unique_suffix());

    let timeout = Duration::from_secs(job.limits.timeout_secs as u64);
    // The git-wire response (the packfile / advertisement) is STREAMED to a host temp file under a
    // GENEROUS cap derived from the job's `disk_bytes` scratch quota (configurable; default
    // [`WIRE_STDOUT_BOUND`] = 512 MiB) — NOT the 256 KiB CI/agent log bound — so a real-size pack comes
    // through whole while host RAM stays bounded to one chunk. Over the cap ⇒ `outcome.stdout_truncated`,
    // which the caller turns into a LOUD [`WireError::OutputTooLarge`] (never a silently-short pack).
    let wire_cap = job.limits.disk_bytes as usize;
    let mode = cfg.invocation_mode();
    let outcome = match run_and_capture(
        bin,
        &bundle_dir,
        &container_id,
        timeout,
        job.limits.mem_bytes,
        RunCaptureOptions {
            stdin: Some(stdin),
            stdout_mode: StdoutMode::StreamToFile { bound: wire_cap },
            cancellation,
            output: None,
        },
        Some(launch_permit),
        mode,
    ) {
        Ok(o) => o,
        Err(e) => {
            delete_container(bin, &container_id, mode);
            let _ = std::fs::remove_dir_all(&bundle_dir);
            return Err(e);
        }
    };
    delete_container(bin, &container_id, mode);

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
            child: Box::new(SpawnedRunsc {
                bin,
                container_id,
                mode,
            }),
            bundle_dir,
            result,
            run_error: outcome.stream_error,
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
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::RunTokenCredential;

    #[derive(Default)]
    struct RecordingOutput {
        bytes: Mutex<Vec<u8>>,
    }

    impl SandboxOutputSink for RecordingOutput {
        fn emit(&self, _stream: SandboxOutputStream, frame: &[u8]) -> Result<(), String> {
            self.bytes.lock().unwrap().extend_from_slice(frame);
            Ok(())
        }
    }

    #[test]
    fn streaming_drain_keeps_only_the_head_but_delivers_the_complete_byte_stream() {
        let input: Vec<u8> = (0..(3 * 64 * 1024 + 17))
            .map(|offset| (offset % 251) as u8)
            .collect();
        let sink = Arc::new(RecordingOutput::default());
        let output = StreamingOutput {
            sink: sink.clone(),
            redaction: RedactionPlan::none(),
        };

        let (head, truncated, error) = drain_capped_streaming(
            std::io::Cursor::new(&input),
            1024,
            SandboxOutputStream::Stdout,
            &output,
        );

        assert_eq!(error, None);
        assert!(truncated);
        assert_eq!(head, input[..1024]);
        assert_eq!(
            *sink.bytes.lock().unwrap(),
            input,
            "bytes beyond the diagnostic capture cap still reach durable output"
        );
    }

    #[test]
    fn runner_host_preflight_refuses_a_non_absolute_runtime_before_intake() {
        let error = preflight_gvisor_runner_host(Path::new("runsc"), Path::new("/unused-rootfs"))
            .expect_err("a PATH-relative runtime is not stable production authority");
        assert!(error.contains("MYELIN_RUNSC_BIN must be an absolute path"));
    }

    /// A real, on-disk, empty fixture rootfs — hashed with the SAME pure-Rust
    /// [`crate::canonical_tar::canonical_tree_sha256_hex`] the registry itself uses — so [`spec`]'s
    /// image is a GENUINELY verifiable pin, not a fabricated placeholder digest a real registry
    /// lookup could never match. Shared (same fixed path) across every test in this module — they
    /// only ever READ it (construction-time hashing happens once, in [`test_registry`]), never
    /// mutate it, so sharing across parallel test threads within this one process is safe.
    fn fixture_rootfs_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "myelin-gvisor-unit-test-fixture-rootfs-{}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    /// The digest-pinned [`ImageRef`] matching [`fixture_rootfs_dir`]'s REAL current content.
    fn fixture_image() -> ImageRef {
        let digest = crate::canonical_tar::canonical_tree_sha256_hex(&fixture_rootfs_dir())
            .expect("hash the fixture rootfs dir");
        ImageRef::pinned(format!("test.local/fixture-rootfs@sha256:{digest}")).unwrap()
    }

    /// A registry mapping [`fixture_image`] to [`fixture_rootfs_dir`] — the registry every unit test
    /// in this module that calls `launch_with`/`launch` constructs its [`GvisorBackend`] with. These
    /// tests never run a real `runsc` (they inject a fake `run` closure), so all that matters is that
    /// construction genuinely verifies (once) before the fake closure runs.
    fn test_registry() -> Arc<crate::asset_registry::GvisorAssetRegistry> {
        Arc::new(
            crate::asset_registry::GvisorAssetRegistry::from_bindings(vec![
                crate::asset_registry::RootfsAssetBinding {
                    image: fixture_image(),
                    rootfs: fixture_rootfs_dir(),
                },
            ])
            .expect("fixture binding verifies"),
        )
    }

    fn spec(allow: Vec<String>) -> JobSpec {
        JobSpec::new(
            JobKind::Agent,
            fixture_image(),
            vec!["python3".into(), "-c".into(), "print(1)".into()],
            vec![],
            vec![],
            EgressPolicy { allow },
            ResourceLimits {
                cpu_millis: 1000,
                mem_bytes: 256 << 20,
                disk_bytes: 1 << 30,
                tmpfs_bytes: 1 << 30,
                pids_max: 64,
                timeout_secs: 120,
            },
            WorkspaceSpec::default(),
            TrustTier::UntrustedFork,
            RunTokenCredential::new("test-bearer", "j", 300).unwrap(),
            MeterTarget {
                reserve_id: "r".into(),
            },
            IdemToken("idem-runsc-1".into()),
        )
        .unwrap()
    }

    fn ok_hooks() -> RunnerHooks {
        RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        )
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
            stream_error: None,
        }
    }

    // CT-004f sub-step 1: `build_result` APPLIES the redaction plan to both captured streams — the
    // boundary seam is wired, not just the `RedactionPlan` unit. A populated plan (the shape CI-1
    // injection will pass) masks the needle before it reaches `SandboxResult`.
    #[test]
    fn build_result_masks_needles_in_both_streams() {
        let s = spec(vec![]);
        // Assemble the scanner-shaped credential at runtime. Keeping the complete sentinel in this
        // source blob would make Myelin's own reject-before-promote scanner reject the repository
        // that implements and tests it.
        let needle = [b"AK".as_slice(), b"IAsecret"].concat();
        let stdout = [b"deploying with ".as_slice(), needle.as_slice(), b" now"].concat();
        let stderr = [b"error: ".as_slice(), needle.as_slice(), b" invalid"].concat();
        let plan = RedactionPlan::for_needles([needle]);
        let o = outcome(&stdout, &stderr);
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
            run_error: None,
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
        assert_eq!(
            out.len(),
            big.len(),
            "a real-size pack under the cap comes through WHOLE"
        );
        assert_eq!(
            out, big,
            "the bytes are byte-identical (no corruption via the temp file)"
        );
        assert!(!truncated, "within the cap ⇒ not truncated");

        // The SAME stream under a 64 KiB cap → head-bounded to the cap AND flagged truncated (fail-loud).
        let (head, over) = drain_to_temp_file(&big[..], 64 * 1024);
        assert_eq!(
            head.len(),
            64 * 1024,
            "over the cap ⇒ exactly the cap bytes are kept"
        );
        assert!(
            over,
            "over the cap ⇒ truncated flag set (the wire seam then refuses loudly)"
        );
    }

    #[test]
    fn drain_to_temp_file_marks_a_read_fault_as_incomplete() {
        struct FaultAfterPrefix(Option<&'static [u8]>);

        impl Read for FaultAfterPrefix {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if let Some(prefix) = self.0.take() {
                    buf[..prefix.len()].copy_from_slice(prefix);
                    Ok(prefix.len())
                } else {
                    Err(std::io::Error::other("injected wire read fault"))
                }
            }
        }

        let (head, incomplete) = drain_to_temp_file(FaultAfterPrefix(Some(b"partial-pack")), 1024);

        assert_eq!(head, b"partial-pack");
        assert!(incomplete);
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
        assert!(
            json.contains("\"type\": \"RLIMIT_NPROC\"")
                && json.contains("\"hard\": 64")
                && json.contains("\"soft\": 64"),
            "rootless gVisor gets an in-sandbox process ceiling independent of host cgroups"
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
        // tmpfs=1 GiB.
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
            "the /tmp tmpfs must be sized from spec.limits.tmpfs_bytes and writable by the non-root payload"
        );
        assert!(
            !json.contains("\"type\": \"user\"") && !json.contains("uidMappings"),
            "Rootless mode (the default) must never declare a user namespace or uid/gid mappings \
             — runsc --rootless installs its own, and a doubly-declared userns fails the gofer"
        );
        assert_eq!(
            cfg.invocation_mode(),
            RunscInvocationMode::Rootless,
            "a config with no explicit user namespace attached must report Rootless"
        );
    }

    /// CT-007 slice 2: `with_user_namespace` must produce the EXACT two-entry OCI mapping the
    /// design specifies, alongside a declared `user` namespace — and `invocation_mode()` must
    /// report `ExplicitUserNamespace` carrying the SAME config back out.
    #[test]
    fn oci_config_with_user_namespace_emits_the_exact_two_entry_mapping() {
        let config = UserNamespaceConfig::for_tests(1000, 1000, 100_005, 200_005);
        let cfg = GvisorBackend::oci_config(&spec(vec![]))
            .unwrap()
            .with_user_namespace(config);
        assert_eq!(
            cfg.invocation_mode(),
            RunscInvocationMode::ExplicitUserNamespace(config)
        );
        let json = cfg.to_json();
        assert!(
            json.contains("\"type\": \"user\""),
            "a user namespace must be declared: {json}"
        );
        assert!(
            json.contains("\"containerID\": 0, \"hostID\": 1000, \"size\": 1"),
            "container uid/gid 0 must map to the runner's own real identity: {json}"
        );
        assert!(
            json.contains("\"containerID\": 65534, \"hostID\": 100005, \"size\": 1"),
            "container uid 65534 must map to the leased subordinate host uid: {json}"
        );
        assert!(
            json.contains("\"containerID\": 65534, \"hostID\": 200005, \"size\": 1"),
            "container gid 65534 must map to the leased subordinate host gid: {json}"
        );
        // Every OTHER hardening assertion from the Rootless test must still hold — attaching a
        // user namespace changes ONLY the namespaces/mappings, nothing else.
        assert!(json.contains("\"readonly\": true"));
        assert!(json.contains("\"noNewPrivileges\": true"));
        assert!(json.contains("\"uid\": 65534") && json.contains("\"gid\": 65534"));
    }

    /// `apply_runsc_invocation_policy` is the ONE place `run`/`kill`/`delete` decide their global
    /// flags AND environment — this test is the single source of truth for `Rootless`'s exact
    /// flag-and-environment contract (Sol's review: "no independent flag decisions left" at any of
    /// the three call sites). `ExplicitUserNamespace`'s own contract is covered by
    /// `apply_explicit_userns_env_matches_the_policy_exactly` below, which exercises the pure
    /// `Command`-mutation mechanism directly against a hand-built policy — NOT through
    /// `apply_runsc_invocation_policy` itself, since that requires the process-global
    /// `EXPLICIT_USERNS_POLICY` to already be validated-and-installed (see that function's own
    /// refusal-without-preflight behavior, covered by
    /// `apply_runsc_invocation_policy_refuses_explicit_userns_without_a_validated_policy`).
    #[test]
    fn apply_runsc_invocation_policy_matches_the_mode_exactly() {
        let mut rootless_cmd = Command::new("runsc");
        apply_runsc_invocation_policy(&mut rootless_cmd, RunscInvocationMode::Rootless).unwrap();
        assert_eq!(
            rootless_cmd
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            vec!["--rootless"],
            "Rootless must be byte-identical to the pre-slice-2 flag"
        );
        assert_eq!(
            rootless_cmd.get_envs().count(),
            0,
            "Rootless must not alter the child's environment at all"
        );
    }

    /// Exercises the pure `Command`-mutation mechanism `ExplicitUserNamespace` mode applies, given
    /// an already-validated policy — independent of the process-global `EXPLICIT_USERNS_POLICY`
    /// `OnceLock` (which, once installed by any test sharing this test binary's process, cannot be
    /// reset) and independent of `preflight_explicit_userns_policy`'s pinned-runsc-digest check
    /// (which needs a real matching binary this test environment may not have).
    #[test]
    fn apply_explicit_userns_env_matches_the_policy_exactly() {
        let policy = ResolvedExplicitUsernsPolicy {
            helper_dir: PathBuf::from("/usr/bin"),
            runsc_root: PathBuf::from("/var/lib/myelin-runsc-explicit-userns"),
        };
        let mut cmd = Command::new("runsc");
        apply_explicit_userns_env(&mut cmd, &policy);
        let args = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(
            args.contains(&"-ignore-cgroups".to_string()),
            "ExplicitUserNamespace must add -ignore-cgroups: {args:?}"
        );
        assert!(
            !args.contains(&"--rootless".to_string()),
            "ExplicitUserNamespace must drop --rootless: {args:?}"
        );
        assert!(
            args.contains(&"--root=/var/lib/myelin-runsc-explicit-userns".to_string()),
            "ExplicitUserNamespace must pass the exact policy's absolute --root=: {args:?}"
        );
        let envs: Vec<_> = cmd
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert_eq!(
            envs,
            vec![("PATH".to_string(), Some("/usr/bin".to_string()))],
            "ExplicitUserNamespace must clear the environment and set PATH to ONLY the exact \
             policy's helper directory: {envs:?}"
        );
    }

    /// Sol's review, round 4/5: `ExplicitUserNamespace` mode must REFUSE outright — not fall back
    /// to ad hoc unvalidated resolution — when no policy has been validated. Calls
    /// `apply_runsc_invocation_policy_given` directly with an EXPLICIT `None`, rather than driving
    /// the real process-global `EXPLICIT_USERNS_POLICY` cell (which, once set by ANY test sharing
    /// this test binary's process — e.g. the live drill — cannot be un-set for a later test to
    /// observe the pre-installation state). This makes the assertion deterministic regardless of
    /// test execution order (round 4's version relied on ordering and silently skipped otherwise;
    /// Sol's review, round 5).
    #[test]
    fn apply_runsc_invocation_policy_refuses_explicit_userns_without_a_validated_policy() {
        let mut cmd = Command::new("runsc");
        let result = apply_runsc_invocation_policy_given(
            &mut cmd,
            RunscInvocationMode::ExplicitUserNamespace(UserNamespaceConfig::for_tests(
                1000, 1000, 100_000, 200_000,
            )),
            None,
        );
        assert!(
            result.is_err(),
            "ExplicitUserNamespace must refuse without a validated policy, not silently proceed"
        );
    }

    /// [`preflight_explicit_userns_helpers`] must accept this development host's real
    /// `/usr/bin` (containing genuine setuid `newuidmap`/`newgidmap`) and must reject a
    /// substitute helper directory containing a non-setuid stand-in.
    #[test]
    fn preflight_explicit_userns_helpers_accepts_real_and_rejects_a_non_setuid_substitute() {
        let real = Path::new("/usr/bin");
        if !real.join("newuidmap").exists() || !real.join("newgidmap").exists() {
            eprintln!("skipping: this host has no /usr/bin/newuidmap or newgidmap");
            return;
        }
        preflight_explicit_userns_helpers(real)
            .expect("this host's real /usr/bin must pass preflight");

        use std::os::unix::fs::PermissionsExt;
        let tmp =
            std::env::temp_dir().join(format!("myelin-preflight-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        for helper in ["newuidmap", "newgidmap"] {
            std::fs::write(tmp.join(helper), b"#!/bin/sh\nexit 1\n").unwrap();
            let mut perms = std::fs::metadata(tmp.join(helper)).unwrap().permissions();
            perms.set_mode(0o755); // executable, but NOT setuid and NOT root-owned
            std::fs::set_permissions(tmp.join(helper), perms).unwrap();
        }
        let result = preflight_explicit_userns_helpers(&tmp);
        std::fs::remove_dir_all(&tmp).ok();
        assert!(
            result.is_err(),
            "a non-root-owned, non-setuid substitute must be refused"
        );
    }

    #[test]
    fn sha256_hex_of_file_matches_a_known_vector() {
        let tmp = std::env::temp_dir().join(format!("myelin-sha256-test-{}", unique_suffix()));
        std::fs::write(&tmp, b"abc").unwrap();
        let digest = sha256_hex_of_file(&tmp).unwrap();
        std::fs::remove_file(&tmp).ok();
        assert_eq!(
            digest, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "must match the well-known SHA-256(\"abc\") test vector"
        );
    }

    /// Sol's review, round 4: pinning must check the binary's own content digest, not only the
    /// version string it happens to print — a forged/rebuilt substitute that echoes the exact
    /// pinned version line must still be refused if its content digest disagrees.
    #[test]
    fn verify_pinned_explicit_userns_runsc_rejects_a_forged_version_string_with_wrong_content() {
        let tmp = std::env::temp_dir().join(format!("myelin-forged-runsc-{}", unique_suffix()));
        std::fs::write(
            &tmp,
            format!("#!/bin/sh\necho '{PINNED_EXPLICIT_USERNS_RUNSC_VERSION}'\n"),
        )
        .unwrap();
        let mut perms = std::fs::metadata(&tmp).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o755);
        std::fs::set_permissions(&tmp, perms).unwrap();
        let result = verify_pinned_explicit_userns_runsc(&tmp);
        std::fs::remove_file(&tmp).ok();
        assert!(
            result.is_err(),
            "a forged version string with the wrong content digest must be refused: {result:?}"
        );
    }

    /// Sol's review, round 5: the digest pin alone doesn't stop the binary being replaced between
    /// preflight and a later launch — `harden_explicit_userns_runsc_binary` must refuse a binary
    /// this process itself owns (which it could `chmod`/replace at will), not only a wrong digest.
    #[test]
    fn harden_explicit_userns_runsc_binary_refuses_a_non_root_owned_file() {
        let tmp = std::env::temp_dir().join(format!("myelin-fake-runsc-{}", unique_suffix()));
        std::fs::write(&tmp, b"#!/bin/sh\nexit 0\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&tmp).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&tmp, perms).unwrap();
        let result = harden_explicit_userns_runsc_binary(&tmp);
        std::fs::remove_file(&tmp).ok();
        assert!(
            result.is_err(),
            "a binary owned by this process's own euid must be refused: {result:?}"
        );
    }

    #[test]
    fn harden_explicit_userns_runsc_binary_refuses_a_symlink() {
        let base =
            std::env::temp_dir().join(format!("myelin-fake-runsc-symlink-{}", unique_suffix()));
        std::fs::create_dir_all(&base).unwrap();
        let real = base.join("real-runsc");
        std::fs::write(&real, b"#!/bin/sh\nexit 0\n").unwrap();
        let link = base.join("runsc");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let result = harden_explicit_userns_runsc_binary(&link);
        let _ = std::fs::remove_dir_all(&base);
        assert!(
            result.is_err(),
            "a symlinked binary path must be refused rather than followed: {result:?}"
        );
    }

    /// Mirrors `strict_construction_refuses_a_leases_dir_whose_parent_is_writable_by_us` in
    /// `user_namespace.rs` — the exact same ancestor-writability requirement, applied here to the
    /// explicit-userns runsc state root (Sol's review, round 5: an absolute path string alone does
    /// not freeze what it names).
    #[test]
    fn harden_explicit_userns_runsc_root_refuses_a_leaf_under_a_writable_parent() {
        let base = std::env::temp_dir().join(format!(
            "myelin-runsc-root-writable-parent-{}",
            unique_suffix()
        ));
        std::fs::create_dir_all(&base).unwrap();
        let leaf = base.join("runsc-root");
        let result = harden_explicit_userns_runsc_root(&leaf);
        let _ = std::fs::remove_dir_all(&base);
        assert!(
            result.is_err(),
            "a leaf whose parent is writable by this process must be refused: {result:?}"
        );
    }

    /// Sol's review, round 6: no auto-creation — a missing leaf must be refused outright (proven
    /// via `verify_explicit_userns_runsc_root_leaf` directly, isolated from the ancestor check).
    #[test]
    fn verify_explicit_userns_runsc_root_leaf_refuses_a_missing_leaf() {
        let missing =
            std::env::temp_dir().join(format!("myelin-missing-runsc-root-{}", unique_suffix()));
        let result = verify_explicit_userns_runsc_root_leaf(&missing);
        assert!(
            result.is_err(),
            "a non-pre-provisioned leaf must be refused, never auto-created: {result:?}"
        );
    }

    /// Isolates JUST `verify_explicit_userns_runsc_root_leaf` (not the full
    /// `harden_explicit_userns_runsc_root`, whose ancestor check would refuse first against any
    /// fixture under a writable temp directory) against a real symlinked leaf.
    #[test]
    fn verify_explicit_userns_runsc_root_leaf_refuses_a_symlinked_leaf() {
        let base =
            std::env::temp_dir().join(format!("myelin-runsc-root-symlink-{}", unique_suffix()));
        std::fs::create_dir_all(&base).unwrap();
        let real_dir = base.join("real");
        std::fs::create_dir_all(&real_dir).unwrap();
        let link = base.join("runsc-root");
        std::os::unix::fs::symlink(&real_dir, &link).unwrap();
        let result = verify_explicit_userns_runsc_root_leaf(&link);
        let _ = std::fs::remove_dir_all(&base);
        assert!(
            result.is_err(),
            "a symlinked state-root leaf must be refused rather than followed: {result:?}"
        );
    }

    /// Sol's review, round 7: rejecting group/other bits alone still admits a mode like `0500`
    /// (owner cannot write) or `0000` (owner cannot even search it) — both unusable for actually
    /// creating/reading runsc state, despite passing the group/other-only check.
    #[test]
    fn verify_explicit_userns_runsc_root_leaf_refuses_an_owner_non_writable_directory() {
        let dir = std::env::temp_dir().join(format!("myelin-runsc-root-0500-{}", unique_suffix()));
        std::fs::create_dir_all(&dir).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dir).unwrap().permissions();
        perms.set_mode(0o500); // r-x------: owner cannot write, though group/other bits are clear.
        std::fs::set_permissions(&dir, perms).unwrap();
        let result = verify_explicit_userns_runsc_root_leaf(&dir);
        let mut restore = std::fs::metadata(&dir).unwrap().permissions();
        restore.set_mode(0o700);
        std::fs::set_permissions(&dir, restore).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            result.is_err(),
            "an owner-non-writable directory must be refused even with no group/other bits: \
             {result:?}"
        );
    }

    #[test]
    fn verify_explicit_userns_runsc_root_leaf_accepts_a_properly_pre_provisioned_leaf() {
        let dir = std::env::temp_dir().join(format!("myelin-runsc-root-ok-{}", unique_suffix()));
        std::fs::create_dir_all(&dir).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dir).unwrap().permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(&dir, perms).unwrap();
        let result = verify_explicit_userns_runsc_root_leaf(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            result.is_ok(),
            "a real, owned, mode-0700 pre-provisioned directory must be accepted: {result:?}"
        );
    }

    /// CT-007 slice 2's live pinned drill: an `OciConfig` with `ExplicitUserNamespace` actually
    /// boots through the REAL production `stage_production_bundle`/`run_and_capture` machinery
    /// (not a throwaway spike bundle) — proving the exact command-line/OCI-JSON contract this
    /// slice produces is genuinely runnable by the pinned `runsc` build, not merely well-formed
    /// JSON. A real [`crate::user_namespace::UserNamespaceAllocator`] leases the subordinate
    /// uid/gid pair from this host's REAL `/etc/subuid`/`/etc/subgid`. SKIPS gracefully without
    /// `runsc` on PATH, the staged escape-drill rootfs, or a usable subordinate-range entry for
    /// this process's own uid (present on this development host — CI hosts may lack one).
    #[test]
    #[cfg(feature = "integration")]
    fn explicit_user_namespace_boots_through_the_real_production_run_path() {
        // Sol's review, round 8: this drill previously resolved its OWN `bin` via a separate PATH
        // search, while `preflight_explicit_userns_policy` validates the process-global cached
        // `runsc_bin()` — the two could structurally diverge (e.g. if `RESOLVED_RUNSC_BIN` was
        // already initialized to something else earlier in this process), letting the drill
        // validate binary A and then execute binary B. Fixed by removing the drill's own
        // resolution entirely and using `runsc_bin()` — the SAME binary preflight just validated —
        // for the actual launch/delete calls below. `preflight_explicit_userns_policy` already
        // fails (and this drill already skips gracefully) if `runsc_bin()` doesn't resolve to a
        // usable, pinned binary at all, so no separate "runsc not on PATH" precondition check is
        // needed here anymore.
        //
        // This drill's whole point is proving the exact CLI/OCI contract this slice produces is
        // genuinely runnable — that claim is only proven against the SAME runsc release+build it
        // was validated against (Sol's review, round 4), a different (even same-version-string)
        // build is a drill PRECONDITION miss, not a bug this drill exists to catch, and the pin
        // check must never run against an unhardened pathname (Sol's review, round 7: a standalone
        // call to `verify_pinned_explicit_userns_runsc` here, BEFORE hardening, violated that
        // function's own documented caller precondition — a matching binary could still be
        // replaced between the hash and the `--version` exec if the pathname itself were never
        // proven immutable first).
        //
        // `apply_runsc_invocation_policy`'s `ExplicitUserNamespace` branch now REFUSES outright
        // without a validated policy (Sol's review, round 4) — this drill exercises the REAL
        // production activation path, so it must actually install one, exactly as a real
        // production caller would, rather than reaching into `EXPLICIT_USERNS_POLICY` directly.
        if let Err(e) = preflight_explicit_userns_policy(
            resolved_explicit_userns_helper_dir(),
            resolved_explicit_userns_runsc_root(),
        ) {
            eprintln!("[explicit-userns drill] SKIP: preflight_explicit_userns_policy failed: {e}");
            return;
        }
        let bin = runsc_bin();
        let rootfs = crate::resolved_gvisor_rootfs();
        if !rootfs.exists() {
            eprintln!("[explicit-userns drill] SKIP: staged rootfs absent at {rootfs:?}");
            return;
        }
        let leases_dir = std::env::temp_dir().join(format!(
            "myelin-userns-drill-leases-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        // `try_new_for_tests` (not the strict production `try_new`): this drill's `leases_dir`
        // sits under `std::env::temp_dir()`, whose PARENT (`/tmp`) is world-writable on virtually
        // every host — the strict production constructor's parent-not-writable-by-us check (Sol's
        // review, closing the replaceable-lock-anchor gap) would always refuse it here, which
        // would test nothing about this drill's actual purpose (proving the REAL bundle/launch
        // path boots an explicit-userns container). That deployment-layout requirement belongs to
        // slice 4's production-activation drills, which verify the REAL runner deployment's
        // directory permissions — not this slice's own test suite. The REAL host `/etc/subuid`/
        // `/etc/subgid` are still used (this host's copies are already root-owned, mode 644).
        let allocator = match crate::user_namespace::UserNamespaceAllocator::try_new_for_tests(
            leases_dir.clone(),
            Path::new("/etc/subuid"),
            Path::new("/etc/subgid"),
            1,
            Arc::new(|msg: &str| eprintln!("[explicit-userns drill incident] {msg}")),
        ) {
            Ok(a) => a,
            Err(
                e @ crate::user_namespace::UserNamespaceAllocatorError::NoSubordinateEntry {
                    ..
                },
            ) => {
                eprintln!(
                    "[explicit-userns drill] SKIP: no usable /etc/subuid|subgid range for this \
                     process's uid: {e}"
                );
                let _ = std::fs::remove_dir_all(&leases_dir);
                return;
            }
            Err(e) => panic!(
                "allocator construction failed with an unexpected (non-\"no usable range\") \
                 error — this indicates a real bug (malformed/unsafe config, lock contention, \
                 corrupt state, unsafe directory), not an absent host configuration: {e}"
            ),
        };
        let mut lease = allocator
            .lease()
            .expect("a fresh allocator's first lease must succeed");

        let mut command_spec = spec(vec![]);
        command_spec.command = vec!["/bin/sh".into(), "-c".into(), "id".into()];
        let profile = HardeningProfile::derive(&command_spec);
        let cfg = OciConfig::from_spec(&command_spec, &profile).with_user_namespace(lease.config());
        assert_eq!(
            cfg.invocation_mode(),
            RunscInvocationMode::ExplicitUserNamespace(lease.config())
        );

        let bundle = stage_production_bundle(&cfg, &rootfs).expect("stage the production bundle");
        let container_id = format!(
            "myelin-userns-drill-{}-{}",
            std::process::id(),
            unique_suffix()
        );
        // CT-007 slice 3: durably bind BEFORE exec — real `runsc_root_identity`/`cgroup_identity`
        // (the pinned runsc state-root's and the `MemoryCgroup`'s own (device, inode) identity)
        // land with this slice's own gvisor.rs wiring piece, not yet built; `(0, 0)` is a
        // placeholder here, matching only what THIS drill (which doesn't yet construct a real
        // `MemoryCgroup`-backed quiescence proof) needs to prove `bind`/`release`'s own contract.
        let runsc_root_identity = (0, 0);
        let cgroup_identity = (0, 0);
        lease
            .bind(container_id.clone(), runsc_root_identity, cgroup_identity)
            .expect("bind must succeed for a fresh Allocated lease");
        let mode = cfg.invocation_mode();
        let outcome = run_and_capture(
            bin,
            &bundle,
            &container_id,
            Duration::from_secs(10),
            command_spec.limits.mem_bytes,
            RunCaptureOptions {
                stdin: None,
                stdout_mode: StdoutMode::CappedHead,
                cancellation: &NEVER_CANCELLED,
                output: None,
            },
            None,
            mode,
        );
        delete_container(bin, &container_id, mode);
        let _ = std::fs::remove_dir_all(&bundle);

        let outcome = outcome.unwrap_or_else(|e| {
            panic!("run_and_capture must succeed through the real production path: {e:?}")
        });
        assert!(
            !outcome.timed_out,
            "the guest `id` command must not time out"
        );
        assert_eq!(
            outcome.exit,
            Some(0),
            "the guest `id` command must exit 0, stderr: {}",
            String::from_utf8_lossy(&outcome.stderr)
        );
        let stdout = String::from_utf8_lossy(&outcome.stdout);
        assert!(
            stdout.contains("uid=65534") && stdout.contains("gid=65534"),
            "the guest must report uid/gid 65534 (mapped via the OCI uidMappings/gidMappings \
             this slice emits), got: {stdout:?}"
        );

        let nonce = lease.nonce_for_tests();
        lease
            .release(
                crate::user_namespace::UserNamespaceQuiescenceProof::assert_for_tests(
                    nonce,
                    container_id,
                    runsc_root_identity,
                    cgroup_identity,
                ),
            )
            .expect("release with the lease's own nonce and bound identity must succeed");
        let _ = std::fs::remove_dir_all(&leases_dir);
    }

    #[test]
    fn gvisor_launch_drives_four_guarantees_on_the_same_trait() {
        // The SAME SandboxBackend trait + the SAME hardening — the named-second backend.
        let backend = GvisorBackend::new(test_registry());
        let launch = backend
            .launch_with(
                &spec(vec![]),
                &ok_hooks(),
                |_spec, _cfg, permit, _rootfs| {
                    permit
                        .commit_and_release()
                        .map_err(|error| RunFailure::uncommitted(error.to_string()))?;
                    Ok(fake_run())
                },
            )
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
        assert!(script.contains("trap cleanup_f1 EXIT"));
        assert!(script.contains("exit 42"));
        assert!(script.contains("[ \"$admitted\" -gt 64 ]"));
        assert!(script.contains("F1_forkbomb ESCAPED admitted=$admitted"));
        // The admitted children are reaped before D2/Mx so the fork-bomb probe cannot consume the
        // shared memory cgroup and vacuously kill a later independent resource probe.
        let reap = script.find("cleanup_f1()").unwrap();
        let verdict = script.find("if [ \"$f1_status\" -eq 42 ]").unwrap();
        let diskfill = script.find("if dd if=/dev/zero").unwrap();
        assert!(
            script.contains("wait 2>/dev/null || true") && reap < verdict && verdict < diskfill
        );
        // CT-003b: the anon-memory hog's ATTEMPT sentinel + the END marker precede the oversized
        // alloc, so the corpus COMPLETES even when the contained hog OOM-kills the whole sentry
        // mid-alloc (the host cgroup bounds host RAM). The ESCAPED line follows only if it HELD.
        let attempt = script
            .find(&format!("{} ATTEMPT", crate::escape_corpus::MEMHOG_ID))
            .expect("memhog ATTEMPT sentinel in the gVisor corpus");
        let end = script.find(crate::escape_corpus::END_MARKER).unwrap();
        assert!(
            attempt < end,
            "the memhog ATTEMPT sentinel must precede the END marker"
        );
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
    fn quiesce_succeeds_on_an_empty_real_cgroup_and_removes_it() {
        let cg = MemoryCgroup::create(64 << 20)
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
    fn quiesce_kills_a_descendant_detached_outside_the_runtime_process_group() {
        // The whole point of `cgroup.kill` (over a mere process-group signal) is that it reaches
        // EVERY member of the cgroup subtree, regardless of session/process-group — including a
        // descendant that has `setsid`'d itself away, exactly the shape a sentry/gofer escape
        // would take. Reuses the exact detached-descendant spawning pattern the existing watchdog
        // test uses, but calls `quiesce()` directly instead of the watchdog's own cgroup.kill.
        let cg = MemoryCgroup::create(64 << 20)
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
    fn quiesce_succeeds_without_the_caller_ever_reaping_a_killed_direct_child() {
        // `populated` tracks LIVENESS, not reap status — quiesce() must not depend on the caller
        // having `wait()`-ed its child first, only on `cgroup.kill` having actually killed it.
        let cg = MemoryCgroup::create(64 << 20)
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
    fn quiesce_refuses_to_mint_evidence_when_an_unexpected_child_cgroup_blocks_removal() {
        // `populated=0` can be true (this cgroup itself holds no processes) while `rmdir` still
        // refuses because a child cgroup exists underneath it — quiescence must not be treated as
        // stable (the hierarchy could still be re-populated) until removal itself succeeds.
        let cg = MemoryCgroup::create(64 << 20)
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
    fn drop_retries_removal_after_an_earlier_failed_cleanup_call() {
        let cg = MemoryCgroup::create(64 << 20)
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
    fn quiesce_detects_an_identity_change_and_refuses_to_mint_evidence() {
        let cg = MemoryCgroup::create(64 << 20)
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
    fn launch_watchdog_cgroup_kills_a_descendant_outside_the_runtime_process_group() {
        use std::time::Instant;
        let cgroup = MemoryCgroup::create(64 << 20)
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

    #[test]
    fn gvisor_drill_config_expresses_the_mandatory_posture() {
        let json = gvisor_drill_config_json(&spec(vec![]), GVISOR_CORPUS_SCRIPT).unwrap();
        // Read-only root, no-new-privs, all caps dropped, the pids ceiling — the SAME mandatory
        // profile the Firecracker backend enforces, expressed through the OCI spec gVisor consumes.
        assert!(json.contains("\"readonly\": true"));
        assert!(json.contains("\"noNewPrivileges\": true"));
        assert!(json.contains("\"bounding\": []"));
        assert!(json.contains("\"limit\": 64"));
        assert!(json.contains("\"type\": \"RLIMIT_NPROC\""));
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
        let backend = GvisorBackend::new(test_registry());
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(|_spec| Err(crate::HookError("exhausted".into()))),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );
        let r = backend.launch_with(&spec(vec![]), &hooks, |_spec, _cfg, _permit, _rootfs| {
            Ok(fake_run())
        });
        assert!(matches!(
            r,
            Err(SandboxLaunchError::Failed(GvisorError::Hook(_)))
        ));
    }

    #[test]
    fn successful_reporter_owned_gvisor_launch_defers_settlement_to_terminal_reporter() {
        let backend = GvisorBackend::new(test_registry());
        let hook_settled = Arc::new(AtomicBool::new(false));
        let hook_settled_at = hook_settled.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::TerminalReporter,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(move |_spec, _h, _u| {
                hook_settled_at.store(true, Ordering::SeqCst);
                Ok(())
            }),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );

        backend
            .launch_with(&spec(vec![]), &hooks, |_spec, _cfg, permit, _rootfs| {
                permit
                    .commit_and_release()
                    .map_err(|error| RunFailure::uncommitted(error.to_string()))?;
                Ok(fake_run())
            })
            .expect("the sandbox returns measured usage for the reporter transaction");
        assert!(
            !hook_settled.load(Ordering::SeqCst),
            "reporter-owned completion must not settle through the hook"
        );
    }

    #[test]
    fn settlement_failure_unconditionally_kills_and_forgets_the_container() {
        let backend = GvisorBackend::new(test_registry());
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(|_spec, _handle, _usage| {
                Err(crate::HookError("injected settlement failure".into()))
            }),
            Box::new(|_spec| Ok(())),
            Box::new(|_spec| Ok(())),
        );

        let result = backend.launch_with(&spec(vec![]), &hooks, |_spec, _cfg, permit, _rootfs| {
            permit
                .commit_and_release()
                .map_err(|error| RunFailure::uncommitted(error.to_string()))?;
            Ok(fake_run())
        });

        assert!(matches!(
            result,
            Err(SandboxLaunchError::Failed(GvisorError::Hook(_)))
        ));
        assert!(
            backend.live.lock().unwrap().is_empty(),
            "an error without a returned handle cannot retain an unreachable live-map entry"
        );
    }

    #[test]
    fn gvisor_releases_the_unused_reserve_when_final_attribution_refuses() {
        let backend = GvisorBackend::new(test_registry());
        let settled = Arc::new(Mutex::new(None));
        let settled_at = settled.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::TerminalReporter,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(move |_spec, _h, usage| {
                *settled_at.lock().unwrap() = Some(usage);
                Ok(())
            }),
            Box::new(|_t| Err(crate::HookError("claim canceled".into()))),
            Box::new(|_s| Ok(())),
        );
        let spawned = Arc::new(AtomicBool::new(false));
        let spawned_at = spawned.clone();
        let result = backend.launch_with(
            &spec(vec![]),
            &hooks,
            move |_spec, _cfg, _permit, _rootfs| {
                spawned_at.store(true, Ordering::SeqCst);
                Ok(fake_run())
            },
        );
        assert!(matches!(
            result,
            Err(SandboxLaunchError::Failed(GvisorError::Hook(_)))
        ));
        assert!(!spawned.load(Ordering::SeqCst));
        assert_eq!(
            *settled.lock().unwrap(),
            Some(ResourceUsage {
                cpu_seconds: 0,
                mem_byte_seconds: 0,
            })
        );
    }

    /// The pre-existing leak this fix closes: previously, ANY error from `run(...)` propagated
    /// straight out of `launch_with` with NEITHER `release_unused` NOR `settle_completed` ever
    /// called — leaking the reservation on every single run failure. These tests prove each of the
    /// four `RunFailure` phases dispatches to the correct outcome, per Sol's corrected disposition
    /// table (phase × `CompletionSettlementOwner`):
    ///
    /// | Phase                    | `Hook` owner                       | `TerminalReporter` owner                  |
    /// |---------------------------|-------------------------------------|--------------------------------------------|
    /// | `Uncommitted`             | `release_unused`, then `Failed`     | `release_unused`, then `Failed`             |
    /// | `CommitOutcomeUnknown`    | `DurableOutcomeUnknown`             | `DurableOutcomeUnknown`                     |
    /// | `CommittedButNotExecuted` | settle zero, then `Failed`          | `RetryableAttempt(SandboxInfrastructure, 0)`|
    /// | `Executed`                | settle usage, then `Failed`         | `RetryableAttempt(SandboxInfrastructure, usage)`|
    ///
    /// `Uncommitted` and `CommitOutcomeUnknown` are owner-INDEPENDENT (an uncommitted attempt has no
    /// terminal report to defer to regardless of owner; an outcome-unknown attempt must never be
    /// guessed at either way) — only the two post-commit phases branch on ownership, since only they
    /// carry a real (if zero) measured cost a `TerminalReporter` must eventually account for.
    #[test]
    fn gvisor_run_failure_uncommitted_releases_reserve_via_release_unused() {
        let backend = GvisorBackend::new(test_registry());
        let settled = Arc::new(Mutex::new(None));
        let settled_at = settled.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::TerminalReporter,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(move |_spec, _h, usage| {
                *settled_at.lock().unwrap() = Some(usage);
                Ok(())
            }),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );
        let result = backend.launch_with(&spec(vec![]), &hooks, |_spec, _cfg, _permit, _rootfs| {
            Err(RunFailure::uncommitted("injected uncommitted run failure"))
        });
        assert!(
            matches!(
                result,
                Err(SandboxLaunchError::Failed(GvisorError::Runtime(_)))
            ),
            "an uncommitted run failure must surface as Failed(GvisorError::Runtime): {result:?}"
        );
        assert_eq!(
            *settled.lock().unwrap(),
            Some(ResourceUsage {
                cpu_seconds: 0,
                mem_byte_seconds: 0,
            }),
            "release_unused must settle at zero even under reporter-owned completion — it is \
             owner-independent, unlike settle_completed"
        );
    }

    /// `CommitOutcomeUnknown` must NEVER release or settle — the durable commit outcome is
    /// genuinely unknown, and guessing either way misaccounts a real reservation. Owner-independent:
    /// this test uses `Hook` ownership specifically to prove the outcome-unknown path bypasses
    /// `settle_completed` entirely rather than merely happening to observe a reporter's no-op.
    #[test]
    fn gvisor_run_failure_commit_outcome_unknown_never_releases_or_settles() {
        let backend = GvisorBackend::new(test_registry());
        let settled = Arc::new(AtomicBool::new(false));
        let settled_at = settled.clone();
        let released = Arc::new(AtomicBool::new(false));
        let released_at = released.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(move |_spec, _h, usage| {
                settled_at.store(true, Ordering::SeqCst);
                if usage
                    == (ResourceUsage {
                        cpu_seconds: 0,
                        mem_byte_seconds: 0,
                    })
                {
                    released_at.store(true, Ordering::SeqCst);
                }
                Ok(())
            }),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );
        let result = backend.launch_with(&spec(vec![]), &hooks, |_spec, _cfg, _permit, _rootfs| {
            Err(RunFailure::commit_outcome_unknown(
                "injected commit-outcome-unknown run failure",
            ))
        });
        assert!(
            matches!(
                result,
                Err(SandboxLaunchError::DurableOutcomeUnknown(GvisorError::Runtime(_)))
            ),
            "a commit-outcome-unknown run failure must surface as DurableOutcomeUnknown: {result:?}"
        );
        assert!(
            !settled.load(Ordering::SeqCst) && !released.load(Ordering::SeqCst),
            "neither settle_completed nor release_unused (which also calls the settle hook) may \
             ever fire for an outcome-unknown attempt"
        );
    }

    /// `CommittedButNotExecuted` under `Hook` ownership settles zero synchronously, then surfaces
    /// `Failed` — a real terminal report IS expected here (unlike `Uncommitted`'s "none will ever
    /// follow"), and `Hook` ownership means the hook itself is the one committing that report.
    #[test]
    fn gvisor_run_failure_committed_but_not_executed_hook_owner_settles_zero_then_fails() {
        let backend = GvisorBackend::new(test_registry());
        let settled = Arc::new(Mutex::new(None));
        let settled_at = settled.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(move |_spec, _h, usage| {
                *settled_at.lock().unwrap() = Some(usage);
                Ok(())
            }),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );
        let result = backend.launch_with(&spec(vec![]), &hooks, |_spec, _cfg, _permit, _rootfs| {
            Err(RunFailure::committed_but_not_executed(
                "injected committed-but-not-executed run failure",
            ))
        });
        assert!(
            matches!(
                result,
                Err(SandboxLaunchError::Failed(GvisorError::Runtime(_)))
            ),
            "a Hook-owned committed-but-not-executed failure must surface as Failed: {result:?}"
        );
        assert_eq!(
            *settled.lock().unwrap(),
            Some(ResourceUsage {
                cpu_seconds: 0,
                mem_byte_seconds: 0,
            }),
            "Hook ownership must settle zero usage synchronously through settle_completed"
        );
    }

    /// `CommittedButNotExecuted` under `TerminalReporter` ownership must NOT call `settle_completed`
    /// at all (it would silently no-op) — it must instead surface `RetryableAttempt` so the RUNNER
    /// routes it through the reporter's own `report_retryable_attempt` transaction, which durably
    /// accounts usage and either requeues or terminalizes the exact claim. This is the exact case
    /// Sol's review caught: the original fix called `settle_completed` here and returned an
    /// ordinary `Failed`, which under reporter ownership silently discarded the accounting with no
    /// terminal report ever following.
    #[test]
    fn gvisor_run_failure_committed_but_not_executed_reporter_owner_yields_retryable_attempt() {
        let backend = GvisorBackend::new(test_registry());
        let settled = Arc::new(AtomicBool::new(false));
        let settled_at = settled.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::TerminalReporter,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(move |_spec, _h, _usage| {
                settled_at.store(true, Ordering::SeqCst);
                Ok(())
            }),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );
        let result = backend.launch_with(&spec(vec![]), &hooks, |_spec, _cfg, _permit, _rootfs| {
            Err(RunFailure::committed_but_not_executed(
                "injected committed-but-not-executed run failure",
            ))
        });
        match result {
            Err(SandboxLaunchError::RetryableAttempt { cause, usage, .. }) => {
                assert_eq!(cause, RetryableAttemptCause::SandboxInfrastructure);
                assert_eq!(
                    usage,
                    ResourceUsage {
                        cpu_seconds: 0,
                        mem_byte_seconds: 0,
                    }
                );
            }
            other => panic!("expected RetryableAttempt with zero usage, got {other:?}"),
        }
        assert!(
            !settled.load(Ordering::SeqCst),
            "settle_completed must never be called directly here — the runner's retryable-attempt \
             transaction is the sole accounting path under reporter ownership"
        );
    }

    /// `Executed` under `Hook` ownership must settle the CONSERVATIVE fallback usage synchronously,
    /// never zero — a job engineered to fail exactly after the runtime was released to exec must not
    /// execute for free (the host-DoS surface Sol's design closes) — then surface `Failed`.
    #[test]
    fn gvisor_run_failure_executed_hook_owner_settles_fallback_usage_then_fails() {
        let backend = GvisorBackend::new(test_registry());
        let settled = Arc::new(Mutex::new(None));
        let settled_at = settled.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(move |_spec, _h, usage| {
                *settled_at.lock().unwrap() = Some(usage);
                Ok(())
            }),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );
        let fallback_usage = ResourceUsage {
            cpu_seconds: 7,
            mem_byte_seconds: 700,
        };
        let result = backend.launch_with(
            &spec(vec![]),
            &hooks,
            move |_spec, _cfg, _permit, _rootfs| {
                Err(RunFailure::executed(
                    "injected executed-phase run failure",
                    fallback_usage,
                ))
            },
        );
        assert!(
            matches!(
                result,
                Err(SandboxLaunchError::Failed(GvisorError::Runtime(_)))
            ),
            "a Hook-owned executed-phase failure must surface as Failed: {result:?}"
        );
        assert_eq!(
            *settled.lock().unwrap(),
            Some(fallback_usage),
            "the executed phase must settle its carried conservative fallback usage, never zero"
        );
    }

    /// `Executed` under `TerminalReporter` ownership must surface `RetryableAttempt` carrying the
    /// SAME conservative fallback usage (never zero) — the reporter's own transaction, not
    /// `settle_completed`, is what durably accounts it.
    #[test]
    fn gvisor_run_failure_executed_reporter_owner_yields_retryable_attempt_with_fallback_usage() {
        let backend = GvisorBackend::new(test_registry());
        let settled = Arc::new(AtomicBool::new(false));
        let settled_at = settled.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::TerminalReporter,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(move |_spec, _h, _usage| {
                settled_at.store(true, Ordering::SeqCst);
                Ok(())
            }),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );
        let fallback_usage = ResourceUsage {
            cpu_seconds: 3,
            mem_byte_seconds: 300,
        };
        let result = backend.launch_with(
            &spec(vec![]),
            &hooks,
            move |_spec, _cfg, _permit, _rootfs| {
                Err(RunFailure::executed(
                    "injected executed-phase run failure",
                    fallback_usage,
                ))
            },
        );
        match result {
            Err(SandboxLaunchError::RetryableAttempt { cause, usage, .. }) => {
                assert_eq!(cause, RetryableAttemptCause::SandboxInfrastructure);
                assert_eq!(usage, fallback_usage);
            }
            other => panic!("expected RetryableAttempt with the fallback usage, got {other:?}"),
        }
        assert!(
            !settled.load(Ordering::SeqCst),
            "settle_completed must never be called directly here — the runner's retryable-attempt \
             transaction is the sole accounting path under reporter ownership"
        );
    }

    /// CT-007 gate 2/4 (f, corrected ordering): a RED isolation floor refuses BEFORE the registry
    /// lookup is ever consulted — proven by using a genuinely UNREGISTERED image (so if the
    /// (wrong-order) implementation queried the registry first, it would refuse there as
    /// `GvisorError::Image` WITHOUT the floor hook ever having been called, and `floor_called` would
    /// read `false`). Asserting `floor_called == true` alongside a `GvisorError::Hook` result is only
    /// possible if the floor really did run first, despite the image being unresolvable — which also
    /// means an exhausted-wallet caller cannot force the (now-cheap, but real) registry lookup by
    /// repeatedly failing the floor.
    #[test]
    fn red_isolation_floor_refuses_before_registry_lookup_reserve_or_spawn() {
        let floor_called = Arc::new(AtomicBool::new(false));
        let floor_called_at = floor_called.clone();
        let reserve_called = Arc::new(AtomicBool::new(false));
        let reserve_called_at = reserve_called.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(move |spec| {
                reserve_called_at.store(true, Ordering::SeqCst);
                Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))
            }),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_t| Ok(())),
            Box::new(move |_spec| {
                floor_called_at.store(true, Ordering::SeqCst);
                Err(crate::HookError(
                    "isolation floor is RED for this test".into(),
                ))
            }),
        );

        let mut unregistered_spec = spec(vec![]);
        unregistered_spec.image = ImageRef::pinned(
            "test.local/genuinely-unregistered@sha256:3333333333333333333333333333333333333333333333333333333333333333",
        )
        .unwrap();
        let spawned = Arc::new(AtomicBool::new(false));
        let spawned_at = spawned.clone();
        // A fresh, otherwise-empty registry — the spec's image is deliberately NOT registered here,
        // so a wrong-order (registry-before-floor) implementation would refuse via `Image`, not
        // `Hook`, and would never call the floor closure at all.
        let backend = GvisorBackend::new(Arc::new(
            crate::asset_registry::GvisorAssetRegistry::from_bindings(vec![]).unwrap(),
        ));
        let result = backend.launch_with(
            &unregistered_spec,
            &hooks,
            move |_spec, _cfg, _permit, _rootfs| {
                spawned_at.store(true, Ordering::SeqCst);
                Ok(fake_run())
            },
        );

        assert!(
            matches!(result, Err(SandboxLaunchError::Failed(GvisorError::Hook(_)))),
            "the isolation floor's own refusal must surface, proving it ran BEFORE the registry \
             lookup (an unregistered image would otherwise short-circuit as `Image` first): {result:?}"
        );
        assert!(
            floor_called.load(Ordering::SeqCst),
            "the isolation floor must be consulted even for an unresolvable image"
        );
        assert!(
            !reserve_called.load(Ordering::SeqCst),
            "no reserve may be attempted"
        );
        assert!(
            !spawned.load(Ordering::SeqCst),
            "the run closure must never be invoked"
        );
    }

    /// CT-007 gate 2/4 (f, still-correct half): a GREEN isolation floor + an unknown image still
    /// refuses before `reserve`/the `run` closure — none of them ever fire. This is the part of the
    /// original ordering test that was already right; it just now runs AFTER the floor instead of
    /// before it.
    #[test]
    fn unknown_image_after_green_floor_refuses_before_reserve_or_spawn() {
        let floor_called = Arc::new(AtomicBool::new(false));
        let floor_called_at = floor_called.clone();
        let reserve_called = Arc::new(AtomicBool::new(false));
        let reserve_called_at = reserve_called.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(move |spec| {
                reserve_called_at.store(true, Ordering::SeqCst);
                Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))
            }),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_t| Ok(())),
            Box::new(move |_spec| {
                floor_called_at.store(true, Ordering::SeqCst);
                Ok(())
            }),
        );

        let mut unregistered_spec = spec(vec![]);
        unregistered_spec.image = ImageRef::pinned(
            "test.local/genuinely-unregistered@sha256:3333333333333333333333333333333333333333333333333333333333333333",
        )
        .unwrap();
        let spawned = Arc::new(AtomicBool::new(false));
        let spawned_at = spawned.clone();
        // A fresh, otherwise-empty registry — the fixture image is deliberately NOT registered here.
        let backend = GvisorBackend::new(Arc::new(
            crate::asset_registry::GvisorAssetRegistry::from_bindings(vec![]).unwrap(),
        ));
        let result = backend.launch_with(
            &unregistered_spec,
            &hooks,
            move |_spec, _cfg, _permit, _rootfs| {
                spawned_at.store(true, Ordering::SeqCst);
                Ok(fake_run())
            },
        );

        assert!(matches!(
            result,
            Err(SandboxLaunchError::Failed(GvisorError::Image(_)))
        ));
        assert!(
            floor_called.load(Ordering::SeqCst),
            "the isolation floor must have been consulted (and passed) first"
        );
        assert!(
            !reserve_called.load(Ordering::SeqCst),
            "no reserve may be attempted"
        );
        assert!(
            !spawned.load(Ordering::SeqCst),
            "the run closure must never be invoked"
        );
    }

    /// A committed regression pin for `GvisorBackend::git_wire_only()`'s refusal of ordinary launch:
    /// the behavior existed (see `launch_with`'s `self.registry.as_ref().ok_or_else(...)`) but had no
    /// test asserting it returns `GvisorError::Image` rather than panicking or hanging.
    #[test]
    fn git_wire_only_backend_refuses_ordinary_launch() {
        let backend = GvisorBackend::git_wire_only();
        let hooks = ok_hooks();
        let result = backend.launch(&spec(vec![]), &hooks);
        assert!(
            matches!(
                result,
                Err(SandboxLaunchError::Failed(GvisorError::Image(_)))
            ),
            "a git-wire-only backend has no asset registry and must refuse an ordinary launch as \
             GvisorError::Image, not panic or hang: {result:?}"
        );
    }

    /// The same refusal for the streaming entry point.
    #[test]
    fn git_wire_only_backend_refuses_ordinary_launch_streaming() {
        let backend = GvisorBackend::git_wire_only();
        let hooks = ok_hooks();
        let output: Arc<dyn SandboxOutputSink> = Arc::new(RecordingOutput::default());
        let result =
            backend.launch_streaming(&spec(vec![]), &hooks, output, SandboxCancellation::new());
        assert!(
            matches!(result, Err(SandboxLaunchError::Failed(GvisorError::Image(_)))),
            "a git-wire-only backend must refuse ordinary launch_streaming the same way as launch: \
             {result:?}"
        );
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
                tmpfs_bytes: 1,
                pids_max: 1,
                timeout_secs: 1,
            },
            run_token: RunTokenCredential::new("test-bearer", "cancel", 300).unwrap(),
            meter_to: MeterTarget {
                reserve_id: "cancel".into(),
            },
            idem_token: IdemToken("cancel".into()),
        };
        let result = GvisorBackend::git_wire_only().launch_git_wire_until_cancelled(
            &spec,
            &ok_hooks(),
            &cancelled,
        );
        assert!(
            matches!(result, Err(WireError::Runtime(message)) if message.contains("cancelled by process shutdown"))
        );
    }

    /// Git-wire is a direct synchronous path with no terminal reporter above it — reporter-owned
    /// hooks must be refused BEFORE reserve or any rootfs/mount/spawn work, exactly like the
    /// analogous agent-service `dispatch_compute` refusal. Proven WITHOUT a real `runsc`: the
    /// refusal happens before any of that is ever touched.
    #[test]
    fn git_wire_refuses_reporter_owned_hooks_before_reserve() {
        // A REAL repo directory under a REAL root — this test is about the ownership refusal, not
        // the symlink/path-confinement defense (`symlinked_repo_path_is_refused_before_mount`
        // covers that), so the path itself must actually pass `assert_repo_under_root` first.
        let tmp = std::env::temp_dir().join(format!(
            "myelin-gitwire-reporter-owned-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let root = tmp.join("git-root");
        let repo = root.join("acme").join("fr-par").join("widgets.git");
        std::fs::create_dir_all(&repo).unwrap();

        let reserve_called = Arc::new(AtomicBool::new(false));
        let reserve_called_at = reserve_called.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::TerminalReporter,
            Box::new(move |spec| {
                reserve_called_at.store(true, Ordering::SeqCst);
                Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))
            }),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );
        let spec = GitWireSpec {
            repo_host_path: repo,
            root,
            git_argv: vec!["upload-pack".into()],
            stdin: Vec::new(),
            env: Vec::new(),
            quarantine_host_path: None,
            limits: ResourceLimits {
                cpu_millis: 1,
                mem_bytes: 1,
                disk_bytes: 1,
                tmpfs_bytes: 1,
                pids_max: 1,
                timeout_secs: 1,
            },
            run_token: RunTokenCredential::new("test-bearer", "reporter-owned", 300).unwrap(),
            meter_to: MeterTarget {
                reserve_id: "reporter-owned".into(),
            },
            idem_token: IdemToken("reporter-owned".into()),
        };
        let result = GvisorBackend::git_wire_only().launch_git_wire_until_cancelled(
            &spec,
            &hooks,
            &NEVER_CANCELLED,
        );
        assert!(
            matches!(result, Err(WireError::Runtime(ref message)) if message.contains("requires Hook-owned")),
            "expected a Hook-ownership refusal, got {result:?}"
        );
        assert!(
            !reserve_called.load(Ordering::SeqCst),
            "reporter-owned hooks must refuse before reserve is ever called"
        );
    }

    /// The four `RunFailure` phases dispatch through `dispose_git_wire_run_failure` exactly as
    /// gVisor's `dispose_run_failure` does under `Hook` ownership (git-wire always settles
    /// synchronously — there is no reporter to defer to): `Uncommitted` -> `release_unused`;
    /// `CommitOutcomeUnknown` -> neither release nor settle; `CommittedButNotExecuted` -> settle
    /// zero; `Executed` -> settle the carried usage. Unit-tested directly (no real `runsc` needed).
    #[test]
    fn dispose_git_wire_run_failure_dispatches_all_four_phases() {
        fn recording_hooks() -> (RunnerHooks, Arc<Mutex<Vec<ResourceUsage>>>) {
            let settled = Arc::new(Mutex::new(Vec::new()));
            let settled_at = settled.clone();
            let hooks = RunnerHooks::new(
                CompletionSettlementOwner::Hook,
                Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
                Box::new(move |_spec, _h, usage| {
                    settled_at.lock().unwrap().push(usage);
                    Ok(())
                }),
                Box::new(|_t| Ok(())),
                Box::new(|_s| Ok(())),
            );
            (hooks, settled)
        }
        let job = spec(vec![]);
        let reserve = ReserveHandle(job.meter_to.reserve_id.clone());
        let zero = ResourceUsage {
            cpu_seconds: 0,
            mem_byte_seconds: 0,
        };

        let (hooks, settled) = recording_hooks();
        let error = dispose_git_wire_run_failure(
            &hooks,
            &job,
            &reserve,
            RunFailure::uncommitted("injected uncommitted"),
        );
        assert!(matches!(error, WireError::Runtime(m) if m.contains("injected uncommitted")));
        assert_eq!(
            *settled.lock().unwrap(),
            vec![zero],
            "release_unused settles zero"
        );

        let (hooks, settled) = recording_hooks();
        let error = dispose_git_wire_run_failure(
            &hooks,
            &job,
            &reserve,
            RunFailure::commit_outcome_unknown("injected outcome unknown"),
        );
        assert!(matches!(error, WireError::Runtime(m) if m.contains("needs reconciliation")));
        assert!(
            settled.lock().unwrap().is_empty(),
            "commit-outcome-unknown must never release or settle"
        );

        let (hooks, settled) = recording_hooks();
        let error = dispose_git_wire_run_failure(
            &hooks,
            &job,
            &reserve,
            RunFailure::committed_but_not_executed("injected committed but not executed"),
        );
        assert!(
            matches!(error, WireError::Runtime(m) if m.contains("injected committed but not executed"))
        );
        assert_eq!(*settled.lock().unwrap(), vec![zero]);

        let (hooks, settled) = recording_hooks();
        let fallback_usage = ResourceUsage {
            cpu_seconds: 4,
            mem_byte_seconds: 400,
        };
        let error = dispose_git_wire_run_failure(
            &hooks,
            &job,
            &reserve,
            RunFailure::executed("injected executed", fallback_usage),
        );
        assert!(matches!(error, WireError::Runtime(m) if m.contains("injected executed")));
        assert_eq!(
            *settled.lock().unwrap(),
            vec![fallback_usage],
            "executed must settle the carried conservative usage, never zero"
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
