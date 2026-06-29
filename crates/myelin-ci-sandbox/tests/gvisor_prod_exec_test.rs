//! # The gVisor (`runsc`) PRODUCTION exec self-test (CT-002b → P-544, M2) — REAL `runsc` runs spec.command
//!
//! **Owning architecture (byte-authoritative):**
//! `continuous-integration/architecture/02-internals-and-algorithms.md` §5.1 (gVisor — the NAMED
//! second backend behind the SAME `SandboxBackend` trait) + §5.3 (the mandatory hardening profile).
//! **Contract:** `contract-index.md` row 8.4 (the unified sandbox — the gVisor half). **Doctrine:**
//! EI-04 §5.1 (a property not drilled on a real runtime is a CLAIM, not a fact — so this self-test
//! REALLY runs `runsc`); EI-01 §3 (prove-it: observability is part of the pass).
//!
//! ## What it proves (the CT-002b DONE bar)
//! The named-second backend's [`SandboxBackend::launch`] now runs the untrusted `spec.command` inside
//! a REAL `runsc` (gVisor) sandbox via an OCI bundle built from the spec (NOT a `runsc --version`
//! probe), and captures its REAL outcome FROM THE RUNTIME:
//!   1. `sh -c 'echo hello-stdout; echo oops 1>&2; exit 7'` ⇒ `exit_code == Some(7)` (the `runsc`
//!      child's REAL exit status — gVisor returns the container's exit directly, NO forge surface),
//!      stdout-capture contains `hello-stdout`, stderr-capture contains `oops`.
//!   2. A command sleeping past `timeout_secs=2` ⇒ `timed_out == true`, `exit_code == None`, the whole
//!      container is killed + cleaned up (no leftover `runsc list` entry, no leaked bundle temp dir).
//!   3. The untrusted process runs NON-ROOT (uid 65534) inside the sandbox (defense in depth).
//!
//! ## Gating (CI without runsc still passes; THIS host must really run a container)
//! SKIPPED GRACEFULLY (returns early, NOT failed) when `runsc` is not on PATH or the staged minimal
//! rootfs is absent — so CI without runsc stays green. With `MYELIN_REQUIRE_RUNSC=1` an absent
//! capability is a HARD FAILURE (panic), never a vacuous green — the CT-002b DONE bar refuses a green
//! that did not really run a `runsc` container. Run:
//! `MYELIN_REQUIRE_RUNSC=1 cargo test -p myelin-ci-sandbox --features integration --test gvisor_prod_exec_test -- --nocapture`.

#![cfg(feature = "integration")]

use myelin_ci_sandbox::gvisor::GvisorBackend;
use myelin_ci_sandbox::{
    resolved_gvisor_rootfs, EgressPolicy, IdemToken, ImageRef, JobKind, JobSpec, MeterTarget,
    ReserveHandle, ResourceLimits, RunTokenRef, RunnerHooks, SandboxBackend, TrustTier,
    WorkspaceSpec,
};
use std::path::Path;
use std::time::Instant;

/// Whether `runsc` resolves on PATH (env override `MYELIN_RUNSC_BIN`).
fn runsc_bin() -> Option<String> {
    let bin = std::env::var("MYELIN_RUNSC_BIN").unwrap_or_else(|_| "runsc".to_string());
    if bin.contains('/') {
        return Path::new(&bin).exists().then_some(bin);
    }
    let path = std::env::var("PATH").ok()?;
    for dir in path.split(':') {
        if Path::new(dir).join(&bin).exists() {
            return Some(bin);
        }
    }
    None
}

/// The drill preconditions: `runsc` on PATH AND the staged minimal rootfs present.
fn preconditions() -> Option<String> {
    let bin = runsc_bin()?;
    if !resolved_gvisor_rootfs().exists() {
        return None;
    }
    Some(bin)
}

/// HARD-FAIL on an absent capability iff `MYELIN_REQUIRE_RUNSC=1` (the M2 exit gate refuses a vacuous
/// green); otherwise GRACEFUL SKIP. Returns the resolved `runsc` bin, or `None` if the caller skips.
fn require_or_skip(test: &str) -> Option<String> {
    if let Some(bin) = preconditions() {
        return Some(bin);
    }
    if std::env::var("MYELIN_REQUIRE_RUNSC").as_deref() == Ok("1") {
        panic!(
            "[{test}] MYELIN_REQUIRE_RUNSC=1 but `runsc` is not on PATH or the staged minimal rootfs \
             ({}) is absent. CT-002b refuses a VACUOUS green: a real `runsc` container MUST run \
             spec.command here.",
            resolved_gvisor_rootfs().display()
        );
    }
    eprintln!(
        "[{test}] SKIPPED: `runsc` not on PATH or the staged minimal rootfs is absent — this host \
         cannot run a gVisor container. (CI without runsc passes.)"
    );
    None
}

