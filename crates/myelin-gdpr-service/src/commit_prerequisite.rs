//! # The pseudonymous-by-default commit-identity prerequisite (P-GA-18 → P-118) — contract 10.9
//!
//! **P-GA-18 → P-118 (M1).** This module records GDPR's **ONE critical-path obligation**: the
//! pseudonymous-by-default commit-identity requirement, written down **NOW** (M1) as the
//! **commit-time prerequisite Git must satisfy in M3**, *before* the Git data model freezes. It
//! ships the recorded obligation **plus the architecture-test scaffolding** that will assert (in
//! M3, P-GA-28) that Git commits hold ONLY the frozen pseudonym form `<pseudonym>@<tenant>.noreply`
//! — never erasable real PII baked into the immutable commit hash.
//!
//! ## Why this is decided NOW (the name-your-floors / decide-before-freeze doctrine)
//! The git author name+email are **baked into the commit hash** (EI-04 §1). You cannot tombstone
//! them without rewriting history and changing every downstream hash — the history-rewrite path is
//! disruptive (the M5 follow-on, P-GA-35). The structural answer is to **never bake erasable PII in
//! the first place**: attribute commits to the stable opaque pseudonym, and keep the person↔pseudonym
//! map as the erasable record (the DSR step-1 shred lever, contract 4.8). EI-04 §1 is explicit:
//! *"decide this BEFORE the git subsystem's data model is fixed; it is nearly impossible to bolt on
//! later."* VISION §3 — name your floors; the prerequisite is decided now, before the data model
//! freezes. This module is that recorded decision (a dated, in-code contract obligation) so the
//! next agent — and the Git M3 prompts — cannot proceed without consuming it.
//!
//! ## Canon
//! - `planning/05-refined-shared-systems-architecture/gdpr-and-audit.md` §7.1.2 (commits
//!   pseudonymous-by-default so the immutable hash never bakes erasable PII; GIT-1, a commit-time
//!   prerequisite) and §7.4 (the Git instance of the ONE posture).
//! - `00-reconciliation-decisions.md` X-7 / OQ-G (the structural floor, lever 2 — the
//!   pseudonym-map shred; commit pseudonymous-by-default, GIT-1).
//! - `contract-index.md` row 10.9 (the prerequisite recorded as a Git-M3 obligation) + row 4.8
//!   (the frozen pseudonym grammar this prerequisite is expressed in, owned by Identity).
//! - `external-insights/04-hard-problems.md` §1 (the git-history immutable hash must never bake
//!   erasable PII; prevent the bake at commit time).
//! - `VISION.md` §3 (name-your-floors — decide the prerequisite before the data model freezes).
//!
//! ## What this module ships (contract 10.9 — the prerequisite leg)
//! 1. [`CommitIdentityPrerequisite`] / [`COMMIT_IDENTITY_PREREQUISITE`] — the **recorded contract
//!    obligation**: a dated, structured statement that Git's M3 commit data model MUST attribute the
//!    commit actor to the frozen pseudonym grammar `<pseudonym>@<tenant>.noreply` (4.8), consumed by
//!    Git (P-GA-27 / P-GA-28 + the Git subsystem). The green artifact of the GATE is this recorded
//!    obligation (a dated note) — it exists, it is the prerequisite, it names its M3 enforcer.
//! 2. [`commit_actor_holds_only_pseudonym`] + [`CommitActorVerdict`] — the **architecture-test
//!    scaffolding** (the verdict): given a commit author/email string, it PASSES iff the bytes are
//!    the frozen pseudonym form (parse as a [`PseudonymHandle`]) and FAILS for any real-identity
//!    form (a routable email, a bare name). This is the predicate the M3 enforcement (P-GA-28)
//!    fires on Git's real commit codec: *Git commits hold only the pseudonym* → this scaffold's
//!    verdict.
//!
//! ## Floor named (deferred → filling prompt)
//! - The **M3 enforcement** — the architecture test firing on Git's *real* commit bytes (Git commits
//!   actually hold only `<pseudonym>@<tenant>.noreply`) — lands with the Git instance of the ONE
//!   posture: **P-GA-27 / P-GA-28 → P-256/P-257**. There the consumer half of the 10.9 CDC pair
//!   completes (Git consuming this prerequisite) and the verdict scaffold runs against Git's commit
//!   codec. Recorded in writing, dated **2026-06-20**.
//! - The **history-rewrite erasure path** (the rare body-expunge, with the disruptive changed-hash
//!   consequence) is the M5 follow-on **P-GA-35** — distinct from this commit-time prevention.
//!
//! ## Mutation floor — the pseudonym-form check is mandatory-core
//! [`commit_actor_holds_only_pseudonym`] is the load-bearing verdict (the prompt's
//! "mandatory-core" check). Its mutation floor is **met by the unit tests below**: a pseudonym-form
//! actor PASSES, and every real-identity form (routable email, bare name, empty, wrong suffix)
//! FAILS — so a `-> true` / `-> false` constant mutant of the predicate is killed by a passing AND
//! a failing case. The floor is the pair {one accept, several reject}, stated and met here.

