use myelin_gdpr_service::{
    git_residual_is_the_one_posture, git_section_references_posture, reference_is_by_reference,
    LegalStatus, StructuralLever, SubsystemReference, CANONICAL_POSTURE, GIT_INSTANCE,
    POSTURE_ANCHOR,
};

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
        "the residual lawful-basis ratification is [OPEN - LEGAL] - one statement"
    );
    assert!(
        CANONICAL_POSTURE.structural_floor_ships(),
        "the structural floor ships regardless of legal ratification (§7.1 / §7.3)"
    );
}

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

#[test]
fn the_real_git_instance_completes_the_10_9_consumer_half() {
    assert_eq!(
        GIT_INSTANCE.cited_anchor, POSTURE_ANCHOR,
        "the real Git instance cites the ONE anchor"
    );
    assert!(
        git_section_references_posture(),
        "the REAL Git erasure section references the posture (does not restate it) - P-GA-28"
    );
    assert!(
        git_residual_is_the_one_posture(),
        "GIT-D2's residual == the ONE platform-posture residual (confirmed equal, not restated)"
    );
}

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
        "a section that restates the posture (a canonical marker phrase) is rejected - X-7"
    );
}
