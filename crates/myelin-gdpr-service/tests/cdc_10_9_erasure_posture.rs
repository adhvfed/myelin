//! # CDC 10.9 — the ONE free-text/immutable erasure posture, by reference (P-GA-16 → P-116)
//!
//! **Contract:** index row 10.9 (the ONE free-text / immutable-content erasure posture — structural
//! floor + documented residual limit, instantiated per subsystem BY REFERENCE, `[OPEN — LEGAL]`).
//! The decision is `00-reconciliation-decisions.md` §X-7; the architecture is `gdpr-and-audit.md`
//! §7. This is the consumer-driven contract test the coverage scanner (P-S21) reads both halves of:
//!
//! - **provider** = the canonical posture as OWNED ([`CANONICAL_POSTURE`]) — the single source of
//!   truth (the structural floor §7.1 + the residual §7.2 + the `[OPEN — LEGAL]` ratified posture
//!   §7.3), citing the ONE anchor.
//! - **consumer** = a subsystem erasure section instantiating the posture BY REFERENCE
//!   ([`SubsystemReference`]) — it CITES the canonical anchor and NEVER restates the posture (§7.4).
//!   This stub exercises the consumer SHAPE (a Git-like by-reference section); the **real** Git
//!   consumer (and the CI/Issues/Knowledge/Chat consumers) register in **P-GA-28 → P-256/P-257** and
//!   **P-GA-29/-31** — at which point the `references-it-never-restates` assertion fires over the
//!   real subsystem docs. The provider half is COMPLETE here.
//!
//! The dated green artifact: the provider exposes ONE canonical posture (row 10.9, `[OPEN — LEGAL]`,
//! the three §7.1 levers); a by-reference consumer cites the anchor and is accepted, a restating
//! consumer is rejected (the X-7 anti-pattern — five statements instead of one). If 10.9's posture
//! shape drifts, this stops compiling/passing — that is the contract.

use myelin_gdpr_service::{
    reference_is_by_reference, LegalStatus, StructuralLever, SubsystemReference, CANONICAL_POSTURE,
    POSTURE_ANCHOR,
};

/// **provider (10.9): the posture as OWNED — the single canonical source.** The structural floor is
/// the three §7.1 levers in order; the residual is the author's-DEK limit; the lawful-basis status
/// is `[OPEN — LEGAL]` (ONE statement). This is the source every consumer references.
#[test]
fn provider_owns_the_one_canonical_posture() {
    assert_eq!(CANONICAL_POSTURE.contract_row, "10.9");
    assert_eq!(CANONICAL_POSTURE.anchor, POSTURE_ANCHOR);
    assert_eq!(
        CANONICAL_POSTURE.structural_floor,
        [
            StructuralLever::PerSubjectDekShred,
            StructuralLever::PseudonymMapShred,
            StructuralLever::RestrictSuppression,
        ],
        "the structural floor is the three §7.1 levers in canonical order"
    );
    assert_eq!(
        CANONICAL_POSTURE.legal_status,
        LegalStatus::OpenLegal,
        "the residual lawful-basis ratification is [OPEN — LEGAL] — one statement"
    );
    assert!(
        CANONICAL_POSTURE.structural_floor_ships(),
        "the structural floor ships regardless of legal ratification (§7.1 / §7.3)"
    );
}

/// **consumer (10.9): a subsystem instantiates the posture BY REFERENCE.** The §7.4 short form — it
/// cites the canonical anchor and adds no restated posture text. This is the Git-like shape the real
/// P-GA-28 consumer takes; the property pinned is "references it, never restates it".
#[test]
fn a_by_reference_consumer_cites_the_anchor_and_is_accepted() {
    let git_like = SubsystemReference {
        subsystem: "git",
        cited_anchor: POSTURE_ANCHOR,
        section_text:
            "Free-text / immutable-content erasure follows the platform posture in \
             00-reconciliation-decisions.md §X-7 / gdpr-and-audit.md §7 (contract 10.9). Git \
             commits are pseudonymous-by-default; the immutable hash holds only the pseudonym form.",
    };
    assert!(
        reference_is_by_reference(&git_like),
        "the provider+consumer 10.9 pair: a by-reference subsystem section is accepted"
    );
}

/// **The X-7 anti-pattern is rejected:** a subsystem that RESTATES the posture (five statements
/// instead of one) fails the by-reference gate. This is the property the architecture test enforces
/// over the real M3/M4 docs (the assertion fires in P-GA-28/-29/-31).
#[test]
fn a_restating_consumer_is_rejected() {
    let restating = SubsystemReference {
        subsystem: "chat",
        cited_anchor: POSTURE_ANCHOR,
        section_text:
            "Chat erasure: per-subject DEK crypto-shred renders messages unrecoverable; the \
             documented lawful-basis limit covers third-party mentions ...",
    };
    assert!(
        !reference_is_by_reference(&restating),
        "a section that restates the posture (a canonical marker phrase) is rejected — X-7"
    );
}
