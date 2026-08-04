#![cfg(feature = "integration")]

use myelin_ci_sandbox::firecracker::{
    resolved_kernel_path, resolved_rootfs_path, FirecrackerBackend,
};
use myelin_ci_sandbox::{
    EgressPolicy, IdemToken, ImageRef, JobKind, JobSpec, MeterTarget, ReserveHandle,
    ResourceLimits, RunTokenCredential, RunnerHooks, SandboxBackend, SandboxCancellation,
    SandboxOutputSink, SandboxOutputStream, TrustTier, WorkspaceSpec, SANDBOX_CAPTURE_BOUND,
};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct LiveOutput {
    frames: Mutex<Vec<(SandboxOutputStream, Vec<u8>)>>,
    arrived: Condvar,
}

impl SandboxOutputSink for LiveOutput {
    fn emit(&self, stream: SandboxOutputStream, frame: &[u8]) -> Result<(), String> {
        self.frames.lock().unwrap().push((stream, frame.to_vec()));
        self.arrived.notify_all();
        Ok(())
    }
}

fn preconditions() -> bool {
    let has_kvm = Path::new("/dev/kvm").exists();
    let has_fc = which_on_path("firecracker", "MYELIN_FC_BIN");
    let assets = resolved_kernel_path().exists() && resolved_rootfs_path().exists();
    has_kvm && has_fc && assets
}

fn which_on_path(default_bin: &str, env_override: &str) -> bool {
    let bin = std::env::var(env_override).unwrap_or_else(|_| default_bin.to_string());
    if bin.contains('/') {
        return Path::new(&bin).exists();
    }
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            if Path::new(dir).join(&bin).exists() {
                return true;
            }
        }
    }
    false
}

fn skip_or_panic(test: &str) -> bool {
    if preconditions() {
        return false;
    }
    if std::env::var("MYELIN_REQUIRE_KVM").as_deref() == Ok("1") {
        panic!(
            "[{test}] MYELIN_REQUIRE_KVM=1 but the host cannot boot a real microVM (/dev/kvm absent, \
             `firecracker` not on PATH, or the staged guest assets missing). CT-002a refuses a \
             VACUOUS green: a real microVM MUST boot and run spec.command here."
        );
    }
    eprintln!(
        "[{test}] SKIPPED: /dev/kvm or `firecracker` or the staged guest assets are absent - this \
         host cannot boot a microVM. (CI without KVM passes.)"
    );
    true
}

fn spec_running(command: Vec<String>, timeout_secs: u32) -> JobSpec {
    JobSpec::new(
        JobKind::Ci,
        ImageRef::pinned("registry.example/runner@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef").unwrap(),
        command,
        vec![],
        vec![],
        EgressPolicy::deny_all(),
        ResourceLimits {
            cpu_millis: 1000,
            mem_bytes: 256 * 1024 * 1024,
            disk_bytes: 1 << 30,
            tmpfs_bytes: 1 << 30,
            pids_max: 128,
            timeout_secs,
        },
        WorkspaceSpec::default(),
        TrustTier::Trusted,
        RunTokenCredential::new("test-bearer", "prod-exec-jti", 300).unwrap(),
        MeterTarget {
            reserve_id: "prod-exec-reserve".into(),
        },
        IdemToken(format!("prod-exec-{timeout_secs}-{}", std::process::id())),
    )
    .unwrap()
}

fn ok_hooks() -> RunnerHooks {
    RunnerHooks::new(
        myelin_ci_sandbox::CompletionSettlementOwner::Hook,
        Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
        Box::new(|_spec, _h, _u| Ok(())),
        Box::new(|_t| Ok(())),
        Box::new(|_s| Ok(())),
    )
}

fn settling_hooks(settlements: Arc<AtomicUsize>) -> RunnerHooks {
    RunnerHooks::new(
        myelin_ci_sandbox::CompletionSettlementOwner::Hook,
        Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
        Box::new(move |_spec, _handle, _usage| {
            settlements.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }),
        Box::new(|_token| Ok(())),
        Box::new(|_spec| Ok(())),
    )
}

