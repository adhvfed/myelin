//! # The CDC pair for contract 3.2 — Automations (`register_automation`) (EB-19 / P-139)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 3.2
//! (`register_automation(AutomationRule{ matcher, action, run_as, delegation, budget, gates })` —
//! the **stateless per-event reflex**; may invoke a durable workflow). Owning architecture:
//! `event-bus.md` §1.2 (the four primitives — Automation = the stateless per-event reflex, NOT
//! the per-person Trigger), §3.5 (the `automation_rule` store — `action.kind = workflow` invokes
//! `myelin-flow`), §5.4 (the registration surface). ADR-19 / ADR-09.
//!
//! ## The seam this pair pins
//! Row 3.2 is the seam between:
//! - the **PROVIDER** — the Automation engine ([`myelin_query::AutomationEngine`], the stateless
//!   per-event reflex over the matcher): a project admin registers a rule; the engine matches each
//!   event (permission-aware), and on a match runs the action under `run_as + delegation` within
//!   `budget + gates`. Its promise: a matching event fires the automation EXACTLY ONCE per
//!   delivery (idempotent on `event_id`); a non-matching one does not; `action.kind = workflow`
//!   DELEGATES to `myelin-flow` (it is never reinvented).
//! - the **CONSUMER** — the Bus dispatch tier (EB-23) + the `myelin-flow` durable executor: the
//!   dispatch tier reads the firing [`Outcome`] (emit a draft / a started durable handle / a
//!   suppressed firing) and records it; the durable executor (the CONSUMED 9.1 seam) receives the
//!   `start` call. Their promise: an `Emit` outcome carries the outbox draft the dispatch tier
//!   co-commits; a `Workflow` outcome means the durable run was actually started through the 9.1
//!   `DurableExecutor::start` seam (the consumer side of 9.1/9.2).
//!
//! The pair asserts both sides agree: the registration shape (`matcher/action/run_as/delegation/
//! budget/gates`), the fire-exactly-once-per-event_id property, and the workflow delegation
//! through the durable-executor seam.

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
    Timestamp, Visibility,
};
use myelin_identity::{
    DelegationCaveats, Literal, ObjectType, Principal, PrincipalId, PrincipalKind, SetExpr,
};
use myelin_query::{
    register_automation, Action, ActionKind, AutomationEngine, AutomationId, Budget, CmpOp,
    Delegation, DurableExecutor, DurableHandle, EventMatcher, ExecutorError, Expr, Gate,
    InMemoryExecutor, Outcome, Predicate, RunAs, WorkflowRef,
};
use myelin_tenancy::{Region, TenantId};

fn type_matcher(type_: &str) -> EventMatcher {
    EventMatcher::compile(
        ObjectType("issue".into()),
        Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var("event.type".into()),
            rhs: Expr::Lit(Literal::Str(type_.into())),
        },
    )
    .unwrap()
}

fn envelope(type_: &str, id: &str, event_id: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(event_id.into()),
        type_: EventType(type_.into()),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("svc-bot".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )),
        subject: ArtifactRef(format!("myelin://acme/issues/issue/{id}")),
        aggregate: AggregateKey(format!("issue:{id}")),
        causation_id: None,
        correlation_id: CorrelationId("root".into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:00Z".into()),
        payload: serde_json::json!({}),
    }
}

fn see_all(_m: &myelin_query::RelMembership) -> bool {
    false
}

fn first_deciding<'a>(outs: &'a [Outcome], rule: &str) -> &'a Outcome {
    outs.iter()
        .find(|o| match o {
            Outcome::NoMatch { .. } => false,
            Outcome::GateFailed { rule_id: r }
            | Outcome::BudgetShed { rule_id: r }
            | Outcome::AwaitingApproval { rule_id: r }
            | Outcome::Emitted { rule_id: r, .. }
            | Outcome::WorkflowStarted { rule_id: r, .. }
            | Outcome::WorkflowStartFailed { rule_id: r, .. }
            | Outcome::AlreadyFired { rule_id: r } => r.0 == rule,
        })
        .expect("a deciding outcome")
}

