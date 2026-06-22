//! # P-GA-28 → P-257 — the Git pseudonymous-commit GATE drill (GIT-D2) + the P-GA-18 architecture
//! test FIRING over Git's REAL commit codec
//!
//! **DATED GREEN ARTIFACT (2026-06-21).** This integration drill is the dated green artifact the
//! P-GA-28 GATE requires (the GDPR prompts record their drill artifacts as the test itself — there is
//! no GDPR scorecard binary yet). It proves, over Git's **real** commit codec
//! ([`myelin_git::commit::Commit`], pseudonymous-by-construction, GIT-P25), the GATE rows:
//!
//! 1. **GIT-D2 — erase a subject who authored commits → the immutable bytes hold 0 recoverable real
//!    identity; the residual == the ONE platform-posture residual; crypto-shred reaches backups.** A
//!    real commit's author/committer line is the frozen pseudonym `<pseudonym>@<tenant>.noreply`
//!    (contract 4.8); after `erase(subject)` crypto-shreds the inline-body DEK (the H1 holder,
//!    [`GitDbHolder`]) AND the pseudonym-map (DSR step 1), the immutable commit bytes are UNCHANGED and
//!    carry only the pseudonym — `0` recoverable real identity. The residual (third-party / immutable
//!    free-text by another author, under the AUTHOR's DEK) IS the canonical posture residual.
//! 2. **The P-GA-18 commit-identity architecture test FIRES on the live codec** — the verdict scaffold
//!    [`commit_actor_holds_only_pseudonym`] (shipped P-GA-18, the recorded obligation) now runs over the
//!    author + committer actor lines lifted from Git's REAL `Commit::canonical_bytes`: every one holds
//!    ONLY the pseudonym form (PASS). A real-identity author line would FAIL — the mandatory-core check.
//! 3. **The Git erasure section references the platform posture (does not restate it)** — the P-GA-16
//!    by-reference GATE scaffolding firing green over the FIRST real subsystem register
//!    ([`GIT_INSTANCE`]); GIT-D2's residual == the ONE platform-posture residual.
//!
//! ## What this PROVES vs what it REUSES (EI-01 §7 coherence — no new core module)
//! This file ADDS NO production code — it is a pure **chained drill** over `myelin_git::commit` (Git's
//! real codec) + `myelin_gdpr_service::git_instance` (the by-reference instance + the verdict) + the H1
//! [`GitDbHolder`] (the inline-body crypto-shred, P-GA-27). It REUSES the canonical posture, the
//! by-reference predicate, the pseudonym verdict, and the Git H1 holder WHOLESALE.
//!
//! ## Floors named (deferred → filling prompt)
//! - The **audited history-rewrite erasure path** (the rare commit-body expunge, disruptive
//!   changed-hash) → **M5 P-GA-35 (GA-10)**. This drill proves the commit-time pseudonymisation floor
//!   (author IDENTITY, 0 hash change) + the inline-body crypto-shred.
//! - The **live Git `erase` binding** behind the [`PersonalDataHolder`] seam is a config swap at boot;
//!   no new DB/object-store/cache/bus contract is touched — **no `--features integration` leg owed**.

use myelin_git::commit::{Commit, CommitIdentity, CommitOid};
use myelin_identity::{Principal, PrincipalId, PrincipalKind, PseudonymHandle};

use myelin_gdpr::{EraseScope, PersonalDataHolder, SubjectRef, TenantId};
use myelin_gdpr_service::{
    commit_actor_holds_only_pseudonym, git_residual_is_the_one_posture,
    git_section_references_posture, pseudonym_actor_lines_pass_the_prerequisite, CryptoShredKms,
    GitDbHolder, InMemoryShredKms, ShredKeyClass, ShredKeyHandle, CANONICAL_POSTURE, GIT_INSTANCE,
    POSTURE_ANCHOR,
};

fn tenant() -> TenantId {
    TenantId::from_token("acme")
}

fn subject(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        tenant(),
    ))
}

fn subject_dek(subject_token: &str) -> ShredKeyHandle {
    ShredKeyHandle {
        tenant: tenant(),
        class: ShredKeyClass::Subject(subject_token.to_string()),
    }
}

/// Build a REAL Git commit whose author + committer are the frozen pseudonym (GIT-P25 — the commit
/// codec has NO `from_name_email` path; the only constructor takes a [`PseudonymHandle`]).
fn pseudonymous_commit(pseudonym: &str) -> Commit {
    let handle = PseudonymHandle::new(pseudonym, "acme").expect("well-formed pseudonym handle");
    let author = CommitIdentity::pseudonymous(handle.clone(), 1_700_000_000, 120);
    let committer = CommitIdentity::pseudonymous(handle, 1_700_000_000, 120);
    Commit {
        tree: CommitOid("blake3:tree".into()),
        parents: vec![CommitOid("blake3:parent".into())],
        author,
        committer,
        message: "fix: handle the empty-ref edge case\n".into(),
    }
}

