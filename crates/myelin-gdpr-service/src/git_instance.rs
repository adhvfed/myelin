//! # The Git pseudonymous-commit instance of X-7 (10.9 BY REFERENCE) + GIT-D2 (P-GA-28 → P-257)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/gdpr-and-audit.md` **§7.4** (*instantiation per
//! subsystem BY REFERENCE — no restatement*: the Git instance — self-authored content crypto-shreds
//! via per-subject DEK; identity via pseudonym-map shred; **commits pseudonymous-by-default**; the
//! third-party / immutable residual is the documented lawful-basis limit + `restrict`; GIT-D2's
//! residual **== the ONE platform-posture residual**) + **§7.2** (the residual). Prove-it:
//! `external-insights/04-hard-problems.md` §1 (the Git instance: pseudonymous-by-default commits so
//! the immutable hash never bakes erasable PII; the residual is the documented limit).
//!
//! **Contract-index:** owns (orchestration) row **10.9** — the Git BY-REFERENCE instantiation,
//! confirmed-not-restated (the canonical artifact is [`crate::posture`]). Consumed: **4.8** (the
//! pseudonym lever — [`myelin_identity::PseudonymHandle`]), **11.4** (per-subject DEK), **10.1** (the
//! Git H1 fan-out — [`crate::producer_holders::GitDbHolder`]).
//!
//! ## What THIS prompt (P-GA-28) ships — and what it CONFIRMS rather than re-builds (EI-01 §7 coherence)
//! The Git instance of the ONE posture is **BY REFERENCE** (§7.4) — so the work here is to CONFIRM the
//! reference is correct, never to restate the posture, and to make the P-GA-18 architecture test FIRE
//! over Git's real commit codec. Concretely:
//! 1. **The Git erasure section references the platform posture** ([`GIT_INSTANCE`]) — a
//!    [`crate::posture::SubsystemReference`] that CITES the canonical anchor
//!    ([`crate::posture::POSTURE_ANCHOR`]) and adds NO restated posture text. The architecture-test
//!    predicate [`crate::posture::reference_is_by_reference`] (the GATE scaffolding shipped in P-GA-16)
//!    now FIRES over a REAL subsystem register (Git is the first) — completing the consumer half of the
//!    10.9 CDC pair. [`git_section_references_posture`] is the in-module assertion.
//! 2. **GIT-D2's residual == the ONE platform-posture residual** ([`git_residual_is_the_one_posture`])
//!    — the Git instance's residual (the third-party / immutable-commit-byte free-text PII typed by
//!    *another* author, sealed under the AUTHOR's DEK) is IDENTICALLY the canonical
//!    [`crate::posture::CANONICAL_POSTURE`]`.residual`. It is confirmed equal, never re-described.
//! 3. **The P-GA-18 commit-identity prerequisite FIRES over Git's real commit codec.** P-GA-18 recorded
//!    the obligation (*Git's M3 commits must hold only `<pseudonym>@<tenant>.noreply`*) + shipped the
//!    architecture-test verdict scaffold ([`crate::commit_prerequisite::commit_actor_holds_only_pseudonym`]).
//!    In M3 the Git data model now exists ([`myelin_git::commit::Commit`] — pseudonymous-by-construction,
//!    GIT-P25). The verdict scaffold now runs over Git's REAL `canonical_bytes` author/committer line:
//!    `tests/git_d2_pseudonymous_commit.rs` is the GIT-D2 drill that fires it (the architecture test
//!    PASSES — Git commits hold only the pseudonym form). [`pseudonym_actor_lines_pass_the_prerequisite`]
//!    is the in-module shape (the live-codec firing lives in the test, behind the `myelin-git`
//!    dev-dependency, to keep the production DAG free of a git edge — EI-01 §7).
//!
//! It REUSES [`crate::posture::CANONICAL_POSTURE`] / [`crate::posture::reference_is_by_reference`] /
//! [`crate::commit_prerequisite::commit_actor_holds_only_pseudonym`] WHOLESALE — it does NOT re-define
//! the posture, the by-reference predicate, the pseudonym grammar, or the verdict. The Git H1 holder
//! (the inline-body crypto-shred) is [`crate::producer_holders::GitDbHolder`] (shipped P-GA-27); the
//! immutable-commit-byte residual posture instance is what THIS module confirms.
//!
//! ## Floors named (deferred → filling prompt) — VISION §3 name-your-floors
//! - **The audited history-rewrite erasure path** (the rare body-expunge case — a commit *message body*
//!   that names a third party must be expunged, with the disruptive changed-hash consequence) is the
//!   **M5 follow-on P-GA-35 (GA-10)** — distinct from the commit-time pseudonymisation this module
//!   confirms. The pseudonymous-by-default floor covers author IDENTITY with 0 hash change; the rare
//!   body expunge is the M5 path (§6.6). Recorded in writing here.
//! - **The live Git `erase` binding** behind the [`myelin_gdpr::PersonalDataHolder`] seam (the real
//!   `myelin-git` DB at boot) is the config swap P-GA-27 named; this module touches NO new DB /
//!   object-store / cache / bus contract — **no `--features integration` leg owed** (it confirms a
//!   reference + fires a pure-bytes architecture test over the in-process commit codec).
//!
//! ## Mutation floor (P-GA-28 TESTS — the pseudonym-form-only-in-commit-bytes check is mandatory-core).
//! The load-bearing predicate is [`crate::commit_prerequisite::commit_actor_holds_only_pseudonym`] (the
//! verdict that a commit actor line holds ONLY the pseudonym form). Its mutation floor is MET by the
//! pair {a pseudonym actor PASSES, every real-identity actor FAILS} — exercised both in
//! [`crate::commit_prerequisite`] (against fixtures) and in `tests/git_d2_pseudonymous_commit.rs`
//! (against Git's REAL `canonical_bytes`). Git's own `erase`-impl floor is owned by Git (GIT-P25 /
//! `myelin-git`). The two predicates THIS module owns — [`git_section_references_posture`] (cites the
//! anchor AND does not restate) + [`git_residual_is_the_one_posture`] (the residual IS the canonical
//! one) — factor their LOGIC into the parameterised cores [`section_references_posture`] /
//! [`residual_is_the_one_posture`], which the unit tests exercise on BOTH polarities (an accepted cite
//! AND a rejected restatement; the canonical residual AND a different one), so every behavioral mutant
//! on the verdict logic is caught.
//!
//! `cargo mutants -p myelin-gdpr-service --file crates/myelin-gdpr-service/src/git_instance.rs`
//! (2026-06-21): **15 mutants, 13 caught, 2 missed**. Every BEHAVIORAL mutant is CAUGHT — the
//! [`pseudonym_actor_lines_pass_the_prerequisite`] roll-up (the empty-set + the all-pass + any-fail
//! branches), the [`section_references_posture`] core (accept/reject), and the
//! [`residual_is_the_one_posture`] core (equal/not-equal). The 2 residuals are documented non-core:
//! `git_section_references_posture -> true` and `git_residual_is_the_one_posture -> true` are the thin
//! NO-ARG public wrappers that delegate to those cores with the REAL production constants
//! ([`GIT_INSTANCE`] — a valid by-reference cite; [`git_residual`] — the canonical residual). Through
//! the public API the production constant ALWAYS yields `true`, so the wrapper's boolean output is
//! unobservable-false — the SAME equivalent-wrapper class already documented for `audit::verify_chain`
//! and `agent_trace_seam::trace_is_distinct_from_audit`, whose delegated LOGIC is mutation-killed.
//! Stated, not hidden (EI-01 §3).

