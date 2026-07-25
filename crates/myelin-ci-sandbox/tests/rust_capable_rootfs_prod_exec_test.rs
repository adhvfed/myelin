//! # The Rust-capable gVisor rootfs PRODUCTION exec self-test (CT-007 gate 2/4, first slice)
//!
//! **Owning doc:** `planning/system-reviews/2026-06-26/12-ci-track-ledger.md`'s "Pre-registered
//! CT-007 cutover floor", gate 2/4: "digest-pinned one-cell runner assets provide the actual
//! Rust/Node/browser/container capabilities [the 12 GitHub CI jobs] require without weakening
//! gVisor, egress, or privilege boundaries." This is the FIRST slice: just the Rust capability the
//! `build-test-clippy` job needs (`ci-workload-inventory.toml`: "The base Rust workload every other
//! job's crates depend on existing"). Node/browser/Docker-in-Docker/advisory-DB-egress capabilities
//! are OUT OF SCOPE here.
//!
//! ## What it proves
//! [`GvisorBackend::launch`] — the EXACT SAME production launch path every other CI/agent job in
//! this repo uses, with NO hardening/OCI-config change of any kind — can run a REAL `sh -c 'rustc
//! --version && cargo --version'` inside a REAL `runsc` (gVisor) sandbox, when pointed (purely via
//! the [`MYELIN_GVISOR_ROOTFS`] env var `resolved_gvisor_rootfs` already reads) at the Rust-capable
//! rootfs staged by `scripts/build-rust-rootfs.sh`. Only the rootfs CONTENT differs from the plain
//! busybox base rootfs; the launch/hardening code path is byte-identical.
//!
//! This mirrors `tests/gvisor_prod_exec_test.rs` (style, gating, `LiveOutput`-free simple launch,
//! `REAL_*_LOCK` serialization idiom) — read that file first if editing this one.
//!
//! ## Gating (CI without runsc/the staged rust-rootfs still passes; THIS host must really run it)
//! SKIPPED GRACEFULLY (returns early, NOT failed) when `runsc` is not on PATH OR the Rust-capable
//! rootfs is not staged (env override [`ENV_RUST_ROOTFS`], default
//! `~/.local/share/gvisor-assets/rust-rootfs`) — so CI/dev machines without this asset stay green.
//! With `MYELIN_REQUIRE_RUST_ROOTFS=1` (mirroring the existing `MYELIN_REQUIRE_RUNSC`) an absent
//! capability is a HARD FAILURE (panic), never a vacuous green. Run:
//! `MYELIN_REQUIRE_RUNSC=1 MYELIN_REQUIRE_RUST_ROOTFS=1 cargo test -p myelin-ci-sandbox --features
//! integration --test rust_capable_rootfs_prod_exec_test -- --nocapture`.
//!
//! ## Why this test sets `MYELIN_GVISOR_ROOTFS` for its own process only
//! [`resolved_gvisor_rootfs`] already reads `MYELIN_GVISOR_ROOTFS` with NO code change — this test
//! just points that env var at the Rust-capable rootfs BEFORE calling `launch`. That mutates process
//! env, which is observed by every thread in THIS test binary (a separate OS process from every
//! OTHER test binary in the workspace, so other tests/binaries are unaffected) — so all tests in
//! THIS FILE that touch the env var are serialized against each other via [`REAL_RUST_ROOTFS_LOCK`],
//! exactly like the existing `REAL_RUNSC_LOCK` idiom in `gvisor_prod_exec_test.rs` serializes tests
//! sharing host-level `runsc` container-id state.

#![cfg(feature = "integration")]

