#![cfg(feature = "integration")]

use myelin_ci_sandbox::asset_registry::{GvisorAssetRegistry, RootfsAssetBinding};
use myelin_ci_sandbox::escape_corpus::{
    build_corpus_script, parse_console, AttackOutcome, Backend, BackendRun, DrillReport,
    EscapeAttestation,
};
use myelin_ci_sandbox::firecracker::{
    resolved_kernel_path, resolved_rootfs_path, FirecrackerBackend,
};
use myelin_ci_sandbox::gvisor::{build_gvisor_corpus_script, GvisorBackend};
use myelin_ci_sandbox::{
    resolved_gvisor_rootfs, EgressPolicy, IdemToken, ImageRef, JobKind, JobSpec, MeterTarget,
    ReserveHandle, ResourceLimits, RunTokenCredential, RunnerHooks, SandboxBackend, TrustTier,
    WorkspaceSpec, LINUX_SMALL_V1_ROOTFS_SHA256,
};
use std::path::Path;
use std::sync::Arc;

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

const PIDS_MAX: u32 = 64;

const PROD_NONROOT_EUID: &str = "guest_euid=65534";
const ROOT_HARNESS_EUID: &str = "guest_euid=0";

const GATE_BOUNDARY_FAMILY: &[&str] = &[
    "M1_metadata_ssrf",
    "C1_controlplane",
    "T1_crosstenant",
    "E1_secret_exfil",
    "D1_root_readonly",
];

const PRIVILEGE_FAMILY: &[&str] = &["K1_module", "K2_devmem", "K3_ioport", "K4_mount"];

const GATE_DISKFILL: &str = "D2_diskfill";

const GATE_FORKBOMB: &str = "F1_forkbomb";

const GATE_MEMHOG: &str = "Mx_memhog";

fn ok_hooks() -> RunnerHooks {
    RunnerHooks::new(
        myelin_ci_sandbox::CompletionSettlementOwner::Hook,
        Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
        Box::new(|_spec, _h, _u| Ok(())),
        Box::new(|_t| Ok(())),
        Box::new(|_s| Ok(())),
    )
}

fn corpus_spec(command: Vec<String>, tag: &str) -> JobSpec {
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
        RunTokenCredential::new("test-bearer", format!("ct003-{tag}"), 300).unwrap(),
        MeterTarget {
            reserve_id: format!("ct003-{tag}"),
        },
        IdemToken(format!("ct003-{tag}-{}", std::process::id())),
    )
    .unwrap()
}

fn which_on_path(default_bin: &str, env_override: &str) -> bool {
    let bin = std::env::var(env_override).unwrap_or_else(|_| default_bin.to_string());
    if bin.contains('/') {
        return Path::new(&bin).exists();
    }
    std::env::var("PATH")
        .ok()
        .map(|p| p.split(':').any(|d| Path::new(d).join(&bin).exists()))
        .unwrap_or(false)
}

fn outcome_of<'a>(report: &'a DrillReport, id: &str) -> &'a AttackOutcome {
    &report
        .outcomes
        .iter()
        .find(|(i, _, _)| *i == id)
        .unwrap_or_else(|| panic!("attack id `{id}` missing from the parsed corpus catalogue"))
        .2
}