/// **PROVIDER side of 3.2** — a `register_automation` registration + the Automation engine that
/// fires. An `Emit`-action rule labelling an issue on creation.
fn provider_emit_engine() -> AutomationEngine {
    let mut engine = AutomationEngine::new();
    engine.add_rule(register_automation(
        AutomationId("label_on_create".into()),
        type_matcher("issues.issue.created"),
        Action {
            kind: ActionKind::Emit {
                emit_type: "issues.issue.labelled".into(),
                subject: ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
            },
        },
        RunAs(PrincipalId("svc-bot".into())),
        Delegation(DelegationCaveats(vec!["scope:issues.write".into()])),
        Budget {
            max_firings: 100,
            cost_units: 1,
        },
        vec![],
    ));
    engine
}

/// The 3.2 pair, EMIT leg: the PROVIDER fires on a matching event and yields the outbox draft; the
/// CONSUMER (dispatch tier) reads exactly that draft (the derived event-type + subject) to
/// co-commit. A non-matching event yields no firing.
#[test]
fn cdc_3_2_provider_fires_on_match_consumer_reads_emit_draft() {
    let mut engine = provider_emit_engine();
    let exec = InMemoryExecutor::new();

    // PROVIDER: a matching issue.created fires the rule.
    let matched = envelope("issues.issue.created", "PROJ-1", "evt-1");
    let outs = engine.ingest(&matched, &SetExpr::All, &see_all, &exec);

    // CONSUMER (dispatch tier) reads the Emit draft to co-commit into the outbox.
    match first_deciding(&outs, "label_on_create") {
        Outcome::Emitted { draft, .. } => {
            assert_eq!(draft.subject, "issues.issue.labelled");
            assert_eq!(
                draft.signal.subject,
                ArtifactRef("myelin://acme/issues/issue/PROJ-1".into())
            );
        }
        other => panic!("expected Emitted, got {other:?}"),
    }

    // PROVIDER: a non-matching event does NOT fire.
    let unmatched = envelope("issues.issue.transitioned", "PROJ-2", "evt-2");
    let outs2 = engine.ingest(&unmatched, &SetExpr::All, &see_all, &exec);
    assert!(matches!(&outs2[0], Outcome::NoMatch { .. }));
}

/// The 3.2 pair, FIRE-EXACTLY-ONCE leg: a redelivered event fires the automation exactly once
/// (idempotent on `event_id`, the EB-06 dedup discipline). The CONSUMER reads `AlreadyFired` on
/// the redelivery and the action runs zero more times.
#[test]
fn cdc_3_2_fires_exactly_once_per_event_id() {
    let mut engine = provider_emit_engine();
    let exec = InMemoryExecutor::new();
    let env = envelope("issues.issue.created", "PROJ-1", "evt-dup");

    let first = engine.ingest(&env, &SetExpr::All, &see_all, &exec);
    assert!(matches!(
        first_deciding(&first, "label_on_create"),
        Outcome::Emitted { .. }
    ));

    let second = engine.ingest(&env, &SetExpr::All, &see_all, &exec);
    assert!(
        matches!(
            first_deciding(&second, "label_on_create"),
            Outcome::AlreadyFired { .. }
        ),
        "a redelivered event fires exactly once (effectively-once on event_id)"
    );
}

/// The 3.2 pair, WORKFLOW-DELEGATION leg (this is ALSO the **consumer side of 9.1/9.2**): the
/// PROVIDER's `action.kind = workflow` DELEGATES to the `myelin-flow` `DurableExecutor::start`
/// seam (contract 9.1); the CONSUMER (the durable executor) receives the `start` and returns a
/// durable handle. The engine never reinvents the durable loop.
#[test]
fn cdc_3_2_workflow_action_delegates_to_durable_executor_9_1() {
    let mut engine = AutomationEngine::new();
    engine.add_rule(register_automation(
        AutomationId("escalate".into()),
        type_matcher("issues.issue.created"),
        Action {
            kind: ActionKind::Workflow {
                workflow_ref: WorkflowRef("escalate_incident".into()),
                input: serde_json::json!({ "ref": "myelin://acme/issues/issue/PROJ-1" }),
            },
        },
        RunAs(PrincipalId("svc-bot".into())),
        Delegation::none(),
        Budget {
            max_firings: 10,
            cost_units: 1,
        },
        vec![],
    ));

    // The CONSUMER side of 9.1: the durable executor seam receives the start. We use the in-memory
    // executor (the deterministic floor; the REAL myelin-flow engine is the named floor P-203).
    let exec = InMemoryExecutor::new();
    let env = envelope("issues.issue.created", "PROJ-1", "evt-wf");
    let outs = engine.ingest(&env, &SetExpr::All, &see_all, &exec);

    // PROVIDER promise: a Workflow action started a durable run (delegated).
    match first_deciding(&outs, "escalate") {
        Outcome::WorkflowStarted { handle, .. } => {
            assert_eq!(handle, &DurableHandle("wf:escalate:svc-bot:evt-wf".into()));
        }
        other => panic!("expected WorkflowStarted, got {other:?}"),
    }
    // CONSUMER (9.1) promise: the executor was invoked exactly once with the rule's workflow_ref +
    // references-not-payloads input (the workflow was INVOKED through the seam, not reinvented).
    assert_eq!(exec.started_count(), 1);
    let run = exec.run_for("escalate:svc-bot:evt-wf").expect("recorded");
    assert_eq!(run.workflow_ref, WorkflowRef("escalate_incident".into()));
    assert_eq!(
        run.input,
        serde_json::json!({ "ref": "myelin://acme/issues/issue/PROJ-1" })
    );
}