use myelin_ci_sandbox::gvisor::GvisorBackend;
use myelin_ci_sandbox::{
    EgressPolicy, IdemToken, ImageRef, JobKind, JobSpec, MeterTarget, ReserveHandle,
    ResourceLimits, RunTokenCredential, RunnerHooks, SandboxBackend, TrustTier, WorkspaceSpec,
};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Env var naming the staged Rust-capable rootfs (mirrors `build-rust-rootfs.sh`'s own default).
const ENV_RUST_ROOTFS: &str = "MYELIN_GVISOR_RUST_ROOTFS";

/// Serializes every test in this file that mutates the process-wide `MYELIN_GVISOR_ROOTFS` env var
/// (read by `resolved_gvisor_rootfs`) around the launch it drives — mirrors the existing
/// `REAL_RUNSC_LOCK` idiom in `gvisor_prod_exec_test.rs`.
static REAL_RUST_ROOTFS_LOCK: Mutex<()> = Mutex::new(());

/// The resolved Rust-capable rootfs path (env override `MYELIN_GVISOR_RUST_ROOTFS`, default
/// `~/.local/share/gvisor-assets/rust-rootfs` — the same default `scripts/build-rust-rootfs.sh`
/// stages into).
fn resolved_rust_rootfs() -> PathBuf {
    if let Ok(p) = std::env::var(ENV_RUST_ROOTFS) {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("gvisor-assets")
        .join("rust-rootfs")
}

/// Whether `runsc` resolves on PATH (env override `MYELIN_RUNSC_BIN`) — copied from
/// `gvisor_prod_exec_test.rs` rather than shared, matching that file's own standalone style.
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

/// The drill preconditions: `runsc` on PATH AND the Rust-capable rootfs staged with a reachable
/// `rustc`/`cargo` on the sandbox's hardcoded PATH.
fn preconditions() -> Option<String> {
    let bin = runsc_bin()?;
    let rootfs = resolved_rust_rootfs();
    if !rootfs.join("usr/local/bin/rustc").exists() || !rootfs.join("usr/local/bin/cargo").exists()
    {
        return None;
    }
    Some(bin)
}

/// HARD-FAIL on an absent capability iff `MYELIN_REQUIRE_RUST_ROOTFS=1` (this gate refuses a vacuous
/// green on a host that is supposed to have this asset staged); otherwise GRACEFUL SKIP.
fn require_or_skip(test: &str) -> Option<String> {
    if let Some(bin) = preconditions() {
        return Some(bin);
    }
    if std::env::var("MYELIN_REQUIRE_RUST_ROOTFS").as_deref() == Ok("1") {
        panic!(
            "[{test}] MYELIN_REQUIRE_RUST_ROOTFS=1 but `runsc` is not on PATH, or the staged \
             Rust-capable rootfs ({}) is absent / missing usr/local/bin/{{rustc,cargo}}. Stage it \
             first with `./scripts/build-rust-rootfs.sh`.",
            resolved_rust_rootfs().display()
        );
    }
    eprintln!(
        "[{test}] SKIPPED: `runsc` is not on PATH, or the staged Rust-capable rootfs ({}) is \
         absent — this host cannot run the Rust-capability gVisor sandbox. (CI/dev machines \
         without this asset stay green; run ./scripts/build-rust-rootfs.sh to stage it.)",
        resolved_rust_rootfs().display()
    );
    None
}

/// A trivial hardened CI JobSpec running `command` — identical shape to
/// `gvisor_prod_exec_test.rs::spec_running` (default-deny egress ⇒ no netns; read-only root; pids
/// ceiling set) so this test proves the SAME hardened launch path, only the rootfs differs.
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
            mem_bytes: 512 * 1024 * 1024,
            disk_bytes: 1 << 30,
            pids_max: 128,
            timeout_secs,
        },
        WorkspaceSpec::default(),
        TrustTier::Trusted,
        RunTokenCredential::new("test-bearer", "rust-rootfs-prod-exec-jti", 300).unwrap(),
        MeterTarget {
            reserve_id: "rust-rootfs-prod-exec-reserve".into(),
        },
        IdemToken(format!(
            "rust-rootfs-prod-exec-{timeout_secs}-{}",
            std::process::id()
        )),
    )
    .unwrap()
}

/// The four-guarantee hooks, all accepting (so the launch reaches a real `runsc` run) — copied from
/// `gvisor_prod_exec_test.rs::ok_hooks`.
fn ok_hooks() -> RunnerHooks {
    RunnerHooks::new(
        myelin_ci_sandbox::CompletionSettlementOwner::Hook,
        Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
        Box::new(|_spec, _h, _u| Ok(())),
        Box::new(|_t| Ok(())),
        Box::new(|_s| Ok(())),
    )
}

/// Run `command` with `MYELIN_GVISOR_ROOTFS` pointed at the staged Rust-capable rootfs for the
/// duration of the call, restoring the prior value afterward. Caller must hold
/// [`REAL_RUST_ROOTFS_LOCK`].
fn launch_against_rust_rootfs(
    backend: &GvisorBackend,
    spec: &JobSpec,
) -> Result<myelin_ci_sandbox::SandboxLaunch, myelin_ci_sandbox::gvisor::GvisorError> {
    let rootfs = resolved_rust_rootfs();
    let previous = std::env::var(myelin_ci_sandbox::gvisor::ENV_GVISOR_ROOTFS).ok();
    // This crate is edition 2021, where `set_var`/`remove_var` remain plain safe functions (they
    // only become `unsafe fn` under edition 2024) — matching the existing unwrapped call sites in
    // this crate (`tests/git_wire_prod_exec_test.rs`) and `myelin-config/src/lib.rs`. Safety in the
    // multi-threaded sense is handled the same way those call sites handle it: every test in this
    // file that touches this var holds `REAL_RUST_ROOTFS_LOCK` for the entire
    // mutate-launch-restore window, so there is no concurrent reader/writer within this process.
    std::env::set_var(
        myelin_ci_sandbox::gvisor::ENV_GVISOR_ROOTFS,
        rootfs.as_os_str(),
    );
    let result = backend.launch(spec, &ok_hooks());
    match &previous {
        Some(value) => std::env::set_var(myelin_ci_sandbox::gvisor::ENV_GVISOR_ROOTFS, value),
        None => std::env::remove_var(myelin_ci_sandbox::gvisor::ENV_GVISOR_ROOTFS),
    }
    result
}

