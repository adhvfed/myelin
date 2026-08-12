use crate::hardening::{
    enforce_egress, EgressEnforceError, EgressEnforcer, EnforcedEgress, HardeningProfile,
};
use crate::launch_gate::SandboxCommand;
use crate::redaction::RedactionPlan;
use crate::{
    drain_capped, JobSpec, LaunchPermit, ResourceUsage, RunnerHooks, SandboxBackend,
    SandboxCancellation, SandboxHandle, SandboxLaunch, SandboxLaunchError, SandboxOutputSink,
    SandboxOutputStream, SandboxResult, SANDBOX_CAPTURE_BOUND,
};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const ENV_FC_KERNEL: &str = "MYELIN_FC_KERNEL";
pub const ENV_FC_ROOTFS: &str = "MYELIN_FC_ROOTFS";
pub const ENV_FC_BIN: &str = "MYELIN_FC_BIN";

pub const BOOT_ARGS_BASE: &str = "console=ttyS0 reboot=k panic=1 pci=off i8042.noaux i8042.nomux \
     i8042.nopnp i8042.dumbkbd root=/dev/vda ro";

const MARK_STDOUT_BEGIN: &str = "__MYELIN_STDOUT_B64_BEGIN__";
const MARK_STDOUT_END: &str = "__MYELIN_STDOUT_B64_END__";
const MARK_STDERR_BEGIN: &str = "__MYELIN_STDERR_B64_BEGIN__";
const MARK_STDERR_END: &str = "__MYELIN_STDERR_B64_END__";
const MARK_STREAM_CHUNK: &str = "__MYELIN_STREAM_B64__";
const MARK_STREAM_END: &str = "__MYELIN_STREAM_END__";
const MARK_EXIT: &str = "__MYELIN_EXIT__";

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

fn asset_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("firecracker-assets")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FcMachineConfig {
    kernel_image_path: PathBuf,
    boot_args: String,
    rootfs_path: PathBuf,
    root_is_read_only: bool,
    vcpu_count: u32,
    mem_size_mib: u32,
    enforced_egress: Option<EnforcedEgress>,
    pids_max: u32,
}

impl FcMachineConfig {
    pub fn from_spec(spec: &JobSpec, profile: &HardeningProfile, oneshot: bool) -> FcMachineConfig {
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
            root_is_read_only: profile.read_only_root,
            vcpu_count: vcpu,
            mem_size_mib: mem_mib,
            enforced_egress: profile.enforced_egress.clone(),
            pids_max: profile.pids_max,
        }
    }

    pub fn to_json(&self) -> String {
        let net = if self.enforced_egress.is_some() {
            ",\n  \"network-interfaces\": [\n    {\n      \"iface_id\": \"eth0\",\n      \
             \"host_dev_name\": \"tap-myelin\"\n    }\n  ]"
        } else {
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

    pub fn command_runner_json(&self, script_drive_path: &Path) -> String {
        let boot_args = format!("{BOOT_ARGS_BASE} init=/bin/bash /dev/vdb");
        two_drive_config_json(
            &self.kernel_image_path,
            &boot_args,
            &self.rootfs_path,
            self.root_is_read_only,
            script_drive_path,
            self.enforced_egress.is_some(),
            self.vcpu_count,
            self.mem_size_mib,
        )
    }

    pub fn root_is_read_only(&self) -> bool {
        self.root_is_read_only
    }

    pub fn has_network_device(&self) -> bool {
        self.enforced_egress.is_some()
    }

    pub fn enforced_egress(&self) -> Option<&EnforcedEgress> {
        self.enforced_egress.as_ref()
    }

    pub fn pids_max(&self) -> u32 {
        self.pids_max
    }
}

#[derive(Debug)]
pub enum FcError {
    Hook(crate::HookError),
    Hardening(String),
    SecretInjection(String),
    Egress(EgressEnforceError),
    Vmm(String),
}

impl std::fmt::Display for FcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FcError::Hook(e) => write!(f, "firecracker backend: guarantee hook failed: {e}"),
            FcError::Hardening(s) => write!(f, "firecracker backend: hardening not enforced: {s}"),
            FcError::SecretInjection(s) => {
                write!(f, "firecracker backend: secret injection refused: {s}")
            }
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
        Ok(EnforcedEgress::new(ruleset.to_string()))
    }
}

pub struct FirecrackerBackend {
    live: Mutex<std::collections::HashMap<String, GuestProc>>,
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

struct GuestProc {
    child: Box<dyn VmmChild + Send>,
    cfg_path: PathBuf,
}

pub struct GuestRun {
    pub child: Box<dyn VmmChild + Send>,
    pub cfg_path: PathBuf,
    pub result: SandboxResult,
    pub run_error: Option<String>,
}

pub trait VmmChild {
    fn kill(&mut self) -> Result<(), String>;
    fn wait(&mut self) -> Result<i32, String>;
}

impl FirecrackerBackend {
    pub fn new() -> FirecrackerBackend {
        FirecrackerBackend::default()
    }

    pub fn with_enforcer(enforcer: Box<dyn EgressEnforcer + Send + Sync>) -> FirecrackerBackend {
        FirecrackerBackend {
            live: Mutex::default(),
            egress_enforcer: enforcer,
        }
    }

