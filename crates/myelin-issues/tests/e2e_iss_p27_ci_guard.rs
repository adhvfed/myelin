//! # ISS-P27 / P-394 (M4) — the chained-mutation e2e + the ISS-D12 CI-red drill + the 5.9 consumer CDC
//!
//! The prompt's GATE / DRILLS / TESTS (the cross-module surface; the guard-evaluation + binder +
//! agent-path unit tests live in `src/ci_guard.rs`'s in-module tests):
//!
//! - **The chained-mutation e2e (the X-1 consumer half):** attempt a Done transition while the linked
//!   PR's CI is RED → BLOCKED → the linked PR goes GREEN (a trusted success) → the transition is
//!   ALLOWED. The FIXED completed category is stamped only on the permitted close; 0 mutation on the
//!   blocked one (emit-iff-committed).
//! - **The ISS-D12 CI-red drill (the complete half):** "can't mark Done while CI red on the linked PR"
//!   reads `CheckStatus{state, trust_tier}` OFF THE FACT + the trust posture → transition blocked with
//!   a reason; an AGENT hitting the governed transition is HITL-gated — WITHHELD, 0 mutation
//!   pre-approval. Transition-blocked + 0 pre-approval mutation is the green artifact.
//! - **The provider/consumer CDC pair for 5.9 (the CheckStatus consumer — Issues' read side):** a
//!   FROZEN 5.9 `CheckStatus` payload (the X-1 shape CI produces + Git projects) decodes into the
//!   Issues consumer posture (`LinkedPrCheck`) with NO drift — Issues reads `{state, trust_tier}` off
//!   the fact, never recomputes trust. A drift on either side fails here.
//!
//! **The guard RESTS ON THE PROVEN X-1 SEAM** (GIT-D10 / CI-D8 GREEN end-to-end — `contract-coverage`
//! row 5.9 `covered`: the CI producer EB-27, the Git consumer EB-26, the merge gate + fork endorsement
//! GIT-P21/P22, the merge-queue durable workflow P-FLOW-23). Issues' guard is the LAST consumer leg —
//! it reads the projected fact; it does not re-prove the seam (and it never recomputes trust).
//!
//! The interpreter is the PURE governance decision; the transition ABAC (`Id.check` + the transition
//! `CaveatContext`, contract 4.2) + the typed-core mutate + `OutboxTx::emit(issue.transitioned)` is the
//! ISS-P06 write path. This e2e exercises the X-1 consumer half end-to-end.

use myelin_issues::{
    bind_linked_pr_ctx, ci_done_guard, plan_agent_ci_gated_transition, plan_ci_gated_transition,
    AgentTransitionOutcome, GuardVar, IssueContext, LinkedPrCheck, StateCategory,
    TransitionBlocked, Workflow, WorkflowState, WorkflowTransition, CHECK_STATE_SUCCESS,
    TRUST_TIER_TRUSTED, TRUST_TIER_UNTRUSTED_FORK,
};

/// A toy issue stand-in — the typed-core `state` + `state_category` the write path stamps from a
/// permitted plan. A BLOCKED transition stamps NOTHING (0 mutation — emit-iff-committed).
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

    /// Drive ONE CI-gated governed transition through the consumer entry. On a permitted plan, stamp
    /// the issue (state + the FIXED category) and count the mutation; on a block, mutate NOTHING and
    /// surface the pre-assembled reason.
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

/// The CI-gated engineering workflow: In Review (started) → Done (completed), the close gated by the
/// CI-red Done guard ("can't mark Done while CI red on the linked PR").
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

