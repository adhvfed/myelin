//! # `lifecycle` — the PR/review/inline-thread lifecycle + branch-protection rulesets + the
//! CODEOWNERS resolver (GIT-P16 / P-277, M3-G3)
//!
//! This is the M3-G3 **domain-entities half** of Git hosting: the hosting-layer entities not in git
//! itself (00-overview §1.1) — the **Pull Request** lifecycle, **Reviews + inline comment THREADS**,
//! **branch-protection rulesets**, and the **CODEOWNERS resolver** — all on the control-plane OLTP
//! (one DB, RLS, per-subject DEK for free-text bodies). The body content + its myelin-content
//! round-trip is the **GIT-P17** follow-on; the diff line-anchor resolver is **GIT-P23/GIT-P24**.
//!
//! **Owning architecture docs (read in full before changing this):**
//! - `00-overview.md` §1.1 (the PR lifecycle, reviews, inline comment threads, CODEOWNERS,
//!   branch-protection rulesets — the hosting-layer domain entities Git OWNS).
//! - `03-events-contracts-and-glue.md` §1 (the `git.pr.*` / `git.review.*` / `git.comment.*` /
//!   `git.thread.*` event taxonomy these transitions emit — the tokens are registered in
//!   [`crate::events`]; the emit body rides each transition here), §5.2 (the frozen ReBAC fragment —
//!   `pull_request{author,reviewer,view,review,merge}`, `ref{code_owner,bypass,push_protected}`,
//!   `repo{approve_untrusted_ci}` — the relations CODEOWNERS compiles to).
//! - `01-tech-and-data-model.md` §4.3 (the `pull_request` / `review` / `review_comment` OLTP rows —
//!   the skeletal tag-carriers in [`crate::schema`] this layer makes live).
//!
//! **Contracts implemented (to the frozen shapes):**
//! - **4.9** the **CODEOWNERS-as-relations consumer** (OWNED — the resolver compiling CODEOWNERS path
//!   patterns to the `code_owner` reviewer-requirement relation, §5.2). This is the GIT-P16
//!   deliverable's contract row: a CODEOWNERS file → a set of [`myelin_identity::TupleDelta`] writing
//!   `ref:<repo>::<glob>#code_owner@<owner>` tuples, so "who must approve this path" is the ordinary
//!   `list_subjects(ref, code_owner)` Expand the merge gate already runs ([`crate::live_check::GitCheckGate::code_owners`]).
//!   The resolver compiles; the live tuple WRITE rides `write_tuples` (4.6) — that path exists in
//!   [`crate::live_check::GitCheckGate::grant_relation`].
//!
//! ## What this prompt (GIT-P16 / P-277) ships — and what it deliberately does NOT (VISION §3)
//! **Ships:**
//! 1. [`PullRequest`] + [`PrState`] — the PR lifecycle state machine (open → review → merge/close;
//!    draft → ready; closed → reopened), with **well-formed transitions only** (0 illegal: every
//!    [`PullRequest::transition`] either advances a legal edge or returns [`LifecycleError`]).
//! 2. [`Review`] + [`ReviewState`] + [`ReviewVerdict`] — review request → submit(verdict) → dismiss.
//! 3. [`Thread`] + [`ThreadState`] — the inline comment THREAD entity (the thread root + its comment
//!    membership; open → resolved → reopened). **The thread ENTITY** — the body content is GIT-P17,
//!    the diff line-anchor is GIT-P23/P24.
//! 4. [`BranchProtectionRuleset`] + [`evaluate_ruleset`] — the entity-layer branch-protection gate: a
//!    protected `base_ref` requires the ruleset's conditions (required contexts green, required
//!    approvals met, CODEOWNERS review satisfied, no un-dismissed stale review), and **0 unprotected
//!    merges to a protected ref** pass the gate.
//! 5. [`CodeOwners`] — the CODEOWNERS file parser + matcher + the **resolver** compiling each path
//!    pattern to a [`myelin_identity::TupleDelta`] `code_owner` tuple (the 4.9 consumer).
//!
//! **Does NOT ship (FLOORS named — VISION §3):**
//! - **PR/review/thread bodies are single-author CAS** here. As of **GIT-P17 / P-278** the body content
//!   IS the frozen [`crate::body::Body`] (`myelin-content` markdown-subset + the three structured inline
//!   nodes, round-tripped `render(parse(md)) === md`, with the content-node → `refs.edge.created`
//!   emission) — the GIT-P16 opaque `BodyRef` ciphertext floor is RESOLVED for the body content. The
//!   per-subject-DEK at-rest SEAL of those bytes (contract 11.4 `erasure = CryptoShred`) rides the
//!   GIT-P20 store wiring; this layer carries the cleartext document. The multi-author
//!   collaborative-edit story is owned by **Knowledge**, not git (§1.1 — git PR/review bodies are
//!   single-author).
//! - **The diff line-anchor** (the `#L<a>-L<b>` content-anchored 4-state resolver a thread anchors to)
//!   is **GIT-P23/GIT-P24** — this layer carries the anchor as an opaque [`DiffAnchor`] handle.
//! - **The live OLTP store + migration + the per-ref ruleset persistence** ride GIT-P20/GIT-P22 (the
//!   merge gate + the required-set policy store). This module is the **entity layer + the pure
//!   transition/evaluation/resolution logic** — the in-memory state machines + the CODEOWNERS compile,
//!   unit- and e2e-tested against the in-memory entities. No DB read/write is done here; the live
//!   `write_tuples` CODEOWNERS tuple persistence is the GIT-P20 wiring (it consumes [`CodeOwners::resolve`]).

use myelin_identity::{ObjectId, PrincipalId, RelName, RelationTuple, TupleDelta};

use crate::body::Body;

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 1. THE PULL-REQUEST LIFECYCLE STATE MACHINE (00-overview §1.1)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// The pull-request lifecycle state (00-overview §1.1 — the PR lifecycle). The **closed set** of PR
/// states; a PR is in exactly one. The transitions between them are the only legal edges
/// ([`PullRequest::transition`]) — 0 illegal transitions is the gate.
///
/// - `Draft` — opened as a draft (work-in-progress; not yet review-ready). Maps to the `git.pr.opened`
///   event with `is_draft = true`.
/// - `Open` — open and ready for review (`marked_ready` from `Draft`, or opened ready). The state the
///   merge gate evaluates against.
/// - `Merged` — TERMINAL: the PR's head landed on the base via the merge gate (`git.pr.merged`).
/// - `Closed` — the PR was closed without merging (`git.pr.closed`); may be `Reopen`ed back to `Open`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PrState {
    /// A draft PR (work-in-progress, not review-ready). `git.pr.opened` with `is_draft`.
    Draft,
    /// Open and ready for review — the state the merge gate evaluates. `git.pr.marked_ready` /
    /// `git.pr.reopened` land here.
    Open,
    /// TERMINAL — merged via the gate. `git.pr.merged`.
    Merged,
    /// Closed without merge; reopenable. `git.pr.closed`.
    Closed,
}

