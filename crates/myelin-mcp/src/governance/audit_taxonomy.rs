use myelin_agent::McpApprovalContract;

use super::AuditPhase;

#[derive(Clone, Copy)]
pub(super) enum EffectAuditOutcome {
    Attempted,
    Applied,
    Gated,
    Denied,
    Indeterminate,
}

struct EffectAuditEvents {
    attempted: &'static str,
    applied: &'static str,
    gated: &'static str,
    denied: &'static str,
    indeterminate: &'static str,
}

impl EffectAuditEvents {
    fn event_for(&self, outcome: EffectAuditOutcome) -> &'static str {
        match outcome {
            EffectAuditOutcome::Attempted => self.attempted,
            EffectAuditOutcome::Applied => self.applied,
            EffectAuditOutcome::Gated => self.gated,
            EffectAuditOutcome::Denied => self.denied,
            EffectAuditOutcome::Indeterminate => self.indeterminate,
        }
    }
}

pub(super) fn effect_event_type(
    tool: &str,
    outcome: EffectAuditOutcome,
) -> Result<&'static str, String> {
    let events = match tool {
        "git.merge" => EffectAuditEvents {
            attempted: myelin_git::events::GIT_MERGE_ATTEMPTED,
            applied: myelin_git::events::GIT_MERGE_APPLIED,
            gated: myelin_git::events::GIT_MERGE_GATED,
            denied: myelin_git::events::GIT_MERGE_DENIED,
            indeterminate: myelin_git::events::GIT_MERGE_INDETERMINATE,
        },
        "git.open_pr" => EffectAuditEvents {
            attempted: myelin_git::events::GIT_OPEN_PR_ATTEMPTED,
            applied: myelin_git::events::GIT_OPEN_PR_APPLIED,
            gated: myelin_git::events::GIT_OPEN_PR_GATED,
            denied: myelin_git::events::GIT_OPEN_PR_DENIED,
            indeterminate: myelin_git::events::GIT_OPEN_PR_INDETERMINATE,
        },
        "git.write_file" => EffectAuditEvents {
            attempted: myelin_git::events::GIT_WRITE_FILE_ATTEMPTED,
            applied: myelin_git::events::GIT_WRITE_FILE_APPLIED,
            gated: myelin_git::events::GIT_WRITE_FILE_GATED,
            denied: myelin_git::events::GIT_WRITE_FILE_DENIED,
            indeterminate: myelin_git::events::GIT_WRITE_FILE_INDETERMINATE,
        },
        "git.submit_review" => EffectAuditEvents {
            attempted: myelin_git::events::GIT_SUBMIT_REVIEW_ATTEMPTED,
            applied: myelin_git::events::GIT_SUBMIT_REVIEW_APPLIED,
            gated: myelin_git::events::GIT_SUBMIT_REVIEW_GATED,
            denied: myelin_git::events::GIT_SUBMIT_REVIEW_DENIED,
            indeterminate: myelin_git::events::GIT_SUBMIT_REVIEW_INDETERMINATE,
        },
        "git.comment" => EffectAuditEvents {
            attempted: myelin_git::events::GIT_COMMENT_ATTEMPTED,
            applied: myelin_git::events::GIT_COMMENT_APPLIED,
            gated: myelin_git::events::GIT_COMMENT_GATED,
            denied: myelin_git::events::GIT_COMMENT_DENIED,
            indeterminate: myelin_git::events::GIT_COMMENT_INDETERMINATE,
        },
        "git.reply" => EffectAuditEvents {
            attempted: myelin_git::events::GIT_REPLY_ATTEMPTED,
            applied: myelin_git::events::GIT_REPLY_APPLIED,
            gated: myelin_git::events::GIT_REPLY_GATED,
            denied: myelin_git::events::GIT_REPLY_DENIED,
            indeterminate: myelin_git::events::GIT_REPLY_INDETERMINATE,
        },
        "git.resolve_thread" => EffectAuditEvents {
            attempted: myelin_git::events::GIT_RESOLVE_THREAD_ATTEMPTED,
            applied: myelin_git::events::GIT_RESOLVE_THREAD_APPLIED,
            gated: myelin_git::events::GIT_RESOLVE_THREAD_GATED,
            denied: myelin_git::events::GIT_RESOLVE_THREAD_DENIED,
            indeterminate: myelin_git::events::GIT_RESOLVE_THREAD_INDETERMINATE,
        },
        "git.endorse_fork_ci" => EffectAuditEvents {
            attempted: myelin_git::events::GIT_ENDORSE_FORK_CI_ATTEMPTED,
            applied: myelin_git::events::GIT_ENDORSE_FORK_CI_APPLIED,
            gated: myelin_git::events::GIT_ENDORSE_FORK_CI_GATED,
            denied: myelin_git::events::GIT_ENDORSE_FORK_CI_DENIED,
            indeterminate: myelin_git::events::GIT_ENDORSE_FORK_CI_INDETERMINATE,
        },
        "chat.post" => EffectAuditEvents {
            attempted: myelin_chat::events::CHAT_POST_ATTEMPTED,
            applied: myelin_chat::events::CHAT_POST_APPLIED,
            gated: myelin_chat::events::CHAT_POST_GATED,
            denied: myelin_chat::events::CHAT_POST_DENIED,
            indeterminate: myelin_chat::events::CHAT_POST_INDETERMINATE,
        },
        "chat.reply" => EffectAuditEvents {
            attempted: myelin_chat::events::CHAT_REPLY_ATTEMPTED,
            applied: myelin_chat::events::CHAT_REPLY_APPLIED,
            gated: myelin_chat::events::CHAT_REPLY_GATED,
            denied: myelin_chat::events::CHAT_REPLY_DENIED,
            indeterminate: myelin_chat::events::CHAT_REPLY_INDETERMINATE,
        },
        "issues.create" => EffectAuditEvents {
            attempted: myelin_issues::events::ISSUE_CREATE_ATTEMPTED,
            applied: myelin_issues::events::ISSUE_CREATE_APPLIED,
            gated: myelin_issues::events::ISSUE_CREATE_GATED,
            denied: myelin_issues::events::ISSUE_CREATE_DENIED,
            indeterminate: myelin_issues::events::ISSUE_CREATE_INDETERMINATE,
        },
        "issues.close" => EffectAuditEvents {
            attempted: myelin_issues::events::ISSUE_CLOSE_ATTEMPTED,
            applied: myelin_issues::events::ISSUE_CLOSE_APPLIED,
            gated: myelin_issues::events::ISSUE_CLOSE_GATED,
            denied: myelin_issues::events::ISSUE_CLOSE_DENIED,
            indeterminate: myelin_issues::events::ISSUE_CLOSE_INDETERMINATE,
        },
        "ci.retry_run" => EffectAuditEvents {
            attempted: myelin_ci_controlplane::events::CI_RETRY_RUN_ATTEMPTED,
            applied: myelin_ci_controlplane::events::CI_RETRY_RUN_APPLIED,
            gated: myelin_ci_controlplane::events::CI_RETRY_RUN_GATED,
            denied: myelin_ci_controlplane::events::CI_RETRY_RUN_DENIED,
            indeterminate: myelin_ci_controlplane::events::CI_RETRY_RUN_INDETERMINATE,
        },
        "ci.cancel_run" => EffectAuditEvents {
            attempted: myelin_ci_controlplane::events::CI_CANCEL_RUN_ATTEMPTED,
            applied: myelin_ci_controlplane::events::CI_CANCEL_RUN_APPLIED,
            gated: myelin_ci_controlplane::events::CI_CANCEL_RUN_GATED,
            denied: myelin_ci_controlplane::events::CI_CANCEL_RUN_DENIED,
            indeterminate: myelin_ci_controlplane::events::CI_CANCEL_RUN_INDETERMINATE,
        },
        "knowledge.create_page" => EffectAuditEvents {
            attempted: myelin_content::events::KNOWLEDGE_CREATE_PAGE_ATTEMPTED,
            applied: myelin_content::events::KNOWLEDGE_CREATE_PAGE_APPLIED,
            gated: myelin_content::events::KNOWLEDGE_CREATE_PAGE_GATED,
            denied: myelin_content::events::KNOWLEDGE_CREATE_PAGE_DENIED,
            indeterminate: myelin_content::events::KNOWLEDGE_CREATE_PAGE_INDETERMINATE,
        },
        "knowledge.link_work" => EffectAuditEvents {
            attempted: myelin_content::events::KNOWLEDGE_LINK_WORK_ATTEMPTED,
            applied: myelin_content::events::KNOWLEDGE_LINK_WORK_APPLIED,
            gated: myelin_content::events::KNOWLEDGE_LINK_WORK_GATED,
            denied: myelin_content::events::KNOWLEDGE_LINK_WORK_DENIED,
            indeterminate: myelin_content::events::KNOWLEDGE_LINK_WORK_INDETERMINATE,
        },
        "workspace.write_file" => EffectAuditEvents {
            attempted: myelin_agent_service::workspace_events::WORKSPACE_WRITE_FILE_ATTEMPTED,
            applied: myelin_agent_service::workspace_events::WORKSPACE_WRITE_FILE_APPLIED,
            gated: myelin_agent_service::workspace_events::WORKSPACE_WRITE_FILE_GATED,
            denied: myelin_agent_service::workspace_events::WORKSPACE_WRITE_FILE_DENIED,
            indeterminate:
                myelin_agent_service::workspace_events::WORKSPACE_WRITE_FILE_INDETERMINATE,
        },
        "workspace.exec" => EffectAuditEvents {
            attempted: myelin_agent_service::workspace_events::WORKSPACE_EXEC_ATTEMPTED,
            applied: myelin_agent_service::workspace_events::WORKSPACE_EXEC_APPLIED,
            gated: myelin_agent_service::workspace_events::WORKSPACE_EXEC_GATED,
            denied: myelin_agent_service::workspace_events::WORKSPACE_EXEC_DENIED,
            indeterminate: myelin_agent_service::workspace_events::WORKSPACE_EXEC_INDETERMINATE,
        },
        _ => return Err("governance audit refused an unregistered tool/outcome taxonomy".into()),
    };
    Ok(events.event_for(outcome))
}

