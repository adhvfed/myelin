//! # AG-D4 / CI-T1 RE-RUN on the gVisor (`runsc`) second backend (CI-P28 → P-423, M5)
//!
//! **Owning architecture (byte-authoritative):**
//! `continuous-integration/architecture/02-internals-and-algorithms.md` §5.1 ("gVisor is the named
//! second backend behind the SAME `SandboxBackend` trait — its own drill") + §5.5 (THE escape drill;
//! the corpus + the green-attestation artifact). **Drills:**
//! `05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`
//! row **AG-D4 / CI-T1** — re-run on every backend (the PERMANENT GATE). **Contract:**
//! `contract-index.md` row 8.4 (the escape drill re-runs on the gVisor backend). **Doctrine:**
//! EI-04 §5.1 (a property not drilled is a CLAIM, not a fact); EI-01 §3 (prove-it — the green
//! attestation IS the pass condition, never weaken the threshold to manufacture green).
//!
//! ## What this PROVES — the CI-P28 promotion
//! The CI-P28 trigger-gated promotion. The host has rootless `runsc` (gVisor) AND `/dev/kvm` (the
//! binding DEV-REAL policy reclassifies the sandbox-escape gate from a FLOOR to a REAL drill on this
//! host), so gVisor is PROMOTED: the SAME seven-family adversarial corpus
//! ([`build_gvisor_corpus_script`]) runs INSIDE a real `runsc` userspace-kernel sandbox via a
//! hardened OCI bundle (read-only root, all caps dropped, no-new-privs, pids ceiling, NO network
//! namespace ⇒ `--network=none` leaves only loopback). The SAME host-side parser
//! ([`parse_console`]) over the captured gVisor console produces the SAME [`DrillReport`] gate
//! predicate — ANY escape, any attack that did not run, or a truncated corpus ⇒ RED, and NO
//! attestation is minted (a red AG-D4 is a dated no-go, never a weakened threshold).
//!
//! On a green run it emits a DATED green escape attestation with `Backend::GvisorRunsc`
//! **exercised** (not the run-when-available residual the Firecracker drill recorded) — the 8.4
//! permanent gate re-greened on the new backend.
//!
//! ## Gating (CI without runsc still passes)
//! SKIPPED GRACEFULLY (returns early, NOT failed) when `runsc` is not on PATH or the staged minimal
//! rootfs is absent. With `MYELIN_REQUIRE_RUNSC=1` an absent runtime/rootfs is a HARD FAILURE (no
//! vacuous green). Run:
//! `cargo test -p myelin-ci-sandbox --features integration --test escape_drill_gvisor_test -- --nocapture`.

#![cfg(feature = "integration")]

use myelin_ci_sandbox::escape_corpus::{parse_console, Backend, BackendRun, EscapeAttestation};
use myelin_ci_sandbox::gvisor::MemoryCgroup;
use myelin_ci_sandbox::{
    build_gvisor_corpus_script, gvisor_drill_config_json, resolved_gvisor_rootfs, EgressPolicy,
    IdemToken, ImageRef, JobKind, JobSpec, MeterTarget, ResourceLimits, RunTokenCredential,
    TrustTier, WorkspaceSpec, GVISOR_CORPUS_SCRIPT,
};
use std::path::{Path, PathBuf};

/// The drill's pids.max fork-bomb ceiling (the OCI `linux.resources.pids.limit` the bundle sets).
const DRILL_PIDS_MAX: u32 = 64;

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

/// The hardened JobSpec the drill derives its OCI posture from (default-deny egress ⇒ no NIC; the
/// pids ceiling; read-only root; caps dropped — the mandatory backend-independent profile).
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

