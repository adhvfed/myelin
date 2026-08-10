use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventHandler,
    EventId, EventType, HandleOutcome, Timestamp, Visibility,
};
use myelin_identity::{Literal, ObjectType, Principal, PrincipalId, PrincipalKind};
use myelin_query::{CmpOp, EventMatcher, Expr, Predicate};
use myelin_storage::{
    AgentTriggerFiringState, DurableAgentTriggerBinding, ReserveAgentTriggerFiringOutcome,
    ReservedAgentTriggerFiring,
};
use myelin_tenancy::{Region, TenantId};

use super::{GovernedTriggerConsumer, TriggerBindingStore, TriggerOwnerVisibility};

struct RecordingStore {
    bindings: Vec<DurableAgentTriggerBinding>,
    reservations: Mutex<Vec<(String, String)>>,
}

impl TriggerBindingStore for RecordingStore {
    fn active_for_event(
        &self,
        _tenant: &str,
        event_type: &str,
        _limit: u32,
    ) -> Result<Vec<DurableAgentTriggerBinding>, String> {
        Ok(self
            .bindings
            .iter()
            .filter(|binding| binding.event_type == event_type)
            .cloned()
            .collect())
    }

    fn reserve_firing(
        &self,
        _tenant: &str,
        binding_id: &str,
        envelope: &EventEnvelope,
        _recorded_at: DateTime<Utc>,
    ) -> Result<ReserveAgentTriggerFiringOutcome, String> {
        self.reservations
            .lock()
            .unwrap()
            .push((binding_id.into(), envelope.event_id.0.clone()));
        Ok(ReserveAgentTriggerFiringOutcome::Reserved(
            ReservedAgentTriggerFiring {
                binding_id: binding_id.into(),
                event_id: envelope.event_id.0.clone(),
                event_type: envelope.type_.0.clone(),
                state: AgentTriggerFiringState::Queued,
            },
        ))
    }
}

struct Visible(bool);

impl TriggerOwnerVisibility for Visible {
    fn can_view(
        &self,
        _binding: &DurableAgentTriggerBinding,
        _envelope: &EventEnvelope,
    ) -> Result<bool, String> {
        Ok(self.0)
    }
}

fn binding(branch: &str) -> DurableAgentTriggerBinding {
    let matcher = EventMatcher::compile(
        ObjectType("run".into()),
        Predicate::And(vec![
            Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: Expr::Var("event.type".into()),
                rhs: Expr::Lit(Literal::Str("ci.run.failed".into())),
            },
            Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: Expr::Var("payload.source_ref".into()),
                rhs: Expr::Lit(Literal::Str(branch.into())),
            },
        ]),
    )
    .unwrap();
    DurableAgentTriggerBinding {
        binding_id: "632cf5b2-207f-42f4-9f89-eedcd79f395f".into(),
        owner_principal_id: "founder".into(),
        run_as_agent_id: "9b98b77a-6293-4a8c-945f-ae665ad29d6c".into(),
        client_nonce: "retry-1".into(),
        event_type: "ci.run.failed".into(),
        matcher: serde_json::to_value(matcher).unwrap(),
        task: "Find the failure and prepare the smallest safe fix.".into(),
        delegation_caveats: vec!["repo:core".into()],
        max_firings: 10,
        firings_used: 0,
        max_causal_depth: 4,
        require_no_personal_data: true,
        require_human_approval: false,
        state: "active".into(),
        created_at: "2026-08-10T08:00:00Z".into(),
    }
}

fn event(event_type: &str, branch: &str, event_id: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(event_id.into()),
        type_: EventType(event_type.into()),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("ci-controlplane".into()),
            PrincipalKind::Service,
            TenantId("acme".into()),
        )),
        subject: ArtifactRef("myelin://acme/ci/run/42".into()),
        aggregate: AggregateKey("run:42".into()),
        causation_id: None,
        correlation_id: CorrelationId("push-41".into()),
        caused_by: None,
        depth: 1,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-08-10T08:00:00Z".into()),
        recorded_at: Timestamp("2026-08-10T08:00:01Z".into()),
        payload: serde_json::json!({ "source_ref": branch }),
    }
}

#[test]
fn one_visible_red_mainline_event_reserves_one_exact_binding() {
    let store = Arc::new(RecordingStore {
        bindings: vec![binding("refs/heads/main")],
        reservations: Mutex::new(Vec::new()),
    });
    let consumer =
        GovernedTriggerConsumer::new("acme", "fr-par", store.clone(), Arc::new(Visible(true)));

    assert_eq!(
        consumer.handle(
            &event("ci.run.failed", "refs/heads/main", "red-main-1"),
            &mut myelin_events::HandlerTx::none(),
        ),
        HandleOutcome::Done
    );
    assert_eq!(
        *store.reservations.lock().unwrap(),
        vec![(
            "632cf5b2-207f-42f4-9f89-eedcd79f395f".into(),
            "red-main-1".into(),
        )],
        "the event names no arbitrary active agent: it reserves the exact durable binding"
    );
}

#[test]
fn green_feature_and_revoked_visibility_are_quiet() {
    for (event_type, branch, visible) in [
        ("ci.run.succeeded", "refs/heads/main", true),
        ("ci.run.failed", "refs/heads/feature/parser", true),
        ("ci.run.failed", "refs/heads/main", false),
    ] {
        let store = Arc::new(RecordingStore {
            bindings: vec![binding("refs/heads/main")],
            reservations: Mutex::new(Vec::new()),
        });
        let consumer = GovernedTriggerConsumer::new(
            "acme",
            "fr-par",
            store.clone(),
            Arc::new(Visible(visible)),
        );
        assert_eq!(
            consumer.handle(
                &event(event_type, branch, "quiet-1"),
                &mut myelin_events::HandlerTx::none(),
            ),
            HandleOutcome::Done
        );
        assert!(
            store.reservations.lock().unwrap().is_empty(),
            "non-matching or no-longer-visible work spends no trigger budget"
        );
    }
}

struct FanoutStore(usize);

impl TriggerBindingStore for FanoutStore {
    fn active_for_event(
        &self,
        _tenant: &str,
        _event_type: &str,
        limit: u32,
    ) -> Result<Vec<DurableAgentTriggerBinding>, String> {
        assert_eq!(limit, super::MAX_EVENT_BINDINGS + 1);
        Ok(vec![binding("refs/heads/main"); self.0])
    }

    fn reserve_firing(
        &self,
        _tenant: &str,
        _binding_id: &str,
        _envelope: &EventEnvelope,
        _recorded_at: DateTime<Utc>,
    ) -> Result<ReserveAgentTriggerFiringOutcome, String> {
        Ok(ReserveAgentTriggerFiringOutcome::BudgetExhausted)
    }
}

#[test]
fn exact_fanout_cap_is_allowed_and_one_more_is_refused_loudly() {
    for (count, expected) in [
        (super::MAX_EVENT_BINDINGS as usize, HandleOutcome::Done),
        (
            super::MAX_EVENT_BINDINGS as usize + 1,
            HandleOutcome::NonRetryable(myelin_events::Reason(
                "event trigger fanout exceeds the 1000-binding safety bound".into(),
            )),
        ),
    ] {
        let consumer = GovernedTriggerConsumer::new(
            "acme",
            "fr-par",
            Arc::new(FanoutStore(count)),
            Arc::new(Visible(true)),
        );
        assert_eq!(
            consumer.handle(
                &event("ci.run.failed", "refs/heads/main", "bounded-1"),
                &mut myelin_events::HandlerTx::none(),
            ),
            expected,
        );
    }
}
