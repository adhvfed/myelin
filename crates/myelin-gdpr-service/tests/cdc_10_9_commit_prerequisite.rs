//! # CDC 10.9 (prerequisite leg) — the pseudonymous-by-default commit-identity prerequisite (P-GA-18 → P-118)
//!
//! **Contract:** index row 10.9, the **prerequisite leg** — GDPR's ONE critical-path obligation: the
//! pseudonymous-by-default commit-identity requirement recorded in M1 as the **commit-time
//! prerequisite Git must satisfy in M3**, before the Git data model freezes (EI-04 §1; gdpr §7.1.2
//! GIT-1; VISION §3 name-your-floors). The grammar is contract 4.8
//! `<pseudonym>@<tenant>.noreply` (owned by Identity; consumed here, never restated). This is the
//! consumer-driven contract test the coverage scanner (P-S21) reads both halves of:
//!
//! - **provider** = GDPR/Audit RECORDS the obligation ([`COMMIT_IDENTITY_PREREQUISITE`]) — the dated,
//!   structured contract obligation that Git's M3 commit data model MUST attribute the commit actor
//!   to the frozen 4.8 grammar. The provider half is COMPLETE here (the recorded obligation + the
//!   architecture-test scaffolding that asserts it).
//! - **consumer** = Git CONSUMES the prerequisite — its commit codec attributes commit actors to the
//!   pseudonym form, and the architecture-test scaffold ([`commit_actor_holds_only_pseudonym`]) FIRES
//!   over Git's real commit bytes. This stub exercises the consumer SHAPE (a Git-like commit-actor
//!   fixture: a pseudonym-form actor passes, a real-identity form fails). The **real** Git consumer
//!   (the scaffold firing over the real commit codec) lands in **P-GA-27 / P-GA-28 → P-256/P-257**.
//!
//! The dated green artifact (2026-06-20): the provider records the obligation (row 10.9, expressed in
//! the frozen 4.8 grammar, naming its M3 enforcer); the consumer-shape verdict accepts a pseudonym
//! actor and rejects a real-identity actor (the bake-PII anti-pattern). If the prerequisite shape or
//! the frozen grammar drifts, this stops compiling/passing — that is the contract.

use myelin_gdpr_service::{
    commit_actor_holds_only_pseudonym, verdict_for, COMMIT_IDENTITY_PREREQUISITE,
    PREREQUISITE_GRAMMAR,
};
use myelin_identity::PseudonymHandle;

/// **provider (10.9 prerequisite leg): GDPR records the obligation.** The dated, structured
/// contract obligation expressed in the frozen 4.8 grammar, naming the M3 band + the enforcing
/// prompt. This is the source Git consumes.
#[test]
fn provider_records_the_commit_identity_prerequisite() {
    let p = COMMIT_IDENTITY_PREREQUISITE;
    assert_eq!(
        p.contract_row, "10.9",
        "the prerequisite leg of contract 10.9"
    );
    assert_eq!(
        p.consumed_grammar_contract, "4.8",
        "expressed in the frozen grammar Identity owns"
    );
    assert_eq!(
        p.required_actor_grammar, PREREQUISITE_GRAMMAR,
        "the commit actor MUST be the frozen <pseudonym>@<tenant>.noreply grammar"
    );
    assert_eq!(
        p.enforced_band, "M3",
        "satisfied in M3, decided in M1 before the data model freezes"
    );
    assert!(
        !p.recorded_on.is_empty(),
        "the recorded obligation is dated"
    );
    assert!(
        p.enforced_by_prompt.contains("P-GA-28") || p.enforced_by_prompt.contains("P-GA-27"),
        "the M3 enforcement follow-on (Git instance) is named in writing"
    );
    // The rendered note is the GATE's green artifact — a dated obligation Git consumes.
    let note = p.render();
    assert!(note.contains("<pseudonym>@<tenant>.noreply") && note.contains("contract 10.9"));
}

/// **consumer (10.9 prerequisite leg): Git consumes the prerequisite.** The Git-like commit-actor
/// SHAPE — a commit attributed to the frozen pseudonym form passes the architecture-test scaffold
/// (the M3 assertion: Git commits hold only the pseudonym). This is the verdict the real P-GA-28
/// consumer runs over Git's commit codec.
#[test]
fn a_git_commit_actor_in_pseudonym_form_is_accepted() {
    // The form a pseudonymous-by-default Git commit must hold in M3 (the bytes in the commit hash).
    let actor = PseudonymHandle::new("anon-7f3a", "acme")
        .expect("valid pseudonym")
        .render();
    assert_eq!(actor, "anon-7f3a@acme.noreply");
    assert!(
        commit_actor_holds_only_pseudonym(&actor),
        "the provider+consumer prerequisite pair: a pseudonym-form commit actor is accepted"
    );
    assert!(verdict_for(&actor).holds_only_pseudonym);
}

/// **The bake-PII anti-pattern is rejected:** a Git commit attributed to a real identity (a routable
/// email + name — the classic git author) FAILS the verdict. This is the property the architecture
/// test enforces over Git's real commit bytes in M3 (P-GA-28): no erasable PII in the immutable hash.
#[test]
fn a_real_identity_commit_actor_is_rejected() {
    let real = "Ada Lovelace <ada@example.com>";
    assert!(
        !commit_actor_holds_only_pseudonym(real),
        "a real-identity commit actor must FAIL (it would bake erasable PII into the commit hash) — the X-7/EI-04 anti-pattern"
    );
    let v = verdict_for(real);
    assert!(!v.holds_only_pseudonym);
    assert_eq!(
        v.actor_bytes, real,
        "the failing verdict names the offending bytes"
    );
}