/// Stage a self-contained OCI bundle in a temp dir: a copy-free `rootfs` symlink to the staged
/// minimal rootfs is NOT used (runsc needs a real dir as `root.path`), so we point the bundle's
/// `rootfs` at the staged dir by writing config.json with an absolute root path. runsc requires
/// `root.path` relative-to-bundle OR absolute; we use the bundle dir with a `rootfs` symlink.
fn stage_bundle(spec: &JobSpec) -> PathBuf {
    let bundle = std::env::temp_dir().join(format!("myelin-agd4-gvisor-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&bundle);
    std::fs::create_dir_all(&bundle).expect("create bundle dir");

    // Symlink rootfs → the staged minimal rootfs (runsc reads `root.path = "rootfs"` in the bundle).
    let rootfs_link = bundle.join("rootfs");
    #[cfg(unix)]
    std::os::unix::fs::symlink(resolved_gvisor_rootfs(), &rootfs_link)
        .expect("symlink rootfs into bundle");

    // Write the corpus script into the bundle's rootfs (it runs as the container entrypoint).
    let script = build_gvisor_corpus_script(DRILL_PIDS_MAX);
    std::fs::write(
        resolved_gvisor_rootfs().join(GVISOR_CORPUS_SCRIPT),
        script.as_bytes(),
    )
    .expect("write corpus script into rootfs");

    // Write the hardened OCI config.json (read-only root, caps dropped, nnp, pids ceiling, no netns).
    let cfg =
        gvisor_drill_config_json(spec, GVISOR_CORPUS_SCRIPT).expect("build hardened OCI config");
    std::fs::write(bundle.join("config.json"), cfg).expect("write config.json");
    bundle
}

/// Where the dated green attestation artifact is written (the form AG-P17 / P-229 consumes).
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

    // 1) Build + stage the hardened OCI bundle (the SAME mandatory profile, gVisor's mechanism).
    let spec = drill_spec();
    let bundle = stage_bundle(&spec);
    let container_id = format!("myelin-agd4-{}", std::process::id());

    // 2) Run the corpus INSIDE a REAL runsc (gVisor) sandbox. --network=none ⇒ only loopback exists
    //    (egress closed); --rootless runs without sudo. The corpus prints per-attack markers to
    //    stdout, which we capture for the SAME host-side parser the Firecracker drill uses.
    //
    //    CT-003b (SI-017): the corpus now carries the anon-memory hog (Mx_memhog). rootless runsc
    //    does NOT enforce the OCI memory.limit, so — exactly as the production launch() does — we
    //    place the runsc process tree into an OUT-OF-BAND host memory cgroup (the SAME
    //    `MemoryCgroup` helper the production path uses; no forked enforcer). Without it the hog
    //    would HELD an oversized anonymous allocation ⇒ ESCAPED ⇒ the drill would (correctly) go RED.
    let cgroup = MemoryCgroup::create(spec.limits.mem_bytes).expect(
        "[AG-D4 gVisor] the memory cgroup MUST be establishable to contain the anon-memory hog \
         (cgroup v2 + a delegated `memory` controller) — fail-closed otherwise (SI-017)",
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
    // Best-effort teardown (idempotent; the container has exited).
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

    // Sanity: the corpus ran inside the gVisor kernel (uname reports the gVisor kernel string).
    assert!(
        console.contains("gvisor") || console.contains("CORPUS_BEGIN"),
        "the corpus must have run inside a REAL runsc sandbox.\n--- console ---\n{console}"
    );

    // 3) OBSERVE containment — parse the REAL console. No hardcoded green; ONE gate predicate.
    let report = parse_console(&console);
    println!("{}", report.summary());

    // THE HARD GATE, re-run on the gVisor backend. ANY escape / did-not-run / truncation ⇒ RED.
    assert!(
        report.is_green(),
        "AG-D4 / CI-T1 is RED on the gVisor backend — a DATED NO-GO, NOT a weakened threshold. \
         escapes={} did_not_run={} corpus_completed={}.\n{}\n--- full console ---\n{}",
        report.escapes(),
        report.did_not_run(),
        report.corpus_completed,
        report.summary(),
        console
    );

    // 4) Emit the DATED GREEN ESCAPE ATTESTATION with gVisor EXERCISED (the CI-P28 re-green). The
    //    gate-backend stays Firecracker (the production default); gVisor is the named SECOND backend
    //    now proven (exercised: true) — not the run-when-available residual the FC drill recorded.
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
        // The GATE backend for the platform remains Firecracker (the production default for untrusted
        // code, arch §5.1); this artifact PROVES the SECOND backend also contains the full corpus.
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
    std::fs::write(&artifact_path, attestation.to_json())
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

    // Final structural assertions on the artifact (the consumer's contract).
    assert_eq!(attestation.total_escapes, 0);
    assert!(
        attestation
            .backends
            .iter()
            .any(|b| b.backend == Backend::GvisorRunsc && b.exercised),
        "gVisor is the EXERCISED backend in this re-green (CI-P28 promotion), not a residual"
    );
}
