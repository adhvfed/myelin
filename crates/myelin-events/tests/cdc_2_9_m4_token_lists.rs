//! # CDC: the per-subsystem M4 token-list VALIDATION HARNESS — contract 2.9 (EB-27 / P-327, M4)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 2.9
//! ("each subsystem completes its list" — the Bus owns the §6.1 grammar + the harness; each
//! subsystem owns + REGISTERS its complete dotted-name list). Owning architecture: `event-bus.md`
//! §6.1 (the grammar — the AUTHORITY), §6.4 (the seed). Doctrine: EI-01 §7 (one grammar, no
//! per-subsystem drift — the X-5 names anchor).
//!
//! ## What this pair pins (the M4 counterpart to the M3 EB-26 harness CDC)
//! EB-26 (P-246, M3) validated the M3 owners' lists (Git/KN) through the Bus harness. This M4 CDC
//! validates the **M4 owners' lists (CI / Issues / Chat)** through the SAME harness: the
//! **PROVIDER** — a subsystem REGISTERS its completed list — and the **CONSUMER** — the Bus harness
//! ADMITS a grammar-conformant list in full + REJECTS a malformed addition LOUDLY (the offending
//! token named). One grammar, one harness, no per-subsystem drift, across both bands.
//!
//! The lists here are REPRESENTATIVE of each subsystem's real `*_EVENT_TOKENS` constant (the Bus
//! crate cannot depend on the subsystem crates — they depend on it; the acyclic §2.9 DAG). Each
//! subsystem's OWN crate test (`myelin_ci_sandbox::events`, `myelin_issues::events`,
//! `myelin_chat::events`) proves its FULL list grammatical; this CDC proves the Bus harness admits
//! the M4 shape + rejects a malformed addition — the seam.

use myelin_events::{
    HarnessError, RegisteredToken, SubsystemTokenList, TaxonomyError, TokenListHarness,
};

/// **PROVIDER side of 2.9 (M4).** CI / Issues / Chat each register a representative completed list;
/// the harness admits each IN FULL (every name §6.1-conformant + own-prefixed + unique), and the
/// three subsystems coexist with no cross-subsystem name collision.
#[test]
fn m4_subsystems_register_their_lists_admitted_in_full() {
    let mut harness = TokenListHarness::new();

    // CI — the producer leg's frozen X-1 tokens + the run/check lifecycle + a *.snapshot reindex.
    let ci = SubsystemTokenList::references_only(
        "ci",
        &[
            "ci.check.updated",
            "ci.result",
            "ci.run.started",
            "ci.run.succeeded",
            "ci.run.failed",
            "ci.log.available",
            "ci.run.snapshot",
            "ci.run.erased",
        ],
    );
    assert_eq!(
        harness.register(&ci).unwrap(),
        8,
        "the whole CI list is admitted"
    );

    // Issues.
    let issues = SubsystemTokenList::references_only(
        "issue",
        &[
            "issue.issue.created",
            "issue.issue.transitioned",
            "issue.issue.snapshot",
            "issue.issue.erased",
        ],
    );
    assert_eq!(harness.register(&issues).unwrap(), 4);

    // Chat (durable set — the firehose-only frames ride the firehose, not the durable harness).
    let chat = SubsystemTokenList::references_only(
        "chat",
        &[
            "chat.message.created",
            "chat.message.edited",
            "chat.message.snapshot",
            "chat.message.erased",
        ],
    );
    assert_eq!(harness.register(&chat).unwrap(), 4);

    assert_eq!(
        harness.registered_subsystems(),
        vec!["chat", "ci", "issue"],
        "all three M4 subsystems coexist (deterministic order)"
    );
    assert!(harness.is_registered("ci.check.updated"));
    assert!(harness.is_registered("ci.result"));
    assert!(harness.is_registered("issue.issue.transitioned"));
    assert!(harness.is_registered("chat.message.snapshot"));
}

/// **CONSUMER side of 2.9 (M4).** The Bus harness REJECTS a malformed addition to each M4
/// subsystem's list LOUDLY — with the specific [`TaxonomyError`] the name broke — never silently
/// coercing, and leaves the harness UNCHANGED (all-or-nothing).
#[test]
fn m4_harness_rejects_malformed_additions_loudly() {
    let mut harness = TokenListHarness::new();
    harness
        .register(&SubsystemTokenList::references_only(
            "ci",
            &["ci.check.updated"],
        ))
        .unwrap();

    // present-tense verb → loud PresentTenseVerb.
    assert!(matches!(
        harness.add("ci", RegisteredToken::references_only("ci.run.start")),
        Err(HarnessError::UngrammaticalToken {
            cause: TaxonomyError::PresentTenseVerb { .. },
            ..
        })
    ));
    // uppercase token → loud BadToken.
    assert!(matches!(
        harness.add("ci", RegisteredToken::references_only("ci.Run.started")),
        Err(HarnessError::UngrammaticalToken {
            cause: TaxonomyError::BadToken { .. },
            ..
        })
    ));
    // a foreign-prefix name → loud ForeignPrefix (the acyclic-producer invariant — CI cannot
    // register a chat.* name).
    assert!(matches!(
        harness.add(
            "ci",
            RegisteredToken::references_only("chat.message.created")
        ),
        Err(HarnessError::ForeignPrefix { .. })
    ));
    // The harness is UNCHANGED after every rejected addition (all-or-nothing).
    assert_eq!(
        harness.len(),
        1,
        "a rejected addition never mutates the harness"
    );
}

/// The Δ1-superseded legacy CI tokens (`ci.status.updated`, `ci.run.passed`) are still GRAMMATICAL
/// (the grammar admits any well-formed `ci.*` name) — but they are DELIBERATELY ABSENT from CI's
/// registered list (arch 03 §1 rename note). This CDC pins that the *grammar* never rejects them
/// (so a future re-add would be a list decision, not a grammar break) — the list curation, not the
/// grammar, is what supersedes them.
#[test]
fn superseded_legacy_tokens_are_grammatical_but_unregistered() {
    // Grammatical (the Bus grammar admits them).
    assert!(myelin_events::validate_event_type("ci.status.updated").is_ok());
    assert!(myelin_events::validate_event_type("ci.run.passed").is_ok());
    // But CI's representative registered list does NOT carry them (the curation supersedes them).
    let mut harness = TokenListHarness::new();
    harness
        .register(&SubsystemTokenList::references_only(
            "ci",
            &["ci.check.updated", "ci.result"],
        ))
        .unwrap();
    assert!(!harness.is_registered("ci.status.updated"));
    assert!(!harness.is_registered("ci.run.passed"));
}