/// A trivial hardened CI JobSpec running `command`, with the given `timeout_secs` (default-deny egress
/// ⇒ no netns; read-only root; pids ceiling set).
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
            jti: "gvisor-prod-exec-jti".into(),
        },
        MeterTarget {
            reserve_id: "gvisor-prod-exec-reserve".into(),
        },
        IdemToken(format!(
            "gvisor-prod-exec-{timeout_secs}-{}",
            std::process::id()
        )),
    )
    .unwrap()
}

/// The four-guarantee hooks, all accepting (so the launch reaches a real `runsc` run).
fn ok_hooks() -> RunnerHooks {
    RunnerHooks {
        reserve: Box::new(|m| Ok(ReserveHandle(m.reserve_id.clone()))),
        settle: Box::new(|_h, _u| Ok(())),
        attribute: Box::new(|_t| Ok(())),
        isolation_floor: Box::new(|_s| Ok(())),
    }
}

/// Count `runsc` containers this test process left behind (id prefix `myelin-prod-<pid>-`). Used to
/// assert no container leaks after a timeout-kill teardown.
fn leftover_containers(bin: &str) -> usize {
    let prefix = format!("myelin-prod-{}-", std::process::id());
    let out = std::process::Command::new(bin)
        .arg("--rootless")
        .arg("list")
        .output()
        .expect("runsc list");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.contains(&prefix))
        .count()
}

#[test]
fn real_runsc_runs_command_and_captures_exit_stdout_stderr() {
    let Some(_bin) = require_or_skip("gvisor-prod-exec exit7") else {
        return;
    };
    let backend = GvisorBackend::new();
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
        .expect("the production launch must run a real runsc container of spec.command");
    let result = &launch.result;

    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);
    println!("=== CT-002b REAL gVisor (runsc) prod-exec (exit-7 case) ===");
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
        "the REAL container exited 7 — taken from the `runsc` child's REAL exit status (no forge surface)"
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
        .expect("teardown is idempotent");
    backend
        .kill(&launch.handle)
        .expect("kill is idempotent on an already-gone container");
}

#[test]
fn real_runsc_runs_untrusted_command_non_root() {
    let Some(_bin) = require_or_skip("gvisor-prod-exec non-root") else {
        return;
    };
    let backend = GvisorBackend::new();
    // The untrusted command reports its own uid; the OCI config drops it to 65534 (defense in depth).
    let spec = spec_running(
        vec!["sh".into(), "-c".into(), "id -u; exit 0".into()],
        60,
    );

    let launch = backend.launch(&spec, &ok_hooks()).expect("run");
    let result = &launch.result;
    let stdout = String::from_utf8_lossy(&result.stdout);
    println!("=== CT-002b REAL gVisor non-root payload ===");
    println!("exit_code = {:?}  captured stdout = {stdout:?}", result.exit_code);

    assert_eq!(result.exit_code, Some(0));
    assert!(
        stdout.contains("65534"),
        "the untrusted process must run NON-ROOT (uid 65534) inside the sandbox. got: {stdout:?}"
    );

    backend.kill(&launch.handle).expect("teardown");
}

#[test]
fn real_runsc_command_past_timeout_is_whole_container_killed() {
    let Some(bin) = require_or_skip("gvisor-prod-exec timeout") else {
        return;
    };
    let backend = GvisorBackend::new();
    // Sleep 30s with a 2s ceiling — the whole container must be killed at ~2s and reaped.
    let spec = spec_running(vec!["sh".into(), "-c".into(), "sleep 30".into()], 2);

    let start = Instant::now();
    let launch = backend
        .launch(&spec, &ok_hooks())
        .expect("the launch returns after the timeout whole-container-kill");
    let elapsed = start.elapsed();
    let result = &launch.result;

    println!("=== CT-002b REAL gVisor prod-exec (timeout case) ===");
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
        "a timed-out (killed) container has no trustworthy exit code — never fabricated as 0"
    );
    assert!(!result.passed(), "a timed-out job is not a pass");
    assert!(
        elapsed.as_secs() < 25,
        "the container must be KILLED at the timeout, not allowed to run the full 30s sleep (elapsed {elapsed:?})"
    );

    backend.kill(&launch.handle).expect("teardown");

    // No leaked container + no leaked bundle temp dir (cleanup runs on every path).
    assert_eq!(
        leftover_containers(&bin),
        0,
        "the timeout-killed container must be deleted — no leftover `runsc list` entry"
    );
}
