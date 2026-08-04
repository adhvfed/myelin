use myelin_identity::{ObjectId, PrincipalId, RelName, RelationTuple, TupleDelta};

use crate::body::Body;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum PrState {
    Draft,
    Open,
    Merged,
    Closed,
}

impl PrState {
    pub fn is_terminal(self) -> bool {
        matches!(self, PrState::Merged)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrTransition {
    MarkReady,
    Merge,
    Close,
    Reopen,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LifecycleError {
    IllegalTransition {
        from: PrState,
        transition: PrTransition,
    },
    MergeGateNotSatisfied,
    IllegalReviewTransition {
        from: ReviewState,
    },
    IllegalThreadTransition {
        from: ThreadState,
    },
}

impl std::fmt::Display for LifecycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LifecycleError::IllegalTransition { from, transition } => {
                write!(f, "illegal PR transition {transition:?} from {from:?}")
            }
            LifecycleError::MergeGateNotSatisfied => {
                write!(
                    f,
                    "merge refused: the branch-protection gate is not satisfied"
                )
            }
            LifecycleError::IllegalReviewTransition { from } => {
                write!(f, "illegal review transition from {from:?}")
            }
            LifecycleError::IllegalThreadTransition { from } => {
                write!(f, "illegal thread transition from {from:?}")
            }
        }
    }
}

impl std::error::Error for LifecycleError {}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DiffAnchor {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullRequest {
    pub number: u64,
    pub state: PrState,
    pub base_ref: String,
    pub head_ref: String,
    pub author_pseudonym: String,
    pub body: Body,
}

impl PullRequest {
    pub fn open(
        number: u64,
        base_ref: impl Into<String>,
        head_ref: impl Into<String>,
        author_pseudonym: impl Into<String>,
        draft: bool,
    ) -> PullRequest {
        PullRequest {
            number,
            state: if draft { PrState::Draft } else { PrState::Open },
            base_ref: base_ref.into(),
            head_ref: head_ref.into(),
            author_pseudonym: author_pseudonym.into(),
            body: Body::empty(),
        }
    }

