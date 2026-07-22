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

use crate::hardening::{
    enforce_egress, EgressEnforceError, EgressEnforcer, EnforcedEgress, HardeningProfile,
};
use crate::redaction::RedactionPlan;
use crate::{
    drain_capped, JobSpec, ResourceUsage, RunnerHooks, SandboxBackend, SandboxHandle,
    SandboxLaunch, SandboxResult, SANDBOX_CAPTURE_BOUND,
};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

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

// --- The forge-resistant serial-console framing (CT-002a → P-544 / P-548 structural fix). --------
// The guest runs UNTRUSTED `spec.command`; its captured outcome is read off the (shared) serial
// console. The captured exit/streams are UNSPOOFABLE by the job's own output by CONSTRUCTION:
//
//   **PRIMARY (structural) guarantee — the untrusted payload is NON-ROOT and cannot write the
//   console.** The trusted init script ([`build_command_runner_script`]) is PID1/root and the ONLY
//   process that writes the nonce-framed markers below; it runs the untrusted argv under
//   `setpriv --reuid 65534 --regid 65534 --clear-groups` (an unprivileged uid/gid). `/dev/console`
//   and `/dev/ttyS0` are root-only (`crw-------`), so a non-root payload physically CANNOT open
//   them — it cannot inject ANY serial-console line (forged or not), regardless of whether it knows
//   the nonce or how many descendants it forks. This is the same boundary containers/gVisor rely on:
//   the workload runs as a non-privileged uid so it cannot forge the runtime's exit report. (A
//   real-kernel probe confirmed a root payload COULD read the nonce off `/dev/vdb` and write the
//   console — secrecy from root inside the guest is impossible — which is exactly why the boundary
//   is non-root, not nonce-secrecy. See the residual note on a future root-in-guest tier below.)
//
//   DEFENCE IN DEPTH (not the primary guarantee), retained belt-and-braces:
//   1. **base64 framing** — the command's stdout/stderr are captured to tmpfs files and base64-
//      encoded between the markers below, so arbitrary/binary/marker-colliding bytes the job prints
//      land as DATA inside the b64 blob (decoded once, never re-parsed as a marker).
//   2. **a per-boot 256-bit NONCE** — every marker is suffixed with a fresh random nonce the HOST
//      generated for THIS boot ([`boot_nonce`]); the parser only accepts lines bearing the exact
//      nonce. The runner reaps all descendants (`kill -KILL -1`) BEFORE emitting the trusted markers
//      and the host takes the LAST nonce-bearing exit line.
//
// NOTE (residual / future trust-tier): untrusted exec runs NON-ROOT, which is the boundary that
// makes the forge structurally impossible. A future "root-in-guest CI" trust-tier (where the
// workload legitimately needs uid 0 in the guest) would NOT be protected by this boundary and would
// require a host-read-only exit channel the guest workload cannot write — the microVM has no native
// in-guest-process-exit channel (`reboot=k` makes the VMM always exit 0), so that channel would have
// to be built. That mode is NOT built here; do not enable root-in-guest exec against this path.
const MARK_STDOUT_BEGIN: &str = "__MYELIN_STDOUT_B64_BEGIN__";
const MARK_STDOUT_END: &str = "__MYELIN_STDOUT_B64_END__";
const MARK_STDERR_BEGIN: &str = "__MYELIN_STDERR_B64_BEGIN__";
const MARK_STDERR_END: &str = "__MYELIN_STDERR_B64_END__";
const MARK_EXIT: &str = "__MYELIN_EXIT__";