impl PrState {
    /// A **terminal** state has no legal outgoing transition that can REOPEN it for landing again —
    /// `Merged` is fully terminal (a merged PR is never reopened; a new PR is opened instead).
    /// `Closed` is NOT terminal (it reopens). Used by [`PullRequest::transition`] to reject reviving a
    /// merged PR (0 illegal transitions).
    pub fn is_terminal(self) -> bool {
        matches!(self, PrState::Merged)
    }
}

/// A transition request against a [`PullRequest`] (the verb the lifecycle applies). Each maps to a
/// `git.pr.*` event token ([`crate::events`]); the emit body rides the applied transition (the emit
/// seam is GIT-P20's outbox wiring — here the transition is the pure entity-layer state change).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrTransition {
    /// `Draft → Open` — a draft is marked ready for review (`git.pr.marked_ready`).
    MarkReady,
    /// `{Draft|Open} → Merged` — the head lands on the base via the merge gate (`git.pr.merged`). Only
    /// legal from a non-terminal, non-closed state and only when the merge gate is satisfied (the
    /// `gate_satisfied` guard — the entity layer NEVER admits an unguarded merge).
    Merge,
    /// `{Draft|Open} → Closed` — closed without merging (`git.pr.closed`).
    Close,
    /// `Closed → Open` — a closed PR is reopened (`git.pr.reopened`). A MERGED PR is never reopened.
    Reopen,
}

/// A loud, typed lifecycle error — an illegal transition is NEVER silently coerced (the
/// 0-illegal-transitions gate; a mutant that drops a guard surfaces here, not as a wrong state).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LifecycleError {
    /// The requested transition has no legal edge from the current state (e.g. `Merged → Open`, or
    /// `MarkReady` on an already-`Open` PR). Carries the offending `(from, transition)` for the loud
    /// audit (never a silent no-op).
    IllegalTransition {
        /// The state the entity was in.
        from: PrState,
        /// The transition that was illegal from `from`.
        transition: PrTransition,
    },
    /// A `Merge` transition was attempted but the branch-protection / merge gate is NOT satisfied. The
    /// entity layer refuses to land an unguarded merge (the 0-unprotected-merges gate) — a `Merge`
    /// from a legal source state still requires `gate_satisfied = true`.
    MergeGateNotSatisfied,
    /// A review verdict transition was illegal (e.g. submitting a verdict on an already-dismissed
    /// review, or dismissing a not-yet-submitted one).
    IllegalReviewTransition {
        /// The review state the entity was in.
        from: ReviewState,
    },
    /// A thread transition was illegal (e.g. resolving an already-resolved thread).
    IllegalThreadTransition {
        /// The thread state the entity was in.
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
                write!(f, "merge refused: the branch-protection gate is not satisfied")
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

/// An opaque diff line-anchor handle a [`Thread`] anchors to (the `#L<a>-L<b>` content-anchored
/// range). The 4-state content-anchored resolver (`live/moved/outdated/gone`) is GIT-P23/GIT-P24; this
/// layer carries the anchor as an opaque handle (the thread ENTITY, not the resolved position).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DiffAnchor {
    /// The path the anchor is in (e.g. `src/payments/charge.rs`) — the resolver re-anchors against it.
    pub path: String,
    /// The 1-based start line at anchor time.
    pub start_line: u32,
    /// The 1-based end line at anchor time (`end >= start`).
    pub end_line: u32,
}

/// The **pull-request entity** (00-overview §1.1; the `pull_request` OLTP row, 01 §4.3). The lifecycle
/// state machine carrier: a PR has a `state`, a stable `number`, the `base_ref`/`head_ref` it spans, an
/// `author_pseudonym` (GIT-1, never a raw identity — [`crate::schema`] tags it), and an opaque body
/// handle (the GIT-P17 floor). The transitions ([`PullRequest::transition`]) are the only legal state
/// changes (0 illegal).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullRequest {
    /// The per-repo PR number — git's STABLE canonical key (REF-3; the `#sub` root is
    /// `git/pr/<repo>:<number>`, [`crate::subs`]). Never a render-time display form.
    pub number: u64,
    /// The current lifecycle state (exactly one of the closed set).
    pub state: PrState,
    /// The base ref the PR targets (e.g. `refs/heads/main`) — the ref the branch-protection ruleset
    /// keys on ([`BranchProtectionRuleset::matches`]).
    pub base_ref: String,
    /// The head ref the PR proposes to land.
    pub head_ref: String,
    /// The PR author's OPAQUE pseudonym (GIT-1, contract 4.8) — never a raw name/email.
    pub author_pseudonym: String,
    /// The PR description body — a frozen [`crate::body::Body`] (`myelin-content` markdown-subset + the
    /// three structured inline nodes; single-author CAS; the content-node → `refs.edge.created`
    /// producer, GIT-P17). The per-subject-DEK at-rest seal of these bytes rides the GIT-P20 store.
    pub body: Body,
}

impl PullRequest {
    /// Open a new PR (`git.pr.opened`). `draft` chooses the initial state (`Draft` vs `Open`). This is
    /// the lifecycle's INITIAL state — a PR always starts `Draft` or `Open`, never `Merged`/`Closed`.
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

    /// **Apply a lifecycle transition (the 0-illegal-transitions gate).** Returns the resulting state
    /// on a legal edge, or [`LifecycleError`] on an illegal one — NEVER silently coerces. The legal
    /// edge table (00-overview §1.1):
    ///
    /// | from | `MarkReady` | `Merge` (gate ✓) | `Close` | `Reopen` |
    /// |------|-------------|------------------|---------|----------|
    /// | `Draft`  | → `Open`  | → `Merged`       | → `Closed` | ✗ |
    /// | `Open`   | ✗ (already ready) | → `Merged` | → `Closed` | ✗ |
    /// | `Merged` | ✗ (terminal) | ✗ | ✗ | ✗ |
    /// | `Closed` | ✗ | ✗ | ✗ | → `Open` |
    ///
    /// `gate_satisfied` is the branch-protection / merge-gate outcome ([`evaluate_ruleset`] →
    /// [`RulesetOutcome::Satisfied`]) — a `Merge` from a legal source still REFUSES on an unsatisfied
    /// gate (`MergeGateNotSatisfied`), so **0 unprotected merges** land. A non-`Merge` transition
    /// ignores `gate_satisfied`.
    pub fn transition(
        &mut self,
        transition: PrTransition,
        gate_satisfied: bool,
    ) -> Result<PrState, LifecycleError> {
        let next = match (self.state, transition) {
            // Draft → Open (mark ready).
            (PrState::Draft, PrTransition::MarkReady) => PrState::Open,
            // {Draft|Open} → Merged — only if the gate is satisfied (the protected-ref guard).
            (PrState::Draft | PrState::Open, PrTransition::Merge) => {
                if !gate_satisfied {
                    return Err(LifecycleError::MergeGateNotSatisfied);
                }
                PrState::Merged
            }
            // {Draft|Open} → Closed.
            (PrState::Draft | PrState::Open, PrTransition::Close) => PrState::Closed,
            // Closed → Open (reopen). A MERGED PR is never reopened (terminal).
            (PrState::Closed, PrTransition::Reopen) => PrState::Open,
            // Everything else is an illegal edge — loud, never a silent no-op.
            (from, transition) => {
                return Err(LifecycleError::IllegalTransition { from, transition })
            }
        };
        self.state = next;
        Ok(next)
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 2. THE REVIEW LIFECYCLE (00-overview §1.1; `git.review.*`)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// A review verdict (`git.review.submitted` carries this + `is_agent`). The closed set: a submitted
/// review is exactly one of approve / request-changes / comment-only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReviewVerdict {
    /// The reviewer approves — counts toward the ruleset's `required_approvals` AND satisfies a
    /// CODEOWNERS requirement if the reviewer is an owner ([`BranchProtectionRuleset`]).
    Approve,
    /// The reviewer requests changes — BLOCKS the merge gate until dismissed or superseded.
    RequestChanges,
    /// A comment-only review — neither approves nor blocks (a non-binding review).
    Comment,
}

/// The review lifecycle state (00-overview §1.1). `Requested → Submitted(verdict) → Dismissed`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReviewState {
    /// A review was REQUESTED (`git.review.requested`) — pending the reviewer's verdict.
    Requested,
    /// A review was SUBMITTED with a verdict (`git.review.submitted`).
    Submitted(ReviewVerdict),
    /// A submitted review was DISMISSED (`git.review.dismissed`) — e.g. a stale review dismissed on a
    /// new push when the ruleset's `dismiss_stale` is set. A dismissed review counts for nothing.
    Dismissed,
}

