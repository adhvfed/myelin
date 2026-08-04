use crate::workflow::{
    GuardVar, IssueContext, TransitionBlocked, TransitionPlan, Workflow, WorkflowGuard,
};
use myelin_identity::Literal;
use myelin_query::{CmpOp, Expr, Predicate};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkedPrCheck {
    pub state: String,
    pub trust_tier: String,
    pub endorsed: bool,
}

pub const CHECK_STATE_SUCCESS: &str = "success";

pub const CHECK_STATE_NEUTRAL: &str = "neutral";

pub const TRUST_TIER_TRUSTED: &str = "trusted";

pub const TRUST_TIER_UNTRUSTED_FORK: &str = "untrusted_fork";

impl LinkedPrCheck {
    pub fn trusted(state: impl Into<String>) -> LinkedPrCheck {
        LinkedPrCheck {
            state: state.into(),
            trust_tier: TRUST_TIER_TRUSTED.to_string(),
            endorsed: false,
        }
    }

    pub fn untrusted_fork(state: impl Into<String>, endorsed: bool) -> LinkedPrCheck {
        LinkedPrCheck {
            state: state.into(),
            trust_tier: TRUST_TIER_UNTRUSTED_FORK.to_string(),
            endorsed,
        }
    }

    pub fn is_acceptable(&self) -> bool {
        if self.state != CHECK_STATE_SUCCESS {
            return false;
        }
        match self.trust_tier.as_str() {
            TRUST_TIER_TRUSTED => true,
            TRUST_TIER_UNTRUSTED_FORK => self.endorsed,
            _ => false,
        }
    }

    fn gated_status_token(&self) -> &str {
        if self.is_acceptable() {
            CHECK_STATE_SUCCESS
        } else {
            CHECK_STATE_NEUTRAL
        }
    }
}

pub fn bind_linked_pr_ctx(ctx: IssueContext, check: &LinkedPrCheck) -> IssueContext {
    ctx.bind(
        GuardVar::LINKED_PR_CHECK_STATUS,
        Literal::Str(check.gated_status_token().to_string()),
    )
    .bind(
        GuardVar::LINKED_PR_TRUST_TIER,
        Literal::Str(check.trust_tier.clone()),
    )
}

pub fn ci_done_guard() -> WorkflowGuard {
    WorkflowGuard::compiled(
        "the linked PR's CI is not green (or is an un-endorsed fork run)",
        Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var(GuardVar::LINKED_PR_CHECK_STATUS.into()),
            rhs: Expr::Lit(Literal::Str(CHECK_STATE_SUCCESS.into())),
        },
    )
    .expect("the CI-red Done guard is a single bounded comparison (within the cost bound)")
}

pub fn plan_ci_gated_transition(
    wf: &Workflow,
    from: &str,
    target: &str,
    base_ctx: IssueContext,
    linked_pr: &LinkedPrCheck,
) -> Result<TransitionPlan, TransitionBlocked> {
    let ctx = bind_linked_pr_ctx(base_ctx, linked_pr);
    wf.plan_transition(from, target, &ctx)
}

#[derive(Clone, Debug, PartialEq)]
pub enum AgentTransitionOutcome {
    Blocked {
        block: TransitionBlocked,
    },
    WithheldForApproval {
        plan: TransitionPlan,
    },
}

impl AgentTransitionOutcome {
    pub fn is_withheld(&self) -> bool {
        matches!(self, AgentTransitionOutcome::WithheldForApproval { .. })
    }

    pub fn is_blocked(&self) -> bool {
        matches!(self, AgentTransitionOutcome::Blocked { .. })
    }

    pub fn pre_approval_mutations(&self) -> u64 {
        0
    }
}