#[test]
fn real_microvm_runs_command_and_captures_exit_stdout_stderr() {
    if skip_or_panic("fc-prod-exec exit7") {
        return;
    }
    let backend = FirecrackerBackend::new();
    let spec = spec_running(
        vec![
            "sh".into(),
            "-c".into(),
            "echo hello-stdout; echo oops 1>&2; exit 7".into(),
        ],
        60,
    );

    let launch = backend
        .launch(&spec, &ok_hooks())
        .expect("the production launch must boot a real microVM and run spec.command");
    let result = &launch.result;

    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);
    println!("=== CT-002a REAL Firecracker prod-exec (exit-7 case) ===");
    println!(
        "exit_code = {:?}  timed_out = {}",
        result.exit_code, result.timed_out
    );
    println!("usage = {:?}", result.usage);
    println!("captured stdout = {stdout:?}");
    println!("captured stderr = {stderr:?}");

    assert_eq!(
        result.exit_code,
        Some(7),
        "the REAL guest command exited 7 (captured from the nonce-framed serial-console marker)"
    );
    assert!(
        !result.timed_out,
        "the command completed well within the timeout"
    );
    assert!(!result.passed(), "a non-zero exit is not a pass");
    assert!(
        stdout.contains("hello-stdout"),
        "captured stdout must contain the command's stdout. got: {stdout:?}"
    );
    assert!(
        stderr.contains("oops"),
        "captured stderr must contain the command's stderr. got: {stderr:?}"
    );

    backend
        .kill(&launch.handle)
        .expect("teardown whole-guest-kill is idempotent");
    backend
        .kill(&launch.handle)
        .expect("kill is idempotent on an already-gone guest");
}

#[test]
fn real_microvm_delivers_output_before_the_command_exits() {
    if skip_or_panic("fc-prod-exec live-output") {
        return;
    }
    let backend = FirecrackerBackend::new();
    let spec = spec_running(
        vec![
            "sh".into(),
            "-c".into(),
            "printf live-before; sleep 2; printf live-after".into(),
        ],
        60,
    );
    let output = Arc::new(LiveOutput::default());

    let launch = std::thread::scope(|scope| {
        let output_for_launch = output.clone();
        let hooks = ok_hooks();
        let backend_ref = &backend;
        let spec_ref = &spec;
        let running = scope.spawn(move || {
            backend_ref.launch_streaming(
                spec_ref,
                &hooks,
                output_for_launch,
                SandboxCancellation::new(),
            )
        });

        let frames = output.frames.lock().unwrap();
        let (frames, wait) = output
            .arrived
            .wait_timeout_while(frames, Duration::from_secs(20), |frames| frames.is_empty())
            .unwrap();
        assert!(!wait.timed_out(), "first microVM output frame arrives");
        assert!(
            !running.is_finished(),
            "the first callback is observable while the guest command is still sleeping"
        );
        drop(frames);
        running
            .join()
            .expect("launch thread")
            .expect("real Firecracker launch")
    });

    let stdout: Vec<u8> = output
        .frames
        .lock()
        .unwrap()
        .iter()
        .filter(|(stream, _)| *stream == SandboxOutputStream::Stdout)
        .flat_map(|(_, frame)| frame.iter().copied())
        .collect();
    assert_eq!(stdout, b"live-beforelive-after");
    backend.kill(&launch.handle).expect("teardown");
}

#[test]
fn real_microvm_cancels_live_after_log_failure_and_returns_measured_failure() {
    if skip_or_panic("fc-prod-exec live-output-cancel") {
        return;
    }
    let backend = FirecrackerBackend::new();
    let spec = spec_running(
        vec![
            "sh".into(),
            "-c".into(),
            "printf before-cancel; sleep 30; printf too-late".into(),
        ],
        60,
    );
    let output = Arc::new(LiveOutput::default());
    let cancellation = SandboxCancellation::new();
    let settlements = Arc::new(AtomicUsize::new(0));
    let started = Instant::now();

    let launch = std::thread::scope(|scope| {
        let hooks = settling_hooks(settlements.clone());
        let output_for_launch = output.clone();
        let cancellation_for_launch = cancellation.clone();
        let backend_for_launch = &backend;
        let running = scope.spawn(move || {
            backend_for_launch.launch_streaming(
                &spec,
                &hooks,
                output_for_launch,
                cancellation_for_launch,
            )
        });
        let frames = output.frames.lock().unwrap();
        let (frames, wait) = output
            .arrived
            .wait_timeout_while(frames, Duration::from_secs(20), |frames| frames.is_empty())
            .unwrap();
        assert!(!wait.timed_out(), "the pre-cancel frame arrives");
        drop(frames);
        cancellation.cancel();
        running
            .join()
            .expect("launch thread")
            .expect("cancelled durable output retains the measured result")
    });

    assert!(
        started.elapsed() < Duration::from_secs(10),
        "whole-guest cancellation is prompt, not the 30s command sleep"
    );
    assert!(!launch.output_complete);
    assert!(!launch.result.passed());
    assert_eq!(
        settlements.load(Ordering::SeqCst),
        1,
        "the acquired reservation settles measured usage once on cancellation"
    );
    backend.kill(&launch.handle).expect("teardown");
}

