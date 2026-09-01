use std::{collections::BTreeSet, sync::Arc, time::Duration as StdDuration};

use chrono::{DateTime, Duration, SecondsFormat, Timelike, Utc};
use myelin_agent_service::{
    workspace::{
        AgentWorkspaceStore, RepositoryReviewPublicationRequest, RepositoryWorkspaceSource,
        WorkspaceAccessError, WorkspaceEnvironmentState, WorkspacePublicationError,
    },
    PlatformToolCatalogue,
};
use myelin_chat::conversation::{Conversation, ConversationKind};
use myelin_chat::events::pseudonymized_event_principal;
use myelin_chat::store::{ConversationId, SystemUlidSource, UlidSource};
use myelin_events::{Actor, EventId, IdMinter, Timestamp};
use myelin_git::pg_pr_store::PrOperationId;
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_identity_service::{
    AgentRegistryError, AgentSessionIssuer, AgentSessionRequest, PgAgentRegistry,
};
use myelin_storage::{
    ActivateAgentThreadOutcome, AdvanceAgentThreadReviewHandoffOutcome, AgentThreadActivation,
    AgentThreadEnvironmentBinding, AgentThreadEnvironmentState, AgentThreadGitBinding,
    AgentThreadProvisioningFailure, AgentThreadRenewal, AgentThreadReviewHandoff,
    AgentThreadRunBinding, AgentThreadState, AgentThreadWorkspaceGuidance,
    BeginAgentThreadRenewalOutcome, BindAgentThreadReviewHandoffOutcome, BindAgentThreadRunOutcome,
    ColumnCryptor, CreateAgentThreadOutcome, DurableAgentThread, DurableAgentThreadBacking,
    KmsEngine, ListAgentThreadsOutcome, MarkAgentThreadReviewReadyOutcome, NewAgentThread,
    NewAgentThreadReviewHandoff, RetryAgentThreadOutcome, SubjectId, SubjectKeyScope,
    WorkspaceSshRouteKey, MAX_AGENT_THREAD_NAME_BYTES, MAX_AGENT_THREAD_RETENTION_DAYS,
    MIN_AGENT_THREAD_RETENTION_DAYS,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::types::Uuid;
use tokio::runtime::Handle;
use tokio::sync::watch;

use myelin_tenancy::{Region, TenantId};

use crate::agent_http::{agent_session_json, map_session_error};
use crate::catalogue::{page_envelope, Handler, HandlerCtx, DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT};
use crate::chat_http::{
    parse_messages_query, DurableChatMutationApi, DurableChatReadApi, PrivateConversationCreation,
    PrivateConversationRenewal,
};
use crate::gateway::GatewayBuilder;
use crate::git_durable::{map_durable_err, DurableGitBackend, ResolvedWorkspaceRepository};
use crate::request::EdgeResponse;
use crate::runtime::drive_edge_future;
use crate::workspace_augmentation::{
    content_digest as workspace_guidance_digest, validate_workspace_files,
    validate_workspace_instructions, WorkspaceAugmentation, WorkspaceFile,
};
use crate::workspace_ssh_http::{
    register_workspace_ssh_access, WorkspaceSshEndpoint, WorkspaceSshHttpInputs,
};
use crate::{EdgeError, Method};

pub(crate) const MAX_AGENT_THREAD_JSON_BYTES: usize = 64 * 1024;
const DEFAULT_RETENTION_DAYS: i16 = 3;
pub const AGENT_THREAD_RECOVERY_TENANTS_ENV: &str = "MYELIN_AGENT_THREAD_RECOVERY_TENANTS";
const DEFAULT_RECOVERY_INTERVAL_MS: u64 = 5_000;
const DEFAULT_RECOVERY_STALE_MS: u64 = 30_000;
const MAX_RECOVERY_BATCH: u32 = 100;

#[derive(Clone)]
pub struct AgentThreadRecoveryConfig {
    tenants: Vec<TenantId>,
    region: Region,
    interval: StdDuration,
    stale_after: StdDuration,
    batch_limit: u32,
}

impl AgentThreadRecoveryConfig {
    pub fn new(
        tenants: Vec<TenantId>,
        region: Region,
        interval: StdDuration,
        stale_after: StdDuration,
        batch_limit: u32,
    ) -> Result<Self, String> {
        if tenants.is_empty() {
            return Err("at least one agent thread recovery tenant is required".into());
        }
        let mut unique = BTreeSet::new();
        for tenant in &tenants {
            let token = tenant.as_str();
            if token.is_empty()
                || token.len() > 128
                || !token
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
            {
                return Err(
                    "agent thread recovery tenants must be opaque 1..=128 byte tokens".into(),
                );
            }
            if !unique.insert(token.to_string()) {
                return Err("agent thread recovery tenant list contains a duplicate".into());
            }
        }
        if region.as_str().is_empty() {
            return Err("agent thread recovery region must not be empty".into());
        }
        if interval.is_zero() {
            return Err("agent thread recovery interval must be non-zero".into());
        }
        if stale_after < interval {
            return Err("agent thread recovery staleness must be at least the interval".into());
        }
        if chrono::Duration::from_std(stale_after).is_err() {
            return Err("agent thread recovery staleness exceeds the supported range".into());
        }
        if !(1..=MAX_RECOVERY_BATCH).contains(&batch_limit) {
            return Err(format!(
                "agent thread recovery batch must be between 1 and {MAX_RECOVERY_BATCH}"
            ));
        }
        Ok(Self {
            tenants,
            region,
            interval,
            stale_after,
            batch_limit,
        })
    }

    pub fn from_env(region: Region) -> Result<Self, String> {
        let raw = std::env::var(AGENT_THREAD_RECOVERY_TENANTS_ENV).map_err(|_| {
            format!("{AGENT_THREAD_RECOVERY_TENANTS_ENV} is required (comma-separated opaque tenant tokens)")
        })?;
        let tenants = raw
            .split(',')
            .map(str::trim)
            .map(|value| TenantId::from_token(value.to_string()))
            .collect::<Vec<_>>();
        Self::new(
            tenants,
            region,
            StdDuration::from_millis(DEFAULT_RECOVERY_INTERVAL_MS),
            StdDuration::from_millis(DEFAULT_RECOVERY_STALE_MS),
            MAX_RECOVERY_BATCH,
        )
    }
}

pub struct AgentThreadRecoveryHandle {
    shutdown: watch::Sender<bool>,
    join: tokio::task::JoinHandle<()>,
}

impl AgentThreadRecoveryHandle {
    pub async fn shutdown(self) -> Result<(), String> {
        let _ = self.shutdown.send(true);
        tokio::time::timeout(StdDuration::from_secs(10), self.join)
            .await
            .map_err(|_| "agent thread recovery did not stop within 10 seconds".to_string())?
            .map_err(|_| "agent thread recovery task join failed".to_string())
    }
}

#[derive(Clone)]
struct AgentThreadHttpApi {
    threads: DurableAgentThreadBacking,
    agents: PgAgentRegistry,
    chat_reads: DurableChatReadApi,
    chat_mutations: DurableChatMutationApi,
    workspaces: Arc<dyn AgentWorkspaceStore>,
    git: Arc<DurableGitBackend>,
    sessions: AgentSessionIssuer,
    catalogue: Arc<PlatformToolCatalogue>,
    event_ids: Arc<dyn IdMinter>,
    conversation_ids: Arc<dyn UlidSource>,
    kms: Arc<KmsEngine>,
    runtime: Handle,
}

impl AgentThreadHttpApi {
    fn drive<F, T>(&self, future: F) -> Result<T, EdgeError>
    where
        F: std::future::Future<Output = T>,
    {
        drive_edge_future(&self.runtime, future, "agent thread HTTP")
    }

    fn create(
        &self,
        principal: &Principal,
        body: CreateAgentThreadBody,
        client_nonce: String,
    ) -> Result<(DurableAgentThread, bool), EdgeError> {
        let intent = ValidatedThreadIntent::new(principal, body, client_nonce)?;
        if let Some(existing) = self
            .drive(self.threads.get_for_owner_by_nonce(
                &principal.tenant.0,
                &principal.principal_id.0,
                &intent.client_nonce,
            ))?
            .map_err(|_| EdgeError::Internal("agent thread replay lookup failed".into()))?
        {
            if !intent.matches(&existing) {
                return Err(EdgeError::Conflict(
                    "that idempotency key was already used for a different agent thread".into(),
                ));
            }
            if parse_stored_timestamp(&existing.expires_at)? <= Utc::now() {
                return Ok((existing, false));
            }
            return match existing.state {
                AgentThreadState::Failed => self.retry(
                    principal,
                    parse_uuid("stored thread id", &existing.thread_id).map_err(|_| {
                        EdgeError::Internal("stored agent thread id is invalid".into())
                    })?,
                ),
                AgentThreadState::Provisioning | AgentThreadState::Ready => {
                    self.provision(principal, existing)
                }
                AgentThreadState::Expiring | AgentThreadState::Deleted => Ok(existing),
            }
            .map(|thread| (thread, false));
        }
        if let Some(project_id) = intent.project_id.as_deref() {
            if !self.chat_reads.may_view_project(principal, project_id) {
                return Err(EdgeError::NotFound("project not found".into()));
            }
        }
        let repository = intent
            .workspace
            .as_ref()
            .and_then(|workspace| {
                workspace
                    .repository
                    .as_ref()
                    .map(|repository| (workspace, repository))
            })
            .map(|(workspace, repository)| {
                self.git
                    .resolve_workspace_repository(
                        principal,
                        repository,
                        workspace.base_ref.as_deref().unwrap_or("main"),
                    )
                    .map_err(map_durable_err)
            })
            .transpose()?;
        let mut proposal =
            intent.proposal(self.conversation_ids.mint().as_str(), repository.as_ref())?;
        let agent = self
            .drive(self.agents.get(principal, &intent.agent_id.to_string()))?
            .map_err(map_agent_registry_error)?;
        if agent.created_by != principal.principal_id.0 {
            return Err(EdgeError::NotFound("agent not found".into()));
        }
        let agent_augmentation =
            crate::agent_http::open_agent_workspace_guidance(&self.kms, principal, &agent)?;
        let request_instructions = intent
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.instructions.as_deref());
        let request_files = intent
            .workspace
            .as_ref()
            .map(|workspace| workspace.files.as_slice())
            .unwrap_or_default();
        proposal.workspace_guidance = WorkspaceAugmentation::merge_for_thread(
            agent_augmentation.as_ref(),
            request_instructions,
            request_files,
        )?
        .as_ref()
        .map(|augmentation| {
            let payload = augmentation.encode()?;
            self.seal_workspace_guidance(
                principal,
                &proposal,
                &payload,
                intent.workspace_request_digest.clone(),
            )
        })
        .transpose()?;
        let outcome = self
            .drive(self.threads.create(&principal.tenant.0, proposal))?
            .map_err(|_| EdgeError::Internal("agent thread storage failed".into()))?;
        let (thread, created) = match outcome {
            CreateAgentThreadOutcome::Created(thread) => (thread, true),
            CreateAgentThreadOutcome::Replayed(thread) => (thread, false),
            CreateAgentThreadOutcome::Conflict => {
                return Err(EdgeError::Conflict(
                    "that idempotency key was already used for a different agent thread".into(),
                ))
            }
            CreateAgentThreadOutcome::NameConflict => {
                return Err(EdgeError::Conflict(
                    "a live agent thread already has that name".into(),
                ))
            }
            CreateAgentThreadOutcome::OwnerUnavailable => {
                return Err(EdgeError::NotFound("agent not found".into()))
            }
            CreateAgentThreadOutcome::AgentUnavailable => {
                return Err(EdgeError::NotFound("agent not found".into()))
            }
            CreateAgentThreadOutcome::AgentRuntimeUnsupported => {
                return Err(EdgeError::BadRequest(
                    "private agent threads require an external MCP agent".into(),
                ))
            }
        };
        if !created && parse_stored_timestamp(&thread.expires_at)? <= Utc::now() {
            return Ok((thread, false));
        }
        if !created
            && thread.state != AgentThreadState::Provisioning
            && thread.state != AgentThreadState::Ready
        {
            return Ok((thread, false));
        }
        self.provision(principal, thread)
            .map(|thread| (thread, created))
    }

    fn retry(
        &self,
        principal: &Principal,
        thread_id: Uuid,
    ) -> Result<DurableAgentThread, EdgeError> {
        let outcome = self
            .drive(self.threads.retry_provisioning(
                &principal.tenant.0,
                &principal.principal_id.0,
                thread_id,
                Utc::now(),
            ))?
            .map_err(|_| EdgeError::Internal("agent thread retry storage failed".into()))?;
        let thread = match outcome {
            RetryAgentThreadOutcome::Retrying(thread)
            | RetryAgentThreadOutcome::AlreadyReady(thread) => thread,
            RetryAgentThreadOutcome::NotFound => {
                return Err(EdgeError::NotFound("agent thread not found".into()))
            }
            RetryAgentThreadOutcome::Unavailable => {
                return Err(EdgeError::Conflict(
                    "agent thread is no longer available for provisioning".into(),
                ))
            }
        };
        self.provision(principal, thread)
    }

    fn mark_ready_for_review(
        &self,
        principal: &Principal,
        thread_id: Uuid,
    ) -> Result<(DurableAgentThread, bool), EdgeError> {
        let marked_at = Utc::now();
        let thread = self.available_owner_thread(principal, thread_id, marked_at)?;
        let project_id = thread.project_id.as_deref().ok_or_else(|| {
            EdgeError::Conflict(
                "an agent thread must belong to a project before it can be marked ready".into(),
            )
        })?;
        let conversation = ConversationId::new(
            principal.tenant.0.clone(),
            principal.region.0.clone(),
            thread.conversation_id.clone(),
        );
        self.chat_mutations
            .publish_private_conversation_to_project(
                principal,
                &conversation,
                project_id,
                Timestamp(thread.expires_at.clone()),
                Timestamp(marked_at.to_rfc3339_opts(SecondsFormat::Secs, true)),
            )?;
        let outcome = self
            .drive(self.threads.mark_review_ready(
                &principal.tenant.0,
                &principal.principal_id.0,
                thread_id,
                marked_at,
            ))?
            .map_err(|_| {
                EdgeError::Internal("agent thread review state could not be saved".into())
            })?;
        match outcome {
            MarkAgentThreadReviewReadyOutcome::Marked(thread) => Ok((thread, true)),
            MarkAgentThreadReviewReadyOutcome::AlreadyReady(thread) => Ok((thread, false)),
            MarkAgentThreadReviewReadyOutcome::NotFound => {
                Err(EdgeError::NotFound("agent thread not found".into()))
            }
            MarkAgentThreadReviewReadyOutcome::ProjectRequired => Err(EdgeError::Conflict(
                "an agent thread must belong to a project before it can be marked ready".into(),
            )),
            MarkAgentThreadReviewReadyOutcome::Unavailable => Err(EdgeError::Conflict(
                "agent thread is no longer available for review publication".into(),
            )),
        }
    }

    fn review_handoff(
        &self,
        principal: &Principal,
        thread_id: Uuid,
    ) -> Result<Option<AgentThreadReviewHandoff>, EdgeError> {
        self.drive(self.threads.review_handoff(
            &principal.tenant.0,
            &principal.principal_id.0,
            thread_id,
        ))?
        .map_err(|_| EdgeError::Internal("agent thread review handoff lookup failed".into()))
    }

    fn publish_review_handoff(
        &self,
        principal: &Principal,
        thread_id: Uuid,
        title: Option<String>,
        body_md: Option<String>,
        reviewers: Vec<String>,
        client_nonce: &str,
    ) -> Result<(AgentThreadReviewHandoff, bool, bool), EdgeError> {
        let existing = self.review_handoff(principal, thread_id)?;
        let observed_at = Utc::now();
        let thread = self.available_owner_thread(principal, thread_id, observed_at)?;
        if thread.review_ready_at.is_none() {
            return Err(EdgeError::Conflict(
                "mark the private thread ready before publishing its branch".into(),
            ));
        }
        let binding = thread.git.as_ref().ok_or_else(|| {
            EdgeError::Conflict("this agent thread has no repository branch to publish".into())
        })?;
        let workspace_id = parse_uuid("stored workspace id", &thread.workspace_id)
            .map_err(|_| EdgeError::Internal("stored agent workspace id is invalid".into()))?;
        let locator = thread.storage_locator.as_deref().ok_or_else(|| {
            EdgeError::Conflict("agent thread workspace is not available for review".into())
        })?;
        let resolved = self
            .git
            .reopen_workspace_repository(
                principal,
                &binding.repository,
                &binding.base_ref,
                &binding.base_oid,
            )
            .map_err(map_durable_err)?;
        let source = RepositoryWorkspaceSource {
            repository: resolved.repository,
            repository_path: resolved.repository_path,
            clone_url: resolved.clone_url,
            base_ref: resolved.base_ref,
            base_oid: resolved.base_oid,
            branch: binding.branch.clone(),
        };
        let head_ref = format!("refs/heads/{}", binding.branch);
        if let Some(handoff) = existing.as_ref() {
            if handoff.repository_slug != binding.repository
                || handoff.base_ref != binding.base_ref
                || handoff.head_ref != head_ref
            {
                return Err(EdgeError::Conflict(
                    "agent thread review handoff conflicts with its durable branch".into(),
                ));
            }
        }
        let protected_paths = self.protected_workspace_paths(principal, &thread, workspace_id)?;
        let published = self
            .workspaces
            .publish_repository_for_review(
                &principal.tenant.0,
                workspace_id,
                RepositoryReviewPublicationRequest {
                    source: &source,
                    storage_locator: locator,
                    protected_paths: &protected_paths,
                },
            )
            .map_err(map_workspace_publication_error)?;

        if let Some(existing) = existing {
            if existing.head_oid == published.head_oid {
                return Ok((existing, false, false));
            }
            let operation = PrOperationId::derive(
                "myelin.agent-thread.review-handoff-update.v2",
                &[
                    principal.tenant.0.as_bytes(),
                    principal.region.0.as_bytes(),
                    principal.principal_id.0.as_bytes(),
                    thread.thread_id.as_bytes(),
                    client_nonce.as_bytes(),
                ],
            )
            .map_err(map_durable_err)?;
            self.git
                .synchronize_pr_head_with_operation(
                    &principal.tenant.0,
                    &principal.region.0,
                    &binding.repository,
                    existing.pull_request_number,
                    &existing.head_oid,
                    &published.head_oid,
                    principal,
                    &operation,
                )
                .map_err(map_durable_err)?;
            let outcome = self
                .drive(self.threads.advance_review_handoff(
                    &principal.tenant.0,
                    &principal.principal_id.0,
                    thread_id,
                    &existing.head_oid,
                    &published.head_oid,
                    observed_at,
                ))?
                .map_err(|_| {
                    EdgeError::Internal(
                        "agent thread review handoff update could not be saved".into(),
                    )
                })?;
            return match outcome {
                AdvanceAgentThreadReviewHandoffOutcome::Advanced(handoff) => {
                    Ok((handoff, false, true))
                }
                AdvanceAgentThreadReviewHandoffOutcome::AlreadyCurrent(handoff) => {
                    Ok((handoff, false, false))
                }
                AdvanceAgentThreadReviewHandoffOutcome::NotFound => {
                    Err(EdgeError::NotFound("agent thread not found".into()))
                }
                AdvanceAgentThreadReviewHandoffOutcome::Conflict => Err(EdgeError::Conflict(
                    "agent thread review handoff changed during publication; retry the update"
                        .into(),
                )),
            };
        }

        let title = title.unwrap_or_else(|| thread.name.clone());
        let body_md = body_md.or_else(|| {
            Some(format!(
                "Published from private agent thread `{}` after its owner marked the work ready for review.",
                thread.name
            ))
        });
        let operation = PrOperationId::derive(
            "myelin.agent-thread.review-handoff.v1",
            &[
                principal.tenant.0.as_bytes(),
                principal.region.0.as_bytes(),
                thread.thread_id.as_bytes(),
            ],
        )
        .map_err(map_durable_err)?;
        let pull_request = self
            .git
            .open_pr_with_operation(
                &principal.tenant.0,
                &principal.region.0,
                &binding.repository,
                &json!({
                    "title": title,
                    "body_md": body_md,
                    "base_ref": binding.base_ref,
                    "head_ref": head_ref,
                    "head_oid": published.head_oid,
                    "draft": false,
                    "reviewers": reviewers,
                }),
                principal,
                &operation,
            )
            .map_err(map_durable_err)?;
        let proposal = NewAgentThreadReviewHandoff {
            repository_slug: binding.repository.clone(),
            base_ref: binding.base_ref.clone(),
            head_ref,
            head_oid: published.head_oid,
            pull_request_number: pull_request.number,
        };
        let outcome = self
            .drive(self.threads.bind_review_handoff(
                &principal.tenant.0,
                &principal.principal_id.0,
                thread_id,
                proposal,
                observed_at,
            ))?
            .map_err(|_| {
                EdgeError::Internal("agent thread review handoff could not be saved".into())
            })?;
        match outcome {
            BindAgentThreadReviewHandoffOutcome::Bound(handoff) => Ok((handoff, true, false)),
            BindAgentThreadReviewHandoffOutcome::AlreadyBound(handoff) => {
                Ok((handoff, false, false))
            }
            BindAgentThreadReviewHandoffOutcome::NotFound => {
                Err(EdgeError::NotFound("agent thread not found".into()))
            }
            BindAgentThreadReviewHandoffOutcome::Conflict => Err(EdgeError::Conflict(
                "agent thread review handoff conflicts with its durable branch".into(),
            )),
            BindAgentThreadReviewHandoffOutcome::Unavailable => Err(EdgeError::Conflict(
                "agent thread is not available for a review handoff".into(),
            )),
        }
    }

    fn renew(
        &self,
        principal: &Principal,
        thread_id: Uuid,
        retention_days: i16,
        client_nonce: String,
    ) -> Result<(DurableAgentThread, bool), EdgeError> {
        let observed_at = Utc::now();
        let outcome = self
            .drive(self.threads.begin_renewal(
                &principal.tenant.0,
                &principal.principal_id.0,
                thread_id,
                Uuid::new_v4(),
                retention_days,
                &client_nonce,
                observed_at,
            ))?
            .map_err(|_| EdgeError::Internal("agent thread renewal storage failed".into()))?;
        match outcome {
            BeginAgentThreadRenewalOutcome::Begun { renewal, thread } => {
                self.complete_renewal_authority(principal, &renewal)?;
                Ok((thread, true))
            }
            BeginAgentThreadRenewalOutcome::Replayed { renewal, thread } => {
                if renewal.completed_at.is_none() {
                    self.complete_renewal_authority(principal, &renewal)?;
                }
                Ok((thread, false))
            }
            BeginAgentThreadRenewalOutcome::AlreadyRetained(thread) => Ok((thread, false)),
            BeginAgentThreadRenewalOutcome::Conflict => Err(EdgeError::Conflict(
                "that idempotency key was already used for a different workspace renewal".into(),
            )),
            BeginAgentThreadRenewalOutcome::NotFound => {
                Err(EdgeError::NotFound("agent thread not found".into()))
            }
            BeginAgentThreadRenewalOutcome::Unavailable => Err(EdgeError::Conflict(
                "agent thread workspace is no longer available for renewal".into(),
            )),
        }
    }

    fn complete_renewal_authority(
        &self,
        principal: &Principal,
        renewal: &AgentThreadRenewal,
    ) -> Result<(), EdgeError> {
        let conversation = ConversationId::new(
            principal.tenant.0.clone(),
            principal.region.0.clone(),
            renewal.conversation_id.clone(),
        );
        let completed_at = Utc::now();
        let agent_id = renewal.agent_id.to_string();
        self.chat_mutations.renew_private_conversation_authority(
            principal,
            PrivateConversationRenewal {
                conversation: &conversation,
                agent_id: &agent_id,
                project_authority_ready: renewal.review_ready,
                retention_days: i32::from(renewal.retention_days),
                expires_at: Timestamp(renewal.expires_at.clone()),
                now: Timestamp(completed_at.to_rfc3339_opts(SecondsFormat::Secs, true)),
            },
        )?;
        self.drive(self.threads.complete_renewal(
            &principal.tenant.0,
            renewal.renewal_id,
            completed_at,
        ))?
        .map_err(|_| EdgeError::Internal("agent thread renewal could not be completed".into()))?;
        Ok(())
    }

    fn recover(&self, tenant: &TenantId, region: &Region, thread: DurableAgentThread) {
        let owner = Principal::new(
            tenant.clone(),
            region.clone(),
            PrincipalId(thread.owner_principal_id.clone()),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
        );
        if let Err(error) = self.provision(&owner, thread) {
            eprintln!("agent thread recovery: provisioning attempt failed: {error}");
        }
    }

    fn recover_renewal(&self, tenant: &TenantId, region: &Region, renewal: AgentThreadRenewal) {
        let owner = Principal::new(
            tenant.clone(),
            region.clone(),
            PrincipalId(renewal.owner_principal_id.clone()),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
        );
        if let Err(error) = self.complete_renewal_authority(&owner, &renewal) {
            eprintln!("agent thread recovery: renewal attempt failed: {error}");
        }
    }

    fn provision(
        &self,
        principal: &Principal,
        thread: DurableAgentThread,
    ) -> Result<DurableAgentThread, EdgeError> {
        if thread.state == AgentThreadState::Ready {
            let (conversation, zookie) = self.create_private_conversation(principal, &thread)?;
            self.chat_mutations
                .activate_private_conversation(&conversation, &zookie)?;
            return self.complete_publication(principal, thread);
        }
        if thread.state != AgentThreadState::Provisioning {
            return Err(EdgeError::Conflict(
                "agent thread is not available for provisioning".into(),
            ));
        }

        let (conversation, zookie) = match self.create_private_conversation(principal, &thread) {
            Ok(conversation) => conversation,
            Err(error) => {
                self.record_provisioning_failure(
                    principal,
                    &thread,
                    AgentThreadProvisioningFailure::ChatUnavailable,
                )?;
                return Err(error);
            }
        };
        let workspace_id = parse_uuid("stored workspace id", &thread.workspace_id)
            .map_err(|_| EdgeError::Internal("stored agent workspace id is invalid".into()))?;
        let provisioned = match self.provision_workspace(principal, &thread, workspace_id) {
            Ok(provisioned) => provisioned,
            Err(_) => {
                self.record_provisioning_failure(
                    principal,
                    &thread,
                    AgentThreadProvisioningFailure::WorkspaceUnavailable,
                )?;
                return Err(EdgeError::Unavailable(
                    "agent workspace provisioning failed".into(),
                ));
            }
        };
        if let Err(error) = self.apply_workspace_guidance(principal, &thread, workspace_id) {
            self.record_provisioning_failure(
                principal,
                &thread,
                AgentThreadProvisioningFailure::WorkspaceUnavailable,
            )?;
            return Err(error);
        }
        let environment =
            provisioned
                .environment
                .as_ref()
                .map(|environment| AgentThreadEnvironmentBinding {
                    provider: environment.provider.clone(),
                    config_path: environment.config_path.clone(),
                    content_digest: environment.content_digest.clone(),
                    state: match environment.state {
                        WorkspaceEnvironmentState::Declared => {
                            AgentThreadEnvironmentState::Declared
                        }
                        WorkspaceEnvironmentState::Invalid => AgentThreadEnvironmentState::Invalid,
                    },
                });
        let activation = self.drive(
            self.threads.activate(
                &principal.tenant.0,
                &principal.principal_id.0,
                parse_uuid("stored thread id", &thread.thread_id)
                    .map_err(|_| EdgeError::Internal("stored agent thread id is invalid".into()))?,
                AgentThreadActivation {
                    workspace_id,
                    conversation_id: thread.conversation_id.clone(),
                    storage_locator: provisioned.locator,
                    environment,
                },
            ),
        )?;
        let activated = match activation {
            Ok(activated) => activated,
            Err(_) => {
                self.record_provisioning_failure(
                    principal,
                    &thread,
                    AgentThreadProvisioningFailure::ActivationUnavailable,
                )?;
                return Err(EdgeError::Internal("agent thread activation failed".into()));
            }
        };
        let ready = match activated {
            ActivateAgentThreadOutcome::Activated(ready)
            | ActivateAgentThreadOutcome::AlreadyReady(ready) => ready,
            ActivateAgentThreadOutcome::NotFound | ActivateAgentThreadOutcome::Conflict => {
                return Err(EdgeError::Conflict(
                    "agent thread provisioning receipt changed".into(),
                ))
            }
        };
        self.chat_mutations
            .activate_private_conversation(&conversation, &zookie)?;
        self.complete_publication(principal, ready)
    }

    fn provision_workspace(
        &self,
        principal: &Principal,
        thread: &DurableAgentThread,
        workspace_id: Uuid,
    ) -> Result<myelin_agent_service::workspace::ProvisionedWorkspace, String> {
        let Some(binding) = thread.git.as_ref() else {
            return self
                .workspaces
                .provision(&principal.tenant.0, workspace_id)
                .map_err(|error| error.to_string());
        };
        let resolved = self
            .git
            .reopen_workspace_repository(
                principal,
                &binding.repository,
                &binding.base_ref,
                &binding.base_oid,
            )
            .map_err(|error| error.to_string())?;
        self.workspaces
            .provision_from_repository(
                &principal.tenant.0,
                workspace_id,
                &RepositoryWorkspaceSource {
                    repository: resolved.repository,
                    repository_path: resolved.repository_path,
                    clone_url: resolved.clone_url,
                    base_ref: resolved.base_ref,
                    base_oid: resolved.base_oid,
                    branch: binding.branch.clone(),
                },
            )
            .map_err(|error| error.to_string())
    }

    fn seal_workspace_guidance(
        &self,
        principal: &Principal,
        proposal: &NewAgentThread,
        payload: &[u8],
        request_content_digest: Option<String>,
    ) -> Result<AgentThreadWorkspaceGuidance, EdgeError> {
        let content_digest = workspace_guidance_digest(payload);
        let encrypted_content = ColumnCryptor::new(&self.kms, principal.region.clone())
            .encrypt_for_subject_scope_with_aad(
                &principal.tenant,
                &SubjectId::new(&principal.principal_id.0),
                SubjectKeyScope::AgentData,
                payload,
                &workspace_guidance_aad(
                    principal,
                    proposal.thread_id,
                    &proposal.owner_principal_id,
                    proposal.workspace_id,
                ),
            )
            .map_err(|error| {
                EdgeError::Internal(format!(
                    "agent workspace guidance encryption failed: {error}"
                ))
            })?;
        Ok(AgentThreadWorkspaceGuidance {
            content_digest,
            request_content_digest,
            encrypted_content,
        })
    }

    fn apply_workspace_guidance(
        &self,
        principal: &Principal,
        thread: &DurableAgentThread,
        workspace_id: Uuid,
    ) -> Result<(), EdgeError> {
        let Some(augmentation) = self.workspace_augmentation(principal, thread, workspace_id)?
        else {
            return Ok(());
        };
        let guidance = thread
            .workspace_guidance
            .as_ref()
            .expect("decoded workspace augmentation has a durable guidance receipt");
        if let Some(instructions) = augmentation.agents_md.as_deref() {
            let existing =
                match self
                    .workspaces
                    .read_file(&principal.tenant.0, workspace_id, "AGENTS.md")
                {
                    Ok(file) => file.bytes,
                    Err(WorkspaceAccessError::NotFound) => Vec::new(),
                    Err(error) => {
                        return Err(EdgeError::Unavailable(format!(
                            "agent workspace AGENTS.md could not be read: {error}"
                        )))
                    }
                };
            let personalized =
                personalized_agents_file(&existing, instructions, &guidance.content_digest)?;
            if personalized != existing {
                self.workspaces
                    .write_file(
                        &principal.tenant.0,
                        workspace_id,
                        "AGENTS.md",
                        &personalized,
                    )
                    .map_err(|error| {
                        EdgeError::Unavailable(format!(
                            "agent workspace AGENTS.md could not be personalized: {error}"
                        ))
                    })?;
            }
        }
        for file in augmentation.files {
            self.apply_workspace_file(principal, workspace_id, &file)?;
        }
        Ok(())
    }

    fn workspace_augmentation(
        &self,
        principal: &Principal,
        thread: &DurableAgentThread,
        workspace_id: Uuid,
    ) -> Result<Option<WorkspaceAugmentation>, EdgeError> {
        let Some(guidance) = thread.workspace_guidance.as_ref() else {
            return Ok(None);
        };
        let thread_id = parse_uuid("stored thread id", &thread.thread_id)
            .map_err(|_| EdgeError::Internal("stored agent thread id is invalid".into()))?;
        let plaintext = ColumnCryptor::new(&self.kms, principal.region.clone())
            .decrypt_with_aad(
                &guidance.encrypted_content,
                &workspace_guidance_aad(
                    principal,
                    thread_id,
                    &thread.owner_principal_id,
                    workspace_id,
                ),
            )
            .map_err(|_| {
                EdgeError::Internal("stored agent workspace guidance cannot be decrypted".into())
            })?;
        if workspace_guidance_digest(&plaintext) != guidance.content_digest {
            return Err(EdgeError::Internal(
                "stored agent workspace guidance failed integrity validation".into(),
            ));
        }
        WorkspaceAugmentation::decode(&plaintext).map(Some)
    }

    fn protected_workspace_paths(
        &self,
        principal: &Principal,
        thread: &DurableAgentThread,
        workspace_id: Uuid,
    ) -> Result<Vec<String>, EdgeError> {
        let Some(augmentation) = self.workspace_augmentation(principal, thread, workspace_id)?
        else {
            return Ok(Vec::new());
        };
        let mut paths = augmentation
            .files
            .into_iter()
            .map(|file| file.path)
            .collect::<Vec<_>>();
        if augmentation.agents_md.is_some() {
            paths.push("AGENTS.md".into());
        }
        paths.sort();
        paths.dedup();
        Ok(paths)
    }

    fn apply_workspace_file(
        &self,
        principal: &Principal,
        workspace_id: Uuid,
        file: &WorkspaceFile,
    ) -> Result<(), EdgeError> {
        match self
            .workspaces
            .read_file(&principal.tenant.0, workspace_id, &file.path)
        {
            Ok(existing) if existing.bytes == file.contents.as_bytes() => Ok(()),
            Ok(_) => Err(EdgeError::Internal(format!(
                "agent workspace augmentation conflicts with existing {}",
                file.path
            ))),
            Err(WorkspaceAccessError::NotFound) => self
                .workspaces
                .write_file(
                    &principal.tenant.0,
                    workspace_id,
                    &file.path,
                    file.contents.as_bytes(),
                )
                .map(|_| ())
                .map_err(|error| {
                    EdgeError::Unavailable(format!(
                        "agent workspace augmentation {} could not be created: {error}",
                        file.path
                    ))
                }),
            Err(error) => Err(EdgeError::Unavailable(format!(
                "agent workspace augmentation {} could not be inspected: {error}",
                file.path
            ))),
        }
    }

    fn complete_publication(
        &self,
        principal: &Principal,
        thread: DurableAgentThread,
    ) -> Result<DurableAgentThread, EdgeError> {
        let completed = self
            .drive(
                self.threads.complete_publication(
                    &principal.tenant.0,
                    &principal.principal_id.0,
                    parse_uuid("stored thread id", &thread.thread_id).map_err(|_| {
                        EdgeError::Internal("stored agent thread id is invalid".into())
                    })?,
                    &thread.conversation_id,
                    Utc::now(),
                ),
            )?
            .map_err(|_| EdgeError::Internal("agent thread publication failed".into()))?;
        completed.ok_or_else(|| {
            EdgeError::Conflict("agent thread changed while publication was completing".into())
        })
    }

    fn record_provisioning_failure(
        &self,
        principal: &Principal,
        thread: &DurableAgentThread,
        failure: AgentThreadProvisioningFailure,
    ) -> Result<(), EdgeError> {
        let thread_id = parse_uuid("stored thread id", &thread.thread_id)
            .map_err(|_| EdgeError::Internal("stored agent thread id is invalid".into()))?;
        self.drive(self.threads.record_provisioning_failure(
            &principal.tenant.0,
            &principal.principal_id.0,
            thread_id,
            failure,
            Utc::now(),
        ))?
        .map_err(|_| EdgeError::Internal("agent thread failure could not be recorded".into()))?;
        Ok(())
    }

    fn owner_thread(
        &self,
        principal: &Principal,
        thread_id: Uuid,
    ) -> Result<DurableAgentThread, EdgeError> {
        self.drive(self.threads.get_for_owner(
            &principal.tenant.0,
            &principal.principal_id.0,
            thread_id,
        ))?
        .map_err(|_| EdgeError::Internal("agent thread lookup failed".into()))?
        .ok_or_else(|| EdgeError::NotFound("agent thread not found".into()))
    }

    fn available_owner_thread(
        &self,
        principal: &Principal,
        thread_id: Uuid,
        observed_at: DateTime<Utc>,
    ) -> Result<DurableAgentThread, EdgeError> {
        let thread = self.owner_thread(principal, thread_id)?;
        if !workspace_is_available(&thread, observed_at)? {
            return Err(EdgeError::NotFound("agent thread not found".into()));
        }
        Ok(thread)
    }

    fn create_private_conversation(
        &self,
        principal: &Principal,
        thread: &DurableAgentThread,
    ) -> Result<(ConversationId, String), EdgeError> {
        let conversation_id = ConversationId::new(
            principal.tenant.0.clone(),
            principal.region.0.clone(),
            thread.conversation_id.clone(),
        );
        let event_principal = pseudonymized_event_principal(&principal.tenant.0, principal);
        let conversation = Conversation {
            home_cell: Conversation::home_cell_for(&conversation_id),
            id: conversation_id,
            kind: ConversationKind::ChannelPrivate,
            parent_project: thread.project_id.clone(),
            name: Some(thread.name.clone()),
            topic: Some("Private agent work".into()),
            linked_ref: Some(thread_ref(&principal.tenant.0, &thread.thread_id)),
            pinned_canvas: None,
            retention_days: Some(i32::from(thread.retention_days)),
            archived: false,
            created_by: event_principal.principal_id.0.clone(),
            acl_zookie: None,
        };
        let (conversation, _, zookie) = self.chat_mutations.create_private_conversation(
            principal,
            PrivateConversationCreation {
                conversation,
                members: vec![
                    principal.principal_id.clone(),
                    PrincipalId(format!("agent:{}", thread.agent_id)),
                ],
                expires_at: Some(Timestamp(thread.expires_at.clone())),
                client_nonce: format!("agent-thread-chat-{}", thread.thread_id),
                event_id: EventId(self.event_ids.mint().0),
                actor: Actor(event_principal),
                now: Timestamp(thread.created_at.clone()),
                defer_visibility: true,
            },
        )?;
        Ok((conversation.id, zookie))
    }
}

