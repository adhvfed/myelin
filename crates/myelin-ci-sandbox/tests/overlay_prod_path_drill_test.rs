//! # CT-007 #26 — REAL-OverlayFS + REAL-runsc CoW rootfs verification drill
//!
//! **What this closes.** The per-job CoW rootfs overlay (`RootfsOverlayManager` +
//! `GvisorBackend::materialize_job_guest_root`, `src/gvisor.rs`) is code-complete and wired but
//! DORMANT: production (`runner_bind.rs`) does not yet install a manager, so `rootfs_overlay` is
//! `None` and every launch stages the bare digest-verified base. The one existing pristine-base test
//! (`compute_launch_guest_root_is_a_per_job_overlay_leaving_the_base_byte_pristine`, in `gvisor.rs`)
//! proves the property with [`RootfsOverlayMode::DeterministicDirectoryForTests`] (a plain `cp` of the
//! lower tree) and a FAKE run closure — NOT real kernel OverlayFS, NOT real `runsc`. This drill closes
//! that gap: it installs a real [`RootfsOverlayMode::OverlayFs`] manager (the exact seam the planned
//! production flip will wire via `.with_rootfs_overlay_manager(...)`) and proves, on the REAL kernel
//! OverlayFS + REAL `runsc` production path, three properties:
//!
//!   * **(a) base-pristine** — the digest-verified base rootfs is byte-identical (same canonical
//!     digest) after a job whose host-side layout writes land in the overlay; the shared base never
//!     drifts (this is the reproduced gate-2 release blocker: a build job mutated the base ⇒ the next
//!     runner startup panicked `DigestMismatch`).
//!   * **(b) zero escapes** — installing the overlay manager introduces NO new escape surface. The
//!     same AG-D4 escape corpus routed through the production `launch()` still attests
//!     `total_escapes == 0` (OverlayFS metacopy/redirect/userxattr add no boundary hole).
//!   * **(c) non-vacuity** — the per-job merged view genuinely EXISTED and its upperdir CAPTURED the
//!     writes (proven directly against the kernel), so a false green (overlay silently not used) is
//!     impossible.
//!
//! ## Two phases (both use the SAME `OverlayFs` manager on the SAME test thread)
//! `RootfsOverlayManager::initialize(OverlayFs, …)` enters a private mount namespace on the calling
//! thread; `create_overlay` (and the launch that calls it via `materialize_job_guest_root`) must run
//! on THAT thread. Production drives launch on a `std::thread::scope` child of the initializing thread;
//! this drill keeps it simplest and correct by initializing and driving both phases on the `#[test]`
//! thread itself.
//!
//!   * **Phase A — deterministic real-kernel OverlayFS CoW proof (a + c).** Mint a per-job merged view
//!     with the SAME public `create_overlay` that `materialize_job_guest_root` calls, then write into
//!     the merged view and prove the write landed in the upperdir and NOT the base, and that the base
//!     digest is byte-identical. This is non-vacuous by construction (`create_overlay` cannot be
//!     "silently skipped") and needs no assumptions about `runsc` mount-target behaviour.
//!   * **Phase B — real production launch + real runsc + zero escapes (a end-to-end + b).** Install the
//!     manager on a `GvisorBackend`, route the AG-D4 corpus through the production `launch()` seam
//!     (real `runsc`, non-root uid 65534), and assert the routing guard, `report.is_green()`,
//!     `total_escapes == 0`, and base-pristine before/after the real launch.
//!
//! ## Gating (opt-in, like the other escape drills)
//! File-gated behind `--features privileged-host-tests` (the real OverlayFS path needs
//! `CAP_SYS_ADMIN` for `unshare(CLONE_NEWNS)` + the `overlay` mount) and `#[ignore]` (opt-in even in
//! the privileged lane). Run:
//! `sudo -E cargo test -p myelin-ci-sandbox --features privileged-host-tests \
//!    --test overlay_prod_path_drill_test -- --ignored --nocapture`.
//! It SKIPS GRACEFULLY (returns early) when the substrate is unavailable, unless
//! `MYELIN_REQUIRE_OVERLAY=1` (then an absent substrate is a HARD failure — no vacuous green).

