use myelin_agent::{EffectKind, ToolDef, ToolName, ToolSurface};
use myelin_git::agent_author as git_author;
use myelin_git::code_tools as git_code;
use myelin_git::rebac_fragment::object_types as git_objects;

use crate::defaults::{
    cap, mutate_tool_def, register_tool_defs, seed_requires_approval, LooseningViolation,
};

pub const GIT_SUBSYSTEM: &str = "git";

pub const GIT_MERGE_TOOL: &str = "merge";

pub const OPEN_PR_TOOL: &str = "open_pr";

pub const GIT_TOOL_VERSION: u32 = 1;

pub fn git_merge_required_caps() -> Vec<String> {
    cap(git_objects::PULL_REQUEST, "merge")
}

pub fn open_pr_required_caps() -> Vec<String> {
    cap(git_objects::REPO, "push")
}

pub fn git_merge_tool_def() -> ToolDef {
    mutate_tool_def(
        GIT_SUBSYSTEM,
        GIT_MERGE_TOOL,
        GIT_TOOL_VERSION,
        r#"{"type":"object","required":["pull_request"],"properties":{"pull_request":{"type":"string"},"strategy":{"type":"string","enum":["merge","squash","rebase"]}}}"#,
        git_merge_required_caps(),
    )
}

pub fn open_pr_tool_def() -> ToolDef {
    mutate_tool_def(
        GIT_SUBSYSTEM,
        OPEN_PR_TOOL,
        GIT_TOOL_VERSION,
        r#"{"type":"object","required":["repo","source_ref","target_ref"],"properties":{"repo":{"type":"string"},"source_ref":{"type":"string"},"target_ref":{"type":"string"},"title":{"type":"string"}}}"#,
        open_pr_required_caps(),
    )
}

pub fn git_history_rewrite_tool_def() -> ToolDef {
    mutate_tool_def(
        git_code::GIT_SUBSYSTEM,
        git_code::HISTORY_REWRITE_TOOL,
        git_code::GIT_CODE_TOOL_VERSION,
        r#"{"type":"object","required":["repo","target_refs","reason_code"],"properties":{"repo":{"type":"string"},"target_refs":{"type":"array","items":{"type":"string"}},"reason_code":{"type":"string"}}}"#,
        git_code::history_rewrite_required_caps(),
    )
}

pub fn git_scip_index_tool_def() -> ToolDef {
    seed_requires_approval(ToolDef {
        name: ToolName(git_code::SCIP_INDEX_TOOL.to_string()),
        subsystem: git_code::GIT_SUBSYSTEM.to_string(),
        version: git_code::GIT_CODE_TOOL_VERSION,
        input_schema: r#"{"type":"object","required":["repo","commit_oid"],"properties":{"repo":{"type":"string"},"commit_oid":{"type":"string"}}}"#.to_string(),
        required_caps: git_code::scip_index_required_caps(),
        effect_kind: EffectKind::Compute,
        side_effecting: false,
        requires_approval: false,
        exposed_over_mcp: false,
    })
}

fn git_author_tool_def(name: &str, input_schema: &str) -> ToolDef {
    mutate_tool_def(
        GIT_SUBSYSTEM,
        name,
        git_author::GIT_AUTHOR_TOOL_VERSION,
        input_schema,
        git_author::review_authoring_required_caps(),
    )
}

pub fn git_comment_tool_def() -> ToolDef {
    git_author_tool_def(
        git_author::COMMENT_TOOL,
        r#"{"type":"object","required":["pull_request","body"],"properties":{"pull_request":{"type":"string"},"body":{"type":"string"},"thread":{"type":"string"}}}"#,
    )
}

pub fn git_submit_review_tool_def() -> ToolDef {
    git_author_tool_def(
        git_author::SUBMIT_REVIEW_TOOL,
        r#"{"type":"object","required":["pull_request","verdict"],"properties":{"pull_request":{"type":"string"},"verdict":{"type":"string","enum":["approve","request_changes","comment"]},"body":{"type":"string"}}}"#,
    )
}

pub fn git_suggest_change_tool_def() -> ToolDef {
    git_author_tool_def(
        git_author::SUGGEST_CHANGE_TOOL,
        r#"{"type":"object","required":["pull_request","path","suggestion"],"properties":{"pull_request":{"type":"string"},"path":{"type":"string"},"suggestion":{"type":"string"}}}"#,
    )
}

pub fn git_resolve_thread_tool_def() -> ToolDef {
    git_author_tool_def(
        git_author::RESOLVE_THREAD_TOOL,
        r#"{"type":"object","required":["pull_request","thread"],"properties":{"pull_request":{"type":"string"},"thread":{"type":"string"}}}"#,
    )
}

pub fn git_author_tool_defs() -> Vec<ToolDef> {
    vec![
        git_comment_tool_def(),
        git_submit_review_tool_def(),
        git_suggest_change_tool_def(),
        git_resolve_thread_tool_def(),
    ]
}

