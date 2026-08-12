pub const COMMENT_TOOL: &str = "comment";

pub const SUBMIT_REVIEW_TOOL: &str = "submit_review";

pub const SUGGEST_CHANGE_TOOL: &str = "suggest_change";

pub const RESOLVE_THREAD_TOOL: &str = "resolve_thread";

pub const GIT_AUTHOR_TOOL_VERSION: u32 = 1;

pub const AUTHOR_TOOLS: [&str; 4] = [
    COMMENT_TOOL,
    SUBMIT_REVIEW_TOOL,
    SUGGEST_CHANGE_TOOL,
    RESOLVE_THREAD_TOOL,
];

pub fn review_authoring_required_caps() -> Vec<String> {
    vec![format!(
        "{}.review",
        crate::rebac_fragment::object_types::PULL_REQUEST
    )]
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentAuthorship {
    pub agent_pseudonym: String,
    pub run_id: String,
    pub rationale: String,
}

impl AgentAuthorship {
    pub fn new(
        agent_pseudonym: impl Into<String>,
        run_id: impl Into<String>,
        rationale: impl Into<String>,
    ) -> AgentAuthorship {
        AgentAuthorship {
            agent_pseudonym: agent_pseudonym.into(),
            run_id: run_id.into(),
            rationale: rationale.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Authorship {
    Human { author_pseudonym: String },
    Agent(AgentAuthorship),
}

impl Authorship {
    pub fn is_agent(&self) -> bool {
        matches!(self, Authorship::Agent(_))
    }

    pub fn agent_provenance(&self) -> Option<&AgentAuthorship> {
        match self {
            Authorship::Agent(a) => Some(a),
            Authorship::Human { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_author_tool_identity_constants_are_the_frozen_keys() {
        assert_eq!(COMMENT_TOOL, "comment");
        assert_eq!(SUBMIT_REVIEW_TOOL, "submit_review");
        assert_eq!(SUGGEST_CHANGE_TOOL, "suggest_change");
        assert_eq!(RESOLVE_THREAD_TOOL, "resolve_thread");
        assert_eq!(AUTHOR_TOOLS.len(), 4);
        assert!(AUTHOR_TOOLS.contains(&COMMENT_TOOL));
        assert!(AUTHOR_TOOLS.contains(&SUBMIT_REVIEW_TOOL));
        assert!(AUTHOR_TOOLS.contains(&SUGGEST_CHANGE_TOOL));
        assert!(AUTHOR_TOOLS.contains(&RESOLVE_THREAD_TOOL));
    }

    #[test]
    fn review_authoring_caps_are_the_frozen_pull_request_review_permission() {
        assert_eq!(
            review_authoring_required_caps(),
            vec!["pull_request.review".to_string()]
        );
        assert_eq!(
            crate::rebac_fragment::object_types::PULL_REQUEST,
            "pull_request"
        );
    }

    #[test]
    fn the_pull_request_fragment_declares_the_review_permission() {
        let frag = crate::rebac_fragment::pull_request_fragment();
        assert!(
            frag.permissions.iter().any(|p| p.0 == "review"),
            "the Git `pull_request` fragment declares the `review` permission (4.9) the agent \
             reviewer/commenter is governed by"
        );
    }

    #[test]
    fn an_agent_author_is_legible_with_provenance_never_disguised() {
        let authored = Authorship::Agent(AgentAuthorship::new(
            "psn:agent-7",
            "run:R1",
            "addresses the failing test in src/foo.rs",
        ));
        assert!(
            authored.is_agent(),
            "an agent author is legibly flagged (is_agent)"
        );
        let prov = authored
            .agent_provenance()
            .expect("agent provenance is REQUIRED (AI-Act)");
        assert_eq!(
            prov.agent_pseudonym, "psn:agent-7",
            "which agent (opaque pseudonym)"
        );
        assert_eq!(
            prov.run_id, "run:R1",
            "which run (the traceable provenance link)"
        );
        assert!(
            prov.rationale.contains("failing test"),
            "the why (the rendered rationale)"
        );
    }

    #[test]
    fn a_human_author_is_not_agent_and_has_no_agent_provenance() {
        let human = Authorship::Human {
            author_pseudonym: "psn:human-x".into(),
        };
        assert!(!human.is_agent(), "a human author is NOT flagged agent");
        assert!(
            human.agent_provenance().is_none(),
            "a human author carries no agent provenance"
        );
    }

    #[test]
    fn authorship_is_agent_drives_the_lifecycle_review_is_agent_flag() {
        let agent = Authorship::Agent(AgentAuthorship::new("psn:agent-7", "run:R1", "lgtm"));
        let human = Authorship::Human {
            author_pseudonym: "psn:human-x".into(),
        };
        let agent_review = crate::lifecycle::Review::request("psn:agent-7", agent.is_agent());
        let human_review = crate::lifecycle::Review::request("psn:human-x", human.is_agent());
        assert!(
            agent_review.is_agent,
            "an agent reviewer rides is_agent = true (legibility)"
        );
        assert!(
            !human_review.is_agent,
            "a human reviewer rides is_agent = false"
        );
    }
}