use crate::commit_prerequisite::commit_actor_holds_only_pseudonym;
use crate::posture::{
    reference_is_by_reference, SubsystemReference, CANONICAL_POSTURE, POSTURE_ANCHOR,
};

/// The subsystem name the Git erasure-section reference registers under.
pub const GIT_SUBSYSTEM: &str = "git";

/// The prompt that ships the M5 audited history-rewrite erasure path (the rare commit *body* expunge,
/// with the disruptive changed-hash consequence). Named in writing so the floor is never
/// pretended-solved — the pseudonymous-by-default floor THIS module confirms covers author IDENTITY;
/// the body-expunge path is M5 (§6.6).
pub const HISTORY_REWRITE_FLOOR_PROMPT: &str = "P-GA-35 (GA-10, M5)";

/// **The Git erasure-section instance of the ONE posture — BY REFERENCE (§7.4).** The canonical §7.4
/// short form: it CITES the platform anchor ([`POSTURE_ANCHOR`]) and adds **no restated posture text**
/// (the structural floor / residual / lawful-basis text lives ONCE in [`CANONICAL_POSTURE`]). This is
/// the FIRST real [`SubsystemReference`] register (the P-GA-16 scaffolding's first live firing) — it
/// completes the consumer half of the 10.9 CDC pair. The `section_text` is exactly the §7.4 form: it
/// names the per-subject DEK / pseudonym-map shred / restrict LEVERS only by *reference to the posture*,
/// never restating their definitions, and states Git's specifics (pseudonymous-by-default commits) which
/// are NOT canonical posture text.
pub const GIT_INSTANCE: SubsystemReference = SubsystemReference {
    subsystem: GIT_SUBSYSTEM,
    cited_anchor: POSTURE_ANCHOR,
    section_text:
        "Free-text / immutable-content erasure follows the platform posture in \
         00-reconciliation-decisions.md §X-7 / gdpr-and-audit.md §7 (contract 10.9). Git commits are \
         pseudonymous-by-default — the immutable commit hash holds only the <pseudonym>@<tenant>.noreply \
         form (contract 4.8), so an erase leaves 0 recoverable real identity in the immutable bytes. \
         The audited history-rewrite path (§6.6) is the M5 follow-on for the rare commit-body expunge.",
};