    pub fn machine_config(spec: &JobSpec, oneshot: bool) -> Result<FcMachineConfig, FcError> {
        let profile = HardeningProfile::derive(spec);
        profile.assert_enforced().map_err(FcError::Hardening)?;
        Ok(FcMachineConfig::from_spec(spec, &profile, oneshot))
    }

    pub fn launch_with<F>(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
        run: F,
    ) -> Result<SandboxLaunch, FcError>
    where
        F: FnOnce(&JobSpec, &HardeningProfile, LaunchPermit) -> Result<GuestRun, String>,
    {
        spec.validate_secret_coverage()
            .map_err(|error| FcError::SecretInjection(error.to_string()))?;
        if spec.resolved_secret_count() != 0 {
            return Err(FcError::SecretInjection(
                "resolved secrets require the in-boundary OCI process-env path".into(),
            ));
        }
        hooks.enforce_isolation_floor(spec)?;
        let mut profile = HardeningProfile::derive(spec);
        profile.enforced_egress = enforce_egress(&profile, self.egress_enforcer.as_ref())?;
        profile.assert_enforced().map_err(FcError::Hardening)?;
        let reserve = hooks.reserve(spec)?;
        let launch_permit = match hooks.acquire_launch_permit(spec) {
            Ok(permit) => permit,
            Err(attribute_error) => {
                hooks.release_unused(spec, &reserve)?;
                return Err(attribute_error.into());
            }
        };

        let GuestRun {
            child,
            cfg_path,
            result,
            run_error,
        } = run(spec, &profile, launch_permit).map_err(FcError::Vmm)?;

        let guest_id = format!("fc-{}", spec.idem_token.0);
        crate::sync::lock_recovering_poison(&self.live)
            .insert(guest_id.clone(), GuestProc { child, cfg_path });

        if let Err(error) = hooks.settle_completed(spec, &reserve, result.usage) {
            let _ = self.kill(&SandboxHandle {
                guest_id: guest_id.clone(),
            });
            return Err(error.into());
        }

        Ok(SandboxLaunch {
            handle: SandboxHandle { guest_id },
            result,
            output_complete: run_error.is_none(),
        })
    }
}

fn run_production_guest(
    spec: &JobSpec,
    profile: &HardeningProfile,
    launch_permit: LaunchPermit,
) -> Result<GuestRun, String> {
    run_production_guest_streaming(
        spec,
        profile,
        launch_permit,
        None,
        SandboxCancellation::new(),
    )
}

fn run_production_guest_streaming(
    spec: &JobSpec,
    profile: &HardeningProfile,
    launch_permit: LaunchPermit,
    output: Option<Arc<dyn SandboxOutputSink>>,
    cancellation: SandboxCancellation,
) -> Result<GuestRun, String> {
    let nonce = boot_nonce()?;
    let script = build_command_runner_script(spec, &nonce);
    let script_drive = stage_padded_script(&script)?;

    let cfg = FcMachineConfig::from_spec(spec, profile, false);
    let cfg_json = cfg.command_runner_json(&script_drive);
    let cfg_path = match write_config_json(&cfg_json) {
        Ok(p) => p,
        Err(e) => {
            let _ = std::fs::remove_file(&script_drive);
            return Err(e);
        }
    };

    let timeout = Duration::from_secs(spec.limits.timeout_secs as u64);
    let redaction = spec.resolved_secrets().redaction_plan().clone();
    let command_capture = CommandStreamSpec {
        nonce: nonce.clone(),
        output,
        redaction: redaction.clone(),
    };
    let (child, outcome) = match spawn_and_capture(
        &cfg_path,
        Some(timeout),
        Some(launch_permit),
        Some(command_capture),
        Some(cancellation),
    ) {
        Ok(v) => v,
        Err(e) => {
            let _ = std::fs::remove_file(&script_drive);
            let _ = std::fs::remove_file(&cfg_path);
            return Err(e);
        }
    };
    let _ = std::fs::remove_file(&script_drive);

    let result = build_result_from_console(&outcome, &nonce, &cfg, &redaction);
    Ok(GuestRun {
        child: Box::new(child),
        cfg_path,
        result,
        run_error: outcome.stream_error,
    })
}

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn write_config_json(json: &str) -> Result<PathBuf, String> {
    let path = std::env::temp_dir().join(format!(
        "myelin-fc-{}-{}.json",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::write(&path, json).map_err(|e| format!("write config {path:?}: {e}"))?;
    Ok(path)
}

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

fn build_command_runner_script(spec: &JobSpec, nonce: &str) -> String {
    let mut exports = String::new();
    for ev in &spec.env {
        exports.push_str(&format!("export {}={}\n", ev.name, sh_squote(&ev.value)));
    }
    let argv: String = spec
        .command
        .iter()
        .map(|a| sh_squote(a))
        .collect::<Vec<_>>()
        .join(" ");

    format!(
        r#"# Myelin CT-002a command-runner init (PID1 bash, hardened Firecracker microVM, REAL KVM kernel).
export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
mount -t proc proc /proc 2>/dev/null
mount -t sysfs sys /sys 2>/dev/null
mount -t tmpfs tmpfs /run 2>/dev/null
mkdir -p /run/scratch 2>/dev/null
mount -t tmpfs -o size={tmpfs_bytes},mode=1777 tmpfs /run/scratch 2>/dev/null
N='{nonce}'
mkfifo /run/myelin.stdout /run/myelin.stderr
mkdir -p /sys/fs/cgroup
if [ ! -e /sys/fs/cgroup/cgroup.controllers ]; then
  mount -t cgroup2 cgroup2 /sys/fs/cgroup 2>/dev/null
fi
PAYLOAD_CGROUP=/sys/fs/cgroup/myelin-payload
mkdir "$PAYLOAD_CGROUP" 2>/dev/null
[ -w "$PAYLOAD_CGROUP/cgroup.procs" ] && [ -w "$PAYLOAD_CGROUP/cgroup.kill" ] || {{
  printf '%s\n' "myelin: payload cgroup unavailable" >/dev/console
  reboot -f
}}
relay_stream() {{
  TAG="$1"
  FIFO="$2"
  exec 7<"$FIFO"
  while true; do
    FRAME="$(dd bs=3072 count=1 <&7 2>/dev/null | base64 -w0)"
    [ -n "$FRAME" ] || break
    exec 9>/run/myelin.console.lock
    flock -x 9
    printf '%s:%s:%s:%s\n' "{stream_chunk}" "$N" "$TAG" "$FRAME"
    flock -u 9
    exec 9>&-
  done
  exec 9>/run/myelin.console.lock
  flock -x 9
  printf '%s:%s:%s\n' "{stream_end}" "$N" "$TAG"
  flock -u 9
  exec 9>&-
}}
relay_stream o /run/myelin.stdout &
OUT_RELAY=$!
relay_stream e /run/myelin.stderr &
ERR_RELAY=$!

mkfifo /run/myelin.start
{exports}( read -r START </run/myelin.start
ulimit -u {pids_max} 2>/dev/null
exec </dev/null >/run/myelin.stdout 2>/run/myelin.stderr
exec setpriv --reuid 65534 --regid 65534 --clear-groups \
        --no-new-privs --bounding-set -all --inh-caps -all --ambient-caps -all {argv} ) &
COMMAND_PID=$!
echo "$COMMAND_PID" >"$PAYLOAD_CGROUP/cgroup.procs"
printf '%s\n' go >/run/myelin.start
wait "$COMMAND_PID"
CODE=$?

echo 1 >"$PAYLOAD_CGROUP/cgroup.kill"
wait "$OUT_RELAY"
wait "$ERR_RELAY"
rmdir "$PAYLOAD_CGROUP" 2>/dev/null
printf '%s:%s:%s\n' "{ex}" "$N" "$CODE"
sync
reboot -f
"#,
        nonce = nonce,
        exports = exports,
        argv = argv,
        tmpfs_bytes = spec.limits.tmpfs_bytes,
        pids_max = spec.limits.pids_max,
        stream_chunk = MARK_STREAM_CHUNK,
        stream_end = MARK_STREAM_END,
        ex = MARK_EXIT,
    )
}

fn stage_padded_script(script: &str) -> Result<PathBuf, String> {
    let mut bytes = script.as_bytes().to_vec();
    bytes.push(b'\n');
    bytes.push(b'#');
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

    fn launch(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
    ) -> Result<SandboxLaunch, SandboxLaunchError<Self::Error>> {
        self.launch_with(spec, hooks, run_production_guest)
            .map_err(SandboxLaunchError::Failed)
    }

    fn launch_streaming(
        &self,
        spec: &JobSpec,
        hooks: &RunnerHooks,
        output: Arc<dyn SandboxOutputSink>,
        cancellation: SandboxCancellation,
    ) -> Result<SandboxLaunch, SandboxLaunchError<Self::Error>> {
        self.launch_with(spec, hooks, move |spec, profile, permit| {
            run_production_guest_streaming(spec, profile, permit, Some(output), cancellation)
        })
        .map_err(SandboxLaunchError::Failed)
    }

    fn kill(&self, h: &SandboxHandle) -> Result<(), Self::Error> {
        let proc = crate::sync::lock_recovering_poison(&self.live).remove(&h.guest_id);
        if let Some(mut proc) = proc {
            let r = proc.child.kill();
            let _ = std::fs::remove_file(&proc.cfg_path);
            r.map_err(FcError::Vmm)?;
        }
        Ok(())
    }
}

struct SpawnedVmm {
    child: Option<Child>,
}

impl VmmChild for SpawnedVmm {
    fn kill(&mut self) -> Result<(), String> {
        if let Some(mut child) = self.child.take() {
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

struct CaptureOutcome {
    exit: Option<i32>,
    timed_out: bool,
    console: String,
    wall: Duration,
    cpu_seconds: Option<u64>,
    command: Option<CommandCapture>,
    stream_error: Option<String>,
}

#[derive(Clone)]
struct CommandStreamSpec {
    nonce: String,
    output: Option<Arc<dyn SandboxOutputSink>>,
    redaction: RedactionPlan,
}

#[derive(Default)]
struct CommandCapture {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    exit_code: Option<i32>,
    stdout_ended: bool,
    stderr_ended: bool,
}

fn read_proc_cpu_seconds(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &stat[stat.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some((utime + stime) / 100)
}

fn drain_command_console<R: Read>(
    mut reader: R,
    console_limit: usize,
    spec: &CommandStreamSpec,
) -> (Vec<u8>, Option<CommandCapture>, Option<String>) {
    const MAX_PROTOCOL_LINE: usize = 8 * 1024;
    const MAX_DECODED_FRAME: usize = 3072;

    let mut console_head = Vec::new();
    let mut capture = CommandCapture::default();
    let mut line = Vec::new();
    let mut dropping_oversized_line = false;
    let mut first_error = None;
    let mut chunk = [0u8; 64 * 1024];

    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if console_head.len() < console_limit {
                    let take = (console_limit - console_head.len()).min(n);
                    console_head.extend_from_slice(&chunk[..take]);
                }
                for byte in &chunk[..n] {
                    if *byte == b'\n' {
                        if !dropping_oversized_line {
                            process_command_console_line(
                                &line,
                                spec,
                                &mut capture,
                                &mut first_error,
                                MAX_DECODED_FRAME,
                            );
                        }
                        line.clear();
                        dropping_oversized_line = false;
                    } else if !dropping_oversized_line {
                        if line.len() >= MAX_PROTOCOL_LINE {
                            first_error.get_or_insert_with(|| {
                                "firecracker command protocol line exceeds bound".to_string()
                            });
                            line.clear();
                            dropping_oversized_line = true;
                        } else {
                            line.push(*byte);
                        }
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                first_error.get_or_insert_with(|| format!("read firecracker console: {error}"));
                break;
            }
        }
    }

    (console_head, Some(capture), first_error)
}

fn process_command_console_line(
    raw_line: &[u8],
    spec: &CommandStreamSpec,
    capture: &mut CommandCapture,
    first_error: &mut Option<String>,
    max_decoded_frame: usize,
) {
    let line = String::from_utf8_lossy(raw_line);
    let line = line.trim_end_matches('\r');
    let chunk_prefix = format!("{MARK_STREAM_CHUNK}:{}:", spec.nonce);
    if let Some(rest) = line.strip_prefix(&chunk_prefix) {
        let Some((tag, encoded)) = rest.split_once(':') else {
            first_error.get_or_insert_with(|| "malformed firecracker stream frame".to_string());
            return;
        };
        let stream = match tag {
            "o" => SandboxOutputStream::Stdout,
            "e" => SandboxOutputStream::Stderr,
            _ => {
                first_error
                    .get_or_insert_with(|| "unknown firecracker stream frame tag".to_string());
                return;
            }
        };
        let decoded = match b64_decode_strict(encoded) {
            Ok(decoded) if decoded.len() <= max_decoded_frame => decoded,
            Ok(_) => {
                first_error
                    .get_or_insert_with(|| "firecracker stream frame exceeds bound".to_string());
                return;
            }
            Err(error) => {
                first_error.get_or_insert(error);
                return;
            }
        };
        let redacted = spec.redaction.redact(&decoded);
        let head = match stream {
            SandboxOutputStream::Stdout => &mut capture.stdout,
            SandboxOutputStream::Stderr => &mut capture.stderr,
        };
        if head.len() < SANDBOX_CAPTURE_BOUND {
            let take = (SANDBOX_CAPTURE_BOUND - head.len()).min(redacted.len());
            head.extend_from_slice(&redacted[..take]);
        }
        if let Some(output) = &spec.output {
            if first_error.is_none() {
                if let Err(error) = output.emit(stream, &redacted) {
                    *first_error = Some(error);
                }
            }
        }
        return;
    }

    let end_prefix = format!("{MARK_STREAM_END}:{}:", spec.nonce);
    if let Some(tag) = line.strip_prefix(&end_prefix) {
        match tag {
            "o" => capture.stdout_ended = true,
            "e" => capture.stderr_ended = true,
            _ => {
                first_error.get_or_insert_with(|| "unknown firecracker stream end tag".to_string());
            }
        }
        return;
    }

    let exit_prefix = format!("{MARK_EXIT}:{}:", spec.nonce);
    if let Some(code) = line.strip_prefix(&exit_prefix) {
        match code.parse::<i32>() {
            Ok(code) => capture.exit_code = Some(code),
            Err(_) => {
                first_error.get_or_insert_with(|| "invalid firecracker exit frame".to_string());
            }
        }
    }
}

fn b64_decode_strict(input: &str) -> Result<Vec<u8>, String> {
    fn val(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }

    let bytes = input.as_bytes();
    if bytes.is_empty() || !bytes.len().is_multiple_of(4) {
        return Err("non-canonical firecracker stream base64".to_string());
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for (group_index, group) in bytes.chunks_exact(4).enumerate() {
        let last = group_index + 1 == bytes.len() / 4;
        let pad = match (group[2], group[3]) {
            (b'=', b'=') => 2,
            (_, b'=') => 1,
            (_, _) => 0,
        };
        if (!last && pad != 0) || group[0] == b'=' || group[1] == b'=' {
            return Err("invalid firecracker stream base64 padding".to_string());
        }
        let a = val(group[0])
            .ok_or_else(|| "invalid firecracker stream base64 alphabet".to_string())?;
        let b = val(group[1])
            .ok_or_else(|| "invalid firecracker stream base64 alphabet".to_string())?;
        let c = if pad == 2 {
            0
        } else {
            val(group[2]).ok_or_else(|| "invalid firecracker stream base64 alphabet".to_string())?
        };
        let d = if pad >= 1 {
            0
        } else {
            val(group[3]).ok_or_else(|| "invalid firecracker stream base64 alphabet".to_string())?
        };
        let word = (u32::from(a) << 18) | (u32::from(b) << 12) | (u32::from(c) << 6) | u32::from(d);
        out.push((word >> 16) as u8);
        if pad < 2 {
            out.push((word >> 8) as u8);
        }
        if pad == 0 {
            out.push(word as u8);
        }
    }
    Ok(out)
}

fn spawn_and_capture(
    cfg_path: &Path,
    timeout: Option<Duration>,
    launch_permit: Option<LaunchPermit>,
    command_stream: Option<CommandStreamSpec>,
    cancellation: Option<SandboxCancellation>,
) -> Result<(SpawnedVmm, CaptureOutcome), String> {
    let watchdog_timeout = if launch_permit.is_some() {
        timeout
    } else {
        None
    };
    let mut sandbox_command =
        SandboxCommand::new(firecracker_bin(), launch_permit, watchdog_timeout)
            .map_err(|error| format!("prepare firecracker launch gate: {error}"))?;
    let fenced = sandbox_command.is_fenced();
    let cmd = sandbox_command.command_mut();
    cmd.arg("--no-api")
        .arg("--config-file")
        .arg(cfg_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if !fenced {
        cmd.stdin(Stdio::null());
    }
    let mut child = sandbox_command
        .spawn()
        .map_err(|e| format!("spawn firecracker: {e}"))?;

    let pid = child.id();
    let start = Instant::now();

    let (Some(mut out), Some(mut err)) = (child.stdout().take(), child.stderr().take()) else {
        child.kill_and_wait();
        return Err("firecracker console pipe unavailable".to_string());
    };
    let th_out = std::thread::spawn(move || match command_stream {
        Some(spec) => drain_command_console(&mut out, CONSOLE_CAPTURE_BOUND, &spec),
        None => (drain_capped(&mut out, CONSOLE_CAPTURE_BOUND).0, None, None),
    });
    let th_err = std::thread::spawn(move || drain_capped(&mut err, CONSOLE_CAPTURE_BOUND).0);

    let timed_out;
    let cancelled;
    let mut last_cpu: Option<u64> = None;
    let exit = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|e| format!("wait firecracker: {e}"))?
        {
            timed_out = child.watchdog_deadline_expired();
            cancelled = false;
            break status.code();
        }
        if let Some(c) = read_proc_cpu_seconds(pid) {
            last_cpu = Some(c);
        }
        if cancellation
            .as_ref()
            .is_some_and(SandboxCancellation::is_cancelled)
        {
            child.kill_and_wait();
            timed_out = false;
            cancelled = true;
            break None;
        }
        if let Some(t) = timeout {
            if start.elapsed() >= t {
                child.kill_and_wait();
                timed_out = true;
                cancelled = false;
                break None;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let wall = start.elapsed();

    let out_result = th_out.join();
    let err_result = th_err.join();
    let (out_buf, command, output_error) =
        out_result.map_err(|_| "firecracker stdout drain thread panicked".to_string())?;
    let err_buf = err_result.map_err(|_| "firecracker stderr drain thread panicked".to_string())?;
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
            command,
            stream_error: output_error.or_else(|| {
                cancelled.then(|| "sandbox execution cancelled by durable log consumer".into())
            }),
        },
    ))
}

pub fn boot_and_capture(cfg_path: &Path) -> Result<(i32, String), String> {
    let (_child, outcome) = spawn_and_capture(cfg_path, None, None, None, None)?;
    Ok((outcome.exit.unwrap_or(-1), outcome.console))
}

fn build_result_from_console(
    o: &CaptureOutcome,
    nonce: &str,
    cfg: &FcMachineConfig,
    redaction: &RedactionPlan,
) -> SandboxResult {
    let (stdout, stderr, streamed_exit) = match &o.command {
        Some(command) => (
            command.stdout.clone(),
            command.stderr.clone(),
            (command.stdout_ended && command.stderr_ended)
                .then_some(command.exit_code)
                .flatten(),
        ),
        None => (
            capture_stream_redacted(
                &o.console,
                MARK_STDOUT_BEGIN,
                MARK_STDOUT_END,
                nonce,
                redaction,
            ),
            capture_stream_redacted(
                &o.console,
                MARK_STDERR_BEGIN,
                MARK_STDERR_END,
                nonce,
                redaction,
            ),
            parse_exit(&o.console, nonce),
        ),
    };
    let exit_code = if o.timed_out { None } else { streamed_exit };

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

#[cfg(test)]
fn capture_stream(console: &str, begin: &str, end: &str, nonce: &str) -> Vec<u8> {
    let mut data = decode_captured_stream(console, begin, end, nonce);
    if data.len() > SANDBOX_CAPTURE_BOUND {
        data.truncate(SANDBOX_CAPTURE_BOUND);
    }
    data
}

fn capture_stream_redacted(
    console: &str,
    begin: &str,
    end: &str,
    nonce: &str,
    redaction: &RedactionPlan,
) -> Vec<u8> {
    let mut data = redaction.redact(&decode_captured_stream(console, begin, end, nonce));
    if data.len() > SANDBOX_CAPTURE_BOUND {
        data.truncate(SANDBOX_CAPTURE_BOUND);
    }
    data
}

fn decode_captured_stream(console: &str, begin: &str, end: &str, nonce: &str) -> Vec<u8> {
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
    b64_decode(&b64)
}

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

pub fn drill_config_json(script_drive_path: &std::path::Path, vcpu: u32, mem_mib: u32) -> String {
    let boot_args = format!("{BOOT_ARGS_BASE} init=/bin/bash /dev/vdb");
    two_drive_config_json(
        &default_kernel(),
        &boot_args,
        &default_rootfs(),
        true,
        script_drive_path,
        false,
        vcpu,
        mem_mib,
    )
}

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

pub fn resolved_kernel_path() -> PathBuf {
    default_kernel()
}

pub fn resolved_rootfs_path() -> PathBuf {
    default_rootfs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardening::{emit_egress_ruleset, HardeningProfile};
    use crate::{
        CompletionSettlementOwner, EgressPolicy, IdemToken, ImageRef, JobKind, MeterTarget,
        ReserveHandle, ResourceLimits, RunTokenCredential, TrustTier, WorkspaceSpec,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex as StdMutex};

    #[derive(Default)]
    struct RecordingOutput {
        frames: StdMutex<Vec<(SandboxOutputStream, Vec<u8>)>>,
    }

    impl SandboxOutputSink for RecordingOutput {
        fn emit(&self, stream: SandboxOutputStream, frame: &[u8]) -> Result<(), String> {
            self.frames.lock().unwrap().push((stream, frame.to_vec()));
            Ok(())
        }
    }

    fn enforced_profile(allow: Vec<String>) -> HardeningProfile {
        let mut p = HardeningProfile::derive(&spec(allow));
        if p.network_device {
            p.enforced_egress = Some(EnforcedEgress::new(emit_egress_ruleset(&p).unwrap()));
        }
        p
    }

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
                tmpfs_bytes: 1 << 30,
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
        RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        )
    }

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
        assert_eq!(cfg.vcpu_count, 2);
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
        let bare = HardeningProfile::derive(&s);
        assert!(!FcMachineConfig::from_spec(&s, &bare, false).has_network_device());
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
        assert!(FirecrackerBackend::machine_config(&spec(vec![]), true).is_ok());
    }

    fn fake_run(killed: Arc<AtomicBool>) -> GuestRun {
        GuestRun {
            child: Box::new(FakeVmm { killed }),
            cfg_path: std::env::temp_dir().join("myelin-fc-fake-cfg-does-not-exist.json"),
            result: SandboxResult::stub_ok(ResourceUsage {
                cpu_seconds: 2,
                mem_byte_seconds: 512 * 1024 * 1024,
            }),
            run_error: None,
        }
    }

    #[test]
    fn launch_drives_the_four_guarantees_and_kill_whole_guest_kills() {
        let backend = FirecrackerBackend::new();
        let killed = Arc::new(AtomicBool::new(false));
        let killed2 = killed.clone();
        let launch = backend
            .launch_with(
                &spec(vec![]),
                &ok_hooks(),
                move |_spec, _profile, permit| {
                    permit
                        .commit_and_release()
                        .map_err(|error| error.to_string())?;
                    Ok(fake_run(killed2))
                },
            )
            .unwrap();
        assert_eq!(launch.handle.guest_id, "fc-idem-fc-1");
        assert_eq!(launch.result.exit_code, Some(0));
        assert!(launch.result.passed());
        assert_eq!(launch.result.usage.cpu_seconds, 2);
        let handle = launch.handle;
        backend.kill(&handle).unwrap();
        assert!(
            killed.load(Ordering::SeqCst),
            "kill must whole-guest-kill the VMM"
        );
        backend.kill(&handle).unwrap();
    }

    #[test]
    fn launch_refuses_to_start_on_an_exhausted_wallet() {
        let backend = FirecrackerBackend::new();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(|_spec| Err(crate::HookError("wallet exhausted".into()))),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        );
        let spawned = Arc::new(AtomicBool::new(false));
        let spawned2 = spawned.clone();
        let r = backend.launch_with(&spec(vec![]), &hooks, move |_spec, _profile, _permit| {
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
    fn successful_reporter_owned_launch_defers_settlement_to_terminal_reporter() {
        let backend = FirecrackerBackend::new();
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
            .launch_with(&spec(vec![]), &hooks, |_spec, _profile, permit| {
                permit
                    .commit_and_release()
                    .map_err(|error| error.to_string())?;
                Ok(fake_run(Arc::new(AtomicBool::new(false))))
            })
            .expect("the sandbox returns measured usage for the reporter transaction");
        assert!(
            !hook_settled.load(Ordering::SeqCst),
            "reporter-owned completion must not settle through the hook"
        );
    }

    #[test]
    fn settlement_failure_unconditionally_kills_and_forgets_the_guest() {
        let backend = FirecrackerBackend::new();
        let killed = Arc::new(AtomicBool::new(false));
        let killed_at_run = killed.clone();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(|_spec, _handle, _usage| {
                Err(crate::HookError("injected settlement failure".into()))
            }),
            Box::new(|_spec| Ok(())),
            Box::new(|_spec| Ok(())),
        );

        let result = backend.launch_with(&spec(vec![]), &hooks, move |_spec, _profile, permit| {
            permit
                .commit_and_release()
                .map_err(|error| error.to_string())?;
            Ok(fake_run(killed_at_run))
        });

        assert!(matches!(result, Err(FcError::Hook(_))));
        assert!(
            killed.load(Ordering::SeqCst),
            "a settlement error cannot leave the VMM alive"
        );
        assert!(
            backend.live.lock().unwrap().is_empty(),
            "an error without a returned handle cannot retain an unreachable live-map entry"
        );
    }

    #[test]
    fn launch_releases_the_unused_reserve_when_final_attribution_refuses() {
        let backend = FirecrackerBackend::new();
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
        let result = backend.launch_with(&spec(vec![]), &hooks, move |_spec, _profile, _permit| {
            spawned_at.store(true, Ordering::SeqCst);
            Ok(fake_run(Arc::new(AtomicBool::new(false))))
        });
        assert!(matches!(result, Err(FcError::Hook(_))));
        assert!(!spawned.load(Ordering::SeqCst));
        assert_eq!(
            *settled.lock().unwrap(),
            Some(ResourceUsage {
                cpu_seconds: 0,
                mem_byte_seconds: 0,
            })
        );
    }

    #[test]
    fn launch_fails_closed_when_the_isolation_floor_hook_rejects() {
        let backend = FirecrackerBackend::new();
        let hooks = RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Err(crate::HookError("hardening not met".into()))),
        );
        let spawned = Arc::new(AtomicBool::new(false));
        let spawned2 = spawned.clone();
        let r = backend.launch_with(&spec(vec![]), &hooks, move |_spec, _profile, _permit| {
            spawned2.store(true, Ordering::SeqCst);
            Ok(fake_run(Arc::new(AtomicBool::new(false))))
        });
        assert!(r.is_err());
        assert!(
            !spawned.load(Ordering::SeqCst),
            "no VMM spawns if the isolation floor is not met"
        );
    }

    #[test]
    fn launch_applies_the_ruleset_and_records_it_on_the_profile_for_an_ip_allowlist() {
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
                move |_s, profile, permit| {
                    permit
                        .commit_and_release()
                        .map_err(|error| error.to_string())?;
                    *gp2.lock().unwrap() = Some(profile.clone());
                    Ok(fake_run(killed.clone()))
                },
            )
            .expect("an IP-literal egress allowlist is enforceable");
        assert_eq!(launch.result.exit_code, Some(0));
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
        let profile = got_profile.lock().unwrap().clone().unwrap();
        assert!(profile.enforced_egress.is_some());
        let s = spec(vec!["93.184.216.34".into()]);
        assert!(FcMachineConfig::from_spec(&s, &profile, false).has_network_device());
    }

    #[test]
    fn launch_fails_closed_when_the_egress_enforcer_apply_fails() {
        let backend = FirecrackerBackend::with_enforcer(Box::new(RecordingEnforcer {
            fail: true,
            seen: Arc::new(StdMutex::new(None)),
        }));
        let ran = Arc::new(AtomicBool::new(false));
        let ran2 = ran.clone();
        let r = backend.launch_with(
            &spec(vec!["93.184.216.34".into()]),
            &ok_hooks(),
            move |_s, _p, _permit| {
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
            move |_s, _p, _permit| {
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
            "argv is dropped to a NON-ROOT uid/gid - the structural forge boundary (cannot write the \
             root-only serial console)"
        );
        assert!(
            script
                .contains("--no-new-privs --bounding-set -all --inh-caps -all --ambient-caps -all"),
            "argv still runs under caps-dropped + no-new-privs (the §5.3 posture)"
        );
        assert!(script.contains("'sh' '-c' 'echo hi'"), "argv single-quoted");
        assert!(script.contains("__MYELIN_EXIT__"));
        assert!(script.contains("__MYELIN_STREAM_B64__"));
        assert!(script.contains("dd bs=3072 count=1"));
        assert!(script.contains("N='NONCE123'"));
        assert!(
            script.contains("echo 1 >\"$PAYLOAD_CGROUP/cgroup.kill\""),
            "the payload cgroup atomically reaps all descendants without killing trusted relays"
        );
    }

    #[test]
    fn sh_squote_neutralises_quote_breakout() {
        assert_eq!(sh_squote("a'b"), "'a'\\''b'");
        assert_eq!(sh_squote("plain"), "'plain'");
    }

    #[test]
    fn b64_decode_round_trips_and_ignores_crlf_and_wrapping() {
        let decoded = b64_decode("aGVsbG8t\r\nc3Rkb3V0\r\nCg==");
        assert_eq!(decoded, b"hello-stdout\n");
        let raw = vec![0u8, 255, 1, 254, 128];
        let enc = test_b64_encode(&raw);
        assert_eq!(b64_decode(&enc), raw);
    }

    #[test]
    fn online_command_console_parser_delivers_bounded_frames_and_tracks_completion() {
        let nonce = "nonce-live";
        let console = format!(
            "kernel noise\r\n\
             {chunk}:{nonce}:o:{}\r\n\
             {chunk}:wrong:o:{}\r\n\
             {chunk}:{nonce}:e:{}\r\n\
             {end}:{nonce}:o\r\n\
             {end}:{nonce}:e\r\n\
             {exit}:{nonce}:7\r\n",
            test_b64_encode(b"hello\n"),
            test_b64_encode(b"ignored"),
            test_b64_encode(&[0xff, 0x00, b'\n']),
            chunk = MARK_STREAM_CHUNK,
            end = MARK_STREAM_END,
            exit = MARK_EXIT,
        );
        let output = Arc::new(RecordingOutput::default());
        let spec = CommandStreamSpec {
            nonce: nonce.into(),
            output: Some(output.clone()),
            redaction: RedactionPlan::none(),
        };

        let (head, parsed, error) =
            drain_command_console(std::io::Cursor::new(console.as_bytes()), 12, &spec);
        assert_eq!(error, None);
        assert_eq!(
            head.len(),
            12,
            "raw diagnostic console remains head-bounded"
        );
        let parsed = parsed.expect("command capture");
        assert_eq!(parsed.stdout, b"hello\n");
        assert_eq!(parsed.stderr, [0xff, 0x00, b'\n']);
        assert_eq!(parsed.exit_code, Some(7));
        assert!(parsed.stdout_ended && parsed.stderr_ended);
        assert_eq!(
            output.frames.lock().unwrap().as_slice(),
            &[
                (SandboxOutputStream::Stdout, b"hello\n".to_vec()),
                (SandboxOutputStream::Stderr, vec![0xff, 0x00, b'\n']),
            ],
            "wrong-nonce frames never reach the callback"
        );
    }

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
        let real_nonce = "deadbeefcafe";
        let forged = format!(
            "{ex}:{n}:0\n{sb}:{n}\nQQ==\n{se}:{n}\n",
            ex = MARK_EXIT,
            sb = MARK_STDOUT_BEGIN,
            se = MARK_STDOUT_END,
            n = real_nonce
        );
        let console = framed_console(real_nonce, forged.as_bytes(), b"", 5);
        assert_eq!(parse_exit(&console, real_nonce), Some(5));
        let out = capture_stream(&console, MARK_STDOUT_BEGIN, MARK_STDOUT_END, real_nonce);
        assert_eq!(out, forged.as_bytes());
    }

    #[test]
    fn a_wrong_nonce_marker_is_ignored() {
        let console = format!("{ex}:wrongnonce:0\n{ex}:realnonce:9\n", ex = MARK_EXIT);
        assert_eq!(parse_exit(&console, "realnonce"), Some(9));
        assert_eq!(parse_exit(&console, "noncepresent-but-absent"), None);
    }

    #[test]
    fn timeout_yields_none_exit_and_streams_bounded() {
        let o = CaptureOutcome {
            exit: None,
            timed_out: true,
            console: framed_console("n", b"partial", b"", 0),
            wall: Duration::from_secs(2),
            cpu_seconds: Some(1),
            command: None,
            stream_error: None,
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
        assert_eq!(res.usage.cpu_seconds, 1);
    }

    #[test]
    fn capture_head_bounds_each_stream_to_the_capture_bound() {
        let big = vec![b'x'; SANDBOX_CAPTURE_BOUND + 4096];
        let console = framed_console("n", &big, b"", 0);
        let out = capture_stream(&console, MARK_STDOUT_BEGIN, MARK_STDOUT_END, "n");
        assert_eq!(out.len(), SANDBOX_CAPTURE_BOUND);
        assert!(out.iter().all(|b| *b == b'x'));
    }

    #[test]
    fn legacy_console_redacts_before_taking_the_bounded_head() {
        let secret = b"SECRETVALUE";
        let mut bytes = vec![b'x'; SANDBOX_CAPTURE_BOUND - 4];
        bytes.extend_from_slice(secret);
        bytes.extend_from_slice(b"tail");
        let console = framed_console("n", &bytes, b"", 0);
        let plan = crate::redaction::RedactionPlan::for_needles([secret.to_vec()]).unwrap();

        let out = capture_stream_redacted(&console, MARK_STDOUT_BEGIN, MARK_STDOUT_END, "n", &plan);
        assert!(!out.windows(secret.len()).any(|window| window == secret));
        assert!(
            !out.windows(4).any(|window| window == &secret[..4]),
            "truncate-before-redact would leak this secret prefix"
        );
    }
}