fn assert_production_path_containment(
    backend_label: &str,
    console: &str,
    f1_marker_authoritative: bool,
) -> DrillReport {
    assert!(
        console.contains(PROD_NONROOT_EUID),
        "[CT-003 routing guard] {backend_label}: the corpus's BEGIN marker must report \
         `{PROD_NONROOT_EUID}` - PROVING it ran NON-ROOT via the production `launch()` command-runner \
         / OCI. Its absence means the corpus did not reach the guest through the production seam.\n\
         --- captured console ---\n{console}"
    );
    assert!(
        !console.contains(ROOT_HARNESS_EUID),
        "[CT-003 routing guard] {backend_label}: the corpus ran as ROOT (`{ROOT_HARNESS_EUID}`) - \
         this is the DRILL-HARNESS signature (Firecracker PID1 / gVisor drill config uid=0), NOT the \
         production seam. A regression has rerouted the corpus to `boot_and_capture` / \
         `drill_config_json` / `stage_bundle` / `gvisor_drill_config_json`. SI-017 has regressed.\n\
         --- captured console ---\n{console}"
    );

    let report = parse_console(console);
    println!("[CT-003 {backend_label}] {}", report.summary());

    for id in GATE_BOUNDARY_FAMILY {
        assert_eq!(
            outcome_of(&report, id),
            &AttackOutcome::Contained,
            "[CT-003 GATE] {backend_label}: boundary-attemptable family `{id}` MUST be CONTAINED \
             through the production `launch()` path (it was genuinely attempted non-root and the \
             hardening posture - no NIC / `--network=none` / read-only root - must contain it). A \
             non-Contained outcome here is a REAL production-path escape ⇒ RED.\n--- console ---\n{console}"
        );
    }

    assert_eq!(
        outcome_of(&report, GATE_DISKFILL),
        &AttackOutcome::Contained,
        "[CT-003a GATE] {backend_label}: `{GATE_DISKFILL}` MUST be CONTAINED through launch() - the \
         production path mounts a scratch tmpfs sized from spec.limits.disk_bytes, so a disk fill hits \
         ENOSPC at the quota (never an unbounded host-RAM-backed tmpfs). A non-Contained outcome is a \
         regression of the SI-017 host-DoS fix ⇒ RED.\n--- console ---\n{console}"
    );

    assert_eq!(
        outcome_of(&report, GATE_MEMHOG),
        &AttackOutcome::Contained,
        "[CT-003b GATE] {backend_label}: `{GATE_MEMHOG}` MUST be CONTAINED through launch() - an \
         oversized anonymous allocation must be OOM-killed within the memory limit (FC: guest-RAM \
         cap; gVisor: the out-of-band cgroup), never holding host RAM beyond mem_bytes. A \
         non-Contained outcome is the SI-017 host-DoS escape ⇒ RED.\n--- console ---\n{console}"
    );

    if f1_marker_authoritative {
        assert_eq!(
            outcome_of(&report, GATE_FORKBOMB),
            &AttackOutcome::Contained,
            "[CT-003a GATE] {backend_label}: `{GATE_FORKBOMB}` MUST be CONTAINED through launch() - the \
             OCI pids.limit is enforced and the marker is authoritative on this backend. A \
             non-Contained outcome ⇒ RED.\n--- console ---\n{console}"
        );
    } else {
        let f1 = outcome_of(&report, GATE_FORKBOMB);
        println!(
            "[CT-003a {backend_label}] F1_forkbomb marker = {f1:?} (non-authoritative: the corpus's \
             cgroup self-check needs root; the NON-ROOT production path enforces the ceiling via \
             `ulimit -u pids_max`). Containment proven structurally: guest survived to CORPUS_END + \
             D2 ran and was CONTAINED (before CT-003a the fork bomb OOM-killed the guest: exit 137, \
             corpus truncated)."
        );
    }

    for id in PRIVILEGE_FAMILY {
        let o = outcome_of(&report, id);
        assert_ne!(
            o, &AttackOutcome::Escaped,
            "[CT-003] {backend_label}: privilege family `{id}` reported ESCAPED - a non-root workload \
             must NEVER breach a kernel primitive.\n--- console ---\n{console}"
        );
        println!(
            "[CT-003 {backend_label}] privilege-contained (NOT a boundary claim): {id} = {o:?}"
        );
    }

    let mut tolerated: Vec<&str> = PRIVILEGE_FAMILY.to_vec();
    if !f1_marker_authoritative {
        tolerated.push(GATE_FORKBOMB);
    }
    for (id, _fam, outcome) in &report.outcomes {
        if *outcome == AttackOutcome::Escaped {
            assert!(
                tolerated.contains(id),
                "[CT-003a anti-vacuity] {backend_label}: `{id}` ESCAPED and it is NOT in the tolerated \
                 set {tolerated:?} - a real production-path escape slipped into a GATED family. RED.\n\
                 --- console ---\n{console}"
            );
            println!(
                "[CT-003a {backend_label}] non-authoritative marker (NOT a boundary breach - see \
                 module docs + the CT-003a report): {id} = Escaped"
            );
        }
    }
    report
}

