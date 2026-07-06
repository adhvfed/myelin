//! # The CDC pair for the CI supply-chain verifier — contracts 2.2 / 10.6 / 4.7 consumed
//! (CI-P23 / P-366, M4, drill CI-D4).
//!
//! **Contracts CONSUMED (this test pins CI's CONSUMER side of each frozen shape):**
//! - **2.2** `OutboxTx::emit(draft, cause)` — the ONLY emit path. CI's supply-chain refusal builds a
//!   `ci.supply_chain.verification_failed` [`EventDraft`] (the frozen envelope shape), to be emitted
//!   via the outbox in the SAME tx as the refusal (no `publish_now`). This test pins the draft is a
//!   well-formed `EventDraft`: a registered durable `ci.supply_chain.*` token, the `ci/run/<run_id>`
//!   aggregate (arch 03 §1.4), PII-free (references-not-payloads).
//! - **10.6** the tamper-evident audit log / CT-Merkle pattern — CI's sigstore **Rekor** transparency
//!   log is the SAME RFC 6962 BLAKE3 Merkle structure (a deterministic root over leaf digests + the
//!   "is this entry recorded" inclusion check). This test pins CI builds the same structure (one
//!   Merkle convention platform-wide), not a second transparency mechanism.
//! - **4.7** OIDC short-lived audience-scoped credentials over static keys — the **build identity**.
//!   The keyless signature binds to a [`BuildIdentity`] (an ephemeral workload identity scoped to the
//!   run), never a long-lived key. This test pins the signature is identity-bound (a different
//!   identity → a different signature → verify-before-use fails).
//!
//! ## The seam this pins (two sides per row)
//! - The **PROVIDER** side is the platform that OWNS each frozen shape: the Bus owns the `EventDraft`
//!   envelope (2.2), GDPR/Audit owns the RFC 6962 CT-Merkle transparency structure (10.6), Identity
//!   owns the OIDC short-lived audience-scoped credential (4.7).
//! - The **CONSUMER** side is the CI supply-chain verifier (CI-P23): it builds a refusal `EventDraft`
//!   to the FROZEN envelope (it does NOT re-define it), builds CI's Rekor log to the SAME RFC 6962
//!   BLAKE3 structure (it does NOT invent a second transparency mechanism), and binds its keyless
//!   signature to the OIDC build identity (it does NOT mint a long-lived signing key).
//!
//! This test pins the CONSUMER (CI) conforms to each PROVIDER-owned frozen shape.

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

// ───────────────────────── contract 2.2 — the verification_failed draft ─────────────────────────

/// **2.2: the supply-chain refusal builds a well-formed `ci.supply_chain.verification_failed`
/// `EventDraft` (the frozen envelope shape) — a registered DURABLE token, the `ci/run/<run_id>`
/// aggregate (arch 03 §1.4), Controller/Internal, PII-free.** The ONLY emit path is the outbox.
#[test]
fn cdc_2_2_the_refusal_is_a_well_formed_outbox_draft() {
    let verifier = SupplyChainVerifier::new();
    let failure = verifier.verify_component("alpine:3", None).unwrap_err();
    let draft = verifier.refusal_event("run-0001", &failure);

    // The token is the frozen `ci.supply_chain.verification_failed`, §6.1-grammatical, and a
    // registered DURABLE token (it rides the durable bus via the outbox).
    assert_eq!(draft.type_.0, CI_SUPPLY_CHAIN_VERIFICATION_FAILED);
    assert!(
        validate_event_type(&draft.type_.0).is_ok(),
        "the token is §6.1-grammatical"
    );
    assert!(
        CI_DURABLE_TOKENS.contains(&draft.type_.0.as_str()),
        "verification_failed is a registered DURABLE ci.* token (rides the outbox)"
    );
    // The aggregate is `ci/run/<run_id>` (arch 03 §1.4 — the run audit aggregate).
    assert_eq!(draft.aggregate.0, "ci/run/run-0001");
    assert_eq!(draft.subject.0, "ci/run/run-0001");
    // Controller-role, internal-visibility, PII-free (references-not-payloads).
    assert_eq!(draft.data_role, DataRole::Controller);
    assert_eq!(draft.visibility, Visibility::Internal);
    assert!(!draft.contains_personal_data);
    assert!(draft.pii_key_ref.is_none());
}

// ───────────────────────── contract 10.6 — the CT-Merkle pattern ─────────────────────────

/// **10.6: CI's Rekor transparency log is the RFC 6962 BLAKE3 Merkle structure — a deterministic
/// `blake3:<hex>` root over leaf digests that CHANGES on append, plus the inclusion check.** The
/// SAME structure GDPR/Audit's tamper-evident audit log builds (one Merkle convention; no second
/// transparency mechanism).
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
    // The inclusion check: the recorded entry is present; an un-recorded one is absent.
    assert!(log.contains(&s1), "the appended entry is recorded");
    let other = KeylessSignature::sign("sha256:never", &identity());
    assert!(!log.contains(&other), "an un-appended entry is absent");

    // Deterministic: a second log with the SAME append yields the SAME root.
    let mut log2 = RekorLog::new();
    log2.append(&s1);
    assert_eq!(log.root(), log2.root(), "the Merkle root is deterministic");
}

// ───────────────────────── contract 4.7 — the keyless OIDC build identity ─────────────────────────

/// **4.7: the keyless signature is bound to the OIDC build IDENTITY (no long-lived key) — a
/// signature minted for one identity does NOT verify for another.** The ephemeral, audience-scoped
/// build identity is the trust anchor, not a static signing key.
#[test]
fn cdc_4_7_the_signature_is_bound_to_the_oidc_build_identity() {
    let digest = digest_of(PINNED);
    let a = BuildIdentity::new("run-0001", "ci-runner@acme");
    let b = BuildIdentity::new("run-0002", "ci-runner@acme"); // a different run (different audience)

    let sig_a = KeylessSignature::sign(&digest, &a);
    assert!(
        sig_a.verifies(),
        "the signature verifies under its own identity"
    );

    // Forge: keep sig_a's bytes but swap in identity `b`. verify-before-use must reject (the bytes
    // were taken over identity `a`, so they do not honestly attest under `b`).
    let mut forged = sig_a.clone();
    forged.identity = b.clone();
    assert!(
        !forged.verifies(),
        "a signature is not portable to a different build identity (4.7 — identity-bound)"
    );
    // A genuinely fresh signature under `b` is distinct from the one under `a` (identity matters).
    let sig_b = KeylessSignature::sign(&digest, &b);
    assert_ne!(
        sig_a.signature, sig_b.signature,
        "different build identities → different signatures over the same digest"
    );
}