/// The **review entity** (00-overview §1.1; the `review` OLTP row, 01 §4.3). Carries the lifecycle
/// state + the reviewer's OPAQUE pseudonym + the agent-legibility flag (ADR-08: an agent reviewer is
/// never disguised as a human — `is_agent` rides `git.review.submitted`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Review {
    /// The reviewer's OPAQUE pseudonym (GIT-1, contract 4.8).
    pub reviewer_pseudonym: String,
    /// The review lifecycle state.
    pub state: ReviewState,
    /// Agent legibility (ADR-08 / AI-Act): `true` iff the reviewer is an agent — rendered visually
    /// distinct with provenance, never disguised as human (`review.is_agent`, arch §7).
    pub is_agent: bool,
}

impl Review {
    /// Request a review from a reviewer (`git.review.requested`) — the INITIAL review state.
    pub fn request(reviewer_pseudonym: impl Into<String>, is_agent: bool) -> Review {
        Review {
            reviewer_pseudonym: reviewer_pseudonym.into(),
            state: ReviewState::Requested,
            is_agent,
        }
    }

    /// **Submit a verdict (`git.review.submitted`).** Legal only from `Requested` OR from a prior
    /// `Submitted` (a reviewer may revise their verdict). Illegal from `Dismissed` (a dismissed review
    /// is re-requested, not re-submitted in place). Returns the new state or [`LifecycleError`].
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

    /// **Dismiss the review (`git.review.dismissed`).** Legal only from `Submitted` (you dismiss a
    /// verdict, not a pending request — a pending request is just left un-submitted). Illegal from
    /// `Requested` or `Dismissed`.
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

    /// Is this review a CURRENT, binding approval (counts toward `required_approvals` + a CODEOWNERS
    /// satisfaction)? Only a `Submitted(Approve)` that is not dismissed.
    pub fn is_current_approval(&self) -> bool {
        matches!(self.state, ReviewState::Submitted(ReviewVerdict::Approve))
    }

    /// Is this review CURRENTLY blocking (a live request-changes)? Blocks the merge gate.
    pub fn is_blocking(&self) -> bool {
        matches!(self.state, ReviewState::Submitted(ReviewVerdict::RequestChanges))
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 3. THE INLINE COMMENT-THREAD ENTITY (00-overview §1.1; `git.comment.*` / `git.thread.*`)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// The thread lifecycle state (00-overview §1.1 — inline comment threads). `Open → Resolved →
/// (reopen) Open`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ThreadState {
    /// The thread is OPEN (unresolved) — counts as an outstanding conversation.
    Open,
    /// The thread is RESOLVED (`git.thread.resolved`) — the conversation is closed; reopenable.
    Resolved,
}

/// A single inline comment in a thread (`git.comment.created`). The body is the frozen
/// [`crate::body::Body`] (`myelin-content` markdown-subset + the three structured inline nodes;
/// single-author CAS; the content-node → `refs.edge.created` producer, GIT-P17). The stable
/// `#comment-<id>` opaque id is the mint [`crate::subs::mint_pr_comment`] produces; here it is carried
/// as a `u128`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comment {
    /// The stable opaque comment id (`#comment-<id>`, 5.7) — survives edits.
    pub id: u128,
    /// The comment author's OPAQUE pseudonym (GIT-1).
    pub author_pseudonym: String,
    /// The comment body — a frozen [`crate::body::Body`] (the content-node → `refs.edge.created`
    /// producer, GIT-P17). The per-subject-DEK at-rest seal rides the GIT-P20 store.
    pub body: Body,
    /// `true` iff the comment author is an agent (ADR-08 legibility — labelled, never disguised).
    pub is_agent: bool,
}

/// The **inline review-thread entity** (00-overview §1.1 — "Reviews + inline comment threads"). The
/// THREAD root (`#thread-<id>`, 5.7) + its ordered comment membership + the diff line-anchor it hangs
/// on. The body content of each comment is GIT-P17; the line-anchor 4-state resolution is GIT-P23/P24
/// (here the anchor is an opaque [`DiffAnchor`] handle). The thread lifecycle (open/resolved) is the
/// gate's "0 outstanding required conversations" input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thread {
    /// The stable opaque thread-root id (`#thread-<id>`, 5.7).
    pub id: u128,
    /// The thread lifecycle state.
    pub state: ThreadState,
    /// The diff line-anchor the thread hangs on (opaque handle; the 4-state resolver is GIT-P23/P24).
    pub anchor: DiffAnchor,
    /// The thread's ordered comments (the root comment + replies). At least one (the root).
    pub comments: Vec<Comment>,
}

impl Thread {
    /// Open a new thread with its root comment (`git.comment.created` + the thread root). The INITIAL
    /// thread state is `Open`.
    pub fn open(id: u128, anchor: DiffAnchor, root: Comment) -> Thread {
        Thread {
            id,
            state: ThreadState::Open,
            anchor,
            comments: vec![root],
        }
    }

    /// Append a reply comment to the thread (`git.comment.created`). A reply may be added to an open
    /// OR resolved thread (a resolved thread can still receive a follow-up, which is the usual signal
    /// to reopen it; the caller reopens explicitly). Returns the new comment count.
    pub fn reply(&mut self, comment: Comment) -> usize {
        self.comments.push(comment);
        self.comments.len()
    }

