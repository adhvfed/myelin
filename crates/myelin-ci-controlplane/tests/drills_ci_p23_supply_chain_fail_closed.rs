//! # CI-D4 drill — supply-chain fail-closed (CI-P23 / P-366, M4).
//!
//! The whole-system drill (07 D-4 / drill catalogue row CI-D4): a **floating tag** and a
//! **tampered/unsigned component** are injected at run; the **digest-pin + sign-verify-before-use**
//! verifier **fails CLOSED** at run, emitting `ci.supply_chain.verification_failed` for EVERY
//! refusal. The quantified gates:
//!   - **0 un-pinned executions** — every floating-tag reference is refused at run;
//!   - **0 unsigned-component runs** — every unsigned / tampered / un-logged component is refused;
//!   - **the `ci.supply_chain.verification_failed` audit event** is built for every refusal.
//!
//! This is the CI-side failure-injection scenario over the [`SupplyChainVerifier`] (arch 05 HP-4):
//! the adversary forces (a) a floating tag past the plan-time resolver, (b) a tampered component
//! (digest mismatch), (c) an unsigned component, (d) an un-logged signature — every one is refused
//! fail-closed AND yields an audit draft; only the digest-pinned + signed + transparency-logged
//! shape is admitted.
//!
//! Emits a dated green artifact line on pass.

use myelin_ci_controlplane::supply_chain::{
    BuildIdentity, KeylessSignature, SbomFormat, SupplyChainVerifier, VerificationFailure,
};
use myelin_ci_sandbox::events::CI_SUPPLY_CHAIN_VERIFICATION_FAILED;
use myelin_events::ArtifactRef;

const PINNED_BUILD: &str = "registry.example/build@sha256:abc123def456";
const PINNED_TEST: &str = "registry.example/test@sha256:ffeeddccbbaa";
const RUN_ID: &str = "run-d4-0001";

fn identity() -> BuildIdentity {
    BuildIdentity::new(RUN_ID, "ci-runner@acme")
}

/// The `@<algo>:<hex>` digest half of a pinned reference (the component's content digest).
fn digest_of(reference: &str) -> String {
    reference
        .rsplit_once('@')
        .map(|(_, d)| d.to_string())
        .unwrap()
}

/// **CI-D4: supply-chain fail-closed — 0 un-pinned/unsigned executions + the audit event.**
///
/// A verifier with one legitimately signed + logged component (`PINNED_BUILD`). The adversary then
/// injects the full attack matrix; every attack is refused fail-closed and emits the audit event,
/// and the ONE honest component still passes.
#[test]
fn ci_d4_supply_chain_fails_closed_with_zero_unpinned_unsigned_executions() {
    let mut verifier = SupplyChainVerifier::new();

    // The legitimate supply step: PINNED_BUILD is signed by the run's keyless build identity and
    // sealed into the Rekor transparency log (the only admitted shape).
    let honest_sig = KeylessSignature::sign(digest_of(PINNED_BUILD), &identity());
    verifier
        .record_signature(&honest_sig)
        .expect("an honest signature is recorded into the transparency log");

    // ---- the failure-injection attack matrix ----
    // Each entry: (label, reference, signature, expected machine reason token).
    let tampered = {
        // A signature whose bytes do not honestly attest its claimed digest+identity (a forgery).
        let mut s = KeylessSignature::sign(digest_of(PINNED_TEST), &identity());
        s.signature = "blake3:00000000deadbeef".into();
        s
    };
    // An honest signature that was NEVER appended to the transparency log (out-of-band).
    let unlogged = KeylessSignature::sign(digest_of(PINNED_TEST), &identity());

    let attacks: Vec<(&str, &str, Option<&KeylessSignature>, &str)> = vec![
        // (a) a FLOATING TAG forced past the plan-time resolver → refused at run.
        ("floating-tag", "alpine:3", None, "floating_tag"),
        (
            "floating-tag-latest",
            "registry/x:latest",
            None,
            "floating_tag",
        ),
        // (b) an UNSIGNED digest-pinned component.
        ("unsigned", PINNED_TEST, None, "unsigned"),
        // (c) a TAMPERED/forged signature (verify-before-use fails).
        (
            "tampered",
            PINNED_TEST,
            Some(&tampered),
            "signature_mismatch",
        ),
        // (d) an honest but UN-LOGGED signature (not in the transparency log).
        (
            "unlogged",
            PINNED_TEST,
            Some(&unlogged),
            "not_in_transparency_log",
        ),
    ];

    let mut unpinned_executions = 0usize;
    let mut unsigned_runs = 0usize;
    let mut audit_events = 0usize;

    for (label, reference, sig, expected_reason) in &attacks {
        match verifier.verify_component(reference, *sig) {
            Ok(()) => panic!("attack `{label}` ({reference}) was ADMITTED — fail-closed violated"),
            Err(failure) => {
                assert_eq!(
                    failure.reason_token(),
                    *expected_reason,
                    "attack `{label}` refused with the expected reason"
                );
                // Tally the two quantified gates.
                match &failure {
                    VerificationFailure::FloatingTag { .. } => unpinned_executions += 1,
                    VerificationFailure::Unsigned { .. }
                    | VerificationFailure::SignatureMismatch { .. }
                    | VerificationFailure::NotInTransparencyLog { .. } => unsigned_runs += 1,
                }
                // The audit-critical fail-closed proof is built for EVERY refusal.
                let draft = verifier.refusal_event(RUN_ID, &failure);
                assert_eq!(draft.type_.0, CI_SUPPLY_CHAIN_VERIFICATION_FAILED);
                assert_eq!(draft.aggregate.0, format!("ci/run/{RUN_ID}"));
                assert_eq!(draft.payload["reason"], *expected_reason);
                assert!(!draft.contains_personal_data, "the audit event is PII-free");
                audit_events += 1;
            }
        }
    }

    // The ONE admitted shape (digest-pinned + signed + transparency-logged) still passes — the gate
    // is fail-closed, not closed-to-everything.
    assert!(
        verifier
            .verify_component(PINNED_BUILD, Some(&honest_sig))
            .is_ok(),
        "the honest digest-pinned + signed + logged component is admitted"
    );

    // The quantified CI-D4 gates: every floating tag and every unsigned/tampered/un-logged
    // component was refused (none executed), and every refusal emitted the audit event.
    let attempted_floating = attacks.iter().filter(|a| a.3 == "floating_tag").count();
    let attempted_unsigned = attacks.len() - attempted_floating;
    assert_eq!(
        unpinned_executions, attempted_floating,
        "every floating-tag attempt was refused → 0 un-pinned executions admitted"
    );
    assert_eq!(
        unsigned_runs, attempted_unsigned,
        "every unsigned/tampered/un-logged attempt was refused → 0 unsigned-component runs admitted"
    );
    assert_eq!(
        audit_events,
        attacks.len(),
        "every refusal emitted ci.supply_chain.verification_failed (0 silent passes)"
    );

    println!(
        "GREEN 2026-06-23 CI-D4 supply-chain fail-closed: {attempted_floating} floating-tag + \
         {attempted_unsigned} unsigned/tampered/un-logged attacks ALL refused at run; \
         0 un-pinned executions, 0 unsigned-component runs; {audit_events} \
         ci.supply_chain.verification_failed audit events emitted; 1 honest component admitted."
    );
}