use myelin_identity::PseudonymHandle;

/// The contract-index row the recorded prerequisite belongs to (the prerequisite leg of 10.9).
pub const PREREQUISITE_CONTRACT_ROW: &str = "10.9";

/// The frozen pseudonym grammar the prerequisite is expressed in (contract 4.8, owned by Identity —
/// consumed here, never restated). The recorded obligation references THIS grammar by name.
pub const PREREQUISITE_GRAMMAR: &str = "<pseudonym>@<tenant>.noreply";

/// The dated note that is the GATE's green artifact: the prerequisite is RECORDED as a Git-M3
/// obligation, before the Git data model freezes. A claim that outlives its verification misleads
/// the next agent (VISION §3) — so it is dated.
pub const PREREQUISITE_RECORDED_ON: &str = "2026-06-20";

/// The prompt id that ENFORCES this prerequisite in M3 (the architecture test fires on Git's real
/// commit bytes there). Named in writing so the floor is never pretended-solved.
pub const M3_ENFORCEMENT_PROMPT: &str = "P-GA-27 / P-GA-28 (P-256/P-257)";

/// **The recorded pseudonymous-by-default commit-identity prerequisite (contract 10.9, the
/// prerequisite leg).** GDPR's ONE critical-path obligation, decided NOW (M1) before the Git data
/// model freezes (EI-04 §1; VISION §3): Git's M3 commit data model MUST attribute the commit actor
/// to the frozen pseudonym grammar `<pseudonym>@<tenant>.noreply` (4.8), so the immutable commit
/// hash never bakes in erasable real PII. Git (P-GA-27 / P-GA-28 + the Git subsystem) consumes this.
///
/// This is a **recorded obligation**, not an executable mechanism: the mechanism (Git's commit
/// codec) lands in M3. The architecture-test scaffolding that will assert the obligation is
/// [`commit_actor_holds_only_pseudonym`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitIdentityPrerequisite {
    /// The contract row this obligation is recorded under ([`PREREQUISITE_CONTRACT_ROW`] = `10.9`).
    pub contract_row: &'static str,
    /// The consumed grammar contract (4.8, owned by Identity) the obligation is expressed in.
    pub consumed_grammar_contract: &'static str,
    /// The frozen grammar the commit actor MUST be ([`PREREQUISITE_GRAMMAR`]).
    pub required_actor_grammar: &'static str,
    /// The band in which the prerequisite must be SATISFIED (M3 — when the Git data model is built),
    /// recorded in M1 (the band in which it is DECIDED).
    pub enforced_band: &'static str,
    /// The date this obligation was recorded ([`PREREQUISITE_RECORDED_ON`]) — a dated status note.
    pub recorded_on: &'static str,
    /// The prompt that enforces the obligation in M3 ([`M3_ENFORCEMENT_PROMPT`]) — the named floor.
    pub enforced_by_prompt: &'static str,
}