    /// **Resolve the thread (`git.thread.resolved`).** Legal only from `Open`. Illegal from
    /// `Resolved` (idempotent re-resolve is rejected loudly, not silently coerced).
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

    /// **Reopen a resolved thread.** Legal only from `Resolved`. Illegal from `Open`.
    pub fn reopen(&mut self) -> Result<ThreadState, LifecycleError> {
        match self.state {
            ThreadState::Resolved => {
                self.state = ThreadState::Open;
                Ok(self.state)
            }
            ThreadState::Open => {
                Err(LifecycleError::IllegalThreadTransition { from: self.state })
            }
        }
    }

    /// Is this thread an OUTSTANDING (unresolved) conversation? The ruleset may require 0 outstanding
    /// threads to merge ([`BranchProtectionRuleset::require_conversation_resolution`]).
    pub fn is_outstanding(&self) -> bool {
        matches!(self.state, ThreadState::Open)
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 4. THE BRANCH-PROTECTION RULESET (00-overview §1.1; the entity-layer gate)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// A **branch-protection ruleset** (00-overview §1.1 — "Branch-protection rulesets"). The entity Git
/// owns that decides "what is allowed to land" on a protected `base_ref`: which contexts gate
/// (`required_contexts` — Git's `required`-set policy, the [`crate::check_status::RequiredSetPolicy`]
/// the merge gate evaluates), how many approvals, whether a CODEOWNERS review is required, whether
/// outstanding conversations block, and the bypass list. **Git decides which facts gate — CI only
/// reports them** (00-overview §1.1, X-1).
///
/// This is the ENTITY (the ruleset row); the per-ref persistence + the live `git.branch.protection_changed`
/// emit ride GIT-P20/GIT-P22. The pure ENTITY-LAYER evaluation ([`evaluate_ruleset`]) is the gate the
/// 0-unprotected-merges drill exercises.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchProtectionRuleset {
    /// The ref-pattern this ruleset protects (e.g. `refs/heads/main`, `refs/heads/release/*`). A PR's
    /// `base_ref` matches via [`BranchProtectionRuleset::matches`] (glob, last-segment `*`/`**`).
    pub ref_pattern: String,
    /// The CI contexts that MUST be green (with an acceptable trust posture) — Git's `required`-set
    /// policy (X-1). The merge gate reads the [`crate::check_status`] projection; here the ruleset
    /// NAMES which contexts gate (Git decides, CI reports).
    pub required_contexts: Vec<String>,
    /// The minimum number of CURRENT approving reviews required to merge (`>= 0`).
    pub required_approvals: u32,
    /// `true` iff a CODEOWNERS review is required — every CODEOWNERS-matched path on the PR must have a
    /// current approval from one of its `code_owner`s ([`CodeOwners`]). The merge gate resolves the
    /// owner set via `list_subjects(ref, code_owner)`.
    pub require_codeowner_review: bool,
    /// `true` iff outstanding (unresolved) conversation threads BLOCK the merge.
    pub require_conversation_resolution: bool,
    /// `true` iff a force-push to the protected ref is allowed (default `false` — a protected ref is
    /// not force-pushed without a bypass; the receive-pack push policy, [`crate::receive_pack`],
    /// enforces the push half).
    pub allow_force_push: bool,
}

impl BranchProtectionRuleset {
    /// Does this ruleset's pattern protect `base_ref`? Matches exact OR a trailing-glob pattern
    /// (`refs/heads/release/*` matches `refs/heads/release/1.0`; `**` matches across `/`). Mirrors the
    /// ref-glob `ref` object scope the ReBAC fragment keys on (§5.2 — the ruleset and the
    /// `ref.push_protected` relation share the ref-pattern scope).
    pub fn matches(&self, base_ref: &str) -> bool {
        glob_match(&self.ref_pattern, base_ref)
    }
}

/// The set of facts the entity-layer gate evaluates a [`BranchProtectionRuleset`] against — the
/// CURRENT PR review/thread/check state at merge time. Built by the merge gate (GIT-P20) from the
/// live entities + the [`crate::check_status`] projection; here it is the evaluation INPUT.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct MergeContext {
    /// The set of CI contexts that are CURRENTLY green-and-acceptable for the PR head (Git reads its
    /// own [`crate::check_status`] projection — `gate_outcome` is the per-context source; here we pass
    /// the resolved green set). A required context absent from this set blocks.
    pub green_contexts: Vec<String>,
    /// The count of CURRENT approving reviews (`Submitted(Approve)`, not dismissed).
    pub current_approvals: u32,
    /// `true` iff every CODEOWNERS-required path on the PR has a current approval from one of its
    /// owners (the merge gate resolves this via the `code_owner` `list_subjects` Expand + the review
    /// set). The ruleset's `require_codeowner_review` only gates if this is `false`.
    pub codeowner_review_satisfied: bool,
    /// `true` iff there is at least one CURRENT request-changes review blocking the PR.
    pub has_blocking_review: bool,
    /// The number of OUTSTANDING (unresolved) conversation threads.
    pub outstanding_conversations: u32,
}

/// The entity-layer branch-protection gate outcome. Either the ruleset is SATISFIED (the merge may
/// proceed — the lifecycle's `Merge` transition is admitted), or it is BLOCKED with the SPECIFIC
/// unmet reasons (humanised into the PR checks panel by Notif — never a raw string). Loud + typed: a
/// blocked merge surfaces exactly why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RulesetOutcome {
    /// Every condition the ruleset imposes is met — the merge is admitted (0 unprotected merges: a
    /// `Satisfied` is the ONLY outcome that lets `PrTransition::Merge` land).
    Satisfied,
    /// At least one condition is unmet — the merge is BLOCKED. Carries the specific unmet reasons.
    Blocked {
        /// The specific reasons the gate blocked (≥ 1).
        reasons: Vec<BlockReason>,
    },
}

impl RulesetOutcome {
    /// `true` exactly when the gate is satisfied — the `gate_satisfied` the `Merge` transition reads.
    pub fn is_satisfied(&self) -> bool {
        matches!(self, RulesetOutcome::Satisfied)
    }
}

/// A specific reason the branch-protection gate blocked a merge (humanisable; never a raw string).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockReason {
    /// A required CI context is not currently green-and-acceptable. Carries the context name.
    MissingRequiredContext(String),
    /// Fewer current approvals than the ruleset requires. Carries `(have, need)`.
    InsufficientApprovals {
        /// The current approval count.
        have: u32,
        /// The required approval count.
        need: u32,
    },
    /// A CODEOWNERS review is required but not satisfied (a CODEOWNERS-matched path lacks an owner's
    /// current approval).
    CodeownerReviewMissing,
    /// A current request-changes review is blocking the merge.
    BlockingReview,
    /// Outstanding (unresolved) conversation threads block the merge. Carries the count.
    OutstandingConversations(u32),
}