pub struct AgentThreadHttpInputs {
    pub threads: DurableAgentThreadBacking,
    pub agents: PgAgentRegistry,
    pub chat_reads: DurableChatReadApi,
    pub chat_mutations: DurableChatMutationApi,
    pub workspaces: Arc<dyn AgentWorkspaceStore>,
    pub git: Arc<DurableGitBackend>,
    pub sessions: AgentSessionIssuer,
    pub ssh_routes: WorkspaceSshRouteKey,
    pub ssh_endpoint: WorkspaceSshEndpoint,
    pub kms: Arc<KmsEngine>,
    pub runtime: Handle,
    pub recovery: AgentThreadRecoveryConfig,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateAgentThreadBody {
    name: String,
    agent_id: String,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default = "default_retention_days")]
    retention_days: i16,
    #[serde(default)]
    workspace: Option<CreateAgentThreadWorkspaceBody>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateAgentThreadWorkspaceBody {
    #[serde(default)]
    repository: Option<String>,
    #[serde(default)]
    base_ref: Option<String>,
    #[serde(default)]
    branch: Option<String>,
    #[serde(default)]
    instructions: Option<String>,
    #[serde(default)]
    files: Vec<WorkspaceFile>,
}

struct ValidatedThreadIntent {
    owner_principal_id: String,
    name: String,
    agent_id: Uuid,
    project_id: Option<String>,
    retention_days: i16,
    workspace: Option<CreateAgentThreadWorkspaceBody>,
    workspace_request_digest: Option<String>,
    client_nonce: String,
    now: DateTime<Utc>,
}

