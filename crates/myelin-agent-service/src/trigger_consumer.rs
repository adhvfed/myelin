use std::sync::Arc;

use chrono::{DateTime, Utc};
use myelin_events::{Backoff, EventEnvelope, EventHandler, HandleOutcome, Reason, SubjectPattern};
use myelin_identity::SetExpr;
use myelin_query::{EvalError, EventMatcher, RelMembership};
use myelin_storage::{
    AgentTriggerEvaluationErrorCode, DurableAgentTriggerBinding, ReserveAgentTriggerFiringOutcome,
    MAX_ACTIVE_AGENT_TRIGGERS_PER_EVENT,
};

pub const TRIGGER_CONSUMER_NAME: &str = "agent-governed-trigger";
pub const MAX_EVENT_BINDINGS: u32 = MAX_ACTIVE_AGENT_TRIGGERS_PER_EVENT;

pub trait TriggerBindingStore: Send + Sync {
    fn active_for_event(
        &self,
        tenant: &str,
        event_type: &str,
        limit: u32,
    ) -> Result<Vec<DurableAgentTriggerBinding>, String>;

    fn reserve_firing(
        &self,
        tenant: &str,
        binding_id: &str,
        envelope: &EventEnvelope,
        recorded_at: DateTime<Utc>,
    ) -> Result<ReserveAgentTriggerFiringOutcome, String>;

    fn record_evaluation_error(
        &self,
        tenant: &str,
        binding_id: &str,
        event_id: &str,
        code: AgentTriggerEvaluationErrorCode,
        detail: &str,
        event_recorded_at: DateTime<Utc>,
    ) -> Result<(), String>;
}

pub trait TriggerOwnerVisibility: Send + Sync {
    fn can_view(
        &self,
        binding: &DurableAgentTriggerBinding,
        envelope: &EventEnvelope,
    ) -> Result<bool, String>;
}

pub trait TriggerApprovalInbox: Send + Sync {
    fn ensure_pending(
        &self,
        binding: &DurableAgentTriggerBinding,
        envelope: &EventEnvelope,
    ) -> Result<(), String>;
}

pub struct GovernedTriggerConsumer {
    tenant: String,
    region: String,
    subjects: Vec<SubjectPattern>,
    store: Arc<dyn TriggerBindingStore>,
    visibility: Arc<dyn TriggerOwnerVisibility>,
    approvals: Arc<dyn TriggerApprovalInbox>,
}

impl GovernedTriggerConsumer {
    pub fn new(
        tenant: impl Into<String>,
        region: impl Into<String>,
        store: Arc<dyn TriggerBindingStore>,
        visibility: Arc<dyn TriggerOwnerVisibility>,
        approvals: Arc<dyn TriggerApprovalInbox>,
    ) -> Self {
        let tenant = tenant.into();
        let subjects = vec![SubjectPattern(format!("myelin://{tenant}/"))];
        Self {
            tenant,
            region: region.into(),
            subjects,
            store,
            visibility,
            approvals,
        }
    }

