//! # Consumer CDC for 8.4 — the Fabric exec gate consumes the PROD-IMAGE re-confirm attestation (AG-P21 → P-348)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 8.4 / CI-T1
//! (the prod-image re-confirm), CONSUMED here as the FABRIC's M4 go/no-go gate. **Owning prompt:**
//! `planning/07-prompts/by-system/agent-fabric.md` §AG-P21. **Architecture:** `agent-fabric.md` §2.2
//! (exec = CI's `kind=agent` job; AG-D4 == CI-T1 re-confirmed on the production image) + §9 row D-4.
//! **Drill:** rows CI-T1 / AG-D4 (re-confirmed green on the production CI runner image — the M4 hard
//! gate). **Doctrine:** EI-04 §5 (AG-D4 is a PERMANENT GATE re-run on every image change; a red AG-D4
//! is a dated no-go, never a weakened threshold); EI-01 §3 (the green attestation IS the pass).
//!
//! ## What this pair pins (the prod-image re-confirm seam — PROVIDER + CONSUMER)
//! The M4 re-confirm runs the SAME real escape battery on the PRODUCTION CI runner image (CI side
//! CI-P27 / P-370; the agent-side re-confirm drill is
//! `myelin-ci-sandbox/tests/escape_drill_prod_image_reconfirm_test.rs`). This CDC pins BOTH sides of
//! the seam:
//!
//! - **PROVIDER (the re-confirm drill, `myelin-ci-sandbox`):** the real-kernel drill on the prod
//!   runner image mints a GREEN [`EscapeAttestation`] ONLY from a genuinely green run (ZERO escapes; a
//!   red drill mints NO attestation — the structural source guard), tagged with the prod-image role.
//!   The helper here reproduces that PROVIDER output (minted from the corpus parser, never hardcoded —
//!   `from_green_drill` refuses to mint over a red drill), so the pair exercises the producer contract.
//! - **CONSUMER (the Fabric exec/escape-gate path, [`AgentExecGate`], REUSED whole, NOT
//!   re-implemented):** consumes that prod-image attestation as the Fabric's M4 go/no-go gate.
//!
//! The CONSUMER contract pinned here:
//!
//! 1. **fail-closed on the production runner image** — no green prod-image attestation ⇒ the Fabric
//!    REFUSES all untrusted compute on the prod image (the structural default is REFUSE);
//! 2. **a green, identity-matched prod-image attestation ADMITS** untrusted compute on the prod image
//!    (the M4 hard gate GREEN: AG-D4 / CI-T1 re-confirmed, ZERO escapes);
//! 3. **the prod image is a DISTINCT identity** — a green attestation for a DIFFERENT image rootfs
//!    does NOT admit the prod image (it must be re-drilled — the permanent gate);
//! 4. **ZERO escapes is the predicate** — a non-zero escape count is REFUSED even if the artifact
//!    claims green (defence-in-depth);
//! 5. **the prod-image RE-CONFIRM is unambiguous in the (reused) attestation** — the role + the
//!    Scaleway-Elastic-Metal residual + the no-floor permanent-gate posture are carried in writing,
//!    distinguishing the M4 prod-image attestation from the M2 base drill WITHOUT forking the type.
//!
//! The default suite NEVER boots a VM (it consumes the [`EscapeAttestation`] VALUE — the same JSON the
//! re-confirm drill writes). The `integration`-gated test at the bottom consumes the REAL prod-image
//! artifact if the drill has run on this host, else proves the gate is fail-closed (never a faked
//! green).

use myelin_agent_service::escape_gate::{AgentExecGate, GateRefusal, ProductionBackendId};
use myelin_ci_sandbox::escape_corpus::{BEGIN_MARKER, END_MARKER};
use myelin_ci_sandbox::{
    parse_console, Backend, BackendRun, EscapeAttestation, CORPUS, CORPUS_VERSION,
};

/// The PRODUCTION CI runner image rootfs digest the Fabric is about to dispatch onto. On a dev host
/// dev↔prod is a config swap (MYELIN_REGION=fr-par / prod=Scaleway), so the production runner image is
/// the config-resolved runner rootfs the re-confirm drill exercised; this fixed digest models that
/// identity for the VM-free consumer pair (the integration leg keys on the REAL artifact's digest).
const PROD_RUNNER_IMAGE_ROOTFS_SHA: &str =
    "7a2bc8ed2c64ed78994971439b00c234b1ce46d247123314d683df7579c77923";

