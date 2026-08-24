use myelin_git::subs::{
    mint_blob_line_range, mint_pr_comment, mint_pr_thread, register_git_sub_kinds,
    GIT_OWNED_SUB_KINDS,
};
use myelin_refs::{format, strip_sub, sub_kind, ArtifactRef, SubKind};

fn provider_mints() -> Vec<(SubKind, ArtifactRef)> {
    vec![
        (
            SubKind::Comment,
            mint_pr_comment("acme-eu", "repo7", 4291, "cAbc").expect("comment mint is grammatical"),
        ),
        (
            SubKind::Thread,
            mint_pr_thread("acme-eu", "repo7", 4291, "tXyz").expect("thread mint is grammatical"),
        ),
        (
            SubKind::LineRange,
            mint_blob_line_range("acme-eu", "repo7", "refs/heads/main", "src/lib.rs", 42, 88)
                .expect("line-range mint uses the canonical blob event coordinate"),
        ),
    ]
}

fn consumer_classifies(r: &ArtifactRef) -> Option<SubKind> {
    let reparsed = myelin_refs::parse(&format(r)).ok()?;
    assert_eq!(format(&reparsed), format(r), "minted ref must be canonical");
    sub_kind(&reparsed).map(|s| s.kind())
}

#[test]
fn cdc_5_7_git_provider_mints_consumer_accepts_and_classifies_every_kind() {
    let reg = register_git_sub_kinds().expect("Refs must ACCEPT git's #sub kind registration");
    assert_eq!(reg.subsystem, "git");
    assert_eq!(reg.kinds, GIT_OWNED_SUB_KINDS.to_vec());

    for (declared, minted) in provider_mints() {
        assert_eq!(
            consumer_classifies(&minted),
            Some(declared),
            "Refs wrongly classified git's mint `{}` (declared {declared:?})",
            format(&minted)
        );
        let root = strip_sub(&minted);
        assert!(
            !format(&root).contains('#'),
            "stripped root still carries a `#sub`: `{}`",
            format(&root)
        );
        assert!(
            myelin_refs::parse(&format(&root)).is_ok(),
            "stripped root `{}` must itself be a parseable canonical root",
            format(&root)
        );
    }
}

#[test]
fn cdc_5_7_consumer_rejects_a_malformed_git_mint_loudly() {
    assert!(mint_pr_comment("acme", "r", 1, "").is_err());
    assert!(mint_blob_line_range("acme", "r", "refs/heads/main", "f.rs", 88, 42).is_err());
}

#[test]
fn cdc_5_7_git_registers_only_its_own_kinds() {
    let reg = register_git_sub_kinds().expect("registration accepted");
    for k in &reg.kinds {
        assert!(
            matches!(k, SubKind::Comment | SubKind::Thread | SubKind::LineRange),
            "git registered a non-git-owned #sub kind `{k:?}`"
        );
    }
    assert!(!reg.kinds.contains(&SubKind::Check));
    assert!(!reg.kinds.contains(&SubKind::Step));
}