/// **The by-reference verdict over ANY subsystem reference (the P-GA-28 GATE logic, parameterised so
/// it is observable on both polarities).** Returns `true` iff `r` is a valid by-reference
/// instantiation: it CITES the canonical anchor AND contains NO canonical restatement marker (the X-7
/// anti-pattern — "five residual statements instead of one ratified posture"). Delegates to the P-GA-16
/// scaffolding ([`reference_is_by_reference`]). The Git instance is checked via the thin wrapper
/// [`git_section_references_posture`]; this core is exercised on a REJECTED reference too (so a
/// `-> true` constant mutant is killed by an observable false).
#[must_use]
pub fn section_references_posture(r: &SubsystemReference) -> bool {
    reference_is_by_reference(r)
}

/// **The architecture test that the Git erasure section REFERENCES the platform posture (does not
/// restate it) — the P-GA-28 GATE.** Returns `true` iff [`GIT_INSTANCE`] is a valid by-reference
/// instantiation. This is the P-GA-16 scaffolding FIRING over the FIRST real subsystem register; the
/// observable-on-both-polarities logic is [`section_references_posture`].
#[must_use]
pub fn git_section_references_posture() -> bool {
    section_references_posture(&GIT_INSTANCE)
}

/// **The residual-equivalence verdict for ANY candidate residual (the GIT-D2 §7.2/§7.4 check,
/// parameterised so it is observable on both polarities).** Returns `true` iff `candidate_residual` is
/// IDENTICALLY the canonical [`CANONICAL_POSTURE`]`.residual`. A subsystem instance does NOT re-describe
/// its residual; it IS the one posture's residual, confirmed equal — a Git-specific restatement (a
/// different string) would NOT be equal and is rejected here (killing a `-> true` constant mutant).
#[must_use]
pub fn residual_is_the_one_posture(candidate_residual: &str) -> bool {
    CANONICAL_POSTURE.residual == candidate_residual
}

