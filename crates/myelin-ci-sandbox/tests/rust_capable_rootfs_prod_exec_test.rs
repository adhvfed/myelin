#![cfg(feature = "integration")]

use myelin_ci_sandbox::asset_registry::{GvisorAssetRegistry, RootfsAssetBinding};
use myelin_ci_sandbox::gvisor::GvisorBackend;
use myelin_ci_sandbox::{
    resolved_gvisor_rust_rootfs, EgressPolicy, IdemToken, ImageRef, JobKind, JobSpec, MeterTarget,
    ReserveHandle, ResourceLimits, RunTokenCredential, RunnerHooks, SandboxBackend, TrustTier,
    WorkspaceSpec, LINUX_RUST_V1_ROOTFS_SHA256,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

static REAL_RUST_ROOTFS_LOCK: Mutex<()> = Mutex::new(());

fn resolved_rust_rootfs() -> PathBuf {
    resolved_gvisor_rust_rootfs()
}

fn linux_rust_v1_image() -> ImageRef {
    ImageRef::pinned(format!(
        "myelin.local/linux-rust-v1-rootfs@sha256:{LINUX_RUST_V1_ROOTFS_SHA256}"
    ))
    .unwrap()
}

fn test_registry() -> Arc<GvisorAssetRegistry> {
    Arc::new(
        GvisorAssetRegistry::from_bindings(vec![RootfsAssetBinding {
            image: linux_rust_v1_image(),
            rootfs: resolved_rust_rootfs(),
        }])
        .expect("the linux-rust-v1 rootfs binding verifies"),
    )
}

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

fn preconditions() -> Option<String> {
    let bin = runsc_bin()?;
    let rootfs = resolved_rust_rootfs();
    if !rootfs.join("usr/local/bin/rustc").exists() || !rootfs.join("usr/local/bin/cargo").exists()
    {
        return None;
    }
    Some(bin)
}

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
         absent - this host cannot run the Rust-capability gVisor sandbox. (CI/dev machines \
         without this asset stay green; run ./scripts/build-rust-rootfs.sh to stage it.)",
        resolved_rust_rootfs().display()
    );
    None
}

fn spec_running(command: Vec<String>, timeout_secs: u32) -> JobSpec {
    JobSpec::new(
        JobKind::Ci,
        linux_rust_v1_image(),
        command,
        vec![],
        vec![],
        EgressPolicy::deny_all(),
        ResourceLimits {
            cpu_millis: 1000,
            mem_bytes: 512 * 1024 * 1024,
            disk_bytes: 1 << 30,
            tmpfs_bytes: 1 << 30,
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

fn ok_hooks() -> RunnerHooks {
    RunnerHooks::new(
        myelin_ci_sandbox::CompletionSettlementOwner::Hook,
        Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
        Box::new(|_spec, _h, _u| Ok(())),
        Box::new(|_t| Ok(())),
        Box::new(|_s| Ok(())),
    )
}

fn launch_against_rust_rootfs(
    backend: &GvisorBackend,
    spec: &JobSpec,
) -> Result<
    myelin_ci_sandbox::SandboxLaunch,
    myelin_ci_sandbox::SandboxLaunchError<myelin_ci_sandbox::gvisor::GvisorError>,
> {
    backend.launch(spec, &ok_hooks())
}

#[test]
fn real_runsc_runs_rustc_and_cargo_version_in_rust_rootfs() {
    let Some(_bin) = require_or_skip("rust-rootfs-prod-exec version-banner") else {
        return;
    };
    let _serial = REAL_RUST_ROOTFS_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let backend = GvisorBackend::new(test_registry());
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
        "rustc --version && cargo --version must succeed inside the sandbox - got stderr {stderr:?}"
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
    let backend = GvisorBackend::new(test_registry());
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
        "the untrusted process must run NON-ROOT (uid 65534) inside the sandbox - hardening must \
         hold identically to every other job. got: {stdout:?}"
    );
    assert!(
        stdout.contains("NETWORK_CONTAINED"),
        "default-deny egress (no network namespace interface) must hold identically to every \
         other job - the rootfs swap must not open a network path. got: {stdout:?}"
    );

    backend.kill(&launch.handle).expect("teardown");
}

#[test]
fn resolve_is_cheap_after_construction_pays_the_real_verification_cost_once() {
    let Some(_bin) = require_or_skip("rust-rootfs-prod-exec resolve-is-cheap") else {
        return;
    };
    let _serial = REAL_RUST_ROOTFS_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());

    let construct_started = std::time::Instant::now();
    let registry = GvisorAssetRegistry::from_bindings(vec![RootfsAssetBinding {
        image: linux_rust_v1_image(),
        rootfs: resolved_rust_rootfs(),
    }])
    .expect("the real staged linux-rust-v1 binding verifies");
    let construct_elapsed = construct_started.elapsed();
    eprintln!(
        "resolve_is_cheap: GvisorAssetRegistry::from_bindings (real construction-time verification \
         of the >800MiB rust rootfs) took {construct_elapsed:?}"
    );

    let image = linux_rust_v1_image();
    let resolve_started = std::time::Instant::now();
    for _ in 0..1000 {
        registry.resolve(&image).expect("already-verified lookup");
    }
    let resolve_elapsed = resolve_started.elapsed();
    eprintln!(
        "resolve_is_cheap: 1000x GvisorAssetRegistry::resolve (cheap O(1) lookup, post-construction) \
         took {resolve_elapsed:?} total ({:?} avg/call)",
        resolve_elapsed / 1000
    );

    assert!(
        resolve_elapsed < std::time::Duration::from_millis(200),
        "1000 resolve() calls took {resolve_elapsed:?} - resolve must be a cheap O(1) lookup with \
         no I/O or hashing, not a rehash of the registered directory"
    );
}