impl ValidatedThreadIntent {
    fn new(
        principal: &Principal,
        body: CreateAgentThreadBody,
        client_nonce: String,
    ) -> Result<Self, EdgeError> {
        validate_name(&body.name)?;
        let agent_id = parse_uuid("agent_id", &body.agent_id)?;
        if let Some(project_id) = body.project_id.as_deref() {
            parse_uuid("project_id", project_id)?;
        }
        if !(MIN_AGENT_THREAD_RETENTION_DAYS..=MAX_AGENT_THREAD_RETENTION_DAYS)
            .contains(&body.retention_days)
        {
            return Err(EdgeError::BadRequest(format!(
                "agent thread retention_days must be between {MIN_AGENT_THREAD_RETENTION_DAYS} and {MAX_AGENT_THREAD_RETENTION_DAYS}"
            )));
        }
        if let Some(workspace) = body.workspace.as_ref() {
            if workspace.repository.is_none()
                && (workspace.base_ref.is_some() || workspace.branch.is_some())
            {
                return Err(EdgeError::BadRequest(
                    "agent thread workspace base_ref and branch require a repository".into(),
                ));
            }
            if workspace.repository.is_none()
                && workspace.instructions.is_none()
                && workspace.files.is_empty()
            {
                return Err(EdgeError::BadRequest(
                    "agent thread workspace must configure a repository, instructions, or files"
                        .into(),
                ));
            }
            if let Some(repository) = workspace.repository.as_deref() {
                if repository.is_empty()
                    || repository.len() > 255
                    || repository.chars().any(char::is_control)
                {
                    return Err(EdgeError::BadRequest(
                        "agent thread workspace repository must be clean bounded text".into(),
                    ));
                }
                validate_workspace_branch(
                    workspace.base_ref.as_deref().unwrap_or("main"),
                    "base_ref",
                    true,
                )?;
                if let Some(branch) = workspace.branch.as_deref() {
                    validate_workspace_branch(branch, "branch", false)?;
                }
            }
            if let Some(instructions) = workspace.instructions.as_deref() {
                validate_workspace_instructions(instructions)?;
            }
            validate_workspace_files(&workspace.files)?;
        }
        let workspace_request_digest = body
            .workspace
            .as_ref()
            .map(workspace_request_digest)
            .transpose()?
            .flatten();
        let now = Utc::now()
            .with_nanosecond(0)
            .expect("zero nanoseconds is a valid timestamp");
        Ok(Self {
            owner_principal_id: principal.principal_id.0.clone(),
            name: body.name,
            agent_id,
            project_id: body.project_id,
            retention_days: body.retention_days,
            workspace: body.workspace,
            workspace_request_digest,
            client_nonce,
            now,
        })
    }