/// The shared hardened unified-sandbox kernel digest (the escape surface IS the kernel; the prod
/// runner image runs on the SAME hardened kernel the base runner does).
const SHARED_KERNEL_SHA: &str = "467367e6b8e88323dd23dedae3119ade9c9fca6a102a84fc2155e3ef1bec00eb";

/// A DIFFERENT image rootfs — used to prove the prod runner image is a DISTINCT identity (a green
/// attestation for another image must NOT admit the prod image; it must be re-drilled).
const OTHER_IMAGE_ROOTFS_SHA: &str =
    "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef0";

/// The prod-image ROLE residual the re-confirm drill tags into the (reused) attestation — the M4
/// re-confirm label that distinguishes it from the M2 base drill (mirrors the drill's tag).
const PROD_IMAGE_ROLE: &str =
    "M4 PROD-IMAGE RE-CONFIRM (AG-P21 / P-348; CI side CI-P27 / P-370): this attestation re-confirms \
     AG-D4 / CI-T1 on the PRODUCTION CI runner image (the config-resolved runner rootfs — dev↔prod is \
     a config swap, MYELIN_REGION=fr-par / prod=Scaleway). The production runner runs on KVM-capable \
     Scaleway Elastic Metal; the prod image is re-drilled there at deploy — that is a NAMED residual, \
     not faked here.";

/// The production backend identity for the PROD RUNNER IMAGE — the identity the AG-D4 gate must match
/// a green attestation against before any untrusted compute dispatches on the prod image.
fn prod_runner_image_id() -> ProductionBackendId {
    ProductionBackendId {
        backend: Backend::FirecrackerMicrovm,
        rootfs_sha256: PROD_RUNNER_IMAGE_ROOTFS_SHA.into(),
        kernel_sha256: SHARED_KERNEL_SHA.into(),
        corpus_version: CORPUS_VERSION,
    }
}

/// A REAL green drill report → a green attestation for `rootfs_sha`, tagged as the prod-image
/// re-confirm exactly as the re-confirm drill does (minted from the corpus parser — NEVER hardcoded;
/// `from_green_drill` refuses to mint over a red drill). `escaped` flips one attack to model a red
/// drill (which mints NO attestation).
fn prod_image_attestation(rootfs_sha: &str, escaped: bool) -> Result<EscapeAttestation, String> {
    let mut console = format!("{BEGIN_MARKER} corpus_version=1 kernel=6.1.168 guest_euid=0\n");
    for atk in CORPUS {
        console.push_str(&format!("{} CONTAINED\n", atk.id));
    }
    if escaped {
        console = console.replace("K1_module CONTAINED", "K1_module ESCAPED");
    }
    console.push_str(&format!("{END_MARKER}\n"));
    let report = parse_console(&console);
    let mut att = EscapeAttestation::from_green_drill(
        "2026-06-23",
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
    )?;
    // The re-confirm drill tags the prod-image role into the REUSED attestation's residuals.
    att.residuals.insert(0, PROD_IMAGE_ROLE.to_string());
    Ok(att)
}

// ───────────────────────── (1) fail-closed on the production runner image ────────────────────────

/// **THE headline AG-P21 property: no green AG-D4 attestation for the production runner image ⇒ the
/// Fabric REFUSES all untrusted compute on the prod image.** The structural default is REFUSE (a
/// red/absent AG-D4 on the prod image blocks all untrusted compute, EI-04 §5).
#[test]
fn no_untrusted_compute_on_the_prod_image_without_a_green_reconfirm_attestation() {
    let r = AgentExecGate::admit(None, &prod_runner_image_id());
    assert_eq!(r.unwrap_err(), GateRefusal::NoAttestation);
}

/// **A RED re-confirm drill on the prod image mints NO attestation (the source guard) — the gate
/// stays closed.** A red AG-D4 on the prod image is a DATED NO-GO that blocks the M4 boundary; it is
/// NOT greened by weakening the assertion.
#[test]
fn a_red_reconfirm_on_the_prod_image_mints_no_attestation() {
    let minted = prod_image_attestation(PROD_RUNNER_IMAGE_ROOTFS_SHA, /* escaped */ true);
    assert!(
        minted.is_err(),
        "a red re-confirm drill on the prod image mints NO green attestation"
    );
    assert_eq!(
        AgentExecGate::admit(None, &prod_runner_image_id()).unwrap_err(),
        GateRefusal::NoAttestation
    );
}

