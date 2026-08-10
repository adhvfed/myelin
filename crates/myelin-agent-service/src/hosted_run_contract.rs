pub const AGENT_RUN_WORKFLOW: &str = "agent.run";
pub const AGENT_RUN_WORKFLOW_VERSION: i32 = 1;

pub fn agent_run_definition_hash() -> String {
    format!(
        "blake3:{}",
        blake3::hash(b"myelin.agent.run@1:resolve-governed-firing-and-dispatch-hosted-agent")
            .to_hex()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_identity_is_stable() {
        assert_eq!(AGENT_RUN_WORKFLOW, "agent.run");
        assert_eq!(AGENT_RUN_WORKFLOW_VERSION, 1);
        assert_eq!(agent_run_definition_hash().len(), "blake3:".len() + 64);
    }
}
