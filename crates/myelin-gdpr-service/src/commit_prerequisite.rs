use myelin_identity::PseudonymHandle;

pub const PREREQUISITE_CONTRACT_ROW: &str = "10.9";

pub const PREREQUISITE_GRAMMAR: &str = "<pseudonym>@<tenant>.noreply";

pub const PREREQUISITE_RECORDED_ON: &str = "2026-06-20";

pub const M3_ENFORCEMENT_PROMPT: &str = "P-GA-27 / P-GA-28 (P-256/P-257)";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitIdentityPrerequisite {
    pub contract_row: &'static str,
    pub consumed_grammar_contract: &'static str,
    pub required_actor_grammar: &'static str,
    pub enforced_band: &'static str,
    pub recorded_on: &'static str,
    pub enforced_by_prompt: &'static str,
}

pub const COMMIT_IDENTITY_PREREQUISITE: CommitIdentityPrerequisite = CommitIdentityPrerequisite {
    contract_row: PREREQUISITE_CONTRACT_ROW,
    consumed_grammar_contract: "4.8",
    required_actor_grammar: PREREQUISITE_GRAMMAR,
    enforced_band: "M3",
    recorded_on: PREREQUISITE_RECORDED_ON,
    enforced_by_prompt: M3_ENFORCEMENT_PROMPT,
};

impl CommitIdentityPrerequisite {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitActorVerdict {
    pub actor_bytes: String,
    pub holds_only_pseudonym: bool,
}

#[must_use]
pub fn commit_actor_holds_only_pseudonym(actor_bytes: &str) -> bool {
    PseudonymHandle::parse(actor_bytes).is_some()
}

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

    #[test]
    fn a_pseudonym_form_commit_actor_passes() {
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

    #[test]
    fn a_real_identity_commit_actor_fails() {
        let real_identity_forms = [
            "Ada Lovelace <ada@example.com>",
            "ada@example.com",
            "Ada Lovelace",
            "",
            "anon-7f3a@acme.com",
            "anon-7f3a@acme",
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
