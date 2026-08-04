use myelin_ci_controlplane::events::CI_DURABLE_TOKENS;
use myelin_ci_controlplane::supply_chain::{
    BuildIdentity, KeylessSignature, RekorLog, SupplyChainVerifier,
};
use myelin_ci_sandbox::events::CI_SUPPLY_CHAIN_VERIFICATION_FAILED;
use myelin_events::{validate_event_type, DataRole, Visibility};

const PINNED: &str = "registry.example/build@sha256:abc123def4560000000000000000000000000000000000000000000000000000";

fn identity() -> BuildIdentity {
    BuildIdentity::new("run-0001", "ci-runner@acme")
}

fn digest_of(reference: &str) -> String {
    reference
        .rsplit_once('@')
        .map(|(_, d)| d.to_string())
        .unwrap()
}

#[test]
fn cdc_2_2_the_refusal_is_a_well_formed_outbox_draft() {
    let verifier = SupplyChainVerifier::new();
    let failure = verifier.verify_component("alpine:3", None).unwrap_err();
    let draft = verifier.refusal_event("run-0001", &failure);

    assert_eq!(draft.type_.0, CI_SUPPLY_CHAIN_VERIFICATION_FAILED);
    assert!(
        validate_event_type(&draft.type_.0).is_ok(),
        "the token is §6.1-grammatical"
    );
    assert!(
        CI_DURABLE_TOKENS.contains(&draft.type_.0.as_str()),
        "verification_failed is a registered DURABLE ci.* token (rides the outbox)"
    );
    assert_eq!(draft.aggregate.0, "ci/run/run-0001");
    assert_eq!(draft.subject.0, "ci/run/run-0001");
    assert_eq!(draft.data_role, DataRole::Controller);
    assert_eq!(draft.visibility, Visibility::Internal);
    assert!(!draft.contains_personal_data);
    assert!(draft.pii_key_ref.is_none());
}

#[test]
fn cdc_10_6_the_rekor_log_is_the_rfc6962_merkle_pattern() {
    let mut log = RekorLog::new();
    let empty_root = log.root();
    assert!(
        empty_root.starts_with("blake3:"),
        "the root is a blake3 multihash"
    );
    assert_eq!(log.tree_size(), 0);

    let s1 = KeylessSignature::sign(digest_of(PINNED), &identity());
    log.append(&s1);
    let root1 = log.root();
    assert_ne!(
        root1, empty_root,
        "the root changes on append (tamper-evident)"
    );
    assert!(root1.starts_with("blake3:"));
    assert!(log.contains(&s1), "the appended entry is recorded");
    let other = KeylessSignature::sign("sha256:never", &identity());
    assert!(!log.contains(&other), "an un-appended entry is absent");

    let mut log2 = RekorLog::new();
    log2.append(&s1);
    assert_eq!(log.root(), log2.root(), "the Merkle root is deterministic");
}

#[test]
fn cdc_4_7_the_signature_is_bound_to_the_oidc_build_identity() {
    let digest = digest_of(PINNED);
    let a = BuildIdentity::new("run-0001", "ci-runner@acme");
    let b = BuildIdentity::new("run-0002", "ci-runner@acme");

    let sig_a = KeylessSignature::sign(&digest, &a);
    assert!(
        sig_a.verifies(),
        "the signature verifies under its own identity"
    );

    let mut forged = sig_a.clone();
    forged.identity = b.clone();
    assert!(
        !forged.verifies(),
        "a signature is not portable to a different build identity (4.7 - identity-bound)"
    );
    let sig_b = KeylessSignature::sign(&digest, &b);
    assert_ne!(
        sig_a.signature, sig_b.signature,
        "different build identities → different signatures over the same digest"
    );
}
