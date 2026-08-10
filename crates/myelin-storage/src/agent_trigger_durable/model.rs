use sqlx::types::chrono::{DateTime, Utc};
use sqlx::types::Uuid;

use crate::pg::PgError;

#[derive(Clone, Debug, PartialEq)]
pub struct NewAgentTriggerBinding {
    pub binding_id: Uuid,
    pub owner_principal_id: String,
    pub run_as_agent_id: Uuid,
    pub client_nonce: String,
    pub event_type: String,
    pub matcher: serde_json::Value,
    pub task: String,
    pub delegation_caveats: Vec<String>,
    pub max_firings: u64,
    pub max_causal_depth: u32,
    pub require_no_personal_data: bool,
    pub require_human_approval: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DurableAgentTriggerBinding {
    pub binding_id: String,
    pub owner_principal_id: String,
    pub run_as_agent_id: String,
    pub client_nonce: String,
    pub event_type: String,
    pub matcher: serde_json::Value,
    pub task: String,
    pub delegation_caveats: Vec<String>,
    pub max_firings: u64,
    pub firings_used: u64,
    pub max_causal_depth: u32,
    pub require_no_personal_data: bool,
    pub require_human_approval: bool,
    pub state: String,
    pub created_at: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CreateAgentTriggerBindingOutcome {
    Created(DurableAgentTriggerBinding),
    Replayed(DurableAgentTriggerBinding),
    Conflict,
    OwnerUnavailable,
    AgentUnavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentTriggerFiringState {
    Queued,
    AwaitingApproval,
    Claimed,
    Started,
    Terminal,
}

impl AgentTriggerFiringState {
    pub(crate) fn token(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::AwaitingApproval => "awaiting_approval",
            Self::Claimed => "claimed",
            Self::Started => "started",
            Self::Terminal => "terminal",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, PgError> {
        match value {
            "queued" => Ok(Self::Queued),
            "awaiting_approval" => Ok(Self::AwaitingApproval),
            "claimed" => Ok(Self::Claimed),
            "started" => Ok(Self::Started),
            "terminal" => Ok(Self::Terminal),
            _ => Err(PgError::Query(
                "agent trigger firing has an invalid durable state".into(),
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReservedAgentTriggerFiring {
    pub binding_id: String,
    pub event_id: String,
    pub event_type: String,
    pub state: AgentTriggerFiringState,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DurableAgentTriggerFiring {
    pub binding_id: String,
    pub event_id: String,
    pub event_type: String,
    pub state: AgentTriggerFiringState,
    pub run_id: Option<String>,
    pub created_at: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ReserveAgentTriggerFiringOutcome {
    Reserved(ReservedAgentTriggerFiring),
    AlreadyReserved(ReservedAgentTriggerFiring),
    BindingUnavailable,
    EventTypeMismatch,
    GateRefused,
    BudgetExhausted,
}
