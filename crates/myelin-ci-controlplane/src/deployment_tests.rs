use super::*;
use myelin_events::validate_event_type;
use myelin_flow::{ApprovalDecision, EffectOutcome};
use myelin_identity::{
    AuthzError, CaveatContext, Consistency, ConsistencyMode, Credential, Decision,
    DelegationCaveats, EffectivePolicy, FailStaticBound, FragmentAdmit, IdentityService,
    ListObjectsResult, NamespaceFragment, ObjectId, ObjectType, Permission, Precondition,
    Principal, PrincipalId, RelName, Result as IdResult, RevokeTarget, RewriteTrace, RunId,
    RunToken, SubjectTree, TupleDelta, Zookie,
};
use myelin_tenancy::{ArtifactRef, TenantId};
use std::collections::HashMap;

#[test]
fn double_click_approve_is_one_apply() {
    let mut applied: HashMap<String, String> = HashMap::new();
    let mut apply_calls = 0;

    let o1 = DeployGate::gate_deploy(
        "card-deploy-1",
        0,
        1,
        ApprovalDecision::Approve,
        &mut applied,
        || {
            apply_calls += 1;
            "dep-7".to_string()
        },
    );
    assert_eq!(o1, DeployGateOutcome::Approved("dep-7".into()));
    assert_eq!(apply_calls, 1);

    let o2 = DeployGate::gate_deploy(
        "card-deploy-1",
        0,
        1,
        ApprovalDecision::Approve,
        &mut applied,
        || {
            apply_calls += 1;
            "dep-OTHER".to_string()
        },
    );
    assert_eq!(
        o2,
        DeployGateOutcome::Approved("dep-7".into()),
        "the recorded deploy id, not a new one"
    );
    assert_eq!(
        apply_calls, 1,
        "a double-click is ONE apply (per-effect idem_key, OQ-F)"
    );
}

#[test]
fn declined_deploy_is_withheld_and_never_mutates() {
    let mut applied: HashMap<String, String> = HashMap::new();
    let mut apply_calls = 0;

    let o = DeployGate::gate_deploy(
        "card-deploy-2",
        0,
        1,
        ApprovalDecision::Decline,
        &mut applied,
        || {
            apply_calls += 1;
            "dep-NEVER".to_string()
        },
    );
    assert_eq!(o, DeployGateOutcome::Withheld(DECLINE_TOKEN.into()));
    assert!(!o.is_applied(), "a declined deploy did NOT apply");
    assert_eq!(
        apply_calls, 0,
        "a declined deploy makes ZERO mutation (AG-8)"
    );
    assert!(applied.is_empty(), "nothing recorded for a withheld effect");
}

#[test]
fn batch_partial_approval_is_well_defined() {
    let mut applied: HashMap<String, String> = HashMap::new();
    let decisions = [
        ApprovalDecision::Approve,
        ApprovalDecision::Decline,
        ApprovalDecision::Approve,
    ];
    let total = decisions.len();
    let mut outcomes = Vec::new();
    for (idx, decision) in decisions.into_iter().enumerate() {
        let o = DeployGate::gate_deploy("card-batch", idx, total, decision, &mut applied, || {
            format!("dep-{idx}")
        });
        outcomes.push(o);
    }
    assert_eq!(outcomes[0], DeployGateOutcome::Approved("dep-0".into()));
    assert_eq!(
        outcomes[1],
        DeployGateOutcome::Withheld(DECLINE_TOKEN.into())
    );
    assert_eq!(outcomes[2], DeployGateOutcome::Approved("dep-2".into()));
    assert_eq!(applied.len(), 2);
    assert!(applied.contains_key("card-batch:0"));
    assert!(applied.contains_key("card-batch:2"));
    assert!(
        !applied.contains_key("card-batch:1"),
        "the declined effect is NOT applied"
    );
}

#[test]
fn deploy_outcome_maps_from_flow_effect_outcome() {
    assert_eq!(
        deploy_outcome_of(&EffectOutcome::Applied("dep-9".into())),
        DeployGateOutcome::Approved("dep-9".into())
    );
    assert_eq!(
        deploy_outcome_of(&EffectOutcome::Withheld("decline".into())),
        DeployGateOutcome::Withheld("decline".into())
    );
}

#[test]
fn protected_env_requires_approval_unprotected_does_not() {
    assert!(
        deploy_requires_approval(true),
        "a protected env requires approval (X-6 default)"
    );
    assert!(
        !deploy_requires_approval(false),
        "an unprotected env deploys directly"
    );
}