#[test]
fn firecracker_production_launch_contains_the_corpus_non_root() {
    let preconds = Path::new("/dev/kvm").exists()
        && which_on_path("firecracker", "MYELIN_FC_BIN")
        && resolved_kernel_path().exists()
        && resolved_rootfs_path().exists();
    if !preconds {
        if std::env::var("MYELIN_REQUIRE_KVM").as_deref() == Ok("1") {
            panic!(
                "[CT-003 firecracker] MYELIN_REQUIRE_KVM=1 but the host cannot boot a real microVM \
                 (/dev/kvm absent, `firecracker` not on PATH, or staged assets missing). CT-003 \
                 refuses a VACUOUS green: the corpus MUST really route through a real microVM launch()."
            );
        }
        eprintln!(
            "[CT-003 firecracker] SKIPPED: no /dev/kvm / firecracker / staged assets (CI without KVM passes)."
        );
        return;
    }

    let backend = FirecrackerBackend::new();
    let spec = corpus_spec(
        vec![
            "/bin/bash".into(),
            "-c".into(),
            build_corpus_script(PIDS_MAX),
        ],
        "fc",
    );
    let launch = backend
        .launch(&spec, &ok_hooks())
        .expect("the production Firecracker launch() must boot a real microVM and run the corpus");

    let console = String::from_utf8_lossy(&launch.result.stdout);
    println!(
        "=== CT-003 Firecracker production-path run (exit={:?} timed_out={}) ===",
        launch.result.exit_code, launch.result.timed_out
    );
    println!("--- captured guest stdout (via launch().result.stdout) ---\n{console}");

    let report = assert_production_path_containment("firecracker", &console, false);

    assert!(
        report.corpus_completed,
        "[CT-003a firecracker] the corpus MUST run to its CORPUS_END marker through launch() - the \
         fork bomb (F1) was refused at the pids ceiling (ulimit -u) so the guest SURVIVED. A missing \
         END marker means the fork bomb OOM-killed / truncated the guest (the pre-CT-003a exit-137 \
         failure) ⇒ RED.\n--- console ---\n{console}"
    );
    assert!(
        !launch.result.timed_out,
        "[CT-003a firecracker] the run must COMPLETE (not time out) - a refused fork bomb lets the \
         corpus finish well within the timeout. A timeout here would mean the guest was destabilised."
    );
    assert_ne!(
        launch.result.exit_code,
        Some(137),
        "[CT-003a firecracker] exit 137 (SIGKILL) is the pre-CT-003a OOM-kill signature of an \
         unbounded fork bomb. The ceiling must keep the guest alive."
    );

    backend
        .kill(&launch.handle)
        .expect("teardown whole-guest-kill is idempotent");
}

#[test]
fn gvisor_production_launch_contains_the_corpus_non_root() {
    let preconds = which_on_path("runsc", "MYELIN_RUNSC_BIN") && resolved_gvisor_rootfs().exists();
    if !preconds {
        if std::env::var("MYELIN_REQUIRE_RUNSC").as_deref() == Ok("1") {
            panic!(
                "[CT-003 gvisor] MYELIN_REQUIRE_RUNSC=1 but `runsc` is not on PATH or the staged \
                 minimal rootfs ({}) is absent. CT-003 refuses a VACUOUS green: the corpus MUST really \
                 route through a real runsc launch().",
                resolved_gvisor_rootfs().display()
            );
        }
        eprintln!(
            "[CT-003 gvisor] SKIPPED: `runsc` not on PATH or the staged rootfs is absent (CI without runsc passes)."
        );
        return;
    }

    let backend = GvisorBackend::new(gvisor_test_registry());
    let spec = corpus_spec(
        vec![
            "/bin/sh".into(),
            "-c".into(),
            build_gvisor_corpus_script(PIDS_MAX),
        ],
        "gvisor",
    );
    let launch = backend
        .launch(&spec, &ok_hooks())
        .expect("the production gVisor launch() must run a real runsc container of the corpus");

    let console = String::from_utf8_lossy(&launch.result.stdout);
    println!(
        "=== CT-003 gVisor production-path run (exit={:?} timed_out={}) ===",
        launch.result.exit_code, launch.result.timed_out
    );
    println!("--- captured container stdout (via launch().result.stdout) ---\n{console}");

    let report = assert_production_path_containment("gvisor", &console, true);

    assert!(
        report.corpus_completed,
        "[CT-003 gvisor] the gVisor corpus must run to its END marker through launch()\n--- console ---\n{console}"
    );

    assert!(
        report.is_green(),
        "[CT-003a gvisor] the gVisor PRODUCTION-PATH report must be genuinely green (all families \
         Contained, 0 escapes, corpus completed) now that D2 is bounded - escapes={} did_not_run={} \
         corpus_completed={}.\n{}\n--- console ---\n{console}",
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
    let date = std::env::var("MYELIN_DRILL_DATE").unwrap_or_else(|_| "2026-06-29".to_string());
    let attestation = EscapeAttestation::from_green_drill(
        date,
        &report,
        vec![BackendRun {
            backend: Backend::GvisorRunsc,
            exercised: true,
            residual_note: None,
        }],
        Backend::GvisorRunsc,
        "gvisor-rootfs-busybox(prod-launch)",
        "gvisor-kernel-runsc(prod-launch)",
        kernel_version,
    )
    .expect(
        "CT-003a: the gVisor PRODUCTION-PATH drill is green ⇒ from_green_drill MUST mint the \
         production-path escape attestation (it refuses over a non-green report)",
    );
    assert_eq!(attestation.total_escapes, 0);
    println!(
        "[CT-003a gvisor PRODUCTION-PATH] {}",
        attestation.green_line()
    );
    println!(
        "[CT-003a] green production-path escape attestation minted from the REAL non-root launch() \
         (guest_euid=65534) - SI-017 closed on the gVisor production seam."
    );

    backend
        .kill(&launch.handle)
        .expect("teardown is idempotent");
}
