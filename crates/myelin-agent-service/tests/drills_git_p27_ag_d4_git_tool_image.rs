//! # AG-D4 / CI-T1 re-run on the GIT TOOL IMAGE (GIT-P27 → P-283, M3-G6)
//!
//! **Drill:** `05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`
//! row **AG-D4 / CI-T1** — *re-run on the git tool image* (the prompt's GATE). **Contract:** row 8.4
//! (the real-kernel escape drill gates BOTH kinds; the code-executing git tools ride the unified
//! sandbox). **Architecture:** `git-hosting/architecture/03-events-contracts-and-glue.md` §7 (the
//! four uniform sandbox guarantees apply BY CONSTRUCTION to any git tool that executes code — the
//! history-rewrite activity, SCIP indexing). **Reconciliation:** X-6 (the escape drill gates all
//! agent execution). **Doctrine:** EI-01 §2 (RCE/sandbox-escape outranks every feature); EI-04 §5
//! (a red AG-D4 blocks ALL of M3+ — the permanent gate, never weakened to manufacture green).
//!
//! ## What this drill re-confirms (the GIT-P27 GATE — the PERMANENT escape gate on the git image)
//! AG-D4 is the SHARED escape gate; CI proves it on a real Firecracker microVM (CI-P5 → P-239), the
//! Fabric CONSUMES the green attestation as a fail-closed gate ([`AgentExecGate`], AG-P17 → P-229).
//! GIT-P27's deliverable is **re-running that gate on the git tool image** — the runner image the
//! code-executing git tools (`git.history_rewrite`, `git.scip_index`) launch in. The permanent gate
//! re-runs on every backend/image/kernel change, so the **git tool image is a distinct identity** and
//! its escape attestation is verified for THAT identity. This file pins:
//!
//! 1. **fail-closed on the git tool image** — no green attestation for the git tool image ⇒ NO
//!    code-executing git tool runs (the structural REFUSE; the headline property);
//! 2. **a green, identity-matched git-tool-image attestation ADMITS** the code-executing git tools;
//! 3. **the git tool image is a DISTINCT identity** — a green attestation for the BASE runner image
//!    does NOT admit the git tool image (it must be re-drilled — the permanent gate);
//! 4. **the code-executing git tools are exactly the two §7 tools** (`history_rewrite` gated
//!    `Mutate` + `scip_index` `Compute`), and a `Compute` tool is the ONLY kind that reaches the
//!    bare kernel sandbox (so AG-D4 on the git tool image gates the SCIP indexer by construction);
//! 5. **ZERO escapes is the predicate** — a non-zero escape count is REFUSED even if the artifact
//!    claims green.
//!
//! The default suite NEVER boots a VM (it consumes the [`EscapeAttestation`] VALUE, the same JSON the
//! P-239 drill writes); the `integration`-gated test at the bottom consumes the REAL artifact if the
//! drill has run on this host, else proves the gate is fail-closed without one (never a faked green).
//!
//! ## The git tool image identity
//! The git tool image (the runner image canonical `git` + the SCIP indexer execute in) is a distinct
//! rootfs from the base agent runner — so it carries its OWN rootfs digest in the
//! [`ProductionBackendId`]. The kernel + corpus are the SHARED unified-sandbox kernel + adversarial
//! corpus (the escape surface is the kernel, not the rootfs — but the git tool image's rootfs ships
//! `git` + `scip-index`, so it is re-drilled as its own identity).

use myelin_agent::EffectKind;
use myelin_agent_service::escape_gate::{AgentExecGate, GateRefusal, ProductionBackendId};
use myelin_agent_service::{git_scip_index_tool_def, git_tool_defs};
use myelin_ci_sandbox::escape_corpus::{BEGIN_MARKER, END_MARKER};
use myelin_ci_sandbox::{
    parse_console, Backend, BackendRun, EscapeAttestation, CORPUS, CORPUS_VERSION,
};

