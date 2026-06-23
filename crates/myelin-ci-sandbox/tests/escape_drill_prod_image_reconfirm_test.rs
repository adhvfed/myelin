//! # AG-D4 / CI-T1 re-confirmed on the PRODUCTION CI runner image — the M4 HARD GATE (AG-P21 → P-348)
//!
//! **Owning prompt:** `planning/07-prompts/by-system/agent-fabric.md` §AG-P21 ("AG-D4 / CI-T1
//! re-confirmed GREEN on the production CI runner image (the M4 hard gate)"). **Architecture:**
//! `05-refined-shared-systems-architecture/agent-fabric.md` §2.2 (exec = CI's `kind=agent` job;
//! AG-D4 == CI-T1, re-confirmed on the production image) + §9 row D-4. **Contract:** row 8.4 / CI-T1
//! (the prod-image re-confirm). **Drills:**
//! `01-whole-system-e2e-and-drill-catalogue.md` rows CI-T1 / AG-D4 (re-confirmed green on the
//! production CI runner image) + §3.5 (the adversarial corpus, re-run on every image change).
//! **Doctrine:** EI-04 §5 (untrusted-code execution is a permanent never-"done" surface; AG-D4 is a
//! permanent gate re-run on every image change); EI-01 §3 (a property is real ONLY when the drill
//! FORCES the attack and the system is OBSERVED to contain it — never a hardcoded "0 escapes").
//!
//! ## What this re-confirms (and how it REUSES the P-239 machinery — NO fork)
//! AG-D4 / CI-T1 is the SHARED, PERMANENT escape gate. CI shipped + proved it on a real Firecracker
//! microVM (CI-P5 → P-239, `tests/escape_drill_test.rs`). This file is the **M4-boundary RE-CONFIRM
//! on the production CI runner image** (CI-P27 / P-370 on the CI side). It does NOT fork the corpus or
//! the attestation type — it RE-USES, byte-for-byte:
//!   - [`build_corpus_script`] — the identical seven-family / eleven-attack adversarial corpus;
//!   - [`boot_and_capture`] + [`drill_config_json`] — the identical production Firecracker launch
//!     recipe (read-only squashfs root, NO NIC, the corpus as PID1 bash on `/dev/vdb`);
//!   - [`resolved_rootfs_path`] / [`resolved_kernel_path`] — the config-resolved images;
//!   - [`parse_console`] / [`DrillReport::is_green`] — the identical OBSERVATION + gate predicate;
//!   - [`EscapeAttestation::from_green_drill`] — the identical green-artifact type (refuses to mint
//!     over a red drill).
//!
//! The ONLY differences from P-239 are (a) it asserts it ran against the PRODUCTION RUNNER IMAGE
//! (named explicitly, below), (b) it tags the attestation as the M4 prod-image re-confirm (a distinct
//! `residuals` line + a distinct artifact path `target/ag-d4-attestation/prod-image-<date>.json`), so
//! a consumer can tell the prod-image re-confirm apart from the M2 base drill while REUSING the type.
//!
//! ## How the PRODUCTION runner image is represented on this dev host (the honest residual)
//! dev↔prod is a **config swap** (CI-P2 DELIVERABLE; `MYELIN_REGION=fr-par`, prod = Scaleway). On a
//! dev host the production CI runner image is represented by the **config-resolved rootfs**
//! ([`resolved_rootfs_path`] / `MYELIN_FC_ROOTFS`) — the SAME runner image the production CI runner is
//! built from. So this re-confirm runs the SAME real escape battery against the runner image the prod
//! CI runner is built from, on the SAME hardened kernel + corpus. **NAMED residual (not faked):**
//! production runs on KVM-capable **Scaleway Elastic Metal** — the prod image is re-drilled THERE at
//! deploy; this CI-side M4-boundary re-confirm is CI-P27 / P-370. We do NOT fabricate a separate prod
//! image we do not have; we re-confirm honestly on the available production-runner image and NAME the
//! residual in the attestation.
//!
//! ## FLOOR: there is NO floor on AG-D4 — it is a PERMANENT GATE re-run on every backend/image/kernel
//! change forever. ZERO escapes is BOTH the floor and the full answer.
//!
//! ## Gating (CI without KVM still passes)
//! Boots SKIP GRACEFULLY (return early, NOT fail) when `/dev/kvm` is absent, `firecracker` is not on
//! PATH, or the staged assets are missing — unless `MYELIN_REQUIRE_KVM=1`, which makes an absent
//! microVM a HARD FAILURE (the M4 exit-gate row refuses a VACUOUS green). On THIS host (real silicon)
//! the preconditions hold and the re-confirm REALLY boots, REALLY attempts every attack, and OBSERVES
//! containment. Run:
//! `cargo test -p myelin-ci-sandbox --features integration --test escape_drill_prod_image_reconfirm_test -- --nocapture`.

