use crate::delegation::{authority_of, effective_policy_of, DelegationAlgebra, DelegationInput};
use crate::machine_auth::Authority;
use myelin_identity::{EffectivePolicy, Principal, PrincipalKind, PrincipalStatus, RunId};
use myelin_storage::{
    DurableDelegationPolicyBacking, DurableDelegationPolicyError,
    DurableDelegationPolicyHeadCursor, DurableDelegationPolicyRevisions,
    DurableDelegationPolicyVersions, TenantScope,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DelegationPolicyVersionCursor {
    head: DurableDelegationPolicyHeadCursor,
}

impl DelegationPolicyVersionCursor {
    pub fn version(&self) -> i64 {
        self.head.version
    }

    pub fn revision(&self) -> i64 {
        self.head.revision
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DelegationRunPolicyCursor {
    pub snapshot: i64,
    pub versions: DurableDelegationPolicyVersions,
    pub revisions: DurableDelegationPolicyRevisions,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedDelegationPolicy {
    pub(crate) run_id: RunId,
    pub(crate) agent_id: myelin_identity::PrincipalId,
    pub(crate) trigger_actor_id: myelin_identity::PrincipalId,
    pub(crate) input: DelegationInput,
    pub(crate) effective_policy: EffectivePolicy,
    pub(crate) cursor: DelegationRunPolicyCursor,
}

impl ResolvedDelegationPolicy {
    pub fn run_id(&self) -> &RunId {
        &self.run_id
    }

    pub fn agent_id(&self) -> &myelin_identity::PrincipalId {
        &self.agent_id
    }

    pub fn trigger_actor_id(&self) -> &myelin_identity::PrincipalId {
        &self.trigger_actor_id
    }

    pub fn input(&self) -> &DelegationInput {
        &self.input
    }

    pub fn effective_policy(&self) -> &EffectivePolicy {
        &self.effective_policy
    }

    pub fn cursor(&self) -> DelegationRunPolicyCursor {
        self.cursor
    }

    pub fn attenuate(mut self, ceiling: &Authority) -> Self {
        self.input = DelegationInput {
            agent_policy: self.input.agent_policy.attenuate(ceiling),
            delegation: self.input.delegation.attenuate(ceiling),
            tenant_policy: self.input.tenant_policy.attenuate(ceiling),
            trigger_actor_held: self.input.trigger_actor_held.attenuate(ceiling),
        };
        self.effective_policy =
            effective_policy_of(&authority_of(&self.effective_policy).attenuate(ceiling));
        self
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn synthetic_for_test(
        run_id: RunId,
        agent_id: myelin_identity::PrincipalId,
        trigger_actor_id: myelin_identity::PrincipalId,
        input: DelegationInput,
        snapshot: i64,
    ) -> Self {
        assert!(snapshot > 0, "synthetic durable snapshot must be positive");
        let delegated = input.delegation.attenuate(&input.trigger_actor_held);
        let effective = input
            .agent_policy
            .attenuate(&delegated)
            .attenuate(&input.tenant_policy);
        let versions = DurableDelegationPolicyVersions {
            agent: 1,
            delegation: 1,
            tenant: 1,
            trigger_actor: 1,
        };
        let revisions = DurableDelegationPolicyRevisions {
            agent: snapshot,
            delegation: snapshot,
            tenant: snapshot,
            trigger_actor: snapshot,
        };
        Self {
            run_id,
            agent_id,
            trigger_actor_id,
            input,
            effective_policy: effective_policy_of(&effective),
            cursor: DelegationRunPolicyCursor {
                snapshot,
                versions,
                revisions,
            },
        }
    }
}

#[derive(Debug)]
pub enum DelegationPolicyError {
    ScopeMismatch,
    InactivePrincipal,
    ExpectedAgentPrincipal,
    InvalidTriggerActor,
    OnBehalfOfMismatch,
    InvalidRunId,
    VersionConflict,
    MissingPolicy(&'static str),
    RevokedPolicy(&'static str),
    StaleSnapshot,
    SnapshotBindingMismatch,
    InvalidGrantSet,
    Storage(DurableDelegationPolicyError),
}

impl core::fmt::Display for DelegationPolicyError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ScopeMismatch => {
                f.write_str("delegation principals do not match the verified tenant/region scope")
            }
            Self::InactivePrincipal => f.write_str("delegation requires active principals"),
            Self::ExpectedAgentPrincipal => {
                f.write_str("delegation subject is not an agent principal")
            }
            Self::InvalidTriggerActor => {
                f.write_str("an agent principal cannot trigger another delegated agent run")
            }
            Self::OnBehalfOfMismatch => f.write_str(
                "agent on_behalf_of binding does not match the authenticated trigger actor",
            ),
            Self::InvalidRunId => f.write_str("delegation run id must be a non-empty opaque id"),
            Self::VersionConflict => f.write_str("delegation provisioning cursor is stale"),
            Self::MissingPolicy(slot) => write!(f, "delegation policy conjunct is missing: {slot}"),
            Self::RevokedPolicy(slot) => write!(f, "delegation policy conjunct is revoked: {slot}"),
            Self::StaleSnapshot => f.write_str("delegation run snapshot is stale"),
            Self::SnapshotBindingMismatch => {
                f.write_str("run id is already bound to different principals")
            }
            Self::InvalidGrantSet => f.write_str("delegation policy grant set is invalid"),
            Self::Storage(error) => write!(f, "delegation policy source failed: {error}"),
        }
    }
}

impl std::error::Error for DelegationPolicyError {}

#[derive(Clone)]
pub struct DelegationPolicySource {
    backing: DurableDelegationPolicyBacking,
}

impl DelegationPolicySource {
    pub fn with_pg(backing: DurableDelegationPolicyBacking) -> Self {
        Self { backing }
    }

    pub async fn provision_tenant_policy(
        &self,
        scope: &TenantScope,
        expected: Option<&DelegationPolicyVersionCursor>,
        policy: &Authority,
    ) -> Result<DelegationPolicyVersionCursor, DelegationPolicyError> {
        self.validate_scope(scope)?;
        let head = self
            .backing
            .provision_tenant_policy(
                &scope.tenant().0,
                expected.map(|cursor| cursor.head),
                grants_of(policy),
            )
            .await
            .map_err(map_storage_error)?;
        Ok(DelegationPolicyVersionCursor { head })
    }

    pub async fn provision_agent_policy(
        &self,
        scope: &TenantScope,
        agent: &Principal,
        expected: Option<&DelegationPolicyVersionCursor>,
        policy: &Authority,
    ) -> Result<DelegationPolicyVersionCursor, DelegationPolicyError> {
        self.validate_agent(scope, agent)?;
        let head = self
            .backing
            .provision_agent_policy(
                &scope.tenant().0,
                &agent.principal_id.0,
                expected.map(|cursor| cursor.head),
                grants_of(policy),
            )
            .await
            .map_err(map_storage_error)?;
        Ok(DelegationPolicyVersionCursor { head })
    }

    pub async fn provision_trigger_actor_held(
        &self,
        scope: &TenantScope,
        trigger_actor: &Principal,
        expected: Option<&DelegationPolicyVersionCursor>,
        held: &Authority,
    ) -> Result<DelegationPolicyVersionCursor, DelegationPolicyError> {
        self.validate_trigger_actor(scope, trigger_actor)?;
        let head = self
            .backing
            .provision_trigger_actor_policy(
                &scope.tenant().0,
                &trigger_actor.principal_id.0,
                expected.map(|cursor| cursor.head),
                grants_of(held),
            )
            .await
            .map_err(map_storage_error)?;
        Ok(DelegationPolicyVersionCursor { head })
    }

    pub async fn provision_delegation(
        &self,
        scope: &TenantScope,
        agent: &Principal,
        trigger_actor: &Principal,
        expected: Option<&DelegationPolicyVersionCursor>,
        delegation: &Authority,
    ) -> Result<DelegationPolicyVersionCursor, DelegationPolicyError> {
        self.validate_bindings(scope, agent, trigger_actor)?;
        let head = self
            .backing
            .provision_delegation(
                &scope.tenant().0,
                &agent.principal_id.0,
                &trigger_actor.principal_id.0,
                expected.map(|cursor| cursor.head),
                grants_of(delegation),
            )
            .await
            .map_err(map_storage_error)?;
        Ok(DelegationPolicyVersionCursor { head })
    }

    pub async fn revoke_delegation(
        &self,
        scope: &TenantScope,
        agent: &Principal,
        trigger_actor: &Principal,
        expected: &DelegationPolicyVersionCursor,
    ) -> Result<DelegationPolicyVersionCursor, DelegationPolicyError> {
        self.validate_bindings(scope, agent, trigger_actor)?;
        let head = self
            .backing
            .revoke_delegation(
                &scope.tenant().0,
                &agent.principal_id.0,
                &trigger_actor.principal_id.0,
                expected.head,
            )
            .await
            .map_err(map_storage_error)?;
        Ok(DelegationPolicyVersionCursor { head })
    }

    pub async fn resolve_for_run(
        &self,
        scope: &TenantScope,
        agent: &Principal,
        trigger_actor: &Principal,
        run_id: &RunId,
    ) -> Result<ResolvedDelegationPolicy, DelegationPolicyError> {
        self.validate_bindings(scope, agent, trigger_actor)?;
        if run_id.0.is_empty() {
            return Err(DelegationPolicyError::InvalidRunId);
        }
        let snapshot = self
            .backing
            .resolve_snapshot(
                &scope.tenant().0,
                &run_id.0,
                &agent.principal_id.0,
                &trigger_actor.principal_id.0,
            )
            .await
            .map_err(map_storage_error)?;
        let input = DelegationInput {
            agent_policy: Authority::of(snapshot.grants.agent_policy),
            delegation: Authority::of(snapshot.grants.delegation),
            tenant_policy: Authority::of(snapshot.grants.tenant_policy),
            trigger_actor_held: Authority::of(snapshot.grants.trigger_actor_held),
        };
        let effective_policy = DelegationAlgebra::new().delegation(agent, trigger_actor, &input);
        Ok(ResolvedDelegationPolicy {
            run_id: run_id.clone(),
            agent_id: agent.principal_id.clone(),
            trigger_actor_id: trigger_actor.principal_id.clone(),
            input,
            effective_policy,
            cursor: DelegationRunPolicyCursor {
                snapshot: snapshot.snapshot_cursor,
                versions: snapshot.versions,
                revisions: snapshot.revisions,
            },
        })
    }

    fn validate_bindings(
        &self,
        scope: &TenantScope,
        agent: &Principal,
        trigger_actor: &Principal,
    ) -> Result<(), DelegationPolicyError> {
        self.validate_agent(scope, agent)?;
        self.validate_trigger_actor(scope, trigger_actor)?;
        let on_behalf_of = match &agent.kind {
            PrincipalKind::Agent { on_behalf_of, .. } => on_behalf_of,
            _ => return Err(DelegationPolicyError::ExpectedAgentPrincipal),
        };
        if let Some(expected_actor) = on_behalf_of {
            if expected_actor != &trigger_actor.principal_id {
                return Err(DelegationPolicyError::OnBehalfOfMismatch);
            }
        }
        Ok(())
    }

    fn validate_scope(&self, scope: &TenantScope) -> Result<(), DelegationPolicyError> {
        if scope.region().0 != self.backing.region() {
            return Err(DelegationPolicyError::ScopeMismatch);
        }
        Ok(())
    }

    fn validate_agent(
        &self,
        scope: &TenantScope,
        agent: &Principal,
    ) -> Result<(), DelegationPolicyError> {
        self.validate_scope(scope)?;
        if scope.tenant() != &agent.tenant || scope.region() != &agent.region {
            return Err(DelegationPolicyError::ScopeMismatch);
        }
        if agent.status != PrincipalStatus::Active {
            return Err(DelegationPolicyError::InactivePrincipal);
        }
        if !matches!(agent.kind, PrincipalKind::Agent { .. }) {
            return Err(DelegationPolicyError::ExpectedAgentPrincipal);
        }
        Ok(())
    }

    fn validate_trigger_actor(
        &self,
        scope: &TenantScope,
        trigger_actor: &Principal,
    ) -> Result<(), DelegationPolicyError> {
        self.validate_scope(scope)?;
        if scope.tenant() != &trigger_actor.tenant || scope.region() != &trigger_actor.region {
            return Err(DelegationPolicyError::ScopeMismatch);
        }
        if trigger_actor.status != PrincipalStatus::Active {
            return Err(DelegationPolicyError::InactivePrincipal);
        }
        if matches!(trigger_actor.kind, PrincipalKind::Agent { .. }) {
            return Err(DelegationPolicyError::InvalidTriggerActor);
        }
        Ok(())
    }
}

fn grants_of(authority: &Authority) -> Vec<String> {
    authority.grants().map(str::to_string).collect()
}

fn map_storage_error(error: DurableDelegationPolicyError) -> DelegationPolicyError {
    match error {
        DurableDelegationPolicyError::InvalidGrantSet => DelegationPolicyError::InvalidGrantSet,
        DurableDelegationPolicyError::VersionConflict => DelegationPolicyError::VersionConflict,
        DurableDelegationPolicyError::MissingPolicy(slot) => {
            DelegationPolicyError::MissingPolicy(slot)
        }
        DurableDelegationPolicyError::RevokedPolicy(slot) => {
            DelegationPolicyError::RevokedPolicy(slot)
        }
        DurableDelegationPolicyError::StaleSnapshot => DelegationPolicyError::StaleSnapshot,
        DurableDelegationPolicyError::SnapshotBindingMismatch => {
            DelegationPolicyError::SnapshotBindingMismatch
        }
        other => DelegationPolicyError::Storage(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{DataRole, PrincipalId, RuntimeRef};
    use myelin_tenancy::{Region, TenantId};

    fn principal(id: &str, kind: PrincipalKind, status: PrincipalStatus) -> Principal {
        Principal::new(
            TenantId("tenant-a".into()),
            Region("eu-west".into()),
            PrincipalId(id.into()),
            kind,
            DataRole::Controller,
            status,
        )
    }

    #[test]
    fn bindings_require_active_agent_and_matching_trigger() {
        let _agent = principal(
            "agent-a",
            PrincipalKind::Agent {
                runtime_ref: RuntimeRef("runtime-a".into()),
                on_behalf_of: Some(PrincipalId("human-a".into())),
            },
            PrincipalStatus::Active,
        );
        let human = principal("human-a", PrincipalKind::Human, PrincipalStatus::Active);
        let wrong = principal("human-b", PrincipalKind::Human, PrincipalStatus::Active);
        assert_ne!(human.principal_id, wrong.principal_id);
    }
}