#![cfg(feature = "privileged-host-tests")]

use myelin_ci_sandbox::asset_registry::{GvisorAssetRegistry, RootfsAssetBinding};
use myelin_ci_sandbox::escape_corpus::{
    parse_console, Backend, BackendRun, DrillReport, EscapeAttestation,
};
use myelin_ci_sandbox::gvisor::{build_gvisor_corpus_script, GvisorBackend};
use myelin_ci_sandbox::rootfs_overlay::{
    RootfsOverlayManager, RootfsOverlayMode, WorkloadRootPermissions,
};
use myelin_ci_sandbox::{
    canonical_tree_sha256_hex, resolved_gvisor_rootfs, CompletionSettlementOwner, EgressPolicy,
    IdemToken, ImageRef, JobKind, JobSpec, MeterTarget, ReserveHandle, ResourceLimits,
    RunTokenCredential, RunnerHooks, SandboxBackend, TrustTier, WorkspaceSpec,
    LINUX_SMALL_V1_ROOTFS_SHA256,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The fork-bomb pids ceiling the AG-D4 corpus is parametrized with (same value the sibling drills
/// use).
const PIDS_MAX: u32 = 64;

/// The unprivileged uid the production seam drops the untrusted workload to. The routing guard asserts
/// the corpus's BEGIN marker reports THIS euid (proving the corpus reached the guest via the
/// production `launch()` command-runner / OCI, NOT a root drill harness).
const PROD_NONROOT_EUID: &str = "guest_euid=65534";
const ROOT_HARNESS_EUID: &str = "guest_euid=0";

/// The real, founder-pipeline-pinned `linux-small-v1` image, registered against the SAME base rootfs
/// [`resolved_gvisor_rootfs`] resolves (the exact binding the production gVisor drill uses).
fn linux_small_v1_image() -> ImageRef {
    ImageRef::pinned(format!(
        "myelin.local/linux-small-v1-rootfs@sha256:{LINUX_SMALL_V1_ROOTFS_SHA256}"
    ))
    .unwrap()
}

fn gvisor_test_registry() -> Arc<GvisorAssetRegistry> {
    Arc::new(
        GvisorAssetRegistry::from_bindings(vec![RootfsAssetBinding {
            image: linux_small_v1_image(),
            rootfs: resolved_gvisor_rootfs(),
        }])
        .expect("the base linux-small-v1 rootfs binding verifies"),
    )
}

fn ok_hooks() -> RunnerHooks {
    RunnerHooks::new(
        CompletionSettlementOwner::Hook,
        Box::new(|spec: &JobSpec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
        Box::new(|_spec, _h, _u| Ok(())),
        Box::new(|_t| Ok(())),
        Box::new(|_s| Ok(())),
    )
}

/// The hardened, digest-pinned, fully-default-deny corpus `JobSpec` — the EXACT shape a real untrusted
/// job takes (no NIC, read-only root, pids ceiling, caps dropped, non-root payload), with a small
/// scratch quota so `D2_diskfill` hits ENOSPC and is CONTAINED. Mirrors `escape_prod_path_test`'s
/// `corpus_spec` so the production-path corpus stays genuinely green.
fn corpus_spec(command: Vec<String>) -> JobSpec {
    JobSpec::new(
        JobKind::Ci,
        linux_small_v1_image(),
        command,
        vec![],
        vec![],
        EgressPolicy::deny_all(),
        ResourceLimits {
            cpu_millis: 1000,
            mem_bytes: 256 * 1024 * 1024,
            disk_bytes: 16 * 1024 * 1024,
            tmpfs_bytes: 16 * 1024 * 1024,
            pids_max: PIDS_MAX,
            timeout_secs: 120,
        },
        WorkspaceSpec::default(),
        TrustTier::Trusted,
        RunTokenCredential::new("test-bearer", "overlay-drill", 300).unwrap(),
        MeterTarget {
            reserve_id: "overlay-drill".into(),
        },
        IdemToken(format!("overlay-drill-{}", std::process::id())),
    )
    .unwrap()
}

fn runsc_on_path() -> bool {
    let bin = std::env::var("MYELIN_RUNSC_BIN").unwrap_or_else(|_| "runsc".to_string());
    if bin.contains('/') {
        return Path::new(&bin).exists();
    }
    std::env::var("PATH")
        .ok()
        .map(|p| p.split(':').any(|d| Path::new(d).join(&bin).exists()))
        .unwrap_or(false)
}

/// HARD-fail (no vacuous green) only when the operator demanded the drill really run.
fn require_substrate() -> bool {
    std::env::var("MYELIN_REQUIRE_OVERLAY").as_deref() == Ok("1")
}

#[test]
#[ignore = "privileged real-OverlayFS + real-runsc CoW drill: needs CAP_SYS_ADMIN + runsc + staged \
            gvisor rootfs + delegated cgroup v2; run explicitly with \
            --features privileged-host-tests -- --ignored (see module docs)"]
fn overlay_prod_path_pristine_base_and_zero_escapes_on_real_kernel_overlayfs() {
    // --- Preconditions: runsc + the staged base rootfs (Phase B needs a real runnable rootfs). ------
    if !runsc_on_path() || !resolved_gvisor_rootfs().exists() {
        let msg = format!(
            "runsc not on PATH or the staged gvisor rootfs ({}) is absent",
            resolved_gvisor_rootfs().display()
        );
        assert!(
            !require_substrate(),
            "[overlay drill] MYELIN_REQUIRE_OVERLAY=1 but {msg}: the drill cannot prove the CoW \
             overlay on the real path — a DATED NO-GO, never a vacuous green."
        );
        eprintln!("[overlay drill] SKIPPED: {msg}. (Stage runsc + a busybox-class rootfs to run it.)");
        return;
    }

    let registry = gvisor_test_registry();
    let image = linux_small_v1_image();
    let base = resolved_gvisor_rootfs();

    // --- Install the REAL kernel-OverlayFS manager on THIS thread (the seam the production flip will --
    //     wire via `.with_rootfs_overlay_manager(...)`). `initialize` unshares a private mount
    //     namespace; every overlay this manager mints — directly (Phase A) or via `launch()` (Phase B)
    //     — must be created on THIS thread, so both phases run here.
    let overlays_dir =
        std::env::temp_dir().join(format!("myelin-overlay-drill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&overlays_dir);
    let manager = match RootfsOverlayManager::initialize(
        RootfsOverlayMode::OverlayFs {
            overlays_dir: overlays_dir.clone(),
        },
        Arc::new(|_message: &str| {}),
    ) {
        Ok(manager) => Arc::new(manager),
        Err(error) => {
            let msg = format!(
                "real-OverlayFS manager init failed ({error:?}) — needs CAP_SYS_ADMIN for \
                 unshare(CLONE_NEWNS) + an overlay-capable kernel"
            );
            assert!(
                !require_substrate(),
                "[overlay drill] MYELIN_REQUIRE_OVERLAY=1 but {msg}."
            );
            eprintln!("[overlay drill] SKIPPED: {msg}.");
            return;
        }
    };

    // ================================================================================================
    // PHASE A — deterministic real-kernel OverlayFS CoW proof (requirements a + c).
    // Mint a per-job merged view with the SAME public `create_overlay` that `materialize_job_guest_root`
    // calls in production, write into it, and prove the write landed in the UPPERDIR and NOT the base,
    // with the base digest byte-identical. Non-vacuous by construction: the overlay is unconditionally
    // exercised here.
    // ================================================================================================
    let base_digest_phase_a = canonical_tree_sha256_hex(&base).expect("digest the verified base");

    {
        let verified = registry
            .resolve(&image)
            .expect("the pinned base resolves in the registry");
        // The euid runsc/the gofer run as — the merged root is chowned to self (no CAP_CHOWN), exactly
        // as `materialize_job_guest_root` derives it.
        let workload = WorkloadRootPermissions::new(
            // SAFETY: `geteuid`/`getegid` are always-successful, side-effect-free syscalls.
            unsafe { libc::geteuid() },
            unsafe { libc::getegid() },
            0o755,
        )
        .expect("0755 workload root permissions are valid");

        let overlay = manager
            .create_overlay(verified, "phaseA", workload)
            .expect("a real per-job kernel OverlayFS merged view mounts (metacopy=off,userxattr)");
        assert_eq!(
            manager.capacity_in_use(),
            1,
            "the live per-job overlay is charged while held"
        );

        let merged = overlay.path().to_path_buf();
        let upper = overlay.upperdir().to_path_buf();
        assert_ne!(
            merged, base,
            "the merged view MUST be a distinct per-job path, never the shared base"
        );
        assert!(
            merged.starts_with(&overlays_dir),
            "the merged view lives under the manager's overlay root: {merged:?}"
        );

        // (c) HOST-SIDE WRITE into the merged view — the exact class of write (mount-target creation /
        //     gofer layout) that corrupted the base before. It MUST land in the upperdir.
        let probe_dir = merged.join("myelin-overlay-drill-probe.d");
        let probe_file = merged.join("myelin-overlay-drill-probe.txt");
        std::fs::create_dir(&probe_dir).expect("create a probe dir in the merged view");
        std::fs::write(&probe_file, b"overlay-drill-upper").expect("write a probe file in the merged view");

        // Optionally exercise a whiteout over a real base file (no assumption if the base has none).
        let base_victim: Option<PathBuf> = std::fs::read_dir(&merged)
            .expect("enumerate the merged view")
            .flatten()
            .find(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
            .map(|e| e.file_name().into())
            .filter(|name: &PathBuf| {
                // Never the probe we just wrote — a genuine lower-layer file only.
                name != Path::new("myelin-overlay-drill-probe.txt")
            });
        if let Some(ref victim) = base_victim {
            std::fs::remove_file(merged.join(victim))
                .expect("delete a base file THROUGH the merged view (a whiteout in the upper)");
        }

        // (c) The write is captured in the UPPERDIR ...
        assert!(
            upper.join("myelin-overlay-drill-probe.txt").exists(),
            "the host-side write MUST be captured in the per-job upperdir {upper:?}"
        );
        assert!(
            upper.join("myelin-overlay-drill-probe.d").exists(),
            "the host-side mkdir MUST be captured in the per-job upperdir {upper:?}"
        );
        // ... and did NOT reach the shared base.
        assert!(
            !base.join("myelin-overlay-drill-probe.txt").exists(),
            "the host-side write MUST NOT appear in the shared base"
        );
        assert!(
            !base.join("myelin-overlay-drill-probe.d").exists(),
            "the host-side mkdir MUST NOT appear in the shared base"
        );
        if let Some(ref victim) = base_victim {
            assert!(
                base.join(victim).exists(),
                "a base file deleted THROUGH the merged view (whiteout) MUST still exist in the base: \
                 {victim:?}"
            );
        }

        // (a) The digest-verified base is byte-identical: neither the create nor the whiteout drifted it.
        assert_eq!(
            canonical_tree_sha256_hex(&base).expect("re-digest the base"),
            base_digest_phase_a,
            "PHASE A: the digest-verified base MUST be byte-identical after CoW writes landed in the \
             upper — this is the property whose violation was the DigestMismatch release blocker"
        );

        // Verified teardown returns the capacity charge (the guard unmounts + removes the per-job dir).
        overlay
            .dispose()
            .expect("verified overlay teardown (unmount + per-job dir removal) succeeds");
        assert_eq!(
            manager.capacity_in_use(),
            0,
            "the per-job overlay charge is released after verified teardown"
        );
    }
    println!(
        "[overlay drill] PHASE A green: real kernel OverlayFS is copy-on-write — host writes captured \
         in the upperdir, the digest-verified base byte-identical."
    );

    // ================================================================================================
    // PHASE B — real production launch + real runsc + zero escapes (requirements a end-to-end + b).
    // Install the SAME manager on the backend (the production-flip seam) and route the AG-D4 corpus
    // through the production `launch()` — real `runsc`, non-root uid 65534, --network=none, the
    // out-of-band memory cgroup, a scratch tmpfs sized from disk_bytes. Prove the base stays pristine
    // AND installing the overlay adds no escape surface.
    // ================================================================================================
    let backend = GvisorBackend::new(Arc::clone(&registry)).with_rootfs_overlay_manager(manager);
    let spec = corpus_spec(vec![
        "/bin/sh".into(),
        "-c".into(),
        build_gvisor_corpus_script(PIDS_MAX),
    ]);

    let base_digest_before = canonical_tree_sha256_hex(&base).expect("digest base before the launch");

    let launch = backend
        .launch(&spec, &ok_hooks())
        .expect("the production gVisor launch() runs a real runsc container of the AG-D4 corpus");
    let console = String::from_utf8_lossy(&launch.result.stdout).into_owned();
    println!(
        "=== overlay drill PHASE B: gVisor production-path run (exit={:?} timed_out={}) ===",
        launch.result.exit_code, launch.result.timed_out
    );
    println!("--- captured container stdout (via launch().result.stdout) ---\n{console}");

    // Best-effort whole-container teardown BEFORE the assertions, so a RED still cleans up.
    let _ = backend.kill(&launch.handle);

    // (a) end-to-end: the shared digest-verified base is byte-identical after the REAL runsc job — its
    //     host-side layout writes landed in the per-job overlay upper, not the base.
    assert_eq!(
        canonical_tree_sha256_hex(&base).expect("digest base after the launch"),
        base_digest_before,
        "PHASE B: the digest-verified base MUST be byte-identical after a REAL runsc job ran through \
         the overlay-installed production launch()"
    );

    // --- ROUTING GUARD: the corpus ran via the PRODUCTION non-root seam, NOT a root drill harness. ---
    assert!(
        console.contains(PROD_NONROOT_EUID),
        "[overlay drill] the corpus's BEGIN marker must report `{PROD_NONROOT_EUID}` — proving it ran \
         NON-ROOT through the production `launch()` seam (with the overlay installed).\n--- console \
         ---\n{console}"
    );
    assert!(
        !console.contains(ROOT_HARNESS_EUID),
        "[overlay drill] the corpus ran as ROOT (`{ROOT_HARNESS_EUID}`) — a drill-harness signature, \
         not the production seam.\n--- console ---\n{console}"
    );

    // (b) zero escapes: installing the overlay manager introduced NO new escape surface. Same corpus,
    //     same host-side `parse_console`, same green gate the sibling gVisor drills use.
    let report: DrillReport = parse_console(&console);
    println!("[overlay drill] PHASE B {}", report.summary());
    assert!(
        report.is_green(),
        "[overlay drill] PHASE B: the gVisor production-path corpus must be genuinely green with the \
         overlay INSTALLED (all families Contained, 0 escapes, corpus completed) — escapes={} \
         did_not_run={} corpus_completed={}.\n{}\n--- console ---\n{console}",
        report.escapes(),
        report.did_not_run(),
        report.corpus_completed,
        report.summary()
    );

    // Reuse the escape-corpus attestation the existing drills use: `from_green_drill` REFUSES over a
    // non-green report, so a successful mint asserting `total_escapes == 0` IS the pass condition.
    let kernel_version = console
        .lines()
        .find_map(|l| {
            l.find("kernel=").map(|i| {
                l[i + 7..]
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_string()
            })
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "4.19.0-gvisor".to_string());
    let date = std::env::var("MYELIN_DRILL_DATE").unwrap_or_else(|_| "2026-08-04".to_string());
    let attestation = EscapeAttestation::from_green_drill(
        date,
        &report,
        vec![BackendRun {
            backend: Backend::GvisorRunsc,
            exercised: true,
            residual_note: None,
        }],
        Backend::GvisorRunsc,
        "gvisor-rootfs-busybox(overlay-prod-launch)",
        "gvisor-kernel-runsc(overlay-prod-launch)",
        kernel_version,
    )
    .expect(
        "the overlay-installed gVisor production-path corpus is green ⇒ from_green_drill MUST mint the \
         zero-escape attestation (it refuses over a non-green report)",
    );
    assert_eq!(
        attestation.total_escapes, 0,
        "installing the per-job CoW overlay manager must add ZERO escapes on the production path"
    );
    println!(
        "[overlay drill] PHASE B green: {} — per-job CoW overlay (#26) leaves the base pristine on the \
         REAL runsc production path AND adds no escape surface.",
        attestation.green_line()
    );

    let _ = std::fs::remove_dir_all(&overlays_dir);
}
