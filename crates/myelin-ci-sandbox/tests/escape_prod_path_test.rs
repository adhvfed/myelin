//! # CT-003 — Production-path escape verification (P-545 / SI-017), BOTH backends
//!
//! **Owning architecture (byte-authoritative):**
//! `continuous-integration/architecture/02-internals-and-algorithms.md` §5.1 (the two backends behind
//! the SAME `SandboxBackend` trait) + §5.2 (the four-guarantee `RunnerHooks` seam) + §5.3 (the
//! mandatory hardening profile) + §5.5 (the AG-D4 escape corpus + the host-side parser). **Drills:**
//! `05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`
//! row **AG-D4 / CI-T1**. **Contract:** `contract-index.md` row 8.4 (the unified sandbox — `exec` IS
//! `launch(JobSpec, hooks)`). **Doctrine:** EI-04 §5.1 (a property not drilled on the REAL path is a
//! CLAIM, not a fact); EI-01 §2 (sandbox-escape outranks every feature), §3 (prove-it).
//!
//! ## The defect this closes (SI-017)
//! The AG-D4 escape corpus has, until now, been run ONLY through SEPARATE harnesses: the Firecracker
//! drill (`boot_and_capture` + `drill_config_json`, `tests/escape_drill_test.rs`) and the gVisor drill
//! (`gvisor_drill_config_json` + a direct `runsc run`, `tests/escape_drill_gvisor_test.rs`). Neither
//! routes through the PRODUCTION `SandboxBackend::launch` seam that REAL jobs (and agent `exec`) take.
//! So "0 escapes" certified a path real jobs DO NOT take. CT-003 routes the SAME corpus through the
//! REAL `launch()` on BOTH backends and proves containment on the path that matters.
//!
//! ## The HONEST non-root scope split (this is load-bearing — read it before trusting the gate)
//! Production `launch()` runs the untrusted workload **NON-ROOT** by deliberate security design
//! (Firecracker: `setpriv --reuid 65534`; gVisor: OCI `process.user` 65534), under the mandatory
//! hardening profile. That posture is the WHOLE POINT — but it splits the corpus into families a
//! non-root workload can vs cannot meaningfully attempt:
//!
//!   * **BOUNDARY-CONTAINED, non-root-attemptable (THE CT-003 GATE).** The egress/SSRF/exfil family
//!     (`M1_metadata_ssrf`, `C1_controlplane`, `T1_crosstenant`, `E1_secret_exfil`) and the
//!     read-only-root probe (`D1_root_readonly`). A non-root workload GENUINELY runs these (the
//!     `/dev/tcp` / `wget` / `nc` connect is actually attempted; the write to `/` is actually
//!     attempted) and the production posture CONTAINS them: no NIC at all (Firecracker, egress
//!     default-deny) / no network namespace + `--network=none` (gVisor) ⇒ no route; read-only root ⇒
//!     the write is denied. These MUST be `Contained` (never `Escaped`, never `DidNotRun`) on BOTH
//!     backends. This is the SI-017-closing supply-chain proof: an untrusted dependency cannot phone
//!     home, hit cloud metadata, reach the control plane, exfil a secret, or tamper with root —
//!     through the REAL path jobs take.
//!
//!   * **PRIVILEGE-CONTAINED, NOT a boundary win (annotated, not a microVM/gVisor-boundary claim).**
//!     The kernel-primitive family (`K1_module`, `K2_devmem`, `K3_ioport`, `K4_mount`). A NON-ROOT
//!     workload cannot even MOUNT these: on Firecracker the corpus's own `setpriv --bounding-set -all`
//!     wrapper needs `CAP_SETPCAP` and EPERMs (the attacks never run ⇒ `DidNotRun`); on gVisor the
//!     privileged device nodes are absent and `mknod` is denied (⇒ `Contained`). EITHER WAY this is
//!     containment at the PRIVILEGE/namespace layer, NOT proof of the microVM/gVisor BOUNDARY against
//!     a ROOT attacker. The root-adversary boundary proof for K1–K4 remains the existing harness
//!     drills (which run the corpus as ROOT / PID1). CT-003 only asserts K1–K4 never `Escaped`.
//!
//!   * **RESOURCE families — NOW GATED on the production path (CT-003a closed the CT-003 residual).**
//!     CT-003 originally only TOLERATED `F1_forkbomb` / `D2_diskfill` non-`Contained` markers, because
//!     the production backend configs declared `spec.limits` (`mem_bytes`/`disk_bytes`/`pids_max`) but
//!     did NOT ENFORCE them in-guest: the gVisor production OCI config bounded only `pids` (no memory
//!     limit, no size-bounded scratch — `/tmp` was an unbounded host-RAM-backed tmpfs ⇒ **D2 a REAL
//!     host-DoS escape**), and the Firecracker production path applied no in-guest `pids` ceiling (a
//!     fork bomb grew until the guest OOM-died at exit 137, truncating the run). **CT-003a enforces the
//!     limits on the production exec paths:** gVisor emits `linux.resources.memory.limit` (= mem_bytes)
//!     and mounts a `/tmp` tmpfs sized from `disk_bytes`; Firecracker mounts a `/run/scratch` tmpfs
//!     sized from `disk_bytes` and applies `ulimit -u pids_max` (RLIMIT_NPROC) before dropping to the
//!     non-root uid. So now: **`D2_diskfill` MUST be `Contained` on BOTH backends** (the disk fill
//!     hits ENOSPC at the quota); **`F1_forkbomb` MUST be `Contained` on gVisor** (the sentry-enforced
//!     OCI pids.limit, authoritative marker); and on **Firecracker** the fork bomb is refused at the
//!     `ulimit -u` ceiling so the guest SURVIVES — proven STRUCTURALLY (corpus runs to `CORPUS_END`,
//!     not exit 137, AND `D2` runs + is `Contained` afterward), because F1's own self-check reads a
//!     cgroup it can only create as root (its marker is non-authoritative non-root). The ONLY tolerated
//!     non-`Contained` markers are now {K1–K4} (privilege family) plus `F1_forkbomb` on Firecracker
//!     ONLY (the documented root-only self-check); ANY escape elsewhere — including a D2 escape, or an
//!     F1 escape on gVisor — fails the test RED.
//!
//!   * **ANON-MEMORY hog — GATED on the production path (CT-003b closes a REAL host-DoS escape the
//!     CT-003 verifier found on the gVisor path).** CT-003a's gVisor OCI `memory.limit` claim was
//!     FALSE: `runsc --rootless` does NOT enforce it (rootless runsc cannot manage a host cgroup), so
//!     an untrusted gVisor job's ANONYMOUS memory was UNBOUNDED — it could drive the HOST to OOM. The
//!     corpus had no anon-memory probe, so the gate passed vacuously. **CT-003b** adds the `Mx_memhog`
//!     attack (an oversized anonymous allocation) to BOTH corpora and enforces the gVisor workload's
//!     memory with an OUT-OF-BAND host `MemoryCgroup` the production `launch()` places the `runsc`
//!     process tree into (FAIL-CLOSED: launch() refuses if the cgroup can't be established).
//!     **`Mx_memhog` MUST be `Contained` on BOTH backends** (FC: hard guest-RAM cap; gVisor: the
//!     cgroup). Containment is proven STRUCTURALLY — the prober is OOM-killed mid-alloc (on gVisor the
//!     whole sentry), so the corpus prints the `ATTEMPT` sentinel + `CORPUS_END` BEFORE the alloc and
//!     an `ESCAPED` line ONLY if the oversized alloc HELD. A regression dropping the enforcer ⇒ HELD
//!     ⇒ Escaped ⇒ RED. `Mx_memhog` is NOT in the tolerated set on either backend.
//!
//! ## The routing guard (SI-017's structural core — trips RED on a quiet reroute to the harness)
//! The corpus reaches the guest ONLY as `spec.command` through `launch()`. The structural proof that
//! it ran via the PRODUCTION seam (and not the drill-harness shortcut) is the BEGIN marker's
//! `guest_euid`: the production command-runner / OCI runs the corpus **NON-ROOT (`guest_euid=65534`)**,
//! whereas BOTH drill harnesses run it as **ROOT (`guest_euid=0`)** (Firecracker `init=/bin/bash` is
//! PID1/root; the gVisor drill config sets `process.user.uid=0`). So a captured `guest_euid=65534`
//! PROVES production-non-root routing; a future regression that reroutes to `boot_and_capture` /
//! `drill_config_json` / `stage_bundle` / `gvisor_drill_config_json` would run the corpus as root and
//! the guard would observe `guest_euid=0` and FAIL. This test imports NONE of those harness fns — it
//! uses only the production `launch()` + the EXISTING corpus builders + the SHARED `parse_console`.

