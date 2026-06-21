//! # The CDC pair for contract 5.7 — git's owned `#sub` mints (GIT-P4 / P-230)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 5.7
//! (the unified `#sub` sub-artifact scheme — ONE grammar, stable opaque ids minted by each owner;
//! Refs stores the full sub-URN + the stripped root). **Reconciliation:**
//! `00-reconciliation-decisions.md` X-4 (the frozen `#sub` grammar + the one resolution ladder).
//! Owning architecture: git
//! `04-subsystem-architectures/git-hosting/architecture/03-events-contracts-and-glue.md` §2 (the
//! `#sub` mints git owns + the canonical root grammar); Refs owns the grammar + the ladder.
//!
//! ## The seam this pair pins (git mints; Refs owns the grammar)
//! Row 5.7 is the seam between the PROVIDER that MINTS stable opaque sub-ids of its declared kinds
//! (here git — [`myelin_git::subs`]) and the CONSUMER that owns the ONE grammar + accepts the
//! registration + validates every minted sub-URN (Refs — [`myelin_refs`]). The frozen behaviour both
//! sides agree on:
//!
//! - the PROVIDER (git) REGISTERS the `#sub` kinds it owns (`comment-` / `thread-` / `L<a>-L<b>`) and
//!   MINTS grammatical sub-URNs of those kinds — every minted ref round-trips the one grammar (0
//!   ungrammatical), and Refs can [`strip_sub`](myelin_refs::strip_sub) each back to git's canonical
//!   root;
//! - the CONSUMER (Refs) ACCEPTS git's registration (the kinds are the frozen vocabulary + the owner
//!   is a canonical Bus token) and would REJECT a malformed mint LOUDLY — git does not author the
//!   grammar.
//!
//! This is the dedicated 5.7 provider+consumer pair the GIT-P4 TESTS field names; the focused
//! per-mint round-trip fixtures live in `myelin_git::subs::tests`. (No cargo-mutants floor: this is
//! REGISTRATION + grammatical mints over the already-proven Refs grammar, not new load-bearing
//! resolution logic — the resolver mutation floors land with GIT-P18 / GIT-P24.)

use myelin_git::subs::{
    mint_blob_line_range, mint_pr_comment, mint_pr_thread, register_git_sub_kinds,
    GIT_OWNED_SUB_KINDS,
};
use myelin_refs::{format, strip_sub, sub_kind, ArtifactRef, SubKind};

/// **PROVIDER side of 5.7** — git registers the `#sub` kinds it OWNS and returns its grammatical
/// mints. The provider's promise: every sub-URN it puts on a ref is one of its declared kinds and is
/// grammar-conformant by construction.
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
            mint_blob_line_range("acme-eu", "repo7", "main", "src/lib.rs", 42, 88)
                .expect("line-range mint is grammatical (path percent-encoded, REF-3 deviation)"),
        ),
    ]
}

/// **CONSUMER side of 5.7** — Refs, the grammar owner, classifies a minted sub-URN through the one
/// frozen grammar (and round-trips it byte-identical). The consumer's promise: it never silently
/// admits an ungrammatical sub-URN.
fn consumer_classifies(r: &ArtifactRef) -> Option<SubKind> {
    // Re-parse the minted ref through the one codec, then classify — a non-canonical ref would not
    // re-parse, proving the mint is grammatical (not merely a string git built).
    let reparsed = myelin_refs::parse(&format(r)).ok()?;
    assert_eq!(format(&reparsed), format(r), "minted ref must be canonical");
    sub_kind(&reparsed).map(|s| s.kind())
}

/// The 5.7 pair, end-to-end: the PROVIDER (git) registers its owned kinds + mints, and the CONSUMER
/// (Refs) ACCEPTS the registration and classifies EVERY mint to the declared kind — 0 ungrammatical.
/// This is the dated green artifact the GIT-P4 GATE names.
#[test]
fn cdc_5_7_git_provider_mints_consumer_accepts_and_classifies_every_kind() {
    // The registration is accepted by Refs (the one grammar owner).
    let reg = register_git_sub_kinds().expect("Refs must ACCEPT git's #sub kind registration");
    assert_eq!(reg.subsystem, "git");
    assert_eq!(reg.kinds, GIT_OWNED_SUB_KINDS.to_vec());

    // Every mint is grammatical: Refs classifies it to the declared kind (0 ungrammatical).
    for (declared, minted) in provider_mints() {
        assert_eq!(
            consumer_classifies(&minted),
            Some(declared),
            "Refs wrongly classified git's mint `{}` (declared {declared:?})",
            format(&minted)
        );
        // Refs can strip every mint back to git's canonical ROOT (the full sub-URN + stripped root
        // is what Refs stores, contract 5.7).
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

/// The CONSUMER (Refs) REJECTS a malformed git-shaped mint LOUDLY — git does NOT get to author the
/// grammar. The negative half of the seam: a malformed opaque body / inverted range never becomes a
/// sub-URN; the mint itself fails (it re-parses through the one grammar).
#[test]
fn cdc_5_7_consumer_rejects_a_malformed_git_mint_loudly() {
    // an empty opaque comment id (the stable-id obligation is git's; the grammar still refuses)
    assert!(mint_pr_comment("acme", "r", 1, "").is_err());
    // an inverted line range
    assert!(mint_blob_line_range("acme", "r", "main", "f.rs", 88, 42).is_err());
}

/// The PROVIDER registers ONLY its own kinds — git owns `comment-`/`thread-`/`L<a>-L<b>`, NOT the
/// CI-owned `check-`/`step-` kinds (architecture §2 — git only RENDERS a CI `details_ref`). The
/// no-foreign-kind invariant, pinned at the contract seam.
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
