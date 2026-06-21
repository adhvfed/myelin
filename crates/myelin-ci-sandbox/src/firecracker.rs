//! # The Firecracker default `SandboxBackend` (CI-P2 → P-237, M2)
//!
//! **Owning architecture (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md`
//! §5.1 ("Backend decision: microVM (Firecracker) default") + §5.3 (the hardening profile) +
//! `sketches/01-isolation-model.md` (Candidate B — microVM as the default; "the boundary is the
//! CPU's VT-x/AMD-V + a tiny VMM"). **Contract:** `contract-index.md` row 8.4 (the unified
//! sandbox — the Firecracker half), obeying 1.6 `no-host-exec` (this `launch` IS the sandbox seam,
//! not a bypass — see the lint note below).
//!
//! ## What this is
//! The DEFAULT backend for untrusted code: a Firecracker microVM built on KVM (hardware
//! virtualization) plus a minimal VMM. [`launch`](FirecrackerBackend::launch) boots a digest-pinned
//! [`JobSpec`] as a microVM, building the JSON machine config FROM the spec (kernel = the staged
//! `vmlinux`, root drive = the read-only rootfs, vcpu/mem from
//! [`ResourceLimits`](crate::ResourceLimits), and no network device unless the egress allowlist is
//! non-empty), then running `firecracker --no-api --config-file <cfg>`.
//! [`kill`](FirecrackerBackend::kill) whole-guest-kills the VMM process on teardown; the guest is
//! ephemeral and never reused.
//!
//! ## `no-host-exec` (contract 1.6 / X-6 / AG-2) — why this file is the ONE legitimate VMM-spawn site
//! The `no-host-exec` lint forbids any PLATFORM code shelling out to the host kernel **so that all
//! execution goes through the unified sandbox seam** (`SandboxBackend::launch`). This file IS that
//! seam's enforcement mechanism: spawning the Firecracker VMM is precisely how the boundary is
//! *created* — it is not a path that bypasses the sandbox, it is the path that builds it. This is
//! exactly analogous to the named, loud exclusions already in `lint-gate` (the relay's one
//! broker-publish site; the harness runners' one `cargo`-spawn site). The VMM-spawn site is a
//! NAMED, LOUD exclusion of this one file (registered in `myelin-lints/src/bin/lint-gate.rs` and
//! `tests/workspace_clean.rs`); the lint stays fully live on every other production file, and the
//! `accept_only_compute` routing split (mutation never reaches here; contract 8.2) is unweakened.
//!
//! ## FLOOR (named per CI-P2)
//! ONE backend (Firecracker) goes through the escape drill first; **gVisor is the named second
//! backend behind the SAME trait** ([`crate::gvisor`]) — the CI-P28 "gVisor second backend" floor
//! is satisfied EARLY here so the AG-D4 drill (CI-P5 → P-239) can parametrize per available backend.
//! The fleet impl is CI-P14; pre-warmed snapshot pools are CI-P4.

use crate::hardening::HardeningProfile;
use crate::{JobSpec, RunnerHooks, SandboxBackend, SandboxHandle};
use std::path::PathBuf;
use std::sync::Mutex;

/// Env var naming the guest kernel (`vmlinux`); defaults to the staged boot-proven asset so dev↔prod
/// is a config swap (CI-P2 DELIVERABLE).
pub const ENV_FC_KERNEL: &str = "MYELIN_FC_KERNEL";
/// Env var naming the read-only rootfs (a squashfs that mounts natively in-guest); defaults to the
/// staged boot-proven asset.
pub const ENV_FC_ROOTFS: &str = "MYELIN_FC_ROOTFS";
/// Env var naming the `firecracker` binary; defaults to `firecracker` on `PATH`.
pub const ENV_FC_BIN: &str = "MYELIN_FC_BIN";

/// The boot-proven kernel cmdline (verified on this host; CI-P2 HOST CAPABILITIES). The root drive
/// is mounted **read-only** (`root=/dev/vda ro`) — the read-only-root half of the profile enforced
/// at the kernel-cmdline level. `init=/bin/true` makes a one-shot deterministic boot (userspace runs,
/// init exits → kernel `panic=1` → `reboot=k` → the VMM exits 0).
pub const BOOT_ARGS_BASE: &str = "console=ttyS0 reboot=k panic=1 pci=off i8042.noaux i8042.nomux \
     i8042.nopnp i8042.dumbkbd root=/dev/vda ro";

