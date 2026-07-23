//! # CDC: git registers its COMPLETE list into the Bus token-list HARNESS (contract 2.9, EB-26)
//!
//! **Contract:** `contract-index.md` row 2.9 ("each subsystem completes its list" — the Bus owns the
//! §6.1 grammar + the EB-26 harness; git OWNS + REGISTERS its complete `git.*` list). Owning
//! architecture: `event-bus.md` §6.1/§6.4. This is the M3 owner-registration half of 2.9 through the
//! Bus's per-subsystem token-list validation harness (`myelin_events::TokenListHarness`).
//!
//! ## What this pins (the harness as the cross-subsystem 2.9 registry)
//! Git already self-checks `GIT_EVENT_TOKENS` against the one Bus grammar in-crate
//! (`register_git_tokens`). This pins the COMPLEMENTARY seam: git's WHOLE list and current schema
//! lineages are admitted into the Bus's cross-subsystem harness IN FULL (every name
//! §6.1-conformant + carries the `git.` prefix + unique), and a malformed addition to git's list is
//! REJECTED LOUDLY by the harness.

use myelin_events::{
    HarnessError, RegisteredToken, TaxonomyError, TokenListHarness,
};
use myelin_git::events::{
    git_event_token_list, GIT_EVENT_TOKENS, GIT_PR_HEAD_TRIGGER_SCHEMA_V2, GIT_PR_OPENED,
    GIT_PR_SYNCHRONIZED,
};

/// **PROVIDER (git) registers its COMPLETE list; the CONSUMER (the Bus harness) admits it in full.**
#[test]
fn git_complete_list_is_admitted_by_the_bus_harness_in_full() {
    let mut harness = TokenListHarness::new();
    let git = git_event_token_list();
    let admitted = harness
        .register(&git)
        .expect("git's complete list is admitted by the harness");
    assert_eq!(
        admitted,
        GIT_EVENT_TOKENS.len(),
        "EVERY git token is admitted (0 ungrammatical)"
    );
    assert_eq!(harness.names_for("git").len(), GIT_EVENT_TOKENS.len());
    // The load-bearing tokens are registered + looked-up by name (the X-5 names anchor).
    assert!(harness.is_registered("git.ref.updated"));
    assert!(harness.is_registered("git.repo.snapshot"));
    assert!(harness.is_registered("git.repo.erased"));
    for name in [GIT_PR_OPENED, GIT_PR_SYNCHRONIZED] {
        let (_, token) = harness
            .lookup(name)
            .expect("PR head-trigger token is registered");
        assert_eq!(token.current_schema_ver, GIT_PR_HEAD_TRIGGER_SCHEMA_V2);
    }
}

/// **The harness REJECTS a malformed ADDITION to git's list — LOUDLY, by the rule.**
#[test]
fn the_harness_rejects_a_malformed_addition_to_gits_list() {
    let mut harness = TokenListHarness::new();
    harness
        .register(&git_event_token_list())
        .unwrap();

    // present-tense verb.
    assert!(matches!(
        harness.add("git", RegisteredToken::references_only("git.pr.open")),
        Err(HarnessError::UngrammaticalToken {
            cause: TaxonomyError::PresentTenseVerb { .. },
            ..
        })
    ));
    // a foreign-prefix name (git cannot register a ci.* name — the acyclic-producer invariant).
    assert!(matches!(
        harness.add("git", RegisteredToken::references_only("ci.check.updated")),
        Err(HarnessError::ForeignPrefix { .. })
    ));
    // The harness is unchanged.
    assert_eq!(harness.names_for("git").len(), GIT_EVENT_TOKENS.len());
}