/// **THE ENTITY-LAYER BRANCH-PROTECTION GATE (00-overview §1.1; the 0-unprotected-merges drill).**
///
/// Evaluate a [`BranchProtectionRuleset`] against the CURRENT [`MergeContext`]. Returns
/// [`RulesetOutcome::Satisfied`] iff EVERY condition holds, else [`RulesetOutcome::Blocked`] with the
/// specific unmet reasons. The lifecycle's `Merge` transition reads `outcome.is_satisfied()` as its
/// `gate_satisfied` guard, so **a protected base_ref with any unmet condition admits 0 merges**.
///
/// The conditions (all must hold for `Satisfied`):
/// 1. every `required_contexts` entry is in the merge context's `green_contexts` (Git's required-set,
///    X-1 — Git decides which contexts gate; CI reports the facts);
/// 2. `current_approvals >= required_approvals`;
/// 3. if `require_codeowner_review`, then `codeowner_review_satisfied`;
/// 4. no `has_blocking_review` (a live request-changes blocks);
/// 5. if `require_conversation_resolution`, then `outstanding_conversations == 0`.
pub fn evaluate_ruleset(
    ruleset: &BranchProtectionRuleset,
    ctx: &MergeContext,
) -> RulesetOutcome {
    let mut reasons: Vec<BlockReason> = Vec::new();

    // 1. required contexts — Git's required-set policy (X-1). Each required context must be green.
    for required in &ruleset.required_contexts {
        if !ctx.green_contexts.iter().any(|g| g == required) {
            reasons.push(BlockReason::MissingRequiredContext(required.clone()));
        }
    }

    // 2. required approvals.
    if ctx.current_approvals < ruleset.required_approvals {
        reasons.push(BlockReason::InsufficientApprovals {
            have: ctx.current_approvals,
            need: ruleset.required_approvals,
        });
    }

    // 3. CODEOWNERS review.
    if ruleset.require_codeowner_review && !ctx.codeowner_review_satisfied {
        reasons.push(BlockReason::CodeownerReviewMissing);
    }

    // 4. a live request-changes blocks unconditionally.
    if ctx.has_blocking_review {
        reasons.push(BlockReason::BlockingReview);
    }

    // 5. conversation resolution.
    if ruleset.require_conversation_resolution && ctx.outstanding_conversations > 0 {
        reasons.push(BlockReason::OutstandingConversations(ctx.outstanding_conversations));
    }

    if reasons.is_empty() {
        RulesetOutcome::Satisfied
    } else {
        RulesetOutcome::Blocked { reasons }
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 5. THE CODEOWNERS RESOLVER (contract 4.9 — CODEOWNERS-as-relations)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// One parsed CODEOWNERS rule: a path PATTERN + the owner subjects required to review a change touching
/// a matched path (§5.2 — the CODEOWNERS path-glob → reviewer-requirement). Owners are opaque subject
/// identifiers (`@alice` → a `user` principal, `@team/payments` → a `team#member` userset) — Git does
/// not resolve the handle to a pseudonym here; Identity owns the handle→principal map (the resolver
/// compiles to the `code_owner` RELATION, and Identity's Expand resolves the membership at query time).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeOwnerRule {
    /// The CODEOWNERS path pattern (e.g. `/src/payments/`, `*.rs`, `/docs/**`).
    pub pattern: String,
    /// The owner subject identifiers (`@alice`, `@acme/payments`) — the required reviewers for a
    /// matched path. At least one (a rule with no owners is a SYNTAX error rejected at parse).
    pub owners: Vec<String>,
}

/// A parsed **CODEOWNERS file** (00-overview §1.1; contract 4.9). The ordered rule list (LAST match
/// wins — GitHub CODEOWNERS semantics: the last matching pattern in the file determines ownership of a
/// path). The resolver ([`CodeOwners::resolve`]) compiles the rules to `code_owner` relation tuples;
/// the matcher ([`CodeOwners::owners_for`]) answers "who owns this path" at parse-fixture time (the
/// 0-mis-resolved-owners gate).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct CodeOwners {
    /// The ordered rules (file order; last match wins).
    pub rules: Vec<CodeOwnerRule>,
}

