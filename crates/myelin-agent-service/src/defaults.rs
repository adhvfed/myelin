use myelin_agent::{EffectKind, ToolDef, ToolName, ToolSurface};

pub fn requires_approval_default(subsystem: &str, tool: &str) -> bool {
    match (subsystem, tool) {
        ("ci", "deploy") => true,
        ("ci", "approve_deploy") => true,
        ("ci", "rollback") => true,
        ("ci", "write_secret") => true,
        ("ci", "run") => false,
        ("ci", "run_pipeline") => false,
        ("ci", "cancel_run") => false,
        ("ci", "retry_run") => false,
        ("ci", "read_log") => false,
        ("ci", "read_run") => false,
        ("ci", "validate") => false,
        ("ci", "plan") => false,

        ("git", "merge") => true,
        ("git", "open_pr") => false,
        ("git", "write_file") => false,
        ("git", "comment") => false,
        ("git", "submit_review") => false,
        ("git", "endorse_fork_ci") => false,
        ("git", "suggest_change") => false,
        ("git", "resolve_thread") => false,
        ("git", "history_rewrite") => true,
        ("git", "scip_index") => false,
        ("git", "list_repositories") => false,
        ("git", "read_file") => false,
        ("git", "search_code") => false,

        ("issues", "forecast") => false,
        ("issues", "list") => false,
        ("issues", "view") => false,
        ("issues", "triage") => false,
        ("issues", "sla_draft") => false,
        ("issues", "create") => false,
        ("issues", "update") => false,
        ("issues", "comment") => false,
        ("issues", "link") => false,
        ("issues", "estimate") => false,
        ("issues", "reorder") => false,
        ("issues", "assign") => false,
        ("issues", "transition") => true,
        ("issues", "close") => true,

        ("knowledge", "publish") => true,
        ("knowledge", "edit_confidential") => true,
        ("knowledge", "draft") => false,
        ("knowledge", "comment") => false,
        ("knowledge", "list_pages") => false,
        ("knowledge", "read_page") => false,
        ("knowledge", "link_work") => false,

        ("chat", "post_message") => false,
        ("chat", "post") => false,
        ("chat", "reply_in_thread") => false,
        ("chat", "react") => false,
        ("chat", "list_conversations") => false,
        ("chat", "read_messages") => false,
        ("chat", "start_dm") => false,
        ("chat", "create_channel") => true,
        ("chat", "invite") => true,
        ("chat", "archive_channel") => true,

        ("projects", "list") => false,

        ("workspace", "read_file") => false,
        ("workspace", "write_file") => false,

        _ => true,
    }
}

pub fn requires_approval_for_landing(
    _invoking_subsystem: &str,
    landing_subsystem: &str,
    tool: &str,
) -> bool {
    requires_approval_default(landing_subsystem, tool)
}

pub fn seed_requires_approval(mut def: ToolDef) -> ToolDef {
    def.requires_approval = requires_approval_default(&def.subsystem, &def.name.0);
    def
}

pub fn mutate_tool_def(
    subsystem: &str,
    name: &str,
    version: u32,
    input_schema: &str,
    required_caps: Vec<String>,
) -> ToolDef {
    seed_requires_approval(ToolDef {
        name: ToolName(name.to_string()),
        subsystem: subsystem.to_string(),
        version,
        input_schema: input_schema.to_string(),
        required_caps,
        effect_kind: EffectKind::Mutate,
        side_effecting: true,
        requires_approval: false,
        exposed_over_mcp: false,
    })
}

pub fn cap(object_type: &str, permission: &str) -> Vec<String> {
    vec![format!("{object_type}.{permission}")]
}