/// The 3.2 pair, CONSUMER-of-9.1 NEGATIVE leg: a `start` failure on the durable-executor seam is
/// SURFACED to the dispatch tier (never a silent no-op), so the firing can be retried/alerted.
#[test]
fn cdc_3_2_workflow_start_failure_is_surfaced_to_consumer() {
    struct Failing;
    impl DurableExecutor for Failing {
        fn start(
            &self,
            _w: &WorkflowRef,
            _i: &serde_json::Value,
            _k: &str,
        ) -> Result<DurableHandle, ExecutorError> {
            Err(ExecutorError("myelin-flow unreachable".into()))
        }
    }
    let mut engine = AutomationEngine::new();
    engine.add_rule(register_automation(
        AutomationId("escalate".into()),
        type_matcher("issues.issue.created"),
        Action {
            kind: ActionKind::Workflow {
                workflow_ref: WorkflowRef("escalate_incident".into()),
                input: serde_json::json!({}),
            },
        },
        RunAs(PrincipalId("svc-bot".into())),
        Delegation::none(),
        Budget {
            max_firings: 10,
            cost_units: 1,
        },
        vec![],
    ));
    let env = envelope("issues.issue.created", "PROJ-1", "evt-fail");
    let outs = engine.ingest(&env, &SetExpr::All, &see_all, &Failing);
    assert!(matches!(
        first_deciding(&outs, "escalate"),
        Outcome::WorkflowStartFailed { .. }
    ));
}

/// The 3.2 pair, REGISTRATION-SHAPE leg: the frozen `AutomationRule{ matcher, action, run_as,
/// delegation, budget, gates }` round-trips byte-stably (the durable `automation_rule` row the
/// CONSUMER reads), and the `matcher` field is the byte-identical `QueryAst` (no drift, 13.3).
#[test]
fn cdc_3_2_registration_shape_round_trips_stably() {
    let rule = register_automation(
        AutomationId("escalate".into()),
        type_matcher("issues.issue.created"),
        Action {
            kind: ActionKind::Workflow {
                workflow_ref: WorkflowRef("escalate_incident".into()),
                input: serde_json::json!({ "ref": "myelin://acme/issues/issue/PROJ-1" }),
            },
        },
        RunAs(PrincipalId("svc-bot".into())),
        Delegation(DelegationCaveats(vec!["scope:issues.write".into()])),
        Budget {
            max_firings: 10,
            cost_units: 2,
        },
        vec![Gate::RequireNoPersonalData, Gate::MaxCausalDepth(5)],
    );
    let json = serde_json::to_string(&rule).unwrap();
    let back = serde_json::from_str(&json).unwrap();
    assert_eq!(rule, back);

    // The matcher field is the byte-identical QueryAst the saved-view/Search/Signal consumers read.
    let v = serde_json::to_value(&rule).unwrap();
    let matcher_predicate = &v["matcher"]["predicate"];
    let bare = serde_json::to_value(type_matcher("issues.issue.created").predicate()).unwrap();
    assert_eq!(
        matcher_predicate, &bare,
        "no QueryAst drift in the matcher field"
    );
}