/// **GIT-D2's residual == the ONE platform-posture residual (§7.2 / §7.4).** The Git instance's residual
/// IS the canonical residual — the third-party / immutable-commit-byte free-text PII typed by *another*
/// author, sealed under the AUTHOR's DEK not the subject's. The observable-on-both-polarities logic is
/// [`residual_is_the_one_posture`].
#[must_use]
pub fn git_residual_is_the_one_posture() -> bool {
    residual_is_the_one_posture(git_residual())
}

/// The Git instance's residual — BY REFERENCE, the ONE canonical residual (no Git-specific
/// restatement). Exposed so a consumer can read "the Git residual" and get back the single source.
#[must_use]
pub const fn git_residual() -> &'static str {
    CANONICAL_POSTURE.residual
}

/// **The P-GA-18 prerequisite verdict over a Git commit actor line (the mandatory-core check, in the
/// production lib for reuse).** Returns `true` iff EVERY supplied commit author/committer actor string
/// holds ONLY the frozen pseudonym form `<pseudonym>@<tenant>.noreply` (contract 4.8) — delegating to
/// [`commit_actor_holds_only_pseudonym`] (the ONE verdict, owned by [`crate::commit_prerequisite`]).
/// The GIT-D2 drill (`tests/git_d2_pseudonymous_commit.rs`) calls this over the author + committer
/// lines lifted from Git's REAL [`myelin_git::commit::Commit::canonical_bytes`] — the architecture
/// test now FIRES on the live codec (Git commits hold only the pseudonym).
#[must_use]
pub fn pseudonym_actor_lines_pass_the_prerequisite(actor_lines: &[&str]) -> bool {
    // A real commit carries an author AND a committer line — both must hold only the pseudonym. An
    // EMPTY input is not a passing commit (a commit has at least an author actor); reject it so a
    // `-> true` constant mutant of the "all pass" roll-up is killed by the empty case.
    !actor_lines.is_empty() && actor_lines.iter().all(|a| commit_actor_holds_only_pseudonym(a))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::posture::restatement_markers;
    use myelin_identity::PseudonymHandle;

    /// **The GATE: the Git erasure section references the platform posture (does not restate it).** The
    /// FIRST real [`SubsystemReference`] register fires the P-GA-16 scaffolding green — Git cites the
    /// canonical anchor and adds no restated posture text (the X-7 anti-pattern is foreclosed).
    #[test]
    fn the_git_instance_references_the_posture_and_does_not_restate() {
        assert_eq!(GIT_INSTANCE.subsystem, "git");
        assert_eq!(GIT_INSTANCE.cited_anchor, POSTURE_ANCHOR, "the Git instance cites the ONE anchor");
        assert!(
            git_section_references_posture(),
            "the Git erasure section is a valid BY-REFERENCE instantiation (cites + does not restate)"
        );
        // It carries NONE of the canonical restatement markers (the load-bearing posture sentences
        // that may live ONLY in the ONE artifact) — the X-7 anti-pattern is structurally absent.
        let lowered = GIT_INSTANCE.section_text.to_ascii_lowercase();
        for marker in restatement_markers() {
            assert!(
                !lowered.contains(&marker.to_ascii_lowercase()),
                "the Git section must not restate the canonical marker {marker:?}"
            );
        }
    }

    /// **A Git-shaped section that RESTATES the posture is rejected** — the gate forbids the X-7
    /// anti-pattern even for Git. This pins that the predicate is load-bearing (a same-anchor section
    /// that restates is NOT accepted), killing a `-> true` constant mutant of the gate.
    #[test]
    fn a_restating_git_section_would_be_rejected() {
        let restating = SubsystemReference {
            subsystem: "git",
            cited_anchor: POSTURE_ANCHOR,
            // Restates the structural floor — the forbidden duplication (a canonical marker phrase).
            section_text:
                "Git erasure: per-subject DEK crypto-shred renders self-authored commit messages \
                 unrecoverable; the documented lawful-basis limit covers third-party mentions ...",
        };
        assert!(
            !reference_is_by_reference(&restating),
            "a Git section that restates the posture (a canonical marker) is rejected — X-7"
        );
    }

    /// **The by-reference verdict logic is observable on BOTH polarities (kills the `-> true` mutant).**
    /// The real [`GIT_INSTANCE`] is accepted; a same-anchor section that RESTATES the posture is
    /// REJECTED through the SAME [`section_references_posture`] core — so the predicate's output is
    /// observably false, not a constant.
    #[test]
    fn section_references_posture_is_observable_on_both_polarities() {
        assert!(section_references_posture(&GIT_INSTANCE), "the real Git instance is accepted");
        let restating = SubsystemReference {
            subsystem: "git",
            cited_anchor: POSTURE_ANCHOR,
            section_text: "Git erasure: per-subject DEK crypto-shred renders messages unrecoverable ...",
        };
        assert!(
            !section_references_posture(&restating),
            "a restating section is rejected (the core returns observable-false — X-7)"
        );
    }

    /// **GIT-D2's residual == the ONE platform-posture residual (§7.2 / §7.4).** The Git instance's
    /// residual IS the canonical residual — confirmed equal, never re-described. The
    /// [`residual_is_the_one_posture`] core is observable on BOTH polarities (a different residual is
    /// rejected — killing the `-> true` mutant).
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
        // The core is observable-false for a DIFFERENT residual (a Git-specific restatement).
        assert!(
            !residual_is_the_one_posture("some Git-specific re-described residual text"),
            "a residual that is NOT the canonical one is rejected (kills the `-> true` mutant)"
        );
        // The canonical residual is the author's-DEK third-party limit (the documented limit, §7.2).
        assert!(
            git_residual().contains("AUTHOR's DEK") && git_residual().contains("not the subject's"),
            "the residual is third-party PII under the AUTHOR's DEK — not shreddable by the subject's key"
        );
    }

    /// **The P-GA-18 prerequisite verdict PASSES over a pseudonym-form commit actor line (the
    /// mandatory-core check).** The frozen 4.8 rendering — the form Git commits hold by default in M3 —
    /// passes. (The REAL-codec firing is `tests/git_d2_pseudonymous_commit.rs`; this is the predicate.)
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

    /// **A real-identity commit actor line FAILS the prerequisite** — the verdict rejects any actor
    /// that would bake erasable PII into the immutable commit hash. Several distinct real-identity
    /// shapes are rejected (kills a `-> true` constant mutant of the mandatory-core roll-up). An EMPTY
    /// input is rejected too (a commit has at least an author actor).
    #[test]
    fn a_real_identity_commit_actor_fails_the_prerequisite() {
        let good = PseudonymHandle::new("psn-7f3a", "acme").unwrap().render();
        // Even ONE real-identity actor among otherwise-good lines fails the whole roll-up.
        for bad in [
            "Ada Lovelace <ada@example.com>",
            "ada@example.com",
            "Ada Lovelace",
            "psn-7f3a@acme.com",   // pseudonym-local but routable domain (wrong suffix)
            "psn-7f3a@acme",       // missing the .noreply suffix
        ] {
            assert!(
                !pseudonym_actor_lines_pass_the_prerequisite(&[&good, bad]),
                "a commit with a real-identity actor {bad:?} must FAIL the prerequisite (would bake PII)"
            );
        }
        // The empty case (not a passing commit) is rejected.
        assert!(
            !pseudonym_actor_lines_pass_the_prerequisite(&[]),
            "an empty actor set is not a passing commit (kills the `all` vacuous-true mutant)"
        );
    }

    /// The M5 history-rewrite floor is named in writing (the rare commit-body expunge) — distinct from
    /// the commit-time pseudonymisation this module confirms.
    #[test]
    fn the_history_rewrite_floor_is_named() {
        assert!(
            HISTORY_REWRITE_FLOOR_PROMPT.contains("P-GA-35"),
            "the audited history-rewrite erasure path is the named M5 follow-on"
        );
    }
}