struct ApproverIdentity {
    approvers: Vec<String>,
}
impl IdentityService for ApproverIdentity {
    fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
        Err(AuthzError::NotYetImplemented("test stub"))
    }
    fn check(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &ArtifactRef,
        _at: &Consistency,
        _cav: Option<&CaveatContext>,
    ) -> IdResult<Decision> {
        Ok(Decision::Deny)
    }
    fn list_objects(
        &self,
        _s: &Principal,
        _p: &Permission,
        _ty: &ObjectType,
        _at: &Consistency,
    ) -> IdResult<ListObjectsResult> {
        Err(AuthzError::NotYetImplemented("test stub"))
    }
    fn list_subjects(
        &self,
        _o: &ObjectId,
        p: &Permission,
        _at: &Consistency,
    ) -> IdResult<SubjectTree> {
        assert_eq!(
            p.0, ENVIRONMENT_APPROVE_PERMISSION,
            "resolves the `approve` set"
        );
        Ok(SubjectTree {
            object: ObjectId("env-1".into()),
            relation: RelName("approve".into()),
            members: self
                .approvers
                .iter()
                .map(|a| PrincipalId(a.clone()))
                .collect(),
            zookie: Zookie("z0".into()),
        })
    }
    fn explain(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &ObjectId,
        _at: &Consistency,
    ) -> IdResult<RewriteTrace> {
        Err(AuthzError::NotYetImplemented("test stub"))
    }
    fn delegation(&self, _a: &Principal, _t: &Principal) -> IdResult<EffectivePolicy> {
        Err(AuthzError::NotYetImplemented("test stub"))
    }
    fn write_tuples(&self, _d: &[TupleDelta], _pre: Option<&Precondition>) -> IdResult<Zookie> {
        Err(AuthzError::NotYetImplemented("test stub"))
    }
    fn mint_run_token(
        &self,
        _a: &PrincipalId,
        _r: &RunId,
        _d: &DelegationCaveats,
        _ttl: &FailStaticBound,
    ) -> IdResult<RunToken> {
        Err(AuthzError::NotYetImplemented("test stub"))
    }
    fn revoke(&self, _t: &RevokeTarget) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("test stub"))
    }
    fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> IdResult<String> {
        Err(AuthzError::NotYetImplemented("test stub"))
    }
    fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("test stub"))
    }
    fn admit_fragment(&self, _f: &NamespaceFragment) -> IdResult<FragmentAdmit> {
        Err(AuthzError::NotYetImplemented("test stub"))
    }
}

#[test]
fn resolve_approvers_flattens_the_list_subjects_tree() {
    let id = ApproverIdentity {
        approvers: vec!["u:alice".into(), "u:bob".into()],
    };
    let at = Consistency {
        at_least: Zookie("z0".into()),
        mode: ConsistencyMode::Strong,
    };
    let approvers =
        resolve_approvers(&id, &ObjectId("env-1".into()), &at).expect("resolves the approver set");
    assert_eq!(approvers, vec!["u:alice".to_string(), "u:bob".to_string()]);
}

#[test]
fn deployment_drafts_carry_frozen_tokens_and_the_deployment_aggregate() {
    let drafts = [
        deployment_requested_draft("acme", "dep-1", "env-1", "run-1"),
        deployment_approval_required_draft("acme", "dep-1", "env-1", "run-1"),
        deployment_approved_draft("acme", "dep-1", "env-1", "run-1", "pseudo:approver"),
        deployment_rejected_draft("acme", "dep-1", "env-1", "run-1"),
        deployment_started_draft("acme", "dep-1", "env-1", "run-1"),
        deployment_succeeded_draft("acme", "dep-1", "env-1", "run-1"),
        deployment_failed_draft("acme", "dep-1", "env-1", "run-1"),
        deployment_rolled_back_draft("acme", "dep-1", "env-1", "run-1"),
    ];
    for d in &drafts {
        validate_event_type(&d.type_.0)
            .unwrap_or_else(|e| panic!("invalid type {}: {e:?}", d.type_.0));
        assert_eq!(d.aggregate.0, "deployment:dep-1");
        assert_eq!(d.subject.0, "myelin://acme/ci/deployment/dep-1");
        assert!(!d.contains_personal_data);
        assert!(d.pii_key_ref.is_none());
    }
}

#[test]
fn approved_draft_carries_the_pseudonym_approver_ref_only() {
    let d = deployment_approved_draft("acme", "dep-1", "env-1", "run-1", "pseudo:approver");
    assert_eq!(d.payload["approved_by"], "pseudo:approver");
    assert_eq!(d.payload["state"], "deploying");
    assert!(
        !d.contains_personal_data,
        "the pseudonym is a ref, not inline clear PII"
    );
}

#[test]
fn deploy_state_tokens_match_the_migration_check_set() {
    assert_eq!(
        DeployState::AwaitingApproval.as_token(),
        "awaiting_approval"
    );
    assert_eq!(DeployState::Deploying.as_token(), "deploying");
    assert_eq!(DeployState::Deployed.as_token(), "deployed");
    assert_eq!(DeployState::Failed.as_token(), "failed");
    assert_eq!(DeployState::RolledBack.as_token(), "rolled_back");
}

#[test]
fn rolled_back_is_first_class() {
    let d = deployment_rolled_back_draft("acme", "dep-1", "env-1", "run-1");
    assert_eq!(d.type_.0, "ci.deployment.rolled_back");
    assert_eq!(d.payload["state"], "rolled_back");
}