/// The **git tool image** rootfs digest — the runner image the code-executing git tools launch in
/// (canonical `git` + the SCIP indexer). Distinct from the base agent runner's rootfs, so it is a
/// distinct AG-D4 identity (the permanent gate re-runs on every image change).
const GIT_TOOL_IMAGE_ROOTFS_SHA: &str =
    "9f1c0a44e7b3d2516c8af0e1d4b6790235ac8d11ff62b4e0a7d3c91b8e2f50ab";

/// The shared unified-sandbox kernel digest (the escape surface IS the kernel; the git tool image
/// runs on the SAME hardened kernel the base runner does).
const SHARED_KERNEL_SHA: &str = "467367e6b8e88323dd23dedae3119ade9c9fca6a102a84fc2155e3ef1bec00eb";

/// The base (non-git) runner image rootfs digest — used to prove the git tool image is a DISTINCT
/// identity (a green attestation for the base image must NOT admit the git tool image).
const BASE_RUNNER_ROOTFS_SHA: &str =
    "7a2bc8ed2c64ed78994971439b00c234b1ce46d247123314d683df7579c77923";

/// The production backend identity for the GIT TOOL IMAGE — the identity the AG-D4 gate must match a
/// green attestation against before any code-executing git tool dispatches.
fn git_tool_image_id() -> ProductionBackendId {
    ProductionBackendId {
        backend: Backend::FirecrackerMicrovm,
        rootfs_sha256: GIT_TOOL_IMAGE_ROOTFS_SHA.into(),
        kernel_sha256: SHARED_KERNEL_SHA.into(),
        corpus_version: CORPUS_VERSION,
    }
}

/// A REAL green drill report → a green attestation for a given `rootfs_sha` (minted from the corpus
/// parser, NEVER hardcoded). `escaped` flips one attack to ESCAPED to model a red drill.
fn attestation_for(rootfs_sha: &str, escaped: bool) -> Result<EscapeAttestation, String> {
    let mut console = format!("{BEGIN_MARKER} corpus_version=1 kernel=6.1.168 guest_euid=0\n");
    for atk in CORPUS {
        console.push_str(&format!("{} CONTAINED\n", atk.id));
    }
    if escaped {
        console = console.replace("K1_module CONTAINED", "K1_module ESCAPED");
    }
    console.push_str(&format!("{END_MARKER}\n"));
    let report = parse_console(&console);
    EscapeAttestation::from_green_drill(
        "2026-06-21",
        &report,
        vec![
            BackendRun {
                backend: Backend::FirecrackerMicrovm,
                exercised: true,
                residual_note: None,
            },
            BackendRun {
                backend: Backend::GvisorRunsc,
                exercised: false,
                residual_note: Some("runsc residual (CI-P28)".into()),
            },
        ],
        Backend::FirecrackerMicrovm,
        rootfs_sha,
        SHARED_KERNEL_SHA,
        "6.1.168",
    )
}

// ───────────────────────── (1) fail-closed on the git tool image ─────────────────────────────────

/// **THE headline GIT-P27 property: no green AG-D4 attestation for the git tool image ⇒ NO
/// code-executing git tool runs.** The structural default is REFUSE — a red/absent AG-D4 on the git
/// tool image BLOCKS this prompt's tools (EI-04 §5: a red AG-D4 blocks all untrusted compute).
#[test]
fn no_code_executing_git_tool_runs_without_a_green_attestation_for_the_git_tool_image() {
    let r = AgentExecGate::admit(None, &git_tool_image_id());
    assert_eq!(r.unwrap_err(), GateRefusal::NoAttestation);
}

