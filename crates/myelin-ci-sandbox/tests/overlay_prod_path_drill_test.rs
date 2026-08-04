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

const PIDS_MAX: u32 = 64;

const PROD_NONROOT_EUID: &str = "guest_euid=65534";
const ROOT_HARNESS_EUID: &str = "guest_euid=0";

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

fn require_substrate() -> bool {
    std::env::var("MYELIN_REQUIRE_OVERLAY").as_deref() == Ok("1")
}

#[test]
#[ignore = "privileged real-OverlayFS + real-runsc CoW drill: needs CAP_SYS_ADMIN + runsc + staged \
            gvisor rootfs + delegated cgroup v2; run explicitly with \
            --features privileged-host-tests -- --ignored (see module docs)"]
fn overlay_prod_path_pristine_base_and_zero_escapes_on_real_kernel_overlayfs() {
    if !runsc_on_path() || !resolved_gvisor_rootfs().exists() {
        let msg = format!(
            "runsc not on PATH or the staged gvisor rootfs ({}) is absent",
            resolved_gvisor_rootfs().display()
        );
        assert!(
            !require_substrate(),
            "[overlay drill] MYELIN_REQUIRE_OVERLAY=1 but {msg}: the drill cannot prove the CoW \
             overlay on the real path - a DATED NO-GO, never a vacuous green."
        );
        eprintln!("[overlay drill] SKIPPED: {msg}. (Stage runsc + a busybox-class rootfs to run it.)");
        return;
    }

    let registry = gvisor_test_registry();
    let image = linux_small_v1_image();
    let base = resolved_gvisor_rootfs();

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
                "real-OverlayFS manager init failed ({error:?}) - needs CAP_SYS_ADMIN for \
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

    let base_digest_phase_a = canonical_tree_sha256_hex(&base).expect("digest the verified base");

    {
        let verified = registry
            .resolve(&image)
            .expect("the pinned base resolves in the registry");
        let workload = WorkloadRootPermissions::new(
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

        let probe_dir = merged.join("myelin-overlay-drill-probe.d");
        let probe_file = merged.join("myelin-overlay-drill-probe.txt");
        std::fs::create_dir(&probe_dir).expect("create a probe dir in the merged view");
        std::fs::write(&probe_file, b"overlay-drill-upper").expect("write a probe file in the merged view");

        let base_victim: Option<PathBuf> = std::fs::read_dir(&merged)
            .expect("enumerate the merged view")
            .flatten()
            .find(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
            .map(|e| e.file_name().into())
            .filter(|name: &PathBuf| {
                name != Path::new("myelin-overlay-drill-probe.txt")
            });
        if let Some(ref victim) = base_victim {
            std::fs::remove_file(merged.join(victim))
                .expect("delete a base file THROUGH the merged view (a whiteout in the upper)");
        }

        assert!(
            upper.join("myelin-overlay-drill-probe.txt").exists(),
            "the host-side write MUST be captured in the per-job upperdir {upper:?}"
        );
        assert!(
            upper.join("myelin-overlay-drill-probe.d").exists(),
            "the host-side mkdir MUST be captured in the per-job upperdir {upper:?}"
        );
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

        assert_eq!(
            canonical_tree_sha256_hex(&base).expect("re-digest the base"),
            base_digest_phase_a,
            "PHASE A: the digest-verified base MUST be byte-identical after CoW writes landed in the \
             upper - this is the property whose violation was the DigestMismatch release blocker"
        );

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
        "[overlay drill] PHASE A green: real kernel OverlayFS is copy-on-write - host writes captured \
         in the upperdir, the digest-verified base byte-identical."
    );

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

    let _ = backend.kill(&launch.handle);

    assert_eq!(
        canonical_tree_sha256_hex(&base).expect("digest base after the launch"),
        base_digest_before,
        "PHASE B: the digest-verified base MUST be byte-identical after a REAL runsc job ran through \
         the overlay-installed production launch()"
    );

    assert!(
        console.contains(PROD_NONROOT_EUID),
        "[overlay drill] the corpus's BEGIN marker must report `{PROD_NONROOT_EUID}` - proving it ran \
         NON-ROOT through the production `launch()` seam (with the overlay installed).\n--- console \
         ---\n{console}"
    );
    assert!(
        !console.contains(ROOT_HARNESS_EUID),
        "[overlay drill] the corpus ran as ROOT (`{ROOT_HARNESS_EUID}`) - a drill-harness signature, \
         not the production seam.\n--- console ---\n{console}"
    );

    let report: DrillReport = parse_console(&console);
    println!("[overlay drill] PHASE B {}", report.summary());
    assert!(
        report.is_green(),
        "[overlay drill] PHASE B: the gVisor production-path corpus must be genuinely green with the \
         overlay INSTALLED (all families Contained, 0 escapes, corpus completed) - escapes={} \
         did_not_run={} corpus_completed={}.\n{}\n--- console ---\n{console}",
        report.escapes(),
        report.did_not_run(),
        report.corpus_completed,
        report.summary()
    );

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
        "[overlay drill] PHASE B green: {} - per-job CoW overlay (#26) leaves the base pristine on the \
         REAL runsc production path AND adds no escape surface.",
        attestation.green_line()
    );

    let _ = std::fs::remove_dir_all(&overlays_dir);
}