/// Host-side cap on the buffered serial console (CT-002c). Unlike gVisor's two separate pipes, the
/// Firecracker console is ONE stream interleaving the kernel boot log with the nonce-framed base64 of
/// BOTH streams; the per-stream `SANDBOX_CAPTURE_BOUND` head bound is applied AFTER base64-decoding in
/// [`capture_stream`]. base64 inflates 4/3 and coreutils wraps at 76 cols, so two streams each at the
/// 256 KiB bound need ~2×(256 KiB × 4/3 × 1.013) ≈ 710 KiB plus the boot log + markers. 8× the bound
/// (2 MiB) comfortably holds a full LEGITIMATE capture (head-bound semantics preserved — both streams
/// can still reach 256 KiB decoded, and the trailing exit line survives) while HARD-bounding the host
/// drain thread to 2 MiB + the throwaway chunk regardless of how much an untrusted guest emits.
const CONSOLE_CAPTURE_BOUND: usize = 8 * SANDBOX_CAPTURE_BOUND;

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
    /// The recorded proof that the per-tap egress firewall was emitted+applied (R0.1). The NIC is
    /// emitted into the JSON **iff this is `Some`** — the machine config cannot conjure a NIC from a
    /// bare bool; it must carry the applied-ruleset attestation. `None` (no NIC) is the fully
    /// default-deny common case.
    enforced_egress: Option<EnforcedEgress>,
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
            // R0.1: the NIC is gated on the profile's RECORDED enforced-egress ruleset, NOT on the
            // `network_device` requirement bool. A profile that requests a NIC but carries no
            // `enforced_egress` record (enforcement not applied) yields NO NIC here — the machine
            // config and the applied firewall are one indivisible thing. The launch path sets
            // `enforced_egress` only after `enforce_egress` succeeds; `assert_enforced` (run before
            // this) additionally refuses a `network_device` profile with no record, so a NIC-bearing
            // config cannot be built without an applied ruleset.
            enforced_egress: profile.enforced_egress.clone(),
            pids_max: profile.pids_max,
        }
    }

    /// Serialize to the Firecracker `--config-file` JSON. Hand-built (no JSON dep) and deterministic;
    /// the drive `is_read_only` flag and the presence/absence of `network-interfaces` reflect the
    /// real enforced posture, so a test asserting over this JSON asserts over the enforced state.
    pub fn to_json(&self) -> String {
        let net = if self.enforced_egress.is_some() {
            // A filtered NIC — emitted ONLY because an enforced-egress ruleset is recorded (R0.1); the
            // host wired the egress allowlist onto this tap device via the applied nftables firewall.
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

    /// Serialize the **two-drive command-runner** machine config: the read-only squashfs root
    /// (`/dev/vda`) PLUS a SECOND read-only virtio drive (`/dev/vdb`) carrying the command-runner
    /// init script, booted as PID1 bash via `init=/bin/bash /dev/vdb`. This is the SAME hardened
    /// posture [`to_json`](Self::to_json) emits (read-only root, NIC iff egress is non-default-deny,
    /// vcpu/mem from the limits) — it just boots `spec.command`'s runner instead of `/bin/true`. It
    /// REUSES the exact two-drive recipe the AG-D4 escape drill ([`drill_config_json`]) is proven on
    /// (no forked boot mechanism); `script_drive_path` is the host path to the staged runner script.
    pub fn command_runner_json(&self, script_drive_path: &Path) -> String {
        let boot_args = format!("{BOOT_ARGS_BASE} init=/bin/bash /dev/vdb");
        two_drive_config_json(
            &self.kernel_image_path,
            &boot_args,
            &self.rootfs_path,
            self.root_is_read_only,
            script_drive_path,
            // R0.1: NIC iff an enforced-egress ruleset is recorded (not a bare bool).
            self.enforced_egress.is_some(),
            self.vcpu_count,
            self.mem_size_mib,
        )
    }

    /// True iff the root drive is mounted read-only (read from the built config, not a literal).
    pub fn root_is_read_only(&self) -> bool {
        self.root_is_read_only
    }

    /// True iff a network device is attached — which is true iff an enforced-egress ruleset is
    /// recorded (R0.1). `false` == egress closed at the device level. There is no path to a NIC that
    /// does not carry the applied-firewall attestation.
    pub fn has_network_device(&self) -> bool {
        self.enforced_egress.is_some()
    }

    /// The recorded enforced-egress attestation (the applied ruleset), if a NIC is attached (R0.1).
    pub fn enforced_egress(&self) -> Option<&EnforcedEgress> {
        self.enforced_egress.as_ref()
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
    /// R0.1: the per-tap egress firewall could not be emitted+applied (a hostname allowlist entry is
    /// unenforceable, or `nft -f` failed) — the job is REFUSED fail-closed; no NIC is attached.
    Egress(EgressEnforceError),
    /// Spawning / waiting on the VMM failed.
    Vmm(String),
}

impl std::fmt::Display for FcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FcError::Hook(e) => write!(f, "firecracker backend: guarantee hook failed: {e}"),
            FcError::Hardening(s) => write!(f, "firecracker backend: hardening not enforced: {s}"),
            FcError::Egress(e) => write!(f, "firecracker backend: egress not enforced: {e}"),
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

impl From<EgressEnforceError> for FcError {
    fn from(e: EgressEnforceError) -> Self {
        FcError::Egress(e)
    }
}

/// The R0.1 real egress enforcer: install the emitted ruleset on the host via `nft -f -` (reading the
/// ruleset from stdin). This is a legitimate host-config action — it CREATES the egress boundary for
/// the guest's tap device, exactly as spawning the VMM creates the isolation boundary — and it lives
/// in THIS file, which is the `no-host-exec` named-exclusion site (registered in
/// `myelin-lints/src/bin/lint-gate.rs` + `tests/workspace_clean.rs`; see the module note above). The
/// `Command::new("nft")` fingerprint would trip the `no-host-exec` lint on any other production file,
/// so the apply site is deliberately colocated here rather than in a fresh (linted) module — and it is
/// injectable ([`EgressEnforcer`]) so production wires this real impl at boot while tests drive the
/// fail-closed control flow with a double. FAIL-CLOSED: a non-zero `nft` exit (or a missing/incapable
/// `nft`) returns [`EgressEnforceError::ApplyFailed`] and the caller attaches NO NIC.
#[derive(Default)]
pub struct NftEgressEnforcer;

impl EgressEnforcer for NftEgressEnforcer {
    fn apply(&self, ruleset: &str) -> Result<EnforcedEgress, EgressEnforceError> {
        use std::io::Write;
        let mut child = Command::new("nft")
            .arg("-f")
            .arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| EgressEnforceError::ApplyFailed(format!("spawn nft: {e}")))?;
        child
            .stdin
            .take()
            .ok_or_else(|| EgressEnforceError::ApplyFailed("nft stdin unavailable".into()))?
            .write_all(ruleset.as_bytes())
            .map_err(|e| EgressEnforceError::ApplyFailed(format!("write ruleset to nft: {e}")))?;
        let out = child
            .wait_with_output()
            .map_err(|e| EgressEnforceError::ApplyFailed(format!("wait nft: {e}")))?;
        if !out.status.success() {
            return Err(EgressEnforceError::ApplyFailed(format!(
                "nft -f exited {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        // The ruleset is now IN FORCE on the tap device — record the exact text as the attestation.
        Ok(EnforcedEgress::new(ruleset.to_string()))
    }
}

/// The Firecracker default backend (microVM = KVM + minimal VMM). Tracks live guest VMM processes so
/// [`kill`](FirecrackerBackend::kill) can whole-guest-kill on teardown. Carries the injectable R0.1
/// [`EgressEnforcer`] (production: [`NftEgressEnforcer`]) used to apply the per-tap egress firewall
/// before any egress-capable NIC is attached.
pub struct FirecrackerBackend {
    /// guest_id → the live VMM child (so teardown whole-guest-kills it). Ephemeral; one job per VMM.
    live: Mutex<std::collections::HashMap<String, GuestProc>>,
    /// The egress-firewall apply seam (R0.1). Injectable so unit tests drive the fail-closed flow
    /// without root / `nft`; production wires [`NftEgressEnforcer`].
    egress_enforcer: Box<dyn EgressEnforcer + Send + Sync>,
}

impl Default for FirecrackerBackend {
    fn default() -> FirecrackerBackend {
        FirecrackerBackend {
            live: Mutex::default(),
            egress_enforcer: Box::new(NftEgressEnforcer),
        }
    }
}

/// A live guest VMM process (the child + its config-file temp path for cleanup).
struct GuestProc {
    child: Box<dyn VmmChild + Send>,
    cfg_path: PathBuf,
}

/// What a launch's run-closure hands back to [`FirecrackerBackend::launch_with`]: the spawned VMM
/// child (so teardown can whole-guest-kill it), the temp config path (cleaned on teardown), and the
/// **already-captured** [`SandboxResult`] for the command (CT-002a — the seam now carries a REAL
/// result, no longer a stub). The real production closure ([`run_production_guest`]) boots a microVM,
/// runs `spec.command`, and fills this from the serial console; unit tests inject a [`FakeVmm`] + a
/// canned result so the four-guarantee control flow is testable without a VMM.
pub struct GuestRun {
    /// The spawned (and, by the time this is returned, already-exited/killed) VMM child.
    pub child: Box<dyn VmmChild + Send>,
    /// The temp machine-config path to remove on teardown.
    pub cfg_path: PathBuf,
    /// The captured command result (exit / timeout / usage / bounded streams).
    pub result: SandboxResult,
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
    /// A new backend with no live guests (production [`NftEgressEnforcer`] wired).
    pub fn new() -> FirecrackerBackend {
        FirecrackerBackend::default()
    }

    /// A backend with an injected [`EgressEnforcer`] (R0.1) — used by unit tests to drive the
    /// fail-closed egress control flow without root / `nft`.
    pub fn with_enforcer(enforcer: Box<dyn EgressEnforcer + Send + Sync>) -> FirecrackerBackend {
        FirecrackerBackend {
            live: Mutex::default(),
            egress_enforcer: enforcer,
        }
    }

    /// Build the machine config a launch WOULD use for `spec` (the hardened profile derived + the
    /// JSON assembled), without booting. Used by the boot self-test to assert posture and by unit
    /// tests to assert the config reflects the real enforced state.
    ///
    /// R0.1: this helper does NOT apply the egress firewall (it has no enforcer seam), so it is
    /// meaningful only for the no-egress common case. An egress-REQUESTING spec (non-empty allowlist)
    /// is refused fail-closed here — `assert_enforced` rejects the derived profile because it claims a
    /// network device without a recorded enforced-egress ruleset. A NIC-bearing config is producible
    /// only through the real launch path ([`launch`](FirecrackerBackend::launch)), which applies+records
    /// the firewall first.
    pub fn machine_config(spec: &JobSpec, oneshot: bool) -> Result<FcMachineConfig, FcError> {
        let profile = HardeningProfile::derive(spec);
        profile.assert_enforced().map_err(FcError::Hardening)?;
        Ok(FcMachineConfig::from_spec(spec, &profile, oneshot))
    }

    /// Drive the four-guarantee seam in the mandated order — **isolation floor → hardening assert →
    /// attribution → reserve → run → settle** — fail-closed at every step, then hand the captured
    /// [`SandboxResult`] back behind the redrawn CT-001 seam. The `run` closure does the actual boot:
    /// it builds/writes whatever machine config it needs, spawns the VMM, runs `spec.command`,
    /// captures the real exit/streams/usage, and returns a [`GuestRun`]. The real trait `launch`
    /// passes [`run_production_guest`] (a REAL microVM); unit tests pass a closure returning a
    /// [`FakeVmm`] + a canned result so the control flow is testable without a VMM (the injectable-
    /// spawn seam — preserved). `run` is only invoked AFTER reserve succeeds, so an exhausted wallet /
    /// unmet isolation floor refuses-to-start and the VMM never spawns (CT-002a: this is what
    /// converges the CT-001 follow-up — the result is now CONSUMED from the run, never hardcoded).
    pub fn launch_with<F>(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
        run: F,
    ) -> Result<SandboxLaunch, FcError>
    where
        F: FnOnce(&JobSpec, &HardeningProfile) -> Result<GuestRun, String>,
    {
        // #4 isolation floor FIRST — the hardening profile must hold before any code runs.
        (hooks.isolation_floor)(spec)?;
        // The mandatory backend-independent hardening profile (arch 02 §5.3).
        let mut profile = HardeningProfile::derive(spec);
        // R0.1 (DELTA now-live HIGH): if the job requests egress (non-empty allowlist), EMIT+APPLY+
        // RECORD the per-tap egress firewall BEFORE we assert or attach anything — fail-closed. A
        // hostname allowlist entry (unenforceable) or an `nft -f` failure returns Err here and the
        // run closure is NEVER invoked, so no NIC-bearing config is ever produced. Only on success
        // does the profile carry the `enforced_egress` record that authorises the NIC. The empty-
        // allowlist common case is a no-op (`Ok(None)`) — no NIC, unchanged.
        profile.enforced_egress = enforce_egress(&profile, self.egress_enforcer.as_ref())?;
        // The mandatory profile is now asserted in force — including R0.1's honesty check that a
        // network-device profile carries the enforced-egress record just recorded above.
        profile.assert_enforced().map_err(FcError::Hardening)?;
        // #2 attribution — the per-run attenuated token (4.7).
        (hooks.attribute)(&spec.run_token)?;
        // #1a cost gate — reserve at dispatch; refuse-to-start on exhaustion (BEFORE any boot).
        let reserve = (hooks.reserve)(&spec.meter_to)?;

        // Boot the microVM + run spec.command + capture the REAL result (the ONE legitimate VMM
        // spawn — the sandbox seam's mechanism; the `no-host-exec` named exclusion). `run` cleans up
        // its own temp files on error.
        let GuestRun {
            child,
            cfg_path,
            result,
        } = run(spec, &profile).map_err(FcError::Vmm)?;

        let guest_id = format!("fc-{}", spec.idem_token.0);
        self.live
            .lock()
            .unwrap()
            .insert(guest_id.clone(), GuestProc { child, cfg_path });

        // #1b settle — release the unused reserve on completion (never interrupt in-flight), now
        // settling against the result's REAL measured usage (CT-002a).
        (hooks.settle)(&reserve, result.usage)?;

        Ok(SandboxLaunch {
            handle: SandboxHandle { guest_id },
            result,
        })
    }
}

/// The REAL production run path (CT-002a → P-544): boot a hardened Firecracker microVM that RUNS the
/// untrusted `spec.command` and capture its real outcome. Mechanism (REUSING the AG-D4 escape-drill
/// recipe — a SECOND read-only virtio drive + `init=/bin/bash /dev/vdb`, no forked boot mechanism):
///
/// 1. Mint a per-boot 256-bit nonce ([`boot_nonce`]) and generate a command-runner init script
///    ([`build_command_runner_script`]) that exports `spec.env`, runs `spec.command` under the
///    hardened in-guest posture (caps-dropped + no-new-privs AND dropped to a NON-ROOT uid via
///    `setpriv` so the payload cannot write the root-only console — the structural forge boundary),
///    captures its stdout/stderr to tmpfs, and prints them base64-framed + the real exit code —
///    every marker nonce-suffixed (the non-root boundary makes the forge impossible; nonce is DiD).
/// 2. Stage the script on a block-boundary-padded host file (the 2nd virtio drive) and write the
///    two-drive machine config ([`FcMachineConfig::command_runner_json`]) — read-only root, NIC iff
///    egress is non-default-deny, vcpu/mem/pids from the derived profile.
/// 3. Boot with a wall-clock timeout = `spec.limits.timeout_secs` ([`spawn_and_capture`]); on expiry
///    the whole guest is killed (`timed_out=true`, `exit_code=None`).
/// 4. Parse the serial console for the nonce-framed exit + base64 streams ([`build_result_from_console`]).
fn run_production_guest(spec: &JobSpec, profile: &HardeningProfile) -> Result<GuestRun, String> {
    let nonce = boot_nonce()?;
    let script = build_command_runner_script(spec, &nonce);
    let script_drive = stage_padded_script(&script)?;

    let cfg = FcMachineConfig::from_spec(spec, profile, /* oneshot = */ false);
    let cfg_json = cfg.command_runner_json(&script_drive);
    let cfg_path = match write_config_json(&cfg_json) {
        Ok(p) => p,
        Err(e) => {
            let _ = std::fs::remove_file(&script_drive);
            return Err(e);
        }
    };

    let timeout = Duration::from_secs(spec.limits.timeout_secs as u64);
    let (child, outcome) = match spawn_and_capture(&cfg_path, Some(timeout)) {
        Ok(v) => v,
        Err(e) => {
            let _ = std::fs::remove_file(&script_drive);
            let _ = std::fs::remove_file(&cfg_path);
            return Err(e);
        }
    };
    // The script drive has been fully read by the guest; remove it now (the guest is gone).
    let _ = std::fs::remove_file(&script_drive);

    let result = build_result_from_console(&outcome, &nonce, &cfg, &RedactionPlan::for_job(spec));
    Ok(GuestRun {
        child: Box::new(child),
        cfg_path,
        result,
    })
}

/// A cheap monotonic-ish suffix to avoid temp-file collisions within a process.
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// Write a machine-config JSON to a unique temp file; returns its path.
fn write_config_json(json: &str) -> Result<PathBuf, String> {
    let path = std::env::temp_dir().join(format!(
        "myelin-fc-{}-{}.json",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::write(&path, json).map_err(|e| format!("write config {path:?}: {e}"))?;
    Ok(path)
}

/// Mint a fresh per-boot 256-bit nonce (hex) from `/dev/urandom`. The nonce makes the serial-console
/// markers unforgeable by the untrusted guest payload (which cannot predict it). FAIL-CLOSED: a weak
/// or unreadable nonce would weaken forge-resistance, so an unreadable `/dev/urandom` is an error.
fn boot_nonce() -> Result<String, String> {
    let mut f =
        std::fs::File::open("/dev/urandom").map_err(|e| format!("open /dev/urandom: {e}"))?;
    let mut buf = [0u8; 32];
    f.read_exact(&mut buf)
        .map_err(|e| format!("read /dev/urandom: {e}"))?;
    let mut hex = String::with_capacity(64);
    for b in buf {
        hex.push_str(&format!("{b:02x}"));
    }
    Ok(hex)
}

/// Single-quote-escape a string for safe inclusion in the generated bash script (`'…'` with embedded
/// `'` rendered as `'\''`). The untrusted `spec.command` argv + `spec.env` values flow through this,
/// so a value containing quotes/metacharacters/newlines cannot break out of its quoting into the
/// runner script (no shell-injection of the host-side script generation).
fn sh_squote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// Build the in-guest **command-runner init script** (PID1 bash via `init=/bin/bash /dev/vdb`). It
/// exports `spec.env` (non-secret literals; secret VALUES are NEVER placed in the spec nor echoed to
/// the console), runs the untrusted `spec.command` under the hardened in-guest posture — caps
/// dropped + no-new-privs AND **dropped to a NON-ROOT uid/gid (65534)** via `setpriv`, so the
/// payload physically cannot open the root-only serial console and therefore cannot inject ANY
/// console line. The payload's stdout/stderr are redirected (by root, BEFORE setpriv drops privs) to
/// tmpfs files whose already-open fds the non-root argv inherits; init then reaps descendants and
/// prints the captured streams base64-framed + the real exit code, each marker nonce-suffixed.
/// The non-root boundary is the PRIMARY (structural) forge guarantee; base64 + nonce + reap are
/// defence in depth (see the marker consts above).
///
/// CT-003a (SI-017) adds two in-guest resource-limit enforcements derived from `spec.limits`: (1) a
/// `ulimit -u spec.limits.pids_max` (RLIMIT_NPROC) applied in the runner subshell BEFORE `setpriv`,
/// inherited by the non-root payload + all descendants, so a fork bomb is refused at the ceiling and
/// the guest survives (rather than OOM-dying); (2) a size-bounded `/run/scratch` tmpfs sized from
/// `spec.limits.disk_bytes` (the profile's scratch quota), separate from the `/run` capture area, so a
/// workload disk fill hits ENOSPC at the quota. (The whole-guest RAM bounds everything else — the
/// microVM keeps the HOST safe regardless.)
fn build_command_runner_script(spec: &JobSpec, nonce: &str) -> String {
    let mut exports = String::new();
    for ev in &spec.env {
        // Both the value is single-quote-escaped; the name is a shell identifier (export NAME=val).
        exports.push_str(&format!("export {}={}\n", ev.name, sh_squote(&ev.value)));
    }
    // The untrusted argv as space-joined single-quoted tokens — passed verbatim to setpriv/exec.
    let argv: String = spec
        .command
        .iter()
        .map(|a| sh_squote(a))
        .collect::<Vec<_>>()
        .join(" ");

    format!(
        r#"# Myelin CT-002a command-runner init (PID1 bash, hardened Firecracker microVM, REAL KVM kernel).
# Untrusted spec.command runs here; the REAL streams/exit are framed by the per-boot NONCE the guest
# payload cannot predict, and base64-encoded so the payload's own output can never spoof a marker.
export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
mount -t proc proc /proc 2>/dev/null
mount -t sysfs sys /sys 2>/dev/null
mount -t tmpfs tmpfs /run 2>/dev/null
# CT-003a (SI-017): a SIZE-BOUNDED writable scratch tmpfs (sized from spec.limits.disk_bytes, the
# profile's scratch quota) at /run/scratch — a SEPARATE mount from /run (which holds the trusted
# capture files /run/myelin.{{out,err}}), so a workload disk fill hits ENOSPC at the quota WITHOUT
# starving the capture or the trusted markers. mode=1777 so the NON-ROOT payload can write it. (The
# whole-guest RAM bounds everything else — the microVM keeps the HOST safe regardless; this makes the
# in-guest disk quota the enforcer for the workload's scratch.)
mkdir -p /run/scratch 2>/dev/null
mount -t tmpfs -o size={disk_bytes},mode=1777 tmpfs /run/scratch 2>/dev/null
N='{nonce}'
# STRUCTURAL forge boundary: init (this script) is PID1/root and is the ONLY writer of the
# nonce-framed markers below. The untrusted argv runs NON-ROOT (--reuid/--regid 65534 =
# nobody/nogroup, numeric so no /etc/passwd lookup, --clear-groups drops supplementary gids), so
# /dev/console + /dev/ttyS0 (root-only crw-------) are UNOPENABLE by the payload — it physically
# cannot inject ANY console line (forged or otherwise) even knowing the nonce. The redirect of the
# payload's stdout/stderr to /run is performed HERE, by root, in the subshell BEFORE setpriv drops
# privileges, so the non-root argv inherits those already-open fds and capture still works.
# CT-003a (SI-017): apply RLIMIT_NPROC = spec.limits.pids_max IN-GUEST via `ulimit -u` in the runner
# subshell BEFORE setpriv drops to the non-root uid — rlimits inherit across setpriv/exec, so the
# untrusted payload (uid 65534) and ALL its descendants are capped at the pids ceiling. A fork bomb is
# REFUSED at the ceiling (fork() → EAGAIN) instead of growing until the guest OOM-dies, so the guest
# SURVIVES and the rest of the run (e.g. the disk-fill probe) still executes. The ceiling counts
# per-real-uid; root init/runner processes (uid 0) are unaffected.
{exports}( exec </dev/null >/run/myelin.out 2>/run/myelin.err
ulimit -u {pids_max} 2>/dev/null
setpriv --reuid 65534 --regid 65534 --clear-groups \
        --no-new-privs --bounding-set -all --inh-caps -all --ambient-caps -all {argv} )
CODE=$?
# Reap any descendants the untrusted command spawned BEFORE emitting the trusted nonce-framed
# markers. The structural guarantee is above (the payload is non-root and cannot write the console);
# the reap + taking the LAST nonce-exit line + the unpredictable nonce are DEFENCE IN DEPTH, not the
# primary guarantee.
kill -KILL -1 2>/dev/null
printf '%s:%s\n' "{so_b}" "$N"
base64 /run/myelin.out 2>/dev/null
printf '%s:%s\n' "{so_e}" "$N"
printf '%s:%s\n' "{se_b}" "$N"
base64 /run/myelin.err 2>/dev/null
printf '%s:%s\n' "{se_e}" "$N"
printf '%s:%s:%s\n' "{ex}" "$N" "$CODE"
sync
reboot -f
"#,
        nonce = nonce,
        exports = exports,
        argv = argv,
        disk_bytes = spec.limits.disk_bytes,
        pids_max = spec.limits.pids_max,
        so_b = MARK_STDOUT_BEGIN,
        so_e = MARK_STDOUT_END,
        se_b = MARK_STDERR_BEGIN,
        se_e = MARK_STDERR_END,
        ex = MARK_EXIT,
    )
}

/// Stage the runner script on a host file padded to an 8 KiB block boundary — a Firecracker drive
/// smaller than one 512-byte sector presents as 0 blocks in-guest (the script would be unreadable),
/// so we pad it with a trailing bash comment (harmless). Mirrors the escape-drill staging. Returns
/// the host path of the 2nd virtio drive.
fn stage_padded_script(script: &str) -> Result<PathBuf, String> {
    let mut bytes = script.as_bytes().to_vec();
    bytes.push(b'\n');
    bytes.push(b'#'); // comment out the padding so bash ignores it
    while bytes.len() < 8192 {
        bytes.push(b'#');
    }
    bytes.push(b'\n');
    let path = std::env::temp_dir().join(format!(
        "myelin-fc-cmd-{}-{}.sh",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::write(&path, &bytes).map_err(|e| format!("write script drive {path:?}: {e}"))?;
    Ok(path)
}

impl SandboxBackend for FirecrackerBackend {
    type Error = FcError;

    /// Boot a digest-pinned [`JobSpec`] as a hardened Firecracker microVM. Blocks until the VMM is
    /// up and the four guarantees have fired (the in-line compute contract). The REAL VMM is spawned
    /// here — the one legitimate host-exec site (the `no-host-exec` named exclusion; this seam IS
    /// the unified sandbox, not a bypass of it).
    fn launch(&self, spec: &JobSpec, hooks: &RunnerHooks) -> Result<SandboxLaunch, Self::Error> {
        self.launch_with(spec, hooks, run_production_guest)
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

/// A spawned real-VMM child wrapping `std::process::Child`. By the time a [`GuestRun`] hands one of
/// these to the live map the VMM has already exited (or been timeout-killed) by [`spawn_and_capture`],
/// so `child` is `None` and [`kill`](VmmChild::kill) is an idempotent no-op (whole-guest-kill already
/// happened). Carrying the `Option<Child>` keeps the teardown contract identical.
struct SpawnedVmm {
    child: Option<Child>,
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

/// The raw outcome of running the VMM to completion (or to a timeout-kill) — consumed by
/// [`build_result_from_console`] into a [`SandboxResult`].
struct CaptureOutcome {
    /// The VMM process exit code (the one-shot guest reboots → the VMM exits 0). `None` if killed.
    exit: Option<i32>,
    /// True iff the wall-clock `timeout_secs` ceiling fired and the whole guest was killed.
    timed_out: bool,
    /// The captured serial console (guest stdout + stderr).
    console: String,
    /// Wall-clock duration the VMM was resident.
    wall: Duration,
    /// Host-side CPU-seconds the VMM process consumed (utime+stime from `/proc/<pid>/stat`, sampled
    /// just before exit), if readable — a REAL measurement, not a fabricated figure.
    cpu_seconds: Option<u64>,
}

/// Read the firecracker VMM process's cumulative CPU time (utime+stime) from `/proc/<pid>/stat`,
/// in whole seconds. Uses the kernel `/proc` clock-tick ABI (USER_HZ = 100 on Linux). Returns `None`
/// if `/proc` is unavailable or unparseable (then the caller falls back to a wall-clock figure).
fn read_proc_cpu_seconds(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The comm field (field 2) is parenthesised and may contain spaces/`)`; skip past the LAST ')'.
    let rest = &stat[stat.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // After comm, the fields are: state(3) ppid(4) ... utime(14) stime(15) ... ⇒ rest indices 11/12.
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some((utime + stime) / 100)
}

/// Spawn the REAL Firecracker VMM (`firecracker --no-api --config-file <cfg>`) — THE one legitimate
/// VMM-spawn site (the `no-host-exec` named exclusion; this is the mechanism that CREATES the
/// isolation boundary, not a bypass of it) — drain its serial console without deadlocking (the
/// stdout/stderr pipes are read on dedicated threads), and wait at most `timeout` for it to exit.
/// On timeout the WHOLE GUEST is killed (`child.kill()` → the microVM dies with the VMM) and
/// `timed_out` is set. Returns the (already-exited) child wrapper for idempotent teardown + the
/// captured outcome. `timeout = None` blocks to completion (the one-shot self-test / escape-drill path).
fn spawn_and_capture(
    cfg_path: &Path,
    timeout: Option<Duration>,
) -> Result<(SpawnedVmm, CaptureOutcome), String> {
    let mut child = Command::new(firecracker_bin())
        .arg("--no-api")
        .arg("--config-file")
        .arg(cfg_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn firecracker: {e}"))?;

    let pid = child.id();
    let start = Instant::now();

    // Drain both pipes on threads so a chatty guest console cannot fill a pipe buffer and deadlock.
    let mut out = child.stdout.take().expect("piped stdout");
    let mut err = child.stderr.take().expect("piped stderr");
    // CT-002c: cap each drained stream at CONSOLE_CAPTURE_BOUND (head capture) and DISCARD the rest to
    // EOF — bounds host memory under a runaway guest while still draining the pipe so the guest never
    // blocks on a full pipe (no deadlock that would defeat the timeout).
    let th_out = std::thread::spawn(move || drain_capped(&mut out, CONSOLE_CAPTURE_BOUND).0);
    let th_err = std::thread::spawn(move || drain_capped(&mut err, CONSOLE_CAPTURE_BOUND).0);

    let mut timed_out = false;
    let mut last_cpu: Option<u64> = None;
    let exit = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|e| format!("wait firecracker: {e}"))?
        {
            break status.code();
        }
        if let Some(c) = read_proc_cpu_seconds(pid) {
            last_cpu = Some(c);
        }
        if let Some(t) = timeout {
            if start.elapsed() >= t {
                // Wall-clock ceiling hit: whole-guest-kill the VMM and reap it.
                let _ = child.kill();
                let _ = child.wait();
                timed_out = true;
                break None;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let wall = start.elapsed();

    // The child has exited/been-killed ⇒ the pipes hit EOF ⇒ the drain threads finish.
    let out_buf = th_out.join().unwrap_or_default();
    let err_buf = th_err.join().unwrap_or_default();
    let mut console = String::from_utf8_lossy(&out_buf).into_owned();
    console.push_str(&String::from_utf8_lossy(&err_buf));

    Ok((
        SpawnedVmm { child: None },
        CaptureOutcome {
            exit,
            timed_out,
            console,
            wall,
            cpu_seconds: last_cpu,
        },
    ))
}

/// Boot the VMM and capture its serial console (the guest kernel log + any in-guest output), blocking
/// to completion (no timeout — the one-shot self-test / escape drill reboots quickly). Used by
/// `tests/hardened_boot_selftest.rs` + `tests/escape_drill_test.rs`. Delegates to [`spawn_and_capture`]
/// (the single VMM-spawn mechanism) so the boot path does not fork.
pub fn boot_and_capture(cfg_path: &Path) -> Result<(i32, String), String> {
    let (_child, outcome) = spawn_and_capture(cfg_path, None)?;
    Ok((outcome.exit.unwrap_or(-1), outcome.console))
}

/// Parse a captured console + per-boot `nonce` into the real [`SandboxResult`] (CT-002a). The exit
/// code is the LAST nonce-framed `__MYELIN_EXIT__:<nonce>:<code>` line (forge-resistant — see the
/// marker consts); the streams are the base64-decoded blobs between the nonce-framed begin/end
/// markers, HEAD-bounded to [`SANDBOX_CAPTURE_BOUND`] bytes EACH. A timeout / boot failure (markers
/// absent) yields `exit_code = None` (surfaced honestly — never fabricated as 0). Usage is the REAL
/// measured figure: host CPU-seconds from `/proc` (or a wall-clock ceil fallback) + mem-byte-seconds.
fn build_result_from_console(
    o: &CaptureOutcome,
    nonce: &str,
    cfg: &FcMachineConfig,
    redaction: &RedactionPlan,
) -> SandboxResult {
    // BOUNDARY REDACTION (CT-004f sub-step 1): mask the job's CI-managed secret needles in the captured
    // console streams before they populate `SandboxResult` (a required argument — no capture path can
    // forward un-redacted bytes; empty today, populated by CI-1 injection — see `crate::redaction`).
    let stdout = redaction.redact(&capture_stream(
        &o.console,
        MARK_STDOUT_BEGIN,
        MARK_STDOUT_END,
        nonce,
    ));
    let stderr = redaction.redact(&capture_stream(
        &o.console,
        MARK_STDERR_BEGIN,
        MARK_STDERR_END,
        nonce,
    ));
    // A timed-out guest was killed mid-flight ⇒ no trustworthy exit code (do NOT fabricate one).
    let exit_code = if o.timed_out {
        None
    } else {
        parse_exit(&o.console, nonce)
    };

    // Real wall-clock-derived metering (resource-seconds, arch §8). cpu_seconds prefers the measured
    // host CPU of the VMM process (utime+stime); when that sampled to 0 (a sub-second boot) we fall
    // back to the wall-clock ceiling so a real run never under-meters to 0. Finer per-guest cgroup
    // cpu.stat accounting is the fleet refinement (CI-P14) — documented, not faked.
    let wall_secs_ceil = o.wall.as_secs() + u64::from(o.wall.subsec_nanos() > 0);
    let cpu_seconds = o.cpu_seconds.filter(|c| *c > 0).unwrap_or(wall_secs_ceil);
    let mem_byte_seconds = (cfg.mem_size_mib as u64) * 1024 * 1024 * wall_secs_ceil;

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

/// Parse the LAST `__MYELIN_EXIT__:<nonce>:<code>` line bearing the exact per-boot `nonce`. The
/// untrusted payload runs non-root and cannot write the console AT ALL (the structural guarantee), so
/// no forged exit line can ever reach this parser; taking the LAST occurrence + the unpredictable
/// nonce + descendant-reaping are defence in depth. A wrong-nonce or nonce-less line never matches.
fn parse_exit(console: &str, nonce: &str) -> Option<i32> {
    let prefix = format!("{MARK_EXIT}:{nonce}:");
    let mut last = None;
    for line in console.lines() {
        if let Some(code) = line.trim().strip_prefix(&prefix) {
            if let Ok(v) = code.trim().parse::<i32>() {
                last = Some(v);
            }
        }
    }
    last
}

/// Extract one captured stream: the base64 blob between the LAST nonce-framed `begin:<nonce>` line and
/// the following `end:<nonce>` line, decoded and HEAD-bounded to [`SANDBOX_CAPTURE_BOUND`]. Serial
/// consoles may CRLF-translate, so lines are trimmed and the decoder ignores non-alphabet bytes.
fn capture_stream(console: &str, begin: &str, end: &str, nonce: &str) -> Vec<u8> {
    let begin_marker = format!("{begin}:{nonce}");
    let end_marker = format!("{end}:{nonce}");
    let lines: Vec<&str> = console.lines().collect();
    let Some(start) = lines.iter().rposition(|l| l.trim() == begin_marker) else {
        return Vec::new();
    };
    let mut b64 = String::new();
    for l in &lines[start + 1..] {
        if l.trim() == end_marker {
            break;
        }
        b64.push_str(l);
    }
    let mut data = b64_decode(&b64);
    if data.len() > SANDBOX_CAPTURE_BOUND {
        data.truncate(SANDBOX_CAPTURE_BOUND); // HEAD capture (documented; full stream → firehose)
    }
    data
}

/// A small dependency-free standard-base64 decoder (the streams the in-guest runner emits are
/// base64-encoded so binary/marker-colliding output cannot break the framing). Non-alphabet bytes
/// (whitespace, CR, the coreutils 76-col wrapping) are skipped; padding (`=`) ends the stream.
fn b64_decode(input: &str) -> Vec<u8> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::new();
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    for &c in input.as_bytes() {
        if c == b'=' {
            break;
        }
        let Some(v) = val(c) else { continue };
        buf = (buf << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    out
}

/// Build the Firecracker `--config-file` JSON for the **AG-D4 escape drill** (CI-P5 → P-239): the
/// SAME staged kernel + read-only squashfs rootfs the hardened-boot self-test boots, plus a SECOND
/// read-only virtio drive (`/dev/vdb`) carrying the adversarial corpus script, booted as PID1 bash
/// via `init=/bin/bash /dev/vdb`. The corpus prints per-attack CONTAINED/ESCAPED markers to the
/// serial console (`ttyS0`), which [`boot_and_capture`] captures for the host-side parser.
///
/// This REUSES the production Firecracker launch recipe (the same `vmlinux` + squashfs assets, the
/// same read-only-root cmdline base, the same no-NIC fully-default-deny posture — there is no
/// `network-interfaces` key) — it does not fork the backend. `script_drive_path` is the host path to
/// the (block-boundary-padded) corpus script; `vcpu`/`mem_mib` size the guest.
pub fn drill_config_json(script_drive_path: &std::path::Path, vcpu: u32, mem_mib: u32) -> String {
    // The corpus runs as PID1 bash reading the script from the second virtio drive (/dev/vdb).
    // root=/dev/vda ro keeps the rootfs READ-ONLY (the read-only-root posture, enforced at the
    // kernel cmdline); no `network-interfaces` key ⇒ NO NIC ⇒ egress closed at the device level.
    let boot_args = format!("{BOOT_ARGS_BASE} init=/bin/bash /dev/vdb");
    two_drive_config_json(
        &default_kernel(),
        &boot_args,
        &default_rootfs(),
        /* root_read_only = */ true,
        script_drive_path,
        /* has_network = */ false,
        vcpu,
        mem_mib,
    )
}

/// The shared **two-drive** Firecracker `--config-file` JSON formatter (hand-built, no JSON dep): the
/// read-only squashfs root (`/dev/vda`) + a SECOND read-only virtio drive (`/dev/vdb`, the script),
/// optionally a filtered NIC. Hosts BOTH the AG-D4 escape-drill config ([`drill_config_json`]) and the
/// production command-runner config ([`FcMachineConfig::command_runner_json`]) so the proven two-drive
/// recipe lives in ONE place (anti-duplication). The `network-interfaces` block matches
/// [`FcMachineConfig::to_json`] exactly; with `has_network=false` the bytes are identical to the
/// drill's original hand-written config.
#[allow(clippy::too_many_arguments)]
fn two_drive_config_json(
    kernel: &Path,
    boot_args: &str,
    rootfs: &Path,
    root_read_only: bool,
    script: &Path,
    has_network: bool,
    vcpu: u32,
    mem_mib: u32,
) -> String {
    let net = if has_network {
        ",\n  \"network-interfaces\": [\n    {\n      \"iface_id\": \"eth0\",\n      \
         \"host_dev_name\": \"tap-myelin\"\n    }\n  ]"
    } else {
        ""
    };
    format!(
        "{{\n  \"boot-source\": {{\n    \"kernel_image_path\": {kernel:?},\n    \
         \"boot_args\": {args:?}\n  }},\n  \"drives\": [\n    {{\n      \
         \"drive_id\": \"rootfs\",\n      \"path_on_host\": {root:?},\n      \
         \"is_root_device\": true,\n      \"is_read_only\": {ro}\n    }},\n    {{\n      \
         \"drive_id\": \"script\",\n      \"path_on_host\": {script:?},\n      \
         \"is_root_device\": false,\n      \"is_read_only\": true\n    }}\n  ],\n  \
         \"machine-config\": {{\n    \"vcpu_count\": {vcpu},\n    \"mem_size_mib\": {mem}\n  \
         }}{net}\n}}",
        kernel = kernel.to_string_lossy(),
        args = boot_args,
        root = rootfs.to_string_lossy(),
        ro = root_read_only,
        script = script.to_string_lossy(),
        vcpu = vcpu,
        mem = mem_mib,
        net = net,
    )
}

/// The resolved staged kernel path (env override → `~/.local/share/firecracker-assets/vmlinux-…`).
/// Public so the escape drill can sha256 the kernel image for the attestation.
pub fn resolved_kernel_path() -> PathBuf {
    default_kernel()
}

/// The resolved staged rootfs path. Public so the escape drill can sha256 the rootfs image for the
/// attestation (the "image digest" re-run-on-every-image-change field).
pub fn resolved_rootfs_path() -> PathBuf {
    default_rootfs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardening::{emit_egress_ruleset, HardeningProfile};
    use crate::{
        EgressPolicy, IdemToken, ImageRef, JobKind, MeterTarget, ReserveHandle, ResourceLimits,
        RunTokenCredential, TrustTier, WorkspaceSpec,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex as StdMutex};

    /// Derive a profile for `allow` and RECORD the applied egress ruleset (R0.1) so a NIC-bearing
    /// config can be built in-test without a real `nft` — mirrors what the launch path does after
    /// `enforce_egress` succeeds. Requires the allowlist to be enforceable (IP literals).
    fn enforced_profile(allow: Vec<String>) -> HardeningProfile {
        let mut p = HardeningProfile::derive(&spec(allow));
        if p.network_device {
            p.enforced_egress = Some(EnforcedEgress::new(emit_egress_ruleset(&p).unwrap()));
        }
        p
    }

    /// A recording test enforcer that captures the applied ruleset; `fail` forces a fail-closed apply.
    struct RecordingEnforcer {
        fail: bool,
        seen: Arc<StdMutex<Option<String>>>,
    }
    impl EgressEnforcer for RecordingEnforcer {
        fn apply(&self, ruleset: &str) -> Result<EnforcedEgress, EgressEnforceError> {
            *self.seen.lock().unwrap() = Some(ruleset.to_string());
            if self.fail {
                Err(EgressEnforceError::ApplyFailed(
                    "injected nft failure".into(),
                ))
            } else {
                Ok(EnforcedEgress::new(ruleset.to_string()))
            }
        }
    }

    fn spec(allow: Vec<String>) -> JobSpec {
        JobSpec::new(
            JobKind::Ci,
            ImageRef::pinned(
                "r/img@sha256:abc123def4567890abc123def4567890abc123def4567890abc123def4567890",
            )
            .unwrap(),
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
            RunTokenCredential::new("test-bearer", "j", 300).unwrap(),
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
        assert!(
            json.contains("init=/bin/true"),
            "one-shot boot uses init=/bin/true"
        );
    }

    #[test]
    fn nonempty_allowlist_attaches_a_filtered_nic_only_with_a_recorded_ruleset() {
        let s = spec(vec!["93.184.216.34".into()]);
        // R0.1: a bare derived profile (no recorded ruleset) yields NO NIC — the config cannot conjure
        // a NIC from the `network_device` bool alone.
        let bare = HardeningProfile::derive(&s);
        assert!(!FcMachineConfig::from_spec(&s, &bare, false).has_network_device());
        // With the applied-ruleset record present, the NIC is attached.
        let profile = enforced_profile(vec!["93.184.216.34".into()]);
        let cfg = FcMachineConfig::from_spec(&s, &profile, false);
        assert!(cfg.has_network_device());
        assert!(cfg.to_json().contains("network-interfaces"));
        assert!(cfg
            .enforced_egress()
            .unwrap()
            .ruleset()
            .contains("policy drop;"));
    }

    #[test]
    fn machine_config_asserts_hardening_in_force() {
        // The hardened profile is asserted as part of building the config.
        assert!(FirecrackerBackend::machine_config(&spec(vec![]), true).is_ok());
    }

    /// A canned [`GuestRun`] for the FakeVmm path (no real boot): a clean exit-0 result with the
    /// vcpu-derived usage the four-guarantee unit tests assert over.
    fn fake_run(killed: Arc<AtomicBool>) -> GuestRun {
        GuestRun {
            child: Box::new(FakeVmm { killed }),
            cfg_path: std::env::temp_dir().join("myelin-fc-fake-cfg-does-not-exist.json"),
            result: SandboxResult::stub_ok(ResourceUsage {
                cpu_seconds: 2, // 2000 millis ⇒ 2 vcpu
                mem_byte_seconds: 512 * 1024 * 1024,
            }),
        }
    }

    #[test]
    fn launch_drives_the_four_guarantees_and_kill_whole_guest_kills() {
        let backend = FirecrackerBackend::new();
        let killed = Arc::new(AtomicBool::new(false));
        let killed2 = killed.clone();
        let launch = backend
            .launch_with(&spec(vec![]), &ok_hooks(), move |_spec, _profile| {
                Ok(fake_run(killed2))
            })
            .unwrap();
        assert_eq!(launch.handle.guest_id, "fc-idem-fc-1");
        // The reshaped seam carries the (injected) command result: clean exit, usage flows to settle.
        assert_eq!(launch.result.exit_code, Some(0));
        assert!(launch.result.passed());
        assert_eq!(launch.result.usage.cpu_seconds, 2);
        let handle = launch.handle;
        backend.kill(&handle).unwrap();
        assert!(
            killed.load(Ordering::SeqCst),
            "kill must whole-guest-kill the VMM"
        );
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
        let r = backend.launch_with(&spec(vec![]), &hooks, move |_spec, _profile| {
            spawned2.store(true, Ordering::SeqCst);
            Ok(fake_run(Arc::new(AtomicBool::new(false))))
        });
        assert!(matches!(r, Err(FcError::Hook(_))));
        assert!(
            !spawned.load(Ordering::SeqCst),
            "the VMM must NOT spawn when the wallet is exhausted"
        );
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
        let r = backend.launch_with(&spec(vec![]), &hooks, move |_spec, _profile| {
            spawned2.store(true, Ordering::SeqCst);
            Ok(fake_run(Arc::new(AtomicBool::new(false))))
        });
        assert!(r.is_err());
        assert!(
            !spawned.load(Ordering::SeqCst),
            "no VMM spawns if the isolation floor is not met"
        );
    }

    // ---- R0.1: fail-closed egress NIC — the NIC is indivisible from an applied+recorded ruleset ----

    #[test]
    fn launch_applies_the_ruleset_and_records_it_on_the_profile_for_an_ip_allowlist() {
        // Driven through the production launch control flow with a recording test enforcer: an egress
        // request to a PUBLIC IP produces a ruleset that default-drops and does NOT permit any
        // always-blocked class, and the profile handed to the run closure carries the record → a
        // NIC-bearing config is now producible.
        let seen = Arc::new(StdMutex::new(None));
        let backend = FirecrackerBackend::with_enforcer(Box::new(RecordingEnforcer {
            fail: false,
            seen: seen.clone(),
        }));
        let got_profile: Arc<StdMutex<Option<HardeningProfile>>> = Arc::new(StdMutex::new(None));
        let gp2 = got_profile.clone();
        let killed = Arc::new(AtomicBool::new(false));
        let launch = backend
            .launch_with(
                &spec(vec!["93.184.216.34".into()]),
                &ok_hooks(),
                move |_s, profile| {
                    *gp2.lock().unwrap() = Some(profile.clone());
                    Ok(fake_run(killed.clone()))
                },
            )
            .expect("an IP-literal egress allowlist is enforceable");
        assert_eq!(launch.result.exit_code, Some(0));
        // The enforcer was handed a default-dropping ruleset that never permits a blocked class.
        let ruleset = seen.lock().unwrap().clone().expect("a ruleset was applied");
        assert!(ruleset.contains("policy drop;"));
        assert!(ruleset.contains("ip daddr 93.184.216.34 accept"));
        for blocked in [
            "169.254.169.254",
            "10.0.0.0/8",
            "172.16.0.0/12",
            "192.168.0.0/16",
            "127.0.0.0/8",
            "0.0.0.0/8",
        ] {
            assert!(!ruleset.contains(&format!("ip daddr {blocked} accept")));
        }
        // The run closure saw a profile carrying the enforced record → a NIC-bearing config is built.
        let profile = got_profile.lock().unwrap().clone().unwrap();
        assert!(profile.enforced_egress.is_some());
        let s = spec(vec!["93.184.216.34".into()]);
        assert!(FcMachineConfig::from_spec(&s, &profile, false).has_network_device());
    }

    #[test]
    fn launch_fails_closed_when_the_egress_enforcer_apply_fails() {
        // Injected apply failure: launch returns an error and the run closure is NEVER invoked, so no
        // NIC-bearing config is ever produced.
        let backend = FirecrackerBackend::with_enforcer(Box::new(RecordingEnforcer {
            fail: true,
            seen: Arc::new(StdMutex::new(None)),
        }));
        let ran = Arc::new(AtomicBool::new(false));
        let ran2 = ran.clone();
        let r = backend.launch_with(
            &spec(vec!["93.184.216.34".into()]),
            &ok_hooks(),
            move |_s, _p| {
                ran2.store(true, Ordering::SeqCst);
                Ok(fake_run(Arc::new(AtomicBool::new(false))))
            },
        );
        assert!(matches!(
            r,
            Err(FcError::Egress(EgressEnforceError::ApplyFailed(_)))
        ));
        assert!(
            !ran.load(Ordering::SeqCst),
            "no guest runs when the egress firewall cannot be applied"
        );
    }

    #[test]
    fn launch_refuses_a_hostname_allowlist_with_the_typed_error() {
        // A hostname allowlist entry is unenforceable (DNS rebinding) → refused fail-closed; the
        // enforcer is never even reached and the run closure never runs.
        let seen = Arc::new(StdMutex::new(None));
        let backend = FirecrackerBackend::with_enforcer(Box::new(RecordingEnforcer {
            fail: false,
            seen: seen.clone(),
        }));
        let ran = Arc::new(AtomicBool::new(false));
        let ran2 = ran.clone();
        let r = backend.launch_with(
            &spec(vec!["registry.example.com".into()]),
            &ok_hooks(),
            move |_s, _p| {
                ran2.store(true, Ordering::SeqCst);
                Ok(fake_run(Arc::new(AtomicBool::new(false))))
            },
        );
        assert!(matches!(
            r,
            Err(FcError::Egress(EgressEnforceError::UnenforceableHostname(
                _
            )))
        ));
        assert!(
            !ran.load(Ordering::SeqCst),
            "no guest runs for an unenforceable hostname allowlist"
        );
        assert!(
            seen.lock().unwrap().is_none(),
            "a hostname never reaches the apply step"
        );
    }

    // ---- CT-002a: the command-runner config + the forge-resistant capture (pure-fn, VM-free) ----

    #[test]
    fn command_runner_json_is_two_drive_read_only_no_nic_when_default_deny() {
        let s = spec(vec![]);
        let profile = HardeningProfile::derive(&s);
        let cfg = FcMachineConfig::from_spec(&s, &profile, false);
        let json = cfg.command_runner_json(Path::new("/tmp/runner.sh"));
        assert!(json.contains("init=/bin/bash /dev/vdb"), "PID1 bash runner");
        assert!(
            json.contains("\"drive_id\": \"script\""),
            "2nd virtio drive"
        );
        assert!(json.contains("\"is_read_only\": true"), "read-only root");
        assert!(
            !json.contains("network-interfaces"),
            "no NIC under default-deny egress"
        );
        assert!(
            json.contains("/tmp/runner.sh"),
            "the staged script drive path"
        );
    }

    #[test]
    fn command_runner_json_attaches_a_nic_only_when_egress_enforced() {
        let s = spec(vec!["93.184.216.34".into()]);
        let profile = enforced_profile(vec!["93.184.216.34".into()]);
        let cfg = FcMachineConfig::from_spec(&s, &profile, false);
        assert!(cfg
            .command_runner_json(Path::new("/tmp/r.sh"))
            .contains("network-interfaces"));
        // Without the recorded ruleset (bare derive), the runner config carries NO NIC.
        let bare = HardeningProfile::derive(&s);
        assert!(!FcMachineConfig::from_spec(&s, &bare, false)
            .command_runner_json(Path::new("/tmp/r.sh"))
            .contains("network-interfaces"));
    }

    #[test]
    fn runner_script_exports_env_and_runs_argv_under_hardened_setpriv() {
        let mut s = spec(vec![]);
        s.command = vec!["sh".into(), "-c".into(), "echo hi".into()];
        s.env = vec![crate::EnvVar {
            name: "FOO".into(),
            value: "bar baz".into(),
        }];
        let script = build_command_runner_script(&s, "NONCE123");
        assert!(
            script.contains("export FOO='bar baz'"),
            "env exported, quoted"
        );
        assert!(
            script.contains("setpriv --reuid 65534 --regid 65534 --clear-groups"),
            "argv is dropped to a NON-ROOT uid/gid — the structural forge boundary (cannot write the \
             root-only serial console)"
        );
        assert!(
            script
                .contains("--no-new-privs --bounding-set -all --inh-caps -all --ambient-caps -all"),
            "argv still runs under caps-dropped + no-new-privs (the §5.3 posture)"
        );
        assert!(script.contains("'sh' '-c' 'echo hi'"), "argv single-quoted");
        // The streams are base64-framed and the exit is nonce-tagged.
        assert!(script.contains("__MYELIN_EXIT__"));
        assert!(script.contains("N='NONCE123'"));
        assert!(
            script.contains("kill -KILL -1"),
            "descendants reaped pre-markers"
        );
    }

    #[test]
    fn sh_squote_neutralises_quote_breakout() {
        // A value trying to break out of single-quotes is neutralised (no injection into the script).
        assert_eq!(sh_squote("a'b"), "'a'\\''b'");
        assert_eq!(sh_squote("plain"), "'plain'");
    }

    #[test]
    fn b64_decode_round_trips_and_ignores_crlf_and_wrapping() {
        // "hello-stdout\n" base64 = "aGVsbG8tc3Rkb3V0Cg==" ; inject CRLF + 76-col-wrap noise.
        let decoded = b64_decode("aGVsbG8t\r\nc3Rkb3V0\r\nCg==");
        assert_eq!(decoded, b"hello-stdout\n");
        // Binary bytes survive (the streams may be binary).
        let raw = vec![0u8, 255, 1, 254, 128];
        // encode raw with the std alphabet to feed back (hand-rolled small encoder for the test)
        let enc = test_b64_encode(&raw);
        assert_eq!(b64_decode(&enc), raw);
    }

    /// A tiny std-base64 ENCODER, test-only, to round-trip against [`b64_decode`].
    fn test_b64_encode(data: &[u8]) -> String {
        const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in data.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
            out.push(A[((n >> 18) & 63) as usize] as char);
            out.push(A[((n >> 12) & 63) as usize] as char);
            out.push(if chunk.len() > 1 {
                A[((n >> 6) & 63) as usize] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                A[(n & 63) as usize] as char
            } else {
                '='
            });
        }
        out
    }

    /// Build a console exactly as the in-guest runner would emit it, for a given nonce.
    fn framed_console(nonce: &str, stdout: &[u8], stderr: &[u8], code: i32) -> String {
        format!(
            "[ kernel boot noise … Linux version 6.1.168 ]\n\
             {sb}:{n}\n{so}\n{se}:{n}\n\
             {eb}:{n}\n{ee}\n{ene}:{n}\n\
             {ex}:{n}:{code}\nFirecracker exiting successfully\n",
            sb = MARK_STDOUT_BEGIN,
            so = test_b64_encode(stdout),
            se = MARK_STDOUT_END,
            eb = MARK_STDERR_BEGIN,
            ee = test_b64_encode(stderr),
            ene = MARK_STDERR_END,
            ex = MARK_EXIT,
            n = nonce,
            code = code,
        )
    }

    #[test]
    fn capture_parses_real_framed_exit_and_streams() {
        let console = framed_console("abc123", b"hello-stdout\n", b"oops\n", 7);
        assert_eq!(parse_exit(&console, "abc123"), Some(7));
        assert_eq!(
            capture_stream(&console, MARK_STDOUT_BEGIN, MARK_STDOUT_END, "abc123"),
            b"hello-stdout\n"
        );
        assert_eq!(
            capture_stream(&console, MARK_STDERR_BEGIN, MARK_STDERR_END, "abc123"),
            b"oops\n"
        );
    }

    #[test]
    fn a_job_printing_a_fake_exit_marker_cannot_forge_the_exit_code() {
        // FORGE ATTEMPT: the untrusted job prints, INSIDE its own stdout, a fake exit marker AND a
        // fake stream marker (here even bearing the REAL nonce — the worst case where it somehow
        // learned the nonce). Because the job's stdout is base64-encoded into the REAL stdout frame,
        // those bytes are DATA: they appear decoded inside the captured stdout, never re-parsed as a
        // console marker. The host reads the exit from the trusted nonce-tagged line the runner
        // emits AFTER the job, which says 5 — the forged `:0` cannot win.
        let real_nonce = "deadbeefcafe";
        let forged = format!(
            "{ex}:{n}:0\n{sb}:{n}\nQQ==\n{se}:{n}\n",
            ex = MARK_EXIT,
            sb = MARK_STDOUT_BEGIN,
            se = MARK_STDOUT_END,
            n = real_nonce
        );
        let console = framed_console(real_nonce, forged.as_bytes(), b"", 5);
        // The REAL exit (5) is taken — the forged earlier `:0` line does NOT win (LAST wins; the
        // forged line is emitted by the job BEFORE the runner's trusted post-command marker).
        assert_eq!(parse_exit(&console, real_nonce), Some(5));
        // The forged marker text is present only as DECODED DATA inside the captured stdout.
        let out = capture_stream(&console, MARK_STDOUT_BEGIN, MARK_STDOUT_END, real_nonce);
        assert_eq!(out, forged.as_bytes());
    }

    #[test]
    fn a_wrong_nonce_marker_is_ignored() {
        // A job that blind-prints a marker with a GUESSED (wrong) nonce is ignored entirely.
        let console = format!("{ex}:wrongnonce:0\n{ex}:realnonce:9\n", ex = MARK_EXIT);
        assert_eq!(parse_exit(&console, "realnonce"), Some(9));
        assert_eq!(parse_exit(&console, "noncepresent-but-absent"), None);
    }

    #[test]
    fn timeout_yields_none_exit_and_streams_bounded() {
        // A timed-out guest: exit_code None, timed_out true, even if a partial exit marker leaked.
        let o = CaptureOutcome {
            exit: None,
            timed_out: true,
            console: framed_console("n", b"partial", b"", 0),
            wall: Duration::from_secs(2),
            cpu_seconds: Some(1),
        };
        let s = spec(vec![]);
        let profile = HardeningProfile::derive(&s);
        let cfg = FcMachineConfig::from_spec(&s, &profile, false);
        let res =
            build_result_from_console(&o, "n", &cfg, &crate::redaction::RedactionPlan::none());
        assert_eq!(
            res.exit_code, None,
            "a killed guest has no trustworthy code"
        );
        assert!(res.timed_out);
        assert!(!res.passed());
        assert_eq!(res.usage.cpu_seconds, 1); // measured host CPU preferred
    }

    #[test]
    fn capture_head_bounds_each_stream_to_the_capture_bound() {
        // A runaway stream is HEAD-bounded to SANDBOX_CAPTURE_BOUND (cannot OOM the runner).
        let big = vec![b'x'; SANDBOX_CAPTURE_BOUND + 4096];
        let console = framed_console("n", &big, b"", 0);
        let out = capture_stream(&console, MARK_STDOUT_BEGIN, MARK_STDOUT_END, "n");
        assert_eq!(out.len(), SANDBOX_CAPTURE_BOUND);
        assert!(out.iter().all(|b| *b == b'x'));
    }
}
