use myelin_git::lifecycle::{
    evaluate_ruleset, BranchProtectionRuleset, CodeOwners, Comment, DiffAnchor, MergeContext,
    PrState, PrTransition, PullRequest, Review, ReviewState, ReviewVerdict, Thread, ThreadState,
};

const CODEOWNERS: &str = "\
# default owners (catch-all first; a later, more-specific rule overrides - last match wins)
*               @acme/core-team
# the payments dir requires payments-team review (later → wins for that path)
/src/payments/  @acme/payments
";

fn ruleset() -> BranchProtectionRuleset {
    BranchProtectionRuleset {
        ref_pattern: "refs/heads/main".into(),
        required_contexts: vec!["ci/build".into(), "ci/test".into()],
        required_approvals: 1,
        require_codeowner_review: true,
        require_conversation_resolution: true,
        allow_force_push: false,
    }
}

#[test]
fn open_review_resolve_merge_chains_end_to_end() {
    let mut pr = PullRequest::open(
        101,
        "refs/heads/main",
        "refs/heads/feat/charge",
        "psn:dev",
        true,
    );
    assert_eq!(pr.state, PrState::Draft);

    let rs = ruleset();
    assert!(rs.matches(&pr.base_ref), "main is protected");

    let co = CodeOwners::parse(CODEOWNERS).expect("valid CODEOWNERS");
    let owners = co.owners_for("src/payments/charge.rs");
    assert_eq!(
        owners,
        &["@acme/payments".to_string()],
        "payments owns the path (last match wins)"
    );

    assert_eq!(
        pr.transition(PrTransition::MarkReady, false).unwrap(),
        PrState::Open
    );

    let mut review = Review::request(owners[0].clone(), false);
    assert_eq!(review.state, ReviewState::Requested);
    review.submit(ReviewVerdict::Approve).unwrap();
    assert!(review.is_current_approval());

    let mut thread = Thread::open(
        1,
        DiffAnchor {
            path: "src/payments/charge.rs".into(),
            start_line: 20,
            end_line: 24,
        },
        Comment {
            id: 500,
            author_pseudonym: "psn:reviewer".into(),
            body: Default::default(),
            is_agent: false,
        },
    );
    assert!(thread.is_outstanding());
    thread.reply(Comment {
        id: 501,
        author_pseudonym: "psn:dev".into(),
        body: Default::default(),
        is_agent: false,
    });
    assert_eq!(thread.resolve().unwrap(), ThreadState::Resolved);
    assert!(!thread.is_outstanding());

    let ctx = MergeContext {
        green_contexts: vec!["ci/build".into(), "ci/test".into()],
        current_approvals: if review.is_current_approval() { 1 } else { 0 },
        codeowner_review_satisfied: true,
        has_blocking_review: false,
        outstanding_conversations: if thread.is_outstanding() { 1 } else { 0 },
    };
    let gate = evaluate_ruleset(&rs, &ctx);
    assert!(
        gate.is_satisfied(),
        "all conditions met → the gate is satisfied: {gate:?}"
    );

    assert_eq!(
        pr.transition(PrTransition::Merge, gate.is_satisfied())
            .unwrap(),
        PrState::Merged
    );
    assert!(pr.state.is_terminal());
}

#[test]
fn blocked_merge_is_refused_then_pr_is_closed() {
    let mut pr = PullRequest::open(
        102,
        "refs/heads/main",
        "refs/heads/feat/x",
        "psn:dev",
        false,
    );
    let rs = ruleset();

    let mut review = Review::request("@acme/payments", false);
    review.submit(ReviewVerdict::RequestChanges).unwrap();
    assert!(review.is_blocking());

    let ctx = MergeContext {
        green_contexts: vec!["ci/build".into()],
        current_approvals: 0,
        codeowner_review_satisfied: false,
        has_blocking_review: review.is_blocking(),
        outstanding_conversations: 2,
    };
    let gate = evaluate_ruleset(&rs, &ctx);
    assert!(!gate.is_satisfied(), "the gate blocks: {gate:?}");

    assert_eq!(
        pr.transition(PrTransition::Merge, gate.is_satisfied()),
        Err(myelin_git::lifecycle::LifecycleError::MergeGateNotSatisfied)
    );
    assert_eq!(pr.state, PrState::Open, "a blocked merge does not land");

    assert_eq!(
        pr.transition(PrTransition::Close, false).unwrap(),
        PrState::Closed
    );
}