    fn proposal(
        &self,
        conversation_id: &str,
        repository: Option<&ResolvedWorkspaceRepository>,
    ) -> Result<NewAgentThread, EdgeError> {
        let expires_at = self
            .now
            .checked_add_signed(Duration::days(i64::from(self.retention_days)))
            .ok_or_else(|| EdgeError::BadRequest("agent thread retention overflowed".into()))?;
        Ok(NewAgentThread {
            thread_id: Uuid::new_v4(),
            owner_principal_id: self.owner_principal_id.clone(),
            agent_id: self.agent_id,
            conversation_id: conversation_id.into(),
            workspace_id: Uuid::new_v4(),
            name: self.name.clone(),
            project_id: self
                .project_id
                .as_deref()
                .map(|value| parse_uuid("project_id", value))
                .transpose()?,
            git: repository.map(|repository| AgentThreadGitBinding {
                repository: repository.repository.clone(),
                base_ref: repository.base_ref.clone(),
                base_oid: repository.base_oid.clone(),
                branch: self
                    .workspace
                    .as_ref()
                    .and_then(|workspace| workspace.branch.clone())
                    .unwrap_or_else(|| {
                        default_workspace_branch(&self.owner_principal_id, &self.client_nonce)
                    }),
            }),
            workspace_guidance: None,
            retention_days: self.retention_days,
            client_nonce: self.client_nonce.clone(),
            created_at: self.now,
            expires_at,
        })
    }

