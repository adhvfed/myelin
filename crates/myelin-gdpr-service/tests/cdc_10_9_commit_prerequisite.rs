use myelin_gdpr_service::{
    commit_actor_holds_only_pseudonym, verdict_for, COMMIT_IDENTITY_PREREQUISITE,
    PREREQUISITE_GRAMMAR,
};
use myelin_identity::PseudonymHandle;

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
    let note = p.render();
    assert!(note.contains("<pseudonym>@<tenant>.noreply") && note.contains("contract 10.9"));
}

#[test]
fn a_git_commit_actor_in_pseudonym_form_is_accepted() {
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

#[test]
fn a_real_identity_commit_actor_is_rejected() {
    let real = "Ada Lovelace <ada@example.com>";
    assert!(
        !commit_actor_holds_only_pseudonym(real),
        "a real-identity commit actor must FAIL (it would bake erasable PII into the commit hash) - the X-7/EI-04 anti-pattern"
    );
    let v = verdict_for(real);
    assert!(!v.holds_only_pseudonym);
    assert_eq!(
        v.actor_bytes, real,
        "the failing verdict names the offending bytes"
    );
}