/// **ZERO escapes is the predicate — a non-zero escape count is REFUSED even if the artifact claims
/// green (defence-in-depth).** The gate checks the escape count itself.
#[test]
fn a_nonzero_escape_count_on_the_prod_image_is_refused() {
    let mut att = prod_image_attestation(PROD_RUNNER_IMAGE_ROOTFS_SHA, false).unwrap();
    att.total_escapes = 3;
    let r = AgentExecGate::admit(Some(&att), &prod_runner_image_id());
    assert_eq!(r.unwrap_err(), GateRefusal::Escapes { total_escapes: 3 });
}

// ───────────────────────── (2) a green prod-image re-confirm admits — the M4 GATE GREEN ──────────

/// **A green, identity-matched prod-image re-confirm attestation ADMITS untrusted compute on the prod
/// image (the M4 HARD GATE GREEN).** AG-D4 / CI-T1 re-confirmed on the production runner image, ZERO
/// escapes — the Fabric's M4 go/no-go reads GO.
#[test]
fn a_green_prod_image_reconfirm_admits_untrusted_compute() {
    let att = prod_image_attestation(PROD_RUNNER_IMAGE_ROOTFS_SHA, false).unwrap();
    let gate = AgentExecGate::admit(Some(&att), &prod_runner_image_id())
        .expect("a green, identity-matched prod-image re-confirm admits");
    assert_eq!(
        gate.backend_id().rootfs_sha256,
        PROD_RUNNER_IMAGE_ROOTFS_SHA
    );
    assert!(gate.open_line().starts_with("[AG-D4 GATE OPEN]"));
    assert!(gate.open_line().contains("ZERO escapes"));
}

/// **The prod-image re-confirm is UNAMBIGUOUS in the (reused) attestation: the role + the
/// Scaleway-Elastic-Metal residual + the no-floor permanent-gate posture are carried in writing.**
/// This is how the M4 prod-image attestation is told apart from the M2 base drill WITHOUT forking the
/// `EscapeAttestation` type.
#[test]
fn the_prod_image_reconfirm_role_and_residuals_are_named_in_writing() {
    let att = prod_image_attestation(PROD_RUNNER_IMAGE_ROOTFS_SHA, false).unwrap();
    assert!(
        att.residuals
            .iter()
            .any(|r| r.contains("PROD-IMAGE RE-CONFIRM") && r.contains("Scaleway Elastic Metal")),
        "the prod-image role + the Scaleway-metal residual (CI-P27 / P-370) are named in writing"
    );
    assert!(
        att.residuals.iter().any(|r| r.contains("PERMANENT GATE")),
        "the no-floor permanent-gate posture is carried in the prod-image attestation"
    );
    assert!(
        att.residuals
            .iter()
            .any(|r| r.contains("CI-P27") || r.contains("P-348")),
        "the M4 re-confirm cross-reference is carried in the attestation"
    );
}

// ───────────────────────── (3) the prod image is a DISTINCT identity (permanent gate) ────────────

/// **The permanent gate: a green attestation for a DIFFERENT image does NOT admit the production
/// runner image (it must be re-drilled).** A green proof for one image is NOT a green proof for the
/// prod image — the re-confirm is keyed on the prod image's own identity.
#[test]
fn a_different_image_attestation_does_not_admit_the_prod_image() {
    let other_att = prod_image_attestation(OTHER_IMAGE_ROOTFS_SHA, false).unwrap();
    let r = AgentExecGate::admit(Some(&other_att), &prod_runner_image_id());
    assert!(
        matches!(r.unwrap_err(), GateRefusal::IdentityMismatch { .. }),
        "the prod image must be re-drilled — a different-image green does not admit it"
    );
}