/// The ONE recorded prerequisite instance (contract 10.9, the prerequisite leg). The single
/// in-process record Git consumes (P-GA-27 / P-GA-28). Decided NOW, before the Git data model
/// freezes — GDPR's ONE critical-path obligation.
pub const COMMIT_IDENTITY_PREREQUISITE: CommitIdentityPrerequisite = CommitIdentityPrerequisite {
    contract_row: PREREQUISITE_CONTRACT_ROW,
    consumed_grammar_contract: "4.8",
    required_actor_grammar: PREREQUISITE_GRAMMAR,
    enforced_band: "M3",
    recorded_on: PREREQUISITE_RECORDED_ON,
    enforced_by_prompt: M3_ENFORCEMENT_PROMPT,
};

impl CommitIdentityPrerequisite {
    /// Render the recorded obligation as a dated note (the GATE's green artifact — a recorded
    /// obligation Git consumes). Stable, deterministic text.
    #[must_use]
    pub fn render(&self) -> String {
        format!(
            "PREREQUISITE (contract {}, recorded {}): Git's M3 commit data model MUST attribute the \
             commit actor to the frozen pseudonym grammar `{}` (contract {}), so the immutable \
             commit hash never bakes erasable real PII (EI-04 §1; gdpr §7.1.2 GIT-1). Decided in M1, \
             enforced in {} by {}. GDPR's ONE critical-path obligation.",
            self.contract_row,
            self.recorded_on,
            self.required_actor_grammar,
            self.consumed_grammar_contract,
            self.enforced_band,
            self.enforced_by_prompt,
        )
    }
}

/// The verdict of the commit-identity architecture test (the scaffold that fires in M3). Carries the
/// candidate actor bytes and whether they hold ONLY the pseudonym form — so a failing verdict names
/// the offending bytes (never silently coerced).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitActorVerdict {
    /// The candidate commit author/email bytes that were inspected.
    pub actor_bytes: String,
    /// `true` iff the bytes are the frozen pseudonym form `<pseudonym>@<tenant>.noreply` (4.8) and
    /// carry no real-identity PII.
    pub holds_only_pseudonym: bool,
}

/// **The architecture-test scaffolding (the mandatory-core verdict).** Given a commit author/email
/// string, return `true` iff it is the frozen pseudonym form `<pseudonym>@<tenant>.noreply`
/// (contract 4.8) — i.e. it parses as a [`PseudonymHandle`]. Any real-identity form (a routable
/// email, a bare name, an empty string, a wrong-suffix string) returns `false`.
///
/// This is the predicate the M3 enforcement (P-GA-28) runs over Git's REAL commit codec: *Git
/// commits hold only the pseudonym* → this verdict. The scaffold is complete and tested here against
/// fixtures; it FIRES on Git's commit bytes in M3. Because it delegates the grammar to the frozen
/// [`PseudonymHandle::parse`] (the single source of truth, owned by Identity), a drift in the
/// `@`/`.noreply` shape is a compile/parse break at the owner — never a silent PII leak here.
#[must_use]
pub fn commit_actor_holds_only_pseudonym(actor_bytes: &str) -> bool {
    PseudonymHandle::parse(actor_bytes).is_some()
}

