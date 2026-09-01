#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpApprovalContract {
    GitMerge,
    IssuesClose,
    CiRetryRun,
}

impl McpApprovalContract {
    pub fn for_tool(tool: &str) -> Option<Self> {
        match tool {
            "git.merge" => Some(Self::GitMerge),
            "issues.close" => Some(Self::IssuesClose),
            "ci.retry_run" => Some(Self::CiRetryRun),
            _ => None,
        }
    }

    pub const fn tool(self) -> &'static str {
        match self {
            Self::GitMerge => "git.merge",
            Self::IssuesClose => "issues.close",
            Self::CiRetryRun => "ci.retry_run",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_tools_with_a_complete_human_decision_path_have_a_contract() {
        for (tool, expected) in [
            ("git.merge", McpApprovalContract::GitMerge),
            ("issues.close", McpApprovalContract::IssuesClose),
            ("ci.retry_run", McpApprovalContract::CiRetryRun),
        ] {
            let contract = McpApprovalContract::for_tool(tool).unwrap();
            assert_eq!(contract, expected);
            assert_eq!(contract.tool(), tool);
        }

        for incomplete in ["git.history_rewrite", "knowledge.publish"] {
            assert_eq!(McpApprovalContract::for_tool(incomplete), None);
        }
    }
}