/// **A changed kernel or corpus on the prod image must be re-drilled (the permanent gate).**
#[test]
fn a_changed_kernel_or_corpus_on_the_prod_image_must_be_redrilled() {
    let att = prod_image_attestation(PROD_RUNNER_IMAGE_ROOTFS_SHA, false).unwrap();
    let mut id = prod_runner_image_id();
    id.corpus_version = CORPUS_VERSION + 7;
    assert!(matches!(
        AgentExecGate::admit(Some(&att), &id).unwrap_err(),
        GateRefusal::IdentityMismatch { .. }
    ));
}

/// **A prod-image attestation whose gate backend is gVisor does NOT admit the Firecracker production
/// default** — the gate is admitted only on the production backend.
#[test]
fn a_gvisor_gate_backend_does_not_admit_the_firecracker_prod_image() {
    let mut att = prod_image_attestation(PROD_RUNNER_IMAGE_ROOTFS_SHA, false).unwrap();
    att.gate_backend = Backend::GvisorRunsc;
    let r = AgentExecGate::admit(Some(&att), &prod_runner_image_id());
    assert_eq!(
        r.unwrap_err(),
        GateRefusal::GateBackendMismatch {
            attested: Backend::GvisorRunsc,
            expected: Backend::FirecrackerMicrovm,
        }
    );
}

// ───────────────────────── (4) the REAL prod-image artifact (integration-gated) ──────────────────

/// **The `integration`-gated REAL leg: the Fabric gate ADMITS on the REAL prod-image re-confirm
/// attestation, or is fail-closed without one (never a faked green).** Gated behind `--features
/// integration` so the default suite never depends on a VM-produced artifact. If the prod-image
/// re-confirm drill ran on this host (the `prod-image-<date>.json` artifact exists), this proves the
/// Fabric exec path admits untrusted compute against the REAL green re-confirm, identity-matched on
/// the attestation's OWN production-image identity. If not, it proves fail-closed.
#[cfg(feature = "integration")]
#[test]
fn the_fabric_gate_on_the_real_prod_image_reconfirm_or_fail_closed() {
    use std::path::PathBuf;
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("ag-d4-attestation");
    // The prod-image re-confirm drill writes a DISTINCT `prod-image-<date>.json` artifact.
    let latest = std::fs::read_dir(&dir)
        .ok()
        .and_then(|rd| {
            let mut entries: Vec<PathBuf> = rd
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.starts_with("prod-image-") && n.ends_with(".json"))
                        .unwrap_or(false)
                })
                .collect();
            entries.sort();
            entries.pop()
        })
        .and_then(|p| std::fs::read_to_string(&p).ok())
        .and_then(|j| serde_json::from_str::<EscapeAttestation>(&j).ok());

    match latest {
        Some(att) => {
            // The prod-image re-confirm ran on real silicon. Admit untrusted compute against the REAL
            // attestation, using the attestation's OWN production-image identity.
            let id = ProductionBackendId {
                backend: att.gate_backend,
                rootfs_sha256: att.rootfs_sha256.clone(),
                kernel_sha256: att.kernel_sha256.clone(),
                corpus_version: att.corpus_version,
            };
            let gate = AgentExecGate::admit(Some(&att), &id)
                .expect("the REAL green prod-image re-confirm admits untrusted compute");
            println!("[AG-P21 prod-image re-confirm] {}", gate.open_line());
            assert_eq!(
                att.total_escapes, 0,
                "the real prod-image attestation is ZERO escapes"
            );
            assert!(
                att.residuals
                    .iter()
                    .any(|r| r.contains("PROD-IMAGE RE-CONFIRM")),
                "the REAL artifact is tagged as the M4 prod-image re-confirm"
            );
            // The permanent gate: a changed corpus version does NOT admit the real attestation.
            let mut changed = id.clone();
            changed.corpus_version = att.corpus_version + 1;
            assert!(matches!(
                AgentExecGate::admit(Some(&att), &changed).unwrap_err(),
                GateRefusal::IdentityMismatch { .. }
            ));
        }
        None => {
            println!(
                "[AG-P21 prod-image re-confirm] no real prod-image attestation artifact — the \
                 re-confirm drill has not run on this host; proving FAIL-CLOSED (no green ⇒ no \
                 untrusted compute on the prod image)"
            );
            assert_eq!(
                AgentExecGate::admit(None, &prod_runner_image_id()).unwrap_err(),
                GateRefusal::NoAttestation
            );
        }
    }
}