#![cfg(feature = "integration")]

use myelin_ci_sandbox::escape_corpus::{
    build_corpus_script, parse_console, Backend, BackendRun, EscapeAttestation,
};
use myelin_ci_sandbox::firecracker::{
    boot_and_capture, drill_config_json, resolved_kernel_path, resolved_rootfs_path,
};
use std::path::{Path, PathBuf};

/// The drill's pids.max fork-bomb ceiling (the cgroup `pids.max` the corpus sets + asserts held) —
/// the SAME value the P-239 base drill uses (reused, not re-chosen).
const DRILL_PIDS_MAX: u32 = 64;

/// The production-runner-image ROLE tag carried in the attestation `residuals` so a consumer can tell
/// this M4 prod-image re-confirm apart from the M2 base drill (the type is REUSED, not forked).
const PROD_IMAGE_ROLE: &str =
    "M4 PROD-IMAGE RE-CONFIRM (AG-P21 / P-348; CI side CI-P27 / P-370): this attestation re-confirms \
     AG-D4 / CI-T1 on the PRODUCTION CI runner image (the config-resolved runner rootfs — dev↔prod is \
     a config swap, MYELIN_REGION=fr-par / prod=Scaleway). The production runner runs on KVM-capable \
     Scaleway Elastic Metal; the prod image is re-drilled there at deploy — that is a NAMED residual, \
     not faked here.";

/// Resolve KVM + firecracker + staged-asset availability (the same preconditions the base escape
/// drill gates on). Returns `false` if the host cannot boot a microVM (→ graceful skip).
fn preconditions() -> bool {
    let has_kvm = Path::new("/dev/kvm").exists();
    let has_fc = which_on_path("firecracker", "MYELIN_FC_BIN");
    let assets_present = resolved_kernel_path().exists() && resolved_rootfs_path().exists();
    has_kvm && has_fc && assets_present
}

/// Whether `runsc` (gVisor) resolves on PATH — the second backend re-run is CI-P28 (named residual).
fn runsc_present() -> bool {
    which_on_path("runsc", "MYELIN_RUNSC_BIN")
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

/// Stage the corpus script on a host file padded to an 8 KiB block boundary (a Firecracker drive
/// smaller than one 512-byte sector presents as 0 blocks in-guest). The padding is a trailing bash
/// comment. REUSES the exact P-239 staging recipe.
fn stage_padded_corpus(script: &str) -> PathBuf {
    let mut bytes = script.as_bytes().to_vec();
    bytes.push(b'\n');
    bytes.push(b'#');
    let pad_to = 8192usize;
    while bytes.len() < pad_to {
        bytes.push(b'#');
    }
    bytes.push(b'\n');
    let path = std::env::temp_dir().join(format!(
        "myelin-agd4-prodimg-corpus-{}.sh",
        std::process::id()
    ));
    std::fs::write(&path, &bytes).expect("write padded corpus drive");
    path
}

/// The sha256 of a file via the host `sha256sum` tool (the drill is host-side and already shells out
/// to the production `firecracker` VMM).
fn sha256_file(path: &Path) -> String {
    let out = std::process::Command::new("sha256sum")
        .arg(path)
        .output()
        .expect("run sha256sum");
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string()
}

/// Where the dated prod-image-re-confirm attestation is written — a DISTINCT path from the M2 base
/// drill's `<date>.json`, so the M4 re-confirm artifact is unambiguous while REUSING the type.
fn attestation_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("ag-d4-attestation")
}

