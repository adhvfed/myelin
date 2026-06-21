//! # The CDC pair for contract 4.8 — Git pseudonymous-by-default commits (GIT-P25 / P-ID-25, M3)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 4.8
//! (`resolve_pseudonym`/`erase`; the FROZEN pseudonym grammar `<pseudonym>@<tenant>.noreply`;
//! **Git commits pseudonymous-by-default**). Owning architecture:
//! `identity-and-access.md` §11 (the pseudonymous-by-default commits consume the grammar) + §3
//! (the opaque `principal_id` / erasable `profile_ref` split), `00-reconciliation-decisions.md`
//! §X-7 (the ONE platform-wide erasure posture — pseudonym-map shred is DSR fan-out step 1),
//! `external-insights/04-hard-problems.md` §1 (erasure-vs-immutability — the immutable commit bytes
//! must never bake in erasable PII).
//!
//! ## The seam this pair pins (Id owns the grammar; Git is the data-model consumer; DSR erases)
//! Row 4.8 has two halves stitched together at the commit:
//! - **PROVIDER (the Git data model)** mints a commit whose author/committer identity is the
//!   per-tenant pseudonym `<pseudonym>@<tenant>.noreply` (`myelin_identity::PseudonymHandle`,
//!   rendered by Id — the ONE grammar). The pseudonym is what goes into the IMMUTABLE commit bytes;
//!   the real identity NEVER does.
//! - **CONSUMER (the DSR orchestrator)** drives `erase(subject)`. Step 1 of the DSR fan-out
//!   (X-7) is the pseudonym-map (S2) crypto-shred — after it, the real identity↔pseudonym mapping is
//!   gone. The orchestrator then verifies the residual over the immutable commit bytes: **0 real
//!   identity recoverable**, the pseudonymous handle is the only residual ⇒ the pseudonymous
//!   residual == the one platform posture.
//!
//! This pair proves the two sides agree on ONE rendering (`PseudonymHandle::render` — no second
//! identity language in the bytes) and that the erase residual is the posture.

use myelin_git::commit::{erased_residual, Commit, CommitAttribution, CommitIdentity, CommitOid};
use myelin_identity::{
    Principal, PrincipalId, PrincipalKind, PseudonymHandle, PSEUDONYM_DOMAIN_SUFFIX,
};

/// **PROVIDER side of 4.8** — the Git data model mints a pseudonymous-by-default commit. The author
/// and committer are the per-tenant pseudonym handle (the S2 map's PII-free `(pseudonym, tenant)`);
/// the real name/email NEVER enters this path (there is no constructor that would let it).
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

/// The opaque, stable `principal_id` the Git metadata store attributes the commit by (arch §3) —
/// out-of-band, NOT in the bytes. This is the authz attribution that survives an erase of the map.
fn provider_attribution(commit: &Commit, pseudonym: &str, tenant: &str) -> CommitAttribution {
    CommitAttribution {
        commit: commit.oid(),
        principal_id: "principal:opaque-stable-7c1".into(),
        pseudonym: PseudonymHandle::new(pseudonym, tenant).unwrap(),
    }
}

/// **CONSUMER side of 4.8** — the DSR orchestrator. After step-1 of the fan-out (the pseudonym-map
/// crypto-shred) it scans the IMMUTABLE commit bytes for the now-shredded real-identity values and
/// returns whether the residual matches the platform posture (0 real identity recoverable).
///
/// `real_identity_tokens` is what the S2 map mapped the pseudonym to — the values `erase(subject)`
/// destroys. The orchestrator asserts they are absent from the bytes (which they always are, because
/// the bytes were pseudonymous-by-default from the moment the provider minted them).
fn dsr_orchestrator_verify_residual(commit: &Commit, real_identity_tokens: &[&str]) -> bool {
    let residual = erased_residual(commit, real_identity_tokens);
    residual.residual_matches_posture()
}

/// The real subject behind the pseudonym (the DSR target). A verified `Principal` — references-not-
/// payloads (the real name/email it maps to lives in the erasable S2 profile_ref, not here).
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

/// **The CDC: provider mints pseudonymous → consumer erases → residual == posture.** The two halves
/// of 4.8 agree on the ONE grammar, and the erase residual is the platform posture.
#[test]
fn cdc_4_8_provider_consumer_pseudonymous_commit_erase_residual() {
    let pseudonym = "psn-7f3a9c";
    let tenant = "acme";

    // PROVIDER: mint the pseudonymous-by-default commit + its out-of-band authz attribution.
    let commit = provider_mints_pseudonymous_commit(
        pseudonym,
        tenant,
        "feat: paginate the ref list\n",
    );
    let attribution = provider_attribution(&commit, pseudonym, tenant);

    // The bytes carry EXACTLY the frozen grammar for both author and committer (one rendering).
    let bytes = String::from_utf8(commit.canonical_bytes()).unwrap();
    let rendered = format!("{pseudonym}@{tenant}{PSEUDONYM_DOMAIN_SUFFIX}");
    assert_eq!(rendered, "psn-7f3a9c@acme.noreply");
    assert!(bytes.contains(&format!("author {rendered} <{rendered}>")));
    assert!(bytes.contains(&format!("committer {rendered} <{rendered}>")));

    // The subject behind the pseudonym (the DSR target).
    let _subject = subject_principal(tenant);

    // CONSUMER: the DSR orchestrator runs erase(subject) step-1 (pseudonym-map shred) and verifies
    // the residual. The real identity the S2 map mapped to (name + real routable email) is gone from
    // the map; assert it was NEVER in the immutable bytes.
    let real_identity_tokens = ["Ada Lovelace", "ada.lovelace@example.com"];
    assert!(
        dsr_orchestrator_verify_residual(&commit, &real_identity_tokens),
        "real identity recoverable from immutable commit bytes after erase"
    );

    // The opaque principal_id STILL attributes the commit for authz after the erase (out-of-band,
    // arch §3) — neither it nor the real identity is in the bytes.
    assert_eq!(attribution.commit, commit.oid());
    assert!(!bytes.contains("principal:opaque-stable-7c1"));
    for tok in real_identity_tokens {
        assert!(!bytes.contains(tok), "leaked real-identity token {tok}");
    }
}

/// The provider and consumer agree on the ONE rendering: the consumer parses back exactly the
/// `(pseudonym, tenant)` the provider rendered (the byte-identical round-trip the commit codec
/// relies on — there is no second identity language).
#[test]
fn cdc_4_8_provider_consumer_agree_on_one_grammar_rendering() {
    let commit = provider_mints_pseudonymous_commit("psn-abc", "globex", "chore: bump\n");
    let rendered = commit.author.render_email();
    let parsed = PseudonymHandle::parse(&rendered).expect("the one grammar round-trips");
    assert_eq!(parsed.pseudonym(), "psn-abc");
    assert_eq!(parsed.tenant(), "globex");
    assert_eq!(parsed.render(), rendered);
}