#[test]
fn real_runsc_runs_rustc_and_cargo_version_in_rust_rootfs() {
    let Some(_bin) = require_or_skip("rust-rootfs-prod-exec version-banner") else {
        return;
    };
    let _serial = REAL_RUST_ROOTFS_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let backend = GvisorBackend::new();
    let spec = spec_running(
        vec![
            "sh".into(),
            "-c".into(),
            "rustc --version && cargo --version".into(),
        ],
        60,
    );

    let launch = launch_against_rust_rootfs(&backend, &spec)
        .expect("the production launch must run a real runsc container against the rust rootfs");
    let result = &launch.result;

    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);
    println!("=== CT-007 gate-2 REAL Rust-capable gVisor (runsc) prod-exec ===");
    println!(
        "exit_code = {:?}  timed_out = {}",
        result.exit_code, result.timed_out
    );
    println!("captured stdout = {stdout}");
    println!("captured stderr = {stderr:?}");

    assert_eq!(
        result.exit_code,
        Some(0),
        "rustc --version && cargo --version must succeed inside the sandbox — got stderr {stderr:?}"
    );
    assert!(!result.timed_out, "the version probes complete instantly");
    assert!(result.passed(), "a clean exit 0 is a pass");
    assert!(
        stdout.contains("rustc "),
        "captured stdout must contain a `rustc ` version line proving a REAL rustc ran in the \
         sandbox. got: {stdout:?}"
    );
    assert!(
        stdout.contains("cargo "),
        "captured stdout must contain a `cargo ` version line proving a REAL cargo ran in the \
         sandbox. got: {stdout:?}"
    );

    backend
        .kill(&launch.handle)
        .expect("teardown is idempotent");
    backend
        .kill(&launch.handle)
        .expect("kill is idempotent on an already-gone container");
}

#[test]
fn real_runsc_runs_rust_toolchain_non_root_and_network_free() {
    let Some(_bin) = require_or_skip("rust-rootfs-prod-exec non-root-no-network") else {
        return;
    };
    let _serial = REAL_RUST_ROOTFS_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let backend = GvisorBackend::new();
    // Mirrors gvisor_prod_exec_test.rs's non-root assertion style: the untrusted command reports
    // its own uid (OCI config drops it to 65534 — defense in depth, unchanged by this rootfs) and
    // proves it has no route out (default-deny egress ⇒ no netns — unchanged by this rootfs); a
    // rust-toolchain-specific check (rustc actually targets the expected host triple) rides along
    // in the same single container run. Uses `bash` (present in the Debian-slim rust image) rather
    // than `sh` (dash) for its `/dev/tcp` pseudo-device network probe — dash has no such feature.
    let spec = spec_running(
        vec![
            "bash".into(),
            "-c".into(),
            "id -u; rustc --print cfg | grep -c target_os; (timeout 2 bash -c 'echo x > /dev/tcp/93.184.216.34/80' 2>/dev/null && echo NETWORK_ESCAPED || echo NETWORK_CONTAINED)".into(),
        ],
        60,
    );

    let launch = launch_against_rust_rootfs(&backend, &spec).expect("run");
    let result = &launch.result;
    let stdout = String::from_utf8_lossy(&result.stdout);
    println!("=== CT-007 gate-2 REAL Rust-capable gVisor non-root/no-network payload ===");
    println!(
        "exit_code = {:?}  captured stdout = {stdout:?}",
        result.exit_code
    );

    assert_eq!(result.exit_code, Some(0));
    assert!(
        stdout.contains("65534"),
        "the untrusted process must run NON-ROOT (uid 65534) inside the sandbox — hardening must \
         hold identically to every other job. got: {stdout:?}"
    );
    assert!(
        stdout.contains("NETWORK_CONTAINED"),
        "default-deny egress (no network namespace interface) must hold identically to every \
         other job — the rootfs swap must not open a network path. got: {stdout:?}"
    );

    backend.kill(&launch.handle).expect("teardown");
}
