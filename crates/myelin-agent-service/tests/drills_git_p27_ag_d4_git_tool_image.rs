use myelin_agent::EffectKind;
use myelin_agent_service::escape_gate::{AgentExecGate, GateRefusal, ProductionBackendId};
use myelin_agent_service::{git_scip_index_tool_def, git_tool_defs};
use myelin_ci_sandbox::escape_corpus::{BEGIN_MARKER, END_MARKER};
use myelin_ci_sandbox::{
    parse_console, Backend, BackendRun, EscapeAttestation, CORPUS, CORPUS_VERSION,
};

const GIT_TOOL_IMAGE_ROOTFS_SHA: &str =
    "9f1c0a44e7b3d2516c8af0e1d4b6790235ac8d11ff62b4e0a7d3c91b8e2f50ab";

const SHARED_KERNEL_SHA: &str = "467367e6b8e88323dd23dedae3119ade9c9fca6a102a84fc2155e3ef1bec00eb";

const BASE_RUNNER_ROOTFS_SHA: &str =
    "7a2bc8ed2c64ed78994971439b00c234b1ce46d247123314d683df7579c77923";

fn git_tool_image_id() -> ProductionBackendId {
    ProductionBackendId {
        backend: Backend::FirecrackerMicrovm,
        rootfs_sha256: GIT_TOOL_IMAGE_ROOTFS_SHA.into(),
        kernel_sha256: SHARED_KERNEL_SHA.into(),
        corpus_version: CORPUS_VERSION,
    }
}

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

#[test]
fn no_code_executing_git_tool_runs_without_a_green_attestation_for_the_git_tool_image() {
    let r = AgentExecGate::admit(None, &git_tool_image_id());
    assert_eq!(r.unwrap_err(), GateRefusal::NoAttestation);
}

#[test]
fn a_red_drill_on_the_git_tool_image_mints_no_attestation() {
    let minted = attestation_for(GIT_TOOL_IMAGE_ROOTFS_SHA, true);
    assert!(
        minted.is_err(),
        "a red drill on the git tool image mints NO green attestation"
    );
    assert_eq!(
        AgentExecGate::admit(None, &git_tool_image_id()).unwrap_err(),
        GateRefusal::NoAttestation
    );
}

#[test]
fn a_nonzero_escape_count_on_the_git_tool_image_is_refused() {
    let mut att = attestation_for(GIT_TOOL_IMAGE_ROOTFS_SHA, false).unwrap();
    att.total_escapes = 2;
    let r = AgentExecGate::admit(Some(&att), &git_tool_image_id());
    assert_eq!(r.unwrap_err(), GateRefusal::Escapes { total_escapes: 2 });
}

#[test]
fn a_green_git_tool_image_attestation_admits_the_code_executing_git_tools() {
    let att = attestation_for(GIT_TOOL_IMAGE_ROOTFS_SHA, false).unwrap();
    let gate = AgentExecGate::admit(Some(&att), &git_tool_image_id())
        .expect("a green, identity-matched git-tool-image attestation admits");
    assert_eq!(gate.backend_id().rootfs_sha256, GIT_TOOL_IMAGE_ROOTFS_SHA);
    assert!(gate.open_line().starts_with("[AG-D4 GATE OPEN]"));
    assert!(gate.open_line().contains("ZERO escapes"));
}

#[test]
fn a_base_image_attestation_does_not_admit_the_git_tool_image() {
    let base_att = attestation_for(BASE_RUNNER_ROOTFS_SHA, false).unwrap();
    let r = AgentExecGate::admit(Some(&base_att), &git_tool_image_id());
    assert!(
        matches!(r.unwrap_err(), GateRefusal::IdentityMismatch { .. }),
        "the git tool image must be re-drilled - a base-image green does not admit it"
    );
}

#[test]
fn a_changed_kernel_or_corpus_on_the_git_tool_image_must_be_redrilled() {
    let att = attestation_for(GIT_TOOL_IMAGE_ROOTFS_SHA, false).unwrap();
    let mut id = git_tool_image_id();
    id.corpus_version = CORPUS_VERSION + 11;
    assert!(matches!(
        AgentExecGate::admit(Some(&att), &id).unwrap_err(),
        GateRefusal::IdentityMismatch { .. }
    ));
}

#[test]
fn the_code_executing_git_tools_are_the_two_section_7_tools() {
    let defs = git_tool_defs();
    let scip = git_scip_index_tool_def();
    assert_eq!(
        scip.effect_kind,
        EffectKind::Compute,
        "SCIP indexing rides the bare sandbox"
    );
    assert!(defs
        .iter()
        .any(|d| d.name.0 == "scip_index" && d.effect_kind == EffectKind::Compute));
    let hr = defs
        .iter()
        .find(|d| d.name.0 == "history_rewrite")
        .expect("history_rewrite registered");
    assert_eq!(hr.effect_kind, EffectKind::Mutate);
    assert!(
        hr.requires_approval,
        "history-rewrite is HITL-gated (the audited erasure-admin op)"
    );
    let compute_count = defs
        .iter()
        .filter(|d| d.effect_kind == EffectKind::Compute)
        .count();
    assert_eq!(
        compute_count, 1,
        "only SCIP indexing reaches the bare sandbox among git tools"
    );
}

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
                "[GIT-P27 AG-D4] no real attestation artifact - the git tool image escape drill has \
                 not run on this host; proving FAIL-CLOSED (no green ⇒ no code-executing git tool)"
            );
            assert_eq!(
                AgentExecGate::admit(None, &git_tool_image_id()).unwrap_err(),
                GateRefusal::NoAttestation
            );
        }
    }
}
