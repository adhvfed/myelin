use crate::commit_prerequisite::commit_actor_holds_only_pseudonym;
use crate::posture::{
    reference_is_by_reference, SubsystemReference, CANONICAL_POSTURE, POSTURE_ANCHOR,
};

pub const GIT_SUBSYSTEM: &str = "git";

pub const HISTORY_REWRITE_FLOOR_PROMPT: &str = "P-GA-35 (GA-10, M5)";

pub const GIT_INSTANCE: SubsystemReference = SubsystemReference {
    subsystem: GIT_SUBSYSTEM,
    cited_anchor: POSTURE_ANCHOR,
    section_text:
        "Free-text / immutable-content erasure follows the platform posture in \
         00-reconciliation-decisions.md §X-7 / gdpr-and-audit.md §7 (contract 10.9). Git commits are \
         pseudonymous-by-default - the immutable commit hash holds only the <pseudonym>@<tenant>.noreply \
         form (contract 4.8), so an erase leaves 0 recoverable real identity in the immutable bytes. \
         The audited history-rewrite path (§6.6) is the M5 follow-on for the rare commit-body expunge.",
};

#[must_use]
pub fn section_references_posture(r: &SubsystemReference) -> bool {
    reference_is_by_reference(r)
}

#[must_use]
pub fn git_section_references_posture() -> bool {
    section_references_posture(&GIT_INSTANCE)
}

#[must_use]
pub fn residual_is_the_one_posture(candidate_residual: &str) -> bool {
    CANONICAL_POSTURE.residual == candidate_residual
}

#[must_use]
pub fn git_residual_is_the_one_posture() -> bool {
    residual_is_the_one_posture(git_residual())
}

#[must_use]
pub const fn git_residual() -> &'static str {
    CANONICAL_POSTURE.residual
}

#[must_use]
pub fn pseudonym_actor_lines_pass_the_prerequisite(actor_lines: &[&str]) -> bool {
    !actor_lines.is_empty()
        && actor_lines
            .iter()
            .all(|a| commit_actor_holds_only_pseudonym(a))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::posture::restatement_markers;
    use myelin_identity::PseudonymHandle;

    #[test]
    fn the_git_instance_references_the_posture_and_does_not_restate() {
        assert_eq!(GIT_INSTANCE.subsystem, "git");
        assert_eq!(
            GIT_INSTANCE.cited_anchor, POSTURE_ANCHOR,
            "the Git instance cites the ONE anchor"
        );
        assert!(
            git_section_references_posture(),
            "the Git erasure section is a valid BY-REFERENCE instantiation (cites + does not restate)"
        );
        let lowered = GIT_INSTANCE.section_text.to_ascii_lowercase();
        for marker in restatement_markers() {
            assert!(
                !lowered.contains(&marker.to_ascii_lowercase()),
                "the Git section must not restate the canonical marker {marker:?}"
            );
        }
    }

    #[test]
    fn a_restating_git_section_would_be_rejected() {
        let restating = SubsystemReference {
            subsystem: "git",
            cited_anchor: POSTURE_ANCHOR,
            section_text:
                "Git erasure: per-subject DEK crypto-shred renders self-authored commit messages \
                 unrecoverable; the documented lawful-basis limit covers third-party mentions ...",
        };
        assert!(
            !reference_is_by_reference(&restating),
            "a Git section that restates the posture (a canonical marker) is rejected - X-7"
        );
    }

    #[test]
    fn section_references_posture_is_observable_on_both_polarities() {
        assert!(
            section_references_posture(&GIT_INSTANCE),
            "the real Git instance is accepted"
        );
        let restating = SubsystemReference {
            subsystem: "git",
            cited_anchor: POSTURE_ANCHOR,
            section_text:
                "Git erasure: per-subject DEK crypto-shred renders messages unrecoverable ...",
        };
        assert!(
            !section_references_posture(&restating),
            "a restating section is rejected (the core returns observable-false - X-7)"
        );
    }

    #[test]
    fn git_d2_residual_is_the_one_platform_posture_residual() {
        assert!(
            git_residual_is_the_one_posture(),
            "GIT-D2's residual == the ONE platform-posture residual (confirmed equal, not restated)"
        );
        assert_eq!(
            git_residual(),
            CANONICAL_POSTURE.residual,
            "the Git residual IS the single-source canonical residual"
        );
        assert!(
            !residual_is_the_one_posture("some Git-specific re-described residual text"),
            "a residual that is NOT the canonical one is rejected (kills the `-> true` mutant)"
        );
        assert!(
            git_residual().contains("AUTHOR's DEK") && git_residual().contains("not the subject's"),
            "the residual is third-party PII under the AUTHOR's DEK - not shreddable by the subject's key"
        );
    }

    #[test]
    fn a_pseudonym_form_commit_actor_passes_the_prerequisite() {
        let author = PseudonymHandle::new("psn-7f3a", "acme").unwrap().render();
        let committer = PseudonymHandle::new("psn-7f3a", "acme").unwrap().render();
        assert_eq!(author, "psn-7f3a@acme.noreply");
        assert!(
            pseudonym_actor_lines_pass_the_prerequisite(&[&author, &committer]),
            "a commit whose author + committer hold only the pseudonym form passes the prerequisite"
        );
    }

    #[test]
    fn a_real_identity_commit_actor_fails_the_prerequisite() {
        let good = PseudonymHandle::new("psn-7f3a", "acme").unwrap().render();
        for bad in [
            "Ada Lovelace <ada@example.com>",
            "ada@example.com",
            "Ada Lovelace",
            "psn-7f3a@acme.com",
            "psn-7f3a@acme",
        ] {
            assert!(
                !pseudonym_actor_lines_pass_the_prerequisite(&[&good, bad]),
                "a commit with a real-identity actor {bad:?} must FAIL the prerequisite (would bake PII)"
            );
        }
        assert!(
            !pseudonym_actor_lines_pass_the_prerequisite(&[]),
            "an empty actor set is not a passing commit (kills the `all` vacuous-true mutant)"
        );
    }

    #[test]
    fn the_history_rewrite_floor_is_named() {
        assert!(
            HISTORY_REWRITE_FLOOR_PROMPT.contains("P-GA-35"),
            "the audited history-rewrite erasure path is the named M5 follow-on"
        );
    }
}
