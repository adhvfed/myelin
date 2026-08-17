use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventHandler,
    EventId, EventType, HandleOutcome, Timestamp, Visibility,
};
use myelin_identity::{Literal, ObjectType, Principal, PrincipalId, PrincipalKind, RuntimeRef};
use myelin_query::{CmpOp, EventMatcher, Expr, Predicate};
use myelin_storage::{
    AgentTriggerEvaluationErrorCode, AgentTriggerFiringState, DurableAgentTriggerBinding,
    ReserveAgentTriggerFiringOutcome, ReservedAgentTriggerFiring,
};
use myelin_tenancy::{Region, TenantId};

use super::{
    GovernedTriggerConsumer, TriggerApprovalInbox, TriggerBindingStore, TriggerOwnerVisibility,
};

struct RecordingStore {
    bindings: Vec<DurableAgentTriggerBinding>,
    reservations: Mutex<Vec<(String, String)>>,
    diagnostics: Mutex<Vec<(String, String, AgentTriggerEvaluationErrorCode, String)>>,
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
        let state = self
            .bindings
            .iter()
            .find(|binding| binding.binding_id == binding_id)
            .filter(|binding| binding.require_human_approval)
            .map_or(AgentTriggerFiringState::Queued, |_| {
                AgentTriggerFiringState::AwaitingApproval
            });
        Ok(ReserveAgentTriggerFiringOutcome::Reserved(
            ReservedAgentTriggerFiring {
                binding_id: binding_id.into(),
                event_id: envelope.event_id.0.clone(),
                event_type: envelope.type_.0.clone(),
                state,
            },
        ))
    }

    fn record_evaluation_error(
        &self,
        _tenant: &str,
        binding_id: &str,
        event_id: &str,
        code: AgentTriggerEvaluationErrorCode,
        detail: &str,
        _event_recorded_at: DateTime<Utc>,
    ) -> Result<(), String> {
        self.diagnostics.lock().unwrap().push((
            binding_id.into(),
            event_id.into(),
            code,
            detail.into(),
        ));
        Ok(())
    }
}

struct Visible(bool);

struct IgnoringApprovalInbox;

impl TriggerApprovalInbox for IgnoringApprovalInbox {
    fn ensure_pending(
        &self,
        _binding: &DurableAgentTriggerBinding,
        _envelope: &EventEnvelope,
    ) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Default)]
struct RecordingApprovalInbox(Mutex<Vec<(String, String)>>);

impl TriggerApprovalInbox for RecordingApprovalInbox {
    fn ensure_pending(
        &self,
        binding: &DurableAgentTriggerBinding,
        envelope: &EventEnvelope,
    ) -> Result<(), String> {
        self.0
            .lock()
            .unwrap()
            .push((binding.binding_id.clone(), envelope.event_id.0.clone()));
        Ok(())
    }
}