/// A CODEOWNERS parse error — loud, never silently dropped (a malformed CODEOWNERS line must not
/// silently grant nobody / everybody — fail the parse).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodeOwnersError {
    /// A non-comment, non-blank line had a pattern but NO owners (e.g. `*.rs` with nothing after) —
    /// an ambiguous "unowned" line. Carries the 1-based line number.
    NoOwners(usize),
    /// An owner token did not start with `@` (the CODEOWNERS owner sigil) — Git owners are `@handle`
    /// or `@org/team` (an email-form owner is NOT supported in this slice; named as a floor). Carries
    /// the offending token.
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
    /// **Parse a CODEOWNERS file** (00-overview §1.1). The grammar: one rule per non-blank,
    /// non-comment line — a path pattern followed by one-or-more `@owner` tokens, whitespace-separated.
    /// `#` starts a comment (to end of line). Blank lines are skipped. A pattern with no owners, or a
    /// non-`@` owner token, is a LOUD parse error ([`CodeOwnersError`]) — never silently dropped.
    pub fn parse(content: &str) -> Result<CodeOwners, CodeOwnersError> {
        let mut rules = Vec::new();
        for (idx, raw) in content.lines().enumerate() {
            let line_no = idx + 1;
            // Strip a trailing comment + surrounding whitespace.
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

    /// **Who owns `path`? (the 0-mis-resolved-owners gate.)** GitHub CODEOWNERS semantics: the LAST
    /// matching rule in file order wins (a later, more-specific rule overrides an earlier one).
    /// Returns the matched rule's owners, or an empty slice if no rule matches (an unowned path). The
    /// merge gate uses this to know which `code_owner` subjects must approve.
    pub fn owners_for(&self, path: &str) -> &[String] {
        // Last match wins: iterate in reverse, return the first (= last in file order) that matches.
        for rule in self.rules.iter().rev() {
            if codeowners_path_match(&rule.pattern, path) {
                return &rule.owners;
            }
        }
        &[]
    }

    /// **THE 4.9 CODEOWNERS-AS-RELATIONS RESOLVER (contract 4.9 — the GIT-P16 deliverable).** Compile
    /// this CODEOWNERS file to the set of `code_owner` relation tuples Identity admits (§5.2 — "the
    /// resolver compiles CODEOWNERS path globs into `code_owner` relations per ref pattern"). Each rule
    /// `<pattern> @owner...` becomes one `ref:<repo>::<pattern>#code_owner@<owner>` tuple per owner, so
    /// "who must approve this path" is the ordinary `list_subjects(ref, code_owner)` Expand the merge
    /// gate runs ([`crate::live_check::GitCheckGate::code_owners`]) — NOT a bespoke glob-matcher in the
    /// hot path (the glob is baked into the `ref` object id at WRITE time, §5.2). The returned deltas
    /// are written through `write_tuples` (4.6) by the live wiring (GIT-P20); this is the pure compile.
    ///
    /// The `ref` object id is `ref:<repo_id>::<pattern>` — the ref-PATTERN-scoped object the §5.2
    /// `definition ref` keys on (mirrors [`crate::rebac_fragment::object_types::REF`]). The owner
    /// handle (`@alice` / `@acme/payments`) becomes the tuple subject verbatim (Identity owns the
    /// handle→principal resolution; the `team#member` userset resolves at Expand time).
    ///
    /// **RECONCILIATION (EI-01 §7 — extend, never duplicate).** Identity's engine crate already carries
    /// a tuple-COMPILE half (`myelin_identity_service::git_fragment::compile_codeowners`) that takes
    /// already-parsed `CodeownersRule`s and emits the SAME `ref:<repo>::<glob>#code_owner@<owner>` tuple
    /// shape. Git is a producer LEAF and CANNOT depend on the Identity SERVICE crate (the §2.9 acyclic
    /// DAG), so this resolver is the **Git-owned half** the architecture assigns it (§5.2 / 00-overview
    /// §1.1 — Git owns the CODEOWNERS file PARSE + the path MATCHER, "Git decides which glob a change
    /// matches"); it produces the byte-identical tuple encoding the engine's `compile_codeowners`
    /// produces, so a Git-written tuple is the exact tuple the engine's `list_subjects(ref, code_owner)`
    /// Expand resolves. The encoding equivalence is PINNED by the CDC (`tests/cdc_4_9_git_codeowners.rs`)
    /// — a drift on either side fails the same CI job. The live tuple WRITE rides `write_tuples` (4.6,
    /// [`crate::live_check::GitCheckGate::grant_relation`]); GIT-P20 wires the write. Here is the pure
    /// Git-owned parse → match → compile.
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

// ════════════════════════════════════════════════════════════════════════════════════════════════
// glob / path matching (one place; mirrors the ref-glob scope §5.2)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// A small glob matcher for the ref-pattern scope (`*` matches within a segment, `**` matches across
/// `/`). Used by [`BranchProtectionRuleset::matches`] for `base_ref` matching. Anchored (full-string).
fn glob_match(pattern: &str, text: &str) -> bool {
    // Fast path: an exact pattern (no glob) is a literal compare.
    if !pattern.contains('*') {
        return pattern == text;
    }
    wildcard_match(pattern.as_bytes(), text.as_bytes())
}

/// GitHub-CODEOWNERS-style path match. Semantics (the slice we support):
/// - a pattern WITHOUT a leading `/` and without a `/` matches the path's BASENAME by glob (e.g.
///   `*.rs` matches `src/lib.rs`); GitHub matches such a pattern at any depth.
/// - a pattern with a leading `/` is anchored at the repo root; a trailing `/` matches a directory
///   prefix (e.g. `/src/payments/` matches `src/payments/charge.rs`).
/// - `*` matches within a path segment, `**` matches across `/`.
fn codeowners_path_match(pattern: &str, path: &str) -> bool {
    let path = path.trim_start_matches('/');

    // Directory pattern: a trailing `/` means "this dir and everything under it".
    if let Some(dir) = pattern.strip_suffix('/') {
        let dir = dir.trim_start_matches('/');
        // Anchored directory prefix: `src/payments/` matches `src/payments/...`.
        return path == dir || path.starts_with(&format!("{dir}/"));
    }

    // Basename glob: a pattern with no `/` matches the path's basename at any depth (GitHub semantics).
    if !pattern.contains('/') {
        let base = path.rsplit('/').next().unwrap_or(path);
        return wildcard_match(pattern.as_bytes(), base.as_bytes());
    }

    // Anchored path glob: `/docs/**` / `/src/*.rs` — match the whole (root-relative) path.
    let pat = pattern.trim_start_matches('/');
    wildcard_match(pat.as_bytes(), path.as_bytes())
}

/// A minimal `*`/`**` wildcard matcher over bytes (anchored, full match). `**` matches across `/`; a
/// single `*` matches any run of NON-`/` bytes. Iterative backtracking (no regex dependency) — kept
/// small + auditable (the ref-glob + CODEOWNERS scope is the only consumer).
fn wildcard_match(pattern: &[u8], text: &[u8]) -> bool {
    // Detect a `**` anywhere → treat `*` runs as cross-`/` (the doublestar case): we collapse the
    // distinction by checking, at each `*`, whether it is part of a `**`.
    let (mut p, mut t) = (0usize, 0usize);
    let (mut star_p, mut star_t): (Option<usize>, usize) = (None, 0);
    let mut star_crosses_slash = false;

    while t < text.len() {
        if p < pattern.len() && (pattern[p] == text[t] || pattern[p] == b'?') {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            // Is this a `**` (cross-slash) or a single `*` (within-segment)?
            let double = p + 1 < pattern.len() && pattern[p + 1] == b'*';
            star_crosses_slash = double;
            // Consume the star run (`*` or `**`).
            p += if double { 2 } else { 1 };
            star_p = Some(p);
            star_t = t;
        } else if let Some(sp) = star_p {
            // Backtrack: the previous star absorbs one more text byte — unless it is a `/` and the
            // star was a single `*` (which does not cross `/`).
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

    // Consume any trailing star run in the pattern.
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── helpers ──────────────────────────────────────────────────────────────────────────────────
    fn a_pr(draft: bool) -> PullRequest {
        PullRequest::open(42, "refs/heads/main", "refs/heads/feature", "psn:alice", draft)
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

    // ════════ 1. PR LIFECYCLE — 0 ILLEGAL TRANSITIONS ════════

    #[test]
    fn pr_open_to_review_to_merge_is_well_formed() {
        let mut pr = a_pr(true);
        assert_eq!(pr.state, PrState::Draft);
        // draft → ready.
        assert_eq!(pr.transition(PrTransition::MarkReady, false).unwrap(), PrState::Open);
        // open → merged (gate satisfied).
        assert_eq!(pr.transition(PrTransition::Merge, true).unwrap(), PrState::Merged);
        assert_eq!(pr.state, PrState::Merged);
    }

    #[test]
    fn pr_close_then_reopen_then_close_is_well_formed() {
        let mut pr = a_pr(false); // opens Open.
        assert_eq!(pr.transition(PrTransition::Close, false).unwrap(), PrState::Closed);
        assert_eq!(pr.transition(PrTransition::Reopen, false).unwrap(), PrState::Open);
        assert_eq!(pr.transition(PrTransition::Close, false).unwrap(), PrState::Closed);
    }

    #[test]
    fn merged_pr_is_terminal_no_transition_revives_it() {
        let mut pr = a_pr(false);
        pr.transition(PrTransition::Merge, true).unwrap();
        assert!(pr.state.is_terminal());
        // every transition out of Merged is illegal (0 illegal transitions — a merged PR is frozen).
        for t in [
            PrTransition::MarkReady,
            PrTransition::Merge,
            PrTransition::Close,
            PrTransition::Reopen,
        ] {
            assert!(
                matches!(pr.transition(t, true), Err(LifecycleError::IllegalTransition { .. })),
                "{t:?} from Merged must be illegal"
            );
            assert_eq!(pr.state, PrState::Merged, "an illegal transition does NOT mutate state");
        }
    }

    #[test]
    fn illegal_pr_edges_are_rejected_loudly() {
        // MarkReady on an already-Open PR is illegal.
        let mut open = a_pr(false);
        assert!(matches!(
            open.transition(PrTransition::MarkReady, false),
            Err(LifecycleError::IllegalTransition { from: PrState::Open, .. })
        ));
        // Reopen on a Draft (never closed) is illegal.
        let mut draft = a_pr(true);
        assert!(matches!(
            draft.transition(PrTransition::Reopen, false),
            Err(LifecycleError::IllegalTransition { from: PrState::Draft, .. })
        ));
        // Reopen on an Open PR is illegal.
        let mut open2 = a_pr(false);
        assert!(matches!(
            open2.transition(PrTransition::Reopen, false),
            Err(LifecycleError::IllegalTransition { .. })
        ));
    }

    #[test]
    fn merge_refuses_an_unsatisfied_gate_zero_unprotected_merges() {
        let mut pr = a_pr(false);
        // a Merge from a legal source (Open) STILL refuses when the gate is not satisfied.
        assert_eq!(
            pr.transition(PrTransition::Merge, /*gate_satisfied*/ false),
            Err(LifecycleError::MergeGateNotSatisfied)
        );
        assert_eq!(pr.state, PrState::Open, "a refused merge does NOT land (0 unprotected merges)");
    }

    // ════════ 2. REVIEW LIFECYCLE ════════

    #[test]
    fn review_request_submit_dismiss_is_well_formed() {
        let mut r = Review::request("psn:bob", false);
        assert_eq!(r.state, ReviewState::Requested);
        assert_eq!(
            r.submit(ReviewVerdict::Approve).unwrap(),
            ReviewState::Submitted(ReviewVerdict::Approve)
        );
        assert!(r.is_current_approval());
        // revise the verdict (Submitted → Submitted) is legal.
        r.submit(ReviewVerdict::RequestChanges).unwrap();
        assert!(r.is_blocking() && !r.is_current_approval());
        // dismiss the submitted review.
        assert_eq!(r.dismiss().unwrap(), ReviewState::Dismissed);
        assert!(!r.is_current_approval() && !r.is_blocking());
    }

    #[test]
    fn illegal_review_transitions_are_rejected() {
        // dismiss a not-yet-submitted (Requested) review is illegal.
        let mut r = Review::request("psn:bob", false);
        assert!(matches!(
            r.dismiss(),
            Err(LifecycleError::IllegalReviewTransition { from: ReviewState::Requested })
        ));
        // submit on a dismissed review is illegal.
        r.submit(ReviewVerdict::Approve).unwrap();
        r.dismiss().unwrap();
        assert!(matches!(
            r.submit(ReviewVerdict::Approve),
            Err(LifecycleError::IllegalReviewTransition { from: ReviewState::Dismissed })
        ));
        // dismiss an already-dismissed review is illegal.
        assert!(matches!(r.dismiss(), Err(LifecycleError::IllegalReviewTransition { .. })));
    }

    #[test]
    fn agent_reviewer_is_legible() {
        let r = Review::request("psn:agent-x", true);
        assert!(r.is_agent, "an agent reviewer carries is_agent (ADR-08 legibility — never disguised)");
    }

    // ════════ 3. THREAD LIFECYCLE ════════

    fn a_comment(id: u128) -> Comment {
        Comment { id, author_pseudonym: "psn:alice".into(), body: Body::empty(), is_agent: false }
    }

    #[test]
    fn thread_open_reply_resolve_reopen_is_well_formed() {
        let anchor = DiffAnchor { path: "src/lib.rs".into(), start_line: 10, end_line: 12 };
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
        // reopen an OPEN thread is illegal.
        assert!(matches!(
            t.reopen(),
            Err(LifecycleError::IllegalThreadTransition { from: ThreadState::Open })
        ));
        // resolve, then a second resolve is illegal.
        t.resolve().unwrap();
        assert!(matches!(
            t.resolve(),
            Err(LifecycleError::IllegalThreadTransition { from: ThreadState::Resolved })
        ));
    }

    // ════════ 4. BRANCH-PROTECTION RULESET — 0 UNPROTECTED MERGES ════════

    #[test]
    fn ruleset_satisfied_when_all_conditions_met() {
        let outcome = evaluate_ruleset(&strict_ruleset(), &satisfied_ctx());
        assert_eq!(outcome, RulesetOutcome::Satisfied);
        assert!(outcome.is_satisfied());
    }

    #[test]
    fn ruleset_blocks_each_unmet_condition_distinctly() {
        let rs = strict_ruleset();

        // missing required context.
        let mut ctx = satisfied_ctx();
        ctx.green_contexts = vec!["ci/build".into()]; // ci/test missing.
        let o = evaluate_ruleset(&rs, &ctx);
        assert!(matches!(&o, RulesetOutcome::Blocked { reasons }
            if reasons.contains(&BlockReason::MissingRequiredContext("ci/test".into()))));

        // insufficient approvals.
        let mut ctx = satisfied_ctx();
        ctx.current_approvals = 1;
        let o = evaluate_ruleset(&rs, &ctx);
        assert!(matches!(&o, RulesetOutcome::Blocked { reasons }
            if reasons.contains(&BlockReason::InsufficientApprovals { have: 1, need: 2 })));

        // codeowner review missing.
        let mut ctx = satisfied_ctx();
        ctx.codeowner_review_satisfied = false;
        let o = evaluate_ruleset(&rs, &ctx);
        assert!(matches!(&o, RulesetOutcome::Blocked { reasons }
            if reasons.contains(&BlockReason::CodeownerReviewMissing)));

        // a blocking request-changes review.
        let mut ctx = satisfied_ctx();
        ctx.has_blocking_review = true;
        let o = evaluate_ruleset(&rs, &ctx);
        assert!(matches!(&o, RulesetOutcome::Blocked { reasons }
            if reasons.contains(&BlockReason::BlockingReview)));

        // outstanding conversations.
        let mut ctx = satisfied_ctx();
        ctx.outstanding_conversations = 3;
        let o = evaluate_ruleset(&rs, &ctx);
        assert!(matches!(&o, RulesetOutcome::Blocked { reasons }
            if reasons.contains(&BlockReason::OutstandingConversations(3))));
    }

    #[test]
    fn protected_ref_admits_zero_unprotected_merges_end_to_end() {
        // a protected base_ref + an UNMET ruleset → the lifecycle Merge transition is refused.
        let rs = strict_ruleset();
        let mut pr = a_pr(false);
        assert!(rs.matches(&pr.base_ref), "the ruleset protects refs/heads/main");

        let mut ctx = satisfied_ctx();
        ctx.current_approvals = 0; // a fresh PR with 0 approvals.
        let gate = evaluate_ruleset(&rs, &ctx);
        assert!(!gate.is_satisfied());

        // the entity layer refuses the merge (0 unprotected merges to the protected ref).
        assert_eq!(
            pr.transition(PrTransition::Merge, gate.is_satisfied()),
            Err(LifecycleError::MergeGateNotSatisfied)
        );
        assert_eq!(pr.state, PrState::Open);

        // now satisfy the gate → the merge lands.
        let gate = evaluate_ruleset(&rs, &satisfied_ctx());
        assert!(gate.is_satisfied());
        assert_eq!(pr.transition(PrTransition::Merge, gate.is_satisfied()).unwrap(), PrState::Merged);
    }

    #[test]
    fn unprotected_ref_has_no_ruleset_match() {
        let rs = strict_ruleset(); // protects refs/heads/main.
        assert!(!rs.matches("refs/heads/scratch"), "an unprotected ref is not gated by this ruleset");
    }

    #[test]
    fn pr_state_terminality_is_exact() {
        // ONLY Merged is terminal; the other three are NOT (kills `is_terminal -> true`).
        assert!(PrState::Merged.is_terminal());
        assert!(!PrState::Draft.is_terminal());
        assert!(!PrState::Open.is_terminal());
        assert!(!PrState::Closed.is_terminal(), "Closed reopens — it is NOT terminal");
    }

    #[test]
    fn wildcard_matcher_exercises_backtracking_and_trailing_stars() {
        // Direct exercise of the glob matcher internals (kills the `wildcard_match` arithmetic /
        // bound mutants: the star-consume `p += 1/2`, the backtrack `star_t += 1`, the trailing-star
        // `p += 1`, and the `p < len` bound).
        // A trailing `*` must consume the rest of a segment (the trailing-star loop, `p += 1`).
        assert!(glob_match("refs/heads/feat*", "refs/heads/feature"));
        assert!(glob_match("refs/heads/*", "refs/heads/main"));
        // A trailing `*` with nothing left to match still completes (exact-at-star).
        assert!(glob_match("refs/heads/main*", "refs/heads/main"));
        // Multi-star backtracking within a segment (the `*` absorbs more bytes on mismatch).
        assert!(glob_match("a*c*e", "abcde"));
        assert!(glob_match("a*c*e", "axxcyye"));
        assert!(!glob_match("a*c*e", "abcdf"), "no trailing e → no match");
        // A single `*` must NOT cross `/` (the backtrack `/` guard) even mid-pattern.
        assert!(!glob_match("refs/*", "refs/heads/main"), "single * stops at /");
        // `**` crosses `/` (the double-star branch, `p += 2`).
        assert!(glob_match("refs/**", "refs/heads/main"));
        // A `*` that should NOT match (text longer, pattern exhausted with a literal mismatch).
        assert!(!glob_match("refs/heads/main", "refs/heads/mainline"));
    }

    #[test]
    fn codeowners_basename_and_anchored_matching() {
        // a leading `/` anchors at root; a bare glob matches the basename at any depth; `**` under an
        // anchored dir crosses `/`. These exercise codeowners_path_match's three branches + the
        // wildcard internals from the CODEOWNERS side.
        let co = CodeOwners::parse(
            "/build/    @a\n*.lock      @b\n/deep/**    @c\n",
        )
        .unwrap();
        assert_eq!(co.owners_for("build/out.o"), &["@a".to_string()], "anchored dir prefix");
        assert_eq!(co.owners_for("a/b/Cargo.lock"), &["@b".to_string()], "basename glob at depth");
        assert_eq!(co.owners_for("deep/a/b/c.txt"), &["@c".to_string()], "anchored ** crosses /");
        assert!(co.owners_for("Cargo.toml").is_empty(), "no rule matches → unowned");
    }

    #[test]
    fn ref_pattern_glob_matches() {
        let rs = BranchProtectionRuleset {
            ref_pattern: "refs/heads/release/*".into(),
            ..strict_ruleset()
        };
        assert!(rs.matches("refs/heads/release/1.0"));
        assert!(!rs.matches("refs/heads/release/1.0/hotfix"), "single * does not cross /");
        let rs2 = BranchProtectionRuleset {
            ref_pattern: "refs/heads/release/**".into(),
            ..strict_ruleset()
        };
        assert!(rs2.matches("refs/heads/release/1.0/hotfix"), "** crosses /");
    }

    // ════════ 5. CODEOWNERS RESOLVER — 0 MIS-RESOLVED OWNERS ════════

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
        // default catch-all.
        assert_eq!(co.owners_for("README.adoc"), &["@acme/core-team".to_string()]);
        // *.ts overrides the catch-all (later rule wins).
        assert_eq!(co.owners_for("web/app.ts"), &["@acme/frontend".to_string()]);
        // the payments dir overrides BOTH the catch-all and *.ts for a .ts under it (last match wins:
        // /src/payments/ is later in the file than *.ts).
        assert_eq!(
            co.owners_for("src/payments/charge.ts"),
            &["@acme/payments".to_string(), "@alice".to_string()]
        );
        // a non-.ts file in payments → payments.
        assert_eq!(
            co.owners_for("src/payments/charge.rs"),
            &["@acme/payments".to_string(), "@alice".to_string()]
        );
        // docs glob.
        assert_eq!(co.owners_for("docs/guide/intro.md"), &["@acme/writers".to_string()]);
        // a .rs file not under payments → only the catch-all (NOT *.ts, NOT payments).
        assert_eq!(co.owners_for("src/core/lib.rs"), &["@acme/core-team".to_string()]);
    }

    #[test]
    fn codeowners_rejects_malformed_lines_loudly() {
        // a pattern with no owners.
        assert_eq!(CodeOwners::parse("*.rs\n"), Err(CodeOwnersError::NoOwners(1)));
        // a non-@ owner token.
        assert!(matches!(
            CodeOwners::parse("*.rs alice@example.com\n"),
            Err(CodeOwnersError::MalformedOwner(_))
        ));
    }

    #[test]
    fn codeowners_resolves_to_code_owner_relation_tuples_4_9() {
        let co = CodeOwners::parse(FIXTURE).unwrap();
        let deltas = co.resolve(/*repo_id*/ 7);
        // one tuple per (rule, owner): 1 + 1 + 2 + 1 = 5.
        assert_eq!(deltas.len(), 5);
        // every delta is an Add of a `code_owner` relation on a ref:<repo>::<pattern> object.
        for d in &deltas {
            match d {
                TupleDelta::Add(t) => {
                    assert_eq!(t.relation.0, crate::live_check::perm::CODE_OWNER);
                    assert!(t.object.0.starts_with("ref:7::"), "ref-pattern-scoped object");
                    assert!(t.subject.0.starts_with('@'), "owner handle subject");
                    assert!(t.caveat.is_none());
                }
                TupleDelta::Remove(_) => panic!("resolve emits only Add deltas"),
            }
        }
        // the payments rule produced TWO tuples (payments + alice) on the same ref object.
        let payments: Vec<&str> = deltas
            .iter()
            .filter_map(|d| match d {
                TupleDelta::Add(t) if t.object.0 == "ref:7::/src/payments/" => Some(t.subject.0.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(payments, vec!["@acme/payments", "@alice"]);
    }
}
