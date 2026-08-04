use myelin_git::commit::{
    erased_residual, Commit, CommitAttribution, CommitIdentity, CommitOid, ErasureResidual,
};
use myelin_identity::PseudonymHandle;

fn pseudonymous_commit(pseudonym: &str, tenant: &str) -> Commit {
    let handle = PseudonymHandle::new(pseudonym, tenant).expect("S2 mints a valid handle");
    let author = CommitIdentity::pseudonymous(handle.clone(), 1_700_000_000, 60);
    let committer = CommitIdentity::pseudonymous(handle, 1_700_000_000, 60);
    Commit {
        tree: CommitOid("blake3:tree".into()),
        parents: vec![CommitOid("blake3:p0".into())],
        author,
        committer,
        message: "fix: bound the receive-pack ref-CAS retry loop\n".into(),
    }
}

#[test]
fn git_d2_erase_commit_author_residual_matches_posture() {
    let pseudonym = "psn-4d8e";
    let tenant = "fr-par-acme";
    let commit = pseudonymous_commit(pseudonym, tenant);

    let attribution = CommitAttribution {
        commit: commit.oid(),
        principal_id: "principal:opaque-9e2".into(),
        pseudonym: PseudonymHandle::new(pseudonym, tenant).unwrap(),
    };

    let real_identity_tokens = [
        "Grace Hopper",
        "grace.hopper@navy.example",
        "ghopper",
        "+1-555-0142",
    ];

    let residual: ErasureResidual = erased_residual(&commit, &real_identity_tokens);

    assert!(
        residual.recoverable_real_identity.is_empty(),
        "GIT-D2 FAIL - real identity recoverable from immutable bytes: {:?}",
        residual.recoverable_real_identity
    );
    assert!(residual.residual_matches_posture());

    assert_eq!(
        residual.pseudonymous_residual,
        PseudonymHandle::new(pseudonym, tenant).unwrap()
    );

    assert_eq!(attribution.commit, commit.oid());
    let bytes = String::from_utf8(commit.canonical_bytes()).unwrap();
    assert!(
        !bytes.contains("principal:opaque-9e2"),
        "the opaque id is out-of-band, not in the bytes"
    );

    println!(
        "[2026-06-21] GIT-D2 PASS - pseudonymous residual == posture; \
         recoverable_real_identity={:?}; pseudonymous_residual={}",
        residual.recoverable_real_identity, residual.pseudonymous_residual
    );
}

#[test]
fn git_d2_residual_scan_catches_a_real_identity_leak() {
    let commit = pseudonymous_commit("psn-x", "acme");
    let clean = erased_residual(&commit, &["Real Name", "real@example.com"]);
    assert!(clean.residual_matches_posture());

    let detected = erased_residual(&commit, &["psn-x@acme.noreply"]);
    assert!(
        !detected.recoverable_real_identity.is_empty(),
        "the residual scan must actually scan the bytes (not a no-op)"
    );
    assert!(!detected.residual_matches_posture());
}
