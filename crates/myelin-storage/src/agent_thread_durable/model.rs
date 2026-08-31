use chrono::{DateTime, SecondsFormat, Utc};
use sqlx::types::Uuid;

pub const MIN_AGENT_THREAD_RETENTION_DAYS: i16 = 1;
pub const MAX_AGENT_THREAD_RETENTION_DAYS: i16 = 30;
pub const MAX_AGENT_THREAD_NAME_BYTES: usize = 80;
pub const MAX_AGENT_THREAD_REPOSITORY_BYTES: usize = 255;
pub const MAX_AGENT_THREAD_GIT_REF_BYTES: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentThreadGitBinding {
    pub repository: String,
    pub base_ref: String,
    pub base_oid: String,
    pub branch: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentThreadEnvironmentBinding {
    pub provider: String,
    pub config_path: String,
    pub content_digest: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentThreadActivation {
    pub workspace_id: Uuid,
    pub conversation_id: String,
    pub storage_locator: String,
    pub environment: Option<AgentThreadEnvironmentBinding>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentThreadState {
    Provisioning,
    Ready,
    Expiring,
    Deleted,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentThreadProvisioningFailure {
    ChatUnavailable,
    WorkspaceUnavailable,
    ActivationUnavailable,
}

impl AgentThreadProvisioningFailure {
    pub(crate) fn token(self) -> &'static str {
        match self {
            Self::ChatUnavailable => "chat_provisioning_failed",
            Self::WorkspaceUnavailable => "workspace_provisioning_failed",
            Self::ActivationUnavailable => "activation_failed",
        }
    }
}

impl AgentThreadState {
    pub fn token(self) -> &'static str {
        match self {
            Self::Provisioning => "provisioning",
            Self::Ready => "ready",
            Self::Expiring => "expiring",
            Self::Deleted => "deleted",
            Self::Failed => "failed",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "provisioning" => Ok(Self::Provisioning),
            "ready" => Ok(Self::Ready),
            "expiring" => Ok(Self::Expiring),
            "deleted" => Ok(Self::Deleted),
            "failed" => Ok(Self::Failed),
            _ => Err("agent thread has an invalid durable state".into()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewAgentThread {
    pub thread_id: Uuid,
    pub owner_principal_id: String,
    pub agent_id: Uuid,
    pub conversation_id: String,
    pub workspace_id: Uuid,
    pub name: String,
    pub project_id: Option<Uuid>,
    pub git: Option<AgentThreadGitBinding>,
    pub retention_days: i16,
    pub client_nonce: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableAgentThread {
    pub thread_id: String,
    pub owner_principal_id: String,
    pub agent_id: String,
    pub conversation_id: String,
    pub workspace_id: String,
    pub workspace_generation: u32,
    pub name: String,
    pub project_id: Option<String>,
    pub git: Option<AgentThreadGitBinding>,
    pub environment: Option<AgentThreadEnvironmentBinding>,
    pub retention_days: i16,
    pub state: AgentThreadState,
    pub storage_locator: Option<String>,
    pub publication_completed_at: Option<String>,
    pub review_ready_at: Option<String>,
    pub failure_reason: Option<String>,
    pub created_at: String,
    pub expires_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkAgentThreadReviewReadyOutcome {
    Marked(DurableAgentThread),
    AlreadyReady(DurableAgentThread),
    NotFound,
    ProjectRequired,
    Unavailable,
}

impl DurableAgentThread {
    pub(crate) fn timestamp(value: DateTime<Utc>) -> String {
        value.to_rfc3339_opts(SecondsFormat::Secs, true)
    }

    pub fn is_published(&self) -> bool {
        self.state == AgentThreadState::Ready && self.publication_completed_at.is_some()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CreateAgentThreadOutcome {
    Created(DurableAgentThread),
    Replayed(DurableAgentThread),
    Conflict,
    NameConflict,
    OwnerUnavailable,
    AgentUnavailable,
    AgentRuntimeUnsupported,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RetryAgentThreadOutcome {
    Retrying(DurableAgentThread),
    AlreadyReady(DurableAgentThread),
    NotFound,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActivateAgentThreadOutcome {
    Activated(DurableAgentThread),
    AlreadyReady(DurableAgentThread),
    NotFound,
    Conflict,
}