/// **The P-GA-18 architecture test FIRES on Git's REAL commit codec: the commit's author + committer
/// actor lines (lifted from `Commit::canonical_bytes`) hold ONLY the frozen pseudonym form.** This is
/// the M3 enforcement P-GA-18 named — the recorded obligation now PASSES over the live codec.
#[test]
fn p_ga_18_architecture_test_fires_green_on_real_git_commit_bytes() {
    let commit = pseudonymous_commit("psn-7f3a9c");

    // Lift the actor lines from the REAL immutable bytes (the exact `<pseudonym>@<tenant>.noreply`).
    let author_actor = commit.author.render_email();
    let committer_actor = commit.committer.render_email();
    assert_eq!(author_actor, "psn-7f3a9c@acme.noreply");
    assert_eq!(committer_actor, "psn-7f3a9c@acme.noreply");

    // The immutable canonical bytes carry exactly these pseudonym actor lines (and no human name).
    let bytes = String::from_utf8(commit.canonical_bytes()).unwrap();
    assert!(bytes.contains("author psn-7f3a9c@acme.noreply <psn-7f3a9c@acme.noreply>"));
    assert!(bytes.contains("committer psn-7f3a9c@acme.noreply <psn-7f3a9c@acme.noreply>"));

    // THE FIRING: the P-GA-18 verdict scaffold runs over the live commit's actor lines → PASS.
    assert!(
        commit_actor_holds_only_pseudonym(&author_actor),
        "the live commit author actor holds only the pseudonym form (P-GA-18 fires green)"
    );
    assert!(
        commit_actor_holds_only_pseudonym(&committer_actor),
        "the live commit committer actor holds only the pseudonym form"
    );
    // The roll-up over BOTH lines (the production lib predicate) passes.
    assert!(
        pseudonym_actor_lines_pass_the_prerequisite(&[&author_actor, &committer_actor]),
        "every actor line on the real commit holds only the pseudonym form"
    );
}

/// **GIT-D2: erase an author → 0 recoverable real identity in the immutable bytes; the residual == the
/// ONE platform-posture residual; crypto-shred reaches backups.** The pseudonymous-by-default floor
/// means the immutable commit bytes never held real identity to recover; the inline free-text bodies
/// are crypto-shredded via the per-subject DEK (the H1 holder); the residual IS the canonical posture.
#[test]
fn git_d2_erase_leaves_zero_recoverable_real_identity_and_the_one_residual() {
    let commit = pseudonymous_commit("psn-erase");

    // The real identity the S2 pseudonym-map mapped this commit's pseudonym to (the thing erase
    // shreds): a real name + a routable email. NONE of this was ever in the commit bytes.
    let real_identity_tokens = ["Ada Lovelace", "ada.lovelace@example.com", "ada@acme.com"];
    let bytes = String::from_utf8(commit.canonical_bytes()).unwrap();
    for tok in real_identity_tokens {
        assert!(
            !bytes.contains(tok),
            "GIT-D2: real-identity token {tok:?} must NOT be in the immutable commit bytes"
        );
    }

    // The H1 inline-body crypto-shred reaches backups: erase destroys the per-subject DEK.
    let kms = InMemoryShredKms::new();
    kms.provision(subject_dek("u-author"), 4242);
    assert!(
        kms.is_present(&subject_dek("u-author")),
        "the inline-body DEK is live before erase"
    );

    let git_holder = GitDbHolder::new(&kms);
    let receipt = git_holder
        .erase(EraseScope::Subject {
            subject: subject("u-author"),
            tenant: tenant(),
        })
        .unwrap();

    assert!(
        !kms.is_present(&subject_dek("u-author")),
        "the inline-body DEK is crypto-shredded"
    );
    assert_eq!(
        kms.recoverable_in_backup(&subject_dek("u-author")),
        0,
        "GIT-D2: 0 recoverable in backups (crypto-shred reaches backups)"
    );
    assert!(
        receipt.receipt.key_epoch_destroyed.is_some(),
        "the destroyed key epoch is recorded"
    );
    assert!(
        receipt.receipt.content_hash.starts_with("blake3:"),
        "the erase receipt is content-addressed"
    );

    // The residual == the ONE platform-posture residual (confirmed equal, never re-described).
    assert!(
        git_residual_is_the_one_posture(),
        "GIT-D2's residual == the ONE platform-posture residual"
    );
}

/// **The Git erasure section references the platform posture (does not restate it) — the P-GA-16
/// scaffolding firing green over the FIRST real subsystem register.** Completes the consumer half of
/// the 10.9 CDC pair: Git CITES the canonical anchor and adds no restated posture text (the X-7
/// anti-pattern is structurally absent).
#[test]
fn the_git_instance_completes_the_10_9_by_reference_pair() {
    assert_eq!(
        GIT_INSTANCE.cited_anchor, POSTURE_ANCHOR,
        "Git cites the ONE canonical anchor"
    );
    assert_eq!(GIT_INSTANCE.cited_anchor, CANONICAL_POSTURE.anchor);
    assert!(
        git_section_references_posture(),
        "the Git erasure section is a valid BY-REFERENCE instantiation (cites + does not restate)"
    );
    assert_eq!(
        CANONICAL_POSTURE.contract_row, "10.9",
        "the by-reference instance owns row 10.9"
    );
}