pub(super) fn approval_event_type(
    contract: McpApprovalContract,
    phase: AuditPhase,
) -> Result<&'static str, String> {
    match contract {
        McpApprovalContract::GitMerge => match phase {
            AuditPhase::Approved => Ok(myelin_git::events::GIT_MERGE_APPROVED),
            AuditPhase::Rejected => Ok(myelin_git::events::GIT_MERGE_REJECTED),
            AuditPhase::Expired => Ok(myelin_git::events::GIT_MERGE_EXPIRED),
            _ => Err("invalid approval audit phase".into()),
        },
        McpApprovalContract::IssuesClose => match phase {
            AuditPhase::Approved => Ok(myelin_issues::events::ISSUE_CLOSE_APPROVED),
            AuditPhase::Rejected => Ok(myelin_issues::events::ISSUE_CLOSE_REJECTED),
            AuditPhase::Expired => Ok(myelin_issues::events::ISSUE_CLOSE_EXPIRED),
            _ => Err("invalid approval audit phase".into()),
        },
        McpApprovalContract::CiRetryRun => match phase {
            AuditPhase::Approved => Ok(myelin_ci_controlplane::events::CI_RETRY_RUN_APPROVED),
            AuditPhase::Rejected => Ok(myelin_ci_controlplane::events::CI_RETRY_RUN_REJECTED),
            AuditPhase::Expired => Ok(myelin_ci_controlplane::events::CI_RETRY_RUN_EXPIRED),
            _ => Err("invalid approval audit phase".into()),
        },
        McpApprovalContract::CiCancelRun => match phase {
            AuditPhase::Approved => Ok(myelin_ci_controlplane::events::CI_CANCEL_RUN_APPROVED),
            AuditPhase::Rejected => Ok(myelin_ci_controlplane::events::CI_CANCEL_RUN_REJECTED),
            AuditPhase::Expired => Ok(myelin_ci_controlplane::events::CI_CANCEL_RUN_EXPIRED),
            _ => Err("invalid approval audit phase".into()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_side_effecting_platform_tool_has_a_complete_audit_lifecycle() {
        let outcomes = [
            EffectAuditOutcome::Attempted,
            EffectAuditOutcome::Applied,
            EffectAuditOutcome::Gated,
            EffectAuditOutcome::Denied,
            EffectAuditOutcome::Indeterminate,
        ];
        for tool in crate::registry::ToolRegistry::platform()
            .unwrap()
            .tools()
            .iter()
            .filter(|tool| tool.side_effecting())
        {
            for outcome in outcomes {
                assert!(
                    effect_event_type(tool.name(), outcome).is_ok(),
                    "side-effecting tool `{}` has no complete governance audit taxonomy",
                    tool.name()
                );
            }
        }
    }

    #[test]
    fn workspace_mutations_have_complete_durable_audit_lifecycles() {
        let outcomes = [
            EffectAuditOutcome::Attempted,
            EffectAuditOutcome::Applied,
            EffectAuditOutcome::Gated,
            EffectAuditOutcome::Denied,
            EffectAuditOutcome::Indeterminate,
        ];
        for (tool, expected) in [
            (
                "workspace.write_file",
                [
                    myelin_agent_service::workspace_events::WORKSPACE_WRITE_FILE_ATTEMPTED,
                    myelin_agent_service::workspace_events::WORKSPACE_WRITE_FILE_APPLIED,
                    myelin_agent_service::workspace_events::WORKSPACE_WRITE_FILE_GATED,
                    myelin_agent_service::workspace_events::WORKSPACE_WRITE_FILE_DENIED,
                    myelin_agent_service::workspace_events::WORKSPACE_WRITE_FILE_INDETERMINATE,
                ],
            ),
            (
                "workspace.exec",
                [
                    myelin_agent_service::workspace_events::WORKSPACE_EXEC_ATTEMPTED,
                    myelin_agent_service::workspace_events::WORKSPACE_EXEC_APPLIED,
                    myelin_agent_service::workspace_events::WORKSPACE_EXEC_GATED,
                    myelin_agent_service::workspace_events::WORKSPACE_EXEC_DENIED,
                    myelin_agent_service::workspace_events::WORKSPACE_EXEC_INDETERMINATE,
                ],
            ),
        ] {
            assert_eq!(
                outcomes.map(|outcome| effect_event_type(tool, outcome).unwrap()),
                expected
            );
        }
    }
}
