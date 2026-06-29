//! # The Firecracker PRODUCTION exec self-test (CT-002a → P-544, M2) — REAL microVM runs spec.command
//!
//! **Owning architecture (byte-authoritative):**
//! `continuous-integration/architecture/02-internals-and-algorithms.md` §5.1 (Firecracker microVM —
//! the production default for untrusted code) + §5.3 (the mandatory hardening profile). **Contract:**
//! `contract-index.md` row 8.4 (the unified sandbox — the Firecracker half). **Doctrine:** EI-04 §5.1
//! (a property not drilled on a real kernel is a CLAIM, not a fact — so this self-test REALLY boots);
//! EI-01 §3 (prove-it: observability is part of the pass).
//!
//! ## What it proves (the CT-002a DONE bar)
//! The DEFAULT backend's [`SandboxBackend::launch`](myelin_ci_sandbox::SandboxBackend::launch) now
//! boots a REAL Firecracker microVM that RUNS the untrusted `spec.command` (NOT `init=/bin/true`) and
//! captures its REAL outcome from the guest serial console:
//!   1. `sh -c 'echo hello-stdout; echo oops 1>&2; exit 7'` ⇒ `exit_code == Some(7)`, stdout-capture
//!      contains `hello-stdout`, stderr-capture contains `oops`.
//!   2. A command that sleeps past `timeout_secs=2` ⇒ `timed_out == true`, `exit_code == None`, the
//!      whole guest was killed (the run returns well under the sleep).
//!   3. FORGE-RESISTANCE on a REAL kernel: a job that PRINTS a fake `__MYELIN_EXIT__:<guess>:0` to its
//!      own stdout cannot spoof the captured exit code (it does not know the per-boot nonce) — the
//!      REAL exit (5) is captured and the forged text appears only as captured-stdout DATA.
//!
//! ## Gating (CI without KVM still passes; THIS host must really boot)
//! SKIPPED GRACEFULLY (returns early, NOT failed) when `/dev/kvm` is absent, `firecracker` is not on
//! PATH, or the staged guest assets are missing — so CI without KVM stays green. With
//! `MYELIN_REQUIRE_KVM=1` an absent capability is a HARD FAILURE (panic), never a vacuous green — the
//! CT-002a DONE bar refuses a green that did not really boot a microVM. Run:
//! `MYELIN_REQUIRE_KVM=1 cargo test -p myelin-ci-sandbox --features integration --test firecracker_prod_exec_test -- --nocapture`.

#![cfg(feature = "integration")]

use myelin_ci_sandbox::firecracker::{
    resolved_kernel_path, resolved_rootfs_path, FirecrackerBackend,
};
use myelin_ci_sandbox::{
    EgressPolicy, IdemToken, ImageRef, JobKind, JobSpec, MeterTarget, ReserveHandle, ResourceLimits,
    RunTokenRef, RunnerHooks, SandboxBackend, TrustTier, WorkspaceSpec,
};
use std::path::Path;
use std::time::Instant;

/// KVM + firecracker + staged-asset availability (the same preconditions the hardened-boot self-test
/// gates on). Returns `false` ⇒ graceful skip (unless `MYELIN_REQUIRE_KVM=1`, which makes it a panic).
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

/// HARD-FAIL on an absent capability iff `MYELIN_REQUIRE_KVM=1` (the M2 exit gate refuses a vacuous
/// green); otherwise GRACEFUL SKIP. Returns `true` if the caller should skip.
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
        "[{test}] SKIPPED: /dev/kvm or `firecracker` or the staged guest assets are absent — this \
         host cannot boot a microVM. (CI without KVM passes.)"
    );
    true
}

/// A trivial hardened CI JobSpec running `command`, with the given `timeout_secs` (default-deny
/// egress ⇒ no NIC; read-only root; pids ceiling set).
fn spec_running(command: Vec<String>, timeout_secs: u32) -> JobSpec {
    JobSpec::new(
        JobKind::Ci,
        ImageRef::pinned("registry.example/runner@sha256:0123456789abcdef0123456789abcdef").unwrap(),
        command,
        vec![],
        vec![],
        EgressPolicy::deny_all(),
        ResourceLimits {
            cpu_millis: 1000,
            mem_bytes: 256 * 1024 * 1024,
            disk_bytes: 1 << 30,
            pids_max: 128,
            timeout_secs,
        },
        WorkspaceSpec::default(),
        TrustTier::Trusted,
        RunTokenRef {
            jti: "prod-exec-jti".into(),
        },
        MeterTarget {
            reserve_id: "prod-exec-reserve".into(),
        },
        IdemToken(format!("prod-exec-{timeout_secs}-{}", std::process::id())),
    )
    .unwrap()
}