/// The full verdict ([`CommitActorVerdict`]) for a candidate commit actor — the bytes plus the
/// pass/fail. The M3 enforcement records one of these per commit-codec fixture; a failing verdict
/// names the offending real-identity bytes.
#[must_use]
pub fn verdict_for(actor_bytes: &str) -> CommitActorVerdict {
    CommitActorVerdict {
        actor_bytes: actor_bytes.to_string(),
        holds_only_pseudonym: commit_actor_holds_only_pseudonym(actor_bytes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The prerequisite is RECORDED as a Git-M3 obligation expressed in the frozen 4.8 grammar,
    /// before the Git data model freezes — GDPR's ONE critical-path obligation, dated.
    #[test]
    fn the_prerequisite_is_recorded_as_a_git_m3_obligation() {
        let p = COMMIT_IDENTITY_PREREQUISITE;
        assert_eq!(p.contract_row, "10.9", "owns the prerequisite leg of 10.9");
        assert_eq!(
            p.consumed_grammar_contract, "4.8",
            "consumes the frozen grammar (Identity owns)"
        );
        assert_eq!(
            p.required_actor_grammar, "<pseudonym>@<tenant>.noreply",
            "the recorded obligation references the frozen grammar, never restates it"
        );
        assert_eq!(
            p.enforced_band, "M3",
            "satisfied in M3 (decided in M1, before the data model freezes)"
        );
        assert!(
            !p.recorded_on.is_empty(),
            "the recorded obligation is dated (a claim must not outlive its verification)"
        );
        assert!(
            p.enforced_by_prompt.contains("P-GA-28") || p.enforced_by_prompt.contains("P-GA-27"),
            "the M3 enforcement follow-on is named in writing"
        );
    }

    /// The rendered obligation is the GATE's green artifact: a dated note naming the grammar, the
    /// band, and the M3 enforcer. Deterministic text Git consumes.
    #[test]
    fn the_recorded_obligation_renders_a_dated_note() {
        let note = COMMIT_IDENTITY_PREREQUISITE.render();
        assert!(
            note.contains("<pseudonym>@<tenant>.noreply"),
            "the note references the frozen grammar"
        );
        assert!(
            note.contains("contract 10.9"),
            "the note records the contract row"
        );
        assert!(note.contains("2026-06-20"), "the note is dated");
        assert!(note.contains("M3"), "the note records the enforcement band");
        assert!(
            note.contains("critical-path obligation"),
            "the note states it is GDPR's ONE obligation"
        );
    }

    /// **The architecture-test scaffold's verdict: a fixture commit-actor in the frozen pseudonym
    /// form PASSES.** This is the M3 assertion shape (P-GA-28): Git commits hold only the pseudonym.
    #[test]
    fn a_pseudonym_form_commit_actor_passes() {
        // The frozen 4.8 rendering — the form Git commits must hold by default in M3.
        let handle = PseudonymHandle::new("anon-7f3a", "acme").expect("valid pseudonym");
        let actor = handle.render();
        assert_eq!(actor, "anon-7f3a@acme.noreply");
        assert!(
            commit_actor_holds_only_pseudonym(&actor),
            "a commit actor in the frozen pseudonym form passes the scaffold's verdict"
        );
        let v = verdict_for(&actor);
        assert!(v.holds_only_pseudonym);
        assert_eq!(v.actor_bytes, "anon-7f3a@acme.noreply");
    }

    /// **A real-identity commit-actor form FAILS the verdict** — the scaffold rejects any bytes that
    /// would bake erasable PII into the immutable commit hash. Several distinct real-identity shapes
    /// are rejected (kills a `-> true` constant mutant of the mandatory-core predicate).
    #[test]
    fn a_real_identity_commit_actor_fails() {
        let real_identity_forms = [
            "Ada Lovelace <ada@example.com>", // a routable email + real name (the classic git author)
            "ada@example.com",                // a bare routable email
            "Ada Lovelace",                   // a bare real name
            "",                               // empty
            "anon-7f3a@acme.com", // pseudonym-LOCAL but ROUTABLE domain (wrong suffix) — must fail
            "anon-7f3a@acme",     // missing the .noreply suffix entirely
        ];
        for form in real_identity_forms {
            assert!(
                !commit_actor_holds_only_pseudonym(form),
                "a real-identity / non-conforming commit actor {form:?} must FAIL the verdict (would bake PII)"
            );
            assert!(
                !verdict_for(form).holds_only_pseudonym,
                "the verdict for {form:?} is a failure"
            );
        }
    }

    /// The verdict carries the offending bytes so a failure is never silent — a failing verdict
    /// names exactly what was rejected (the M3 enforcement reports the offending commit bytes).
    #[test]
    fn a_failing_verdict_names_the_offending_bytes() {
        let v = verdict_for("ada@example.com");
        assert!(!v.holds_only_pseudonym);
        assert_eq!(
            v.actor_bytes, "ada@example.com",
            "the failing verdict names the rejected bytes"
        );
    }
}