/// **CI-D4 (the attestation half): a verified run produces a SIGNED SLSA L1–L2 provenance + an SBOM,
/// both sealed/inventoried from DIGEST-PINNED inputs only.** A produced artifact's provenance is
/// auditable (which run, which snapshot, which input digests) and is itself a transparency-log leaf;
/// a floating input is refused fail-closed (the same control).
#[test]
fn ci_d4_a_verified_run_attests_provenance_and_sbom() {
    let mut verifier = SupplyChainVerifier::new();
    let snapshot = ArtifactRef("myelin://acme/ci/snapshot/blake3:cafef00d".into());

    let before = verifier.rekor().tree_size();
    let (provenance, sbom) = verifier
        .attest(
            "blake3:produced-artifact",
            &identity(),
            &snapshot,
            &[PINNED_TEST.to_string(), PINNED_BUILD.to_string()],
            SbomFormat::CycloneDx,
        )
        .expect("a pinned-input attestation succeeds");

    // The provenance is a signed, honest, auditable attestation, sealed into the transparency log.
    assert!(
        provenance.sign().verifies(),
        "the provenance is honestly signed"
    );
    assert_eq!(provenance.definition_snapshot, snapshot);
    assert_eq!(
        verifier.rekor().tree_size(),
        before + 1,
        "the provenance is sealed into the Rekor transparency log"
    );
    // The SBOM inventories the digest-pinned components.
    assert_eq!(sbom.format, SbomFormat::CycloneDx);
    assert_eq!(
        sbom.components.len(),
        2,
        "two digest-pinned components inventoried"
    );
    assert!(
        sbom.components.iter().all(|c| c.contains('@')),
        "every SBOM component is digest-pinned"
    );

    // A floating INPUT is refused fail-closed (the same control extends to attestation inputs).
    let floating = verifier.attest(
        "blake3:x",
        &identity(),
        &snapshot,
        &["ubuntu:22.04".to_string()],
        SbomFormat::Spdx,
    );
    assert!(
        matches!(floating, Err(VerificationFailure::FloatingTag { .. })),
        "a floating attestation input is refused fail-closed"
    );

    println!(
        "GREEN 2026-06-23 CI-D4 SLSA/SBOM: 1 signed SLSA L1-L2 provenance + 1 CycloneDX SBOM over \
         2 digest-pinned inputs (provenance sealed into the Rekor transparency log); \
         floating input refused. FLOOR: SLSA L3+ (hermetic) is demand-triggered (CI-M5)."
    );
}
