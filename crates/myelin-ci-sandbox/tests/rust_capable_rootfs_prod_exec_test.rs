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
//! --version && cargo --version'` inside a REAL `runsc` (gVisor) sandbox, when `spec.image` is the
//! REAL `linux-rust-v1` image and the backend's registry maps it to the Rust-capable rootfs staged
//! by `scripts/build-rust-rootfs.sh`. Only the rootfs CONTENT differs from the plain busybox base
//! rootfs; the launch/hardening code path is byte-identical.
//!
//! This mirrors `tests/gvisor_prod_exec_test.rs` (style, gating, `LiveOutput`-free simple launch,
//! `REAL_*_LOCK` serialization idiom) — read that file first if editing this one.
//!
//! ## Gating (CI without runsc/the staged rust-rootfs still passes; THIS host must really run it)
//! SKIPPED GRACEFULLY (returns early, NOT failed) when `runsc` is not on PATH OR the Rust-capable
//! rootfs is not staged (env override `MYELIN_GVISOR_RUST_ROOTFS`, default
//! `~/.local/share/gvisor-assets/rust-rootfs`) — so CI/dev machines without this asset stay green.
//! With `MYELIN_REQUIRE_RUST_ROOTFS=1` (mirroring the existing `MYELIN_REQUIRE_RUNSC`) an absent
//! capability is a HARD FAILURE (panic), never a vacuous green. Run:
//! `MYELIN_REQUIRE_RUNSC=1 MYELIN_REQUIRE_RUST_ROOTFS=1 cargo test -p myelin-ci-sandbox --features
//! integration --test rust_capable_rootfs_prod_exec_test -- --nocapture`.
//!
//! ## CT-007 gate 2/4: a REAL registry entry, not an env-var override
//! This test used to point the SHARED `MYELIN_GVISOR_ROOTFS` env var at the Rust-capable rootfs for
//! its own process before calling `launch` — since production ignored `spec.image` entirely and
//! always resolved from that one env var, ANY image could be dispatched while this test secretly
//! rerouted every job (including one declaring an unrelated image) onto the Rust rootfs. That was
//! exactly the "security theatre" CT-007 gate 2/4 closes: `spec.image` is now the real launch
//! authority, so this test registers the REAL `linux-rust-v1` image
//! (`myelin.local/linux-rust-v1-rootfs@sha256:<LINUX_RUST_V1_ROOTFS_SHA256>`, the same pin committed
//! in `runner-assets.toml`) against [`resolved_gvisor_rust_rootfs`] and calls `launch` directly — no
//! env-var mutation, no restore-after dance, and an unrelated image genuinely could NOT resolve to
//! this rootfs.

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

/// Serializes every test in this file — mirrors the existing `REAL_RUNSC_LOCK` idiom in
/// `gvisor_prod_exec_test.rs` (shared host-level `runsc` container-id state).
static REAL_RUST_ROOTFS_LOCK: Mutex<()> = Mutex::new(());

/// The resolved Rust-capable rootfs path — re-exported from `myelin_ci_sandbox::gvisor` (the SAME
/// resolver the production registry composition would use); kept as a local alias since the rest of
/// this file already refers to it as `resolved_rust_rootfs()`.
fn resolved_rust_rootfs() -> PathBuf {
    resolved_gvisor_rust_rootfs()
}

/// The REAL, `runner-assets.toml`-pinned `linux-rust-v1` image.
fn linux_rust_v1_image() -> ImageRef {
    ImageRef::pinned(format!(
        "myelin.local/linux-rust-v1-rootfs@sha256:{LINUX_RUST_V1_ROOTFS_SHA256}"
    ))
    .unwrap()
}

/// A registry mapping the REAL `linux-rust-v1` image to the REAL staged rust-rootfs directory — the
/// authority this test's `GvisorBackend` resolves `spec.image` through.
fn test_registry() -> Arc<GvisorAssetRegistry> {
    Arc::new(
        GvisorAssetRegistry::from_bindings(vec![RootfsAssetBinding {
            image: linux_rust_v1_image(),
            rootfs: resolved_rust_rootfs(),
        }])
        .expect("the linux-rust-v1 rootfs binding verifies"),
    )
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
        linux_rust_v1_image(),
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

/// Run `command` against the Rust-capable rootfs via the REAL registry (`test_registry()`) — no env
/// var mutation, no restore-after dance. `spec.image` names the real `linux-rust-v1` image; the
/// registry is the only thing that turns that into an actual rootfs.
fn launch_against_rust_rootfs(
    backend: &GvisorBackend,
    spec: &JobSpec,
) -> Result<myelin_ci_sandbox::SandboxLaunch, myelin_ci_sandbox::gvisor::GvisorError> {
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
    let backend = GvisorBackend::new(test_registry());
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

/// **Issue 1 (the whole point): construction pays the real hashing cost ONCE; `resolve` after that
/// is cheap.** Before the fix, `GvisorAssetRegistry::resolve_verified` re-canonicalized AND re-hashed
/// the ENTIRE registered directory on EVERY call — measured ~15s per full rehash of this >800MiB
/// Rust asset on the founder-dogfood host, paid before the isolation floor even ran on every single
/// job launch. This test builds the registry (paying the real construction-time verification cost —
/// timed and printed, not hard-asserted, since it varies by host/disk) against the REAL staged
/// `linux-rust-v1` asset, then calls the NOW-CHEAP [`myelin_ci_sandbox::asset_registry::GvisorAssetRegistry::resolve`]
/// 1000 times and asserts the TOTAL time for all 1000 lookups is under 200ms — several orders of
/// magnitude faster than a single old-style rehash, proving `resolve` is a pure O(1) map lookup with
/// no I/O or hashing left in it.
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
        "1000 resolve() calls took {resolve_elapsed:?} — resolve must be a cheap O(1) lookup with \
         no I/O or hashing, not a rehash of the registered directory"
    );
}