#[test]
fn real_microvm_atomically_reaps_a_session_escaped_forking_descendant() {
    if skip_or_panic("fc-prod-exec payload-cgroup-kill") {
        return;
    }
    let backend = FirecrackerBackend::new();
    let spec = spec_running(
        vec![
            "sh".into(),
            "-c".into(),
            "setsid sh -c 'while :; do sleep 1 & done' >/dev/null 2>&1 & printf leader-done".into(),
        ],
        20,
    );

    let launch = backend
        .launch(&spec, &ok_hooks())
        .expect("the payload cgroup tears down every descendant after the leader exits");
    assert_eq!(launch.result.exit_code, Some(0));
    assert!(
        !launch.result.timed_out,
        "a session-escaped process retaining the FIFO writer is killed atomically, not at timeout"
    );
    assert_eq!(launch.result.stdout, b"leader-done");
    backend.kill(&launch.handle).expect("teardown");
}

#[test]
fn real_microvm_runaway_stdout_is_capped_without_deadlock() {
    if skip_or_panic("fc-prod-exec runaway-stdout-cap") {
        return;
    }
    let backend = FirecrackerBackend::new();
    let timeout_secs = 120u32;
    let spec = spec_running(
        vec![
            "sh".into(),
            "-c".into(),
            "dd if=/dev/zero bs=4096 count=512 2>/dev/null | tr '\\0' 'x'".into(),
        ],
        timeout_secs,
    );

    let start = Instant::now();
    let launch = backend
        .launch(&spec, &ok_hooks())
        .expect("the production launch must boot and run the runaway-output command");
    let elapsed = start.elapsed();
    let result = &launch.result;

    println!("=== CT-002c REAL Firecracker runaway-stdout host-memory-DoS cap ===");
    println!(
        "exit_code = {:?}  timed_out = {}  elapsed = {:?}",
        result.exit_code, result.timed_out, elapsed
    );
    println!(
        "captured stdout.len() = {}  (bound = {})",
        result.stdout.len(),
        SANDBOX_CAPTURE_BOUND
    );

    assert!(
        result.stdout.len() <= SANDBOX_CAPTURE_BOUND,
        "captured stdout MUST be head-bounded to <= {SANDBOX_CAPTURE_BOUND}; got {} (a runaway guest \
         must not be able to force the host to buffer the whole stream)",
        result.stdout.len()
    );
    assert!(
        !result.timed_out,
        "a 2 MiB completing flood must NOT time out - the host must keep draining the >cap console so \
         the guest's base64 dump never blocks (a timeout here = the host stopped draining and the \
         guest deadlocked on a full console pipe)"
    );
    assert!(
        elapsed < Duration::from_secs(u64::from(timeout_secs) - 30),
        "the guest must complete + reboot WELL under the {timeout_secs}s ceiling (no hang); elapsed {elapsed:?}"
    );

    backend
        .kill(&launch.handle)
        .expect("teardown whole-guest-kill is idempotent");
}