    fn matches(&self, thread: &DurableAgentThread) -> bool {
        if thread.owner_principal_id != self.owner_principal_id
            || thread.agent_id != self.agent_id.to_string()
            || thread.name != self.name
            || thread.project_id != self.project_id
            || thread.retention_days != self.retention_days
        {
            return false;
        }
        if self.workspace_request_digest.as_deref()
            != thread
                .workspace_guidance
                .as_ref()
                .and_then(|guidance| guidance.request_content_digest.as_deref())
        {
            return false;
        }
        match (
            self.workspace
                .as_ref()
                .and_then(|workspace| workspace.repository.as_ref().map(|_| workspace)),
            thread.git.as_ref(),
        ) {
            (None, None) => true,
            (Some(requested), Some(stored)) => {
                let branch = requested.branch.clone().unwrap_or_else(|| {
                    default_workspace_branch(&self.owner_principal_id, &self.client_nonce)
                });
                stored.repository == requested.repository.as_deref().unwrap_or_default()
                    && stored.base_ref
                        == normalized_workspace_base_ref(
                            requested.base_ref.as_deref().unwrap_or("main"),
                        )
                    && stored.branch == branch
            }
            _ => false,
        }
    }
}

struct AgentThreadCreateHandler {
    api: AgentThreadHttpApi,
}

impl Handler for AgentThreadCreateHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_query(ctx)?;
        let body = parse_create_body(&ctx.request.body)?;
        let client_nonce = ctx
            .request
            .stable_idempotency_nonce(&ctx.principal.principal_id.0)?;
        let (thread, created) = self.api.create(ctx.principal, body, client_nonce)?;
        Ok(no_store(EdgeResponse::json(
            if created { 201 } else { 200 },
            &json!({
                "thread": thread_json(&ctx.principal.tenant.0, &thread),
                "created": created,
                "durable": true,
            }),
        )))
    }
}

struct AgentThreadGetHandler {
    api: AgentThreadHttpApi,
}

struct AgentThreadRetryHandler {
    api: AgentThreadHttpApi,
}

impl Handler for AgentThreadRetryHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_query(ctx)?;
        crate::request::require_empty_json_object(
            &ctx.request.body,
            "private agent thread retry",
            MAX_AGENT_THREAD_JSON_BYTES,
        )?;
        let thread = self.api.retry(ctx.principal, thread_param(ctx)?)?;
        Ok(no_store(EdgeResponse::json(
            200,
            &json!({
                "thread": thread_json(&ctx.principal.tenant.0, &thread),
                "created": false,
                "durable": true,
            }),
        )))
    }
}

struct AgentThreadReviewReadyHandler {
    api: AgentThreadHttpApi,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentThreadRenewalBody {
    retention_days: i16,
}

struct AgentThreadRenewalHandler {
    api: AgentThreadHttpApi,
}

impl Handler for AgentThreadRenewalHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_query(ctx)?;
        let body: AgentThreadRenewalBody =
            parse_interactive_body(&ctx.request.body, "agent thread renewal")?;
        if !(MIN_AGENT_THREAD_RETENTION_DAYS..=MAX_AGENT_THREAD_RETENTION_DAYS)
            .contains(&body.retention_days)
        {
            return Err(EdgeError::BadRequest(format!(
                "agent thread renewal retention_days must be between {MIN_AGENT_THREAD_RETENTION_DAYS} and {MAX_AGENT_THREAD_RETENTION_DAYS}"
            )));
        }
        let client_nonce = ctx
            .request
            .stable_idempotency_nonce(&ctx.principal.principal_id.0)?;
        let (thread, changed) = self.api.renew(
            ctx.principal,
            thread_param(ctx)?,
            body.retention_days,
            client_nonce,
        )?;
        Ok(no_store(EdgeResponse::json(
            200,
            &json!({
                "thread": thread_json(&ctx.principal.tenant.0, &thread),
                "changed": changed,
                "durable": true,
            }),
        )))
    }
}

impl Handler for AgentThreadReviewReadyHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_query(ctx)?;
        crate::request::require_empty_json_object(
            &ctx.request.body,
            "agent thread review publication",
            MAX_AGENT_THREAD_JSON_BYTES,
        )?;
        let (thread, changed) = self
            .api
            .mark_ready_for_review(ctx.principal, thread_param(ctx)?)?;
        Ok(no_store(EdgeResponse::json(
            200,
            &json!({
                "thread": thread_json(&ctx.principal.tenant.0, &thread),
                "changed": changed,
                "durable": true,
            }),
        )))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentThreadReviewHandoffBody {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    body_md: Option<String>,
    #[serde(default)]
    reviewers: Vec<String>,
}

struct AgentThreadReviewHandoffGetHandler {
    api: AgentThreadHttpApi,
}

impl Handler for AgentThreadReviewHandoffGetHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_query(ctx)?;
        require_empty_body(ctx)?;
        let handoff = self
            .api
            .review_handoff(ctx.principal, thread_param(ctx)?)?
            .ok_or_else(|| EdgeError::NotFound("agent thread review handoff not found".into()))?;
        Ok(no_store(EdgeResponse::json(
            200,
            &json!({
                "handoff": review_handoff_json(&ctx.principal.tenant.0, &handoff),
                "durable": true,
            }),
        )))
    }
}

struct AgentThreadReviewHandoffCreateHandler {
    api: AgentThreadHttpApi,
}

impl Handler for AgentThreadReviewHandoffCreateHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_query(ctx)?;
        let body: AgentThreadReviewHandoffBody =
            parse_interactive_body(&ctx.request.body, "agent thread review handoff")?;
        let client_nonce = ctx
            .request
            .stable_idempotency_nonce(&ctx.principal.principal_id.0)?;
        let (handoff, created, updated) = self.api.publish_review_handoff(
            ctx.principal,
            thread_param(ctx)?,
            body.title,
            body.body_md,
            body.reviewers,
            &client_nonce,
        )?;
        Ok(no_store(EdgeResponse::json(
            if created { 201 } else { 200 },
            &json!({
                "handoff": review_handoff_json(&ctx.principal.tenant.0, &handoff),
                "created": created,
                "updated": updated,
                "durable": true,
            }),
        )))
    }
}

struct AgentThreadRunCreateHandler {
    api: AgentThreadHttpApi,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentThreadMessageBody {
    content: String,
}

struct AgentThreadMessageListHandler {
    api: AgentThreadHttpApi,
}

impl Handler for AgentThreadMessageListHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_body(ctx)?;
        let thread =
            self.api
                .available_owner_thread(ctx.principal, thread_param(ctx)?, Utc::now())?;
        let (limit, before) = parse_messages_query(&ctx.request.query)?;
        Ok(no_store(EdgeResponse::json(
            200,
            &self.api.chat_reads.read_messages(
                ctx.principal,
                &thread.conversation_id,
                limit,
                before,
            )?,
        )))
    }
}

struct AgentThreadMessagePostHandler {
    api: AgentThreadHttpApi,
}

impl Handler for AgentThreadMessagePostHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_query(ctx)?;
        let body: AgentThreadMessageBody =
            parse_interactive_body(&ctx.request.body, "agent thread message")?;
        let thread =
            self.api
                .available_owner_thread(ctx.principal, thread_param(ctx)?, Utc::now())?;
        let client_nonce = ctx
            .request
            .stable_idempotency_nonce(&ctx.principal.principal_id.0)?;
        let message_id = self.api.chat_mutations.post_message(
            ctx.principal,
            ctx.principal,
            &thread.conversation_id,
            &body.content,
            &[],
            client_nonce,
        )?;
        Ok(no_store(EdgeResponse::json(
            201,
            &json!({
                "message_id": message_id.as_str(),
                "thread_id": thread.thread_id,
                "durable": true,
            }),
        )))
    }
}

impl Handler for AgentThreadRunCreateHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_query(ctx)?;
        crate::request::require_empty_json_object(
            &ctx.request.body,
            "private agent thread run",
            MAX_AGENT_THREAD_JSON_BYTES,
        )?;
        let thread_id = thread_param(ctx)?;
        let thread = self.api.owner_thread(ctx.principal, thread_id)?;
        let now = Utc::now();
        let workspace_expires_at = parse_stored_timestamp(&thread.expires_at)?;
        if !workspace_is_available_at(&thread, workspace_expires_at, now) {
            return Err(EdgeError::Conflict(
                "agent thread workspace is not available for a new run".into(),
            ));
        }

        let client_nonce = ctx
            .request
            .stable_idempotency_nonce(&ctx.principal.principal_id.0)?;
        let capability = ctx.identity.capability();
        let issued = self
            .api
            .drive(self.api.sessions.start_until(
                ctx.principal,
                AgentSessionRequest {
                    agent_id: thread.agent_id.clone(),
                    client_nonce,
                    trigger_credential_jti: capability.jti.clone(),
                    trigger_expires_at_unix: capability.expires_at_unix,
                    trigger_authority: capability.effective_authority.clone(),
                    now,
                },
                workspace_expires_at,
            ))?
            .map_err(map_session_error)?;
        let run_id = parse_uuid("stored run id", &issued.session.run_id)
            .map_err(|_| EdgeError::Internal("stored agent run id is invalid".into()))?;
        let binding = self
            .api
            .drive(self.api.threads.bind_run(
                &ctx.principal.tenant.0,
                &ctx.principal.principal_id.0,
                thread_id,
                run_id,
                now,
            ))?
            .map_err(|_| EdgeError::Internal("agent thread run binding failed".into()))?;
        let binding = match binding {
            BindAgentThreadRunOutcome::Bound(binding)
            | BindAgentThreadRunOutcome::Replayed(binding) => binding,
            BindAgentThreadRunOutcome::NotFound | BindAgentThreadRunOutcome::Conflict => {
                self.api
                    .drive(self.api.sessions.revoke_unreturned(ctx.principal, &issued))?
                    .map_err(map_session_error)?;
                return Err(EdgeError::Conflict(
                    "agent thread changed while its run was starting".into(),
                ));
            }
        };
        Ok(no_store(EdgeResponse::json(
            if issued.created { 201 } else { 200 },
            &thread_run_json(
                &ctx.principal.tenant.0,
                &self.api.catalogue,
                &issued,
                &binding,
            ),
        )))
    }
}

