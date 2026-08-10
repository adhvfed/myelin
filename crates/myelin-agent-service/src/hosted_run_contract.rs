use myelin_refs::ArtifactRef;
pub use myelin_storage::hitl_gate_durable::gate_ref_token;
use myelin_tenancy::TenantId;

pub const AGENT_RUN_WORKFLOW: &str = "agent.run";
pub const LEGACY_AGENT_RUN_WORKFLOW_VERSION: i32 = 1;
pub const AGENT_RUN_WORKFLOW_VERSION: i32 = 2;
pub const HOSTED_AGENT_APPROVAL_SIGNAL: &str = "agent.hitl.decided";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostedAgentDecision {
    Approved,
    Rejected,
    Expired,
}

impl HostedAgentDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Expired => "expired",
        }
    }
}

pub fn agent_run_definition_hash() -> String {
    format!(
        "blake3:{}",
        blake3::hash(
            b"myelin.agent.run@2:resolve-governed-firing-dispatch-and-durable-human-approval",
        )
        .to_hex()
    )
}

pub fn legacy_agent_run_definition_hash() -> String {
    format!(
        "blake3:{}",
        blake3::hash(b"myelin.agent.run@1:resolve-governed-firing-and-dispatch-hosted-agent")
            .to_hex()
    )
}

pub fn hosted_agent_decision_ref(
    tenant: &TenantId,
    run_id: &str,
    gate_id: &str,
    decision: HostedAgentDecision,
) -> ArtifactRef {
    ArtifactRef(format!(
        "myelin://{}/agent/run/{run_id}:hitl-gate:{}:decision:{}",
        tenant.0,
        gate_ref_token(gate_id),
        decision.as_str(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_identity_is_stable() {
        assert_eq!(AGENT_RUN_WORKFLOW, "agent.run");
        assert_eq!(LEGACY_AGENT_RUN_WORKFLOW_VERSION, 1);
        assert_eq!(AGENT_RUN_WORKFLOW_VERSION, 2);
        assert_eq!(agent_run_definition_hash().len(), "blake3:".len() + 64);
        assert_ne!(
            agent_run_definition_hash(),
            legacy_agent_run_definition_hash()
        );
    }

    #[test]
    fn approval_decisions_are_exact_run_and_gate_scoped_artifacts() {
        let decision = hosted_agent_decision_ref(
            &TenantId("acme".into()),
            "run-7",
            "gate:secret",
            HostedAgentDecision::Approved,
        );
        assert_eq!(
            decision.0,
            "myelin://acme/agent/run/run-7:hitl-gate:676174653a736563726574:decision:approved"
        );
        assert_eq!(
            myelin_refs::parse_scoped(&decision.0)
                .expect("a hosted decision is one canonical ArtifactRef")
                .tenant,
            TenantId("acme".into())
        );
    }
}
