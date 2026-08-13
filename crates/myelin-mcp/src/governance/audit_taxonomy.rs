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
        "knowledge.link_work" => EffectAuditEvents {
            attempted: myelin_content::events::KNOWLEDGE_LINK_WORK_ATTEMPTED,
            applied: myelin_content::events::KNOWLEDGE_LINK_WORK_APPLIED,
            gated: myelin_content::events::KNOWLEDGE_LINK_WORK_GATED,
            denied: myelin_content::events::KNOWLEDGE_LINK_WORK_DENIED,
            indeterminate: myelin_content::events::KNOWLEDGE_LINK_WORK_INDETERMINATE,
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
    }
}