impl Handler for AgentThreadGetHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_query(ctx)?;
        require_empty_body(ctx)?;
        let thread_id = thread_param(ctx)?;
        let thread = self.api.owner_thread(ctx.principal, thread_id)?;
        Ok(no_store(EdgeResponse::json(
            200,
            &json!({ "thread": thread_json(&ctx.principal.tenant.0, &thread) }),
        )))
    }
}

struct AgentThreadListHandler {
    api: AgentThreadHttpApi,
}

impl Handler for AgentThreadListHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_body(ctx)?;
        let (limit, cursor) = page_query(&ctx.request.query)?;
        let outcome = self
            .api
            .drive(self.api.threads.list_for_owner(
                &ctx.principal.tenant.0,
                &ctx.principal.principal_id.0,
                cursor,
                limit + 1,
            ))?
            .map_err(|_| EdgeError::Internal("agent thread list failed".into()))?;
        let ListAgentThreadsOutcome::Page(mut threads) = outcome else {
            return Err(EdgeError::BadRequest(
                "agent thread cursor does not identify this owner's thread list".into(),
            ));
        };
        let has_more = threads.len() > limit as usize;
        threads.truncate(limit as usize);
        let next = has_more
            .then(|| threads.last().map(|thread| thread.thread_id.clone()))
            .flatten();
        let items = threads
            .iter()
            .map(|thread| thread_json(&ctx.principal.tenant.0, thread))
            .collect::<Vec<_>>();
        Ok(no_store(EdgeResponse::json(
            200,
            &page_envelope(json!(items), next, limit as usize),
        )))
    }
}

pub fn register_agent_threads(
    builder: GatewayBuilder,
    inputs: AgentThreadHttpInputs,
) -> (GatewayBuilder, AgentThreadRecoveryHandle) {
    let ssh = WorkspaceSshHttpInputs {
        threads: inputs.threads.clone(),
        routes: inputs.ssh_routes,
        endpoint: inputs.ssh_endpoint,
        runtime: inputs.runtime.clone(),
    };
    let recovery_config = inputs.recovery;
    let api = AgentThreadHttpApi {
        threads: inputs.threads,
        agents: inputs.agents,
        chat_reads: inputs.chat_reads,
        chat_mutations: inputs.chat_mutations,
        workspaces: inputs.workspaces,
        git: inputs.git,
        sessions: inputs.sessions,
        catalogue: Arc::new(
            PlatformToolCatalogue::platform()
                .expect("the built-in platform ToolDef catalogue must be valid"),
        ),
        event_ids: Arc::new(myelin_events::UlidMinter::new()),
        conversation_ids: Arc::new(SystemUlidSource::new()),
        kms: inputs.kms,
        runtime: inputs.runtime,
    };
    let builder = builder
        .route(
            Method::Get,
            "/v1/agent-threads",
            "identity.agent_threads.list",
            Arc::new(AgentThreadListHandler { api: api.clone() }),
        )
        .route(
            Method::Post,
            "/v1/agent-threads",
            "identity.agent_thread.create",
            Arc::new(AgentThreadCreateHandler { api: api.clone() }),
        )
        .route(
            Method::Get,
            "/v1/agent-threads/{thread}",
            "identity.agent_thread.view",
            Arc::new(AgentThreadGetHandler { api: api.clone() }),
        )
        .route(
            Method::Post,
            "/v1/agent-threads/{thread}/retry",
            "identity.agent_thread.create",
            Arc::new(AgentThreadRetryHandler { api: api.clone() }),
        )
        .route(
            Method::Post,
            "/v1/agent-threads/{thread}/ready",
            "identity.agent_thread.review.publish",
            Arc::new(AgentThreadReviewReadyHandler { api: api.clone() }),
        )
        .route(
            Method::Get,
            "/v1/agent-threads/{thread}/pull-request",
            "identity.agent_thread.view",
            Arc::new(AgentThreadReviewHandoffGetHandler { api: api.clone() }),
        )
        .route(
            Method::Post,
            "/v1/agent-threads/{thread}/pull-request",
            "identity.agent_thread.review.publish",
            Arc::new(AgentThreadReviewHandoffCreateHandler { api: api.clone() }),
        )
        .route(
            Method::Post,
            "/v1/agent-threads/{thread}/renew",
            "identity.agent_thread.create",
            Arc::new(AgentThreadRenewalHandler { api: api.clone() }),
        )
        .route(
            Method::Post,
            "/v1/agent-threads/{thread}/runs",
            "identity.agent_thread.run.create",
            Arc::new(AgentThreadRunCreateHandler { api: api.clone() }),
        )
        .route(
            Method::Get,
            "/v1/agent-threads/{thread}/messages",
            "chat.messages.list",
            Arc::new(AgentThreadMessageListHandler { api: api.clone() }),
        )
        .route(
            Method::Post,
            "/v1/agent-threads/{thread}/messages",
            "chat.message.post",
            Arc::new(AgentThreadMessagePostHandler { api: api.clone() }),
        );
    let builder = register_workspace_ssh_access(builder, ssh);
    let recovery = spawn_agent_thread_recovery(api, recovery_config);
    (builder, recovery)
}

fn spawn_agent_thread_recovery(
    api: AgentThreadHttpApi,
    config: AgentThreadRecoveryConfig,
) -> AgentThreadRecoveryHandle {
    let (shutdown, receiver) = watch::channel(false);
    let join = tokio::spawn(run_agent_thread_recovery(api, config, receiver));
    AgentThreadRecoveryHandle { shutdown, join }
}

async fn run_agent_thread_recovery(
    api: AgentThreadHttpApi,
    config: AgentThreadRecoveryConfig,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        if *shutdown.borrow() {
            return;
        }
        let now = Utc::now();
        let stale_before = now
            - chrono::Duration::from_std(config.stale_after)
                .expect("validated agent thread recovery staleness");
        for tenant in &config.tenants {
            if *shutdown.borrow() {
                return;
            }
            match api
                .threads
                .claim_provisioning_recovery(tenant.as_str(), stale_before, now, config.batch_limit)
                .await
            {
                Ok(work) => {
                    for thread in work {
                        if *shutdown.borrow() {
                            return;
                        }
                        api.recover(tenant, &config.region, thread);
                    }
                }
                Err(error) => eprintln!("agent thread recovery: tenant sweep failed: {error}"),
            }
            match api
                .threads
                .claim_pending_renewals(tenant.as_str(), stale_before, now, config.batch_limit)
                .await
            {
                Ok(work) => {
                    for renewal in work {
                        if *shutdown.borrow() {
                            return;
                        }
                        api.recover_renewal(tenant, &config.region, renewal);
                    }
                }
                Err(error) => {
                    eprintln!("agent thread recovery: renewal tenant sweep failed: {error}");
                }
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(config.interval) => {}
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return;
                }
            }
        }
    }
}

fn thread_run_json(
    tenant: &str,
    catalogue: &PlatformToolCatalogue,
    issued: &myelin_identity_service::IssuedAgentSession,
    binding: &AgentThreadRunBinding,
) -> Value {
    let mut value = agent_session_json(tenant, catalogue, issued);
    value["run"]["context"] = json!({
        "thread_id": binding.thread_id,
        "thread_ref": thread_ref(tenant, &binding.thread_id),
        "conversation_id": binding.conversation_id,
        "conversation_ref": format!(
            "myelin://{tenant}/chat/channel/{}",
            binding.conversation_id
        ),
        "workspace": {
            "id": binding.workspace_id,
            "generation": binding.workspace_generation,
            "expires_at": binding.workspace_expires_at,
        },
    });
    value
}

fn thread_json(tenant: &str, thread: &DurableAgentThread) -> Value {
    let public_state = agent_thread_public_state(thread);
    json!({
        "id": thread.thread_id,
        "ref": thread_ref(tenant, &thread.thread_id),
        "name": thread.name,
        "agent_id": thread.agent_id,
        "agent_ref": format!("myelin://{tenant}/identity/agent/{}", thread.agent_id),
        "project_id": thread.project_id,
        "conversation_id": thread.conversation_id,
        "conversation_ref": format!(
            "myelin://{tenant}/chat/channel/{}",
            thread.conversation_id
        ),
        "review_state": if thread.review_ready_at.is_some() { "ready" } else { "draft" },
        "review_ready_at": thread.review_ready_at,
        "workspace": {
            "id": thread.workspace_id,
            "generation": thread.workspace_generation,
            "state": public_state,
            "retention_days": thread.retention_days,
            "expires_at": thread.expires_at,
            "repository": thread.git.as_ref().map(|git| json!({
                "slug": git.repository,
                "base_ref": git.base_ref,
                "base_oid": git.base_oid,
                "branch": git.branch,
            })),
            "personalization": thread.workspace_guidance.as_ref().map(|guidance| json!({
                "format": "workspace_augmentation_v1",
                "content_digest": guidance.content_digest,
            })),
            "environment": thread.environment.as_ref().map(|environment| json!({
                "provider": environment.provider,
                "config_path": environment.config_path,
                "content_digest": environment.content_digest,
                "state": environment.state.token(),
            })),
        },
        "created_at": thread.created_at,
        "updated_at": thread.updated_at,
    })
}

pub(crate) fn agent_thread_public_state(thread: &DurableAgentThread) -> &'static str {
    if thread.state == AgentThreadState::Ready && !thread.is_published() {
        AgentThreadState::Provisioning.token()
    } else {
        thread.state.token()
    }
}

fn thread_ref(tenant: &str, thread_id: &str) -> String {
    format!("myelin://{tenant}/agent/thread/{thread_id}")
}

fn review_handoff_json(tenant: &str, handoff: &AgentThreadReviewHandoff) -> Value {
    json!({
        "thread_id": handoff.thread_id.to_string(),
        "repository": handoff.repository_slug,
        "base_ref": handoff.base_ref,
        "head_ref": handoff.head_ref,
        "head_oid": handoff.head_oid,
        "pull_request": {
            "number": handoff.pull_request_number,
            "ref": format!(
                "myelin://{tenant}/git/pr/{}:{}",
                handoff.repository_slug, handoff.pull_request_number
            ),
        },
        "created_at": handoff.created_at,
        "updated_at": handoff.updated_at,
    })
}

fn map_workspace_publication_error(error: WorkspacePublicationError) -> EdgeError {
    match error {
        WorkspacePublicationError::LocatorMismatch
        | WorkspacePublicationError::WorkspaceUnavailable => {
            EdgeError::Conflict("agent thread workspace is no longer available for review".into())
        }
        WorkspacePublicationError::NoChanges => EdgeError::Conflict(
            "commit at least one workspace change before opening a review".into(),
        ),
        WorkspacePublicationError::DirtyWorkspace(path) => EdgeError::Conflict(format!(
            "commit or discard the workspace change at `{path}` before opening a review"
        )),
        WorkspacePublicationError::ProtectedWorkspaceData(path) => EdgeError::Conflict(format!(
            "the review commit includes private workspace data at `{path}`; remove it from the commit before publishing"
        )),
        WorkspacePublicationError::SecretDetected(path) => EdgeError::Conflict(format!(
            "the review history contains a possible secret at `{path}`; rewrite it out of the branch before publishing"
        )),
        WorkspacePublicationError::ObjectTooLarge(path) => EdgeError::Conflict(format!(
            "the review history contains an oversized object at `{path}`; remove it from the branch before publishing"
        )),
        WorkspacePublicationError::HistoryTooLarge => EdgeError::Conflict(
            "the workspace review history exceeds safe publication limits".into(),
        ),
        WorkspacePublicationError::Diverged => EdgeError::Conflict(
            "the workspace review branch diverged from its durable publication history".into(),
        ),
        WorkspacePublicationError::InvalidRepository(reason) => EdgeError::Conflict(reason),
        WorkspacePublicationError::Io(_) => {
            EdgeError::Unavailable("agent workspace branch could not be published".into())
        }
    }
}

