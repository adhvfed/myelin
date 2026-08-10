use chrono::{DateTime, Utc};
use myelin_ci_controlplane::ci_run_store::CiRunStore;
use myelin_events::EventEnvelope;
use myelin_identity::{
    Consistency, ConsistencyMode, DataRole, Decision, IdentityService, Permission, Principal,
    PrincipalId, PrincipalKind, PrincipalStatus, Zookie,
};
use myelin_identity_service::StoreBackedCheck;
use myelin_storage::{
    DurableAgentTriggerBacking, DurableAgentTriggerBinding, DurablePrincipalBacking,
    ReserveAgentTriggerFiringOutcome, SubstrateProvider,
};
use myelin_tenancy::{ArtifactRef, Region, TenantId};
use sqlx::types::Uuid;
use tokio::runtime::{Handle, RuntimeFlavor};

use super::{TriggerBindingStore, TriggerOwnerVisibility};

pub struct DurableTriggerBindingStore {
    backing: DurableAgentTriggerBacking,
    runtime: Handle,
}

pub struct DurableOwnerVisibility {
    principals: DurablePrincipalBacking,
    runs: CiRunStore,
    identity: StoreBackedCheck,
    runtime: Handle,
    region: String,
}

impl DurableTriggerBindingStore {
    pub fn new(backing: DurableAgentTriggerBacking, runtime: Handle) -> Self {
        Self { backing, runtime }
    }

    fn drive<F: std::future::Future>(&self, future: F) -> Result<F::Output, String> {
        drive_with_runtime(&self.runtime, future, "trigger consumer")
    }
}

impl DurableOwnerVisibility {
    pub fn new(
        provider: SubstrateProvider,
        runs: CiRunStore,
        identity: StoreBackedCheck,
        runtime: Handle,
    ) -> Self {
        Self {
            principals: DurablePrincipalBacking::new(provider.clone()),
            runs,
            identity,
            runtime,
            region: provider.config().region.clone(),
        }
    }

    fn drive<F: std::future::Future>(&self, future: F) -> Result<F::Output, String> {
        drive_with_runtime(&self.runtime, future, "trigger visibility")
    }

    fn owner(&self, tenant: &str, principal_id: &str) -> Result<Option<Principal>, String> {
        let row = self
            .drive(self.principals.get_principal(tenant, principal_id))?
            .map_err(|_| "trigger owner identity is unavailable".to_string())?;
        row.map(|row| {
            Ok(Principal::new(
                TenantId(tenant.into()),
                Region(self.region.clone()),
                PrincipalId(row.principal_id),
                serde_json::from_str::<PrincipalKind>(&row.kind)
                    .map_err(|_| "trigger owner kind is invalid".to_string())?,
                serde_json::from_str::<DataRole>(&row.data_role)
                    .map_err(|_| "trigger owner data role is invalid".to_string())?,
                serde_json::from_str::<PrincipalStatus>(&row.status)
                    .map_err(|_| "trigger owner status is invalid".to_string())?,
            ))
        })
        .transpose()
    }
}

fn drive_with_runtime<F: std::future::Future>(
    runtime: &Handle,
    future: F,
    operation: &str,
) -> Result<F::Output, String> {
    match Handle::try_current() {
        Ok(current) if current.runtime_flavor() == RuntimeFlavor::MultiThread => {
            Ok(tokio::task::block_in_place(|| runtime.block_on(future)))
        }
        Ok(_) => Err(format!("{operation} requires a multi-thread runtime")),
        Err(_) => Ok(runtime.block_on(future)),
    }
}

impl TriggerBindingStore for DurableTriggerBindingStore {
    fn active_for_event(
        &self,
        tenant: &str,
        event_type: &str,
        limit: u32,
    ) -> Result<Vec<DurableAgentTriggerBinding>, String> {
        self.drive(self.backing.active_for_event(tenant, event_type, limit))?
            .map_err(|_| "durable trigger discovery is unavailable".into())
    }

    fn reserve_firing(
        &self,
        tenant: &str,
        binding_id: &str,
        envelope: &EventEnvelope,
        recorded_at: DateTime<Utc>,
    ) -> Result<ReserveAgentTriggerFiringOutcome, String> {
        let binding_id = Uuid::parse_str(binding_id)
            .map_err(|_| "durable trigger binding id is not a UUID".to_string())?;
        let stored_envelope = serde_json::to_value(envelope)
            .map_err(|_| "event envelope could not be serialized".to_string())?;
        self.drive(self.backing.reserve_firing(
            tenant,
            binding_id,
            &envelope.event_id.0,
            &envelope.type_.0,
            stored_envelope,
            envelope.depth,
            envelope.contains_personal_data,
            recorded_at,
        ))?
        .map_err(|_| "durable trigger reservation is unavailable".into())
    }
}

impl TriggerOwnerVisibility for DurableOwnerVisibility {
    fn can_view(
        &self,
        binding: &DurableAgentTriggerBinding,
        envelope: &EventEnvelope,
    ) -> Result<bool, String> {
        let Some(run_key) = myelin_refs::object_key(&envelope.subject) else {
            return Ok(false);
        };
        if run_key.tenant.as_deref() != Some(envelope.tenant.as_str())
            || run_key.object_type.as_deref() != Some("run")
        {
            return Ok(false);
        }
        let Some(owner) = self.owner(&envelope.tenant.0, &binding.owner_principal_id)? else {
            return Ok(false);
        };
        if owner.kind != PrincipalKind::Human || owner.status != PrincipalStatus::Active {
            return Ok(false);
        }
        let Some(run) = self
            .drive(
                self.runs
                    .get_ci_run(&envelope.tenant.0, &envelope.region.0, &run_key.id),
            )?
            .map_err(|_| "trigger CI run lookup is unavailable".to_string())?
        else {
            return Ok(false);
        };
        let Some(repo_ref) = run.repo_ref else {
            return Ok(false);
        };
        let Some(repo_key) = myelin_refs::object_key(&ArtifactRef(repo_ref)) else {
            return Ok(false);
        };
        if repo_key.object_type.as_deref() != Some("repo")
            || repo_key
                .tenant
                .as_deref()
                .is_some_and(|tenant| tenant != envelope.tenant.as_str())
        {
            return Ok(false);
        }
        let at = Consistency {
            at_least: Zookie(String::new()),
            mode: ConsistencyMode::Strong,
        };
        Ok(matches!(
            self.identity.check(
                &owner,
                &Permission("pull".into()),
                &ArtifactRef(format!("repo:{}", repo_key.id)),
                &at,
                None,
            ),
            Ok(Decision::Allow)
        ))
    }
}