#![cfg(feature = "integration")]

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
    WorkspaceSpec,
};
use std::path::Path;

/// The fork-bomb pids ceiling the corpus is parametrized with (the same value the harness drills use).
const PIDS_MAX: u32 = 64;

/// The unprivileged uid the production seam drops the untrusted workload to on BOTH backends. The
/// routing guard asserts the corpus's BEGIN marker reports THIS euid (proving production-non-root
/// routing) and NOT `guest_euid=0` (which only the root drill harness produces).
const PROD_NONROOT_EUID: &str = "guest_euid=65534";
const ROOT_HARNESS_EUID: &str = "guest_euid=0";

/// The CT-003 GATE families: the egress/SSRF/exfil family + the read-only-root probe. A NON-ROOT
/// untrusted workload genuinely ATTEMPTS each of these through the production path, and the mandatory
/// hardening posture CONTAINS them. Each MUST be `Contained` on BOTH backends (the SI-017 proof).
const GATE_BOUNDARY_FAMILY: &[&str] = &[
    "M1_metadata_ssrf",
    "C1_controlplane",
    "T1_crosstenant",
    "E1_secret_exfil",
    "D1_root_readonly",
];

/// The privilege-contained kernel-primitive family — annotated, asserted only NOT-`Escaped` (a
/// non-root workload cannot mount these; this is NOT a microVM/gVisor-boundary claim — that proof is
/// the root-harness drills).
const PRIVILEGE_FAMILY: &[&str] = &["K1_module", "K2_devmem", "K3_ioport", "K4_mount"];