    pub fn transition(
        &mut self,
        transition: PrTransition,
        gate_satisfied: bool,
    ) -> Result<PrState, LifecycleError> {
        let next = match (self.state, transition) {
            (PrState::Draft, PrTransition::MarkReady) => PrState::Open,
            (PrState::Draft | PrState::Open, PrTransition::Merge) => {
                if !gate_satisfied {
                    return Err(LifecycleError::MergeGateNotSatisfied);
                }
                PrState::Merged
            }
            (PrState::Draft | PrState::Open, PrTransition::Close) => PrState::Closed,
            (PrState::Closed, PrTransition::Reopen) => PrState::Open,
            (from, transition) => {
                return Err(LifecycleError::IllegalTransition { from, transition })
            }
        };
        self.state = next;
        Ok(next)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ReviewVerdict {
    Approve,
    RequestChanges,
    Comment,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ReviewState {
    Requested,
    Submitted(ReviewVerdict),
    Dismissed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Review {
    pub reviewer_pseudonym: String,
    pub state: ReviewState,
    pub is_agent: bool,
}

impl Review {
    pub fn request(reviewer_pseudonym: impl Into<String>, is_agent: bool) -> Review {
        Review {
            reviewer_pseudonym: reviewer_pseudonym.into(),
            state: ReviewState::Requested,
            is_agent,
        }
    }

    pub fn submit(&mut self, verdict: ReviewVerdict) -> Result<ReviewState, LifecycleError> {
        match self.state {
            ReviewState::Requested | ReviewState::Submitted(_) => {
                self.state = ReviewState::Submitted(verdict);
                Ok(self.state)
            }
            ReviewState::Dismissed => {
                Err(LifecycleError::IllegalReviewTransition { from: self.state })
            }
        }
    }

    pub fn dismiss(&mut self) -> Result<ReviewState, LifecycleError> {
        match self.state {
            ReviewState::Submitted(_) => {
                self.state = ReviewState::Dismissed;
                Ok(self.state)
            }
            ReviewState::Requested | ReviewState::Dismissed => {
                Err(LifecycleError::IllegalReviewTransition { from: self.state })
            }
        }
    }

    pub fn is_current_approval(&self) -> bool {
        matches!(self.state, ReviewState::Submitted(ReviewVerdict::Approve))
    }

    pub fn is_blocking(&self) -> bool {
        matches!(
            self.state,
            ReviewState::Submitted(ReviewVerdict::RequestChanges)
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ThreadState {
    Open,
    Resolved,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comment {
    pub id: u128,
    pub author_pseudonym: String,
    pub body: Body,
    pub is_agent: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thread {
    pub id: u128,
    pub state: ThreadState,
    pub anchor: DiffAnchor,
    pub comments: Vec<Comment>,
}

impl Thread {
    pub fn open(id: u128, anchor: DiffAnchor, root: Comment) -> Thread {
        Thread {
            id,
            state: ThreadState::Open,
            anchor,
            comments: vec![root],
        }
    }

    pub fn reply(&mut self, comment: Comment) -> usize {
        self.comments.push(comment);
        self.comments.len()
    }

    pub fn resolve(&mut self) -> Result<ThreadState, LifecycleError> {
        match self.state {
            ThreadState::Open => {
                self.state = ThreadState::Resolved;
                Ok(self.state)
            }
            ThreadState::Resolved => {
                Err(LifecycleError::IllegalThreadTransition { from: self.state })
            }
        }
    }

    pub fn reopen(&mut self) -> Result<ThreadState, LifecycleError> {
        match self.state {
            ThreadState::Resolved => {
                self.state = ThreadState::Open;
                Ok(self.state)
            }
            ThreadState::Open => Err(LifecycleError::IllegalThreadTransition { from: self.state }),
        }
    }

    pub fn is_outstanding(&self) -> bool {
        matches!(self.state, ThreadState::Open)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BranchProtectionRuleset {
    pub ref_pattern: String,
    pub required_contexts: Vec<String>,
    pub required_approvals: u32,
    pub require_codeowner_review: bool,
    pub require_conversation_resolution: bool,
    pub allow_force_push: bool,
}

impl BranchProtectionRuleset {
    pub fn matches(&self, base_ref: &str) -> bool {
        glob_match(&self.ref_pattern, base_ref)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct MergeContext {
    pub green_contexts: Vec<String>,
    pub current_approvals: u32,
    pub codeowner_review_satisfied: bool,
    pub has_blocking_review: bool,
    pub outstanding_conversations: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RulesetOutcome {
    Satisfied,
    Blocked {
        reasons: Vec<BlockReason>,
    },
}

impl RulesetOutcome {
    pub fn is_satisfied(&self) -> bool {
        matches!(self, RulesetOutcome::Satisfied)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BlockReason {
    MissingRequiredContext(String),
    InsufficientApprovals {
        have: u32,
        need: u32,
    },
    CodeownerReviewMissing,
    BlockingReview,
    OutstandingConversations(u32),
}

pub fn evaluate_ruleset(ruleset: &BranchProtectionRuleset, ctx: &MergeContext) -> RulesetOutcome {
    let mut reasons: Vec<BlockReason> = Vec::new();

    for required in &ruleset.required_contexts {
        if !ctx.green_contexts.iter().any(|g| g == required) {
            reasons.push(BlockReason::MissingRequiredContext(required.clone()));
        }
    }

    if ctx.current_approvals < ruleset.required_approvals {
        reasons.push(BlockReason::InsufficientApprovals {
            have: ctx.current_approvals,
            need: ruleset.required_approvals,
        });
    }

    if ruleset.require_codeowner_review && !ctx.codeowner_review_satisfied {
        reasons.push(BlockReason::CodeownerReviewMissing);
    }

    if ctx.has_blocking_review {
        reasons.push(BlockReason::BlockingReview);
    }

    if ruleset.require_conversation_resolution && ctx.outstanding_conversations > 0 {
        reasons.push(BlockReason::OutstandingConversations(
            ctx.outstanding_conversations,
        ));
    }

    if reasons.is_empty() {
        RulesetOutcome::Satisfied
    } else {
        RulesetOutcome::Blocked { reasons }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeOwnerRule {
    pub pattern: String,
    pub owners: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct CodeOwners {
    pub rules: Vec<CodeOwnerRule>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodeOwnersError {
    NoOwners(usize),
    MalformedOwner(String),
}

impl std::fmt::Display for CodeOwnersError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CodeOwnersError::NoOwners(line) => {
                write!(f, "CODEOWNERS line {line}: a pattern with no owners")
            }
            CodeOwnersError::MalformedOwner(tok) => {
                write!(f, "CODEOWNERS: owner `{tok}` must start with `@`")
            }
        }
    }
}

impl std::error::Error for CodeOwnersError {}

impl CodeOwners {
    pub fn parse(content: &str) -> Result<CodeOwners, CodeOwnersError> {
        let mut rules = Vec::new();
        for (idx, raw) in content.lines().enumerate() {
            let line_no = idx + 1;
            let line = match raw.split_once('#') {
                Some((before, _)) => before,
                None => raw,
            }
            .trim();
            if line.is_empty() {
                continue;
            }
            let mut tokens = line.split_whitespace();
            let pattern = tokens
                .next()
                .expect("a non-empty trimmed line has a first token")
                .to_string();
            let owners: Vec<String> = tokens.map(|t| t.to_string()).collect();
            if owners.is_empty() {
                return Err(CodeOwnersError::NoOwners(line_no));
            }
            for owner in &owners {
                if !owner.starts_with('@') {
                    return Err(CodeOwnersError::MalformedOwner(owner.clone()));
                }
            }
            rules.push(CodeOwnerRule {
                pattern: pattern.clone(),
                owners,
            });
        }
        Ok(CodeOwners { rules })
    }

    pub fn owners_for(&self, path: &str) -> &[String] {
        for rule in self.rules.iter().rev() {
            if codeowners_path_match(&rule.pattern, path) {
                return &rule.owners;
            }
        }
        &[]
    }

    pub fn resolve(&self, repo_id: u128) -> Vec<TupleDelta> {
        let mut deltas = Vec::new();
        for rule in &self.rules {
            let object = ObjectId(format!("ref:{repo_id}::{}", rule.pattern));
            for owner in &rule.owners {
                deltas.push(TupleDelta::Add(RelationTuple {
                    object: object.clone(),
                    relation: RelName(crate::live_check::perm::CODE_OWNER.to_string()),
                    subject: PrincipalId(owner.clone()),
                    caveat: None,
                }));
            }
        }
        deltas
    }
}

fn glob_match(pattern: &str, text: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == text;
    }
    wildcard_match(pattern.as_bytes(), text.as_bytes())
}

fn codeowners_path_match(pattern: &str, path: &str) -> bool {
    let path = path.trim_start_matches('/');

    if let Some(dir) = pattern.strip_suffix('/') {
        let dir = dir.trim_start_matches('/');
        return path == dir || path.starts_with(&format!("{dir}/"));
    }

    if !pattern.contains('/') {
        let base = path.rsplit('/').next().unwrap_or(path);
        return wildcard_match(pattern.as_bytes(), base.as_bytes());
    }

    let pat = pattern.trim_start_matches('/');
    wildcard_match(pat.as_bytes(), path.as_bytes())
}

fn wildcard_match(pattern: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0usize, 0usize);
    let (mut star_p, mut star_t): (Option<usize>, usize) = (None, 0);
    let mut star_crosses_slash = false;

    while t < text.len() {
        if p < pattern.len() && (pattern[p] == text[t] || pattern[p] == b'?') {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            let double = p + 1 < pattern.len() && pattern[p + 1] == b'*';
            star_crosses_slash = double;
            p += if double { 2 } else { 1 };
            star_p = Some(p);
            star_t = t;
        } else if let Some(sp) = star_p {
            if !star_crosses_slash && text[star_t] == b'/' {
                return false;
            }
            p = sp;
            star_t += 1;
            t = star_t;
        } else {
            return false;
        }
    }

    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_pr(draft: bool) -> PullRequest {
        PullRequest::open(
            42,
            "refs/heads/main",
            "refs/heads/feature",
            "psn:alice",
            draft,
        )
    }

    fn satisfied_ctx() -> MergeContext {
        MergeContext {
            green_contexts: vec!["ci/build".into(), "ci/test".into()],
            current_approvals: 2,
            codeowner_review_satisfied: true,
            has_blocking_review: false,
            outstanding_conversations: 0,
        }
    }

    fn strict_ruleset() -> BranchProtectionRuleset {
        BranchProtectionRuleset {
            ref_pattern: "refs/heads/main".into(),
            required_contexts: vec!["ci/build".into(), "ci/test".into()],
            required_approvals: 2,
            require_codeowner_review: true,
            require_conversation_resolution: true,
            allow_force_push: false,
        }
    }

    #[test]
    fn pr_open_to_review_to_merge_is_well_formed() {
        let mut pr = a_pr(true);
        assert_eq!(pr.state, PrState::Draft);
        assert_eq!(
            pr.transition(PrTransition::MarkReady, false).unwrap(),
            PrState::Open
        );
        assert_eq!(
            pr.transition(PrTransition::Merge, true).unwrap(),
            PrState::Merged
        );
        assert_eq!(pr.state, PrState::Merged);
    }

    #[test]
    fn pr_close_then_reopen_then_close_is_well_formed() {
        let mut pr = a_pr(false);
        assert_eq!(
            pr.transition(PrTransition::Close, false).unwrap(),
            PrState::Closed
        );
        assert_eq!(
            pr.transition(PrTransition::Reopen, false).unwrap(),
            PrState::Open
        );
        assert_eq!(
            pr.transition(PrTransition::Close, false).unwrap(),
            PrState::Closed
        );
    }

    #[test]
    fn merged_pr_is_terminal_no_transition_revives_it() {
        let mut pr = a_pr(false);
        pr.transition(PrTransition::Merge, true).unwrap();
        assert!(pr.state.is_terminal());
        for t in [
            PrTransition::MarkReady,
            PrTransition::Merge,
            PrTransition::Close,
            PrTransition::Reopen,
        ] {
            assert!(
                matches!(
                    pr.transition(t, true),
                    Err(LifecycleError::IllegalTransition { .. })
                ),
                "{t:?} from Merged must be illegal"
            );
            assert_eq!(
                pr.state,
                PrState::Merged,
                "an illegal transition does NOT mutate state"
            );
        }
    }

    #[test]
    fn illegal_pr_edges_are_rejected_loudly() {
        let mut open = a_pr(false);
        assert!(matches!(
            open.transition(PrTransition::MarkReady, false),
            Err(LifecycleError::IllegalTransition {
                from: PrState::Open,
                ..
            })
        ));
        let mut draft = a_pr(true);
        assert!(matches!(
            draft.transition(PrTransition::Reopen, false),
            Err(LifecycleError::IllegalTransition {
                from: PrState::Draft,
                ..
            })
        ));
        let mut open2 = a_pr(false);
        assert!(matches!(
            open2.transition(PrTransition::Reopen, false),
            Err(LifecycleError::IllegalTransition { .. })
        ));
    }

    #[test]
    fn merge_refuses_an_unsatisfied_gate_zero_unprotected_merges() {
        let mut pr = a_pr(false);
        assert_eq!(
            pr.transition(PrTransition::Merge,  false),
            Err(LifecycleError::MergeGateNotSatisfied)
        );
        assert_eq!(
            pr.state,
            PrState::Open,
            "a refused merge does NOT land (0 unprotected merges)"
        );
    }

    #[test]
    fn review_request_submit_dismiss_is_well_formed() {
        let mut r = Review::request("psn:bob", false);
        assert_eq!(r.state, ReviewState::Requested);
        assert_eq!(
            r.submit(ReviewVerdict::Approve).unwrap(),
            ReviewState::Submitted(ReviewVerdict::Approve)
        );
        assert!(r.is_current_approval());
        r.submit(ReviewVerdict::RequestChanges).unwrap();
        assert!(r.is_blocking() && !r.is_current_approval());
        assert_eq!(r.dismiss().unwrap(), ReviewState::Dismissed);
        assert!(!r.is_current_approval() && !r.is_blocking());
    }

    #[test]
    fn illegal_review_transitions_are_rejected() {
        let mut r = Review::request("psn:bob", false);
        assert!(matches!(
            r.dismiss(),
            Err(LifecycleError::IllegalReviewTransition {
                from: ReviewState::Requested
            })
        ));
        r.submit(ReviewVerdict::Approve).unwrap();
        r.dismiss().unwrap();
        assert!(matches!(
            r.submit(ReviewVerdict::Approve),
            Err(LifecycleError::IllegalReviewTransition {
                from: ReviewState::Dismissed
            })
        ));
        assert!(matches!(
            r.dismiss(),
            Err(LifecycleError::IllegalReviewTransition { .. })
        ));
    }

    #[test]
    fn agent_reviewer_is_legible() {
        let r = Review::request("psn:agent-x", true);
        assert!(
            r.is_agent,
            "an agent reviewer carries is_agent (ADR-08 legibility - never disguised)"
        );
    }

    fn a_comment(id: u128) -> Comment {
        Comment {
            id,
            author_pseudonym: "psn:alice".into(),
            body: Body::empty(),
            is_agent: false,
        }
    }

    #[test]
    fn thread_open_reply_resolve_reopen_is_well_formed() {
        let anchor = DiffAnchor {
            path: "src/lib.rs".into(),
            start_line: 10,
            end_line: 12,
        };
        let mut t = Thread::open(1, anchor, a_comment(100));
        assert_eq!(t.state, ThreadState::Open);
        assert!(t.is_outstanding());
        assert_eq!(t.reply(a_comment(101)), 2);
        assert_eq!(t.resolve().unwrap(), ThreadState::Resolved);
        assert!(!t.is_outstanding());
        assert_eq!(t.reopen().unwrap(), ThreadState::Open);
        assert!(t.is_outstanding());
    }

    #[test]
    fn illegal_thread_transitions_are_rejected() {
        let anchor = DiffAnchor::default();
        let mut t = Thread::open(1, anchor, a_comment(100));
        assert!(matches!(
            t.reopen(),
            Err(LifecycleError::IllegalThreadTransition {
                from: ThreadState::Open
            })
        ));
        t.resolve().unwrap();
        assert!(matches!(
            t.resolve(),
            Err(LifecycleError::IllegalThreadTransition {
                from: ThreadState::Resolved
            })
        ));
    }

    #[test]
    fn ruleset_satisfied_when_all_conditions_met() {
        let outcome = evaluate_ruleset(&strict_ruleset(), &satisfied_ctx());
        assert_eq!(outcome, RulesetOutcome::Satisfied);
        assert!(outcome.is_satisfied());
    }

    #[test]
    fn ruleset_blocks_each_unmet_condition_distinctly() {
        let rs = strict_ruleset();

        let mut ctx = satisfied_ctx();
        ctx.green_contexts = vec!["ci/build".into()];
        let o = evaluate_ruleset(&rs, &ctx);
        assert!(matches!(&o, RulesetOutcome::Blocked { reasons }
            if reasons.contains(&BlockReason::MissingRequiredContext("ci/test".into()))));

        let mut ctx = satisfied_ctx();
        ctx.current_approvals = 1;
        let o = evaluate_ruleset(&rs, &ctx);
        assert!(matches!(&o, RulesetOutcome::Blocked { reasons }
            if reasons.contains(&BlockReason::InsufficientApprovals { have: 1, need: 2 })));

        let mut ctx = satisfied_ctx();
        ctx.codeowner_review_satisfied = false;
        let o = evaluate_ruleset(&rs, &ctx);
        assert!(matches!(&o, RulesetOutcome::Blocked { reasons }
            if reasons.contains(&BlockReason::CodeownerReviewMissing)));

        let mut ctx = satisfied_ctx();
        ctx.has_blocking_review = true;
        let o = evaluate_ruleset(&rs, &ctx);
        assert!(matches!(&o, RulesetOutcome::Blocked { reasons }
            if reasons.contains(&BlockReason::BlockingReview)));

        let mut ctx = satisfied_ctx();
        ctx.outstanding_conversations = 3;
        let o = evaluate_ruleset(&rs, &ctx);
        assert!(matches!(&o, RulesetOutcome::Blocked { reasons }
            if reasons.contains(&BlockReason::OutstandingConversations(3))));
    }

    #[test]
    fn protected_ref_admits_zero_unprotected_merges_end_to_end() {
        let rs = strict_ruleset();
        let mut pr = a_pr(false);
        assert!(
            rs.matches(&pr.base_ref),
            "the ruleset protects refs/heads/main"
        );

        let mut ctx = satisfied_ctx();
        ctx.current_approvals = 0;
        let gate = evaluate_ruleset(&rs, &ctx);
        assert!(!gate.is_satisfied());

        assert_eq!(
            pr.transition(PrTransition::Merge, gate.is_satisfied()),
            Err(LifecycleError::MergeGateNotSatisfied)
        );
        assert_eq!(pr.state, PrState::Open);

        let gate = evaluate_ruleset(&rs, &satisfied_ctx());
        assert!(gate.is_satisfied());
        assert_eq!(
            pr.transition(PrTransition::Merge, gate.is_satisfied())
                .unwrap(),
            PrState::Merged
        );
    }

    #[test]
    fn unprotected_ref_has_no_ruleset_match() {
        let rs = strict_ruleset();
        assert!(
            !rs.matches("refs/heads/scratch"),
            "an unprotected ref is not gated by this ruleset"
        );
    }

    #[test]
    fn pr_state_terminality_is_exact() {
        assert!(PrState::Merged.is_terminal());
        assert!(!PrState::Draft.is_terminal());
        assert!(!PrState::Open.is_terminal());
        assert!(
            !PrState::Closed.is_terminal(),
            "Closed reopens - it is NOT terminal"
        );
    }

    #[test]
    fn wildcard_matcher_exercises_backtracking_and_trailing_stars() {
        assert!(glob_match("refs/heads/feat*", "refs/heads/feature"));
        assert!(glob_match("refs/heads/*", "refs/heads/main"));
        assert!(glob_match("refs/heads/main*", "refs/heads/main"));
        assert!(glob_match("a*c*e", "abcde"));
        assert!(glob_match("a*c*e", "axxcyye"));
        assert!(!glob_match("a*c*e", "abcdf"), "no trailing e → no match");
        assert!(
            !glob_match("refs/*", "refs/heads/main"),
            "single * stops at /"
        );
        assert!(glob_match("refs/**", "refs/heads/main"));
        assert!(!glob_match("refs/heads/main", "refs/heads/mainline"));
    }

    #[test]
    fn codeowners_basename_and_anchored_matching() {
        let co = CodeOwners::parse("/build/    @a\n*.lock      @b\n/deep/**    @c\n").unwrap();
        assert_eq!(
            co.owners_for("build/out.o"),
            &["@a".to_string()],
            "anchored dir prefix"
        );
        assert_eq!(
            co.owners_for("a/b/Cargo.lock"),
            &["@b".to_string()],
            "basename glob at depth"
        );
        assert_eq!(
            co.owners_for("deep/a/b/c.txt"),
            &["@c".to_string()],
            "anchored ** crosses /"
        );
        assert!(
            co.owners_for("Cargo.toml").is_empty(),
            "no rule matches → unowned"
        );
    }

    #[test]
    fn ref_pattern_glob_matches() {
        let rs = BranchProtectionRuleset {
            ref_pattern: "refs/heads/release/*".into(),
            ..strict_ruleset()
        };
        assert!(rs.matches("refs/heads/release/1.0"));
        assert!(
            !rs.matches("refs/heads/release/1.0/hotfix"),
            "single * does not cross /"
        );
        let rs2 = BranchProtectionRuleset {
            ref_pattern: "refs/heads/release/**".into(),
            ..strict_ruleset()
        };
        assert!(rs2.matches("refs/heads/release/1.0/hotfix"), "** crosses /");
    }

    const FIXTURE: &str = "\
# Default owners for everything (a comment)
*               @acme/core-team

# JS / TS owned by the frontend team
*.ts            @acme/frontend

# the payments dir is owned by payments + a named human
/src/payments/  @acme/payments @alice

# docs
/docs/**        @acme/writers
";

    #[test]
    fn codeowners_parse_is_correct() {
        let co = CodeOwners::parse(FIXTURE).expect("valid CODEOWNERS");
        assert_eq!(co.rules.len(), 4, "comments + blanks skipped; 4 real rules");
        assert_eq!(co.rules[0].pattern, "*");
        assert_eq!(co.rules[0].owners, vec!["@acme/core-team"]);
        assert_eq!(co.rules[2].owners, vec!["@acme/payments", "@alice"]);
    }

    #[test]
    fn codeowners_resolves_paths_last_match_wins_zero_mis_resolved() {
        let co = CodeOwners::parse(FIXTURE).unwrap();
        assert_eq!(
            co.owners_for("README.adoc"),
            &["@acme/core-team".to_string()]
        );
        assert_eq!(co.owners_for("web/app.ts"), &["@acme/frontend".to_string()]);
        assert_eq!(
            co.owners_for("src/payments/charge.ts"),
            &["@acme/payments".to_string(), "@alice".to_string()]
        );
        assert_eq!(
            co.owners_for("src/payments/charge.rs"),
            &["@acme/payments".to_string(), "@alice".to_string()]
        );
        assert_eq!(
            co.owners_for("docs/guide/intro.md"),
            &["@acme/writers".to_string()]
        );
        assert_eq!(
            co.owners_for("src/core/lib.rs"),
            &["@acme/core-team".to_string()]
        );
    }

    #[test]
    fn codeowners_rejects_malformed_lines_loudly() {
        assert_eq!(
            CodeOwners::parse("*.rs\n"),
            Err(CodeOwnersError::NoOwners(1))
        );
        assert!(matches!(
            CodeOwners::parse("*.rs alice@example.com\n"),
            Err(CodeOwnersError::MalformedOwner(_))
        ));
    }

    #[test]
    fn codeowners_resolves_to_code_owner_relation_tuples_4_9() {
        let co = CodeOwners::parse(FIXTURE).unwrap();
        let deltas = co.resolve( 7);
        assert_eq!(deltas.len(), 5);
        for d in &deltas {
            match d {
                TupleDelta::Add(t) => {
                    assert_eq!(t.relation.0, crate::live_check::perm::CODE_OWNER);
                    assert!(
                        t.object.0.starts_with("ref:7::"),
                        "ref-pattern-scoped object"
                    );
                    assert!(t.subject.0.starts_with('@'), "owner handle subject");
                    assert!(t.caveat.is_none());
                }
                TupleDelta::Remove(_) => panic!("resolve emits only Add deltas"),
            }
        }
        let payments: Vec<&str> = deltas
            .iter()
            .filter_map(|d| match d {
                TupleDelta::Add(t) if t.object.0 == "ref:7::/src/payments/" => {
                    Some(t.subject.0.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(payments, vec!["@acme/payments", "@alice"]);
    }
}
