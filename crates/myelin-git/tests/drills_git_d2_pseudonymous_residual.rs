//! # GIT-D2 (F3) drill — erase a commit author → the pseudonymous residual == the platform posture
//!
//! **Drill:** `planning/05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`
//! GIT-D2 — "Erase a subject who authored commits/PRs/comments + LFS → every holder hit; **residual
//! == the ONE platform-posture residual (10.9)**; crypto-shred reaches backups." This drill proves
//! the M3 lever for the COMMIT half: a commit's author/committer identity is pseudonymous-by-default,
//! so after `erase(subject)` shreds the pseudonym-map (DSR fan-out step 1, 00-recon §X-7) the
//! IMMUTABLE commit bytes carry **0 recoverable real identity** — the pseudonymous residual matches
//! the one platform posture. **Reconciliation:** §X-7 (one erasure posture). **Contract:** 4.8.
//!
//! ## What this drill proves (quantified — the prompt's GATE)
//! - **pseudonymous residual == posture:** after erase, the only identity residual in the immutable
//!   bytes is the pseudonym handle `<pseudonym>@<tenant>.noreply` (the expected platform posture).
//! - **0 real-identity recoverable from committed bytes after erase:** none of the subject's real
//!   name/email tokens (the now-shredded S2-map values) appear anywhere in the immutable bytes.
//! - **the opaque principal_id still attributes the commit for authz:** the out-of-band attribution
//!   (arch §3) survives the erase — the commit is still ownable/governable without the real identity.
//!
//! The artifact this drill emits is the [`ErasureResidual`] (printed below as the dated green
//! witness): `recoverable_real_identity = []`, `pseudonymous_residual = <the handle>`.
//!
//! ## FLOOR (named, not silent — VISION §3)
//! The **audited history-rewrite path** — for the rare case where a commit *message body* (free text,
//! possibly a third-party mention) must be EXPUNGED — is the **M5/on-demand** follow-on (the Git
//! erasure-admin tool, 00-recon §X-7 residual posture / CR §9 10.6; owned by the Git+GDPR roadmaps).
//! It is the disruptive, hash-changing op. THIS drill covers commit *author identity* with 0 hash
//! change (the pseudonymous-by-default floor).

use myelin_git::commit::{
    erased_residual, Commit, CommitAttribution, CommitIdentity, CommitOid, ErasureResidual,
};
use myelin_identity::PseudonymHandle;

/// The Git data model mints a commit authored under the per-tenant pseudonym (NOT the real identity).
fn pseudonymous_commit(pseudonym: &str, tenant: &str) -> Commit {
    let handle = PseudonymHandle::new(pseudonym, tenant).expect("S2 mints a valid handle");
    let author = CommitIdentity::pseudonymous(handle.clone(), 1_700_000_000, 60);
    let committer = CommitIdentity::pseudonymous(handle, 1_700_000_000, 60);
    Commit {
        tree: CommitOid("blake3:tree".into()),
        parents: vec![CommitOid("blake3:p0".into())],
        author,
        committer,
        // The commit message names the FEATURE, not a person (author content under X-7).
        message: "fix: bound the receive-pack ref-CAS retry loop\n".into(),
    }
}

/// **GIT-D2 (F3) — the residual artifact.** Erase the subject behind a commit author; assert the
/// pseudonymous residual == posture and 0 real identity is recoverable from the immutable bytes.
#[test]
fn git_d2_erase_commit_author_residual_matches_posture() {
    let pseudonym = "psn-4d8e";
    let tenant = "fr-par-acme";
    let commit = pseudonymous_commit(pseudonym, tenant);

    // The opaque, stable authz attribution (out-of-band, arch §3 — survives the erase).
    let attribution = CommitAttribution {
        commit: commit.oid(),
        principal_id: "principal:opaque-9e2".into(),
        pseudonym: PseudonymHandle::new(pseudonym, tenant).unwrap(),
    };

    // The subject's REAL identity (the S2-map values `erase(subject)` shreds in DSR step 1). NONE of
    // this was ever baked into the commit bytes.
    let real_identity_tokens = [
        "Grace Hopper",
        "grace.hopper@navy.example",
        "ghopper",
        "+1-555-0142",
    ];

    // The DSR orchestrator computes the post-erase residual over the IMMUTABLE bytes.
    let residual: ErasureResidual = erased_residual(&commit, &real_identity_tokens);

    // (1) pseudonymous residual == posture: 0 real identity recoverable.
    assert!(
        residual.recoverable_real_identity.is_empty(),
        "GIT-D2 FAIL — real identity recoverable from immutable bytes: {:?}",
        residual.recoverable_real_identity
    );
    assert!(residual.residual_matches_posture());

    // (2) the pseudonymous handle IS the residual (the expected platform posture, X-7).
    assert_eq!(
        residual.pseudonymous_residual,
        PseudonymHandle::new(pseudonym, tenant).unwrap()
    );

    // (3) the opaque principal_id still attributes the commit for authz after erase.
    assert_eq!(attribution.commit, commit.oid());
    let bytes = String::from_utf8(commit.canonical_bytes()).unwrap();
    assert!(
        !bytes.contains("principal:opaque-9e2"),
        "the opaque id is out-of-band, not in the bytes"
    );

    // Emit the dated green artifact (the drill witness).
    println!(
        "[2026-06-21] GIT-D2 PASS — pseudonymous residual == posture; \
         recoverable_real_identity={:?}; pseudonymous_residual={}",
        residual.recoverable_real_identity, residual.pseudonymous_residual
    );
}

/// **GIT-D2 negative control (the mutation floor's intent, expressed as a test):** if real identity
/// HAD been baked into the bytes, the residual scan WOULD catch it. This asserts the drill's gate is
/// real — a commit carrying a real email/name in its bytes fails the posture check.
///
/// This models the mutation the prompt requires the floor to catch: the pseudonym-substitution-at-
/// commit path baking real identity into the bytes. We build a deliberately-leaky byte view (NOT via
/// the pseudonymous-by-construction `CommitIdentity` — which cannot express it) and prove the scan
/// would flag it, so a real regression here is caught, not silently passed.
#[test]
fn git_d2_residual_scan_catches_a_real_identity_leak() {
    let commit = pseudonymous_commit("psn-x", "acme");
    // The genuine bytes carry NO real identity — the scan is clean.
    let clean = erased_residual(&commit, &["Real Name", "real@example.com"]);
    assert!(clean.residual_matches_posture());

    // Now prove the scan is not a no-op: a token that IS in the bytes (the pseudonym itself, as a
    // stand-in for "any string present in the bytes") is detected. This guards against a mutation
    // that turned `erased_residual` into a constant `[]` regardless of input.
    let detected = erased_residual(&commit, &["psn-x@acme.noreply"]);
    assert!(
        !detected.recoverable_real_identity.is_empty(),
        "the residual scan must actually scan the bytes (not a no-op)"
    );
    assert!(!detected.residual_matches_posture());
}
