use myelin_identity::ObjectType;
use myelin_query::{EvalContext, EvalError, EventMatcher, Predicate, QueryAst};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateCategory {
    Unstarted,
    Started,
    Completed,
    Cancelled,
}

impl StateCategory {
    pub fn wire_token(self) -> &'static str {
        match self {
            StateCategory::Unstarted => "unstarted",
            StateCategory::Started => "started",
            StateCategory::Completed => "completed",
            StateCategory::Cancelled => "cancelled",
        }
    }

    pub fn all() -> [StateCategory; 4] {
        [
            StateCategory::Unstarted,
            StateCategory::Started,
            StateCategory::Completed,
            StateCategory::Cancelled,
        ]
    }

    pub fn parse(token: &str) -> Result<StateCategory, WorkflowError> {
        StateCategory::all()
            .into_iter()
            .find(|c| c.wire_token() == token)
            .ok_or_else(|| WorkflowError::UnknownCategory {
                token: token.to_string(),
            })
    }

    pub fn is_open(self) -> bool {
        matches!(self, StateCategory::Unstarted | StateCategory::Started)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowState {
    pub name: String,
    pub category: StateCategory,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkflowTransition {
    pub from: String,
    pub to: String,
    pub guards: Vec<WorkflowGuard>,
    pub required_fields: Vec<String>,
    pub post_actions: Vec<PostAction>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkflowGuard {
    pub label: String,
    pub predicate: QueryAst,
}

impl WorkflowGuard {
    pub fn compiled(
        label: impl Into<String>,
        predicate: Predicate,
    ) -> Result<WorkflowGuard, myelin_query::PredicateError> {
        Ok(WorkflowGuard {
            label: label.into(),
            predicate: QueryAst::compiled(predicate)?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PostAction {
    Assign {
        assignee: String,
    },
    SetField {
        field_id: String,
        value: serde_json::Value,
    },
    Link {
        relation: String,
        target_ref: String,
    },
    ArmTrigger {
        trigger: String,
        matcher: EventMatcher,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Workflow {
    pub states: Vec<WorkflowState>,
    pub transitions: Vec<WorkflowTransition>,
}

impl Workflow {
    pub fn category_of(&self, state: &str) -> Result<StateCategory, WorkflowError> {
        self.states
            .iter()
            .find(|s| s.name == state)
            .map(|s| s.category)
            .ok_or_else(|| WorkflowError::UnknownState {
                state: state.to_string(),
            })
    }

    pub fn plan_transition(
        &self,
        from: &str,
        target: &str,
        ctx: &IssueContext,
    ) -> Result<TransitionPlan, TransitionBlocked> {
        let t = self
            .transitions
            .iter()
            .find(|t| t.from == from && t.to == target)
            .ok_or_else(|| TransitionBlocked::NoSuchTransition {
                from: from.to_string(),
                to: target.to_string(),
            })?;

        let to_category =
            self.category_of(target)
                .map_err(|_| TransitionBlocked::NoSuchTransition {
                    from: from.to_string(),
                    to: target.to_string(),
                })?;

        for guard in &t.guards {
            match guard.predicate.eval(&ctx.attrs) {
                Ok(true) => {}
                Ok(false) => {
                    return Err(TransitionBlocked::GuardFailed {
                        reason: assemble_reason(&guard.label, from, target),
                    });
                }
                Err(e) => {
                    return Err(TransitionBlocked::GuardFailed {
                        reason: assemble_unevaluable_reason(&guard.label, from, target, &e),
                    });
                }
            }
        }

        for f in &t.required_fields {
            if !ctx.has_field(f) {
                return Err(TransitionBlocked::MissingRequiredField { field: f.clone() });
            }
        }

        Ok(TransitionPlan {
            from: from.to_string(),
            to: target.to_string(),
            to_category,
            post_actions: t.post_actions.clone(),
        })
    }

    pub fn from_body(body: &serde_json::Value) -> Result<Workflow, WorkflowError> {
        let wf: Workflow =
            serde_json::from_value(body.clone()).map_err(|e| WorkflowError::Malformed {
                reason: e.to_string(),
            })?;
        wf.validate()?;
        Ok(wf)
    }

    pub fn to_body(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("a Workflow always serializes")
    }

    pub fn validate(&self) -> Result<(), WorkflowError> {
        for t in &self.transitions {
            self.category_of(&t.from)?;
            self.category_of(&t.to)?;
        }
        let mut seen = std::collections::BTreeSet::new();
        for t in &self.transitions {
            if !seen.insert((t.from.as_str(), t.to.as_str())) {
                return Err(WorkflowError::DuplicateTransition {
                    from: t.from.clone(),
                    to: t.to.clone(),
                });
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct IssueContext {
    attrs: EvalContext,
    present_fields: std::collections::BTreeSet<String>,
}

impl IssueContext {
    pub fn new() -> IssueContext {
        IssueContext::default()
    }

    pub fn bind(
        mut self,
        name: impl Into<String>,
        value: myelin_identity::Literal,
    ) -> IssueContext {
        let name = name.into();
        self.present_fields.insert(name.clone());
        self.attrs = std::mem::take(&mut self.attrs).bind(name, value);
        self
    }

    pub fn mark_present(mut self, field_id: impl Into<String>) -> IssueContext {
        self.present_fields.insert(field_id.into());
        self
    }

    pub fn attrs(&self) -> &EvalContext {
        &self.attrs
    }

    pub fn has_field(&self, field_id: &str) -> bool {
        self.present_fields.contains(field_id)
    }
}

pub struct GuardVar;

impl GuardVar {
    pub const BLOCKED_BY_OPEN_COUNT: &'static str = "blocked_by_open_count";

    pub const LINKED_PR_CHECK_STATUS: &'static str = "linked_pr_check_status";

    pub const LINKED_PR_TRUST_TIER: &'static str = "linked_pr_trust_tier";
}

#[derive(Clone, Debug, PartialEq)]
pub struct TransitionPlan {
    pub from: String,
    pub to: String,
    pub to_category: StateCategory,
    pub post_actions: Vec<PostAction>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransitionBlocked {
    NoSuchTransition { from: String, to: String },
    GuardFailed { reason: String },
    MissingRequiredField { field: String },
}

impl TransitionBlocked {
    pub fn reason(&self) -> String {
        match self {
            TransitionBlocked::NoSuchTransition { from, to } => {
                format!("no transition from `{from}` to `{to}` in this workflow")
            }
            TransitionBlocked::GuardFailed { reason } => reason.clone(),
            TransitionBlocked::MissingRequiredField { field } => {
                format!("the field `{field}` is required before this transition")
            }
        }
    }
}

impl std::fmt::Display for TransitionBlocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.reason())
    }
}

impl std::error::Error for TransitionBlocked {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkflowError {
    Malformed { reason: String },
    UnknownCategory { token: String },
    UnknownState { state: String },
    DuplicateTransition { from: String, to: String },
}

impl std::fmt::Display for WorkflowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkflowError::Malformed { reason } => {
                write!(f, "malformed workflow scheme body: {reason}")
            }
            WorkflowError::UnknownCategory { token } => write!(
                f,
                "unknown state category `{token}` (the fixed set is unstarted/started/completed/cancelled)"
            ),
            WorkflowError::UnknownState { state } => {
                write!(f, "transition references undeclared state `{state}`")
            }
            WorkflowError::DuplicateTransition { from, to } => {
                write!(f, "duplicate transition `{from}` → `{to}`")
            }
        }
    }
}

impl std::error::Error for WorkflowError {}

fn assemble_reason(label: &str, from: &str, to: &str) -> String {
    format!("cannot transition `{from}` → `{to}`: {label}")
}

fn assemble_unevaluable_reason(label: &str, from: &str, to: &str, err: &EvalError) -> String {
    format!("cannot transition `{from}` → `{to}`: {label} (guard could not be evaluated: {err})")
}

pub fn arm_trigger_body(
    ctx_now_seconds: i64,
    trigger: &str,
    matcher: &EventMatcher,
) -> ArmedTrigger {
    ArmedTrigger {
        trigger: trigger.to_string(),
        armed_at_seconds: ctx_now_seconds,
        matcher: matcher.clone(),
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ArmedTrigger {
    pub trigger: String,
    pub armed_at_seconds: i64,
    pub matcher: EventMatcher,
}

pub fn blocked_by_guard() -> WorkflowGuard {
    use myelin_identity::Literal;
    use myelin_query::{CmpOp, Expr};
    WorkflowGuard::compiled(
        "this issue is blocked by an open issue",
        Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var(GuardVar::BLOCKED_BY_OPEN_COUNT.into()),
            rhs: Expr::Lit(Literal::Int(0)),
        },
    )
    .expect("the blocked_by guard is a single bounded comparison (within the cost bound)")
}

pub fn linked_pr_ci_green_guard() -> WorkflowGuard {
    use myelin_identity::Literal;
    use myelin_query::{CmpOp, Expr};
    WorkflowGuard::compiled(
        "the linked PR's CI is not green",
        Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var(GuardVar::LINKED_PR_CHECK_STATUS.into()),
            rhs: Expr::Lit(Literal::Str("success".into())),
        },
    )
    .expect("the CI-status guard is a single bounded comparison (within the cost bound)")
}

pub fn example_arm_trigger(trigger: impl Into<String>) -> PostAction {
    let matcher = EventMatcher::new(ObjectType("issue".into()), QueryAst::raw("state == 'Done'"));
    PostAction::ArmTrigger {
        trigger: trigger.into(),
        matcher,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::Literal;
    use myelin_query::{CmpOp, Expr};

    fn simple_workflow() -> Workflow {
        Workflow {
            states: vec![
                WorkflowState {
                    name: "Todo".into(),
                    category: StateCategory::Unstarted,
                },
                WorkflowState {
                    name: "In Progress".into(),
                    category: StateCategory::Started,
                },
                WorkflowState {
                    name: "Done".into(),
                    category: StateCategory::Completed,
                },
                WorkflowState {
                    name: "Cancelled".into(),
                    category: StateCategory::Cancelled,
                },
            ],
            transitions: vec![
                WorkflowTransition {
                    from: "Todo".into(),
                    to: "In Progress".into(),
                    guards: vec![],
                    required_fields: vec![],
                    post_actions: vec![],
                },
                WorkflowTransition {
                    from: "In Progress".into(),
                    to: "Done".into(),
                    guards: vec![blocked_by_guard()],
                    required_fields: vec![],
                    post_actions: vec![],
                },
                WorkflowTransition {
                    from: "Todo".into(),
                    to: "Cancelled".into(),
                    guards: vec![],
                    required_fields: vec![],
                    post_actions: vec![],
                },
            ],
        }
    }

    #[test]
    fn the_state_category_set_is_the_fixed_four() {
        let tokens: Vec<&str> = StateCategory::all()
            .iter()
            .map(|c| c.wire_token())
            .collect();
        assert_eq!(
            tokens,
            vec!["unstarted", "started", "completed", "cancelled"],
            "the fixed four categories (the one mandatory invariant)"
        );
        assert_eq!(
            StateCategory::parse("in_review"),
            Err(WorkflowError::UnknownCategory {
                token: "in_review".into()
            }),
            "a fifth category is rejected (the fixed invariant)"
        );
        assert!(StateCategory::Unstarted.is_open());
        assert!(StateCategory::Started.is_open());
        assert!(!StateCategory::Completed.is_open());
        assert!(!StateCategory::Cancelled.is_open());
    }

    #[test]
    fn every_state_maps_to_a_fixed_category() {
        let wf = simple_workflow();
        assert_eq!(wf.category_of("Todo").unwrap(), StateCategory::Unstarted);
        assert_eq!(
            wf.category_of("In Progress").unwrap(),
            StateCategory::Started
        );
        assert_eq!(wf.category_of("Done").unwrap(), StateCategory::Completed);
        assert_eq!(
            wf.category_of("Cancelled").unwrap(),
            StateCategory::Cancelled
        );
        assert_eq!(
            wf.category_of("Nope"),
            Err(WorkflowError::UnknownState {
                state: "Nope".into()
            })
        );
    }

    #[test]
    fn a_permitted_transition_returns_the_fixed_category() {
        let wf = simple_workflow();
        let plan = wf
            .plan_transition("Todo", "In Progress", &IssueContext::new())
            .expect("the unguarded transition is permitted");
        assert_eq!(
            plan.to_category,
            StateCategory::Started,
            "the FIXED category"
        );
        assert_eq!(plan.from, "Todo");
        assert_eq!(plan.to, "In Progress");
    }

    #[test]
    fn an_undeclared_transition_is_blocked() {
        let wf = simple_workflow();
        let blocked = wf
            .plan_transition("Todo", "Done", &IssueContext::new())
            .expect_err("Todo → Done is not a declared edge");
        assert_eq!(
            blocked,
            TransitionBlocked::NoSuchTransition {
                from: "Todo".into(),
                to: "Done".into()
            }
        );
    }

    #[test]
    fn cannot_close_while_blocked_by_an_open_issue() {
        let wf = simple_workflow();

        let ctx_blocked =
            IssueContext::new().bind(GuardVar::BLOCKED_BY_OPEN_COUNT, Literal::Int(1));
        let blocked = wf
            .plan_transition("In Progress", "Done", &ctx_blocked)
            .expect_err("a transition with an open blocker is blocked");
        match &blocked {
            TransitionBlocked::GuardFailed { reason } => {
                assert!(
                    reason.contains("blocked by an open issue"),
                    "the reason names the guard: {reason}"
                );
                assert!(
                    reason.contains("In Progress") && reason.contains("Done"),
                    "the reason names the from→to context: {reason}"
                );
            }
            other => panic!("expected GuardFailed, got {other:?}"),
        }
        assert!(!blocked.reason().is_empty());

        let ctx_clear = IssueContext::new().bind(GuardVar::BLOCKED_BY_OPEN_COUNT, Literal::Int(0));
        let plan = wf
            .plan_transition("In Progress", "Done", &ctx_clear)
            .expect("with no open blocker the transition is permitted");
        assert_eq!(plan.to_category, StateCategory::Completed);
    }

    #[test]
    fn an_unbound_guard_blocks_fail_closed() {
        let wf = simple_workflow();
        let blocked = wf
            .plan_transition("In Progress", "Done", &IssueContext::new())
            .expect_err("an un-evaluable guard fails closed (blocks)");
        match &blocked {
            TransitionBlocked::GuardFailed { reason } => {
                assert!(
                    reason.contains("could not be evaluated"),
                    "the reason names the un-evaluable cause: {reason}"
                );
            }
            other => panic!("expected GuardFailed (fail-closed), got {other:?}"),
        }
    }

    #[test]
    fn a_missing_required_field_blocks_the_transition() {
        let wf = Workflow {
            states: vec![
                WorkflowState {
                    name: "Open".into(),
                    category: StateCategory::Started,
                },
                WorkflowState {
                    name: "Resolved".into(),
                    category: StateCategory::Completed,
                },
            ],
            transitions: vec![WorkflowTransition {
                from: "Open".into(),
                to: "Resolved".into(),
                guards: vec![],
                required_fields: vec!["resolution".into()],
                post_actions: vec![],
            }],
        };
        let blocked = wf
            .plan_transition("Open", "Resolved", &IssueContext::new())
            .expect_err("a missing required field blocks");
        assert_eq!(
            blocked,
            TransitionBlocked::MissingRequiredField {
                field: "resolution".into()
            }
        );
        assert!(blocked.reason().contains("resolution"));
        let ctx = IssueContext::new().mark_present("resolution");
        let plan = wf
            .plan_transition("Open", "Resolved", &ctx)
            .expect("the required field is present");
        assert_eq!(plan.to_category, StateCategory::Completed);
    }

    #[test]
    fn post_actions_fire_on_a_permitted_transition() {
        let wf = Workflow {
            states: vec![
                WorkflowState {
                    name: "Todo".into(),
                    category: StateCategory::Unstarted,
                },
                WorkflowState {
                    name: "Doing".into(),
                    category: StateCategory::Started,
                },
            ],
            transitions: vec![WorkflowTransition {
                from: "Todo".into(),
                to: "Doing".into(),
                guards: vec![],
                required_fields: vec![],
                post_actions: vec![
                    PostAction::Assign {
                        assignee: "alice@acme.noreply".into(),
                    },
                    PostAction::SetField {
                        field_id: "started_at".into(),
                        value: serde_json::json!(1000),
                    },
                    example_arm_trigger("sla_timer"),
                ],
            }],
        };
        let plan = wf
            .plan_transition("Todo", "Doing", &IssueContext::new())
            .expect("permitted");
        assert_eq!(plan.post_actions.len(), 3, "all three post-actions staged");
        assert!(matches!(
            &plan.post_actions[0],
            PostAction::Assign { assignee } if assignee == "alice@acme.noreply"
        ));
        assert!(
            matches!(&plan.post_actions[2], PostAction::ArmTrigger { trigger, .. } if trigger == "sla_timer")
        );
    }

    #[test]
    fn the_workflow_body_round_trips() {
        let wf = simple_workflow();
        let body = wf.to_body();
        assert!(body.to_string().contains("\"unstarted\""));
        assert!(body.to_string().contains("\"Todo\""));
        let back = Workflow::from_body(&body).expect("the body parses back");
        assert_eq!(back, wf, "the workflow body round-trips byte-identically");
    }

    #[test]
    fn an_unknown_category_in_the_body_is_rejected() {
        let body = serde_json::json!({
            "states": [{"name": "Review", "category": "in_review"}],
            "transitions": []
        });
        let err = Workflow::from_body(&body).expect_err("a fifth category is rejected");
        assert!(matches!(err, WorkflowError::Malformed { .. }));
    }

    #[test]
    fn a_transition_to_an_undeclared_state_is_rejected() {
        let wf = Workflow {
            states: vec![WorkflowState {
                name: "Todo".into(),
                category: StateCategory::Unstarted,
            }],
            transitions: vec![WorkflowTransition {
                from: "Todo".into(),
                to: "Ghost".into(),
                guards: vec![],
                required_fields: vec![],
                post_actions: vec![],
            }],
        };
        assert_eq!(
            wf.validate(),
            Err(WorkflowError::UnknownState {
                state: "Ghost".into()
            })
        );
    }

    #[test]
    fn a_duplicate_transition_edge_is_rejected() {
        let wf = Workflow {
            states: vec![
                WorkflowState {
                    name: "A".into(),
                    category: StateCategory::Unstarted,
                },
                WorkflowState {
                    name: "B".into(),
                    category: StateCategory::Started,
                },
            ],
            transitions: vec![
                WorkflowTransition {
                    from: "A".into(),
                    to: "B".into(),
                    guards: vec![],
                    required_fields: vec![],
                    post_actions: vec![],
                },
                WorkflowTransition {
                    from: "A".into(),
                    to: "B".into(),
                    guards: vec![],
                    required_fields: vec![],
                    post_actions: vec![],
                },
            ],
        };
        assert_eq!(
            wf.validate(),
            Err(WorkflowError::DuplicateTransition {
                from: "A".into(),
                to: "B".into()
            })
        );
    }

    #[test]
    fn the_guard_is_the_frozen_query_ast() {
        let guard = WorkflowGuard::compiled(
            "high-severity issues need no open blocker",
            Predicate::And(vec![
                Predicate::Cmp {
                    op: CmpOp::Ge,
                    lhs: Expr::Var("severity".into()),
                    rhs: Expr::Lit(Literal::Int(3)),
                },
                Predicate::Cmp {
                    op: CmpOp::Eq,
                    lhs: Expr::Var(GuardVar::BLOCKED_BY_OPEN_COUNT.into()),
                    rhs: Expr::Lit(Literal::Int(0)),
                },
            ]),
        )
        .expect("a bounded compound guard builds");
        let ctx = EvalContext::new()
            .bind("severity", Literal::Int(5))
            .bind(GuardVar::BLOCKED_BY_OPEN_COUNT, Literal::Int(0));
        assert_eq!(guard.predicate.eval(&ctx), Ok(true));
        let action = example_arm_trigger("t");
        assert!(matches!(action, PostAction::ArmTrigger { .. }));
    }

    #[test]
    fn the_arm_trigger_body_is_determinism_clean() {
        let matcher = EventMatcher::new(ObjectType("issue".into()), QueryAst::raw("x == 1"));
        let armed = arm_trigger_body(1_700_000_000, "sla_timer", &matcher);
        assert_eq!(armed.armed_at_seconds, 1_700_000_000);
        assert_eq!(armed.trigger, "sla_timer");
        let replay = arm_trigger_body(1_700_000_000, "sla_timer", &matcher);
        assert_eq!(armed, replay, "the workflow body replays deterministically");
    }

    #[test]
    fn the_ci_status_guard_shape_fails_closed_until_iss_p27() {
        let guard = linked_pr_ci_green_guard();
        assert_eq!(
            guard.predicate.eval(&EvalContext::new()),
            Err(EvalError::MissingContext {
                name: GuardVar::LINKED_PR_CHECK_STATUS.into()
            }),
            "until ISS-P27 binds the X-1 CheckStatus, the guard fails closed (never a silent allow)"
        );
        let green = EvalContext::new().bind(
            GuardVar::LINKED_PR_CHECK_STATUS,
            Literal::Str("success".into()),
        );
        assert_eq!(guard.predicate.eval(&green), Ok(true));
        let red = EvalContext::new().bind(
            GuardVar::LINKED_PR_CHECK_STATUS,
            Literal::Str("failure".into()),
        );
        assert_eq!(guard.predicate.eval(&red), Ok(false));
    }

    #[test]
    fn the_issue_context_exposes_its_bound_attrs() {
        let ctx = IssueContext::new().bind("severity", Literal::Int(7));
        let ast = WorkflowGuard::compiled(
            "sev",
            Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: Expr::Var("severity".into()),
                rhs: Expr::Lit(Literal::Int(7)),
            },
        )
        .unwrap();
        assert_eq!(
            ast.predicate.eval(ctx.attrs()),
            Ok(true),
            "the attrs() accessor returns the bound guard context"
        );
    }

    #[test]
    fn the_error_displays_carry_their_reasons() {
        let blocked = TransitionBlocked::MissingRequiredField {
            field: "resolution".into(),
        };
        let shown = format!("{blocked}");
        assert!(
            shown.contains("resolution") && shown == blocked.reason(),
            "Display == reason(), naming the field: {shown}"
        );

        let nost = TransitionBlocked::NoSuchTransition {
            from: "A".into(),
            to: "B".into(),
        };
        assert!(format!("{nost}").contains("no transition from `A` to `B`"));

        let cfg = WorkflowError::UnknownCategory {
            token: "weird".into(),
        };
        assert!(
            format!("{cfg}").contains("weird") && format!("{cfg}").contains("unstarted/started"),
            "the config error names the bad token + the fixed set"
        );
        let dup = WorkflowError::DuplicateTransition {
            from: "X".into(),
            to: "Y".into(),
        };
        assert!(format!("{dup}").contains("duplicate transition `X` → `Y`"));
        let unk = WorkflowError::UnknownState { state: "Z".into() };
        assert!(format!("{unk}").contains("undeclared state `Z`"));
        let mal = WorkflowError::Malformed {
            reason: "bad".into(),
        };
        assert!(format!("{mal}").contains("bad"));
    }
}