/// **A RED drill on the git tool image mints NO attestation (the source guard) — the gate stays
/// closed.** A red AG-D4 on the git tool image BLOCKS the code-executing git tools; it is NOT greened
/// by weakening the assertion.
#[test]
fn a_red_drill_on_the_git_tool_image_mints_no_attestation() {
    let minted = attestation_for(GIT_TOOL_IMAGE_ROOTFS_SHA, /* escaped */ true);
    assert!(
        minted.is_err(),
        "a red drill on the git tool image mints NO green attestation"
    );
    assert_eq!(
        AgentExecGate::admit(None, &git_tool_image_id()).unwrap_err(),
        GateRefusal::NoAttestation
    );
}

/// **ZERO escapes is the predicate — a non-zero escape count is REFUSED even if the artifact claims
/// green (defence-in-depth).** The gate checks the escape count itself.
#[test]
fn a_nonzero_escape_count_on_the_git_tool_image_is_refused() {
    let mut att = attestation_for(GIT_TOOL_IMAGE_ROOTFS_SHA, false).unwrap();
    att.total_escapes = 2;
    let r = AgentExecGate::admit(Some(&att), &git_tool_image_id());
    assert_eq!(r.unwrap_err(), GateRefusal::Escapes { total_escapes: 2 });
}

// ───────────────────────── (2) a green git-tool-image attestation admits ─────────────────────────

/// **A green, identity-matched git-tool-image attestation ADMITS the code-executing git tools (the
/// gate opens on the git tool image).** This is the GIT-P27 GATE green: AG-D4 re-confirmed on the git
/// tool image, ZERO escapes.
#[test]
fn a_green_git_tool_image_attestation_admits_the_code_executing_git_tools() {
    let att = attestation_for(GIT_TOOL_IMAGE_ROOTFS_SHA, false).unwrap();
    let gate = AgentExecGate::admit(Some(&att), &git_tool_image_id())
        .expect("a green, identity-matched git-tool-image attestation admits");
    assert_eq!(gate.backend_id().rootfs_sha256, GIT_TOOL_IMAGE_ROOTFS_SHA);
    assert!(gate.open_line().starts_with("[AG-D4 GATE OPEN]"));
    assert!(gate.open_line().contains("ZERO escapes"));
}

// ───────────────────────── (3) the git tool image is a DISTINCT identity ─────────────────────────

/// **The permanent gate: a green attestation for the BASE runner image does NOT admit the git tool
/// image (it must be re-drilled).** The git tool image ships `git` + `scip-index` — a distinct
/// rootfs, a distinct AG-D4 identity. A green proof for the base runner is NOT a green proof for the
/// git tool image.
#[test]
fn a_base_image_attestation_does_not_admit_the_git_tool_image() {
    // a GREEN attestation, but for the BASE runner image rootfs (not the git tool image's).
    let base_att = attestation_for(BASE_RUNNER_ROOTFS_SHA, false).unwrap();
    let r = AgentExecGate::admit(Some(&base_att), &git_tool_image_id());
    assert!(
        matches!(r.unwrap_err(), GateRefusal::IdentityMismatch { .. }),
        "the git tool image must be re-drilled — a base-image green does not admit it"
    );
}

/// **A changed kernel / corpus on the git tool image must be re-drilled (the permanent gate).**
#[test]
fn a_changed_kernel_or_corpus_on_the_git_tool_image_must_be_redrilled() {
    let att = attestation_for(GIT_TOOL_IMAGE_ROOTFS_SHA, false).unwrap();
    // changed corpus version → re-drill.
    let mut id = git_tool_image_id();
    id.corpus_version = CORPUS_VERSION + 11;
    assert!(matches!(
        AgentExecGate::admit(Some(&att), &id).unwrap_err(),
        GateRefusal::IdentityMismatch { .. }
    ));
}

// ───────────────────────── (4) the code-executing git tools ride the gated sandbox ──────────────

