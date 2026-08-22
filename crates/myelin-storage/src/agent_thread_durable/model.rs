use chrono::{DateTime, SecondsFormat, Utc};
use sqlx::types::Uuid;

pub const MIN_AGENT_THREAD_RETENTION_DAYS: i16 = 1;
pub const MAX_AGENT_THREAD_RETENTION_DAYS: i16 = 30;
pub const MAX_AGENT_THREAD_NAME_BYTES: usize = 80;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentThreadState {
    Provisioning,
    Ready,
    Expiring,
    Deleted,
    Failed,
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
    pub retention_days: i16,
    pub state: AgentThreadState,
    pub storage_locator: Option<String>,
    pub failure_reason: Option<String>,
    pub created_at: String,
    pub expires_at: String,
    pub updated_at: String,
}

impl DurableAgentThread {
    pub(crate) fn timestamp(value: DateTime<Utc>) -> String {
        value.to_rfc3339_opts(SecondsFormat::Secs, true)
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
pub enum ActivateAgentThreadOutcome {
    Activated(DurableAgentThread),
    AlreadyReady(DurableAgentThread),
    NotFound,
    Conflict,
}
