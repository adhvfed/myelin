//! # The chained e2e for GIT-P16 / P-277 — open PR → request review (CODEOWNERS resolves) →
//! submit review → close (and the merge path).
//!
//! "Actually try it — chain the mutations end-to-end" (EI-01 §4). This drives the PR/review/thread
//! lifecycle + the branch-protection gate + the CODEOWNERS resolver through ONE realistic flow against
//! the in-memory entities, the SAME way the live OLTP store (GIT-P20) will: a draft PR is opened,
//! marked ready, a CODEOWNERS reviewer set is resolved from a repo's CODEOWNERS file, a review is
//! requested + submitted, an inline thread is opened + resolved, the branch-protection ruleset is
//! evaluated at merge time, and the PR is merged (gate satisfied) or closed.
//!
//! The CODEOWNERS half rides through the real engine in `cdc_4_9_git_codeowners.rs`; here we chain the
//! whole lifecycle (the GATE: 0 illegal transitions over a real flow; 0 unprotected merges).

use myelin_git::lifecycle::{
    evaluate_ruleset, BranchProtectionRuleset, CodeOwners, Comment, DiffAnchor, MergeContext,
    PrState, PrTransition, PullRequest, Review, ReviewState, ReviewVerdict, Thread, ThreadState,
};

const CODEOWNERS: &str = "\
# default owners (catch-all first; a later, more-specific rule overrides — last match wins)
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

/// **The happy path: open draft → ready → review requested (CODEOWNERS resolves) → approved → thread
/// resolved → gate satisfied → merged.** Every transition is legal; the merge lands only once the
/// branch-protection ruleset is satisfied.
#[test]
fn open_review_resolve_merge_chains_end_to_end() {
    // 1) open a DRAFT PR targeting the protected main.
    let mut pr = PullRequest::open(101, "refs/heads/main", "refs/heads/feat/charge", "psn:dev", true);
    assert_eq!(pr.state, PrState::Draft);

    // 2) the branch-protection ruleset protects main (the base ref).
    let rs = ruleset();
    assert!(rs.matches(&pr.base_ref), "main is protected");

    // 3) the PR touches /src/payments/charge.rs → resolve its CODEOWNERS reviewers.
    let co = CodeOwners::parse(CODEOWNERS).expect("valid CODEOWNERS");
    let owners = co.owners_for("src/payments/charge.rs");
    assert_eq!(owners, &["@acme/payments".to_string()], "payments owns the path (last match wins)");

    // 4) mark the draft ready for review.
    assert_eq!(pr.transition(PrTransition::MarkReady, false).unwrap(), PrState::Open);

    // 5) request a review from the resolved CODEOWNER, then submit an approval.
    let mut review = Review::request(owners[0].clone(), /*is_agent*/ false);
    assert_eq!(review.state, ReviewState::Requested);
    review.submit(ReviewVerdict::Approve).unwrap();
    assert!(review.is_current_approval());

    // 6) an inline thread on the diff is opened, discussed, and resolved.
    let mut thread = Thread::open(
        1,
        DiffAnchor { path: "src/payments/charge.rs".into(), start_line: 20, end_line: 24 },
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

    // 7) at merge time, evaluate the ruleset against the CURRENT state.
    let ctx = MergeContext {
        green_contexts: vec!["ci/build".into(), "ci/test".into()],
        current_approvals: if review.is_current_approval() { 1 } else { 0 },
        codeowner_review_satisfied: true, // the CODEOWNER (payments) approved.
        has_blocking_review: false,
        outstanding_conversations: if thread.is_outstanding() { 1 } else { 0 },
    };
    let gate = evaluate_ruleset(&rs, &ctx);
    assert!(gate.is_satisfied(), "all conditions met → the gate is satisfied: {gate:?}");

    // 8) the merge lands (the gate guard admits it).
    assert_eq!(pr.transition(PrTransition::Merge, gate.is_satisfied()).unwrap(), PrState::Merged);
    assert!(pr.state.is_terminal());
}

/// **The blocked path: a request-changes review + a missing context + an outstanding thread block the
/// merge; the PR is closed instead (0 unprotected merges).** The merge transition is REFUSED while the
/// gate is unsatisfied — then the PR is closed (a legal terminal-ish state) without ever landing.
#[test]
fn blocked_merge_is_refused_then_pr_is_closed() {
    let mut pr = PullRequest::open(102, "refs/heads/main", "refs/heads/feat/x", "psn:dev", false);
    let rs = ruleset();

    // a reviewer requests changes; a context is red; a thread is still open.
    let mut review = Review::request("@acme/payments", false);
    review.submit(ReviewVerdict::RequestChanges).unwrap();
    assert!(review.is_blocking());

    let ctx = MergeContext {
        green_contexts: vec!["ci/build".into()], // ci/test missing.
        current_approvals: 0,
        codeowner_review_satisfied: false,
        has_blocking_review: review.is_blocking(),
        outstanding_conversations: 2,
    };
    let gate = evaluate_ruleset(&rs, &ctx);
    assert!(!gate.is_satisfied(), "the gate blocks: {gate:?}");

    // the merge is REFUSED (0 unprotected merges to the protected ref).
    assert_eq!(
        pr.transition(PrTransition::Merge, gate.is_satisfied()),
        Err(myelin_git::lifecycle::LifecycleError::MergeGateNotSatisfied)
    );
    assert_eq!(pr.state, PrState::Open, "a blocked merge does not land");

    // the author closes the PR instead — a legal transition.
    assert_eq!(pr.transition(PrTransition::Close, false).unwrap(), PrState::Closed);
}
