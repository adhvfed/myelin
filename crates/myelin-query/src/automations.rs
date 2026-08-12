use crate::matcher::RelMembership;
use crate::{EventMatcher, PublishDraft, PublishKind, Severity, Signal, SignalState};
use myelin_events::{ArtifactRef, EventEnvelope, EventId};
use myelin_identity::{DelegationCaveats, PrincipalId, SetExpr};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AutomationId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WorkflowRef(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunAs(pub PrincipalId);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delegation(pub DelegationCaveats);

impl Delegation {
    pub fn none() -> Delegation {
        Delegation(DelegationCaveats(Vec::new()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budget {
    pub max_firings: u64,
    pub cost_units: u64,
}

impl Budget {
    pub fn unbounded_within(max_firings: u64) -> Budget {
        Budget {
            max_firings,
            cost_units: 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Gate {
    RequireHumanApproval,
    RequireNoPersonalData,
    MaxCausalDepth(u32),
}

impl Gate {
    fn passes_inline(&self, envelope: &EventEnvelope) -> bool {
        match self {
            Gate::RequireHumanApproval => true,
            Gate::RequireNoPersonalData => !envelope.contains_personal_data,
            Gate::MaxCausalDepth(max) => envelope.depth <= *max,
        }
    }

    fn is_approval(&self) -> bool {
        matches!(self, Gate::RequireHumanApproval)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    pub kind: ActionKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionKind {
    Emit {
        emit_type: String,
        subject: ArtifactRef,
    },
    Workflow {
        workflow_ref: WorkflowRef,
        input: serde_json::Value,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationRule {
    pub rule_id: AutomationId,
    pub matcher: EventMatcher,
    pub action: Action,
    pub run_as: RunAs,
    pub delegation: Delegation,
    pub budget: Budget,
    pub gates: Vec<Gate>,
}

#[allow(clippy::too_many_arguments)]
pub fn register_automation(
    rule_id: AutomationId,
    matcher: EventMatcher,
    action: Action,
    run_as: RunAs,
    delegation: Delegation,
    budget: Budget,
    gates: Vec<Gate>,
) -> AutomationRule {
    AutomationRule {
        rule_id,
        matcher,
        action,
        run_as,
        delegation,
        budget,
        gates,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DurableHandle(pub String);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutorError(pub String);

pub trait DurableExecutor {
    fn start(
        &self,
        workflow_ref: &WorkflowRef,
        input: &serde_json::Value,
        idem_key: &str,
    ) -> Result<DurableHandle, ExecutorError>;
}

#[derive(Debug, Default)]
pub struct InMemoryExecutor {
    started: std::cell::RefCell<std::collections::BTreeMap<String, StartedRun>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartedRun {
    pub workflow_ref: WorkflowRef,
    pub input: serde_json::Value,
    pub handle: DurableHandle,
}

impl InMemoryExecutor {
    pub fn new() -> InMemoryExecutor {
        InMemoryExecutor::default()
    }

    pub fn started_count(&self) -> usize {
        self.started.borrow().len()
    }

    pub fn run_for(&self, idem_key: &str) -> Option<StartedRun> {
        self.started.borrow().get(idem_key).cloned()
    }
}

impl DurableExecutor for InMemoryExecutor {
    fn start(
        &self,
        workflow_ref: &WorkflowRef,
        input: &serde_json::Value,
        idem_key: &str,
    ) -> Result<DurableHandle, ExecutorError> {
        let mut started = self.started.borrow_mut();
        if let Some(existing) = started.get(idem_key) {
            return Ok(existing.handle.clone());
        }
        let handle = DurableHandle(format!("wf:{idem_key}"));
        started.insert(
            idem_key.to_string(),
            StartedRun {
                workflow_ref: workflow_ref.clone(),
                input: input.clone(),
                handle: handle.clone(),
            },
        );
        Ok(handle)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    NoMatch {
        rule_id: AutomationId,
    },
    GateFailed {
        rule_id: AutomationId,
    },
    BudgetShed {
        rule_id: AutomationId,
    },
    AwaitingApproval {
        rule_id: AutomationId,
    },
    Emitted {
        rule_id: AutomationId,
        draft: PublishDraft,
    },
    WorkflowStarted {
        rule_id: AutomationId,
        handle: DurableHandle,
    },
    WorkflowStartFailed {
        rule_id: AutomationId,
        error: ExecutorError,
    },
    AlreadyFired {
        rule_id: AutomationId,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct BudgetState {
    firings: u64,
}

#[derive(Debug, Default)]
pub struct AutomationEngine {
    rules: Vec<AutomationRule>,
    fired: BTreeSet<(AutomationId, EventId)>,
    budgets: std::collections::BTreeMap<AutomationId, BudgetState>,
}

impl AutomationEngine {
    pub fn new() -> AutomationEngine {
        AutomationEngine::default()
    }

    pub fn add_rule(&mut self, rule: AutomationRule) -> &mut AutomationEngine {
        self.rules.push(rule);
        self
    }

    pub fn firings(&self, rule_id: &AutomationId) -> u64 {
        self.budgets.get(rule_id).map(|b| b.firings).unwrap_or(0)
    }

    pub fn has_fired(&self, rule_id: &AutomationId, event_id: &EventId) -> bool {
        self.fired.contains(&(rule_id.clone(), event_id.clone()))
    }

    pub fn ingest(
        &mut self,
        envelope: &EventEnvelope,
        visible: &SetExpr,
        member_oracle: &dyn Fn(&RelMembership) -> bool,
        executor: &dyn DurableExecutor,
    ) -> Vec<Outcome> {
        let rules = self.rules.clone();
        let mut outcomes = Vec::with_capacity(rules.len());
        for rule in &rules {
            outcomes.push(self.fire_one(rule, envelope, visible, member_oracle, executor));
        }
        outcomes
    }

    fn fire_one(
        &mut self,
        rule: &AutomationRule,
        envelope: &EventEnvelope,
        visible: &SetExpr,
        member_oracle: &dyn Fn(&RelMembership) -> bool,
        executor: &dyn DurableExecutor,
    ) -> Outcome {
        let matched = rule
            .matcher
            .matches(envelope, visible, member_oracle)
            .unwrap_or(false);
        if !matched {
            return Outcome::NoMatch {
                rule_id: rule.rule_id.clone(),
            };
        }

        let ledger_key = (rule.rule_id.clone(), envelope.event_id.clone());
        if self.fired.contains(&ledger_key) {
            return Outcome::AlreadyFired {
                rule_id: rule.rule_id.clone(),
            };
        }

        for gate in &rule.gates {
            if !gate.passes_inline(envelope) {
                return Outcome::GateFailed {
                    rule_id: rule.rule_id.clone(),
                };
            }
        }
        if rule.gates.iter().any(Gate::is_approval) {
            self.fired.insert(ledger_key);
            return Outcome::AwaitingApproval {
                rule_id: rule.rule_id.clone(),
            };
        }

        let state = self.budgets.entry(rule.rule_id.clone()).or_default();
        if state.firings >= rule.budget.max_firings {
            return Outcome::BudgetShed {
                rule_id: rule.rule_id.clone(),
            };
        }

        state.firings += 1;
        self.fired.insert(ledger_key);

        match &rule.action.kind {
            ActionKind::Emit { emit_type, subject } => Outcome::Emitted {
                rule_id: rule.rule_id.clone(),
                draft: self.emit_draft(rule, envelope, emit_type, subject),
            },
            ActionKind::Workflow {
                workflow_ref,
                input,
            } => {
                let idem_key = format!(
                    "{}:{}:{}",
                    rule.rule_id.0, rule.run_as.0 .0, envelope.event_id.0
                );
                match executor.start(workflow_ref, input, &idem_key) {
                    Ok(handle) => Outcome::WorkflowStarted {
                        rule_id: rule.rule_id.clone(),
                        handle,
                    },
                    Err(error) => Outcome::WorkflowStartFailed {
                        rule_id: rule.rule_id.clone(),
                        error,
                    },
                }
            }
        }
    }

    fn emit_draft(
        &self,
        rule: &AutomationRule,
        envelope: &EventEnvelope,
        emit_type: &str,
        subject: &ArtifactRef,
    ) -> PublishDraft {
        PublishDraft {
            subject: emit_type.to_string(),
            signal: Signal {
                rule_id: crate::RuleId(rule.rule_id.0.clone()),
                tenant: envelope.tenant.clone(),
                severity: Severity::Info,
                dedup_key: crate::DedupKey(format!("{}:{}", rule.rule_id.0, envelope.event_id.0)),
                subject: subject.clone(),
                count: 1,
                state: SignalState::Open,
                first_seen: envelope.recorded_at.0.clone(),
                last_seen: envelope.recorded_at.0.clone(),
            },
            kind: PublishKind::Opened,
        }
    }

    pub fn reset_budgets(&mut self) {
        self.budgets.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CmpOp, Expr, Predicate};
    use myelin_events::{
        Actor, AggregateKey, CorrelationId, DataRole, EventType, Timestamp, Visibility,
    };
    use myelin_identity::{Literal, ObjectType, Principal, PrincipalKind};
    use myelin_tenancy::{Region, TenantId};

    fn var(name: &str) -> Expr {
        Expr::Var(name.into())
    }
    fn str_(s: &str) -> Expr {
        Expr::Lit(Literal::Str(s.into()))
    }

    fn principal() -> Principal {
        Principal::stub(
            PrincipalId("svc-bot".into()),
            PrincipalKind::Human,
            TenantId("t1".into()),
        )
    }

    fn type_matcher(object_type: &str, type_: &str) -> EventMatcher {
        EventMatcher::compile(
            ObjectType(object_type.into()),
            Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var("event.type"),
                rhs: str_(type_),
            },
        )
        .unwrap()
    }

    fn envelope(type_: &str, id: &str, event_id: &str) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId(event_id.into()),
            type_: EventType(type_.into()),
            schema_ver: 1,
            tenant: TenantId("t1".into()),
            region: Region("fr-par".into()),
            actor: Actor(principal()),
            subject: ArtifactRef(format!("myelin://t1/issues/issue/{id}")),
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

    fn no_rel(_m: &RelMembership) -> bool {
        false
    }

    fn emit_rule(rule_id: &str, on_type: &str) -> AutomationRule {
        register_automation(
            AutomationId(rule_id.into()),
            type_matcher("issue", on_type),
            Action {
                kind: ActionKind::Emit {
                    emit_type: "issues.issue.labelled".into(),
                    subject: ArtifactRef("myelin://t1/issues/issue/PROJ-1".into()),
                },
            },
            RunAs(PrincipalId("svc-bot".into())),
            Delegation::none(),
            Budget::unbounded_within(100),
            vec![],
        )
    }

    fn workflow_rule(rule_id: &str, on_type: &str) -> AutomationRule {
        register_automation(
            AutomationId(rule_id.into()),
            type_matcher("issue", on_type),
            Action {
                kind: ActionKind::Workflow {
                    workflow_ref: WorkflowRef("escalate_incident".into()),
                    input: serde_json::json!({ "ref": "myelin://t1/issues/issue/PROJ-1" }),
                },
            },
            RunAs(PrincipalId("svc-bot".into())),
            Delegation::none(),
            Budget::unbounded_within(100),
            vec![],
        )
    }

    fn outcome_for<'a>(outs: &'a [Outcome], rule_id: &str) -> &'a Outcome {
        outs.iter()
            .find(|o| match o {
                Outcome::NoMatch { .. } => false,
                Outcome::GateFailed { rule_id: r }
                | Outcome::BudgetShed { rule_id: r }
                | Outcome::AwaitingApproval { rule_id: r }
                | Outcome::Emitted { rule_id: r, .. }
                | Outcome::WorkflowStarted { rule_id: r, .. }
                | Outcome::WorkflowStartFailed { rule_id: r, .. }
                | Outcome::AlreadyFired { rule_id: r } => r.0 == rule_id,
            })
            .expect("a deciding outcome for the rule")
    }

    #[test]
    fn matching_event_fires_automation_non_matching_does_not() {
        let mut engine = AutomationEngine::new();
        engine.add_rule(emit_rule("label_on_create", "issues.issue.created"));
        let exec = InMemoryExecutor::new();

        let matching = envelope("issues.issue.created", "PROJ-1", "evt-1");
        let outs = engine.ingest(&matching, &SetExpr::All, &no_rel, &exec);
        assert!(
            matches!(
                outcome_for(&outs, "label_on_create"),
                Outcome::Emitted { .. }
            ),
            "a matching event fires the automation"
        );

        let other = envelope("issues.issue.transitioned", "PROJ-2", "evt-2");
        let outs2 = engine.ingest(&other, &SetExpr::All, &no_rel, &exec);
        assert!(
            matches!(&outs2[0], Outcome::NoMatch { .. }),
            "a non-matching event does not fire the automation"
        );
    }

    #[test]
    fn workflow_action_delegates_to_durable_executor() {
        let mut engine = AutomationEngine::new();
        engine.add_rule(workflow_rule("escalate", "issues.issue.created"));
        let exec = InMemoryExecutor::new();

        let env = envelope("issues.issue.created", "PROJ-1", "evt-wf-1");
        let outs = engine.ingest(&env, &SetExpr::All, &no_rel, &exec);

        match outcome_for(&outs, "escalate") {
            Outcome::WorkflowStarted { handle, .. } => {
                assert_eq!(
                    handle,
                    &DurableHandle("wf:escalate:svc-bot:evt-wf-1".into())
                );
            }
            other => panic!("expected WorkflowStarted, got {other:?}"),
        }
        assert_eq!(exec.started_count(), 1, "exactly one durable run started");
        let run = exec
            .run_for("escalate:svc-bot:evt-wf-1")
            .expect("the run is recorded");
        assert_eq!(run.workflow_ref, WorkflowRef("escalate_incident".into()));
        assert_eq!(
            run.input,
            serde_json::json!({ "ref": "myelin://t1/issues/issue/PROJ-1" })
        );
    }

    #[test]
    fn fires_exactly_once_per_event_id_redelivery_is_a_noop() {
        let mut engine = AutomationEngine::new();
        engine.add_rule(workflow_rule("escalate", "issues.issue.created"));
        let exec = InMemoryExecutor::new();

        let env = envelope("issues.issue.created", "PROJ-1", "evt-dup");
        let first = engine.ingest(&env, &SetExpr::All, &no_rel, &exec);
        assert!(matches!(
            outcome_for(&first, "escalate"),
            Outcome::WorkflowStarted { .. }
        ));
        assert!(engine.has_fired(&AutomationId("escalate".into()), &EventId("evt-dup".into())));

        let second = engine.ingest(&env, &SetExpr::All, &no_rel, &exec);
        assert!(
            matches!(
                outcome_for(&second, "escalate"),
                Outcome::AlreadyFired { .. }
            ),
            "a redelivered event is a no-op (effectively-once on event_id)"
        );
        assert_eq!(
            exec.started_count(),
            1,
            "the workflow started exactly once across the redelivery"
        );
        assert_eq!(engine.firings(&AutomationId("escalate".into())), 1);
    }

    #[test]
    fn unviewable_subject_never_fires_the_automation() {
        let mut engine = AutomationEngine::new();
        engine.add_rule(emit_rule("label_on_create", "issues.issue.created"));
        let exec = InMemoryExecutor::new();
        let env = envelope("issues.issue.created", "PROJ-1", "evt-hidden");
        let outs = engine.ingest(&env, &SetExpr::None, &no_rel, &exec);
        assert!(
            matches!(&outs[0], Outcome::NoMatch { .. }),
            "an unviewable subject never fires (0-leak, the permission compose rides through)"
        );
    }

    #[test]
    fn run_as_identity_scopes_the_firing() {
        let mut engine = AutomationEngine::new();
        let mut bot_rule = workflow_rule("escalate", "issues.issue.created");
        bot_rule.run_as = RunAs(PrincipalId("svc-bot".into()));
        let mut ops_rule = workflow_rule("escalate_ops", "issues.issue.created");
        ops_rule.run_as = RunAs(PrincipalId("svc-ops".into()));
        engine.add_rule(bot_rule);
        engine.add_rule(ops_rule);
        let exec = InMemoryExecutor::new();

        let env = envelope("issues.issue.created", "PROJ-1", "evt-runas");
        let outs = engine.ingest(&env, &SetExpr::All, &no_rel, &exec);
        assert!(matches!(
            outcome_for(&outs, "escalate"),
            Outcome::WorkflowStarted { .. }
        ));
        assert!(matches!(
            outcome_for(&outs, "escalate_ops"),
            Outcome::WorkflowStarted { .. }
        ));
        assert_eq!(
            exec.started_count(),
            2,
            "distinct run_as identities → distinct durable runs"
        );
        assert!(exec.run_for("escalate:svc-bot:evt-runas").is_some());
        assert!(exec.run_for("escalate_ops:svc-ops:evt-runas").is_some());
    }

    #[test]
    fn budget_sheds_over_budget_firings() {
        let mut engine = AutomationEngine::new();
        let mut rule = emit_rule("label", "issues.issue.created");
        rule.budget = Budget {
            max_firings: 2,
            cost_units: 1,
        };
        engine.add_rule(rule);
        let exec = InMemoryExecutor::new();

        let mut emitted = 0;
        let mut shed = 0;
        for i in 0..3 {
            let env = envelope("issues.issue.created", "PROJ-1", &format!("evt-budget-{i}"));
            let outs = engine.ingest(&env, &SetExpr::All, &no_rel, &exec);
            match outcome_for(&outs, "label") {
                Outcome::Emitted { .. } => emitted += 1,
                Outcome::BudgetShed { .. } => shed += 1,
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(emitted, 2, "exactly max_firings firings ran");
        assert_eq!(shed, 1, "the over-budget firing was shed, not run");
        assert_eq!(engine.firings(&AutomationId("label".into())), 2);
    }

    #[test]
    fn gate_fail_closes_on_personal_data() {
        let mut engine = AutomationEngine::new();
        let mut rule = emit_rule("label", "issues.issue.created");
        rule.gates = vec![Gate::RequireNoPersonalData];
        engine.add_rule(rule);
        let exec = InMemoryExecutor::new();

        let mut env = envelope("issues.issue.created", "PROJ-1", "evt-pii");
        env.contains_personal_data = true;
        let outs = engine.ingest(&env, &SetExpr::All, &no_rel, &exec);
        assert!(
            matches!(outcome_for(&outs, "label"), Outcome::GateFailed { .. }),
            "a gate that does not hold suppresses the firing (fail-closed)"
        );

        let clean = envelope("issues.issue.created", "PROJ-2", "evt-clean");
        let outs2 = engine.ingest(&clean, &SetExpr::All, &no_rel, &exec);
        assert!(matches!(
            outcome_for(&outs2, "label"),
            Outcome::Emitted { .. }
        ));
    }

    #[test]
    fn human_approval_gate_routes_to_approval_lane() {
        let mut engine = AutomationEngine::new();
        let mut rule = workflow_rule("escalate", "issues.issue.created");
        rule.gates = vec![Gate::RequireHumanApproval];
        engine.add_rule(rule);
        let exec = InMemoryExecutor::new();

        let env = envelope("issues.issue.created", "PROJ-1", "evt-approve");
        let outs = engine.ingest(&env, &SetExpr::All, &no_rel, &exec);
        assert!(
            matches!(
                outcome_for(&outs, "escalate"),
                Outcome::AwaitingApproval { .. }
            ),
            "a human-approval gate holds the action for a human decision"
        );
        assert_eq!(
            exec.started_count(),
            0,
            "the action is held, not run inline"
        );
        let again = engine.ingest(&env, &SetExpr::All, &no_rel, &exec);
        assert!(matches!(
            outcome_for(&again, "escalate"),
            Outcome::AlreadyFired { .. }
        ));
    }

    #[test]
    fn causal_depth_gate_suppresses_deep_firing() {
        let mut engine = AutomationEngine::new();
        let mut rule = emit_rule("label", "issues.issue.created");
        rule.gates = vec![Gate::MaxCausalDepth(3)];
        engine.add_rule(rule);
        let exec = InMemoryExecutor::new();

        let shallow = envelope("issues.issue.created", "PROJ-1", "evt-shallow");
        assert!(matches!(
            outcome_for(
                &engine.ingest(&shallow, &SetExpr::All, &no_rel, &exec),
                "label"
            ),
            Outcome::Emitted { .. }
        ));

        let mut deep = envelope("issues.issue.created", "PROJ-2", "evt-deep");
        deep.depth = 7;
        assert!(
            matches!(
                outcome_for(
                    &engine.ingest(&deep, &SetExpr::All, &no_rel, &exec),
                    "label"
                ),
                Outcome::GateFailed { .. }
            ),
            "a firing past the causal-depth ceiling is suppressed (the self-trigger guard)"
        );
    }

    #[test]
    fn workflow_start_failure_is_surfaced() {
        struct FailingExec;
        impl DurableExecutor for FailingExec {
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
        engine.add_rule(workflow_rule("escalate", "issues.issue.created"));
        let env = envelope("issues.issue.created", "PROJ-1", "evt-fail");
        let outs = engine.ingest(&env, &SetExpr::All, &no_rel, &FailingExec);
        assert!(
            matches!(
                outcome_for(&outs, "escalate"),
                Outcome::WorkflowStartFailed { .. }
            ),
            "a start failure is surfaced, never swallowed"
        );
    }

    #[test]
    fn ingest_is_replay_deterministic() {
        let stream: Vec<EventEnvelope> = (0..5)
            .map(|i| envelope("issues.issue.created", "PROJ-1", &format!("evt-{i}")))
            .collect();
        let run = || {
            let mut e = AutomationEngine::new();
            e.add_rule(emit_rule("label", "issues.issue.created"));
            let exec = InMemoryExecutor::new();
            let mut all = Vec::new();
            for env in &stream {
                all.extend(e.ingest(env, &SetExpr::All, &no_rel, &exec));
            }
            all
        };
        assert_eq!(
            run(),
            run(),
            "the same stream → the same outcomes (deterministic)"
        );
    }

    #[test]
    fn automation_rule_round_trips_stably() {
        let rule = workflow_rule("escalate", "issues.issue.created");
        let json = serde_json::to_string(&rule).unwrap();
        let back: AutomationRule = serde_json::from_str(&json).unwrap();
        assert_eq!(rule, back);
    }

    #[test]
    fn executor_start_is_idempotent_on_idem_key() {
        let exec = InMemoryExecutor::new();
        let w = WorkflowRef("w".into());
        let i = serde_json::json!({});
        let h1 = exec.start(&w, &i, "k").unwrap();
        let h2 = exec.start(&w, &i, "k").unwrap();
        assert_eq!(h1, h2, "same idem_key → same handle (effectively-once)");
        assert_eq!(exec.started_count(), 1, "one run for one idem_key");
    }
}
