use myelin_ci_sandbox::events;
use myelin_events::{AggregateKey, ArtifactRef, DataRole, EventDraft, EventType, Visibility};
use myelin_flow::{per_effect_idem_key, ApprovalDecision, EffectOutcome};
use myelin_identity::{Consistency, IdentityService, ObjectId, Permission, Result as IdResult};

pub const ENVIRONMENT_APPROVE_PERMISSION: &str = "approve";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeployState {
    AwaitingApproval,
    Deploying,
    Deployed,
    Failed,
    RolledBack,
}

impl DeployState {
    pub fn as_token(self) -> &'static str {
        match self {
            DeployState::AwaitingApproval => "awaiting_approval",
            DeployState::Deploying => "deploying",
            DeployState::Deployed => "deployed",
            DeployState::Failed => "failed",
            DeployState::RolledBack => "rolled_back",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeployGateOutcome {
    Approved(String),
    Withheld(String),
}

impl DeployGateOutcome {
    pub fn is_applied(&self) -> bool {
        matches!(self, DeployGateOutcome::Approved(_))
    }
}

pub struct DeployGate;

impl DeployGate {
    pub fn gate_deploy(
        card_id: &str,
        effect_idx: usize,
        total_effects: usize,
        decision: ApprovalDecision,
        applied: &mut std::collections::HashMap<String, String>,
        apply: impl FnOnce() -> String,
    ) -> DeployGateOutcome {
        let key = per_effect_idem_key(card_id, effect_idx, total_effects);
        match decision {
            ApprovalDecision::Decline => DeployGateOutcome::Withheld(DECLINE_TOKEN.to_string()),
            ApprovalDecision::Approve => {
                if let Some(existing) = applied.get(&key) {
                    return DeployGateOutcome::Approved(existing.clone());
                }
                let dep_id = apply();
                applied.insert(key, dep_id.clone());
                DeployGateOutcome::Approved(dep_id)
            }
        }
    }
}

pub const DECLINE_TOKEN: &str = "declined";

pub fn deploy_outcome_of(effect: &EffectOutcome) -> DeployGateOutcome {
    match effect {
        EffectOutcome::Applied(id) => DeployGateOutcome::Approved(id.clone()),
        EffectOutcome::Withheld(reason) => DeployGateOutcome::Withheld(reason.clone()),
    }
}

pub fn resolve_approvers<I: IdentityService>(
    identity: &I,
    environment: &ObjectId,
    at: &Consistency,
) -> IdResult<Vec<String>> {
    let tree = identity.list_subjects(
        environment,
        &Permission(ENVIRONMENT_APPROVE_PERMISSION.to_string()),
        at,
    )?;
    Ok(tree.members.into_iter().map(|p| p.0).collect())
}

pub fn deploy_requires_approval(protected: bool) -> bool {
    protected
}

fn deployment_subject(tenant_id: &str, dep_id: &str) -> ArtifactRef {
    ArtifactRef(format!("myelin://{tenant_id}/ci/deployment/{dep_id}"))
}

#[allow(clippy::too_many_arguments)]
fn deployment_draft(
    event_type: &str,
    tenant_id: &str,
    dep_id: &str,
    env_id: &str,
    run_id: &str,
    state: DeployState,
    approved_by: Option<&str>,
) -> EventDraft {
    EventDraft {
        type_: EventType(event_type.to_string()),
        subject: deployment_subject(tenant_id, dep_id),
        aggregate: AggregateKey(format!("deployment:{dep_id}")),
        payload: serde_json::json!({
            "dep_id": dep_id,
            "env_id": env_id,
            "run_id": run_id,
            "state": state.as_token(),
            "approved_by": approved_by,
        }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

pub fn deployment_requested_draft(
    tenant_id: &str,
    dep_id: &str,
    env_id: &str,
    run_id: &str,
) -> EventDraft {
    deployment_draft(
        events::CI_DEPLOYMENT_REQUESTED,
        tenant_id,
        dep_id,
        env_id,
        run_id,
        DeployState::AwaitingApproval,
        None,
    )
}

pub fn deployment_approval_required_draft(
    tenant_id: &str,
    dep_id: &str,
    env_id: &str,
    run_id: &str,
) -> EventDraft {
    deployment_draft(
        events::CI_DEPLOYMENT_APPROVAL_REQUIRED,
        tenant_id,
        dep_id,
        env_id,
        run_id,
        DeployState::AwaitingApproval,
        None,
    )
}

pub fn deployment_approved_draft(
    tenant_id: &str,
    dep_id: &str,
    env_id: &str,
    run_id: &str,
    approved_by: &str,
) -> EventDraft {
    deployment_draft(
        events::CI_DEPLOYMENT_APPROVED,
        tenant_id,
        dep_id,
        env_id,
        run_id,
        DeployState::Deploying,
        Some(approved_by),
    )
}

pub fn deployment_rejected_draft(
    tenant_id: &str,
    dep_id: &str,
    env_id: &str,
    run_id: &str,
) -> EventDraft {
    deployment_draft(
        events::CI_DEPLOYMENT_REJECTED,
        tenant_id,
        dep_id,
        env_id,
        run_id,
        DeployState::AwaitingApproval,
        None,
    )
}

pub fn deployment_started_draft(
    tenant_id: &str,
    dep_id: &str,
    env_id: &str,
    run_id: &str,
) -> EventDraft {
    deployment_draft(
        events::CI_DEPLOYMENT_STARTED,
        tenant_id,
        dep_id,
        env_id,
        run_id,
        DeployState::Deploying,
        None,
    )
}

pub fn deployment_succeeded_draft(
    tenant_id: &str,
    dep_id: &str,
    env_id: &str,
    run_id: &str,
) -> EventDraft {
    deployment_draft(
        events::CI_DEPLOYMENT_SUCCEEDED,
        tenant_id,
        dep_id,
        env_id,
        run_id,
        DeployState::Deployed,
        None,
    )
}

pub fn deployment_failed_draft(
    tenant_id: &str,
    dep_id: &str,
    env_id: &str,
    run_id: &str,
) -> EventDraft {
    deployment_draft(
        events::CI_DEPLOYMENT_FAILED,
        tenant_id,
        dep_id,
        env_id,
        run_id,
        DeployState::Failed,
        None,
    )
}

pub fn deployment_rolled_back_draft(
    tenant_id: &str,
    dep_id: &str,
    env_id: &str,
    run_id: &str,
) -> EventDraft {
    deployment_draft(
        events::CI_DEPLOYMENT_ROLLED_BACK,
        tenant_id,
        dep_id,
        env_id,
        run_id,
        DeployState::RolledBack,
        None,
    )
}

#[cfg(test)]
#[path = "deployment_tests.rs"]
mod tests;
