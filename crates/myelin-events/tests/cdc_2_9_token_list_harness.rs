//! # The CDC pair for the per-subsystem token-list VALIDATION HARNESS — contract 2.9 (EB-26 / P-246)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 2.9
//! ("each subsystem completes its list" — the Bus owns the §6.1 grammar + the harness; each
//! subsystem owns + REGISTERS its complete dotted-name list). Owning architecture: `event-bus.md`
//! §6.1 (the grammar — the AUTHORITY), §6.4 (the seed). Doctrine: EI-01 §7 (one grammar, no
//! per-subsystem drift — the X-5 names anchor).
//!
//! ## What this pair pins (the harness as the 2.9 seam)
//! Row 2.9's M3 completion is the seam between the **PROVIDER** — a subsystem (Git/KN) that REGISTERS
//! its completed list into the Bus harness — and the **CONSUMER** — the Bus harness that ADMITS a
//! grammar-conformant list in full + REJECTS a malformed addition LOUDLY (the offending token named).
//! This is the Bus's narrow half of "each subsystem completes its list": one grammar, registered
//! through one harness, no per-subsystem drift.

use myelin_events::{
    HarnessError, PayloadShape, RegisteredToken, SubsystemTokenList, TaxonomyError, TokenListHarness,
};

/// **PROVIDER side of 2.9 (a subsystem registers its completed list).** Git registers a
/// representative completed `git.*` list; the harness admits it IN FULL (every name §6.1-conformant +
/// own-prefixed + unique).
#[test]
fn provider_subsystem_registers_its_completed_list_admitted_in_full() {
    let mut harness = TokenListHarness::new();
    let git = SubsystemTokenList::references_only(
        "git",
        &["git.ref.updated", "git.pr.opened", "git.pr.merged", "git.repo.snapshot", "git.repo.erased"],
    );
    assert_eq!(harness.register(&git).unwrap(), 5, "the whole git list is admitted");

    // A second subsystem (KN) registers its M3 list — admitted, no collision with git.
    let kn = SubsystemTokenList::references_only(
        "knowledge",
        &["knowledge.page.created", "knowledge.block.updated", "knowledge.page.snapshot"],
    );
    assert_eq!(harness.register(&kn).unwrap(), 3);
    assert_eq!(harness.registered_subsystems(), vec!["git", "knowledge"]);
    assert!(harness.is_registered("git.ref.updated"));
    assert!(harness.is_registered("knowledge.block.updated"));
}

/// **CONSUMER side of 2.9 (the Bus harness rejects a malformed addition).** The harness REJECTS a
/// malformed addition LOUDLY — with the specific [`TaxonomyError`] the name broke — never silently
/// coercing, and leaves the harness UNCHANGED (all-or-nothing).
#[test]
fn consumer_harness_rejects_a_malformed_addition_loudly() {
    let mut harness = TokenListHarness::new();
    harness
        .register(&SubsystemTokenList::references_only("git", &["git.ref.updated"]))
        .unwrap();

    // present-tense verb → loud PresentTenseVerb.
    assert!(matches!(
        harness.add("git", RegisteredToken::references_only("git.pr.open")),
        Err(HarnessError::UngrammaticalToken { cause: TaxonomyError::PresentTenseVerb { .. }, .. })
    ));
    // uppercase token → loud BadToken.
    assert!(matches!(
        harness.add("git", RegisteredToken::references_only("git.PR.opened")),
        Err(HarnessError::UngrammaticalToken { cause: TaxonomyError::BadToken { .. }, .. })
    ));
    // a foreign-prefix name → loud ForeignPrefix (the acyclic-producer invariant).
    assert!(matches!(
        harness.add("git", RegisteredToken::references_only("ci.check.updated")),
        Err(HarnessError::ForeignPrefix { .. })
    ));
    // The harness is UNCHANGED after every rejected addition.
    assert_eq!(harness.len(), 1, "a rejected addition never mutates the harness");
}

/// The harness holds the **schema_ver lineage + payload-shape descriptor** per registered name
/// (contract 2.9 — "with its schema_ver lineage and payload shapes"), so the consumer/upcaster legs
/// read the current shape version + the references-only/inline-PII/firehose class.
#[test]
fn harness_holds_schema_ver_lineage_and_payload_shapes() {
    let mut harness = TokenListHarness::new();
    let kn = SubsystemTokenList::new(
        "knowledge",
        vec![
            RegisteredToken::references_only("knowledge.page.created"),
            RegisteredToken::references_only("knowledge.page.updated").at_schema_ver(2),
            RegisteredToken::inline_personal_data("knowledge.page.snapshot"),
            RegisteredToken::firehose("knowledge.block.op"),
        ],
    );
    harness.register(&kn).unwrap();

    assert_eq!(harness.lookup("knowledge.page.updated").unwrap().1.current_schema_ver, 2);
    assert_eq!(
        harness.lookup("knowledge.page.created").unwrap().1.shape,
        PayloadShape::ReferencesOnly
    );
    assert_eq!(
        harness.lookup("knowledge.page.snapshot").unwrap().1.shape,
        PayloadShape::InlinePersonalData
    );
    assert_eq!(harness.lookup("knowledge.block.op").unwrap().1.shape, PayloadShape::EphemeralFirehose);
}

/// A non-canonical leading subsystem token is rejected (the leading token must be a §6.2 token) —
/// the harness is the gate that keeps the subsystem set the frozen canonical one.
#[test]
fn harness_rejects_an_unknown_subsystem() {
    let mut harness = TokenListHarness::new();
    assert!(matches!(
        harness.register(&SubsystemTokenList::references_only("billing", &["billing.invoice.created"])),
        Err(HarnessError::UnknownSubsystem { .. })
    ));
}
