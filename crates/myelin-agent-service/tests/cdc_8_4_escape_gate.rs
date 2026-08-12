use myelin_agent_service::escape_gate::{AgentExecGate, GateRefusal, ProductionBackendId};
use myelin_ci_sandbox::escape_corpus::{BEGIN_MARKER, END_MARKER};
use myelin_ci_sandbox::{
    parse_console, Backend, BackendRun, EscapeAttestation, CORPUS, CORPUS_VERSION,
};

fn prod_id() -> ProductionBackendId {
    ProductionBackendId {
        backend: Backend::FirecrackerMicrovm,
        rootfs_sha256: "7a2bc8ed2c64ed78994971439b00c234b1ce46d247123314d683df7579c77923".into(),
        kernel_sha256: "467367e6b8e88323dd23dedae3119ade9c9fca6a102a84fc2155e3ef1bec00eb".into(),
        corpus_version: CORPUS_VERSION,
    }
}

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

#[test]
fn fabric_exec_is_fail_closed_without_a_green_attestation() {
    let r = AgentExecGate::admit(None, &prod_id());
    assert_eq!(r.unwrap_err(), GateRefusal::NoAttestation);
}

#[test]
fn a_red_drill_mints_no_attestation_and_the_gate_stays_closed() {
    let minted = attestation(true);
    assert!(minted.is_err(), "a red drill mints NO green attestation");
    assert_eq!(
        AgentExecGate::admit(None, &prod_id()).unwrap_err(),
        GateRefusal::NoAttestation
    );
}

#[test]
fn a_nonzero_escape_count_is_refused_even_if_the_artifact_claims_green() {
    let mut att = attestation(false).unwrap();
    att.total_escapes = 3;
    let r = AgentExecGate::admit(Some(&att), &prod_id());
    assert_eq!(r.unwrap_err(), GateRefusal::Escapes { total_escapes: 3 });
}

#[test]
fn a_green_attestation_for_the_production_backend_admits() {
    let att = attestation(false).unwrap();
    let gate = AgentExecGate::admit(Some(&att), &prod_id())
        .expect("a green, identity-matched attestation admits");
    assert_eq!(gate.backend_id().backend, Backend::FirecrackerMicrovm);
    assert!(gate.open_line().starts_with("[AG-D4 GATE OPEN]"));
    assert!(gate.open_line().contains("ZERO escapes"));
}

#[test]
fn the_gate_consumes_the_real_json_artifact_form() {
    let json = attestation(false)
        .unwrap()
        .to_json()
        .expect("the test attestation serializes");
    let gate =
        AgentExecGate::admit_from_json(&json, &prod_id()).expect("the green JSON artifact admits");
    assert_eq!(gate.backend_id().corpus_version, CORPUS_VERSION);
}

#[test]
fn a_changed_image_kernel_or_corpus_must_be_redrilled() {
    let att = attestation(false).unwrap();
    let mut id = prod_id();
    id.rootfs_sha256 = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef0".into();
    assert!(matches!(
        AgentExecGate::admit(Some(&att), &id).unwrap_err(),
        GateRefusal::IdentityMismatch { .. }
    ));
    let mut id2 = prod_id();
    id2.corpus_version = CORPUS_VERSION + 7;
    assert!(matches!(
        AgentExecGate::admit(Some(&att), &id2).unwrap_err(),
        GateRefusal::IdentityMismatch { .. }
    ));
}

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
