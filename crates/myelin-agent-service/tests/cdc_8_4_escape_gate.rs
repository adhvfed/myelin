//! # The consumer CDC for contract 8.4 (the drill-as-gate) — the Fabric exec path is fail-closed on AG-D4
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 8.4 (*the
//! real-kernel escape drill gates BOTH kinds — CI and agent*), CONSUMED here as a GATE.
//! **Reconciliation:** `00-reconciliation-decisions.md` X-6 (the escape drill gates all agent
//! execution; `ToolHands::exec` **is** CI's `kind=agent` job). **Architecture:** `agent-fabric.md`
//! §2.2 guarantee 4 (*the real-kernel escape drill is the single hard go/no-go before any untrusted
//! customer code runs — CI or agent*) + §9 row D-4. **Drill:** row **AG-D4 / CI-T1**.
//!
//! ## What this pair pins (the FABRIC half of AG-P17 → P-229)
//! Row 8.4 has a CI half (the runner + the real-kernel escape drill that emits the green
//! [`EscapeAttestation`], CI-P5 → P-239, proven on a real Firecracker microVM) and a FABRIC half
//! (this prompt): the exec dispatch path REFUSES to dispatch a `kind=agent` compute job unless a GREEN
//! `EscapeAttestation` exists for the production backend. This file pins the CONSUMER contract:
//!
//! - **PROVIDER (CI's drill, `myelin-ci-sandbox`):** mints an [`EscapeAttestation`] ONLY from a
//!   genuinely green drill (ZERO escapes; a red drill mints NO attestation — the structural source
//!   guard). The Fabric CONSUMES this type — it does NOT re-implement the drill nor fork the type.
//! - **CONSUMER (the Fabric exec gate, [`AgentExecGate`]):** admits untrusted compute IFF the
//!   attestation is green for the production backend (`total_escapes == 0`, `gate_backend ==
//!   Firecracker`, exercised on real silicon, matching rootfs/kernel/corpus identity). A missing /
//!   red / mismatched attestation is a structural REFUSAL — and a `SandboxToolHands` (the exec
//!   dispatch path) cannot even be constructed without a green gate, so **no green attestation ⇒ no
//!   untrusted compute** is encoded in the TYPE (never a hardcoded `true`).
//!
//! The check keys on the REAL [`EscapeAttestation`] fields consumed from `myelin-ci-sandbox` (the
//! same JSON the P-239 drill wrote). This default suite NEVER boots a VM; the `integration`-gated
//! `tests/integration_escape_gate.rs` consumes the REAL attestation artifact.

use myelin_agent_service::escape_gate::{AgentExecGate, GateRefusal, ProductionBackendId};
use myelin_ci_sandbox::escape_corpus::{BEGIN_MARKER, END_MARKER};
use myelin_ci_sandbox::{
    parse_console, Backend, BackendRun, EscapeAttestation, CORPUS, CORPUS_VERSION,
};

/// The production backend identity the Fabric is about to dispatch onto (the gate verifies the
/// attestation describes EXACTLY this — the permanent gate, re-run on every identity change).
fn prod_id() -> ProductionBackendId {
    ProductionBackendId {
        backend: Backend::FirecrackerMicrovm,
        rootfs_sha256: "7a2bc8ed2c64ed78994971439b00c234b1ce46d247123314d683df7579c77923".into(),
        kernel_sha256: "467367e6b8e88323dd23dedae3119ade9c9fca6a102a84fc2155e3ef1bec00eb".into(),
        corpus_version: CORPUS_VERSION,
    }
}

/// A REAL green drill report → a green attestation (PROVIDER side, minted from the corpus parser —
/// never hardcoded). `escaped` flips one attack to ESCAPED to model a red drill.
fn attestation(escaped: bool) -> Result<EscapeAttestation, String> {
    let mut console = format!("{BEGIN_MARKER} corpus_version=1 kernel=6.1.168 guest_euid=0\n");
    for atk in CORPUS {
        console.push_str(&format!("{} CONTAINED\n", atk.id));
    }
    if escaped {
        console = console.replace("K1_module CONTAINED", "K1_module ESCAPED");
    }
    console.push_str(&format!("{END_MARKER}\n"));
    let report = parse_console(&console);
    let id = prod_id();
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
        id.rootfs_sha256,
        id.kernel_sha256,
        "6.1.168",
    )
}

// ───────────────────────────── the FAIL-CLOSED contract (no green ⇒ no compute) ──────────────────

/// **THE headline consumer property: with NO attestation, the gate REFUSES all untrusted compute.**
/// The structural default is REFUSE — no green AG-D4 attestation ⇒ no untrusted compute.
#[test]
fn fabric_exec_is_fail_closed_without_a_green_attestation() {
    let r = AgentExecGate::admit(None, &prod_id());
    assert_eq!(r.unwrap_err(), GateRefusal::NoAttestation);
}

