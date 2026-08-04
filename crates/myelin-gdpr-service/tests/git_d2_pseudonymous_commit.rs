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

#[test]
fn p_ga_18_architecture_test_fires_green_on_real_git_commit_bytes() {
    let commit = pseudonymous_commit("psn-7f3a9c");

    let author_actor = commit.author.render_email();
    let committer_actor = commit.committer.render_email();
    assert_eq!(author_actor, "psn-7f3a9c@acme.noreply");
    assert_eq!(committer_actor, "psn-7f3a9c@acme.noreply");

    let bytes = String::from_utf8(commit.canonical_bytes()).unwrap();
    assert!(bytes.contains("author psn-7f3a9c@acme.noreply <psn-7f3a9c@acme.noreply>"));
    assert!(bytes.contains("committer psn-7f3a9c@acme.noreply <psn-7f3a9c@acme.noreply>"));

    assert!(
        commit_actor_holds_only_pseudonym(&author_actor),
        "the live commit author actor holds only the pseudonym form (P-GA-18 fires green)"
    );
    assert!(
        commit_actor_holds_only_pseudonym(&committer_actor),
        "the live commit committer actor holds only the pseudonym form"
    );
    assert!(
        pseudonym_actor_lines_pass_the_prerequisite(&[&author_actor, &committer_actor]),
        "every actor line on the real commit holds only the pseudonym form"
    );
}

#[test]
fn git_d2_erase_leaves_zero_recoverable_real_identity_and_the_one_residual() {
    let commit = pseudonymous_commit("psn-erase");

    let real_identity_tokens = ["Ada Lovelace", "ada.lovelace@example.com", "ada@acme.com"];
    let bytes = String::from_utf8(commit.canonical_bytes()).unwrap();
    for tok in real_identity_tokens {
        assert!(
            !bytes.contains(tok),
            "GIT-D2: real-identity token {tok:?} must NOT be in the immutable commit bytes"
        );
    }

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

    assert!(
        git_residual_is_the_one_posture(),
        "GIT-D2's residual == the ONE platform-posture residual"
    );
}

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