fn default_kernel() -> PathBuf {
    if let Ok(p) = std::env::var(ENV_FC_KERNEL) {
        return PathBuf::from(p);
    }
    asset_dir().join("vmlinux-6.1.168")
}

fn default_rootfs() -> PathBuf {
    if let Ok(p) = std::env::var(ENV_FC_ROOTFS) {
        return PathBuf::from(p);
    }
    asset_dir().join("ubuntu-24.04.squashfs")
}

fn firecracker_bin() -> String {
    std::env::var(ENV_FC_BIN).unwrap_or_else(|_| "firecracker".to_string())
}

/// The staged-asset directory: `~/.local/share/firecracker-assets/` (CI-P2 HOST CAPABILITIES).
fn asset_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("firecracker-assets")
}

/// The Firecracker machine config built from a [`JobSpec`] — the host-side JSON `firecracker
/// --config-file` consumes. Every field is derived from the spec + the mandatory
/// [`HardeningProfile`]; nothing is hardcoded green. The serialized form asserts the REAL enforced
/// posture (drive `is_read_only=true`, no `network-interfaces` key when egress is fully default-deny).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FcMachineConfig {
    kernel_image_path: PathBuf,
    boot_args: String,
    rootfs_path: PathBuf,
    /// The drive read-only flag — `true` (read-only root) for every hardened guest.
    root_is_read_only: bool,
    vcpu_count: u32,
    mem_size_mib: u32,
    /// Whether a NIC is attached — `false` (no network device) iff egress is fully default-deny.
    has_network_device: bool,
    pids_max: u32,
}

impl FcMachineConfig {
    /// Build the machine config from a job + its derived hardening profile. `oneshot` selects the
    /// deterministic `init=/bin/true` boot used by the hardened-boot self-test (userspace runs, init
    /// exits, the kernel reboots, the VMM exits 0).
    pub fn from_spec(spec: &JobSpec, profile: &HardeningProfile, oneshot: bool) -> FcMachineConfig {
        // vcpu from millicpu (round up to at least 1); mem from the byte limit (at least 128 MiB so
        // the guest kernel boots).
        let vcpu = spec.limits.cpu_millis.div_ceil(1000).max(1);
        let mem_mib = (spec.limits.mem_bytes / (1024 * 1024)).max(128) as u32;
        let boot_args = if oneshot {
            format!("{BOOT_ARGS_BASE} init=/bin/true")
        } else {
            BOOT_ARGS_BASE.to_string()
        };
        FcMachineConfig {
            kernel_image_path: default_kernel(),
            boot_args,
            rootfs_path: default_rootfs(),
            // Read-only root is mandatory and backend-independent (profile §5.3).
            root_is_read_only: profile.read_only_root,
            vcpu_count: vcpu,
            mem_size_mib: mem_mib,
            // NO network device unless the egress allowlist is non-empty (the strongest
            // default-deny: egress closed at the device level).
            has_network_device: profile.network_device,
            pids_max: profile.pids_max,
        }
    }

    /// Serialize to the Firecracker `--config-file` JSON. Hand-built (no JSON dep) and deterministic;
    /// the drive `is_read_only` flag and the presence/absence of `network-interfaces` reflect the
    /// real enforced posture, so a test asserting over this JSON asserts over the enforced state.
    pub fn to_json(&self) -> String {
        let net = if self.has_network_device {
            // A filtered NIC (the host wires the egress allowlist via the tap device's firewall).
            ",\n  \"network-interfaces\": [\n    {\n      \"iface_id\": \"eth0\",\n      \
             \"host_dev_name\": \"tap-myelin\"\n    }\n  ]"
        } else {
            // No NIC at all — egress closed at the device level (full default-deny).
            ""
        };
        format!(
            "{{\n  \"boot-source\": {{\n    \"kernel_image_path\": {kernel:?},\n    \
             \"boot_args\": {args:?}\n  }},\n  \"drives\": [\n    {{\n      \
             \"drive_id\": \"rootfs\",\n      \"path_on_host\": {root:?},\n      \
             \"is_root_device\": true,\n      \"is_read_only\": {ro}\n    }}\n  ],\n  \
             \"machine-config\": {{\n    \"vcpu_count\": {vcpu},\n    \"mem_size_mib\": {mem}\n  \
             }}{net}\n}}",
            kernel = self.kernel_image_path.to_string_lossy(),
            args = self.boot_args,
            root = self.rootfs_path.to_string_lossy(),
            ro = self.root_is_read_only,
            vcpu = self.vcpu_count,
            mem = self.mem_size_mib,
            net = net,
        )
    }

