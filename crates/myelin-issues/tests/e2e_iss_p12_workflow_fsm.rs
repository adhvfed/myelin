use myelin_identity::Literal;
use myelin_issues::{
    blocked_by_guard, GuardVar, IssueContext, PostAction, StateCategory, TransitionBlocked,
    Workflow, WorkflowGuard, WorkflowState, WorkflowTransition,
};
use myelin_query::{CmpOp, Expr, Predicate};

#[derive(Clone, Debug, PartialEq)]
struct ToyIssue {
    state: String,
    category: StateCategory,
    mutations: u64,
}

impl ToyIssue {
    fn new(state: &str, category: StateCategory) -> Self {
        ToyIssue {
            state: state.into(),
            category,
            mutations: 0,
        }
    }

    fn transition(
        &mut self,
        wf: &Workflow,
        target: &str,
        ctx: &IssueContext,
    ) -> Result<(), TransitionBlocked> {
        let plan = wf.plan_transition(&self.state, target, ctx)?;
        self.state = plan.to.clone();
        self.category = plan.to_category;
        self.mutations += 1;
        Ok(())
    }
}

fn engineering_workflow() -> Workflow {
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
                post_actions: vec![PostAction::Assign {
                    assignee: "alice@acme.noreply".into(),
                }],
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
fn chained_transition_stamps_the_fixed_category_at_each_step() {
    let wf = engineering_workflow();
    let mut issue = ToyIssue::new("Todo", StateCategory::Unstarted);

    let plan_start = wf
        .plan_transition("Todo", "In Progress", &IssueContext::new())
        .expect("the start transition is permitted");
    assert_eq!(plan_start.to_category, StateCategory::Started);
    assert_eq!(
        plan_start.post_actions.len(),
        1,
        "the assign post-action staged"
    );
    assert!(matches!(
        &plan_start.post_actions[0],
        PostAction::Assign { assignee } if assignee == "alice@acme.noreply"
    ));
    issue
        .transition(&wf, "In Progress", &IssueContext::new())
        .expect("permitted");
    assert_eq!(
        issue.category,
        StateCategory::Started,
        "FIXED category stamped"
    );

    let ctx_clear = IssueContext::new().bind(GuardVar::BLOCKED_BY_OPEN_COUNT, Literal::Int(0));
    issue
        .transition(&wf, "Done", &ctx_clear)
        .expect("with no open blocker the close is permitted");
    assert_eq!(
        issue.category,
        StateCategory::Completed,
        "the close stamps the FIXED completed category"
    );
    assert_eq!(
        issue.mutations, 2,
        "two permitted mutations (start + close)"
    );
}

#[test]
fn drill_iss_d12_cannot_close_while_blocked_by_open_issue() {
    let wf = engineering_workflow();
    let mut issue = ToyIssue::new("In Progress", StateCategory::Started);

    let ctx_blocked = IssueContext::new().bind(GuardVar::BLOCKED_BY_OPEN_COUNT, Literal::Int(1));
    let blocked = issue
        .transition(&wf, "Done", &ctx_blocked)
        .expect_err("an open blocker blocks the close");

    match &blocked {
        TransitionBlocked::GuardFailed { reason } => {
            assert!(
                reason.contains("blocked by an open issue"),
                "the reason names the guard: {reason}"
            );
            assert!(
                reason.contains("In Progress") && reason.contains("Done"),
                "the reason names the from→to: {reason}"
            );
        }
        other => panic!("expected GuardFailed, got {other:?}"),
    }
    assert!(!blocked.reason().is_empty(), "the reason is non-empty");

    assert_eq!(
        issue.state, "In Progress",
        "state unchanged on a blocked close"
    );
    assert_eq!(
        issue.category,
        StateCategory::Started,
        "category unchanged on a blocked close"
    );
    assert_eq!(issue.mutations, 0, "0 mutation on a blocked transition");

    let ctx_clear = IssueContext::new().bind(GuardVar::BLOCKED_BY_OPEN_COUNT, Literal::Int(0));
    issue
        .transition(&wf, "Done", &ctx_clear)
        .expect("with the blocker cleared the close is permitted");
    assert_eq!(issue.category, StateCategory::Completed);
    assert_eq!(issue.mutations, 1);
}

#[test]
fn cdc_the_workflow_guard_config_shape_round_trips() {
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
            guards: vec![
                WorkflowGuard::compiled(
                    "high-severity only",
                    Predicate::Cmp {
                        op: CmpOp::Ge,
                        lhs: Expr::Var("severity".into()),
                        rhs: Expr::Lit(Literal::Int(3)),
                    },
                )
                .unwrap(),
                blocked_by_guard(),
            ],
            required_fields: vec!["resolution".into()],
            post_actions: vec![PostAction::SetField {
                field_id: "resolved_at".into(),
                value: serde_json::json!(1700),
            }],
        }],
    };

    let body = wf.to_body();
    let json = serde_json::to_string(&body).expect("the workflow body serializes");
    assert!(json.contains("\"completed\""), "the FIXED category token");
    assert!(json.contains("\"resolution\""), "the required field");
    assert!(json.contains("severity"), "the guard predicate var");

    let back = Workflow::from_body(&body).expect("the interpreter parses the body back");
    assert_eq!(
        back, wf,
        "the workflow guard config shape round-trips byte-identically"
    );

    let blocked = back
        .plan_transition(
            "Open",
            "Resolved",
            &IssueContext::new()
                .bind("severity", Literal::Int(5))
                .bind(GuardVar::BLOCKED_BY_OPEN_COUNT, Literal::Int(2))
                .mark_present("resolution"),
        )
        .expect_err("an open blocker blocks even after the round-trip");
    assert!(matches!(blocked, TransitionBlocked::GuardFailed { .. }));
}
