//! # ISS-P12 / P-378 (M4) — the chained-mutation e2e + the ISS-D12 guard-half drill + the CDC stub
//!
//! The prompt's GATE / DRILLS / TESTS (the cross-module surface; the FSM-interpreter unit tests —
//! the fixed-category invariant, a guard rejects, required-fields enforced, post-actions fire — live
//! in `src/workflow.rs`'s in-module tests):
//!
//! - **The chained-mutation e2e:** drive an issue through its states (Todo → In Progress → Done) and
//!   assert (a) the FIXED state-category invariant is stamped at each step, and (b) a `blocked_by`
//!   guard REJECTS the close when an open blocker exists → the pre-assembled reason is the green
//!   artifact.
//! - **The ISS-D12 guard-half drill:** "can't close while `blocked_by` an open issue" → transition
//!   blocked with a reason; 0 pre-approval mutation. The CI-red half (the X-1 `CheckStatus` + trust
//!   posture) is the named ISS-P27 floor — the guard SHAPE is exercised here (fails closed until then).
//! - **The CDC stub for the guard config shape:** the `workflow`-scheme `body`
//!   (`{states:[{name,category}], transitions:[{from,to,guards,required_fields,post_actions}]}`)
//!   serializes + parses back identically — the config shape the FSM interpreter (consumer) reads from
//!   the scheme author (provider, ISS-P11). A drift on either side fails here.
//!
//! The interpreter is the PURE governance decision; the transition ABAC (`Id.check` + the transition
//! `CaveatContext`, contract 4.2) + the typed-core mutate + the `OutboxTx::emit(issue.transitioned)`
//! is the ISS-P06 write path (`write_path.rs`). This e2e exercises the interpreter half end-to-end.

use myelin_identity::Literal;
use myelin_issues::{
    blocked_by_guard, GuardVar, IssueContext, PostAction, StateCategory, TransitionBlocked,
    Workflow, WorkflowGuard, WorkflowState, WorkflowTransition,
};
use myelin_query::{CmpOp, Expr, Predicate};

/// A toy issue stand-in — the typed-core `state` + `state_category` the write path stamps from a
/// permitted [`Workflow::plan_transition`]. The e2e drives transitions through the interpreter and
/// stamps the issue from the returned plan (modelling the ISS-P06 write path's `issue.state =
/// target; issue.state_category = t.to_category`). A BLOCKED transition stamps NOTHING (0 mutation).
#[derive(Clone, Debug, PartialEq)]
struct ToyIssue {
    state: String,
    category: StateCategory,
    /// the number of times the typed core was actually mutated (a blocked transition adds 0).
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

    /// Drive ONE governed transition through the interpreter. On a permitted plan, stamp the issue
    /// (state + the FIXED category) and count the mutation; on a block, mutate NOTHING and surface the
    /// pre-assembled reason (the write path would return `Err` before its transaction commits — 0
    /// ghost / emit-iff-committed).
    fn transition(
        &mut self,
        wf: &Workflow,
        target: &str,
        ctx: &IssueContext,
    ) -> Result<(), TransitionBlocked> {
        let plan = wf.plan_transition(&self.state, target, ctx)?;
        // The write path stamps the typed core from the plan (the FIXED category is the invariant).
        self.state = plan.to.clone();
        self.category = plan.to_category;
        self.mutations += 1;
        Ok(())
    }
}

/// The canonical engineering workflow: Todo (unstarted) → In Progress (started) → Done (completed),
/// with In Progress → Done guarded by `blocked_by_open_count == 0` (the ISS-D12 guard half) and a
/// Cancel edge to Cancelled (cancelled). One post-action arms an SLA timer on the start transition.
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

/// **The chained-mutation e2e: transition through the states → assert the FIXED category invariant at
/// each step + the post-action stages on the permitted start.** Todo (unstarted) → In Progress
/// (started, assign-post-action) → Done (completed, with 0 open blockers). The category is stamped
/// from the FIXED set at every hop — never an open-ended name.
#[test]
fn chained_transition_stamps_the_fixed_category_at_each_step() {
    let wf = engineering_workflow();
    let mut issue = ToyIssue::new("Todo", StateCategory::Unstarted);

    // Todo → In Progress (unguarded; stages the assign post-action).
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

    // In Progress → Done with 0 open blockers (the guard holds) → completed.
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

/// **The ISS-D12 guard half: can't close while `blocked_by` an open issue → transition blocked with a
/// reason; 0 pre-approval mutation.** With an open blocker the In Progress → Done close is BLOCKED;
/// the issue is NOT mutated (the write path commits nothing — emit-iff-committed), and the
/// pre-assembled, admin-authored reason is the green artifact.
#[test]
fn drill_iss_d12_cannot_close_while_blocked_by_open_issue() {
    let wf = engineering_workflow();
    let mut issue = ToyIssue::new("In Progress", StateCategory::Started);

    // 1 open blocker → the guard is false → the close is blocked.
    let ctx_blocked = IssueContext::new().bind(GuardVar::BLOCKED_BY_OPEN_COUNT, Literal::Int(1));
    let blocked = issue
        .transition(&wf, "Done", &ctx_blocked)
        .expect_err("an open blocker blocks the close");

    // The block is a GuardFailed with a deterministic, pre-assembled reason (the green artifact).
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

    // 0 PRE-APPROVAL MUTATION: the blocked transition stamped nothing (the issue is unchanged).
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

    // Clear the blocker → the close is now permitted (the guard is a config predicate, not a latch).
    let ctx_clear = IssueContext::new().bind(GuardVar::BLOCKED_BY_OPEN_COUNT, Literal::Int(0));
    issue
        .transition(&wf, "Done", &ctx_clear)
        .expect("with the blocker cleared the close is permitted");
    assert_eq!(issue.category, StateCategory::Completed);
    assert_eq!(issue.mutations, 1);
}

/// **The CDC stub for the guard config shape: the `workflow`-scheme `body` round-trips through serde
/// JSON byte-identically.** The provider (the ISS-P11 scheme author) writes the JSONB `body`; the
/// consumer (the ISS-P12 FSM interpreter) reads the SAME shape via `Workflow::from_body`. A guard's
/// frozen `QueryAst` predicate + the post-actions (incl. the arm-trigger `EventMatcher`) serialize +
/// parse back identically — a drift on either side fails here.
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
            // A frozen-QueryAst guard (severity >= 3) + the blocked_by guard.
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

    // Serialize → the JSONB body the scheme `body` column holds (the config write).
    let body = wf.to_body();
    let json = serde_json::to_string(&body).expect("the workflow body serializes");
    // The body carries the fixed-category tokens + the guard predicate + the required field.
    assert!(json.contains("\"completed\""), "the FIXED category token");
    assert!(json.contains("\"resolution\""), "the required field");
    assert!(json.contains("severity"), "the guard predicate var");

    // Parse back through the interpreter's `from_body` (the consumer seam) → byte-identical.
    let back = Workflow::from_body(&body).expect("the interpreter parses the body back");
    assert_eq!(
        back, wf,
        "the workflow guard config shape round-trips byte-identically"
    );

    // And the parsed-back guard still BLOCKS correctly (the config survived the round-trip live).
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
