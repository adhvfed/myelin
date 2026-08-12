#![cfg(feature = "integration")]

use myelin_ci_sandbox::escape_corpus::{parse_console, Backend, BackendRun, EscapeAttestation};
use myelin_ci_sandbox::gvisor::MemoryCgroup;
use myelin_ci_sandbox::{
    build_gvisor_corpus_script, gvisor_drill_config_json, resolved_gvisor_rootfs, EgressPolicy,
    IdemToken, ImageRef, JobKind, JobSpec, MeterTarget, ResourceLimits, RunTokenCredential,
    TrustTier, WorkspaceSpec, GVISOR_CORPUS_SCRIPT,
};
use std::path::{Path, PathBuf};

const DRILL_PIDS_MAX: u32 = 64;

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
    if !resolved_gvisor_rootfs().exists() {
        return None;
    }
    Some(bin)
}

fn drill_spec() -> JobSpec {
    JobSpec::new(
        JobKind::Agent,
        ImageRef::pinned(
            "r/agd4@sha256:abc123def4567890abc123def4567890abc123def4567890abc123def4567890",
        )
        .unwrap(),
        vec!["/bin/sh".into(), format!("/{GVISOR_CORPUS_SCRIPT}")],
        vec![],
        vec![],
        EgressPolicy::deny_all(),
        ResourceLimits {
            cpu_millis: 1000,
            mem_bytes: 256 << 20,
            disk_bytes: 1 << 30,
            tmpfs_bytes: 1 << 30,
            pids_max: DRILL_PIDS_MAX,
            timeout_secs: 120,
        },
        WorkspaceSpec::default(),
        TrustTier::UntrustedFork,
        RunTokenCredential::new("test-bearer", "agd4-gvisor", 300).unwrap(),
        MeterTarget {
            reserve_id: "agd4-gvisor".into(),
        },
        IdemToken("agd4-gvisor-1".into()),
    )
    .unwrap()
}