#[test]
fn real_microvm_command_past_timeout_is_whole_guest_killed() {
    if skip_or_panic("fc-prod-exec timeout") {
        return;
    }
    let backend = FirecrackerBackend::new();
    let spec = spec_running(vec!["sh".into(), "-c".into(), "sleep 30".into()], 2);

    let start = Instant::now();
    let launch = backend
        .launch(&spec, &ok_hooks())
        .expect("the launch returns after the timeout whole-guest-kill");
    let elapsed = start.elapsed();
    let result = &launch.result;

    println!("=== CT-002a REAL Firecracker prod-exec (timeout case) ===");
    println!(
        "exit_code = {:?}  timed_out = {}  elapsed = {:?}",
        result.exit_code, result.timed_out, elapsed
    );
    println!("usage = {:?}", result.usage);

    assert!(
        result.timed_out,
        "the sleep past timeout_secs=2 must time out"
    );
    assert_eq!(
        result.exit_code, None,
        "a timed-out (killed) guest has no trustworthy exit code - never fabricated as 0"
    );
    assert!(!result.passed(), "a timed-out job is not a pass");
    assert!(
        elapsed.as_secs() < 25,
        "the guest must be KILLED at the timeout, not allowed to run the full 30s sleep (elapsed {elapsed:?})"
    );

    backend.kill(&launch.handle).expect("teardown");
}

#[test]
fn real_microvm_job_cannot_forge_the_exit_code() {
    if skip_or_panic("fc-prod-exec forge") {
        return;
    }
    let backend = FirecrackerBackend::new();
    let spec = spec_running(
        vec![
            "sh".into(),
            "-c".into(),
            "printf '__MYELIN_EXIT__:guessednonce:0\\n'; echo real-output; exit 5".into(),
        ],
        60,
    );

    let launch = backend.launch(&spec, &ok_hooks()).expect("boot + run");
    let result = &launch.result;
    let stdout = String::from_utf8_lossy(&result.stdout);
    println!("=== CT-002a REAL Firecracker forge-resistance (real kernel) ===");
    println!(
        "exit_code = {:?} (must be 5, NOT the forged 0)",
        result.exit_code
    );
    println!("captured stdout = {stdout:?}");

    assert_eq!(
        result.exit_code,
        Some(5),
        "the forged `__MYELIN_EXIT__:guessednonce:0` the job printed CANNOT spoof the real exit 5"
    );
    assert!(
        stdout.contains("__MYELIN_EXIT__:guessednonce:0"),
        "the forged marker survives only as captured-stdout DATA (base64-framed), never as a marker"
    );
    assert!(stdout.contains("real-output"));

    backend.kill(&launch.handle).expect("teardown");
}

#[test]
fn real_microvm_nonroot_payload_cannot_forge_exit_via_console_write() {
    if skip_or_panic("fc-prod-exec structural-forge") {
        return;
    }
    let backend = FirecrackerBackend::new();
    let attack = r#"
N=$(tr -cd '[:print:]\n' < /dev/vdb 2>/dev/null | sed -n "s/^N='\([0-9a-f]*\)'.*/\1/p" | head -1)
echo "id=$(id)"
echo "nonce_read_len=${#N}"
if printf '__MYELIN_EXIT__:%s:0\n' "$N" > /dev/console 2>/dev/null; then
  echo "console-write: OK"
else
  echo "console-write: DENIED"
fi
exit 3
"#;
    let spec = spec_running(vec!["sh".into(), "-c".into(), attack.into()], 60);
    let launch = backend.launch(&spec, &ok_hooks()).expect("boot + run");
    let result = &launch.result;
    let stdout = String::from_utf8_lossy(&result.stdout);
    println!("=== CT-002a STRUCTURAL forge-resistance (non-root payload, real kernel) ===");
    println!(
        "exit_code = {:?} (must be 3, NEVER the forged 0)",
        result.exit_code
    );
    println!("captured stdout = {stdout:?}");

    assert!(
        stdout.contains("console-write: DENIED"),
        "the NON-ROOT payload must be DENIED writing the root-only /dev/console (the structural \
         forge boundary). got stdout: {stdout:?}"
    );
    assert!(
        !stdout.contains("console-write: OK"),
        "the payload must NOT be able to write /dev/console. got stdout: {stdout:?}"
    );
    assert!(
        stdout.contains("uid=65534"),
        "the untrusted payload must run NON-ROOT (uid=65534). got stdout: {stdout:?}"
    );
    assert_eq!(
        result.exit_code,
        Some(3),
        "the forged exit-0 (even with the REAL nonce) cannot win - the non-root payload could not \
         write the console at all, so the only exit line is init's trusted 3"
    );
    assert!(
        !result.timed_out,
        "the command completed within the timeout"
    );
    assert!(
        !result.passed(),
        "a non-zero exit is not a pass - the forge did NOT flip it to a pass"
    );

    backend.kill(&launch.handle).expect("teardown");
}