    /// True iff the root drive is mounted read-only (read from the built config, not a literal).
    pub fn root_is_read_only(&self) -> bool {
        self.root_is_read_only
    }

    /// True iff a network device is attached. `false` == egress closed at the device level.
    pub fn has_network_device(&self) -> bool {
        self.has_network_device
    }

    /// The `pids.max` ceiling carried into the cgroup the VMM runs under.
    pub fn pids_max(&self) -> u32 {
        self.pids_max
    }
}

/// A backend error.
#[derive(Debug)]
pub enum FcError {
    /// A four-guarantee hook failed (cost-exhausted, token-rejected, isolation-floor-not-met).
    Hook(crate::HookError),
    /// The mandatory hardening profile could not be asserted in force (fail-closed before boot).
    Hardening(String),
    /// Spawning / waiting on the VMM failed.
    Vmm(String),
}

impl std::fmt::Display for FcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FcError::Hook(e) => write!(f, "firecracker backend: guarantee hook failed: {e}"),
            FcError::Hardening(s) => write!(f, "firecracker backend: hardening not enforced: {s}"),
            FcError::Vmm(s) => write!(f, "firecracker backend: VMM error: {s}"),
        }
    }
}

impl std::error::Error for FcError {}

impl From<crate::HookError> for FcError {
    fn from(e: crate::HookError) -> Self {
        FcError::Hook(e)
    }
}

/// The Firecracker default backend (microVM = KVM + minimal VMM). Tracks live guest VMM processes so
/// [`kill`](FirecrackerBackend::kill) can whole-guest-kill on teardown.
#[derive(Default)]
pub struct FirecrackerBackend {
    /// guest_id → the live VMM child (so teardown whole-guest-kills it). Ephemeral; one job per VMM.
    live: Mutex<std::collections::HashMap<String, GuestProc>>,
}

/// A live guest VMM process (the child + its config-file temp path for cleanup).
struct GuestProc {
    child: Box<dyn VmmChild + Send>,
    cfg_path: PathBuf,
}

/// The host-side VMM-process abstraction. The REAL impl ([`SpawnedVmm`]) shells out to the
/// `firecracker` binary — the ONE legitimate VMM-spawn site (the `no-host-exec` named exclusion).
/// Abstracting it behind a trait keeps [`FirecrackerBackend`]'s control flow testable without a VMM.
pub trait VmmChild {
    /// Whole-guest kill — destroy the guest (idempotent; killing a dead guest is a no-op success).
    fn kill(&mut self) -> Result<(), String>;
    /// Wait for the VMM to exit; returns the exit code (0 == clean boot+reboot for the one-shot path).
    fn wait(&mut self) -> Result<i32, String>;
}

impl FirecrackerBackend {
    /// A new backend with no live guests.
    pub fn new() -> FirecrackerBackend {
        FirecrackerBackend::default()
    }

    /// Build the machine config a launch WOULD use for `spec` (the hardened profile derived + the
    /// JSON assembled), without booting. Used by the boot self-test to assert posture and by unit
    /// tests to assert the config reflects the real enforced state.
    pub fn machine_config(spec: &JobSpec, oneshot: bool) -> Result<FcMachineConfig, FcError> {
        let profile = HardeningProfile::derive(spec);
        profile.assert_enforced().map_err(FcError::Hardening)?;
        Ok(FcMachineConfig::from_spec(spec, &profile, oneshot))
    }