fn stage_bundle(spec: &JobSpec) -> PathBuf {
    let bundle = std::env::temp_dir().join(format!("myelin-agd4-gvisor-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&bundle);
    std::fs::create_dir_all(&bundle).expect("create bundle dir");

    let rootfs_link = bundle.join("rootfs");
    #[cfg(unix)]
    std::os::unix::fs::symlink(resolved_gvisor_rootfs(), &rootfs_link)
        .expect("symlink rootfs into bundle");

    let script = build_gvisor_corpus_script(DRILL_PIDS_MAX);
    std::fs::write(
        resolved_gvisor_rootfs().join(GVISOR_CORPUS_SCRIPT),
        script.as_bytes(),
    )
    .expect("write corpus script into rootfs");

    let cfg =
        gvisor_drill_config_json(spec, GVISOR_CORPUS_SCRIPT).expect("build hardened OCI config");
    std::fs::write(bundle.join("config.json"), cfg).expect("write config.json");
    bundle
}

fn attestation_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("ag-d4-attestation")
}

#[test]
fn ag_d4_ci_t1_escape_gate_re_runs_green_on_the_gvisor_backend() {
    let Some(bin) = preconditions() else {
        if std::env::var("MYELIN_REQUIRE_RUNSC").as_deref() == Ok("1") {
            panic!(
                "[AG-D4 gVisor] MYELIN_REQUIRE_RUNSC=1 but `runsc` is not on PATH or the staged \
                 minimal rootfs ({}) is absent. The gate is RED until the corpus really runs inside \
                 a real runsc sandbox and attests zero escapes (EI-04 §5.1).",
                resolved_gvisor_rootfs().display()
            );
        }
        eprintln!(
            "[AG-D4 gVisor] SKIPPED: `runsc` not on PATH or the staged minimal rootfs is absent. \
             (CI without runsc passes; the gVisor gate is not claimed green on a host that cannot \
             run it.) Stage a busybox-class rootfs at {} to exercise it.",
            resolved_gvisor_rootfs().display()
        );
        return;
    };

    let spec = drill_spec();
    let bundle = stage_bundle(&spec);
    let container_id = format!("myelin-agd4-{}", std::process::id());

    let cgroup = MemoryCgroup::create(spec.limits.mem_bytes, spec.limits.cpu_millis).expect(
        "[AG-D4 gVisor] the resource cgroup MUST be establishable to contain the anon-memory hog \
         and bound CPU (cgroup v2 + delegated `memory` and `cpu` controllers) - fail-closed \
         otherwise (SI-017)",
    );
    let mut cmd = std::process::Command::new(&bin);
    cmd.arg("--rootless")
        .arg("--network=none")
        .arg("run")
        .arg("-bundle")
        .arg(&bundle)
        .arg(&container_id);
    cgroup
        .place_child(&mut cmd)
        .expect("bind the runsc process tree into the memory cgroup");
    let out = cmd
        .output()
        .expect("run the corpus inside a real runsc sandbox");
    cgroup.cleanup();
    let _ = std::process::Command::new(&bin)
        .arg("--rootless")
        .arg("delete")
        .arg("-force")
        .arg(&container_id)
        .output();
    let _ = std::fs::remove_dir_all(&bundle);
    let _ = std::fs::remove_file(resolved_gvisor_rootfs().join(GVISOR_CORPUS_SCRIPT));

    let mut console = String::from_utf8_lossy(&out.stdout).into_owned();
    console.push_str(&String::from_utf8_lossy(&out.stderr));

    println!("=== AG-D4 escape-drill guest console (REAL gVisor / runsc sandbox) ===");
    for line in console.lines() {
        if line.contains("CONTAINED") || line.contains("ESCAPED") || line.contains("CORPUS_") {
            println!("  {line}");
        }
    }
    println!("=== (runsc exit_code={:?}) ===", out.status.code());

    assert!(
        console.contains("gvisor") || console.contains("CORPUS_BEGIN"),
        "the corpus must have run inside a REAL runsc sandbox.\n--- console ---\n{console}"
    );

    let report = parse_console(&console);
    println!("{}", report.summary());

    assert!(
        report.is_green(),
        "AG-D4 / CI-T1 is RED on the gVisor backend - a DATED NO-GO, NOT a weakened threshold. \
         escapes={} did_not_run={} corpus_completed={}.\n{}\n--- full console ---\n{}",
        report.escapes(),
        report.did_not_run(),
        report.corpus_completed,
        report.summary(),
        console
    );

    let backends = vec![BackendRun {
        backend: Backend::GvisorRunsc,
        exercised: true,
        residual_note: None,
    }];
    let date = std::env::var("MYELIN_DRILL_DATE").unwrap_or_else(|_| "2026-06-24".to_string());
    let attestation = EscapeAttestation::from_green_drill(
        date.clone(),
        &report,
        backends,
        Backend::GvisorRunsc,
        "gvisor-rootfs-busybox",
        "gvisor-kernel-runsc",
        console
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
            .unwrap_or_else(|| "4.19.0-gvisor".to_string()),
    )
    .expect("a green gVisor drill MUST mint a green attestation");

    let dir = attestation_dir();
    std::fs::create_dir_all(&dir).expect("create attestation dir");
    let artifact_path = dir.join(format!("{date}-gvisor.json"));
    std::fs::write(
        &artifact_path,
        attestation.to_json().expect("serialize gVisor attestation"),
    )
    .expect("write gVisor attestation artifact");

    println!("{}", attestation.green_line());
    println!(
        "[AG-D4] dated green escape attestation (gVisor backend RE-GREEN) written: {}",
        artifact_path.display()
    );
    println!(
        "[AG-D4] CI-P28 promotion: gvisor(runsc)=EXERCISED green; contract 8.4 re-greened on the \
         second backend (the permanent gate re-runs per backend)."
    );

    assert_eq!(attestation.total_escapes, 0);
    assert!(
        attestation
            .backends
            .iter()
            .any(|b| b.backend == Backend::GvisorRunsc && b.exercised),
        "gVisor is the EXERCISED backend in this re-green (CI-P28 promotion), not a residual"
    );
}