fn workspace_request_digest(
    workspace: &CreateAgentThreadWorkspaceBody,
) -> Result<Option<String>, EdgeError> {
    if workspace.files.is_empty() {
        return Ok(workspace
            .instructions
            .as_deref()
            .map(|instructions| workspace_guidance_digest(instructions.as_bytes())));
    }
    let augmentation =
        WorkspaceAugmentation::from_parts(workspace.instructions.clone(), workspace.files.clone())?
            .expect("workspace files make the augmentation non-empty");
    let payload = augmentation.encode()?;
    Ok(Some(workspace_guidance_digest(&payload)))
}

fn map_agent_registry_error(error: AgentRegistryError) -> EdgeError {
    match error {
        AgentRegistryError::BadInput(message) => EdgeError::BadRequest(message),
        AgentRegistryError::NotFound => EdgeError::NotFound("agent not found".into()),
        AgentRegistryError::Conflict(message) => EdgeError::Conflict(message),
        AgentRegistryError::Policy(message) => EdgeError::Forbidden(message),
        AgentRegistryError::Storage(message) => EdgeError::Internal(message),
    }
}

fn workspace_guidance_aad(
    principal: &Principal,
    thread_id: Uuid,
    owner_principal_id: &str,
    workspace_id: Uuid,
) -> Vec<u8> {
    let mut aad = Vec::new();
    let thread_id = thread_id.to_string();
    let workspace_id = workspace_id.to_string();
    for field in [
        "myelin.agent-thread.workspace-guidance.v1",
        principal.tenant.as_str(),
        principal.region.as_str(),
        thread_id.as_str(),
        owner_principal_id,
        workspace_id.as_str(),
    ] {
        aad.extend_from_slice(&(field.len() as u64).to_be_bytes());
        aad.extend_from_slice(field.as_bytes());
    }
    aad
}

fn personalized_agents_file(
    existing: &[u8],
    instructions: &str,
    content_digest: &str,
) -> Result<Vec<u8>, EdgeError> {
    let marker_prefix = b"<!-- myelin:personal-workspace-guidance:";
    let start = format!("<!-- myelin:personal-workspace-guidance:{content_digest} -->\n");
    let mut section = start.into_bytes();
    section.extend_from_slice(instructions.as_bytes());
    if !section.ends_with(b"\n") {
        section.push(b'\n');
    }
    section.extend_from_slice(b"<!-- /myelin:personal-workspace-guidance -->\n");

    if existing
        .windows(section.len())
        .any(|candidate| candidate == section)
    {
        return Ok(existing.to_vec());
    }
    if existing
        .windows(marker_prefix.len())
        .any(|candidate| candidate == marker_prefix)
    {
        return Err(EdgeError::Internal(
            "agent workspace has conflicting durable personal guidance".into(),
        ));
    }
    std::str::from_utf8(existing).map_err(|_| {
        EdgeError::Unavailable("agent workspace AGENTS.md is not UTF-8 text".into())
    })?;
    let mut personalized = existing.to_vec();
    if !personalized.is_empty() {
        if !personalized.ends_with(b"\n") {
            personalized.push(b'\n');
        }
        personalized.push(b'\n');
    }
    personalized.extend_from_slice(&section);
    Ok(personalized)
}

fn default_workspace_branch(owner: &str, client_nonce: &str) -> String {
    let mut digest = blake3::Hasher::new();
    digest.update(b"myelin.agent-thread.workspace-branch.v1\0");
    digest.update(owner.as_bytes());
    digest.update(b"\0");
    digest.update(client_nonce.as_bytes());
    format!("myelin/thread-{}", &digest.finalize().to_hex()[..20])
}

fn normalized_workspace_base_ref(value: &str) -> String {
    if value.starts_with("refs/heads/") {
        value.to_string()
    } else {
        format!("refs/heads/{value}")
    }
}

fn validate_workspace_branch(
    value: &str,
    field: &str,
    allow_full_ref: bool,
) -> Result<(), EdgeError> {
    let branch = if allow_full_ref {
        if let Some(branch) = value.strip_prefix("refs/heads/") {
            branch
        } else if value.starts_with("refs/") {
            return Err(EdgeError::BadRequest(format!(
                "agent thread workspace {field} must identify a branch"
            )));
        } else {
            value
        }
    } else {
        if value.starts_with("refs/") {
            return Err(EdgeError::BadRequest(format!(
                "agent thread workspace {field} must be a branch name, not a full ref"
            )));
        }
        value
    };
    let full_ref = format!("refs/heads/{branch}");
    if value.is_empty() || value.len() > 1024 || !git2::Reference::is_valid_name(&full_ref) {
        return Err(EdgeError::BadRequest(format!(
            "agent thread workspace {field} must be a canonical Git branch"
        )));
    }
    Ok(())
}

fn workspace_is_available(
    thread: &DurableAgentThread,
    observed_at: DateTime<Utc>,
) -> Result<bool, EdgeError> {
    Ok(workspace_is_available_at(
        thread,
        parse_stored_timestamp(&thread.expires_at)?,
        observed_at,
    ))
}

fn workspace_is_available_at(
    thread: &DurableAgentThread,
    expires_at: DateTime<Utc>,
    observed_at: DateTime<Utc>,
) -> bool {
    thread.is_published() && expires_at > observed_at
}

fn parse_create_body(body: &[u8]) -> Result<CreateAgentThreadBody, EdgeError> {
    parse_interactive_body(body, "agent thread")
}

fn parse_interactive_body<T: for<'de> Deserialize<'de>>(
    body: &[u8],
    resource: &str,
) -> Result<T, EdgeError> {
    if body.is_empty() {
        return Err(EdgeError::BadRequest(format!(
            "{resource} request body is empty"
        )));
    }
    if body.len() > MAX_AGENT_THREAD_JSON_BYTES {
        return Err(EdgeError::PayloadTooLarge(format!(
            "{resource} request exceeds the interactive body limit"
        )));
    }
    serde_json::from_slice(body)
        .map_err(|error| EdgeError::BadRequest(format!("invalid {resource} request: {error}")))
}

fn parse_uuid(field: &str, value: &str) -> Result<Uuid, EdgeError> {
    let parsed = Uuid::parse_str(value).map_err(|_| {
        EdgeError::BadRequest(format!("agent thread {field} must be a canonical UUID"))
    })?;
    if parsed.to_string() != value {
        return Err(EdgeError::BadRequest(format!(
            "agent thread {field} must be a canonical UUID"
        )));
    }
    Ok(parsed)
}

pub(crate) fn parse_stored_timestamp(value: &str) -> Result<DateTime<Utc>, EdgeError> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| EdgeError::Internal("stored agent thread expiry is invalid".into()))
}

fn validate_name(name: &str) -> Result<(), EdgeError> {
    if name.trim() == name
        && !name.is_empty()
        && name.len() <= MAX_AGENT_THREAD_NAME_BYTES
        && !name.chars().any(char::is_control)
    {
        Ok(())
    } else {
        Err(EdgeError::BadRequest(format!(
            "agent thread name must contain 1..={MAX_AGENT_THREAD_NAME_BYTES} clean UTF-8 bytes without surrounding whitespace"
        )))
    }
}

fn default_retention_days() -> i16 {
    DEFAULT_RETENTION_DAYS
}

fn page_query(query: &str) -> Result<(u32, Option<Uuid>), EdgeError> {
    let mut limit = None;
    let mut cursor = None;
    if !query.is_empty() {
        for pair in query.split('&') {
            let (name, value) = pair.split_once('=').ok_or_else(|| {
                EdgeError::BadRequest("malformed agent thread query parameter".into())
            })?;
            match name {
                "limit" if limit.is_none() => {
                    limit = Some(value.parse::<u32>().map_err(|_| {
                        EdgeError::BadRequest("agent thread limit must be an integer".into())
                    })?);
                }
                "cursor" if cursor.is_none() => cursor = Some(parse_uuid("cursor", value)?),
                "limit" | "cursor" => {
                    return Err(EdgeError::BadRequest(format!(
                        "duplicate agent thread {name}"
                    )))
                }
                other => {
                    return Err(EdgeError::BadRequest(format!(
                        "unknown agent thread query parameter `{other}`"
                    )))
                }
            }
        }
    }
    let limit = limit.unwrap_or(DEFAULT_PAGE_LIMIT as u32);
    if !(1..=MAX_PAGE_LIMIT as u32).contains(&limit) {
        return Err(EdgeError::BadRequest(
            "agent thread limit must be between 1 and 100".into(),
        ));
    }
    Ok((limit, cursor))
}

pub(crate) fn thread_param(ctx: &HandlerCtx<'_>) -> Result<Uuid, EdgeError> {
    let value = ctx
        .params
        .get("thread")
        .ok_or_else(|| EdgeError::BadRequest("route did not bind an agent thread id".into()))?;
    parse_uuid("id", value)
}

pub(crate) fn require_empty_query(ctx: &HandlerCtx<'_>) -> Result<(), EdgeError> {
    if ctx.request.query.is_empty() {
        Ok(())
    } else {
        Err(EdgeError::BadRequest(
            "agent thread request accepts no query parameters".into(),
        ))
    }
}

fn require_empty_body(ctx: &HandlerCtx<'_>) -> Result<(), EdgeError> {
    if ctx.request.body.is_empty() {
        Ok(())
    } else {
        Err(EdgeError::BadRequest(
            "agent thread read accepts no request body".into(),
        ))
    }
}