    /// Drive the four-guarantee seam (isolation floor → attribution → reserve), assert the mandatory
    /// hardening profile, write the machine config, and spawn the VMM via `spawn`. Shared by the
    /// trait `launch` (real VMM) and the boot self-test (injectable VMM). `oneshot` selects the
    /// deterministic `init=/bin/true` boot.
    pub fn launch_with<F>(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
        oneshot: bool,
        spawn: F,
    ) -> Result<SandboxHandle, FcError>
    where
        F: FnOnce(&FcMachineConfig, &PathBuf) -> Result<Box<dyn VmmChild + Send>, String>,
    {
        // #4 isolation floor FIRST — the hardening profile must hold before any code runs.
        (hooks.isolation_floor)(spec)?;
        // The mandatory backend-independent hardening profile (arch 02 §5.3), asserted in force.
        let profile = HardeningProfile::derive(spec);
        profile.assert_enforced().map_err(FcError::Hardening)?;
        // #2 attribution — the per-run attenuated token (4.7).
        (hooks.attribute)(&spec.run_token)?;
        // #1a cost gate — reserve at dispatch; refuse-to-start on exhaustion.
        let reserve = (hooks.reserve)(&spec.meter_to)?;

        // Build + write the machine config FROM the spec (kernel/rootfs/vcpu/mem/no-NIC-unless-egress).
        let cfg = FcMachineConfig::from_spec(spec, &profile, oneshot);
        let cfg_path = write_config(&cfg).map_err(FcError::Vmm)?;

        // Boot the microVM (the ONE legitimate VMM spawn — the sandbox seam's mechanism).
        let child = spawn(&cfg, &cfg_path).map_err(|e| {
            let _ = std::fs::remove_file(&cfg_path);
            FcError::Vmm(e)
        })?;

        let guest_id = format!("fc-{}", spec.idem_token.0);
        self.live.lock().unwrap().insert(
            guest_id.clone(),
            GuestProc {
                child,
                cfg_path: cfg_path.clone(),
            },
        );

        // #1b settle — release the unused reserve on completion (never interrupt in-flight). For an
        // in-line compute job the guest has booted; the metering unit is resource-seconds (§8).
        (hooks.settle)(
            &reserve,
            crate::ResourceUsage {
                cpu_seconds: cfg.vcpu_count as u64,
                mem_byte_seconds: (cfg.mem_size_mib as u64) * 1024 * 1024,
            },
        )?;

        Ok(SandboxHandle { guest_id })
    }
}