/// **A RED drill mints NO attestation (the source guard) — so a red AG-D4 fails the gate closed.**
/// CI's drill refuses to mint a green attestation over a red drill; with no artifact, the Fabric gate
/// stays closed. A red AG-D4 blocks ALL untrusted compute (EI-04 §5).
#[test]
fn a_red_drill_mints_no_attestation_and_the_gate_stays_closed() {
    let minted = attestation(true);
    assert!(minted.is_err(), "a red drill mints NO green attestation");
    // No artifact to feed the gate ⇒ fail-closed.
    assert_eq!(
        AgentExecGate::admit(None, &prod_id()).unwrap_err(),
        GateRefusal::NoAttestation
    );
}

/// **Defence-in-depth: even a forged green-looking attestation with escapes > 0 is REFUSED.** The
/// gate checks the escape count itself — it never trusts "green" without verifying ZERO escapes.
#[test]
fn a_nonzero_escape_count_is_refused_even_if_the_artifact_claims_green() {
    let mut att = attestation(false).unwrap();
    att.total_escapes = 3;
    let r = AgentExecGate::admit(Some(&att), &prod_id());
    assert_eq!(r.unwrap_err(), GateRefusal::Escapes { total_escapes: 3 });
}

// ───────────────────────────── a GREEN matching attestation admits ───────────────────────────────

/// **A green, identity-matched attestation ADMITS untrusted compute (the gate opens).**
#[test]
fn a_green_attestation_for_the_production_backend_admits() {
    let att = attestation(false).unwrap();
    let gate = AgentExecGate::admit(Some(&att), &prod_id())
        .expect("a green, identity-matched attestation admits");
    assert_eq!(gate.backend_id().backend, Backend::FirecrackerMicrovm);
    assert!(gate.open_line().starts_with("[AG-D4 GATE OPEN]"));
    assert!(gate.open_line().contains("ZERO escapes"));
}

/// **The gate consumes the SAME JSON artifact the P-239 drill writes (round-trip).**
#[test]
fn the_gate_consumes_the_real_json_artifact_form() {
    let json = attestation(false).unwrap().to_json();
    let gate =
        AgentExecGate::admit_from_json(&json, &prod_id()).expect("the green JSON artifact admits");
    assert_eq!(gate.backend_id().corpus_version, CORPUS_VERSION);
}

// ───────────────────────────── identity is load-bearing (the PERMANENT gate) ─────────────────────

/// **A green attestation for a DIFFERENT image must be RE-DRILLED (the permanent gate).** A green
/// proof for one rootfs/kernel/corpus identity does not admit a changed production backend.
#[test]
fn a_changed_image_kernel_or_corpus_must_be_redrilled() {
    let att = attestation(false).unwrap();
    // changed rootfs
    let mut id = prod_id();
    id.rootfs_sha256 = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef0".into();
    assert!(matches!(
        AgentExecGate::admit(Some(&att), &id).unwrap_err(),
        GateRefusal::IdentityMismatch { .. }
    ));
    // changed corpus version
    let mut id2 = prod_id();
    id2.corpus_version = CORPUS_VERSION + 7;
    assert!(matches!(
        AgentExecGate::admit(Some(&att), &id2).unwrap_err(),
        GateRefusal::IdentityMismatch { .. }
    ));
}

/// **A gVisor-only attestation does not admit the Firecracker production backend.** The gate is
/// admitted only on the production backend (the gVisor second backend is drilled separately, CI-P28).
#[test]
fn a_non_production_gate_backend_is_refused() {
    let mut att = attestation(false).unwrap();
    att.gate_backend = Backend::GvisorRunsc;
    let r = AgentExecGate::admit(Some(&att), &prod_id());
    assert_eq!(
        r.unwrap_err(),
        GateRefusal::GateBackendMismatch {
            attested: Backend::GvisorRunsc,
            expected: Backend::FirecrackerMicrovm,
        }
    );
}

/// **The production backend recorded as a residual (not exercised on real silicon) does not admit.**
#[test]
fn a_residual_production_backend_does_not_admit() {
    let mut att = attestation(false).unwrap();
    for b in att.backends.iter_mut() {
        if b.backend == Backend::FirecrackerMicrovm {
            b.exercised = false;
            b.residual_note = Some("recorded but not run on real silicon".into());
        }
    }
    let r = AgentExecGate::admit(Some(&att), &prod_id());
    assert_eq!(
        r.unwrap_err(),
        GateRefusal::ProductionBackendNotExercised {
            backend: Backend::FirecrackerMicrovm,
        }
    );
}

/// **The named residuals are carried IN WRITING in the consumed attestation (the no-floor posture).**
/// There is NO floor on AG-D4; it is a PERMANENT GATE; the M4 re-confirm is AG-P21/P-348; continuous
/// fuzzing + CVE corpus + pre-GA pentest remain ongoing.
#[test]
fn the_no_floor_permanent_gate_residuals_are_carried_in_writing() {
    let att = attestation(false).unwrap();
    assert!(att.residuals.iter().any(|r| r.contains("PERMANENT GATE")));
    assert!(att
        .residuals
        .iter()
        .any(|r| r.contains("P-348") || r.contains("CI-P27")));
    assert!(att
        .residuals
        .iter()
        .any(|r| r.contains("fuzzing") && r.contains("pentest")));
}