pub fn register_tool_defs<S: ToolSurface>(
    surface: &mut S,
    defs: Vec<ToolDef>,
) -> Result<Vec<ToolDef>, LooseningViolation> {
    for def in &defs {
        assert_no_silent_loosening(def, &[])?;
    }
    for def in &defs {
        surface.register_tool(def.clone());
    }
    Ok(defs)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WrittenDeviation {
    pub subsystem: String,
    pub tool: String,
    pub rationale: String,
}

impl WrittenDeviation {
    pub fn new(
        subsystem: impl Into<String>,
        tool: impl Into<String>,
        rationale: impl Into<String>,
    ) -> WrittenDeviation {
        WrittenDeviation {
            subsystem: subsystem.into(),
            tool: tool.into(),
            rationale: rationale.into(),
        }
    }

    fn authorises(&self, subsystem: &str, tool: &str) -> bool {
        self.subsystem == subsystem && self.tool == tool && !self.rationale.trim().is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LooseningViolation {
    pub subsystem: String,
    pub tool: String,
}

impl core::fmt::Display for LooseningViolation {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "registration loosens the frozen requires_approval=yes default for {}.{} to no WITHOUT a \
             written deviation (VISION §3: a consequential action may not be silently un-gated)",
            self.subsystem, self.tool
        )
    }
}

impl std::error::Error for LooseningViolation {}

pub fn assert_no_silent_loosening(
    def: &ToolDef,
    deviations: &[WrittenDeviation],
) -> Result<(), LooseningViolation> {
    let frozen = requires_approval_default(&def.subsystem, &def.name.0);
    if frozen && !def.requires_approval {
        let authorised = deviations
            .iter()
            .any(|d| d.authorises(&def.subsystem, &def.name.0));
        if !authorised {
            return Err(LooseningViolation {
                subsystem: def.subsystem.clone(),
                tool: def.name.0.clone(),
            });
        }
    }
    Ok(())
}

pub fn default_for_tool(subsystem: &str, name: &ToolName) -> bool {
    requires_approval_default(subsystem, &name.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_agent::EffectKind;

    fn tool_def(subsystem: &str, name: &str, requires_approval: bool) -> ToolDef {
        ToolDef {
            name: ToolName(name.into()),
            subsystem: subsystem.into(),
            version: 1,
            input_schema: "{}".into(),
            required_caps: vec![],
            effect_kind: EffectKind::Mutate,
            side_effecting: true,
            requires_approval,
            exposed_over_mcp: false,
        }
    }

    #[test]
    fn the_frozen_6_3_defaults_table_is_seeded_verbatim() {
        assert!(
            requires_approval_default("ci", "deploy"),
            "CI deploy is gated (consequential)"
        );
        assert!(
            requires_approval_default("ci", "approve_deploy"),
            "CI approve_deploy is gated"
        );
        assert!(
            requires_approval_default("ci", "write_secret"),
            "CI write_secret is gated"
        );
        assert!(
            !requires_approval_default("ci", "run_pipeline"),
            "CI non-prod pipeline is NOT gated"
        );

        assert!(
            requires_approval_default("git", "merge"),
            "git.merge is gated (AG-8)"
        );
        assert!(
            !requires_approval_default("git", "open_pr"),
            "open_pr is reversible → NOT gated"
        );
        assert!(
            requires_approval_default("git", "history_rewrite"),
            "history-rewrite is gated (changes every downstream hash - consequential)"
        );
        assert!(
            !requires_approval_default("git", "scip_index"),
            "SCIP indexing is a read-only index build → NOT gated (governed by AG-D4, not HITL)"
        );

        assert!(
            !requires_approval_default("git", "comment"),
            "git.comment is reversible → NOT gated"
        );
        assert!(
            !requires_approval_default("git", "submit_review"),
            "git.submit_review is reversible → NOT gated"
        );
        assert!(
            !requires_approval_default("git", "suggest_change"),
            "git.suggest_change is reversible → NOT gated"
        );
        assert!(
            !requires_approval_default("git", "resolve_thread"),
            "git.resolve_thread is reversible → NOT gated"
        );
        for read in ["list_repositories", "read_file", "search_code"] {
            assert!(
                !requires_approval_default("git", read),
                "git.{read} is read-only → NOT gated"
            );
        }

        assert!(
            !requires_approval_default("issues", "forecast"),
            "forecast is advisory → NOT gated"
        );
        assert!(
            !requires_approval_default("issues", "triage"),
            "triage is advisory → NOT gated"
        );
        assert!(
            !requires_approval_default("issues", "sla_draft"),
            "sla_draft is advisory → NOT gated"
        );
        assert!(
            requires_approval_default("issues", "transition"),
            "SLA transition is caveat-gated (floor)"
        );

        assert!(
            requires_approval_default("knowledge", "publish"),
            "publish is gated (consequential)"
        );
        assert!(
            requires_approval_default("knowledge", "edit_confidential"),
            "confidential edit is gated"
        );
        assert!(
            !requires_approval_default("knowledge", "draft"),
            "draft is reversible → NOT gated"
        );
        assert!(
            !requires_approval_default("knowledge", "comment"),
            "comment is reversible → NOT gated"
        );
        for read in ["list_pages", "read_page"] {
            assert!(
                !requires_approval_default("knowledge", read),
                "knowledge.{read} is read-only → NOT gated"
            );
        }
        for read in ["list_conversations", "read_messages"] {
            assert!(
                !requires_approval_default("chat", read),
                "chat.{read} is read-only → NOT gated"
            );
        }

        assert!(
            !requires_approval_default("chat", "post_message"),
            "post_message is reversible → NOT gated"
        );
        assert!(
            !requires_approval_default("chat", "react"),
            "react is reversible → NOT gated"
        );
    }

    #[test]
    fn an_unknown_action_is_gated_fail_closed() {
        assert!(
            requires_approval_default("ci", "nuke_prod"),
            "an unknown action is gated (fail-closed)"
        );
        assert!(
            requires_approval_default("brand_new_subsystem", "anything"),
            "unknown subsystem → gated"
        );
    }

    #[test]
    fn cross_subsystem_effect_inherits_the_landing_subsystems_default() {
        assert!(
            requires_approval_for_landing("chat", "git", "merge"),
            "a chat-invoked git.merge is governed where it LANDS (git → gated)"
        );
        assert!(
            !requires_approval_for_landing("chat", "issues", "forecast"),
            "a chat-invoked issues.forecast lands in issues → advisory (NOT gated)"
        );
        assert_eq!(
            requires_approval_for_landing("git", "git", "merge"),
            requires_approval_default("git", "merge"),
            "invoking == landing collapses to the plain default"
        );
    }

    #[test]
    fn seed_stamps_the_frozen_default_onto_the_tool_def() {
        let wrong = tool_def("git", "merge", false);
        let seeded = seed_requires_approval(wrong);
        assert!(
            seeded.requires_approval,
            "git.merge is seeded gated regardless of the input value"
        );

        let wrong_pr = tool_def("git", "open_pr", true);
        let seeded_pr = seed_requires_approval(wrong_pr);
        assert!(!seeded_pr.requires_approval, "open_pr is seeded NOT gated");

        let already = tool_def("git", "merge", true);
        assert_eq!(
            seed_requires_approval(already.clone()),
            seed_requires_approval(already)
        );
    }

    #[test]
    fn loosening_a_frozen_yes_without_a_deviation_is_rejected() {
        let loosened = tool_def("git", "merge", false);
        let err = assert_no_silent_loosening(&loosened, &[]).unwrap_err();
        assert_eq!(err.subsystem, "git");
        assert_eq!(err.tool, "merge");
        assert!(
            err.to_string().contains("WITHOUT a written deviation"),
            "the violation is surfaced LOUD: {err}"
        );

        let dev = WrittenDeviation::new("git", "merge", "tenant policy: auto-merge bot, audited");
        assert!(
            assert_no_silent_loosening(&loosened, std::slice::from_ref(&dev)).is_ok(),
            "a written deviation authorises the loosening"
        );

        let other = WrittenDeviation::new("ci", "deploy", "unrelated");
        assert!(
            assert_no_silent_loosening(&loosened, &[other]).is_err(),
            "a deviation for another tool does not authorise this one"
        );

        let empty = WrittenDeviation::new("git", "merge", "   ");
        assert!(
            assert_no_silent_loosening(&loosened, &[empty]).is_err(),
            "an empty-rationale deviation is not a real written deviation"
        );
    }

    #[test]
    fn tightening_a_frozen_no_is_always_allowed() {
        let tightened = tool_def("git", "open_pr", true);
        assert!(
            assert_no_silent_loosening(&tightened, &[]).is_ok(),
            "tightening (no → yes) needs no deviation"
        );
        let chat_tight = tool_def("chat", "post_message", true);
        assert!(assert_no_silent_loosening(&chat_tight, &[]).is_ok());
    }

    #[test]
    fn a_registration_matching_the_frozen_default_is_admitted() {
        let gated = tool_def("git", "merge", true);
        assert!(assert_no_silent_loosening(&gated, &[]).is_ok());
        let ungated = tool_def("git", "open_pr", false);
        assert!(assert_no_silent_loosening(&ungated, &[]).is_ok());
    }

    #[test]
    fn default_for_tool_matches_the_str_form() {
        assert_eq!(
            default_for_tool("git", &ToolName("merge".into())),
            requires_approval_default("git", "merge")
        );
    }
}