    fn evaluate(&self, event: &EventEnvelope) -> Result<(), TriggerDeliveryError> {
        if event.tenant.0 != self.tenant || event.region.0 != self.region {
            return Err(TriggerDeliveryError::Malformed(
                "event is outside the consumer's exact tenant and region binding".into(),
            ));
        }
        let bindings = self
            .store
            .active_for_event(
                &self.tenant,
                &event.type_.0,
                MAX_EVENT_BINDINGS.saturating_add(1),
            )
            .map_err(|_| TriggerDeliveryError::Unavailable)?;
        if bindings.len() > MAX_EVENT_BINDINGS as usize {
            return Err(TriggerDeliveryError::Malformed(format!(
                "durable event trigger capacity invariant exceeds the {MAX_EVENT_BINDINGS}-binding safety bound"
            )));
        }
        let recorded_at = DateTime::parse_from_rfc3339(&event.recorded_at.0)
            .map_err(|_| {
                TriggerDeliveryError::Malformed(
                    "event recorded_at is not a canonical RFC 3339 timestamp".into(),
                )
            })?
            .with_timezone(&Utc);

        for binding in bindings {
            if !self
                .visibility
                .can_view(&binding, event)
                .map_err(|_| TriggerDeliveryError::Unavailable)?
            {
                continue;
            }
            let matcher: EventMatcher = match serde_json::from_value(binding.matcher.clone()) {
                Ok(matcher) => matcher,
                Err(_) => {
                    self.record_evaluation_error(
                        &binding,
                        event,
                        recorded_at,
                        AgentTriggerEvaluationErrorCode::InvalidMatcher,
                        "stored matcher could not be decoded; recreate the automation",
                    )?;
                    continue;
                }
            };
            let no_relations = no_relation as fn(&RelMembership) -> bool;
            let matches = match matcher.matches(event, &SetExpr::All, &no_relations) {
                Ok(matches) => matches,
                Err(error) => {
                    let (code, detail) = evaluation_diagnostic(&error);
                    self.record_evaluation_error(&binding, event, recorded_at, code, &detail)?;
                    continue;
                }
            };
            if !matches {
                continue;
            }
            let reservation = self
                .store
                .reserve_firing(&self.tenant, &binding.binding_id, event, recorded_at)
                .map_err(|_| TriggerDeliveryError::Unavailable)?;
            let awaits_approval = match &reservation {
                ReserveAgentTriggerFiringOutcome::Reserved(firing)
                | ReserveAgentTriggerFiringOutcome::AlreadyReserved(firing) => {
                    firing.state == myelin_storage::AgentTriggerFiringState::AwaitingApproval
                }
                _ => false,
            };
            if awaits_approval {
                self.approvals
                    .ensure_pending(&binding, event)
                    .map_err(|_| TriggerDeliveryError::Unavailable)?;
            }
        }
        Ok(())
    }

    fn record_evaluation_error(
        &self,
        binding: &DurableAgentTriggerBinding,
        event: &EventEnvelope,
        recorded_at: DateTime<Utc>,
        code: AgentTriggerEvaluationErrorCode,
        detail: &str,
    ) -> Result<(), TriggerDeliveryError> {
        self.store
            .record_evaluation_error(
                &self.tenant,
                &binding.binding_id,
                &event.event_id.0,
                code,
                &bounded_evaluation_detail(detail),
                recorded_at,
            )
            .map_err(|_| TriggerDeliveryError::Unavailable)
    }
}

impl EventHandler for GovernedTriggerConsumer {
    fn subjects(&self) -> &[SubjectPattern] {
        &self.subjects
    }

    fn handle(
        &self,
        event: &EventEnvelope,
        _tx: &mut myelin_events::HandlerTx<'_>,
    ) -> HandleOutcome {
        match self.evaluate(event) {
            Ok(()) => HandleOutcome::Done,
            Err(TriggerDeliveryError::Malformed(reason)) => {
                HandleOutcome::NonRetryable(Reason(reason))
            }
            Err(TriggerDeliveryError::Unavailable) => HandleOutcome::Retry(Backoff { seconds: 2 }),
        }
    }
}

fn no_relation(_: &RelMembership) -> bool {
    false
}

fn evaluation_diagnostic(error: &EvalError) -> (AgentTriggerEvaluationErrorCode, String) {
    let code = match error {
        EvalError::MissingContext { .. } => AgentTriggerEvaluationErrorCode::MissingContext,
        EvalError::TypeError => AgentTriggerEvaluationErrorCode::TypeError,
        EvalError::CostExceeded => AgentTriggerEvaluationErrorCode::CostExceeded,
        EvalError::NotCompiled => AgentTriggerEvaluationErrorCode::NotCompiled,
    };
    (code, error.to_string())
}

fn bounded_evaluation_detail(detail: &str) -> String {
    const MAX_BYTES: usize = 1024;
    if detail.len() <= MAX_BYTES {
        return detail.to_string();
    }
    let mut end = MAX_BYTES;
    while !detail.is_char_boundary(end) {
        end -= 1;
    }
    detail[..end].to_string()
}

enum TriggerDeliveryError {
    Malformed(String),
    Unavailable,
}

#[cfg(feature = "integration")]
pub mod durable;

#[cfg(test)]
mod tests;