pub(crate) fn no_store(response: EdgeResponse) -> EdgeResponse {
    response.with_header("Cache-Control", "no-store")
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalKind, PrincipalStatus};
    use myelin_tenancy::TenantId;

    fn principal() -> Principal {
        let mut principal = Principal::stub(
            PrincipalId("p:alice".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        );
        principal.status = PrincipalStatus::Active;
        principal
    }

    #[test]
    fn creation_intent_has_a_fixed_visible_expiry() {
        let body: CreateAgentThreadBody = serde_json::from_value(json!({
            "name": "Investigate checkout race",
            "agent_id": "11111111-1111-4111-8111-111111111111",
            "retention_days": 3,
        }))
        .unwrap();
        let intent = ValidatedThreadIntent::new(&principal(), body, "retry".into()).unwrap();
        let proposal = intent.proposal("01J00000000000000000000000", None).unwrap();
        assert_eq!(proposal.expires_at - proposal.created_at, Duration::days(3));
    }

    #[test]
    fn repository_workspace_intent_has_a_stable_private_branch() {
        let parse = || {
            serde_json::from_value::<CreateAgentThreadBody>(json!({
                "name": "Investigate checkout race",
                "agent_id": "11111111-1111-4111-8111-111111111111",
                "workspace": { "repository": "platform", "base_ref": "main" },
            }))
            .unwrap()
        };
        let source = ResolvedWorkspaceRepository {
            repository: "platform".into(),
            repository_path: "/durable/git/platform.git".into(),
            clone_url: "https://myelin.example/acme/local/platform.git".into(),
            base_ref: "refs/heads/main".into(),
            base_oid: "0123456789abcdef0123456789abcdef01234567".into(),
        };
        let first = ValidatedThreadIntent::new(&principal(), parse(), "same-retry".into())
            .unwrap()
            .proposal("01J00000000000000000000000", Some(&source))
            .unwrap();
        let replay = ValidatedThreadIntent::new(&principal(), parse(), "same-retry".into())
            .unwrap()
            .proposal("01J00000000000000000000001", Some(&source))
            .unwrap();
        assert_eq!(first.git, replay.git);
        assert!(first.git.unwrap().branch.starts_with("myelin/thread-"));

        let intent =
            ValidatedThreadIntent::new(&principal(), parse(), "same-retry".into()).unwrap();
        let stored = DurableAgentThread {
            thread_id: "11111111-1111-4111-8111-111111111111".into(),
            owner_principal_id: "p:alice".into(),
            agent_id: "11111111-1111-4111-8111-111111111111".into(),
            conversation_id: "01J00000000000000000000000".into(),
            workspace_id: "22222222-2222-4222-8222-222222222222".into(),
            workspace_generation: 1,
            name: "Investigate checkout race".into(),
            project_id: None,
            git: replay.git,
            workspace_guidance: None,
            environment: None,
            retention_days: 3,
            state: AgentThreadState::Ready,
            storage_locator: Some("workspace:v1:receipt".into()),
            publication_completed_at: Some("2026-08-31T00:00:01Z".into()),
            review_ready_at: None,
            failure_reason: None,
            created_at: "2026-08-31T00:00:00Z".into(),
            expires_at: "2026-09-03T00:00:00Z".into(),
            updated_at: "2026-08-31T00:00:01Z".into(),
        };
        assert!(intent.matches(&stored));
    }

    #[test]
    fn repository_workspace_rejects_non_branch_refs() {
        let body: CreateAgentThreadBody = serde_json::from_value(json!({
            "name": "Invalid source",
            "agent_id": "11111111-1111-4111-8111-111111111111",
            "workspace": { "repository": "platform", "base_ref": "refs/tags/v1" },
        }))
        .unwrap();
        assert!(matches!(
            ValidatedThreadIntent::new(&principal(), body, "retry".into()),
            Err(EdgeError::BadRequest(_))
        ));
    }

    #[test]
    fn personal_instructions_can_configure_an_empty_workspace() {
        let body: CreateAgentThreadBody = serde_json::from_value(json!({
            "name": "Use my playbook",
            "agent_id": "11111111-1111-4111-8111-111111111111",
            "workspace": {
                "instructions": "# My workflow\n\nRun the focused journey first."
            },
        }))
        .unwrap();
        let intent = ValidatedThreadIntent::new(&principal(), body, "retry".into()).unwrap();
        let proposal = intent.proposal("01J00000000000000000000000", None).unwrap();
        assert!(proposal.git.is_none());

        for workspace in [
            json!({}),
            json!({ "instructions": "   \n" }),
            json!({ "instructions": "x\u{0000}y" }),
            json!({ "base_ref": "main", "instructions": "work carefully" }),
        ] {
            let body: CreateAgentThreadBody = serde_json::from_value(json!({
                "name": "Invalid personalization",
                "agent_id": "11111111-1111-4111-8111-111111111111",
                "workspace": workspace,
            }))
            .unwrap();
            assert!(ValidatedThreadIntent::new(&principal(), body, "retry".into()).is_err());
        }
    }

    #[test]
    fn declarative_workspace_files_are_retry_significant() {
        let parse = |contents: &str| {
            serde_json::from_value::<CreateAgentThreadWorkspaceBody>(json!({
                "files": [{
                    "path": ".myelin/context.md",
                    "contents": contents,
                    "strategy": "create"
                }]
            }))
            .unwrap()
        };
        let first = workspace_request_digest(&parse("first")).unwrap();
        let second = workspace_request_digest(&parse("second")).unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn personal_agents_guidance_preserves_repository_defaults_and_is_idempotent() {
        let repository = b"# Repository workflow\n\nRun cargo fmt.\n";
        let personal = "# My workflow\n\nExplain interface tradeoffs.";
        let digest = workspace_guidance_digest(personal.as_bytes());
        let personalized = personalized_agents_file(repository, personal, &digest).unwrap();
        let rendered = std::str::from_utf8(&personalized).unwrap();
        assert!(rendered.starts_with(std::str::from_utf8(repository).unwrap()));
        assert!(rendered.contains(personal));
        assert!(rendered.contains(&digest));
        assert_eq!(
            personalized_agents_file(&personalized, personal, &digest).unwrap(),
            personalized
        );
        assert!(personalized_agents_file(repository, "different", "blake3:deadbeef").is_ok());
        assert!(personalized_agents_file(&personalized, "different", "blake3:deadbeef").is_err());
    }

    #[test]
    fn recovery_config_requires_exact_bounded_tenant_partitions() {
        let interval = StdDuration::from_secs(5);
        let stale_after = StdDuration::from_secs(30);
        assert!(AgentThreadRecoveryConfig::new(
            vec![TenantId::from_token("acme")],
            Region::new("local"),
            interval,
            stale_after,
            100,
        )
        .is_ok());
        for tenants in [
            Vec::new(),
            vec![TenantId::from_token("")],
            vec![TenantId::from_token("acme"), TenantId::from_token("acme")],
            vec![TenantId::from_token("tenant@example.test")],
        ] {
            assert!(AgentThreadRecoveryConfig::new(
                tenants,
                Region::new("local"),
                interval,
                stale_after,
                100,
            )
            .is_err());
        }
        assert!(AgentThreadRecoveryConfig::new(
            vec![TenantId::from_token("acme")],
            Region::new("local"),
            StdDuration::ZERO,
            stale_after,
            100,
        )
        .is_err());
        assert!(AgentThreadRecoveryConfig::new(
            vec![TenantId::from_token("acme")],
            Region::new("local"),
            interval,
            StdDuration::from_secs(1),
            100,
        )
        .is_err());
        assert!(AgentThreadRecoveryConfig::new(
            vec![TenantId::from_token("acme")],
            Region::new("local"),
            interval,
            stale_after,
            MAX_RECOVERY_BATCH + 1,
        )
        .is_err());
    }

    #[test]
    fn malformed_or_surprising_creation_input_is_refused() {
        for body in [
            br#"{}"#.as_slice(),
            br#"{"name":" x","agent_id":"11111111-1111-4111-8111-111111111111"}"#,
            br#"{"name":"x","agent_id":"not-a-uuid"}"#,
            br#"{"name":"x","agent_id":"11111111-1111-4111-8111-111111111111","retention_days":31}"#,
            br#"{"name":"x","agent_id":"11111111-1111-4111-8111-111111111111","extra":true}"#,
        ] {
            let refused = parse_create_body(body)
                .and_then(|body| ValidatedThreadIntent::new(&principal(), body, "retry".into()))
                .is_err();
            assert!(refused, "unexpectedly accepted {}", String::from_utf8_lossy(body));
        }
    }

    #[test]
    fn thread_json_never_projects_the_internal_storage_locator() {
        let mut thread = DurableAgentThread {
            thread_id: "11111111-1111-4111-8111-111111111111".into(),
            owner_principal_id: "p:alice".into(),
            agent_id: "22222222-2222-4222-8222-222222222222".into(),
            conversation_id: "01J00000000000000000000000".into(),
            workspace_id: "33333333-3333-4333-8333-333333333333".into(),
            workspace_generation: 1,
            name: "Investigate checkout race".into(),
            project_id: None,
            git: None,
            workspace_guidance: None,
            environment: None,
            retention_days: 3,
            state: AgentThreadState::Ready,
            storage_locator: Some("workspace:v1:internal".into()),
            publication_completed_at: Some("2026-08-22T00:00:01Z".into()),
            review_ready_at: None,
            failure_reason: None,
            created_at: "2026-08-22T00:00:00Z".into(),
            expires_at: "2026-08-25T00:00:00Z".into(),
            updated_at: "2026-08-22T00:00:01Z".into(),
        };
        let rendered = thread_json("acme", &thread);
        assert!(!rendered.to_string().contains("internal"));
        assert_eq!(rendered["workspace"]["state"], "ready");
        assert_eq!(rendered["review_state"], "draft");

        thread.review_ready_at = Some("2026-08-22T00:00:02Z".into());
        let ready_for_review = thread_json("acme", &thread);
        assert_eq!(ready_for_review["review_state"], "ready");
        assert_eq!(ready_for_review["review_ready_at"], "2026-08-22T00:00:02Z");

        thread.publication_completed_at = None;
        let unpublished = thread_json("acme", &thread);
        assert_eq!(unpublished["workspace"]["state"], "provisioning");
        assert!(!workspace_is_available_at(
            &thread,
            Utc::now() + Duration::hours(1),
            Utc::now(),
        ));

        thread.state = AgentThreadState::Failed;
        thread.storage_locator = None;
        thread.publication_completed_at = None;
        thread.failure_reason = Some("workspace_provisioning_failed".into());
        let failed = thread_json("acme", &thread);
        assert_eq!(failed["workspace"]["state"], "failed");
        assert!(!failed.to_string().contains("provisioning_failed"));
    }

    #[test]
    fn message_authority_ends_at_the_workspace_deadline_even_before_reconciliation() {
        let thread = DurableAgentThread {
            thread_id: "11111111-1111-4111-8111-111111111111".into(),
            owner_principal_id: "p:alice".into(),
            agent_id: "22222222-2222-4222-8222-222222222222".into(),
            conversation_id: "01J00000000000000000000000".into(),
            workspace_id: "33333333-3333-4333-8333-333333333333".into(),
            workspace_generation: 1,
            name: "Investigate checkout race".into(),
            project_id: None,
            git: None,
            workspace_guidance: None,
            environment: None,
            retention_days: 3,
            state: AgentThreadState::Ready,
            storage_locator: Some("workspace:v1:internal".into()),
            publication_completed_at: Some("2026-08-22T00:00:01Z".into()),
            review_ready_at: None,
            failure_reason: None,
            created_at: "2026-08-22T00:00:00Z".into(),
            expires_at: "2026-08-25T00:00:00Z".into(),
            updated_at: "2026-08-22T00:00:01Z".into(),
        };
        let before = parse_stored_timestamp("2026-08-24T23:59:59Z").unwrap();
        let deadline = parse_stored_timestamp(&thread.expires_at).unwrap();
        assert!(workspace_is_available(&thread, before).unwrap());
        assert!(!workspace_is_available(&thread, deadline).unwrap());
    }
}