/// The four-guarantee hooks, all accepting (so the launch reaches a real boot).
fn ok_hooks() -> RunnerHooks {
    RunnerHooks {
        reserve: Box::new(|m| Ok(ReserveHandle(m.reserve_id.clone()))),
        settle: Box::new(|_h, _u| Ok(())),
        attribute: Box::new(|_t| Ok(())),
        isolation_floor: Box::new(|_s| Ok(())),
    }
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
    println!("exit_code = {:?}  timed_out = {}", result.exit_code, result.timed_out);
    println!("usage = {:?}", result.usage);
    println!("captured stdout = {stdout:?}");
    println!("captured stderr = {stderr:?}");

    assert_eq!(
        result.exit_code,
        Some(7),
        "the REAL guest command exited 7 (captured from the nonce-framed serial-console marker)"
    );
    assert!(!result.timed_out, "the command completed well within the timeout");
    assert!(!result.passed(), "a non-zero exit is not a pass");
    assert!(
        stdout.contains("hello-stdout"),
        "captured stdout must contain the command's stdout. got: {stdout:?}"
    );
    assert!(
        stderr.contains("oops"),
        "captured stderr must contain the command's stderr. got: {stderr:?}"
    );

    backend.kill(&launch.handle).expect("teardown whole-guest-kill is idempotent");
    backend.kill(&launch.handle).expect("kill is idempotent on an already-gone guest");
}

#[test]
fn real_microvm_command_past_timeout_is_whole_guest_killed() {
    if skip_or_panic("fc-prod-exec timeout") {
        return;
    }
    let backend = FirecrackerBackend::new();
    // Sleep 30s with a 2s ceiling — the whole guest must be killed at ~2s.
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

    assert!(result.timed_out, "the sleep past timeout_secs=2 must time out");
    assert_eq!(
        result.exit_code, None,
        "a timed-out (killed) guest has no trustworthy exit code — never fabricated as 0"
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
    // FORGE ATTEMPT on a REAL kernel: the job prints a fake exit marker (with a GUESSED nonce it
    // cannot know) to its own stdout, then really exits 5. The real exit (5) must be captured.
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
    println!("exit_code = {:?} (must be 5, NOT the forged 0)", result.exit_code);
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

/// STRUCTURAL forge-resistance on a REAL kernel (CT-002a → P-548). The earlier forge test prints a
/// fake marker with a GUESSED nonce; this one is the worst case the security probe confirmed: the
/// untrusted payload READS the per-boot nonce in plaintext off `/dev/vdb` (the init script lives
/// there) and tries to write a forged `__MYELIN_EXIT__:<REAL nonce>:0` directly to `/dev/console`.
///
/// The fix makes this STRUCTURALLY impossible (not merely unlikely): the payload runs NON-ROOT
/// (`setpriv --reuid/--regid 65534`), and `/dev/console`/`/dev/ttyS0` are root-only (`crw-------`),
/// so the console write is DENIED by the kernel regardless of whether the payload knows the nonce.
/// The test asserts (a) the payload's own diagnostics report the console-write was DENIED, and
/// (b) `launch` yields the REAL non-zero exit (3), never the forged 0 — locking the fix against
/// regression (e.g. a future edit that drops the `--reuid` and reintroduces a root payload).
#[test]
fn real_microvm_nonroot_payload_cannot_forge_exit_via_console_write() {
    if skip_or_panic("fc-prod-exec structural-forge") {
        return;
    }
    let backend = FirecrackerBackend::new();
    // The payload: (1) read the REAL nonce off /dev/vdb; (2) attempt to write a forged exit-0 marker
    // bearing that real nonce to /dev/console; (3) report whether the write succeeded or was denied;
    // (4) really exit 3. The forged 0 must NOT be captured, and the write must be DENIED.
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
    println!("exit_code = {:?} (must be 3, NEVER the forged 0)", result.exit_code);
    println!("captured stdout = {stdout:?}");

    // (a) the kernel DENIED the non-root payload's /dev/console write (the structural boundary).
    assert!(
        stdout.contains("console-write: DENIED"),
        "the NON-ROOT payload must be DENIED writing the root-only /dev/console (the structural \
         forge boundary). got stdout: {stdout:?}"
    );
    assert!(
        !stdout.contains("console-write: OK"),
        "the payload must NOT be able to write /dev/console. got stdout: {stdout:?}"
    );
    // Confirm the payload is genuinely non-root (uid 65534), proving the boundary is in force.
    assert!(
        stdout.contains("uid=65534"),
        "the untrusted payload must run NON-ROOT (uid=65534). got stdout: {stdout:?}"
    );
    // (b) the REAL exit (3) is captured; the forged `:0` it tried to inject never reaches the parser.
    assert_eq!(
        result.exit_code,
        Some(3),
        "the forged exit-0 (even with the REAL nonce) cannot win — the non-root payload could not \
         write the console at all, so the only exit line is init's trusted 3"
    );
    assert!(!result.timed_out, "the command completed within the timeout");
    assert!(!result.passed(), "a non-zero exit is not a pass — the forge did NOT flip it to a pass");

    backend.kill(&launch.handle).expect("teardown");
}
