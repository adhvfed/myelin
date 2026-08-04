use myelin_refs::{mint, ArtifactRef, ParseError, Sub, SubKind, SubKindRegistration};

pub const GIT_SUBSYSTEM: &str = "git";

pub const GIT_OWNED_SUB_KINDS: &[SubKind] =
    &[SubKind::Comment, SubKind::Thread, SubKind::LineRange];

pub fn register_git_sub_kinds() -> Result<SubKindRegistration, myelin_refs::RegistrationError> {
    SubKindRegistration {
        subsystem: GIT_SUBSYSTEM.to_string(),
        kinds: GIT_OWNED_SUB_KINDS.to_vec(),
    }
    .validate()
}

fn pr_root(tenant: &str, repo: &str, pr_number: u64) -> Result<ArtifactRef, ParseError> {
    myelin_refs::parse(&format!("myelin://{tenant}/git/pr/{repo}:{pr_number}"))
}

fn blob_root(
    tenant: &str,
    repo: &str,
    git_ref: &str,
    path: &str,
) -> Result<ArtifactRef, ParseError> {
    let encoded_path = encode_path_segment(path);
    myelin_refs::parse(&format!(
        "myelin://{tenant}/git/blob/{repo}:{git_ref}:{encoded_path}"
    ))
}

pub fn encode_path_segment(path: &str) -> String {
    path.replace('%', "%25").replace('/', "%2F")
}

pub fn decode_path_segment(encoded: &str) -> String {
    encoded.replace("%2F", "/").replace("%25", "%")
}

pub fn mint_pr_comment(
    tenant: &str,
    repo: &str,
    pr_number: u64,
    comment_id: &str,
) -> Result<ArtifactRef, ParseError> {
    let root = pr_root(tenant, repo, pr_number)?;
    mint(&root, Sub::Comment(comment_id.to_string()))
}

pub fn mint_pr_thread(
    tenant: &str,
    repo: &str,
    pr_number: u64,
    thread_id: &str,
) -> Result<ArtifactRef, ParseError> {
    let root = pr_root(tenant, repo, pr_number)?;
    mint(&root, Sub::Thread(thread_id.to_string()))
}

pub fn mint_blob_line_range(
    tenant: &str,
    repo: &str,
    git_ref: &str,
    path: &str,
    start: u64,
    end: u64,
) -> Result<ArtifactRef, ParseError> {
    let root = blob_root(tenant, repo, git_ref, path)?;
    mint(&root, Sub::LineRange { start, end })
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_refs::{strip_sub, sub_kind};

    #[test]
    fn git_sub_kind_registration_is_accepted_and_declares_only_git_owned_kinds() {
        let reg = register_git_sub_kinds().expect("Refs must accept git's #sub registration");
        assert_eq!(reg.subsystem, "git");
        assert_eq!(
            reg.kinds,
            vec![SubKind::Comment, SubKind::Thread, SubKind::LineRange]
        );
        assert!(!reg.kinds.contains(&SubKind::Check));
        assert!(!reg.kinds.contains(&SubKind::Step));
    }

    #[test]
    fn git_mints_produce_grammatical_round_tripping_sub_urns() {
        let c = mint_pr_comment("acme-eu", "repo7", 4291, "cAbc123").unwrap();
        assert_eq!(
            myelin_refs::format(&c),
            "myelin://acme-eu/git/pr/repo7:4291#comment-cAbc123"
        );
        assert_eq!(sub_kind(&c).map(|s| s.kind()), Some(SubKind::Comment));
        assert_eq!(
            myelin_refs::format(&strip_sub(&c)),
            "myelin://acme-eu/git/pr/repo7:4291"
        );

        let t = mint_pr_thread("acme-eu", "repo7", 4291, "tXyz").unwrap();
        assert_eq!(
            myelin_refs::format(&t),
            "myelin://acme-eu/git/pr/repo7:4291#thread-tXyz"
        );
        assert_eq!(sub_kind(&t).map(|s| s.kind()), Some(SubKind::Thread));

        let l = mint_blob_line_range("acme-eu", "repo7", "main", "src/lib.rs", 42, 88).unwrap();
        assert_eq!(
            myelin_refs::format(&l),
            "myelin://acme-eu/git/blob/repo7:main:src%2Flib.rs#L42-L88"
        );
        assert_eq!(sub_kind(&l).map(|s| s.kind()), Some(SubKind::LineRange));
        assert_eq!(
            myelin_refs::format(&strip_sub(&l)),
            "myelin://acme-eu/git/blob/repo7:main:src%2Flib.rs"
        );
        assert_eq!(decode_path_segment("src%2Flib.rs"), "src/lib.rs");
        assert_eq!(decode_path_segment("a%25b%2Fc"), "a%b/c");
    }

    #[test]
    fn line_range_endpoints_are_grammar_checked_at_mint_time() {
        assert!(mint_blob_line_range("acme", "r", "main", "f.rs", 7, 7).is_ok());
        assert!(matches!(
            mint_blob_line_range("acme", "r", "main", "f.rs", 88, 42),
            Err(ParseError::UnknownSubKind { .. })
        ));
    }

    #[test]
    fn empty_opaque_id_is_rejected_at_mint_time() {
        assert!(matches!(
            mint_pr_comment("acme", "r", 1, ""),
            Err(ParseError::UnknownSubKind { .. })
        ));
        assert!(matches!(
            mint_pr_thread("acme", "r", 1, ""),
            Err(ParseError::UnknownSubKind { .. })
        ));
    }
}
