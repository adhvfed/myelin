#![cfg(feature = "integration")]

use myelin_ci_sandbox::escape_corpus::{
    build_corpus_script, parse_console, Backend, BackendRun, EscapeAttestation,
};
use myelin_ci_sandbox::firecracker::{
    boot_and_capture, drill_config_json, resolved_kernel_path, resolved_rootfs_path,
};
use std::path::{Path, PathBuf};

const DRILL_PIDS_MAX: u32 = 64;

const CI_P27_ROLE: &str =
    "M4 CI-SIDE COMMITTED-GATE RE-CONFIRM (CI-P27 / P-370): this attestation is the CI control \
     plane's COMMITTED M4-boundary re-confirm of AG-D4 / CI-T1 on the PRODUCTION CI runner image \
     (the config-resolved runner rootfs - dev↔prod is a config swap, MYELIN_REGION=fr-par / \
     prod=Scaleway). Declared as a committed CI job in \
     myelin_ci_controlplane::m4_boundary_permanent_gates (an uncommitted re-run is no gate, EI-01 \
     §5). Production runs on KVM-capable Scaleway Elastic Metal; the prod image is re-drilled there \
     at deploy - a NAMED residual, not faked. gVisor (runsc) re-runs this drill on the second \
     backend at CI-P28.";

fn preconditions() -> bool {
    let has_kvm = Path::new("/dev/kvm").exists();
    let has_fc = which_on_path("firecracker", "MYELIN_FC_BIN");
    let assets_present = resolved_kernel_path().exists() && resolved_rootfs_path().exists();
    has_kvm && has_fc && assets_present
}

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
        "myelin-agd4-cip27-corpus-{}.sh",
        std::process::id()
    ));
    std::fs::write(&path, &bytes).expect("write padded corpus drive");
    path
}

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

fn attestation_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("ag-d4-attestation")
}

#[test]
fn ag_d4_ci_t1_committed_gate_reconfirmed_zero_escapes_on_the_prod_runner_image() {
    if !preconditions() {
        if std::env::var("MYELIN_REQUIRE_KVM").as_deref() == Ok("1") {
            panic!(
                "[AG-D4 CI-P27 committed-gate re-confirm] MYELIN_REQUIRE_KVM=1 but the host cannot \
                 boot a real microVM (/dev/kvm absent, `firecracker` not on PATH, or the staged \
                 guest assets missing). The M4 exit gate refuses a VACUOUS green: this row is RED \
                 until the re-confirm really boots a microVM on the production runner image and \
                 attests ZERO escapes."
            );
        }
        eprintln!(
            "[AG-D4 CI-P27 committed-gate re-confirm] SKIPPED: /dev/kvm or `firecracker` or the \
             staged guest assets are absent - this host cannot boot a microVM. (CI without KVM \
             passes; the M4 hard gate is not claimed green on a host that cannot run it.)"
        );
        return;
    }

    let prod_rootfs = resolved_rootfs_path();
    let prod_kernel = resolved_kernel_path();
    println!("=== AG-D4 / CI-T1 - M4 CI-SIDE COMMITTED-GATE RE-CONFIRM (CI-P27 / P-370) ===");
    println!(
        "  production runner image (config-resolved rootfs): {}",
        prod_rootfs.display()
    );
    println!(
        "  shared hardened kernel:                           {}",
        prod_kernel.display()
    );

    let script = build_corpus_script(DRILL_PIDS_MAX);
    let corpus_drive = stage_padded_corpus(&script);

    let cfg_json = drill_config_json(&corpus_drive, 1, 256);
    let cfg_path =
        std::env::temp_dir().join(format!("myelin-agd4-cip27-cfg-{}.json", std::process::id()));
    std::fs::write(&cfg_path, &cfg_json).expect("write drill machine config");

    let (exit_code, console) =
        boot_and_capture(&cfg_path).expect("boot the CI-P27 escape-drill microVM");
    let _ = std::fs::remove_file(&cfg_path);
    let _ = std::fs::remove_file(&corpus_drive);

    println!("=== CI-P27 escape-drill guest serial console (REAL Firecracker microVM) ===");
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

    assert!(
        console.contains("Linux version"),
        "the CI-P27 re-confirm must boot a REAL guest kernel.\n--- console ---\n{console}"
    );

    let report = parse_console(&console);
    println!("{}", report.summary());

    assert!(
        report.is_green(),
        "AG-D4 / CI-T1 CI-P27 committed-gate re-confirm on the PRODUCTION runner image is RED - a \
         DATED NO-GO that blocks M5, NOT a weakened threshold. escapes={} did_not_run={} \
         corpus_completed={}.\n{}\n--- full console ---\n{}",
        report.escapes(),
        report.did_not_run(),
        report.corpus_completed,
        report.summary(),
        console
    );

    let mut backends = vec![BackendRun {
        backend: Backend::FirecrackerMicrovm,
        exercised: true,
        residual_note: None,
    }];
    let gvisor_note = if runsc_present() {
        "runsc is on PATH but running the corpus under it needs an OCI bundle + root/userns \
         privileges this host lacks; recorded as the CI-P28 run-when-available residual, NOT faked."
    } else {
        "runsc not on PATH - the CI-P28 second-backend residual."
    };
    backends.push(BackendRun {
        backend: Backend::GvisorRunsc,
        exercised: false,
        residual_note: Some(gvisor_note.to_string()),
    });

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
    .expect("a green CI-P27 drill MUST mint a green attestation");

    attestation.residuals.insert(0, CI_P27_ROLE.to_string());

    let dir = attestation_dir();
    std::fs::create_dir_all(&dir).expect("create attestation dir");
    let artifact_path = dir.join(format!("ci-p27-reconfirm-{date}.json"));
    std::fs::write(
        &artifact_path,
        attestation.to_json().expect("serialize CI-P27 attestation"),
    )
    .expect("write CI-P27 attestation");

    println!("{}", attestation.green_line());
    println!(
        "[AG-D4 CI-P27 committed-gate re-confirm] dated GREEN attestation written: {}",
        artifact_path.display()
    );
    for r in &attestation.residuals {
        println!("[AG-D4 CI-P27 residual] {r}");
    }

    assert_eq!(attestation.total_escapes, 0);
    assert_eq!(attestation.gate_backend, Backend::FirecrackerMicrovm);
    assert!(
        !rootfs_sha.is_empty(),
        "the CI-P27 attestation carries the prod runner rootfs sha256"
    );
    assert!(
        attestation
            .residuals
            .iter()
            .any(|r| r.contains("CI-SIDE COMMITTED-GATE RE-CONFIRM (CI-P27")
                && r.contains("Scaleway Elastic Metal")),
        "the CI-P27 committed-gate role + the Scaleway-metal residual are named IN WRITING"
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
            .any(|b| b.backend == Backend::GvisorRunsc && !b.exercised),
        "gVisor is recorded as the NAMED residual (CI-P28), not faked green"
    );
}
