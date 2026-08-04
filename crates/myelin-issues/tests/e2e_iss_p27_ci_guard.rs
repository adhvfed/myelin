use myelin_issues::{
    bind_linked_pr_ctx, ci_done_guard, plan_agent_ci_gated_transition, plan_ci_gated_transition,
    AgentTransitionOutcome, GuardVar, IssueContext, LinkedPrCheck, StateCategory,
    TransitionBlocked, Workflow, WorkflowState, WorkflowTransition, CHECK_STATE_SUCCESS,
    TRUST_TIER_TRUSTED, TRUST_TIER_UNTRUSTED_FORK,
};

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

    fn ci_transition(
        &mut self,
        wf: &Workflow,
        target: &str,
        base_ctx: IssueContext,
        linked_pr: &LinkedPrCheck,
    ) -> Result<(), TransitionBlocked> {
        let plan = plan_ci_gated_transition(wf, &self.state, target, base_ctx, linked_pr)?;
        self.state = plan.to.clone();
        self.category = plan.to_category;
        self.mutations += 1;
        Ok(())
    }
}

fn ci_gated_workflow() -> Workflow {
    Workflow {
        states: vec![
            WorkflowState {
                name: "In Review".into(),
                category: StateCategory::Started,
            },
            WorkflowState {
                name: "Done".into(),
                category: StateCategory::Completed,
            },
        ],
        transitions: vec![WorkflowTransition {
            from: "In Review".into(),
            to: "Done".into(),
            guards: vec![ci_done_guard()],
            required_fields: vec![],
            post_actions: vec![],
        }],
    }
}

#[test]
fn chained_done_blocked_while_ci_red_then_allowed_when_green() {
    let wf = ci_gated_workflow();
    let mut issue = ToyIssue::new("In Review", StateCategory::Started);

    let red = LinkedPrCheck::trusted("failure");
    let blocked = issue
        .ci_transition(&wf, "Done", IssueContext::new(), &red)
        .expect_err("a CI-red linked PR blocks the close");
    assert!(matches!(blocked, TransitionBlocked::GuardFailed { .. }));
    assert_eq!(
        issue.state, "In Review",
        "state unchanged on a blocked close"
    );
    assert_eq!(issue.category, StateCategory::Started);
    assert_eq!(issue.mutations, 0, "0 mutation on a blocked transition");

    let green = LinkedPrCheck::trusted(CHECK_STATE_SUCCESS);
    issue
        .ci_transition(&wf, "Done", IssueContext::new(), &green)
        .expect("a trusted-green linked PR permits the close");
    assert_eq!(
        issue.category,
        StateCategory::Completed,
        "the close stamps the FIXED completed category once CI is green"
    );
    assert_eq!(issue.mutations, 1, "exactly one permitted mutation");
}

#[test]
fn drill_iss_d12_ci_red_guard_blocks_and_agent_is_hitl_gated() {
    let wf = ci_gated_workflow();

    let mut human_issue = ToyIssue::new("In Review", StateCategory::Started);
    let red = LinkedPrCheck::trusted("failure");
    let blocked = human_issue
        .ci_transition(&wf, "Done", IssueContext::new(), &red)
        .expect_err("the human close is blocked while CI is red");
    match &blocked {
        TransitionBlocked::GuardFailed { reason } => {
            assert!(
                reason.contains("CI is not green"),
                "the reason names the guard: {reason}"
            );
        }
        other => panic!("expected GuardFailed, got {other:?}"),
    }
    assert_eq!(
        human_issue.mutations, 0,
        "0 mutation on the blocked human close"
    );

    let green = LinkedPrCheck::trusted(CHECK_STATE_SUCCESS);
    let agent_outcome =
        plan_agent_ci_gated_transition(&wf, "In Review", "Done", IssueContext::new(), &green);
    assert!(
        agent_outcome.is_withheld(),
        "the agent's permitted governed transition is WITHHELD for HITL approval"
    );
    assert_eq!(
        agent_outcome.pre_approval_mutations(),
        0,
        "0 pre-approval mutation (the headline ISS-D12 green artifact)"
    );
    if let AgentTransitionOutcome::WithheldForApproval { plan } = agent_outcome {
        assert_eq!(plan.to_category, StateCategory::Completed);
    }

    let agent_red =
        plan_agent_ci_gated_transition(&wf, "In Review", "Done", IssueContext::new(), &red);
    assert!(
        agent_red.is_blocked(),
        "a CI-red PR is blocked for the agent too"
    );
    assert_eq!(agent_red.pre_approval_mutations(), 0);
}