fn ignoring_approvals() -> Arc<dyn TriggerApprovalInbox> {
    Arc::new(IgnoringApprovalInbox)
}

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
        budget_minor_units: 250_000,
        max_firings: 10,
        firings_used: 0,
        max_causal_depth: 4,
        require_no_personal_data: true,
        require_human_approval: false,
        state: "active".into(),
        created_at: "2026-08-10T08:00:00Z".into(),
        last_evaluation_error: None,
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
        diagnostics: Mutex::new(Vec::new()),
    });
    let consumer = GovernedTriggerConsumer::new(
        "acme",
        "fr-par",
        store.clone(),
        Arc::new(Visible(true)),
        ignoring_approvals(),
    );

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
            diagnostics: Mutex::new(Vec::new()),
        });
        let consumer = GovernedTriggerConsumer::new(
            "acme",
            "fr-par",
            store.clone(),
            Arc::new(Visible(visible)),
            ignoring_approvals(),
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

#[test]
fn an_agent_cannot_fire_its_own_automation_but_neighbouring_agents_still_can() {
    let self_binding = binding("refs/heads/main");
    let mut neighbour_binding = self_binding.clone();
    neighbour_binding.binding_id = "4bf441cb-33e1-49e1-91bc-bbb8b5a5217d".into();
    neighbour_binding.run_as_agent_id = "20000000-0000-4000-8000-000000000002".into();
    let store = Arc::new(RecordingStore {
        bindings: vec![self_binding.clone(), neighbour_binding.clone()],
        reservations: Mutex::new(Vec::new()),
        diagnostics: Mutex::new(Vec::new()),
    });
    let consumer = GovernedTriggerConsumer::new(
        "acme",
        "fr-par",
        store.clone(),
        Arc::new(Visible(true)),
        ignoring_approvals(),
    );
    let mut own_event = event("ci.run.failed", "refs/heads/main", "agent-authored-1");
    own_event.actor = Actor(Principal::stub(
        PrincipalId(format!("agent:{}", self_binding.run_as_agent_id)),
        PrincipalKind::Agent {
            runtime_ref: RuntimeRef("hosted:luna".into()),
            on_behalf_of: Some(PrincipalId("founder".into())),
        },
        TenantId("acme".into()),
    ));

    assert_eq!(
        consumer.handle(&own_event, &mut myelin_events::HandlerTx::none()),
        HandleOutcome::Done,
    );
    assert_eq!(
        *store.reservations.lock().unwrap(),
        vec![(neighbour_binding.binding_id, "agent-authored-1".into())],
        "the authoring agent spends nothing on its own event while an unrelated explicit automation remains live",
    );
    assert!(
        store.diagnostics.lock().unwrap().is_empty(),
        "a structural self-guard is a quiet non-match, not a broken automation",
    );
}

#[test]
fn one_imperfect_rule_cannot_starve_another_rule() {
    let mut brittle = binding("refs/heads/main");
    brittle.binding_id = "4bf441cb-33e1-49e1-91bc-bbb8b5a5217d".into();
    brittle.matcher = serde_json::to_value(
        EventMatcher::compile(
            ObjectType("run".into()),
            Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: Expr::Var("payload.field_this_event_does_not_carry".into()),
                rhs: Expr::Lit(Literal::Bool(true)),
            },
        )
        .unwrap(),
    )
    .unwrap();
    let healthy = binding("refs/heads/main");
    let store = Arc::new(RecordingStore {
        bindings: vec![brittle, healthy],
        reservations: Mutex::new(Vec::new()),
        diagnostics: Mutex::new(Vec::new()),
    });
    let consumer = GovernedTriggerConsumer::new(
        "acme",
        "fr-par",
        store.clone(),
        Arc::new(Visible(true)),
        ignoring_approvals(),
    );

    assert_eq!(
        consumer.handle(
            &event("ci.run.failed", "refs/heads/main", "red-main-2"),
            &mut myelin_events::HandlerTx::none(),
        ),
        HandleOutcome::Done
    );
    assert_eq!(
        *store.reservations.lock().unwrap(),
        vec![(
            "632cf5b2-207f-42f4-9f89-eedcd79f395f".into(),
            "red-main-2".into(),
        )],
        "a rule that cannot evaluate fails closed by itself while healthy rules still run"
    );
    assert_eq!(
        *store.diagnostics.lock().unwrap(),
        vec![(
            "4bf441cb-33e1-49e1-91bc-bbb8b5a5217d".into(),
            "red-main-2".into(),
            AgentTriggerEvaluationErrorCode::MissingContext,
            "predicate references unbound variable `payload.field_this_event_does_not_carry` (missing context)".into(),
        )],
        "the owner gets one exact explanation without starving a healthy rule"
    );
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

    fn record_evaluation_error(
        &self,
        _tenant: &str,
        _binding_id: &str,
        _event_id: &str,
        _code: AgentTriggerEvaluationErrorCode,
        _detail: &str,
        _event_recorded_at: DateTime<Utc>,
    ) -> Result<(), String> {
        Ok(())
    }
}

#[test]
fn exact_fanout_cap_is_allowed_and_one_more_is_refused_loudly() {
    for (count, expected) in [
        (super::MAX_EVENT_BINDINGS as usize, HandleOutcome::Done),
        (
            super::MAX_EVENT_BINDINGS as usize + 1,
            HandleOutcome::NonRetryable(myelin_events::Reason(
                "durable event trigger capacity invariant exceeds the 1000-binding safety bound"
                    .into(),
            )),
        ),
    ] {
        let consumer = GovernedTriggerConsumer::new(
            "acme",
            "fr-par",
            Arc::new(FanoutStore(count)),
            Arc::new(Visible(true)),
            ignoring_approvals(),
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

#[test]
fn a_parked_firing_becomes_one_actionable_approval_for_its_owner() {
    let mut approval_binding = binding("refs/heads/main");
    approval_binding.require_human_approval = true;
    let store = Arc::new(RecordingStore {
        bindings: vec![approval_binding],
        reservations: Mutex::new(Vec::new()),
        diagnostics: Mutex::new(Vec::new()),
    });
    let approvals = Arc::new(RecordingApprovalInbox::default());
    let consumer = GovernedTriggerConsumer::new(
        "acme",
        "fr-par",
        store,
        Arc::new(Visible(true)),
        approvals.clone(),
    );

    assert_eq!(
        consumer.handle(
            &event("ci.run.failed", "refs/heads/main", "red-main-needs-human"),
            &mut myelin_events::HandlerTx::none(),
        ),
        HandleOutcome::Done
    );
    assert_eq!(
        *approvals.0.lock().unwrap(),
        vec![(
            "632cf5b2-207f-42f4-9f89-eedcd79f395f".into(),
            "red-main-needs-human".into(),
        )],
        "the durable firing and the human's inbox point at the same exact decision"
    );
}
