//! # `integration_escape_gate` — the Fabric gate consumes the REAL P-239 attestation (AG-P17 → P-229)
//!
//! **Gated behind the `integration` feature** so the DEFAULT `cargo test --workspace` never depends
//! on a VM-produced artifact. This test consumes the REAL green [`EscapeAttestation`] the CI-side
//! escape drill wrote on a real Firecracker microVM (CI-P5 → P-239,
//! `target/ag-d4-attestation/<date>.json`) and proves the Fabric exec gate ADMITS untrusted compute
//! against it — the live, end-to-end half of the drill-as-gate (contract 8.4).
//!
//! Run with: `cargo test -p myelin-agent-service --features integration --test integration_escape_gate
//! -- --nocapture`.
//!
//! The default (VM-free) consumer contract is pinned in `tests/cdc_8_4_escape_gate.rs`; this test adds
//! the REAL-artifact leg. If the artifact is absent (the drill has not run on this host), the test
//! proves the gate is fail-closed on the absent artifact (no green attestation ⇒ no untrusted compute)
//! and reports it — it never fakes a green.
#![cfg(feature = "integration")]

use myelin_agent_service::escape_gate::{AgentExecGate, GateRefusal, ProductionBackendId};
use myelin_ci_sandbox::EscapeAttestation;
use std::path::PathBuf;

/// The directory the P-239 drill writes the dated attestation artifact into (workspace
/// `target/ag-d4-attestation/`).
fn attestation_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("ag-d4-attestation")
}

/// Load the most recent attestation artifact JSON, if any was produced by the real drill.
fn latest_attestation() -> Option<(PathBuf, EscapeAttestation)> {
    let dir = attestation_dir();
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
        .collect();
    entries.sort();
    let path = entries.pop()?;
    let json = std::fs::read_to_string(&path).ok()?;
    let att: EscapeAttestation = serde_json::from_str(&json).ok()?;
    Some((path, att))
}

#[test]
fn the_fabric_gate_admits_on_the_real_p239_attestation_or_is_fail_closed_without_one() {
    match latest_attestation() {
        Some((path, att)) => {
            println!(
                "[AG-D4] consuming REAL attestation artifact: {}",
                path.display()
            );
            // The gate's required production identity is taken FROM the attestation the real drill
            // produced (the runner's pinned images / corpus). Admitting against this identity proves
            // the Fabric exec path is gated on the REAL green attestation, end-to-end.
            let id = ProductionBackendId {
                backend: att.gate_backend,
                rootfs_sha256: att.rootfs_sha256.clone(),
                kernel_sha256: att.kernel_sha256.clone(),
                corpus_version: att.corpus_version,
            };
            let gate = AgentExecGate::admit(Some(&att), &id)
                .expect("the REAL green attestation must admit the Fabric exec gate");
            println!("{}", gate.open_line());
            assert_eq!(att.total_escapes, 0, "the real attestation is ZERO escapes");

            // The permanent gate: a changed identity (a different corpus version) does NOT admit the
            // real attestation — it must be re-drilled.
            let mut changed = id.clone();
            changed.corpus_version = att.corpus_version + 1;
            assert!(matches!(
                AgentExecGate::admit(Some(&att), &changed).unwrap_err(),
                GateRefusal::IdentityMismatch { .. }
            ));
        }
        None => {
            // No real artifact on this host — prove the gate is fail-closed (never a faked green).
            println!(
                "[AG-D4] no real attestation artifact found at {} — the drill has not run on this \
                 host; proving the gate is FAIL-CLOSED (no green attestation ⇒ no untrusted compute)",
                attestation_dir().display()
            );
            let id = ProductionBackendId {
                backend: myelin_ci_sandbox::Backend::FirecrackerMicrovm,
                rootfs_sha256: "absent".into(),
                kernel_sha256: "absent".into(),
                corpus_version: myelin_ci_sandbox::CORPUS_VERSION,
            };
            assert_eq!(
                AgentExecGate::admit(None, &id).unwrap_err(),
                GateRefusal::NoAttestation
            );
        }
    }
}