/// The disk-fill probe — **now GATED on BOTH backends (CT-003a closes the SI-017 residual).** The
/// production path now mounts a SIZE-BOUNDED writable scratch sized from `spec.limits.disk_bytes`
/// (gVisor: a `/tmp` tmpfs + a `linux.resources.memory.limit`; Firecracker: a `/run/scratch` tmpfs),
/// so the corpus's disk fill hits ENOSPC at the quota and reports `Contained`. A regression that drops
/// the bound lets the fill succeed (`Escaped`/host-RAM DoS) ⇒ RED.
const GATE_DISKFILL: &str = "D2_diskfill";

/// The fork-bomb probe. On **gVisor** the OCI `linux.resources.pids.limit` is enforced by the sentry
/// and the corpus's marker is authoritative ⇒ GATED `Contained`. On **Firecracker** the corpus's F1
/// self-check reads a cgroup it can only create as ROOT, so its marker is NOT authoritative on the
/// NON-ROOT production path (it reports `Escaped` from an unreadable cgroup). CT-003a instead applies
/// `ulimit -u pids_max` (RLIMIT_NPROC) in the in-guest runner so the fork bomb is REFUSED at the
/// ceiling and the guest SURVIVES — proven STRUCTURALLY by the caller: the corpus runs to its
/// `CORPUS_END` marker (no OOM truncation / exit 137) AND `D2_diskfill` runs and is `Contained`
/// AFTER F1. Before this fix the fork bomb OOM-killed the whole guest (exit 137, corpus truncated).
const GATE_FORKBOMB: &str = "F1_forkbomb";

