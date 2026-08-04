use myelin_git::commit::{erased_residual, Commit, CommitAttribution, CommitIdentity, CommitOid};
use myelin_identity::{
    Principal, PrincipalId, PrincipalKind, PseudonymHandle, PSEUDONYM_DOMAIN_SUFFIX,
};

fn provider_mints_pseudonymous_commit(pseudonym: &str, tenant: &str, message: &str) -> Commit {
    let handle = PseudonymHandle::new(pseudonym, tenant).expect("the S2 map mints a valid handle");
    let author = CommitIdentity::pseudonymous(handle.clone(), 1_700_000_000, 120);
    let committer = CommitIdentity::pseudonymous(handle, 1_700_000_000, 120);
    Commit {
        tree: CommitOid("blake3:tree-oid".into()),
        parents: vec![CommitOid("blake3:parent-oid".into())],
        author,
        committer,
        message: message.into(),
    }
}

fn provider_attribution(commit: &Commit, pseudonym: &str, tenant: &str) -> CommitAttribution {
    CommitAttribution {
        commit: commit.oid(),
        principal_id: "principal:opaque-stable-7c1".into(),
        pseudonym: PseudonymHandle::new(pseudonym, tenant).unwrap(),
    }
}

fn dsr_orchestrator_verify_residual(commit: &Commit, real_identity_tokens: &[&str]) -> bool {
    let residual = erased_residual(commit, real_identity_tokens);
    residual.residual_matches_posture()
}

fn subject_principal(tenant: &str) -> Principal {
    use myelin_tenancy::{Region, TenantId};
    Principal::new(
        TenantId(tenant.into()),
        Region("fr-par".into()),
        PrincipalId("principal:opaque-stable-7c1".into()),
        PrincipalKind::Human,
        myelin_identity::DataRole::Processor,
        myelin_identity::PrincipalStatus::Active,
    )
}

#[test]
fn cdc_4_8_provider_consumer_pseudonymous_commit_erase_residual() {
    let pseudonym = "psn-7f3a9c";
    let tenant = "acme";

    let commit =
        provider_mints_pseudonymous_commit(pseudonym, tenant, "feat: paginate the ref list\n");
    let attribution = provider_attribution(&commit, pseudonym, tenant);

    let bytes = String::from_utf8(commit.canonical_bytes()).unwrap();
    let rendered = format!("{pseudonym}@{tenant}{PSEUDONYM_DOMAIN_SUFFIX}");
    assert_eq!(rendered, "psn-7f3a9c@acme.noreply");
    assert!(bytes.contains(&format!("author {rendered} <{rendered}>")));
    assert!(bytes.contains(&format!("committer {rendered} <{rendered}>")));

    let _subject = subject_principal(tenant);

    let real_identity_tokens = ["Ada Lovelace", "ada.lovelace@example.com"];
    assert!(
        dsr_orchestrator_verify_residual(&commit, &real_identity_tokens),
        "real identity recoverable from immutable commit bytes after erase"
    );

    assert_eq!(attribution.commit, commit.oid());
    assert!(!bytes.contains("principal:opaque-stable-7c1"));
    for tok in real_identity_tokens {
        assert!(!bytes.contains(tok), "leaked real-identity token {tok}");
    }
}

#[test]
fn cdc_4_8_provider_consumer_agree_on_one_grammar_rendering() {
    let commit = provider_mints_pseudonymous_commit("psn-abc", "globex", "chore: bump\n");
    let rendered = commit.author.render_email();
    let parsed = PseudonymHandle::parse(&rendered).expect("the one grammar round-trips");
    assert_eq!(parsed.pseudonym(), "psn-abc");
    assert_eq!(parsed.tenant(), "globex");
    assert_eq!(parsed.render(), rendered);
}

use myelin_git::commit::{enforce_pseudonymous_commit, NonPseudonymousIdentity};

fn client_commit_bytes(identity_line: &str) -> Vec<u8> {
    format!("tree blake3:t\nauthor {identity_line}\ncommitter {identity_line}\n\nfeat: x\n")
        .into_bytes()
}

#[test]
fn cdc_4_8_git_enforces_pseudonymous_commit_identity_at_the_door() {
    let tenant = "acme";

    let handle = PseudonymHandle::new("psn-7f3a9c", tenant).unwrap();
    let pseudonymous = client_commit_bytes(&format!(
        "{rendered} <{rendered}> 1700000000 +0000",
        rendered = handle.render()
    ));
    let (author, committer) =
        enforce_pseudonymous_commit(&pseudonymous, tenant).expect("a tenant pseudonym admits");
    assert_eq!(author, handle);
    assert_eq!(committer, handle);

    let raw = client_commit_bytes("Ada Lovelace <ada.lovelace@example.com> 1700000000 +0000");
    assert_eq!(
        enforce_pseudonymous_commit(&raw, tenant),
        Err(NonPseudonymousIdentity::NotAPseudonym {
            role: "author".into(),
            offending_email: "ada.lovelace@example.com".into(),
        }),
        "the door rejects a non-pseudonymous identity"
    );
}