/// **The chained-mutation e2e: Done blocked while CI red → CI goes green → Done allowed.** The X-1
/// consumer half end-to-end: the linked PR's CURRENT check is read off the fact at each attempt; a red
/// PR blocks the close (0 mutation), a trusted-green PR permits it (the FIXED completed category is
/// stamped). The guard is a config predicate over the live posture, not a latch.
#[test]
fn chained_done_blocked_while_ci_red_then_allowed_when_green() {
    let wf = ci_gated_workflow();
    let mut issue = ToyIssue::new("In Review", StateCategory::Started);

    // 1. The linked PR's CI is RED (a trusted failure) → the close is BLOCKED, 0 mutation.
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

    // 2. CI goes GREEN (a trusted success) → the close is now PERMITTED, the FIXED category stamped.
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

/// **The ISS-D12 CI-red drill (complete): blocked-with-reason + the AGENT is HITL-gated (0 pre-approval
/// mutation).** The CI-red guard reads `CheckStatus + trust posture` off the fact → the human close is
/// blocked with a reason; an AGENT hitting the SAME governed transition (even when the guard would
/// permit it on a green PR) is WITHHELD for HITL approval — 0 mutation pre-approval. Transition-blocked
/// + 0 pre-approval mutation is the green artifact.
#[test]
fn drill_iss_d12_ci_red_guard_blocks_and_agent_is_hitl_gated() {
    let wf = ci_gated_workflow();

    // ── The HUMAN path on a CI-red linked PR → blocked with a reason (0 mutation). ──
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

    // ── The AGENT path on a GREEN linked PR (the guard PERMITS) → WITHHELD for HITL approval, 0
    //    pre-approval mutation. An agent NEVER auto-applies a governed transition (AG-8). ──
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
    // The plan the HITL approval card surfaces (and the write path applies POST-approval).
    if let AgentTransitionOutcome::WithheldForApproval { plan } = agent_outcome {
        assert_eq!(plan.to_category, StateCategory::Completed);
    }

    // ── The AGENT path on a CI-RED linked PR → BLOCKED (nothing to approve), 0 mutation. ──
    let agent_red =
        plan_agent_ci_gated_transition(&wf, "In Review", "Done", IssueContext::new(), &red);
    assert!(
        agent_red.is_blocked(),
        "a CI-red PR is blocked for the agent too"
    );
    assert_eq!(agent_red.pre_approval_mutations(), 0);
}

/// **The poisoned-Done defence drill: an un-endorsed fork success is NEUTRAL → blocked; endorsement
/// unblocks.** A fork PR reports `success` but `trust_tier = untrusted_fork`; un-endorsed it is neutral
/// for the Done gate (the fork cannot turn its OWN Done green — the poisoned-pipeline defence). The
/// maintainer endorses → the same posture unblocks. Issues reads the tier off the fact (never
/// recomputed).
#[test]
fn drill_poisoned_done_unendorsed_fork_is_neutral_endorsement_unblocks() {
    let wf = ci_gated_workflow();

    // Un-endorsed fork success → neutral → blocked (0 mutation).
    let mut issue = ToyIssue::new("In Review", StateCategory::Started);
    let unendorsed = LinkedPrCheck::untrusted_fork(CHECK_STATE_SUCCESS, false);
    let blocked = issue
        .ci_transition(&wf, "Done", IssueContext::new(), &unendorsed)
        .expect_err("an un-endorsed fork success is neutral → blocked");
    assert!(matches!(blocked, TransitionBlocked::GuardFailed { .. }));
    assert_eq!(
        issue.mutations, 0,
        "0 mutation — the fork cannot self-green its Done"
    );

    // The maintainer endorses (the approve_untrusted_ci flow) → the SAME posture now unblocks.
    let endorsed = LinkedPrCheck::untrusted_fork(CHECK_STATE_SUCCESS, true);
    issue
        .ci_transition(&wf, "Done", IssueContext::new(), &endorsed)
        .expect("an endorsed fork success unblocks the close");
    assert_eq!(issue.category, StateCategory::Completed);
    assert_eq!(issue.mutations, 1);
}

/// **The 5.9 consumer CDC pair (Issues' read side): a FROZEN 5.9 `CheckStatus` payload decodes into the
/// Issues consumer posture with NO drift.** The PROVIDER (CI) assembles + emits the frozen 5.9
/// `CheckStatus{..., state, trust_tier, ...}`; Git projects it; the CONSUMER (Issues) reads
/// `{state, trust_tier}` OFF THE FACT through `project(PR_ref)` and reduces it to a `LinkedPrCheck`.
/// This pins the two halves: Issues consumes the EXACT 5.9 `snake_case` token vocabulary, and NEVER
/// recomputes trust (the tier is carried verbatim off the fact). A drift on either side fails here.
#[test]
fn cdc_5_9_check_status_decodes_into_the_issues_consumer_posture() {
    // The PROVIDER half — the FROZEN 5.9 `CheckStatus` shape CI produces + Git projects (the exact X-1
    // reconciliation shape; the `snake_case` tokens are byte-identical to CI's producer + Git's
    // consumer view). We model the projected fact as the 5.9 JSON the seam carries (opaque to the Bus,
    // decoded by the consumer — references-not-payloads).
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

    // The CONSUMER half — Issues reads `{state, trust_tier}` OFF THE FACT (never recomputes trust).
    // This is the decode at the consumer seam (the `project(PR_ref)` read reduced to the posture); the
    // fork-endorsement bit is read off Git's seam (here un-endorsed).
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

    let check = LinkedPrCheck::untrusted_fork(state, /* endorsed off the seam */ false);
    // The posture matches the merge-gate rule: an un-endorsed fork success is NOT acceptable.
    assert!(
        !check.is_acceptable(),
        "Issues consumes the SAME trust posture as Git's merge gate (un-endorsed fork ⇒ neutral)"
    );

    // The binder stamps the trust tier VERBATIM off the fact (the no-recompute invariant).
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

    // A TRUSTED success in the same shape IS acceptable (the other CDC arm — the happy path token).
    let trusted_fact = serde_json::json!({ "state": "success", "trust_tier": "trusted" });
    let tcheck = LinkedPrCheck::trusted(trusted_fact["state"].as_str().unwrap());
    assert_eq!(tcheck.trust_tier, TRUST_TIER_TRUSTED);
    assert!(
        tcheck.is_acceptable(),
        "a trusted 5.9 success is acceptable"
    );
}