#[test]
fn drill_poisoned_done_unendorsed_fork_is_neutral_endorsement_unblocks() {
    let wf = ci_gated_workflow();

    let mut issue = ToyIssue::new("In Review", StateCategory::Started);
    let unendorsed = LinkedPrCheck::untrusted_fork(CHECK_STATE_SUCCESS, false);
    let blocked = issue
        .ci_transition(&wf, "Done", IssueContext::new(), &unendorsed)
        .expect_err("an un-endorsed fork success is neutral → blocked");
    assert!(matches!(blocked, TransitionBlocked::GuardFailed { .. }));
    assert_eq!(
        issue.mutations, 0,
        "0 mutation - the fork cannot self-green its Done"
    );

    let endorsed = LinkedPrCheck::untrusted_fork(CHECK_STATE_SUCCESS, true);
    issue
        .ci_transition(&wf, "Done", IssueContext::new(), &endorsed)
        .expect("an endorsed fork success unblocks the close");
    assert_eq!(issue.category, StateCategory::Completed);
    assert_eq!(issue.mutations, 1);
}

#[test]
fn cdc_5_9_check_status_decodes_into_the_issues_consumer_posture() {
    let projected_fact = serde_json::json!({
        "tenant": "acme",
        "repo": "myelin://acme/git/repo/core",
        "commit_oid": "blake3:deadbeef",
        "context": { "provider": "ci", "name": "build" },
        "state": "success",
        "required": true,
        "run": "myelin://acme/ci/run/7",
        "run_attempt": 2,
        "trust_tier": "untrusted_fork",
        "details_ref": "myelin://acme/ci/run/7#step-3",
        "summary": { "template_key": "ci.check.success", "args": {} },
        "started_at": "2026-06-21T00:00:00Z",
        "completed_at": "2026-06-21T00:01:00Z",
        "cost_settled": true
    });

    let state = projected_fact["state"]
        .as_str()
        .expect("the 5.9 state token");
    let trust_tier = projected_fact["trust_tier"]
        .as_str()
        .expect("the 5.9 trust_tier token");
    assert_eq!(
        state, CHECK_STATE_SUCCESS,
        "the consumer reads the 5.9 success token"
    );
    assert_eq!(
        trust_tier, TRUST_TIER_UNTRUSTED_FORK,
        "the consumer reads the 5.9 untrusted_fork token verbatim (never recomputed)"
    );

    let check = LinkedPrCheck::untrusted_fork(state,  false);
    assert!(
        !check.is_acceptable(),
        "Issues consumes the SAME trust posture as Git's merge gate (un-endorsed fork ⇒ neutral)"
    );

    let ctx = bind_linked_pr_ctx(IssueContext::new(), &check);
    let tier_guard = myelin_issues::WorkflowGuard::compiled(
        "tier",
        myelin_query::Predicate::Cmp {
            op: myelin_query::CmpOp::Eq,
            lhs: myelin_query::Expr::Var(GuardVar::LINKED_PR_TRUST_TIER.into()),
            rhs: myelin_query::Expr::Lit(myelin_identity::Literal::Str(
                TRUST_TIER_UNTRUSTED_FORK.into(),
            )),
        },
    )
    .unwrap();
    assert_eq!(
        tier_guard.predicate.eval(ctx.attrs()),
        Ok(true),
        "the CI-stamped trust tier is bound verbatim off the fact"
    );

    let trusted_fact = serde_json::json!({ "state": "success", "trust_tier": "trusted" });
    let tcheck = LinkedPrCheck::trusted(trusted_fact["state"].as_str().unwrap());
    assert_eq!(tcheck.trust_tier, TRUST_TIER_TRUSTED);
    assert!(
        tcheck.is_acceptable(),
        "a trusted 5.9 success is acceptable"
    );
}
