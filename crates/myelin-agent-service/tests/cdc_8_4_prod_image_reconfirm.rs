use myelin_agent_service::escape_gate::{AgentExecGate, GateRefusal, ProductionBackendId};
use myelin_ci_sandbox::escape_corpus::{BEGIN_MARKER, END_MARKER};
use myelin_ci_sandbox::{
    parse_console, Backend, BackendRun, EscapeAttestation, CORPUS, CORPUS_VERSION,
};

const PROD_RUNNER_IMAGE_ROOTFS_SHA: &str =
    "7a2bc8ed2c64ed78994971439b00c234b1ce46d247123314d683df7579c77923";

const SHARED_KERNEL_SHA: &str = "467367e6b8e88323dd23dedae3119ade9c9fca6a102a84fc2155e3ef1bec00eb";

const OTHER_IMAGE_ROOTFS_SHA: &str =
    "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef0";

const PROD_IMAGE_ROLE: &str =
    "M4 PROD-IMAGE RE-CONFIRM (AG-P21 / P-348; CI side CI-P27 / P-370): this attestation re-confirms \
     AG-D4 / CI-T1 on the PRODUCTION CI runner image (the config-resolved runner rootfs - dev↔prod is \
     a config swap, MYELIN_REGION=fr-par / prod=Scaleway). The production runner runs on KVM-capable \
     Scaleway Elastic Metal; the prod image is re-drilled there at deploy - that is a NAMED residual, \
     not faked here.";

fn prod_runner_image_id() -> ProductionBackendId {
    ProductionBackendId {
        backend: Backend::FirecrackerMicrovm,
        rootfs_sha256: PROD_RUNNER_IMAGE_ROOTFS_SHA.into(),
        kernel_sha256: SHARED_KERNEL_SHA.into(),
        corpus_version: CORPUS_VERSION,
    }
}

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
    att.residuals.insert(0, PROD_IMAGE_ROLE.to_string());
    Ok(att)
}

#[test]
fn no_untrusted_compute_on_the_prod_image_without_a_green_reconfirm_attestation() {
    let r = AgentExecGate::admit(None, &prod_runner_image_id());
    assert_eq!(r.unwrap_err(), GateRefusal::NoAttestation);
}

#[test]
fn a_red_reconfirm_on_the_prod_image_mints_no_attestation() {
    let minted = prod_image_attestation(PROD_RUNNER_IMAGE_ROOTFS_SHA, true);
    assert!(
        minted.is_err(),
        "a red re-confirm drill on the prod image mints NO green attestation"
    );
    assert_eq!(
        AgentExecGate::admit(None, &prod_runner_image_id()).unwrap_err(),
        GateRefusal::NoAttestation
    );
}

#[test]
fn a_nonzero_escape_count_on_the_prod_image_is_refused() {
    let mut att = prod_image_attestation(PROD_RUNNER_IMAGE_ROOTFS_SHA, false).unwrap();
    att.total_escapes = 3;
    let r = AgentExecGate::admit(Some(&att), &prod_runner_image_id());
    assert_eq!(r.unwrap_err(), GateRefusal::Escapes { total_escapes: 3 });
}

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

#[test]
fn a_different_image_attestation_does_not_admit_the_prod_image() {
    let other_att = prod_image_attestation(OTHER_IMAGE_ROOTFS_SHA, false).unwrap();
    let r = AgentExecGate::admit(Some(&other_att), &prod_runner_image_id());
    assert!(
        matches!(r.unwrap_err(), GateRefusal::IdentityMismatch { .. }),
        "the prod image must be re-drilled - a different-image green does not admit it"
    );
}

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

#[cfg(feature = "integration")]
#[test]
fn the_fabric_gate_on_the_real_prod_image_reconfirm_or_fail_closed() {
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
            let mut changed = id.clone();
            changed.corpus_version = att.corpus_version + 1;
            assert!(matches!(
                AgentExecGate::admit(Some(&att), &changed).unwrap_err(),
                GateRefusal::IdentityMismatch { .. }
            ));
        }
        None => {
            println!(
                "[AG-P21 prod-image re-confirm] no real prod-image attestation artifact - the \
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
