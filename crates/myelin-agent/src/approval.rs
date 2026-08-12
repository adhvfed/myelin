#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpApprovalContract {
    GitMerge,
}

impl McpApprovalContract {
    pub fn for_tool(tool: &str) -> Option<Self> {
        match tool {
            "git.merge" => Some(Self::GitMerge),
            _ => None,
        }
    }

    pub const fn tool(self) -> &'static str {
        match self {
            Self::GitMerge => "git.merge",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_tools_with_a_complete_human_decision_path_have_a_contract() {
        let contract = McpApprovalContract::for_tool("git.merge").unwrap();
        assert_eq!(contract, McpApprovalContract::GitMerge);
        assert_eq!(contract.tool(), "git.merge");

        for incomplete in ["git.history_rewrite", "issues.close", "knowledge.publish"] {
            assert_eq!(McpApprovalContract::for_tool(incomplete), None);
        }
    }
}