pub fn plan_agent_ci_gated_transition(
    wf: &Workflow,
    from: &str,
    target: &str,
    base_ctx: IssueContext,
    linked_pr: &LinkedPrCheck,
) -> AgentTransitionOutcome {
    match plan_ci_gated_transition(wf, from, target, base_ctx, linked_pr) {
        Ok(plan) => AgentTransitionOutcome::WithheldForApproval { plan },
        Err(block) => AgentTransitionOutcome::Blocked { block },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::{StateCategory, WorkflowState, WorkflowTransition};

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
    fn a_trusted_success_unblocks_done() {
        let wf = ci_gated_workflow();
        let check = LinkedPrCheck::trusted(CHECK_STATE_SUCCESS);
        let plan = plan_ci_gated_transition(&wf, "In Review", "Done", IssueContext::new(), &check)
            .expect("a trusted success unblocks the close");
        assert_eq!(plan.to_category, StateCategory::Completed);
        assert_eq!(plan.from, "In Review");
        assert_eq!(plan.to, "Done");
    }

    #[test]
    fn a_ci_red_linked_pr_blocks_done() {
        let wf = ci_gated_workflow();
        let check = LinkedPrCheck::trusted("failure");
        let blocked =
            plan_ci_gated_transition(&wf, "In Review", "Done", IssueContext::new(), &check)
                .expect_err("a CI-red linked PR blocks the close");
        match &blocked {
            TransitionBlocked::GuardFailed { reason } => {
                assert!(
                    reason.contains("CI is not green"),
                    "the reason names the CI-red guard: {reason}"
                );
                assert!(
                    reason.contains("In Review") && reason.contains("Done"),
                    "the reason names the from→to: {reason}"
                );
            }
            other => panic!("expected GuardFailed, got {other:?}"),
        }
        assert!(!blocked.reason().is_empty());
    }

    #[test]
    fn an_unendorsed_fork_success_is_neutral_and_blocks() {
        let wf = ci_gated_workflow();
        let check = LinkedPrCheck::untrusted_fork(CHECK_STATE_SUCCESS, false);
        assert!(
            !check.is_acceptable(),
            "an un-endorsed fork success is NOT acceptable (neutral)"
        );
        let blocked =
            plan_ci_gated_transition(&wf, "In Review", "Done", IssueContext::new(), &check)
                .expect_err("an un-endorsed fork success is neutral → blocked");
        assert!(matches!(blocked, TransitionBlocked::GuardFailed { .. }));
    }

    #[test]
    fn an_endorsed_fork_success_unblocks() {
        let wf = ci_gated_workflow();
        let check = LinkedPrCheck::untrusted_fork(CHECK_STATE_SUCCESS, true);
        assert!(
            check.is_acceptable(),
            "an endorsed fork success is acceptable"
        );
        let plan = plan_ci_gated_transition(&wf, "In Review", "Done", IssueContext::new(), &check)
            .expect("an endorsed fork success unblocks");
        assert_eq!(plan.to_category, StateCategory::Completed);
    }

    #[test]
    fn the_trust_tier_is_bound_verbatim_off_the_fact() {
        let check = LinkedPrCheck::untrusted_fork(CHECK_STATE_SUCCESS, false);
        let ctx = bind_linked_pr_ctx(IssueContext::new(), &check);
        let status_guard = WorkflowGuard::compiled(
            "status",
            Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: Expr::Var(GuardVar::LINKED_PR_CHECK_STATUS.into()),
                rhs: Expr::Lit(Literal::Str(CHECK_STATE_NEUTRAL.into())),
            },
        )
        .unwrap();
        assert_eq!(
            status_guard.predicate.eval(ctx.attrs()),
            Ok(true),
            "an un-endorsed fork success binds the neutral status token"
        );
        let tier_guard = WorkflowGuard::compiled(
            "tier",
            Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: Expr::Var(GuardVar::LINKED_PR_TRUST_TIER.into()),
                rhs: Expr::Lit(Literal::Str(TRUST_TIER_UNTRUSTED_FORK.into())),
            },
        )
        .unwrap();
        assert_eq!(
            tier_guard.predicate.eval(ctx.attrs()),
            Ok(true),
            "the CI-stamped trust tier is bound verbatim off the fact"
        );
    }

    #[test]
    fn an_unknown_trust_token_fails_closed() {
        let check = LinkedPrCheck {
            state: CHECK_STATE_SUCCESS.into(),
            trust_tier: "some_new_tier".into(),
            endorsed: true,
        };
        assert!(
            !check.is_acceptable(),
            "an unrecognised trust tier is never acceptable (fail closed)"
        );
    }

    #[test]
    fn the_agent_path_withholds_a_permitted_transition_for_approval() {
        let wf = ci_gated_workflow();
        let check = LinkedPrCheck::trusted(CHECK_STATE_SUCCESS);
        let outcome =
            plan_agent_ci_gated_transition(&wf, "In Review", "Done", IssueContext::new(), &check);
        assert!(
            outcome.is_withheld(),
            "the agent's permitted transition is WITHHELD for HITL approval, not auto-applied"
        );
        assert!(
            !outcome.is_blocked(),
            "a withheld (permitted-but-gated) transition is NOT a guard block"
        );
        assert_eq!(
            outcome.pre_approval_mutations(),
            0,
            "0 pre-approval mutation (the agent path is pre-approval, 0-mutation)"
        );
        if let AgentTransitionOutcome::WithheldForApproval { plan } = outcome {
            assert_eq!(plan.to_category, StateCategory::Completed);
            assert_eq!(plan.to, "Done");
        }
    }

    #[test]
    fn the_agent_path_blocks_on_a_ci_red_linked_pr() {
        let wf = ci_gated_workflow();
        let check = LinkedPrCheck::trusted("failure");
        let outcome =
            plan_agent_ci_gated_transition(&wf, "In Review", "Done", IssueContext::new(), &check);
        assert!(
            outcome.is_blocked(),
            "a CI-red linked PR is BLOCKED for the agent (nothing to approve)"
        );
        assert!(!outcome.is_withheld());
        assert_eq!(outcome.pre_approval_mutations(), 0, "0 mutation on a block");
        if let AgentTransitionOutcome::Blocked { block } = outcome {
            assert!(matches!(block, TransitionBlocked::GuardFailed { .. }));
        }
    }

    #[test]
    fn is_acceptable_matches_the_merge_gate_posture() {
        assert!(!LinkedPrCheck::trusted("failure").is_acceptable());
        assert!(!LinkedPrCheck::trusted("in_progress").is_acceptable());
        assert!(!LinkedPrCheck::trusted("queued").is_acceptable());
        assert!(!LinkedPrCheck::untrusted_fork("error", true).is_acceptable());
        assert!(LinkedPrCheck::trusted(CHECK_STATE_SUCCESS).is_acceptable());
        assert!(!LinkedPrCheck::untrusted_fork(CHECK_STATE_SUCCESS, false).is_acceptable());
        assert!(LinkedPrCheck::untrusted_fork(CHECK_STATE_SUCCESS, true).is_acceptable());
    }

    #[test]
    fn the_ci_done_guard_is_the_frozen_query_ast() {
        let guard = ci_done_guard();
        let green = IssueContext::new().bind(
            GuardVar::LINKED_PR_CHECK_STATUS,
            Literal::Str(CHECK_STATE_SUCCESS.into()),
        );
        assert_eq!(guard.predicate.eval(green.attrs()), Ok(true));
        let red = IssueContext::new().bind(
            GuardVar::LINKED_PR_CHECK_STATUS,
            Literal::Str(CHECK_STATE_NEUTRAL.into()),
        );
        assert_eq!(guard.predicate.eval(red.attrs()), Ok(false));
    }
}
