use myelin_gdpr_service::{commit_actor_holds_only_pseudonym, verdict_for};
use myelin_identity::PseudonymHandle;

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