/// The anonymous-memory-hog probe (CT-003b / SI-017) — **GATED CONTAINED on BOTH backends.** This
/// closes the REAL host-DoS escape CT-003 found on the gVisor production path: rootless `runsc` does
/// NOT enforce the OCI `memory.limit`, so an untrusted gVisor job could drive the HOST to OOM.
/// Firecracker bounds it with the hard guest-RAM cap; gVisor bounds it with the OUT-OF-BAND host
/// `MemoryCgroup` the production `launch()` now places the `runsc` process tree into. The corpus
/// allocates WELL OVER `mem_bytes` of anonymous memory; containment is proven STRUCTURALLY (the
/// ATTEMPT sentinel + END print before the alloc; an ESCAPED line prints ONLY if the oversized alloc
/// HELD). A regression dropping the memory enforcer ⇒ the hog HELDs ⇒ `Mx_memhog` Escaped ⇒ RED.
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

/// A hardened, digest-pinned, fully-default-deny `JobSpec` running `command` — the EXACT shape a real
/// untrusted job takes (no NIC, read-only root, pids ceiling, caps dropped, non-root payload).
fn corpus_spec(command: Vec<String>, tag: &str) -> JobSpec {
    JobSpec::new(
        JobKind::Ci,
        ImageRef::pinned("registry.example/runner@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef").unwrap(),
        command,
        vec![],
        vec![],
        EgressPolicy::deny_all(),
        ResourceLimits {
            cpu_millis: 1000,
            mem_bytes: 256 * 1024 * 1024,
            // A realistic scratch-disk quota (16 MiB) — SMALLER than the disk-fill attack writes (the
            // gVisor corpus dd's 512 MiB, the Firecracker corpus dd's 64 MiB), so the bounded scratch
            // tmpfs the production path now mounts (gVisor: /tmp sized from disk_bytes; Firecracker:
            // /run/scratch sized from disk_bytes) makes D2_diskfill hit ENOSPC and report CONTAINED.
            // This is the SI-017 fix: a disk fill is bounded at the quota, not the host's free RAM.
            disk_bytes: 16 * 1024 * 1024,
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

/// The shared CT-003 assertions over a real, parsed production-path run. `console` is the captured
/// `result.stdout` from `launch()`; this is the SAME `parse_console` the harness drills use. The split
/// is enforced here (boundary families gated; privilege family annotated, not silently passed). With
/// CT-003a the resource families are gated too: `D2_diskfill` MUST be `Contained` on both backends, and
/// `F1_forkbomb` MUST be `Contained` on a backend whose marker is authoritative (`f1_marker_authoritative`
/// — gVisor); on a backend where it is not (Firecracker, whose F1 self-check is root-only), F1 is
/// tolerated as a non-authoritative marker and its containment is proven structurally by the caller.
/// Returns the parsed report for the caller's logging.
fn assert_production_path_containment(
    backend_label: &str,
    console: &str,
    f1_marker_authoritative: bool,
) -> DrillReport {
    // --- ROUTING GUARD (SI-017): the corpus ran via the PRODUCTION non-root seam, NOT the harness. ---
    assert!(
        console.contains(PROD_NONROOT_EUID),
        "[CT-003 routing guard] {backend_label}: the corpus's BEGIN marker must report \
         `{PROD_NONROOT_EUID}` — PROVING it ran NON-ROOT via the production `launch()` command-runner \
         / OCI. Its absence means the corpus did not reach the guest through the production seam.\n\
         --- captured console ---\n{console}"
    );
    assert!(
        !console.contains(ROOT_HARNESS_EUID),
        "[CT-003 routing guard] {backend_label}: the corpus ran as ROOT (`{ROOT_HARNESS_EUID}`) — \
         this is the DRILL-HARNESS signature (Firecracker PID1 / gVisor drill config uid=0), NOT the \
         production seam. A regression has rerouted the corpus to `boot_and_capture` / \
         `drill_config_json` / `stage_bundle` / `gvisor_drill_config_json`. SI-017 has regressed.\n\
         --- captured console ---\n{console}"
    );

    let report = parse_console(console);
    println!("[CT-003 {backend_label}] {}", report.summary());

    // --- THE GATE: every BOUNDARY-attemptable family is genuinely CONTAINED (not Escaped, not -----
    //     DidNotRun). These markers are deterministic non-root and prove the SI-017 supply-chain
    //     containment through the REAL path.
    for id in GATE_BOUNDARY_FAMILY {
        assert_eq!(
            outcome_of(&report, id),
            &AttackOutcome::Contained,
            "[CT-003 GATE] {backend_label}: boundary-attemptable family `{id}` MUST be CONTAINED \
             through the production `launch()` path (it was genuinely attempted non-root and the \
             hardening posture — no NIC / `--network=none` / read-only root — must contain it). A \
             non-Contained outcome here is a REAL production-path escape ⇒ RED.\n--- console ---\n{console}"
        );
    }

    // --- CT-003a: the disk-fill probe is GATED on BOTH backends. The production path now mounts a ----
    //     size-bounded writable scratch (sized from spec.limits.disk_bytes), so the corpus's disk fill
    //     hits ENOSPC at the quota and reports Contained. A regression dropping the bound (an unbounded
    //     host-RAM-backed tmpfs again) lets the fill succeed ⇒ Escaped ⇒ RED. This closes the SI-017
    //     host-DoS escape D2 surfaced through the REAL launch().
    assert_eq!(
        outcome_of(&report, GATE_DISKFILL),
        &AttackOutcome::Contained,
        "[CT-003a GATE] {backend_label}: `{GATE_DISKFILL}` MUST be CONTAINED through launch() — the \
         production path mounts a scratch tmpfs sized from spec.limits.disk_bytes, so a disk fill hits \
         ENOSPC at the quota (never an unbounded host-RAM-backed tmpfs). A non-Contained outcome is a \
         regression of the SI-017 host-DoS fix ⇒ RED.\n--- console ---\n{console}"
    );

    // --- CT-003b: the anon-memory hog is GATED CONTAINED on BOTH backends. Firecracker bounds it ----
    //     with the hard guest-RAM cap; gVisor bounds it with the out-of-band host MemoryCgroup the
    //     production launch() places the runsc process tree into (rootless runsc ignores the OCI
    //     memory.limit). Containment is structural (ATTEMPT present, ESCAPED absent ⇒ Contained). A
    //     regression dropping the memory enforcer lets the oversized anon alloc HELD ⇒ Escaped ⇒ RED.
    //     This closes the SI-017 host-DoS escape on the gVisor production seam.
    assert_eq!(
        outcome_of(&report, GATE_MEMHOG),
        &AttackOutcome::Contained,
        "[CT-003b GATE] {backend_label}: `{GATE_MEMHOG}` MUST be CONTAINED through launch() — an \
         oversized anonymous allocation must be OOM-killed within the memory limit (FC: guest-RAM \
         cap; gVisor: the out-of-band cgroup), never holding host RAM beyond mem_bytes. A \
         non-Contained outcome is the SI-017 host-DoS escape ⇒ RED.\n--- console ---\n{console}"
    );

    // --- CT-003a: the fork-bomb probe. On a backend whose marker is authoritative (gVisor: the OCI ---
    //     pids.limit is sentry-enforced) F1 MUST be Contained. On Firecracker the F1 self-check reads a
    //     cgroup it can only create as ROOT, so its marker is NOT authoritative non-root; containment
    //     there is via `ulimit -u pids_max` (the fork bomb is refused at the ceiling, the guest
    //     survives) and is proven STRUCTURALLY by the caller (corpus runs to CORPUS_END + D2 runs).
    if f1_marker_authoritative {
        assert_eq!(
            outcome_of(&report, GATE_FORKBOMB),
            &AttackOutcome::Contained,
            "[CT-003a GATE] {backend_label}: `{GATE_FORKBOMB}` MUST be CONTAINED through launch() — the \
             OCI pids.limit is enforced and the marker is authoritative on this backend. A \
             non-Contained outcome ⇒ RED.\n--- console ---\n{console}"
        );
    } else {
        // The Firecracker non-root case: the marker is non-authoritative (root-only cgroup self-check),
        // so do NOT trust it as Contained. We DO require it never reports a genuine boundary breach as
        // anything other than the documented self-check artifact — the real containment proof is the
        // caller's survival assertion (CORPUS_END reached, not exit 137) + D2 Contained above.
        let f1 = outcome_of(&report, GATE_FORKBOMB);
        println!(
            "[CT-003a {backend_label}] F1_forkbomb marker = {f1:?} (non-authoritative: the corpus's \
             cgroup self-check needs root; the NON-ROOT production path enforces the ceiling via \
             `ulimit -u pids_max`). Containment proven structurally: guest survived to CORPUS_END + \
             D2 ran and was CONTAINED (before CT-003a the fork bomb OOM-killed the guest: exit 137, \
             corpus truncated)."
        );
    }

    // --- PRIVILEGE family: annotated, asserted only NOT-Escaped (never a silent boundary win). ------
    for id in PRIVILEGE_FAMILY {
        let o = outcome_of(&report, id);
        assert_ne!(
            o, &AttackOutcome::Escaped,
            "[CT-003] {backend_label}: privilege family `{id}` reported ESCAPED — a non-root workload \
             must NEVER breach a kernel primitive.\n--- console ---\n{console}"
        );
        // Honest annotation: this is privilege/namespace containment, NOT a microVM/gVisor boundary
        // proof (that is the root-harness drills' job).
        println!(
            "[CT-003 {backend_label}] privilege-contained (NOT a boundary claim): {id} = {o:?}"
        );
    }

    // --- ANTI-VACUITY / anti-cherry-pick: the ONLY tolerated `Escaped` outcomes are the documented ---
    //     privilege family (K1–K4) plus — ONLY where the marker is non-authoritative (Firecracker's
    //     root-only F1 self-check) — `F1_forkbomb`. With CT-003a, `D2_diskfill` is GATED on both
    //     backends and `F1_forkbomb` is GATED where its marker is authoritative (gVisor), so neither is
    //     tolerated there. ANY Escaped outside this set — a real M1/C1/T1/E1/D1/D2 breach, or an F1
    //     breach on the authoritative backend — fails RED. This keeps the parser honest: the gate
    //     cannot be satisfied by an escape hiding in a gated family.
    let mut tolerated: Vec<&str> = PRIVILEGE_FAMILY.to_vec();
    if !f1_marker_authoritative {
        tolerated.push(GATE_FORKBOMB);
    }
    for (id, _fam, outcome) in &report.outcomes {
        if *outcome == AttackOutcome::Escaped {
            assert!(
                tolerated.contains(id),
                "[CT-003a anti-vacuity] {backend_label}: `{id}` ESCAPED and it is NOT in the tolerated \
                 set {tolerated:?} — a real production-path escape slipped into a GATED family. RED.\n\
                 --- console ---\n{console}"
            );
            println!(
                "[CT-003a {backend_label}] non-authoritative marker (NOT a boundary breach — see \
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

    // Route the EXISTING Firecracker corpus (bash; `/dev/tcp` egress probes) through the PRODUCTION
    // `launch()` as `spec.command` — NOT `boot_and_capture` / `drill_config_json`. The production
    // command-runner runs it NON-ROOT (uid 65534) under the mandatory hardening profile.
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

    // Firecracker's F1 self-check is root-only ⇒ its marker is NOT authoritative on the non-root
    // production path; pass `f1_marker_authoritative = false`. D2 is gated Contained inside.
    let report = assert_production_path_containment("firecracker", &console, false);

    // --- CT-003a F1 STRUCTURAL containment proof (the non-vacuous fork-bomb gate for Firecracker). --
    //     Before CT-003a the fork bomb grew until the guest OOM-died (exit 137) and TRUNCATED the
    //     corpus (no CORPUS_END; D2 never ran). After `ulimit -u pids_max` the fork bomb is REFUSED at
    //     the ceiling, the guest SURVIVES, the corpus runs to its END marker, and D2 runs + is
    //     Contained (asserted above). Gating BOTH conditions makes a regression that drops the ceiling
    //     (→ OOM → truncation, or → no D2) trip RED — the fork bomb is genuinely contained, not vacuous.
    assert!(
        report.corpus_completed,
        "[CT-003a firecracker] the corpus MUST run to its CORPUS_END marker through launch() — the \
         fork bomb (F1) was refused at the pids ceiling (ulimit -u) so the guest SURVIVED. A missing \
         END marker means the fork bomb OOM-killed / truncated the guest (the pre-CT-003a exit-137 \
         failure) ⇒ RED.\n--- console ---\n{console}"
    );
    assert!(
        !launch.result.timed_out,
        "[CT-003a firecracker] the run must COMPLETE (not time out) — a refused fork bomb lets the \
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

    // Route the EXISTING gVisor corpus (busybox sh; `wget`/`nc` egress probes — the faithful gVisor
    // expression of the SAME catalogued attacks, sharing the SAME `parse_console`) through the
    // PRODUCTION `launch()` as `spec.command` — NOT `gvisor_drill_config_json` / the drill's
    // `stage_bundle`. The production OCI config runs it NON-ROOT (uid 65534), `--network=none`.
    let backend = GvisorBackend::new();
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

    // gVisor's OCI pids.limit is sentry-enforced and its F1 marker is authoritative ⇒
    // `f1_marker_authoritative = true`. D2 + F1 are both gated Contained inside.
    let report = assert_production_path_containment("gvisor", &console, true);

    // gVisor's busybox corpus runs the full catalogue to the END marker (it does not self-destabilize
    // like the Firecracker bash corpus's unbounded fork bomb), so the corpus completed here.
    assert!(
        report.corpus_completed,
        "[CT-003 gvisor] the gVisor corpus must run to its END marker through launch()\n--- console ---\n{console}"
    );

    // --- CT-003b: the gVisor production-path report is now GENUINELY GREEN through launch() ----------
    //     (every catalogued family Contained, zero escapes, corpus completed) now that BOTH the disk
    //     fill (D2 — a /tmp tmpfs sized from disk_bytes) AND the anonymous-memory hog (Mx_memhog — the
    //     out-of-band MemoryCgroup the runsc process tree is placed into) are GENUINELY bounded. The
    //     memhog is the load-bearing addition: before CT-003b rootless runsc ignored the OCI
    //     memory.limit, so an untrusted job's anon memory was UNBOUNDED (host-DoS). Mint the dated
    //     green PRODUCTION-PATH escape attestation. `from_green_drill` REFUSES over a non-green report,
    //     so a successful mint is itself proof the real launch() path is green (EI-01 §3: the green
    //     attestation IS the pass) — and it is minted ONLY because memory is now truly contained (if a
    //     host could not establish the cgroup, launch() would have FAILED CLOSED and no green minted).
    //     This is distinct from the harness drill's attestation: this one is minted from the NON-ROOT
    //     PRODUCTION seam (guest_euid=65534), closing SI-017 on the real path.
    assert!(
        report.is_green(),
        "[CT-003a gvisor] the gVisor PRODUCTION-PATH report must be genuinely green (all families \
         Contained, 0 escapes, corpus completed) now that D2 is bounded — escapes={} did_not_run={} \
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
         (guest_euid=65534) — SI-017 closed on the gVisor production seam."
    );

    backend
        .kill(&launch.handle)
        .expect("teardown is idempotent");
}