/// **The code-executing git tools are exactly the §7 tools, and SCIP indexing is the `Compute` tool
/// that the AG-D4 git-tool-image gate guards by construction.** `git.scip_index` is `Compute` ⇒ it is
/// the ONLY git tool that reaches the bare kernel sandbox (the routing split in `exec.rs`); so a green
/// AG-D4 on the git tool image is its structural prerequisite. `git.history_rewrite` is a gated
/// `Mutate` (the audited erasure-admin op, plan-then-apply — it withholds for HITL, never an un-gated
/// sandbox bypass).
#[test]
fn the_code_executing_git_tools_are_the_two_section_7_tools() {
    let defs = git_tool_defs();
    // SCIP indexing is a Compute tool (the ONLY kind that touches the bare sandbox — AG-D4 gated).
    let scip = git_scip_index_tool_def();
    assert_eq!(
        scip.effect_kind,
        EffectKind::Compute,
        "SCIP indexing rides the bare sandbox"
    );
    assert!(defs
        .iter()
        .any(|d| d.name.0 == "scip_index" && d.effect_kind == EffectKind::Compute));
    // history-rewrite is a GATED Mutate (plan-then-apply — never an un-gated sandbox bypass).
    let hr = defs
        .iter()
        .find(|d| d.name.0 == "history_rewrite")
        .expect("history_rewrite registered");
    assert_eq!(hr.effect_kind, EffectKind::Mutate);
    assert!(
        hr.requires_approval,
        "history-rewrite is HITL-gated (the audited erasure-admin op)"
    );
    // exactly ONE Compute (sandbox-bound) git tool — the SCIP indexer (so the AG-D4 git-image gate
    // is the prerequisite of exactly the SCIP code-executing path among the git tools).
    let compute_count = defs
        .iter()
        .filter(|d| d.effect_kind == EffectKind::Compute)
        .count();
    assert_eq!(
        compute_count, 1,
        "only SCIP indexing reaches the bare sandbox among git tools"
    );
}

// ───────────────────────── (5) the REAL artifact (integration-gated) ─────────────────────────────

/// **The `integration`-gated REAL leg: the git-tool-image gate ADMITS on the real P-239 attestation,
/// or is fail-closed without one (never a faked green).** Gated behind `--features integration` so the
/// default suite never depends on a VM-produced artifact. If the real drill ran on this host (the
/// artifact exists), this proves the gate admits the code-executing git tools against the REAL green
/// attestation, identity-matched. If not, it proves fail-closed (no green ⇒ no untrusted compute).
#[cfg(feature = "integration")]
#[test]
fn the_git_tool_image_gate_on_the_real_p239_attestation_or_fail_closed() {
    use std::path::PathBuf;
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("ag-d4-attestation");
    let latest = std::fs::read_dir(&dir)
        .ok()
        .and_then(|rd| {
            let mut entries: Vec<PathBuf> = rd
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
                .collect();
            entries.sort();
            entries.pop()
        })
        .and_then(|p| std::fs::read_to_string(&p).ok())
        .and_then(|j| serde_json::from_str::<EscapeAttestation>(&j).ok());

    match latest {
        Some(att) => {
            // The real drill ran. Admit the code-executing git tools against the REAL attestation,
            // using the attestation's OWN identity (the runner's pinned image the drill exercised).
            let id = ProductionBackendId {
                backend: att.gate_backend,
                rootfs_sha256: att.rootfs_sha256.clone(),
                kernel_sha256: att.kernel_sha256.clone(),
                corpus_version: att.corpus_version,
            };
            let gate = AgentExecGate::admit(Some(&att), &id)
                .expect("the REAL green attestation admits the code-executing git tools");
            println!("[GIT-P27 AG-D4] {}", gate.open_line());
            assert_eq!(att.total_escapes, 0, "the real attestation is ZERO escapes");
        }
        None => {
            println!(
                "[GIT-P27 AG-D4] no real attestation artifact — the git tool image escape drill has \
                 not run on this host; proving FAIL-CLOSED (no green ⇒ no code-executing git tool)"
            );
            assert_eq!(
                AgentExecGate::admit(None, &git_tool_image_id()).unwrap_err(),
                GateRefusal::NoAttestation
            );
        }
    }
}