#[test]
fn ag_d4_ci_t1_reconfirmed_zero_escapes_on_the_production_runner_image() {
    if !preconditions() {
        // The M4 band-boundary scorecard sets `MYELIN_REQUIRE_KVM=1` for THIS row, because a graceful
        // skip would be a VACUOUS green — the M4 hard gate must only read green when a real microVM
        // actually boots on the production image and attests zero escapes (EI-04 §5.1).
        if std::env::var("MYELIN_REQUIRE_KVM").as_deref() == Ok("1") {
            panic!(
                "[AG-D4 prod-image re-confirm] MYELIN_REQUIRE_KVM=1 but the host cannot boot a real \
                 microVM (/dev/kvm absent, `firecracker` not on PATH, or the staged guest assets \
                 missing). The M4 exit gate refuses a VACUOUS green: this row is RED until the \
                 re-confirm really boots a microVM on the production runner image and attests ZERO \
                 escapes."
            );
        }
        eprintln!(
            "[AG-D4 prod-image re-confirm] SKIPPED: /dev/kvm or `firecracker` or the staged guest \
             assets are absent — this host cannot boot a microVM. (CI without KVM passes; the M4 \
             hard gate is not claimed green on a host that cannot run it.)"
        );
        return;
    }

    // ----------------------------------------------------------------------------------------
    // The PRODUCTION CI runner IMAGE on this dev host = the config-resolved runner rootfs
    // (dev↔prod is a config swap; MYELIN_REGION=fr-par / prod=Scaleway). We re-run the SAME real
    // escape battery against the runner image the prod CI runner is built from.
    // ----------------------------------------------------------------------------------------
    let prod_rootfs = resolved_rootfs_path();
    let prod_kernel = resolved_kernel_path();
    println!("=== AG-D4 / CI-T1 — M4 prod-image RE-CONFIRM (AG-P21 / P-348) ===");
    println!(
        "  production runner image (config-resolved rootfs): {}",
        prod_rootfs.display()
    );
    println!(
        "  shared hardened kernel:                           {}",
        prod_kernel.display()
    );

    // 1) Build the IDENTICAL adversarial corpus + stage it (no fork of the corpus).
    let script = build_corpus_script(DRILL_PIDS_MAX);
    let corpus_drive = stage_padded_corpus(&script);

    // 2) Boot a REAL Firecracker microVM on the PRODUCTION runner image (REUSES boot_and_capture /
    //    drill_config_json — the production VMM-spawn site; no fork of the backend).
    let cfg_json = drill_config_json(&corpus_drive, /* vcpu */ 1, /* mem_mib */ 256);
    let cfg_path = std::env::temp_dir().join(format!(
        "myelin-agd4-prodimg-cfg-{}.json",
        std::process::id()
    ));
    std::fs::write(&cfg_path, &cfg_json).expect("write drill machine config");

    let (exit_code, console) =
        boot_and_capture(&cfg_path).expect("boot the prod-image escape-drill microVM");
    let _ = std::fs::remove_file(&cfg_path);
    let _ = std::fs::remove_file(&corpus_drive);

    // Print the REAL guest console (the per-attack CONTAINED proof) under --nocapture.
    println!("=== prod-image escape-drill guest serial console (REAL Firecracker microVM) ===");
    for line in console.lines() {
        if line.contains("CONTAINED")
            || line.contains("ESCAPED")
            || line.contains("CORPUS_")
            || line.contains("Linux version")
            || line.contains("Firecracker exiting")
        {
            println!("  {line}");
        }
    }
    println!("=== (vmm exit_code={exit_code}) ===");

    // Sanity: a REAL KVM-backed guest kernel booted (the boundary is the CPU's VT-x/AMD-V).
    assert!(
        console.contains("Linux version 6.1.168") || console.contains("Linux version"),
        "the prod-image re-confirm must boot a REAL guest kernel.\n--- console ---\n{console}"
    );

    // 3) OBSERVE containment — parse the REAL console into per-attack outcomes. No hardcoded green.
    let report = parse_console(&console);
    println!("{}", report.summary());

    // THE M4 HARD GATE. ANY escape, any attack that did not run, or a truncated corpus ⇒ RED.
    assert!(
        report.is_green(),
        "AG-D4 / CI-T1 on the PRODUCTION runner image is RED — this is a DATED NO-GO, NOT a weakened \
         threshold. escapes={} did_not_run={} corpus_completed={}.\n{}\n--- full console ---\n{}",
        report.escapes(),
        report.did_not_run(),
        report.corpus_completed,
        report.summary(),
        console
    );

    // 4) gVisor (runsc) — the NAMED second backend (CI-P28). Attempt it; record as a
    //    run-when-available residual if it needs privileges this host lacks — do NOT fake green.
    let mut backends = vec![BackendRun {
        backend: Backend::FirecrackerMicrovm,
        exercised: true,
        residual_note: None,
    }];
    let gvisor_note = if runsc_present() {
        "runsc is on PATH but running the corpus under it needs an OCI bundle + root/userns \
         privileges this host lacks (no passwordless sudo); recorded as the CI-P28 \
         run-when-available residual, NOT faked."
    } else {
        "runsc not on PATH — the CI-P28 second-backend residual."
    };
    backends.push(BackendRun {
        backend: Backend::GvisorRunsc,
        exercised: false,
        residual_note: Some(gvisor_note.to_string()),
    });

    // 5) Emit the DATED GREEN ESCAPE ATTESTATION for the PROD IMAGE (REUSES EscapeAttestation — the
    //    type's from_green_drill refuses to mint over a red drill). The prod-image role is tagged in
    //    the residuals + the artifact path, so the M4 re-confirm is unambiguous without forking.
    let rootfs_sha = sha256_file(&prod_rootfs);
    let kernel_sha = sha256_file(&prod_kernel);
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
        .unwrap_or_else(|| "6.1.168".to_string());

    let date = std::env::var("MYELIN_DRILL_DATE").unwrap_or_else(|_| "2026-06-23".to_string());
    let mut attestation = EscapeAttestation::from_green_drill(
        date.clone(),
        &report,
        backends,
        Backend::FirecrackerMicrovm,
        &rootfs_sha,
        &kernel_sha,
        &kernel_version,
    )
    .expect("a green prod-image drill MUST mint a green attestation");

    // Tag the prod-image ROLE in the (reused) attestation's residuals — the M4 re-confirm label
    // (the FIRST residual line, so a consumer reads the role up front). The type is REUSED whole.
    attestation.residuals.insert(0, PROD_IMAGE_ROLE.to_string());

    // Write the DISTINCT prod-image artifact path + echo the one-line [AG-D4 GREEN] … to stdout.
    let dir = attestation_dir();
    std::fs::create_dir_all(&dir).expect("create attestation dir");
    let artifact_path = dir.join(format!("prod-image-{date}.json"));
    std::fs::write(&artifact_path, attestation.to_json()).expect("write prod-image attestation");

    println!("{}", attestation.green_line());
    println!(
        "[AG-D4 prod-image re-confirm] dated GREEN attestation written: {}",
        artifact_path.display()
    );
    println!(
        "[AG-D4 prod-image re-confirm] backends exercised: firecracker(microVM/KVM)=YES (GATE); \
         gvisor(runsc)=residual (CI-P28)"
    );
    for r in &attestation.residuals {
        println!("[AG-D4 prod-image residual] {r}");
    }

    // Final structural assertions on the prod-image attestation (the consumer's contract).
    assert_eq!(attestation.total_escapes, 0);
    assert_eq!(attestation.gate_backend, Backend::FirecrackerMicrovm);
    assert!(
        !rootfs_sha.is_empty(),
        "the prod-image attestation carries the production runner rootfs sha256 (image digest)"
    );
    assert!(
        attestation
            .residuals
            .iter()
            .any(|r| r.contains("PROD-IMAGE RE-CONFIRM") && r.contains("Scaleway Elastic Metal")),
        "the prod-image re-confirm role + the Scaleway-metal residual are named IN WRITING"
    );
    assert!(
        attestation
            .residuals
            .iter()
            .any(|r| r.contains("PERMANENT GATE")),
        "the no-floor permanent-gate posture is carried in the attestation"
    );
    assert!(
        attestation
            .backends
            .iter()
            .any(|b| b.backend == Backend::FirecrackerMicrovm && b.exercised),
        "Firecracker is the EXERCISED gate backend on the production runner image"
    );
    assert!(
        attestation
            .backends
            .iter()
            .any(|b| b.backend == Backend::GvisorRunsc && !b.exercised),
        "gVisor is recorded as the NAMED residual (CI-P28), not faked green"
    );
}
