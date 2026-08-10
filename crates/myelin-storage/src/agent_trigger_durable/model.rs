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
pub struct ClaimedAgentTriggerFiring {
    pub binding_id: String,
    pub event_id: String,
    pub event_type: String,
    pub event_envelope: serde_json::Value,
    pub owner_principal_id: String,
    pub run_as_agent_id: String,
    pub runtime_ref: String,
    pub task: String,
    pub delegation_caveats: Vec<String>,
    pub claim_owner: String,
    pub claim_until: String,
    pub claim_attempts: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentTriggerClaimRequest {
    pub runtime_ref: String,
    pub worker_id: String,
    pub now: DateTime<Utc>,
    pub claim_until: DateTime<Utc>,
}

impl AgentTriggerClaimRequest {
    pub const MAX_LEASE_SECONDS: u64 = 15 * 60;

    pub fn new(
        runtime_ref: impl Into<String>,
        worker_id: impl Into<String>,
        now: DateTime<Utc>,
        lease_seconds: u64,
    ) -> Result<Self, &'static str> {
        let runtime_ref = runtime_ref.into();
        let worker_id = worker_id.into();
        if runtime_ref.is_empty() || runtime_ref.len() > 255 || runtime_ref.trim() != runtime_ref {
            return Err("trigger claim runtime_ref must be a trimmed 1..=255 byte token");
        }
        if worker_id.is_empty() || worker_id.len() > 128 || worker_id.trim() != worker_id {
            return Err("trigger claim worker_id must be a trimmed 1..=128 byte token");
        }
        if !(1..=Self::MAX_LEASE_SECONDS).contains(&lease_seconds) {
            return Err("trigger claim lease is outside its bounded lifetime");
        }
        let seconds = i64::try_from(lease_seconds)
            .map_err(|_| "trigger claim lease is outside its bounded lifetime")?;
        let claim_until = now
            .timestamp()
            .checked_add(seconds)
            .and_then(|timestamp| {
                DateTime::<Utc>::from_timestamp(timestamp, now.timestamp_subsec_nanos())
            })
            .ok_or("trigger claim lease overflows the timestamp range")?;
        Ok(Self {
            runtime_ref,
            worker_id,
            now,
            claim_until,
        })
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firing_claim_leases_are_short_and_canonically_identified() {
        let now = Utc::now();
        let claim = AgentTriggerClaimRequest::new("hosted:luna", "worker-1", now, 30).unwrap();
        assert_eq!(claim.claim_until.timestamp() - now.timestamp(), 30);

        assert!(AgentTriggerClaimRequest::new(" external:mcp", "worker-1", now, 30).is_err());
        assert!(AgentTriggerClaimRequest::new("hosted:luna", "", now, 30).is_err());
        assert!(AgentTriggerClaimRequest::new(
            "hosted:luna",
            "worker-1",
            now,
            AgentTriggerClaimRequest::MAX_LEASE_SECONDS + 1,
        )
        .is_err());
    }
}