pub fn git_tool_defs() -> Vec<ToolDef> {
    let mut defs = vec![
        git_merge_tool_def(),
        open_pr_tool_def(),
        git_history_rewrite_tool_def(),
        git_scip_index_tool_def(),
    ];
    defs.extend(git_author_tool_defs());
    defs
}

pub fn register_git_tools<S: ToolSurface>(
    surface: &mut S,
) -> Result<Vec<ToolDef>, LooseningViolation> {
    register_tool_defs(surface, git_tool_defs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defaults::{assert_no_silent_loosening, requires_approval_default};

    struct Catalogue {
        defs: Vec<ToolDef>,
    }
    impl ToolSurface for Catalogue {
        fn register_tool(&mut self, def: ToolDef) {
            self.defs.push(def);
        }
        fn resolve(&self, name: &ToolName) -> Option<&ToolDef> {
            self.defs.iter().find(|d| &d.name == name)
        }
    }

    #[test]
    fn git_merge_is_gated_by_the_frozen_default() {
        let def = git_merge_tool_def();
        assert!(
            def.requires_approval,
            "git.merge is HITL-gated (§6.3 / AG-8)"
        );
        assert_eq!(
            def.requires_approval,
            requires_approval_default(GIT_SUBSYSTEM, GIT_MERGE_TOOL),
            "git.merge's gating IS the frozen §6.3 default (seeded, not hand-set)"
        );
        assert_eq!(def.effect_kind, EffectKind::Mutate);
        assert!(def.side_effecting);
    }

    #[test]
    fn open_pr_is_not_gated_by_the_frozen_default() {
        let def = open_pr_tool_def();
        assert!(
            !def.requires_approval,
            "open_pr is reversible → NOT gated (§6.3)"
        );
        assert_eq!(
            def.requires_approval,
            requires_approval_default(GIT_SUBSYSTEM, OPEN_PR_TOOL),
            "open_pr's (non-)gating IS the frozen §6.3 default (seeded, not hand-set)"
        );
        assert_eq!(def.effect_kind, EffectKind::Mutate);
        assert!(def.side_effecting);
    }

    #[test]
    fn required_caps_are_the_git_rebac_fragment_permissions() {
        assert_eq!(
            git_merge_tool_def().required_caps,
            vec!["pull_request.merge".to_string()]
        );
        assert_eq!(
            open_pr_tool_def().required_caps,
            vec!["repo.push".to_string()]
        );
        assert_eq!(git_objects::PULL_REQUEST, "pull_request");
        assert_eq!(git_objects::REPO, "repo");
        let mcp_defs = myelin_git::api::agent_tools();
        assert_eq!(
            mcp_defs
                .iter()
                .find(|d| d.name == "git.merge")
                .unwrap()
                .required_caps,
            git_merge_required_caps()
        );
        assert_eq!(
            mcp_defs
                .iter()
                .find(|d| d.name == "git.open_pr")
                .unwrap()
                .required_caps,
            open_pr_required_caps()
        );
    }

    #[test]
    fn register_git_tools_registers_both_into_the_one_surface() {
        let mut cat = Catalogue { defs: vec![] };
        let registered = register_git_tools(&mut cat).expect("seeded defs always admit");
        assert_eq!(
            registered.len(),
            8,
            "merge + open_pr + history_rewrite + scip_index + comment + submit_review + \
             suggest_change + resolve_thread"
        );

        let merge = cat
            .resolve(&ToolName(GIT_MERGE_TOOL.into()))
            .expect("git.merge registered");
        assert_eq!(merge.subsystem, GIT_SUBSYSTEM);
        assert!(merge.requires_approval, "the registered git.merge is gated");
        assert_eq!(merge.required_caps, vec!["pull_request.merge".to_string()]);

        let pr = cat
            .resolve(&ToolName(OPEN_PR_TOOL.into()))
            .expect("open_pr registered");
        assert_eq!(pr.subsystem, GIT_SUBSYSTEM);
        assert!(!pr.requires_approval, "the registered open_pr is NOT gated");
        assert_eq!(pr.required_caps, vec!["repo.push".to_string()]);

        assert!(cat.resolve(&ToolName("git.delete_repo".into())).is_none());
    }

    #[test]
    fn git_history_rewrite_is_a_gated_mutate_tool() {
        let def = git_history_rewrite_tool_def();
        assert_eq!(def.subsystem, GIT_SUBSYSTEM);
        assert_eq!(def.name.0, myelin_git::code_tools::HISTORY_REWRITE_TOOL);
        assert_eq!(def.effect_kind, EffectKind::Mutate);
        assert!(def.side_effecting);
        assert!(
            def.requires_approval,
            "history-rewrite is HITL-gated (the §6.3 consequential row)"
        );
        assert_eq!(
            def.requires_approval,
            requires_approval_default(GIT_SUBSYSTEM, myelin_git::code_tools::HISTORY_REWRITE_TOOL),
            "the gating IS the frozen §6.3 seed (the consequential history-rewrite row)"
        );
        assert_eq!(def.required_caps, vec!["repo.administer".to_string()]);
        assert!(
            !def.exposed_over_mcp,
            "GF-9: no external MCP endpoint at v1"
        );
    }

    #[test]
    fn git_scip_index_is_a_compute_tool_on_the_unified_sandbox() {
        let def = git_scip_index_tool_def();
        assert_eq!(def.subsystem, GIT_SUBSYSTEM);
        assert_eq!(def.name.0, myelin_git::code_tools::SCIP_INDEX_TOOL);
        assert_eq!(def.effect_kind, EffectKind::Compute);
        assert!(
            !def.side_effecting,
            "a read-only index build is not a mutation"
        );
        assert!(
            !def.requires_approval,
            "a read-only index build is NOT gated"
        );
        assert_eq!(def.required_caps, vec!["repo.pull".to_string()]);
        assert!(
            !def.exposed_over_mcp,
            "GF-9: no external MCP endpoint at v1"
        );
    }

    #[test]
    fn git_author_tools_are_reversible_ungated_mutate_tools() {
        for def in git_author_tool_defs() {
            assert_eq!(def.subsystem, GIT_SUBSYSTEM);
            assert_eq!(
                def.effect_kind,
                EffectKind::Mutate,
                "{} routes through EffectApi",
                def.name.0
            );
            assert!(def.side_effecting);
            assert!(
                !def.requires_approval,
                "{} is reversible authoring → NOT gated",
                def.name.0
            );
            assert_eq!(
                def.requires_approval,
                requires_approval_default(GIT_SUBSYSTEM, &def.name.0),
                "{}'s (non-)gating IS the frozen §6.3 seed (not hand-set)",
                def.name.0
            );
            assert_eq!(
                def.required_caps,
                vec!["pull_request.review".to_string()],
                "{} is governed by the SAME pull_request.review cap a human reviewer is (EI-02 §2)",
                def.name.0
            );
            assert!(!def.exposed_over_mcp);
        }
        let names: Vec<String> = git_author_tool_defs()
            .iter()
            .map(|d| d.name.0.clone())
            .collect();
        assert_eq!(
            names,
            vec![
                "comment",
                "submit_review",
                "suggest_change",
                "resolve_thread"
            ]
        );
    }

    #[test]
    fn all_four_git_tools_are_seeded_from_the_frozen_defaults() {
        let defs = git_tool_defs();
        assert_eq!(
            defs.len(),
            8,
            "merge + open_pr + history_rewrite + scip_index + comment + submit_review + \
             suggest_change + resolve_thread"
        );
        for d in &defs {
            assert_eq!(d.subsystem, GIT_SUBSYSTEM);
            assert_eq!(
                d.requires_approval,
                requires_approval_default(&d.subsystem, &d.name.0),
                "{}.{} gating is the frozen §6.3 seed",
                d.subsystem,
                d.name.0
            );
        }
        let gated: Vec<&str> = defs
            .iter()
            .filter(|d| d.requires_approval)
            .map(|d| d.name.0.as_str())
            .collect();
        assert_eq!(
            gated,
            vec!["merge", "history_rewrite"],
            "exactly two git tools are gated: the merge gate + the consequential history-rewrite"
        );
        assert!(!gated.contains(&"open_pr"));
        assert!(!gated.contains(&"scip_index"));
        assert!(!gated.contains(&"comment"));
        assert!(!gated.contains(&"submit_review"));
        assert!(!gated.contains(&"suggest_change"));
        assert!(!gated.contains(&"resolve_thread"));
        let compute: Vec<&str> = defs
            .iter()
            .filter(|d| d.effect_kind == EffectKind::Compute)
            .map(|d| d.name.0.as_str())
            .collect();
        assert_eq!(
            compute,
            vec!["scip_index"],
            "only SCIP indexing reaches the bare sandbox"
        );
    }

    #[test]
    fn a_hand_loosened_git_merge_registration_is_rejected_loud() {
        let mut loosened = git_merge_tool_def();
        loosened.requires_approval = false;
        let err = assert_no_silent_loosening(&loosened, &[]).unwrap_err();
        assert_eq!(err.subsystem, "git");
        assert_eq!(err.tool, "merge");
        assert!(
            err.to_string().contains("WITHOUT a written deviation"),
            "the loosening is surfaced LOUD: {err}"
        );
    }

    #[test]
    fn the_git_producer_mutations_are_a_projection_not_a_new_engine() {
        let mutations = [git_merge_tool_def(), open_pr_tool_def()];
        for d in &mutations {
            assert_eq!(
                d.effect_kind,
                EffectKind::Mutate,
                "every Git producer MUTATION routes through EffectApi (plan-then-apply) - no new path"
            );
            assert!(d.side_effecting);
            assert_eq!(
                d.requires_approval,
                requires_approval_default(&d.subsystem, &d.name.0),
                "{}.{} gating is the frozen §6.3 seed",
                d.subsystem,
                d.name.0
            );
        }
        assert_ne!(
            mutations[0].requires_approval, mutations[1].requires_approval,
            "git.merge is gated, open_pr is not - the consequential split"
        );
    }
}