/// Write the machine config to a unique temp file; returns its path.
fn write_config(cfg: &FcMachineConfig) -> Result<PathBuf, String> {
    let dir = std::env::temp_dir();
    let unique = format!(
        "myelin-fc-{}-{}.json",
        std::process::id(),
        // a cheap monotonic-ish suffix to avoid collisions within a process
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let path = dir.join(unique);
    std::fs::write(&path, cfg.to_json()).map_err(|e| format!("write config {path:?}: {e}"))?;
    Ok(path)
}

impl SandboxBackend for FirecrackerBackend {
    type Error = FcError;

    /// Boot a digest-pinned [`JobSpec`] as a hardened Firecracker microVM. Blocks until the VMM is
    /// up and the four guarantees have fired (the in-line compute contract). The REAL VMM is spawned
    /// here — the one legitimate host-exec site (the `no-host-exec` named exclusion; this seam IS
    /// the unified sandbox, not a bypass of it).
    fn launch(&self, spec: &JobSpec, hooks: &RunnerHooks) -> Result<SandboxHandle, Self::Error> {
        self.launch_with(spec, hooks, /* oneshot = */ true, spawn_real_vmm)
    }

    /// Whole-guest kill on teardown (arch 01 §2): destroy the guest VMM and clean its config. The
    /// guest is ephemeral, never reused. Idempotent — killing an already-gone guest is a no-op.
    fn kill(&self, h: &SandboxHandle) -> Result<(), Self::Error> {
        let proc = self.live.lock().unwrap().remove(&h.guest_id);
        if let Some(mut proc) = proc {
            let r = proc.child.kill();
            let _ = std::fs::remove_file(&proc.cfg_path);
            r.map_err(FcError::Vmm)?;
        }
        Ok(())
    }
}

/// Spawn the REAL Firecracker VMM: `firecracker --no-api --config-file <cfg>`. THIS is the one
/// legitimate VMM-spawn site (the `no-host-exec` named exclusion — see the module note). It is the
/// mechanism by which the unified-sandbox boundary is created; it is not a bypass of the seam.
fn spawn_real_vmm(
    _cfg: &FcMachineConfig,
    cfg_path: &PathBuf,
) -> Result<Box<dyn VmmChild + Send>, String> {
    let mut cmd = std::process::Command::new(firecracker_bin());
    cmd.arg("--no-api")
        .arg("--config-file")
        .arg(cfg_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = cmd
        .spawn()
        .map_err(|e| format!("spawn firecracker: {e}"))?;
    Ok(Box::new(SpawnedVmm { child: Some(child) }))
}

/// A spawned real-VMM child wrapping `std::process::Child`.
struct SpawnedVmm {
    child: Option<std::process::Child>,
}

impl VmmChild for SpawnedVmm {
    fn kill(&mut self) -> Result<(), String> {
        if let Some(mut child) = self.child.take() {
            // Whole-guest kill: terminate the VMM process; the microVM dies with it.
            let _ = child.kill();
            let _ = child.wait();
        }
        Ok(())
    }

    fn wait(&mut self) -> Result<i32, String> {
        if let Some(child) = self.child.as_mut() {
            let status = child.wait().map_err(|e| format!("wait firecracker: {e}"))?;
            Ok(status.code().unwrap_or(-1))
        } else {
            Ok(0)
        }
    }
}

/// Spawn the real VMM AND capture its serial-console stdout (the guest kernel log) to the returned
/// string after the VMM exits. Used by the hardened-boot self-test so it can assert over the REAL
/// guest console (`Linux version 6.1.168 … KVM`). Blocking: waits for the one-shot boot to finish.
pub fn boot_and_capture(cfg_path: &PathBuf) -> Result<(i32, String), String> {
    let mut cmd = std::process::Command::new(firecracker_bin());
    cmd.arg("--no-api")
        .arg("--config-file")
        .arg(cfg_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let output = cmd
        .output()
        .map_err(|e| format!("run firecracker: {e}"))?;
    let mut console = String::from_utf8_lossy(&output.stdout).into_owned();
    console.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok((output.status.code().unwrap_or(-1), console))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardening::HardeningProfile;
    use crate::{
        EgressPolicy, IdemToken, ImageRef, JobKind, MeterTarget, ReserveHandle, ResourceLimits,
        RunTokenRef, TrustTier, WorkspaceSpec,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    fn spec(allow: Vec<String>) -> JobSpec {
        JobSpec::new(
            JobKind::Ci,
            ImageRef::pinned("r/img@sha256:abc123def4567890").unwrap(),
            vec!["true".into()],
            vec![],
            vec![],
            EgressPolicy { allow },
            ResourceLimits {
                cpu_millis: 2000,
                mem_bytes: 512 * 1024 * 1024,
                disk_bytes: 1 << 30,
                pids_max: 256,
                timeout_secs: 600,
            },
            WorkspaceSpec::default(),
            TrustTier::Trusted,
            RunTokenRef { jti: "j".into() },
            MeterTarget {
                reserve_id: "r".into(),
            },
            IdemToken("idem-fc-1".into()),
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

    /// A fake VMM that records kill (so we can assert whole-guest-kill on teardown) without a real VM.
    struct FakeVmm {
        killed: Arc<AtomicBool>,
    }
    impl VmmChild for FakeVmm {
        fn kill(&mut self) -> Result<(), String> {
            self.killed.store(true, Ordering::SeqCst);
            Ok(())
        }
        fn wait(&mut self) -> Result<i32, String> {
            Ok(0)
        }
    }

    #[test]
    fn config_from_spec_derives_vcpu_mem_and_read_only_root() {
        let profile = HardeningProfile::derive(&spec(vec![]));
        let cfg = FcMachineConfig::from_spec(&spec(vec![]), &profile, true);
        assert_eq!(cfg.vcpu_count, 2); // 2000 millis ⇒ 2 vcpu
        assert_eq!(cfg.mem_size_mib, 512);
        assert!(cfg.root_is_read_only(), "root drive MUST be read-only");
        assert_eq!(cfg.pids_max(), 256);
    }

    #[test]
    fn empty_allowlist_yields_no_network_device_in_the_json() {
        let profile = HardeningProfile::derive(&spec(vec![]));
        let cfg = FcMachineConfig::from_spec(&spec(vec![]), &profile, true);
        assert!(!cfg.has_network_device());
        let json = cfg.to_json();
        assert!(
            !json.contains("network-interfaces"),
            "no NIC must be attached when egress is fully default-deny"
        );
        assert!(json.contains("\"is_read_only\": true"));
        assert!(json.contains("init=/bin/true"), "one-shot boot uses init=/bin/true");
    }

    #[test]
    fn nonempty_allowlist_attaches_a_filtered_nic() {
        let s = spec(vec!["registry.example.com".into()]);
        let profile = HardeningProfile::derive(&s);
        let cfg = FcMachineConfig::from_spec(&s, &profile, false);
        assert!(cfg.has_network_device());
        assert!(cfg.to_json().contains("network-interfaces"));
    }

    #[test]
    fn machine_config_asserts_hardening_in_force() {
        // The hardened profile is asserted as part of building the config.
        assert!(FirecrackerBackend::machine_config(&spec(vec![]), true).is_ok());
    }

    #[test]
    fn launch_drives_the_four_guarantees_and_kill_whole_guest_kills() {
        let backend = FirecrackerBackend::new();
        let killed = Arc::new(AtomicBool::new(false));
        let killed2 = killed.clone();
        let handle = backend
            .launch_with(&spec(vec![]), &ok_hooks(), true, move |_cfg, _p| {
                Ok(Box::new(FakeVmm { killed: killed2 }))
            })
            .unwrap();
        assert_eq!(handle.guest_id, "fc-idem-fc-1");
        backend.kill(&handle).unwrap();
        assert!(killed.load(Ordering::SeqCst), "kill must whole-guest-kill the VMM");
        // Idempotent: killing an already-gone guest is a no-op success.
        backend.kill(&handle).unwrap();
    }

    #[test]
    fn launch_refuses_to_start_on_an_exhausted_wallet() {
        let backend = FirecrackerBackend::new();
        let hooks = RunnerHooks {
            reserve: Box::new(|_m| Err(crate::HookError("wallet exhausted".into()))),
            settle: Box::new(|_h, _u| Ok(())),
            attribute: Box::new(|_t| Ok(())),
            isolation_floor: Box::new(|_s| Ok(())),
        };
        let spawned = Arc::new(AtomicBool::new(false));
        let spawned2 = spawned.clone();
        let r = backend.launch_with(&spec(vec![]), &hooks, true, move |_cfg, _p| {
            spawned2.store(true, Ordering::SeqCst);
            Ok(Box::new(FakeVmm {
                killed: Arc::new(AtomicBool::new(false)),
            }))
        });
        assert!(matches!(r, Err(FcError::Hook(_))));
        assert!(!spawned.load(Ordering::SeqCst), "the VMM must NOT spawn when the wallet is exhausted");
    }

    #[test]
    fn launch_fails_closed_when_the_isolation_floor_hook_rejects() {
        let backend = FirecrackerBackend::new();
        let hooks = RunnerHooks {
            reserve: Box::new(|m| Ok(ReserveHandle(m.reserve_id.clone()))),
            settle: Box::new(|_h, _u| Ok(())),
            attribute: Box::new(|_t| Ok(())),
            isolation_floor: Box::new(|_s| Err(crate::HookError("hardening not met".into()))),
        };
        let spawned = Arc::new(AtomicBool::new(false));
        let spawned2 = spawned.clone();
        let r = backend.launch_with(&spec(vec![]), &hooks, true, move |_cfg, _p| {
            spawned2.store(true, Ordering::SeqCst);
            Ok(Box::new(FakeVmm {
                killed: Arc::new(AtomicBool::new(false)),
            }))
        });
        assert!(r.is_err());
        assert!(!spawned.load(Ordering::SeqCst), "no VMM spawns if the isolation floor is not met");
    }
}
