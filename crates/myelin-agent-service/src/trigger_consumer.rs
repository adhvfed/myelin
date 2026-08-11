use std::sync::Arc;

use chrono::{DateTime, Utc};
use myelin_events::{Backoff, EventEnvelope, EventHandler, HandleOutcome, Reason, SubjectPattern};
use myelin_identity::SetExpr;
use myelin_query::{EventMatcher, RelMembership};
use myelin_storage::{
    DurableAgentTriggerBinding, ReserveAgentTriggerFiringOutcome,
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
    subjects: &'static [SubjectPattern],
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
        let subjects =
            Box::leak(vec![SubjectPattern(format!("myelin://{tenant}/"))].into_boxed_slice());
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
            let matcher: EventMatcher =
                serde_json::from_value(binding.matcher.clone()).map_err(|_| {
                    TriggerDeliveryError::Malformed(format!(
                        "trigger binding {} contains an invalid compiled matcher",
                        binding.binding_id
                    ))
                })?;
            if !self
                .visibility
                .can_view(&binding, event)
                .map_err(|_| TriggerDeliveryError::Unavailable)?
            {
                continue;
            }
            let no_relations = no_relation as fn(&RelMembership) -> bool;
            let matches = matcher
                .matches(event, &SetExpr::All, &no_relations)
                .unwrap_or(false);
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
}

impl EventHandler for GovernedTriggerConsumer {
    fn subjects(&self) -> &'static [SubjectPattern] {
        self.subjects
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

enum TriggerDeliveryError {
    Malformed(String),
    Unavailable,
}

#[cfg(feature = "integration")]
pub mod durable;

#[cfg(test)]
mod tests;
