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
    provider: SubstrateProvider,
    principals: DurablePrincipalBacking,
    runs: CiRunStore,
    identity: StoreBackedCheck,
    runtime: Handle,
    region: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriggerVisibilityRule {
    CiRunRepository,
    Direct {
        subsystem: &'static str,
        identity_object_type: &'static str,
        permission: &'static str,
        address: TriggerSubjectAddress,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriggerSubjectAddress {
    SubjectId,
    IssueKey,
}

pub fn trigger_visibility_rule(subject_type: &str) -> Option<TriggerVisibilityRule> {
    match subject_type {
        "run" => Some(TriggerVisibilityRule::CiRunRepository),
        "repo" => Some(TriggerVisibilityRule::Direct {
            subsystem: "git",
            identity_object_type: "repo",
            permission: "pull",
            address: TriggerSubjectAddress::SubjectId,
        }),
        "pr" => Some(TriggerVisibilityRule::Direct {
            subsystem: "git",
            identity_object_type: "pull_request",
            permission: "view",
            address: TriggerSubjectAddress::SubjectId,
        }),
        "comment" => Some(TriggerVisibilityRule::Direct {
            subsystem: "git",
            identity_object_type: "pr_comment",
            permission: "view",
            address: TriggerSubjectAddress::SubjectId,
        }),
        "issue" => Some(TriggerVisibilityRule::Direct {
            subsystem: "issue",
            identity_object_type: "issue",
            permission: "view",
            address: TriggerSubjectAddress::IssueKey,
        }),
        "page" => Some(TriggerVisibilityRule::Direct {
            subsystem: "knowledge",
            identity_object_type: "page",
            permission: "read",
            address: TriggerSubjectAddress::SubjectId,
        }),
        "row" => Some(TriggerVisibilityRule::Direct {
            subsystem: "knowledge",
            identity_object_type: "database_row",
            permission: "read",
            address: TriggerSubjectAddress::SubjectId,
        }),
        "channel" => Some(TriggerVisibilityRule::Direct {
            subsystem: "chat",
            identity_object_type: "channel",
            permission: "read",
            address: TriggerSubjectAddress::SubjectId,
        }),
        "message" => Some(TriggerVisibilityRule::Direct {
            subsystem: "chat",
            identity_object_type: "message",
            permission: "view",
            address: TriggerSubjectAddress::SubjectId,
        }),
        _ => None,
    }
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
            provider: provider.clone(),
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

    fn direct_identity_object(
        &self,
        tenant: &str,
        subject_id: &str,
        identity_object_type: &str,
        address: TriggerSubjectAddress,
    ) -> Result<Option<String>, String> {
        if address == TriggerSubjectAddress::SubjectId {
            return Ok(Some(format!("{identity_object_type}:{subject_id}")));
        }
        let tenant = tenant.to_string();
        let region = self.region.clone();
        let issue_key = subject_id.to_string();
        let issue_id = self
            .drive(self.provider.with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    sqlx::query_scalar::<_, Uuid>(
                        "SELECT id FROM issue \
                          WHERE tenant_id = $1 AND region = $2 AND key = $3 \
                            AND deleted_at IS NULL",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&issue_key)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|_| {
                        myelin_storage::PgError::Query(
                            "resolve trigger issue authorization object".into(),
                        )
                    })
                })
            }))?
            .map_err(|_| "trigger issue lookup is unavailable".to_string())?;
        Ok(issue_id.map(|id| format!("{identity_object_type}:{id}")))
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
        let Some(subject_key) = myelin_refs::object_key(&envelope.subject) else {
            return Ok(false);
        };
        if subject_key.tenant.as_deref() != Some(envelope.tenant.as_str()) {
            return Ok(false);
        }
        let Some(subject_type) = subject_key.object_type.as_deref() else {
            return Ok(false);
        };
        let Some(rule) = trigger_visibility_rule(subject_type) else {
            return Ok(false);
        };
        let Some(owner) = self.owner(&envelope.tenant.0, &binding.owner_principal_id)? else {
            return Ok(false);
        };
        if owner.kind != PrincipalKind::Human || owner.status != PrincipalStatus::Active {
            return Ok(false);
        }
        if let TriggerVisibilityRule::Direct {
            subsystem,
            identity_object_type,
            permission,
            address,
        } = rule
        {
            if subject_key.subsystem.as_deref() != Some(subsystem) {
                return Ok(false);
            }
            let Some(object) = self.direct_identity_object(
                &envelope.tenant.0,
                &subject_key.id,
                identity_object_type,
                address,
            )?
            else {
                return Ok(false);
            };
            return Ok(self.identity_allows(&owner, permission, &object));
        }
        if subject_key.subsystem.as_deref() != Some("ci") {
            return Ok(false);
        }
        let Some(run) = self
            .drive(
                self.runs
                    .get_ci_run(&envelope.tenant.0, &envelope.region.0, &subject_key.id),
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
        Ok(self.identity_allows(&owner, "pull", &format!("repo:{}", repo_key.id)))
    }
}

impl DurableOwnerVisibility {
    fn identity_allows(&self, owner: &Principal, permission: &str, object: &str) -> bool {
        let at = Consistency {
            at_least: Zookie(String::new()),
            mode: ConsistencyMode::Strong,
        };
        matches!(
            self.identity.check(
                owner,
                &Permission(permission.into()),
                &ArtifactRef(object.into()),
                &at,
                None,
            ),
            Ok(Decision::Allow)
        )
    }
}

#[cfg(test)]
mod visibility_rule_tests {
    use super::*;

    #[test]
    fn event_artifact_types_translate_to_their_live_read_boundary() {
        assert_eq!(
            trigger_visibility_rule("issue"),
            Some(TriggerVisibilityRule::Direct {
                subsystem: "issue",
                identity_object_type: "issue",
                permission: "view",
                address: TriggerSubjectAddress::IssueKey,
            })
        );
        assert_eq!(
            trigger_visibility_rule("row"),
            Some(TriggerVisibilityRule::Direct {
                subsystem: "knowledge",
                identity_object_type: "database_row",
                permission: "read",
                address: TriggerSubjectAddress::SubjectId,
            })
        );
        assert_eq!(
            trigger_visibility_rule("run"),
            Some(TriggerVisibilityRule::CiRunRepository)
        );
        assert_eq!(trigger_visibility_rule("deployment"), None);
        assert!(myelin_events::AUTOMATION_SUBJECT_TYPE_TOKENS
            .iter()
            .all(|subject_type| trigger_visibility_rule(subject_type).is_some()));
    }
}
