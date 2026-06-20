//! # The CDC pair for contract 2.9 — Git's `git.*` token registration (GIT-P2 / P-124)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 2.9
//! (Event taxonomy + token table — `<subsystem>.<artifact_type>.<event_name>`; **each subsystem
//! completes its list**). Owning architecture: Git
//! `04-subsystem-architectures/git-hosting/architecture/03-events-contracts-and-glue.md` §1 (the
//! complete `git.*` taxonomy Git OWNS); Bus `event-bus.md` §6.1 (the grammar — the AUTHORITY),
//! §6.2 (the subsystem/type token table).
//!
//! ## The seam this pair pins (git registers; the Bus owns the grammar)
//! Row 2.9 is the seam between the side that OWNS + registers a subsystem's dotted-name list (the
//! **PROVIDER** — here Git, [`myelin_git::events`]) and the side that owns the grammar + validates
//! every registered name (the **CONSUMER** — the one Bus validator,
//! [`myelin_events::validate_event_type`]). The frozen behaviour both sides agree on:
//!
//! - the PROVIDER (Git) registers its COMPLETE v1 `git.*` list ([`GIT_EVENT_TOKENS`]) — every name
//!   of the §6.1 shape (lowercase, singular, past-tense, `[a-z][a-z0-9_]*` tokens, 2-or-3 segments,
//!   the leading token the canonical `git` subsystem), and registers NO foreign-subsystem token;
//! - the CONSUMER (the Bus validator) ADMITS every registered git name (0 ungrammatical) and would
//!   REJECT a malformed git-shaped name LOUDLY — git does not get to author the grammar.
//!
//! This is the dedicated 2.9 provider+consumer pair the GIT-P2 TESTS field names; the focused
//! per-token round-trip fixtures live in `myelin_git::events::tests`.

use myelin_events::{validate_event_type, TaxonomyError};
use myelin_git::events::{register_git_tokens, GIT_EVENT_TOKENS, GIT_REF_UPDATED};

/// **PROVIDER side of 2.9** — Git, the owner, registers its complete `git.*` list. The provider's
/// promise: every `type` token it puts on the wire is one of these, grammar-conformant by
/// construction. This returns the registry the consumer validates.
fn provider_registers_git_tokens() -> &'static [&'static str] {
    GIT_EVENT_TOKENS
}

/// **CONSUMER side of 2.9** — the one Bus grammar validator every consumer (and the Bus itself)
/// runs a `type` through. It ADMITS a canonical name and REJECTS a malformed one. The consumer's
/// promise: it never silently accepts a non-conformant `type`.
fn consumer_admits(type_name: &str) -> bool {
    validate_event_type(type_name).is_ok()
}

/// The 2.9 pair, end-to-end: the PROVIDER (Git) registers its complete list, and the CONSUMER (the
/// Bus validator) admits **every** registered token — 0 ungrammatical. This is the dated green
/// artifact the GIT-P2 GATE names.
#[test]
fn cdc_2_9_git_provider_registers_consumer_admits_every_token() {
    for &tok in provider_registers_git_tokens() {
        assert!(
            consumer_admits(tok),
            "consumer (Bus validator) wrongly REJECTED registered git token `{tok}`: {:?}",
            validate_event_type(tok)
        );
    }
    // The whole-list registration helper is the provider's one-call assertion (0 ungrammatical).
    assert!(
        register_git_tokens().is_ok(),
        "Git's register_git_tokens() must be green: {:?}",
        register_git_tokens()
    );
}

/// The CONSUMER validator REJECTS a malformed git-shaped `type` LOUDLY (the specific
/// [`TaxonomyError`] for the broken rule), never silently coerced — git does NOT get to author the
/// grammar. The negative half of the seam: the validator is a real gate, not a pass-through.
#[test]
fn cdc_2_9_consumer_rejects_a_malformed_git_type_loudly() {
    // present-tense verb (git.pr.open, not opened)
    assert!(matches!(
        validate_event_type("git.pr.open"),
        Err(TaxonomyError::PresentTenseVerb { .. })
    ));
    // plural artifact-type token (git.comments.created)
    assert!(matches!(
        validate_event_type("git.comments.created"),
        Err(TaxonomyError::PluralToken { .. })
    ));
    // uppercase token
    assert!(matches!(
        validate_event_type("git.PR.opened"),
        Err(TaxonomyError::BadToken { .. })
    ));
}

/// The PROVIDER registers NO foreign-subsystem token — git does NOT emit `ci.*` (the dependency is
/// acyclic: CI emits, Git reads) nor the Identity-owned `key.*` / `token.*` echoes (arch §1). The
/// acyclic-producer invariant (EI-02 §3), pinned at the contract seam.
#[test]
fn cdc_2_9_git_registers_only_its_own_subsystem() {
    for &tok in provider_registers_git_tokens() {
        assert!(
            tok.starts_with("git."),
            "git registered the foreign-subsystem token `{tok}` (must own `git.*` only)"
        );
    }
    // The load-bearing core push event is present under its named constant (the names anchor X-5).
    assert!(provider_registers_git_tokens().contains(&GIT_REF_UPDATED));
}
